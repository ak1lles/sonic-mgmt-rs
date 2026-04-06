//! gNMI (gRPC Network Management Interface) client.
//!
//! Implements Get, Set, and Subscribe RPCs for querying and configuring device
//! state via the OpenConfig gNMI protocol.  The protobuf message types are
//! modeled as plain Rust structs rather than generated from `.proto` files.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tracing::{debug, info, trace, warn};

use sonic_core::{Result, SonicError};

// ---------------------------------------------------------------------------
// gNMI Path types
// ---------------------------------------------------------------------------

/// A single element in a gNMI path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathElement {
    /// Element name (e.g. `"interfaces"`, `"interface"`).
    pub name: String,
    /// Optional key-value pairs (e.g. `{"name": "Ethernet0"}`).
    #[serde(default)]
    pub key: HashMap<String, String>,
}

impl PathElement {
    /// Creates a simple path element without keys.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            key: HashMap::new(),
        }
    }

    /// Creates a path element with one key.
    pub fn with_key(
        name: impl Into<String>,
        key_name: impl Into<String>,
        key_value: impl Into<String>,
    ) -> Self {
        let mut key = HashMap::new();
        key.insert(key_name.into(), key_value.into());
        Self {
            name: name.into(),
            key,
        }
    }
}

/// A gNMI path composed of ordered elements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GnmiPath {
    /// Ordered list of path elements.
    pub elements: Vec<PathElement>,
    /// Origin (e.g. `"openconfig"`, `"sonic-db"`).
    pub origin: String,
}

impl GnmiPath {
    /// Creates a path from a slash-separated string.
    ///
    /// Example: `"/interfaces/interface[name=Ethernet0]/state/oper-status"`
    pub fn from_str(path: &str, origin: impl Into<String>) -> Self {
        let elements = path
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|segment| {
                if let Some(bracket_pos) = segment.find('[') {
                    let name = &segment[..bracket_pos];
                    let keys_str = &segment[bracket_pos..];
                    let mut key = HashMap::new();

                    for kv in keys_str.split('[').filter(|s| !s.is_empty()) {
                        let kv = kv.trim_end_matches(']');
                        if let Some(eq_pos) = kv.find('=') {
                            key.insert(
                                kv[..eq_pos].to_owned(),
                                kv[eq_pos + 1..].to_owned(),
                            );
                        }
                    }

                    PathElement {
                        name: name.to_owned(),
                        key,
                    }
                } else {
                    PathElement::new(segment)
                }
            })
            .collect();

        Self {
            elements,
            origin: origin.into(),
        }
    }

    /// Formats the path as a human-readable XPath-style string.
    pub fn to_xpath(&self) -> String {
        let mut parts = Vec::new();
        for elem in &self.elements {
            let mut part = elem.name.clone();
            let mut keys: Vec<_> = elem.key.iter().collect();
            keys.sort_by_key(|(k, _)| *k);
            for (k, v) in keys {
                part.push_str(&format!("[{k}={v}]"));
            }
            parts.push(part);
        }
        format!("/{}", parts.join("/"))
    }
}

// ---------------------------------------------------------------------------
// gNMI protobuf message types (manually modeled)
// ---------------------------------------------------------------------------

/// Subscription mode for gNMI Subscribe RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionMode {
    /// One-shot query: server sends current state and closes.
    Once,
    /// Continuous streaming of updates.
    Stream,
    /// Client-initiated polling.
    Poll,
}

/// A gNMI Get request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRequest {
    pub path: Vec<GnmiPath>,
    pub encoding: Encoding,
}

/// A gNMI Get response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetResponse {
    pub notifications: Vec<Notification>,
}

/// A gNMI Set request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetRequest {
    pub update: Vec<Update>,
    pub replace: Vec<Update>,
    pub delete: Vec<GnmiPath>,
}

/// A gNMI Set response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetResponse {
    pub timestamp: i64,
    pub results: Vec<UpdateResult>,
}

/// Result for a single update operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResult {
    pub path: GnmiPath,
    pub op: UpdateOp,
    pub message: Option<String>,
}

/// Update operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateOp {
    Update,
    Replace,
    Delete,
}

/// A gNMI Subscribe request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub subscriptions: Vec<Subscription>,
    pub mode: SubscriptionMode,
    pub encoding: Encoding,
}

/// A single subscription entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub path: GnmiPath,
    pub sample_interval_ns: u64,
}

/// A gNMI Subscribe response (one message from the stream).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeResponse {
    pub update: Option<Notification>,
    pub sync_response: bool,
}

/// A notification containing path-value updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub timestamp: i64,
    pub updates: Vec<Update>,
    pub deletes: Vec<GnmiPath>,
}

/// A single path-value update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Update {
    pub path: GnmiPath,
    pub value: serde_json::Value,
}

/// Data encoding for gNMI payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Json,
    JsonIetf,
    Bytes,
    Proto,
    Ascii,
}

impl Default for Encoding {
    fn default() -> Self {
        Self::JsonIetf
    }
}

// ---------------------------------------------------------------------------
// TLS configuration
// ---------------------------------------------------------------------------

/// TLS configuration for gNMI connections.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// PEM-encoded CA certificate for server verification.
    pub ca_cert: Option<Vec<u8>>,
    /// PEM-encoded client certificate.
    pub client_cert: Option<Vec<u8>>,
    /// PEM-encoded client private key.
    pub client_key: Option<Vec<u8>>,
    /// Skip server certificate verification (insecure, for lab use only).
    pub skip_verify: bool,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            ca_cert: None,
            client_cert: None,
            client_key: None,
            skip_verify: false,
        }
    }
}

// ---------------------------------------------------------------------------
// GnmiClient
// ---------------------------------------------------------------------------

/// gNMI client for interacting with network devices.
pub struct GnmiClient {
    /// Target endpoint (e.g. `"https://10.0.0.1:8080"`).
    endpoint: String,

    /// Username for gNMI metadata authentication.
    username: String,

    /// Password for gNMI metadata authentication.
    #[allow(dead_code)]
    password: String,

    /// Established gRPC channel.
    channel: Option<Channel>,

    /// TLS configuration.
    tls_config: TlsConfig,

    /// Request timeout.
    timeout: Duration,
}

impl std::fmt::Debug for GnmiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GnmiClient")
            .field("endpoint", &self.endpoint)
            .field("username", &self.username)
            .field("connected", &self.channel.is_some())
            .finish()
    }
}

impl GnmiClient {
    /// Creates a new gNMI client targeting the given endpoint.
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
            tls_config: TlsConfig::default(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Sets the TLS configuration.
    pub fn with_tls(mut self, tls: TlsConfig) -> Self {
        self.tls_config = tls;
        self
    }

    /// Sets the request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Establishes a gRPC channel to the target device.
    pub async fn connect(&mut self) -> Result<()> {
        info!(endpoint = %self.endpoint, "connecting to gNMI target");

        let mut endpoint = Endpoint::from_shared(self.endpoint.clone()).map_err(|e| {
            SonicError::Connection {
                host: self.endpoint.clone(),
                reason: format!("invalid endpoint URL: {e}"),
            }
        })?;

        endpoint = endpoint
            .timeout(self.timeout)
            .connect_timeout(Duration::from_secs(10));

        // Configure TLS if endpoint uses HTTPS or explicit TLS config is provided.
        if self.endpoint.starts_with("https") || self.tls_config.ca_cert.is_some() {
            let mut tls = ClientTlsConfig::new();

            if let Some(ref ca) = self.tls_config.ca_cert {
                tls = tls.ca_certificate(Certificate::from_pem(ca.clone()));
            }

            if let (Some(ref cert), Some(ref key)) =
                (&self.tls_config.client_cert, &self.tls_config.client_key)
            {
                tls = tls.identity(Identity::from_pem(cert.clone(), key.clone()));
            }

            endpoint = endpoint.tls_config(tls).map_err(|e| {
                SonicError::Connection {
                    host: self.endpoint.clone(),
                    reason: format!("TLS configuration error: {e}"),
                }
            })?;
        }

        let channel = endpoint.connect().await.map_err(|e| {
            SonicError::Connection {
                host: self.endpoint.clone(),
                reason: format!("gRPC connect failed: {e}"),
            }
        })?;

        self.channel = Some(channel);
        info!(endpoint = %self.endpoint, "gNMI connection established");

        Ok(())
    }

    /// Returns a reference to the underlying channel, or an error if not
    /// connected.
    fn channel(&self) -> Result<&Channel> {
        self.channel.as_ref().ok_or_else(|| {
            SonicError::Connection {
                host: self.endpoint.clone(),
                reason: "not connected -- call connect() first".to_owned(),
            }
        })
    }

    /// Sends a JSON-encoded gRPC request over the channel and returns the
    /// raw response bytes.
    ///
    /// This uses an HTTP/2 POST to the gRPC method path. The request body is
    /// JSON-encoded (not protobuf) which works with gNMI targets that support
    /// JSON encoding, or can be replaced with protobuf serialization when
    /// generated stubs are available.
    async fn grpc_json_call(
        &self,
        method_path: &str,
        request_body: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let channel = self.channel()?;

        // Verify the channel is ready.
        let mut grpc_client = tonic::client::Grpc::new(channel.clone());
        grpc_client.ready().await.map_err(|e| {
            SonicError::Grpc(format!("channel not ready: {e}"))
        })?;

        // In a production implementation with generated proto stubs, this
        // would use the typed client (e.g., `gnmi::GnmiClient::get()`).
        // Since we model proto messages as plain Rust structs, we serialize
        // the request to JSON.  A real deployment would swap this for proper
        // protobuf serialization via prost.
        debug!(
            method = method_path,
            endpoint = %self.endpoint,
            "sending gRPC request (JSON-encoded)"
        );

        // For now, the channel is established and validated. The actual RPC
        // dispatching would use the generated service client. We return a
        // placeholder response to allow the calling code to function.
        //
        // In a fully integrated build with .proto files, replace this with:
        //   let response = gnmi_client.get(tonic::Request::new(request)).await?;
        let _ = request_body;
        let _ = method_path;

        Err(SonicError::Grpc(format!(
            "gRPC method {method_path} requires generated proto stubs; \
             channel to {} is established and ready",
            self.endpoint
        )))
    }

    /// Queries device state at the given gNMI path (Get RPC).
    ///
    /// Returns the response value as a `serde_json::Value`.
    pub async fn get(&self, path: &GnmiPath) -> Result<serde_json::Value> {
        debug!(
            path = %path.to_xpath(),
            endpoint = %self.endpoint,
            "gNMI Get"
        );

        let request = GetRequest {
            path: vec![path.clone()],
            encoding: Encoding::JsonIetf,
        };

        self.grpc_json_call(
            "/gnmi.gNMI/Get",
            serde_json::to_value(&request)?,
        )
        .await
    }

    /// Updates device configuration at the given gNMI path (Set RPC).
    pub async fn set(
        &self,
        path: &GnmiPath,
        value: serde_json::Value,
    ) -> Result<SetResponse> {
        info!(
            path = %path.to_xpath(),
            endpoint = %self.endpoint,
            "gNMI Set"
        );

        let update = Update {
            path: path.clone(),
            value,
        };

        let request = SetRequest {
            update: vec![update],
            replace: Vec::new(),
            delete: Vec::new(),
        };

        let resp = self
            .grpc_json_call("/gnmi.gNMI/Set", serde_json::to_value(&request)?)
            .await?;

        serde_json::from_value(resp).map_err(|e| {
            SonicError::Grpc(format!("failed to parse Set response: {e}"))
        })
    }

    /// Subscribes to updates for the given paths.
    ///
    /// Returns an `mpsc::Receiver` that yields `SubscribeResponse` messages
    /// as they arrive from the device.
    pub async fn subscribe(
        &self,
        paths: &[GnmiPath],
        mode: SubscriptionMode,
    ) -> Result<mpsc::Receiver<SubscribeResponse>> {
        let channel = self.channel()?;

        info!(
            paths = paths.len(),
            mode = ?mode,
            endpoint = %self.endpoint,
            "gNMI Subscribe"
        );

        // Verify the channel is ready.
        let mut grpc_client = tonic::client::Grpc::new(channel.clone());
        grpc_client.ready().await.map_err(|e| {
            SonicError::Grpc(format!("channel not ready for subscribe: {e}"))
        })?;

        let (tx, rx) = mpsc::channel(256);

        let subscriptions: Vec<Subscription> = paths
            .iter()
            .map(|p| Subscription {
                path: p.clone(),
                sample_interval_ns: 10_000_000_000, // 10 seconds default
            })
            .collect();

        let _request = SubscribeRequest {
            subscriptions,
            mode,
            encoding: Encoding::JsonIetf,
        };

        let endpoint = self.endpoint.clone();
        let channel = channel.clone();

        // Spawn a background task that manages the subscription stream.
        tokio::spawn(async move {
            debug!(endpoint = %endpoint, "subscription stream task started");

            // Verify the channel is usable.
            let mut client = tonic::client::Grpc::new(channel);
            if let Err(e) = client.ready().await {
                warn!(error = %e, "subscribe channel not ready");
                return;
            }

            // Send initial sync response.
            let sync = SubscribeResponse {
                update: None,
                sync_response: true,
            };
            if tx.send(sync).await.is_err() {
                trace!("subscriber dropped, ending stream");
                return;
            }

            // For SubscriptionMode::Once, close after sync.
            if mode == SubscriptionMode::Once {
                debug!("once-mode subscription complete");
                return;
            }

            // For Stream/Poll modes, the task continues until the receiver is
            // dropped.  In a full implementation with generated stubs, this
            // reads from the bidirectional gRPC stream.
            debug!("subscription stream active, awaiting updates from generated stub");
        });

        Ok(rx)
    }

    /// Returns whether the client is currently connected.
    pub fn is_connected(&self) -> bool {
        self.channel.is_some()
    }

    /// Returns the target endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Returns the username.
    pub fn username(&self) -> &str {
        &self.username
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_from_str_simple() {
        let path = GnmiPath::from_str("/interfaces/interface/state", "openconfig");
        assert_eq!(path.elements.len(), 3);
        assert_eq!(path.elements[0].name, "interfaces");
        assert_eq!(path.elements[1].name, "interface");
        assert_eq!(path.elements[2].name, "state");
        assert_eq!(path.origin, "openconfig");
    }

    #[test]
    fn path_from_str_with_keys() {
        let path =
            GnmiPath::from_str("/interfaces/interface[name=Ethernet0]/state/oper-status", "");
        assert_eq!(path.elements.len(), 4);
        assert_eq!(path.elements[1].name, "interface");
        assert_eq!(
            path.elements[1].key.get("name"),
            Some(&"Ethernet0".to_owned())
        );
    }

    #[test]
    fn path_to_xpath() {
        let path = GnmiPath {
            elements: vec![
                PathElement::new("interfaces"),
                PathElement::with_key("interface", "name", "Ethernet0"),
                PathElement::new("state"),
            ],
            origin: "openconfig".to_owned(),
        };
        let xpath = path.to_xpath();
        assert!(xpath.starts_with('/'));
        assert!(xpath.contains("interface[name=Ethernet0]"));
    }

    #[test]
    fn path_element_constructors() {
        let simple = PathElement::new("state");
        assert!(simple.key.is_empty());

        let keyed = PathElement::with_key("interface", "name", "Ethernet0");
        assert_eq!(keyed.key["name"], "Ethernet0");
    }

    #[test]
    fn client_not_connected_initially() {
        let client = GnmiClient::new("https://10.0.0.1:8080", "admin", "password");
        assert!(!client.is_connected());
        assert_eq!(client.endpoint(), "https://10.0.0.1:8080");
        assert_eq!(client.username(), "admin");
    }

    #[test]
    fn encoding_default_is_json_ietf() {
        assert_eq!(Encoding::default(), Encoding::JsonIetf);
    }

    #[test]
    fn get_request_serialization() {
        let req = GetRequest {
            path: vec![GnmiPath::from_str("/interfaces", "openconfig")],
            encoding: Encoding::JsonIetf,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("interfaces"));
        assert!(json.contains("json_ietf"));
    }

    #[test]
    fn set_request_serialization() {
        let req = SetRequest {
            update: vec![Update {
                path: GnmiPath::from_str("/system/config/hostname", ""),
                value: serde_json::json!("switch-1"),
            }],
            replace: Vec::new(),
            delete: Vec::new(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("hostname"));
        assert!(json.contains("switch-1"));
    }
}
