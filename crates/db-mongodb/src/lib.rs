use async_trait::async_trait;
use db_common::{
    ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind, Result,
};
use std::time::Instant;
use tokio::net::TcpStream;

#[derive(Debug, Clone, Copy, Default)]
pub struct MongoDbConnectionTester;

#[async_trait]
impl ConnectionTester for MongoDbConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::MongoDb
    }
    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started = Instant::now();
        let host = profile.required_host()?;
        let port = profile.required_port()?;
        TcpStream::connect((host, port)).await.map_err(|e| {
            db_common::AppError::new(
                db_common::ErrorKind::Connection,
                "failed to connect to MongoDB",
            )
            .with_detail(e.to_string())
        })?;
        Ok(ConnectionTestReport::success(
            DataSourceKind::MongoDb,
            started.elapsed().as_millis(),
        ))
    }
}
