//! Core types, traits, and error definitions for the SONiC management framework.
//!
//! This crate is the foundation of the workspace. Every other crate depends on it
//! for shared abstractions.
//!
//! # Types
//!
//! Enumerations and structs that model the SONiC testing domain:
//!
//! - [`DeviceType`] -- SONiC, EOS, Cisco, Fanout, PTF, and others
//! - [`Platform`] -- ASIC vendors: Broadcom, Mellanox, Barefoot, Marvell, Nokia
//! - [`TopologyType`] -- T0, T1, T2, Dualtor, PTF variants, and 13 total layouts
//! - [`ConnectionType`] -- SSH, Telnet, Console, gRPC, Local
//! - [`DeviceInfo`] -- identity, management IP, credentials, and metadata for a device
//! - [`Credentials`] -- username with optional password or key path
//! - [`CommandResult`] -- stdout, stderr, and exit code from a remote command
//!
//! # Traits
//!
//! Async traits that define the interfaces between crates:
//!
//! - [`Device`] -- connect, disconnect, execute, reboot, wait_ready
//! - [`Connection`] -- open, close, send_command, is_alive
//! - [`FactsProvider`] -- basic_facts, bgp_facts, interface_facts, config_facts
//! - [`TestbedManager`] -- deploy, teardown, health_check, announce_routes
//! - [`TopologyGenerator`] -- generate topology definitions from a type
//! - [`TestRunner`] -- discover, run, and stop test suites
//!
//! # Errors
//!
//! [`SonicError`] covers connection failures, authentication errors, command
//! execution issues, configuration problems, timeouts, and more. All crates
//! return [`Result<T>`](Result) which is `std::result::Result<T, SonicError>`.

pub mod error;
pub mod traits;
pub mod types;

pub use error::{Result, SonicError};
pub use traits::*;
pub use types::*;
