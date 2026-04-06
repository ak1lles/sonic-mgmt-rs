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
