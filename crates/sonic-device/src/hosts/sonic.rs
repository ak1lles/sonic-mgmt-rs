use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sonic_core::{
    BasicFacts, BgpFacts, CommandResult, ConfigFacts, ConfigReloadType, Connection,
    ConnectionType, Device, DeviceInfo, FactsProvider, InterfaceFacts, RebootType,
    SonicError, Result,
};
use tokio::sync::Mutex;
use tracing::{debug, info, instrument};

use crate::connection::ssh::SshConnection;
use crate::connection::telnet::TelnetConnection;
use crate::facts::cache::FactsCache;
use crate::facts::parser;

// -----------------------------------------------------------------------
// SonicHost
// -----------------------------------------------------------------------

/// Primary SONiC DUT host abstraction -- the Rust counterpart of the Python
/// `SonicHost` class from `sonic-mgmt`.
pub struct SonicHost {
    info: DeviceInfo,
    conn: Arc<Mutex<Option<Box<dyn Connection>>>>,
    facts_cache: FactsCache,
}

impl SonicHost {
    /// Creates a new SONiC host driver with a 5-minute facts cache TTL.
    pub fn new(info: DeviceInfo) -> Self {
        Self {
            info,
            conn: Arc::new(Mutex::new(None)),
            facts_cache: FactsCache::new(Duration::from_secs(300)),
        }
    }

    // -- helper to get a reference to the open connection ----------------

    async fn conn(&self) -> Result<tokio::sync::MutexGuard<'_, Option<Box<dyn Connection>>>> {
        let guard = self.conn.lock().await;
        if guard.is_none() {
            return Err(SonicError::connection(
                &self.info.hostname,
                "not connected",
            ));
        }
        Ok(guard)
    }

    // -- SONiC-specific methods ------------------------------------------

    /// Reload configuration using the specified strategy.
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn config_reload(&self, reload_type: ConfigReloadType) -> Result<CommandResult> {
        let cmd = match reload_type {
            ConfigReloadType::Reload => "sudo config reload -y",
            ConfigReloadType::LoadMinigraph => "sudo config load_minigraph -y",
            ConfigReloadType::GoldenConfig => "sudo config reload /etc/sonic/golden_config_db.json -y",
            ConfigReloadType::FactoryReset => "sudo config factory -y",
        };
        info!("{}: config reload ({:?})", self.info.hostname, reload_type);
        self.execute(cmd).await
    }

    /// Load minigraph XML and apply it (shortcut for `config_reload`).
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn load_minigraph(&self) -> Result<CommandResult> {
        self.config_reload(ConfigReloadType::LoadMinigraph).await
    }

    /// Retrieve the running configuration JSON.
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn get_running_config(&self) -> Result<serde_json::Value> {
        let result = self.execute_checked("sonic-cfggen -d --print-data").await?;
        let value: serde_json::Value = serde_json::from_str(&result.stdout)?;
        Ok(value)
    }

    /// Apply a JSON config patch via `config apply-patch`.
    #[instrument(skip(self, patch_json), fields(host = %self.info.hostname))]
    pub async fn apply_patch(&self, patch_json: &str) -> Result<CommandResult> {
        // Write patch to a temporary file, apply it, then clean up.
        let tmp = "/tmp/sonic_patch.json";
        let write_cmd = format!(
            "cat > {} << 'PATCH_EOF'\n{}\nPATCH_EOF",
            tmp, patch_json
        );
        self.execute(&write_cmd).await?;
        let result = self
            .execute_checked(&format!("sudo config apply-patch {}", tmp))
            .await?;
        self.execute(&format!("rm -f {}", tmp)).await.ok();
        Ok(result)
    }

    /// Get the status of all SONiC containers.
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn get_container_status(&self) -> Result<HashMap<String, String>> {
        let result = self.execute_checked("docker ps --format '{{.Names}}|{{.Status}}'").await?;
        let mut map = HashMap::new();
        for line in result.stdout.lines() {
            if let Some((name, status)) = line.split_once('|') {
                map.insert(name.trim().to_string(), status.trim().to_string());
            }
        }
        Ok(map)
    }

    /// Restart a SONiC service (systemd or container).
    #[instrument(skip(self), fields(host = %self.info.hostname, service = %service))]
    pub async fn restart_service(&self, service: &str) -> Result<CommandResult> {
        self.execute_checked(&format!("sudo systemctl restart {}", service))
            .await
    }

    /// Get CRM (Critical Resource Monitoring) counters.
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn get_crm_resources(&self) -> Result<serde_json::Value> {
        let result = self.execute_checked("crm show resources all").await?;
        // CRM output is plain text; we return it as a JSON string for now.
        // A dedicated parser could be written to produce a typed struct.
        Ok(serde_json::Value::String(result.stdout))
    }

    /// Get queue counter statistics.
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn get_queue_counters(&self) -> Result<serde_json::Value> {
        let result = self.execute_checked("show queue counters").await?;
        Ok(serde_json::Value::String(result.stdout))
    }
}

// -----------------------------------------------------------------------
// `Device` trait
// -----------------------------------------------------------------------

#[async_trait]
impl Device for SonicHost {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    #[instrument(skip(self), fields(host = %self.info.hostname, ip = %self.info.mgmt_ip))]
    async fn connect(&mut self) -> Result<()> {
        let mut connection: Box<dyn Connection> = match self.info.connection_type {
            ConnectionType::Ssh => Box::new(SshConnection::new(
                self.info.mgmt_ip.to_string(),
                self.info.port,
                self.info.credentials.clone(),
            )),
            ConnectionType::Telnet => Box::new(TelnetConnection::new(
                self.info.mgmt_ip.to_string(),
                self.info.port,
                self.info.credentials.clone(),
            )),
            other => {
                return Err(SonicError::other(format!(
                    "unsupported connection type for SonicHost: {}",
                    other
                )));
            }
        };

        connection.open().await?;
        let mut guard = self.conn.lock().await;
        *guard = Some(connection);
        info!("connected to SONiC host {}", self.info.hostname);
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

    #[instrument(skip(self), fields(host = %self.info.hostname, command = %command))]
    async fn execute(&self, command: &str) -> Result<CommandResult> {
        let guard = self.conn().await?;
        guard.as_ref().unwrap().send_command(command).await
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn reboot(&self, reboot_type: RebootType) -> Result<()> {
        let cmd = match reboot_type {
            RebootType::Cold => "sudo reboot",
            RebootType::Warm => "sudo warm-reboot",
            RebootType::Fast => "sudo fast-reboot",
            RebootType::PowerCycle => "sudo reboot -p",
            RebootType::Watchdog => "sudo watchdogutil arm -s 5",
            RebootType::Supervisor => "sudo reboot --supervisor",
            RebootType::Kdump => "sudo echo c > /proc/sysrq-trigger",
        };
        info!("{}: issuing {} reboot", self.info.hostname, reboot_type);
        // Send the reboot command but do not wait for a response -- the
        // connection will drop.
        let guard = self.conn().await?;
        guard.as_ref().unwrap().send(cmd).await.ok();
        Ok(())
    }

    #[instrument(skip(self), fields(host = %self.info.hostname, timeout_secs))]
    async fn wait_ready(&self, timeout_secs: u64) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
        let poll_interval = Duration::from_secs(10);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(SonicError::Timeout {
                    seconds: timeout_secs,
                    operation: format!("{}: wait_ready", self.info.hostname),
                });
            }

            match self.execute("echo ready").await {
                Ok(r) if r.stdout.trim() == "ready" => {
                    info!("{}: device is ready", self.info.hostname);
                    return Ok(());
                }
                _ => {
                    debug!("{}: not ready yet, retrying...", self.info.hostname);
                    tokio::time::sleep(poll_interval).await;
                }
            }
        }
    }
}

// -----------------------------------------------------------------------
// `FactsProvider` trait
// -----------------------------------------------------------------------

#[async_trait]
impl FactsProvider for SonicHost {
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn basic_facts(&self) -> Result<BasicFacts> {
        if let Some(cached) = self.facts_cache.get::<BasicFacts>("basic").await {
            return Ok(cached);
        }

        let version_output = self.execute_checked("show version").await?;
        let platform_output = self.execute_checked("show platform summary").await?;

        let combined = format!("{}\n{}", version_output.stdout, platform_output.stdout);
        let facts = parser::parse_show_version(&combined);

        self.facts_cache.set("basic", &facts).await;
        Ok(facts)
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn bgp_facts(&self) -> Result<BgpFacts> {
        if let Some(cached) = self.facts_cache.get::<BgpFacts>("bgp").await {
            return Ok(cached);
        }

        // Try JSON first (more reliable to parse).
        let json_result = self
            .execute("vtysh -c 'show bgp summary json'")
            .await;

        let facts = match json_result {
            Ok(ref r) if r.exit_code == 0 && r.stdout.trim_start().starts_with('{') => {
                parser::parse_bgp_summary_json(&r.stdout)?
            }
            _ => {
                let plain = self.execute_checked("show ip bgp summary").await?;
                parser::parse_bgp_summary(&plain.stdout)
            }
        };

        self.facts_cache.set("bgp", &facts).await;
        Ok(facts)
    }

    #[instrument(skip(self), fields(host = %self.info.hostname))]
    async fn interface_facts(&self) -> Result<InterfaceFacts> {
        if let Some(cached) = self.facts_cache.get::<InterfaceFacts>("interfaces").await {
            return Ok(cached);
        }

        let intf_output = self.execute_checked("show interfaces status").await?;
        let ports = parser::parse_interface_status(&intf_output.stdout);

        // VLANs
        let vlan_output = self.execute("show vlan brief").await.unwrap_or_else(|_| CommandResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            duration: Duration::ZERO,
            command: String::new(),
        });
        let vlans = parser::parse_vlan_brief(&vlan_output.stdout);

        // LAGs
        let lag_output = self.execute("show interfaces portchannel").await.unwrap_or_else(|_| CommandResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            duration: Duration::ZERO,
            command: String::new(),
        });
        let lags = parser::parse_lag_brief(&lag_output.stdout);

        let facts = InterfaceFacts {
            ports,
            vlans,
            lags,
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

        let cfg_output = self.execute_checked("sonic-cfggen -d --print-data").await?;
        let running: HashMap<String, serde_json::Value> =
            serde_json::from_str(&cfg_output.stdout).unwrap_or_default();

        // Features
        let feat_output = self.execute("show feature status").await.unwrap_or_else(|_| CommandResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            duration: Duration::ZERO,
            command: String::new(),
        });
        let mut features = HashMap::new();
        for line in feat_output.stdout.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                features.insert(
                    parts[0].to_string(),
                    sonic_core::FeatureState {
                        name: parts[0].to_string(),
                        state: parts[1].to_string(),
                        auto_restart: parts.get(2).map_or(false, |s| *s == "enabled"),
                        high_mem_alert: false,
                    },
                );
            }
        }

        // Services
        let svc_output = self.execute("systemctl list-units --type=service --state=running --no-pager --plain --no-legend").await.unwrap_or_else(|_| CommandResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
            duration: Duration::ZERO,
            command: String::new(),
        });
        let mut services = Vec::new();
        for line in svc_output.stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(name) = parts.first() {
                services.push(sonic_core::ServiceInfo {
                    name: name.to_string(),
                    status: parts.get(3).unwrap_or(&"unknown").to_string(),
                    pid: None,
                });
            }
        }

        let facts = ConfigFacts {
            running_config: running,
            startup_config: HashMap::new(),
            features,
            services,
        };

        self.facts_cache.set("config", &facts).await;
        Ok(facts)
    }
}
