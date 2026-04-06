use chrono::{DateTime, Utc};
use ipnetwork::{Ipv4Network, Ipv6Network};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Device types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Sonic,
    Eos,
    Cisco,
    Fanout,
    Ptf,
    K8sMaster,
    Aos,
    Cumulus,
    Onie,
}

impl fmt::Display for DeviceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sonic => write!(f, "SONiC"),
            Self::Eos => write!(f, "Arista EOS"),
            Self::Cisco => write!(f, "Cisco"),
            Self::Fanout => write!(f, "Fanout"),
            Self::Ptf => write!(f, "PTF"),
            Self::K8sMaster => write!(f, "K8s Master"),
            Self::Aos => write!(f, "AOS"),
            Self::Cumulus => write!(f, "Cumulus"),
            Self::Onie => write!(f, "ONIE"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionType {
    Ssh,
    Telnet,
    Console,
    Grpc,
    Local,
}

impl fmt::Display for ConnectionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ssh => write!(f, "SSH"),
            Self::Telnet => write!(f, "Telnet"),
            Self::Console => write!(f, "Console"),
            Self::Grpc => write!(f, "gRPC"),
            Self::Local => write!(f, "Local"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Broadcom,
    Mellanox,
    Barefoot,
    Marvell,
    Nokia,
    Cisco,
    Centec,
    Virtual,
    Unknown,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Broadcom => "Broadcom",
            Self::Mellanox => "Mellanox",
            Self::Barefoot => "Barefoot",
            Self::Marvell => "Marvell",
            Self::Nokia => "Nokia",
            Self::Cisco => "Cisco",
            Self::Centec => "Centec",
            Self::Virtual => "Virtual",
            Self::Unknown => "Unknown",
        };
        write!(f, "{name}")
    }
}

// ---------------------------------------------------------------------------
// Topology
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyType {
    T0,
    T064,
    T0116,
    T1,
    T164,
    T1Lag,
    T2,
    Dualtor,
    MgmtTor,
    M0Vlan,
    Ptf32,
    Ptf64,
    Ptf,
    Any,
}

impl fmt::Display for TopologyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::T0 => write!(f, "t0"),
            Self::T064 => write!(f, "t0-64"),
            Self::T0116 => write!(f, "t0-116"),
            Self::T1 => write!(f, "t1"),
            Self::T164 => write!(f, "t1-64"),
            Self::T1Lag => write!(f, "t1-lag"),
            Self::T2 => write!(f, "t2"),
            Self::Dualtor => write!(f, "dualtor"),
            Self::MgmtTor => write!(f, "mgmt-tor"),
            Self::M0Vlan => write!(f, "m0-vlan"),
            Self::Ptf32 => write!(f, "ptf-32"),
            Self::Ptf64 => write!(f, "ptf-64"),
            Self::Ptf => write!(f, "ptf"),
            Self::Any => write!(f, "any"),
        }
    }
}

impl TopologyType {
    pub fn vm_count(&self) -> usize {
        match self {
            Self::T0 => 4,
            Self::T064 | Self::T164 => 64,
            Self::T0116 => 116,
            Self::T1 => 32,
            Self::T1Lag => 32,
            Self::T2 => 64,
            Self::Dualtor => 4,
            Self::MgmtTor => 4,
            Self::M0Vlan => 4,
            Self::Ptf32 => 0,
            Self::Ptf64 => 0,
            Self::Ptf => 0,
            Self::Any => 0,
        }
    }

    pub fn requires_vms(&self) -> bool {
        self.vm_count() > 0
    }

    pub fn is_ptf_only(&self) -> bool {
        matches!(self, Self::Ptf | Self::Ptf32 | Self::Ptf64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmType {
    Veos,
    Ceos,
    Vsonic,
    Vcisco,
    Csonic,
}

impl fmt::Display for VmType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Veos => write!(f, "veos"),
            Self::Ceos => write!(f, "ceos"),
            Self::Vsonic => write!(f, "vsonic"),
            Self::Vcisco => write!(f, "vcisco"),
            Self::Csonic => write!(f, "csonic"),
        }
    }
}

// ---------------------------------------------------------------------------
// Reboot and operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebootType {
    Cold,
    Warm,
    Fast,
    PowerCycle,
    Watchdog,
    Supervisor,
    Kdump,
}

impl fmt::Display for RebootType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cold => write!(f, "cold"),
            Self::Warm => write!(f, "warm"),
            Self::Fast => write!(f, "fast"),
            Self::PowerCycle => write!(f, "power-cycle"),
            Self::Watchdog => write!(f, "watchdog"),
            Self::Supervisor => write!(f, "supervisor"),
            Self::Kdump => write!(f, "kdump"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigReloadType {
    Reload,
    LoadMinigraph,
    GoldenConfig,
    FactoryReset,
}

// ---------------------------------------------------------------------------
// Test result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    Passed,
    Failed,
    Skipped,
    Error,
    XFail,
    XPass,
}

impl fmt::Display for TestOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Passed => write!(f, "PASSED"),
            Self::Failed => write!(f, "FAILED"),
            Self::Skipped => write!(f, "SKIPPED"),
            Self::Error => write!(f, "ERROR"),
            Self::XFail => write!(f, "XFAIL"),
            Self::XPass => write!(f, "XPASS"),
        }
    }
}

// ---------------------------------------------------------------------------
// Core data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    #[serde(skip_serializing)]
    pub password: Option<String>,
    pub key_path: Option<String>,
    pub passphrase: Option<String>,
}

impl Credentials {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: None,
            key_path: None,
            passphrase: None,
        }
    }

    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    pub fn with_key(mut self, key_path: impl Into<String>) -> Self {
        self.key_path = Some(key_path.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: Uuid,
    pub hostname: String,
    pub mgmt_ip: IpAddr,
    pub device_type: DeviceType,
    pub platform: Platform,
    pub hwsku: String,
    pub os_version: Option<String>,
    pub serial: Option<String>,
    pub model: Option<String>,
    pub credentials: Credentials,
    pub connection_type: ConnectionType,
    pub port: u16,
    pub console_server: Option<ConsoleInfo>,
    pub metadata: HashMap<String, String>,
}

impl DeviceInfo {
    pub fn new(
        hostname: impl Into<String>,
        mgmt_ip: IpAddr,
        device_type: DeviceType,
        credentials: Credentials,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            hostname: hostname.into(),
            mgmt_ip,
            device_type,
            platform: Platform::Unknown,
            hwsku: String::new(),
            os_version: None,
            serial: None,
            model: None,
            credentials,
            connection_type: ConnectionType::Ssh,
            port: 22,
            console_server: None,
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleInfo {
    pub server: String,
    pub port: u16,
    pub protocol: ConnectionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration: std::time::Duration,
    pub command: String,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    pub fn stdout_lines(&self) -> Vec<&str> {
        self.stdout.lines().collect()
    }
}

// ---------------------------------------------------------------------------
// Network primitives
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpPair {
    pub ipv4: Option<Ipv4Addr>,
    pub ipv6: Option<Ipv6Addr>,
    pub ipv4_prefix: Option<Ipv4Network>,
    pub ipv6_prefix: Option<Ipv6Network>,
}

impl IpPair {
    pub fn v4_only(addr: Ipv4Addr, prefix: Ipv4Network) -> Self {
        Self {
            ipv4: Some(addr),
            ipv6: None,
            ipv4_prefix: Some(prefix),
            ipv6_prefix: None,
        }
    }

    pub fn dual_stack(
        v4: Ipv4Addr,
        v4_prefix: Ipv4Network,
        v6: Ipv6Addr,
        v6_prefix: Ipv6Network,
    ) -> Self {
        Self {
            ipv4: Some(v4),
            ipv6: Some(v6),
            ipv4_prefix: Some(v4_prefix),
            ipv6_prefix: Some(v6_prefix),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortInfo {
    pub name: String,
    pub alias: Option<String>,
    pub index: u32,
    pub speed: u64,
    pub lanes: Vec<u32>,
    pub mtu: u16,
    pub admin_status: PortStatus,
    pub oper_status: PortStatus,
    pub fec: Option<String>,
    pub autoneg: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortStatus {
    Up,
    Down,
    NotPresent,
}

impl fmt::Display for PortStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Up => write!(f, "up"),
            Self::Down => write!(f, "down"),
            Self::NotPresent => write!(f, "not-present"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanInfo {
    pub id: u16,
    pub name: String,
    pub members: Vec<VlanMember>,
    pub ip_addresses: Vec<IpPair>,
    pub dhcp_servers: Vec<IpAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanMember {
    pub port: String,
    pub tagging_mode: TaggingMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaggingMode {
    Tagged,
    Untagged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LagInfo {
    pub name: String,
    pub members: Vec<String>,
    pub min_links: u32,
    pub lacp_mode: LacpMode,
    pub admin_status: PortStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LacpMode {
    Active,
    Passive,
    On,
}

// ---------------------------------------------------------------------------
// BGP types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BgpNeighbor {
    pub address: IpAddr,
    pub remote_as: u32,
    pub local_as: u32,
    pub state: BgpState,
    pub description: Option<String>,
    pub hold_time: u32,
    pub keepalive: u32,
    pub prefixes_received: u64,
    pub prefixes_sent: u64,
    pub up_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BgpState {
    Idle,
    Connect,
    Active,
    OpenSent,
    OpenConfirm,
    Established,
}

impl fmt::Display for BgpState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Connect => write!(f, "Connect"),
            Self::Active => write!(f, "Active"),
            Self::OpenSent => write!(f, "OpenSent"),
            Self::OpenConfirm => write!(f, "OpenConfirm"),
            Self::Established => write!(f, "Established"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BgpRoute {
    pub prefix: String,
    pub next_hop: IpAddr,
    pub metric: u32,
    pub local_pref: u32,
    pub as_path: Vec<u32>,
    pub origin: BgpOrigin,
    pub communities: Vec<String>,
    pub valid: bool,
    pub best: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BgpOrigin {
    Igp,
    Egp,
    Incomplete,
}

// ---------------------------------------------------------------------------
// ACL types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclTable {
    pub name: String,
    pub table_type: AclTableType,
    pub stage: AclStage,
    pub ports: Vec<String>,
    pub rules: Vec<AclRule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AclTableType {
    L3,
    L3V6,
    Mirror,
    MirrorDscp,
    Pfcwd,
    Ctrlplane,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclStage {
    Ingress,
    Egress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclRule {
    pub name: String,
    pub priority: u32,
    pub action: AclAction,
    pub src_ip: Option<String>,
    pub dst_ip: Option<String>,
    pub src_port: Option<String>,
    pub dst_port: Option<String>,
    pub protocol: Option<String>,
    pub ether_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AclAction {
    Forward,
    Drop,
    Redirect,
    MirrorIngress,
    MirrorEgress,
}

// ---------------------------------------------------------------------------
// Facts aggregates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BasicFacts {
    pub hostname: String,
    pub hwsku: String,
    pub platform: String,
    pub os_version: String,
    pub serial_number: String,
    pub model: String,
    pub mac_address: String,
    pub uptime: u64,
    pub asic_type: String,
    pub kernel_version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BgpFacts {
    pub router_id: String,
    pub local_as: u32,
    pub neighbors: Vec<BgpNeighbor>,
    pub routes_ipv4: Vec<BgpRoute>,
    pub routes_ipv6: Vec<BgpRoute>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InterfaceFacts {
    pub ports: Vec<PortInfo>,
    pub vlans: Vec<VlanInfo>,
    pub lags: Vec<LagInfo>,
    pub loopbacks: Vec<LoopbackInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopbackInfo {
    pub name: String,
    pub ip_addresses: Vec<IpPair>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFacts {
    pub running_config: HashMap<String, serde_json::Value>,
    pub startup_config: HashMap<String, serde_json::Value>,
    pub features: HashMap<String, FeatureState>,
    pub services: Vec<ServiceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureState {
    pub name: String,
    pub state: String,
    pub auto_restart: bool,
    pub high_mem_alert: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub name: String,
    pub status: String,
    pub pid: Option<u32>,
}

// ---------------------------------------------------------------------------
// Testbed types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestbedState {
    Available,
    InUse,
    Deploying,
    Error,
    Maintenance,
    Destroyed,
}

impl fmt::Display for TestbedState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available => write!(f, "available"),
            Self::InUse => write!(f, "in-use"),
            Self::Deploying => write!(f, "deploying"),
            Self::Error => write!(f, "error"),
            Self::Maintenance => write!(f, "maintenance"),
            Self::Destroyed => write!(f, "destroyed"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

impl fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

// ---------------------------------------------------------------------------
// Report / analytics types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportFormat {
    JunitXml,
    Json,
    Toml,
    Csv,
    Html,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    AppKey,
    ManagedIdentity,
    AzureDefault,
    AzureCli,
    DeviceCode,
    UserToken,
    AppToken,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::time::Duration;

    // -----------------------------------------------------------------------
    // TopologyType::vm_count()
    // -----------------------------------------------------------------------

    #[test]
    fn topology_vm_count_t0() {
        assert_eq!(TopologyType::T0.vm_count(), 4);
    }

    #[test]
    fn topology_vm_count_t064() {
        assert_eq!(TopologyType::T064.vm_count(), 64);
    }

    #[test]
    fn topology_vm_count_t0116() {
        assert_eq!(TopologyType::T0116.vm_count(), 116);
    }

    #[test]
    fn topology_vm_count_t1() {
        assert_eq!(TopologyType::T1.vm_count(), 32);
    }

    #[test]
    fn topology_vm_count_t164() {
        assert_eq!(TopologyType::T164.vm_count(), 64);
    }

    #[test]
    fn topology_vm_count_t1_lag() {
        assert_eq!(TopologyType::T1Lag.vm_count(), 32);
    }

    #[test]
    fn topology_vm_count_t2() {
        assert_eq!(TopologyType::T2.vm_count(), 64);
    }

    #[test]
    fn topology_vm_count_dualtor() {
        assert_eq!(TopologyType::Dualtor.vm_count(), 4);
    }

    #[test]
    fn topology_vm_count_ptf_variants_are_zero() {
        assert_eq!(TopologyType::Ptf.vm_count(), 0);
        assert_eq!(TopologyType::Ptf32.vm_count(), 0);
        assert_eq!(TopologyType::Ptf64.vm_count(), 0);
    }

    #[test]
    fn topology_vm_count_any_is_zero() {
        assert_eq!(TopologyType::Any.vm_count(), 0);
    }

    // -----------------------------------------------------------------------
    // TopologyType::requires_vms()
    // -----------------------------------------------------------------------

    #[test]
    fn topology_requires_vms_true_for_t0() {
        assert!(TopologyType::T0.requires_vms());
    }

    #[test]
    fn topology_requires_vms_true_for_t1() {
        assert!(TopologyType::T1.requires_vms());
    }

    #[test]
    fn topology_requires_vms_false_for_ptf() {
        assert!(!TopologyType::Ptf.requires_vms());
    }

    #[test]
    fn topology_requires_vms_false_for_any() {
        assert!(!TopologyType::Any.requires_vms());
    }

    // -----------------------------------------------------------------------
    // TopologyType::is_ptf_only()
    // -----------------------------------------------------------------------

    #[test]
    fn topology_is_ptf_only_true_for_ptf() {
        assert!(TopologyType::Ptf.is_ptf_only());
    }

    #[test]
    fn topology_is_ptf_only_true_for_ptf32() {
        assert!(TopologyType::Ptf32.is_ptf_only());
    }

    #[test]
    fn topology_is_ptf_only_true_for_ptf64() {
        assert!(TopologyType::Ptf64.is_ptf_only());
    }

    #[test]
    fn topology_is_ptf_only_false_for_t0() {
        assert!(!TopologyType::T0.is_ptf_only());
    }

    #[test]
    fn topology_is_ptf_only_false_for_t1() {
        assert!(!TopologyType::T1.is_ptf_only());
    }

    #[test]
    fn topology_is_ptf_only_false_for_any() {
        assert!(!TopologyType::Any.is_ptf_only());
    }

    // -----------------------------------------------------------------------
    // Display impls
    // -----------------------------------------------------------------------

    #[test]
    fn display_device_type() {
        assert_eq!(DeviceType::Sonic.to_string(), "SONiC");
        assert_eq!(DeviceType::Eos.to_string(), "Arista EOS");
        assert_eq!(DeviceType::Cisco.to_string(), "Cisco");
        assert_eq!(DeviceType::Fanout.to_string(), "Fanout");
        assert_eq!(DeviceType::Ptf.to_string(), "PTF");
        assert_eq!(DeviceType::K8sMaster.to_string(), "K8s Master");
        assert_eq!(DeviceType::Aos.to_string(), "AOS");
        assert_eq!(DeviceType::Cumulus.to_string(), "Cumulus");
        assert_eq!(DeviceType::Onie.to_string(), "ONIE");
    }

    #[test]
    fn display_connection_type() {
        assert_eq!(ConnectionType::Ssh.to_string(), "SSH");
        assert_eq!(ConnectionType::Telnet.to_string(), "Telnet");
        assert_eq!(ConnectionType::Console.to_string(), "Console");
        assert_eq!(ConnectionType::Grpc.to_string(), "gRPC");
        assert_eq!(ConnectionType::Local.to_string(), "Local");
    }

    #[test]
    fn display_platform() {
        assert_eq!(Platform::Broadcom.to_string(), "Broadcom");
        assert_eq!(Platform::Mellanox.to_string(), "Mellanox");
        assert_eq!(Platform::Barefoot.to_string(), "Barefoot");
        assert_eq!(Platform::Marvell.to_string(), "Marvell");
        assert_eq!(Platform::Nokia.to_string(), "Nokia");
        assert_eq!(Platform::Cisco.to_string(), "Cisco");
        assert_eq!(Platform::Centec.to_string(), "Centec");
        assert_eq!(Platform::Virtual.to_string(), "Virtual");
        assert_eq!(Platform::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn display_topology_type() {
        assert_eq!(TopologyType::T0.to_string(), "t0");
        assert_eq!(TopologyType::T064.to_string(), "t0-64");
        assert_eq!(TopologyType::T0116.to_string(), "t0-116");
        assert_eq!(TopologyType::T1.to_string(), "t1");
        assert_eq!(TopologyType::T164.to_string(), "t1-64");
        assert_eq!(TopologyType::T1Lag.to_string(), "t1-lag");
        assert_eq!(TopologyType::T2.to_string(), "t2");
        assert_eq!(TopologyType::Dualtor.to_string(), "dualtor");
        assert_eq!(TopologyType::MgmtTor.to_string(), "mgmt-tor");
        assert_eq!(TopologyType::M0Vlan.to_string(), "m0-vlan");
        assert_eq!(TopologyType::Ptf32.to_string(), "ptf-32");
        assert_eq!(TopologyType::Ptf64.to_string(), "ptf-64");
        assert_eq!(TopologyType::Ptf.to_string(), "ptf");
        assert_eq!(TopologyType::Any.to_string(), "any");
    }

    #[test]
    fn display_vm_type() {
        assert_eq!(VmType::Veos.to_string(), "veos");
        assert_eq!(VmType::Ceos.to_string(), "ceos");
        assert_eq!(VmType::Vsonic.to_string(), "vsonic");
        assert_eq!(VmType::Vcisco.to_string(), "vcisco");
        assert_eq!(VmType::Csonic.to_string(), "csonic");
    }

    #[test]
    fn display_reboot_type() {
        assert_eq!(RebootType::Cold.to_string(), "cold");
        assert_eq!(RebootType::Warm.to_string(), "warm");
        assert_eq!(RebootType::Fast.to_string(), "fast");
        assert_eq!(RebootType::PowerCycle.to_string(), "power-cycle");
        assert_eq!(RebootType::Watchdog.to_string(), "watchdog");
        assert_eq!(RebootType::Supervisor.to_string(), "supervisor");
        assert_eq!(RebootType::Kdump.to_string(), "kdump");
    }

    #[test]
    fn display_test_outcome() {
        assert_eq!(TestOutcome::Passed.to_string(), "PASSED");
        assert_eq!(TestOutcome::Failed.to_string(), "FAILED");
        assert_eq!(TestOutcome::Skipped.to_string(), "SKIPPED");
        assert_eq!(TestOutcome::Error.to_string(), "ERROR");
        assert_eq!(TestOutcome::XFail.to_string(), "XFAIL");
        assert_eq!(TestOutcome::XPass.to_string(), "XPASS");
    }

    #[test]
    fn display_bgp_state() {
        assert_eq!(BgpState::Idle.to_string(), "Idle");
        assert_eq!(BgpState::Connect.to_string(), "Connect");
        assert_eq!(BgpState::Active.to_string(), "Active");
        assert_eq!(BgpState::OpenSent.to_string(), "OpenSent");
        assert_eq!(BgpState::OpenConfirm.to_string(), "OpenConfirm");
        assert_eq!(BgpState::Established.to_string(), "Established");
    }

    #[test]
    fn display_port_status() {
        assert_eq!(PortStatus::Up.to_string(), "up");
        assert_eq!(PortStatus::Down.to_string(), "down");
        assert_eq!(PortStatus::NotPresent.to_string(), "not-present");
    }

    #[test]
    fn display_testbed_state() {
        assert_eq!(TestbedState::Available.to_string(), "available");
        assert_eq!(TestbedState::InUse.to_string(), "in-use");
        assert_eq!(TestbedState::Deploying.to_string(), "deploying");
        assert_eq!(TestbedState::Error.to_string(), "error");
        assert_eq!(TestbedState::Maintenance.to_string(), "maintenance");
        assert_eq!(TestbedState::Destroyed.to_string(), "destroyed");
    }

    #[test]
    fn display_health_status() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Degraded.to_string(), "degraded");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
        assert_eq!(HealthStatus::Unknown.to_string(), "unknown");
    }

    // -----------------------------------------------------------------------
    // Credentials
    // -----------------------------------------------------------------------

    #[test]
    fn credentials_new_sets_username_only() {
        let creds = Credentials::new("admin");
        assert_eq!(creds.username, "admin");
        assert!(creds.password.is_none());
        assert!(creds.key_path.is_none());
        assert!(creds.passphrase.is_none());
    }

    #[test]
    fn credentials_with_password() {
        let creds = Credentials::new("admin").with_password("secret");
        assert_eq!(creds.username, "admin");
        assert_eq!(creds.password.as_deref(), Some("secret"));
        assert!(creds.key_path.is_none());
    }

    #[test]
    fn credentials_with_key() {
        let creds = Credentials::new("admin").with_key("/home/user/.ssh/id_rsa");
        assert_eq!(creds.username, "admin");
        assert!(creds.password.is_none());
        assert_eq!(creds.key_path.as_deref(), Some("/home/user/.ssh/id_rsa"));
    }

    #[test]
    fn credentials_builder_chain() {
        let creds = Credentials::new("user")
            .with_password("pass")
            .with_key("/key");
        assert_eq!(creds.username, "user");
        assert_eq!(creds.password.as_deref(), Some("pass"));
        assert_eq!(creds.key_path.as_deref(), Some("/key"));
    }

    // -----------------------------------------------------------------------
    // DeviceInfo
    // -----------------------------------------------------------------------

    #[test]
    fn device_info_new_defaults() {
        let creds = Credentials::new("admin");
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let dev = DeviceInfo::new("switch-01", ip, DeviceType::Sonic, creds);

        assert_eq!(dev.hostname, "switch-01");
        assert_eq!(dev.mgmt_ip, ip);
        assert_eq!(dev.device_type, DeviceType::Sonic);
        assert_eq!(dev.platform, Platform::Unknown);
        assert_eq!(dev.hwsku, "");
        assert!(dev.os_version.is_none());
        assert!(dev.serial.is_none());
        assert!(dev.model.is_none());
        assert_eq!(dev.connection_type, ConnectionType::Ssh);
        assert_eq!(dev.port, 22);
        assert!(dev.console_server.is_none());
        assert!(dev.metadata.is_empty());
        assert_eq!(dev.credentials.username, "admin");
    }

    // -----------------------------------------------------------------------
    // CommandResult
    // -----------------------------------------------------------------------

    fn make_cmd_result(stdout: &str, stderr: &str, exit_code: i32) -> CommandResult {
        CommandResult {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit_code,
            duration: Duration::from_millis(100),
            command: "test_cmd".to_string(),
        }
    }

    #[test]
    fn command_result_success_true_when_zero() {
        let r = make_cmd_result("ok", "", 0);
        assert!(r.success());
    }

    #[test]
    fn command_result_success_false_when_nonzero() {
        let r = make_cmd_result("", "err", 1);
        assert!(!r.success());
    }

    #[test]
    fn command_result_success_false_for_negative_code() {
        let r = make_cmd_result("", "signal", -1);
        assert!(!r.success());
    }

    #[test]
    fn command_result_stdout_lines_splits_correctly() {
        let r = make_cmd_result("line1\nline2\nline3", "", 0);
        assert_eq!(r.stdout_lines(), vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn command_result_stdout_lines_empty_string() {
        let r = make_cmd_result("", "", 0);
        assert!(r.stdout_lines().is_empty());
    }

    #[test]
    fn command_result_stdout_lines_single_line() {
        let r = make_cmd_result("only", "", 0);
        assert_eq!(r.stdout_lines(), vec!["only"]);
    }

    // -----------------------------------------------------------------------
    // IpPair
    // -----------------------------------------------------------------------

    #[test]
    fn ip_pair_v4_only() {
        let addr: Ipv4Addr = "192.168.1.1".parse().unwrap();
        let prefix: ipnetwork::Ipv4Network = "192.168.1.0/24".parse().unwrap();
        let pair = IpPair::v4_only(addr, prefix);

        assert_eq!(pair.ipv4, Some(addr));
        assert_eq!(pair.ipv4_prefix, Some(prefix));
        assert!(pair.ipv6.is_none());
        assert!(pair.ipv6_prefix.is_none());
    }

    #[test]
    fn ip_pair_dual_stack() {
        let v4: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let v4_prefix: ipnetwork::Ipv4Network = "10.0.0.0/24".parse().unwrap();
        let v6: Ipv6Addr = "fd00::1".parse().unwrap();
        let v6_prefix: ipnetwork::Ipv6Network = "fd00::/64".parse().unwrap();
        let pair = IpPair::dual_stack(v4, v4_prefix, v6, v6_prefix);

        assert_eq!(pair.ipv4, Some(v4));
        assert_eq!(pair.ipv4_prefix, Some(v4_prefix));
        assert_eq!(pair.ipv6, Some(v6));
        assert_eq!(pair.ipv6_prefix, Some(v6_prefix));
    }
}
