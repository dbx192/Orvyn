//! MySQL adapter crate.

mod brand;
mod doris;

pub use brand::MysqlBrand;

use async_trait::async_trait;
use db_common::{
    is_result_set_statement, redact_connection_url, AppError, CellValue, Column, ConnectionId,
    ConnectionPoolConfig, ConnectionPoolStats, ConnectionProfile, ConnectionTestReport,
    ConnectionTester, DataSourceKind, DatabaseFunction, DatabaseObject, DatabaseObjectKind,
    ErrorKind, LogicalType, QueryId, QueryResult, RelationalMetadataReader,
    RelationalQueryExecutor, Result, Row, SslMode, TableColumn, TableForeignKey, TableIndex,
    TableTrigger,
};
use sqlx::{
    mysql::{MySqlConnectOptions, MySqlConnection, MySqlPoolOptions, MySqlRow, MySqlSslMode},
    pool::PoolConnection,
    types::{
        chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc},
        Decimal,
    },
    Column as SqlxColumn, MySql, MySqlPool, Row as SqlxRow, TypeInfo,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tokio::sync::Mutex;

pub const ADAPTER_NAME: &str = "mysql";

/// Resolves the MySQL-family brand for a profile. Returns `None` for non-MySQL
/// kinds (which should not reach this adapter at all).
fn brand_for(profile: &ConnectionProfile) -> Option<MysqlBrand> {
    MysqlBrand::of(profile.kind)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MySqlConnectionTester;

#[derive(Debug, Clone, Default)]
pub struct MySqlMetadataReader {
    pools: Arc<Mutex<BTreeMap<MySqlPoolKey, MySqlPool>>>,
}

#[derive(Debug, Clone, Default)]
pub struct MySqlQueryExecutor {
    pools: Arc<Mutex<BTreeMap<MySqlPoolKey, MySqlPool>>>,
    transactions: Arc<Mutex<BTreeMap<QueryId, PoolConnection<MySql>>>>,
}

type MySqlPoolKey = (ConnectionId, Option<String>, SslMode, Option<String>);

impl MySqlQueryExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    async fn pool_for(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        config: ConnectionPoolConfig,
    ) -> Result<MySqlPool> {
        let key = mysql_pool_key(profile);
        if let Some(pool) = self.pools.lock().await.get(&key).cloned() {
            return Ok(pool);
        }

        let pool = mysql_pool(profile, password, config).await?;
        self.pools.lock().await.insert(key, pool.clone());
        Ok(pool)
    }
}

impl MySqlMetadataReader {
    pub fn new() -> Self {
        Self::default()
    }

    async fn pool_for(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<MySqlPool> {
        let key = mysql_pool_key(profile);
        if let Some(pool) = self.pools.lock().await.get(&key).cloned() {
            return Ok(pool);
        }

        let pool = mysql_pool(profile, password, ConnectionPoolConfig::default()).await?;
        self.pools.lock().await.insert(key, pool.clone());
        Ok(pool)
    }
}

#[async_trait]
impl ConnectionTester for MySqlConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::MySql
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;

        let started_at = Instant::now();
        let pool = mysql_pool(profile, password, ConnectionPoolConfig::default()).await?;
        let brand = brand_for(profile);
        let version: Option<String> = if brand.is_some_and(MysqlBrand::uses_text_protocol) {
            // Doris-family servers do not implement the MySQL prepared-statement
            // command. `raw_sql` uses the text protocol and is sufficient here
            // because this health check has no parameters.
            sqlx::raw_sql("SELECT VERSION()")
                .fetch_one(&pool)
                .await
                .map_err(mysql_query_error)?
                .try_get(0)
                .ok()
        } else {
            sqlx::query_scalar("SELECT VERSION()")
                .fetch_optional(&pool)
                .await
                .map_err(mysql_query_error)?
        };
        pool.close().await;

        let mut report =
            ConnectionTestReport::success(DataSourceKind::MySql, started_at.elapsed().as_millis());

        if let Some(version) = version {
            report = report.with_server_version(version);
        }

        Ok(report)
    }
}

#[async_trait]
impl RelationalMetadataReader for MySqlMetadataReader {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::MySql
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let pool = self.pool_for(profile, password).await?;
        let rows = sqlx::raw_sql("SHOW DATABASES")
            .fetch_all(&pool)
            .await
            .map_err(mysql_query_error)?;

        let mut databases = Vec::new();
        for row in rows {
            let name = mysql_string_at(&row, 0)?;
            if matches!(name.as_str(), "information_schema" | "performance_schema") {
                continue;
            }
            databases.push(DatabaseObject {
                kind: DatabaseObjectKind::Database,
                name,
                schema: None,
                parent: None,
            });
        }
        Ok(databases)
    }

    async fn list_schemas(
        &self,
        _profile: &ConnectionProfile,
        _password: Option<&str>,
        database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        Ok(database
            .map(|name| {
                vec![DatabaseObject {
                    kind: DatabaseObjectKind::Schema,
                    name: name.to_string(),
                    schema: None,
                    parent: None,
                }]
            })
            .unwrap_or_default())
    }

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let database = database.or(profile.database.as_deref()).ok_or_else(|| {
            AppError::new(
                ErrorKind::Validation,
                "database is required for MySQL objects",
            )
        })?;
        let pool = self.pool_for(profile, password).await?;
        if brand_for(profile).is_some_and(|b| !b.supports_information_schema()) {
            let sql = format!("SHOW TABLES FROM `{}`", escape_mysql_identifier(database));
            let rows = sqlx::raw_sql(&sql)
                .fetch_all(&pool)
                .await
                .map_err(mysql_query_error)?;
            return rows
                .into_iter()
                .map(|row| {
                    Ok(DatabaseObject {
                        kind: DatabaseObjectKind::Table,
                        name: mysql_string_at(&row, 0)?,
                        schema: Some(database.to_string()),
                        parent: Some(database.to_string()),
                    })
                })
                .collect();
        }
        list_mysql_objects(&pool, database, "BASE TABLE", DatabaseObjectKind::Table).await
    }

    async fn list_views(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let database = database.or(profile.database.as_deref()).ok_or_else(|| {
            AppError::new(
                ErrorKind::Validation,
                "database is required for MySQL objects",
            )
        })?;
        let pool = self.pool_for(profile, password).await?;
        if brand_for(profile).is_some_and(|b| !b.supports_information_schema()) {
            let sql = format!(
                "SHOW FULL TABLES FROM `{}` WHERE Table_type = 'VIEW'",
                escape_mysql_identifier(database)
            );
            let rows = sqlx::raw_sql(&sql)
                .fetch_all(&pool)
                .await
                .map_err(mysql_query_error)?;
            return rows
                .into_iter()
                .map(|row| {
                    Ok(DatabaseObject {
                        kind: DatabaseObjectKind::View,
                        name: mysql_string_at(&row, 0)?,
                        schema: Some(database.to_string()),
                        parent: Some(database.to_string()),
                    })
                })
                .collect();
        }
        list_mysql_objects(&pool, database, "VIEW", DatabaseObjectKind::View).await
    }

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let database = database.or(profile.database.as_deref()).ok_or_else(|| {
            AppError::new(
                ErrorKind::Validation,
                "database is required for MySQL columns",
            )
        })?;
        let pool = self.pool_for(profile, password).await?;
        let rows = if brand_for(profile).is_some_and(|b| !b.supports_information_schema()) {
            let sql = format!(
                "SHOW COLUMNS FROM `{}`.`{}`",
                escape_mysql_identifier(database),
                escape_mysql_identifier(table)
            );
            sqlx::raw_sql(&sql)
                .fetch_all(&pool)
                .await
                .map_err(mysql_query_error)?
        } else {
            sqlx::query(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY, COLUMN_COMMENT \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
             ORDER BY ORDINAL_POSITION",
            )
            .bind(database)
            .bind(table)
            .fetch_all(&pool)
            .await
            .map_err(mysql_query_error)?
        };

        let mut columns = Vec::new();
        let is_doris_family = brand_for(profile).is_some_and(|b| b.is_doris_family());
        for row in rows {
            let (name, database_type, nullable, default_value, primary_key, comment) =
                if is_doris_family {
                    let key = mysql_string_at(&row, 3)?;
                    (
                        mysql_string_at(&row, 0)?,
                        mysql_string_at(&row, 1)?,
                        mysql_string_at(&row, 2)?.eq_ignore_ascii_case("YES"),
                        // SHOW COLUMNS returns Field, Type, Null, Key, Default, Extra.
                        mysql_optional_string_at(&row, 4)?,
                        key.eq_ignore_ascii_case("PRI"),
                        None,
                    )
                } else {
                    let comment = mysql_optional_string(&row, "COLUMN_COMMENT")?;
                    (
                        mysql_string(&row, "COLUMN_NAME")?,
                        mysql_string(&row, "COLUMN_TYPE")?,
                        mysql_string(&row, "IS_NULLABLE")?.eq_ignore_ascii_case("YES"),
                        mysql_optional_string(&row, "COLUMN_DEFAULT")?,
                        mysql_string(&row, "COLUMN_KEY")? == "PRI",
                        comment.filter(|value| !value.is_empty()),
                    )
                };
            columns.push(TableColumn {
                name,
                database_type,
                nullable,
                default_value,
                primary_key,
                comment: comment.filter(|value| !value.is_empty()),
            });
        }
        Ok(columns)
    }

    async fn list_indexes(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableIndex>> {
        let database = database.or(profile.database.as_deref()).ok_or_else(|| {
            AppError::new(
                ErrorKind::Validation,
                "database is required for MySQL indexes",
            )
        })?;
        let pool = self.pool_for(profile, password).await?;
        let rows = if brand_for(profile).is_some_and(|b| !b.supports_information_schema()) {
            let sql = format!(
                "SHOW INDEX FROM `{}`.`{}`",
                escape_mysql_identifier(database),
                escape_mysql_identifier(table)
            );
            sqlx::raw_sql(&sql)
                .fetch_all(&pool)
                .await
                .map_err(mysql_query_error)?
        } else {
            sqlx::query(
                "SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
             ORDER BY INDEX_NAME, SEQ_IN_INDEX",
            )
            .bind(database)
            .bind(table)
            .fetch_all(&pool)
            .await
            .map_err(mysql_query_error)?
        };

        if brand_for(profile).is_some_and(|b| b.is_doris_family()) {
            doris::group_indexes(rows)
        } else {
            group_mysql_indexes(rows)
        }
    }

    async fn list_foreign_keys(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableForeignKey>> {
        if brand_for(profile).is_some_and(|b| !b.supports_routines_metadata()) {
            // Doris-family servers do not expose MySQL referential constraints.
            return Ok(Vec::new());
        }
        let database = database.or(profile.database.as_deref()).ok_or_else(|| {
            AppError::new(
                ErrorKind::Validation,
                "database is required for MySQL foreign keys",
            )
        })?;
        let pool = self.pool_for(profile, password).await?;
        let rows = sqlx::query(
            "SELECT kcu.CONSTRAINT_NAME, kcu.COLUMN_NAME, kcu.REFERENCED_TABLE_NAME, \
                    kcu.REFERENCED_COLUMN_NAME, rc.UPDATE_RULE, rc.DELETE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE kcu \
             JOIN information_schema.REFERENTIAL_CONSTRAINTS rc \
               ON kcu.CONSTRAINT_NAME = rc.CONSTRAINT_NAME \
              AND kcu.TABLE_SCHEMA = rc.CONSTRAINT_SCHEMA \
             WHERE kcu.TABLE_SCHEMA = ? AND kcu.TABLE_NAME = ? \
               AND kcu.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&pool)
        .await
        .map_err(mysql_query_error)?;

        group_mysql_foreign_keys(rows)
    }

    async fn list_triggers(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableTrigger>> {
        if brand_for(profile).is_some_and(|b| !b.supports_routines_metadata()) {
            return Ok(Vec::new());
        }
        let database = database
            .or(schema)
            .or(profile.database.as_deref())
            .ok_or_else(|| {
                AppError::new(
                    ErrorKind::Validation,
                    "database is required for MySQL triggers",
                )
            })?;
        let pool = self.pool_for(profile, password).await?;
        let rows = sqlx::query(
            "SELECT TRIGGER_NAME, EVENT_MANIPULATION, ACTION_TIMING, ACTION_STATEMENT \
             FROM information_schema.TRIGGERS \
             WHERE TRIGGER_SCHEMA = ? AND EVENT_OBJECT_TABLE = ? \
             ORDER BY TRIGGER_NAME",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&pool)
        .await
        .map_err(mysql_query_error)?;

        let mut triggers = Vec::new();
        for row in rows {
            triggers.push(TableTrigger {
                name: mysql_string(&row, "TRIGGER_NAME")?,
                event: mysql_string(&row, "EVENT_MANIPULATION")?,
                timing: mysql_string(&row, "ACTION_TIMING")?,
                statement: mysql_optional_string(&row, "ACTION_STATEMENT")?,
            });
        }
        Ok(triggers)
    }

    async fn list_functions(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<DatabaseFunction>> {
        if brand_for(profile).is_some_and(|b| !b.supports_routines_metadata()) {
            return Ok(Vec::new());
        }
        let database = database.or(profile.database.as_deref()).ok_or_else(|| {
            AppError::new(
                ErrorKind::Validation,
                "database is required for MySQL functions",
            )
        })?;
        let pool = self.pool_for(profile, password).await?;
        let rows = sqlx::query(
            "SELECT ROUTINE_NAME, ROUTINE_TYPE, DTD_IDENTIFIER, EXTERNAL_LANGUAGE \
             FROM information_schema.ROUTINES \
             WHERE ROUTINE_SCHEMA = ? \
             ORDER BY ROUTINE_NAME",
        )
        .bind(database)
        .fetch_all(&pool)
        .await
        .map_err(mysql_query_error)?;

        let mut functions = Vec::new();
        for row in rows {
            let name = mysql_string(&row, "ROUTINE_NAME")?;
            let kind = mysql_string(&row, "ROUTINE_TYPE")?;
            let ddl_sql = format!(
                "SHOW CREATE {} `{}`.`{}`",
                if kind.eq_ignore_ascii_case("PROCEDURE") {
                    "PROCEDURE"
                } else {
                    "FUNCTION"
                },
                database.replace('`', "``"),
                name.replace('`', "``")
            );
            // SHOW CREATE includes parameters, characteristics, security and body.
            // Missing SHOW ROUTINE privileges must not produce a fabricated editable stub.
            let definition = match sqlx::query(&ddl_sql).fetch_one(&pool).await {
                Ok(ddl) => mysql_optional_string(
                    &ddl,
                    if kind.eq_ignore_ascii_case("PROCEDURE") {
                        "Create Procedure"
                    } else {
                        "Create Function"
                    },
                )?,
                Err(_) => None,
            };
            functions.push(DatabaseFunction {
                name,
                kind,
                return_type: mysql_optional_string(&row, "DTD_IDENTIFIER")?,
                language: mysql_optional_string(&row, "EXTERNAL_LANGUAGE")?,
                definition,
                identity_arguments: None,
            });
        }
        Ok(functions)
    }

    async fn get_create_table_sql(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<String> {
        let database = database.or(profile.database.as_deref()).ok_or_else(|| {
            AppError::new(
                ErrorKind::Validation,
                "database is required for MySQL create table",
            )
        })?;
        let pool = self.pool_for(profile, password).await?;
        let sql = format!("SHOW CREATE TABLE `{}`.`{}`", database, table);
        let row = if brand_for(profile).is_some_and(MysqlBrand::uses_text_protocol) {
            sqlx::raw_sql(&sql)
                .fetch_one(&pool)
                .await
                .map_err(mysql_query_error)?
        } else {
            sqlx::query(&sql)
                .fetch_one(&pool)
                .await
                .map_err(mysql_query_error)?
        };

        mysql_string_at(&row, 1)
    }
}

#[async_trait]
impl RelationalQueryExecutor for MySqlQueryExecutor {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::MySql
    }

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        pool_config: ConnectionPoolConfig,
    ) -> Result<Vec<QueryResult>> {
        let pool = self.pool_for(profile, password, pool_config).await?;
        if brand_for(profile).is_some_and(MysqlBrand::uses_text_protocol) {
            return doris::execute(pool, statements.to_vec()).await;
        }
        execute_mysql_statements_on_pool(&pool, statements).await
    }

    async fn execute_cancellable(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        pool_config: ConnectionPoolConfig,
        cancellation: db_common::QueryCancellation,
    ) -> Result<Vec<QueryResult>> {
        if brand_for(profile).is_some_and(MysqlBrand::uses_text_protocol) {
            return tokio::select! {
                _ = cancellation.cancelled() => Err(AppError::new(ErrorKind::Cancelled, "Client cancelled; server termination unconfirmed")),
                result = self.execute(profile, password, statements, pool_config) => result,
            };
        }
        let pool = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(cancellation.error()),
            pool = self.pool_for(profile, password, pool_config) => pool?,
        };
        let mut connection = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(cancellation.error()),
            connection = pool.acquire() => connection.map_err(mysql_connection_error)?,
        };
        let id: u64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
            .fetch_one(&mut *connection).await.map_err(mysql_query_error)?;
        let (result, cancelled) = {
            let execution = execute_mysql_statements_on_connection(&mut connection, statements);
            tokio::pin!(execution);
            tokio::select! {
                result = &mut execution => (result, false),
                _ = cancellation.cancelled() => {
                    // A separate control connection avoids deadlocking a pool of size one.
                    // Keep the original connection reserved until KILL and response draining finish.
                    let control = async {
                        use sqlx::Connection;
                        let mut control = MySqlConnection::connect_with(&pool.connect_options()).await?;
                        sqlx::raw_sql(&format!("KILL QUERY {id}")).execute(&mut control).await?;
                        Ok::<(), sqlx::Error>(())
                    };
                    let sent = tokio::time::timeout(std::time::Duration::from_secs(5), control).await;
                    let message = if matches!(sent, Ok(Ok(()))) {
                        if tokio::time::timeout(std::time::Duration::from_secs(10), &mut execution).await.is_ok() {
                            "Query cancelled; MySQL request completed"
                        } else { "Client cancelled; MySQL completion unconfirmed; connection discarded" }
                    } else { "Client cancelled; MySQL server cancellation unconfirmed; connection discarded" };
                    (Err(AppError::new(ErrorKind::Cancelled, message)), true)
                }
            }
        };
        if cancelled { connection.close_on_drop(); }
        result
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
            stats.size += pool.size();
            stats.idle += pool.num_idle() as u32;
            stats.max_connections += pool.options().get_max_connections();
        }
        Ok(Some(stats))
    }

    async fn close_pool(&self, connection_id: ConnectionId) -> Result<()> {
        let pools = {
            let mut pools = self.pools.lock().await;
            let keys = pools
                .keys()
                .filter(|key| key.0 == connection_id)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| pools.remove(&key))
                .collect::<Vec<_>>()
        };
        for pool in pools {
            pool.close().await;
        }
        Ok(())
    }

    async fn begin_transaction(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        transaction_id: QueryId,
        pool_config: ConnectionPoolConfig,
    ) -> Result<()> {
        let pool = self.pool_for(profile, password, pool_config).await?;
        let mut conn = pool.acquire().await.map_err(mysql_query_error)?;
        if brand_for(profile).is_some_and(|b| !b.supports_transactions()) {
            return Err(AppError::unsupported(
                "transactions are not supported on this MySQL-family brand",
            ));
        }
        sqlx::query("BEGIN")
            .execute(&mut *conn)
            .await
            .map_err(mysql_query_error)?;
        self.transactions.lock().await.insert(transaction_id, conn);
        Ok(())
    }

    async fn execute_in_transaction(
        &self,
        transaction_id: QueryId,
        statements: &[String],
    ) -> Result<Vec<QueryResult>> {
        let mut transactions = self.transactions.lock().await;
        let conn = transactions
            .get_mut(&transaction_id)
            .ok_or_else(|| AppError::validation("transaction is not active"))?;
        execute_mysql_statements_on_connection(conn, statements).await
    }

    async fn commit_transaction(&self, transaction_id: QueryId) -> Result<()> {
        let mut conn = self
            .transactions
            .lock()
            .await
            .remove(&transaction_id)
            .ok_or_else(|| AppError::validation("transaction is not active"))?;
        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await
            .map_err(mysql_query_error)?;
        Ok(())
    }

    async fn rollback_transaction(&self, transaction_id: QueryId) -> Result<()> {
        let mut conn = self
            .transactions
            .lock()
            .await
            .remove(&transaction_id)
            .ok_or_else(|| AppError::validation("transaction is not active"))?;
        sqlx::query("ROLLBACK")
            .execute(&mut *conn)
            .await
            .map_err(mysql_query_error)?;
        Ok(())
    }
}

fn mysql_pool_key(profile: &ConnectionProfile) -> MySqlPoolKey {
    (
        profile.id,
        profile
            .database
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned),
        profile.security.ssl_mode,
        tls_ca_path(profile).map(ToOwned::to_owned),
    )
}

async fn execute_mysql_statements_on_pool(
    pool: &MySqlPool,
    statements: &[String],
) -> Result<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());

    for statement in statements {
        let started_at = Instant::now();
        // User SQL is already split into complete statements and has no bound
        // parameters. COM_QUERY (text protocol) is required for procedural DDL
        // such as CREATE TRIGGER / PROCEDURE / FUNCTION, which MySQL rejects
        // on the prepared-statement protocol.
        if is_result_set_statement(statement) {
            let rows = sqlx::raw_sql(statement)
                .fetch_all(pool)
                .await
                .map_err(|error| mysql_execute_error(error, statement))?;
            results.push(
                mysql_rows_to_query_result(statement, rows).with_elapsed(started_at.elapsed()),
            );
            continue;
        }

        let done = sqlx::raw_sql(statement)
            .execute(pool)
            .await
            .map_err(|error| mysql_execute_error(error, statement))?;
        let mut result = QueryResult::empty(statement).with_elapsed(started_at.elapsed());
        result.affected_rows = done.rows_affected();
        results.push(result);
    }

    Ok(results)
}

async fn execute_mysql_statements_on_connection(
    conn: &mut MySqlConnection,
    statements: &[String],
) -> Result<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());

    for statement in statements {
        let started_at = Instant::now();
        let statement = statement.as_str();

        if is_result_set_statement(statement) {
            let statement = statement.to_owned();
            let rows = sqlx::query(&statement)
                .persistent(false)
                .fetch_all(&mut *conn)
                .await
                .map_err(mysql_query_error)?;
            results.push(
                mysql_rows_to_query_result(&statement, rows).with_elapsed(started_at.elapsed()),
            );
            continue;
        }

        let statement = statement.to_owned();
        let done = sqlx::query(&statement)
            .persistent(false)
            .execute(&mut *conn)
            .await
            .map_err(mysql_query_error)?;
        let mut result = QueryResult::empty(statement).with_elapsed(started_at.elapsed());
        result.affected_rows = done.rows_affected();
        results.push(result);
    }

    Ok(results)
}

fn mysql_url(profile: &ConnectionProfile, password: Option<&str>) -> Result<String> {
    let host = profile.required_host()?;
    let port = profile.required_port()?;
    let username = profile.username.as_deref().unwrap_or_default();
    let database = profile.database.as_deref().unwrap_or_default();

    let auth = if username.is_empty() {
        String::new()
    } else {
        let username = urlencoding::encode(username);
        let password = password.map(urlencoding::encode);
        match password {
            Some(password) => format!("{username}:{password}@"),
            None => format!("{username}@"),
        }
    };

    let database = if database.is_empty() {
        String::new()
    } else {
        format!("/{}", urlencoding::encode(database))
    };

    Ok(format!("mysql://{auth}{host}:{port}{database}"))
}

async fn mysql_pool(
    profile: &ConnectionProfile,
    password: Option<&str>,
    config: ConnectionPoolConfig,
) -> Result<MySqlPool> {
    profile.validate()?;
    // Reject brands that cannot be reached over the MySQL wire protocol
    // before we even attempt to dial the server. OceanBase in Oracle mode is
    // the main case: it speaks Oracle's proprietary protocol, so the MySQL
    // driver would just hang or fail with a confusing handshake error.
    if let Some(brand) = brand_for(profile) {
        if !brand.is_connectable(profile) {
            return Err(AppError::new(
                ErrorKind::Unsupported,
                format!(
                    "{} in this mode is not reachable via the MySQL protocol; switch the brand mode or use a dedicated Oracle driver",
                    profile.kind.display_label()
                ),
            ));
        }
    }
    let url = mysql_url(profile, password)?;
    // Doris-family servers speak the MySQL wire protocol but reject SQLx's
    // default `SET sql_mode=(SELECT CONCAT(...))` session initialization:
    // they require SET values to be literals. Apply the Doris-specific
    // connect-options tweak to every Doris-family brand and keep the default
    // SQLx options for the rest.
    let mut connect_options: MySqlConnectOptions =
        url.parse::<MySqlConnectOptions>().map_err(|error| {
            AppError::new(ErrorKind::Connection, "invalid MySQL connection URL")
                .with_detail(error.to_string())
        })?;
    if brand_for(profile).is_some_and(|b| b.is_doris_family()) {
        connect_options = doris::configure_connect_options(connect_options);
    }
    connect_options = connect_options.ssl_mode(mysql_ssl_mode(profile.security.ssl_mode));
    if let Some(ca_path) = tls_ca_path(profile) {
        connect_options = connect_options.ssl_ca(ca_path);
    }

    MySqlPoolOptions::new()
        .min_connections(config.min_connections)
        .max_connections(config.max_connections.max(1))
        .acquire_timeout(config.acquire_timeout())
        .idle_timeout(config.idle_timeout())
        .max_lifetime(config.max_lifetime())
        .connect_with(connect_options)
        .await
        .map_err(mysql_connection_error)
}

fn mysql_connection_error(error: sqlx::Error) -> AppError {
    AppError::new(ErrorKind::Connection, "failed to connect to MySQL")
        .with_detail(redact_connection_url(&error.to_string()))
}

fn mysql_ssl_mode(mode: SslMode) -> MySqlSslMode {
    match mode {
        SslMode::Disable => MySqlSslMode::Disabled,
        SslMode::Prefer => MySqlSslMode::Preferred,
        SslMode::Require => MySqlSslMode::Required,
        SslMode::VerifyCa => MySqlSslMode::VerifyCa,
        SslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
    }
}

fn tls_ca_path(profile: &ConnectionProfile) -> Option<&str> {
    profile
        .extra_param("tls_ca_path")
        .filter(|value| !value.trim().is_empty())
}

fn mysql_query_error(error: sqlx::Error) -> AppError {
    AppError::new(ErrorKind::Query, "failed to query MySQL metadata")
        .with_detail(redact_connection_url(&error.to_string()))
}

fn mysql_execute_error(error: sqlx::Error, statement: &str) -> AppError {
    let sql = redact_connection_url(statement);
    match error {
        sqlx::Error::Database(db) => {
            let message = db.message().trim();
            let mut app = AppError::new(
                ErrorKind::Query,
                if message.is_empty() {
                    "MySQL rejected the statement".to_string()
                } else {
                    message.to_string()
                },
            )
            .with_detail(format!("SQL:\n{sql}"));
            if let Some(code) = db.code() {
                app = app.with_code(code.to_string());
            }
            app
        }
        other => AppError::new(ErrorKind::Query, "failed to execute MySQL statement").with_detail(
            format!(
                "{}\n\nSQL:\n{sql}",
                redact_connection_url(&other.to_string())
            ),
        ),
    }
}

async fn list_mysql_objects(
    pool: &MySqlPool,
    database: &str,
    table_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let rows = if table_type == "BASE TABLE" {
        sqlx::query(&format!(
            "SHOW TABLES FROM `{}`",
            escape_mysql_identifier(database)
        ))
        .fetch_all(pool)
        .await
        .map_err(mysql_query_error)?
    } else {
        sqlx::query(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
         WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = ? \
         ORDER BY TABLE_NAME",
        )
        .bind(database)
        .bind(table_type)
        .fetch_all(pool)
        .await
        .map_err(mysql_query_error)?
    };

    let mut objects = Vec::new();
    for row in rows {
        objects.push(DatabaseObject {
            kind,
            name: mysql_string(&row, "TABLE_NAME").or_else(|_| mysql_string_at(&row, 0))?,
            schema: Some(database.to_string()),
            parent: Some(database.to_string()),
        });
    }
    Ok(objects)
}

fn escape_mysql_identifier(value: &str) -> String {
    value.replace('`', "``")
}

pub(crate) fn mysql_string_at(row: &MySqlRow, index: usize) -> Result<String> {
    mysql_optional_string_at(row, index)?.ok_or_else(|| {
        AppError::new(ErrorKind::Query, "failed to query MySQL metadata")
            .with_detail(format!("column {index} was NULL"))
    })
}

fn mysql_optional_string_at(row: &MySqlRow, index: usize) -> Result<Option<String>> {
    if let Ok(value) = row.try_get::<Option<String>, _>(index) {
        return Ok(value);
    }
    if let Ok(value) = row.try_get::<Option<Vec<u8>>, _>(index) {
        return Ok(value.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()));
    }
    row.try_get::<Option<String>, _>(index)
        .map_err(mysql_query_error)
}

fn mysql_string(row: &MySqlRow, name: &str) -> Result<String> {
    mysql_optional_string(row, name)?.ok_or_else(|| {
        AppError::new(ErrorKind::Query, "failed to query MySQL metadata")
            .with_detail(format!("column {name} was NULL"))
    })
}

fn mysql_optional_string(row: &MySqlRow, name: &str) -> Result<Option<String>> {
    if let Ok(value) = row.try_get::<Option<String>, _>(name) {
        return Ok(value);
    }
    if let Ok(value) = row.try_get::<Option<Vec<u8>>, _>(name) {
        return Ok(value.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()));
    }
    row.try_get::<Option<String>, _>(name)
        .map_err(mysql_query_error)
}

fn mysql_rows_to_query_result(statement: &str, rows: Vec<sqlx::mysql::MySqlRow>) -> QueryResult {
    let columns = rows
        .first()
        .map(|row| {
            row.columns()
                .iter()
                .map(|column| Column {
                    name: column.name().to_string(),
                    database_type: column.type_info().name().to_string(),
                    logical_type: mysql_logical_type(column.type_info().name()),
                    nullable: true,
                    primary_key: false,
                })
                .collect()
        })
        .unwrap_or_default();

    let rows = rows
        .into_iter()
        .map(|row| {
            let values = (0..row.len())
                .map(|index| mysql_cell_value(&row, index))
                .collect();
            Row::new(values)
        })
        .collect();

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

fn mysql_cell_value(row: &sqlx::mysql::MySqlRow, index: usize) -> CellValue {
    let type_name = row
        .columns()
        .get(index)
        .map(|column| column.type_info().name().to_ascii_lowercase())
        .unwrap_or_default();

    if type_name.contains("timestamp") {
        // SQLx's NaiveDateTime is only type-compatible with MySQL DATETIME.
        // DateTime<Utc> explicitly supports TIMESTAMP; keep the displayed value
        // naive so it matches the session-local value returned by MySQL.
        if let Ok(value) = row.try_get::<Option<DateTime<Utc>>, _>(index) {
            return value
                .map(|value| CellValue::DateTime(value.naive_utc()))
                .unwrap_or(CellValue::Null);
        }
    }
    if type_name.contains("datetime") {
        if let Ok(value) = row.try_get::<Option<NaiveDateTime>, _>(index) {
            return value.map(CellValue::DateTime).unwrap_or(CellValue::Null);
        }
    }
    if type_name == "date" {
        if let Ok(value) = row.try_get::<Option<NaiveDate>, _>(index) {
            return value.map(CellValue::Date).unwrap_or(CellValue::Null);
        }
    }
    if type_name == "time" {
        if let Ok(value) = row.try_get::<Option<NaiveTime>, _>(index) {
            return value.map(CellValue::Time).unwrap_or(CellValue::Null);
        }
    }
    if type_name.contains("decimal") || type_name.contains("numeric") {
        if let Ok(value) = row.try_get::<Option<Decimal>, _>(index) {
            return value
                .map(|value| CellValue::Decimal(value.to_string()))
                .unwrap_or(CellValue::Null);
        }
    }
    // Doris types such as LARGEINT, DECIMALV3, JSONB, ARRAY and MAP can be
    // reported with MySQL-compatible text metadata but are not supported by
    // SQLx's typed decoders. Decode their wire value as text/bytes instead of
    // failing the whole table result.
    if type_name.contains("largeint")
        || type_name.contains("json")
        || type_name.contains("array")
        || type_name.contains("map")
        || type_name.contains("struct")
    {
        if let Ok(value) = row.try_get::<Option<Vec<u8>>, _>(index) {
            return value
                .map(|bytes| CellValue::Text(String::from_utf8_lossy(&bytes).into_owned()))
                .unwrap_or(CellValue::Null);
        }
        if let Ok(value) = row.try_get::<Option<String>, _>(index) {
            return value.map(CellValue::Text).unwrap_or(CellValue::Null);
        }
    }
    if let Ok(value) = row.try_get::<Option<String>, _>(index) {
        return value.map(CellValue::Text).unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<u64>, _>(index) {
        return value.map(CellValue::UInt).unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(index) {
        return value.map(CellValue::Int).unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<f64>, _>(index) {
        return value.map(CellValue::Float).unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<f32>, _>(index) {
        return value
            .map(|value| CellValue::Float(f64::from(value)))
            .unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<Option<Vec<u8>>, _>(index) {
        return value.map(CellValue::Bytes).unwrap_or(CellValue::Null);
    }

    CellValue::Null
}

fn mysql_logical_type(type_name: &str) -> LogicalType {
    let type_name = type_name.to_ascii_lowercase();
    if type_name == "bool" || type_name == "boolean" || type_name == "tinyint(1)" {
        LogicalType::Bool
    } else if type_name.contains("unsigned")
        && (type_name.contains("int") || type_name.contains("bit"))
    {
        LogicalType::UInt
    } else if type_name.contains("int") || type_name == "bit" {
        LogicalType::Int
    } else if type_name.contains("float")
        || type_name.contains("double")
        || type_name.contains("real")
    {
        LogicalType::Float
    } else if type_name.contains("decimal") || type_name.contains("numeric") {
        LogicalType::Decimal
    } else if type_name == "date" {
        LogicalType::Date
    } else if type_name == "time" {
        LogicalType::Time
    } else if type_name.contains("datetime") || type_name.contains("timestamp") {
        LogicalType::DateTime
    } else if type_name.contains("json")
        || type_name.contains("array")
        || type_name.contains("map")
        || type_name.contains("struct")
    {
        LogicalType::Json
    } else if type_name.contains("blob")
        || type_name.contains("binary")
        || type_name.contains("varbinary")
    {
        LogicalType::Bytes
    } else {
        LogicalType::Text
    }
}

fn group_mysql_indexes(rows: Vec<MySqlRow>) -> Result<Vec<TableIndex>> {
    let mut indexes: Vec<TableIndex> = Vec::new();

    for row in rows {
        let name = mysql_string(&row, "INDEX_NAME")?;
        let column = mysql_string(&row, "COLUMN_NAME")?;
        let non_unique: i8 = row.try_get("NON_UNIQUE").map_err(mysql_query_error)?;

        if let Some(index) = indexes.iter_mut().find(|index| index.name == name) {
            index.columns.push(column);
            continue;
        }

        indexes.push(TableIndex {
            primary: name == "PRIMARY",
            name,
            columns: vec![column],
            unique: non_unique == 0,
        });
    }

    Ok(indexes)
}

fn group_mysql_foreign_keys(rows: Vec<MySqlRow>) -> Result<Vec<TableForeignKey>> {
    let mut fks: Vec<TableForeignKey> = Vec::new();

    for row in rows {
        let name = mysql_string(&row, "CONSTRAINT_NAME")?;
        let column = mysql_string(&row, "COLUMN_NAME")?;
        let ref_table = mysql_string(&row, "REFERENCED_TABLE_NAME")?;
        let ref_column = mysql_string(&row, "REFERENCED_COLUMN_NAME")?;
        let on_update = mysql_optional_string(&row, "UPDATE_RULE")?;
        let on_delete = mysql_optional_string(&row, "DELETE_RULE")?;

        if let Some(fk) = fks.iter_mut().find(|fk| fk.name == name) {
            fk.columns.push(column);
            fk.referenced_columns.push(ref_column);
            continue;
        }

        fks.push(TableForeignKey {
            name,
            columns: vec![column],
            referenced_table: ref_table,
            referenced_columns: vec![ref_column],
            on_update,
            on_delete,
        });
    }

    Ok(fks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Type;

    #[test]
    fn mysql_timestamp_uses_timezone_aware_decoder() {
        let timestamp_type = <DateTime<Utc> as Type<MySql>>::type_info();

        assert!(<DateTime<Utc> as Type<MySql>>::compatible(&timestamp_type));
        assert!(!<NaiveDateTime as Type<MySql>>::compatible(&timestamp_type));
    }

    #[test]
    fn mysql_temporal_types_are_classified_as_datetime() {
        assert_eq!(mysql_logical_type("TIMESTAMP"), LogicalType::DateTime);
        assert_eq!(mysql_logical_type("DATETIME"), LogicalType::DateTime);
    }

    #[test]
    fn ssl_modes_map_to_sqlx_certificate_policies() {
        assert!(matches!(
            mysql_ssl_mode(SslMode::Disable),
            MySqlSslMode::Disabled
        ));
        assert!(matches!(
            mysql_ssl_mode(SslMode::Prefer),
            MySqlSslMode::Preferred
        ));
        assert!(matches!(
            mysql_ssl_mode(SslMode::Require),
            MySqlSslMode::Required
        ));
        assert!(matches!(
            mysql_ssl_mode(SslMode::VerifyCa),
            MySqlSslMode::VerifyCa
        ));
        assert!(matches!(
            mysql_ssl_mode(SslMode::VerifyFull),
            MySqlSslMode::VerifyIdentity
        ));
    }

    #[tokio::test]
    async fn oceanbase_oracle_mode_is_rejected_before_dial() {
        let profile = ConnectionProfile::builder(DataSourceKind::OceanBase, "ob")
            .host("localhost")
            .port(2883)
            .username("u")
            .extra_param("ob_mode", "oracle")
            .build()
            .expect("profile");

        let error = mysql_pool(&profile, None, ConnectionPoolConfig::default())
            .await
            .expect_err("oracle mode should be rejected");
        assert_eq!(error.kind, ErrorKind::Unsupported, "{error:?}");
    }

    #[tokio::test]
    async fn oceanbase_mysql_mode_attempts_connection() {
        let profile = ConnectionProfile::builder(DataSourceKind::OceanBase, "ob")
            .host("localhost")
            .port(2883)
            .username("u")
            .extra_param("ob_mode", "mysql")
            .build()
            .expect("profile");

        let result = mysql_pool(&profile, None, ConnectionPoolConfig::default()).await;
        // We don't have a live server, but the brand check must pass; the
        // failure (if any) should come from the TCP layer, not Unsupported.
        if let Err(error) = result {
            assert_ne!(
                error.kind,
                ErrorKind::Unsupported,
                "mysql mode must not be rejected as unsupported: {error:?}"
            );
        }
    }
}
