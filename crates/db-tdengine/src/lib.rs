//! TDengine adapter crate (restricted).
//!
//! TDengine's flagship interface is the TDengine REST API (`/rest/sql`), which
//! accepts SQL over HTTP and returns typed JSON. This adapter only implements
//! connectivity probing so the user can test a saved TDengine config; full SQL
//! query / metadata support is not yet wired to a driver, so callers routing
//! through [`DriverRegistry::relational_*`] get a structured `Unsupported`
//! error (the kind is not registered there).

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind, ErrorKind,
    Result,
};
use std::time::Duration;

pub const ADAPTER_NAME: &str = "tdengine";

#[derive(Debug, Clone, Copy, Default)]
pub struct TdengineConnectionTester;

async fn rest_query(
    profile: &ConnectionProfile,
    password: Option<&str>,
    sql: &str,
) -> Result<serde_json::Value> {
    let url = format!(
        "http://{}:{}/rest/sql",
        profile.required_host()?,
        profile.required_port()?
    );

    let username = profile.username.as_deref().unwrap_or_default();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to build HTTP client")
                .with_detail(error.to_string())
        })?;

    let response = client
        .post(&url)
        .basic_auth(username, password.filter(|value| !value.is_empty()))
        .body(sql.to_string())
        .send()
        .await
        .map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to reach TDengine REST API")
                .with_detail(error.to_string())
        })?;

    let status = response.status();
    let body = response.text().await.map_err(|error| {
        AppError::new(ErrorKind::Connection, "failed to read TDengine response")
            .with_detail(error.to_string())
    })?;

    if !status.is_success() {
        return Err(
            AppError::new(ErrorKind::Connection, "TDengine REST API returned an error")
                .with_detail(format!("HTTP {status}: {body}")),
        );
    }

    let json: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
        AppError::new(
            ErrorKind::Serialization,
            "failed to parse TDengine response",
        )
        .with_detail(error.to_string())
    })?;

    let code = json
        .get("code")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if code != 0 {
        let desc = json
            .get("desc")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown TDengine error");
        return Err(AppError::new(
            ErrorKind::Connection,
            format!("TDengine error: {desc}"),
        ));
    }

    Ok(json)
}

#[async_trait]
impl ConnectionTester for TdengineConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::TDengine
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let started_at = std::time::Instant::now();
        let json = rest_query(profile, password, "SELECT server_version()").await?;
        let version = json
            .get("data")
            .and_then(|data| data.as_array())
            .and_then(|rows| rows.first())
            .and_then(|row| row.as_array())
            .and_then(|cells| cells.first())
            .and_then(|cell| cell.as_str())
            .map(ToOwned::to_owned);

        let mut report = ConnectionTestReport::success(
            DataSourceKind::TDengine,
            started_at.elapsed().as_millis(),
        );
        if let Some(version) = version {
            report = report.with_server_version(version);
        }
        Ok(report)
    }
}
