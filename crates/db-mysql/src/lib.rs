//! MySQL adapter crate.

use async_trait::async_trait;
use db_common::{
    redact_connection_url, AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester,
    DataSourceKind, DatabaseObject, DatabaseObjectKind, ErrorKind, RelationalMetadataReader,
    Result, TableColumn, TableIndex,
};
use mysql_async::{prelude::Queryable, Opts, Pool};
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
        let url = mysql_url(profile, password)?;
        let opts = Opts::from_url(&url).map_err(|error| {
            AppError::new(ErrorKind::Connection, "invalid MySQL connection options")
                .with_detail(error.to_string())
        })?;
        let pool = Pool::new(opts);
        let mut conn = pool.get_conn().await.map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to connect to MySQL")
                .with_detail(redact_connection_url(&error.to_string()))
        })?;

        let version = conn
            .query_first::<String, _>("SELECT VERSION()")
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Query, "failed to test MySQL connection")
                    .with_detail(error.to_string())
            })?;

        drop(conn);
        pool.disconnect().await.map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to close MySQL connection")
                .with_detail(error.to_string())
        })?;

        let mut report = ConnectionTestReport::success(
            DataSourceKind::MySql,
            started_at.elapsed().as_millis(),
        );

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
        let pool = mysql_pool(profile, password)?;
        let mut conn = mysql_conn(&pool).await?;
        let rows: Vec<String> = conn.query("SHOW DATABASES").await.map_err(query_error)?;
        close_pool(pool, conn);

        Ok(rows
            .into_iter()
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
        list_mysql_objects(profile, password, database, "BASE TABLE", DatabaseObjectKind::Table)
            .await
    }

    async fn list_views(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        list_mysql_objects(profile, password, database, "VIEW", DatabaseObjectKind::View).await
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
            AppError::new(ErrorKind::Validation, "database is required for MySQL columns")
        })?;
        let pool = mysql_pool(profile, password)?;
        let mut conn = mysql_conn(&pool).await?;
        let rows: Vec<(String, String, String, Option<String>, String, Option<String>)> = conn
            .exec(
                "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY, COLUMN_COMMENT \
                 FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
                 ORDER BY ORDINAL_POSITION",
                (database, table),
            )
            .await
            .map_err(query_error)?;
        close_pool(pool, conn);

        Ok(rows
            .into_iter()
            .map(|(name, database_type, nullable, default_value, column_key, comment)| {
                TableColumn {
                    name,
                    database_type,
                    nullable: nullable.eq_ignore_ascii_case("YES"),
                    default_value,
                    primary_key: column_key == "PRI",
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
            AppError::new(ErrorKind::Validation, "database is required for MySQL indexes")
        })?;
        let pool = mysql_pool(profile, password)?;
        let mut conn = mysql_conn(&pool).await?;
        let rows: Vec<(String, String, u8, String)> = conn
            .exec(
                "SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE, INDEX_NAME = 'PRIMARY' AS IS_PRIMARY \
                 FROM information_schema.STATISTICS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
                 ORDER BY INDEX_NAME, SEQ_IN_INDEX",
                (database, table),
            )
            .await
            .map_err(query_error)?;
        close_pool(pool, conn);

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

fn mysql_pool(profile: &ConnectionProfile, password: Option<&str>) -> Result<Pool> {
    profile.validate()?;
    let url = mysql_url(profile, password)?;
    let opts = Opts::from_url(&url).map_err(|error| {
        AppError::new(ErrorKind::Connection, "invalid MySQL connection options")
            .with_detail(error.to_string())
    })?;
    Ok(Pool::new(opts))
}

async fn mysql_conn(pool: &Pool) -> Result<mysql_async::Conn> {
    pool.get_conn().await.map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to connect to MySQL")
            .with_detail(redact_connection_url(&error.to_string()))
    })
}

fn close_pool(pool: Pool, conn: mysql_async::Conn) {
    drop(conn);
    drop(pool);
}

fn query_error(error: mysql_async::Error) -> AppError {
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
        AppError::new(ErrorKind::Validation, "database is required for MySQL objects")
    })?;
    let pool = mysql_pool(profile, password)?;
    let mut conn = mysql_conn(&pool).await?;
    let rows: Vec<String> = conn
        .exec(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = ? \
             ORDER BY TABLE_NAME",
            (database, table_type),
        )
        .await
        .map_err(query_error)?;
    close_pool(pool, conn);

    Ok(rows
        .into_iter()
        .map(|name| DatabaseObject {
            kind,
            name,
            schema: Some(database.to_string()),
            parent: Some(database.to_string()),
        })
        .collect())
}

fn group_mysql_indexes(rows: Vec<(String, String, u8, String)>) -> Vec<TableIndex> {
    let mut indexes: Vec<TableIndex> = Vec::new();

    for (name, column, non_unique, is_primary) in rows {
        if let Some(index) = indexes.iter_mut().find(|index| index.name == name) {
            index.columns.push(column);
            continue;
        }

        indexes.push(TableIndex {
            name,
            columns: vec![column],
            unique: non_unique == 0,
            primary: is_primary == "1",
        });
    }

    indexes
}
