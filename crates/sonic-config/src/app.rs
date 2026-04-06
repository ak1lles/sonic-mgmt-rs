//! Top-level application configuration (`sonic-mgmt.toml`).
//!
//! [`AppConfig`] is the single root struct that aggregates every tuneable knob
//! the framework exposes: which testbed to target, how to connect, how to run
//! tests, where to send reports, topology generation parameters, and logging.
//!
//! The primary entry-point is [`AppConfig::load`] which reads a TOML file from
//! disk.  [`AppConfig::load_or_default`] falls back to compiled-in defaults
//! when the file does not exist, which is useful for first-run or CI scenarios.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use sonic_core::{AuthMethod, ReportFormat, SonicError, Result};

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// Root application configuration.
///
/// Maps one-to-one with a `sonic-mgmt.toml` file.  Every section is optional
/// and carries sensible defaults so a minimal (or even empty) file is valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Testbed selection.
    pub testbed: TestbedSection,
    /// Connection defaults.
    pub connection: ConnectionSection,
    /// Test execution settings.
    pub testing: TestingSection,
    /// Reporting / analytics backend.
    pub reporting: ReportingSection,
    /// Topology generation parameters.
    pub topology: TopologySection,
    /// Logging configuration.
    pub logging: LoggingSection,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            testbed: TestbedSection::default(),
            connection: ConnectionSection::default(),
            testing: TestingSection::default(),
            reporting: ReportingSection::default(),
            topology: TopologySection::default(),
            logging: LoggingSection::default(),
        }
    }
}

impl AppConfig {
    /// Loads configuration from a TOML file at `path`.
    ///
    /// Returns [`SonicError::Io`] if the file cannot be read and
    /// [`SonicError::TomlDeserialize`] if parsing fails.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        info!(path = %path.display(), "loading application config");

        let contents = std::fs::read_to_string(path).map_err(|e| {
            SonicError::config(format!("failed to read config file {}: {e}", path.display()))
        })?;

        let config: Self = toml::from_str(&contents).map_err(|e| {
            SonicError::config(format!(
                "failed to parse config file {}: {e}",
                path.display()
            ))
        })?;

        debug!(?config, "parsed application config");
        config.validate()?;
        Ok(config)
    }

    /// Loads configuration from `path` if it exists, otherwise returns the
    /// compiled-in defaults.
    ///
    /// This never fails due to a missing file, but *will* propagate parse or
    /// validation errors if the file exists and is malformed.
    pub fn load_or_default(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            Self::load(path)
        } else {
            warn!(
                path = %path.display(),
                "config file not found, using defaults"
            );
            Ok(Self::default())
        }
    }

    /// Merges `other` into `self`.
    ///
    /// Non-default values in `other` overwrite the corresponding fields in
    /// `self`.  This is useful for layering a user-specific override file on
    /// top of a project-level config.
    pub fn merge(&mut self, other: &AppConfig) {
        self.testbed.merge(&other.testbed);
        self.connection.merge(&other.connection);
        self.testing.merge(&other.testing);
        self.reporting.merge(&other.reporting);
        self.topology.merge(&other.topology);
        self.logging.merge(&other.logging);
        debug!("merged application configs");
    }

    /// Validates cross-field constraints that cannot be captured by serde
    /// defaults alone.
    pub fn validate(&self) -> Result<()> {
        self.connection.validate()?;
        self.testing.validate()?;
        self.topology.validate()?;
        self.logging.validate()?;
        debug!("application config validation passed");
        Ok(())
    }

    /// Serializes the configuration to TOML and writes it to `path`.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let contents = toml::to_string_pretty(self)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(path, contents)?;
        info!(path = %path.display(), "saved application config");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Section: testbed
// ---------------------------------------------------------------------------

/// `[testbed]` -- which testbed to target and where to find its definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TestbedSection {
    /// Logical name of the active testbed (e.g. `"vms-t0"`).
    pub active: String,
    /// Path to the testbed definition file (TOML or YAML).
    pub file: PathBuf,
}

impl Default for TestbedSection {
    fn default() -> Self {
        Self {
            active: String::new(),
            file: PathBuf::from("testbed.yaml"),
        }
    }
}

impl TestbedSection {
    fn merge(&mut self, other: &Self) {
        if !other.active.is_empty() {
            self.active.clone_from(&other.active);
        }
        let default_file = PathBuf::from("testbed.yaml");
        if other.file != default_file {
            self.file.clone_from(&other.file);
        }
    }
}

// ---------------------------------------------------------------------------
// Section: connection
// ---------------------------------------------------------------------------

/// Default SSH port.
const DEFAULT_SSH_PORT: u16 = 22;
/// Default connection timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Default number of connection retries.
const DEFAULT_RETRIES: u32 = 3;

/// `[connection]` -- transport-layer defaults applied when a per-device
/// override is not specified.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionSection {
    /// Default SSH port.
    pub default_ssh_port: u16,
    /// Connection timeout in seconds.
    pub timeout_secs: u64,
    /// Number of retries on transient connection failures.
    pub retries: u32,
    /// Path to the default SSH private key.
    pub key_path: Option<PathBuf>,
}

impl Default for ConnectionSection {
    fn default() -> Self {
        Self {
            default_ssh_port: DEFAULT_SSH_PORT,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            retries: DEFAULT_RETRIES,
            key_path: None,
        }
    }
}

impl ConnectionSection {
    fn merge(&mut self, other: &Self) {
        if other.default_ssh_port != DEFAULT_SSH_PORT {
            self.default_ssh_port = other.default_ssh_port;
        }
        if other.timeout_secs != DEFAULT_TIMEOUT_SECS {
            self.timeout_secs = other.timeout_secs;
        }
        if other.retries != DEFAULT_RETRIES {
            self.retries = other.retries;
        }
        if other.key_path.is_some() {
            self.key_path.clone_from(&other.key_path);
        }
    }

    fn validate(&self) -> Result<()> {
        if self.default_ssh_port == 0 {
            return Err(SonicError::ConfigValidation {
                path: "connection.default_ssh_port".into(),
                reason: "port must be non-zero".into(),
            });
        }
        if self.timeout_secs == 0 {
            return Err(SonicError::ConfigValidation {
                path: "connection.timeout_secs".into(),
                reason: "timeout must be positive".into(),
            });
        }
        if let Some(ref key) = self.key_path {
            if !key.as_os_str().is_empty() && !key.exists() {
                warn!(key = %key.display(), "configured SSH key file does not exist");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Section: testing
// ---------------------------------------------------------------------------

/// Default number of parallel test workers.
const DEFAULT_WORKERS: usize = 1;
/// Default per-test timeout in seconds.
const DEFAULT_TEST_TIMEOUT_SECS: u64 = 900;

/// `[testing]` -- test-runner tunables.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TestingSection {
    /// Number of parallel test workers.
    pub parallel_workers: usize,
    /// Per-test timeout in seconds.
    pub timeout_secs: u64,
    /// Directory for test artefacts (logs, captures, reports).
    pub output_dir: PathBuf,
    /// Report output format.
    pub report_format: ReportFormat,
}

impl Default for TestingSection {
    fn default() -> Self {
        Self {
            parallel_workers: DEFAULT_WORKERS,
            timeout_secs: DEFAULT_TEST_TIMEOUT_SECS,
            output_dir: PathBuf::from("output"),
            report_format: ReportFormat::JunitXml,
        }
    }
}

impl TestingSection {
    fn merge(&mut self, other: &Self) {
        if other.parallel_workers != DEFAULT_WORKERS {
            self.parallel_workers = other.parallel_workers;
        }
        if other.timeout_secs != DEFAULT_TEST_TIMEOUT_SECS {
            self.timeout_secs = other.timeout_secs;
        }
        let default_output = PathBuf::from("output");
        if other.output_dir != default_output {
            self.output_dir.clone_from(&other.output_dir);
        }
        if other.report_format != ReportFormat::JunitXml {
            self.report_format = other.report_format;
        }
    }

    fn validate(&self) -> Result<()> {
        if self.parallel_workers == 0 {
            return Err(SonicError::ConfigValidation {
                path: "testing.parallel_workers".into(),
                reason: "at least one worker is required".into(),
            });
        }
        if self.timeout_secs == 0 {
            return Err(SonicError::ConfigValidation {
                path: "testing.timeout_secs".into(),
                reason: "timeout must be positive".into(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Section: reporting
// ---------------------------------------------------------------------------

/// `[reporting]` -- external analytics / results backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportingSection {
    /// Base URL of the reporting backend (e.g. Kusto, Elasticsearch).
    pub backend_url: Option<String>,
    /// Authentication method for the backend.
    pub auth_method: AuthMethod,
    /// Database / index name.
    pub database: String,
    /// Table / collection name.
    pub table: String,
}

impl Default for ReportingSection {
    fn default() -> Self {
        Self {
            backend_url: None,
            auth_method: AuthMethod::AzureDefault,
            database: String::new(),
            table: String::new(),
        }
    }
}

impl ReportingSection {
    fn merge(&mut self, other: &Self) {
        if other.backend_url.is_some() {
            self.backend_url.clone_from(&other.backend_url);
        }
        if other.auth_method != AuthMethod::AzureDefault {
            self.auth_method = other.auth_method;
        }
        if !other.database.is_empty() {
            self.database.clone_from(&other.database);
        }
        if !other.table.is_empty() {
            self.table.clone_from(&other.table);
        }
    }
}

// ---------------------------------------------------------------------------
// Section: topology
// ---------------------------------------------------------------------------

/// Default VM base IP (10.250.0.0-style).
const DEFAULT_VM_BASE_IP: &str = "10.250.0.2";
/// Default starting VLAN ID for topo-generated VLANs.
const DEFAULT_VLAN_BASE: u16 = 1000;
/// Default IP address offset for numbering VM interfaces.
const DEFAULT_IP_OFFSET: u32 = 1;
/// Maximum valid VLAN ID per 802.1Q.
const MAX_VLAN_ID: u16 = 4094;

/// `[topology]` -- parameters that feed the topology generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TopologySection {
    /// Base management IP for neighbor VMs.
    pub vm_base_ip: String,
    /// Starting VLAN ID for auto-generated VLANs.
    pub vlan_base: u16,
    /// Starting IP offset for VM interface addressing.
    pub ip_offset: u32,
}

impl Default for TopologySection {
    fn default() -> Self {
        Self {
            vm_base_ip: DEFAULT_VM_BASE_IP.to_owned(),
            vlan_base: DEFAULT_VLAN_BASE,
            ip_offset: DEFAULT_IP_OFFSET,
        }
    }
}

impl TopologySection {
    fn merge(&mut self, other: &Self) {
        if other.vm_base_ip != DEFAULT_VM_BASE_IP {
            self.vm_base_ip.clone_from(&other.vm_base_ip);
        }
        if other.vlan_base != DEFAULT_VLAN_BASE {
            self.vlan_base = other.vlan_base;
        }
        if other.ip_offset != DEFAULT_IP_OFFSET {
            self.ip_offset = other.ip_offset;
        }
    }

    fn validate(&self) -> Result<()> {
        // Validate that vm_base_ip is a parsable IPv4 address.
        if self.vm_base_ip.parse::<std::net::Ipv4Addr>().is_err() {
            return Err(SonicError::ConfigValidation {
                path: "topology.vm_base_ip".into(),
                reason: format!("`{}` is not a valid IPv4 address", self.vm_base_ip),
            });
        }
        if self.vlan_base == 0 || self.vlan_base > MAX_VLAN_ID {
            return Err(SonicError::ConfigValidation {
                path: "topology.vlan_base".into(),
                reason: format!("vlan_base must be in 1..={MAX_VLAN_ID}"),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Section: logging
// ---------------------------------------------------------------------------

/// `[logging]` -- tracing / log output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingSection {
    /// tracing directive string (e.g. `"info"`, `"sonic_config=debug,warn"`).
    pub level: String,
    /// Optional path to a log file.  When `None`, logs go to stderr only.
    pub file: Option<PathBuf>,
    /// Output format: `"full"`, `"compact"`, `"pretty"`, or `"json"`.
    pub format: LogFormat,
}

impl Default for LoggingSection {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            file: None,
            format: LogFormat::Full,
        }
    }
}

impl LoggingSection {
    fn merge(&mut self, other: &Self) {
        if other.level != "info" {
            self.level.clone_from(&other.level);
        }
        if other.file.is_some() {
            self.file.clone_from(&other.file);
        }
        if other.format != LogFormat::Full {
            self.format = other.format;
        }
    }

    fn validate(&self) -> Result<()> {
        // We intentionally do *not* reject unknown tracing directives here
        // because the directive syntax is rich and best validated by the
        // tracing-subscriber layer at initialization time.  We do reject an
        // entirely empty string, which would silence all output.
        if self.level.is_empty() {
            return Err(SonicError::ConfigValidation {
                path: "logging.level".into(),
                reason: "log level must not be empty".into(),
            });
        }
        Ok(())
    }
}

/// Supported log output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// `tracing_subscriber::fmt::format::Full` -- the default multi-line format.
    Full,
    /// Single-line compact format.
    Compact,
    /// Coloured, human-friendly format.
    Pretty,
    /// Machine-readable JSON (one object per event).
    Json,
}

impl Default for LogFormat {
    fn default() -> Self {
        Self::Full
    }
}

impl fmt::Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => f.write_str("full"),
            Self::Compact => f.write_str("compact"),
            Self::Pretty => f.write_str("pretty"),
            Self::Json => f.write_str("json"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        let cfg = AppConfig::default();
        cfg.validate().expect("default config should be valid");
    }

    #[test]
    fn roundtrip_toml() {
        let cfg = AppConfig::default();
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: AppConfig = toml::from_str(&serialized).unwrap();
        deserialized.validate().unwrap();
        assert_eq!(cfg.connection.default_ssh_port, deserialized.connection.default_ssh_port);
    }

    #[test]
    fn merge_overrides_non_defaults() {
        let mut base = AppConfig::default();
        let mut overlay = AppConfig::default();
        overlay.connection.timeout_secs = 60;
        overlay.testing.parallel_workers = 4;
        overlay.topology.vlan_base = 2000;

        base.merge(&overlay);

        assert_eq!(base.connection.timeout_secs, 60);
        assert_eq!(base.testing.parallel_workers, 4);
        assert_eq!(base.topology.vlan_base, 2000);
        // Unchanged fields stay at defaults.
        assert_eq!(base.connection.default_ssh_port, DEFAULT_SSH_PORT);
    }

    #[test]
    fn validate_rejects_zero_workers() {
        let mut cfg = AppConfig::default();
        cfg.testing.parallel_workers = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_bad_vm_base_ip() {
        let mut cfg = AppConfig::default();
        cfg.topology.vm_base_ip = "not-an-ip".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_vlan_base() {
        let mut cfg = AppConfig::default();
        cfg.topology.vlan_base = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_log_level() {
        let mut cfg = AppConfig::default();
        cfg.logging.level = String::new();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn load_or_default_returns_default_for_missing_file() {
        let cfg = AppConfig::load_or_default("/nonexistent/sonic-mgmt.toml").unwrap();
        assert_eq!(cfg.connection.default_ssh_port, DEFAULT_SSH_PORT);
    }
}
