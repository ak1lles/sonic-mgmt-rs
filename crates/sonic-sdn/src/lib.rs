//! SDN protocol support for SONiC management.
//!
//! Provides clients for the three main gRPC-based SDN interfaces:
//! - **gNMI** (gRPC Network Management Interface): device state and config
//! - **gNOI** (gRPC Network Operations Interface): operational commands
//! - **P4Runtime**: programmable forwarding-plane control

pub mod gnmi;
pub mod gnoi;
pub mod p4rt;

pub use gnmi::{GnmiClient, GnmiPath, PathElement, SubscriptionMode};
pub use gnoi::GnoiClient;
pub use p4rt::{P4RuntimeClient, TableEntry};
