//! Device connection transports.
//!
//! Each submodule implements the [`sonic_core::Connection`] trait for a
//! specific transport protocol:
//!
//! - [`ssh`] -- SSH via the `russh` crate (the default for most devices).
//! - [`telnet`] -- Raw TCP telnet with RFC 854 option negotiation.
//! - [`console`] -- Serial console via a conserver aggregation server.
//! - [`pool`] -- Connection pooling and reuse.
//!
//! All transports share the same async interface: [`open`](sonic_core::Connection::open),
//! [`send_command`](sonic_core::Connection::send_command), and
//! [`close`](sonic_core::Connection::close).

pub mod ssh;
pub mod telnet;
pub mod console;
pub mod pool;
