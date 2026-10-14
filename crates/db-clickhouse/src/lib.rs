//! ClickHouse adapter crate backed by the ClickHouse HTTP interface.
//!
//! ClickHouse is reached over HTTP at `http://host:port/` (default 8123). SQL is
//! POSTed as the request body with `Content-Type: text/plain`; typed result sets
//! are requested with `FORMAT JSON` (meta + data + rows) and metadata queries use
//! `FORMAT JSONEachRow`. Because the HTTP interface is stateless there is no
//! connection pool and no client-side transaction support.

use async_trait::async_trait;
use db_common::{
    is_result_set_statement, AppError, CellValue, Column, ConnectionId, ConnectionPoolConfig,
    ConnectionPoolStats, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseFunction, DatabaseObject, DatabaseObjectKind, ErrorKind, LogicalType, QueryId,
    QueryResult, RelationalMetadataReader, RelationalQueryExecutor, Result, Row, SslMode,
    TableColumn, TableForeignKey, TableIndex, TableTrigger,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub const ADAPTER_NAME: &str = "clickhouse";

#[derive(Debug, Clone, Copy, Default)]
pub struct ClickHouseConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct ClickHouseMetadataReader;

#[derive(Debug, Clone, Copy, Default)]
pub struct ClickHouseQueryExecutor;

/// Build an HTTP client for the ClickHouse HTTP interface. Auth (HTTP basic
/// auth) is applied per-request from the profile username when present.
fn http_client(_profile: &ConnectionProfile, _password: Option<&str>) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to build HTTP client")
                .with_detail(error.to_string())
        })
}

/// Assemble the ClickHouse HTTP endpoint URL.
fn endpoint_url(profile: &ConnectionProfile) -> Result<String> {
    let scheme = match profile.security.ssl_mode {
        SslMode::Disable => "http",
        SslMode::Prefer | SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => "https",
    };
    Ok(format!(
        "{scheme}://{}:{}/",
        profile.required_host()?,
        profile.required_port()?
    ))
}

/// POST `sql` as the request body and return the raw response body. Applies
/// HTTP basic auth when the profile has a username. Non-2xx responses include
/// the server body in the error detail.
async fn post_sql(
    client: &reqwest::Client,
    profile: &ConnectionProfile,
    password: Option<&str>,
    sql: &str,
) -> Result<String> {
    let url = endpoint_url(profile)?;
    let mut request = client
        .post(&url)
        .header("Content-Type", "text/plain")
        .body(sql.to_string());

    if let Some(username) = profile
        .username
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        request = request.basic_auth(username, password.filter(|value| !value.is_empty()));
    }

    let response = request.send().await.map_err(|error| {
        AppError::new(ErrorKind::Query, "ClickHouse query failed").with_detail(error.to_string())
    })?;

    let status = response.status();
    let body = response.text().await.map_err(|error| {
        AppError::new(ErrorKind::Query, "ClickHouse query failed").with_detail(error.to_string())
    })?;

    if !status.is_success() {
        return Err(AppError::new(ErrorKind::Query, "ClickHouse query failed")
            .with_detail(format!("HTTP {status}: {body}")));
    }

    Ok(body)
}

/// Run a query and parse each JSONEachRow line into a JSON value. The caller
/// is responsible for appending `FORMAT JSONEachRow`.
async fn post_json_each_row(
    client: &reqwest::Client,
    profile: &ConnectionProfile,
    password: Option<&str>,
    sql: &str,
) -> Result<Vec<serde_json::Value>> {
    let body = post_sql(client, profile, password, sql).await?;
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                AppError::new(
                    ErrorKind::Serialization,
                    "failed to parse ClickHouse JSONEachRow response",
                )
                .with_detail(error.to_string())
            })
        })
        .collect()
}

/// Resolve the effective ClickHouse database: prefer the explicit argument,
/// fall back to the profile's configured database.
fn effective_database(profile: &ConnectionProfile, database: Option<&str>) -> Option<String> {
    database
        .or(profile.database.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

/// Escape a value for use in a single-quoted SQL string literal.
fn sql_str(value: &str) -> String {
    value.replace('\'', "''")
}

/// Quote an identifier with backticks.
fn quote_ident(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

/// Strip a `Nullable(...)` / `LowCardinality(...)` wrapper to expose the
/// underlying ClickHouse type.
fn unwrap_wrapper(ty: &str) -> &str {
    let lower = ty.to_ascii_lowercase();
    if lower.starts_with("nullable(") || lower.starts_with("lowcardinality(") {
        if let Some(open) = ty.find('(') {
            if let Some(inner) = ty.get(open + 1..) {
                return inner.strip_suffix(')').unwrap_or(inner);
            }
        }
    }
    ty
}

/// Map a ClickHouse type string to a db_common [`LogicalType`].
fn clickhouse_logical_type(ty: &str) -> LogicalType {
    let inner = unwrap_wrapper(ty);
    let lower = inner.to_ascii_lowercase();

    if lower.starts_with("bool") {
        LogicalType::Bool
    } else if lower.starts_with("int") {
        LogicalType::Int
    } else if lower.starts_with("uint") {
        LogicalType::UInt
    } else if lower.starts_with("float") {
        LogicalType::Float
    } else if lower.starts_with("decimal") {
        LogicalType::Decimal
    } else if lower.starts_with("datetime") {
        LogicalType::DateTime
    } else if lower.starts_with("date") {
        LogicalType::Date
    } else if lower.starts_with("time") {
        LogicalType::Time
    } else if lower.starts_with("fixedstring")
        || lower.starts_with("enum")
        || lower.starts_with("uuid")
        || lower.starts_with("lowcardinality")
        || lower.starts_with("ip")
        || lower.starts_with("string")
    {
        LogicalType::Text
    } else if lower.starts_with("array")
        || lower.starts_with("map")
        || lower.starts_with("tuple")
        || lower.starts_with("json")
    {
        LogicalType::Json
    } else {
        LogicalType::Text
    }
}

/// Convert a parsed JSON scalar into a [`CellValue`]. Date / time values are
/// carried as text to avoid an additional chrono dependency.
fn json_to_cell(value: &serde_json::Value, type_str: &str) -> CellValue {
    if value.is_null() {
        return CellValue::Null;
    }

    let inner = unwrap_wrapper(type_str);
    let lower = inner.to_ascii_lowercase();

    if lower.starts_with("int") {
        if let Some(number) = value.as_i64() {
            return CellValue::Int(number);
        }
        if let Some(number) = value.as_str().and_then(|s| s.parse::<i64>().ok()) {
            return CellValue::Int(number);
        }
    } else if lower.starts_with("uint") {
        if let Some(number) = value.as_u64() {
            return CellValue::UInt(number);
        }
        if let Some(number) = value.as_str().and_then(|s| s.parse::<u64>().ok()) {
            return CellValue::UInt(number);
        }
    } else if lower.starts_with("float") {
        if let Some(number) = value.as_f64() {
            return CellValue::Float(number);
        }
        if let Some(number) = value.as_str().and_then(|s| s.parse::<f64>().ok()) {
            return CellValue::Float(number);
        }
    } else if lower.starts_with("bool") {
        match value {
            serde_json::Value::Bool(boolean) => return CellValue::Bool(*boolean),
            serde_json::Value::String(s) => return CellValue::Bool(s == "true" || s == "1"),
            _ => {}
        }
    }

    match value {
        serde_json::Value::String(s) => CellValue::Text(s.clone()),
        serde_json::Value::Number(n) => CellValue::Text(n.to_string()),
        serde_json::Value::Bool(b) => CellValue::Text(b.to_string()),
        _ => CellValue::Text(value.to_string()),
    }
}

/// Build a [`QueryResult`] from a ClickHouse `FORMAT JSON` response object.
fn query_result_from_json(statement: &str, json: &serde_json::Value) -> QueryResult {
    let columns: Vec<Column> = json
        .get("meta")
        .and_then(|meta| meta.as_array())
        .map(|meta| {
            meta.iter()
                .map(|column| {
                    let database_type = column
                        .get("type")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    Column {
                        name: column
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        database_type: database_type.clone(),
                        logical_type: clickhouse_logical_type(&database_type),
                        nullable: database_type.contains("Nullable("),
                        primary_key: false,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let rows = json
        .get("data")
        .and_then(|data| data.as_array())
        .map(|data| {
            data.iter()
                .map(|object| {
                    let values = columns
                        .iter()
                        .map(|column| {
                            let value = object
                                .get(&column.name)
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            json_to_cell(&value, &column.database_type)
                        })
                        .collect();
                    Row::new(values)
                })
                .collect()
        })
        .unwrap_or_default();

    QueryResult {
        statement: statement.to_string(),
        columns,
        rows,
        affected_rows: 0,
        execution_time_ms: 0,
        notices: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

#[async_trait]
impl ConnectionTester for ClickHouseConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::ClickHouse
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started = Instant::now();
        let client = http_client(profile, password)?;

        // Real round-trip: the HTTP interface answers `SELECT 1` reliably where
        // a TCP socket does not (it also exercises auth).
        post_sql(&client, profile, password, "SELECT 1 FORMAT JSON").await?;

        let version = post_sql(
            &client,
            profile,
            password,
            "SELECT version() AS version FORMAT JSONEachRow",
        )
        .await
        .ok()
        .and_then(|body| {
            body.lines().find_map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).ok()?;
                value
                    .as_object()?
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned)
            })
        });

        let mut report = ConnectionTestReport::success(
            DataSourceKind::ClickHouse,
            started.elapsed().as_millis(),
        );
        if let Some(version) = version {
            report = report.with_server_version(version);
        }
        Ok(report)
    }
}

#[async_trait]
impl RelationalMetadataReader for ClickHouseMetadataReader {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::ClickHouse
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let client = http_client(profile, password)?;
        let rows = post_json_each_row(
            &client,
            profile,
            password,
            "SELECT name FROM system.databases ORDER BY name FORMAT JSONEachRow",
        )
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| DatabaseObject {
                kind: DatabaseObjectKind::Database,
                name: row
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
                schema: None,
                parent: None,
            })
            .collect())
    }

    async fn list_schemas(
        &self,
        _profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        // ClickHouse has no separate schema layer: the database *is* the schema.
        Ok(Vec::new())
    }

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        self.list_objects(profile, password, database, false).await
    }

    async fn list_views(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        self.list_objects(profile, password, database, true).await
    }

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let client = http_client(profile, password)?;
        let database = effective_database(profile, database).unwrap_or_default();
        let sql = format!(
            "SELECT name, type, default_expression, comment \
             FROM system.columns \
             WHERE database = '{}' AND table = '{}' \
             ORDER BY position FORMAT JSONEachRow",
            sql_str(&database),
            sql_str(table)
        );
        let rows = post_json_each_row(&client, profile, password, &sql).await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let database_type = row
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                TableColumn {
                    name: row
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    nullable: database_type.contains("Nullable("),
                    database_type,
                    default_value: row
                        .get("default_expression")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                    primary_key: false,
                    comment: row
                        .get("comment")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                }
            })
            .collect())
    }

    async fn list_indexes(
        &self,
        _profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
        _table: &str,
    ) -> Result<Vec<TableIndex>> {
        // ClickHouse exposes data-skipping indexes separately; not surfaced here.
        Ok(Vec::new())
    }

    async fn list_foreign_keys(
        &self,
        _profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
        _table: &str,
    ) -> Result<Vec<TableForeignKey>> {
        // ClickHouse has no enforced foreign-key layer.
        Ok(Vec::new())
    }

    async fn list_triggers(
        &self,
        _profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
        _table: &str,
    ) -> Result<Vec<TableTrigger>> {
        // ClickHouse has no triggers in the relational sense.
        Ok(Vec::new())
    }

    async fn list_functions(
        &self,
        _profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<DatabaseFunction>> {
        // User-defined functions are not enumerated for now.
        Ok(Vec::new())
    }

    async fn get_create_table_sql(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<String> {
        let client = http_client(profile, password)?;
        let database = effective_database(profile, database).unwrap_or_default();
        let sql = format!(
            "SHOW CREATE TABLE {}.{} FORMAT JSONEachRow",
            quote_ident(&database),
            quote_ident(table)
        );
        let body = post_sql(&client, profile, password, &sql).await?;

        body.lines()
            .find_map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).ok()?;
                value
                    .get("statement")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned)
            })
            .ok_or_else(|| {
                AppError::new(
                    ErrorKind::Query,
                    format!("could not read CREATE DDL for `{table}`"),
                )
                .with_detail(body[..body.len().min(512)].to_string())
            })
    }
}

impl ClickHouseMetadataReader {
    async fn list_objects(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        want_views: bool,
    ) -> Result<Vec<DatabaseObject>> {
        let _ = self;
        let client = http_client(profile, password)?;
        let database = effective_database(profile, database);
        let filter = database
            .as_deref()
            .map(|db| format!(" WHERE database = '{}'", sql_str(db)))
            .unwrap_or_default();
        let sql = format!(
            "SELECT name, engine FROM system.tables{filters} ORDER BY name FORMAT JSONEachRow",
            filters = filter
        );
        let rows = post_json_each_row(&client, profile, password, &sql).await?;
        let kind = if want_views {
            DatabaseObjectKind::View
        } else {
            DatabaseObjectKind::Table
        };

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let name = row
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let engine = row
                    .get("engine")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let is_view = engine.contains("View");
                if is_view != want_views || name.is_empty() {
                    return None;
                }
                Some(DatabaseObject {
                    kind,
                    name,
                    schema: database.clone(),
                    parent: database.clone(),
                })
            })
            .collect())
    }
}

#[async_trait]
impl RelationalQueryExecutor for ClickHouseQueryExecutor {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::ClickHouse
    }

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        _pool_config: ConnectionPoolConfig,
    ) -> Result<Vec<QueryResult>> {
        let client = http_client(profile, password)?;
        let mut results = Vec::with_capacity(statements.len());

        for statement in statements {
            let started = Instant::now();

            // Statement that is expected to return rows is parsed with FORMAT
            // JSON so we get columns + typed values; everything else just runs.
            let output_sql = if is_result_set_statement(statement) {
                format!("{} FORMAT JSON", statement.trim_end().trim_end_matches(';'))
            } else {
                statement.trim_end().to_string()
            };
            let body = post_sql(&client, profile, password, &output_sql).await?;

            let mut result = if output_sql.contains("FORMAT JSON") {
                let json: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
                    AppError::new(ErrorKind::Query, "ClickHouse query failed")
                        .with_detail(error.to_string())
                })?;
                query_result_from_json(statement, &json)
            } else {
                QueryResult::empty(statement)
            };
            result.execution_time_ms = started.elapsed().as_millis();
            results.push(result);
        }

        Ok(results)
    }

    async fn pool_stats(
        &self,
        _connection_id: ConnectionId,
    ) -> Result<Option<ConnectionPoolStats>> {
        // HTTP interface is stateless; no pool to report on.
        Ok(None)
    }

    async fn close_pool(&self, _connection_id: ConnectionId) -> Result<()> {
        Ok(())
    }

    async fn begin_transaction(
        &self,
        _profile: &ConnectionProfile,
        _password: Option<&str>,
        _transaction_id: QueryId,
        _pool_config: ConnectionPoolConfig,
    ) -> Result<()> {
        Err(AppError::new(
            ErrorKind::Unsupported,
            "ClickHouse has no client-side transactions",
        ))
    }

    async fn execute_in_transaction(
        &self,
        _transaction_id: QueryId,
        _statements: &[String],
    ) -> Result<Vec<QueryResult>> {
        Err(AppError::new(
            ErrorKind::Unsupported,
            "ClickHouse has no client-side transactions",
        ))
    }

    async fn commit_transaction(&self, _transaction_id: QueryId) -> Result<()> {
        Ok(())
    }

    async fn rollback_transaction(&self, _transaction_id: QueryId) -> Result<()> {
        Ok(())
    }
}
