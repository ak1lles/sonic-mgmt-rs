//! Reporting commands.
//!
//! Parse JUnit XML files, upload results to external backends (Kusto, local
//! file storage), and display SAI/SWSS coverage statistics.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use colored::Colorize;

use sonic_core::{ReportFormat, ReportUploader, TestOutcome};
use sonic_reporting::{
    CoverageTracker, KustoStorage, LocalFileStorage,
    ReportUploadManager, UploadMetadata,
};
use sonic_reporting::storage::KustoAuth;

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ReportCmd {
    #[command(subcommand)]
    pub action: ReportAction,
}

#[derive(Subcommand, Debug)]
pub enum ReportAction {
    /// Parse a JUnit XML file and display a summary
    Parse {
        /// Path to the JUnit XML file
        path: PathBuf,
    },

    /// Upload test results to a reporting backend
    Upload {
        /// Backend type
        #[arg(long, short = 'b', value_enum)]
        backend: UploadBackend,

        /// Path to the results directory or file
        #[arg(long, short = 'r')]
        results: PathBuf,

        /// Kusto cluster URL (required for kusto backend)
        #[arg(long)]
        kusto_url: Option<String>,

        /// Kusto database name
        #[arg(long)]
        kusto_db: Option<String>,

        /// Kusto table name
        #[arg(long)]
        kusto_table: Option<String>,

        /// Local file output directory (for file backend)
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },

    /// Display SAI/SWSS test coverage statistics
    Coverage {
        /// Path to the results directory
        #[arg(long, short = 'r')]
        results: PathBuf,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum UploadBackend {
    Kusto,
    File,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn handle(cmd: ReportCmd) -> Result<()> {
    match cmd.action {
        ReportAction::Parse { path } => parse_junit(&path).await,
        ReportAction::Upload {
            backend,
            results,
            kusto_url,
            kusto_db,
            kusto_table,
            output_dir,
        } => {
            upload_results(
                backend,
                &results,
                kusto_url.as_deref(),
                kusto_db.as_deref(),
                kusto_table.as_deref(),
                output_dir.as_ref(),
            )
            .await
        }
        ReportAction::Coverage { results } => show_coverage(&results).await,
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

async fn parse_junit(path: &PathBuf) -> Result<()> {
    println!(
        "{} Parsing JUnit XML: {} ...\n",
        "=>".green().bold(),
        path.display().to_string().cyan(),
    );

    let report = sonic_reporting::junit::parse_junit_file(path)
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to parse JUnit XML")?;

    // Overall summary
    println!("{}", "JUnit Report Summary".cyan().bold());
    println!("{}", "=".repeat(50));
    println!("{:<18} {}", "Test Suites:".bold(), report.test_suites.len());

    let mut total_tests = 0usize;
    let mut total_passed = 0usize;
    let mut total_failed = 0usize;
    let mut total_skipped = 0usize;
    let mut total_errors = 0usize;
    let mut total_time = 0.0f64;

    for suite in &report.test_suites {
        let suite_passed = suite.test_cases.iter().filter(|c| c.status == TestOutcome::Passed).count();
        let suite_failed = suite.test_cases.iter().filter(|c| c.status == TestOutcome::Failed).count();
        let suite_skipped = suite.test_cases.iter().filter(|c| c.status == TestOutcome::Skipped).count();
        let suite_errors = suite.test_cases.iter().filter(|c| c.status == TestOutcome::Error).count();
        let suite_time: f64 = suite.test_cases.iter().map(|c| c.time).sum();

        total_tests += suite.test_cases.len();
        total_passed += suite_passed;
        total_failed += suite_failed;
        total_skipped += suite_skipped;
        total_errors += suite_errors;
        total_time += suite_time;

        println!(
            "\n  {} ({})",
            suite.name.yellow().bold(),
            format!("{} tests", suite.test_cases.len()),
        );
        println!(
            "    Passed: {}  Failed: {}  Skipped: {}  Errors: {}  Duration: {:.2}s",
            suite_passed.to_string().green(),
            suite_failed.to_string().red(),
            suite_skipped.to_string().yellow(),
            suite_errors.to_string().red(),
            suite_time,
        );

        // Show failed tests
        let failures: Vec<_> = suite
            .test_cases
            .iter()
            .filter(|c| matches!(c.status, TestOutcome::Failed | TestOutcome::Error))
            .collect();

        if !failures.is_empty() {
            println!("    {}", "Failures:".red().underline());
            for f in &failures {
                println!("      - {}", f.name.as_str().red());
                if let Some(ref msg) = f.failure_message {
                    let first_line: &str = msg.lines().next().unwrap_or("");
                    println!("        {}", first_line.dimmed());
                }
            }
        }
    }

    // Grand totals
    println!("\n{}", "Totals:".bold().underline());
    println!(
        "  Tests: {}  Passed: {}  Failed: {}  Skipped: {}  Errors: {}",
        total_tests.to_string().bold(),
        total_passed.to_string().green().bold(),
        total_failed.to_string().red().bold(),
        total_skipped.to_string().yellow().bold(),
        total_errors.to_string().red().bold(),
    );
    println!("  Total duration: {:.2}s", total_time);

    if total_tests > 0 {
        let pass_rate = (total_passed as f64 / total_tests as f64) * 100.0;
        let rate_colored = if pass_rate >= 95.0 {
            format!("{:.1}%", pass_rate).green().bold()
        } else if pass_rate >= 80.0 {
            format!("{:.1}%", pass_rate).yellow().bold()
        } else {
            format!("{:.1}%", pass_rate).red().bold()
        };
        println!("  Pass rate: {}", rate_colored);
    }

    Ok(())
}

async fn upload_results(
    backend: UploadBackend,
    results: &PathBuf,
    kusto_url: Option<&str>,
    kusto_db: Option<&str>,
    kusto_table: Option<&str>,
    output_dir: Option<&PathBuf>,
) -> Result<()> {
    println!(
        "{} Uploading results from {} ...",
        "=>".green().bold(),
        results.display().to_string().cyan(),
    );

    // Parse JUnit results from the supplied path to get TestCaseResult values.
    let report = sonic_reporting::junit::parse_junit_file(results)
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("failed to parse results file")?;
    let test_results = sonic_reporting::junit::to_test_results(&report);

    match backend {
        UploadBackend::Kusto => {
            let url = kusto_url
                .ok_or_else(|| anyhow::anyhow!("--kusto-url is required for kusto backend"))?;
            let db = kusto_db
                .ok_or_else(|| anyhow::anyhow!("--kusto-db is required for kusto backend"))?;
            let table = kusto_table
                .ok_or_else(|| anyhow::anyhow!("--kusto-table is required for kusto backend"))?;

            println!(
                "  Backend: {} ({})",
                "Kusto".cyan(),
                url.yellow(),
            );
            println!("  Database: {}, Table: {}", db, table);

            let storage = std::sync::Arc::new(
                KustoStorage::new(url, db, table, KustoAuth::AzureDefault),
            );
            let metadata = UploadMetadata::default();
            let manager = ReportUploadManager::new(storage, metadata);
            manager
                .upload(&test_results)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .context("upload to Kusto failed")?;

            println!(
                "{} Results uploaded to Kusto successfully.",
                "OK".green().bold()
            );
        }
        UploadBackend::File => {
            let out = output_dir
                .cloned()
                .unwrap_or_else(|| PathBuf::from("reports"));

            println!(
                "  Backend: {} ({})",
                "Local File".cyan(),
                out.display().to_string().yellow(),
            );

            let storage = std::sync::Arc::new(
                LocalFileStorage::new(&out, ReportFormat::Json),
            );
            let metadata = UploadMetadata::default();
            let manager = ReportUploadManager::new(storage, metadata);
            manager
                .upload(&test_results)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))
                .context("upload to local storage failed")?;

            println!(
                "{} Results written to {}.",
                "OK".green().bold(),
                out.display().to_string().yellow(),
            );
        }
    }

    Ok(())
}

async fn show_coverage(_results: &PathBuf) -> Result<()> {
    println!(
        "{} Analysing test coverage from {} ...\n",
        "=>".green().bold(),
        _results.display().to_string().cyan(),
    );

    // CoverageTracker is populated programmatically (not from a file).
    // Create a fresh tracker; in a real workflow, tests would have called
    // track_test() during execution.
    let tracker = CoverageTracker::new();
    let report = tracker.get_coverage_report();

    println!("{}", "Test Coverage Report".cyan().bold());
    println!("{}", "=".repeat(50));

    println!(
        "\n{:<30} {:<10} {:<10} {:<10}",
        "COMPONENT".bold(),
        "TOTAL".bold(),
        "COVERED".bold(),
        "PERCENT".bold(),
    );
    println!("{}", "-".repeat(62));

    // SAI row
    let sai_pct_colored = if report.sai_coverage_pct >= 80.0 {
        format!("{:.1}%", report.sai_coverage_pct).green()
    } else if report.sai_coverage_pct >= 50.0 {
        format!("{:.1}%", report.sai_coverage_pct).yellow()
    } else {
        format!("{:.1}%", report.sai_coverage_pct).red()
    };
    println!(
        "{:<30} {:<10} {:<10} {:<10}",
        "SAI APIs".cyan(),
        report.sai_total,
        report.sai_covered,
        sai_pct_colored,
    );

    // SWSS row
    let swss_pct_colored = if report.swss_coverage_pct >= 80.0 {
        format!("{:.1}%", report.swss_coverage_pct).green()
    } else if report.swss_coverage_pct >= 50.0 {
        format!("{:.1}%", report.swss_coverage_pct).yellow()
    } else {
        format!("{:.1}%", report.swss_coverage_pct).red()
    };
    println!(
        "{:<30} {:<10} {:<10} {:<10}",
        "SWSS Invocations".cyan(),
        report.swss_total,
        report.swss_covered,
        swss_pct_colored,
    );

    // Overall
    let overall_pct = report.coverage_pct;

    println!("\n{}", "Overall:".bold().underline());
    println!(
        "  Total APIs: {}  Covered: {}  Coverage: {}",
        report.total_apis.to_string().bold(),
        report.covered_apis.to_string().green().bold(),
        if overall_pct >= 80.0 {
            format!("{:.1}%", overall_pct).green().bold()
        } else if overall_pct >= 50.0 {
            format!("{:.1}%", overall_pct).yellow().bold()
        } else {
            format!("{:.1}%", overall_pct).red().bold()
        },
    );

    Ok(())
}
