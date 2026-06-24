//! MySQL adapter crate.

use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester, DataSourceKind,
    ErrorKind, Result,
};
use mysql_async::{prelude::Queryable, Opts, Pool};
use std::time::Instant;

pub const ADAPTER_NAME: &str = "mysql";

#[derive(Debug, Clone, Copy, Default)]
pub struct MySqlConnectionTester;

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
        let opts = Opts::from_url(&mysql_url(profile, password)?).map_err(|error| {
            AppError::new(ErrorKind::Connection, "invalid MySQL connection options")
                .with_detail(error.to_string())
        })?;
        let pool = Pool::new(opts);
        let mut conn = pool.get_conn().await.map_err(|error| {
            AppError::new(ErrorKind::Connection, "failed to connect to MySQL")
                .with_detail(error.to_string())
        })?;

        let version = conn
            .query_first::<String, _>("SELECT VERSION()")
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Query, "failed to test MySQL connection")
                    .with_detail(error.to_string())
            })?;

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
