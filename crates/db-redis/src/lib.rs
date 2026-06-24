//! Redis adapter crate.

use async_trait::async_trait;
use db_common::{
    redact_connection_url, AppError, ConnectionProfile, ConnectionTestReport, ConnectionTester,
    DataSourceKind, ErrorKind, RedisDatabase, RedisKeyInfo, RedisKeyScan, RedisMetadataReader,
    Result,
};
use std::time::Instant;

pub const ADAPTER_NAME: &str = "redis";

#[derive(Debug, Clone, Copy, Default)]
pub struct RedisConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct RedisMetadataReaderImpl;

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

#[async_trait]
impl RedisMetadataReader for RedisMetadataReaderImpl {
    fn kind(&self) -> DataSourceKind {
        DataSourceKind::Redis
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<RedisDatabase>> {
        profile.validate()?;
        let client = redis_client(profile, password)?;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_connection_error)?;
        let count = match redis_database_count(&mut conn).await {
            Ok(count) => count,
            Err(_) => 16,
        };
        let mut databases = Vec::new();

        for index in 0..count {
            let key_count = redis_db_key_count(&mut conn, index).await.ok();
            databases.push(RedisDatabase { index, key_count });
        }

        Ok(databases)
    }

    async fn scan_keys(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: u8,
        pattern: Option<&str>,
        cursor: u64,
        limit: u32,
    ) -> Result<RedisKeyScan> {
        profile.validate()?;
        let client = redis_client(profile, password)?;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_connection_error)?;
        select_database(&mut conn, database).await?;

        let mut command = redis::cmd("SCAN");
        command.arg(cursor).arg("COUNT").arg(limit.max(1));
        if let Some(pattern) = pattern.filter(|value| !value.is_empty()) {
            command.arg("MATCH").arg(pattern);
        }

        let (next_cursor, keys): (u64, Vec<String>) =
            command.query_async(&mut conn).await.map_err(redis_query_error)?;
        let mut infos = Vec::with_capacity(keys.len());

        for key in keys {
            let type_name: String = redis::cmd("TYPE")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .map_err(redis_query_error)?;
            let ttl_seconds: i64 = redis::cmd("TTL")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .map_err(redis_query_error)?;
            infos.push(RedisKeyInfo {
                name: key,
                type_name,
                ttl_seconds: (ttl_seconds >= 0).then_some(ttl_seconds),
                size_hint: None,
            });
        }

        Ok(RedisKeyScan {
            next_cursor,
            keys: infos,
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

fn redis_client(profile: &ConnectionProfile, password: Option<&str>) -> Result<redis::Client> {
    let url = redis_url(profile, password)?;
    redis::Client::open(url.as_str()).map_err(|error| {
        AppError::new(ErrorKind::Connection, "invalid Redis connection options")
            .with_detail(redact_connection_url(&error.to_string()))
    })
}

fn redis_connection_error(error: redis::RedisError) -> AppError {
    AppError::new(ErrorKind::Connection, "failed to connect to Redis")
        .with_detail(redact_connection_url(&error.to_string()))
}

fn redis_query_error(error: redis::RedisError) -> AppError {
    AppError::new(ErrorKind::Query, "failed to query Redis metadata")
        .with_detail(redact_connection_url(&error.to_string()))
}

async fn redis_database_count(
    conn: &mut redis::aio::MultiplexedConnection,
) -> Result<u8> {
    let values: Vec<String> = redis::cmd("CONFIG")
        .arg("GET")
        .arg("databases")
        .query_async(conn)
        .await
        .map_err(redis_query_error)?;

    let Some(count) = values.get(1) else {
        return Ok(16);
    };

    count
        .parse::<u8>()
        .map_err(|error| AppError::new(ErrorKind::Query, "invalid Redis databases value").with_detail(error.to_string()))
}

async fn redis_db_key_count(
    conn: &mut redis::aio::MultiplexedConnection,
    database: u8,
) -> Result<u64> {
    select_database(conn, database).await?;
    redis::cmd("DBSIZE")
        .query_async(conn)
        .await
        .map_err(redis_query_error)
}

async fn select_database(
    conn: &mut redis::aio::MultiplexedConnection,
    database: u8,
) -> Result<()> {
    let _: String = redis::cmd("SELECT")
        .arg(database)
        .query_async(conn)
        .await
        .map_err(redis_query_error)?;
    Ok(())
}
