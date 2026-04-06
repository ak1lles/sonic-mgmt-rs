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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helper constructors
    // -----------------------------------------------------------------------

    #[test]
    fn connection_helper() {
        let err = SonicError::connection("switch-01", "refused");
        match &err {
            SonicError::Connection { host, reason } => {
                assert_eq!(host, "switch-01");
                assert_eq!(reason, "refused");
            }
            other => panic!("expected Connection, got {:?}", other),
        }
    }

    #[test]
    fn timeout_helper() {
        let err = SonicError::timeout(30, "BGP convergence");
        match &err {
            SonicError::Timeout { seconds, operation } => {
                assert_eq!(*seconds, 30);
                assert_eq!(operation, "BGP convergence");
            }
            other => panic!("expected Timeout, got {:?}", other),
        }
    }

    #[test]
    fn config_helper() {
        let err = SonicError::config("missing field");
        match &err {
            SonicError::Configuration(msg) => assert_eq!(msg, "missing field"),
            other => panic!("expected Configuration, got {:?}", other),
        }
    }

    #[test]
    fn testbed_helper() {
        let err = SonicError::testbed("no VMs available");
        match &err {
            SonicError::Testbed(msg) => assert_eq!(msg, "no VMs available"),
            other => panic!("expected Testbed, got {:?}", other),
        }
    }

    #[test]
    fn topology_helper() {
        let err = SonicError::topology("invalid link");
        match &err {
            SonicError::Topology(msg) => assert_eq!(msg, "invalid link"),
            other => panic!("expected Topology, got {:?}", other),
        }
    }

    #[test]
    fn test_helper() {
        let err = SonicError::test("assertion failed");
        match &err {
            SonicError::Test(msg) => assert_eq!(msg, "assertion failed"),
            other => panic!("expected Test, got {:?}", other),
        }
    }

    #[test]
    fn other_helper() {
        let err = SonicError::other("something unexpected");
        match &err {
            SonicError::Other(msg) => assert_eq!(msg, "something unexpected"),
            other => panic!("expected Other, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Display messages
    // -----------------------------------------------------------------------

    #[test]
    fn display_connection() {
        let err = SonicError::connection("host1", "timed out");
        let msg = err.to_string();
        assert!(msg.contains("host1"), "expected host in message: {msg}");
        assert!(msg.contains("timed out"), "expected reason in message: {msg}");
    }

    #[test]
    fn display_timeout() {
        let err = SonicError::timeout(60, "reboot");
        let msg = err.to_string();
        assert!(msg.contains("60"), "expected seconds in message: {msg}");
        assert!(msg.contains("reboot"), "expected operation in message: {msg}");
    }

    #[test]
    fn display_config() {
        let err = SonicError::config("bad value");
        let msg = err.to_string();
        assert!(msg.contains("bad value"), "expected detail in message: {msg}");
        assert!(msg.contains("configuration"), "expected 'configuration' in message: {msg}");
    }

    #[test]
    fn display_testbed() {
        let err = SonicError::testbed("locked");
        let msg = err.to_string();
        assert!(msg.contains("locked"), "expected detail in message: {msg}");
        assert!(msg.contains("testbed"), "expected 'testbed' in message: {msg}");
    }

    #[test]
    fn display_topology() {
        let err = SonicError::topology("cycle detected");
        let msg = err.to_string();
        assert!(msg.contains("cycle detected"), "expected detail in message: {msg}");
        assert!(msg.contains("topology"), "expected 'topology' in message: {msg}");
    }

    #[test]
    fn display_test() {
        let err = SonicError::test("flaky");
        let msg = err.to_string();
        assert!(msg.contains("flaky"), "expected detail in message: {msg}");
        assert!(msg.contains("test"), "expected 'test' in message: {msg}");
    }

    #[test]
    fn display_other() {
        let err = SonicError::other("unknown issue");
        let msg = err.to_string();
        assert_eq!(msg, "unknown issue");
    }

    // -----------------------------------------------------------------------
    // From impls
    // -----------------------------------------------------------------------

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let sonic_err: SonicError = io_err.into();
        match &sonic_err {
            SonicError::Io(_) => {}
            other => panic!("expected Io variant, got {:?}", other),
        }
        let msg = sonic_err.to_string();
        assert!(msg.contains("file missing"), "expected io detail in message: {msg}");
    }

    #[test]
    fn from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let sonic_err: SonicError = json_err.into();
        match &sonic_err {
            SonicError::Json(_) => {}
            other => panic!("expected Json variant, got {:?}", other),
        }
    }
}
