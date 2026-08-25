//! SQL Server adapter crate backed by the `tiberius` driver.
//!
//! SQL Server is a client-server RDBMS reached over the TDS wire protocol.
//! `tiberius` has no built-in pool, so this adapter keeps one live client per
//! connection (identity + database) behind a mutex and reuses it across a
//! conversation. Transactions borrow that same client so BEGIN/COMMIT/ROLLBACK
//! all hit a single session.

use async_trait::async_trait;
use db_common::{
    is_result_set_statement, AppError, CellValue, Column, ConnectionId, ConnectionPoolConfig,
    ConnectionPoolStats, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseObject, DatabaseObjectKind, ErrorKind, LogicalType, QueryId, QueryResult,
    RelationalMetadataReader, RelationalQueryExecutor, Result, Row, TableColumn, TableIndex,
};
use futures_util::{StreamExt, TryStreamExt};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tiberius::{AuthMethod, Client, ColumnType, EncryptionLevel};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::TokioAsyncWriteCompatExt;

pub const ADAPTER_NAME: &str = "sqlserver";

/// Concrete tiberius client: a tokio TCP stream wrapped in a futures-io compat
/// adapter (tiberius requires `futures_io` streams).
type SqlServerClient = Client<tokio_util::compat::Compat<TcpStream>>;

type SqlServerPoolKey = (ConnectionId, Option<String>);

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlServerConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlServerMetadataReader;

#[derive(Debug, Clone, Default)]
pub struct SqlServerQueryExecutor {
    pools: Arc<Mutex<BTreeMap<SqlServerPoolKey, Arc<SqlServerPool>>>>,
    transactions: Arc<Mutex<BTreeMap<QueryId, Arc<SqlServerPool>>>>,
}

impl SqlServerQueryExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    async fn pool_for(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Arc<SqlServerPool>> {
        let key = sqlserver_pool_key(profile);
        if let Some(pool) = self.pools.lock().await.get(&key).cloned() {
            return Ok(pool);
        }

        let pool = Arc::new(SqlServerPool::new(profile, password)?);
        self.pools.lock().await.insert(key, pool.clone());
        Ok(pool)
    }
}

#[derive(Debug)]
struct SqlServerPool {
    connection_id: ConnectionId,
    database: Option<String>,
    client: Mutex<Option<SqlServerClient>>,
}

impl SqlServerPool {
    fn new(profile: &ConnectionProfile, password: Option<&str>) -> Result<Self> {
        let database = selected_database(profile, None);
        // Validate eagerly so a bad profile surfaces before any connection.
        let _config = build_config(profile, password, database.as_deref())?;
        Ok(Self {
            connection_id: profile.id,
            database,
            client: Mutex::new(None),
        })
    }

    async fn acquire(
        self: &Arc<Self>,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<SqlServerClient>>> {
        let mut guard = self.client.lock().await;
        if guard.is_none() {
            *guard = Some(connect(profile, password, self.database.as_deref()).await?);
        }
        Ok(guard)
    }

    async fn stats(&self) -> ConnectionPoolStats {
        let connected = self.client.lock().await.is_some();
        ConnectionPoolStats {
            connection_id: self.connection_id,
            size: u32::from(connected),
            idle: u32::from(connected),
            max_connections: 1,
        }
    }
}

#[async_trait]
impl ConnectionTester for SqlServerConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::SqlServer
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started_at = Instant::now();
        let mut client = connect(profile, password, None).await?;
        let rows = query_rows(&mut client, "SELECT @@VERSION").await?;
        let version = rows.first().and_then(|row| cell_string(row, 0));

        Ok(ConnectionTestReport::success(
            DataSourceKind::SqlServer,
            started_at.elapsed().as_millis(),
        )
        .with_server_version(version.unwrap_or_default()))
    }
}

#[async_trait]
impl RelationalMetadataReader for SqlServerMetadataReader {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::SqlServer
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let mut client = connect(profile, password, None).await?;
        let rows = query_rows(&mut client, "SELECT name FROM sys.databases ORDER BY name").await?;
        Ok(rows
            .into_iter()
            .map(|row| DatabaseObject {
                kind: DatabaseObjectKind::Database,
                name: cell_string(&row, 0).unwrap_or_default(),
                schema: None,
                parent: None,
            })
            .collect())
    }

    async fn list_schemas(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let mut client = connect(profile, password, database).await?;
        let rows = query_rows(
            &mut client,
            "SELECT name FROM sys.schemas \
             WHERE name NOT IN ('sys','INFORMATION_SCHEMA','guest','db_owner', \
                  'db_accessadmin','db_securityadmin','db_ddladmin','db_backupoperator', \
                  'db_datareader','db_datawriter','db_denydatareader','db_denydatawriter') \
             ORDER BY name",
        )
        .await?;
        let parent = selected_database(profile, database);
        Ok(rows
            .into_iter()
            .map(|row| DatabaseObject {
                kind: DatabaseObjectKind::Schema,
                name: cell_string(&row, 0).unwrap_or_default(),
                schema: None,
                parent: parent.clone(),
            })
            .collect())
    }

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        list_sqlserver_objects(
            profile,
            password,
            database,
            schema,
            "BASE TABLE",
            DatabaseObjectKind::Table,
        )
        .await
    }

    async fn list_views(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        list_sqlserver_objects(
            profile,
            password,
            database,
            schema,
            "VIEW",
            DatabaseObjectKind::View,
        )
        .await
    }

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let schema = schema.unwrap_or("dbo");
        let mut client = connect(profile, password, database).await?;
        let sql = format!(
            "SELECT c.COLUMN_NAME, c.DATA_TYPE, c.IS_NULLABLE, c.COLUMN_DEFAULT, \
                    ISNULL(pk.PK, 0) AS IS_PK \
             FROM INFORMATION_SCHEMA.COLUMNS c \
             OUTER APPLY ( \
                 SELECT 1 AS PK \
                 FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu \
                 JOIN INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc \
                   ON kcu.CONSTRAINT_NAME = tc.CONSTRAINT_NAME \
                  AND kcu.TABLE_SCHEMA = tc.TABLE_SCHEMA \
                 WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY' \
                   AND tc.TABLE_SCHEMA = c.TABLE_SCHEMA \
                   AND tc.TABLE_NAME = c.TABLE_NAME \
                   AND kcu.COLUMN_NAME = c.COLUMN_NAME \
                   AND kcu.TABLE_SCHEMA = c.TABLE_SCHEMA \
                   AND kcu.TABLE_NAME = c.TABLE_NAME \
             ) pk \
             WHERE c.TABLE_SCHEMA = '{}' AND c.TABLE_NAME = {} \
             ORDER BY c.ORDINAL_POSITION",
            schema.replace('\'', "''"),
            quote(table),
        );
        let rows = query_rows(&mut client, &sql).await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let nullable =
                    cell_string(&row, 2).is_some_and(|value| value.eq_ignore_ascii_case("YES"));
                TableColumn {
                    name: cell_string(&row, 0).unwrap_or_default(),
                    database_type: cell_string(&row, 1).unwrap_or_default(),
                    nullable,
                    default_value: cell_string(&row, 3),
                    primary_key: cell_i32(&row, 4).unwrap_or(0) == 1,
                    comment: None,
                }
            })
            .collect())
    }

    async fn list_indexes(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableIndex>> {
        let schema = schema.unwrap_or("dbo");
        let mut client = connect(profile, password, database).await?;
        let sql = format!(
            "SELECT i.name, i.is_unique, i.is_primary_key, \
                    STRING_AGG(c.name, ',') WITHIN GROUP (ORDER BY ic.key_ordinal) AS columns \
             FROM sys.indexes i \
             JOIN sys.objects o ON i.object_id = o.object_id \
             JOIN sys.schemas s ON o.schema_id = s.schema_id \
             JOIN sys.index_columns ic ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
             JOIN sys.columns c ON ic.object_id = c.object_id AND ic.column_id = c.column_id \
             WHERE s.name = '{}' AND o.name = {} AND i.name IS NOT NULL \
             GROUP BY i.name, i.is_unique, i.is_primary_key \
             ORDER BY i.name",
            schema.replace('\'', "''"),
            quote(table),
        );
        let rows = query_rows(&mut client, &sql).await?;

        Ok(rows
            .into_iter()
            .map(|row| TableIndex {
                name: cell_string(&row, 0).unwrap_or_default(),
                unique: cell_bool(&row, 1).unwrap_or(false),
                primary: cell_bool(&row, 2).unwrap_or(false),
                columns: cell_string(&row, 3)
                    .map(|columns| columns.split(',').map(str::to_string).collect())
                    .unwrap_or_default(),
            })
            .collect())
    }

    async fn get_create_table_sql(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<String> {
        let schema = schema.unwrap_or("dbo");
        let columns = self
            .list_columns(profile, password, database, Some(schema), table)
            .await?;
        if columns.is_empty() {
            return Err(AppError::new(
                ErrorKind::NotFound,
                format!("table {}.{} not found", schema, table),
            ));
        }

        let mut col_defs: Vec<String> = Vec::new();
        for column in &columns {
            let mut col = format!("    [{}] {}", column.name, column.database_type);
            if let Some(default_value) = &column.default_value {
                col.push_str(&format!(" DEFAULT {}", default_value));
            }
            if !column.nullable && !column.primary_key {
                col.push_str(" NOT NULL");
            }
            col_defs.push(col);
        }

        let mut ddl = format!("CREATE TABLE [{}].[{}] (\n", schema, table);
        ddl.push_str(&col_defs.join(",\n"));
        ddl.push_str("\n);");
        Ok(ddl)
    }
}

#[async_trait]
impl RelationalQueryExecutor for SqlServerQueryExecutor {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::SqlServer
    }

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        _pool_config: ConnectionPoolConfig,
    ) -> Result<Vec<QueryResult>> {
        let pool = self.pool_for(profile, password).await?;
        let mut guard = pool.acquire(profile, password).await?;
        let client = guard
            .as_mut()
            .ok_or_else(|| AppError::internal("SQL Server pool client missing"))?;
        execute_sqlserver_statements(client, statements).await
    }

    async fn pool_stats(&self, connection_id: ConnectionId) -> Result<Option<ConnectionPoolStats>> {
        let pools = self
            .pools
            .lock()
            .await
            .iter()
            .filter(|(key, _)| key.0 == connection_id)
            .map(|(_, pool)| pool.clone())
            .collect::<Vec<_>>();
        if pools.is_empty() {
            return Ok(None);
        }

        let mut stats = ConnectionPoolStats {
            connection_id,
            size: 0,
            idle: 0,
            max_connections: 0,
        };
        for pool in pools {
            let pool_stats = pool.stats().await;
            stats.size += pool_stats.size;
            stats.idle += pool_stats.idle;
            stats.max_connections += pool_stats.max_connections;
        }
        Ok(Some(stats))
    }

    async fn close_pool(&self, connection_id: ConnectionId) -> Result<()> {
        self.pools
            .lock()
            .await
            .retain(|key, _| key.0 != connection_id);
        Ok(())
    }

    async fn begin_transaction(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        transaction_id: QueryId,
        _pool_config: ConnectionPoolConfig,
    ) -> Result<()> {
        let pool = self.pool_for(profile, password).await?;
        {
            let mut guard = pool.acquire(profile, password).await?;
            let client = guard
                .as_mut()
                .ok_or_else(|| AppError::internal("SQL Server pool client missing"))?;
            client
                .execute("BEGIN TRANSACTION", &[])
                .await
                .map_err(query_error)?;
        }
        self.transactions.lock().await.insert(transaction_id, pool);
        Ok(())
    }

    async fn execute_in_transaction(
        &self,
        transaction_id: QueryId,
        statements: &[String],
    ) -> Result<Vec<QueryResult>> {
        let pool = sqlserver_active_pool(&self.transactions, transaction_id).await?;
        let mut guard = pool.client.lock().await;
        let client = guard
            .as_mut()
            .ok_or_else(|| AppError::internal("transaction connection missing"))?;
        execute_sqlserver_statements(client, statements).await
    }

    async fn commit_transaction(&self, transaction_id: QueryId) -> Result<()> {
        let pool = self
            .transactions
            .lock()
            .await
            .remove(&transaction_id)
            .ok_or_else(|| AppError::validation("transaction is not active"))?;
        let mut guard = pool.client.lock().await;
        let client = guard
            .as_mut()
            .ok_or_else(|| AppError::internal("transaction connection missing"))?;
        client.execute("COMMIT", &[]).await.map_err(query_error)?;
        Ok(())
    }

    async fn rollback_transaction(&self, transaction_id: QueryId) -> Result<()> {
        let pool = self
            .transactions
            .lock()
            .await
            .remove(&transaction_id)
            .ok_or_else(|| AppError::validation("transaction is not active"))?;
        let mut guard = pool.client.lock().await;
        let client = guard
            .as_mut()
            .ok_or_else(|| AppError::internal("transaction connection missing"))?;
        client.execute("ROLLBACK", &[]).await.map_err(query_error)?;
        Ok(())
    }
}

async fn sqlserver_active_pool(
    transactions: &Mutex<BTreeMap<QueryId, Arc<SqlServerPool>>>,
    transaction_id: QueryId,
) -> Result<Arc<SqlServerPool>> {
    transactions
        .lock()
        .await
        .get(&transaction_id)
        .cloned()
        .ok_or_else(|| AppError::validation("transaction is not active"))
}

async fn list_sqlserver_objects(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: Option<&str>,
    schema: Option<&str>,
    table_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let schema = schema.unwrap_or("dbo");
    let mut client = connect(profile, password, database).await?;
    let sql = format!(
        "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
         WHERE TABLE_SCHEMA = '{}' AND TABLE_TYPE = '{}' \
         ORDER BY TABLE_NAME",
        schema.replace('\'', "''"),
        table_type,
    );
    let rows = query_rows(&mut client, &sql).await?;
    let parent = selected_database(profile, database);

    Ok(rows
        .into_iter()
        .map(|row| DatabaseObject {
            kind,
            name: cell_string(&row, 0).unwrap_or_default(),
            schema: Some(schema.to_string()),
            parent: parent.clone(),
        })
        .collect())
}

/// Execute a SELECT and buffer all rows as dynamic tiberius rows.
async fn query_rows(client: &mut SqlServerClient, sql: &str) -> Result<Vec<tiberius::Row>> {
    let stream = client.query(sql, &[]).await.map_err(query_error)?;
    futures_util::pin_mut!(stream);
    let mut rows = Vec::new();
    while let Some(item) = stream.try_next().await.map_err(query_error)? {
        if let tiberius::QueryItem::Row(row) = item {
            rows.push(row);
        }
    }
    Ok(rows)
}

fn cell(row: &tiberius::Row, index: usize) -> Option<&tiberius::ColumnData<'static>> {
    row.cells().nth(index).map(|(_, data)| data)
}

fn cell_string(row: &tiberius::Row, index: usize) -> Option<String> {
    cell(row, index).and_then(|data| format_column_data(data).text())
}

fn cell_i32(row: &tiberius::Row, index: usize) -> Option<i32> {
    cell(row, index).and_then(|data| match data {
        tiberius::ColumnData::I32(Some(value)) => Some(*value),
        _ => None,
    })
}

fn cell_bool(row: &tiberius::Row, index: usize) -> Option<bool> {
    cell(row, index).and_then(|data| match data {
        tiberius::ColumnData::Bit(Some(value)) => Some(*value),
        _ => None,
    })
}

async fn execute_sqlserver_statements(
    client: &mut SqlServerClient,
    statements: &[String],
) -> Result<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());

    for statement in statements {
        let started_at = Instant::now();
        let statement = statement.as_str();

        if is_result_set_statement(statement) {
            let stream = client.query(statement, &[]).await.map_err(query_error)?;
            futures_util::pin_mut!(stream);
            let columns: Vec<Column> = stream
                .columns()
                .await
                .map_err(query_error)?
                .unwrap_or_default()
                .iter()
                .map(|column| Column {
                    name: column.name().to_string(),
                    database_type: format!("{:?}", column.column_type()),
                    logical_type: logical_type(column.column_type()),
                    nullable: true,
                    primary_key: false,
                })
                .collect();
            let columns_for_result = columns;

            let column_count = columns_for_result.len();
            let mut rows = Vec::new();
            while let Some(item) = stream.try_next().await.map_err(query_error)? {
                if let tiberius::QueryItem::Row(row) = item {
                    let mut values = Vec::with_capacity(column_count);
                    for data in row.cells().map(|(_, data)| data.to_owned()) {
                        values.push(format_column_data(&data));
                    }
                    rows.push(Row::new(values));
                }
            }

            results.push(QueryResult {
                statement: statement.to_string(),
                columns: columns_for_result,
                rows,
                affected_rows: 0,
                execution_time_ms: started_at.elapsed().as_millis(),
                notices: Vec::new(),
                metadata: BTreeMap::new(),
            });
            continue;
        }

        let result = client.execute(statement, &[]).await.map_err(query_error)?;
        let mut query_result = QueryResult::empty(statement).with_elapsed(started_at.elapsed());
        query_result.affected_rows = result.total();
        results.push(query_result);
    }

    Ok(results)
}

fn format_column_data(data: &tiberius::ColumnData) -> CellValue {
    match data {
        tiberius::ColumnData::U8(Some(value)) => CellValue::Int(i64::from(*value)),
        tiberius::ColumnData::I16(Some(value)) => CellValue::Int(i64::from(*value)),
        tiberius::ColumnData::I32(Some(value)) => CellValue::Int(i64::from(*value)),
        tiberius::ColumnData::I64(Some(value)) => CellValue::Int(*value),
        tiberius::ColumnData::F32(Some(value)) => CellValue::Float(f64::from(*value)),
        tiberius::ColumnData::F64(Some(value)) => CellValue::Float(*value),
        tiberius::ColumnData::Bit(Some(value)) => CellValue::Bool(*value),
        tiberius::ColumnData::String(value) => value
            .as_ref()
            .map(|value| CellValue::Text(value.to_string()))
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::Guid(value) => value
            .map(|value| CellValue::Text(value.to_string()))
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::Binary(value) => value
            .as_ref()
            .map(|value| CellValue::Bytes(value.to_vec()))
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::Numeric(value) => value
            .map(|value| CellValue::Decimal(value.to_string()))
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::Xml(value) => value
            .as_ref()
            .map(|value| CellValue::Text(value.to_string()))
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::DateTime(value) => value
            .map(|value| {
                CellValue::Text(format_tds_datetime(
                    i64::from(value.days()),
                    value.seconds_fragments(),
                ))
            })
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::SmallDateTime(value) => value
            .map(|value| {
                CellValue::Text(format_tds_datetime(
                    i64::from(value.days()),
                    u32::from(value.seconds_fragments()),
                ))
            })
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::Time(value) => value
            .map(|value| CellValue::Text(format_tds_time(value)))
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::Date(value) => value
            .map(|value| CellValue::Text(format_tds_date(value.days())))
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::DateTime2(value) => value
            .map(|value| {
                let date = format_tds_date(value.date().days());
                let time = format_tds_time(value.time());
                CellValue::Text(format!("{date} {time}"))
            })
            .unwrap_or(CellValue::Null),
        tiberius::ColumnData::DateTimeOffset(value) => value
            .map(|value| {
                let dt = value.datetime2();
                let date = format_tds_date(dt.date().days());
                let time = format_tds_time(dt.time());
                let offset_minutes = value.offset();
                let sign = if offset_minutes < 0 { '-' } else { '+' };
                let abs = offset_minutes.unsigned_abs();
                let (oh, om) = (abs / 60, abs % 60);
                CellValue::Text(format!("{date} {time}{sign}{oh:02}:{om:02}"))
            })
            .unwrap_or(CellValue::Null),
        _ => CellValue::Null,
    }
}

impl CellValueHelper for CellValue {}

/// Convert a TDS `date` day count (days since 0001-01-01, 1-based) into a
/// `YYYY-MM-DD` string. Deltas are relative to 1970-01-01, whose TDS day
/// number is 719163 (i.e. 719162 days before the last day of year 0).
fn format_tds_date(days: u32) -> String {
    let z = i64::from(days) - 719163;
    civil_from_days(z)
        .map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}"))
        .unwrap_or_default()
}

/// Format a TDS `time` value measured in 10^-scale second increments.
fn format_tds_time(value: tiberius::time::Time) -> String {
    let scale = value.scale() as u32;
    let increments = value.increments();
    let total_seconds = increments / 10u64.pow(scale);
    let fraction = increments % 10u64.pow(scale);
    let h = total_seconds / 3600;
    let m = (total_seconds % 3600) / 60;
    let s = total_seconds % 60;
    if fraction == 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        // Left-pad the fractional seconds to `scale` digits, trimming nothing.
        let frac = format!("{fraction:0scale$}", scale = scale as usize);
        format!("{h:02}:{m:02}:{s:02}.{frac}")
    }
}

/// Format a TDS `datetime`/`smalldatetime` value given days since 1900-01-01
/// and a 1/300 second fraction.
fn format_tds_datetime(days_since_1900: i64, seconds_fragments: u32) -> String {
    let z = days_since_1900 - 25567;
    let date = civil_from_days(z)
        .map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}"))
        .unwrap_or_default();
    let (total_seconds, frac) = (seconds_fragments / 300, seconds_fragments % 300);
    let h = total_seconds / 3600;
    let m = (total_seconds % 3600) / 60;
    let s = total_seconds % 60;
    if frac == 0 {
        format!("{date} {h:02}:{m:02}:{s:02}")
    } else {
        let millis = frac * 1000 / 300;
        format!("{date} {h:02}:{m:02}:{s:02}.{millis:03}")
    }
}

/// Howard Hinnant's `civil_from_days`: convert days since 1970-01-01 into a
/// Gregorian `(year, month, day)`.
fn civil_from_days(z: i64) -> Option<(i32, u32, u32)> {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = (if m <= 2 { y + 1 } else { y }) as i32;
    (m >= 1 && m <= 12 && d >= 1 && d <= 31).then_some((y, m, d))
}

trait CellValueHelper {
    fn text(&self) -> Option<String> {
        None
    }
}

fn logical_type(column_type: ColumnType) -> LogicalType {
    match column_type {
        ColumnType::Bit | ColumnType::Bitn => LogicalType::Bool,
        ColumnType::Int1
        | ColumnType::Int2
        | ColumnType::Int4
        | ColumnType::Int8
        | ColumnType::Intn => LogicalType::Int,
        ColumnType::Float4 | ColumnType::Float8 | ColumnType::Floatn => LogicalType::Float,
        ColumnType::Money | ColumnType::Money4 | ColumnType::Decimaln | ColumnType::Numericn => {
            LogicalType::Decimal
        }
        ColumnType::Daten => LogicalType::Date,
        ColumnType::Timen => LogicalType::Time,
        ColumnType::Datetime
        | ColumnType::Datetime4
        | ColumnType::Datetimen
        | ColumnType::Datetime2
        | ColumnType::DatetimeOffsetn => LogicalType::DateTime,
        ColumnType::BigVarBin | ColumnType::BigBinary | ColumnType::Image => LogicalType::Bytes,
        _ => LogicalType::Text,
    }
}

fn build_config(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: Option<&str>,
) -> Result<tiberius::Config> {
    profile.validate()?;
    let mut config = tiberius::Config::new();
    config.host(profile.required_host()?);
    config.port(profile.required_port()?);
    config.encryption(EncryptionLevel::NotSupported);
    if let Some(username) = profile
        .username
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        config.authentication(AuthMethod::sql_server(
            username,
            password
                .filter(|value| !value.is_empty())
                .unwrap_or_default(),
        ));
    }
    if let Some(database) = database.filter(|value| !value.trim().is_empty()) {
        config.database(database);
    }
    Ok(config)
}

async fn connect(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: Option<&str>,
) -> Result<SqlServerClient> {
    let config = build_config(profile, password, database)?;
    let address = config.get_addr();
    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(address),
    )
    .await
    .map_err(|_| AppError::new(ErrorKind::Timeout, "timed out connecting to SQL Server"))?
    .map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to connect to SQL Server")
            .with_detail(error.to_string())
    })?;
    let _ = tcp.set_nodelay(true);

    Client::connect(config, tcp.compat_write())
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to log in to SQL Server")
                .with_detail(error.to_string())
        })
}

fn selected_database(profile: &ConnectionProfile, database: Option<&str>) -> Option<String> {
    database
        .or(profile.database.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn sqlserver_pool_key(profile: &ConnectionProfile) -> SqlServerPoolKey {
    (
        profile.id,
        profile
            .database
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned),
    )
}

fn quote(identifier: &str) -> String {
    format!("'{}'", identifier.replace('\'', "''"))
}

fn query_error(error: tiberius::error::Error) -> AppError {
    AppError::new(ErrorKind::Query, "SQL Server query failed").with_detail(error.to_string())
}
