//! JUnit XML parser and converter.
//!
//! Parses standard JUnit XML reports (as produced by pytest, Java test runners,
//! etc.) into structured Rust types, and provides conversion to JSON and to
//! the `sonic-core` [`TestCaseResult`] format.

use std::path::Path;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use sonic_core::{Result, SonicError, TestCase, TestCaseResult, TestOutcome};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Top-level JUnit report, corresponding to the `<testsuites>` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JunitReport {
    /// Report name.
    pub name: String,
    /// Total number of tests across all suites.
    pub tests: usize,
    /// Total failure count.
    pub failures: usize,
    /// Total error count.
    pub errors: usize,
    /// Total skipped count.
    pub skipped: usize,
    /// Total time in seconds.
    pub time: f64,
    /// Individual test suites.
    pub test_suites: Vec<JunitTestSuite>,
}

/// A single `<testsuite>` element.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JunitTestSuite {
    /// Suite name.
    pub name: String,
    /// Number of tests in this suite.
    pub tests: usize,
    /// Number of failures.
    pub failures: usize,
    /// Number of errors.
    pub errors: usize,
    /// Elapsed time in seconds.
    pub time: f64,
    /// Individual test cases.
    pub test_cases: Vec<JunitTestCase>,
}

/// A single `<testcase>` element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JunitTestCase {
    /// Test name.
    pub name: String,
    /// Classname / module.
    pub classname: String,
    /// Elapsed time in seconds.
    pub time: f64,
    /// Computed outcome.
    pub status: TestOutcome,
    /// Failure message (from `<failure>` element), if any.
    pub failure_message: Option<String>,
    /// Failure type attribute.
    pub failure_type: Option<String>,
    /// Content of `<system-out>`.
    pub system_out: Option<String>,
    /// Content of `<system-err>`.
    pub system_err: Option<String>,
}

impl Default for JunitTestCase {
    fn default() -> Self {
        Self {
            name: String::new(),
            classname: String::new(),
            time: 0.0,
            status: TestOutcome::Passed,
            failure_message: None,
            failure_type: None,
            system_out: None,
            system_err: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Tracks which child element we are inside for text collection.
#[derive(Clone, Copy, PartialEq)]
enum Inside {
    None,
    Failure,
    Error,
    SystemOut,
    SystemErr,
}

/// Processes the attributes of an opening (or self-closing) tag.
fn process_open(
    tag_name: &str,
    e: &BytesStart<'_>,
    report: &mut JunitReport,
    current_suite: &mut Option<JunitTestSuite>,
    current_case: &mut Option<JunitTestCase>,
    inside: &mut Inside,
    text_buf: &mut String,
) {
    match tag_name {
        "testsuites" => {
            report.name = attr_str(e, "name").unwrap_or_default();
            report.tests = attr_usize(e, "tests").unwrap_or(0);
            report.failures = attr_usize(e, "failures").unwrap_or(0);
            report.errors = attr_usize(e, "errors").unwrap_or(0);
            report.skipped = attr_usize(e, "skipped").unwrap_or(0);
            report.time = attr_f64(e, "time").unwrap_or(0.0);
        }
        "testsuite" => {
            let suite = JunitTestSuite {
                name: attr_str(e, "name").unwrap_or_default(),
                tests: attr_usize(e, "tests").unwrap_or(0),
                failures: attr_usize(e, "failures").unwrap_or(0),
                errors: attr_usize(e, "errors").unwrap_or(0),
                time: attr_f64(e, "time").unwrap_or(0.0),
                test_cases: Vec::new(),
            };
            *current_suite = Some(suite);
        }
        "testcase" => {
            let tc = JunitTestCase {
                name: attr_str(e, "name").unwrap_or_default(),
                classname: attr_str(e, "classname").unwrap_or_default(),
                time: attr_f64(e, "time").unwrap_or(0.0),
                status: TestOutcome::Passed,
                ..Default::default()
            };
            *current_case = Some(tc);
        }
        "failure" => {
            if let Some(ref mut tc) = current_case {
                tc.status = TestOutcome::Failed;
                tc.failure_message = attr_str(e, "message");
                tc.failure_type = attr_str(e, "type");
            }
            *inside = Inside::Failure;
            text_buf.clear();
        }
        "error" => {
            if let Some(ref mut tc) = current_case {
                tc.status = TestOutcome::Error;
                tc.failure_message = attr_str(e, "message");
                tc.failure_type = attr_str(e, "type");
            }
            *inside = Inside::Error;
            text_buf.clear();
        }
        "skipped" => {
            if let Some(ref mut tc) = current_case {
                tc.status = TestOutcome::Skipped;
                tc.failure_message = attr_str(e, "message");
            }
        }
        "system-out" => {
            *inside = Inside::SystemOut;
            text_buf.clear();
        }
        "system-err" => {
            *inside = Inside::SystemErr;
            text_buf.clear();
        }
        _ => {
            trace!(tag = %tag_name, "ignoring unknown XML element");
        }
    }
}

/// Processes a closing tag (or the close side of a self-closing tag).
fn process_close(
    tag_name: &str,
    report: &mut JunitReport,
    current_suite: &mut Option<JunitTestSuite>,
    current_case: &mut Option<JunitTestCase>,
    inside: &mut Inside,
    text_buf: &mut String,
) {
    match tag_name {
        "testcase" => {
            if let Some(tc) = current_case.take() {
                if let Some(ref mut suite) = current_suite {
                    suite.test_cases.push(tc);
                }
            }
        }
        "testsuite" => {
            if let Some(suite) = current_suite.take() {
                report.test_suites.push(suite);
            }
        }
        "failure" | "error" => {
            if let Some(ref mut tc) = current_case {
                if tc.failure_message.is_none() && !text_buf.is_empty() {
                    tc.failure_message = Some(text_buf.clone());
                }
            }
            *inside = Inside::None;
            text_buf.clear();
        }
        "skipped" => {
            // Nothing additional to do on close.
        }
        "system-out" => {
            if let Some(ref mut tc) = current_case {
                if !text_buf.is_empty() {
                    tc.system_out = Some(text_buf.clone());
                }
            }
            *inside = Inside::None;
            text_buf.clear();
        }
        "system-err" => {
            if let Some(ref mut tc) = current_case {
                if !text_buf.is_empty() {
                    tc.system_err = Some(text_buf.clone());
                }
            }
            *inside = Inside::None;
            text_buf.clear();
        }
        _ => {}
    }
}

/// Parses a JUnit XML string into a [`JunitReport`].
pub fn parse_junit_xml(xml_content: &str) -> Result<JunitReport> {
    let mut reader = Reader::from_str(xml_content);
    reader.config_mut().trim_text(true);

    let mut report = JunitReport::default();
    let mut current_suite: Option<JunitTestSuite> = None;
    let mut current_case: Option<JunitTestCase> = None;
    let mut inside = Inside::None;
    let mut text_buf = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                process_open(
                    &tag, e,
                    &mut report, &mut current_suite, &mut current_case,
                    &mut inside, &mut text_buf,
                );
            }
            Ok(Event::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                process_open(
                    &tag, e,
                    &mut report, &mut current_suite, &mut current_case,
                    &mut inside, &mut text_buf,
                );
                // Self-closing tags need immediate close processing.
                process_close(
                    &tag,
                    &mut report, &mut current_suite, &mut current_case,
                    &mut inside, &mut text_buf,
                );
            }
            Ok(Event::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                process_close(
                    &tag,
                    &mut report, &mut current_suite, &mut current_case,
                    &mut inside, &mut text_buf,
                );
            }
            Ok(Event::Text(ref e)) => {
                if inside != Inside::None {
                    if let Ok(text) = e.unescape() {
                        text_buf.push_str(&text);
                    }
                }
            }
            Ok(Event::CData(ref e)) => {
                if inside != Inside::None {
                    if let Ok(text) = std::str::from_utf8(e.as_ref()) {
                        text_buf.push_str(text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(SonicError::ReportParse(format!(
                    "XML parse error at position {}: {e}",
                    reader.error_position()
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    // If no <testsuites> wrapper was present (single <testsuite> at root),
    // recompute aggregate counts.
    if report.tests == 0 && !report.test_suites.is_empty() {
        report.tests = report.test_suites.iter().map(|s| s.tests).sum();
        report.failures = report.test_suites.iter().map(|s| s.failures).sum();
        report.errors = report.test_suites.iter().map(|s| s.errors).sum();
        report.time = report
            .test_suites
            .iter()
            .map(|s| s.time)
            .fold(0.0, f64::max);
        report.skipped = report
            .test_suites
            .iter()
            .flat_map(|s| &s.test_cases)
            .filter(|tc| tc.status == TestOutcome::Skipped)
            .count();
    }

    debug!(
        suites = report.test_suites.len(),
        tests = report.tests,
        failures = report.failures,
        "parsed JUnit XML report"
    );

    Ok(report)
}

/// Parses a JUnit XML file.
pub fn parse_junit_file(path: &Path) -> Result<JunitReport> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        SonicError::ReportParse(format!(
            "failed to read JUnit file {}: {e}",
            path.display()
        ))
    })?;
    parse_junit_xml(&content)
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// Converts a [`JunitReport`] to a `serde_json::Value`.
pub fn to_json(report: &JunitReport) -> serde_json::Value {
    serde_json::to_value(report).unwrap_or(serde_json::Value::Null)
}

/// Converts a [`JunitReport`] to a list of sonic-core [`TestCaseResult`] values.
pub fn to_test_results(report: &JunitReport) -> Vec<TestCaseResult> {
    let now = chrono::Utc::now();

    report
        .test_suites
        .iter()
        .flat_map(|suite| {
            suite.test_cases.iter().map(move |tc| {
                let test_case = TestCase {
                    id: format!("{}::{}", tc.classname, tc.name),
                    name: tc.name.clone(),
                    module: if tc.classname.is_empty() {
                        suite.name.clone()
                    } else {
                        tc.classname.clone()
                    },
                    tags: Vec::new(),
                    topology: None,
                    platform: None,
                    description: None,
                    timeout_secs: 0,
                };

                TestCaseResult {
                    test_case,
                    outcome: tc.status,
                    duration: std::time::Duration::from_secs_f64(tc.time),
                    message: tc.failure_message.clone(),
                    stdout: tc.system_out.clone(),
                    stderr: tc.system_err.clone(),
                    started_at: now,
                    finished_at: now,
                }
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// XML attribute helpers
// ---------------------------------------------------------------------------

fn attr_str(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name.as_bytes())
        .and_then(|a| String::from_utf8(a.value.to_vec()).ok())
}

fn attr_f64(e: &BytesStart<'_>, name: &str) -> Option<f64> {
    attr_str(e, name).and_then(|s| s.parse().ok())
}

fn attr_usize(e: &BytesStart<'_>, name: &str) -> Option<usize> {
    attr_str(e, name).and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites tests="4" failures="1" errors="1" skipped="1" time="12.345">
  <testsuite name="bgp_tests" tests="4" failures="1" errors="1" time="12.345">
    <testcase classname="bgp" name="test_convergence" time="3.200" />
    <testcase classname="bgp" name="test_route_install" time="5.100">
      <failure message="route not found" type="AssertionError">Expected route 10.0.0.0/24</failure>
    </testcase>
    <testcase classname="bgp" name="test_neighbor_down" time="0.500">
      <error message="connection refused" type="ConnectionError">SSH failed</error>
    </testcase>
    <testcase classname="bgp" name="test_ecmp" time="0.0">
      <skipped message="ECMP not supported on virtual platform" />
    </testcase>
  </testsuite>
</testsuites>"#;

    #[test]
    fn parse_sample_report() {
        let report = parse_junit_xml(SAMPLE_XML).unwrap();
        assert_eq!(report.tests, 4);
        assert_eq!(report.failures, 1);
        assert_eq!(report.errors, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.test_suites.len(), 1);

        let suite = &report.test_suites[0];
        assert_eq!(suite.name, "bgp_tests");
        assert_eq!(suite.test_cases.len(), 4);
    }

    #[test]
    fn parse_test_outcomes() {
        let report = parse_junit_xml(SAMPLE_XML).unwrap();
        let cases = &report.test_suites[0].test_cases;

        assert_eq!(cases[0].status, TestOutcome::Passed);
        assert_eq!(cases[0].name, "test_convergence");

        assert_eq!(cases[1].status, TestOutcome::Failed);
        assert_eq!(cases[1].failure_message.as_deref(), Some("route not found"));

        assert_eq!(cases[2].status, TestOutcome::Error);
        assert_eq!(cases[2].failure_type.as_deref(), Some("ConnectionError"));

        assert_eq!(cases[3].status, TestOutcome::Skipped);
    }

    #[test]
    fn to_json_roundtrip() {
        let report = parse_junit_xml(SAMPLE_XML).unwrap();
        let json = to_json(&report);
        assert_eq!(json["tests"], 4);
        assert!(json["test_suites"].is_array());
    }

    #[test]
    fn to_test_results_converts() {
        let report = parse_junit_xml(SAMPLE_XML).unwrap();
        let results = to_test_results(&report);
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].outcome, TestOutcome::Passed);
        assert_eq!(results[1].outcome, TestOutcome::Failed);
    }

    #[test]
    fn parse_single_suite_no_wrapper() {
        let xml = r#"<?xml version="1.0"?>
<testsuite name="acl" tests="2" failures="0" errors="0" time="1.5">
  <testcase classname="acl" name="test_deny" time="0.8" />
  <testcase classname="acl" name="test_allow" time="0.7" />
</testsuite>"#;

        let report = parse_junit_xml(xml).unwrap();
        assert_eq!(report.test_suites.len(), 1);
        assert_eq!(report.tests, 2);
    }

    #[test]
    fn parse_malformed_xml_returns_error() {
        let xml = "<testsuites><broken";
        let result = parse_junit_xml(xml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_report() {
        let xml = r#"<?xml version="1.0"?><testsuites tests="0"></testsuites>"#;
        let report = parse_junit_xml(xml).unwrap();
        assert_eq!(report.tests, 0);
        assert!(report.test_suites.is_empty());
    }
}
