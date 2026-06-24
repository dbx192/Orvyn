//! SQLite adapter crate.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    DatabaseObject, DatabaseObjectKind, ErrorKind, RelationalMetadataReader, Result, TableColumn,
    TableIndex,
};
use std::{path::PathBuf, time::Instant};

pub const ADAPTER_NAME: &str = "sqlite";

#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteMetadataReader;

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

        Ok(ConnectionTestReport::success(
            DataSourceKind::Sqlite,
            started_at.elapsed().as_millis(),
        )
        .with_server_version(version))
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
}

fn sqlite_version(path: PathBuf) -> Result<String> {
    if !path.exists() {
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

fn sqlite_connection(path: PathBuf) -> Result<rusqlite::Connection> {
    if !path.exists() {
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
