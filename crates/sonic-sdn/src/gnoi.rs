//! gNOI (gRPC Network Operations Interface) client.
//!
//! Implements operational RPCs for device management: reboot, time sync,
//! ping/traceroute, certificate rotation, file transfer, and OS management.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tracing::{debug, info, warn};

use sonic_core::{Result, SonicError};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Reboot method for the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebootMethod {
    /// Cold (full power-cycle) reboot.
    Cold,
    /// Warm reboot (preserving forwarding state where possible).
    Warm,
    /// Fast reboot.
    Fast,
    /// Power-cycle via BMC / PDU.
    PowerCycle,
    /// NSF (Non-Stop Forwarding) restart.
    Nsf,
}

/// Result of a ping operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResult {
    /// Destination that was pinged.
    pub destination: String,
    /// Number of packets sent.
    pub sent: u32,
    /// Number of packets received.
    pub received: u32,
    /// Minimum RTT in milliseconds.
    pub min_rtt_ms: f64,
    /// Average RTT in milliseconds.
    pub avg_rtt_ms: f64,
    /// Maximum RTT in milliseconds.
    pub max_rtt_ms: f64,
    /// Packet loss percentage.
    pub packet_loss_pct: f64,
}

/// A single hop in a traceroute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracerouteHop {
    /// Hop number (1-based).
    pub hop: u32,
    /// IP address of the hop (may be empty for timeouts).
    pub address: String,
    /// Hostname if resolved.
    pub hostname: Option<String>,
    /// Round-trip times for probes (in milliseconds).
    pub rtt_ms: Vec<f64>,
}

/// Result of a traceroute operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracerouteResult {
    /// Target destination.
    pub destination: String,
    /// Hops along the path.
    pub hops: Vec<TracerouteHop>,
}

/// Result of an OS install operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsInstallResult {
    /// Version that was installed.
    pub version: String,
    /// Whether installation succeeded.
    pub success: bool,
    /// Status message.
    pub message: String,
}

// ---------------------------------------------------------------------------
// GnoiClient
// ---------------------------------------------------------------------------

/// gNOI client for network operations.
pub struct GnoiClient {
    /// Target endpoint.
    endpoint: String,

    /// Username for authentication.
    username: String,

    /// Password for authentication.
    #[allow(dead_code)]
    password: String,

    /// Established gRPC channel.
    channel: Option<Channel>,

    /// Request timeout.
    timeout: Duration,
}

impl std::fmt::Debug for GnoiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GnoiClient")
            .field("endpoint", &self.endpoint)
            .field("username", &self.username)
            .field("connected", &self.channel.is_some())
            .finish()
    }
}

impl GnoiClient {
    /// Creates a new gNOI client.
    pub fn new(
        endpoint: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            username: username.into(),
            password: password.into(),
            channel: None,
            timeout: Duration::from_secs(30),
        }
    }

    /// Sets the request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Establishes the gRPC channel to the target.
    pub async fn connect(&mut self) -> Result<()> {
        info!(endpoint = %self.endpoint, "connecting to gNOI target");

        let mut ep = Endpoint::from_shared(self.endpoint.clone()).map_err(|e| {
            SonicError::Connection {
                host: self.endpoint.clone(),
                reason: format!("invalid endpoint URL: {e}"),
            }
        })?;

        ep = ep
            .timeout(self.timeout)
            .connect_timeout(Duration::from_secs(10));

        if self.endpoint.starts_with("https") {
            let tls = ClientTlsConfig::new();
            ep = ep.tls_config(tls).map_err(|e| {
                SonicError::Connection {
                    host: self.endpoint.clone(),
                    reason: format!("TLS config error: {e}"),
                }
            })?;
        }

        let channel = ep.connect().await.map_err(|e| {
            SonicError::Connection {
                host: self.endpoint.clone(),
                reason: format!("gRPC connect failed: {e}"),
            }
        })?;

        self.channel = Some(channel);
        info!(endpoint = %self.endpoint, "gNOI connection established");
        Ok(())
    }

    /// Returns a reference to the channel or an error if not connected.
    fn channel(&self) -> Result<&Channel> {
        self.channel.as_ref().ok_or_else(|| {
            SonicError::Connection {
                host: self.endpoint.clone(),
                reason: "not connected".to_owned(),
            }
        })
    }

    /// Verifies the channel is ready and sends a JSON-encoded gRPC request.
    ///
    /// In a full deployment with generated proto stubs, this would be replaced
    /// by typed RPC calls. The channel establishment and readiness check are
    /// real; the RPC dispatch requires proto codegen.
    async fn grpc_call(
        &self,
        method: &str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let channel = self.channel()?;

        let mut client = tonic::client::Grpc::new(channel.clone());
        client.ready().await.map_err(|e| {
            SonicError::Grpc(format!("channel not ready: {e}"))
        })?;

        debug!(
            method,
            endpoint = %self.endpoint,
            "gNOI RPC (channel ready, requires proto stubs for dispatch)"
        );

        let _ = request;

        Err(SonicError::Grpc(format!(
            "gNOI method {method} requires generated proto stubs; \
             channel to {} is established and ready",
            self.endpoint
        )))
    }

    // -----------------------------------------------------------------------
    // System service RPCs
    // -----------------------------------------------------------------------

    /// Triggers a device reboot.
    pub async fn system_reboot(
        &self,
        method: RebootMethod,
        delay_secs: u64,
    ) -> Result<()> {
        info!(
            endpoint = %self.endpoint,
            method = ?method,
            delay_secs,
            "issuing system reboot"
        );

        let request = serde_json::json!({
            "method": method,
            "delay": delay_secs,
            "message": format!("gNOI reboot: {:?}", method),
        });

        self.grpc_call("/gnoi.system.System/Reboot", request).await?;
        info!(method = ?method, "reboot command accepted");
        Ok(())
    }

    /// Retrieves the current system time from the device.
    pub async fn system_time(&self) -> Result<chrono::DateTime<chrono::Utc>> {
        debug!(endpoint = %self.endpoint, "querying system time");

        let resp = self
            .grpc_call("/gnoi.system.System/Time", serde_json::json!({}))
            .await?;

        let nanos = resp["time"].as_i64().unwrap_or(0);
        let secs = nanos / 1_000_000_000;
        let nsecs = (nanos % 1_000_000_000) as u32;

        let dt = chrono::DateTime::from_timestamp(secs, nsecs)
            .unwrap_or_else(chrono::Utc::now);

        debug!(time = %dt, "device system time");
        Ok(dt)
    }

    /// Executes a ping from the device to the given destination.
    pub async fn system_ping(
        &self,
        destination: &str,
        count: u32,
    ) -> Result<PingResult> {
        info!(
            endpoint = %self.endpoint,
            destination,
            count,
            "executing ping"
        );

        let request = serde_json::json!({
            "destination": destination,
            "count": count,
            "wait": 1,
        });

        let resp = self
            .grpc_call("/gnoi.system.System/Ping", request)
            .await?;

        Ok(PingResult {
            destination: destination.to_owned(),
            sent: resp["sent"].as_u64().unwrap_or(count as u64) as u32,
            received: resp["received"].as_u64().unwrap_or(0) as u32,
            min_rtt_ms: resp["min_rtt_ms"].as_f64().unwrap_or(0.0),
            avg_rtt_ms: resp["avg_rtt_ms"].as_f64().unwrap_or(0.0),
            max_rtt_ms: resp["max_rtt_ms"].as_f64().unwrap_or(0.0),
            packet_loss_pct: resp["packet_loss_pct"].as_f64().unwrap_or(100.0),
        })
    }

    /// Executes a traceroute from the device to the given destination.
    pub async fn system_traceroute(
        &self,
        destination: &str,
    ) -> Result<TracerouteResult> {
        info!(
            endpoint = %self.endpoint,
            destination,
            "executing traceroute"
        );

        let request = serde_json::json!({
            "destination": destination,
            "max_ttl": 30,
            "wait": 2,
        });

        let resp = self
            .grpc_call("/gnoi.system.System/Traceroute", request)
            .await?;

        let hops = resp["hops"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .enumerate()
                    .map(|(i, hop)| TracerouteHop {
                        hop: (i + 1) as u32,
                        address: hop["address"].as_str().unwrap_or("").to_owned(),
                        hostname: hop["hostname"].as_str().map(|s| s.to_owned()),
                        rtt_ms: hop["rtt_ms"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(TracerouteResult {
            destination: destination.to_owned(),
            hops,
        })
    }

    // -----------------------------------------------------------------------
    // Certificate service RPCs
    // -----------------------------------------------------------------------

    /// Rotates TLS certificates on the device.
    pub async fn cert_rotate(
        &self,
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> Result<()> {
        info!(
            endpoint = %self.endpoint,
            cert_len = cert_pem.len(),
            "rotating TLS certificate"
        );

        let request = serde_json::json!({
            "certificate": base64_encode(cert_pem),
            "key": base64_encode(key_pem),
        });

        self.grpc_call("/gnoi.cert.CertificateManagement/Rotate", request)
            .await?;

        info!("certificate rotation complete");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // File service RPCs
    // -----------------------------------------------------------------------

    /// Downloads a file from the device.
    pub async fn file_get(&self, remote_path: &str) -> Result<Vec<u8>> {
        info!(
            endpoint = %self.endpoint,
            path = remote_path,
            "downloading file from device"
        );

        let request = serde_json::json!({ "remote_file": remote_path });

        let resp = self.grpc_call("/gnoi.file.File/Get", request).await?;

        let data = resp["contents"]
            .as_str()
            .map(base64_decode)
            .unwrap_or_default();

        debug!(path = remote_path, size = data.len(), "file downloaded");
        Ok(data)
    }

    /// Uploads a file to the device.
    pub async fn file_put(
        &self,
        local_data: &[u8],
        remote_path: &str,
    ) -> Result<()> {
        info!(
            endpoint = %self.endpoint,
            remote_path,
            size = local_data.len(),
            "uploading file to device"
        );

        let request = serde_json::json!({
            "remote_file": remote_path,
            "contents": base64_encode(local_data),
        });

        self.grpc_call("/gnoi.file.File/Put", request).await?;

        info!(remote_path, "file uploaded");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // OS service RPCs
    // -----------------------------------------------------------------------

    /// Installs a new OS image on the device.
    pub async fn os_install(
        &self,
        image_url: &str,
        version: &str,
    ) -> Result<OsInstallResult> {
        info!(
            endpoint = %self.endpoint,
            image_url,
            version,
            "installing OS image"
        );

        let request = serde_json::json!({
            "image_url": image_url,
            "version": version,
        });

        let resp = self.grpc_call("/gnoi.os.OS/Install", request).await?;

        let result = OsInstallResult {
            version: version.to_owned(),
            success: resp["success"].as_bool().unwrap_or(false),
            message: resp["message"]
                .as_str()
                .unwrap_or("no message")
                .to_owned(),
        };

        if result.success {
            info!(version, "OS installation successful");
        } else {
            warn!(version, message = %result.message, "OS installation failed");
        }

        Ok(result)
    }

    /// Activates an installed OS version.
    pub async fn os_activate(&self, version: &str) -> Result<()> {
        info!(
            endpoint = %self.endpoint,
            version,
            "activating OS version"
        );

        let request = serde_json::json!({ "version": version });
        self.grpc_call("/gnoi.os.OS/Activate", request).await?;

        info!(version, "OS version activated");
        Ok(())
    }

    /// Returns whether the client is connected.
    pub fn is_connected(&self) -> bool {
        self.channel.is_some()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Base64-encodes a byte slice.
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut encoded = String::with_capacity((data.len() + 2) / 3 * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        encoded.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        encoded.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            encoded.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            encoded.push('=');
        }

        if chunk.len() > 2 {
            encoded.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
}

/// Base64-decodes a string.
fn base64_decode(data: &str) -> Vec<u8> {
    fn decode_char(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes: Vec<u8> = data.bytes().filter(|b| *b != b'=').collect();
    let mut result = Vec::with_capacity(bytes.len() * 3 / 4);

    for chunk in bytes.chunks(4) {
        let vals: Vec<u8> = chunk.iter().filter_map(|&b| decode_char(b)).collect();
        if vals.is_empty() {
            continue;
        }

        let n = vals.len();
        let triple = (vals[0] as u32) << 18
            | vals.get(1).map(|&v| (v as u32) << 12).unwrap_or(0)
            | vals.get(2).map(|&v| (v as u32) << 6).unwrap_or(0)
            | vals.get(3).map(|&v| v as u32).unwrap_or(0);

        result.push((triple >> 16) as u8);
        if n > 2 {
            result.push((triple >> 8) as u8);
        }
        if n > 3 {
            result.push(triple as u8);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        let data = b"Hello, gNOI!";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded);
        assert_eq!(decoded, data);
    }

    #[test]
    fn base64_empty() {
        assert_eq!(base64_encode(b""), "");
        assert!(base64_decode("").is_empty());
    }

    #[test]
    fn client_not_connected() {
        let client = GnoiClient::new("https://10.0.0.1:9339", "admin", "pass");
        assert!(!client.is_connected());
    }

    #[test]
    fn reboot_method_serialization() {
        for method in [
            RebootMethod::Cold,
            RebootMethod::Warm,
            RebootMethod::Fast,
            RebootMethod::PowerCycle,
            RebootMethod::Nsf,
        ] {
            let json = serde_json::to_string(&method).unwrap();
            let deserialized: RebootMethod = serde_json::from_str(&json).unwrap();
            assert_eq!(method, deserialized);
        }
    }

    #[test]
    fn ping_result_fields() {
        let result = PingResult {
            destination: "10.0.0.1".into(),
            sent: 5,
            received: 4,
            min_rtt_ms: 0.5,
            avg_rtt_ms: 1.2,
            max_rtt_ms: 3.4,
            packet_loss_pct: 20.0,
        };
        assert_eq!(result.sent - result.received, 1);
    }

    #[test]
    fn traceroute_result_structure() {
        let result = TracerouteResult {
            destination: "8.8.8.8".into(),
            hops: vec![
                TracerouteHop {
                    hop: 1,
                    address: "10.0.0.1".into(),
                    hostname: Some("gateway".into()),
                    rtt_ms: vec![0.5, 0.6, 0.7],
                },
                TracerouteHop {
                    hop: 2,
                    address: "172.16.0.1".into(),
                    hostname: None,
                    rtt_ms: vec![1.2, 1.3],
                },
            ],
        };
        assert_eq!(result.hops.len(), 2);
        assert_eq!(result.hops[0].rtt_ms.len(), 3);
    }

    #[test]
    fn os_install_result_serialization() {
        let result = OsInstallResult {
            version: "20240101.1".into(),
            success: true,
            message: "installed successfully".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: OsInstallResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.version, "20240101.1");
        assert!(deserialized.success);
    }
}
