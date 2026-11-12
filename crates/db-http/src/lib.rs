//! HTTP/SQL and search-service connectors (Phase 5).
//!
//! This crate implements on-the-wire REST/JSON adapters for:
//! * Search services: Elasticsearch / Easysearch, Meilisearch, Manticore Search.
//! * HTTP‑SQL: Cloudflare D1 (the Cloudflare `/query` HTTP API).
//!
//! Every connector is stateless and derives its endpoint configuration from a
//! [`ConnectionProfile`] at call time, following the workspace's adapter
//! pattern (see `db-tdengine`).

use async_trait::async_trait;
use base64::engine::Engine;
use db_common::{
    AppError, CellValue, Column, ConnectionProfile, ConnectionTestReport, ConnectionTester,
    DataSourceKind, DatabaseObject, DatabaseObjectKind, ErrorKind, HttpSqlConnector, QueryResult,
    Result, Row, SearchConnector, SearchIndex, SearchResult, SslMode, TableColumn,
};
use reqwest::Method;
use std::collections::BTreeMap;
use std::time::Duration;

pub const ADAPTER_NAME: &str = "http";

/// How a request should authenticate. Variants map directly onto reqwest
/// builder methods so each connector can share the same transport helper.
#[derive(Debug, Clone)]
enum Auth<'a> {
    Basic(&'a str, Option<&'a str>),
    Bearer(&'a str),
    /// Raw `Authorization` header value (already-formatted, e.g. ES `ApiKey ...`).
    ApiKey(String),
}

/// A fully-specified outbound HTTP request. Kept small so a single shared
/// [`make_request`] can service every connector.
#[derive(Debug, Clone)]
struct HttpRequest<'a> {
    method: Method,
    url: String,
    auth: Auth<'a>,
    /// Optional `Content-Type` header value.
    content_type: Option<&'a str>,
    /// Optional raw body.
    body: Option<String>,
    /// Label embedded in the `Connection` error message on failure.
    context: &'a str,
}

impl<'a> HttpRequest<'a> {
    fn new(method: Method, url: String, auth: Auth<'a>, context: &'a str) -> Self {
        Self {
            method,
            url,
            auth,
            content_type: None,
            body: None,
            context,
        }
    }
}

/// Shared transport helper: reuse a pooled client, apply per-request auth / body,
/// send, and return the parsed JSON after checking the HTTP status code.
async fn make_request(req: HttpRequest<'_>) -> Result<serde_json::Value> {
    static CLIENT: std::sync::OnceLock<Result<reqwest::Client>> = std::sync::OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                // `connect_timeout` does not cover a server that accepts the TCP
                // connection but never sends a response. Without a total deadline the
                // connection dialog remains busy indefinitely and its actions stay
                // disabled.
                .timeout(Duration::from_secs(10))
                .user_agent(format!("orvyn/{ADAPTER_NAME}"))
                .build()
                .map_err(|error| {
                    AppError::new(ErrorKind::Connection, "failed to build HTTP client")
                        .with_detail(error.to_string())
                })
        })
        .as_ref()
        .map_err(Clone::clone)?;

    let mut builder = client.request(req.method, &req.url);
    match req.auth {
        Auth::Basic(username, password) => {
            builder = builder.basic_auth(username, password.filter(|value| !value.is_empty()));
        }
        Auth::Bearer(token) => {
            builder = builder.bearer_auth(token);
        }
        Auth::ApiKey(header) => {
            builder = builder.header("Authorization", header);
        }
    }
    if let Some(content_type) = req.content_type {
        builder = builder.header("Content-Type", content_type);
    }
    if let Some(body) = &req.body {
        builder = builder.body(body.clone());
    }

    let response = builder.send().await.map_err(|error| {
        AppError::new(ErrorKind::Connection, req.context).with_detail(error.to_string())
    })?;

    let status = response.status();
    let text = response.text().await.map_err(|error| {
        AppError::new(
            ErrorKind::Connection,
            format!("{}: failed to read body", req.context),
        )
        .with_detail(error.to_string())
    })?;

    if !status.is_success() {
        return Err(http_response_error(req.context, status.as_u16(), &text));
    }

    serde_json::from_str(&text).map_err(|error| {
        AppError::new(
            ErrorKind::Serialization,
            format!("{}: failed to parse JSON response", req.context),
        )
        .with_detail(error.to_string())
    })
}

/// Turns Elasticsearch-style structured error payloads into a concise
/// diagnostic while retaining the original response for unknown formats.
fn http_response_error(context: &str, status: u16, text: &str) -> AppError {
    let kind = match status {
        401 => ErrorKind::Authentication,
        403 => ErrorKind::Permission,
        400..=499 => ErrorKind::Query,
        _ => ErrorKind::Connection,
    };
    let parsed = serde_json::from_str::<serde_json::Value>(text).ok();
    let error = parsed.as_ref().and_then(|json| json.get("error"));
    let object = error.and_then(|value| value.as_object());
    let code = object
        .and_then(|value| value.get("type"))
        .and_then(|value| value.as_str());
    let reason = object
        .and_then(|value| value.get("reason"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|json| json.get("message"))
                .and_then(|value| value.as_str())
        });

    let message = match reason {
        Some(reason) => format!("{context}: {reason}"),
        None => format!("{context}: HTTP {status}"),
    };
    let mut detail = format!("HTTP {status}");
    if let Some(object) = object {
        let line = object.get("line").and_then(|value| value.as_u64());
        let column = object
            .get("col")
            .or_else(|| object.get("column"))
            .and_then(|value| value.as_u64());
        if let (Some(line), Some(column)) = (line, column) {
            detail.push_str(&format!("; location: line {line}, column {column}"));
        }
        if let Some(cause) = object
            .get("root_cause")
            .and_then(|value| value.as_array())
            .and_then(|causes| causes.first())
            .and_then(|cause| cause.get("reason"))
            .and_then(|value| value.as_str())
        {
            detail.push_str(&format!("; root cause: {cause}"));
        }
    }
    if parsed.is_none() {
        detail.push_str(&format!("; response: {text}"));
    }

    let error = AppError::new(kind, message).with_detail(detail);
    match code {
        Some(code) => error.with_code(code),
        None => error,
    }
}

/// Map a JSON value into a [`CellValue`] following D1's type coercion rules.
fn json_to_cell(value: Option<&serde_json::Value>) -> CellValue {
    match value {
        None => CellValue::Null,
        Some(serde_json::Value::Null) => CellValue::Null,
        Some(serde_json::Value::Bool(b)) => CellValue::Bool(*b),
        Some(serde_json::Value::Number(number)) => {
            if let Some(integer) = number.as_i64() {
                CellValue::Int(integer)
            } else {
                CellValue::Float(number.as_f64().unwrap_or(0.0))
            }
        }
        Some(serde_json::Value::String(s)) => CellValue::Text(s.clone()),
        Some(other) => CellValue::Text(other.to_string()),
    }
}

/// Build a [`QueryResult`] from D1-style row objects (`[{col: val, ...}]`).
/// Column names are the union of keys across all rows, ordered by first
/// appearance per the connector spec.
fn rows_to_query_result(statement: &str, rows: &[serde_json::Value]) -> QueryResult {
    let mut names: Vec<String> = Vec::new();
    for row in rows {
        if let Some(object) = row.as_object() {
            for key in object.keys() {
                if !names.iter().any(|existing| existing == key) {
                    names.push(key.clone());
                }
            }
        }
    }

    let columns: Vec<Column> = names
        .iter()
        .map(|name| Column::new(name.clone(), "TEXT"))
        .collect();

    let mapped_rows: Vec<Row> = rows
        .iter()
        .map(|row| {
            let values = names
                .iter()
                .map(|name| json_to_cell(row.get(name)))
                .collect();
            Row::new(values)
        })
        .collect();

    QueryResult {
        statement: statement.to_string(),
        columns,
        rows: mapped_rows,
        affected_rows: 0,
        execution_time_ms: 0,
        notices: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

/// Pull the plain-text content of the first column of every row out of a
/// [`QueryResult`], used by the D1 metadata helpers.
fn extract_first_column(result: &QueryResult) -> Vec<String> {
    result
        .rows
        .iter()
        .filter_map(|row| row.values.first())
        .filter_map(|cell| cell.as_text().map(ToOwned::to_owned))
        .collect()
}

// ===========================================================================
// Elasticsearch / Easysearch
// ===========================================================================

/// Build an ES authentication choice. If both `es_api_id` and `es_api_key`
/// extra params are present, use an `ApiKey` header (base64 of `id:key`);
/// otherwise fall back to basic auth.
fn elasticsearch_auth<'a>(profile: &'a ConnectionProfile, password: Option<&'a str>) -> Auth<'a> {
    let id = profile.extra_param("es_api_id");
    let key = profile.extra_param("es_api_key");
    match (id, key) {
        (Some(id), Some(key)) => {
            use base64::engine::general_purpose::STANDARD;
            let encoded = STANDARD.encode(format!("{id}:{key}").as_bytes());
            Auth::ApiKey(format!("ApiKey {encoded}"))
        }
        _ => Auth::Basic(profile.username.as_deref().unwrap_or_default(), password),
    }
}

/// Return the HTTP origin for the profile. All HTTP-backed data sources use
/// the same SSL policy so a saved security setting has consistent behavior.
fn http_origin(profile: &ConnectionProfile) -> Result<String> {
    let host = profile.required_host()?;
    let port = profile.required_port()?;
    let scheme = match profile.security.ssl_mode {
        SslMode::Disable => "http",
        SslMode::Prefer | SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => "https",
    };
    Ok(format!("{scheme}://{host}:{port}"))
}

/// Compute the Elasticsearch base URL, honoring the optional `http_path` prefix.
fn elasticsearch_base_url(profile: &ConnectionProfile) -> Result<String> {
    let origin = http_origin(profile)?;
    match profile.extra_param("http_path") {
        Some(path) if !path.trim().is_empty() => {
            let prefix = path.trim_matches('/');
            if prefix.is_empty() {
                Ok(format!("{origin}/"))
            } else {
                Ok(format!("{origin}/{prefix}/"))
            }
        }
        _ => Ok(format!("{origin}/")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elasticsearch_url_honors_ssl_mode_and_path() {
        let profile = ConnectionProfile::builder(DataSourceKind::Elasticsearch, "es")
            .host("search.example.test")
            .port(9200)
            .ssl_mode(SslMode::Require)
            .extra_param("http_path", "/elastic/")
            .build()
            .expect("valid Elasticsearch profile");

        assert_eq!(
            elasticsearch_base_url(&profile).expect("valid URL"),
            "https://search.example.test:9200/elastic/"
        );
    }

    #[test]
    fn elasticsearch_url_uses_http_when_ssl_is_disabled() {
        let profile = ConnectionProfile::builder(DataSourceKind::Elasticsearch, "es")
            .host("localhost")
            .port(9200)
            .ssl_mode(SslMode::Disable)
            .build()
            .expect("valid Elasticsearch profile");

        assert_eq!(
            elasticsearch_base_url(&profile).expect("valid URL"),
            "http://localhost:9200/"
        );
    }

    #[test]
    fn elasticsearch_url_collapses_root_http_path() {
        let profile = ConnectionProfile::builder(DataSourceKind::Elasticsearch, "es")
            .host("10.0.0.1")
            .port(9200)
            .ssl_mode(SslMode::Disable)
            .extra_param("http_path", "/")
            .build()
            .expect("valid Elasticsearch profile");

        assert_eq!(
            elasticsearch_base_url(&profile).expect("valid URL"),
            "http://10.0.0.1:9200/"
        );
    }

    #[test]
    fn elasticsearch_error_reports_code_reason_and_location() {
        let error = http_response_error(
            "failed to run Elasticsearch search",
            400,
            r#"{"error":{"type":"parsing_exception","reason":"unexpected token","line":2,"col":14,"root_cause":[{"reason":"expected END_OBJECT"}]},"status":400}"#,
        );

        assert_eq!(error.kind, ErrorKind::Query);
        assert_eq!(error.code.as_deref(), Some("parsing_exception"));
        assert_eq!(
            error.message,
            "failed to run Elasticsearch search: unexpected token"
        );
        assert_eq!(
            error.detail.as_deref(),
            Some("HTTP 400; location: line 2, column 14; root cause: expected END_OBJECT")
        );
    }
}

/// Shared Elasticsearch / Easysearch search connector. Backed by the same HTTP
/// `/_search` + `/_cat` API surface; a `brand` marker lets callers express the
/// Easysearch flavour while defaulting to Elasticsearch.
#[derive(Debug, Clone, Copy)]
pub struct ElasticsearchSearchConnector {
    brand: DataSourceKind,
}

impl Default for ElasticsearchSearchConnector {
    fn default() -> Self {
        Self {
            brand: DataSourceKind::Elasticsearch,
        }
    }
}

impl ElasticsearchSearchConnector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn easysearch() -> Self {
        Self {
            brand: DataSourceKind::Easysearch,
        }
    }
}

#[async_trait]
impl SearchConnector for ElasticsearchSearchConnector {
    fn kind(&self) -> DataSourceKind {
        self.brand
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<()> {
        profile.validate()?;
        let base = elasticsearch_base_url(profile)?;
        let auth = elasticsearch_auth(profile, password);
        make_request(HttpRequest::new(
            Method::GET,
            base.to_string(),
            auth,
            "failed to reach Elasticsearch",
        ))
        .await?;
        Ok(())
    }

    async fn list_indices(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<SearchIndex>> {
        let base = elasticsearch_base_url(profile)?;
        let auth = elasticsearch_auth(profile, password);
        let json = make_request(HttpRequest::new(
            Method::GET,
            format!("{base}_cat/indices?format=json&h=index,status,docs.count"),
            auth,
            "failed to list Elasticsearch indices",
        ))
        .await?;

        let mut indices = Vec::new();
        if let Some(array) = json.as_array() {
            for object in array {
                if let Some(obj) = object.as_object() {
                    let name = obj
                        .get("index")
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned)
                        .unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    let document_count = obj
                        .get("docs.count")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<u64>().ok());
                    let health = obj
                        .get("status")
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned);
                    indices.push(SearchIndex {
                        name,
                        document_count,
                        health,
                    });
                }
            }
        }
        Ok(indices)
    }

    async fn get_mapping(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
    ) -> Result<serde_json::Value> {
        let base = elasticsearch_base_url(profile)?;
        let auth = elasticsearch_auth(profile, password);
        make_request(HttpRequest::new(
            Method::GET,
            format!("{base}{index}/_mapping"),
            auth,
            "failed to fetch Elasticsearch mapping",
        ))
        .await
    }

    async fn search(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
        query: &serde_json::Value,
        limit: u32,
    ) -> Result<SearchResult> {
        let base = elasticsearch_base_url(profile)?;
        let auth = elasticsearch_auth(profile, password);
        let body = serde_json::to_string(query).map_err(|error| {
            AppError::new(
                ErrorKind::Serialization,
                "failed to serialize Elasticsearch query",
            )
            .with_detail(error.to_string())
        })?;
        let mut req = HttpRequest::new(
            Method::POST,
            format!("{base}{index}/_search"),
            auth,
            "failed to run Elasticsearch search",
        );
        req.content_type = Some("application/json");
        req.body = Some(body);

        let json = make_request(req).await?;

        let total = json
            .get("hits")
            .and_then(|hits| hits.get("total"))
            .and_then(|total| {
                total
                    .as_u64()
                    .or_else(|| total.get("value").and_then(|v| v.as_u64()))
            });

        let mut documents = Vec::new();
        if let Some(hits) = json
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
        {
            for hit in hits {
                if documents.len() >= limit as usize {
                    break;
                }
                if let Some(source) = hit.get("_source") {
                    documents.push(source.clone());
                } else {
                    documents.push(hit.clone());
                }
            }
        }

        Ok(SearchResult {
            total,
            documents,
            raw_response: Some(json),
        })
    }
}

/// Connectivity probe for Elasticsearch / Easysearch.
#[derive(Debug, Clone, Copy, Default)]
pub struct ElasticsearchConnectionTester;

#[async_trait]
impl ConnectionTester for ElasticsearchConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::Elasticsearch
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started_at = std::time::Instant::now();
        let base = elasticsearch_base_url(profile)?;
        let auth = elasticsearch_auth(profile, password);
        let json = make_request(HttpRequest::new(
            Method::GET,
            base.to_string(),
            auth,
            "failed to reach Elasticsearch",
        ))
        .await?;

        let version = json
            .get("version")
            .and_then(|v| v.get("number"))
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);

        let mut report = ConnectionTestReport::success(
            DataSourceKind::Elasticsearch,
            started_at.elapsed().as_millis(),
        );
        if let Some(version) = version {
            report = report.with_server_version(version);
        }
        Ok(report)
    }
}

// ===========================================================================
// Meilisearch
// ===========================================================================

fn meilisearch_auth<'a>(profile: &'a ConnectionProfile, password: Option<&'a str>) -> Auth<'a> {
    match profile.extra_param("meili_api_key") {
        Some(key) if !key.is_empty() => Auth::Bearer(key),
        _ => Auth::Basic(profile.username.as_deref().unwrap_or_default(), password),
    }
}

/// Meilisearch search connector.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeilisearchSearchConnector;

#[async_trait]
impl SearchConnector for MeilisearchSearchConnector {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::Meilisearch
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<()> {
        profile.validate()?;
        let origin = http_origin(profile)?;
        let auth = meilisearch_auth(profile, password);
        let json = make_request(HttpRequest::new(
            Method::GET,
            format!("{origin}/health"),
            auth,
            "failed to reach Meilisearch",
        ))
        .await?;
        // Consider a reached endpoint successful even if health fields vary.
        let _status = json.get("status").and_then(|v| v.as_str());
        Ok(())
    }

    async fn list_indices(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<SearchIndex>> {
        let origin = http_origin(profile)?;
        let auth = meilisearch_auth(profile, password);
        let json = make_request(HttpRequest::new(
            Method::GET,
            format!("{origin}/indexes"),
            auth,
            "failed to list Meilisearch indexes",
        ))
        .await?;

        let mut indices = Vec::new();
        if let Some(array) = json.as_array() {
            for object in array {
                if let Some(obj) = object.as_object() {
                    let name = obj
                        .get("uid")
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned)
                        .unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    let document_count = obj.get("numberOfDocuments").and_then(|v| v.as_u64());
                    indices.push(SearchIndex {
                        name,
                        document_count,
                        health: None,
                    });
                }
            }
        }
        Ok(indices)
    }

    async fn get_mapping(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
    ) -> Result<serde_json::Value> {
        let origin = http_origin(profile)?;
        let auth = meilisearch_auth(profile, password);
        make_request(HttpRequest::new(
            Method::GET,
            format!("{origin}/indexes/{index}"),
            auth,
            "failed to fetch Meilisearch index settings",
        ))
        .await
    }

    async fn search(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
        query: &serde_json::Value,
        limit: u32,
    ) -> Result<SearchResult> {
        let origin = http_origin(profile)?;
        let auth = meilisearch_auth(profile, password);

        let mut body = query.clone();
        if let Some(obj) = body.as_object_mut() {
            if !obj.contains_key("limit") {
                obj.insert("limit".to_string(), serde_json::json!(limit));
            }
        }
        let body_json = serde_json::to_string(&body).map_err(|error| {
            AppError::new(
                ErrorKind::Serialization,
                "failed to serialize Meilisearch query",
            )
            .with_detail(error.to_string())
        })?;

        let mut req = HttpRequest::new(
            Method::POST,
            format!("{origin}/indexes/{index}/search"),
            auth,
            "failed to run Meilisearch search",
        );
        req.content_type = Some("application/json");
        req.body = Some(body_json);

        let json = make_request(req).await?;

        let total = json.get("estimatedTotalHits").and_then(|v| v.as_u64());
        let mut documents = Vec::new();
        if let Some(hits) = json.get("hits").and_then(|v| v.as_array()) {
            for hit in hits {
                if documents.len() >= limit as usize {
                    break;
                }
                documents.push(hit.clone());
            }
        }

        Ok(SearchResult {
            total,
            documents,
            raw_response: Some(json),
        })
    }
}

/// Connectivity probe for Meilisearch.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeilisearchConnectionTester;

#[async_trait]
impl ConnectionTester for MeilisearchConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::Meilisearch
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started_at = std::time::Instant::now();
        let origin = http_origin(profile)?;
        let auth = meilisearch_auth(profile, password);
        make_request(HttpRequest::new(
            Method::GET,
            format!("{origin}/health"),
            auth,
            "failed to reach Meilisearch",
        ))
        .await?;
        Ok(ConnectionTestReport::success(
            DataSourceKind::Meilisearch,
            started_at.elapsed().as_millis(),
        ))
    }
}

// ===========================================================================
// Manticore Search
// ===========================================================================

/// Manticore Search connector using its HTTP JSON API (`/sql` for DDL/DML and
/// metadata, `/search` for full-text queries).
#[derive(Debug, Clone, Copy, Default)]
pub struct ManticoreSearchConnector;

async fn manticore_sql(
    profile: &ConnectionProfile,
    password: Option<&str>,
    sql: &str,
) -> Result<serde_json::Value> {
    let origin = http_origin(profile)?;
    let username = profile.username.as_deref().unwrap_or_default();
    let mut req = HttpRequest::new(
        Method::POST,
        format!("{origin}/sql"),
        Auth::Basic(username, password),
        "failed to run Manticore SQL",
    );
    req.content_type = Some("text/plain");
    req.body = Some(sql.to_string());
    make_request(req).await
}

#[async_trait]
impl SearchConnector for ManticoreSearchConnector {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::ManticoreSearch
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<()> {
        profile.validate()?;
        manticore_sql(profile, password, "SELECT 1").await?;
        Ok(())
    }

    async fn list_indices(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<SearchIndex>> {
        let json = manticore_sql(profile, password, "SHOW TABLES").await?;

        let mut indices = Vec::new();
        if let Some(rows) = json.get("data").and_then(|v| v.as_array()) {
            for row in rows {
                if let Some(obj) = row.as_object() {
                    let name = obj
                        .get("Table")
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned)
                        .unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    // `catalog` / `Found` carry doc-count hints when present.
                    let document_count =
                        obj.get("Found")
                            .or_else(|| obj.get("catalog"))
                            .and_then(|v| match v {
                                serde_json::Value::Number(n) => n.as_u64(),
                                serde_json::Value::String(s) => s.parse::<u64>().ok(),
                                _ => None,
                            });
                    indices.push(SearchIndex {
                        name,
                        document_count,
                        health: None,
                    });
                }
            }
        }
        Ok(indices)
    }

    async fn get_mapping(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
    ) -> Result<serde_json::Value> {
        manticore_sql(profile, password, &format!("SHOW CREATE TABLE {index}")).await
    }

    async fn search(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
        query: &serde_json::Value,
        limit: u32,
    ) -> Result<SearchResult> {
        let origin = http_origin(profile)?;
        let username = profile.username.as_deref().unwrap_or_default();

        let search_body = match query {
            serde_json::Value::String(term) => serde_json::json!({
                "index": index,
                "query": { "query_string": term }
            }),
            serde_json::Value::Object(object) => {
                let nested = ["query", "query_string", "q"]
                    .iter()
                    .find_map(|key| object.get(*key))
                    .cloned();
                match nested {
                    Some(query_value) => {
                        serde_json::json!({ "index": index, "query": query_value })
                    }
                    None => serde_json::json!({ "index": index, "query": query }),
                }
            }
            other => serde_json::json!({ "index": index, "query": other }),
        };

        let body = serde_json::to_string(&search_body).map_err(|error| {
            AppError::new(
                ErrorKind::Serialization,
                "failed to serialize Manticore query",
            )
            .with_detail(error.to_string())
        })?;

        let mut req = HttpRequest::new(
            Method::POST,
            format!("{origin}/search"),
            Auth::Basic(username, password),
            "failed to run Manticore search",
        );
        req.content_type = Some("application/json");
        req.body = Some(body);

        let json = make_request(req).await?;

        let hits_len = json
            .get("hits")
            .and_then(|v| v.as_array())
            .map(|v| v.len() as u64);
        let total = json.get("total").and_then(|v| v.as_u64()).or(hits_len);

        let mut documents = Vec::new();
        if let Some(hits) = json.get("hits").and_then(|v| v.as_array()) {
            for hit in hits {
                if documents.len() >= limit as usize {
                    break;
                }
                documents.push(hit.clone());
            }
        }

        Ok(SearchResult {
            total,
            documents,
            raw_response: Some(json),
        })
    }
}

/// Connectivity probe for Manticore Search.
#[derive(Debug, Clone, Copy, Default)]
pub struct ManticoreConnectionTester;

#[async_trait]
impl ConnectionTester for ManticoreConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::ManticoreSearch
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started_at = std::time::Instant::now();
        manticore_sql(profile, password, "SELECT 1").await?;
        Ok(ConnectionTestReport::success(
            DataSourceKind::ManticoreSearch,
            started_at.elapsed().as_millis(),
        ))
    }
}

// ===========================================================================
// Cloudflare D1 (HTTP-SQL)
// ===========================================================================

/// Resolve the Cloudflare D1 HTTP `/query` endpoint from profile extra params.
fn d1_base_url(profile: &ConnectionProfile) -> Result<String> {
    let account = profile
        .extra_param("cf_account")
        .ok_or_else(|| AppError::validation("missing cf_account"))?;
    let database_id = profile
        .extra_param("cf_database_id")
        .ok_or_else(|| AppError::validation("missing cf_database_id"))?;
    Ok(format!(
        "https://api.cloudflare.com/client/v4/accounts/{account}/d1/database/{database_id}/query"
    ))
}

fn d1_auth<'a>(profile: &'a ConnectionProfile) -> Result<Auth<'a>> {
    let token = profile
        .extra_param("cf_token")
        .ok_or_else(|| AppError::validation("missing cf_token"))?;
    Ok(Auth::Bearer(token))
}

/// Cloudflare D1 connector over the HTTP `/query` API.
#[derive(Debug, Clone, Copy, Default)]
pub struct CloudflareD1HttpConnector;

/// Connectivity probe for Cloudflare D1.
#[derive(Debug, Clone, Copy, Default)]
pub struct CloudflareD1ConnectionTester;

#[async_trait]
impl ConnectionTester for CloudflareD1ConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::CloudflareD1
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        let _ = password;
        profile.validate()?;
        let started_at = std::time::Instant::now();
        let base = d1_base_url(profile)?;
        let auth = d1_auth(profile)?;

        let body = serde_json::json!({
            "sql": "SELECT 1",
            "params": [],
            "namedParams": {}
        });
        let body_json = serde_json::to_string(&body).map_err(|error| {
            AppError::new(ErrorKind::Serialization, "failed to serialize D1 probe")
                .with_detail(error.to_string())
        })?;

        let mut req = HttpRequest::new(Method::POST, base, auth, "failed to reach Cloudflare D1");
        req.content_type = Some("application/json");
        req.body = Some(body_json);

        let json = make_request(req).await?;

        if json.get("success").and_then(|v| v.as_bool()) != Some(true) {
            let message = json
                .get("errors")
                .and_then(|e| e.as_array())
                .and_then(|errors| errors.first())
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown Cloudflare D1 error");
            return Err(AppError::new(
                ErrorKind::Connection,
                format!("Cloudflare D1 error: {message}"),
            ));
        }
        if json
            .get("result")
            .and_then(|r| r.as_array())
            .and_then(|results| results.first())
            .and_then(|r| r.get("success"))
            .and_then(|v| v.as_bool())
            != Some(true)
        {
            return Err(AppError::new(
                ErrorKind::Connection,
                "Cloudflare D1 probe did not report success",
            ));
        }

        let report = ConnectionTestReport::success(
            DataSourceKind::CloudflareD1,
            started_at.elapsed().as_millis(),
        );
        Ok(report.with_server_version("Cloudflare D1"))
    }
}

#[async_trait]
impl HttpSqlConnector for CloudflareD1HttpConnector {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::CloudflareD1
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<()> {
        let _ = password;
        profile.validate()?;
        let base = d1_base_url(profile)?;
        let auth = d1_auth(profile)?;

        let body = serde_json::json!({
            "sql": "SELECT 1",
            "params": [],
            "namedParams": {}
        });
        let body_json = serde_json::to_string(&body).map_err(|error| {
            AppError::new(ErrorKind::Serialization, "failed to serialize D1 probe")
                .with_detail(error.to_string())
        })?;

        let mut req = HttpRequest::new(Method::POST, base, auth, "failed to reach Cloudflare D1");
        req.content_type = Some("application/json");
        req.body = Some(body_json);

        let json = make_request(req).await?;
        if json.get("success").and_then(|v| v.as_bool()) != Some(true) {
            let message = json
                .get("errors")
                .and_then(|e| e.as_array())
                .and_then(|errors| errors.first())
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown Cloudflare D1 error");
            return Err(AppError::new(
                ErrorKind::Connection,
                format!("Cloudflare D1 error: {message}"),
            ));
        }
        Ok(())
    }

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let _ = password;
        profile.validate()?;
        let base = d1_base_url(profile)?;
        let auth = d1_auth(profile)?;

        let body = serde_json::json!({
            "sql": sql,
            "params": [],
            "namedParams": {}
        });
        let body_json = serde_json::to_string(&body).map_err(|error| {
            AppError::new(ErrorKind::Serialization, "failed to serialize D1 statement")
                .with_detail(error.to_string())
        })?;

        let mut req = HttpRequest::new(
            Method::POST,
            base,
            auth,
            "failed to execute Cloudflare D1 SQL",
        );
        req.content_type = Some("application/json");
        req.body = Some(body_json);

        let json = make_request(req).await?;

        if json.get("success").and_then(|v| v.as_bool()) != Some(true) {
            let message = json
                .get("errors")
                .and_then(|e| e.as_array())
                .and_then(|errors| errors.first())
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown Cloudflare D1 error");
            return Err(AppError::new(
                ErrorKind::Connection,
                format!("Cloudflare D1 error: {message}"),
            ));
        }

        let result = json
            .get("result")
            .and_then(|r| r.as_array())
            .and_then(|results| results.first())
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let rows = result
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();

        Ok(vec![rows_to_query_result(sql, &rows)])
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let _ = password;
        let name = profile
            .database
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("d1")
            .to_string();
        Ok(vec![DatabaseObject {
            kind: DatabaseObjectKind::Database,
            name,
            schema: None,
            parent: None,
        }])
    }

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let _ = database;
        let results = self
            .execute(
                profile,
                password,
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .await?;
        let names = results
            .first()
            .map(extract_first_column)
            .unwrap_or_default();
        Ok(names
            .into_iter()
            .map(|name| DatabaseObject {
                kind: DatabaseObjectKind::Table,
                name,
                schema: None,
                parent: None,
            })
            .collect())
    }

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let results = self
            .execute(
                profile,
                password,
                &format!("PRAGMA table_info(\"{table}\")"),
            )
            .await?;
        let Some(result) = results.first() else {
            return Ok(Vec::new());
        };

        // PRAGMA column order: cid, name, type, notnull, dflt_value, pk
        let int_at = |row: &Row, index: usize| -> i64 {
            match row.values.get(index) {
                Some(CellValue::Int(i)) => *i,
                Some(CellValue::UInt(u)) => *u as i64,
                _ => 0,
            }
        };

        let mut columns = Vec::new();
        for row in &result.rows {
            let name = row
                .values
                .get(1)
                .and_then(CellValue::as_text)
                .unwrap_or_default();
            let database_type = row
                .values
                .get(2)
                .and_then(CellValue::as_text)
                .unwrap_or_default();
            let notnull = int_at(row, 3);
            let default_value = row
                .values
                .get(4)
                .and_then(CellValue::as_text)
                .map(ToOwned::to_owned);
            let primary_key = int_at(row, 5) != 0;

            columns.push(TableColumn {
                name: name.to_string(),
                database_type: database_type.to_string(),
                nullable: notnull == 0,
                default_value,
                primary_key,
                comment: None,
            });
        }

        Ok(columns)
    }
}
