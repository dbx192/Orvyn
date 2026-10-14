//! Query executor for the ODBC bridge.
//!
//! One live ODBC connection per profile id is kept behind a `std::sync::Mutex`
//! (odbc-api connections are `Send` but not `Sync`, and every driver call is
//! blocking, so the lock is only ever taken on blocking threads). Transactions
//! reuse the same pool slot so BEGIN/COMMIT/ROLLBACK hit a single session.
//!
//! Rows are fetched text-first (`SQLGetData` into a byte buffer) and re-typed
//! per column in [`crate::value`], which sidesteps driver quirks such as
//! Oracle's missing 64-bit integer support.

use crate::connect::{self, connect_blocking, map_odbc_error_for};
use crate::kind::VendorDialect;
use crate::value::{cell_from_text, database_type_label, logical_type};
use async_trait::async_trait;
use db_common::{
    is_result_set_statement, AppError, Column, ConnectionId, ConnectionPoolConfig,
    ConnectionPoolStats, ConnectionProfile, DataSourceKind, ErrorKind, QueryId, QueryResult,
    RelationalQueryExecutor, Result, Row,
};
use odbc_api::{Connection, Cursor};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// ODBC connections cannot switch database after login (unlike a MySQL
/// `USE`), so the pool key is just the profile id; per-call `database`
/// parameters are ignored and selection happens in the connection string.
type PoolKey = ConnectionId;

#[derive(Debug)]
pub struct OdbcPool {
    client: Mutex<Option<Connection<'static>>>,
}

impl OdbcPool {
    fn new(profile: &ConnectionProfile) -> Result<Self> {
        // Validate eagerly so a bad profile surfaces before any I/O.
        profile.validate()?;
        Ok(Self {
            client: Mutex::new(None),
        })
    }

    /// Borrow (and lazily establish) the pooled connection. The std mutex
    /// must only be locked on blocking threads and is held for the whole
    /// statement batch — one connection cannot interleave queries anyway.
    fn acquire(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<MutexGuard<'_, Option<Connection<'static>>>> {
        let mut guard = self
            .client
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC pool mutex poisoned"))?;
        if guard.is_none() {
            *guard = Some(connect_blocking(profile, password)?);
        }
        Ok(guard)
    }

    fn connection(
        &self,
    ) -> Result<MutexGuard<'_, Option<Connection<'static>>>> {
        self.client
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC pool mutex poisoned"))
    }
}

#[derive(Debug)]
pub struct OdbcQueryExecutor {
    kind: DataSourceKind,
    pools: Arc<Mutex<BTreeMap<PoolKey, Arc<OdbcPool>>>>,
    transactions: Arc<Mutex<BTreeMap<QueryId, Arc<OdbcPool>>>>,
}

impl OdbcQueryExecutor {
    pub fn new(kind: DataSourceKind) -> Self {
        Self {
            kind,
            pools: Arc::default(),
            transactions: Arc::default(),
        }
    }

    async fn pool_for(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
    ) -> Result<Arc<OdbcPool>> {
        if let Some(pool) = self
            .pools
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC pool map mutex poisoned"))?
            .get(&profile.id)
            .cloned()
        {
            return Ok(pool);
        }

        let pool = Arc::new(OdbcPool::new(profile)?);
        self.pools
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC pool map mutex poisoned"))?
            .insert(profile.id, pool.clone());
        Ok(pool)
    }

    fn active_pool(&self, transaction_id: QueryId) -> Result<Arc<OdbcPool>> {
        self.transactions
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC transaction map mutex poisoned"))?
            .get(&transaction_id)
            .cloned()
            .ok_or_else(|| AppError::validation("transaction is not active"))
    }

    /// Shorten `spawn_blocking` plumbing: clone the profile and password and
    /// run the closure on the blocking pool.
    async fn on_blocking<T, F>(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        f: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&ConnectionProfile, Option<&str>) -> Result<T> + Send + 'static,
    {
        let profile = profile.clone();
        let password = password.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let password = password.as_deref();
            f(&profile, password)
        })
        .await
        .map_err(|join| AppError::internal(format!("ODBC blocking task failed: {join}")))?
    }
}

#[async_trait]
impl RelationalQueryExecutor for OdbcQueryExecutor {
    fn kind(&self) -> DataSourceKind {
        self.kind
    }

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        _pool_config: ConnectionPoolConfig,
    ) -> Result<Vec<QueryResult>> {
        let pool = self.pool_for(profile, password).await?;
        let kind = self.kind;
        let statements = statements.to_vec();
        self.on_blocking(profile, password, move |profile, password| {
            let guard = pool.acquire(profile, password)?;
            let connection = guard
                .as_ref()
                .ok_or_else(|| AppError::internal("ODBC pooled connection missing"))?;
            run_statements(connection, &statements, kind, profile)
        })
        .await
    }

    async fn pool_stats(&self, connection_id: ConnectionId) -> Result<Option<ConnectionPoolStats>> {
        let pool = self
            .pools
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC pool map mutex poisoned"))?
            .get(&connection_id)
            .cloned();
        let Some(pool) = pool else {
            return Ok(None);
        };
        let connected = pool.connection()?.is_some();
        Ok(Some(ConnectionPoolStats {
            connection_id,
            size: u32::from(connected),
            idle: u32::from(connected),
            max_connections: 1,
        }))
    }

    async fn close_pool(&self, connection_id: ConnectionId) -> Result<()> {
        self.pools
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC pool map mutex poisoned"))?
            .remove(&connection_id);
        Ok(())
    }

    async fn begin_transaction(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        transaction_id: QueryId,
        _pool_config: ConnectionPoolConfig,
    ) -> Result<()> {
        let dialect = VendorDialect::of(self.kind)?;
        if !dialect.supports_transactions() {
            return Err(AppError::new(
                ErrorKind::Unsupported,
                format!(
                    "{} does not support transactions through its ODBC driver",
                    dialect.display_label()
                ),
            ));
        }

        let pool = self.pool_for(profile, password).await?;
        let kind = self.kind;
        let transaction_pool = pool.clone();
        self.on_blocking(profile, password, move |profile, password| {
            let guard = transaction_pool.acquire(profile, password)?;
            let connection = guard
                .as_ref()
                .ok_or_else(|| AppError::internal("ODBC pooled connection missing"))?;
            connection
                .set_autocommit(false)
                .map_err(|error| map_odbc_error_for(kind, &connect::driver_name_for(profile), error))
        })
        .await?;
        self.transactions
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC transaction map mutex poisoned"))?
            .insert(transaction_id, pool);
        Ok(())
    }

    async fn execute_in_transaction(
        &self,
        transaction_id: QueryId,
        statements: &[String],
    ) -> Result<Vec<QueryResult>> {
        let pool = self.active_pool(transaction_id)?;
        let kind = self.kind;
        let statements = statements.to_vec();
        tokio::task::spawn_blocking(move || {
            let guard = pool.connection()?;
            let connection = guard
                .as_ref()
                .ok_or_else(|| AppError::internal("transaction connection missing"))?;
            // The pool keeps no profile after construction; transaction
            // statements reuse the kind for error mapping only.
            run_transaction_statements(connection, &statements, kind)
        })
        .await
        .map_err(|join| AppError::internal(format!("ODBC blocking task failed: {join}")))?
    }

    async fn commit_transaction(&self, transaction_id: QueryId) -> Result<()> {
        let pool = self
            .transactions
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC transaction map mutex poisoned"))?
            .remove(&transaction_id)
            .ok_or_else(|| AppError::validation("transaction is not active"))?;
        let kind = self.kind;
        tokio::task::spawn_blocking(move || {
            let guard = pool.connection()?;
            let connection = guard
                .as_ref()
                .ok_or_else(|| AppError::internal("transaction connection missing"))?;
            connection
                .commit()
                .map_err(|error| map_odbc_error_for(kind, "connection", error))?;
            // Restore autocommit so the pooled connection is reusable outside
            // the finished transaction.
            connection
                .set_autocommit(true)
                .map_err(|error| map_odbc_error_for(kind, "connection", error))
        })
        .await
        .map_err(|join| AppError::internal(format!("ODBC blocking task failed: {join}")))?
    }

    async fn rollback_transaction(&self, transaction_id: QueryId) -> Result<()> {
        let pool = self
            .transactions
            .lock()
            .map_err(|_poisoned| AppError::internal("ODBC transaction map mutex poisoned"))?
            .remove(&transaction_id)
            .ok_or_else(|| AppError::validation("transaction is not active"))?;
        let kind = self.kind;
        tokio::task::spawn_blocking(move || {
            let guard = pool.connection()?;
            let connection = guard
                .as_ref()
                .ok_or_else(|| AppError::internal("transaction connection missing"))?;
            connection
                .rollback()
                .map_err(|error| map_odbc_error_for(kind, "connection", error))?;
            connection
                .set_autocommit(true)
                .map_err(|error| map_odbc_error_for(kind, "connection", error))
        })
        .await
        .map_err(|join| AppError::internal(format!("ODBC blocking task failed: {join}")))?
    }
}

/// Execute a batch of statements on a dedicated connection. Non-SELECT
/// statements report `SQLRowCount` as `affected_rows`.
fn run_statements(
    connection: &Connection<'static>,
    statements: &[String],
    kind: DataSourceKind,
    profile: &ConnectionProfile,
) -> Result<Vec<QueryResult>> {
    let driver_name = connect::driver_name_for(profile);
    run_odbc_statements(connection, statements, kind, &driver_name)
}

fn run_transaction_statements(
    connection: &Connection<'static>,
    statements: &[String],
    kind: DataSourceKind,
) -> Result<Vec<QueryResult>> {
    run_odbc_statements(connection, statements, kind, "transaction connection")
}

fn run_odbc_statements(
    connection: &Connection<'static>,
    statements: &[String],
    kind: DataSourceKind,
    driver_name: &str,
) -> Result<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());

    for statement in statements {
        let started_at = Instant::now();
        let mut prepared = connection
            .prepare(statement)
            .map_err(|error| map_odbc_error_for(kind, driver_name, error))?;

        if is_result_set_statement(statement) {
            let cursor = prepared
                .execute(())
                .map_err(|error| map_odbc_error_for(kind, driver_name, error))?;
            let result = match cursor {
                Some(mut cursor) => {
                    fetch_result_set(statement, &mut cursor, started_at.elapsed(), kind, driver_name)?
                }
                // Heuristic misclassification (no result set): report an
                // empty result rather than guessing a row count.
                None => QueryResult::empty(statement).with_elapsed(started_at.elapsed()),
            };
            results.push(result);
        } else {
            // The cursorless Option is dropped at the semicolon, releasing
            // the borrow before `row_count` re-borrows the statement.
            prepared
                .execute(())
                .map_err(|error| map_odbc_error_for(kind, driver_name, error))?;
            // No result set: SQLRowCount carries the affected row count
            // (None when the driver cannot tell).
            let affected = prepared
                .row_count()
                .map_err(|error| map_odbc_error_for(kind, driver_name, error))?
                .unwrap_or(0);
            let mut result = QueryResult::empty(statement).with_elapsed(started_at.elapsed());
            result.affected_rows = affected as u64;
            results.push(result);
        }
    }

    Ok(results)
}

/// Drain a SELECT cursor into a buffered [`QueryResult`]. `is_result_set_statement`
/// misclassifications (no cursor) never reach here — the caller falls back to
/// the row-count path.
fn fetch_result_set(
    statement: &str,
    cursor: &mut impl Cursor,
    elapsed: std::time::Duration,
    kind: DataSourceKind,
    driver_name: &str,
) -> Result<QueryResult> {
    let column_count = cursor
        .num_result_cols()
        .map_err(|error| map_odbc_error_for(kind, driver_name, error))?
        .max(0) as usize;

    let mut columns = Vec::with_capacity(column_count);
    let mut data_types = Vec::with_capacity(column_count);
    for index in 1..=column_count as u16 {
        let name = cursor
            .col_name(index)
            .map_err(|error| map_odbc_error_for(kind, driver_name, error))?;
        let data_type = cursor
            .col_data_type(index)
            .map_err(|error| map_odbc_error_for(kind, driver_name, error))?;
        data_types.push(data_type);
        columns.push(Column {
            name,
            database_type: database_type_label(data_type),
            logical_type: logical_type(data_type),
            nullable: true,
            primary_key: false,
        });
    }

    let mut rows = Vec::new();
    let mut buffer = Vec::new();
    while let Some(mut row) = cursor
        .next_row()
        .map_err(|error| map_odbc_error_for(kind, driver_name, error))?
    {
        let mut values = Vec::with_capacity(column_count);
        for (index, data_type) in data_types.iter().enumerate() {
            buffer.clear();
            let has_value = row
                .get_text(index as u16 + 1, &mut buffer)
                .map_err(|error| map_odbc_error_for(kind, driver_name, error))?;
            let cell = if has_value {
                cell_from_text(*data_type, Some(&String::from_utf8_lossy(&buffer)))
            } else {
                cell_from_text(*data_type, None)
            };
            values.push(cell);
        }
        rows.push(Row::new(values));
    }

    Ok(QueryResult {
        statement: statement.to_string(),
        columns,
        rows,
        affected_rows: 0,
        execution_time_ms: elapsed.as_millis(),
        notices: Vec::new(),
        metadata: BTreeMap::new(),
    })
}
