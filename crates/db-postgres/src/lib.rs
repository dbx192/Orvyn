//! PostgreSQL adapter crate.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    ErrorKind, Result,
};
use std::time::Instant;
use tokio_postgres::{Config, NoTls};

pub const ADAPTER_NAME: &str = "postgresql";

#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresConnectionTester;

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
