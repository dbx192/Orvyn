//! DuckDB adapter crate.

use async_trait::async_trait;
use db_common::{
    is_result_set_statement, AppError, CellValue, Column, ConnectionId, ConnectionPoolConfig,
    ConnectionPoolStats, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseFunction, DatabaseObject, DatabaseObjectKind, ErrorKind, LogicalType, QueryResult,
    RelationalMetadataReader, RelationalQueryExecutor, Result, Row, TableColumn, TableForeignKey,
    TableIndex, TableTrigger,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};

pub const ADAPTER_NAME: &str = "duckdb";

#[derive(Debug, Clone, Copy, Default)]
pub struct DuckDbConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct DuckDbMetadataReader;

#[derive(Debug, Clone, Default)]
pub struct DuckDbQueryExecutor {
    pools: Arc<tokio::sync::Mutex<BTreeMap<ConnectionId, Arc<BlockingDuckDbPool>>>>,
}

impl DuckDbQueryExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    async fn pool_for(
        &self,
        profile: &ConnectionProfile,
        config: ConnectionPoolConfig,
    ) -> Result<Arc<BlockingDuckDbPool>> {
        if let Some(pool) = self.pools.lock().await.get(&profile.id).cloned() {
            return Ok(pool);
        }

        let pool = Arc::new(BlockingDuckDbPool::new(profile, config)?);
        self.pools.lock().await.insert(profile.id, pool.clone());
        Ok(pool)
    }
}

#[derive(Debug)]
struct BlockingDuckDbPool {
    connection_id: ConnectionId,
    path: PathBuf,
    config: ConnectionPoolConfig,
    state: Mutex<BlockingPoolState>,
    available: Condvar,
}

#[derive(Debug)]
struct BlockingPoolState {
    idle: Vec<duckdb::Connection>,
    size: u32,
}

impl BlockingDuckDbPool {
    fn new(profile: &ConnectionProfile, config: ConnectionPoolConfig) -> Result<Self> {
        Ok(Self {
            connection_id: profile.id,
            path: PathBuf::from(profile.required_file_path()?),
            config,
            state: Mutex::new(BlockingPoolState {
                idle: Vec::new(),
                size: 0,
            }),
            available: Condvar::new(),
        })
    }

    fn acquire(self: &Arc<Self>) -> Result<PooledDuckDbConnection> {
        let deadline = Instant::now() + self.config.acquire_timeout();
        let mut state = self
            .state
            .lock()
            .map_err(|_| AppError::internal("DuckDB pool lock poisoned"))?;

        loop {
            if let Some(conn) = state.idle.pop() {
                return Ok(PooledDuckDbConnection {
                    pool: self.clone(),
                    conn: Some(conn),
                });
            }

            if state.size < self.config.max_connections.max(1) {
                state.size += 1;
                drop(state);
                let conn = duckdb_connection(self.path.clone())?;
                return Ok(PooledDuckDbConnection {
                    pool: self.clone(),
                    conn: Some(conn),
                });
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(AppError::new(
                    ErrorKind::Timeout,
                    "timed out acquiring DuckDB connection",
                ));
            }

            let remaining = deadline.saturating_duration_since(now);
            let (guard, timeout) = self
                .available
                .wait_timeout(state, remaining)
                .map_err(|_| AppError::internal("DuckDB pool lock poisoned"))?;
            state = guard;

            if timeout.timed_out() {
                return Err(AppError::new(
                    ErrorKind::Timeout,
                    "timed out acquiring DuckDB connection",
                ));
            }
        }
    }

    fn release(&self, conn: duckdb::Connection) {
        if let Ok(mut state) = self.state.lock() {
            state.idle.push(conn);
            self.available.notify_one();
        }
    }

    fn stats(&self) -> Result<ConnectionPoolStats> {
        let state = self
            .state
            .lock()
            .map_err(|_| AppError::internal("DuckDB pool lock poisoned"))?;
        Ok(ConnectionPoolStats {
            connection_id: self.connection_id,
            size: state.size,
            idle: state.idle.len() as u32,
            max_connections: self.config.max_connections.max(1),
        })
    }
}

struct PooledDuckDbConnection {
    pool: Arc<BlockingDuckDbPool>,
    conn: Option<duckdb::Connection>,
}

impl PooledDuckDbConnection {
    fn connection(&self) -> &duckdb::Connection {
        self.conn
            .as_ref()
            .expect("DuckDB pooled connection missing")
    }
}

impl Drop for PooledDuckDbConnection {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.release(conn);
        }
    }
}

#[async_trait]
impl ConnectionTester for DuckDbConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::DuckDb
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;

        let started_at = Instant::now();
        let path = PathBuf::from(profile.required_file_path()?);
        let version = tokio::task::spawn_blocking(move || duckdb_version(path))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "DuckDB test task failed")
                    .with_detail(error.to_string())
            })??;

        Ok(
            ConnectionTestReport::success(DataSourceKind::DuckDb, started_at.elapsed().as_millis())
                .with_server_version(version),
        )
    }
}

#[async_trait]
impl RelationalMetadataReader for DuckDbMetadataReader {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::DuckDb
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        profile.validate()?;
        Ok(vec![DatabaseObject {
            kind: DatabaseObjectKind::Database,
            name: profile.required_file_path()?.to_string(),
            schema: None,
            parent: None,
        }])
    }

    async fn list_schemas(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let path = PathBuf::from(profile.required_file_path()?);
        tokio::task::spawn_blocking(move || duckdb_schemas(path))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "DuckDB schemas task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        duckdb_objects(profile, schema, "BASE TABLE", DatabaseObjectKind::Table).await
    }

    async fn list_views(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        duckdb_objects(profile, schema, "VIEW", DatabaseObjectKind::View).await
    }

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let path = PathBuf::from(profile.required_file_path()?);
        let schema = schema.unwrap_or("main").to_string();
        let table = table.to_string();
        tokio::task::spawn_blocking(move || duckdb_columns(path, &schema, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "DuckDB columns task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn list_indexes(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableIndex>> {
        let path = PathBuf::from(profile.required_file_path()?);
        let schema = schema.unwrap_or("main").to_string();
        let table = table.to_string();
        tokio::task::spawn_blocking(move || duckdb_indexes(path, &schema, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "DuckDB indexes task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn list_foreign_keys(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableForeignKey>> {
        let path = PathBuf::from(profile.required_file_path()?);
        let schema = schema.unwrap_or("main").to_string();
        let table = table.to_string();
        tokio::task::spawn_blocking(move || duckdb_foreign_keys(path, &schema, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "DuckDB foreign keys task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn list_functions(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseFunction>> {
        let path = PathBuf::from(profile.required_file_path()?);
        let schema = schema.unwrap_or("main").to_string();
        tokio::task::spawn_blocking(move || duckdb_functions(path, &schema))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "DuckDB functions task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn get_create_table_sql(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<String> {
        let path = PathBuf::from(profile.required_file_path()?);
        let schema = schema.unwrap_or("main").to_string();
        let table = table.to_string();
        tokio::task::spawn_blocking(move || duckdb_create_table_sql(path, &schema, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "DuckDB create table task failed")
                    .with_detail(error.to_string())
            })?
    }
}

#[async_trait]
impl RelationalQueryExecutor for DuckDbQueryExecutor {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::DuckDb
    }

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        statements: &[String],
        pool_config: ConnectionPoolConfig,
    ) -> Result<Vec<QueryResult>> {
        let pool = self.pool_for(profile, pool_config).await?;
        let statements = statements.to_vec();

        tokio::task::spawn_blocking(move || {
            let pooled = pool.acquire()?;
            let conn = pooled.connection();
            let mut results = Vec::with_capacity(statements.len());

            for statement in statements {
                let started_at = Instant::now();
                if is_result_set_statement(&statement) {
                    results.push(
                        duckdb_query_result(conn, &statement)?.with_elapsed(started_at.elapsed()),
                    );
                    continue;
                }

                let affected_rows = conn.execute(&statement, []).map_err(duckdb_query_error)?;
                let mut result = QueryResult::empty(statement).with_elapsed(started_at.elapsed());
                result.affected_rows = affected_rows as u64;
                results.push(result);
            }

            Ok(results)
        })
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "DuckDB query task failed")
                .with_detail(error.to_string())
        })?
    }

    async fn pool_stats(&self, connection_id: ConnectionId) -> Result<Option<ConnectionPoolStats>> {
        let Some(pool) = self.pools.lock().await.get(&connection_id).cloned() else {
            return Ok(None);
        };
        pool.stats().map(Some)
    }

    async fn close_pool(&self, connection_id: ConnectionId) -> Result<()> {
        self.pools.lock().await.remove(&connection_id);
        Ok(())
    }
}

fn duckdb_version(path: PathBuf) -> Result<String> {
    if !path.exists() {
        return Err(AppError::new(
            ErrorKind::Validation,
            "DuckDB database file does not exist",
        ));
    }

    let conn = duckdb::Connection::open(path).map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to open DuckDB database")
            .with_detail(error.to_string())
    })?;

    conn.query_row("SELECT version()", [], |row| row.get(0))
        .map_err(|error| {
            AppError::new(ErrorKind::Query, "failed to test DuckDB connection")
                .with_detail(error.to_string())
        })
}

fn duckdb_schemas(path: PathBuf) -> Result<Vec<DatabaseObject>> {
    let conn = duckdb_connection(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name NOT IN ('information_schema', 'pg_catalog') \
             ORDER BY schema_name",
        )
        .map_err(duckdb_query_error)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(duckdb_query_error)?;

    let mut schemas = Vec::new();
    for row in rows {
        schemas.push(DatabaseObject {
            kind: DatabaseObjectKind::Schema,
            name: row.map_err(duckdb_query_error)?,
            schema: None,
            parent: None,
        });
    }
    Ok(schemas)
}

async fn duckdb_objects(
    profile: &ConnectionProfile,
    schema: Option<&str>,
    table_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let path = PathBuf::from(profile.required_file_path()?);
    let schema = schema.unwrap_or("main").to_string();
    let table_type = table_type.to_string();
    tokio::task::spawn_blocking(move || duckdb_objects_blocking(path, &schema, &table_type, kind))
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "DuckDB objects task failed")
                .with_detail(error.to_string())
        })?
}

fn duckdb_objects_blocking(
    path: PathBuf,
    schema: &str,
    table_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let conn = duckdb_connection(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = ?1 AND table_type = ?2 \
             ORDER BY table_name",
        )
        .map_err(duckdb_query_error)?;
    let rows = stmt
        .query_map([schema, table_type], |row| row.get::<_, String>(0))
        .map_err(duckdb_query_error)?;

    let mut objects = Vec::new();
    for row in rows {
        objects.push(DatabaseObject {
            kind,
            name: row.map_err(duckdb_query_error)?,
            schema: Some(schema.to_string()),
            parent: None,
        });
    }
    Ok(objects)
}

fn duckdb_columns(path: PathBuf, schema: &str, table: &str) -> Result<Vec<TableColumn>> {
    let conn = duckdb_connection(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT column_name, data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = ?1 AND table_name = ?2 \
             ORDER BY ordinal_position",
        )
        .map_err(duckdb_query_error)?;
    let rows = stmt
        .query_map([schema, table], |row| {
            Ok(TableColumn {
                name: row.get(0)?,
                database_type: row.get(1)?,
                nullable: row.get::<_, String>(2)?.eq_ignore_ascii_case("YES"),
                default_value: row.get(3)?,
                primary_key: false,
                comment: None,
            })
        })
        .map_err(duckdb_query_error)?;

    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(duckdb_query_error)?);
    }
    Ok(columns)
}

fn duckdb_indexes(path: PathBuf, schema: &str, table: &str) -> Result<Vec<TableIndex>> {
    let conn = duckdb_connection(path)?;
    let Ok(mut stmt) = conn.prepare(
        "SELECT index_name, is_unique, expressions \
         FROM duckdb_indexes() \
         WHERE schema_name = ?1 AND table_name = ?2 \
         ORDER BY index_name",
    ) else {
        return Ok(Vec::new());
    };
    let rows = stmt
        .query_map([schema, table], |row| {
            let expressions: String = row.get(2)?;
            Ok(TableIndex {
                name: row.get(0)?,
                unique: row.get(1)?,
                primary: false,
                columns: expressions
                    .split(',')
                    .map(|value| value.trim().trim_matches('"').to_string())
                    .filter(|value| !value.is_empty())
                    .collect(),
            })
        })
        .map_err(duckdb_query_error)?;

    let mut indexes = Vec::new();
    for row in rows {
        indexes.push(row.map_err(duckdb_query_error)?);
    }
    Ok(indexes)
}

fn duckdb_foreign_keys(path: PathBuf, schema: &str, table: &str) -> Result<Vec<TableForeignKey>> {
    let conn = duckdb_connection(path)?;
    let mut stmt = match conn.prepare(
        "SELECT constraint_name, column_name, foreign_table_name, foreign_column_name \
         FROM duckdb_constraints() \
         WHERE schema_name = ?1 AND table_name = ?2 AND constraint_type = 'FOREIGN KEY' \
         ORDER BY constraint_name, column_index",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Ok(Vec::new()),
    };
    let rows = match stmt.query_map([schema, table], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => return Ok(Vec::new()),
    };

    let mut fks: Vec<TableForeignKey> = Vec::new();
    for row in rows {
        let (name, col, ref_table, ref_col) = row.map_err(duckdb_query_error)?;
        if let Some(fk) = fks.iter_mut().find(|fk| fk.name == name) {
            fk.columns.push(col);
            fk.referenced_columns.push(ref_col);
        } else {
            fks.push(TableForeignKey {
                name,
                columns: vec![col],
                referenced_table: ref_table,
                referenced_columns: vec![ref_col],
                on_update: None,
                on_delete: None,
            });
        }
    }
    Ok(fks)
}

fn duckdb_functions(path: PathBuf, schema: &str) -> Result<Vec<DatabaseFunction>> {
    let conn = duckdb_connection(path)?;
    let mut stmt = match conn.prepare(
        "SELECT function_name, return_type, schema_name \
         FROM duckdb_functions() \
         WHERE schema_name = ?1 \
         ORDER BY function_name",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Ok(Vec::new()),
    };
    let rows = match stmt.query_map([schema], |row| {
        Ok(DatabaseFunction {
            name: row.get::<_, String>(0)?,
            return_type: row.get::<_, Option<String>>(1)?,
            language: None,
            kind: "FUNCTION".to_string(),
        })
    }) {
        Ok(rows) => rows,
        Err(_) => return Ok(Vec::new()),
    };

    let mut functions = Vec::new();
    for row in rows {
        functions.push(row.map_err(duckdb_query_error)?);
    }
    Ok(functions)
}

fn duckdb_create_table_sql(path: PathBuf, schema: &str, table: &str) -> Result<String> {
    let conn = duckdb_connection(path)?;

    let mut col_stmt = conn
        .prepare(
            "SELECT column_name, data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = ?1 AND table_name = ?2 \
             ORDER BY ordinal_position",
        )
        .map_err(duckdb_query_error)?;
    let col_rows = col_stmt
        .query_map([schema, table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(duckdb_query_error)?;

    let mut col_defs: Vec<String> = Vec::new();
    let mut table_exists = false;
    for row in col_rows {
        table_exists = true;
        let (name, dtype, nullable, default) = row.map_err(duckdb_query_error)?;
        let mut col_def = format!("    \"{}\" {}", name, dtype);
        if let Some(default_value) = default {
            col_def.push_str(&format!(" DEFAULT {}", default_value));
        }
        if !nullable.eq_ignore_ascii_case("YES") {
            col_def.push_str(" NOT NULL");
        }
        col_defs.push(col_def);
    }

    if !table_exists {
        return Err(AppError::new(
            ErrorKind::NotFound,
            format!("table {}.{} not found", schema, table),
        ));
    }

    let mut ddl = format!("CREATE TABLE \"{}\".\"{}\" (\n", schema, table);
    ddl.push_str(&col_defs.join(",\n"));
    ddl.push_str("\n);");

    let idx_result: Result<Vec<TableIndex>> = duckdb_indexes(path.clone(), schema, table);
    if let Ok(indexes) = idx_result {
        for idx in &indexes {
            let cols: Vec<String> = idx.columns.iter().map(|c| format!("\"{}\"", c)).collect();
            let unique = if idx.unique { " UNIQUE" } else { "" };
            ddl.push_str(&format!(
                "\nCREATE{} INDEX \"{}\" ON \"{}\".\"{}\" ({});",
                unique, idx.name, schema, table, cols.join(", ")
            ));
        }
    }

    Ok(ddl)
}

fn duckdb_connection(path: PathBuf) -> Result<duckdb::Connection> {
    if !path.exists() {
        return Err(AppError::new(
            ErrorKind::Validation,
            "DuckDB database file does not exist",
        ));
    }

    duckdb::Connection::open(path).map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to open DuckDB database")
            .with_detail(error.to_string())
    })
}

fn duckdb_query_error(error: duckdb::Error) -> AppError {
    AppError::new(ErrorKind::Query, "failed to query DuckDB metadata")
        .with_detail(error.to_string())
}

fn duckdb_query_result(conn: &duckdb::Connection, statement: &str) -> Result<QueryResult> {
    let mut stmt = conn.prepare(statement).map_err(duckdb_query_error)?;
    let column_count = stmt.column_count();
    let columns = (0..column_count)
        .map(|index| Column {
            name: stmt
                .column_name(index)
                .map_or_else(|_| String::new(), ToString::to_string),
            database_type: String::new(),
            logical_type: LogicalType::Unknown,
            nullable: true,
            primary_key: false,
        })
        .collect();

    let rows = stmt
        .query_map([], |row| {
            let values = (0..column_count)
                .map(|index| duckdb_cell_value(row, index))
                .collect::<duckdb::Result<Vec<_>>>()?;
            Ok(Row::new(values))
        })
        .map_err(duckdb_query_error)?;

    let mut result_rows = Vec::new();
    for row in rows {
        result_rows.push(row.map_err(duckdb_query_error)?);
    }

    Ok(QueryResult {
        statement: statement.to_string(),
        columns,
        rows: result_rows,
        affected_rows: 0,
        execution_time_ms: 0,
        notices: Vec::new(),
        metadata: BTreeMap::new(),
    })
}

fn duckdb_cell_value(row: &duckdb::Row<'_>, index: usize) -> duckdb::Result<CellValue> {
    let value = row.get_ref(index)?;
    Ok(match value {
        duckdb::types::ValueRef::Null => CellValue::Null,
        duckdb::types::ValueRef::Boolean(value) => CellValue::Bool(value),
        duckdb::types::ValueRef::TinyInt(value) => CellValue::Int(i64::from(value)),
        duckdb::types::ValueRef::SmallInt(value) => CellValue::Int(i64::from(value)),
        duckdb::types::ValueRef::Int(value) => CellValue::Int(i64::from(value)),
        duckdb::types::ValueRef::BigInt(value) => CellValue::Int(value),
        duckdb::types::ValueRef::HugeInt(value) => CellValue::Decimal(value.to_string()),
        duckdb::types::ValueRef::UTinyInt(value) => CellValue::UInt(u64::from(value)),
        duckdb::types::ValueRef::USmallInt(value) => CellValue::UInt(u64::from(value)),
        duckdb::types::ValueRef::UInt(value) => CellValue::UInt(u64::from(value)),
        duckdb::types::ValueRef::UBigInt(value) => CellValue::UInt(value),
        duckdb::types::ValueRef::Float(value) => CellValue::Float(f64::from(value)),
        duckdb::types::ValueRef::Double(value) => CellValue::Float(value),
        duckdb::types::ValueRef::Decimal(value) => CellValue::Decimal(value.to_string()),
        duckdb::types::ValueRef::Text(value) => {
            CellValue::Text(String::from_utf8_lossy(value).to_string())
        }
        duckdb::types::ValueRef::Blob(value) => CellValue::Bytes(value.to_vec()),
        duckdb::types::ValueRef::Enum(_, _) => {
            CellValue::Text(value.as_str().unwrap_or_default().to_string())
        }
        duckdb::types::ValueRef::Timestamp(unit, value) => {
            CellValue::Text(format!("{unit:?}({value})"))
        }
        duckdb::types::ValueRef::Date32(value) => CellValue::Text(value.to_string()),
        duckdb::types::ValueRef::Time64(unit, value) => {
            CellValue::Text(format!("{unit:?}({value})"))
        }
        duckdb::types::ValueRef::Interval {
            months,
            days,
            nanos,
        } => CellValue::Text(format!("{months} months {days} days {nanos} ns")),
        duckdb::types::ValueRef::List(_, _)
        | duckdb::types::ValueRef::Struct(_, _)
        | duckdb::types::ValueRef::Array(_, _)
        | duckdb::types::ValueRef::Map(_, _)
        | duckdb::types::ValueRef::Union(_, _) => {
            CellValue::Text(format!("{:?}", value.to_owned()))
        }
    })
}
