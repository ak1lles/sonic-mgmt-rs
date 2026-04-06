//! Test result aggregation and multi-format output.
//!
//! Provides [`TestSuite`] for collecting results, [`TestSummary`] for quick
//! statistics, and formatters for JUnit XML, JSON, TOML, CSV, and HTML output.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use sonic_core::{ReportFormat, Result, TestCaseResult, TestOutcome};

// ---------------------------------------------------------------------------
// TestSuite
// ---------------------------------------------------------------------------

/// A collection of test results from a single run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSuite {
    /// Suite name (e.g. `"sonic-vms-t0"`).
    pub name: String,

    /// Individual test results.
    pub tests: Vec<TestCaseResult>,

    /// When the suite started.
    pub started_at: DateTime<Utc>,

    /// When the suite finished.
    pub finished_at: DateTime<Utc>,

    /// Arbitrary metadata (testbed, topology, pipeline ID, etc.).
    pub metadata: HashMap<String, String>,
}

impl TestSuite {
    /// Computes a summary of this suite's results.
    pub fn summary(&self) -> TestSummary {
        let total = self.tests.len();
        let passed = self.tests.iter().filter(|t| t.outcome == TestOutcome::Passed).count();
        let failed = self.tests.iter().filter(|t| t.outcome == TestOutcome::Failed).count();
        let skipped = self.tests.iter().filter(|t| t.outcome == TestOutcome::Skipped).count();
        let errors = self.tests.iter().filter(|t| t.outcome == TestOutcome::Error).count();

        let duration: Duration = self.tests.iter().map(|t| t.duration).sum();

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
            duration,
            pass_rate,
        }
    }

    /// Returns only the failed test results.
    pub fn failures(&self) -> Vec<&TestCaseResult> {
        self.tests
            .iter()
            .filter(|t| t.outcome == TestOutcome::Failed)
            .collect()
    }

    /// Returns only the error test results.
    pub fn errors(&self) -> Vec<&TestCaseResult> {
        self.tests
            .iter()
            .filter(|t| t.outcome == TestOutcome::Error)
            .collect()
    }

    /// Total wall-clock duration of the suite.
    pub fn wall_duration(&self) -> Duration {
        let diff = self.finished_at - self.started_at;
        diff.to_std().unwrap_or(Duration::ZERO)
    }
}

// ---------------------------------------------------------------------------
// TestSummary
// ---------------------------------------------------------------------------

/// Aggregate statistics for a set of test results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: usize,
    pub duration: Duration,
    pub pass_rate: f64,
}

// ---------------------------------------------------------------------------
// Formatters
// ---------------------------------------------------------------------------

/// Formats a test suite in the requested report format.
pub fn format_results(suite: &TestSuite, format: ReportFormat) -> Result<String> {
    match format {
        ReportFormat::JunitXml => format_junit_xml(suite),
        ReportFormat::Json => format_json(suite),
        ReportFormat::Toml => format_toml(suite),
        ReportFormat::Csv => format_csv(suite),
        ReportFormat::Html => format_html(suite),
    }
}

/// Writes formatted results to a file.
pub fn write_results(suite: &TestSuite, path: &Path, format: ReportFormat) -> Result<()> {
    let content = format_results(suite, format)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, &content)?;
    info!(
        path = %path.display(),
        format = ?format,
        tests = suite.tests.len(),
        "wrote test results"
    );

    Ok(())
}

/// Produces a colored summary string suitable for terminal output.
pub fn print_summary(summary: &TestSummary) -> String {
    let status_line = if summary.failed > 0 || summary.errors > 0 {
        "FAILED"
    } else if summary.total == 0 {
        "NO TESTS"
    } else {
        "PASSED"
    };

    format!(
        "\n\
         ======================== {} ========================\n\
         Total:     {total}\n\
         Passed:    {passed}\n\
         Failed:    {failed}\n\
         Skipped:   {skipped}\n\
         Errors:    {errors}\n\
         Duration:  {duration:.2}s\n\
         Pass Rate: {rate:.1}%\n\
         ===========================================================\n",
        status_line,
        total = summary.total,
        passed = summary.passed,
        failed = summary.failed,
        skipped = summary.skipped,
        errors = summary.errors,
        duration = summary.duration.as_secs_f64(),
        rate = summary.pass_rate,
    )
}

// ---------------------------------------------------------------------------
// JUnit XML formatter
// ---------------------------------------------------------------------------

fn format_junit_xml(suite: &TestSuite) -> Result<String> {
    let summary = suite.summary();
    let time = suite.wall_duration().as_secs_f64();

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{}\" errors=\"{}\" skipped=\"{}\" time=\"{:.3}\">\n",
        summary.total, summary.failed, summary.errors, summary.skipped, time,
    ));
    xml.push_str(&format!(
        "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"{}\" skipped=\"{}\" time=\"{:.3}\">\n",
        xml_escape(&suite.name),
        summary.total,
        summary.failed,
        summary.errors,
        summary.skipped,
        time,
    ));

    for result in &suite.tests {
        let classname = xml_escape(&result.test_case.module);
        let name = xml_escape(&result.test_case.name);
        let tc_time = result.duration.as_secs_f64();

        xml.push_str(&format!(
            "    <testcase classname=\"{classname}\" name=\"{name}\" time=\"{tc_time:.3}\"",
        ));

        match result.outcome {
            TestOutcome::Passed | TestOutcome::XPass => {
                xml.push_str(" />\n");
            }
            TestOutcome::Failed | TestOutcome::XFail => {
                xml.push_str(">\n");
                let msg = result.message.as_deref().unwrap_or("test failed");
                xml.push_str(&format!(
                    "      <failure message=\"{}\" type=\"AssertionError\">{}</failure>\n",
                    xml_escape(msg),
                    xml_escape(msg),
                ));
                if let Some(ref stdout) = result.stdout {
                    xml.push_str(&format!(
                        "      <system-out>{}</system-out>\n",
                        xml_escape(stdout)
                    ));
                }
                if let Some(ref stderr) = result.stderr {
                    xml.push_str(&format!(
                        "      <system-err>{}</system-err>\n",
                        xml_escape(stderr)
                    ));
                }
                xml.push_str("    </testcase>\n");
            }
            TestOutcome::Skipped => {
                xml.push_str(">\n");
                let msg = result.message.as_deref().unwrap_or("skipped");
                xml.push_str(&format!(
                    "      <skipped message=\"{}\" />\n",
                    xml_escape(msg)
                ));
                xml.push_str("    </testcase>\n");
            }
            TestOutcome::Error => {
                xml.push_str(">\n");
                let msg = result.message.as_deref().unwrap_or("error");
                xml.push_str(&format!(
                    "      <error message=\"{}\" type=\"Error\">{}</error>\n",
                    xml_escape(msg),
                    xml_escape(msg),
                ));
                xml.push_str("    </testcase>\n");
            }
        }
    }

    xml.push_str("  </testsuite>\n");
    xml.push_str("</testsuites>\n");

    debug!(len = xml.len(), "formatted JUnit XML");
    Ok(xml)
}

/// Escapes XML special characters.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// JSON formatter
// ---------------------------------------------------------------------------

fn format_json(suite: &TestSuite) -> Result<String> {
    let json = serde_json::to_string_pretty(suite)?;
    debug!(len = json.len(), "formatted JSON");
    Ok(json)
}

// ---------------------------------------------------------------------------
// TOML formatter
// ---------------------------------------------------------------------------

fn format_toml(suite: &TestSuite) -> Result<String> {
    let toml_str = toml::to_string_pretty(suite)?;
    debug!(len = toml_str.len(), "formatted TOML");
    Ok(toml_str)
}

// ---------------------------------------------------------------------------
// CSV formatter
// ---------------------------------------------------------------------------

fn format_csv(suite: &TestSuite) -> Result<String> {
    let mut csv = String::new();
    csv.push_str("id,name,module,outcome,duration_secs,message\n");

    for result in &suite.tests {
        let msg = result
            .message
            .as_deref()
            .unwrap_or("")
            .replace('"', "\"\"");
        csv.push_str(&format!(
            "\"{}\",\"{}\",\"{}\",\"{}\",{:.3},\"{}\"\n",
            csv_escape(&result.test_case.id),
            csv_escape(&result.test_case.name),
            csv_escape(&result.test_case.module),
            result.outcome,
            result.duration.as_secs_f64(),
            msg,
        ));
    }

    debug!(len = csv.len(), rows = suite.tests.len(), "formatted CSV");
    Ok(csv)
}

/// Escapes double quotes in CSV fields.
fn csv_escape(s: &str) -> String {
    s.replace('"', "\"\"")
}

// ---------------------------------------------------------------------------
// HTML formatter
// ---------------------------------------------------------------------------

fn format_html(suite: &TestSuite) -> Result<String> {
    let summary = suite.summary();
    let mut html = String::new();

    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("  <meta charset=\"UTF-8\">\n");
    html.push_str(&format!(
        "  <title>Test Report: {}</title>\n",
        html_escape(&suite.name)
    ));
    html.push_str("  <style>\n");
    html.push_str(
        "    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; margin: 2em; }\n",
    );
    html.push_str(
        "    table { border-collapse: collapse; width: 100%; margin-top: 1em; }\n",
    );
    html.push_str(
        "    th, td { border: 1px solid #ddd; padding: 8px; text-align: left; }\n",
    );
    html.push_str("    th { background: #f5f5f5; }\n");
    html.push_str("    .passed { color: #22863a; }\n");
    html.push_str("    .failed { color: #cb2431; }\n");
    html.push_str("    .skipped { color: #6a737d; }\n");
    html.push_str("    .error { color: #d73a49; font-weight: bold; }\n");
    html.push_str(
        "    .summary { display: flex; gap: 2em; margin: 1em 0; }\n",
    );
    html.push_str(
        "    .summary div { padding: 1em; border-radius: 4px; background: #f6f8fa; }\n",
    );
    html.push_str("  </style>\n</head>\n<body>\n");

    html.push_str(&format!(
        "<h1>Test Report: {}</h1>\n",
        html_escape(&suite.name)
    ));

    // Summary section.
    html.push_str("<div class=\"summary\">\n");
    html.push_str(&format!("  <div><strong>Total:</strong> {}</div>\n", summary.total));
    html.push_str(&format!(
        "  <div class=\"passed\"><strong>Passed:</strong> {}</div>\n",
        summary.passed
    ));
    html.push_str(&format!(
        "  <div class=\"failed\"><strong>Failed:</strong> {}</div>\n",
        summary.failed
    ));
    html.push_str(&format!(
        "  <div class=\"skipped\"><strong>Skipped:</strong> {}</div>\n",
        summary.skipped
    ));
    html.push_str(&format!(
        "  <div class=\"error\"><strong>Errors:</strong> {}</div>\n",
        summary.errors
    ));
    html.push_str(&format!(
        "  <div><strong>Pass Rate:</strong> {:.1}%</div>\n",
        summary.pass_rate
    ));
    html.push_str(&format!(
        "  <div><strong>Duration:</strong> {:.2}s</div>\n",
        summary.duration.as_secs_f64()
    ));
    html.push_str("</div>\n");

    // Metadata.
    if !suite.metadata.is_empty() {
        html.push_str("<h2>Metadata</h2>\n<ul>\n");
        let mut keys: Vec<_> = suite.metadata.keys().collect();
        keys.sort();
        for key in keys {
            html.push_str(&format!(
                "  <li><strong>{}:</strong> {}</li>\n",
                html_escape(key),
                html_escape(&suite.metadata[key]),
            ));
        }
        html.push_str("</ul>\n");
    }

    // Results table.
    html.push_str("<h2>Results</h2>\n");
    html.push_str("<table>\n");
    html.push_str(
        "  <tr><th>Name</th><th>Module</th><th>Outcome</th><th>Duration</th><th>Message</th></tr>\n",
    );

    for result in &suite.tests {
        let class = match result.outcome {
            TestOutcome::Passed | TestOutcome::XPass => "passed",
            TestOutcome::Failed | TestOutcome::XFail => "failed",
            TestOutcome::Skipped => "skipped",
            TestOutcome::Error => "error",
        };
        let msg = result.message.as_deref().unwrap_or("");

        html.push_str(&format!(
            "  <tr><td>{}</td><td>{}</td><td class=\"{class}\">{}</td><td>{:.3}s</td><td>{}</td></tr>\n",
            html_escape(&result.test_case.name),
            html_escape(&result.test_case.module),
            result.outcome,
            result.duration.as_secs_f64(),
            html_escape(msg),
        ));
    }

    html.push_str("</table>\n");
    html.push_str(&format!(
        "<p><em>Generated at {}</em></p>\n",
        suite.finished_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    html.push_str("</body>\n</html>\n");

    debug!(len = html.len(), "formatted HTML");
    Ok(html)
}

/// Escapes HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_core::TestCase;

    fn sample_suite() -> TestSuite {
        let now = Utc::now();
        TestSuite {
            name: "test-suite".to_owned(),
            tests: vec![
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
                    duration: Duration::from_millis(1500),
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
                    duration: Duration::from_millis(3200),
                    message: Some("expected 42, got 0".into()),
                    stdout: Some("some output".into()),
                    stderr: Some("error detail".into()),
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
                    duration: Duration::ZERO,
                    message: Some("platform not supported".into()),
                    stdout: None,
                    stderr: None,
                    started_at: now,
                    finished_at: now,
                },
            ],
            started_at: now,
            finished_at: now,
            metadata: HashMap::from([
                ("testbed".into(), "vms-t0".into()),
            ]),
        }
    }

    #[test]
    fn summary_computes_correctly() {
        let suite = sample_suite();
        let summary = suite.summary();
        assert_eq!(summary.total, 3);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.errors, 0);
    }

    #[test]
    fn format_junit_xml_valid() {
        let suite = sample_suite();
        let xml = format_results(&suite, ReportFormat::JunitXml).unwrap();
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("testsuites"));
        assert!(xml.contains("test_pass"));
        assert!(xml.contains("test_fail"));
        assert!(xml.contains("<failure"));
        assert!(xml.contains("<skipped"));
    }

    #[test]
    fn format_json_valid() {
        let suite = sample_suite();
        let json = format_results(&suite, ReportFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["name"], "test-suite");
    }

    #[test]
    fn format_csv_has_header_and_rows() {
        let suite = sample_suite();
        let csv = format_results(&suite, ReportFormat::Csv).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "id,name,module,outcome,duration_secs,message");
        assert_eq!(lines.len(), 4); // header + 3 rows
    }

    #[test]
    fn format_html_contains_structure() {
        let suite = sample_suite();
        let html = format_results(&suite, ReportFormat::Html).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<table>"));
        assert!(html.contains("test_pass"));
        assert!(html.contains("test_fail"));
    }

    #[test]
    fn print_summary_output() {
        let summary = TestSummary {
            total: 10,
            passed: 8,
            failed: 1,
            skipped: 1,
            errors: 0,
            duration: Duration::from_secs(42),
            pass_rate: 80.0,
        };
        let text = print_summary(&summary);
        assert!(text.contains("PASSED"));
        assert!(text.contains("80.0%"));
    }

    #[test]
    fn print_summary_failed() {
        let summary = TestSummary {
            total: 10,
            passed: 7,
            failed: 3,
            skipped: 0,
            errors: 0,
            duration: Duration::from_secs(100),
            pass_rate: 70.0,
        };
        let text = print_summary(&summary);
        assert!(text.contains("FAILED"));
    }

    #[test]
    fn failures_and_errors_accessors() {
        let suite = sample_suite();
        assert_eq!(suite.failures().len(), 1);
        assert_eq!(suite.errors().len(), 0);
    }
}
