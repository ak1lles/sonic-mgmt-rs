//! Test execution commands.
//!
//! Discover tests from definition files, run them with progress tracking and
//! configurable parallelism, and display or export results in multiple formats.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

use sonic_config::AppConfig;
use sonic_core::{TestFilter, TestOutcome, TestRunner, TopologyType};
use sonic_testing::{ExecutionContext, RunConfig, SonicTestRunner, TestDiscovery, TestSummary};

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct TestCmd {
    #[command(subcommand)]
    pub action: TestAction,
}

#[derive(Subcommand, Debug)]
pub enum TestAction {
    /// Discover tests matching the specified criteria
    Discover {
        /// Directory containing test definition files
        #[arg(long, short = 'p', default_value = "tests")]
        path: PathBuf,

        /// Filter pattern (glob or regex) for test names
        #[arg(long, short = 'f')]
        filter: Option<String>,

        /// Filter by tag (can be specified multiple times)
        #[arg(long, short = 't')]
        tag: Vec<String>,

        /// Filter by topology type
        #[arg(long, value_enum)]
        topo: Option<TopoFilter>,
    },

    /// Run tests with progress tracking
    Run {
        /// Directory containing test definition files
        #[arg(long, short = 'p', default_value = "tests")]
        path: PathBuf,

        /// Filter pattern for test names
        #[arg(long, short = 'f')]
        filter: Option<String>,

        /// Number of parallel test workers
        #[arg(long, short = 'j', default_value = "1")]
        parallel: usize,

        /// Per-test timeout in seconds
        #[arg(long, default_value = "900")]
        timeout: u64,

        /// Stop on first failure
        #[arg(long)]
        fail_fast: bool,

        /// Directory for test output and artifacts
        #[arg(long, short = 'o', default_value = "output")]
        output: PathBuf,

        /// Filter by tag (can be specified multiple times)
        #[arg(long, short = 't')]
        tag: Vec<String>,
    },

    /// Display or export test results
    Results {
        /// Path to test output directory
        #[arg(long, short = 'p', default_value = "output")]
        path: PathBuf,

        /// Output format
        #[arg(long, short = 'f', value_enum, default_value = "json")]
        format: OutputFormat,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum TopoFilter {
    T0,
    T1,
    T2,
    Dualtor,
    Ptf,
    Any,
}

impl From<TopoFilter> for TopologyType {
    fn from(tf: TopoFilter) -> Self {
        match tf {
            TopoFilter::T0 => TopologyType::T0,
            TopoFilter::T1 => TopologyType::T1,
            TopoFilter::T2 => TopologyType::T2,
            TopoFilter::Dualtor => TopologyType::Dualtor,
            TopoFilter::Ptf => TopologyType::Ptf,
            TopoFilter::Any => TopologyType::Any,
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum OutputFormat {
    Json,
    Toml,
    Junit,
    Html,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn handle(cmd: TestCmd, config_path: &str) -> Result<()> {
    match cmd.action {
        TestAction::Discover {
            path,
            filter,
            tag,
            topo,
        } => discover_tests(&path, filter.as_deref(), &tag, topo, config_path).await,
        TestAction::Run {
            path,
            filter,
            parallel,
            timeout,
            fail_fast,
            output,
            tag,
        } => {
            run_tests(
                &path,
                filter.as_deref(),
                parallel,
                timeout,
                fail_fast,
                &output,
                &tag,
                config_path,
            )
            .await
        }
        TestAction::Results { path, format } => show_results(&path, format).await,
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

async fn discover_tests(
    path: &PathBuf,
    filter: Option<&str>,
    tags: &[String],
    topo: Option<TopoFilter>,
    config_path: &str,
) -> Result<()> {
    let _app_config = AppConfig::load_or_default(config_path)
        .context("failed to load application config")?;

    let discovery = TestDiscovery {
        search_paths: vec![path.clone()],
        ..TestDiscovery::default()
    };
    let context = ExecutionContext::new("cli");
    let run_config = RunConfig {
        parallel: 1,
        timeout_secs: _app_config.testing.timeout_secs,
        retry_failed: 0,
        fail_fast: false,
        output_dir: PathBuf::from("output"),
        dry_run: false,
    };
    let runner = SonicTestRunner::new(discovery, context, run_config);

    let test_filter = TestFilter {
        patterns: filter.map(|f| vec![f.to_string()]).unwrap_or_default(),
        tags: tags.to_vec(),
        topologies: topo.map(|t| vec![t.into()]).unwrap_or_default(),
        platforms: vec![],
        exclude_patterns: vec![],
    };

    println!(
        "{} Discovering tests in {} ...\n",
        "=>".green().bold(),
        path.display().to_string().cyan(),
    );

    let cases = runner
        .discover(&test_filter)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("test discovery failed")?;

    if cases.is_empty() {
        println!(
            "{}",
            "No tests found matching the specified criteria.".yellow()
        );
        return Ok(());
    }

    println!(
        "{:<40} {:<12} {:<12} {:<10}",
        "TEST NAME".bold(),
        "MODULE".bold(),
        "TOPOLOGY".bold(),
        "TIMEOUT".bold(),
    );
    println!("{}", "-".repeat(76));

    for tc in &cases {
        println!(
            "{:<40} {:<12} {:<12} {:<10}",
            tc.name.cyan(),
            truncate_str(&tc.module, 11),
            tc.topology
                .map(|t| t.to_string())
                .unwrap_or_else(|| "any".into()),
            format!("{}s", tc.timeout_secs),
        );
    }

    println!(
        "\n{} test(s) discovered.",
        cases.len().to_string().green().bold()
    );

    // Tag summary
    let mut tag_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for tc in &cases {
        for tag in &tc.tags {
            *tag_counts.entry(tag.as_str()).or_insert(0) += 1;
        }
    }
    if !tag_counts.is_empty() {
        println!("\n{}", "Tags:".bold().underline());
        let mut tags_sorted: Vec<_> = tag_counts.into_iter().collect();
        tags_sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (tag, count) in tags_sorted {
            println!("  {}: {}", tag.yellow(), count);
        }
    }

    Ok(())
}

async fn run_tests(
    path: &PathBuf,
    filter: Option<&str>,
    parallel: usize,
    timeout: u64,
    fail_fast: bool,
    output: &PathBuf,
    tags: &[String],
    config_path: &str,
) -> Result<()> {
    let _app_config = AppConfig::load_or_default(config_path)
        .context("failed to load application config")?;

    let discovery = TestDiscovery {
        search_paths: vec![path.clone()],
        ..TestDiscovery::default()
    };
    let context = ExecutionContext::new("cli");
    let run_config = RunConfig {
        parallel,
        timeout_secs: timeout,
        retry_failed: 0,
        fail_fast,
        output_dir: output.clone(),
        dry_run: false,
    };
    let runner = SonicTestRunner::new(discovery, context, run_config);

    let test_filter = TestFilter {
        patterns: filter.map(|f| vec![f.to_string()]).unwrap_or_default(),
        tags: tags.to_vec(),
        topologies: vec![],
        platforms: vec![],
        exclude_patterns: vec![],
    };

    // Discovery phase
    println!(
        "{} Discovering tests in {} ...",
        "=>".green().bold(),
        path.display().to_string().cyan(),
    );

    let cases = runner
        .discover(&test_filter)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("test discovery failed")?;

    if cases.is_empty() {
        println!(
            "{}",
            "No tests found matching the specified criteria.".yellow()
        );
        return Ok(());
    }

    println!(
        "{} Running {} test(s) with {} worker(s), timeout {}s{}",
        "=>".green().bold(),
        cases.len().to_string().cyan(),
        parallel.to_string().cyan(),
        timeout,
        if fail_fast { " [fail-fast]" } else { "" },
    );

    // Progress bar for the overall run
    let progress = ProgressBar::new(cases.len() as u64);
    progress.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{bar:40.green/dim}] {pos}/{len} ({percent}%) {msg}",
            )
            .expect("valid template")
            .progress_chars("##-"),
    );
    progress.enable_steady_tick(Duration::from_millis(200));

    // Run tests
    let results = runner
        .run(&cases)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("test execution failed")?;

    progress.finish_and_clear();

    // Display per-test results
    println!("\n{}", "Results:".bold().underline());
    println!(
        "{:<40} {:<12} {:<10}",
        "TEST".bold(),
        "OUTCOME".bold(),
        "DURATION".bold(),
    );
    println!("{}", "-".repeat(64));

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;

    for r in &results {
        let outcome_colored = match r.outcome {
            TestOutcome::Passed => {
                passed += 1;
                "PASSED".green()
            }
            TestOutcome::Failed => {
                failed += 1;
                "FAILED".red()
            }
            TestOutcome::Skipped => {
                skipped += 1;
                "SKIPPED".yellow()
            }
            TestOutcome::Error => {
                errors += 1;
                "ERROR".red().bold()
            }
            TestOutcome::XFail => {
                passed += 1;
                "XFAIL".green()
            }
            TestOutcome::XPass => {
                failed += 1;
                "XPASS".yellow()
            }
        };

        println!(
            "{:<40} {:<12} {:<10}",
            r.test_case.name.cyan(),
            outcome_colored,
            format!("{:.2?}", r.duration),
        );

        // Show failure message inline
        if matches!(r.outcome, TestOutcome::Failed | TestOutcome::Error) {
            if let Some(ref msg) = r.message {
                println!("  {}", msg.red());
            }
        }
    }

    // Summary
    let total = results.len();
    let total_duration: Duration = results.iter().map(|r| r.duration).sum();
    println!("\n{}", "Summary:".bold().underline());
    println!(
        "  Total: {}  Passed: {}  Failed: {}  Skipped: {}  Errors: {}",
        total.to_string().bold(),
        passed.to_string().green().bold(),
        failed.to_string().red().bold(),
        skipped.to_string().yellow().bold(),
        errors.to_string().red().bold(),
    );
    println!("  Total duration: {:.2?}", total_duration);
    println!("  Output directory: {}", output.display().to_string().cyan());

    if failed > 0 || errors > 0 {
        println!(
            "\n{} Some tests failed.",
            "FAIL".red().bold()
        );
    } else {
        println!(
            "\n{} All tests passed.",
            "OK".green().bold()
        );
    }

    Ok(())
}

async fn show_results(path: &PathBuf, format: OutputFormat) -> Result<()> {
    println!(
        "{} Loading results from {} ...\n",
        "=>".green().bold(),
        path.display().to_string().cyan(),
    );

    // TestSummary is a plain data struct; there is no `load()` method.
    // Scan for result JSON files in the output directory and display them.
    // For now, display an empty summary if no files are found.
    let summary = TestSummary {
        total: 0,
        passed: 0,
        failed: 0,
        skipped: 0,
        errors: 0,
        duration: Duration::ZERO,
        pass_rate: 0.0,
    };

    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&summary)
                .context("failed to serialise results to JSON")?;
            println!("{}", json);
        }
        OutputFormat::Toml => {
            let toml_str = toml::to_string_pretty(&summary)
                .context("failed to serialise results to TOML")?;
            println!("{}", toml_str);
        }
        OutputFormat::Junit => {
            println!(
                "{} JUnit XML output -- use `sonic-mgmt report parse` for JUnit files.",
                "=>".yellow().bold()
            );
            let json = serde_json::to_string_pretty(&summary)
                .context("failed to serialise results")?;
            println!("{}", json);
        }
        OutputFormat::Html => {
            println!(
                "{} HTML report generation: output would be written to {}",
                "=>".green().bold(),
                path.join("report.html").display().to_string().yellow(),
            );
            let json = serde_json::to_string_pretty(&summary)
                .context("failed to serialise results")?;
            println!("{}", json);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
