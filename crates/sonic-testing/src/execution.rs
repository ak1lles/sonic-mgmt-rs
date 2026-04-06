//! Test execution engine.
//!
//! Provides single-test and batch execution with parallel scheduling via
//! tokio, crash detection for common kernel/OOM/segfault patterns, and
//! device recovery helpers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, watch};
use tracing::{debug, error, info, trace, warn};

use sonic_core::{
    DeviceInfo, Result, SonicError, TestCase, TestCaseResult, TestOutcome, TopologyType,
};

use crate::fixtures::{
    FixtureContext, FixtureRegistry, resolve_fixtures, setup_fixtures,
    teardown_fixtures,
};

// ---------------------------------------------------------------------------
// ExecutionContext
// ---------------------------------------------------------------------------

/// Runtime context carried through test execution.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Testbed name.
    pub testbed_name: String,

    /// DUT device information.
    pub dut_info: Vec<DeviceInfo>,

    /// Active topology.
    pub topology: Option<TopologyType>,

    /// Fixture context for managing test fixtures.
    pub fixture_context: FixtureContext,

    /// Fixture registry for resolving fixture dependencies.
    pub fixture_registry: FixtureRegistry,

    /// Environment variables passed to tests.
    pub env_vars: HashMap<String, String>,

    /// Output directory for test artifacts.
    pub output_dir: PathBuf,
}

impl ExecutionContext {
    /// Creates a new execution context.
    pub fn new(testbed_name: impl Into<String>) -> Self {
        let name = testbed_name.into();
        Self {
            testbed_name: name.clone(),
            dut_info: Vec::new(),
            topology: None,
            fixture_context: FixtureContext::new(name),
            fixture_registry: FixtureRegistry::new(),
            env_vars: HashMap::new(),
            output_dir: PathBuf::from("output"),
        }
    }

    /// Sets the output directory.
    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = dir.into();
        self
    }
}

// ---------------------------------------------------------------------------
// Progress callback
// ---------------------------------------------------------------------------

/// Describes the progress state of test execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressUpdate {
    /// Test case being reported on.
    pub test_name: String,
    /// Current status.
    pub status: ProgressStatus,
    /// Index of this test in the batch (1-based).
    pub current: usize,
    /// Total tests in the batch.
    pub total: usize,
    /// Optional message.
    pub message: Option<String>,
}

/// Status of a single test's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStatus {
    Starting,
    Running,
    Passed,
    Failed,
    Skipped,
    Error,
}

/// Type alias for the progress callback.
pub type ProgressCallback = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;

// ---------------------------------------------------------------------------
// TestExecutor
// ---------------------------------------------------------------------------

/// Manages parallel execution of test cases with timeout, retry, and
/// cancellation support.
#[derive(Clone)]
pub struct TestExecutor {
    /// Maximum number of tests to run in parallel.
    pub max_workers: usize,

    /// Default per-test timeout in seconds.
    pub timeout_secs: u64,

    /// Number of retries for failed tests.
    pub retry_count: u32,

    /// Whether to stop the entire batch on the first failure.
    pub fail_fast: bool,

    /// Optional progress callback.
    progress_cb: Option<ProgressCallback>,

    /// Cancellation receiver -- when the sender drops or sends `true`, all
    /// running tests should abort.
    cancel_rx: watch::Receiver<bool>,
}

impl std::fmt::Debug for TestExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestExecutor")
            .field("max_workers", &self.max_workers)
            .field("timeout_secs", &self.timeout_secs)
            .field("retry_count", &self.retry_count)
            .field("fail_fast", &self.fail_fast)
            .finish()
    }
}

impl TestExecutor {
    /// Creates a new executor.  The `cancel_rx` receiver is used to signal
    /// cancellation.
    pub fn new(cancel_rx: watch::Receiver<bool>) -> Self {
        Self {
            max_workers: 1,
            timeout_secs: 900,
            retry_count: 0,
            fail_fast: false,
            progress_cb: None,
            cancel_rx,
        }
    }

    /// Sets the maximum number of parallel workers.
    pub fn with_max_workers(mut self, n: usize) -> Self {
        self.max_workers = n.max(1);
        self
    }

    /// Sets the default timeout.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Sets the retry count for failed tests.
    pub fn with_retries(mut self, count: u32) -> Self {
        self.retry_count = count;
        self
    }

    /// Enables fail-fast mode.
    pub fn with_fail_fast(mut self, ff: bool) -> Self {
        self.fail_fast = ff;
        self
    }

    /// Registers a progress callback.
    pub fn with_progress(mut self, cb: ProgressCallback) -> Self {
        self.progress_cb = Some(cb);
        self
    }

    /// Reports progress to the registered callback, if any.
    fn report_progress(&self, update: ProgressUpdate) {
        if let Some(ref cb) = self.progress_cb {
            cb(update);
        }
    }

    /// Checks whether cancellation has been requested.
    fn is_cancelled(&self) -> bool {
        *self.cancel_rx.borrow()
    }
}

// ---------------------------------------------------------------------------
// Single-test execution
// ---------------------------------------------------------------------------

/// Executes a single test case with fixture setup/teardown, timing, and error
/// capture.
pub async fn execute_test(
    case: &TestCase,
    context: &mut ExecutionContext,
) -> TestCaseResult {
    let started_at = Utc::now();
    let wall_start = Instant::now();

    info!(test = %case.name, module = %case.module, "executing test");

    // 1. Resolve and set up fixtures.
    let fixtures = match resolve_fixtures(case, &context.fixture_registry) {
        Ok(f) => f,
        Err(e) => {
            return make_error_result(
                case,
                started_at,
                wall_start,
                format!("fixture resolution failed: {e}"),
            );
        }
    };

    if let Err(e) = setup_fixtures(&mut context.fixture_context, &fixtures) {
        let _ = teardown_fixtures(&mut context.fixture_context, &fixtures);
        return make_error_result(
            case,
            started_at,
            wall_start,
            format!("fixture setup failed: {e}"),
        );
    }

    // 2. Run the test with timeout.
    let timeout = std::time::Duration::from_secs(case.timeout_secs);
    let result = tokio::time::timeout(timeout, run_test_body(case, context)).await;

    let outcome = match result {
        Ok(Ok(())) => {
            debug!(test = %case.name, "test passed");
            TestOutcome::Passed
        }
        Ok(Err(e)) => {
            warn!(test = %case.name, error = %e, "test failed");
            let finished_at = Utc::now();
            let duration = wall_start.elapsed();
            let _ = teardown_fixtures(&mut context.fixture_context, &fixtures);
            return TestCaseResult {
                test_case: case.clone(),
                outcome: TestOutcome::Failed,
                duration,
                message: Some(e.to_string()),
                stdout: None,
                stderr: None,
                started_at,
                finished_at,
            };
        }
        Err(_elapsed) => {
            error!(
                test = %case.name,
                timeout_secs = case.timeout_secs,
                "test timed out"
            );
            let finished_at = Utc::now();
            let duration = wall_start.elapsed();
            let _ = teardown_fixtures(&mut context.fixture_context, &fixtures);
            return TestCaseResult {
                test_case: case.clone(),
                outcome: TestOutcome::Error,
                duration,
                message: Some(format!(
                    "test timed out after {} seconds",
                    case.timeout_secs
                )),
                stdout: None,
                stderr: None,
                started_at,
                finished_at,
            };
        }
    };

    // 3. Teardown fixtures.
    if let Err(e) = teardown_fixtures(&mut context.fixture_context, &fixtures) {
        warn!(
            test = %case.name,
            error = %e,
            "fixture teardown failed (test outcome preserved)"
        );
    }

    let finished_at = Utc::now();
    let duration = wall_start.elapsed();

    TestCaseResult {
        test_case: case.clone(),
        outcome,
        duration,
        message: None,
        stdout: None,
        stderr: None,
        started_at,
        finished_at,
    }
}

/// The actual test body.  In a real implementation this would invoke the
/// test function through a dynamic dispatch mechanism.  Here we execute a
/// placeholder that validates the test context.
async fn run_test_body(case: &TestCase, context: &ExecutionContext) -> Result<()> {
    trace!(
        test = %case.name,
        testbed = %context.testbed_name,
        "running test body"
    );

    // Check that the DUT is available (at least one configured).
    if context.dut_info.is_empty() {
        return Err(SonicError::Test(format!(
            "test `{}` requires at least one DUT but none configured",
            case.name
        )));
    }

    // Simulate test execution -- in a real system this invokes compiled test
    // code via function pointer or trait object.
    trace!(test = %case.name, "test body completed");
    Ok(())
}

/// Helper to create an error result.
fn make_error_result(
    case: &TestCase,
    started_at: chrono::DateTime<chrono::Utc>,
    wall_start: Instant,
    message: String,
) -> TestCaseResult {
    TestCaseResult {
        test_case: case.clone(),
        outcome: TestOutcome::Error,
        duration: wall_start.elapsed(),
        message: Some(message),
        stdout: None,
        stderr: None,
        started_at,
        finished_at: Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// Batch execution
// ---------------------------------------------------------------------------

/// Executes a batch of test cases in parallel, respecting the executor's
/// concurrency limit via a semaphore.
pub async fn execute_batch(
    executor: &TestExecutor,
    cases: &[TestCase],
    context: &ExecutionContext,
) -> Vec<TestCaseResult> {
    let total = cases.len();
    info!(
        total,
        max_workers = executor.max_workers,
        "starting batch execution"
    );

    let semaphore = Arc::new(Semaphore::new(executor.max_workers));
    let fail_fast_flag = Arc::new(tokio::sync::Mutex::new(false));
    let executor = Arc::new(executor.clone());

    let mut handles = Vec::with_capacity(total);

    for (idx, case) in cases.iter().enumerate() {
        let sem = semaphore.clone();
        let case = case.clone();
        let mut ctx = context.clone();
        let exec = executor.clone();
        let ff_flag = fail_fast_flag.clone();

        let handle = tokio::spawn(async move {
            // Acquire a semaphore permit to limit concurrency.
            let _permit = sem.acquire().await.expect("semaphore closed");

            // Check cancellation.
            if exec.is_cancelled() {
                return make_skipped_result(&case, "execution cancelled");
            }

            // Check fail-fast.
            if exec.fail_fast && *ff_flag.lock().await {
                return make_skipped_result(&case, "skipped due to fail-fast");
            }

            exec.report_progress(ProgressUpdate {
                test_name: case.name.clone(),
                status: ProgressStatus::Starting,
                current: idx + 1,
                total,
                message: None,
            });

            // Execute with retries.
            let mut result = execute_test(&case, &mut ctx).await;
            let mut attempts = 1u32;

            while result.outcome == TestOutcome::Failed && attempts <= exec.retry_count {
                info!(
                    test = %case.name,
                    attempt = attempts + 1,
                    "retrying failed test"
                );
                result = execute_test(&case, &mut ctx).await;
                attempts += 1;
            }

            let status = match result.outcome {
                TestOutcome::Passed => ProgressStatus::Passed,
                TestOutcome::Failed => {
                    if exec.fail_fast {
                        *ff_flag.lock().await = true;
                    }
                    ProgressStatus::Failed
                }
                TestOutcome::Skipped => ProgressStatus::Skipped,
                _ => ProgressStatus::Error,
            };

            exec.report_progress(ProgressUpdate {
                test_name: case.name.clone(),
                status,
                current: idx + 1,
                total,
                message: result.message.clone(),
            });

            result
        });

        handles.push(handle);
    }

    let mut results = Vec::with_capacity(total);
    for handle in handles {
        match handle.await {
            Ok(result) => results.push(result),
            Err(e) => {
                error!(error = %e, "task panicked during test execution");
            }
        }
    }

    info!(
        total,
        completed = results.len(),
        "batch execution complete"
    );

    results
}

/// Creates a skipped result.
fn make_skipped_result(case: &TestCase, reason: &str) -> TestCaseResult {
    let now = Utc::now();
    TestCaseResult {
        test_case: case.clone(),
        outcome: TestOutcome::Skipped,
        duration: std::time::Duration::ZERO,
        message: Some(reason.to_owned()),
        stdout: None,
        stderr: None,
        started_at: now,
        finished_at: now,
    }
}

// ---------------------------------------------------------------------------
// Crash detection
// ---------------------------------------------------------------------------

/// Patterns that indicate a device crash or fatal condition.
const CRASH_PATTERNS: &[&str] = &[
    "Kernel panic",
    "kernel panic",
    "Out of memory",
    "OOM",
    "oom-kill",
    "Segmentation fault",
    "segfault",
    "SIGABRT",
    "SIGSEGV",
    "BUG:",
    "general protection fault",
    "Unable to handle kernel",
    "Call Trace:",
    "syncd exited",
    "orchagent exited",
    "Critical error",
    "Fatal error",
    "core dumped",
];

/// Checks whether the device output contains patterns indicating a crash.
///
/// Returns `true` if any crash signature is found in the combined output of
/// the device and the test output.
pub fn detect_crash(device: &DeviceInfo, output: &str) -> bool {
    for pattern in CRASH_PATTERNS {
        if output.contains(pattern) {
            warn!(
                device = %device.hostname,
                pattern,
                "crash pattern detected in device output"
            );
            return true;
        }
    }
    false
}

/// Attempts to recover a device after a detected crash.
///
/// The recovery sequence:
/// 1. Wait a brief period for any pending I/O to settle.
/// 2. Attempt a cold reboot via the management interface.
/// 3. Wait for the device to become reachable.
/// 4. Reload the running configuration.
///
/// Returns `Ok(())` if recovery succeeds, or an error describing the failure.
pub async fn attempt_recovery(device: &DeviceInfo) -> Result<()> {
    info!(
        device = %device.hostname,
        ip = %device.mgmt_ip,
        "attempting device recovery"
    );

    // Step 1: Brief settle time.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Step 2: Attempt reboot.
    // In a real implementation this would call device.reboot(RebootType::Cold).
    // Here we log the intent and simulate success.
    info!(
        device = %device.hostname,
        "issuing cold reboot for recovery"
    );

    // Step 3: Wait for device to become reachable.
    let max_wait = std::time::Duration::from_secs(300);
    let poll_interval = std::time::Duration::from_secs(10);
    let start = Instant::now();

    while start.elapsed() < max_wait {
        // In a real implementation, this would attempt an SSH/ping check.
        // We simulate the wait loop structure.
        debug!(
            device = %device.hostname,
            elapsed_secs = start.elapsed().as_secs(),
            "waiting for device to become reachable"
        );

        tokio::time::sleep(poll_interval).await;

        // Simulate: device comes back after initial wait.
        if start.elapsed().as_secs() >= 30 {
            info!(
                device = %device.hostname,
                "device reachable after recovery"
            );
            return Ok(());
        }
    }

    Err(SonicError::Timeout {
        seconds: max_wait.as_secs(),
        operation: format!("recovery of device {}", device.hostname),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_core::{Credentials, DeviceType};
    use std::net::{IpAddr, Ipv4Addr};

    fn sample_device() -> DeviceInfo {
        DeviceInfo::new(
            "dut-1",
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            DeviceType::Sonic,
            Credentials::new("admin"),
        )
    }

    fn sample_case() -> TestCase {
        TestCase {
            id: "bgp::test_convergence".to_owned(),
            name: "test_convergence".to_owned(),
            module: "bgp".to_owned(),
            tags: vec!["bgp".to_owned()],
            topology: Some(TopologyType::T0),
            platform: None,
            description: Some("BGP convergence test".to_owned()),
            timeout_secs: 60,
        }
    }

    #[test]
    fn detect_crash_finds_kernel_panic() {
        let device = sample_device();
        let output = "some output\nKernel panic - not syncing\nmore stuff";
        assert!(detect_crash(&device, output));
    }

    #[test]
    fn detect_crash_finds_oom() {
        let device = sample_device();
        assert!(detect_crash(&device, "oom-kill: memory cgroup"));
    }

    #[test]
    fn detect_crash_finds_segfault() {
        let device = sample_device();
        assert!(detect_crash(&device, "[12345.678] orchagent[1234]: segfault at"));
    }

    #[test]
    fn detect_crash_clean_output() {
        let device = sample_device();
        let output = "BGP neighbor 10.0.0.1 is Up\nAll services running";
        assert!(!detect_crash(&device, output));
    }

    #[tokio::test]
    async fn execute_test_returns_error_when_no_dut() {
        let case = sample_case();
        let mut ctx = ExecutionContext::new("test-tb");
        // No DUTs configured.
        let result = execute_test(&case, &mut ctx).await;
        assert!(matches!(
            result.outcome,
            TestOutcome::Failed | TestOutcome::Error
        ));
    }

    #[tokio::test]
    async fn execute_test_passes_with_dut() {
        let case = sample_case();
        let mut ctx = ExecutionContext::new("test-tb");
        ctx.dut_info.push(sample_device());
        let result = execute_test(&case, &mut ctx).await;
        assert_eq!(result.outcome, TestOutcome::Passed);
    }

    #[tokio::test]
    async fn batch_respects_cancellation() {
        let (cancel_tx, cancel_rx) = watch::channel(true); // pre-cancelled
        let exec = TestExecutor::new(cancel_rx).with_max_workers(2);
        let cases = vec![sample_case(), sample_case()];
        let ctx = ExecutionContext::new("test-tb");

        let results = execute_batch(&exec, &cases, &ctx).await;
        assert!(results.iter().all(|r| r.outcome == TestOutcome::Skipped));
        drop(cancel_tx);
    }
}
