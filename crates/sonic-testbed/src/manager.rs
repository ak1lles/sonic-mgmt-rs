//! Testbed lifecycle management.
//!
//! The [`Testbed`] struct is the central coordinator for a SONiC test
//! environment.  It owns the topology definition, device inventory, and
//! current state, and implements `sonic_core::TestbedManager` to drive
//! the full deploy / teardown / health-check workflow.

use std::collections::HashMap;
use std::net::IpAddr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use sonic_core::{
    Credentials, DeviceInfo, DeviceType, HealthStatus, Platform, SonicError,
    TestbedManager, TestbedState, TopologyDefinition, TopologyGenerator, TopologyType,
};

use crate::health::HealthChecker;

// ---------------------------------------------------------------------------
// Placeholder config types
// ---------------------------------------------------------------------------
// In the final integration these come from `sonic_config`.  We define minimal
// local versions here to avoid a compile-time circular dependency.

/// Minimal placeholder for `sonic_config::TestbedConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestbedConfig {
    /// Testbed name (e.g. `vms-t0-1`).
    pub name: String,
    /// Topology type to deploy.
    #[serde(default = "default_topo")]
    pub topology: String,
    /// DUT definitions.
    #[serde(default)]
    pub duts: Vec<DutConfig>,
    /// Neighbor (VM) definitions.
    #[serde(default)]
    pub neighbors: Vec<NeighborConfig>,
    /// PTF container hostname / IP.
    #[serde(default)]
    pub ptf_ip: Option<String>,
    #[serde(default)]
    pub ptf_user: Option<String>,
    /// Testbed server hostname / IP.
    #[serde(default)]
    pub server: Option<String>,
    /// Default credentials shared across devices.
    #[serde(default)]
    pub default_user: Option<String>,
    #[serde(default)]
    pub default_password: Option<String>,
    /// Fanout switch configurations.
    #[serde(default)]
    pub fanouts: Vec<FanoutConfig>,
    /// Physical link wiring.
    #[serde(default)]
    pub connection_graph: Vec<PhysicalLink>,
}

fn default_topo() -> String {
    "t0".to_string()
}

/// Minimal placeholder for `sonic_config::DutConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DutConfig {
    pub hostname: String,
    pub mgmt_ip: String,
    #[serde(default)]
    pub hwsku: String,
    #[serde(default)]
    pub platform: String,
}

/// Minimal placeholder for `sonic_config::NeighborConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborConfig {
    pub hostname: String,
    pub mgmt_ip: String,
    #[serde(default = "default_neighbor_type")]
    pub device_type: String,
}

fn default_neighbor_type() -> String {
    "eos".to_string()
}

/// Minimal placeholder for `sonic_config::FanoutConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanoutConfig {
    pub hostname: String,
    pub mgmt_ip: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub hwsku: String,
}

/// Minimal placeholder for `sonic_config::PhysicalLink`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalLink {
    pub dut_port: String,
    pub fanout_host: String,
    pub fanout_port: String,
    #[serde(default)]
    pub ptf_port: String,
    #[serde(default)]
    pub vlan_id: u16,
}

// ---------------------------------------------------------------------------
// Testbed
// ---------------------------------------------------------------------------

/// A live testbed instance.
pub struct Testbed {
    /// Human-readable testbed name.
    name: String,
    /// Parsed configuration.
    config: TestbedConfig,
    /// Active topology definition (populated after deploy).
    topology: Option<TopologyDefinition>,
    /// Current lifecycle state.
    state: TestbedState,
    /// All known devices keyed by hostname.
    devices: HashMap<String, DeviceInfo>,
    /// Fanout switch devices keyed by hostname.
    fanouts: HashMap<String, DeviceInfo>,
    /// Physical link wiring between DUT, fanout, and PTF ports.
    links: Vec<PhysicalLink>,
    /// PTF container device info.
    ptf: Option<DeviceInfo>,
    /// Server hosting the VMs.
    #[allow(dead_code)]
    server: Option<DeviceInfo>,
    /// When the testbed was created.
    created_at: DateTime<Utc>,
    /// When the state last changed.
    updated_at: DateTime<Utc>,
}

impl Testbed {
    /// Constructs a `Testbed` from a configuration, validating and
    /// populating the device map.
    pub fn from_config(config: TestbedConfig) -> sonic_core::Result<Self> {
        info!(name = %config.name, "building testbed from config");

        let default_creds = Credentials::new(
            config.default_user.as_deref().unwrap_or("admin"),
        )
        .with_password(
            config
                .default_password
                .as_deref()
                .unwrap_or("password"),
        );

        // -- build device map -----------------------------------------------
        let mut devices = HashMap::new();

        for dut in &config.duts {
            let ip: IpAddr = dut
                .mgmt_ip
                .parse()
                .map_err(|e| SonicError::config(format!("DUT {} bad IP: {e}", dut.hostname)))?;

            let mut info = DeviceInfo::new(
                &dut.hostname,
                ip,
                DeviceType::Sonic,
                default_creds.clone(),
            );
            info.hwsku = dut.hwsku.clone();
            info.platform = match dut.platform.to_lowercase().as_str() {
                "broadcom" => Platform::Broadcom,
                "mellanox" => Platform::Mellanox,
                "virtual" => Platform::Virtual,
                _ => Platform::Unknown,
            };
            devices.insert(dut.hostname.clone(), info);
        }

        for nbr in &config.neighbors {
            let ip: IpAddr = nbr.mgmt_ip.parse().map_err(|e| {
                SonicError::config(format!("neighbor {} bad IP: {e}", nbr.hostname))
            })?;

            let dt = match nbr.device_type.to_lowercase().as_str() {
                "eos" => DeviceType::Eos,
                "sonic" => DeviceType::Sonic,
                "cisco" => DeviceType::Cisco,
                _ => DeviceType::Eos,
            };

            let info = DeviceInfo::new(&nbr.hostname, ip, dt, default_creds.clone());
            devices.insert(nbr.hostname.clone(), info);
        }

        // -- Fanout switches ---------------------------------------------------
        let mut fanouts = HashMap::new();
        for fo in &config.fanouts {
            let ip: IpAddr = fo
                .mgmt_ip
                .parse()
                .map_err(|e| SonicError::config(format!("fanout {} bad IP: {e}", fo.hostname)))?;

            let mut info = DeviceInfo::new(
                &fo.hostname,
                ip,
                DeviceType::Fanout,
                default_creds.clone(),
            );
            info.hwsku = fo.hwsku.clone();
            info.platform = match fo.platform.to_lowercase().as_str() {
                "broadcom" => Platform::Broadcom,
                "mellanox" => Platform::Mellanox,
                "virtual" => Platform::Virtual,
                _ => Platform::Unknown,
            };
            fanouts.insert(fo.hostname.clone(), info);
        }

        let links = config.connection_graph.clone();

        // -- PTF ------------------------------------------------------------
        let ptf = if let Some(ptf_ip_str) = &config.ptf_ip {
            let ip: IpAddr = ptf_ip_str
                .parse()
                .map_err(|e| SonicError::config(format!("PTF bad IP: {e}")))?;
            let ptf_creds = Credentials::new(
                config.ptf_user.as_deref().unwrap_or("root"),
            );
            Some(DeviceInfo::new("ptf_host", ip, DeviceType::Ptf, ptf_creds))
        } else {
            None
        };

        // -- Server ---------------------------------------------------------
        let server = if let Some(srv) = &config.server {
            let ip: IpAddr = srv
                .parse()
                .map_err(|e| SonicError::config(format!("server bad IP: {e}")))?;
            Some(DeviceInfo::new(
                "server",
                ip,
                DeviceType::Sonic,
                default_creds.clone(),
            ))
        } else {
            None
        };

        let now = Utc::now();
        debug!(
            duts = config.duts.len(),
            neighbors = config.neighbors.len(),
            has_ptf = ptf.is_some(),
            "testbed built"
        );

        Ok(Self {
            name: config.name.clone(),
            config,
            topology: None,
            state: TestbedState::Available,
            devices,
            fanouts,
            links,
            ptf,
            server,
            created_at: now,
            updated_at: now,
        })
    }

    // -- public accessors ---------------------------------------------------

    /// Returns the testbed name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the active topology definition, if one is deployed.
    pub fn topology(&self) -> Option<&TopologyDefinition> {
        self.topology.as_ref()
    }

    /// Sets the active topology definition.
    pub fn set_topology(&mut self, topo: TopologyDefinition) {
        self.topology = Some(topo);
        self.touch();
    }

    /// Clears the topology definition.
    pub fn clear_topology(&mut self) {
        self.topology = None;
        self.touch();
    }

    /// Overwrites the testbed state.
    pub fn set_state(&mut self, state: TestbedState) {
        debug!(
            old = %self.state,
            new = %state,
            "state transition"
        );
        self.state = state;
        self.touch();
    }

    /// Returns a reference to the original configuration.
    pub fn config(&self) -> &TestbedConfig {
        &self.config
    }

    /// Returns the creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Returns the last-modified timestamp.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    // -- device accessors ---------------------------------------------------

    /// Returns the DUT with the given hostname.
    pub fn get_dut(&self, hostname: &str) -> sonic_core::Result<&DeviceInfo> {
        let info = self
            .devices
            .get(hostname)
            .ok_or_else(|| SonicError::DeviceNotFound(hostname.to_string()))?;

        if info.device_type != DeviceType::Sonic {
            return Err(SonicError::DeviceNotFound(format!(
                "{hostname} is not a DUT"
            )));
        }
        Ok(info)
    }

    /// Returns all DUT devices.
    pub fn get_all_duts(&self) -> Vec<&DeviceInfo> {
        self.devices
            .values()
            .filter(|d| d.device_type == DeviceType::Sonic)
            .collect()
    }

    /// Returns the PTF container device, if configured.
    pub fn get_ptf(&self) -> sonic_core::Result<&DeviceInfo> {
        self.ptf
            .as_ref()
            .ok_or_else(|| SonicError::DeviceNotFound("ptf_host".to_string()))
    }

    /// Returns all neighbor (non-DUT, non-PTF) devices.
    pub fn get_neighbors(&self) -> Vec<&DeviceInfo> {
        self.devices
            .values()
            .filter(|d| {
                d.device_type != DeviceType::Sonic && d.device_type != DeviceType::Ptf
            })
            .collect()
    }

    /// Returns the fanout device with the given hostname.
    pub fn get_fanout(&self, hostname: &str) -> sonic_core::Result<&DeviceInfo> {
        self.fanouts
            .get(hostname)
            .ok_or_else(|| SonicError::DeviceNotFound(hostname.to_string()))
    }

    /// Returns all fanout devices.
    pub fn get_all_fanouts(&self) -> Vec<&DeviceInfo> {
        self.fanouts.values().collect()
    }

    /// Returns the physical links in the connection graph.
    pub fn connection_graph(&self) -> &[PhysicalLink] {
        &self.links
    }

    /// Returns the physical link for a given DUT port, if one exists.
    pub fn link_for_dut_port(&self, dut_port: &str) -> Option<&PhysicalLink> {
        self.links.iter().find(|l| l.dut_port == dut_port)
    }

    /// Returns all physical links associated with a given fanout hostname.
    pub fn links_for_fanout(&self, fanout_host: &str) -> Vec<&PhysicalLink> {
        self.links
            .iter()
            .filter(|l| l.fanout_host == fanout_host)
            .collect()
    }

    /// Returns all device hostnames.
    pub fn all_device_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.devices.keys().cloned().collect();
        for fo in self.fanouts.keys() {
            names.push(fo.clone());
        }
        if let Some(ptf) = &self.ptf {
            names.push(ptf.hostname.clone());
        }
        names
    }

    // -- internal -----------------------------------------------------------

    fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

// ---------------------------------------------------------------------------
// TestbedManager trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl TestbedManager for Testbed {
    /// Deploys the topology specified in the testbed configuration.
    ///
    /// Orchestrates VM creation, topology setup, PTF container start, and
    /// route announcement.  State transitions: `Available` -> `Deploying` ->
    /// `Available` (or `Error` on failure).
    async fn deploy(&self) -> sonic_core::Result<()> {
        // Because `TestbedManager::deploy` takes `&self` (per the trait),
        // we cannot mutate state here.  In production this would use
        // interior mutability (e.g., RwLock on state).  We log the intent.
        info!(testbed = %self.name, "deploy requested");

        let topo_type: TopologyType = parse_topo_type(&self.config.topology)?;

        let generator = sonic_topology::DefaultTopologyGenerator::veos();
        let topo_def = generator.generate(topo_type)?;

        info!(
            vms = topo_def.vms.len(),
            vlans = topo_def.vlans.len(),
            host_ifs = topo_def.host_interfaces.len(),
            "topology generated"
        );

        // Simulate deploy steps.
        debug!("step 1/4: creating VMs (simulated)");
        debug!("step 2/4: starting PTF container (simulated)");
        debug!("step 3/4: pushing base configs (simulated)");
        debug!("step 4/4: announcing routes (simulated)");

        info!(testbed = %self.name, "deploy complete (simulated)");
        Ok(())
    }

    /// Tears down the testbed (stop VMs, remove topology, stop PTF).
    async fn teardown(&self) -> sonic_core::Result<()> {
        info!(testbed = %self.name, "teardown requested");

        debug!("step 1/3: stopping VMs (simulated)");
        debug!("step 2/3: removing topology config (simulated)");
        debug!("step 3/3: stopping PTF container (simulated)");

        info!(testbed = %self.name, "teardown complete (simulated)");
        Ok(())
    }

    /// Pushes minigraph / golden config to the DUT and triggers a config
    /// reload.
    async fn deploy_config(&self) -> sonic_core::Result<()> {
        info!(testbed = %self.name, "deploy_config requested");

        let topo = self
            .topology
            .as_ref()
            .ok_or_else(|| SonicError::testbed("no topology deployed"))?;

        let renderer = sonic_topology::TopologyRenderer::default();
        let _minigraph = renderer.render_minigraph(topo)?;

        for dut in self.get_all_duts() {
            debug!(dut = %dut.hostname, "pushing minigraph (simulated)");
            debug!(dut = %dut.hostname, "config reload (simulated)");
        }

        info!(testbed = %self.name, "config deployed (simulated)");
        Ok(())
    }

    /// Runs a health check across all devices and returns the aggregate
    /// status.
    async fn health_check(&self) -> sonic_core::Result<HealthStatus> {
        info!(testbed = %self.name, "health check requested");

        let checker = HealthChecker::new();
        let all_devices: Vec<DeviceInfo> = self
            .devices
            .values()
            .cloned()
            .chain(self.fanouts.values().cloned())
            .chain(self.ptf.clone())
            .collect();

        if all_devices.is_empty() {
            return Ok(HealthStatus::Unknown);
        }

        let health = checker.check_testbed(&all_devices).await?;
        info!(overall = %health.overall, "health check complete");
        Ok(health.overall)
    }

    /// Triggers BGP route announcement from neighbor VMs.
    async fn announce_routes(&self) -> sonic_core::Result<()> {
        let neighbors = self.get_neighbors();
        info!(
            testbed = %self.name,
            neighbor_count = neighbors.len(),
            "announcing routes"
        );

        for nbr in &neighbors {
            debug!(neighbor = %nbr.hostname, "route announce (simulated)");
        }

        Ok(())
    }

    /// Returns the current testbed state.
    fn state(&self) -> TestbedState {
        self.state
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parses a topology type string into a `TopologyType` enum.
fn parse_topo_type(s: &str) -> sonic_core::Result<TopologyType> {
    match s.to_lowercase().as_str() {
        "t0" => Ok(TopologyType::T0),
        "t0-64" => Ok(TopologyType::T064),
        "t0-116" => Ok(TopologyType::T0116),
        "t1" => Ok(TopologyType::T1),
        "t1-64" => Ok(TopologyType::T164),
        "t1-lag" => Ok(TopologyType::T1Lag),
        "t2" => Ok(TopologyType::T2),
        "dualtor" => Ok(TopologyType::Dualtor),
        "mgmt-tor" => Ok(TopologyType::MgmtTor),
        "m0-vlan" => Ok(TopologyType::M0Vlan),
        "ptf-32" | "ptf32" => Ok(TopologyType::Ptf32),
        "ptf-64" | "ptf64" => Ok(TopologyType::Ptf64),
        "ptf" => Ok(TopologyType::Ptf),
        "any" => Ok(TopologyType::Any),
        other => Err(SonicError::UnsupportedTopology(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> TestbedConfig {
        TestbedConfig {
            name: "vms-t0-1".into(),
            topology: "t0".into(),
            duts: vec![DutConfig {
                hostname: "dut1".into(),
                mgmt_ip: "10.0.0.1".into(),
                hwsku: "ACS-MSN2700".into(),
                platform: "mellanox".into(),
            }],
            neighbors: vec![NeighborConfig {
                hostname: "ARISTA00".into(),
                mgmt_ip: "10.250.0.2".into(),
                device_type: "eos".into(),
            }],
            ptf_ip: Some("10.0.0.100".into()),
            ptf_user: None,
            server: Some("10.0.0.200".into()),
            default_user: Some("admin".into()),
            default_password: Some("password".into()),
            fanouts: vec![],
            connection_graph: vec![],
        }
    }

    #[test]
    fn from_config_builds_devices() {
        let tb = Testbed::from_config(sample_config()).unwrap();
        assert_eq!(tb.name(), "vms-t0-1");
        assert_eq!(tb.state(), TestbedState::Available);
        assert_eq!(tb.get_all_duts().len(), 1);
        assert_eq!(tb.get_neighbors().len(), 1);
        assert!(tb.get_ptf().is_ok());
    }

    #[test]
    fn get_dut_by_name() {
        let tb = Testbed::from_config(sample_config()).unwrap();
        let dut = tb.get_dut("dut1").unwrap();
        assert_eq!(dut.device_type, DeviceType::Sonic);
    }

    #[test]
    fn get_dut_not_found() {
        let tb = Testbed::from_config(sample_config()).unwrap();
        assert!(tb.get_dut("nonexistent").is_err());
    }

    #[test]
    fn neighbor_is_not_dut() {
        let tb = Testbed::from_config(sample_config()).unwrap();
        assert!(tb.get_dut("ARISTA00").is_err());
    }

    #[test]
    fn parse_topo_types() {
        assert_eq!(parse_topo_type("t0").unwrap(), TopologyType::T0);
        assert_eq!(parse_topo_type("T1-LAG").unwrap(), TopologyType::T1Lag);
        assert_eq!(parse_topo_type("dualtor").unwrap(), TopologyType::Dualtor);
        assert!(parse_topo_type("invalid").is_err());
    }

    #[test]
    fn state_transitions() {
        let mut tb = Testbed::from_config(sample_config()).unwrap();
        assert_eq!(tb.state(), TestbedState::Available);
        tb.set_state(TestbedState::Deploying);
        assert_eq!(tb.state(), TestbedState::Deploying);
    }

    #[test]
    fn topology_set_clear() {
        let mut tb = Testbed::from_config(sample_config()).unwrap();
        assert!(tb.topology().is_none());

        let gen = sonic_topology::DefaultTopologyGenerator::veos();
        let def = gen.generate(TopologyType::T0).unwrap();
        tb.set_topology(def);
        assert!(tb.topology().is_some());

        tb.clear_topology();
        assert!(tb.topology().is_none());
    }
}
