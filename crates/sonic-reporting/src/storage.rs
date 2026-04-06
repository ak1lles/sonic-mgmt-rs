//! Pluggable data storage backends for test reports.
//!
//! Defines the [`ReportStorage`] trait and provides two implementations:
//! - [`KustoStorage`]: Azure Data Explorer (Kusto) ingest/query
//! - [`LocalFileStorage`]: File-system-based JSON/TOML storage

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use sonic_core::{AuthMethod, ReportFormat, Result, SonicError, TestCaseResult, TestOutcome};

// ---------------------------------------------------------------------------
// ReportStorage trait
// ---------------------------------------------------------------------------

/// Asynchronous storage backend for test results.
#[async_trait]
pub trait ReportStorage: Send + Sync {
    /// Stores a batch of test results.
    async fn store(&self, results: &[TestCaseResult]) -> Result<()>;

    /// Queries stored results matching the given filter criteria.
    ///
    /// The `filter` map supports keys like `"name"`, `"module"`, `"outcome"`,
    /// `"since"` (ISO-8601 timestamp), and `"run_id"`.
    async fn query(&self, filter: &HashMap<String, String>) -> Result<Vec<TestCaseResult>>;

    /// Returns a description of the storage schema.
    async fn schema_info(&self) -> Result<StorageSchema>;
}

/// Describes the schema of a storage backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSchema {
    /// Backend type name.
    pub backend: String,
    /// Column / field definitions.
    pub columns: Vec<ColumnDef>,
}

/// A single column / field in the storage schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

// ---------------------------------------------------------------------------
// StorageConfig
// ---------------------------------------------------------------------------

/// Unified configuration for selecting and configuring a storage backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Backend type: `"kusto"` or `"file"`.
    pub backend: StorageBackendType,

    /// Kusto cluster URL (only for Kusto backend).
    pub cluster_url: Option<String>,

    /// Database name (only for Kusto backend).
    pub database: Option<String>,

    /// Table name (only for Kusto backend).
    pub table: Option<String>,

    /// Authentication method (only for Kusto backend).
    pub auth_method: Option<AuthMethod>,

    /// Directory for file storage (only for file backend).
    pub directory: Option<PathBuf>,

    /// Output format for file storage.
    pub format: Option<ReportFormat>,
}

/// Supported storage backend types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackendType {
    Kusto,
    File,
}

// ---------------------------------------------------------------------------
// KustoStorage
// ---------------------------------------------------------------------------

/// Authentication credentials for Kusto.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum KustoAuth {
    AppKey {
        client_id: String,
        client_secret: String,
        tenant_id: String,
    },
    ManagedIdentity,
    AzureDefault,
    AzureCli,
    DeviceCode {
        tenant_id: String,
    },
    UserToken {
        token: String,
    },
    AppToken {
        token: String,
    },
}

impl KustoAuth {
    /// Returns the authorization header value for HTTP requests.
    ///
    /// For token-based methods the bearer token is returned directly. For
    /// other methods a placeholder is returned; in production these would
    /// acquire tokens via the Azure SDK.
    fn authorization_header(&self) -> String {
        match self {
            Self::UserToken { token } | Self::AppToken { token } => {
                format!("Bearer {token}")
            }
            _ => {
                // In production, these variants would acquire a token via
                // the Azure Identity SDK.  Here we return a placeholder.
                "Bearer <pending-token-acquisition>".to_owned()
            }
        }
    }
}

/// Azure Data Explorer (Kusto) storage backend.
#[derive(Debug, Clone)]
pub struct KustoStorage {
    /// Cluster URL (e.g. `https://mycluster.kusto.windows.net`).
    pub cluster_url: String,
    /// Database name.
    pub database: String,
    /// Table name for ingesting results.
    pub table: String,
    /// Authentication configuration.
    pub auth: KustoAuth,
    /// HTTP client.
    client: reqwest::Client,
}

impl KustoStorage {
    /// Creates a new Kusto storage backend.
    pub fn new(
        cluster_url: impl Into<String>,
        database: impl Into<String>,
        table: impl Into<String>,
        auth: KustoAuth,
    ) -> Self {
        Self {
            cluster_url: cluster_url.into(),
            database: database.into(),
            table: table.into(),
            auth,
            client: reqwest::Client::new(),
        }
    }

    /// Builds the ingest URL.
    fn ingest_url(&self) -> String {
        format!(
            "{}/v1/rest/ingest/{}/{}",
            self.cluster_url, self.database, self.table
        )
    }

    /// Builds a query URL.
    fn query_url(&self) -> String {
        format!("{}/v2/rest/query", self.cluster_url)
    }

    /// Serializes test results to the Kusto-compatible JSON ingest format.
    ///
    /// Each result becomes one JSON object per line (JSON Lines / NDJSON).
    fn serialize_for_ingest(&self, results: &[TestCaseResult]) -> Result<String> {
        let mut lines = Vec::with_capacity(results.len());
        for result in results {
            let record = serde_json::json!({
                "test_id": result.test_case.id,
                "test_name": result.test_case.name,
                "module": result.test_case.module,
                "outcome": format!("{}", result.outcome),
                "duration_secs": result.duration.as_secs_f64(),
                "message": result.message,
                "started_at": result.started_at.to_rfc3339(),
                "finished_at": result.finished_at.to_rfc3339(),
                "tags": result.test_case.tags,
                "topology": result.test_case.topology.as_ref().map(|t| t.to_string()),
                "platform": result.test_case.platform.as_ref().map(|p| p.to_string()),
            });
            lines.push(serde_json::to_string(&record)?);
        }
        Ok(lines.join("\n"))
    }
}

#[async_trait]
impl ReportStorage for KustoStorage {
    async fn store(&self, results: &[TestCaseResult]) -> Result<()> {
        if results.is_empty() {
            debug!("no results to store");
            return Ok(());
        }

        let body = self.serialize_for_ingest(results)?;
        let url = self.ingest_url();
        let auth = self.auth.authorization_header();

        info!(
            url = %url,
            count = results.len(),
            "ingesting results to Kusto"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| SonicError::Upload {
                destination: url.clone(),
                reason: e.to_string(),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".to_owned());
            return Err(SonicError::Upload {
                destination: url,
                reason: format!("HTTP {status}: {body}"),
            });
        }

        info!(count = results.len(), "ingested results to Kusto");
        Ok(())
    }

    async fn query(&self, filter: &HashMap<String, String>) -> Result<Vec<TestCaseResult>> {
        // Build a KQL query from the filter.
        let mut predicates = Vec::new();

        if let Some(name) = filter.get("name") {
            predicates.push(format!("test_name == '{}'", kql_escape(name)));
        }
        if let Some(module) = filter.get("module") {
            predicates.push(format!("module == '{}'", kql_escape(module)));
        }
        if let Some(outcome) = filter.get("outcome") {
            predicates.push(format!("outcome == '{}'", kql_escape(outcome)));
        }
        if let Some(since) = filter.get("since") {
            predicates.push(format!("started_at >= datetime('{}')", kql_escape(since)));
        }
        if let Some(run_id) = filter.get("run_id") {
            predicates.push(format!("run_id == '{}'", kql_escape(run_id)));
        }

        let where_clause = if predicates.is_empty() {
            String::new()
        } else {
            format!("| where {}", predicates.join(" and "))
        };

        let kql = format!(
            "{} {} | order by started_at desc | limit 1000",
            self.table, where_clause
        );

        let url = self.query_url();
        let auth = self.auth.authorization_header();

        debug!(kql = %kql, "executing Kusto query");

        let request_body = serde_json::json!({
            "db": self.database,
            "csl": kql,
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| SonicError::Other(format!("Kusto query failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".to_owned());
            return Err(SonicError::Other(format!(
                "Kusto query HTTP {status}: {body}"
            )));
        }

        // Parse the Kusto v2 response. The actual row format depends on the
        // table schema; here we parse a simplified JSON response.
        let resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| SonicError::Other(format!("failed to parse Kusto response: {e}")))?;

        let results = parse_kusto_response(&resp_body);
        debug!(count = results.len(), "query returned results");

        Ok(results)
    }

    async fn schema_info(&self) -> Result<StorageSchema> {
        Ok(StorageSchema {
            backend: "kusto".to_owned(),
            columns: vec![
                ColumnDef { name: "test_id".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "test_name".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "module".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "outcome".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "duration_secs".into(), data_type: "real".into(), nullable: false },
                ColumnDef { name: "message".into(), data_type: "string".into(), nullable: true },
                ColumnDef { name: "started_at".into(), data_type: "datetime".into(), nullable: false },
                ColumnDef { name: "finished_at".into(), data_type: "datetime".into(), nullable: false },
                ColumnDef { name: "tags".into(), data_type: "dynamic".into(), nullable: true },
                ColumnDef { name: "topology".into(), data_type: "string".into(), nullable: true },
                ColumnDef { name: "platform".into(), data_type: "string".into(), nullable: true },
                ColumnDef { name: "run_id".into(), data_type: "string".into(), nullable: true },
            ],
        })
    }
}

/// Escapes single quotes for KQL string literals.
fn kql_escape(s: &str) -> String {
    s.replace('\'', "\\'")
}

/// Parses a Kusto v2 query response into TestCaseResult values.
///
/// This is a best-effort parser: missing fields are substituted with defaults.
fn parse_kusto_response(resp: &serde_json::Value) -> Vec<TestCaseResult> {
    let now = chrono::Utc::now();
    let empty_arr = vec![];

    // Kusto v2 responses have frames; the data frame contains a "Rows" array.
    let rows = resp
        .get("Tables")
        .or_else(|| resp.get("frames"))
        .and_then(|t| t.as_array())
        .and_then(|tables| tables.first())
        .and_then(|table| table.get("Rows"))
        .and_then(|r| r.as_array())
        .unwrap_or(&empty_arr);

    rows.iter()
        .filter_map(|row| {
            let arr = row.as_array()?;
            // Expected column order: test_id, test_name, module, outcome,
            // duration_secs, message, started_at, finished_at
            let test_id = arr.first()?.as_str()?.to_owned();
            let test_name = arr.get(1)?.as_str()?.to_owned();
            let module = arr.get(2)?.as_str()?.to_owned();
            let outcome_str = arr.get(3)?.as_str()?;
            let duration = arr.get(4)?.as_f64().unwrap_or(0.0);
            let message = arr.get(5).and_then(|v| v.as_str()).map(|s| s.to_owned());

            let outcome = match outcome_str {
                "PASSED" | "passed" => TestOutcome::Passed,
                "FAILED" | "failed" => TestOutcome::Failed,
                "SKIPPED" | "skipped" => TestOutcome::Skipped,
                "ERROR" | "error" => TestOutcome::Error,
                "XFAIL" | "xfail" => TestOutcome::XFail,
                "XPASS" | "xpass" => TestOutcome::XPass,
                _ => TestOutcome::Error,
            };

            Some(TestCaseResult {
                test_case: sonic_core::TestCase {
                    id: test_id,
                    name: test_name,
                    module,
                    tags: Vec::new(),
                    topology: None,
                    platform: None,
                    description: None,
                    timeout_secs: 0,
                },
                outcome,
                duration: std::time::Duration::from_secs_f64(duration),
                message,
                stdout: None,
                stderr: None,
                started_at: now,
                finished_at: now,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// LocalFileStorage
// ---------------------------------------------------------------------------

/// File-system-based storage backend. Each `store()` call writes results to a
/// timestamped file in the configured directory.
#[derive(Debug, Clone)]
pub struct LocalFileStorage {
    /// Root directory for stored reports.
    pub directory: PathBuf,
    /// Output format (JSON or TOML).
    pub format: ReportFormat,
}

impl LocalFileStorage {
    /// Creates a new file storage backend.
    pub fn new(directory: impl Into<PathBuf>, format: ReportFormat) -> Self {
        Self {
            directory: directory.into(),
            format,
        }
    }

    /// Returns the file extension for the configured format.
    fn extension(&self) -> &str {
        match self.format {
            ReportFormat::Json => "json",
            ReportFormat::Toml => "toml",
            _ => "json",
        }
    }

    /// Generates a unique filename for a new report.
    fn generate_filename(&self) -> PathBuf {
        let ts = Utc::now().format("%Y%m%dT%H%M%S");
        let id = &Uuid::new_v4().to_string()[..8];
        self.directory
            .join(format!("report_{ts}_{id}.{}", self.extension()))
    }
}

/// Serializable wrapper for test results stored on disk.
#[derive(Debug, Serialize, Deserialize)]
struct StoredReport {
    stored_at: String,
    run_id: String,
    results: Vec<StoredResult>,
}

/// Simplified result record for file storage.
#[derive(Debug, Serialize, Deserialize)]
struct StoredResult {
    test_id: String,
    test_name: String,
    module: String,
    outcome: String,
    duration_secs: f64,
    message: Option<String>,
    started_at: String,
    finished_at: String,
    tags: Vec<String>,
}

impl From<&TestCaseResult> for StoredResult {
    fn from(r: &TestCaseResult) -> Self {
        Self {
            test_id: r.test_case.id.clone(),
            test_name: r.test_case.name.clone(),
            module: r.test_case.module.clone(),
            outcome: r.outcome.to_string(),
            duration_secs: r.duration.as_secs_f64(),
            message: r.message.clone(),
            started_at: r.started_at.to_rfc3339(),
            finished_at: r.finished_at.to_rfc3339(),
            tags: r.test_case.tags.clone(),
        }
    }
}

#[async_trait]
impl ReportStorage for LocalFileStorage {
    async fn store(&self, results: &[TestCaseResult]) -> Result<()> {
        if results.is_empty() {
            debug!("no results to store");
            return Ok(());
        }

        std::fs::create_dir_all(&self.directory)?;

        let report = StoredReport {
            stored_at: Utc::now().to_rfc3339(),
            run_id: Uuid::new_v4().to_string(),
            results: results.iter().map(StoredResult::from).collect(),
        };

        let path = self.generate_filename();
        let content = match self.format {
            ReportFormat::Toml => toml::to_string_pretty(&report)?,
            _ => serde_json::to_string_pretty(&report)?,
        };

        std::fs::write(&path, &content)?;
        info!(
            path = %path.display(),
            count = results.len(),
            "stored results to file"
        );

        Ok(())
    }

    async fn query(&self, filter: &HashMap<String, String>) -> Result<Vec<TestCaseResult>> {
        let mut all_results = Vec::new();

        if !self.directory.exists() {
            return Ok(all_results);
        }

        let entries = std::fs::read_dir(&self.directory)?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");

            if ext != "json" && ext != "toml" {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping unreadable file");
                    continue;
                }
            };

            let report: StoredReport = match ext {
                "toml" => match toml::from_str(&content) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "skipping malformed TOML");
                        continue;
                    }
                },
                _ => match serde_json::from_str(&content) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "skipping malformed JSON");
                        continue;
                    }
                },
            };

            for stored in &report.results {
                // Apply filters.
                if let Some(name) = filter.get("name") {
                    if !stored.test_name.contains(name.as_str()) {
                        continue;
                    }
                }
                if let Some(module) = filter.get("module") {
                    if stored.module != *module {
                        continue;
                    }
                }
                if let Some(outcome) = filter.get("outcome") {
                    if stored.outcome.to_lowercase() != outcome.to_lowercase() {
                        continue;
                    }
                }

                let outcome = match stored.outcome.as_str() {
                    "PASSED" => TestOutcome::Passed,
                    "FAILED" => TestOutcome::Failed,
                    "SKIPPED" => TestOutcome::Skipped,
                    "ERROR" => TestOutcome::Error,
                    "XFAIL" => TestOutcome::XFail,
                    "XPASS" => TestOutcome::XPass,
                    _ => TestOutcome::Error,
                };

                let started_at = chrono::DateTime::parse_from_rfc3339(&stored.started_at)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let finished_at = chrono::DateTime::parse_from_rfc3339(&stored.finished_at)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                all_results.push(TestCaseResult {
                    test_case: sonic_core::TestCase {
                        id: stored.test_id.clone(),
                        name: stored.test_name.clone(),
                        module: stored.module.clone(),
                        tags: stored.tags.clone(),
                        topology: None,
                        platform: None,
                        description: None,
                        timeout_secs: 0,
                    },
                    outcome,
                    duration: std::time::Duration::from_secs_f64(stored.duration_secs),
                    message: stored.message.clone(),
                    stdout: None,
                    stderr: None,
                    started_at,
                    finished_at,
                });
            }
        }

        debug!(count = all_results.len(), "queried local file storage");
        Ok(all_results)
    }

    async fn schema_info(&self) -> Result<StorageSchema> {
        Ok(StorageSchema {
            backend: "file".to_owned(),
            columns: vec![
                ColumnDef { name: "test_id".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "test_name".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "module".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "outcome".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "duration_secs".into(), data_type: "float".into(), nullable: false },
                ColumnDef { name: "message".into(), data_type: "string".into(), nullable: true },
                ColumnDef { name: "started_at".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "finished_at".into(), data_type: "string".into(), nullable: false },
                ColumnDef { name: "tags".into(), data_type: "array".into(), nullable: true },
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_core::{TestCase, TestOutcome};
    use std::time::Duration;

    fn sample_results() -> Vec<TestCaseResult> {
        let now = Utc::now();
        vec![
            TestCaseResult {
                test_case: TestCase {
                    id: "bgp::test_convergence".into(),
                    name: "test_convergence".into(),
                    module: "bgp".into(),
                    tags: vec!["bgp".into(), "smoke".into()],
                    topology: None,
                    platform: None,
                    description: None,
                    timeout_secs: 60,
                },
                outcome: TestOutcome::Passed,
                duration: Duration::from_millis(3200),
                message: None,
                stdout: None,
                stderr: None,
                started_at: now,
                finished_at: now,
            },
            TestCaseResult {
                test_case: TestCase {
                    id: "acl::test_deny".into(),
                    name: "test_deny".into(),
                    module: "acl".into(),
                    tags: vec!["acl".into()],
                    topology: None,
                    platform: None,
                    description: None,
                    timeout_secs: 60,
                },
                outcome: TestOutcome::Failed,
                duration: Duration::from_millis(1500),
                message: Some("ACL rule not applied".into()),
                stdout: None,
                stderr: None,
                started_at: now,
                finished_at: now,
            },
        ]
    }

    #[tokio::test]
    async fn local_file_storage_roundtrip_json() {
        let dir = tempfile::tempdir().unwrap();
        let storage = LocalFileStorage::new(dir.path(), ReportFormat::Json);

        let results = sample_results();
        storage.store(&results).await.unwrap();

        // Query all.
        let queried = storage.query(&HashMap::new()).await.unwrap();
        assert_eq!(queried.len(), 2);
    }

    #[tokio::test]
    async fn local_file_storage_roundtrip_toml() {
        let dir = tempfile::tempdir().unwrap();
        let storage = LocalFileStorage::new(dir.path(), ReportFormat::Toml);

        let results = sample_results();
        storage.store(&results).await.unwrap();

        let queried = storage.query(&HashMap::new()).await.unwrap();
        assert_eq!(queried.len(), 2);
    }

    #[tokio::test]
    async fn local_file_storage_query_by_module() {
        let dir = tempfile::tempdir().unwrap();
        let storage = LocalFileStorage::new(dir.path(), ReportFormat::Json);

        storage.store(&sample_results()).await.unwrap();

        let mut filter = HashMap::new();
        filter.insert("module".into(), "bgp".into());
        let queried = storage.query(&filter).await.unwrap();
        assert_eq!(queried.len(), 1);
        assert_eq!(queried[0].test_case.module, "bgp");
    }

    #[tokio::test]
    async fn local_file_storage_query_by_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let storage = LocalFileStorage::new(dir.path(), ReportFormat::Json);

        storage.store(&sample_results()).await.unwrap();

        let mut filter = HashMap::new();
        filter.insert("outcome".into(), "failed".into());
        let queried = storage.query(&filter).await.unwrap();
        assert_eq!(queried.len(), 1);
        assert_eq!(queried[0].outcome, TestOutcome::Failed);
    }

    #[tokio::test]
    async fn local_file_storage_schema() {
        let storage = LocalFileStorage::new("/tmp", ReportFormat::Json);
        let schema = storage.schema_info().await.unwrap();
        assert_eq!(schema.backend, "file");
        assert!(!schema.columns.is_empty());
    }

    #[tokio::test]
    async fn local_file_storage_empty_results() {
        let dir = tempfile::tempdir().unwrap();
        let storage = LocalFileStorage::new(dir.path(), ReportFormat::Json);
        storage.store(&[]).await.unwrap();

        // No files should be created.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .collect();
        assert!(entries.is_empty());
    }
}
