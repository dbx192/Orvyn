//! MySQL adapter crate.

use async_trait::async_trait;
use db_common::{
    redact_connection_url, AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester,
    DataSourceKind, DatabaseObject, DatabaseObjectKind, ErrorKind, RelationalMetadataReader,
    Result, TableColumn, TableIndex,
};
use sqlx::{mysql::MySqlPoolOptions, MySqlPool, Row};
use std::time::Instant;

pub const ADAPTER_NAME: &str = "mysql";

#[derive(Debug, Clone, Copy, Default)]
pub struct MySqlConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct MySqlMetadataReader;

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
        let pool = mysql_pool(profile, password).await?;
        let version: Option<String> = sqlx::query_scalar("SELECT VERSION()")
            .fetch_optional(&pool)
            .await
            .map_err(mysql_query_error)?;
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
        let pool = mysql_pool(profile, password).await?;
        let rows = sqlx::query("SHOW DATABASES")
            .fetch_all(&pool)
            .await
            .map_err(mysql_query_error)?;
        pool.close().await;

        Ok(rows
            .into_iter()
            .map(|row| row.get::<String, _>(0))
            .filter(|name| !matches!(name.as_str(), "information_schema" | "performance_schema"))
            .map(|name| DatabaseObject {
                kind: DatabaseObjectKind::Database,
                name,
                schema: None,
                parent: None,
            })
            .collect())
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
        list_mysql_objects(
            profile,
            password,
            database,
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
        _schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        list_mysql_objects(
            profile,
            password,
            database,
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
        _schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let database = database.or(profile.database.as_deref()).ok_or_else(|| {
            AppError::new(
                ErrorKind::Validation,
                "database is required for MySQL columns",
            )
        })?;
        let pool = mysql_pool(profile, password).await?;
        let rows = sqlx::query(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY, COLUMN_COMMENT \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
             ORDER BY ORDINAL_POSITION",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&pool)
        .await
        .map_err(mysql_query_error)?;
        pool.close().await;

        Ok(rows
            .into_iter()
            .map(|row| {
                let comment: Option<String> = row.get("COLUMN_COMMENT");
                TableColumn {
                    name: row.get("COLUMN_NAME"),
                    database_type: row.get("COLUMN_TYPE"),
                    nullable: row
                        .get::<String, _>("IS_NULLABLE")
                        .eq_ignore_ascii_case("YES"),
                    default_value: row.get("COLUMN_DEFAULT"),
                    primary_key: row.get::<String, _>("COLUMN_KEY") == "PRI",
                    comment: comment.filter(|value| !value.is_empty()),
                }
            })
            .collect())
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
        let pool = mysql_pool(profile, password).await?;
        let rows = sqlx::query(
            "SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
             ORDER BY INDEX_NAME, SEQ_IN_INDEX",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&pool)
        .await
        .map_err(mysql_query_error)?;
        pool.close().await;

        Ok(group_mysql_indexes(rows))
    }
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

async fn mysql_pool(profile: &ConnectionProfile, password: Option<&str>) -> Result<MySqlPool> {
    profile.validate()?;
    let url = mysql_url(profile, password)?;
    MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .map_err(mysql_connection_error)
}

fn mysql_connection_error(error: sqlx::Error) -> AppError {
    AppError::new(ErrorKind::Connection, "failed to connect to MySQL")
        .with_detail(redact_connection_url(&error.to_string()))
}

fn mysql_query_error(error: sqlx::Error) -> AppError {
    AppError::new(ErrorKind::Query, "failed to query MySQL metadata")
        .with_detail(redact_connection_url(&error.to_string()))
}

async fn list_mysql_objects(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: Option<&str>,
    table_type: &str,
    kind: DatabaseObjectKind,
) -> Result<Vec<DatabaseObject>> {
    let database = database.or(profile.database.as_deref()).ok_or_else(|| {
        AppError::new(
            ErrorKind::Validation,
            "database is required for MySQL objects",
        )
    })?;
    let pool = mysql_pool(profile, password).await?;
    let rows = sqlx::query(
        "SELECT TABLE_NAME FROM information_schema.TABLES \
         WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = ? \
         ORDER BY TABLE_NAME",
    )
    .bind(database)
    .bind(table_type)
    .fetch_all(&pool)
    .await
    .map_err(mysql_query_error)?;
    pool.close().await;

    Ok(rows
        .into_iter()
        .map(|row| {
            let name: String = row.get("TABLE_NAME");
            DatabaseObject {
                kind,
                name,
                schema: Some(database.to_string()),
                parent: Some(database.to_string()),
            }
        })
        .collect())
}

fn group_mysql_indexes(rows: Vec<sqlx::mysql::MySqlRow>) -> Vec<TableIndex> {
    let mut indexes: Vec<TableIndex> = Vec::new();

    for row in rows {
        let name: String = row.get("INDEX_NAME");
        let column: String = row.get("COLUMN_NAME");
        let non_unique: i8 = row.get("NON_UNIQUE");

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

    indexes
}
