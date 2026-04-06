//! Integration tests for the SONiC management framework.
//!
//! Tests live in `tests/integration.rs` and run against a real SONiC device.
//! They are `#[ignore]`d by default so `cargo test` skips them.
//!
//! Target resolution (first match wins):
//!
//! 1. `SONIC_SSH_HOST` env var -- use `SONIC_SSH_*` env vars directly
//! 2. `SONIC_TESTBED` env var -- load testbed config, use the primary DUT
//! 3. Fall back to `127.0.0.1:22` with admin/password
//!
//! Run with:
//!
//! ```sh
//! SONIC_TESTBED=lab1 cargo test -p sonic-integration-tests --test integration -- --ignored
//! ```
