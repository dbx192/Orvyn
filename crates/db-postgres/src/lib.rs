//! PostgreSQL adapter crate.

use async_trait::async_trait;
use db_common::{
    is_result_set_statement, AppError, CellValue, Column, ConnectionId, ConnectionPoolConfig,
    ConnectionPoolStats, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseObject, DatabaseObjectKind, ErrorKind, LogicalType, QueryResult,
    RelationalMetadataReader, RelationalQueryExecutor, Result, Row, TableColumn, TableIndex,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tokio::sync::{Mutex, Semaphore};
use tokio_postgres::{types::Type, Client, Config, NoTls, Row as PgRow};

pub const ADAPTER_NAME: &str = "postgresql";

#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresMetadataReader;

#[derive(Debug, Clone, Default)]
pub struct PostgresQueryExecutor {
    pools: Arc<Mutex<BTreeMap<ConnectionId, Arc<PostgresPool>>>>,
}

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
        if let Some(pool) = self.pools.lock().await.get(&profile.id).cloned() {
            return Ok(pool);
        }

        let pool = Arc::new(PostgresPool::new(profile, password, config)?);
        self.pools.lock().await.insert(profile.id, pool.clone());
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
        _database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let client = postgres_client(profile, password).await?;
        let rows = client
            .query(
                "SELECT schema_name FROM information_schema.schemata \
                 WHERE schema_name NOT LIKE 'pg_%' AND schema_name <> 'information_schema' \
                 ORDER BY schema_name",
                &[],
            )
            .await
            .map_err(query_error)?;

        Ok(rows
            .into_iter()
            .map(|row| DatabaseObject {
                kind: DatabaseObjectKind::Schema,
                name: row.get(0),
                schema: None,
                parent: profile.database.clone(),
            })
            .collect())
    }

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        list_postgres_objects(
            profile,
            password,
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
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        list_postgres_objects(profile, password, schema, "VIEW", DatabaseObjectKind::View).await
    }

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let schema = schema.unwrap_or("public");
        let client = postgres_client(profile, password).await?;
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
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableIndex>> {
        let schema = schema.unwrap_or("public");
        let client = postgres_client(profile, password).await?;
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
        let Some(pool) = self.pools.lock().await.get(&connection_id).cloned() else {
            return Ok(None);
        };
        Ok(Some(pool.stats().await))
    }

    async fn close_pool(&self, connection_id: ConnectionId) -> Result<()> {
        self.pools.lock().await.remove(&connection_id);
        Ok(())
    }
}

async fn postgres_client(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<tokio_postgres::Client> {
    let config = postgres_config(profile, password)?;

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
    if let Some(database) = profile
        .database
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        config.dbname(database);
    }

    Ok(config)
}

async fn list_postgres_objects(
    profile: &ConnectionProfile,
    password: Option<&str>,
    schema: Option<&str>,
    table_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let schema = schema.unwrap_or("public");
    let client = postgres_client(profile, password).await?;
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
            parent: profile.database.clone(),
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
