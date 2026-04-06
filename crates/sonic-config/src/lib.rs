//! Configuration management for the SONiC management framework.
//!
//! This crate provides strongly-typed configuration loading, validation, and
//! serialization for every layer of a SONiC testbed: application settings,
//! testbed definitions, device inventories, and topology templates.
//!
//! All configuration files may be authored in TOML or YAML. The canonical
//! on-disk format for application settings is TOML; testbed and inventory files
//! support both TOML and YAML so that existing `testbed.yaml` files can be
//! consumed without conversion.

pub mod app;
pub mod inventory;
pub mod testbed;
pub mod topology;

pub use app::{AppConfig, LogFormat};
pub use inventory::{DeviceEntry, InventoryConfig};
pub use testbed::{
    DutConfig, DutCredentials, FanoutConfig, NeighborConfig, PhysicalLink, TestbedConfig,
};
pub use topology::{ConfigProperty, TopologyConfig, VlanConfig, VlanType, VmConfig};
