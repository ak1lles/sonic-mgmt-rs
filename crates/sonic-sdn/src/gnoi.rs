//! gNOI (gRPC Network Operations Interface) client.
//!
//! Implements operational RPCs for device management: reboot, time sync,
//! ping/traceroute, certificate rotation, file transfer, and OS management.
//! The System service RPCs use generated tonic stubs for proper protobuf
//! serialization; other services (cert, file, OS) remain as stub placeholders
//! until their proto definitions are added.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tracing::{debug, info};

use sonic_core::{Result, SonicError};

use crate::proto::gnoi_system as pb;

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

impl RebootMethod {
    fn to_proto(self) -> i32 {
        match self {
            Self::Cold => pb::RebootMethod::Cold as i32,
            Self::Warm => pb::RebootMethod::Warm as i32,
            Self::Fast => pb::RebootMethod::Cold as i32, // No "fast" in proto; use cold
            Self::PowerCycle => pb::RebootMethod::Powerdown as i32,
            Self::Nsf => pb::RebootMethod::Nsf as i32,
        }
    }
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

    /// Builds a generated gNOI System client from the current channel.
    fn system_client(&self) -> Result<pb::system_client::SystemClient<Channel>> {
        let channel = self.channel()?.clone();
        Ok(pb::system_client::SystemClient::new(channel))
    }

    // -----------------------------------------------------------------------
    // System service RPCs (using generated stubs)
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

        let request = pb::RebootRequest {
            method: method.to_proto(),
            delay: delay_secs,
            message: format!("gNOI reboot: {method:?}"),
            subcomponents: Vec::new(),
            force: false,
        };

        let mut client = self.system_client()?;
        client.reboot(request).await.map_err(|e| {
            SonicError::Grpc(format!("gNOI Reboot failed: {e}"))
        })?;

        info!(method = ?method, "reboot command accepted");
        Ok(())
    }

    /// Retrieves the current system time from the device.
    pub async fn system_time(&self) -> Result<chrono::DateTime<chrono::Utc>> {
        debug!(endpoint = %self.endpoint, "querying system time");

        let mut client = self.system_client()?;
        let response = client.time(pb::TimeRequest {}).await.map_err(|e| {
            SonicError::Grpc(format!("gNOI Time failed: {e}"))
        })?;

        let nanos = response.into_inner().time;
        let secs = (nanos / 1_000_000_000) as i64;
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

        let request = pb::PingRequest {
            destination: destination.to_owned(),
            source: String::new(),
            count: count as i32,
            interval: 0,
            wait: 1,
            size: 0,
            do_not_fragment: false,
            do_not_resolve: false,
            l3protocol: String::new(),
        };

        let mut client = self.system_client()?;
        let response = client.ping(request).await.map_err(|e| {
            SonicError::Grpc(format!("gNOI Ping failed: {e}"))
        })?;

        // Ping is a server-streaming RPC. Collect all responses and build an
        // aggregate PingResult from the final summary message.
        let mut stream = response.into_inner();
        let mut last_resp: Option<pb::PingResponse> = None;

        while let Some(msg) = stream.message().await.map_err(|e| {
            SonicError::Grpc(format!("ping stream error: {e}"))
        })? {
            last_resp = Some(msg);
        }

        let resp = last_resp.unwrap_or_default();
        let sent = if resp.sent > 0 { resp.sent as u32 } else { count };
        let received = resp.received as u32;
        let loss = if sent > 0 {
            ((sent - received) as f64 / sent as f64) * 100.0
        } else {
            100.0
        };

        Ok(PingResult {
            destination: destination.to_owned(),
            sent,
            received,
            min_rtt_ms: resp.min_time as f64,
            avg_rtt_ms: resp.avg_time as f64,
            max_rtt_ms: resp.max_time as f64,
            packet_loss_pct: loss,
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

        let request = pb::TracerouteRequest {
            destination: destination.to_owned(),
            source: String::new(),
            initial_ttl: 1,
            max_ttl: 30,
            wait: 2,
            do_not_fragment: false,
            do_not_resolve: false,
            l3protocol: String::new(),
            do_not_lookup: false,
        };

        let mut client = self.system_client()?;
        let response = client.traceroute(request).await.map_err(|e| {
            SonicError::Grpc(format!("gNOI Traceroute failed: {e}"))
        })?;

        // Traceroute is a server-streaming RPC. Each message contains one hop.
        let mut stream = response.into_inner();
        let mut hops = Vec::new();

        while let Some(msg) = stream.message().await.map_err(|e| {
            SonicError::Grpc(format!("traceroute stream error: {e}"))
        })? {
            if let Some(hop) = msg.hop {
                hops.push(TracerouteHop {
                    hop: (hop.index + 1) as u32,
                    address: hop.address.clone(),
                    hostname: if hop.name.is_empty() { None } else { Some(hop.name) },
                    rtt_ms: vec![hop.rtt as f64 / 1000.0],
                });
            }
        }

        Ok(TracerouteResult {
            destination: destination.to_owned(),
            hops,
        })
    }

    // -----------------------------------------------------------------------
    // Certificate service RPCs (no proto definition yet)
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

        let _ = (cert_pem, key_pem);
        self.channel()?;

        Err(SonicError::Grpc(
            "gNOI CertificateManagement service requires proto definitions \
             not yet included in this crate"
                .to_owned(),
        ))
    }

    // -----------------------------------------------------------------------
    // File service RPCs (no proto definition yet)
    // -----------------------------------------------------------------------

    /// Downloads a file from the device.
    pub async fn file_get(&self, remote_path: &str) -> Result<Vec<u8>> {
        info!(
            endpoint = %self.endpoint,
            path = remote_path,
            "downloading file from device"
        );

        self.channel()?;

        Err(SonicError::Grpc(
            "gNOI File service requires proto definitions not yet included \
             in this crate"
                .to_owned(),
        ))
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

        self.channel()?;

        Err(SonicError::Grpc(
            "gNOI File service requires proto definitions not yet included \
             in this crate"
                .to_owned(),
        ))
    }

    // -----------------------------------------------------------------------
    // OS service RPCs (no proto definition yet)
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

        self.channel()?;

        Err(SonicError::Grpc(
            "gNOI OS service requires proto definitions not yet included \
             in this crate"
                .to_owned(),
        ))
    }

    /// Activates an installed OS version.
    pub async fn os_activate(&self, version: &str) -> Result<()> {
        info!(
            endpoint = %self.endpoint,
            version,
            "activating OS version"
        );

        self.channel()?;

        Err(SonicError::Grpc(
            "gNOI OS service requires proto definitions not yet included \
             in this crate"
                .to_owned(),
        ))
    }

    /// Returns whether the client is connected.
    pub fn is_connected(&self) -> bool {
        self.channel.is_some()
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn reboot_method_to_proto() {
        assert_eq!(RebootMethod::Cold.to_proto(), pb::RebootMethod::Cold as i32);
        assert_eq!(RebootMethod::Warm.to_proto(), pb::RebootMethod::Warm as i32);
        assert_eq!(RebootMethod::Nsf.to_proto(), pb::RebootMethod::Nsf as i32);
        assert_eq!(
            RebootMethod::PowerCycle.to_proto(),
            pb::RebootMethod::Powerdown as i32
        );
    }
}
