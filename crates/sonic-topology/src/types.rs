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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // -- Vm -----------------------------------------------------------------

    #[test]
    fn vm_serialize_roundtrip() {
        let vm = Vm {
            name: "ARISTA01T1".into(),
            vm_offset: 0,
            vlans: vec![1000, 1001],
            port_mapping: HashMap::from([
                ("Ethernet0".into(), "et1".into()),
                ("Ethernet4".into(), "et2".into()),
            ]),
        };
        let json = serde_json::to_string(&vm).unwrap();
        let deserialized: Vm = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "ARISTA01T1");
        assert_eq!(deserialized.vm_offset, 0);
        assert_eq!(deserialized.vlans, vec![1000, 1001]);
        assert_eq!(deserialized.port_mapping.len(), 2);
    }

    // -- Vlan ---------------------------------------------------------------

    #[test]
    fn vlan_serialize_roundtrip() {
        let vlan = Vlan {
            id: 1000,
            name: "Vlan1000".into(),
            net_prefix: "192.168.0.0/21".parse().unwrap(),
            intfs: vec!["Ethernet0".into(), "Ethernet4".into()],
        };
        let json = serde_json::to_string(&vlan).unwrap();
        let deserialized: Vlan = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, 1000);
        assert_eq!(deserialized.name, "Vlan1000");
        assert_eq!(deserialized.intfs.len(), 2);
    }

    // -- VlanGroup ----------------------------------------------------------

    #[test]
    fn vlan_group_ptf_only_flag() {
        let group = VlanGroup {
            group_name: "servers".into(),
            vlans: Vec::new(),
            is_ptf_only: true,
        };
        assert!(group.is_ptf_only);

        let json = serde_json::to_string(&group).unwrap();
        let deserialized: VlanGroup = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_ptf_only);
    }

    // -- HostInterface ------------------------------------------------------

    #[test]
    fn host_interface_ptf_only_sentinel() {
        let hif = HostInterface {
            vm_index: u32::MAX,
            port_index: 0,
            dut_port: "Ethernet0".into(),
            ptf_port: "eth0".into(),
            vlan_id: 0,
        };
        assert_eq!(hif.vm_index, u32::MAX);
    }

    #[test]
    fn host_interface_serialize_roundtrip() {
        let hif = HostInterface {
            vm_index: 2,
            port_index: 1,
            dut_port: "Ethernet8".into(),
            ptf_port: "eth2".into(),
            vlan_id: 1000,
        };
        let json = serde_json::to_string(&hif).unwrap();
        let deserialized: HostInterface = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.vm_index, 2);
        assert_eq!(deserialized.dut_port, "Ethernet8");
        assert_eq!(deserialized.vlan_id, 1000);
    }

    // -- LagLink / LagPort -------------------------------------------------

    #[test]
    fn lag_link_serialize_roundtrip() {
        let lag = LagLink {
            lag_id: 1,
            vm_index: 0,
            dut_ports: vec!["Ethernet0".into(), "Ethernet4".into()],
            vm_ports: vec!["et1".into(), "et2".into()],
        };
        let json = serde_json::to_string(&lag).unwrap();
        let deserialized: LagLink = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.lag_id, 1);
        assert_eq!(deserialized.dut_ports.len(), 2);
        assert_eq!(deserialized.vm_ports.len(), 2);
    }

    #[test]
    fn lag_port_serialize_roundtrip() {
        let port = LagPort {
            lag_id: 3,
            member_ports: vec!["Ethernet12".into(), "Ethernet16".into()],
        };
        let json = serde_json::to_string(&port).unwrap();
        let deserialized: LagPort = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.lag_id, 3);
        assert_eq!(deserialized.member_ports.len(), 2);
    }

    // -- TopoMetadata -------------------------------------------------------

    #[test]
    fn topo_metadata_from_counts() {
        let meta = TopoMetadata::from_counts(TopologyType::T0, 4, 8, 32, 32, 32);
        assert_eq!(meta.topo_type, TopologyType::T0);
        assert_eq!(meta.num_vms, 4);
        assert_eq!(meta.num_vlans, 8);
        assert_eq!(meta.num_host_ifs, 32);
    }

    #[test]
    fn topo_metadata_serialize_roundtrip() {
        let meta = TopoMetadata::from_counts(TopologyType::T1Lag, 32, 16, 64, 64, 64);
        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: TopoMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.topo_type, TopologyType::T1Lag);
        assert_eq!(deserialized.num_vms, 32);
    }
}
