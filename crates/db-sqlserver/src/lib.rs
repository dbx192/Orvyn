use async_trait::async_trait;
use db_common::{
    ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind, Result,
};
use std::time::Instant;
use tokio::net::TcpStream;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlServerConnectionTester;

#[async_trait]
impl ConnectionTester for SqlServerConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::SqlServer
    }
    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started = Instant::now();
        TcpStream::connect((profile.required_host()?, profile.required_port()?))
            .await
            .map_err(|e| {
                db_common::AppError::new(
                    db_common::ErrorKind::Connection,
                    "failed to connect to SQL Server",
                )
                .with_detail(e.to_string())
            })?;
        Ok(ConnectionTestReport::success(
            DataSourceKind::SqlServer,
            started.elapsed().as_millis(),
        ))
    }
}
