//! DuckDB adapter crate.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseObject, DatabaseObjectKind, ErrorKind, RelationalMetadataReader, Result, TableColumn,
    TableIndex,
};
use std::{path::PathBuf, time::Instant};

pub const ADAPTER_NAME: &str = "duckdb";

#[derive(Debug, Clone, Copy, Default)]
pub struct DuckDbConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct DuckDbMetadataReader;

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

        Ok(ConnectionTestReport::success(
            DataSourceKind::DuckDb,
            started_at.elapsed().as_millis(),
        )
        .with_server_version(version))
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
