//! SDN protocol support for SONiC management.
//!
//! Provides clients for the three main gRPC-based SDN interfaces:
//! - **gNMI** (gRPC Network Management Interface): device state and config
//! - **gNOI** (gRPC Network Operations Interface): operational commands
//! - **P4Runtime**: programmable forwarding-plane control

pub mod gnmi;
pub mod gnoi;
pub mod p4rt;

/// Generated protobuf types and gRPC client stubs.
pub mod proto {
    /// gNMI (gRPC Network Management Interface) protobuf types.
    pub mod gnmi {
        tonic::include_proto!("gnmi");
    }
    /// gNOI System service protobuf types.
    pub mod gnoi_system {
        tonic::include_proto!("gnoi.system");
    }
    /// gNOI shared types.
    pub mod gnoi_types {
        tonic::include_proto!("gnoi.types");
    }
    /// P4Runtime protobuf types.
    #[allow(clippy::module_inception)]
    pub mod p4 {
        pub mod v1 {
            tonic::include_proto!("p4.v1");
        }
        pub mod config {
            pub mod v1 {
                tonic::include_proto!("p4.config.v1");
            }
        }
    }
    /// Google RPC status type used by P4Runtime master arbitration.
    pub mod google {
        pub mod rpc {
            tonic::include_proto!("google.rpc");
        }
    }
}

pub use gnmi::{GnmiClient, GnmiPath, PathElement, SubscriptionMode};
pub use gnoi::GnoiClient;
pub use p4rt::{P4RuntimeClient, TableEntry};
