//! Testbed management and provisioning for SONiC testing.
//!
//! This crate provides:
//!
//! - **manager**: the [`Testbed`] struct implementing `sonic_core::TestbedManager`
//!   for full lifecycle management (deploy, teardown, health-check, route
//!   announcement).
//! - **inventory**: loading, saving, and querying device inventories in TOML.
//! - **health**: concurrent health checking of all devices in a testbed.
//! - **operations**: high-level orchestration commands (add/remove topology,
//!   deploy minigraph, upgrade SONiC, recover testbed).

pub mod health;
pub mod inventory;
pub mod manager;
pub mod operations;

pub use health::{DeviceHealth, HealthChecker, TestbedHealth};
pub use inventory::{DeviceEntry, InventoryManager};
pub use manager::{DutConfig, NeighborConfig, Testbed, TestbedConfig};
pub use operations::TestbedOps;
