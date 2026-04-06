use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sonic_core::{
    BasicFacts, BgpFacts, CommandResult, ConfigFacts, Connection, Device,
    DeviceInfo, FactsProvider, InterfaceFacts, PortInfo, PortStatus, RebootType, SonicError,
    Result,
};
use tokio::sync::Mutex;
use tracing::{info, instrument};

use crate::connection::ssh::SshConnection;
use crate::facts::cache::FactsCache;

// -----------------------------------------------------------------------
// FanoutHost
// -----------------------------------------------------------------------

/// A fanout switch that connects the DUT to the PTF container.  Fanout
/// switches can be Arista, Dell, or SONiC-based.  This abstraction exposes
/// per-port operations needed by the test harness (shutdown / unshut, speed
/// changes, etc.).
pub struct FanoutHost {
    info: DeviceInfo,
    conn: Arc<Mutex<Option<Box<dyn Connection>>>>,
    facts_cache: FactsCache,
}

impl FanoutHost {
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

    // -- fanout-specific methods -----------------------------------------

    /// Shutdown one or more ports.
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn shutdown_ports(&self, ports: &[&str]) -> Result<Vec<CommandResult>> {
        let mut results = Vec::with_capacity(ports.len());
        for port in ports {
            let cmd = format!(
                "configure terminal\ninterface {}\nshutdown\nend",
                port
            );
            results.push(self.execute(&cmd).await?);
        }
        Ok(results)
    }

    /// Bring ports back up.
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn no_shutdown_ports(&self, ports: &[&str]) -> Result<Vec<CommandResult>> {
        let mut results = Vec::with_capacity(ports.len());
        for port in ports {
            let cmd = format!(
                "configure terminal\ninterface {}\nno shutdown\nend",
                port
            );
            results.push(self.execute(&cmd).await?);
        }
        Ok(results)
    }

    /// Set the speed on a port (e.g. `"100gfull"`).
    #[instrument(skip(self), fields(host = %self.info.hostname, port = %port, speed = %speed))]
    pub async fn set_speed(&self, port: &str, speed: &str) -> Result<CommandResult> {
        let cmd = format!(
            "configure terminal\ninterface {}\nspeed {}\nend",
            port, speed
        );
        self.execute(&cmd).await
    }

    /// Query the operational status of a specific port.
    #[instrument(skip(self), fields(host = %self.info.hostname, port = %port))]
    pub async fn get_port_status(&self, port: &str) -> Result<PortStatus> {
        let cmd = format!("show interfaces {} status", port);
        let result = self.execute_checked(&cmd).await?;
        let lower = result.stdout.to_lowercase();
        if lower.contains("connected") || lower.contains(" up ") {
            Ok(PortStatus::Up)
        } else if lower.contains("notconnect") || lower.contains(" down ") {
            Ok(PortStatus::Down)
        } else {
            Ok(PortStatus::NotPresent)
        }
    }
}

// -----------------------------------------------------------------------
// `Device` trait
// -----------------------------------------------------------------------

#[async_trait]
impl Device for FanoutHost {
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
        info!("connected to fanout host {}", self.info.hostname);
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
        info!("{}: rebooting fanout", self.info.hostname);
        let guard = self.conn().await?;
        guard.as_ref().unwrap().send("reload now").await.ok();
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
impl FactsProvider for FanoutHost {
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn basic_facts(&self) -> Result<BasicFacts> {
        if let Some(cached) = self.facts_cache.get::<BasicFacts>("basic").await {
            return Ok(cached);
        }

        let r = self.execute_checked("show version").await?;
        let mut facts = BasicFacts::default();
        facts.hostname = self.info.hostname.clone();

        for line in r.stdout.lines() {
            let line = line.trim();
            if line.starts_with("Software image version:") || line.contains("System version") {
                facts.os_version = line.split(':').nth(1).unwrap_or("").trim().to_string();
            }
            if line.starts_with("Serial number:") {
                facts.serial_number = line.split(':').nth(1).unwrap_or("").trim().to_string();
            }
        }
        facts.platform = "Fanout".to_string();

        self.facts_cache.set("basic", &facts).await;
        Ok(facts)
    }

    async fn bgp_facts(&self) -> Result<BgpFacts> {
        // Most fanout switches do not run BGP.
        Ok(BgpFacts::default())
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn interface_facts(&self) -> Result<InterfaceFacts> {
        if let Some(cached) = self.facts_cache.get::<InterfaceFacts>("interfaces").await {
            return Ok(cached);
        }

        let r = self.execute_checked("show interfaces status").await?;
        let mut ports = Vec::new();
        let mut in_table = false;

        for line in r.stdout.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Port") || trimmed.starts_with("Interface") {
                in_table = true;
                continue;
            }
            if trimmed.starts_with("---") {
                continue;
            }
            if in_table && !trimmed.is_empty() {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if !parts.is_empty() {
                    let name = parts[0].to_string();
                    let oper = if parts.iter().any(|p| {
                        p.eq_ignore_ascii_case("connected") || p.eq_ignore_ascii_case("up")
                    }) {
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

    async fn config_facts(&self) -> Result<ConfigFacts> {
        let r = self.execute("show running-config").await?;
        let mut running = HashMap::new();
        running.insert(
            "running-config".to_string(),
            serde_json::Value::String(r.stdout),
        );
        Ok(ConfigFacts {
            running_config: running,
            startup_config: HashMap::new(),
            features: HashMap::new(),
            services: Vec::new(),
        })
    }
}
