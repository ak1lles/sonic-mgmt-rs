use std::net::IpAddr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SonicError {
    #[error("connection failed to {host}: {reason}")]
    Connection { host: String, reason: String },

    #[error("authentication failed for {user}@{host}")]
    Authentication { user: String, host: String },

    #[error("command `{command}` failed (exit {exit_code}): {stderr}")]
    CommandExecution {
        command: String,
        exit_code: i32,
        stderr: String,
    },

    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("invalid configuration at `{path}`: {reason}")]
    ConfigValidation { path: String, reason: String },

    #[error("testbed error: {0}")]
    Testbed(String),

    #[error("testbed `{name}` not found")]
    TestbedNotFound { name: String },

    #[error("topology error: {0}")]
    Topology(String),

    #[error("unsupported topology type: {0}")]
    UnsupportedTopology(String),

    #[error("test error: {0}")]
    Test(String),

    #[error("test `{name}` failed: {reason}")]
    TestFailure { name: String, reason: String },

    #[error("timeout after {seconds}s waiting for {operation}")]
    Timeout { seconds: u64, operation: String },

    #[error("device `{0}` not found")]
    DeviceNotFound(String),

    #[error("device `{host}` not reachable at {ip}")]
    DeviceUnreachable { host: String, ip: IpAddr },

    #[error("feature `{feature}` not supported on {device}")]
    FeatureNotSupported { feature: String, device: String },

    #[error("insufficient resources: {0}")]
    InsufficientResources(String),

    #[error("port `{port}` not found on device `{device}`")]
    PortNotFound { port: String, device: String },

    #[error("VLAN {vlan_id} conflict: {reason}")]
    VlanConflict { vlan_id: u16, reason: String },

    #[error("IP allocation exhausted for subnet {0}")]
    IpExhausted(String),

    #[error("report parse error: {0}")]
    ReportParse(String),

    #[error("upload failed to {destination}: {reason}")]
    Upload { destination: String, reason: String },

    #[error("gRPC error: {0}")]
    Grpc(String),

    #[error("SSH error: {0}")]
    Ssh(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML deserialization error: {0}")]
    TomlDeserialize(#[from] toml::de::Error),

    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SonicError>;

impl SonicError {
    pub fn connection(host: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Connection {
            host: host.into(),
            reason: reason.into(),
        }
    }

    pub fn timeout(seconds: u64, operation: impl Into<String>) -> Self {
        Self::Timeout {
            seconds,
            operation: operation.into(),
        }
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Configuration(msg.into())
    }

    pub fn testbed(msg: impl Into<String>) -> Self {
        Self::Testbed(msg.into())
    }

    pub fn topology(msg: impl Into<String>) -> Self {
        Self::Topology(msg.into())
    }

    pub fn test(msg: impl Into<String>) -> Self {
        Self::Test(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
