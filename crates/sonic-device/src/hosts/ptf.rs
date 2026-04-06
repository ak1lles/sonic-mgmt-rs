use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sonic_core::{
    BasicFacts, BgpFacts, CommandResult, ConfigFacts, Connection, Device,
    DeviceInfo, FactsProvider, InterfaceFacts, RebootType, SonicError, Result,
};
use tokio::sync::Mutex;
use tracing::{info, instrument};

use crate::connection::ssh::SshConnection;

// -----------------------------------------------------------------------
// PtfHost
// -----------------------------------------------------------------------

/// PTF (Packet Test Framework) container host.  PTF containers run scapy and
/// the `ptf` test runner to inject and capture packets on the data plane.
pub struct PtfHost {
    info: DeviceInfo,
    conn: Arc<Mutex<Option<Box<dyn Connection>>>>,
}

impl PtfHost {
    /// Creates a new PTF host driver.
    pub fn new(info: DeviceInfo) -> Self {
        Self {
            info,
            conn: Arc::new(Mutex::new(None)),
        }
    }

    async fn conn(&self) -> Result<tokio::sync::MutexGuard<'_, Option<Box<dyn Connection>>>> {
        let guard = self.conn.lock().await;
        if guard.is_none() {
            return Err(SonicError::connection(&self.info.hostname, "not connected"));
        }
        Ok(guard)
    }

    // -- PTF-specific methods --------------------------------------------

    /// Run a PTF test by name.
    ///
    /// * `test_name`  -- Python module path, e.g. `"ptftests.py3.ip_test.DataplaneTest"`.
    /// * `test_params` -- key=value parameter map passed via `--test-params`.
    /// * `platform_dir` -- path to the platform directory inside the PTF
    ///   container, typically `"/root/ptftests"`.
    #[instrument(skip(self, test_params), fields(host = %self.info.hostname, test = %test_name))]
    pub async fn run_ptf_test(
        &self,
        test_name: &str,
        test_params: &HashMap<String, String>,
        platform_dir: &str,
    ) -> Result<CommandResult> {
        let params_str = test_params
            .iter()
            .map(|(k, v)| format!("{}='{}'", k, v))
            .collect::<Vec<_>>()
            .join(";");

        let cmd = format!(
            "ptf --test-dir {} --test-params '{}' {}",
            platform_dir, params_str, test_name,
        );
        self.execute_checked(&cmd).await
    }

    /// Copy a file from the local filesystem into the PTF container via
    /// `scp`-style `cat > file` over the SSH channel.
    #[instrument(skip(self, content), fields(host = %self.info.hostname, dst = %remote_path))]
    pub async fn copy_file(&self, remote_path: &str, content: &[u8]) -> Result<()> {
        // Encode as base64 so we can safely transport arbitrary bytes.
        let encoded = base64_encode(content);
        let cmd = format!(
            "echo '{}' | base64 -d > {}",
            encoded, remote_path
        );
        self.execute_checked(&cmd).await?;
        Ok(())
    }

    /// Get the management IP address from within the container.
    #[instrument(skip(self), fields(host = %self.info.hostname))]
    pub async fn get_ip_addr(&self) -> Result<Vec<String>> {
        let result = self
            .execute_checked("ip -4 addr show scope global | grep inet")
            .await?;
        let addrs = result
            .stdout
            .lines()
            .filter_map(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                parts.get(1).map(|s| s.to_string())
            })
            .collect();
        Ok(addrs)
    }

    /// Install a Python package inside the PTF container.
    #[instrument(skip(self), fields(host = %self.info.hostname, package = %package))]
    pub async fn install_package(&self, package: &str) -> Result<CommandResult> {
        self.execute_checked(&format!("pip install {}", package)).await
    }
}

/// Minimal base64 encoder (avoids pulling in a whole crate for one use).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

// -----------------------------------------------------------------------
// `Device` trait
// -----------------------------------------------------------------------

#[async_trait]
impl Device for PtfHost {
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
        info!("connected to PTF host {}", self.info.hostname);
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
        // PTF containers are typically restarted externally via Docker.
        let guard = self.conn().await?;
        guard.as_ref().unwrap().send("reboot").await.ok();
        Ok(())
    }

    #[instrument(skip(self), fields(host = %self.info.hostname, timeout_secs))]
    async fn wait_ready(&self, timeout_secs: u64) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
        let poll = Duration::from_secs(5);
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(SonicError::Timeout {
                    seconds: timeout_secs,
                    operation: format!("{}: wait_ready", self.info.hostname),
                });
            }
            match self.execute("echo ready").await {
                Ok(r) if r.stdout.trim() == "ready" => return Ok(()),
                _ => tokio::time::sleep(poll).await,
            }
        }
    }
}

// -----------------------------------------------------------------------
// `FactsProvider` trait
// -----------------------------------------------------------------------

#[async_trait]
impl FactsProvider for PtfHost {
    async fn basic_facts(&self) -> Result<BasicFacts> {
        let hostname_r = self.execute_checked("hostname").await?;
        let uname_r = self.execute_checked("uname -r").await?;
        Ok(BasicFacts {
            hostname: hostname_r.stdout.trim().to_string(),
            platform: "PTF".to_string(),
            kernel_version: uname_r.stdout.trim().to_string(),
            ..Default::default()
        })
    }

    async fn bgp_facts(&self) -> Result<BgpFacts> {
        // PTF containers do not run BGP.
        Ok(BgpFacts::default())
    }

    async fn interface_facts(&self) -> Result<InterfaceFacts> {
        let r = self.execute_checked("ip -o link show").await?;
        let mut ports = Vec::new();
        for line in r.stdout.lines() {
            // Format: "2: eth0: <BROADCAST,...> mtu 9100 ..."
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[1].trim_end_matches(':').to_string();
                if name == "lo" {
                    continue;
                }
                let lower = line.to_lowercase();
                let oper = if lower.contains("state up") {
                    sonic_core::PortStatus::Up
                } else {
                    sonic_core::PortStatus::Down
                };
                ports.push(sonic_core::PortInfo {
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
        Ok(InterfaceFacts {
            ports,
            vlans: Vec::new(),
            lags: Vec::new(),
            loopbacks: Vec::new(),
        })
    }

    async fn config_facts(&self) -> Result<ConfigFacts> {
        Ok(ConfigFacts::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64_single_byte() {
        // 'A' = 0x41 -> base64 "QQ=="
        assert_eq!(base64_encode(b"A"), "QQ==");
    }

    #[test]
    fn base64_two_bytes() {
        // "AB" -> base64 "QUI="
        assert_eq!(base64_encode(b"AB"), "QUI=");
    }

    #[test]
    fn base64_three_bytes() {
        // "ABC" -> base64 "QUJD" (no padding)
        assert_eq!(base64_encode(b"ABC"), "QUJD");
    }

    #[test]
    fn base64_hello_world() {
        assert_eq!(base64_encode(b"Hello, World!"), "SGVsbG8sIFdvcmxkIQ==");
    }

    #[test]
    fn base64_binary_data() {
        let data: Vec<u8> = (0..=255).collect();
        let encoded = base64_encode(&data);
        // Just verify it doesn't panic and produces valid base64 chars
        assert!(encoded.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='
        }));
        // Length should be ceil(256/3)*4 = 344
        assert_eq!(encoded.len(), 344);
    }
}
