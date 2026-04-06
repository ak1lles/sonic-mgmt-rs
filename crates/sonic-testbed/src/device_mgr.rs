//! Live device connection manager.
//!
//! [`DeviceManager`] holds active [`Device`] instances keyed by hostname,
//! providing connect/disconnect lifecycle management and command dispatch.

use std::collections::HashMap;

use tracing::{debug, info, warn};

use sonic_core::{CommandResult, Device, DeviceInfo, SonicError};

/// Manages live SSH/Telnet connections to testbed devices.
///
/// Holds `Box<dyn Device>` instances keyed by hostname. Use
/// [`connect_device`](Self::connect_device) to bring up a connection and
/// [`execute_on`](Self::execute_on) to run commands on a connected host.
pub struct DeviceManager {
    devices: HashMap<String, Box<dyn Device>>,
}

impl DeviceManager {
    /// Creates an empty device manager with no active connections.
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
        }
    }

    /// Connects to a device described by `info`.
    ///
    /// Uses [`sonic_device::create_host`] to instantiate the appropriate
    /// driver, then calls `connect()`. The device is stored by hostname
    /// for later command execution.
    pub async fn connect_device(&mut self, info: &DeviceInfo) -> sonic_core::Result<()> {
        let hostname = &info.hostname;

        if self.devices.contains_key(hostname) {
            debug!(hostname = %hostname, "device already connected, skipping");
            return Ok(());
        }

        info!(hostname = %hostname, ip = %info.mgmt_ip, "connecting to device");
        let mut host = sonic_device::create_host(info.clone());
        host.connect().await?;
        info!(hostname = %hostname, "device connected");

        self.devices.insert(hostname.clone(), host);
        Ok(())
    }

    /// Disconnects a single device by hostname.
    ///
    /// Calls `disconnect()` on the underlying driver and removes it from the
    /// map. Returns `Ok(())` if the hostname was not connected.
    pub async fn disconnect_device(&mut self, hostname: &str) -> sonic_core::Result<()> {
        if let Some(mut device) = self.devices.remove(hostname) {
            info!(hostname = %hostname, "disconnecting device");
            if let Err(e) = device.disconnect().await {
                warn!(hostname = %hostname, error = %e, "disconnect returned error");
            }
        }
        Ok(())
    }

    /// Disconnects all connected devices.
    pub async fn disconnect_all(&mut self) -> sonic_core::Result<()> {
        let hostnames: Vec<String> = self.devices.keys().cloned().collect();
        for hostname in hostnames {
            self.disconnect_device(&hostname).await?;
        }
        Ok(())
    }

    /// Returns a reference to a connected device, if present.
    pub fn get_device(&self, hostname: &str) -> Option<&dyn Device> {
        self.devices.get(hostname).map(|b| b.as_ref())
    }

    /// Returns a mutable reference to a connected device, if present.
    pub fn get_device_mut(&mut self, hostname: &str) -> Option<&mut Box<dyn Device>> {
        self.devices.get_mut(hostname)
    }

    /// Executes a command on a connected device.
    ///
    /// Returns [`SonicError::DeviceNotFound`] if the hostname has no active
    /// connection.
    pub async fn execute_on(
        &self,
        hostname: &str,
        command: &str,
    ) -> sonic_core::Result<CommandResult> {
        let device = self
            .devices
            .get(hostname)
            .ok_or_else(|| SonicError::DeviceNotFound(hostname.to_string()))?;

        debug!(hostname = %hostname, command = %command, "executing command");
        device.execute(command).await
    }

    /// Returns the hostnames of all currently connected devices.
    pub fn connected_hosts(&self) -> Vec<&str> {
        self.devices.keys().map(|s| s.as_str()).collect()
    }

    /// Returns true if the given hostname has an active connection.
    pub fn is_connected(&self, hostname: &str) -> bool {
        self.devices.contains_key(hostname)
    }
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_is_empty() {
        let mgr = DeviceManager::new();
        assert!(mgr.connected_hosts().is_empty());
        assert!(!mgr.is_connected("anything"));
        assert!(mgr.get_device("anything").is_none());
    }

    #[test]
    fn default_is_new() {
        let mgr = DeviceManager::default();
        assert!(mgr.connected_hosts().is_empty());
    }
}
