//! Device facts collection and caching.
//!
//! Facts are structured snapshots of a device's state -- basic system info,
//! BGP sessions, interface inventory, and running configuration. Each host
//! driver implements [`sonic_core::FactsProvider`] and delegates parsing to
//! the [`parser`] module.
//!
//! The [`cache`] module provides a TTL-based in-memory cache so that
//! repeated facts queries within the same test run avoid redundant SSH
//! round-trips.

pub mod cache;
pub mod parser;
