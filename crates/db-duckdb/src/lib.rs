//! DuckDB adapter crate.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    ErrorKind, Result,
};
use std::{path::PathBuf, time::Instant};

pub const ADAPTER_NAME: &str = "duckdb";

#[derive(Debug, Clone, Copy, Default)]
pub struct DuckDbConnectionTester;

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
