use async_trait::async_trait;

use crate::error::Result;
use crate::types::*;

/// Core trait for all network devices (DUTs, neighbors, PTF, fanout, etc.)
#[async_trait]
pub trait Device: Send + Sync {
    /// Returns static device information.
    fn info(&self) -> &DeviceInfo;

    /// Establishes a connection to the device.
    async fn connect(&mut self) -> Result<()>;

    /// Tears down the connection.
    async fn disconnect(&mut self) -> Result<()>;

    /// Checks whether the connection is still alive.
    async fn is_connected(&self) -> bool;

    /// Executes a CLI command and returns the result.
    async fn execute(&self, command: &str) -> Result<CommandResult>;

    /// Executes a CLI command, returning an error on non-zero exit code.
    async fn execute_checked(&self, command: &str) -> Result<CommandResult> {
        let result = self.execute(command).await?;
        if result.success() {
            Ok(result)
        } else {
            Err(crate::error::SonicError::CommandExecution {
                command: command.to_string(),
                exit_code: result.exit_code,
                stderr: result.stderr.clone(),
            })
        }
    }

    /// Reboots the device using the specified reboot strategy.
    async fn reboot(&self, reboot_type: RebootType) -> Result<()>;

    /// Waits for the device to become reachable (e.g., after reboot).
    async fn wait_ready(&self, timeout_secs: u64) -> Result<()>;

    /// Returns the device hostname.
    fn hostname(&self) -> &str {
        &self.info().hostname
    }

    /// Returns the device type.
    fn device_type(&self) -> DeviceType {
        self.info().device_type
    }
}

/// Trait for collecting device facts (BGP, interfaces, config, etc.)
#[async_trait]
pub trait FactsProvider: Send + Sync {
    /// Collects basic system facts (hostname, HW SKU, OS version, etc.)
    async fn basic_facts(&self) -> Result<BasicFacts>;

    /// Collects BGP neighbor and route facts.
    async fn bgp_facts(&self) -> Result<BgpFacts>;

    /// Collects interface, VLAN, LAG, and loopback facts.
    async fn interface_facts(&self) -> Result<InterfaceFacts>;

    /// Collects running/startup configuration facts.
    async fn config_facts(&self) -> Result<ConfigFacts>;
}

/// Low-level transport connection (SSH session, telnet socket, console, etc.)
#[async_trait]
pub trait Connection: Send + Sync {
    /// Opens the transport.
    async fn open(&mut self) -> Result<()>;

    /// Closes the transport.
    async fn close(&mut self) -> Result<()>;

    /// Sends a raw command string and returns the output.
    async fn send(&self, data: &str) -> Result<String>;

    /// Sends a command and waits for a prompt, returning the output.
    async fn send_command(&self, command: &str) -> Result<CommandResult>;

    /// Checks whether the transport is still alive.
    async fn is_alive(&self) -> bool;

    /// Returns the connection type.
    fn connection_type(&self) -> ConnectionType;
}

/// Trait for managing testbed lifecycle.
#[async_trait]
pub trait TestbedManager: Send + Sync {
    /// Deploys (creates / brings up) the testbed topology.
    async fn deploy(&self) -> Result<()>;

    /// Tears down (removes) the topology.
    async fn teardown(&self) -> Result<()>;

    /// Redeploys minigraph / golden config on the DUT.
    async fn deploy_config(&self) -> Result<()>;

    /// Runs a health check across all devices in the testbed.
    async fn health_check(&self) -> Result<HealthStatus>;

    /// Announces BGP routes from the neighbor VMs.
    async fn announce_routes(&self) -> Result<()>;

    /// Returns the current testbed state.
    fn state(&self) -> TestbedState;
}

/// Trait for topology generation.
pub trait TopologyGenerator: Send + Sync {
    /// Generates a complete topology definition for the given type.
    fn generate(&self, topo_type: TopologyType) -> Result<TopologyDefinition>;

    /// Lists all supported topology types.
    fn supported_topologies(&self) -> Vec<TopologyType>;
}

/// A complete topology definition produced by a generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyDefinition {
    pub topo_type: TopologyType,
    pub vms: Vec<VmDefinition>,
    pub vlans: Vec<VlanDefinition>,
    pub host_interfaces: Vec<HostInterfaceDefinition>,
    pub lag_links: Vec<LagLinkDefinition>,
    pub ip_pairs: Vec<IpPairAllocation>,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmDefinition {
    pub name: String,
    pub vm_type: VmType,
    pub vm_offset: u32,
    pub mgmt_ip: std::net::IpAddr,
    pub peer_ports: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanDefinition {
    pub id: u16,
    pub name: String,
    pub intfs: Vec<String>,
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInterfaceDefinition {
    pub vm_index: u32,
    pub port_index: u32,
    pub dut_port: String,
    pub ptf_port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LagLinkDefinition {
    pub lag_id: u32,
    pub members: Vec<String>,
    pub vm_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpPairAllocation {
    pub vm_index: u32,
    pub dut_ip: IpPair,
    pub neighbor_ip: IpPair,
}

/// Trait for the test runner engine.
#[async_trait]
pub trait TestRunner: Send + Sync {
    /// Discovers all test cases matching the provided filters.
    async fn discover(&self, filter: &TestFilter) -> Result<Vec<TestCase>>;

    /// Runs a set of test cases and returns results.
    async fn run(&self, cases: &[TestCase]) -> Result<Vec<TestCaseResult>>;

    /// Stops a running test execution.
    async fn stop(&self) -> Result<()>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestFilter {
    pub patterns: Vec<String>,
    pub tags: Vec<String>,
    pub topologies: Vec<TopologyType>,
    pub platforms: Vec<Platform>,
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub id: String,
    pub name: String,
    pub module: String,
    pub tags: Vec<String>,
    pub topology: Option<TopologyType>,
    pub platform: Option<Platform>,
    pub description: Option<String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCaseResult {
    pub test_case: TestCase,
    pub outcome: TestOutcome,
    pub duration: std::time::Duration,
    pub message: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: chrono::DateTime<chrono::Utc>,
}

/// Trait for uploading test reports to external storage/analytics.
#[async_trait]
pub trait ReportUploader: Send + Sync {
    /// Uploads a batch of test results.
    async fn upload(&self, results: &[TestCaseResult]) -> Result<()>;

    /// Checks connectivity to the reporting backend.
    async fn check_connection(&self) -> Result<()>;
}
