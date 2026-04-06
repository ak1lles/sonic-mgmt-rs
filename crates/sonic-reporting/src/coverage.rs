//! SAI / SWSS test coverage tracking.
//!
//! Records which SAI APIs and SWSS invocations each test exercises, then
//! aggregates the data into a coverage report showing which APIs are tested
//! and which are not.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use sonic_core::{Result, SonicError};

// ---------------------------------------------------------------------------
// CoverageTracker
// ---------------------------------------------------------------------------

/// Tracks which SAI and SWSS APIs are exercised by each test.
#[derive(Debug, Clone, Default)]
pub struct CoverageTracker {
    /// Map of SAI API name -> set of test names that exercise it.
    sai_apis: HashMap<String, HashSet<String>>,

    /// Map of SWSS invocation name -> set of test names that exercise it.
    swss_invocations: HashMap<String, HashSet<String>>,

    /// Known universe of all SAI APIs (for computing uncovered list).
    known_sai_apis: HashSet<String>,

    /// Known universe of all SWSS invocations.
    known_swss_invocations: HashSet<String>,
}

impl CoverageTracker {
    /// Creates a new, empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a known SAI API. This is used to compute the "uncovered"
    /// set in the coverage report.
    pub fn register_sai_api(&mut self, api: impl Into<String>) {
        self.known_sai_apis.insert(api.into());
    }

    /// Registers multiple known SAI APIs.
    pub fn register_sai_apis(&mut self, apis: impl IntoIterator<Item = impl Into<String>>) {
        for api in apis {
            self.known_sai_apis.insert(api.into());
        }
    }

    /// Registers a known SWSS invocation.
    pub fn register_swss_invocation(&mut self, invocation: impl Into<String>) {
        self.known_swss_invocations.insert(invocation.into());
    }

    /// Registers multiple known SWSS invocations.
    pub fn register_swss_invocations(
        &mut self,
        invocations: impl IntoIterator<Item = impl Into<String>>,
    ) {
        for inv in invocations {
            self.known_swss_invocations.insert(inv.into());
        }
    }

    /// Records which SAI and SWSS APIs a particular test exercises.
    pub fn track_test(
        &mut self,
        test_name: &str,
        sai_calls: &[String],
        swss_calls: &[String],
    ) {
        debug!(
            test = test_name,
            sai_count = sai_calls.len(),
            swss_count = swss_calls.len(),
            "tracking test API calls"
        );

        for api in sai_calls {
            self.sai_apis
                .entry(api.clone())
                .or_default()
                .insert(test_name.to_owned());
            // Auto-register APIs seen in actual usage.
            self.known_sai_apis.insert(api.clone());
        }

        for inv in swss_calls {
            self.swss_invocations
                .entry(inv.clone())
                .or_default()
                .insert(test_name.to_owned());
            self.known_swss_invocations.insert(inv.clone());
        }
    }

    /// Generates a coverage report aggregating all tracked data.
    pub fn get_coverage_report(&self) -> CoverageReport {
        // SAI coverage.
        let sai_total = self.known_sai_apis.len();
        let sai_covered: HashSet<&str> = self
            .sai_apis
            .keys()
            .map(|k| k.as_str())
            .collect();
        let sai_covered_count = sai_covered.len();

        let mut sai_uncovered: Vec<String> = self
            .known_sai_apis
            .iter()
            .filter(|api| !sai_covered.contains(api.as_str()))
            .cloned()
            .collect();
        sai_uncovered.sort();

        let sai_pct = if sai_total > 0 {
            sai_covered_count as f64 / sai_total as f64 * 100.0
        } else {
            0.0
        };

        // SWSS coverage.
        let swss_total = self.known_swss_invocations.len();
        let swss_covered: HashSet<&str> = self
            .swss_invocations
            .keys()
            .map(|k| k.as_str())
            .collect();
        let swss_covered_count = swss_covered.len();

        let mut swss_uncovered: Vec<String> = self
            .known_swss_invocations
            .iter()
            .filter(|inv| !swss_covered.contains(inv.as_str()))
            .cloned()
            .collect();
        swss_uncovered.sort();

        let swss_pct = if swss_total > 0 {
            swss_covered_count as f64 / swss_total as f64 * 100.0
        } else {
            0.0
        };

        // Combined totals.
        let total_apis = sai_total + swss_total;
        let covered_apis = sai_covered_count + swss_covered_count;
        let coverage_pct = if total_apis > 0 {
            covered_apis as f64 / total_apis as f64 * 100.0
        } else {
            0.0
        };

        let mut all_uncovered = sai_uncovered.clone();
        all_uncovered.extend(swss_uncovered.iter().cloned());
        all_uncovered.sort();

        // Per-test breakdown: which APIs each test covers.
        let mut per_test: HashMap<String, TestCoverage> = HashMap::new();

        for (api, tests) in &self.sai_apis {
            for test in tests {
                per_test
                    .entry(test.clone())
                    .or_insert_with(|| TestCoverage {
                        test_name: test.clone(),
                        sai_apis: Vec::new(),
                        swss_invocations: Vec::new(),
                    })
                    .sai_apis
                    .push(api.clone());
            }
        }

        for (inv, tests) in &self.swss_invocations {
            for test in tests {
                per_test
                    .entry(test.clone())
                    .or_insert_with(|| TestCoverage {
                        test_name: test.clone(),
                        sai_apis: Vec::new(),
                        swss_invocations: Vec::new(),
                    })
                    .swss_invocations
                    .push(inv.clone());
            }
        }

        // Sort the per-test data for deterministic output.
        let mut per_test_list: Vec<TestCoverage> = per_test.into_values().collect();
        for tc in &mut per_test_list {
            tc.sai_apis.sort();
            tc.swss_invocations.sort();
        }
        per_test_list.sort_by(|a, b| a.test_name.cmp(&b.test_name));

        info!(
            total_apis,
            covered_apis,
            coverage_pct = format!("{coverage_pct:.1}%"),
            sai_total,
            sai_covered = sai_covered_count,
            swss_total,
            swss_covered = swss_covered_count,
            "generated coverage report"
        );

        CoverageReport {
            total_apis,
            covered_apis,
            coverage_pct,
            uncovered: all_uncovered,
            per_test: per_test_list,
            sai_total,
            sai_covered: sai_covered_count,
            sai_coverage_pct: sai_pct,
            sai_uncovered,
            swss_total,
            swss_covered: swss_covered_count,
            swss_coverage_pct: swss_pct,
            swss_uncovered,
        }
    }
}

// ---------------------------------------------------------------------------
// CoverageReport
// ---------------------------------------------------------------------------

/// Aggregate coverage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageReport {
    /// Total number of known APIs (SAI + SWSS).
    pub total_apis: usize,
    /// Number of APIs covered by at least one test.
    pub covered_apis: usize,
    /// Overall coverage percentage.
    pub coverage_pct: f64,
    /// List of uncovered API names.
    pub uncovered: Vec<String>,
    /// Per-test coverage breakdown.
    pub per_test: Vec<TestCoverage>,

    // SAI-specific.
    pub sai_total: usize,
    pub sai_covered: usize,
    pub sai_coverage_pct: f64,
    pub sai_uncovered: Vec<String>,

    // SWSS-specific.
    pub swss_total: usize,
    pub swss_covered: usize,
    pub swss_coverage_pct: f64,
    pub swss_uncovered: Vec<String>,
}

/// Coverage data for a single test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCoverage {
    pub test_name: String,
    pub sai_apis: Vec<String>,
    pub swss_invocations: Vec<String>,
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Exports a coverage report to the specified format (JSON or CSV).
pub fn export_coverage(report: &CoverageReport, format: &str) -> Result<String> {
    match format.to_lowercase().as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(report)?;
            Ok(json)
        }
        "csv" => {
            let mut csv = String::new();
            csv.push_str("category,api_name,covered,test_count,tests\n");

            // We don't have direct access to the tracker here, so we
            // reconstruct from the report.
            let mut sai_test_map: HashMap<String, Vec<String>> = HashMap::new();
            for tc in &report.per_test {
                for api in &tc.sai_apis {
                    sai_test_map
                        .entry(api.clone())
                        .or_default()
                        .push(tc.test_name.clone());
                }
            }

            // All known SAI APIs.
            let mut all_sai: HashSet<String> = sai_test_map.keys().cloned().collect();
            for api in &report.sai_uncovered {
                all_sai.insert(api.clone());
            }
            let mut sai_sorted: Vec<String> = all_sai.into_iter().collect();
            sai_sorted.sort();

            for api in &sai_sorted {
                let tests = sai_test_map.get(api);
                let covered = tests.is_some();
                let count = tests.map(|t| t.len()).unwrap_or(0);
                let test_list = tests
                    .map(|t| t.join(";"))
                    .unwrap_or_default();
                csv.push_str(&format!(
                    "sai,\"{}\",{},{},\"{}\"\n",
                    csv_escape(api),
                    covered,
                    count,
                    csv_escape(&test_list),
                ));
            }

            // Same for SWSS.
            let mut swss_test_map: HashMap<String, Vec<String>> = HashMap::new();
            for tc in &report.per_test {
                for inv in &tc.swss_invocations {
                    swss_test_map
                        .entry(inv.clone())
                        .or_default()
                        .push(tc.test_name.clone());
                }
            }

            let mut all_swss: HashSet<String> = swss_test_map.keys().cloned().collect();
            for inv in &report.swss_uncovered {
                all_swss.insert(inv.clone());
            }
            let mut swss_sorted: Vec<String> = all_swss.into_iter().collect();
            swss_sorted.sort();

            for inv in &swss_sorted {
                let tests = swss_test_map.get(inv);
                let covered = tests.is_some();
                let count = tests.map(|t| t.len()).unwrap_or(0);
                let test_list = tests
                    .map(|t| t.join(";"))
                    .unwrap_or_default();
                csv.push_str(&format!(
                    "swss,\"{}\",{},{},\"{}\"\n",
                    csv_escape(inv),
                    covered,
                    count,
                    csv_escape(&test_list),
                ));
            }

            Ok(csv)
        }
        other => Err(SonicError::Other(format!(
            "unsupported coverage export format: {other}"
        ))),
    }
}

fn csv_escape(s: &str) -> String {
    s.replace('"', "\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tracker() -> CoverageTracker {
        let mut tracker = CoverageTracker::new();

        // Register known APIs.
        tracker.register_sai_apis([
            "sai_create_route_entry",
            "sai_remove_route_entry",
            "sai_create_next_hop",
            "sai_create_acl_table",
            "sai_create_acl_entry",
        ].map(String::from));

        tracker.register_swss_invocations([
            "PortInitDone",
            "RouteOrch::addRoute",
            "AclOrch::addAclTable",
            "AclOrch::addAclRule",
        ].map(String::from));

        // Track test coverage.
        tracker.track_test(
            "test_route_add",
            &["sai_create_route_entry".into(), "sai_create_next_hop".into()],
            &["RouteOrch::addRoute".into()],
        );

        tracker.track_test(
            "test_acl_create",
            &["sai_create_acl_table".into(), "sai_create_acl_entry".into()],
            &["AclOrch::addAclTable".into(), "AclOrch::addAclRule".into()],
        );

        tracker
    }

    #[test]
    fn coverage_report_totals() {
        let tracker = sample_tracker();
        let report = tracker.get_coverage_report();

        assert_eq!(report.sai_total, 5);
        assert_eq!(report.sai_covered, 4);
        assert_eq!(report.sai_uncovered, vec!["sai_remove_route_entry"]);

        assert_eq!(report.swss_total, 4);
        assert_eq!(report.swss_covered, 3);
        assert_eq!(report.swss_uncovered, vec!["PortInitDone"]);

        assert_eq!(report.total_apis, 9);
        assert_eq!(report.covered_apis, 7);
    }

    #[test]
    fn coverage_pct_calculation() {
        let tracker = sample_tracker();
        let report = tracker.get_coverage_report();

        // 7 out of 9 = 77.78%
        assert!((report.coverage_pct - 77.78).abs() < 1.0);
        // 4 out of 5 = 80%
        assert!((report.sai_coverage_pct - 80.0).abs() < 0.1);
        // 3 out of 4 = 75%
        assert!((report.swss_coverage_pct - 75.0).abs() < 0.1);
    }

    #[test]
    fn per_test_breakdown() {
        let tracker = sample_tracker();
        let report = tracker.get_coverage_report();

        assert_eq!(report.per_test.len(), 2);

        let acl = report
            .per_test
            .iter()
            .find(|t| t.test_name == "test_acl_create")
            .unwrap();
        assert_eq!(acl.sai_apis.len(), 2);
        assert_eq!(acl.swss_invocations.len(), 2);
    }

    #[test]
    fn export_json() {
        let tracker = sample_tracker();
        let report = tracker.get_coverage_report();
        let json = export_coverage(&report, "json").unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["total_apis"], 9);
    }

    #[test]
    fn export_csv() {
        let tracker = sample_tracker();
        let report = tracker.get_coverage_report();
        let csv = export_coverage(&report, "csv").unwrap();

        let lines: Vec<&str> = csv.lines().collect();
        // Header + SAI APIs + SWSS invocations.
        assert!(lines.len() > 1);
        assert!(lines[0].contains("category,api_name,covered"));
        // Verify at least one covered entry.
        assert!(csv.contains("sai,\"sai_create_route_entry\",true"));
        // And one uncovered.
        assert!(csv.contains("sai,\"sai_remove_route_entry\",false"));
    }

    #[test]
    fn export_unsupported_format() {
        let tracker = sample_tracker();
        let report = tracker.get_coverage_report();
        assert!(export_coverage(&report, "xml").is_err());
    }

    #[test]
    fn empty_tracker_report() {
        let tracker = CoverageTracker::new();
        let report = tracker.get_coverage_report();

        assert_eq!(report.total_apis, 0);
        assert_eq!(report.covered_apis, 0);
        assert_eq!(report.coverage_pct, 0.0);
        assert!(report.uncovered.is_empty());
        assert!(report.per_test.is_empty());
    }

    #[test]
    fn auto_register_from_tracking() {
        let mut tracker = CoverageTracker::new();
        // Don't register any APIs upfront; they should be auto-registered.
        tracker.track_test(
            "test_foo",
            &["sai_new_api".into()],
            &["NewOrch::doSomething".into()],
        );

        let report = tracker.get_coverage_report();
        assert_eq!(report.sai_total, 1);
        assert_eq!(report.sai_covered, 1);
        assert_eq!(report.swss_total, 1);
        assert_eq!(report.swss_covered, 1);
        assert_eq!(report.coverage_pct, 100.0);
    }
}
