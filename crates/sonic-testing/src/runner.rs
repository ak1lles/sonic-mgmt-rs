//! Top-level test runner implementing the `TestRunner` trait.
//!
//! Orchestrates the full test lifecycle: discovery, validation, global fixture
//! setup, batch execution, teardown, and result collection.

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::{error, info, warn};

use sonic_core::{
    Result, SonicError, TestCase, TestCaseResult, TestFilter, TestOutcome, TestRunner,
};

use crate::discovery::{self, TestDiscovery};
use crate::execution::{self, ExecutionContext, ProgressCallback, TestExecutor};
use crate::fixtures::{FixtureScope, setup_fixtures, teardown_fixtures};
use crate::results::{TestSuite, TestSummary};

// ---------------------------------------------------------------------------
// RunConfig
// ---------------------------------------------------------------------------

/// Configuration for a test run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    /// Number of parallel test workers.
    pub parallel: usize,

    /// Per-test timeout in seconds.
    pub timeout_secs: u64,

    /// Number of retries for failed tests.
    pub retry_failed: u32,

    /// Stop on first failure.
    pub fail_fast: bool,

    /// Directory for output artifacts (logs, reports).
    pub output_dir: PathBuf,

    /// If true, discover and report tests without executing them.
    pub dry_run: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            parallel: 1,
            timeout_secs: 900,
            retry_failed: 0,
            fail_fast: false,
            output_dir: PathBuf::from("output"),
            dry_run: false,
        }
    }
}

// ---------------------------------------------------------------------------
// SonicTestRunner
// ---------------------------------------------------------------------------

/// The top-level test runner that ties together discovery, fixtures, execution,
/// and result aggregation.
pub struct SonicTestRunner {
    /// Test discovery engine.
    discovery: TestDiscovery,

    /// Execution context shared across all tests in a run.
    context: ExecutionContext,

    /// Run configuration.
    config: RunConfig,

    /// Sender side of the cancellation channel. Dropping or sending `true`
    /// signals all running tests to abort.
    cancel_tx: watch::Sender<bool>,

    /// Receiver cloned into the executor.
    cancel_rx: watch::Receiver<bool>,

    /// Optional progress callback.
    progress_cb: Option<ProgressCallback>,
}

impl std::fmt::Debug for SonicTestRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SonicTestRunner")
            .field("config", &self.config)
            .field("testbed", &self.context.testbed_name)
            .finish()
    }
}

impl SonicTestRunner {
    /// Creates a new test runner.
    pub fn new(
        discovery: TestDiscovery,
        context: ExecutionContext,
        config: RunConfig,
    ) -> Self {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        Self {
            discovery,
            context,
            config,
            cancel_tx,
            cancel_rx,
            progress_cb: None,
        }
    }

    /// Registers a progress callback for real-time status updates.
    pub fn with_progress(mut self, cb: ProgressCallback) -> Self {
        self.progress_cb = Some(cb);
        self
    }

    /// Returns a reference to the run configuration.
    pub fn config(&self) -> &RunConfig {
        &self.config
    }

    /// Returns a mutable reference to the execution context.
    pub fn context_mut(&mut self) -> &mut ExecutionContext {
        &mut self.context
    }

    /// Generates a summary from a set of results.
    pub fn summarize(results: &[TestCaseResult]) -> TestSummary {
        let total = results.len();
        let passed = results
            .iter()
            .filter(|r| r.outcome == TestOutcome::Passed)
            .count();
        let failed = results
            .iter()
            .filter(|r| r.outcome == TestOutcome::Failed)
            .count();
        let skipped = results
            .iter()
            .filter(|r| r.outcome == TestOutcome::Skipped)
            .count();
        let errors = results
            .iter()
            .filter(|r| r.outcome == TestOutcome::Error)
            .count();

        let total_duration: std::time::Duration =
            results.iter().map(|r| r.duration).sum();

        let pass_rate = if total > 0 {
            passed as f64 / total as f64 * 100.0
        } else {
            0.0
        };

        TestSummary {
            total,
            passed,
            failed,
            skipped,
            errors,
            duration: total_duration,
            pass_rate,
        }
    }

    /// Builds a [`TestSuite`] from the results.
    pub fn build_suite(
        &self,
        results: Vec<TestCaseResult>,
        started_at: chrono::DateTime<chrono::Utc>,
    ) -> TestSuite {
        let finished_at = Utc::now();
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("testbed".to_owned(), self.context.testbed_name.clone());
        if let Some(ref topo) = self.context.topology {
            metadata.insert("topology".to_owned(), topo.to_string());
        }

        TestSuite {
            name: format!("sonic-{}", self.context.testbed_name),
            tests: results,
            started_at,
            finished_at,
            metadata,
        }
    }
}

#[async_trait]
impl TestRunner for SonicTestRunner {
    /// Discovers test cases matching the provided filter.
    async fn discover(&self, filter: &TestFilter) -> Result<Vec<TestCase>> {
        let mut all_cases = Vec::new();

        for search_path in &self.discovery.search_paths {
            let cases = discovery::discover_tests(search_path, filter)?;
            all_cases.extend(cases);
        }

        info!(
            total = all_cases.len(),
            paths = ?self.discovery.search_paths,
            "discovered tests"
        );

        Ok(all_cases)
    }

    /// Runs the provided test cases through the full execution lifecycle.
    ///
    /// Steps:
    /// 1. Validate the testbed (at least one DUT).
    /// 2. Setup global (session-scoped) fixtures.
    /// 3. Execute test batches with the configured parallelism.
    /// 4. Teardown global fixtures.
    /// 5. Collect and return results.
    async fn run(&self, cases: &[TestCase]) -> Result<Vec<TestCaseResult>> {
        if cases.is_empty() {
            info!("no test cases to run");
            return Ok(Vec::new());
        }

        info!(
            count = cases.len(),
            testbed = %self.context.testbed_name,
            parallel = self.config.parallel,
            dry_run = self.config.dry_run,
            "starting test run"
        );

        // Dry-run: just list what would execute.
        if self.config.dry_run {
            info!("dry-run mode: listing tests without executing");
            let now = Utc::now();
            let results: Vec<TestCaseResult> = cases
                .iter()
                .map(|c| TestCaseResult {
                    test_case: c.clone(),
                    outcome: TestOutcome::Skipped,
                    duration: std::time::Duration::ZERO,
                    message: Some("dry run".to_owned()),
                    stdout: None,
                    stderr: None,
                    started_at: now,
                    finished_at: now,
                })
                .collect();
            return Ok(results);
        }

        // Step 1: Validate testbed.
        if self.context.dut_info.is_empty() {
            warn!("no DUT devices configured in execution context");
        }

        // Step 2: Setup global fixtures.
        let mut fixture_ctx = self.context.fixture_context.clone();
        let global_fixtures: Vec<_> = self
            .context
            .fixture_registry
            .names()
            .iter()
            .filter_map(|name| {
                let def = self.context.fixture_registry.get(name)?;
                if def.scope == FixtureScope::Session {
                    Some(def.clone())
                } else {
                    None
                }
            })
            .collect();

        if !global_fixtures.is_empty() {
            info!(
                count = global_fixtures.len(),
                "setting up session-scoped fixtures"
            );
            setup_fixtures(&mut fixture_ctx, &global_fixtures).map_err(|e| {
                SonicError::Test(format!("global fixture setup failed: {e}"))
            })?;
        }

        // Step 3: Build executor and run batches.
        let executor = {
            let mut e = TestExecutor::new(self.cancel_rx.clone())
                .with_max_workers(self.config.parallel)
                .with_timeout(self.config.timeout_secs)
                .with_retries(self.config.retry_failed)
                .with_fail_fast(self.config.fail_fast);

            if let Some(ref cb) = self.progress_cb {
                e = e.with_progress(cb.clone());
            }

            e
        };

        let mut exec_context = self.context.clone();
        exec_context.fixture_context = fixture_ctx.clone();

        let results = execution::execute_batch(&executor, cases, &exec_context).await;

        // Step 4: Teardown global fixtures.
        if !global_fixtures.is_empty() {
            info!("tearing down session-scoped fixtures");
            if let Err(e) = teardown_fixtures(&mut fixture_ctx, &global_fixtures) {
                error!(error = %e, "global fixture teardown failed");
            }
        }

        // Step 5: Log summary.
        let summary = Self::summarize(&results);
        info!(
            total = summary.total,
            passed = summary.passed,
            failed = summary.failed,
            skipped = summary.skipped,
            errors = summary.errors,
            pass_rate = format!("{:.1}%", summary.pass_rate),
            duration_secs = summary.duration.as_secs(),
            "test run complete"
        );

        Ok(results)
    }

    /// Signals cancellation to all running tests.
    async fn stop(&self) -> Result<()> {
        info!("stopping test execution");
        self.cancel_tx.send(true).map_err(|e| {
            SonicError::Test(format!("failed to send cancel signal: {e}"))
        })?;
        Ok(())
    }
}

/// Generates a human-readable summary string with failure details.
pub fn format_run_summary(results: &[TestCaseResult]) -> String {
    let summary = SonicTestRunner::summarize(results);
    let mut out = String::new();

    out.push_str(&format!(
        "\n=== Test Run Summary ===\n\
         Total:   {}\n\
         Passed:  {}\n\
         Failed:  {}\n\
         Skipped: {}\n\
         Errors:  {}\n\
         Duration: {:.1}s\n\
         Pass Rate: {:.1}%\n",
        summary.total,
        summary.passed,
        summary.failed,
        summary.skipped,
        summary.errors,
        summary.duration.as_secs_f64(),
        summary.pass_rate,
    ));

    // List failures.
    let failures: Vec<_> = results
        .iter()
        .filter(|r| r.outcome == TestOutcome::Failed || r.outcome == TestOutcome::Error)
        .collect();

    if !failures.is_empty() {
        out.push_str("\n--- Failures ---\n");
        for result in &failures {
            out.push_str(&format!(
                "  {} [{}] {}\n",
                result.test_case.name,
                result.outcome,
                result.message.as_deref().unwrap_or(""),
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_core::TestOutcome;

    fn sample_results() -> Vec<TestCaseResult> {
        let now = Utc::now();
        vec![
            TestCaseResult {
                test_case: TestCase {
                    id: "a::t1".into(),
                    name: "test_pass".into(),
                    module: "a".into(),
                    tags: vec![],
                    topology: None,
                    platform: None,
                    description: None,
                    timeout_secs: 60,
                },
                outcome: TestOutcome::Passed,
                duration: std::time::Duration::from_secs(2),
                message: None,
                stdout: None,
                stderr: None,
                started_at: now,
                finished_at: now,
            },
            TestCaseResult {
                test_case: TestCase {
                    id: "a::t2".into(),
                    name: "test_fail".into(),
                    module: "a".into(),
                    tags: vec![],
                    topology: None,
                    platform: None,
                    description: None,
                    timeout_secs: 60,
                },
                outcome: TestOutcome::Failed,
                duration: std::time::Duration::from_secs(5),
                message: Some("assertion failed".into()),
                stdout: None,
                stderr: None,
                started_at: now,
                finished_at: now,
            },
            TestCaseResult {
                test_case: TestCase {
                    id: "b::t3".into(),
                    name: "test_skip".into(),
                    module: "b".into(),
                    tags: vec![],
                    topology: None,
                    platform: None,
                    description: None,
                    timeout_secs: 60,
                },
                outcome: TestOutcome::Skipped,
                duration: std::time::Duration::ZERO,
                message: Some("not applicable".into()),
                stdout: None,
                stderr: None,
                started_at: now,
                finished_at: now,
            },
        ]
    }

    #[test]
    fn summarize_counts() {
        let results = sample_results();
        let summary = SonicTestRunner::summarize(&results);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.errors, 0);
    }

    #[test]
    fn summarize_pass_rate() {
        let results = sample_results();
        let summary = SonicTestRunner::summarize(&results);
        // 1 passed out of 3 = 33.33%
        assert!((summary.pass_rate - 33.33).abs() < 1.0);
    }

    #[test]
    fn format_summary_includes_failures() {
        let results = sample_results();
        let text = format_run_summary(&results);
        assert!(text.contains("test_fail"));
        assert!(text.contains("assertion failed"));
        assert!(text.contains("Pass Rate:"));
    }

    #[test]
    fn empty_results_summary() {
        let summary = SonicTestRunner::summarize(&[]);
        assert_eq!(summary.total, 0);
        assert_eq!(summary.pass_rate, 0.0);
    }
}
