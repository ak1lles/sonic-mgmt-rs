use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sonic_core::{
    BasicFacts, BgpFacts, BgpNeighbor, BgpState, CommandResult, ConfigFacts, Connection,
    Device, DeviceInfo, FactsProvider, InterfaceFacts, PortInfo, PortStatus,
    RebootType, SonicError, Result,
};
use tokio::sync::Mutex;
use tracing::{info, instrument};

use crate::connection::ssh::SshConnection;
use crate::facts::cache::FactsCache;

// -----------------------------------------------------------------------
// EosHost
// -----------------------------------------------------------------------

/// Arista EOS neighbor host.
pub struct EosHost {
    info: DeviceInfo,
    conn: Arc<Mutex<Option<Box<dyn Connection>>>>,
    facts_cache: FactsCache,
}

impl EosHost {
    pub fn new(info: DeviceInfo) -> Self {
        Self {
            info,
            conn: Arc::new(Mutex::new(None)),
            facts_cache: FactsCache::new(Duration::from_secs(300)),
        }
    }

    // -- helpers ---------------------------------------------------------

    async fn conn(&self) -> Result<tokio::sync::MutexGuard<'_, Option<Box<dyn Connection>>>> {
        let guard = self.conn.lock().await;
        if guard.is_none() {
            return Err(SonicError::connection(&self.info.hostname, "not connected"));
        }
        Ok(guard)
    }

    /// Wrap a command with EOS `enable` context, which is needed for most
    /// show commands on vEOS / cEOS.
    fn enable_wrap(cmd: &str) -> String {
        format!("enable\n{}", cmd)
    }

    // -- EOS-specific methods --------------------------------------------

    /// Enter configuration mode, run `commands` (one per line), then exit.
    #[instrument(skip(self, commands), fields(host = %self.info.hostname))]
    pub async fn configure(&self, commands: &[&str]) -> Result<CommandResult> {
        let mut script = String::from("enable\nconfigure terminal\n");
        for c in commands {
            script.push_str(c);
            script.push('\n');
        }
        script.push_str("end\n");
        self.execute(&script).await
    }

    /// `show running-config`
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn show_running(&self) -> Result<String> {
        let r = self
            .execute_checked(&Self::enable_wrap("show running-config"))
            .await?;
        Ok(r.stdout)
    }

    /// `show ip bgp summary`
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn show_bgp(&self) -> Result<String> {
        let r = self
            .execute_checked(&Self::enable_wrap("show ip bgp summary"))
            .await?;
        Ok(r.stdout)
    }

    /// Shutdown an interface.
    #[instrument(skip(self), fields(host = %self.info.hostname, interface = %interface))]
    pub async fn shutdown_interface(&self, interface: &str) -> Result<CommandResult> {
        self.configure(&[
            &format!("interface {}", interface),
            "shutdown",
        ])
        .await
    }

    /// Bring an interface back up.
    #[instrument(skip(self), fields(host = %self.info.hostname, interface = %interface))]
    pub async fn no_shutdown_interface(&self, interface: &str) -> Result<CommandResult> {
        self.configure(&[
            &format!("interface {}", interface),
            "no shutdown",
        ])
        .await
    }
}

// -----------------------------------------------------------------------
// `Device` trait
// -----------------------------------------------------------------------

#[async_trait]
impl Device for EosHost {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn connect(&mut self) -> Result<()> {
        let mut connection: Box<dyn Connection> = Box::new(SshConnection::new(
            self.info.mgmt_ip.to_string(),
            self.info.port,
            self.info.credentials.clone(),
        ));
        connection.open().await?;

        let mut guard = self.conn.lock().await;
        *guard = Some(connection);
        info!("connected to EOS host {}", self.info.hostname);
        Ok(())
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn disconnect(&mut self) -> Result<()> {
        let mut guard = self.conn.lock().await;
        if let Some(mut conn) = guard.take() {
            conn.close().await?;
        }
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        let guard = self.conn.lock().await;
        match guard.as_ref() {
            Some(conn) => conn.is_alive().await,
            None => false,
        }
    }

    async fn execute(&self, command: &str) -> Result<CommandResult> {
        let guard = self.conn().await?;
        guard.as_ref().unwrap().send_command(command).await
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn reboot(&self, _reboot_type: RebootType) -> Result<()> {
        let guard = self.conn().await?;
        guard.as_ref().unwrap().send("enable\nreload now").await.ok();
        Ok(())
    }

    #[instrument(skip(self), fields(host = %self.info.hostname, timeout_secs))]
    async fn wait_ready(&self, timeout_secs: u64) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
        let poll = Duration::from_secs(10);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(SonicError::Timeout {
                    seconds: timeout_secs,
                    operation: format!("{}: wait_ready", self.info.hostname),
                });
            }
            match self.execute(&Self::enable_wrap("show version")).await {
                Ok(r) if r.exit_code == 0 => return Ok(()),
                _ => tokio::time::sleep(poll).await,
            }
        }
    }
}

// -----------------------------------------------------------------------
// `FactsProvider` trait
// -----------------------------------------------------------------------

#[async_trait]
impl FactsProvider for EosHost {
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn basic_facts(&self) -> Result<BasicFacts> {
        if let Some(cached) = self.facts_cache.get::<BasicFacts>("basic").await {
            return Ok(cached);
        }

        let r = self
            .execute_checked(&Self::enable_wrap("show version"))
            .await?;
        let output = &r.stdout;

        let mut facts = BasicFacts::default();
        for line in output.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("Arista") {
                // e.g. "Arista vEOS-lab"
                facts.model = val.trim().to_string();
            }
            if line.starts_with("Software image version:") {
                facts.os_version = line
                    .split(':')
                    .nth(1)
                    .unwrap_or("")
                    .trim()
                    .to_string();
            }
            if line.starts_with("Hardware version:") {
                facts.hwsku = line.split(':').nth(1).unwrap_or("").trim().to_string();
            }
            if line.starts_with("Serial number:") {
                facts.serial_number =
                    line.split(':').nth(1).unwrap_or("").trim().to_string();
            }
            if let Some(rest) = line.strip_prefix("Uptime:") {
                // Very rough: parse "X days, H:M:S" into seconds.
                facts.uptime = parse_eos_uptime(rest.trim());
            }
        }
        facts.hostname = self.info.hostname.clone();
        facts.platform = "Arista".to_string();

        self.facts_cache.set("basic", &facts).await;
        Ok(facts)
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn bgp_facts(&self) -> Result<BgpFacts> {
        if let Some(cached) = self.facts_cache.get::<BgpFacts>("bgp").await {
            return Ok(cached);
        }

        let r = self
            .execute_checked(&Self::enable_wrap("show ip bgp summary"))
            .await?;

        let mut facts = BgpFacts::default();
        let mut in_table = false;

        for line in r.stdout.lines() {
            let line = line.trim();
            if line.starts_with("BGP router identifier") {
                // "BGP router identifier 10.0.0.1, local AS number 65000"
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(id) = parts.get(3) {
                    facts.router_id = id.trim_end_matches(',').to_string();
                }
                if let Some(asn) = parts.last() {
                    facts.local_as = asn.parse().unwrap_or(0);
                }
            }
            if line.starts_with("Neighbor") {
                in_table = true;
                continue;
            }
            if in_table && !line.is_empty() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 9 {
                    let addr = parts[0].parse().ok();
                    let remote_as = parts[2].parse().unwrap_or(0);
                    let state_or_pfx = parts[parts.len() - 1];
                    let state = if state_or_pfx.parse::<u64>().is_ok() {
                        BgpState::Established
                    } else {
                        match state_or_pfx.to_lowercase().as_str() {
                            "idle" => BgpState::Idle,
                            "connect" => BgpState::Connect,
                            "active" => BgpState::Active,
                            "opensent" => BgpState::OpenSent,
                            "openconfirm" => BgpState::OpenConfirm,
                            _ => BgpState::Idle,
                        }
                    };
                    let prefixes_received = state_or_pfx.parse().unwrap_or(0);

                    if let Some(address) = addr {
                        facts.neighbors.push(BgpNeighbor {
                            address,
                            remote_as,
                            local_as: facts.local_as,
                            state,
                            description: None,
                            hold_time: 180,
                            keepalive: 60,
                            prefixes_received,
                            prefixes_sent: 0,
                            up_since: None,
                        });
                    }
                }
            }
        }

        self.facts_cache.set("bgp", &facts).await;
        Ok(facts)
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn interface_facts(&self) -> Result<InterfaceFacts> {
        if let Some(cached) = self.facts_cache.get::<InterfaceFacts>("interfaces").await {
            return Ok(cached);
        }

        let r = self
            .execute_checked(&Self::enable_wrap("show interfaces status"))
            .await?;

        let mut ports = Vec::new();
        let mut in_table = false;
        for line in r.stdout.lines() {
            let line = line.trim();
            if line.starts_with("Port") {
                in_table = true;
                continue;
            }
            if line.starts_with("---") {
                continue;
            }
            if in_table && !line.is_empty() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                    let name = parts[0].to_string();
                    let status = match parts[2].to_lowercase().as_str() {
                        "connected" | "up" => PortStatus::Up,
                        "notconnect" | "down" => PortStatus::Down,
                        _ => PortStatus::Down,
                    };
                    let speed_str = parts.last().unwrap_or(&"0");
                    let speed = parse_speed_string(speed_str);

                    ports.push(PortInfo {
                        name,
                        alias: None,
                        index: 0,
                        speed,
                        lanes: Vec::new(),
                        mtu: 9214,
                        admin_status: status,
                        oper_status: status,
                        fec: None,
                        autoneg: None,
                    });
                }
            }
        }

        let facts = InterfaceFacts {
            ports,
            vlans: Vec::new(),
            lags: Vec::new(),
            loopbacks: Vec::new(),
        };
        self.facts_cache.set("interfaces", &facts).await;
        Ok(facts)
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn config_facts(&self) -> Result<ConfigFacts> {
        if let Some(cached) = self.facts_cache.get::<ConfigFacts>("config").await {
            return Ok(cached);
        }

        let r = self
            .execute_checked(&Self::enable_wrap("show running-config"))
            .await?;

        let mut running = HashMap::new();
        running.insert(
            "running-config".to_string(),
            serde_json::Value::String(r.stdout.clone()),
        );

        let facts = ConfigFacts {
            running_config: running,
            startup_config: HashMap::new(),
            features: HashMap::new(),
            services: Vec::new(),
        };
        self.facts_cache.set("config", &facts).await;
        Ok(facts)
    }
}

// -----------------------------------------------------------------------
// Utility parsers
// -----------------------------------------------------------------------

/// Rough parse of an EOS uptime string like "1 day, 3:24:56".
fn parse_eos_uptime(s: &str) -> u64 {
    let mut total_secs: u64 = 0;

    // Days
    if let Some(day_part) = s.split("day").next() {
        if let Ok(days) = day_part.trim().trim_end_matches(',').parse::<u64>() {
            total_secs += days * 86400;
        }
    }

    // H:M:S -- take the last colon-separated token group.
    for token in s.split_whitespace() {
        if token.contains(':') {
            let hms: Vec<&str> = token.split(':').collect();
            if hms.len() == 3 {
                let h: u64 = hms[0].parse().unwrap_or(0);
                let m: u64 = hms[1].parse().unwrap_or(0);
                let sec: u64 = hms[2].parse().unwrap_or(0);
                total_secs += h * 3600 + m * 60 + sec;
            }
        }
    }
    total_secs
}

/// Parse speed strings like "100G", "25G", "10G", "1G", "100M" into bits/sec.
fn parse_speed_string(s: &str) -> u64 {
    let s = s.trim().to_uppercase();
    if s.ends_with('G') || s.ends_with("GB") {
        let num: u64 = s.trim_end_matches("GB").trim_end_matches('G').parse().unwrap_or(0);
        num * 1_000_000_000
    } else if s.ends_with('M') || s.ends_with("MB") {
        let num: u64 = s.trim_end_matches("MB").trim_end_matches('M').parse().unwrap_or(0);
        num * 1_000_000
    } else {
        s.parse().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- enable_wrap --------------------------------------------------------

    #[test]
    fn enable_wrap_prepends_enable() {
        assert_eq!(EosHost::enable_wrap("show version"), "enable\nshow version");
    }

    // -- parse_eos_uptime ---------------------------------------------------

    #[test]
    fn uptime_days_and_hms() {
        assert_eq!(parse_eos_uptime("1 day, 2:03:04"), 86400 + 7384);
    }

    #[test]
    fn uptime_hms_only() {
        assert_eq!(parse_eos_uptime("0:30:00"), 1800);
    }

    #[test]
    fn uptime_days_only() {
        assert_eq!(parse_eos_uptime("5 days"), 5 * 86400);
    }

    #[test]
    fn uptime_empty() {
        assert_eq!(parse_eos_uptime(""), 0);
    }

    // -- parse_speed_string -------------------------------------------------

    #[test]
    fn speed_100g() {
        assert_eq!(parse_speed_string("100G"), 100_000_000_000);
    }

    #[test]
    fn speed_25g_lowercase() {
        assert_eq!(parse_speed_string("25g"), 25_000_000_000);
    }

    #[test]
    fn speed_1gb() {
        assert_eq!(parse_speed_string("1GB"), 1_000_000_000);
    }

    #[test]
    fn speed_100m() {
        assert_eq!(parse_speed_string("100M"), 100_000_000);
    }

    #[test]
    fn speed_100mb() {
        assert_eq!(parse_speed_string("100MB"), 100_000_000);
    }

    #[test]
    fn speed_raw_number() {
        assert_eq!(parse_speed_string("1000000"), 1_000_000);
    }

    #[test]
    fn speed_garbage() {
        assert_eq!(parse_speed_string("notaspeed"), 0);
    }
}
