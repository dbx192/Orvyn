//! PostgreSQL adapter crate.

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use db_common::{
    is_result_set_statement, AppError, CellValue, Column, ConnectionId, ConnectionPoolConfig,
    ConnectionPoolStats, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseFunction, DatabaseObject, DatabaseObjectKind, ErrorKind, LogicalType, QueryResult,
    RelationalMetadataReader, RelationalQueryExecutor, Result, Row, TableColumn, TableForeignKey,
    TableIndex, TableTrigger,
};
use std::{collections::BTreeMap, error::Error, fmt::Write, sync::Arc, time::Instant};
use tokio::sync::{Mutex, Semaphore};
use tokio_postgres::{
    types::{FromSql, Type},
    Client, Config, NoTls, Row as PgRow,
};

pub const ADAPTER_NAME: &str = "postgresql";

#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresMetadataReader;

#[derive(Debug, Clone, Default)]
pub struct PostgresQueryExecutor {
    pools: Arc<Mutex<BTreeMap<PostgresPoolKey, Arc<PostgresPool>>>>,
}

type PostgresPoolKey = (ConnectionId, Option<String>);

impl PostgresQueryExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    async fn pool_for(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        config: ConnectionPoolConfig,
    ) -> Result<Arc<PostgresPool>> {
        let key = postgres_pool_key(profile);
        if let Some(pool) = self.pools.lock().await.get(&key).cloned() {
            return Ok(pool);
        }

        let pool = Arc::new(PostgresPool::new(profile, password, config)?);
        self.pools.lock().await.insert(key, pool.clone());
        Ok(pool)
    }
}

#[derive(Debug)]
struct PostgresPool {
    connection_id: ConnectionId,
    connect_config: Config,
    config: ConnectionPoolConfig,
    semaphore: Arc<Semaphore>,
    idle: Mutex<Vec<Client>>,
    size: Mutex<u32>,
}

impl PostgresPool {
    fn new(
        profile: &ConnectionProfile,
        password: Option<&str>,
        config: ConnectionPoolConfig,
    ) -> Result<Self> {
        Ok(Self {
            connection_id: profile.id,
            connect_config: postgres_config(profile, password)?,
            config,
            semaphore: Arc::new(Semaphore::new(config.max_connections.max(1) as usize)),
            idle: Mutex::new(Vec::new()),
            size: Mutex::new(0),
        })
    }

    async fn acquire(self: &Arc<Self>) -> Result<PostgresPoolClient> {
        let permit = tokio::time::timeout(
            self.config.acquire_timeout(),
            self.semaphore.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
            AppError::new(
                ErrorKind::Timeout,
                "timed out acquiring PostgreSQL connection",
            )
        })?
        .map_err(|_| AppError::internal("PostgreSQL pool semaphore closed"))?;

        if let Some(client) = self.idle.lock().await.pop() {
            return Ok(PostgresPoolClient {
                pool: self.clone(),
                client: Some(client),
                _permit: permit,
            });
        }

        let client = self.connect().await?;
        *self.size.lock().await += 1;

        Ok(PostgresPoolClient {
            pool: self.clone(),
            client: Some(client),
            _permit: permit,
        })
    }

    async fn connect(&self) -> Result<Client> {
        let (client, connection) = tokio::time::timeout(
            self.config.connect_timeout(),
            self.connect_config.connect(NoTls),
        )
        .await
        .map_err(|_| AppError::new(ErrorKind::Timeout, "timed out connecting to PostgreSQL"))?
        .map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to connect to PostgreSQL")
                .with_detail(error.to_string())
        })?;

        tokio::spawn(async move {
            let _ = connection.await;
        });

        Ok(client)
    }

    async fn release(&self, client: Client) {
        self.idle.lock().await.push(client);
    }

    async fn stats(&self) -> ConnectionPoolStats {
        ConnectionPoolStats {
            connection_id: self.connection_id,
            size: *self.size.lock().await,
            idle: self.idle.lock().await.len() as u32,
            max_connections: self.config.max_connections.max(1),
        }
    }
}

struct PostgresPoolClient {
    pool: Arc<PostgresPool>,
    client: Option<Client>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl PostgresPoolClient {
    fn client(&self) -> &Client {
        self.client.as_ref().expect("pool client missing")
    }

    async fn release(mut self) {
        if let Some(client) = self.client.take() {
            self.pool.release(client).await;
        }
    }
}

#[async_trait]
impl ConnectionTester for PostgresConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::PostgreSql
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;

        let started_at = Instant::now();
        let config = postgres_config(profile, password)?;

        let (client, connection) = config.connect(NoTls).await.map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to connect to PostgreSQL")
                .with_detail(error.to_string())
        })?;

        tokio::spawn(async move {
            let _ = connection.await;
        });

        let row = client
            .query_one("SELECT version()", &[])
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Query, "failed to test PostgreSQL connection")
                    .with_detail(error.to_string())
            })?;
        let version: String = row.get(0);

        Ok(ConnectionTestReport::success(
            DataSourceKind::PostgreSql,
            started_at.elapsed().as_millis(),
        )
        .with_server_version(version))
    }
}

#[async_trait]
impl RelationalMetadataReader for PostgresMetadataReader {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::PostgreSql
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let client = postgres_client(profile, password).await?;
        let rows = client
            .query(
                "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname",
                &[],
            )
            .await
            .map_err(query_error)?;

        Ok(rows
            .into_iter()
            .map(|row| DatabaseObject {
                kind: DatabaseObjectKind::Database,
                name: row.get(0),
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
        let client = postgres_client_for_database(profile, password, database).await?;
        let rows = client
            .query(
                "SELECT n.nspname \
                 FROM pg_namespace n \
                 WHERE n.nspname NOT LIKE 'pg_%' \
                   AND n.nspname <> 'information_schema' \
                   AND NOT EXISTS ( \
                       SELECT 1 \
                       FROM pg_depend d \
                       JOIN pg_extension e ON e.oid = d.refobjid \
                       WHERE d.classid = 'pg_namespace'::regclass \
                         AND d.objid = n.oid \
                         AND d.deptype = 'e' \
                   ) \
                 ORDER BY n.nspname",
                &[],
            )
            .await
            .map_err(query_error)?;
        let parent = selected_database(profile, database);

        Ok(rows
            .into_iter()
            .map(|row| DatabaseObject {
                kind: DatabaseObjectKind::Schema,
                name: row.get(0),
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
        list_postgres_objects(
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
        list_postgres_objects(
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
        let schema = schema.unwrap_or("public");
        let client = postgres_client_for_database(profile, password, database).await?;
        let rows = client
            .query(
                "SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, \
                        EXISTS ( \
                            SELECT 1 FROM information_schema.table_constraints tc \
                            JOIN information_schema.key_column_usage kcu \
                              ON tc.constraint_name = kcu.constraint_name \
                             AND tc.table_schema = kcu.table_schema \
                            WHERE tc.constraint_type = 'PRIMARY KEY' \
                              AND tc.table_schema = c.table_schema \
                              AND tc.table_name = c.table_name \
                              AND kcu.column_name = c.column_name \
                        ) AS is_primary \
                 FROM information_schema.columns c \
                 WHERE c.table_schema = $1 AND c.table_name = $2 \
                 ORDER BY c.ordinal_position",
                &[&schema, &table],
            )
            .await
            .map_err(query_error)?;

        Ok(rows
            .into_iter()
            .map(|row| TableColumn {
                name: row.get(0),
                database_type: row.get(1),
                nullable: row.get::<_, String>(2).eq_ignore_ascii_case("YES"),
                default_value: row.get(3),
                primary_key: row.get(4),
                comment: None,
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
        let schema = schema.unwrap_or("public");
        let client = postgres_client_for_database(profile, password, database).await?;
        let rows = client
            .query(
                "SELECT indexname, indexdef FROM pg_indexes \
                 WHERE schemaname = $1 AND tablename = $2 \
                 ORDER BY indexname",
                &[&schema, &table],
            )
            .await
            .map_err(query_error)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let name: String = row.get(0);
                let definition: String = row.get(1);
                TableIndex {
                    primary: definition.contains("CREATE UNIQUE INDEX") && name.ends_with("_pkey"),
                    unique: definition.contains("CREATE UNIQUE INDEX"),
                    name,
                    columns: parse_postgres_index_columns(&definition),
                }
            })
            .collect())
    }

    async fn list_foreign_keys(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableForeignKey>> {
        let schema = schema.unwrap_or("public");
        let client = postgres_client_for_database(profile, password, database).await?;
        let rows = client
            .query(
                "SELECT tc.constraint_name, kcu.column_name, \
                        ccu.table_name AS referenced_table, \
                        ccu.column_name AS referenced_column, \
                        rc.update_rule, rc.delete_rule \
                 FROM information_schema.table_constraints tc \
                 JOIN information_schema.key_column_usage kcu \
                   ON tc.constraint_name = kcu.constraint_name \
                  AND tc.table_schema = kcu.table_schema \
                 JOIN information_schema.constraint_column_usage ccu \
                   ON tc.constraint_name = ccu.constraint_name \
                  AND tc.table_schema = ccu.table_schema \
                 JOIN information_schema.referential_constraints rc \
                   ON tc.constraint_name = rc.constraint_name \
                  AND tc.table_schema = rc.constraint_schema \
                 WHERE tc.constraint_type = 'FOREIGN KEY' \
                   AND tc.table_schema = $1 AND tc.table_name = $2 \
                 ORDER BY tc.constraint_name, kcu.ordinal_position",
                &[&schema, &table],
            )
            .await
            .map_err(query_error)?;

        Ok(group_postgres_foreign_keys(rows))
    }

    async fn list_triggers(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableTrigger>> {
        let schema = schema.unwrap_or("public");
        let client = postgres_client_for_database(profile, password, database).await?;
        let rows = client
            .query(
                "SELECT trigger_name, event_manipulation, action_timing, action_statement \
                 FROM information_schema.triggers \
                 WHERE trigger_schema = $1 AND event_object_table = $2 \
                 ORDER BY trigger_name",
                &[&schema, &table],
            )
            .await
            .map_err(query_error)?;

        Ok(rows
            .into_iter()
            .map(|row| TableTrigger {
                name: row.get(0),
                event: row.get(1),
                timing: row.get(2),
                statement: row.get(3),
            })
            .collect())
    }

    async fn list_functions(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseFunction>> {
        let schema = schema.unwrap_or("public");
        let client = postgres_client_for_database(profile, password, database).await?;
        let rows = client
            .query(
                "SELECT p.proname AS function_name, \
                        pg_get_function_result(p.oid) AS return_type, \
                        l.lanname AS language, \
                        CASE p.prokind \
                            WHEN 'f' THEN 'FUNCTION' \
                            WHEN 'p' THEN 'PROCEDURE' \
                            WHEN 'a' THEN 'AGGREGATE' \
                            WHEN 'w' THEN 'WINDOW' \
                            ELSE 'FUNCTION' \
                        END AS kind \
                 FROM pg_proc p \
                 JOIN pg_namespace n ON p.pronamespace = n.oid \
                 LEFT JOIN pg_language l ON p.prolang = l.oid \
                 WHERE n.nspname = $1 \
                   AND p.prokind IN ('f', 'p') \
                 ORDER BY p.proname",
                &[&schema],
            )
            .await
            .map_err(query_error)?;

        Ok(rows
            .into_iter()
            .map(|row| DatabaseFunction {
                name: row.get(0),
                return_type: row.get(1),
                language: row.get(2),
                kind: row.get(3),
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
        let schema = schema.unwrap_or("public");
        let client = postgres_client_for_database(profile, password, database).await?;

        let ddl = pg_get_create_table(&client, schema, table).await?;
        Ok(ddl)
    }
}

#[async_trait]
impl RelationalQueryExecutor for PostgresQueryExecutor {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::PostgreSql
    }

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        pool_config: ConnectionPoolConfig,
    ) -> Result<Vec<QueryResult>> {
        let pool = self.pool_for(profile, password, pool_config).await?;
        let pooled = pool.acquire().await?;
        let client = pooled.client();
        let mut results = Vec::with_capacity(statements.len());

        for statement in statements {
            let started_at = Instant::now();
            let statement = statement.as_str();

            if is_result_set_statement(statement) {
                let rows = client.query(statement, &[]).await.map_err(query_error)?;
                results.push(
                    postgres_rows_to_query_result(statement, rows)
                        .with_elapsed(started_at.elapsed()),
                );
                continue;
            }

            let affected_rows = client.execute(statement, &[]).await.map_err(query_error)?;
            let mut result = QueryResult::empty(statement).with_elapsed(started_at.elapsed());
            result.affected_rows = affected_rows;
            results.push(result);
        }

        pooled.release().await;
        Ok(results)
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
}

async fn postgres_client(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<tokio_postgres::Client> {
    postgres_client_for_database(profile, password, None).await
}

async fn postgres_client_for_database(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: Option<&str>,
) -> Result<tokio_postgres::Client> {
    let config = postgres_config_for_database(profile, password, database)?;

    let (client, connection) = config.connect(NoTls).await.map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to connect to PostgreSQL")
            .with_detail(error.to_string())
    })?;

    tokio::spawn(async move {
        let _ = connection.await;
    });

    Ok(client)
}

fn postgres_config(profile: &ConnectionProfile, password: Option<&str>) -> Result<Config> {
    postgres_config_for_database(profile, password, None)
}

fn postgres_config_for_database(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: Option<&str>,
) -> Result<Config> {
    profile.validate()?;
    let mut config = Config::new();
    config.host(profile.required_host()?);
    config.port(profile.required_port()?);

    if let Some(username) = profile
        .username
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        config.user(username);
    }
    if let Some(password) = password.filter(|value| !value.is_empty()) {
        config.password(password);
    }
    if let Some(database) = database
        .or(profile.database.as_deref())
        .filter(|value| !value.trim().is_empty())
    {
        config.dbname(database);
    }

    Ok(config)
}

fn selected_database(profile: &ConnectionProfile, database: Option<&str>) -> Option<String> {
    database
        .or(profile.database.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn postgres_pool_key(profile: &ConnectionProfile) -> PostgresPoolKey {
    (
        profile.id,
        profile
            .database
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned),
    )
}

async fn list_postgres_objects(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: Option<&str>,
    schema: Option<&str>,
    table_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let schema = schema.unwrap_or("public");
    let client = postgres_client_for_database(profile, password, database).await?;
    let rows = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = $1 AND table_type = $2 \
             ORDER BY table_name",
            &[&schema, &table_type],
        )
        .await
        .map_err(query_error)?;

    Ok(rows
        .into_iter()
        .map(|row| DatabaseObject {
            kind,
            name: row.get(0),
            schema: Some(schema.to_string()),
            parent: selected_database(profile, database),
        })
        .collect())
}

fn query_error(error: tokio_postgres::Error) -> AppError {
    AppError::new(ErrorKind::Query, "failed to query PostgreSQL metadata")
        .with_detail(error.to_string())
}

fn parse_postgres_index_columns(definition: &str) -> Vec<String> {
    let Some(start) = definition.rfind('(') else {
        return Vec::new();
    };
    let Some(end) = definition.rfind(')') else {
        return Vec::new();
    };

    definition[start + 1..end]
        .split(',')
        .map(|column| column.trim().trim_matches('"').to_string())
        .filter(|column| !column.is_empty())
        .collect()
}

fn group_postgres_foreign_keys(rows: Vec<PgRow>) -> Vec<TableForeignKey> {
    let mut fks: Vec<TableForeignKey> = Vec::new();

    for row in rows {
        let name: String = row.get(0);
        let column: String = row.get(1);
        let ref_table: String = row.get(2);
        let ref_column: String = row.get(3);
        let on_update: Option<String> = row.get(4);
        let on_delete: Option<String> = row.get(5);

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

    fks
}

async fn pg_get_create_table(client: &Client, schema: &str, table: &str) -> Result<String> {
    let mut ddl = String::new();

    let col_rows = client
        .query(
            "SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, \
                    c.character_maximum_length, c.numeric_precision, c.numeric_scale, \
                    c.datetime_precision, c.udt_name \
             FROM information_schema.columns c \
             WHERE c.table_schema = $1 AND c.table_name = $2 \
             ORDER BY c.ordinal_position",
            &[&schema, &table],
        )
        .await
        .map_err(query_error)?;

    if col_rows.is_empty() {
        return Err(AppError::new(
            ErrorKind::NotFound,
            format!("table {}.{} not found", schema, table),
        ));
    }

    ddl.push_str(&format!("CREATE TABLE \"{}\".\"{}\" (\n", schema, table));

    let mut col_defs: Vec<String> = Vec::new();
    for row in &col_rows {
        let col_name: String = row.get(0);
        let data_type: String = row.get(1);
        let is_nullable: String = row.get(2);
        let default: Option<String> = row.get(3);

        let mut col_def = format!("    \"{}\" {}", col_name, data_type);
        if let Some(default_value) = default {
            col_def.push_str(&format!(" DEFAULT {}", default_value));
        }
        if is_nullable == "NO" {
            col_def.push_str(" NOT NULL");
        }
        col_defs.push(col_def);
    }

    let pk_rows = client
        .query(
            "SELECT kcu.constraint_name, kcu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name \
              AND tc.table_schema = kcu.table_schema \
             WHERE tc.constraint_type = 'PRIMARY KEY' \
               AND tc.table_schema = $1 AND tc.table_name = $2 \
             ORDER BY tc.constraint_name, kcu.ordinal_position",
            &[&schema, &table],
        )
        .await
        .map_err(query_error)?;

    if !pk_rows.is_empty() {
        let pk_name: String = pk_rows[0].get(0);
        let pk_cols: Vec<String> = pk_rows
            .iter()
            .map(|r| format!("\"{}\"", r.get::<_, String>(1)))
            .collect();
        col_defs.push(format!(
            "    CONSTRAINT \"{}\" PRIMARY KEY ({})",
            pk_name,
            pk_cols.join(", ")
        ));
    }

    let fk_rows = client
        .query(
            "SELECT tc.constraint_name, kcu.column_name, \
                    ccu.table_name AS referenced_table, \
                    ccu.column_name AS referenced_column, \
                    rc.update_rule, rc.delete_rule \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name \
              AND tc.table_schema = kcu.table_schema \
             JOIN information_schema.constraint_column_usage ccu \
               ON tc.constraint_name = ccu.constraint_name \
              AND tc.table_schema = ccu.table_schema \
             JOIN information_schema.referential_constraints rc \
               ON tc.constraint_name = rc.constraint_name \
              AND tc.table_schema = rc.constraint_schema \
             WHERE tc.constraint_type = 'FOREIGN KEY' \
               AND tc.table_schema = $1 AND tc.table_name = $2 \
             ORDER BY tc.constraint_name, kcu.ordinal_position",
            &[&schema, &table],
        )
        .await
        .map_err(query_error)?;

    let fks = group_postgres_foreign_keys(fk_rows);
    for fk in &fks {
        let cols: Vec<String> = fk.columns.iter().map(|c| format!("\"{}\"", c)).collect();
        let ref_cols: Vec<String> = fk
            .referenced_columns
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect();
        let mut fk_def = format!(
            "    CONSTRAINT \"{}\" FOREIGN KEY ({}) REFERENCES \"{}\" ({})",
            fk.name,
            cols.join(", "),
            fk.referenced_table,
            ref_cols.join(", ")
        );
        if let Some(on_del) = &fk.on_delete {
            fk_def.push_str(&format!(" ON DELETE {}", on_del));
        }
        if let Some(on_upd) = &fk.on_update {
            fk_def.push_str(&format!(" ON UPDATE {}", on_upd));
        }
        col_defs.push(fk_def);
    }

    ddl.push_str(&col_defs.join(",\n"));
    ddl.push_str("\n);");

    let idx_rows = client
        .query(
            "SELECT indexname, indexdef FROM pg_indexes \
             WHERE schemaname = $1 AND tablename = $2 \
               AND indexname NOT IN ( \
                   SELECT kcu.constraint_name \
                   FROM information_schema.table_constraints tc \
                   JOIN information_schema.key_column_usage kcu \
                     ON tc.constraint_name = kcu.constraint_name \
                    AND tc.table_schema = kcu.table_schema \
                   WHERE tc.constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY') \
                     AND tc.table_schema = $1 AND tc.table_name = $2 \
               ) \
             ORDER BY indexname",
            &[&schema, &table],
        )
        .await
        .map_err(query_error)?;

    for row in idx_rows {
        let indexdef: String = row.get(1);
        ddl.push('\n');
        ddl.push_str(&indexdef);
        ddl.push(';');
    }

    Ok(ddl)
}

fn postgres_rows_to_query_result(statement: &str, rows: Vec<PgRow>) -> QueryResult {
    let columns = rows
        .first()
        .map(|row| {
            row.columns()
                .iter()
                .map(|column| Column {
                    name: column.name().to_string(),
                    database_type: column.type_().name().to_string(),
                    logical_type: postgres_logical_type(column.type_()),
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
                .map(|index| postgres_cell_value(&row, index))
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

fn postgres_cell_value(row: &PgRow, index: usize) -> CellValue {
    let value_type = row.columns().get(index).map(|column| column.type_());

    match value_type {
        Some(&Type::DATE) => {
            if let Ok(value) = row.try_get::<_, Option<NaiveDate>>(index) {
                return value.map(CellValue::Date).unwrap_or(CellValue::Null);
            }
        }
        Some(&Type::TIME) => {
            if let Ok(value) = row.try_get::<_, Option<NaiveTime>>(index) {
                return value.map(CellValue::Time).unwrap_or(CellValue::Null);
            }
        }
        Some(&Type::TIMESTAMP) => {
            if let Ok(value) = row.try_get::<_, Option<NaiveDateTime>>(index) {
                return value.map(CellValue::DateTime).unwrap_or(CellValue::Null);
            }
        }
        Some(&Type::TIMESTAMPTZ) => {
            if let Ok(value) = row.try_get::<_, Option<DateTime<Utc>>>(index) {
                return value.map(CellValue::Timestamp).unwrap_or(CellValue::Null);
            }
        }
        Some(&Type::NUMERIC) => {
            if let Ok(value) = row.try_get::<_, Option<PgNumericString>>(index) {
                return value
                    .map(|value| CellValue::Decimal(value.0))
                    .unwrap_or(CellValue::Null);
            }
        }
        Some(&Type::JSON) | Some(&Type::JSONB) => {
            if let Ok(value) = row.try_get::<_, Option<serde_json::Value>>(index) {
                return value.map(CellValue::Json).unwrap_or(CellValue::Null);
            }
        }
        _ => {}
    }

    if let Ok(value) = row.try_get::<_, Option<String>>(index) {
        return value.map(CellValue::Text).unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<_, Option<i64>>(index) {
        return value.map(CellValue::Int).unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<_, Option<i32>>(index) {
        return value
            .map(|value| CellValue::Int(i64::from(value)))
            .unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<_, Option<f64>>(index) {
        return value.map(CellValue::Float).unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<_, Option<f32>>(index) {
        return value
            .map(|value| CellValue::Float(f64::from(value)))
            .unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<_, Option<bool>>(index) {
        return value.map(CellValue::Bool).unwrap_or(CellValue::Null);
    }
    if let Ok(value) = row.try_get::<_, Option<Vec<u8>>>(index) {
        return value.map(CellValue::Bytes).unwrap_or(CellValue::Null);
    }

    CellValue::Null
}

fn postgres_logical_type(value_type: &Type) -> LogicalType {
    match *value_type {
        Type::BOOL => LogicalType::Bool,
        Type::INT2 | Type::INT4 | Type::INT8 => LogicalType::Int,
        Type::FLOAT4 | Type::FLOAT8 => LogicalType::Float,
        Type::NUMERIC => LogicalType::Decimal,
        Type::DATE => LogicalType::Date,
        Type::TIME => LogicalType::Time,
        Type::TIMESTAMP | Type::TIMESTAMPTZ => LogicalType::DateTime,
        Type::JSON | Type::JSONB => LogicalType::Json,
        Type::BYTEA => LogicalType::Bytes,
        _ => LogicalType::Text,
    }
}

#[derive(Debug)]
struct PgNumericString(String);

impl<'a> FromSql<'a> for PgNumericString {
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn Error + Sync + Send>> {
        decode_postgres_numeric(raw).map(Self)
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::NUMERIC
    }
}

fn decode_postgres_numeric(
    raw: &[u8],
) -> std::result::Result<String, Box<dyn Error + Sync + Send>> {
    let mut offset = 0;
    let ndigits = read_pg_u16(raw, &mut offset)? as usize;
    let weight = read_pg_i16(raw, &mut offset)?;
    let sign = read_pg_u16(raw, &mut offset)?;
    let dscale = read_pg_u16(raw, &mut offset)? as usize;

    let mut digits = Vec::with_capacity(ndigits);
    for _ in 0..ndigits {
        let digit = read_pg_u16(raw, &mut offset)?;
        if digit >= 10_000 {
            return Err(pg_numeric_error("invalid numeric digit"));
        }
        digits.push(digit);
    }

    match sign {
        0xC000 => return Ok("NaN".to_string()),
        0xD000 => return Ok("Infinity".to_string()),
        0xF000 => return Ok("-Infinity".to_string()),
        0x0000 | 0x4000 => {}
        _ => return Err(pg_numeric_error("invalid numeric sign")),
    }

    let is_negative = sign == 0x4000 && digits.iter().any(|digit| *digit != 0);
    let integer_group_count = (i32::from(weight) + 1).max(0) as usize;
    let mut integer = String::new();
    if integer_group_count == 0 {
        integer.push('0');
    } else {
        for group_index in 0..integer_group_count {
            let digit = digits.get(group_index).copied().unwrap_or(0);
            if group_index == 0 {
                let _ = write!(&mut integer, "{digit}");
            } else {
                let _ = write!(&mut integer, "{digit:04}");
            }
        }
        let trimmed = integer.trim_start_matches('0');
        if trimmed.is_empty() {
            integer = "0".to_string();
        } else if trimmed.len() != integer.len() {
            integer = trimmed.to_string();
        }
    }

    let mut fraction = String::new();
    if dscale > 0 {
        if weight < 0 {
            for _ in 0..((-i32::from(weight) - 1) as usize) {
                fraction.push_str("0000");
            }
        }

        let fraction_start = if weight >= 0 {
            (weight as usize) + 1
        } else {
            0
        };
        for digit in digits.iter().skip(fraction_start) {
            let _ = write!(&mut fraction, "{digit:04}");
        }
        if fraction.len() < dscale {
            fraction.push_str(&"0".repeat(dscale - fraction.len()));
        }
        fraction.truncate(dscale);
    }

    let mut value = String::new();
    if is_negative {
        value.push('-');
    }
    value.push_str(&integer);
    if dscale > 0 {
        value.push('.');
        value.push_str(&fraction);
    }
    Ok(value)
}

fn read_pg_u16(
    raw: &[u8],
    offset: &mut usize,
) -> std::result::Result<u16, Box<dyn Error + Sync + Send>> {
    if raw.len().saturating_sub(*offset) < 2 {
        return Err(pg_numeric_error("truncated numeric value"));
    }
    let value = u16::from_be_bytes([raw[*offset], raw[*offset + 1]]);
    *offset += 2;
    Ok(value)
}

fn read_pg_i16(
    raw: &[u8],
    offset: &mut usize,
) -> std::result::Result<i16, Box<dyn Error + Sync + Send>> {
    read_pg_u16(raw, offset).map(|value| value as i16)
}

fn pg_numeric_error(message: &str) -> Box<dyn Error + Sync + Send> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_positive_postgres_numeric() {
        let raw = postgres_numeric_raw(0, 0x0000, 2, &[123, 4500]);
        assert_eq!(decode_postgres_numeric(&raw).unwrap(), "123.45");
    }

    #[test]
    fn decodes_small_fraction_postgres_numeric() {
        let raw = postgres_numeric_raw(-1, 0x0000, 4, &[1]);
        assert_eq!(decode_postgres_numeric(&raw).unwrap(), "0.0001");
    }

    #[test]
    fn decodes_negative_postgres_numeric() {
        let raw = postgres_numeric_raw(1, 0x4000, 2, &[1, 0, 2500]);
        assert_eq!(decode_postgres_numeric(&raw).unwrap(), "-10000.25");
    }

    fn postgres_numeric_raw(weight: i16, sign: u16, dscale: u16, digits: &[u16]) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(digits.len() as u16).to_be_bytes());
        raw.extend_from_slice(&weight.to_be_bytes());
        raw.extend_from_slice(&sign.to_be_bytes());
        raw.extend_from_slice(&dscale.to_be_bytes());
        for digit in digits {
            raw.extend_from_slice(&digit.to_be_bytes());
        }
        raw
    }
}
