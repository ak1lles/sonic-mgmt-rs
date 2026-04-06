//! Shared domain model for the sonic-mgmt workspace.
//!
//! Every crate in the workspace depends on these types. They represent devices,
//! connections, topologies, test results, network primitives, and testbed
//! lifecycle states. All types derive [`Serialize`] and [`Deserialize`] for
//! configuration file round-tripping.

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

/// Classification of network devices managed by the test framework.
///
/// Each variant maps to a host implementation in `sonic-device` that knows
/// how to connect, configure, and query that device type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    /// SONiC network operating system switch (device under test).
    Sonic,
    /// Arista EOS switch, typically used as a BGP neighbor VM.
    Eos,
    /// Cisco IOS/NX-OS switch, used as a BGP neighbor VM.
    Cisco,
    /// Fanout switch that breaks out physical ports to the DUT.
    Fanout,
    /// Packet Test Framework container running scapy-based tests.
    Ptf,
    /// Kubernetes master node for container-based test orchestration.
    K8sMaster,
    /// Aruba AOS switch.
    Aos,
    /// Cumulus Linux switch.
    Cumulus,
    /// Open Network Install Environment, used during image provisioning.
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

/// Transport protocol used to reach a device.
///
/// The connection layer in `sonic-device` dispatches on this enum to create
/// the appropriate session type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionType {
    /// Secure Shell (port 22 by default).
    Ssh,
    /// Telnet, used for legacy console servers.
    Telnet,
    /// Serial console via a conserver aggregation server.
    Console,
    /// gRPC, used for gNMI/gNOI/P4Runtime management.
    Grpc,
    /// Local shell execution, used when running on the device itself.
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

/// ASIC vendor platform running on a switch.
///
/// Determines which hardware-specific behaviors apply (e.g., counter polling
/// intervals, supported features, warm reboot capability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Broadcom switch ASICs.
    Broadcom,
    /// Mellanox/NVIDIA Spectrum switch ASICs.
    Mellanox,
    /// Intel Barefoot Tofino programmable ASICs.
    Barefoot,
    /// Marvell Prestera/Aldrin switch ASICs.
    Marvell,
    /// Nokia custom silicon.
    Nokia,
    /// Cisco Silicon One ASICs.
    Cisco,
    /// Centec switch ASICs.
    Centec,
    /// Software dataplane for virtual/KVM-based switches.
    Virtual,
    /// Platform has not been identified yet.
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

/// Testbed topology layout.
///
/// Each variant defines a specific arrangement of VMs, ports, and links
/// that models a SONiC deployment tier. Use [`TopologyType::vm_count`] to
/// query how many neighbor VMs a topology requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyType {
    /// Leaf (ToR) topology with 4 upstream VMs.
    T0,
    /// Leaf topology scaled to 64 ports.
    T064,
    /// Leaf topology scaled to 116 ports.
    T0116,
    /// Spine topology with 32 downstream VMs.
    T1,
    /// Spine topology scaled to 64 ports.
    T164,
    /// Spine topology with LAG bundles to downstream VMs.
    T1Lag,
    /// Super-spine topology with 64 VMs.
    T2,
    /// Dual ToR (active-standby mux cable) topology with 4 VMs.
    Dualtor,
    /// Management ToR topology with 4 VMs.
    MgmtTor,
    /// M0 VLAN topology with 4 VMs.
    M0Vlan,
    /// PTF-only topology with 32 ports, no VMs.
    Ptf32,
    /// PTF-only topology with 64 ports, no VMs.
    Ptf64,
    /// PTF-only topology with default port count, no VMs.
    Ptf,
    /// Wildcard that matches any topology.
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
    /// Returns the number of neighbor VMs required by this topology.
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

    /// Returns `true` if this topology needs at least one neighbor VM.
    pub fn requires_vms(&self) -> bool {
        self.vm_count() > 0
    }

    /// Returns `true` if this topology uses only a PTF container with no VMs.
    pub fn is_ptf_only(&self) -> bool {
        matches!(self, Self::Ptf | Self::Ptf32 | Self::Ptf64)
    }
}

/// Virtual machine image flavor used for neighbor simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmType {
    /// Arista vEOS running as a KVM virtual machine.
    Veos,
    /// Arista cEOS running as a Docker container.
    Ceos,
    /// SONiC virtual switch running as a KVM VM.
    Vsonic,
    /// Cisco virtual router running as a KVM VM.
    Vcisco,
    /// SONiC virtual switch running as a Docker container.
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

/// Method used to reboot a device under test.
///
/// Different reboot types exercise different SONiC subsystems. Tests select
/// a reboot type to verify that services recover correctly under each method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebootType {
    /// Full power-off and restart. All state is lost.
    Cold,
    /// Warm reboot that preserves the data plane during restart.
    Warm,
    /// Fast reboot that minimizes control plane downtime.
    Fast,
    /// Hard power cycle via PDU or BMC.
    PowerCycle,
    /// Hardware watchdog timer expiry triggers the reboot.
    Watchdog,
    /// Supervisor module restart on modular chassis.
    Supervisor,
    /// Kernel crash dump followed by reboot.
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

/// Method used to reload device configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigReloadType {
    /// Reload config_db.json from disk.
    Reload,
    /// Load and apply a minigraph XML topology file.
    LoadMinigraph,
    /// Apply the golden configuration template.
    GoldenConfig,
    /// Erase all configuration and restore factory defaults.
    FactoryReset,
}

// ---------------------------------------------------------------------------
// Test result types
// ---------------------------------------------------------------------------

/// Final result of a single test case execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    /// Test assertions succeeded.
    Passed,
    /// One or more assertions failed.
    Failed,
    /// Test was skipped (e.g., unsupported platform or topology).
    Skipped,
    /// Test could not run due to an infrastructure error.
    Error,
    /// Test failed as expected (known issue marked with `xfail`).
    XFail,
    /// Test marked `xfail` passed unexpectedly.
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

/// Authentication credentials for connecting to a device.
///
/// Construct with [`Credentials::new`] and chain builder methods to add
/// a password or SSH key path.
///
/// The `password` field is excluded from serialization to avoid leaking
/// secrets into config files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// Login username.
    pub username: String,
    /// Password for password-based authentication. Skipped during serialization.
    #[serde(skip_serializing)]
    pub password: Option<String>,
    /// Filesystem path to an SSH private key.
    pub key_path: Option<String>,
    /// Passphrase that unlocks the SSH private key, if encrypted.
    pub passphrase: Option<String>,
}

impl Credentials {
    /// Creates credentials with the given username and no password or key.
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: None,
            key_path: None,
            passphrase: None,
        }
    }

    /// Sets the password for password-based authentication.
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Sets the filesystem path to an SSH private key.
    pub fn with_key(mut self, key_path: impl Into<String>) -> Self {
        self.key_path = Some(key_path.into());
        self
    }
}

/// Complete identity and connection details for a managed device.
///
/// Construct with [`DeviceInfo::new`], which assigns a random [`Uuid`],
/// defaults to SSH on port 22, and sets the platform to [`Platform::Unknown`].
/// Populate remaining fields directly after construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Unique identifier for this device instance.
    pub id: Uuid,
    /// DNS hostname or inventory name.
    pub hostname: String,
    /// Management IP address used for out-of-band access.
    pub mgmt_ip: IpAddr,
    /// Classification that selects the host driver.
    pub device_type: DeviceType,
    /// ASIC vendor platform.
    pub platform: Platform,
    /// Hardware SKU string (e.g., `"Arista-7060CX-32S-C32"`).
    pub hwsku: String,
    /// SONiC or NOS version string, if known.
    pub os_version: Option<String>,
    /// Device serial number, if known.
    pub serial: Option<String>,
    /// Device model name, if known.
    pub model: Option<String>,
    /// Authentication credentials.
    pub credentials: Credentials,
    /// Transport protocol for the primary management session.
    pub connection_type: ConnectionType,
    /// TCP port for the primary management session.
    pub port: u16,
    /// Console server details for out-of-band serial access.
    pub console_server: Option<ConsoleInfo>,
    /// Arbitrary key-value pairs for testbed-specific data.
    pub metadata: HashMap<String, String>,
}

impl DeviceInfo {
    /// Creates a new device with SSH on port 22 and [`Platform::Unknown`].
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

/// Connection details for a console server port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleInfo {
    /// Hostname or IP of the console server (e.g., conserver).
    pub server: String,
    /// TCP port on the console server mapped to this device.
    pub port: u16,
    /// Protocol used to reach the console server.
    pub protocol: ConnectionType,
}

/// Output captured from a remote command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    /// Standard output of the command.
    pub stdout: String,
    /// Standard error of the command.
    pub stderr: String,
    /// Process exit code. Zero indicates success.
    pub exit_code: i32,
    /// Wall-clock time the command took to complete.
    pub duration: std::time::Duration,
    /// The command string that was executed.
    pub command: String,
}

impl CommandResult {
    /// Returns `true` if the command exited with code 0.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Splits stdout into lines.
    pub fn stdout_lines(&self) -> Vec<&str> {
        self.stdout.lines().collect()
    }
}

// ---------------------------------------------------------------------------
// Network primitives
// ---------------------------------------------------------------------------

/// Dual-stack IPv4/IPv6 address pair with associated prefixes.
///
/// Either address family may be absent. Use [`IpPair::v4_only`] or
/// [`IpPair::dual_stack`] to construct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpPair {
    /// IPv4 host address.
    pub ipv4: Option<Ipv4Addr>,
    /// IPv6 host address.
    pub ipv6: Option<Ipv6Addr>,
    /// IPv4 subnet containing the host address.
    pub ipv4_prefix: Option<Ipv4Network>,
    /// IPv6 subnet containing the host address.
    pub ipv6_prefix: Option<Ipv6Network>,
}

impl IpPair {
    /// Creates a pair with only an IPv4 address and prefix.
    pub fn v4_only(addr: Ipv4Addr, prefix: Ipv4Network) -> Self {
        Self {
            ipv4: Some(addr),
            ipv6: None,
            ipv4_prefix: Some(prefix),
            ipv6_prefix: None,
        }
    }

    /// Creates a pair with both IPv4 and IPv6 addresses and prefixes.
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

/// Physical or logical port on a switch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortInfo {
    /// SONiC interface name (e.g., `"Ethernet0"`).
    pub name: String,
    /// Front-panel alias (e.g., `"fortyGigE0/0"`).
    pub alias: Option<String>,
    /// Zero-based port index.
    pub index: u32,
    /// Link speed in bits per second.
    pub speed: u64,
    /// ASIC lane assignments.
    pub lanes: Vec<u32>,
    /// Maximum transmission unit in bytes.
    pub mtu: u16,
    /// Administratively configured status.
    pub admin_status: PortStatus,
    /// Operational (link-level) status.
    pub oper_status: PortStatus,
    /// Forward error correction mode, if configured.
    pub fec: Option<String>,
    /// Auto-negotiation enabled, if configured.
    pub autoneg: Option<bool>,
}

/// Administrative or operational status of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortStatus {
    /// Port is up.
    Up,
    /// Port is administratively or operationally down.
    Down,
    /// Port hardware is not present (empty slot or unsupported transceiver).
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

/// VLAN configuration on a switch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanInfo {
    /// IEEE 802.1Q VLAN ID (1-4094).
    pub id: u16,
    /// VLAN name (e.g., `"Vlan1000"`).
    pub name: String,
    /// Ports assigned to this VLAN.
    pub members: Vec<VlanMember>,
    /// IP addresses assigned to the VLAN SVI.
    pub ip_addresses: Vec<IpPair>,
    /// DHCP relay server addresses.
    pub dhcp_servers: Vec<IpAddr>,
}

/// A port's membership in a VLAN.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanMember {
    /// Interface name (e.g., `"Ethernet0"`).
    pub port: String,
    /// Whether the port carries the VLAN tag on the wire.
    pub tagging_mode: TaggingMode,
}

/// IEEE 802.1Q tagging mode for a VLAN member port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaggingMode {
    /// Frames carry the VLAN tag (trunk port).
    Tagged,
    /// Frames are sent without a VLAN tag (access port).
    Untagged,
}

/// Link aggregation group (port channel) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LagInfo {
    /// LAG interface name (e.g., `"PortChannel0001"`).
    pub name: String,
    /// Member port names.
    pub members: Vec<String>,
    /// Minimum number of active links for the LAG to be operationally up.
    pub min_links: u32,
    /// LACP negotiation mode.
    pub lacp_mode: LacpMode,
    /// Administrative status of the LAG interface.
    pub admin_status: PortStatus,
}

/// LACP negotiation mode for a link aggregation group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LacpMode {
    /// Actively sends LACP PDUs to negotiate aggregation.
    Active,
    /// Responds to LACP PDUs but does not initiate.
    Passive,
    /// Static aggregation with no LACP negotiation.
    On,
}

// ---------------------------------------------------------------------------
// BGP types
// ---------------------------------------------------------------------------

/// BGP neighbor session state and counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BgpNeighbor {
    /// Neighbor's IP address (IPv4 or IPv6).
    pub address: IpAddr,
    /// Neighbor's autonomous system number.
    pub remote_as: u32,
    /// Local autonomous system number.
    pub local_as: u32,
    /// Current BGP FSM state.
    pub state: BgpState,
    /// Configured neighbor description string.
    pub description: Option<String>,
    /// Negotiated hold time in seconds.
    pub hold_time: u32,
    /// Negotiated keepalive interval in seconds.
    pub keepalive: u32,
    /// Number of prefixes received from this neighbor.
    pub prefixes_received: u64,
    /// Number of prefixes advertised to this neighbor.
    pub prefixes_sent: u64,
    /// Timestamp when the session entered Established state.
    pub up_since: Option<DateTime<Utc>>,
}

/// BGP finite state machine states (RFC 4271 Section 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BgpState {
    /// No resources allocated; waiting for a start event.
    Idle,
    /// TCP connection in progress.
    Connect,
    /// Listening for an incoming TCP connection.
    Active,
    /// OPEN message sent, waiting for peer OPEN.
    OpenSent,
    /// OPEN received, waiting for KEEPALIVE or NOTIFICATION.
    OpenConfirm,
    /// Session is up and exchanging UPDATE messages.
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

/// A single BGP route entry from the RIB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BgpRoute {
    /// IP prefix in CIDR notation (e.g., `"10.0.0.0/24"`).
    pub prefix: String,
    /// Next-hop address for this route.
    pub next_hop: IpAddr,
    /// Multi-exit discriminator (MED).
    pub metric: u32,
    /// BGP local preference.
    pub local_pref: u32,
    /// Sequence of AS numbers the route has traversed.
    pub as_path: Vec<u32>,
    /// Origin attribute.
    pub origin: BgpOrigin,
    /// BGP community strings attached to the route.
    pub communities: Vec<String>,
    /// Whether the route passes validity checks.
    pub valid: bool,
    /// Whether this route is the best path for the prefix.
    pub best: bool,
}

/// BGP ORIGIN path attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BgpOrigin {
    /// Route originated from an IGP (most preferred).
    Igp,
    /// Route originated from an EGP.
    Egp,
    /// Route origin is unknown (least preferred).
    Incomplete,
}

// ---------------------------------------------------------------------------
// ACL types
// ---------------------------------------------------------------------------

/// Access control list table bound to a set of ports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclTable {
    /// Table name (e.g., `"DATAACL"`).
    pub name: String,
    /// ACL classification type.
    pub table_type: AclTableType,
    /// Pipeline stage where the ACL is applied.
    pub stage: AclStage,
    /// Ports this table is bound to.
    pub ports: Vec<String>,
    /// Ordered list of match-action rules.
    pub rules: Vec<AclRule>,
}

/// Classification type of an ACL table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AclTableType {
    /// IPv4 L3 ACL.
    L3,
    /// IPv6 L3 ACL.
    L3V6,
    /// Port mirroring ACL.
    Mirror,
    /// DSCP-based mirroring ACL.
    MirrorDscp,
    /// Priority flow control watchdog ACL.
    Pfcwd,
    /// Control plane protection ACL.
    Ctrlplane,
}

/// Pipeline stage where an ACL table is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclStage {
    /// Applied to incoming packets.
    Ingress,
    /// Applied to outgoing packets.
    Egress,
}

/// A single match-action rule within an ACL table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclRule {
    /// Rule name.
    pub name: String,
    /// Priority (higher value = matched first).
    pub priority: u32,
    /// Action to take on a match.
    pub action: AclAction,
    /// Source IP or prefix to match.
    pub src_ip: Option<String>,
    /// Destination IP or prefix to match.
    pub dst_ip: Option<String>,
    /// Source L4 port or range to match.
    pub src_port: Option<String>,
    /// Destination L4 port or range to match.
    pub dst_port: Option<String>,
    /// IP protocol number or name.
    pub protocol: Option<String>,
    /// Ethernet type to match (e.g., `"0x0800"` for IPv4).
    pub ether_type: Option<String>,
}

/// Action taken when an ACL rule matches a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AclAction {
    /// Allow the packet through.
    Forward,
    /// Silently discard the packet.
    Drop,
    /// Redirect the packet to a different port or next-hop.
    Redirect,
    /// Mirror a copy of the ingress packet to an analyzer port.
    MirrorIngress,
    /// Mirror a copy of the egress packet to an analyzer port.
    MirrorEgress,
}

// ---------------------------------------------------------------------------
// Facts aggregates
// ---------------------------------------------------------------------------

/// System-level facts collected from a device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BasicFacts {
    /// Device hostname.
    pub hostname: String,
    /// Hardware SKU identifier.
    pub hwsku: String,
    /// Platform string reported by the device.
    pub platform: String,
    /// SONiC or NOS version string.
    pub os_version: String,
    /// Chassis serial number.
    pub serial_number: String,
    /// Device model name.
    pub model: String,
    /// System base MAC address.
    pub mac_address: String,
    /// System uptime in seconds.
    pub uptime: u64,
    /// ASIC type string (e.g., `"Memory"`).
    pub asic_type: String,
    /// Linux kernel version.
    pub kernel_version: String,
}

/// BGP facts collected from a device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BgpFacts {
    /// BGP router ID.
    pub router_id: String,
    /// Local autonomous system number.
    pub local_as: u32,
    /// Configured BGP neighbor sessions.
    pub neighbors: Vec<BgpNeighbor>,
    /// IPv4 unicast routes in the BGP RIB.
    pub routes_ipv4: Vec<BgpRoute>,
    /// IPv6 unicast routes in the BGP RIB.
    pub routes_ipv6: Vec<BgpRoute>,
}

/// Interface facts collected from a device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InterfaceFacts {
    /// Physical and breakout ports.
    pub ports: Vec<PortInfo>,
    /// VLAN interfaces and their members.
    pub vlans: Vec<VlanInfo>,
    /// Link aggregation groups.
    pub lags: Vec<LagInfo>,
    /// Loopback interfaces.
    pub loopbacks: Vec<LoopbackInfo>,
}

/// Loopback interface and its assigned addresses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopbackInfo {
    /// Interface name (e.g., `"Loopback0"`).
    pub name: String,
    /// IP addresses assigned to the loopback.
    pub ip_addresses: Vec<IpPair>,
}

/// Configuration facts collected from a device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigFacts {
    /// Running configuration tables keyed by SONiC config_db table name.
    pub running_config: HashMap<String, serde_json::Value>,
    /// Startup configuration tables.
    pub startup_config: HashMap<String, serde_json::Value>,
    /// SONiC feature states keyed by feature name.
    pub features: HashMap<String, FeatureState>,
    /// Systemd service statuses.
    pub services: Vec<ServiceInfo>,
}

/// State of a SONiC feature (container-managed service).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureState {
    /// Feature name (e.g., `"bgp"`, `"swss"`).
    pub name: String,
    /// Current state string (e.g., `"enabled"`, `"disabled"`).
    pub state: String,
    /// Whether the feature auto-restarts on failure.
    pub auto_restart: bool,
    /// Whether high memory alerts are enabled.
    pub high_mem_alert: bool,
}

/// Status of a systemd service on the device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Systemd unit name.
    pub name: String,
    /// Service status (e.g., `"running"`, `"exited"`).
    pub status: String,
    /// Main process ID, if running.
    pub pid: Option<u32>,
}

// ---------------------------------------------------------------------------
// Testbed types
// ---------------------------------------------------------------------------

/// Lifecycle state of a testbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestbedState {
    /// Testbed is idle and can accept new test runs.
    Available,
    /// A test run is currently executing on this testbed.
    InUse,
    /// Topology or VM deployment is in progress.
    Deploying,
    /// An unrecoverable error occurred; manual intervention needed.
    Error,
    /// Testbed is offline for planned maintenance.
    Maintenance,
    /// Testbed resources have been torn down.
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

/// Result of a health check against a device or testbed component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// All checks passed.
    Healthy,
    /// Some non-critical checks failed.
    Degraded,
    /// Critical checks failed; the component cannot serve traffic.
    Unhealthy,
    /// Health state could not be determined.
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

/// Output format for test result reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportFormat {
    /// JUnit XML format consumed by CI systems.
    JunitXml,
    /// JSON format.
    Json,
    /// TOML format.
    Toml,
    /// Comma-separated values.
    Csv,
    /// HTML report for human viewing.
    Html,
}

/// Authentication method for result upload destinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// Application key / API key.
    AppKey,
    /// Azure managed identity.
    ManagedIdentity,
    /// Azure default credential chain.
    AzureDefault,
    /// Azure CLI cached credential.
    AzureCli,
    /// OAuth device code flow.
    DeviceCode,
    /// User-scoped bearer token.
    UserToken,
    /// Application-scoped bearer token.
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
