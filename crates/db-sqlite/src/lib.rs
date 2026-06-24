//! SQLite adapter crate.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    ErrorKind, Result,
};
use std::{path::PathBuf, time::Instant};

pub const ADAPTER_NAME: &str = "sqlite";

#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteConnectionTester;

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
