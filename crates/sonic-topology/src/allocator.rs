//! IP, VLAN, and VM resource allocators.
//!
//! Each allocator maintains internal state so that successive calls yield
//! non-overlapping resources.  They are designed to be constructed once per
//! topology generation pass and then discarded.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ipnetwork::{Ipv4Network, Ipv6Network};
use tracing::debug;

use sonic_core::{IpPair, IpPairAllocation, SonicError, VmDefinition, VmType};

// ---------------------------------------------------------------------------
// IP allocator
// ---------------------------------------------------------------------------

/// Allocates point-to-point and subnet IP pairs from a base v4/v6 network.
///
/// For point-to-point links the allocator hands out `/31` pairs (or `/127` for
/// v6), incrementing by 2 addresses per call.  `allocate_subnet` carves out a
/// block of the requested prefix length.
pub struct IpAllocator {
    base_v4: Ipv4Network,
    base_v6: Ipv6Network,
    v4_offset: u32,
    v6_offset: u128,
}

impl IpAllocator {
    pub fn new(base_v4: Ipv4Network, base_v6: Ipv6Network) -> Self {
        Self {
            base_v4,
            base_v6,
            v4_offset: 0,
            v6_offset: 0,
        }
    }

    /// Allocates a point-to-point `/31` (v4) + `/127` (v6) pair.
    ///
    /// Returns an `IpPairAllocation` with both the DUT-side and neighbor-side
    /// addresses populated.
    pub fn allocate_pair(&mut self, vm_index: u32) -> sonic_core::Result<IpPairAllocation> {
        let dut_v4 = self.next_v4()?;
        let nbr_v4 = self.next_v4()?;

        let dut_v6 = self.next_v6()?;
        let nbr_v6 = self.next_v6()?;

        let v4_prefix =
            Ipv4Network::new(dut_v4, 31).map_err(|e| SonicError::topology(e.to_string()))?;
        let v6_prefix =
            Ipv6Network::new(dut_v6, 127).map_err(|e| SonicError::topology(e.to_string()))?;

        let dut_ip = IpPair::dual_stack(dut_v4, v4_prefix, dut_v6, v6_prefix);
        let neighbor_ip = IpPair::dual_stack(nbr_v4, v4_prefix, nbr_v6, v6_prefix);

        debug!(
            vm_index,
            %dut_v4, %nbr_v4, %dut_v6, %nbr_v6,
            "allocated IP pair"
        );

        Ok(IpPairAllocation {
            vm_index,
            dut_ip,
            neighbor_ip,
        })
    }

    /// Allocates a subnet of the given IPv4 prefix length, returning the
    /// gateway (DUT-side) address and the first usable host (neighbor-side)
    /// address.
    pub fn allocate_subnet(
        &mut self,
        prefix_len: u8,
        vm_index: u32,
    ) -> sonic_core::Result<IpPairAllocation> {
        // Calculate how many addresses this prefix covers and align the offset.
        let block_size: u32 = 1u32
            .checked_shl(32 - prefix_len as u32)
            .ok_or_else(|| SonicError::topology("invalid prefix length"))?;

        // Align v4_offset to the next block boundary.
        let remainder = self.v4_offset % block_size;
        if remainder != 0 {
            self.v4_offset += block_size - remainder;
        }

        let base: u32 = self.base_v4.ip().into();
        let subnet_start = base
            .checked_add(self.v4_offset)
            .ok_or_else(|| SonicError::IpExhausted(self.base_v4.to_string()))?;
        let subnet_end = subnet_start
            .checked_add(block_size - 1)
            .ok_or_else(|| SonicError::IpExhausted(self.base_v4.to_string()))?;

        let network_end: u32 = self.base_v4.broadcast().into();
        if subnet_end > network_end {
            return Err(SonicError::IpExhausted(self.base_v4.to_string()));
        }

        let gateway = Ipv4Addr::from(subnet_start + 1);
        let first_host = Ipv4Addr::from(subnet_start + 2);
        let v4_prefix = Ipv4Network::new(Ipv4Addr::from(subnet_start), prefix_len)
            .map_err(|e| SonicError::topology(e.to_string()))?;

        self.v4_offset += block_size;

        let dut_ip = IpPair::v4_only(gateway, v4_prefix);
        let neighbor_ip = IpPair::v4_only(first_host, v4_prefix);

        debug!(
            vm_index,
            %gateway, %first_host, prefix_len,
            "allocated subnet"
        );

        Ok(IpPairAllocation {
            vm_index,
            dut_ip,
            neighbor_ip,
        })
    }

    /// Resets the allocator to the beginning of its address space.
    pub fn reset(&mut self) {
        self.v4_offset = 0;
        self.v6_offset = 0;
    }

    // -- internal helpers ---------------------------------------------------

    fn next_v4(&mut self) -> sonic_core::Result<Ipv4Addr> {
        let base: u32 = self.base_v4.ip().into();
        let addr = base
            .checked_add(self.v4_offset)
            .ok_or_else(|| SonicError::IpExhausted(self.base_v4.to_string()))?;

        let max: u32 = self.base_v4.broadcast().into();
        if addr > max {
            return Err(SonicError::IpExhausted(self.base_v4.to_string()));
        }

        self.v4_offset += 1;
        Ok(Ipv4Addr::from(addr))
    }

    fn next_v6(&mut self) -> sonic_core::Result<Ipv6Addr> {
        let base: u128 = self.base_v6.ip().into();
        let addr = base
            .checked_add(self.v6_offset)
            .ok_or_else(|| SonicError::IpExhausted(self.base_v6.to_string()))?;

        // Rough upper bound: base + size of the prefix.
        let size = 1u128 << (128 - self.base_v6.prefix() as u32);
        if self.v6_offset >= size {
            return Err(SonicError::IpExhausted(self.base_v6.to_string()));
        }

        self.v6_offset += 1;
        Ok(Ipv6Addr::from(addr))
    }
}

// ---------------------------------------------------------------------------
// VLAN allocator
// ---------------------------------------------------------------------------

/// Allocates VLAN IDs from a contiguous range.
pub struct VlanAllocator {
    next_vlan: u16,
    max_vlan: u16,
}

impl VlanAllocator {
    pub fn new(base_vlan: u16, max_vlan: u16) -> Self {
        Self {
            next_vlan: base_vlan,
            max_vlan,
        }
    }

    /// Allocates `count` consecutive VLAN IDs.
    ///
    /// Returns an error if the requested count would exceed the configured
    /// range.
    pub fn allocate(&mut self, count: u16) -> sonic_core::Result<Vec<u16>> {
        let end = self
            .next_vlan
            .checked_add(count)
            .ok_or_else(|| SonicError::topology("VLAN ID overflow"))?;

        if end - 1 > self.max_vlan {
            return Err(SonicError::VlanConflict {
                vlan_id: self.next_vlan,
                reason: format!(
                    "requested {} VLANs starting at {} but max is {}",
                    count, self.next_vlan, self.max_vlan
                ),
            });
        }

        let ids: Vec<u16> = (self.next_vlan..end).collect();
        self.next_vlan = end;

        debug!(start = ids[0], end = end - 1, count, "allocated VLANs");
        Ok(ids)
    }

    /// Returns the next VLAN ID that would be allocated (without consuming it).
    pub fn peek(&self) -> u16 {
        self.next_vlan
    }
}

// ---------------------------------------------------------------------------
// VM allocator
// ---------------------------------------------------------------------------

/// Allocates VM names and management IP addresses.
///
/// Names follow the pattern `{base_name}{offset:02}` (e.g. `ARISTA01T1`).
/// Management IPs are derived by incrementing from the supplied base address.
pub struct VmAllocator {
    base_name: String,
    base_ip: Ipv4Addr,
    vm_type: VmType,
    offset: u32,
}

impl VmAllocator {
    pub fn new(base_name: impl Into<String>, base_ip: Ipv4Addr, vm_type: VmType) -> Self {
        Self {
            base_name: base_name.into(),
            base_ip,
            vm_type,
            offset: 0,
        }
    }

    /// Allocates `count` VM definitions with sequential names and IPs.
    pub fn allocate(&mut self, count: u32) -> sonic_core::Result<Vec<VmDefinition>> {
        let mut vms = Vec::with_capacity(count as usize);

        for _ in 0..count {
            let name = format!("{}{:02}", self.base_name, self.offset);
            let ip_raw: u32 = self.base_ip.into();
            let mgmt_ip = Ipv4Addr::from(
                ip_raw
                    .checked_add(self.offset)
                    .ok_or_else(|| SonicError::IpExhausted(self.base_ip.to_string()))?,
            );

            debug!(name = %name, mgmt_ip = %mgmt_ip, "allocated VM");

            vms.push(VmDefinition {
                name,
                vm_type: self.vm_type,
                vm_offset: self.offset,
                mgmt_ip: IpAddr::V4(mgmt_ip),
                peer_ports: Vec::new(), // populated by the generator
            });

            self.offset += 1;
        }

        Ok(vms)
    }

    /// Returns the current offset (number of VMs allocated so far).
    pub fn allocated_count(&self) -> u32 {
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_allocator_pair() {
        let v4 = "10.0.0.0/24".parse::<Ipv4Network>().unwrap();
        let v6 = "fc00::/120".parse::<Ipv6Network>().unwrap();
        let mut alloc = IpAllocator::new(v4, v6);

        let pair = alloc.allocate_pair(0).unwrap();
        assert_eq!(pair.dut_ip.ipv4.unwrap(), Ipv4Addr::new(10, 0, 0, 0));
        assert_eq!(pair.neighbor_ip.ipv4.unwrap(), Ipv4Addr::new(10, 0, 0, 1));

        let pair2 = alloc.allocate_pair(1).unwrap();
        assert_eq!(pair2.dut_ip.ipv4.unwrap(), Ipv4Addr::new(10, 0, 0, 2));
    }

    #[test]
    fn vlan_allocator_basic() {
        let mut alloc = VlanAllocator::new(1000, 4094);
        let ids = alloc.allocate(4).unwrap();
        assert_eq!(ids, vec![1000, 1001, 1002, 1003]);
        assert_eq!(alloc.peek(), 1004);
    }

    #[test]
    fn vlan_allocator_overflow() {
        let mut alloc = VlanAllocator::new(4090, 4094);
        assert!(alloc.allocate(10).is_err());
    }

    #[test]
    fn vm_allocator_names() {
        let mut alloc = VmAllocator::new("ARISTA", Ipv4Addr::new(10, 250, 0, 2), VmType::Veos);
        let vms = alloc.allocate(3).unwrap();
        assert_eq!(vms[0].name, "ARISTA00");
        assert_eq!(vms[1].name, "ARISTA01");
        assert_eq!(vms[2].name, "ARISTA02");
        assert_eq!(
            vms[2].mgmt_ip,
            IpAddr::V4(Ipv4Addr::new(10, 250, 0, 4))
        );
    }
}
