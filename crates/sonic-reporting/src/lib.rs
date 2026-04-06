//! Test reporting, JUnit parsing, and analytics for SONiC testing.
//!
//! This crate provides:
//! - JUnit XML parsing and conversion
//! - Pluggable storage backends (Kusto, local file)
//! - Report upload orchestration with retry logic
//! - SAI/SWSS test coverage tracking

pub mod coverage;
pub mod junit;
pub mod storage;
pub mod uploader;

pub use coverage::{CoverageReport, CoverageTracker};
pub use junit::{JunitReport, JunitTestCase, JunitTestSuite};
pub use storage::{KustoStorage, LocalFileStorage, ReportStorage, StorageConfig};
pub use uploader::{ReportUploadManager, UploadMetadata};
