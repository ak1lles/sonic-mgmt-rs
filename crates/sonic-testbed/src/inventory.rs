//! Inventory management.
//!
//! An [`InventoryManager`] loads, stores, and manipulates device inventories
//! that describe which hosts belong to a testbed and how they are grouped.
//! The canonical on-disk format is TOML.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use sonic_core::{
    ConnectionType, Credentials, DeviceInfo, DeviceType, Platform, SonicError,
};

// ---------------------------------------------------------------------------
// Placeholder config types
// ---------------------------------------------------------------------------
// In the final integration these come from `sonic_config`.  We define minimal
// local versions so this crate compiles without a circular dependency.

/// Minimal placeholder for `sonic_config::DeviceEntry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub hostname: String,
    pub mgmt_ip: String,
    #[serde(default = "default_device_type")]
    pub device_type: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub hwsku: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub key_path: Option<String>,
    #[serde(default)]
    pub connection_type: Option<String>,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_device_type() -> String {
    "sonic".to_string()
}
fn default_port() -> u16 {
    22
}

impl DeviceEntry {
    /// Converts to a `sonic_core::DeviceInfo`.
    pub fn to_device_info(&self) -> sonic_core::Result<DeviceInfo> {
        let ip: std::net::IpAddr = self
            .mgmt_ip
            .parse()
            .map_err(|e| SonicError::config(format!("invalid IP '{}': {e}", self.mgmt_ip)))?;

        let dt = match self.device_type.to_lowercase().as_str() {
            "sonic" => DeviceType::Sonic,
            "eos" => DeviceType::Eos,
            "cisco" => DeviceType::Cisco,
            "fanout" => DeviceType::Fanout,
            "ptf" => DeviceType::Ptf,
            other => {
                return Err(SonicError::config(format!(
                    "unknown device type '{other}'"
                )));
            }
        };

        let platform = match self.platform.to_lowercase().as_str() {
            "broadcom" => Platform::Broadcom,
            "mellanox" => Platform::Mellanox,
            "barefoot" => Platform::Barefoot,
            "virtual" => Platform::Virtual,
            "" => Platform::Unknown,
            _ => Platform::Unknown,
        };

        let conn = match self
            .connection_type
            .as_deref()
            .unwrap_or("ssh")
            .to_lowercase()
            .as_str()
        {
            "ssh" => ConnectionType::Ssh,
            "telnet" => ConnectionType::Telnet,
            "console" => ConnectionType::Console,
            "grpc" => ConnectionType::Grpc,
            _ => ConnectionType::Ssh,
        };

        let mut creds = Credentials::new(&self.username);
        if let Some(pw) = &self.password {
            creds = creds.with_password(pw);
        }
        if let Some(kp) = &self.key_path {
            creds = creds.with_key(kp);
        }

        let mut info = DeviceInfo::new(&self.hostname, ip, dt, creds);
        info.platform = platform;
        info.hwsku = self.hwsku.clone();
        info.connection_type = conn;
        info.port = self.port;
        info.metadata = self.metadata.clone();
        Ok(info)
    }

    /// Creates a `DeviceEntry` from a `DeviceInfo` (reverse mapping).
    pub fn from_device_info(info: &DeviceInfo) -> Self {
        Self {
            hostname: info.hostname.clone(),
            mgmt_ip: info.mgmt_ip.to_string(),
            device_type: format!("{:?}", info.device_type).to_lowercase(),
            platform: info.platform.to_string().to_lowercase(),
            hwsku: info.hwsku.clone(),
            username: info.credentials.username.clone(),
            password: info.credentials.password.clone(),
            key_path: info.credentials.key_path.clone(),
            connection_type: Some(format!("{:?}", info.connection_type).to_lowercase()),
            port: info.port,
            metadata: info.metadata.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Inventory file model
// ---------------------------------------------------------------------------

/// Serializable inventory file format.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct InventoryFile {
    #[serde(default)]
    devices: Vec<DeviceEntry>,
    #[serde(default)]
    groups: HashMap<String, Vec<String>>,
}

// ---------------------------------------------------------------------------
// InventoryManager
// ---------------------------------------------------------------------------

/// Manages a device inventory (load, save, query, merge).
pub struct InventoryManager {
    /// Devices keyed by hostname.
    devices: HashMap<String, DeviceEntry>,
    /// Named groups of hostnames.
    groups: HashMap<String, Vec<String>>,
    /// Path to the backing TOML file (if loaded from disk).
    file_path: Option<PathBuf>,
}

impl InventoryManager {
    /// Creates an empty inventory.
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            groups: HashMap::new(),
            file_path: None,
        }
    }

    /// Loads an inventory from a TOML file.
    pub fn load(path: impl AsRef<Path>) -> sonic_core::Result<Self> {
        let path = path.as_ref();
        info!(path = %path.display(), "loading inventory");

        let content = std::fs::read_to_string(path)?;
        let inv: InventoryFile = toml::from_str(&content)?;

        let mut devices = HashMap::with_capacity(inv.devices.len());
        for entry in inv.devices {
            if devices.contains_key(&entry.hostname) {
                warn!(hostname = %entry.hostname, "duplicate device entry, last wins");
            }
            devices.insert(entry.hostname.clone(), entry);
        }

        debug!(
            device_count = devices.len(),
            group_count = inv.groups.len(),
            "inventory loaded"
        );

        Ok(Self {
            devices,
            groups: inv.groups,
            file_path: Some(path.to_path_buf()),
        })
    }

    /// Writes the current inventory back to a TOML file.
    pub fn save(&self, path: impl AsRef<Path>) -> sonic_core::Result<()> {
        let path = path.as_ref();
        info!(path = %path.display(), "saving inventory");

        let inv = InventoryFile {
            devices: self.devices.values().cloned().collect(),
            groups: self.groups.clone(),
        };

        let content = toml::to_string_pretty(&inv)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    // -- device CRUD -------------------------------------------------------

    /// Adds or replaces a device entry.
    pub fn add_device(&mut self, entry: DeviceEntry) {
        debug!(hostname = %entry.hostname, "adding device");
        self.devices.insert(entry.hostname.clone(), entry);
    }

    /// Removes a device by hostname, returning it if it existed.
    pub fn remove_device(&mut self, hostname: &str) -> Option<DeviceEntry> {
        debug!(hostname = %hostname, "removing device");
        let entry = self.devices.remove(hostname);
        // Also remove from any groups.
        for members in self.groups.values_mut() {
            members.retain(|h| h != hostname);
        }
        entry
    }

    /// Looks up a device by hostname.
    pub fn get_device(&self, hostname: &str) -> Option<&DeviceEntry> {
        self.devices.get(hostname)
    }

    /// Returns all device entries.
    pub fn all_devices(&self) -> Vec<&DeviceEntry> {
        self.devices.values().collect()
    }

    /// Returns the number of devices in the inventory.
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    // -- group management --------------------------------------------------

    /// Adds or replaces a group.
    pub fn add_group(&mut self, name: impl Into<String>, members: Vec<String>) {
        let name = name.into();
        debug!(group = %name, member_count = members.len(), "adding group");
        self.groups.insert(name, members);
    }

    /// Returns the hostnames in a group.
    pub fn get_group(&self, name: &str) -> Option<&Vec<String>> {
        self.groups.get(name)
    }

    /// Resolves a group to `DeviceInfo` objects.
    ///
    /// Devices that are referenced in the group but absent from the inventory
    /// are silently skipped with a warning.
    pub fn resolve_group(&self, name: &str) -> sonic_core::Result<Vec<DeviceInfo>> {
        let members = self
            .groups
            .get(name)
            .ok_or_else(|| SonicError::config(format!("group '{name}' not found")))?;

        let mut infos = Vec::with_capacity(members.len());
        for hostname in members {
            match self.devices.get(hostname) {
                Some(entry) => infos.push(entry.to_device_info()?),
                None => {
                    warn!(
                        group = name,
                        hostname = %hostname,
                        "group member not in inventory, skipping"
                    );
                }
            }
        }
        Ok(infos)
    }

    // -- merge / generation ------------------------------------------------

    /// Merges another inventory into this one.
    ///
    /// Devices present in `other` overwrite existing entries with the same
    /// hostname.  Groups from `other` are merged with existing groups (union
    /// of members, deduplicated).
    pub fn merge(&mut self, other: &InventoryManager) {
        info!(
            incoming_devices = other.devices.len(),
            incoming_groups = other.groups.len(),
            "merging inventory"
        );

        for (hostname, entry) in &other.devices {
            self.devices.insert(hostname.clone(), entry.clone());
        }

        for (group_name, members) in &other.groups {
            let existing = self.groups.entry(group_name.clone()).or_default();
            for m in members {
                if !existing.contains(m) {
                    existing.push(m.clone());
                }
            }
        }
    }

    /// Auto-generates an inventory from a list of `DeviceInfo` objects,
    /// grouping them by device type.
    pub fn generate_from_devices(devices: &[DeviceInfo]) -> Self {
        info!(
            device_count = devices.len(),
            "generating inventory from device list"
        );

        let mut inv = Self::new();
        let mut type_groups: HashMap<String, Vec<String>> = HashMap::new();

        for device in devices {
            let entry = DeviceEntry::from_device_info(device);
            let type_key = format!("{:?}", device.device_type).to_lowercase();
            type_groups
                .entry(type_key)
                .or_default()
                .push(device.hostname.clone());
            inv.add_device(entry);
        }

        for (group_name, members) in type_groups {
            inv.add_group(group_name, members);
        }

        inv
    }

    /// Returns the file path this inventory was loaded from (if any).
    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }
}

impl Default for InventoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(name: &str, ip: &str) -> DeviceEntry {
        DeviceEntry {
            hostname: name.to_string(),
            mgmt_ip: ip.to_string(),
            device_type: "sonic".to_string(),
            platform: String::new(),
            hwsku: String::new(),
            username: "admin".to_string(),
            password: Some("admin".to_string()),
            key_path: None,
            connection_type: None,
            port: 22,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn add_and_get() {
        let mut inv = InventoryManager::new();
        inv.add_device(sample_entry("dut1", "10.0.0.1"));
        assert!(inv.get_device("dut1").is_some());
        assert_eq!(inv.device_count(), 1);
    }

    #[test]
    fn remove_cleans_groups() {
        let mut inv = InventoryManager::new();
        inv.add_device(sample_entry("dut1", "10.0.0.1"));
        inv.add_device(sample_entry("dut2", "10.0.0.2"));
        inv.add_group("all", vec!["dut1".into(), "dut2".into()]);
        inv.remove_device("dut1");
        assert_eq!(inv.get_group("all").unwrap(), &vec!["dut2".to_string()]);
    }

    #[test]
    fn merge_combines() {
        let mut a = InventoryManager::new();
        a.add_device(sample_entry("dut1", "10.0.0.1"));
        a.add_group("sonic", vec!["dut1".into()]);

        let mut b = InventoryManager::new();
        b.add_device(sample_entry("dut2", "10.0.0.2"));
        b.add_group("sonic", vec!["dut2".into()]);

        a.merge(&b);
        assert_eq!(a.device_count(), 2);
        assert_eq!(a.get_group("sonic").unwrap().len(), 2);
    }

    #[test]
    fn device_entry_roundtrip() {
        let entry = sample_entry("sw1", "192.168.1.1");
        let info = entry.to_device_info().unwrap();
        assert_eq!(info.hostname, "sw1");
        assert_eq!(info.device_type, DeviceType::Sonic);

        let back = DeviceEntry::from_device_info(&info);
        assert_eq!(back.hostname, "sw1");
    }

    #[test]
    fn generate_from_devices_groups_by_type() {
        let creds = Credentials::new("admin");
        let d1 = DeviceInfo::new(
            "dut1",
            "10.0.0.1".parse().unwrap(),
            DeviceType::Sonic,
            creds.clone(),
        );
        let d2 = DeviceInfo::new(
            "ptf1",
            "10.0.0.2".parse().unwrap(),
            DeviceType::Ptf,
            creds,
        );

        let inv = InventoryManager::generate_from_devices(&[d1, d2]);
        assert_eq!(inv.device_count(), 2);
        assert!(inv.get_group("sonic").is_some());
        assert!(inv.get_group("ptf").is_some());
    }
}
