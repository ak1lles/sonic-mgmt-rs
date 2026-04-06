//! Device inventory configuration.
//!
//! An *inventory* is a flat catalogue of every device the framework may need to
//! contact: DUTs, fanout switches, PTF containers, console servers, etc.
//! Devices are keyed by hostname and may be organised into named groups for
//! batch operations.
//!
//! The format mirrors (and extends) the Ansible-style host inventory used by
//! the original Python framework.  Both TOML and YAML are supported.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use sonic_core::{
    ConnectionType, Credentials, DeviceType, Platform, SonicError, Result,
};

// ---------------------------------------------------------------------------
// InventoryConfig
// ---------------------------------------------------------------------------

/// Root inventory document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InventoryConfig {
    /// Map of hostname to device entry.
    pub devices: HashMap<String, DeviceEntry>,

    /// Named groups of hostnames.  A group name maps to a list of hostnames
    /// that must exist in `devices`.
    pub groups: HashMap<String, Vec<String>>,
}

impl InventoryConfig {
    /// Returns a reference to the device entry for `hostname`, or `None`.
    pub fn get_device(&self, hostname: &str) -> Option<&DeviceEntry> {
        self.devices.get(hostname)
    }

    /// Returns all device entries that belong to `group`.
    ///
    /// Hostnames listed in the group that do not exist in the `devices` map
    /// are silently skipped (a warning is emitted via `tracing`).
    pub fn get_group_devices(&self, group: &str) -> Vec<(&String, &DeviceEntry)> {
        let Some(hostnames) = self.groups.get(group) else {
            return Vec::new();
        };

        hostnames
            .iter()
            .filter_map(|h| {
                match self.devices.get(h) {
                    Some(entry) => Some((h, entry)),
                    None => {
                        warn!(
                            group,
                            hostname = h.as_str(),
                            "group references unknown device"
                        );
                        None
                    }
                }
            })
            .collect()
    }

    /// Merges `other` into `self`.
    ///
    /// Devices in `other` overwrite devices with the same hostname in `self`.
    /// Group memberships are merged (union of hostnames).
    pub fn merge(&mut self, other: &InventoryConfig) {
        for (hostname, entry) in &other.devices {
            self.devices.insert(hostname.clone(), entry.clone());
        }

        for (group, hostnames) in &other.groups {
            let existing = self.groups.entry(group.clone()).or_default();
            for h in hostnames {
                if !existing.contains(h) {
                    existing.push(h.clone());
                }
            }
        }
        debug!(
            devices = self.devices.len(),
            groups = self.groups.len(),
            "merged inventories"
        );
    }

    /// Validates internal consistency:
    /// - Every hostname in a group must exist in `devices`.
    /// - Every device entry must pass its own validation.
    pub fn validate(&self) -> Result<()> {
        for (hostname, entry) in &self.devices {
            entry.validate(hostname)?;
        }

        for (group, hostnames) in &self.groups {
            for h in hostnames {
                if !self.devices.contains_key(h) {
                    return Err(SonicError::ConfigValidation {
                        path: format!("inventory.groups[{group}]"),
                        reason: format!(
                            "group references hostname `{h}` which is not in the devices map"
                        ),
                    });
                }
            }
        }

        debug!(
            devices = self.devices.len(),
            groups = self.groups.len(),
            "inventory validation passed"
        );
        Ok(())
    }

    /// Returns the total number of devices in the inventory.
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    /// Returns all hostnames, sorted alphabetically.
    pub fn hostnames(&self) -> Vec<&String> {
        let mut names: Vec<&String> = self.devices.keys().collect();
        names.sort();
        names
    }

    /// Returns all group names, sorted alphabetically.
    pub fn group_names(&self) -> Vec<&String> {
        let mut names: Vec<&String> = self.groups.keys().collect();
        names.sort();
        names
    }

    /// Returns devices filtered by device type.
    pub fn devices_by_type(&self, dt: DeviceType) -> Vec<(&String, &DeviceEntry)> {
        self.devices
            .iter()
            .filter(|(_, e)| e.device_type == dt)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// DeviceEntry
// ---------------------------------------------------------------------------

/// A single device in the inventory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    /// Management IP address (v4 or v6).
    pub mgmt_ip: IpAddr,

    /// What kind of device this is.
    #[serde(default = "default_device_type")]
    pub device_type: DeviceType,

    /// Platform / ASIC vendor.
    #[serde(default = "default_platform")]
    pub platform: Platform,

    /// Hardware SKU string.
    #[serde(default)]
    pub hwsku: String,

    /// Login credentials.
    #[serde(default)]
    pub credentials: InventoryCredentials,

    /// Connection transport details.
    #[serde(default)]
    pub connection: ConnectionInfo,

    /// Out-of-band console access details (optional).
    #[serde(default)]
    pub console: Option<ConsoleEntry>,

    /// Arbitrary per-device metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_device_type() -> DeviceType {
    DeviceType::Sonic
}

fn default_platform() -> Platform {
    Platform::Unknown
}

impl DeviceEntry {
    fn validate(&self, hostname: &str) -> Result<()> {
        if self.connection.port == 0 {
            return Err(SonicError::ConfigValidation {
                path: format!("inventory.devices[{hostname}].connection.port"),
                reason: "connection port must be non-zero".into(),
            });
        }
        if let Some(ref console) = self.console {
            console.validate(hostname)?;
        }
        Ok(())
    }

    /// Converts the inventory credentials into the core `Credentials` type.
    pub fn to_core_credentials(&self) -> Credentials {
        let mut creds = Credentials::new(&self.credentials.username);
        if let Some(ref pw) = self.credentials.password {
            creds = creds.with_password(pw.clone());
        }
        if let Some(ref key) = self.credentials.key_path {
            creds = creds.with_key(key.to_string_lossy().to_string());
        }
        creds
    }
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Credentials stored in an inventory file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InventoryCredentials {
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_path: Option<PathBuf>,
}

impl Default for InventoryCredentials {
    fn default() -> Self {
        Self {
            username: "admin".to_owned(),
            password: None,
            key_path: None,
        }
    }
}

/// Transport-layer connection info for a device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionInfo {
    /// Transport type.
    pub connection_type: ConnectionType,
    /// Port number (e.g. 22 for SSH, 23 for telnet).
    pub port: u16,
    /// Optional timeout override in seconds.
    pub timeout_secs: Option<u64>,
}

impl Default for ConnectionInfo {
    fn default() -> Self {
        Self {
            connection_type: ConnectionType::Ssh,
            port: 22,
            timeout_secs: None,
        }
    }
}

/// Out-of-band console access information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleEntry {
    /// Console server hostname or IP.
    pub server: String,
    /// Port number on the console server.
    pub port: u16,
    /// Protocol used for the console connection.
    #[serde(default = "default_console_protocol")]
    pub protocol: ConnectionType,
}

fn default_console_protocol() -> ConnectionType {
    ConnectionType::Ssh
}

impl ConsoleEntry {
    fn validate(&self, hostname: &str) -> Result<()> {
        if self.server.is_empty() {
            return Err(SonicError::ConfigValidation {
                path: format!("inventory.devices[{hostname}].console.server"),
                reason: "console server must not be empty".into(),
            });
        }
        if self.port == 0 {
            return Err(SonicError::ConfigValidation {
                path: format!("inventory.devices[{hostname}].console.port"),
                reason: "console port must be non-zero".into(),
            });
        }
        Ok(())
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
                "failed to parse inventory YAML {}: {e}",
                path.display()
            ))
        }),
        Some("toml") => toml::from_str(&contents).map_err(|e| {
            SonicError::config(format!(
                "failed to parse inventory TOML {}: {e}",
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

/// Loads a device inventory from `path` (TOML or YAML).
pub fn load_inventory(path: impl AsRef<Path>) -> Result<InventoryConfig> {
    let path = path.as_ref();
    info!(path = %path.display(), "loading inventory");

    let inventory: InventoryConfig = deserialize_from_path(path)?;
    inventory.validate()?;

    info!(
        devices = inventory.devices.len(),
        groups = inventory.groups.len(),
        "loaded inventory"
    );
    Ok(inventory)
}

/// Merges multiple inventory files into a single [`InventoryConfig`].
///
/// Files are processed in order; later files win when hostnames collide.
pub fn merge_inventories(paths: &[impl AsRef<Path>]) -> Result<InventoryConfig> {
    let mut combined = InventoryConfig::default();

    for path in paths {
        let path = path.as_ref();
        match load_inventory(path) {
            Ok(inv) => combined.merge(&inv),
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping inventory file that failed to load"
                );
            }
        }
    }

    combined.validate()?;

    info!(
        devices = combined.devices.len(),
        groups = combined.groups.len(),
        "merged inventories from {} files",
        paths.len()
    );
    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn sample_device() -> DeviceEntry {
        DeviceEntry {
            mgmt_ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            device_type: DeviceType::Sonic,
            platform: Platform::Broadcom,
            hwsku: "ACS-MSN2700".into(),
            credentials: InventoryCredentials::default(),
            connection: ConnectionInfo::default(),
            console: None,
            metadata: HashMap::new(),
        }
    }

    fn sample_inventory() -> InventoryConfig {
        let mut inv = InventoryConfig::default();
        inv.devices.insert("switch-1".into(), sample_device());
        inv.devices.insert("switch-2".into(), {
            let mut d = sample_device();
            d.mgmt_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
            d
        });
        inv.groups
            .insert("all".into(), vec!["switch-1".into(), "switch-2".into()]);
        inv
    }

    #[test]
    fn valid_inventory_passes() {
        sample_inventory().validate().expect("should be valid");
    }

    #[test]
    fn get_device_found() {
        let inv = sample_inventory();
        assert!(inv.get_device("switch-1").is_some());
    }

    #[test]
    fn get_device_missing() {
        let inv = sample_inventory();
        assert!(inv.get_device("nonexistent").is_none());
    }

    #[test]
    fn get_group_devices_returns_entries() {
        let inv = sample_inventory();
        let devs = inv.get_group_devices("all");
        assert_eq!(devs.len(), 2);
    }

    #[test]
    fn get_group_devices_unknown_group() {
        let inv = sample_inventory();
        let devs = inv.get_group_devices("nonexistent");
        assert!(devs.is_empty());
    }

    #[test]
    fn group_referencing_unknown_device_rejected() {
        let mut inv = sample_inventory();
        inv.groups
            .insert("bad".into(), vec!["nonexistent-host".into()]);
        assert!(inv.validate().is_err());
    }

    #[test]
    fn zero_port_rejected() {
        let mut inv = sample_inventory();
        inv.devices
            .get_mut("switch-1")
            .unwrap()
            .connection
            .port = 0;
        assert!(inv.validate().is_err());
    }

    #[test]
    fn console_empty_server_rejected() {
        let mut inv = sample_inventory();
        inv.devices.get_mut("switch-1").unwrap().console = Some(ConsoleEntry {
            server: String::new(),
            port: 3000,
            protocol: ConnectionType::Ssh,
        });
        assert!(inv.validate().is_err());
    }

    #[test]
    fn merge_union_groups() {
        let mut a = sample_inventory();
        let mut b = InventoryConfig::default();
        b.devices.insert("switch-3".into(), {
            let mut d = sample_device();
            d.mgmt_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));
            d
        });
        b.groups.insert("all".into(), vec!["switch-3".into()]);

        a.merge(&b);

        assert_eq!(a.devices.len(), 3);
        assert_eq!(a.groups["all"].len(), 3);
    }

    #[test]
    fn merge_overwrite_device() {
        let mut a = sample_inventory();
        let mut b = InventoryConfig::default();
        let mut replacement = sample_device();
        replacement.hwsku = "REPLACED".into();
        b.devices.insert("switch-1".into(), replacement);

        a.merge(&b);

        assert_eq!(a.devices["switch-1"].hwsku, "REPLACED");
    }

    #[test]
    fn to_core_credentials_converts() {
        let entry = sample_device();
        let creds = entry.to_core_credentials();
        assert_eq!(creds.username, "admin");
    }

    #[test]
    fn toml_roundtrip() {
        let inv = sample_inventory();
        let serialized = toml::to_string_pretty(&inv).unwrap();
        let deserialized: InventoryConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(inv.devices.len(), deserialized.devices.len());
        deserialized.validate().unwrap();
    }

    #[test]
    fn hostnames_sorted() {
        let inv = sample_inventory();
        let names = inv.hostnames();
        let as_strs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        assert_eq!(as_strs, vec!["switch-1", "switch-2"]);
    }

    #[test]
    fn devices_by_type_filters() {
        let mut inv = sample_inventory();
        inv.devices.get_mut("switch-2").unwrap().device_type = DeviceType::Fanout;

        let sonic = inv.devices_by_type(DeviceType::Sonic);
        assert_eq!(sonic.len(), 1);

        let fanout = inv.devices_by_type(DeviceType::Fanout);
        assert_eq!(fanout.len(), 1);
    }
}
