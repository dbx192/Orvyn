//! TDengine REST SQL, typed results and metadata. Transactions are unsupported.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind, ErrorKind,
    Result, SslMode,
};
use std::time::Duration;

pub const ADAPTER_NAME: &str = "tdengine";

#[derive(Debug, Clone, Copy, Default)]
pub struct TdengineConnectionTester;

async fn rest_query(
    profile: &ConnectionProfile,
    password: Option<&str>,
    sql: &str,
) -> Result<serde_json::Value> {
    let scheme = match profile.security.ssl_mode {
        SslMode::Disable => "http",
        SslMode::Prefer | SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => "https",
    };
    let path = profile.extra_param("http_path").unwrap_or("/rest/sql");
    let database = profile
        .database
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|name| format!("/{}", urlencoding::encode(name)))
        .unwrap_or_default();
    let url = format!(
        "{scheme}://{}:{}/{}{database}",
        profile.required_host()?,
        profile.required_port()?,
        path.trim_matches('/')
    );

    let username = profile.username.as_deref().unwrap_or_default();
    static CLIENT: std::sync::OnceLock<Result<reqwest::Client>> = std::sync::OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|error| {
                    AppError::new(ErrorKind::Connection, "failed to build HTTP client")
                        .with_detail(error.to_string())
                })
        })
        .as_ref()
        .map_err(Clone::clone)?;

    let response = client
        .post(&url)
        .basic_auth(username, password.filter(|value| !value.is_empty()))
        .body(sql.to_string())
        .send()
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to reach TDengine REST API")
                .with_detail(error.to_string())
        })?;

    let status = response.status();
    let body = response.text().await.map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to read TDengine response")
            .with_detail(error.to_string())
    })?;

    if !status.is_success() {
        return Err(
            AppError::new(ErrorKind::Connection, "TDengine REST API returned an error")
                .with_detail(format!("HTTP {status}: {body}")),
        );
    }

    let json: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
        AppError::new(
            ErrorKind::Serialization,
            "failed to parse TDengine response",
        )
        .with_detail(error.to_string())
    })?;

    let code = json
        .get("code")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if code != 0 {
        let desc = json
            .get("desc")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown TDengine error");
        return Err(AppError::new(
            ErrorKind::Connection,
            format!("TDengine error: {desc}"),
        ));
    }

    Ok(json)
}

#[async_trait]
impl ConnectionTester for TdengineConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::TDengine
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started_at = std::time::Instant::now();
        let json = rest_query(profile, password, "SELECT server_version()").await?;
        let version = json
            .get("data")
            .and_then(|data| data.as_array())
            .and_then(|rows| rows.first())
            .and_then(|row| row.as_array())
            .and_then(|cells| cells.first())
            .and_then(|cell| cell.as_str())
            .map(ToOwned::to_owned);

        let mut report = ConnectionTestReport::success(
            DataSourceKind::TDengine,
            started_at.elapsed().as_millis(),
        );
        if let Some(version) = version {
            report = report.with_server_version(version);
        }
        Ok(report)
    }
}

use db_common::{
    CellValue, Column, ConnectionId, ConnectionPoolConfig, ConnectionPoolStats, DatabaseObject,
    DatabaseObjectKind, QueryId, QueryResult, RelationalMetadataReader, RelationalQueryExecutor,
    Row, TableColumn, TableIndex,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct TdengineQueryExecutor;
#[derive(Debug, Clone, Copy, Default)]
pub struct TdengineMetadataReader;

fn decode_result(sql: &str, json: &serde_json::Value) -> Result<QueryResult> {
    let mut result = QueryResult::empty(sql);
    if let Some(meta) = json.get("column_meta").and_then(|v| v.as_array()) {
        for column in meta {
            let name = column.get(0).and_then(|v| v.as_str()).ok_or_else(|| {
                AppError::new(ErrorKind::Serialization, "invalid TDengine column metadata")
            })?;
            let mut output = Column::new(
                name,
                column.get(1).map(ToString::to_string).unwrap_or_default(),
            );
            output.logical_type = match column.get(1).and_then(|v| v.as_u64()) {
                Some(1) => db_common::LogicalType::Bool,
                Some(2..=5) => db_common::LogicalType::Int,
                Some(6 | 7) => db_common::LogicalType::Float,
                Some(8 | 10 | 16) => db_common::LogicalType::Text,
                Some(9) => db_common::LogicalType::DateTime,
                Some(11..=14) => db_common::LogicalType::UInt,
                Some(15) => db_common::LogicalType::Json,
                Some(17) => db_common::LogicalType::Bytes,
                _ => db_common::LogicalType::Unknown,
            };
            result.columns.push(output);
        }
    } else if let Some(head) = json.get("head").and_then(|v| v.as_array()) {
        result.columns = head
            .iter()
            .map(|v| Column::new(v.as_str().unwrap_or(""), "UNKNOWN"))
            .collect();
    }
    if let Some(rows) = json.get("data").and_then(|v| v.as_array()) {
        for row in rows {
            let cells = row
                .as_array()
                .ok_or_else(|| AppError::new(ErrorKind::Serialization, "invalid TDengine row"))?;
            result.rows.push(Row::new(
                cells
                    .iter()
                    .map(|cell| match cell {
                        serde_json::Value::Null => CellValue::Null,
                        serde_json::Value::Bool(v) => CellValue::Bool(*v),
                        serde_json::Value::Number(v) if v.is_i64() => {
                            CellValue::Int(v.as_i64().unwrap())
                        }
                        serde_json::Value::Number(v) if v.is_u64() => {
                            CellValue::UInt(v.as_u64().unwrap())
                        }
                        serde_json::Value::Number(v) => {
                            CellValue::Float(v.as_f64().unwrap_or_default())
                        }
                        serde_json::Value::String(v) => CellValue::Text(v.clone()),
                        v => CellValue::Json(v.clone()),
                    })
                    .collect(),
            ));
        }
    }
    result.affected_rows = if result.columns.is_empty() {
        json.get("rows")
            .and_then(|v| v.as_u64())
            .unwrap_or_default()
    } else {
        0
    };
    Ok(result)
}

#[async_trait]
impl RelationalQueryExecutor for TdengineQueryExecutor {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::TDengine
    }
    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        _config: ConnectionPoolConfig,
    ) -> Result<Vec<QueryResult>> {
        let mut results = Vec::new();
        for sql in statements {
            if profile.security.read_only && !db_common::is_read_only_statement(sql) {
                return Err(AppError::new(
                    ErrorKind::Permission,
                    "connection is read-only",
                ));
            }
            let start = std::time::Instant::now();
            results.push(
                decode_result(sql, &rest_query(profile, password, sql).await?)?
                    .with_elapsed(start.elapsed()),
            );
        }
        Ok(results)
    }
    async fn pool_stats(&self, _: ConnectionId) -> Result<Option<ConnectionPoolStats>> {
        Ok(None)
    }
    async fn close_pool(&self, _: ConnectionId) -> Result<()> {
        Ok(())
    }
    async fn begin_transaction(
        &self,
        _: &ConnectionProfile,
        _: Option<&str>,
        _: QueryId,
        _: ConnectionPoolConfig,
    ) -> Result<()> {
        Err(AppError::unsupported("TDengine has no client transactions"))
    }
    async fn execute_in_transaction(&self, _: QueryId, _: &[String]) -> Result<Vec<QueryResult>> {
        Err(AppError::unsupported("TDengine has no client transactions"))
    }
    async fn commit_transaction(&self, _: QueryId) -> Result<()> {
        Err(AppError::unsupported("TDengine has no client transactions"))
    }
    async fn rollback_transaction(&self, _: QueryId) -> Result<()> {
        Err(AppError::unsupported("TDengine has no client transactions"))
    }
}
fn identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
fn selected_profile(profile: &ConnectionProfile, database: Option<&str>) -> ConnectionProfile {
    let mut profile = profile.clone();
    if let Some(database) = database {
        profile.database = Some(database.to_string());
    }
    profile
}
#[async_trait]
impl RelationalMetadataReader for TdengineMetadataReader {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::TDengine
    }
    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        objects(
            profile,
            password,
            "SHOW DATABASES",
            DatabaseObjectKind::Database,
        )
        .await
    }
    async fn list_schemas(
        &self,
        _: &ConnectionProfile,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        Ok(Vec::new())
    }
    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let profile = selected_profile(profile, database);
        let mut tables =
            objects(&profile, password, "SHOW TABLES", DatabaseObjectKind::Table).await?;
        tables.extend(
            objects(
                &profile,
                password,
                "SHOW STABLES",
                DatabaseObjectKind::Table,
            )
            .await?,
        );
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        tables.dedup_by(|a, b| a.name == b.name);
        Ok(tables)
    }
    async fn list_views(
        &self,
        _: &ConnectionProfile,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        Ok(Vec::new())
    }
    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let sql = format!("DESCRIBE {}", identifier(table));
        let result = decode_result(
            &sql,
            &rest_query(&selected_profile(profile, database), password, &sql).await?,
        )?;
        Ok(result
            .rows
            .iter()
            .map(|row| TableColumn {
                name: row
                    .values
                    .first()
                    .map(CellValue::display)
                    .unwrap_or_default(),
                database_type: row
                    .values
                    .get(1)
                    .map(CellValue::display)
                    .unwrap_or_default(),
                nullable: true,
                default_value: None,
                primary_key: false,
                comment: row
                    .values
                    .get(3)
                    .map(CellValue::display)
                    .filter(|s| !s.is_empty()),
            })
            .collect())
    }
    async fn list_indexes(
        &self,
        _: &ConnectionProfile,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
        _: &str,
    ) -> Result<Vec<TableIndex>> {
        Ok(Vec::new())
    }
    async fn get_create_table_sql(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _: Option<&str>,
        table: &str,
    ) -> Result<String> {
        let sql = format!("SHOW CREATE TABLE {}", identifier(table));
        let result = decode_result(
            &sql,
            &rest_query(&selected_profile(profile, database), password, &sql).await?,
        )?;
        result
            .rows
            .first()
            .and_then(|row| row.values.last())
            .map(CellValue::display)
            .ok_or_else(|| AppError::new(ErrorKind::Query, "TDengine returned no table definition"))
    }
}
async fn objects(
    profile: &ConnectionProfile,
    password: Option<&str>,
    sql: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let result = decode_result(sql, &rest_query(profile, password, sql).await?)?;
    Ok(result
        .rows
        .iter()
        .filter_map(|row| row.values.first())
        .map(|cell| DatabaseObject {
            kind,
            name: cell.display(),
            schema: None,
            parent: profile.database.clone(),
        })
        .collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_rest_types_without_truncating_unsigned_values() {
        let json = serde_json::json!({"column_meta":[["n",14,8],["text",8,20],["empty",4,4]],
            "data":[[18446744073709551615u64,"hello",null]],"rows":1});
        let result = decode_result("SELECT * FROM t", &json).unwrap();
        assert_eq!(result.columns[0].name, "n");
        assert_eq!(result.columns[0].logical_type, db_common::LogicalType::UInt);
        assert_eq!(result.affected_rows, 0);
        assert_eq!(result.rows[0].values[0], CellValue::UInt(u64::MAX));
        assert_eq!(result.rows[0].values[2], CellValue::Null);
    }
}
