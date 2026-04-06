//! Report upload orchestration with retry logic.
//!
//! Implements the [`ReportUploader`] trait from `sonic-core`, adding metadata
//! enrichment and exponential-backoff retries on top of a pluggable
//! [`ReportStorage`] backend.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use sonic_core::{ReportUploader, Result, TestCaseResult};

use crate::storage::ReportStorage;

// ---------------------------------------------------------------------------
// UploadMetadata
// ---------------------------------------------------------------------------

/// Metadata attached to every uploaded batch of results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadMetadata {
    /// Name of the testbed that produced the results.
    pub testbed_name: String,

    /// SONiC OS version under test.
    pub sonic_version: String,

    /// Unique identifier for this test run.
    pub run_id: String,

    /// Optional external CI/CD pipeline identifier.
    pub external_id: Option<String>,

    /// Optional pipeline ID (e.g. Azure DevOps build ID).
    pub pipeline_id: Option<String>,

    /// Topology used during the run.
    pub topology: Option<String>,

    /// Platform / ASIC vendor.
    pub platform: Option<String>,
}

impl Default for UploadMetadata {
    fn default() -> Self {
        Self {
            testbed_name: String::new(),
            sonic_version: String::new(),
            run_id: Uuid::new_v4().to_string(),
            external_id: None,
            pipeline_id: None,
            topology: None,
            platform: None,
        }
    }
}

impl UploadMetadata {
    /// Creates metadata with the required fields.
    pub fn new(
        testbed_name: impl Into<String>,
        sonic_version: impl Into<String>,
    ) -> Self {
        Self {
            testbed_name: testbed_name.into(),
            sonic_version: sonic_version.into(),
            ..Default::default()
        }
    }

    /// Sets the run ID.
    pub fn with_run_id(mut self, id: impl Into<String>) -> Self {
        self.run_id = id.into();
        self
    }

    /// Sets the external CI/CD identifier.
    pub fn with_external_id(mut self, id: impl Into<String>) -> Self {
        self.external_id = Some(id.into());
        self
    }

    /// Sets the pipeline ID.
    pub fn with_pipeline_id(mut self, id: impl Into<String>) -> Self {
        self.pipeline_id = Some(id.into());
        self
    }

    /// Sets the topology.
    pub fn with_topology(mut self, topo: impl Into<String>) -> Self {
        self.topology = Some(topo.into());
        self
    }

    /// Sets the platform.
    pub fn with_platform(mut self, plat: impl Into<String>) -> Self {
        self.platform = Some(plat.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Retry configuration
// ---------------------------------------------------------------------------

/// Configuration for exponential-backoff retries.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of attempts (including the initial one).
    pub max_attempts: u32,
    /// Initial delay between retries.
    pub initial_delay: Duration,
    /// Maximum delay cap.
    pub max_delay: Duration,
    /// Multiplier applied to the delay after each retry.
    pub backoff_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            backoff_factor: 2.0,
        }
    }
}

impl RetryConfig {
    /// Computes the delay for the given attempt number (0-indexed).
    fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let multiplier = self.backoff_factor.powi(attempt as i32);
        let delay = Duration::from_secs_f64(
            self.initial_delay.as_secs_f64() * multiplier,
        );
        delay.min(self.max_delay)
    }
}

// ---------------------------------------------------------------------------
// ReportUploadManager
// ---------------------------------------------------------------------------

/// Orchestrates report uploads to a storage backend with metadata enrichment
/// and retry logic.
pub struct ReportUploadManager {
    /// The underlying storage backend.
    storage: Arc<dyn ReportStorage>,

    /// Metadata attached to uploads.
    pub metadata: UploadMetadata,

    /// Retry configuration.
    pub retry_config: RetryConfig,
}

impl std::fmt::Debug for ReportUploadManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReportUploadManager")
            .field("metadata", &self.metadata)
            .field("retry_config", &self.retry_config)
            .finish()
    }
}

impl ReportUploadManager {
    /// Creates a new upload manager.
    pub fn new(
        storage: Arc<dyn ReportStorage>,
        metadata: UploadMetadata,
    ) -> Self {
        Self {
            storage,
            metadata,
            retry_config: RetryConfig::default(),
        }
    }

    /// Sets the retry configuration.
    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Enriches test results with upload metadata by injecting metadata
    /// fields into each result's test case tags.
    fn enrich_results(&self, results: &[TestCaseResult]) -> Vec<TestCaseResult> {
        results
            .iter()
            .map(|r| {
                let mut enriched = r.clone();
                let tags = &mut enriched.test_case.tags;

                // Add metadata as tags for easy querying.
                if !self.metadata.testbed_name.is_empty() {
                    tags.push(format!("testbed:{}", self.metadata.testbed_name));
                }
                if !self.metadata.sonic_version.is_empty() {
                    tags.push(format!("version:{}", self.metadata.sonic_version));
                }
                tags.push(format!("run_id:{}", self.metadata.run_id));

                if let Some(ref ext_id) = self.metadata.external_id {
                    tags.push(format!("external_id:{ext_id}"));
                }
                if let Some(ref pipeline_id) = self.metadata.pipeline_id {
                    tags.push(format!("pipeline_id:{pipeline_id}"));
                }
                if let Some(ref topo) = self.metadata.topology {
                    tags.push(format!("topology:{topo}"));
                }
                if let Some(ref plat) = self.metadata.platform {
                    tags.push(format!("platform:{plat}"));
                }

                enriched
            })
            .collect()
    }

    /// Executes a closure with exponential-backoff retry.
    async fn with_retry<F, Fut>(&self, operation: &str, f: F) -> Result<()>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let mut attempt = 0u32;

        loop {
            match f().await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    attempt += 1;
                    if attempt >= self.retry_config.max_attempts {
                        error!(
                            operation,
                            attempts = attempt,
                            error = %e,
                            "all retry attempts exhausted"
                        );
                        return Err(e);
                    }

                    let delay = self.retry_config.delay_for_attempt(attempt - 1);
                    warn!(
                        operation,
                        attempt,
                        max_attempts = self.retry_config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "retrying after failure"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

#[async_trait]
impl ReportUploader for ReportUploadManager {
    /// Uploads results to the storage backend with metadata enrichment and
    /// retry logic.
    async fn upload(&self, results: &[TestCaseResult]) -> Result<()> {
        if results.is_empty() {
            debug!("no results to upload");
            return Ok(());
        }

        let enriched = self.enrich_results(results);
        let storage = self.storage.clone();
        let count = enriched.len();

        info!(
            count,
            testbed = %self.metadata.testbed_name,
            run_id = %self.metadata.run_id,
            "uploading test results"
        );

        // Clone enriched for the retry closure.
        let enriched = Arc::new(enriched);
        let storage_ref = storage.clone();
        let enriched_ref = enriched.clone();

        self.with_retry("upload", || {
            let s = storage_ref.clone();
            let e = enriched_ref.clone();
            async move { s.store(&e).await }
        })
        .await?;

        info!(count, "upload complete");
        Ok(())
    }

    /// Checks connectivity to the storage backend by requesting schema info.
    async fn check_connection(&self) -> Result<()> {
        info!("checking storage backend connection");

        let storage = self.storage.clone();
        let schema = storage.schema_info().await?;

        debug!(
            backend = %schema.backend,
            columns = schema.columns.len(),
            "storage connection verified"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::storage::LocalFileStorage;
    use sonic_core::{ReportFormat, TestCase, TestOutcome};
    use std::collections::HashMap;

    fn sample_results() -> Vec<TestCaseResult> {
        let now = Utc::now();
        vec![TestCaseResult {
            test_case: TestCase {
                id: "bgp::test_conv".into(),
                name: "test_conv".into(),
                module: "bgp".into(),
                tags: vec!["bgp".into()],
                topology: None,
                platform: None,
                description: None,
                timeout_secs: 60,
            },
            outcome: TestOutcome::Passed,
            duration: Duration::from_secs(3),
            message: None,
            stdout: None,
            stderr: None,
            started_at: now,
            finished_at: now,
        }]
    }

    #[test]
    fn metadata_builder() {
        let meta = UploadMetadata::new("vms-t0", "20240101.1")
            .with_run_id("abc-123")
            .with_topology("t0")
            .with_platform("broadcom");

        assert_eq!(meta.testbed_name, "vms-t0");
        assert_eq!(meta.sonic_version, "20240101.1");
        assert_eq!(meta.run_id, "abc-123");
        assert_eq!(meta.topology.as_deref(), Some("t0"));
        assert_eq!(meta.platform.as_deref(), Some("broadcom"));
    }

    #[test]
    fn retry_delay_computation() {
        let cfg = RetryConfig {
            max_attempts: 5,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(16),
            backoff_factor: 2.0,
        };

        assert_eq!(cfg.delay_for_attempt(0), Duration::from_secs(1));
        assert_eq!(cfg.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(cfg.delay_for_attempt(2), Duration::from_secs(4));
        assert_eq!(cfg.delay_for_attempt(3), Duration::from_secs(8));
        // Capped at max_delay.
        assert_eq!(cfg.delay_for_attempt(4), Duration::from_secs(16));
        assert_eq!(cfg.delay_for_attempt(5), Duration::from_secs(16));
    }

    #[test]
    fn enrich_results_adds_metadata_tags() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(LocalFileStorage::new(dir.path(), ReportFormat::Json));
        let meta = UploadMetadata::new("vms-t0", "20240101.1")
            .with_topology("t0");

        let manager = ReportUploadManager::new(storage, meta);
        let enriched = manager.enrich_results(&sample_results());

        assert_eq!(enriched.len(), 1);
        let tags = &enriched[0].test_case.tags;
        assert!(tags.iter().any(|t| t == "testbed:vms-t0"));
        assert!(tags.iter().any(|t| t == "version:20240101.1"));
        assert!(tags.iter().any(|t| t.starts_with("run_id:")));
        assert!(tags.iter().any(|t| t == "topology:t0"));
    }

    #[tokio::test]
    async fn upload_to_local_file_storage() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(LocalFileStorage::new(dir.path(), ReportFormat::Json));
        let meta = UploadMetadata::new("vms-t0", "v1");

        let manager = ReportUploadManager::new(storage.clone(), meta);
        manager.upload(&sample_results()).await.unwrap();

        // Verify results were stored.
        let queried = storage.query(&HashMap::new()).await.unwrap();
        assert_eq!(queried.len(), 1);
    }

    #[tokio::test]
    async fn check_connection_succeeds_for_file_storage() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(LocalFileStorage::new(dir.path(), ReportFormat::Json));
        let meta = UploadMetadata::default();
        let manager = ReportUploadManager::new(storage, meta);

        manager.check_connection().await.unwrap();
    }
}
