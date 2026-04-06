//! gNMI (gRPC Network Management Interface) client.
//!
//! Implements Get, Set, and Subscribe RPCs for querying and configuring device
//! state via the OpenConfig gNMI protocol. Protobuf serialization is handled by
//! the generated tonic/prost stubs; the public API uses ergonomic Rust types
//! with conversions to and from the proto layer.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tracing::{debug, info, trace, warn};

use sonic_core::{Result, SonicError};

use crate::proto::gnmi as pb;

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
// Proto <-> public type conversions
// ---------------------------------------------------------------------------

impl From<&GnmiPath> for pb::Path {
    fn from(path: &GnmiPath) -> Self {
        pb::Path {
            elem: path.elements.iter().map(|e| pb::PathElem {
                name: e.name.clone(),
                key: e.key.clone(),
            }).collect(),
            origin: path.origin.clone(),
            target: String::new(),
        }
    }
}

impl From<&pb::Path> for GnmiPath {
    fn from(path: &pb::Path) -> Self {
        GnmiPath {
            elements: path.elem.iter().map(|e| PathElement {
                name: e.name.clone(),
                key: e.key.clone(),
            }).collect(),
            origin: path.origin.clone(),
        }
    }
}

/// Converts a proto TypedValue into a serde_json::Value for the public API.
fn typed_value_to_json(tv: &pb::TypedValue) -> serde_json::Value {
    match &tv.value {
        Some(pb::typed_value::Value::StringVal(s)) => serde_json::Value::String(s.clone()),
        Some(pb::typed_value::Value::IntVal(n)) => serde_json::json!(*n),
        Some(pb::typed_value::Value::UintVal(n)) => serde_json::json!(*n),
        Some(pb::typed_value::Value::BoolVal(b)) => serde_json::Value::Bool(*b),
        Some(pb::typed_value::Value::FloatVal(f)) => serde_json::json!(*f),
        Some(pb::typed_value::Value::DoubleVal(f)) => serde_json::json!(*f),
        Some(pb::typed_value::Value::JsonIetfVal(bytes))
        | Some(pb::typed_value::Value::JsonVal(bytes)) => {
            serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null)
        }
        Some(pb::typed_value::Value::BytesVal(bytes)) => {
            serde_json::json!(bytes)
        }
        _ => serde_json::Value::Null,
    }
}

/// Converts a serde_json::Value into a proto TypedValue, preferring JSON_IETF
/// encoding for objects/arrays and direct typed encoding for scalars.
fn json_to_typed_value(val: &serde_json::Value) -> pb::TypedValue {
    let value = match val {
        serde_json::Value::String(s) => {
            Some(pb::typed_value::Value::StringVal(s.clone()))
        }
        serde_json::Value::Bool(b) => {
            Some(pb::typed_value::Value::BoolVal(*b))
        }
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(pb::typed_value::Value::IntVal(i))
            } else if let Some(u) = n.as_u64() {
                Some(pb::typed_value::Value::UintVal(u))
            } else if let Some(f) = n.as_f64() {
                Some(pb::typed_value::Value::DoubleVal(f))
            } else {
                None
            }
        }
        _ => {
            // Objects, arrays, null -- encode as JSON_IETF bytes.
            let bytes = serde_json::to_vec(val).unwrap_or_default();
            Some(pb::typed_value::Value::JsonIetfVal(bytes))
        }
    };
    pb::TypedValue { value }
}

// ---------------------------------------------------------------------------
// gNMI public message types
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

impl Encoding {
    fn to_proto(self) -> i32 {
        match self {
            Self::Json => pb::Encoding::Json as i32,
            Self::JsonIetf => pb::Encoding::JsonIetf as i32,
            Self::Bytes => pb::Encoding::Bytes as i32,
            Self::Proto => pb::Encoding::Proto as i32,
            Self::Ascii => pb::Encoding::Ascii as i32,
        }
    }
}

// ---------------------------------------------------------------------------
// Proto response -> public type conversions
// ---------------------------------------------------------------------------

fn notification_from_proto(n: &pb::Notification) -> Notification {
    Notification {
        timestamp: n.timestamp,
        updates: n.update.iter().map(|u| {
            let path = u.path.as_ref()
                .map(GnmiPath::from)
                .unwrap_or_else(|| GnmiPath { elements: vec![], origin: String::new() });
            let value = u.val.as_ref()
                .map(typed_value_to_json)
                .unwrap_or(serde_json::Value::Null);
            Update { path, value }
        }).collect(),
        deletes: n.delete.iter().map(GnmiPath::from).collect(),
    }
}

fn update_result_from_proto(r: &pb::UpdateResult) -> UpdateResult {
    let path = r.path.as_ref()
        .map(GnmiPath::from)
        .unwrap_or_else(|| GnmiPath { elements: vec![], origin: String::new() });
    let op = match pb::Operation::try_from(r.op) {
        Ok(pb::Operation::Delete) => UpdateOp::Delete,
        Ok(pb::Operation::Replace) => UpdateOp::Replace,
        _ => UpdateOp::Update,
    };
    let message = if r.message.is_empty() { None } else { Some(r.message.clone()) };
    UpdateResult { path, op, message }
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

    /// Builds a generated gNMI client from the current channel.
    fn grpc_client(&self) -> Result<pb::g_nmi_client::GNmiClient<Channel>> {
        let channel = self.channel()?.clone();
        Ok(pb::g_nmi_client::GNmiClient::new(channel))
    }

    /// Queries device state at the given gNMI path (Get RPC).
    ///
    /// Returns the response value as a `serde_json::Value`. When the response
    /// contains multiple notifications with multiple updates, they are merged
    /// into a single JSON object. For a single update the value is returned
    /// directly.
    pub async fn get(&self, path: &GnmiPath) -> Result<serde_json::Value> {
        debug!(
            path = %path.to_xpath(),
            endpoint = %self.endpoint,
            "gNMI Get"
        );

        let request = pb::GetRequest {
            path: vec![pb::Path::from(path)],
            encoding: Encoding::JsonIetf.to_proto(),
            prefix: None,
            r#type: pb::DataType::All as i32,
        };

        let mut client = self.grpc_client()?;
        let response = client.get(request).await.map_err(|e| {
            SonicError::Grpc(format!("gNMI Get failed: {e}"))
        })?;

        let resp = response.into_inner();
        let notifications: Vec<Notification> = resp.notification.iter()
            .map(notification_from_proto)
            .collect();

        // Flatten all updates into a single JSON result.
        let mut values: Vec<serde_json::Value> = notifications.iter()
            .flat_map(|n| n.updates.iter().map(|u| u.value.clone()))
            .collect();

        match values.len() {
            0 => Ok(serde_json::Value::Null),
            1 => Ok(values.remove(0)),
            _ => Ok(serde_json::Value::Array(values)),
        }
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

        let proto_update = pb::Update {
            path: Some(pb::Path::from(path)),
            val: Some(json_to_typed_value(&value)),
        };

        let request = pb::SetRequest {
            prefix: None,
            delete: Vec::new(),
            replace: Vec::new(),
            update: vec![proto_update],
        };

        let mut client = self.grpc_client()?;
        let response = client.set(request).await.map_err(|e| {
            SonicError::Grpc(format!("gNMI Set failed: {e}"))
        })?;

        let resp = response.into_inner();
        Ok(SetResponse {
            timestamp: resp.timestamp,
            results: resp.response.iter().map(update_result_from_proto).collect(),
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
        let channel = self.channel()?.clone();

        info!(
            paths = paths.len(),
            mode = ?mode,
            endpoint = %self.endpoint,
            "gNMI Subscribe"
        );

        let (tx, rx) = mpsc::channel(256);

        // Build the proto SubscriptionList mode. Our public SubscriptionMode
        // maps to the proto SubscriptionList.mode for ONCE/STREAM/POLL, while
        // individual Subscription.mode uses SAMPLE by default.
        let list_mode = match mode {
            SubscriptionMode::Once => pb::SubscriptionMode::OnChange as i32,
            SubscriptionMode::Stream => pb::SubscriptionMode::Sample as i32,
            SubscriptionMode::Poll => pb::SubscriptionMode::TargetDefined as i32,
        };

        let subscriptions: Vec<pb::Subscription> = paths
            .iter()
            .map(|p| pb::Subscription {
                path: Some(pb::Path::from(p)),
                mode: pb::SubscriptionMode::Sample as i32,
                sample_interval: 10_000_000_000, // 10 seconds default
            })
            .collect();

        let subscribe_request = pb::SubscribeRequest {
            request: Some(pb::subscribe_request::Request::Subscribe(
                pb::SubscriptionList {
                    prefix: None,
                    subscription: subscriptions,
                    mode: list_mode,
                    encoding: Encoding::JsonIetf.to_proto(),
                },
            )),
        };

        let endpoint = self.endpoint.clone();
        let is_once = mode == SubscriptionMode::Once;

        tokio::spawn(async move {
            debug!(endpoint = %endpoint, "subscription stream task started");

            let mut client = pb::g_nmi_client::GNmiClient::new(channel);

            // The Subscribe RPC is a bidirectional stream. We send a single
            // request and then read responses.
            let (req_tx, req_rx) = mpsc::channel(1);
            if req_tx.send(subscribe_request).await.is_err() {
                warn!("failed to send subscribe request");
                return;
            }
            drop(req_tx); // Close the request stream after sending.

            let stream = tokio_stream::wrappers::ReceiverStream::new(req_rx);
            let result = client.subscribe(stream).await;

            match result {
                Ok(response) => {
                    let mut stream = response.into_inner();
                    loop {
                        match stream.message().await {
                            Ok(Some(msg)) => {
                                let sub_resp = match msg.response {
                                    Some(pb::subscribe_response::Response::Update(notif)) => {
                                        SubscribeResponse {
                                            update: Some(notification_from_proto(&notif)),
                                            sync_response: false,
                                        }
                                    }
                                    Some(pb::subscribe_response::Response::SyncResponse(sync)) => {
                                        SubscribeResponse {
                                            update: None,
                                            sync_response: sync,
                                        }
                                    }
                                    None => continue,
                                };

                                let is_sync = sub_resp.sync_response;
                                if tx.send(sub_resp).await.is_err() {
                                    trace!("subscriber dropped, ending stream");
                                    return;
                                }

                                // For once-mode, close after sync response.
                                if is_once && is_sync {
                                    debug!("once-mode subscription complete");
                                    return;
                                }
                            }
                            Ok(None) => {
                                debug!("subscribe stream closed by server");
                                return;
                            }
                            Err(e) => {
                                warn!(error = %e, "subscribe stream error");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "subscribe RPC failed");
                }
            }
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

    #[test]
    fn proto_path_roundtrip() {
        let path = GnmiPath {
            elements: vec![
                PathElement::new("interfaces"),
                PathElement::with_key("interface", "name", "Ethernet0"),
            ],
            origin: "openconfig".to_owned(),
        };

        let proto: pb::Path = (&path).into();
        assert_eq!(proto.elem.len(), 2);
        assert_eq!(proto.elem[0].name, "interfaces");
        assert_eq!(proto.elem[1].key["name"], "Ethernet0");
        assert_eq!(proto.origin, "openconfig");

        let back = GnmiPath::from(&proto);
        assert_eq!(back.elements.len(), 2);
        assert_eq!(back.origin, "openconfig");
    }

    #[test]
    fn json_to_typed_value_string() {
        let val = serde_json::json!("hello");
        let tv = json_to_typed_value(&val);
        match tv.value {
            Some(pb::typed_value::Value::StringVal(s)) => assert_eq!(s, "hello"),
            other => panic!("expected StringVal, got {other:?}"),
        }
    }

    #[test]
    fn json_to_typed_value_int() {
        let val = serde_json::json!(42);
        let tv = json_to_typed_value(&val);
        match tv.value {
            Some(pb::typed_value::Value::IntVal(n)) => assert_eq!(n, 42),
            other => panic!("expected IntVal, got {other:?}"),
        }
    }

    #[test]
    fn json_to_typed_value_object() {
        let val = serde_json::json!({"key": "value"});
        let tv = json_to_typed_value(&val);
        match tv.value {
            Some(pb::typed_value::Value::JsonIetfVal(bytes)) => {
                let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(parsed["key"], "value");
            }
            other => panic!("expected JsonIetfVal, got {other:?}"),
        }
    }

    #[test]
    fn typed_value_to_json_string() {
        let tv = pb::TypedValue {
            value: Some(pb::typed_value::Value::StringVal("test".to_owned())),
        };
        assert_eq!(typed_value_to_json(&tv), serde_json::json!("test"));
    }

    #[test]
    fn typed_value_to_json_ietf() {
        let obj = serde_json::json!({"enabled": true});
        let bytes = serde_json::to_vec(&obj).unwrap();
        let tv = pb::TypedValue {
            value: Some(pb::typed_value::Value::JsonIetfVal(bytes)),
        };
        let result = typed_value_to_json(&tv);
        assert_eq!(result["enabled"], true);
    }
}
