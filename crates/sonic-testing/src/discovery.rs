//! Test discovery engine.
//!
//! Scans directories for TOML test definition files (`test_*.toml`), parses
//! their metadata, applies filters, and returns a sorted list of discovered
//! [`TestCase`]s ready for execution.

use std::path::{Path, PathBuf};

use glob::glob;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace, warn};

use sonic_core::{Platform, Result, SonicError, TestCase, TestFilter, TopologyType};

// ---------------------------------------------------------------------------
// TestDefinitionFile -- the on-disk TOML format
// ---------------------------------------------------------------------------

/// Represents the on-disk TOML format for a test definition file.
///
/// Each file contains a `[[test]]` array where every entry describes one test
/// case.  Example:
///
/// ```toml
/// [[test]]
/// name = "test_bgp_convergence"
/// module = "bgp"
/// tags = ["bgp", "convergence"]
/// topology = "t0"
/// timeout_secs = 300
/// description = "Verify BGP converges within 60 seconds after link flap"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDefinitionFile {
    /// The array of test case definitions in this file.
    #[serde(rename = "test")]
    pub tests: Vec<TestDefinition>,
}

/// A single test definition entry within a TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDefinition {
    /// Unique test name (e.g. `"test_bgp_convergence"`).
    pub name: String,

    /// Module / subsystem this test belongs to (e.g. `"bgp"`, `"acl"`).
    #[serde(default)]
    pub module: String,

    /// Free-form tags for filtering (e.g. `["smoke", "p0"]`).
    #[serde(default)]
    pub tags: Vec<String>,

    /// Required topology type, if any.
    #[serde(default)]
    pub topology: Option<TopologyType>,

    /// Required platform, if any.
    #[serde(default)]
    pub platform: Option<Platform>,

    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Per-test timeout in seconds. Defaults to 900 (15 minutes).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    900
}

// ---------------------------------------------------------------------------
// TestDiscovery
// ---------------------------------------------------------------------------

/// Configuration and state for the test discovery engine.
#[derive(Debug, Clone)]
pub struct TestDiscovery {
    /// Directories to scan for test files.
    pub search_paths: Vec<PathBuf>,

    /// Glob patterns for matching test files within each search path.
    /// Defaults to `["test_*.toml"]`.
    pub file_patterns: Vec<String>,
}

impl Default for TestDiscovery {
    fn default() -> Self {
        Self {
            search_paths: vec![PathBuf::from(".")],
            file_patterns: vec!["test_*.toml".to_owned()],
        }
    }
}

impl TestDiscovery {
    /// Creates a new `TestDiscovery` with the given search paths.
    pub fn new(search_paths: Vec<PathBuf>) -> Self {
        Self {
            search_paths,
            ..Default::default()
        }
    }

    /// Adds a file pattern (glob) to match against.
    pub fn with_patterns(mut self, patterns: Vec<String>) -> Self {
        self.file_patterns = patterns;
        self
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Discovers test cases under `base_path`, applying `filter` to narrow results.
///
/// Scans all directories matching the discovery configuration for files that
/// match the configured patterns, parses each as a TOML test definition file,
/// and returns the filtered, sorted list of [`TestCase`]s.
pub fn discover_tests(base_path: &Path, filter: &TestFilter) -> Result<Vec<TestCase>> {
    info!(
        base = %base_path.display(),
        patterns = ?filter.patterns,
        tags = ?filter.tags,
        "starting test discovery"
    );

    let mut all_cases = Vec::new();

    // Collect all matching TOML files.
    let test_files = find_test_files(base_path)?;
    debug!(count = test_files.len(), "found test definition files");

    for file_path in &test_files {
        match parse_test_file(file_path) {
            Ok(cases) => {
                trace!(
                    file = %file_path.display(),
                    count = cases.len(),
                    "parsed test cases"
                );
                all_cases.extend(cases);
            }
            Err(e) => {
                warn!(
                    file = %file_path.display(),
                    error = %e,
                    "skipping malformed test file"
                );
            }
        }
    }

    let before_filter = all_cases.len();
    let mut filtered = apply_filter(all_cases, filter)?;

    // Sort by module, then by name for deterministic ordering.
    filtered.sort_by(|a, b| a.module.cmp(&b.module).then_with(|| a.name.cmp(&b.name)));

    info!(
        total_discovered = before_filter,
        after_filter = filtered.len(),
        "test discovery complete"
    );

    Ok(filtered)
}

/// Parses a single TOML test definition file and returns the test cases it
/// defines.
pub fn parse_test_file(path: &Path) -> Result<Vec<TestCase>> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        SonicError::Test(format!("failed to read test file {}: {e}", path.display()))
    })?;

    let def_file: TestDefinitionFile = toml::from_str(&contents).map_err(|e| {
        SonicError::Test(format!(
            "failed to parse test file {}: {e}",
            path.display()
        ))
    })?;

    // Derive the module name from the file path if not specified per-test.
    let default_module = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .strip_prefix("test_")
        .unwrap_or(
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
        )
        .to_owned();

    let cases = def_file
        .tests
        .into_iter()
        .map(|def| {
            let module = if def.module.is_empty() {
                default_module.clone()
            } else {
                def.module
            };

            // Generate a stable ID from module + name.
            let id = format!("{module}::{}", def.name);

            TestCase {
                id,
                name: def.name,
                module,
                tags: def.tags,
                topology: def.topology,
                platform: def.platform,
                description: def.description,
                timeout_secs: def.timeout_secs,
            }
        })
        .collect();

    Ok(cases)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Finds all test definition files under `base_path` using glob patterns.
fn find_test_files(base_path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    // Search for test_*.toml files recursively.
    let pattern = format!("{}/**/test_*.toml", base_path.display());
    let entries = glob(&pattern).map_err(|e| {
        SonicError::Test(format!("invalid glob pattern `{pattern}`: {e}"))
    })?;

    for entry in entries {
        match entry {
            Ok(path) => {
                if path.is_file() {
                    files.push(path);
                }
            }
            Err(e) => {
                warn!(error = %e, "glob entry error, skipping");
            }
        }
    }

    // Also search for test_*.rs files that might have TOML frontmatter.
    let rs_pattern = format!("{}/**/test_*.rs", base_path.display());
    if let Ok(entries) = glob(&rs_pattern) {
        for entry in entries.flatten() {
            if entry.is_file() && has_toml_frontmatter(&entry) {
                files.push(entry);
            }
        }
    }

    files.sort();
    files.dedup();

    Ok(files)
}

/// Checks whether a `.rs` file starts with a TOML frontmatter block
/// delimited by `//! ---` markers.
fn has_toml_frontmatter(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    contents.starts_with("//! ---")
}

/// Applies the [`TestFilter`] to a list of test cases.
fn apply_filter(cases: Vec<TestCase>, filter: &TestFilter) -> Result<Vec<TestCase>> {
    // Pre-compile include patterns.
    let include_regexes: Vec<Regex> = filter
        .patterns
        .iter()
        .map(|p| {
            // Convert glob-like patterns to regex: `*` -> `.*`, `?` -> `.`
            let regex_str = format!(
                "^{}$",
                regex::escape(p).replace(r"\*", ".*").replace(r"\?", ".")
            );
            Regex::new(&regex_str)
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Pre-compile exclude patterns.
    let exclude_regexes: Vec<Regex> = filter
        .exclude_patterns
        .iter()
        .map(|p| {
            let regex_str = format!(
                "^{}$",
                regex::escape(p).replace(r"\*", ".*").replace(r"\?", ".")
            );
            Regex::new(&regex_str)
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let filtered = cases
        .into_iter()
        .filter(|tc| {
            // 1. Exclusion patterns: if any match, skip.
            let full_name = format!("{}::{}", tc.module, tc.name);
            if exclude_regexes
                .iter()
                .any(|re| re.is_match(&full_name) || re.is_match(&tc.name))
            {
                trace!(test = %full_name, "excluded by pattern");
                return false;
            }

            // 2. Include patterns: if specified, at least one must match.
            if !include_regexes.is_empty()
                && !include_regexes
                    .iter()
                    .any(|re| re.is_match(&full_name) || re.is_match(&tc.name))
            {
                trace!(test = %full_name, "not matched by include patterns");
                return false;
            }

            // 3. Tag filter: if specified, test must have at least one matching tag.
            if !filter.tags.is_empty()
                && !filter.tags.iter().any(|tag| tc.tags.contains(tag))
            {
                trace!(test = %full_name, "no matching tags");
                return false;
            }

            // 4. Topology filter: if specified, test topology must match one.
            if !filter.topologies.is_empty() {
                match &tc.topology {
                    Some(topo) => {
                        if !filter.topologies.contains(topo) {
                            trace!(test = %full_name, "topology mismatch");
                            return false;
                        }
                    }
                    // Tests without a topology are included only if the filter
                    // contains `Any`.
                    None => {
                        if !filter.topologies.contains(&TopologyType::Any) {
                            trace!(test = %full_name, "no topology, filter requires one");
                            return false;
                        }
                    }
                }
            }

            // 5. Platform filter: if specified, test platform must match one.
            if !filter.platforms.is_empty() {
                match &tc.platform {
                    Some(plat) => {
                        if !filter.platforms.contains(plat) {
                            trace!(test = %full_name, "platform mismatch");
                            return false;
                        }
                    }
                    None => {
                        // Tests without a platform constraint pass any platform filter.
                    }
                }
            }

            true
        })
        .collect();

    Ok(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_toml() -> &'static str {
        r#"
[[test]]
name = "test_bgp_convergence"
module = "bgp"
tags = ["bgp", "convergence", "smoke"]
topology = "t0"
timeout_secs = 300
description = "Verify BGP convergence after link flap"

[[test]]
name = "test_acl_deny"
module = "acl"
tags = ["acl", "security"]
topology = "t1"
platform = "broadcom"
timeout_secs = 120

[[test]]
name = "test_vlan_basic"
tags = ["vlan"]
topology = "t0"
"#
    }

    fn write_temp_toml(dir: &Path, filename: &str, content: &str) -> PathBuf {
        let path = dir.join(filename);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parse_test_file_parses_all_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp_toml(dir.path(), "test_bgp.toml", sample_toml());

        let cases = parse_test_file(&path).unwrap();
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].name, "test_bgp_convergence");
        assert_eq!(cases[0].module, "bgp");
        assert_eq!(cases[1].platform, Some(Platform::Broadcom));
        // Third test should inherit module from filename.
        assert_eq!(cases[2].module, "bgp");
    }

    #[test]
    fn discover_tests_with_empty_filter() {
        let dir = tempfile::tempdir().unwrap();
        write_temp_toml(dir.path(), "test_bgp.toml", sample_toml());

        let filter = TestFilter::default();
        let cases = discover_tests(dir.path(), &filter).unwrap();
        assert_eq!(cases.len(), 3);
    }

    #[test]
    fn filter_by_tags() {
        let dir = tempfile::tempdir().unwrap();
        write_temp_toml(dir.path(), "test_bgp.toml", sample_toml());

        let filter = TestFilter {
            tags: vec!["smoke".to_owned()],
            ..Default::default()
        };
        let cases = discover_tests(dir.path(), &filter).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "test_bgp_convergence");
    }

    #[test]
    fn filter_by_topology() {
        let dir = tempfile::tempdir().unwrap();
        write_temp_toml(dir.path(), "test_bgp.toml", sample_toml());

        let filter = TestFilter {
            topologies: vec![TopologyType::T1],
            ..Default::default()
        };
        let cases = discover_tests(dir.path(), &filter).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "test_acl_deny");
    }

    #[test]
    fn filter_by_exclude_pattern() {
        let dir = tempfile::tempdir().unwrap();
        write_temp_toml(dir.path(), "test_bgp.toml", sample_toml());

        let filter = TestFilter {
            exclude_patterns: vec!["*acl*".to_owned()],
            ..Default::default()
        };
        let cases = discover_tests(dir.path(), &filter).unwrap();
        assert_eq!(cases.len(), 2);
        assert!(cases.iter().all(|c| !c.name.contains("acl")));
    }
}
