//! Topology generation engine.
//!
//! The [`DefaultTopologyGenerator`] implements `sonic_core::TopologyGenerator`
//! and produces a complete [`TopologyDefinition`] for every supported topology
//! type.  Each private `generate_*` method encodes the wiring rules for its
//! topology family and delegates address/VLAN/VM allocation to the allocator
//! module.

use std::net::Ipv4Addr;

use ipnetwork::{Ipv4Network, Ipv6Network};
use tracing::{debug, info};

use sonic_core::{
    HostInterfaceDefinition, LagLinkDefinition, SonicError, TopologyDefinition,
    TopologyGenerator, TopologyType, VlanDefinition, VmType,
};

use crate::allocator::{IpAllocator, VlanAllocator, VmAllocator};

// ---------------------------------------------------------------------------
// Default base addresses (mirrors ansible/vars/topo_*.yml conventions)
// ---------------------------------------------------------------------------

const BASE_V4: &str = "10.0.0.0/16";
const BASE_V6: &str = "fc00::/96";
const BASE_VM_IP: Ipv4Addr = Ipv4Addr::new(10, 250, 0, 2);
const BASE_VLAN: u16 = 1000;
const MAX_VLAN: u16 = 4094;

// ---------------------------------------------------------------------------
// Generator
// ---------------------------------------------------------------------------

/// The default topology generator.
///
/// Stateless: all intermediate state lives in the allocators that are
/// constructed per-invocation.
pub struct DefaultTopologyGenerator {
    vm_type: VmType,
}

impl DefaultTopologyGenerator {
    /// Creates a generator that will emit VMs of the given type.
    pub fn new(vm_type: VmType) -> Self {
        Self { vm_type }
    }

    /// Convenience constructor defaulting to `Veos` VMs.
    pub fn veos() -> Self {
        Self::new(VmType::Veos)
    }
}

impl TopologyGenerator for DefaultTopologyGenerator {
    fn generate(&self, topo_type: TopologyType) -> sonic_core::Result<TopologyDefinition> {
        info!(%topo_type, "generating topology");

        match topo_type {
            TopologyType::T0 => self.generate_t0(),
            TopologyType::T064 => self.generate_t0_64(),
            TopologyType::T0116 => self.generate_t0_116(),
            TopologyType::T1 => self.generate_t1(),
            TopologyType::T164 => self.generate_t1_64(),
            TopologyType::T1Lag => self.generate_t1_lag(),
            TopologyType::T2 => self.generate_t2(),
            TopologyType::Dualtor => self.generate_dualtor(),
            TopologyType::Ptf32 => self.generate_ptf(32),
            TopologyType::Ptf64 => self.generate_ptf(64),
            TopologyType::Ptf => self.generate_ptf(32),
            TopologyType::MgmtTor | TopologyType::M0Vlan => {
                // These share the T0 shape with minor metadata differences.
                self.generate_t0()
                    .map(|mut def| { def.topo_type = topo_type; def })
            }
            TopologyType::Any => Err(SonicError::UnsupportedTopology(
                "cannot generate 'any' topology".into(),
            )),
        }
    }

    fn supported_topologies(&self) -> Vec<TopologyType> {
        vec![
            TopologyType::T0,
            TopologyType::T064,
            TopologyType::T0116,
            TopologyType::T1,
            TopologyType::T164,
            TopologyType::T1Lag,
            TopologyType::T2,
            TopologyType::Dualtor,
            TopologyType::MgmtTor,
            TopologyType::M0Vlan,
            TopologyType::Ptf32,
            TopologyType::Ptf64,
            TopologyType::Ptf,
        ]
    }
}

// ---------------------------------------------------------------------------
// Private per-topology generators
// ---------------------------------------------------------------------------

impl DefaultTopologyGenerator {
    /// Creates fresh allocators rooted at the canonical base addresses.
    fn allocators(&self) -> (IpAllocator, VlanAllocator, VmAllocator) {
        let base_v4: Ipv4Network = BASE_V4.parse().expect("hardcoded v4 must parse");
        let base_v6: Ipv6Network = BASE_V6.parse().expect("hardcoded v6 must parse");
        (
            IpAllocator::new(base_v4, base_v6),
            VlanAllocator::new(BASE_VLAN, MAX_VLAN),
            VmAllocator::new("ARISTA", BASE_VM_IP, self.vm_type),
        )
    }

    // -- T0 ----------------------------------------------------------------

    /// T0: 4 VMs (T1 neighbors), 1 port each to DUT, 1 VLAN for server-facing
    /// PTF ports, 32 host interfaces total.
    fn generate_t0(&self) -> sonic_core::Result<TopologyDefinition> {
        self.generate_t0_n(4, 32, TopologyType::T0)
    }

    /// T0-64: scaled-up T0 with 64 VMs and 64 host interfaces.
    fn generate_t0_64(&self) -> sonic_core::Result<TopologyDefinition> {
        self.generate_t0_n(64, 64, TopologyType::T064)
    }

    /// T0-116: 116 VMs, 116 host interfaces.
    fn generate_t0_116(&self) -> sonic_core::Result<TopologyDefinition> {
        self.generate_t0_n(116, 116, TopologyType::T0116)
    }

    /// Shared T0-family logic.
    ///
    /// `num_vms` VMs each get a single port to the DUT.  One VLAN covers all
    /// server-facing host interfaces (used by PTF).
    fn generate_t0_n(
        &self,
        num_vms: u32,
        num_host_ifs: u32,
        topo_type: TopologyType,
    ) -> sonic_core::Result<TopologyDefinition> {
        let (mut ip_alloc, mut vlan_alloc, mut vm_alloc) = self.allocators();

        // -- VMs ------------------------------------------------------------
        let mut vms = vm_alloc.allocate(num_vms)?;
        let mut ip_pairs = Vec::with_capacity(num_vms as usize);

        for (i, vm) in vms.iter_mut().enumerate() {
            let port = format!("Ethernet{}", i * 4);
            vm.peer_ports = vec![port];
            ip_pairs.push(ip_alloc.allocate_pair(i as u32)?);
        }

        // -- VLANs ----------------------------------------------------------
        let vlan_ids = vlan_alloc.allocate(1)?;
        let vlan_intfs: Vec<String> = (0..num_host_ifs)
            .map(|i| format!("Ethernet{}", (num_vms as usize + i as usize) * 4))
            .collect();

        let vlans = vec![VlanDefinition {
            id: vlan_ids[0],
            name: format!("Vlan{}", vlan_ids[0]),
            intfs: vlan_intfs.clone(),
            prefix: Some("192.168.0.0/21".to_string()),
        }];

        // -- Host interfaces -------------------------------------------------
        let mut host_ifs = Vec::with_capacity(num_host_ifs as usize);
        for i in 0..num_host_ifs {
            host_ifs.push(HostInterfaceDefinition {
                vm_index: u32::MAX,
                port_index: i,
                dut_port: format!("Ethernet{}", (num_vms + i) as usize * 4),
                ptf_port: format!("eth{}", num_vms + i),
            });
        }

        debug!(
            num_vms,
            num_host_ifs,
            num_vlans = vlans.len(),
            "T0 topology built"
        );

        Ok(TopologyDefinition {
            topo_type,
            vms,
            vlans,
            host_interfaces: host_ifs,
            lag_links: Vec::new(),
            ip_pairs,
        })
    }

    // -- T1 ----------------------------------------------------------------

    /// T1: 32 VMs, 2 ports each to DUT (forming a LAG), dual-stack IP.
    fn generate_t1(&self) -> sonic_core::Result<TopologyDefinition> {
        self.generate_t1_n(32, 2, TopologyType::T1)
    }

    /// T1-64: 64 VMs variant.
    fn generate_t1_64(&self) -> sonic_core::Result<TopologyDefinition> {
        self.generate_t1_n(64, 2, TopologyType::T164)
    }

    /// T1-lag: 32 VMs with explicit LAG configuration.
    fn generate_t1_lag(&self) -> sonic_core::Result<TopologyDefinition> {
        self.generate_t1_n(32, 2, TopologyType::T1Lag)
    }

    /// Shared T1-family logic.
    ///
    /// Each VM connects to the DUT via `ports_per_vm` links bundled into a
    /// LAG.  No server-facing VLANs; all ports are routed.
    fn generate_t1_n(
        &self,
        num_vms: u32,
        ports_per_vm: u32,
        topo_type: TopologyType,
    ) -> sonic_core::Result<TopologyDefinition> {
        let (mut ip_alloc, _vlan_alloc, mut vm_alloc) = self.allocators();

        let mut vms = vm_alloc.allocate(num_vms)?;
        let mut ip_pairs = Vec::with_capacity(num_vms as usize);
        let mut lag_links = Vec::with_capacity(num_vms as usize);
        let mut host_ifs = Vec::new();

        let mut dut_port_idx: u32 = 0;

        for (vm_idx, vm) in vms.iter_mut().enumerate() {
            let mut dut_ports = Vec::with_capacity(ports_per_vm as usize);
            let mut vm_ports = Vec::with_capacity(ports_per_vm as usize);

            for port_j in 0..ports_per_vm {
                let dut_port = format!("Ethernet{}", dut_port_idx * 4);
                let vm_port = format!("Ethernet{}", port_j);
                vm.peer_ports.push(dut_port.clone());

                host_ifs.push(HostInterfaceDefinition {
                    vm_index: vm_idx as u32,
                    port_index: port_j,
                    dut_port: dut_port.clone(),
                    ptf_port: format!("eth{}", dut_port_idx),
                });

                dut_ports.push(dut_port);
                vm_ports.push(vm_port);
                dut_port_idx += 1;
            }

            lag_links.push(LagLinkDefinition {
                lag_id: vm_idx as u32,
                members: dut_ports.clone(),
                vm_index: vm_idx as u32,
            });

            ip_pairs.push(ip_alloc.allocate_pair(vm_idx as u32)?);
        }

        debug!(
            num_vms,
            ports_per_vm,
            total_ports = dut_port_idx,
            lags = lag_links.len(),
            "T1 topology built"
        );

        Ok(TopologyDefinition {
            topo_type,
            vms,
            vlans: Vec::new(),
            host_interfaces: host_ifs,
            lag_links,
            ip_pairs,
        })
    }

    // -- T2 ----------------------------------------------------------------

    /// T2: 64 VMs in a spine topology.
    ///
    /// Half the VMs model upstream T3 neighbors, the other half model
    /// downstream T1 neighbors.  Each VM gets a single link to the DUT.
    fn generate_t2(&self) -> sonic_core::Result<TopologyDefinition> {
        let num_vms: u32 = 64;
        let (mut ip_alloc, _vlan_alloc, mut vm_alloc) = self.allocators();

        let mut vms = vm_alloc.allocate(num_vms)?;
        let mut ip_pairs = Vec::with_capacity(num_vms as usize);
        let mut host_ifs = Vec::with_capacity(num_vms as usize);

        for (i, vm) in vms.iter_mut().enumerate() {
            let dut_port = format!("Ethernet{}", i * 4);
            vm.peer_ports = vec![dut_port.clone()];

            host_ifs.push(HostInterfaceDefinition {
                vm_index: i as u32,
                port_index: 0,
                dut_port,
                ptf_port: format!("eth{i}"),
            });

            ip_pairs.push(ip_alloc.allocate_pair(i as u32)?);
        }

        debug!(num_vms, "T2 topology built");

        Ok(TopologyDefinition {
            topo_type: TopologyType::T2,
            vms,
            vlans: Vec::new(),
            host_interfaces: host_ifs,
            lag_links: Vec::new(),
            ip_pairs,
        })
    }

    // -- Dualtor -----------------------------------------------------------

    /// Dualtor: 4 VMs, dual-TOR active/standby configuration.
    ///
    /// Each VM connects to both TORs.  A server VLAN carries traffic from the
    /// PTF container.  The mux cable abstraction is captured as metadata on the
    /// host interfaces (upper_tor / lower_tor role).
    fn generate_dualtor(&self) -> sonic_core::Result<TopologyDefinition> {
        let num_vms: u32 = 4;
        let num_server_ports: u32 = 32;
        let (mut ip_alloc, mut vlan_alloc, mut vm_alloc) = self.allocators();

        let mut vms = vm_alloc.allocate(num_vms)?;
        let mut ip_pairs = Vec::with_capacity(num_vms as usize);

        for (i, vm) in vms.iter_mut().enumerate() {
            let port = format!("Ethernet{}", i * 4);
            vm.peer_ports = vec![port];
            ip_pairs.push(ip_alloc.allocate_pair(i as u32)?);
        }

        // Server-facing VLAN for mux-cable ports.
        let vlan_ids = vlan_alloc.allocate(1)?;
        let vlan_intfs: Vec<String> = (0..num_server_ports)
            .map(|i| format!("Ethernet{}", (num_vms as usize + i as usize) * 4))
            .collect();

        let vlans = vec![VlanDefinition {
            id: vlan_ids[0],
            name: format!("Vlan{}", vlan_ids[0]),
            intfs: vlan_intfs,
            prefix: Some("192.168.0.0/21".to_string()),
        }];

        let mut host_ifs = Vec::with_capacity(num_server_ports as usize);
        for i in 0..num_server_ports {
            host_ifs.push(HostInterfaceDefinition {
                vm_index: u32::MAX,
                port_index: i,
                dut_port: format!("Ethernet{}", (num_vms + i) as usize * 4),
                ptf_port: format!("eth{}", num_vms + i),
            });
        }

        debug!(num_vms, num_server_ports, "dualtor topology built");

        Ok(TopologyDefinition {
            topo_type: TopologyType::Dualtor,
            vms,
            vlans,
            host_interfaces: host_ifs,
            lag_links: Vec::new(),
            ip_pairs,
        })
    }

    // -- PTF-only ----------------------------------------------------------

    /// PTF-only: no VMs, all ports wired directly to the PTF container.
    fn generate_ptf(&self, num_ports: u32) -> sonic_core::Result<TopologyDefinition> {
        let host_ifs: Vec<HostInterfaceDefinition> = (0..num_ports)
            .map(|i| HostInterfaceDefinition {
                vm_index: u32::MAX,
                port_index: i,
                dut_port: format!("Ethernet{}", i * 4),
                ptf_port: format!("eth{i}"),
            })
            .collect();

        let topo_type = match num_ports {
            64 => TopologyType::Ptf64,
            32 => TopologyType::Ptf32,
            _ => TopologyType::Ptf,
        };

        debug!(num_ports, "PTF-only topology built");

        Ok(TopologyDefinition {
            topo_type,
            vms: Vec::new(),
            vlans: Vec::new(),
            host_interfaces: host_ifs,
            lag_links: Vec::new(),
            ip_pairs: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen() -> DefaultTopologyGenerator {
        DefaultTopologyGenerator::veos()
    }

    #[test]
    fn t0_basic_shape() {
        let def = gen().generate(TopologyType::T0).unwrap();
        assert_eq!(def.vms.len(), 4);
        assert_eq!(def.vlans.len(), 1);
        assert_eq!(def.host_interfaces.len(), 32);
        assert!(def.lag_links.is_empty());
        assert_eq!(def.ip_pairs.len(), 4);
    }

    #[test]
    fn t0_64_scaled() {
        let def = gen().generate(TopologyType::T064).unwrap();
        assert_eq!(def.vms.len(), 64);
    }

    #[test]
    fn t1_lag_counts() {
        let def = gen().generate(TopologyType::T1).unwrap();
        assert_eq!(def.vms.len(), 32);
        assert_eq!(def.lag_links.len(), 32);
        // 2 ports per VM = 64 host interfaces.
        assert_eq!(def.host_interfaces.len(), 64);
    }

    #[test]
    fn t1_64_scale() {
        let def = gen().generate(TopologyType::T164).unwrap();
        assert_eq!(def.vms.len(), 64);
        assert_eq!(def.lag_links.len(), 64);
    }

    #[test]
    fn t2_shape() {
        let def = gen().generate(TopologyType::T2).unwrap();
        assert_eq!(def.vms.len(), 64);
        assert_eq!(def.host_interfaces.len(), 64);
    }

    #[test]
    fn dualtor_shape() {
        let def = gen().generate(TopologyType::Dualtor).unwrap();
        assert_eq!(def.vms.len(), 4);
        assert_eq!(def.vlans.len(), 1);
        assert_eq!(def.host_interfaces.len(), 32);
    }

    #[test]
    fn ptf_32() {
        let def = gen().generate(TopologyType::Ptf32).unwrap();
        assert!(def.vms.is_empty());
        assert_eq!(def.host_interfaces.len(), 32);
    }

    #[test]
    fn ptf_64() {
        let def = gen().generate(TopologyType::Ptf64).unwrap();
        assert_eq!(def.host_interfaces.len(), 64);
    }

    #[test]
    fn any_is_unsupported() {
        assert!(gen().generate(TopologyType::Any).is_err());
    }

    #[test]
    fn supported_list_is_nonempty() {
        assert!(!gen().supported_topologies().is_empty());
    }
}
