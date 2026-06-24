//! PostgreSQL adapter crate.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseObject, DatabaseObjectKind, ErrorKind, RelationalMetadataReader, Result, TableColumn,
    TableIndex,
};
use std::time::Instant;
use tokio_postgres::{Config, NoTls};

pub const ADAPTER_NAME: &str = "postgresql";

#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresMetadataReader;

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
        let mut config = Config::new();
        config.host(profile.required_host()?);
        config.port(profile.required_port()?);

        if let Some(username) = profile.username.as_deref().filter(|value| !value.is_empty()) {
            config.user(username);
        }
        if let Some(password) = password.filter(|value| !value.is_empty()) {
            config.password(password);
        }
        if let Some(database) = profile.database.as_deref().filter(|value| !value.is_empty()) {
            config.dbname(database);
        }

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
        list_postgres_objects(profile, password, schema, "BASE TABLE", DatabaseObjectKind::Table)
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

async fn postgres_client(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<tokio_postgres::Client> {
    profile.validate()?;
    let mut config = Config::new();
    config.host(profile.required_host()?);
    config.port(profile.required_port()?);

    if let Some(username) = profile.username.as_deref().filter(|value| !value.is_empty()) {
        config.user(username);
    }
    if let Some(password) = password.filter(|value| !value.is_empty()) {
        config.password(password);
    }
    if let Some(database) = profile.database.as_deref().filter(|value| !value.is_empty()) {
        config.dbname(database);
    }

    let (client, connection) = config.connect(NoTls).await.map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to connect to PostgreSQL")
            .with_detail(error.to_string())
    })?;

    tokio::spawn(async move {
        let _ = connection.await;
    });

    Ok(client)
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
