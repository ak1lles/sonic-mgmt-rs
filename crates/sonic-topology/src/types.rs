//! Topology-specific data types.
//!
//! These types complement the core `TopologyDefinition` with richer
//! topology-internal detail that generators and renderers need but that
//! external crates rarely interact with directly.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use sonic_core::TopologyType;

// ---------------------------------------------------------------------------
// VM representation
// ---------------------------------------------------------------------------

/// A virtual-machine slot inside a topology.
///
/// Each VM corresponds to a BGP neighbor that is connected to the DUT via one
/// or more point-to-point links.  `vlans` lists any VLAN IDs that the VM's
/// ports are associated with, and `port_mapping` records the physical wiring
/// between DUT front-panel ports and VM-side ports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vm {
    /// Symbolic name (e.g. `ARISTA01T1`).
    pub name: String,
    /// Offset relative to the base VM index (used for IP/name generation).
    pub vm_offset: u32,
    /// VLAN IDs this VM participates in.
    pub vlans: Vec<u16>,
    /// Mapping of DUT port name -> VM port name.
    pub port_mapping: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// VLAN types
// ---------------------------------------------------------------------------

/// A single VLAN inside a topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vlan {
    /// 802.1Q VLAN ID.
    pub id: u16,
    /// Human-readable name (e.g. `Vlan1000`).
    pub name: String,
    /// IPv4 prefix assigned to this VLAN's SVI (e.g. `192.168.0.0/21`).
    pub net_prefix: ipnetwork::Ipv4Network,
    /// Interfaces that are members of this VLAN.
    pub intfs: Vec<String>,
}

/// A named group of VLANs (e.g. all server-facing VLANs vs. uplink VLANs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanGroup {
    /// Group identifier.
    pub group_name: String,
    /// VLANs belonging to this group.
    pub vlans: Vec<Vlan>,
    /// If `true`, these VLANs are wired exclusively to the PTF container
    /// (no VMs).
    pub is_ptf_only: bool,
}

// ---------------------------------------------------------------------------
// Host interface
// ---------------------------------------------------------------------------

/// Maps a single DUT port to the corresponding PTF port, optionally through a
/// VM.
///
/// In PTF-only topologies `vm_index` is set to `u32::MAX` (sentinel) to
/// indicate no VM involvement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInterface {
    /// Index into the topology's VM list (`u32::MAX` if PTF-only).
    pub vm_index: u32,
    /// Port ordinal within the VM.
    pub port_index: u32,
    /// DUT front-panel port name (e.g. `Ethernet0`).
    pub dut_port: String,
    /// PTF container interface name (e.g. `eth0`).
    pub ptf_port: String,
    /// VLAN ID the host interface is associated with (0 if none).
    pub vlan_id: u16,
}

// ---------------------------------------------------------------------------
// LAG types
// ---------------------------------------------------------------------------

/// A single member port inside a LAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LagPort {
    /// LAG identifier this port belongs to.
    pub lag_id: u32,
    /// Physical port names that are members of this LAG.
    pub member_ports: Vec<String>,
}

/// A LAG link connecting a DUT to a VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LagLink {
    /// LAG identifier.
    pub lag_id: u32,
    /// VM index this LAG connects to.
    pub vm_index: u32,
    /// DUT-side member port names.
    pub dut_ports: Vec<String>,
    /// VM-side member port names.
    pub vm_ports: Vec<String>,
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// Summary counters for a topology, useful for validation and display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopoMetadata {
    pub topo_type: TopologyType,
    pub num_vms: usize,
    pub num_vlans: usize,
    pub num_host_ifs: usize,
    pub dut_port_count: usize,
    pub ptf_port_count: usize,
}

impl TopoMetadata {
    /// Builds metadata by inspecting the provided collections.
    pub fn from_counts(
        topo_type: TopologyType,
        num_vms: usize,
        num_vlans: usize,
        num_host_ifs: usize,
        dut_port_count: usize,
        ptf_port_count: usize,
    ) -> Self {
        Self {
            topo_type,
            num_vms,
            num_vlans,
            num_host_ifs,
            dut_port_count,
            ptf_port_count,
        }
    }
}
