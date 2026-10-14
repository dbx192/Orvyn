//! Connection tester for the ODBC bridge. A successful test is a login plus
//! one `SQLGetInfo` call for the DBMS product name.

use crate::connect;
use async_trait::async_trait;
use db_common::{
    ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind, Result,
};
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct OdbcConnectionTester {
    kind: DataSourceKind,
}

impl OdbcConnectionTester {
    pub const fn new(kind: DataSourceKind) -> Self {
        Self { kind }
    }
}

#[async_trait]
impl ConnectionTester for OdbcConnectionTester {
    fn kind(&self) -> DataSourceKind {
        self.kind
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started_at = Instant::now();
        let connection = connect::connect(profile, password).await?;

        // The connection is open; SQLGetInfo failing is cosmetic (e.g. exotic
        // drivers that reject it), so degrade to an empty version string.
        let server_version = tokio::task::spawn_blocking(move || {
            connection
                .database_management_system_name()
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();

        Ok(ConnectionTestReport::success(
            self.kind,
            started_at.elapsed().as_millis(),
        )
        .with_server_version(server_version))
    }
}
