//! CLI subcommand implementations.
//!
//! Each submodule registers a clap subcommand and its handler:
//!
//! - [`config`] -- validate and inspect testbed configuration files.
//! - [`device`] -- connect to devices and run ad-hoc commands.
//! - [`docker`] -- manage the Docker-based test runner container.
//! - [`report`] -- parse, display, and upload test result reports.
//! - [`sdn`] -- gNMI/gNOI/P4Runtime operations.
//! - [`test`][mod@test] -- discover and execute test cases.
//! - [`testbed`] -- deploy, inspect, and tear down testbeds.
//! - [`topology`] -- generate and render topology files.

pub mod config;
pub mod device;
pub mod docker;
pub mod report;
pub mod sdn;
pub mod test;
pub mod testbed;
pub mod topology;
