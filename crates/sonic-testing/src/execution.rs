//! Test execution engine.
//!
//! Provides single-test and batch execution with parallel scheduling via
//! tokio, crash detection for common kernel/OOM/segfault patterns, and
//! device recovery helpers.
//!
//! Tests are dispatched at runtime through a [`TestRegistry`] that maps test
//! IDs to async function implementations, or through a [`ScriptTestRunner`]
//! that invokes external scripts (Python, shell) when no registered
//! implementation is found.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
// TestOutput
// ---------------------------------------------------------------------------

/// Output produced by a test function execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestOutput {
    /// Whether the test passed.
    pub passed: bool,
    /// Optional human-readable message describing the outcome.
    pub message: Option<String>,
    /// Captured standard output from the test.
    pub stdout: Option<String>,
    /// Captured standard error from the test.
    pub stderr: Option<String>,
}

impl TestOutput {
    /// Creates a passing test output with no captured I/O.
    pub fn pass() -> Self {
        Self {
            passed: true,
            message: None,
            stdout: None,
            stderr: None,
        }
    }

    /// Creates a failing test output with the given message.
    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            passed: false,
            message: Some(message.into()),
            stdout: None,
            stderr: None,
        }
    }
}

// ---------------------------------------------------------------------------
// TestFn / TestRegistry
// ---------------------------------------------------------------------------

/// An async test function that receives the execution context and returns a
/// [`TestOutput`] describing the result.
pub type TestFn = Arc<
    dyn Fn(&ExecutionContext) -> Pin<Box<dyn Future<Output = Result<TestOutput>> + Send>>
        + Send
        + Sync,
>;

/// Registry mapping test IDs (`module::name`) to their implementation functions.
///
/// The executor consults this registry at dispatch time. If a test ID is not
/// found here, the executor falls back to searching for an external script.
pub struct TestRegistry {
    tests: HashMap<String, TestFn>,
}

impl std::fmt::Debug for TestRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestRegistry")
            .field("count", &self.tests.len())
            .finish()
    }
}

impl TestRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            tests: HashMap::new(),
        }
    }

    /// Registers an async test function under the given ID.
    pub fn register(&mut self, id: &str, f: TestFn) {
        self.tests.insert(id.to_owned(), f);
    }

    /// Convenience method for registering a synchronous test function.
    ///
    /// The function is wrapped in an async adapter so it can be stored
    /// alongside async test functions in the same registry.
    pub fn register_sync<F>(&mut self, id: &str, f: F)
    where
        F: Fn(&ExecutionContext) -> Result<TestOutput> + Send + Sync + 'static,
    {
        let f = Arc::new(f);
        self.tests.insert(
            id.to_owned(),
            Arc::new(move |ctx: &ExecutionContext| {
                let result = f(ctx);
                Box::pin(async move { result })
                    as Pin<Box<dyn Future<Output = Result<TestOutput>> + Send>>
            }),
        );
    }

    /// Looks up a test function by ID.
    pub fn get(&self, id: &str) -> Option<&TestFn> {
        self.tests.get(id)
    }

    /// Returns the number of registered tests.
    pub fn len(&self) -> usize {
        self.tests.len()
    }

    /// Returns true if no tests are registered.
    pub fn is_empty(&self) -> bool {
        self.tests.is_empty()
    }

    /// Returns an iterator over all registered test IDs.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.tests.keys().map(String::as_str)
    }

    /// Returns true if the registry contains a test with the given ID.
    pub fn contains(&self, id: &str) -> bool {
        self.tests.contains_key(id)
    }
}

impl Default for TestRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ScriptTestRunner
// ---------------------------------------------------------------------------

/// Runs test cases implemented as external scripts or commands.
///
/// When a test is defined in TOML but has no registered Rust implementation,
/// the executor delegates to this runner which invokes the script as a child
/// process, captures its output, and maps the exit code to pass/fail.
pub struct ScriptTestRunner {
    /// Working directory for script execution.
    work_dir: PathBuf,
    /// Environment variables passed to the script process.
    env: HashMap<String, String>,
}

impl ScriptTestRunner {
    /// Creates a new script runner with the given working directory.
    pub fn new(work_dir: PathBuf) -> Self {
        Self {
            work_dir,
            env: HashMap::new(),
        }
    }

    /// Adds an environment variable to be passed to scripts.
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_owned(), value.to_owned());
        self
    }

    /// Runs a script at `script_path` with the given arguments and timeout.
    ///
    /// Exit code 0 is mapped to a passing result. Any other exit code or a
    /// timeout produces a failure. Stdout and stderr are always captured.
    pub async fn run_script(
        &self,
        script_path: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<TestOutput> {
        info!(
            script = %script_path.display(),
            work_dir = %self.work_dir.display(),
            "running test script"
        );

        let mut cmd = tokio::process::Command::new(script_path);
        cmd.args(args)
            .current_dir(&self.work_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (k, v) in &self.env {
            cmd.env(k, v);
        }

        let child = cmd.spawn().map_err(|e| {
            SonicError::Test(format!(
                "failed to spawn script `{}`: {e}",
                script_path.display()
            ))
        })?;

        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(SonicError::Test(format!(
                    "script `{}` I/O error: {e}",
                    script_path.display()
                )));
            }
            Err(_) => {
                return Ok(TestOutput {
                    passed: false,
                    message: Some(format!(
                        "script `{}` timed out after {}s",
                        script_path.display(),
                        timeout.as_secs()
                    )),
                    stdout: None,
                    stderr: None,
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let passed = output.status.success();

        Ok(TestOutput {
            passed,
            message: if passed {
                None
            } else {
                Some(format!(
                    "script exited with code {}",
                    output.status.code().unwrap_or(-1)
                ))
            },
            stdout: if stdout.is_empty() {
                None
            } else {
                Some(stdout)
            },
            stderr: if stderr.is_empty() {
                None
            } else {
                Some(stderr)
            },
        })
    }
}

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

    /// Registry of test function implementations.
    pub test_registry: Arc<TestRegistry>,

    /// Environment variables passed to tests.
    pub env_vars: HashMap<String, String>,

    /// Output directory for test artifacts.
    pub output_dir: PathBuf,
}

impl ExecutionContext {
    /// Creates a new execution context with an empty test registry.
    pub fn new(testbed_name: impl Into<String>) -> Self {
        let name = testbed_name.into();
        Self {
            testbed_name: name.clone(),
            dut_info: Vec::new(),
            topology: None,
            fixture_context: FixtureContext::new(name),
            fixture_registry: FixtureRegistry::new(),
            test_registry: Arc::new(TestRegistry::new()),
            env_vars: HashMap::new(),
            output_dir: PathBuf::from("output"),
        }
    }

    /// Sets the output directory.
    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = dir.into();
        self
    }

    /// Sets the test registry.
    pub fn with_registry(mut self, registry: Arc<TestRegistry>) -> Self {
        self.test_registry = registry;
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
    let timeout = Duration::from_secs(case.timeout_secs);
    let result = tokio::time::timeout(timeout, run_test_body(case, context)).await;

    let (outcome, message, stdout, stderr) = match result {
        Ok(Ok(output)) => {
            if output.passed {
                debug!(test = %case.name, "test passed");
                (TestOutcome::Passed, output.message, output.stdout, output.stderr)
            } else {
                let msg = output
                    .message
                    .unwrap_or_else(|| "test reported failure".to_owned());
                warn!(test = %case.name, error = %msg, "test failed");
                let finished_at = Utc::now();
                let duration = wall_start.elapsed();
                let _ = teardown_fixtures(&mut context.fixture_context, &fixtures);
                return TestCaseResult {
                    test_case: case.clone(),
                    outcome: TestOutcome::Failed,
                    duration,
                    message: Some(msg),
                    stdout: output.stdout,
                    stderr: output.stderr,
                    started_at,
                    finished_at,
                };
            }
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
        message,
        stdout,
        stderr,
        started_at,
        finished_at,
    }
}

/// Dispatches a test case to its implementation.
///
/// Resolution order:
/// 1. Look up the test by `case.id` in `context.test_registry`.
/// 2. Search for a script at `{output_dir}/../tests/{module}/test_{name}.py`
///    or `.sh`.
/// 3. Return an error if no implementation is found.
async fn run_test_body(case: &TestCase, context: &ExecutionContext) -> Result<TestOutput> {
    trace!(
        test = %case.name,
        testbed = %context.testbed_name,
        "running test body"
    );

    // Check that at least one DUT is configured.
    if context.dut_info.is_empty() {
        return Err(SonicError::Test(format!(
            "test `{}` requires at least one DUT but none configured",
            case.name
        )));
    }

    // 1. Try the registry.
    if let Some(test_fn) = context.test_registry.get(&case.id) {
        debug!(test = %case.id, "dispatching via test registry");
        return test_fn(context).await;
    }

    // 2. Try external script discovery.
    let tests_dir = context.output_dir.join("..").join("tests").join(&case.module);
    let script_base = format!("test_{}", case.name);

    for ext in &["py", "sh"] {
        let script_path = tests_dir.join(format!("{script_base}.{ext}"));
        if script_path.exists() {
            debug!(
                test = %case.id,
                script = %script_path.display(),
                "dispatching via script runner"
            );

            let dut_hostnames: Vec<&str> =
                context.dut_info.iter().map(|d| d.hostname.as_str()).collect();

            let mut runner = ScriptTestRunner::new(
                script_path.parent().unwrap_or(Path::new(".")).to_owned(),
            )
            .with_env("SONIC_TESTBED", &context.testbed_name)
            .with_env("SONIC_DUT_HOSTNAMES", &dut_hostnames.join(","))
            .with_env("SONIC_TEST_NAME", &case.name)
            .with_env("SONIC_TEST_MODULE", &case.module);

            if let Some(ref topo) = context.topology {
                runner = runner.with_env("SONIC_TOPOLOGY", &topo.to_string());
            }

            let timeout = Duration::from_secs(case.timeout_secs);
            return runner.run_script(&script_path, &[], timeout).await;
        }
    }

    // 3. No implementation found.
    Err(SonicError::Test(format!(
        "no implementation found for test `{}`; register it in the TestRegistry or provide a script",
        case.id
    )))
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
/// 2. Log the intent to reboot (actual SSH-based reboot requires
///    `sonic_device`, which cannot be imported here).
/// 3. Poll the device management port with TCP connect probes until it
///    becomes reachable or the timeout expires.
///
/// Returns `Ok(())` if the device becomes reachable, or an error describing
/// the failure.
pub async fn attempt_recovery(device: &DeviceInfo) -> Result<()> {
    info!(
        device = %device.hostname,
        ip = %device.mgmt_ip,
        "attempting device recovery"
    );

    // Step 1: Brief settle time.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Step 2: Log reboot intent. A full implementation would SSH in and run
    // `sudo reboot`, but the dependency graph does not allow importing
    // sonic_device from this crate.
    info!(
        device = %device.hostname,
        "issuing cold reboot for recovery"
    );

    // Step 3: Poll the management port with TCP connect probes.
    let max_wait = Duration::from_secs(300);
    let poll_interval = Duration::from_secs(10);
    let start = Instant::now();
    let addr = std::net::SocketAddr::new(device.mgmt_ip, device.port);

    while start.elapsed() < max_wait {
        debug!(
            device = %device.hostname,
            elapsed_secs = start.elapsed().as_secs(),
            %addr,
            "probing device management port"
        );

        match tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(addr),
        )
        .await
        {
            Ok(Ok(_)) => {
                info!(device = %device.hostname, "device reachable after recovery");
                return Ok(());
            }
            _ => {
                tokio::time::sleep(poll_interval).await;
                continue;
            }
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

    // -- crash detection tests (unchanged) --

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

    // -- execution tests --

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

        // Register a passing implementation for this test ID.
        let mut registry = TestRegistry::new();
        registry.register_sync("bgp::test_convergence", |_ctx| {
            Ok(TestOutput::pass())
        });
        ctx.test_registry = Arc::new(registry);

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

    // -- TestRegistry tests --

    #[test]
    fn registry_register_and_lookup() {
        let mut registry = TestRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        registry.register_sync("mod::test_a", |_ctx| Ok(TestOutput::pass()));
        registry.register_sync("mod::test_b", |_ctx| {
            Ok(TestOutput::fail("oops"))
        });

        assert_eq!(registry.len(), 2);
        assert!(registry.contains("mod::test_a"));
        assert!(registry.contains("mod::test_b"));
        assert!(!registry.contains("mod::test_c"));

        let ids: Vec<&str> = registry.ids().collect();
        assert!(ids.contains(&"mod::test_a"));
        assert!(ids.contains(&"mod::test_b"));
    }

    // -- ScriptTestRunner tests --

    #[tokio::test]
    async fn script_runner_executes_echo_script() {
        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("test_echo.sh");
        std::fs::write(
            &script_path,
            "#!/bin/sh\necho hello from script\nexit 0\n",
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &script_path,
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        let runner = ScriptTestRunner::new(dir.path().to_owned());
        let output = runner
            .run_script(&script_path, &[], Duration::from_secs(10))
            .await
            .unwrap();

        assert!(output.passed);
        assert!(output.stdout.as_deref().unwrap().contains("hello from script"));
    }

    #[tokio::test]
    async fn script_runner_captures_failure() {
        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("test_fail.sh");
        std::fs::write(
            &script_path,
            "#!/bin/sh\necho bad >&2\nexit 1\n",
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &script_path,
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        let runner = ScriptTestRunner::new(dir.path().to_owned());
        let output = runner
            .run_script(&script_path, &[], Duration::from_secs(10))
            .await
            .unwrap();

        assert!(!output.passed);
        assert!(output.stderr.as_deref().unwrap().contains("bad"));
    }

    // -- run_test_body dispatch tests --

    #[tokio::test]
    async fn run_test_body_errors_when_no_implementation() {
        let case = sample_case();
        let mut ctx = ExecutionContext::new("test-tb");
        ctx.dut_info.push(sample_device());

        let result = run_test_body(&case, &ctx).await;
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no implementation found"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn execute_test_propagates_stdout_stderr() {
        let case = sample_case();
        let mut ctx = ExecutionContext::new("test-tb");
        ctx.dut_info.push(sample_device());

        let mut registry = TestRegistry::new();
        registry.register_sync("bgp::test_convergence", |_ctx| {
            Ok(TestOutput {
                passed: true,
                message: None,
                stdout: Some("collected output".to_owned()),
                stderr: Some("debug info".to_owned()),
            })
        });
        ctx.test_registry = Arc::new(registry);

        let result = execute_test(&case, &mut ctx).await;
        assert_eq!(result.outcome, TestOutcome::Passed);
        assert_eq!(result.stdout.as_deref(), Some("collected output"));
        assert_eq!(result.stderr.as_deref(), Some("debug info"));
    }
}
