use async_trait::async_trait;
use db_common::{
    ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind, Result,
};
use std::time::Instant;
use tokio::net::TcpStream;

#[derive(Debug, Clone, Copy, Default)]
pub struct ClickHouseConnectionTester;

#[async_trait]
impl ConnectionTester for ClickHouseConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::ClickHouse
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
                    "failed to connect to ClickHouse",
                )
                .with_detail(e.to_string())
            })?;
        Ok(ConnectionTestReport::success(
            DataSourceKind::ClickHouse,
            started.elapsed().as_millis(),
        ))
    }
}
