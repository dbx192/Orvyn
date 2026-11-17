//! SQLite adapter crate.

use async_trait::async_trait;
use db_common::{
    is_result_set_statement, AppError, CellValue, Column, ConnectionId, ConnectionPoolConfig,
    ConnectionPoolStats, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseObject, DatabaseObjectKind, ErrorKind, LogicalType, QueryId, QueryResult,
    RelationalMetadataReader, RelationalQueryExecutor, Result, Row, TableColumn, TableForeignKey,
    TableIndex, TableTrigger,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};

pub const ADAPTER_NAME: &str = "sqlite";

#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteMetadataReader;

#[derive(Debug, Clone, Default)]
pub struct SqliteQueryExecutor {
    pools: Arc<tokio::sync::Mutex<BTreeMap<ConnectionId, Arc<BlockingSqlitePool>>>>,
    transactions: Arc<Mutex<BTreeMap<QueryId, PooledSqliteConnection>>>,
}

impl SqliteQueryExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    async fn pool_for(
        &self,
        profile: &ConnectionProfile,
        config: ConnectionPoolConfig,
    ) -> Result<Arc<BlockingSqlitePool>> {
        if let Some(pool) = self.pools.lock().await.get(&profile.id).cloned() {
            return Ok(pool);
        }

        let pool = Arc::new(BlockingSqlitePool::new(profile, config)?);
        self.pools.lock().await.insert(profile.id, pool.clone());
        Ok(pool)
    }
}

#[derive(Debug)]
struct BlockingSqlitePool {
    connection_id: ConnectionId,
    path: PathBuf,
    config: ConnectionPoolConfig,
    state: Mutex<BlockingPoolState>,
    available: Condvar,
}

#[derive(Debug)]
struct BlockingPoolState {
    idle: Vec<rusqlite::Connection>,
    size: u32,
}

impl BlockingSqlitePool {
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

    fn acquire(self: &Arc<Self>) -> Result<PooledSqliteConnection> {
        let deadline = Instant::now() + self.config.acquire_timeout();
        let mut state = self
            .state
            .lock()
            .map_err(|_| AppError::internal("SQLite pool lock poisoned"))?;

        loop {
            if let Some(conn) = state.idle.pop() {
                return Ok(PooledSqliteConnection {
                    pool: self.clone(),
                    conn: Some(conn),
                });
            }

            if state.size < self.config.max_connections.max(1) {
                state.size += 1;
                drop(state);
                let conn = match sqlite_connection(self.path.clone()) {
                    Ok(conn) => conn,
                    Err(error) => {
                        let mut state = self
                            .state
                            .lock()
                            .map_err(|_| AppError::internal("SQLite pool lock poisoned"))?;
                        state.size = state.size.saturating_sub(1);
                        self.available.notify_one();
                        return Err(error);
                    }
                };
                return Ok(PooledSqliteConnection {
                    pool: self.clone(),
                    conn: Some(conn),
                });
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(AppError::new(
                    ErrorKind::Timeout,
                    "timed out acquiring SQLite connection",
                ));
            }

            let remaining = deadline.saturating_duration_since(now);
            let (guard, timeout) = self
                .available
                .wait_timeout(state, remaining)
                .map_err(|_| AppError::internal("SQLite pool lock poisoned"))?;
            state = guard;

            if timeout.timed_out() {
                return Err(AppError::new(
                    ErrorKind::Timeout,
                    "timed out acquiring SQLite connection",
                ));
            }
        }
    }

    fn release(&self, conn: rusqlite::Connection) {
        if let Ok(mut state) = self.state.lock() {
            state.idle.push(conn);
            self.available.notify_one();
        }
    }

    fn stats(&self) -> Result<ConnectionPoolStats> {
        let state = self
            .state
            .lock()
            .map_err(|_| AppError::internal("SQLite pool lock poisoned"))?;
        Ok(ConnectionPoolStats {
            connection_id: self.connection_id,
            size: state.size,
            idle: state.idle.len() as u32,
            max_connections: self.config.max_connections.max(1),
        })
    }
}

#[derive(Debug)]
struct PooledSqliteConnection {
    pool: Arc<BlockingSqlitePool>,
    conn: Option<rusqlite::Connection>,
}

impl PooledSqliteConnection {
    fn connection(&self) -> Result<&rusqlite::Connection> {
        self.conn
            .as_ref()
            .ok_or_else(|| AppError::internal("SQLite pooled connection missing"))
    }
}

impl Drop for PooledSqliteConnection {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.release(conn);
        }
    }
}

#[async_trait]
impl ConnectionTester for SqliteConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::Sqlite
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;

        let started_at = Instant::now();
        let path = PathBuf::from(profile.required_file_path()?);
        let version = tokio::task::spawn_blocking(move || sqlite_version(path))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "SQLite test task failed")
                    .with_detail(error.to_string())
            })??;

        Ok(
            ConnectionTestReport::success(DataSourceKind::Sqlite, started_at.elapsed().as_millis())
                .with_server_version(version),
        )
    }
}

#[async_trait]
impl RelationalMetadataReader for SqliteMetadataReader {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::Sqlite
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
        _profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        Ok(vec![DatabaseObject {
            kind: DatabaseObjectKind::Schema,
            name: "main".to_string(),
            schema: None,
            parent: None,
        }])
    }

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        sqlite_objects(profile, schema, "table", DatabaseObjectKind::Table).await
    }

    async fn list_views(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        sqlite_objects(profile, schema, "view", DatabaseObjectKind::View).await
    }

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let path = PathBuf::from(profile.required_file_path()?);
        let table = table.to_string();
        tokio::task::spawn_blocking(move || sqlite_columns(path, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "SQLite columns task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn list_indexes(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableIndex>> {
        let path = PathBuf::from(profile.required_file_path()?);
        let table = table.to_string();
        tokio::task::spawn_blocking(move || sqlite_indexes(path, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "SQLite indexes task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn list_foreign_keys(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableForeignKey>> {
        let path = PathBuf::from(profile.required_file_path()?);
        let table = table.to_string();
        tokio::task::spawn_blocking(move || sqlite_foreign_keys(path, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "SQLite foreign keys task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn list_triggers(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableTrigger>> {
        let path = PathBuf::from(profile.required_file_path()?);
        let table = table.to_string();
        tokio::task::spawn_blocking(move || sqlite_triggers(path, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "SQLite triggers task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn get_create_table_sql(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        _database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<String> {
        let path = PathBuf::from(profile.required_file_path()?);
        let table = table.to_string();
        tokio::task::spawn_blocking(move || sqlite_create_table_sql(path, &table))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "SQLite create table task failed")
                    .with_detail(error.to_string())
            })?
    }
}

#[async_trait]
impl RelationalQueryExecutor for SqliteQueryExecutor {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::Sqlite
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
            execute_sqlite_statements(pooled.connection()?, &statements)
        })
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "SQLite query task failed")
                .with_detail(error.to_string())
        })?
    }

    async fn execute_cancellable(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        statements: &[String],
        pool_config: ConnectionPoolConfig,
        cancellation: db_common::QueryCancellation,
    ) -> Result<Vec<QueryResult>> {
        let pool = self.pool_for(profile, pool_config).await?;
        let statements = statements.to_vec();

        tokio::task::spawn_blocking(move || {
            let pooled = pool.acquire()?;
            let connection = pooled.connection()?;
            let interrupt = connection.get_interrupt_handle();
            let _registration = cancellation.register(move || interrupt.interrupt());
            if cancellation.is_cancelled() {
                return Err(cancellation.error());
            }
            let result = execute_sqlite_statements(connection, &statements);
            if cancellation.is_cancelled() {
                Err(cancellation.error())
            } else {
                result
            }
        })
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "SQLite query task failed")
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

    async fn begin_transaction(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
        transaction_id: QueryId,
        pool_config: ConnectionPoolConfig,
    ) -> Result<()> {
        let pool = self.pool_for(profile, pool_config).await?;
        let transactions = self.transactions.clone();
        tokio::task::spawn_blocking(move || {
            let pooled = pool.acquire()?;
            pooled
                .connection()?
                .execute("BEGIN", [])
                .map_err(sqlite_query_error)?;
            transactions
                .lock()
                .map_err(|_| AppError::internal("SQLite transaction lock poisoned"))?
                .insert(transaction_id, pooled);
            Ok(())
        })
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "SQLite transaction task failed")
                .with_detail(error.to_string())
        })?
    }

    async fn execute_in_transaction(
        &self,
        transaction_id: QueryId,
        statements: &[String],
    ) -> Result<Vec<QueryResult>> {
        let statements = statements.to_vec();
        let transactions = self.transactions.clone();
        tokio::task::spawn_blocking(move || {
            let mut transactions = transactions
                .lock()
                .map_err(|_| AppError::internal("SQLite transaction lock poisoned"))?;
            let pooled = transactions
                .get_mut(&transaction_id)
                .ok_or_else(|| AppError::validation("transaction is not active"))?;
            execute_sqlite_statements(pooled.connection()?, &statements)
        })
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "SQLite transaction task failed")
                .with_detail(error.to_string())
        })?
    }

    async fn execute_in_transaction_cancellable(
        &self,
        transaction_id: QueryId,
        statements: &[String],
        cancellation: db_common::QueryCancellation,
    ) -> Result<Vec<QueryResult>> {
        let statements = statements.to_vec();
        let transactions = self.transactions.clone();
        tokio::task::spawn_blocking(move || {
            let mut transactions = transactions
                .lock()
                .map_err(|_| AppError::internal("SQLite transaction lock poisoned"))?;
            let pooled = transactions
                .get_mut(&transaction_id)
                .ok_or_else(|| AppError::validation("transaction is not active"))?;
            let connection = pooled.connection()?;
            let interrupt = connection.get_interrupt_handle();
            let _registration = cancellation.register(move || interrupt.interrupt());
            if cancellation.is_cancelled() {
                return Err(cancellation.error());
            }
            let result = execute_sqlite_statements(connection, &statements);
            if cancellation.is_cancelled() {
                Err(cancellation.error())
            } else {
                result
            }
        })
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "SQLite transaction task failed")
                .with_detail(error.to_string())
        })?
    }

    async fn commit_transaction(&self, transaction_id: QueryId) -> Result<()> {
        let transactions = self.transactions.clone();
        tokio::task::spawn_blocking(move || {
            let pooled = transactions
                .lock()
                .map_err(|_| AppError::internal("SQLite transaction lock poisoned"))?
                .remove(&transaction_id)
                .ok_or_else(|| AppError::validation("transaction is not active"))?;
            pooled
                .connection()?
                .execute("COMMIT", [])
                .map_err(sqlite_query_error)?;
            Ok(())
        })
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "SQLite transaction task failed")
                .with_detail(error.to_string())
        })?
    }

    async fn rollback_transaction(&self, transaction_id: QueryId) -> Result<()> {
        let transactions = self.transactions.clone();
        tokio::task::spawn_blocking(move || {
            let pooled = transactions
                .lock()
                .map_err(|_| AppError::internal("SQLite transaction lock poisoned"))?
                .remove(&transaction_id)
                .ok_or_else(|| AppError::validation("transaction is not active"))?;
            pooled
                .connection()?
                .execute("ROLLBACK", [])
                .map_err(sqlite_query_error)?;
            Ok(())
        })
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "SQLite transaction task failed")
                .with_detail(error.to_string())
        })?
    }
}

fn execute_sqlite_statements(
    conn: &rusqlite::Connection,
    statements: &[String],
) -> Result<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());

    for statement in statements {
        let started_at = Instant::now();
        if is_result_set_statement(statement) {
            results.push(sqlite_query_result(conn, statement)?.with_elapsed(started_at.elapsed()));
            continue;
        }

        let affected_rows = conn.execute(statement, []).map_err(sqlite_query_error)?;
        let mut result = QueryResult::empty(statement).with_elapsed(started_at.elapsed());
        result.affected_rows = affected_rows as u64;
        results.push(result);
    }

    Ok(results)
}

fn sqlite_version(path: PathBuf) -> Result<String> {
    if path != std::path::Path::new(":memory:") && !path.exists() {
        return Err(AppError::new(
            ErrorKind::Validation,
            "SQLite database file does not exist",
        ));
    }

    let conn = rusqlite::Connection::open(path).map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to open SQLite database")
            .with_detail(error.to_string())
    })?;

    conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .map_err(|error| {
            AppError::new(ErrorKind::Query, "failed to test SQLite connection")
                .with_detail(error.to_string())
        })
}

async fn sqlite_objects(
    profile: &ConnectionProfile,
    schema: Option<&str>,
    object_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let path = PathBuf::from(profile.required_file_path()?);
    let schema = schema.unwrap_or("main").to_string();
    let object_type = object_type.to_string();
    tokio::task::spawn_blocking(move || sqlite_objects_blocking(path, &schema, &object_type, kind))
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Internal, "SQLite objects task failed")
                .with_detail(error.to_string())
        })?
}

fn sqlite_objects_blocking(
    path: PathBuf,
    schema: &str,
    object_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let conn = sqlite_connection(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = ?1 AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .map_err(sqlite_query_error)?;
    let rows = stmt
        .query_map([object_type], |row| row.get::<_, String>(0))
        .map_err(sqlite_query_error)?;

    let mut objects = Vec::new();
    for row in rows {
        objects.push(DatabaseObject {
            kind,
            name: row.map_err(sqlite_query_error)?,
            schema: Some(schema.to_string()),
            parent: None,
        });
    }
    Ok(objects)
}

fn sqlite_columns(path: PathBuf, table: &str) -> Result<Vec<TableColumn>> {
    let conn = sqlite_connection(path)?;
    let sql = format!("PRAGMA table_info({})", quote_sqlite_identifier(table));
    let mut stmt = conn.prepare(&sql).map_err(sqlite_query_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(TableColumn {
                name: row.get(1)?,
                database_type: row.get(2)?,
                nullable: row.get::<_, i64>(3)? == 0,
                default_value: row.get(4)?,
                primary_key: row.get::<_, i64>(5)? > 0,
                comment: None,
            })
        })
        .map_err(sqlite_query_error)?;

    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(sqlite_query_error)?);
    }
    Ok(columns)
}

fn sqlite_indexes(path: PathBuf, table: &str) -> Result<Vec<TableIndex>> {
    let conn = sqlite_connection(path)?;
    let sql = format!("PRAGMA index_list({})", quote_sqlite_identifier(table));
    let mut stmt = conn.prepare(&sql).map_err(sqlite_query_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? == 1,
                row.get::<_, String>(3)? == "pk",
            ))
        })
        .map_err(sqlite_query_error)?;

    let mut indexes = Vec::new();
    for row in rows {
        let (name, unique, primary) = row.map_err(sqlite_query_error)?;
        let columns = sqlite_index_columns(&conn, &name)?;
        indexes.push(TableIndex {
            name,
            columns,
            unique,
            primary,
        });
    }
    Ok(indexes)
}

fn sqlite_index_columns(conn: &rusqlite::Connection, index_name: &str) -> Result<Vec<String>> {
    let sql = format!("PRAGMA index_info({})", quote_sqlite_identifier(index_name));
    let mut stmt = conn.prepare(&sql).map_err(sqlite_query_error)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(2))
        .map_err(sqlite_query_error)?;

    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(sqlite_query_error)?);
    }
    Ok(columns)
}

fn sqlite_foreign_keys(path: PathBuf, table: &str) -> Result<Vec<TableForeignKey>> {
    let conn = sqlite_connection(path)?;
    let sql = format!(
        "PRAGMA foreign_key_list({})",
        quote_sqlite_identifier(table)
    );
    let mut stmt = conn.prepare(&sql).map_err(sqlite_query_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(sqlite_query_error)?;

    let mut fks: Vec<TableForeignKey> = Vec::new();
    for row in rows {
        let (id, ref_table, from, to, on_update, on_delete) = row.map_err(sqlite_query_error)?;
        let fk_name = format!("fk_{id}");
        if let Some(fk) = fks.iter_mut().find(|fk| fk.name == fk_name) {
            fk.columns.push(from);
            fk.referenced_columns.push(to);
        } else {
            fks.push(TableForeignKey {
                name: fk_name,
                columns: vec![from],
                referenced_table: ref_table,
                referenced_columns: vec![to],
                on_update: if on_update == "NO ACTION" {
                    None
                } else {
                    Some(on_update)
                },
                on_delete: if on_delete == "NO ACTION" {
                    None
                } else {
                    Some(on_delete)
                },
            });
        }
    }
    Ok(fks)
}

fn sqlite_triggers(path: PathBuf, table: &str) -> Result<Vec<TableTrigger>> {
    let conn = sqlite_connection(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT name, sql FROM sqlite_master \
             WHERE type = 'trigger' AND tbl_name = ?1 \
             ORDER BY name",
        )
        .map_err(sqlite_query_error)?;
    let rows = stmt
        .query_map([table], |row| {
            let name: String = row.get(0)?;
            let sql: Option<String> = row.get(1)?;
            Ok((name, sql))
        })
        .map_err(sqlite_query_error)?;

    let mut triggers = Vec::new();
    for row in rows {
        let (name, sql) = row.map_err(sqlite_query_error)?;
        let (timing, event) = parse_sqlite_trigger_sql(sql.as_deref().unwrap_or_default());
        triggers.push(TableTrigger {
            name,
            event,
            timing,
            statement: sql,
        });
    }
    Ok(triggers)
}

fn parse_sqlite_trigger_sql(sql: &str) -> (String, String) {
    let upper = sql.to_uppercase();
    let timing = if upper.contains(" BEFORE ") {
        "BEFORE"
    } else if upper.contains(" AFTER ") {
        "AFTER"
    } else if upper.contains(" INSTEAD OF ") {
        "INSTEAD OF"
    } else {
        ""
    };
    let event = if upper.contains(" INSERT ") {
        "INSERT"
    } else if upper.contains(" UPDATE ") {
        "UPDATE"
    } else if upper.contains(" DELETE ") {
        "DELETE"
    } else {
        ""
    };
    (timing.to_string(), event.to_string())
}

fn sqlite_create_table_sql(path: PathBuf, table: &str) -> Result<String> {
    let conn = sqlite_connection(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT sql FROM sqlite_master \
             WHERE type = 'table' AND name = ?1",
        )
        .map_err(sqlite_query_error)?;
    let mut rows = stmt.query([table]).map_err(sqlite_query_error)?;

    if let Some(row) = rows.next().map_err(sqlite_query_error)? {
        let ddl: String = row.get(0).map_err(sqlite_query_error)?;
        let mut result = ddl;
        if !result.ends_with(';') {
            result.push(';');
        }

        let mut index_stmt = conn
            .prepare(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'index' AND tbl_name = ?1 \
                   AND name NOT LIKE 'sqlite_%' \
                   AND sql IS NOT NULL \
                 ORDER BY name",
            )
            .map_err(sqlite_query_error)?;
        let index_rows = index_stmt
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(sqlite_query_error)?;

        for index_row in index_rows {
            let idx_sql = index_row.map_err(sqlite_query_error)?;
            result.push('\n');
            result.push_str(&idx_sql);
            result.push(';');
        }

        Ok(result)
    } else {
        Err(AppError::new(
            ErrorKind::NotFound,
            format!("table {table} not found"),
        ))
    }
}

fn sqlite_connection(path: PathBuf) -> Result<rusqlite::Connection> {
    if path != std::path::Path::new(":memory:") && !path.exists() {
        return Err(AppError::new(
            ErrorKind::Validation,
            "SQLite database file does not exist",
        ));
    }

    rusqlite::Connection::open(path).map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to open SQLite database")
            .with_detail(error.to_string())
    })
}

fn sqlite_query_error(error: rusqlite::Error) -> AppError {
    AppError::new(ErrorKind::Query, "failed to query SQLite metadata")
        .with_detail(error.to_string())
}

fn quote_sqlite_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn sqlite_query_result(conn: &rusqlite::Connection, statement: &str) -> Result<QueryResult> {
    let mut stmt = conn.prepare(statement).map_err(sqlite_query_error)?;
    let column_count = stmt.column_count();
    let columns = (0..column_count)
        .map(|index| Column {
            name: stmt.column_name(index).unwrap_or_default().to_string(),
            database_type: String::new(),
            logical_type: LogicalType::Unknown,
            nullable: true,
            primary_key: false,
        })
        .collect();

    let rows = stmt
        .query_map([], |row| {
            let values = (0..column_count)
                .map(|index| sqlite_cell_value(row, index))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Row::new(values))
        })
        .map_err(sqlite_query_error)?;

    let mut result_rows = Vec::new();
    for row in rows {
        result_rows.push(row.map_err(sqlite_query_error)?);
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

fn sqlite_cell_value(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<CellValue> {
    let value = row.get_ref(index)?;
    Ok(match value {
        rusqlite::types::ValueRef::Null => CellValue::Null,
        rusqlite::types::ValueRef::Integer(value) => CellValue::Int(value),
        rusqlite::types::ValueRef::Real(value) => CellValue::Float(value),
        rusqlite::types::ValueRef::Text(value) => {
            CellValue::Text(String::from_utf8_lossy(value).to_string())
        }
        rusqlite::types::ValueRef::Blob(value) => CellValue::Bytes(value.to_vec()),
    })
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[tokio::test]
    async fn interrupts_long_query_and_allows_connection_reuse() {
        let executor = SqliteQueryExecutor::new();
        let profile = ConnectionProfile::builder(DataSourceKind::Sqlite, "cancel test")
            .file_path(":memory:")
            .build()
            .unwrap();
        let cancellation = db_common::QueryCancellation::default();
        let cancel = cancellation.clone();
        let config = ConnectionPoolConfig {
            max_connections: 1,
            ..Default::default()
        };
        let statements = ["WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n".into()];
        let query = executor.execute_cancellable(&profile, None, &statements, config, cancellation);
        let trigger = async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            cancel.cancel();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(query, trigger)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        let result = executor
            .execute(&profile, None, &["SELECT 42".into()], config)
            .await
            .unwrap();
        assert_eq!(result[0].rows[0].values[0], CellValue::Int(42));
    }
}
