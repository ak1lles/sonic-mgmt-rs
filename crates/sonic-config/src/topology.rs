//! Topology configuration templates.
//!
//! A *topology config* describes the structure of a testbed topology: which VMs
//! to spin up, which VLANs to create, how host interfaces map to DUT ports, and
//! any additional configuration properties (BGP AS numbers, IP prefixes, etc.).
//!
//! These templates are the input to the topology *generator* (in the
//! `sonic-topology` crate) which resolves them into concrete
//! [`TopologyDefinition`](sonic_core::TopologyDefinition) objects.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use sonic_core::{Result, SonicError, TopologyType, VmType};

// ---------------------------------------------------------------------------
// TopologyConfig
// ---------------------------------------------------------------------------

/// Maximum valid VLAN ID per IEEE 802.1Q.
const MAX_VLAN_ID: u16 = 4094;

/// A topology template that describes the logical structure of a test
/// environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyConfig {
    /// The topology type this template defines.
    #[serde(rename = "type")]
    pub topo_type: TopologyType,

    /// VM definitions for the neighbor switches.
    #[serde(default)]
    pub vms: Vec<VmConfig>,

    /// VLAN definitions used by the topology.
    #[serde(default)]
    pub vlans: Vec<VlanConfig>,

    /// Host interface mappings (PTF port <-> DUT port).
    #[serde(default)]
    pub host_interfaces: Vec<HostInterfaceConfig>,

    /// Arbitrary configuration properties keyed by name (e.g. BGP AS numbers,
    /// prefix lists, timers).
    #[serde(default)]
    pub configuration_properties: HashMap<String, ConfigProperty>,
}

impl TopologyConfig {
    /// Validates the topology template.
    pub fn validate(&self) -> Result<()> {
        self.validate_vms()?;
        self.validate_vlans()?;
        self.validate_host_interfaces()?;

        debug!(topo_type = %self.topo_type, "topology config validation passed");
        Ok(())
    }

    fn validate_vms(&self) -> Result<()> {
        let mut seen_names = std::collections::HashSet::new();
        for (idx, vm) in self.vms.iter().enumerate() {
            if vm.name.is_empty() {
                return Err(SonicError::ConfigValidation {
                    path: format!("topology.vms[{idx}].name"),
                    reason: "VM name must not be empty".into(),
                });
            }
            if !seen_names.insert(&vm.name) {
                return Err(SonicError::ConfigValidation {
                    path: format!("topology.vms[{idx}].name"),
                    reason: format!("duplicate VM name `{}`", vm.name),
                });
            }
            // Ensure all VLAN references in a VM exist in the top-level vlans list.
            for vlan_id in &vm.vlans {
                if !self.vlans.iter().any(|v| v.id == *vlan_id) {
                    return Err(SonicError::ConfigValidation {
                        path: format!("topology.vms[{}].vlans", vm.name),
                        reason: format!(
                            "VM `{}` references VLAN {vlan_id} which is not defined",
                            vm.name
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_vlans(&self) -> Result<()> {
        let mut seen_ids = std::collections::HashSet::new();
        for (idx, vlan) in self.vlans.iter().enumerate() {
            if vlan.id == 0 || vlan.id > MAX_VLAN_ID {
                return Err(SonicError::ConfigValidation {
                    path: format!("topology.vlans[{idx}].id"),
                    reason: format!("VLAN ID must be in 1..={MAX_VLAN_ID}, got {}", vlan.id),
                });
            }
            if !seen_ids.insert(vlan.id) {
                return Err(SonicError::ConfigValidation {
                    path: format!("topology.vlans[{idx}].id"),
                    reason: format!("duplicate VLAN ID {}", vlan.id),
                });
            }
        }
        Ok(())
    }

    fn validate_host_interfaces(&self) -> Result<()> {
        let mut seen_dut_ports = std::collections::HashSet::new();
        let mut seen_ptf_ports = std::collections::HashSet::new();

        for (idx, hi) in self.host_interfaces.iter().enumerate() {
            if hi.dut_port.is_empty() {
                return Err(SonicError::ConfigValidation {
                    path: format!("topology.host_interfaces[{idx}].dut_port"),
                    reason: "dut_port must not be empty".into(),
                });
            }
            if hi.ptf_port.is_empty() {
                return Err(SonicError::ConfigValidation {
                    path: format!("topology.host_interfaces[{idx}].ptf_port"),
                    reason: "ptf_port must not be empty".into(),
                });
            }
            if !seen_dut_ports.insert(&hi.dut_port) {
                return Err(SonicError::ConfigValidation {
                    path: format!("topology.host_interfaces[{idx}].dut_port"),
                    reason: format!("duplicate DUT port `{}`", hi.dut_port),
                });
            }
            if !seen_ptf_ports.insert(&hi.ptf_port) {
                return Err(SonicError::ConfigValidation {
                    path: format!("topology.host_interfaces[{idx}].ptf_port"),
                    reason: format!("duplicate PTF port `{}`", hi.ptf_port),
                });
            }
        }
        Ok(())
    }

    /// Returns the number of VMs defined in this template.
    pub fn vm_count(&self) -> usize {
        self.vms.len()
    }

    /// Returns the number of VLANs defined in this template.
    pub fn vlan_count(&self) -> usize {
        self.vlans.len()
    }

    /// Returns the VMs sorted by `ip_offset`.
    pub fn vms_by_offset(&self) -> Vec<&VmConfig> {
        let mut sorted: Vec<&VmConfig> = self.vms.iter().collect();
        sorted.sort_by_key(|vm| vm.ip_offset);
        sorted
    }

    /// Looks up a VM config by name.
    pub fn get_vm(&self, name: &str) -> Option<&VmConfig> {
        self.vms.iter().find(|vm| vm.name == name)
    }

    /// Looks up a VLAN config by ID.
    pub fn get_vlan(&self, id: u16) -> Option<&VlanConfig> {
        self.vlans.iter().find(|v| v.id == id)
    }

    /// Returns a configuration property by key, or `None`.
    pub fn get_property(&self, key: &str) -> Option<&ConfigProperty> {
        self.configuration_properties.get(key)
    }
}

// ---------------------------------------------------------------------------
// VmConfig
// ---------------------------------------------------------------------------

/// Configuration for a single neighbor VM within a topology template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    /// Logical name of this VM (e.g. `"ARISTA01T1"`).
    pub name: String,

    /// VLAN IDs this VM is connected to.
    #[serde(default)]
    pub vlans: Vec<u16>,

    /// IP offset used for address calculation.
    #[serde(default)]
    pub ip_offset: u32,

    /// Mapping of DUT physical ports to VM-facing ports.
    /// Key: DUT port (e.g. `"Ethernet0"`), Value: VM port index.
    #[serde(default)]
    pub port_mapping: HashMap<String, u32>,

    /// VM type override.  When absent the testbed-level default applies.
    #[serde(default)]
    pub vm_type: Option<VmType>,

    /// Arbitrary per-VM properties (e.g. `bgp_as`, `router_id`).
    #[serde(default)]
    pub properties: HashMap<String, ConfigProperty>,
}

// ---------------------------------------------------------------------------
// VlanConfig
// ---------------------------------------------------------------------------

/// Configuration for a VLAN within a topology template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanConfig {
    /// 802.1Q VLAN ID (1..4094).
    pub id: u16,

    /// IP subnet prefix for this VLAN (e.g. `"192.168.0.0/21"`).
    #[serde(default)]
    pub prefix: Option<String>,

    /// Tagging mode.
    #[serde(default)]
    pub vlan_type: VlanType,
}

/// Whether VLAN members are tagged (trunk) or untagged (access).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VlanType {
    Tagged,
    Untagged,
}

impl Default for VlanType {
    fn default() -> Self {
        Self::Untagged
    }
}

impl std::fmt::Display for VlanType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tagged => f.write_str("tagged"),
            Self::Untagged => f.write_str("untagged"),
        }
    }
}

// ---------------------------------------------------------------------------
// HostInterfaceConfig
// ---------------------------------------------------------------------------

/// Mapping between a DUT physical port and a PTF port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInterfaceConfig {
    /// Index of the VM this interface is associated with (if any).
    #[serde(default)]
    pub vm_index: Option<u32>,

    /// Physical port index on the DUT side.
    #[serde(default)]
    pub port_index: u32,

    /// DUT port name (e.g. `"Ethernet0"`).
    pub dut_port: String,

    /// PTF port name (e.g. `"eth0"`).
    pub ptf_port: String,
}

// ---------------------------------------------------------------------------
// ConfigProperty
// ---------------------------------------------------------------------------

/// A typed configuration property value.
///
/// Topology templates often carry ad-hoc properties (BGP AS numbers, timer
/// values, prefix lists) that do not warrant their own struct fields.  This
/// enum provides a small set of concrete types so that consumers can work with
/// values without string-parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigProperty {
    /// An integer value.
    Integer(i64),
    /// A floating-point value.
    Float(f64),
    /// A boolean flag.
    Bool(bool),
    /// A string value.
    String(String),
    /// A list of string values.
    StringList(Vec<String>),
    /// A nested map of properties.
    Map(HashMap<String, ConfigProperty>),
}

impl ConfigProperty {
    /// Attempts to interpret the property as an `i64`.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(v) => Some(*v),
            _ => None,
        }
    }

    /// Attempts to interpret the property as a `bool`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// Attempts to interpret the property as a `&str`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(v) => Some(v),
            _ => None,
        }
    }

    /// Attempts to interpret the property as a string slice list.
    pub fn as_string_list(&self) -> Option<&[String]> {
        match self {
            Self::StringList(v) => Some(v),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Loading helpers
// ---------------------------------------------------------------------------

/// Detects format from extension and deserializes.
fn deserialize_from_path<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = std::fs::read_to_string(path)?;

    match path.extension().and_then(|e| e.to_str()) {
        Some("yaml" | "yml") => serde_yaml::from_str(&contents).map_err(|e| {
            SonicError::config(format!(
                "failed to parse topology YAML {}: {e}",
                path.display()
            ))
        }),
        Some("toml") => toml::from_str(&contents).map_err(|e| {
            SonicError::config(format!(
                "failed to parse topology TOML {}: {e}",
                path.display()
            ))
        }),
        other => {
            let ext = other.unwrap_or("(none)");
            warn!(
                path = %path.display(),
                extension = ext,
                "unrecognised extension, attempting YAML then TOML"
            );
            serde_yaml::from_str(&contents).or_else(|_| {
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

/// Loads a single topology template from `path` (TOML or YAML).
pub fn load_topology(path: impl AsRef<Path>) -> Result<TopologyConfig> {
    let path = path.as_ref();
    info!(path = %path.display(), "loading topology config");

    let config: TopologyConfig = deserialize_from_path(path)?;
    config.validate()?;

    info!(
        topo_type = %config.topo_type,
        vms = config.vms.len(),
        vlans = config.vlans.len(),
        "loaded topology config"
    );
    Ok(config)
}

/// Scans a directory of topology templates and returns the one matching the
/// requested [`TopologyType`].
///
/// The function looks for files named after the topology type (e.g.
/// `t0.toml`, `t1-lag.yaml`) inside `dir_path`.  If no matching file is
/// found it tries loading every file and checking the `type` field.
pub fn get_topology_template(
    dir_path: impl AsRef<Path>,
    topo_type: TopologyType,
) -> Result<TopologyConfig> {
    let dir_path = dir_path.as_ref();
    let topo_str = topo_type.to_string();

    info!(
        dir = %dir_path.display(),
        topo = %topo_str,
        "searching for topology template"
    );

    // Strategy 1: Look for a file whose stem matches the topology name.
    for ext in &["toml", "yaml", "yml"] {
        let candidate = dir_path.join(format!("{topo_str}.{ext}"));
        if candidate.is_file() {
            debug!(path = %candidate.display(), "found candidate by name");
            return load_topology(&candidate);
        }
    }

    // Strategy 2: Scan all topology files and find one whose `topo_type` field
    // matches.
    for ext in &["toml", "yaml", "yml"] {
        let pattern = format!("{}/*.{ext}", dir_path.display());
        let paths: Vec<PathBuf> = glob::glob(&pattern)
            .map_err(|e| SonicError::config(format!("bad glob pattern: {e}")))?
            .filter_map(|entry| entry.ok())
            .collect();

        for path in paths {
            match load_topology(&path) {
                Ok(tc) if tc.topo_type == topo_type => {
                    debug!(path = %path.display(), "matched topology by type field");
                    return Ok(tc);
                }
                Ok(_) => continue,
                Err(e) => {
                    debug!(
                        path = %path.display(),
                        error = %e,
                        "skipping file while scanning for topology"
                    );
                }
            }
        }
    }

    Err(SonicError::UnsupportedTopology(format!(
        "no topology template found for `{topo_str}` in {}",
        dir_path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vlan(id: u16) -> VlanConfig {
        VlanConfig {
            id,
            prefix: Some("192.168.0.0/21".into()),
            vlan_type: VlanType::Untagged,
        }
    }

    fn sample_vm(name: &str, vlans: Vec<u16>) -> VmConfig {
        VmConfig {
            name: name.into(),
            vlans,
            ip_offset: 1,
            port_mapping: HashMap::new(),
            vm_type: None,
            properties: HashMap::new(),
        }
    }

    fn sample_host_if(dut: &str, ptf: &str) -> HostInterfaceConfig {
        HostInterfaceConfig {
            vm_index: None,
            port_index: 0,
            dut_port: dut.into(),
            ptf_port: ptf.into(),
        }
    }

    fn sample_topo() -> TopologyConfig {
        TopologyConfig {
            topo_type: TopologyType::T0,
            vms: vec![
                sample_vm("ARISTA01T1", vec![1000]),
                sample_vm("ARISTA02T1", vec![1001]),
            ],
            vlans: vec![sample_vlan(1000), sample_vlan(1001)],
            host_interfaces: vec![
                sample_host_if("Ethernet0", "eth0"),
                sample_host_if("Ethernet4", "eth1"),
            ],
            configuration_properties: HashMap::new(),
        }
    }

    #[test]
    fn valid_topology_passes() {
        sample_topo().validate().expect("should be valid");
    }

    #[test]
    fn empty_vm_name_rejected() {
        let mut topo = sample_topo();
        topo.vms[0].name = String::new();
        assert!(topo.validate().is_err());
    }

    #[test]
    fn duplicate_vm_name_rejected() {
        let mut topo = sample_topo();
        topo.vms[1].name = "ARISTA01T1".into();
        assert!(topo.validate().is_err());
    }

    #[test]
    fn vm_referencing_missing_vlan_rejected() {
        let mut topo = sample_topo();
        topo.vms[0].vlans.push(9999);
        assert!(topo.validate().is_err());
    }

    #[test]
    fn zero_vlan_id_rejected() {
        let mut topo = sample_topo();
        topo.vlans[0].id = 0;
        assert!(topo.validate().is_err());
    }

    #[test]
    fn vlan_id_above_max_rejected() {
        let mut topo = sample_topo();
        topo.vlans[0].id = 4095;
        assert!(topo.validate().is_err());
    }

    #[test]
    fn duplicate_vlan_id_rejected() {
        let mut topo = sample_topo();
        topo.vlans[1].id = 1000;
        assert!(topo.validate().is_err());
    }

    #[test]
    fn empty_dut_port_rejected() {
        let mut topo = sample_topo();
        topo.host_interfaces[0].dut_port = String::new();
        assert!(topo.validate().is_err());
    }

    #[test]
    fn duplicate_dut_port_rejected() {
        let mut topo = sample_topo();
        topo.host_interfaces[1].dut_port = "Ethernet0".into();
        assert!(topo.validate().is_err());
    }

    #[test]
    fn duplicate_ptf_port_rejected() {
        let mut topo = sample_topo();
        topo.host_interfaces[1].ptf_port = "eth0".into();
        assert!(topo.validate().is_err());
    }

    #[test]
    fn get_vm_by_name() {
        let topo = sample_topo();
        assert!(topo.get_vm("ARISTA01T1").is_some());
        assert!(topo.get_vm("nonexistent").is_none());
    }

    #[test]
    fn get_vlan_by_id() {
        let topo = sample_topo();
        assert!(topo.get_vlan(1000).is_some());
        assert!(topo.get_vlan(9999).is_none());
    }

    #[test]
    fn vms_by_offset_sorted() {
        let mut topo = sample_topo();
        topo.vms[0].ip_offset = 10;
        topo.vms[1].ip_offset = 2;
        let sorted = topo.vms_by_offset();
        assert_eq!(sorted[0].name, "ARISTA02T1");
        assert_eq!(sorted[1].name, "ARISTA01T1");
    }

    #[test]
    fn config_property_accessors() {
        let int_prop = ConfigProperty::Integer(65000);
        assert_eq!(int_prop.as_integer(), Some(65000));
        assert_eq!(int_prop.as_bool(), None);

        let bool_prop = ConfigProperty::Bool(true);
        assert_eq!(bool_prop.as_bool(), Some(true));

        let str_prop = ConfigProperty::String("hello".into());
        assert_eq!(str_prop.as_str(), Some("hello"));

        let list_prop = ConfigProperty::StringList(vec!["a".into(), "b".into()]);
        assert_eq!(list_prop.as_string_list().unwrap().len(), 2);
    }

    #[test]
    fn toml_roundtrip() {
        let topo = sample_topo();
        let serialized = toml::to_string_pretty(&topo).unwrap();
        let deserialized: TopologyConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(topo.topo_type, deserialized.topo_type);
        assert_eq!(topo.vms.len(), deserialized.vms.len());
        assert_eq!(topo.vlans.len(), deserialized.vlans.len());
        deserialized.validate().unwrap();
    }

    #[test]
    fn get_topology_template_missing_dir() {
        let result = get_topology_template("/nonexistent/dir", TopologyType::T0);
        assert!(result.is_err());
    }

    #[test]
    fn vlan_type_default_is_untagged() {
        let vt: VlanType = Default::default();
        assert_eq!(vt, VlanType::Untagged);
    }

    #[test]
    fn vlan_type_display() {
        assert_eq!(VlanType::Tagged.to_string(), "tagged");
        assert_eq!(VlanType::Untagged.to_string(), "untagged");
    }

    #[test]
    fn topology_config_with_properties() {
        let mut topo = sample_topo();
        topo.configuration_properties.insert(
            "bgp_asn".into(),
            ConfigProperty::Integer(65100),
        );
        topo.configuration_properties.insert(
            "peer_groups".into(),
            ConfigProperty::StringList(vec!["PEER_V4".into(), "PEER_V6".into()]),
        );

        topo.validate().unwrap();

        assert_eq!(
            topo.get_property("bgp_asn").unwrap().as_integer(),
            Some(65100)
        );
        assert_eq!(
            topo.get_property("peer_groups")
                .unwrap()
                .as_string_list()
                .unwrap()
                .len(),
            2
        );
        assert!(topo.get_property("nonexistent").is_none());
    }
}
