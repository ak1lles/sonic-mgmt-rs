//! Test fixture management.
//!
//! Provides a registry of named fixtures with scope, dependency tracking, and
//! topological-sort-based resolution -- the Rust equivalent of pytest's
//! `conftest.py` fixture system.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace};

use sonic_core::{DeviceInfo, Result, SonicError, TestCase, TopologyType};

// ---------------------------------------------------------------------------
// FixtureScope
// ---------------------------------------------------------------------------

/// Determines how long a fixture stays active before teardown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureScope {
    /// Active for the entire test session.
    Session,
    /// Active for every test in the same module.
    Module,
    /// Active for every test in the same class / group.
    Class,
    /// Setup and torn down per individual test function.
    Function,
}

impl FixtureScope {
    /// Returns a numeric priority where higher values represent broader scopes.
    /// Used for ordering setup (broad first) and teardown (narrow first).
    fn priority(self) -> u8 {
        match self {
            Self::Session => 4,
            Self::Module => 3,
            Self::Class => 2,
            Self::Function => 1,
        }
    }
}

impl std::fmt::Display for FixtureScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session => write!(f, "session"),
            Self::Module => write!(f, "module"),
            Self::Class => write!(f, "class"),
            Self::Function => write!(f, "function"),
        }
    }
}

// ---------------------------------------------------------------------------
// FixtureDef
// ---------------------------------------------------------------------------

/// Definition of a single fixture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureDef {
    /// Unique fixture name (e.g. `"dut_connection"`, `"bgp_baseline"`).
    pub name: String,

    /// Scope of the fixture's lifetime.
    pub scope: FixtureScope,

    /// Name of the setup function / handler to invoke.
    pub setup_fn: String,

    /// Name of the teardown function / handler to invoke.
    pub teardown_fn: String,

    /// Names of other fixtures this one depends on (must be set up first).
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl FixtureDef {
    /// Creates a new fixture definition.
    pub fn new(
        name: impl Into<String>,
        scope: FixtureScope,
        setup_fn: impl Into<String>,
        teardown_fn: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            scope,
            setup_fn: setup_fn.into(),
            teardown_fn: teardown_fn.into(),
            dependencies: Vec::new(),
        }
    }

    /// Adds a dependency on another fixture.
    pub fn with_dependency(mut self, dep: impl Into<String>) -> Self {
        self.dependencies.push(dep.into());
        self
    }
}

// ---------------------------------------------------------------------------
// FixtureRegistry
// ---------------------------------------------------------------------------

/// Central registry of all known fixtures, keyed by name.
#[derive(Debug, Clone, Default)]
pub struct FixtureRegistry {
    fixtures: HashMap<String, FixtureDef>,
}

impl FixtureRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a fixture definition. Overwrites any existing fixture with the
    /// same name.
    pub fn register(&mut self, fixture: FixtureDef) {
        debug!(
            name = %fixture.name,
            scope = %fixture.scope,
            deps = ?fixture.dependencies,
            "registering fixture"
        );
        self.fixtures.insert(fixture.name.clone(), fixture);
    }

    /// Returns the fixture definition for `name`, if registered.
    pub fn get(&self, name: &str) -> Option<&FixtureDef> {
        self.fixtures.get(name)
    }

    /// Returns the total number of registered fixtures.
    pub fn len(&self) -> usize {
        self.fixtures.len()
    }

    /// Returns `true` if the registry contains no fixtures.
    pub fn is_empty(&self) -> bool {
        self.fixtures.is_empty()
    }

    /// Returns all registered fixture names.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.fixtures.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Validates that all dependencies reference registered fixtures and that
    /// no cycles exist.
    pub fn validate(&self) -> Result<()> {
        // Check that all dependencies exist.
        for (name, def) in &self.fixtures {
            for dep in &def.dependencies {
                if !self.fixtures.contains_key(dep) {
                    return Err(SonicError::Test(format!(
                        "fixture `{name}` depends on unknown fixture `{dep}`"
                    )));
                }
            }
        }

        // Check for cycles via DFS.
        let mut visited = HashSet::new();
        let mut in_stack = HashSet::new();

        for name in self.fixtures.keys() {
            if !visited.contains(name.as_str()) {
                self.detect_cycle(name, &mut visited, &mut in_stack)?;
            }
        }

        debug!(count = self.fixtures.len(), "fixture registry validated");
        Ok(())
    }

    /// DFS cycle detection helper.
    fn detect_cycle<'a>(
        &'a self,
        name: &'a str,
        visited: &mut HashSet<&'a str>,
        in_stack: &mut HashSet<&'a str>,
    ) -> Result<()> {
        visited.insert(name);
        in_stack.insert(name);

        if let Some(def) = self.fixtures.get(name) {
            for dep in &def.dependencies {
                if !visited.contains(dep.as_str()) {
                    self.detect_cycle(dep, visited, in_stack)?;
                } else if in_stack.contains(dep.as_str()) {
                    return Err(SonicError::Test(format!(
                        "circular fixture dependency detected: `{name}` -> `{dep}`"
                    )));
                }
            }
        }

        in_stack.remove(name);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FixtureContext
// ---------------------------------------------------------------------------

/// Runtime context holding the currently-active fixtures and device references.
#[derive(Debug, Clone)]
pub struct FixtureContext {
    /// Map of fixture name -> whether it is currently active.
    pub active_fixtures: HashMap<String, bool>,

    /// References to devices under test.
    pub devices: Vec<DeviceInfo>,

    /// Name of the testbed being used.
    pub testbed_name: String,

    /// Active topology type.
    pub topology: Option<TopologyType>,

    /// Arbitrary context variables set by fixtures during setup.
    pub variables: HashMap<String, String>,
}

impl FixtureContext {
    /// Creates a new, empty fixture context.
    pub fn new(testbed_name: impl Into<String>) -> Self {
        Self {
            active_fixtures: HashMap::new(),
            devices: Vec::new(),
            testbed_name: testbed_name.into(),
            topology: None,
            variables: HashMap::new(),
        }
    }

    /// Returns `true` if the named fixture is currently active.
    pub fn is_active(&self, fixture_name: &str) -> bool {
        self.active_fixtures
            .get(fixture_name)
            .copied()
            .unwrap_or(false)
    }

    /// Marks a fixture as active.
    pub fn activate(&mut self, fixture_name: &str) {
        self.active_fixtures
            .insert(fixture_name.to_owned(), true);
    }

    /// Marks a fixture as inactive.
    pub fn deactivate(&mut self, fixture_name: &str) {
        self.active_fixtures
            .insert(fixture_name.to_owned(), false);
    }

    /// Sets a context variable.
    pub fn set_variable(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(key.into(), value.into());
    }

    /// Gets a context variable.
    pub fn get_variable(&self, key: &str) -> Option<&str> {
        self.variables.get(key).map(|s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// Resolution & setup/teardown
// ---------------------------------------------------------------------------

/// Resolves the full, dependency-ordered list of fixtures required for a test
/// case.
///
/// Performs a topological sort (Kahn's algorithm) on the dependency graph so
/// that fixtures are set up in the correct order.  Broader scopes (session)
/// are sorted before narrower ones (function) at the same dependency level.
pub fn resolve_fixtures(
    test_case: &TestCase,
    registry: &FixtureRegistry,
) -> Result<Vec<FixtureDef>> {
    // Determine which fixtures this test needs. Convention: tags prefixed
    // with `fixture:` request that fixture. E.g. `fixture:dut_connection`.
    let requested: Vec<&str> = test_case
        .tags
        .iter()
        .filter_map(|tag| tag.strip_prefix("fixture:"))
        .collect();

    if requested.is_empty() {
        trace!(test = %test_case.name, "no fixtures requested");
        return Ok(Vec::new());
    }

    // Collect transitive closure of all required fixtures.
    let mut required: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = requested.iter().map(|s| (*s).to_owned()).collect();

    while let Some(name) = queue.pop_front() {
        if required.contains(&name) {
            continue;
        }
        let def = registry.get(&name).ok_or_else(|| {
            SonicError::Test(format!(
                "test `{}` requests unknown fixture `{name}`",
                test_case.name
            ))
        })?;
        required.insert(name.clone());
        for dep in &def.dependencies {
            if !required.contains(dep) {
                queue.push_back(dep.clone());
            }
        }
    }

    // Topological sort using Kahn's algorithm.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for name in &required {
        in_degree.entry(name.as_str()).or_insert(0);
        adj.entry(name.as_str()).or_default();
    }

    for name in &required {
        let def = registry.get(name).unwrap();
        for dep in &def.dependencies {
            if required.contains(dep) {
                adj.entry(dep.as_str()).or_default().push(name.as_str());
                *in_degree.entry(name.as_str()).or_insert(0) += 1;
            }
        }
    }

    let mut ready: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&name, _)| name)
        .collect();

    // Sort ready nodes by scope priority (broadest first), then alphabetically
    // for determinism.
    ready.sort_by(|a, b| {
        let scope_a = registry.get(a).map(|d| d.scope.priority()).unwrap_or(0);
        let scope_b = registry.get(b).map(|d| d.scope.priority()).unwrap_or(0);
        scope_b.cmp(&scope_a).then_with(|| a.cmp(b))
    });

    let mut sorted = Vec::new();
    let mut ready_queue: VecDeque<&str> = ready.into_iter().collect();

    while let Some(node) = ready_queue.pop_front() {
        sorted.push(node.to_owned());

        let neighbors: Vec<&str> = adj.get(node).cloned().unwrap_or_default();
        let mut next_ready = Vec::new();

        for neighbor in neighbors {
            let deg = in_degree.get_mut(neighbor).unwrap();
            *deg -= 1;
            if *deg == 0 {
                next_ready.push(neighbor);
            }
        }

        next_ready.sort_by(|a, b| {
            let scope_a = registry.get(a).map(|d| d.scope.priority()).unwrap_or(0);
            let scope_b = registry.get(b).map(|d| d.scope.priority()).unwrap_or(0);
            scope_b.cmp(&scope_a).then_with(|| a.cmp(b))
        });

        for n in next_ready {
            ready_queue.push_back(n);
        }
    }

    if sorted.len() != required.len() {
        return Err(SonicError::Test(
            "circular dependency detected during fixture resolution".to_owned(),
        ));
    }

    let ordered: Vec<FixtureDef> = sorted
        .iter()
        .filter_map(|name| registry.get(name).cloned())
        .collect();

    debug!(
        test = %test_case.name,
        fixtures = ?sorted,
        "resolved fixture order"
    );

    Ok(ordered)
}

/// Sets up fixtures in the provided dependency order.
///
/// Each fixture's `setup_fn` name is recorded in the context as active. In a
/// real deployment the setup_fn names would be looked up in a function
/// registry; here we track activation state so the execution engine can
/// invoke the actual functions.
pub fn setup_fixtures(
    context: &mut FixtureContext,
    fixtures: &[FixtureDef],
) -> Result<()> {
    for fixture in fixtures {
        if context.is_active(&fixture.name) {
            trace!(fixture = %fixture.name, "already active, skipping setup");
            continue;
        }

        // Verify all dependencies are active.
        for dep in &fixture.dependencies {
            if !context.is_active(dep) {
                return Err(SonicError::Test(format!(
                    "cannot setup fixture `{}`: dependency `{dep}` is not active",
                    fixture.name
                )));
            }
        }

        info!(
            fixture = %fixture.name,
            scope = %fixture.scope,
            setup_fn = %fixture.setup_fn,
            "setting up fixture"
        );

        // Record the fixture as active. The actual function invocation is
        // performed by the execution engine which maps setup_fn names to
        // real function pointers.
        context.activate(&fixture.name);
        context.set_variable(
            format!("fixture.{}.setup_fn", fixture.name),
            fixture.setup_fn.clone(),
        );
    }

    Ok(())
}

/// Tears down fixtures in reverse dependency order.
///
/// Fixtures are torn down from narrowest scope to broadest, and within the
/// same scope in reverse of the setup order.
pub fn teardown_fixtures(
    context: &mut FixtureContext,
    fixtures: &[FixtureDef],
) -> Result<()> {
    // Reverse order: last setup = first teardown.
    for fixture in fixtures.iter().rev() {
        if !context.is_active(&fixture.name) {
            trace!(fixture = %fixture.name, "not active, skipping teardown");
            continue;
        }

        info!(
            fixture = %fixture.name,
            scope = %fixture.scope,
            teardown_fn = %fixture.teardown_fn,
            "tearing down fixture"
        );

        context.deactivate(&fixture.name);
        context.variables.remove(&format!("fixture.{}.setup_fn", fixture.name));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_registry() -> FixtureRegistry {
        let mut reg = FixtureRegistry::new();

        reg.register(FixtureDef::new(
            "connection",
            FixtureScope::Session,
            "setup_connection",
            "teardown_connection",
        ));

        reg.register(
            FixtureDef::new(
                "dut_ready",
                FixtureScope::Module,
                "setup_dut",
                "teardown_dut",
            )
            .with_dependency("connection"),
        );

        reg.register(
            FixtureDef::new(
                "bgp_baseline",
                FixtureScope::Function,
                "setup_bgp",
                "teardown_bgp",
            )
            .with_dependency("dut_ready"),
        );

        reg
    }

    fn test_case_with_fixtures(fixtures: &[&str]) -> TestCase {
        TestCase {
            id: "test::example".to_owned(),
            name: "test_example".to_owned(),
            module: "test".to_owned(),
            tags: fixtures
                .iter()
                .map(|f| format!("fixture:{f}"))
                .collect(),
            topology: None,
            platform: None,
            description: None,
            timeout_secs: 60,
        }
    }

    #[test]
    fn validate_good_registry() {
        let reg = sample_registry();
        reg.validate().unwrap();
    }

    #[test]
    fn validate_detects_missing_dependency() {
        let mut reg = FixtureRegistry::new();
        reg.register(
            FixtureDef::new("a", FixtureScope::Function, "s", "t")
                .with_dependency("nonexistent"),
        );
        assert!(reg.validate().is_err());
    }

    #[test]
    fn validate_detects_cycle() {
        let mut reg = FixtureRegistry::new();
        reg.register(
            FixtureDef::new("a", FixtureScope::Function, "s", "t")
                .with_dependency("b"),
        );
        reg.register(
            FixtureDef::new("b", FixtureScope::Function, "s", "t")
                .with_dependency("a"),
        );
        assert!(reg.validate().is_err());
    }

    #[test]
    fn resolve_orders_by_dependency() {
        let reg = sample_registry();
        let tc = test_case_with_fixtures(&["bgp_baseline"]);
        let fixtures = resolve_fixtures(&tc, &reg).unwrap();

        let names: Vec<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["connection", "dut_ready", "bgp_baseline"]);
    }

    #[test]
    fn setup_and_teardown_lifecycle() {
        let reg = sample_registry();
        let tc = test_case_with_fixtures(&["bgp_baseline"]);
        let fixtures = resolve_fixtures(&tc, &reg).unwrap();

        let mut ctx = FixtureContext::new("test-tb");

        setup_fixtures(&mut ctx, &fixtures).unwrap();
        assert!(ctx.is_active("connection"));
        assert!(ctx.is_active("dut_ready"));
        assert!(ctx.is_active("bgp_baseline"));

        teardown_fixtures(&mut ctx, &fixtures).unwrap();
        assert!(!ctx.is_active("connection"));
        assert!(!ctx.is_active("dut_ready"));
        assert!(!ctx.is_active("bgp_baseline"));
    }

    #[test]
    fn resolve_no_fixtures_returns_empty() {
        let reg = sample_registry();
        let tc = TestCase {
            id: "test::plain".to_owned(),
            name: "test_plain".to_owned(),
            module: "test".to_owned(),
            tags: vec!["smoke".to_owned()],
            topology: None,
            platform: None,
            description: None,
            timeout_secs: 60,
        };
        let fixtures = resolve_fixtures(&tc, &reg).unwrap();
        assert!(fixtures.is_empty());
    }
}
