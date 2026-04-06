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
// CiscoHost
// -----------------------------------------------------------------------

/// Cisco IOS / NX-OS device host.
pub struct CiscoHost {
    info: DeviceInfo,
    conn: Arc<Mutex<Option<Box<dyn Connection>>>>,
    facts_cache: FactsCache,
}

impl CiscoHost {
    /// Creates a new Cisco host driver with a 5-minute facts cache TTL.
    pub fn new(info: DeviceInfo) -> Self {
        Self {
            info,
            conn: Arc::new(Mutex::new(None)),
            facts_cache: FactsCache::new(Duration::from_secs(300)),
        }
    }

    async fn conn(&self) -> Result<tokio::sync::MutexGuard<'_, Option<Box<dyn Connection>>>> {
        let guard = self.conn.lock().await;
        if guard.is_none() {
            return Err(SonicError::connection(&self.info.hostname, "not connected"));
        }
        Ok(guard)
    }

    // -- Cisco-specific methods ------------------------------------------

    /// Enter config terminal, run commands, then exit.
    #[instrument(skip(self, commands), fields(host = %self.info.hostname))]
    pub async fn configure_terminal(&self, commands: &[&str]) -> Result<CommandResult> {
        let mut script = String::from("configure terminal\n");
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
        let r = self.execute_checked("show running-config").await?;
        Ok(r.stdout)
    }

    /// `show ip bgp summary`
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn show_bgp(&self) -> Result<String> {
        let r = self.execute_checked("show ip bgp summary").await?;
        Ok(r.stdout)
    }
}

// -----------------------------------------------------------------------
// `Device` trait
// -----------------------------------------------------------------------

#[async_trait]
impl Device for CiscoHost {
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
        info!("connected to Cisco host {}", self.info.hostname);
        Ok(())
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn disconnect(&mut self) -> Result<()> {
        let mut guard = self.conn.lock().await;
        if let Some(mut c) = guard.take() {
            c.close().await?;
        }
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        let guard = self.conn.lock().await;
        match guard.as_ref() {
            Some(c) => c.is_alive().await,
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
        guard.as_ref().unwrap().send("reload\n\n").await.ok();
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
            match self.execute("show version").await {
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
impl FactsProvider for CiscoHost {
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn basic_facts(&self) -> Result<BasicFacts> {
        if let Some(cached) = self.facts_cache.get::<BasicFacts>("basic").await {
            return Ok(cached);
        }

        let r = self.execute_checked("show version").await?;
        let output = &r.stdout;
        let mut facts = BasicFacts::default();
        facts.hostname = self.info.hostname.clone();
        facts.platform = "Cisco".to_string();

        for line in output.lines() {
            let line = line.trim();
            // Cisco IOS: "Cisco IOS Software, ... Version 15.4(3)M"
            // NX-OS:     "  NXOS: version 9.3(8)"
            if line.contains("Version") || line.contains("version") {
                facts.os_version = line.to_string();
            }
            if line.starts_with("Processor board ID") || line.starts_with("  Serial number:") {
                facts.serial_number = line
                    .split_whitespace()
                    .last()
                    .unwrap_or("")
                    .to_string();
            }
            // "cisco Nexus9000 C9332C" or "cisco WS-C3750G-24T"
            if line.to_lowercase().starts_with("cisco") && facts.model.is_empty() {
                facts.model = line.to_string();
            }
            if line.contains("uptime is") {
                facts.uptime = parse_cisco_uptime(line);
            }
        }

        self.facts_cache.set("basic", &facts).await;
        Ok(facts)
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn bgp_facts(&self) -> Result<BgpFacts> {
        if let Some(cached) = self.facts_cache.get::<BgpFacts>("bgp").await {
            return Ok(cached);
        }

        let r = self.execute_checked("show ip bgp summary").await?;
        let mut facts = BgpFacts::default();
        let mut in_table = false;

        for line in r.stdout.lines() {
            let line = line.trim();
            if line.starts_with("BGP router identifier") {
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
                    if let Ok(address) = parts[0].parse() {
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

        let r = self.execute_checked("show ip interface brief").await?;
        let mut ports = Vec::new();
        let mut in_table = false;

        for line in r.stdout.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Interface") {
                in_table = true;
                continue;
            }
            if trimmed.starts_with("---") {
                continue;
            }
            if in_table && !trimmed.is_empty() {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 3 {
                    let name = parts[0].to_string();
                    let status_str = parts.last().unwrap_or(&"down").to_lowercase();
                    let oper = if status_str == "up" {
                        PortStatus::Up
                    } else {
                        PortStatus::Down
                    };
                    ports.push(PortInfo {
                        name,
                        alias: None,
                        index: 0,
                        speed: 0,
                        lanes: Vec::new(),
                        mtu: 9100,
                        admin_status: oper,
                        oper_status: oper,
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

        let r = self.execute_checked("show running-config").await?;
        let mut running = HashMap::new();
        running.insert(
            "running-config".to_string(),
            serde_json::Value::String(r.stdout),
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
// Utilities
// -----------------------------------------------------------------------

/// Parse a Cisco uptime string like "3 days 2 hours 15 minutes" into seconds.
fn parse_cisco_uptime(s: &str) -> u64 {
    let mut total = 0u64;
    let parts: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0;
    while i < parts.len() {
        if let Ok(n) = parts[i].parse::<u64>() {
            if let Some(unit) = parts.get(i + 1) {
                let u = unit.to_lowercase();
                if u.starts_with("year") {
                    total += n * 365 * 86400;
                } else if u.starts_with("week") {
                    total += n * 7 * 86400;
                } else if u.starts_with("day") {
                    total += n * 86400;
                } else if u.starts_with("hour") {
                    total += n * 3600;
                } else if u.starts_with("minute") || u.starts_with("min") {
                    total += n * 60;
                } else if u.starts_with("second") || u.starts_with("sec") {
                    total += n;
                }
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_full() {
        assert_eq!(
            parse_cisco_uptime("uptime is 1 year, 2 weeks, 3 days, 4 hours, 5 minutes"),
            365 * 86400 + 2 * 7 * 86400 + 3 * 86400 + 4 * 3600 + 5 * 60
        );
    }

    #[test]
    fn uptime_days_hours() {
        assert_eq!(
            parse_cisco_uptime("uptime is 10 days 12 hours"),
            10 * 86400 + 12 * 3600
        );
    }

    #[test]
    fn uptime_minutes_seconds() {
        assert_eq!(
            parse_cisco_uptime("uptime is 30 minutes 45 seconds"),
            30 * 60 + 45
        );
    }

    #[test]
    fn uptime_empty() {
        assert_eq!(parse_cisco_uptime(""), 0);
    }

    #[test]
    fn uptime_no_numbers() {
        assert_eq!(parse_cisco_uptime("uptime is unknown"), 0);
    }
}
