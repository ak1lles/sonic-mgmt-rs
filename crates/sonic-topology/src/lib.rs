//! Network topology generation and management for SONiC.
//!
//! This crate provides:
//!
//! - **types**: topology-internal data structures (VMs, VLANs, host interfaces,
//!   LAG links, metadata).
//! - **allocator**: IP, VLAN, and VM resource allocators used during generation.
//! - **generator**: the [`DefaultTopologyGenerator`] that implements
//!   `sonic_core::TopologyGenerator` for every supported topology family.
//! - **templates**: Tera-based rendering of inventory, minigraph XML, and
//!   `config_db.json`.

pub mod allocator;
pub mod generator;
pub mod templates;
pub mod types;

pub use allocator::{IpAllocator, VlanAllocator, VmAllocator};
pub use generator::DefaultTopologyGenerator;
pub use templates::TopologyRenderer;
pub use types::*;
