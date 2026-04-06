//! Testbed configuration -- the Rust equivalent of `testbed.yaml`.
//!
//! A *testbed* ties together a topology type, a PTF container, one or more DUTs
//! (devices-under-test), and the neighbor VMs that simulate an upstream /
//! downstream network.  The canonical upstream format is YAML but this module
//! transparently supports TOML as well, choosing the parser based on the file
//! extension.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use sonic_core::{
    Credentials, Platform, SonicError, Result, TopologyType, VmType,
};

// ---------------------------------------------------------------------------
// TestbedConfig
// ---------------------------------------------------------------------------

/// A single testbed definition -- one row in the original `testbed.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestbedConfig {
    /// Unique name of this testbed (e.g. `"vms-t0"`).
    pub name: String,

    /// Logical group the testbed belongs to (used for CI partitioning).
    #[serde(default)]
    pub group: String,

    /// Topology type that should be deployed.
    pub topo: TopologyType,

    /// Docker image used for the PTF container.
    #[serde(default = "default_ptf_image")]
    pub ptf_image: String,

    /// Management IP of the PTF container.
    #[serde(default)]
    pub ptf_ip: Option<IpAddr>,

    /// The physical server (hypervisor) hosting VMs for this testbed.
    #[serde(default)]
    pub server: String,

    /// Base name for neighbor VMs (e.g. `"VM0100"`).
    #[serde(default)]
    pub vm_base: String,

    /// DUT configurations -- one entry per switch under test.
    #[serde(default)]
    pub duts: Vec<DutConfig>,

    /// Neighbor VM configurations.
    #[serde(default)]
    pub neighbors: Vec<NeighborConfig>,

    /// Free-form comment / description.
    #[serde(default)]
    pub comment: String,

    /// Fanout switch configurations.
    #[serde(default)]
    pub fanouts: Vec<FanoutConfig>,

    /// Physical wiring between DUT ports, fanout ports, and PTF interfaces.
    #[serde(default)]
    pub connection_graph: Vec<PhysicalLink>,

    /// Arbitrary key-value metadata attached to the testbed.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_ptf_image() -> String {
    "docker-ptf".to_owned()
}

impl TestbedConfig {
    /// Validates internal consistency of the testbed definition.
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(SonicError::ConfigValidation {
                path: "testbed.name".into(),
                reason: "testbed name must not be empty".into(),
            });
        }

        if self.duts.is_empty() {
            return Err(SonicError::ConfigValidation {
                path: format!("testbed[{}].duts", self.name),
                reason: "testbed must define at least one DUT".into(),
            });
        }

        // Enforce unique DUT hostnames within a testbed.
        let mut seen_hostnames = std::collections::HashSet::new();
        for dut in &self.duts {
            if !seen_hostnames.insert(&dut.hostname) {
                return Err(SonicError::ConfigValidation {
                    path: format!("testbed[{}].duts", self.name),
                    reason: format!("duplicate DUT hostname `{}`", dut.hostname),
                });
            }
            dut.validate(&self.name)?;
        }

        // If the topology requires VMs, ensure vm_base is set.
        if self.topo.requires_vms() && self.vm_base.is_empty() {
            return Err(SonicError::ConfigValidation {
                path: format!("testbed[{}].vm_base", self.name),
                reason: format!(
                    "topology `{}` requires VMs but vm_base is empty",
                    self.topo
                ),
            });
        }

        // Validate neighbor entries.
        for neighbor in &self.neighbors {
            neighbor.validate(&self.name)?;
        }

        // Validate fanout entries.
        let mut seen_fanouts = std::collections::HashSet::new();
        for fanout in &self.fanouts {
            if !seen_fanouts.insert(&fanout.hostname) {
                return Err(SonicError::ConfigValidation {
                    path: format!("testbed[{}].fanouts", self.name),
                    reason: format!("duplicate fanout hostname `{}`", fanout.hostname),
                });
            }
            fanout.validate(&self.name)?;
        }

        // Validate connection graph references.
        let fanout_names: std::collections::HashSet<&str> =
            self.fanouts.iter().map(|f| f.hostname.as_str()).collect();
        for link in &self.connection_graph {
            if !fanout_names.is_empty() && !fanout_names.contains(link.fanout_host.as_str()) {
                return Err(SonicError::ConfigValidation {
                    path: format!("testbed[{}].connection_graph", self.name),
                    reason: format!(
                        "link references fanout `{}` which is not defined in fanouts",
                        link.fanout_host,
                    ),
                });
            }
        }

        debug!(testbed = %self.name, "testbed config validation passed");
        Ok(())
    }

    /// Returns the first DUT, which is the primary switch in single-DUT
    /// topologies.
    pub fn primary_dut(&self) -> Option<&DutConfig> {
        self.duts.first()
    }

    /// Returns the number of VMs the topology expects.
    pub fn expected_vm_count(&self) -> usize {
        self.topo.vm_count()
    }
}

// ---------------------------------------------------------------------------
// DutConfig
// ---------------------------------------------------------------------------

/// Configuration for a single device-under-test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DutConfig {
    /// Hostname (as it appears in the SONiC `DEVICE_METADATA`).
    pub hostname: String,

    /// Management IP address.
    pub mgmt_ip: IpAddr,

    /// Hardware SKU string (e.g. `"ACS-MSN2700"`).
    #[serde(default)]
    pub hwsku: String,

    /// Platform / ASIC vendor.
    #[serde(default = "default_platform")]
    pub platform: Platform,

    /// Login credentials for the DUT.
    #[serde(default = "default_credentials")]
    pub credentials: DutCredentials,

    /// Arbitrary per-DUT metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_platform() -> Platform {
    Platform::Unknown
}

impl DutConfig {
    fn validate(&self, testbed_name: &str) -> Result<()> {
        if self.hostname.is_empty() {
            return Err(SonicError::ConfigValidation {
                path: format!("testbed[{testbed_name}].dut.hostname"),
                reason: "DUT hostname must not be empty".into(),
            });
        }
        Ok(())
    }
}

/// Credentials embedded in a DUT definition.
///
/// This is intentionally a simpler struct than `sonic_core::Credentials` so
/// that config files stay concise; it can be converted into the core type when
/// building a `DeviceInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DutCredentials {
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_path: Option<PathBuf>,
}

impl Default for DutCredentials {
    fn default() -> Self {
        Self {
            username: "admin".to_owned(),
            password: None,
            key_path: None,
        }
    }
}

fn default_credentials() -> DutCredentials {
    DutCredentials::default()
}

impl From<DutCredentials> for Credentials {
    fn from(dc: DutCredentials) -> Self {
        let mut creds = Credentials::new(dc.username);
        if let Some(pw) = dc.password {
            creds = creds.with_password(pw);
        }
        if let Some(key) = dc.key_path {
            creds = creds.with_key(key.to_string_lossy().to_string());
        }
        creds
    }
}

// ---------------------------------------------------------------------------
// NeighborConfig
// ---------------------------------------------------------------------------

/// Configuration for a simulated neighbor VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborConfig {
    /// Hostname of the neighbor VM.
    pub hostname: String,

    /// The virtualisation type for this neighbor.
    #[serde(default = "default_vm_type")]
    pub vm_type: VmType,

    /// Management IP address.
    #[serde(default)]
    pub mgmt_ip: Option<IpAddr>,

    /// Arbitrary per-neighbor metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_vm_type() -> VmType {
    VmType::Veos
}

impl NeighborConfig {
    fn validate(&self, testbed_name: &str) -> Result<()> {
        if self.hostname.is_empty() {
            return Err(SonicError::ConfigValidation {
                path: format!("testbed[{testbed_name}].neighbor.hostname"),
                reason: "neighbor hostname must not be empty".into(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FanoutConfig
// ---------------------------------------------------------------------------

/// Configuration for a fanout switch in the testbed.
///
/// Fanout switches sit between the tester server and the DUT, using VLANs
/// to map individual tester ports to DUT front-panel ports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanoutConfig {
    /// Hostname of the fanout switch.
    pub hostname: String,

    /// Management IP address.
    pub mgmt_ip: IpAddr,

    /// Platform / ASIC vendor.
    #[serde(default = "default_platform")]
    pub platform: Platform,

    /// Hardware SKU string.
    #[serde(default)]
    pub hwsku: String,

    /// Login credentials.
    #[serde(default = "default_credentials")]
    pub credentials: DutCredentials,

    /// Arbitrary per-fanout metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl FanoutConfig {
    fn validate(&self, testbed_name: &str) -> Result<()> {
        if self.hostname.is_empty() {
            return Err(SonicError::ConfigValidation {
                path: format!("testbed[{testbed_name}].fanout.hostname"),
                reason: "fanout hostname must not be empty".into(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PhysicalLink (connection graph)
// ---------------------------------------------------------------------------

/// A single physical link in the connection graph.
///
/// Maps one DUT front-panel port through a fanout switch to a PTF
/// test interface. The fanout uses the VLAN ID to isolate traffic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalLink {
    /// DUT front-panel port name (e.g. `"Ethernet0"`).
    pub dut_port: String,

    /// Hostname of the fanout switch carrying this link.
    pub fanout_host: String,

    /// Fanout switch port name (e.g. `"Ethernet1"`).
    pub fanout_port: String,

    /// PTF interface name (e.g. `"eth0"`).
    #[serde(default)]
    pub ptf_port: String,

    /// VLAN ID used on the fanout for this link.
    #[serde(default)]
    pub vlan_id: u16,
}

// ---------------------------------------------------------------------------
// Loading helpers
// ---------------------------------------------------------------------------

/// Detects whether `path` is YAML or TOML based on its extension and
/// deserializes accordingly.
fn deserialize_from_path<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = std::fs::read_to_string(path)?;

    match path.extension().and_then(|e| e.to_str()) {
        Some("yaml" | "yml") => {
            serde_yaml::from_str(&contents).map_err(|e| {
                SonicError::config(format!(
                    "failed to parse YAML file {}: {e}",
                    path.display()
                ))
            })
        }
        Some("toml") => {
            toml::from_str(&contents).map_err(|e| {
                SonicError::config(format!(
                    "failed to parse TOML file {}: {e}",
                    path.display()
                ))
            })
        }
        other => {
            // Fall back: try YAML first, then TOML.
            let ext = other.unwrap_or("(none)");
            warn!(
                path = %path.display(),
                extension = ext,
                "unrecognised extension, attempting YAML then TOML"
            );

            serde_yaml::from_str(&contents)
                .or_else(|_| {
                    toml::from_str(&contents).map_err(|e| {
                        SonicError::config(format!(
                            "failed to parse {} as either YAML or TOML: {e}",
                            path.display()
                        ))
                    })
                })
        }
    }
}

/// Loads a single testbed definition from `path`.
///
/// The file may be TOML or YAML.  When the file contains a *list* of testbeds
/// (as in the upstream `testbed.yaml`), all entries are returned.
pub fn load_testbed(path: impl AsRef<Path>) -> Result<Vec<TestbedConfig>> {
    let path = path.as_ref();
    info!(path = %path.display(), "loading testbed config");

    // Try to deserialize as a list first (the upstream format is a YAML
    // list), then fall back to a single-object document.
    let testbeds: Vec<TestbedConfig> =
        match deserialize_from_path::<Vec<TestbedConfig>>(path) {
            Ok(list) => list,
            Err(_) => {
                // Single testbed document.
                let single: TestbedConfig = deserialize_from_path(path)?;
                vec![single]
            }
        };

    if testbeds.is_empty() {
        return Err(SonicError::config(format!(
            "testbed file {} contains no entries",
            path.display()
        )));
    }

    for tb in &testbeds {
        tb.validate()?;
    }

    info!(
        count = testbeds.len(),
        path = %path.display(),
        "loaded testbed configs"
    );
    Ok(testbeds)
}

/// Scans `dir_path` for testbed files (`.yaml`, `.yml`, `.toml`) and loads all
/// of them, returning a flat list of every testbed definition found.
pub fn load_all_testbeds(dir_path: impl AsRef<Path>) -> Result<Vec<TestbedConfig>> {
    let dir_path = dir_path.as_ref();
    info!(dir = %dir_path.display(), "scanning for testbed files");

    if !dir_path.is_dir() {
        return Err(SonicError::config(format!(
            "{} is not a directory",
            dir_path.display()
        )));
    }

    let mut all_testbeds = Vec::new();

    for extension in &["yaml", "yml", "toml"] {
        let pattern = format!("{}/*.{extension}", dir_path.display());
        let paths: Vec<PathBuf> = glob::glob(&pattern)
            .map_err(|e| SonicError::config(format!("bad glob pattern: {e}")))?
            .filter_map(|entry| entry.ok())
            .collect();

        for path in paths {
            match load_testbed(&path) {
                Ok(mut tbs) => all_testbeds.append(&mut tbs),
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping testbed file that failed to parse"
                    );
                }
            }
        }
    }

    info!(count = all_testbeds.len(), "loaded all testbed configs");
    Ok(all_testbeds)
}

/// Convenience wrapper: validates a [`TestbedConfig`] in isolation, returning
/// a rich error on failure.
pub fn validate_testbed(testbed: &TestbedConfig) -> Result<()> {
    testbed.validate()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn sample_dut() -> DutConfig {
        DutConfig {
            hostname: "dut-1".into(),
            mgmt_ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            hwsku: "ACS-MSN2700".into(),
            platform: Platform::Mellanox,
            credentials: DutCredentials::default(),
            metadata: HashMap::new(),
        }
    }

    fn sample_testbed() -> TestbedConfig {
        TestbedConfig {
            name: "vms-t0".into(),
            group: "vms".into(),
            topo: TopologyType::T0,
            ptf_image: default_ptf_image(),
            ptf_ip: Some(IpAddr::V4(Ipv4Addr::new(10, 255, 0, 188))),
            server: "server-1".into(),
            vm_base: "VM0100".into(),
            duts: vec![sample_dut()],
            neighbors: vec![],
            fanouts: vec![],
            connection_graph: vec![],
            comment: "unit test".into(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn valid_testbed_passes() {
        sample_testbed().validate().expect("should be valid");
    }

    #[test]
    fn empty_name_rejected() {
        let mut tb = sample_testbed();
        tb.name = String::new();
        assert!(tb.validate().is_err());
    }

    #[test]
    fn no_duts_rejected() {
        let mut tb = sample_testbed();
        tb.duts.clear();
        assert!(tb.validate().is_err());
    }

    #[test]
    fn duplicate_dut_hostnames_rejected() {
        let mut tb = sample_testbed();
        tb.duts.push(sample_dut());
        assert!(tb.validate().is_err());
    }

    #[test]
    fn vm_topo_without_vm_base_rejected() {
        let mut tb = sample_testbed();
        tb.vm_base = String::new();
        assert!(tb.validate().is_err());
    }

    #[test]
    fn ptf_topo_without_vm_base_ok() {
        let mut tb = sample_testbed();
        tb.topo = TopologyType::Ptf;
        tb.vm_base = String::new();
        tb.validate().expect("PTF topo does not need vm_base");
    }

    #[test]
    fn dut_credentials_convert_to_core() {
        let dc = DutCredentials {
            username: "testuser".into(),
            password: Some("secret".into()),
            key_path: Some(PathBuf::from("/home/user/.ssh/id_rsa")),
        };
        let creds: Credentials = dc.into();
        assert_eq!(creds.username, "testuser");
        assert_eq!(creds.password.as_deref(), Some("secret"));
        assert_eq!(creds.key_path.as_deref(), Some("/home/user/.ssh/id_rsa"));
    }

    #[test]
    fn toml_roundtrip() {
        let tb = sample_testbed();
        let serialized = toml::to_string_pretty(&tb).unwrap();
        let deserialized: TestbedConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(tb.name, deserialized.name);
        assert_eq!(tb.duts.len(), deserialized.duts.len());
        deserialized.validate().unwrap();
    }

    #[test]
    fn primary_dut_returns_first() {
        let tb = sample_testbed();
        let primary = tb.primary_dut().unwrap();
        assert_eq!(primary.hostname, "dut-1");
    }
}
