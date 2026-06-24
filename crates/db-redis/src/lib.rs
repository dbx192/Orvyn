//! Redis adapter crate.

use async_trait::async_trait;
use db_common::{
    redact_connection_url, AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester,
    DataSourceKind, ErrorKind, Result,
};
use std::time::Instant;

pub const ADAPTER_NAME: &str = "redis";

#[derive(Debug, Clone, Copy, Default)]
pub struct RedisConnectionTester;

#[async_trait]
impl ConnectionTester for RedisConnectionTester {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::Redis
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;

        let started_at = Instant::now();
        let url = redis_url(profile, password)?;
        let client = redis::Client::open(url.as_str()).map_err(|error| {
            AppError::new(ErrorKind::Connection, "invalid Redis connection options")
                .with_detail(redact_connection_url(&error.to_string()))
        })?;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Connection, "failed to connect to Redis")
                    .with_detail(redact_connection_url(&error.to_string()))
            })?;
        let pong: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Query, "failed to test Redis connection")
                    .with_detail(error.to_string())
            })?;

        Ok(ConnectionTestReport {
            kind: DataSourceKind::Redis,
            success: pong.eq_ignore_ascii_case("PONG"),
            elapsed_ms: started_at.elapsed().as_millis(),
            server_version: None,
            message: pong,
        })
    }
}

fn redis_url(profile: &ConnectionProfile, password: Option<&str>) -> Result<String> {
    let host = profile.required_host()?;
    let port = profile.required_port()?;
    let database = profile
        .database
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("0");
    let database_index = database.parse::<u8>().map_err(|error| {
        AppError::new(ErrorKind::Validation, "Redis database must be between 0 and 255")
            .with_detail(error.to_string())
    })?;
    let username = profile.username.as_deref().unwrap_or_default();

    let auth = match (username.is_empty(), password.filter(|value| !value.is_empty())) {
        (true, Some(password)) => format!(":{}@", urlencoding::encode(password)),
        (false, Some(password)) => {
            format!(
                "{}:{}@",
                urlencoding::encode(username),
                urlencoding::encode(password)
            )
        }
        (false, None) => format!("{}@", urlencoding::encode(username)),
        (true, None) => String::new(),
    };

    Ok(format!("redis://{auth}{host}:{port}/{database_index}"))
}
