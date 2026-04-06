//! Test execution framework for SONiC network testing.
//!
//! This crate provides the complete test lifecycle: discovery of test cases from
//! TOML definition files, fixture management with dependency resolution,
//! parallel test execution with crash detection and recovery, and result
//! aggregation with multi-format output.

pub mod discovery;
pub mod execution;
pub mod fixtures;
pub mod results;
pub mod runner;

pub use discovery::{TestDefinitionFile, TestDiscovery};
pub use execution::{ExecutionContext, TestExecutor};
pub use fixtures::{FixtureContext, FixtureDef, FixtureRegistry, FixtureScope};
pub use results::{TestSuite, TestSummary};
pub use runner::{RunConfig, SonicTestRunner};
