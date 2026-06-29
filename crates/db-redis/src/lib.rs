//! Redis adapter crate.

pub use redis;

use async_trait::async_trait;
use db_common::{
    redact_connection_url, AppError, CellValue, ConnectionProfile, ConnectionTestReport,
    ConnectionTester, DataSourceKind, ErrorKind, RedisConnector, RedisDatabase, RedisKeyInfo,
    RedisKeyScan, RedisMetadataReader, RedisValue, Result,
};
use std::time::Instant;

pub const ADAPTER_NAME: &str = "redis";

#[derive(Debug, Clone, Copy, Default)]
pub struct RedisConnectionTester;

#[derive(Debug, Clone, Copy, Default)]
pub struct RedisMetadataReaderImpl;

#[derive(Debug, Clone, Copy, Default)]
pub struct RedisConnectorImpl;

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

        let (next_cursor, keys): (u64, Vec<String>) = command
            .query_async(&mut conn)
            .await
            .map_err(redis_query_error)?;
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
        AppError::new(
            ErrorKind::Validation,
            "Redis database must be between 0 and 255",
        )
        .with_detail(error.to_string())
    })?;
    let username = profile.username.as_deref().unwrap_or_default();

    let auth = match (
        username.is_empty(),
        password.filter(|value| !value.is_empty()),
    ) {
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

async fn redis_database_count(conn: &mut redis::aio::MultiplexedConnection) -> Result<u8> {
    let values: Vec<String> = redis::cmd("CONFIG")
        .arg("GET")
        .arg("databases")
        .query_async(conn)
        .await
        .map_err(redis_query_error)?;

    let Some(count) = values.get(1) else {
        return Ok(16);
    };

    count.parse::<u8>().map_err(|error| {
        AppError::new(ErrorKind::Query, "invalid Redis databases value")
            .with_detail(error.to_string())
    })
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

async fn select_database(conn: &mut redis::aio::MultiplexedConnection, database: u8) -> Result<()> {
    let _: String = redis::cmd("SELECT")
        .arg(database)
        .query_async(conn)
        .await
        .map_err(redis_query_error)?;
    Ok(())
}

#[async_trait]
impl RedisConnector for RedisConnectorImpl {
    async fn test_connection(&self, profile: &ConnectionProfile) -> Result<()> {
        profile.validate()?;
        let url = redis_url(profile, None)?;
        let client = redis::Client::open(url.as_str()).map_err(|error| {
            AppError::new(ErrorKind::Connection, "invalid Redis connection options")
                .with_detail(redact_connection_url(&error.to_string()))
        })?;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_connection_error)?;
        let _: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Query, "failed to test Redis connection")
                    .with_detail(error.to_string())
            })?;
        Ok(())
    }

    async fn list_databases(&self) -> Result<Vec<RedisDatabase>> {
        Err(AppError::internal(
            "RedisConnector::list_databases requires profile and password",
        ))
    }

    async fn scan_keys(
        &self,
        database: u8,
        pattern: Option<&str>,
        cursor: u64,
        limit: u32,
    ) -> Result<(u64, Vec<RedisKeyInfo>)> {
        Err(AppError::internal(
            "RedisConnector::scan_keys requires profile and password",
        ))
    }

    async fn get_value(&self, database: u8, key: &str) -> Result<RedisValue> {
        Err(AppError::internal(
            "RedisConnector::get_value requires profile and password",
        ))
    }

    async fn set_value(
        &self,
        database: u8,
        key: &str,
        value: RedisValue,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        let _ = (database, key, value, ttl_seconds);
        Err(AppError::internal(
            "set_value not implemented in metadata reader",
        ))
    }

    async fn delete_key(&self, database: u8, key: &str) -> Result<()> {
        let _ = (database, key);
        Err(AppError::internal(
            "RedisConnector::delete_key requires profile and password",
        ))
    }

    async fn execute_command(&self, database: u8, command: &str) -> Result<RedisValue> {
        let _ = (database, command);
        Err(AppError::internal(
            "RedisConnector::execute_command requires profile and password",
        ))
    }
}

/// Extended RedisConnector that works with ConnectionProfile and password.
/// This is the main connector used by the application layer.
#[derive(Debug, Clone, Copy, Default)]
pub struct RedisConnectorWithAuth;

impl RedisConnectorWithAuth {
    pub async fn new(
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<redis::aio::MultiplexedConnection> {
        profile.validate()?;
        let client = redis_client(profile, password)?;
        client
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_connection_error)
    }

    pub async fn get_key_value(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
    ) -> Result<RedisValue> {
        select_database(conn, database).await?;
        let type_name: String = redis::cmd("TYPE")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;

        match type_name.as_str() {
            "string" => {
                let value: Vec<u8> = redis::cmd("GET")
                    .arg(key)
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                Ok(RedisValue::String(value))
            }
            "list" => {
                let len: i64 = redis::cmd("LLEN")
                    .arg(key)
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                let items: Vec<String> = redis::cmd("LRANGE")
                    .arg(key)
                    .arg(0)
                    .arg(len.saturating_sub(1))
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                Ok(RedisValue::List(
                    items.into_iter().map(CellValue::Text).collect(),
                ))
            }
            "set" => {
                let members: Vec<String> = redis::cmd("SMEMBERS")
                    .arg(key)
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                Ok(RedisValue::Set(
                    members.into_iter().map(CellValue::Text).collect(),
                ))
            }
            "zset" => {
                let members: Vec<(String, f64)> = redis::cmd("ZRANGE")
                    .arg(key)
                    .arg(0)
                    .arg(-1)
                    .arg("WITHSCORES")
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                Ok(RedisValue::SortedSet(
                    members
                        .into_iter()
                        .map(|(m, s)| (CellValue::Text(m), s))
                        .collect(),
                ))
            }
            "hash" => {
                let entries: Vec<(String, String)> = redis::cmd("HGETALL")
                    .arg(key)
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                Ok(RedisValue::Hash(
                    entries
                        .into_iter()
                        .map(|(k, v)| (k, CellValue::Text(v)))
                        .collect(),
                ))
            }
            "none" => Ok(RedisValue::None),
            _ => Ok(RedisValue::None),
        }
    }

    pub async fn get_key_info(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
    ) -> Result<RedisKeyInfo> {
        select_database(conn, database).await?;
        let type_name: String = redis::cmd("TYPE")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        if type_name == "none" {
            return Err(AppError::new(ErrorKind::NotFound, "Redis key not found"));
        }

        let ttl_seconds: i64 = redis::cmd("TTL")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        let size_hint = Some(Self::get_key_size(conn, database, key).await?);

        Ok(RedisKeyInfo {
            name: key.to_string(),
            type_name,
            ttl_seconds: (ttl_seconds >= 0).then_some(ttl_seconds),
            size_hint,
        })
    }

    pub async fn delete_key(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
    ) -> Result<()> {
        select_database(conn, database).await?;
        redis::cmd("DEL")
            .arg(key)
            .query_async::<_, i32>(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(())
    }

    pub async fn update_ttl(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        ttl_seconds: Option<i64>,
    ) -> Result<()> {
        select_database(conn, database).await?;
        match ttl_seconds {
            Some(ttl) if ttl > 0 => {
                redis::cmd("EXPIRE")
                    .arg(key)
                    .arg(ttl)
                    .query_async::<_, i32>(conn)
                    .await
                    .map_err(redis_query_error)?;
            }
            Some(_) | None => {
                redis::cmd("PERSIST")
                    .arg(key)
                    .query_async::<_, i32>(conn)
                    .await
                    .map_err(redis_query_error)?;
            }
        }
        Ok(())
    }

    pub async fn get_key_size(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
    ) -> Result<u64> {
        select_database(conn, database).await?;
        let type_name: String = redis::cmd("TYPE")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;

        let size: u64 = match type_name.as_str() {
            "string" => redis::cmd("STRLEN")
                .arg(key)
                .query_async(conn)
                .await
                .map_err(redis_query_error)?,
            "list" => redis::cmd("LLEN")
                .arg(key)
                .query_async(conn)
                .await
                .map_err(redis_query_error)?,
            "set" => redis::cmd("SCARD")
                .arg(key)
                .query_async(conn)
                .await
                .map_err(redis_query_error)?,
            "zset" => redis::cmd("ZCARD")
                .arg(key)
                .query_async(conn)
                .await
                .map_err(redis_query_error)?,
            "hash" => redis::cmd("HLEN")
                .arg(key)
                .query_async(conn)
                .await
                .map_err(redis_query_error)?,
            _ => 0,
        };
        Ok(size)
    }

    pub async fn set_string_value(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        select_database(conn, database).await?;
        let mut cmd = redis::cmd("SET");
        cmd.arg(key).arg(value);
        if let Some(ttl) = ttl_seconds {
            if ttl > 0 {
                cmd.arg("EX").arg(ttl);
            }
        }
        cmd.query_async::<_, ()>(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(())
    }

    pub async fn hset_field(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        field: &str,
        value: &str,
    ) -> Result<u32> {
        select_database(conn, database).await?;
        let result: u32 = redis::cmd("HSET")
            .arg(key)
            .arg(field)
            .arg(value)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn hdel_field(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        field: &str,
    ) -> Result<u32> {
        select_database(conn, database).await?;
        let result: u32 = redis::cmd("HDEL")
            .arg(key)
            .arg(field)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn lpush(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        value: &str,
    ) -> Result<i64> {
        select_database(conn, database).await?;
        let result: i64 = redis::cmd("LPUSH")
            .arg(key)
            .arg(value)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn rpush(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        value: &str,
    ) -> Result<i64> {
        select_database(conn, database).await?;
        let result: i64 = redis::cmd("RPUSH")
            .arg(key)
            .arg(value)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn lpop(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
    ) -> Result<Option<String>> {
        select_database(conn, database).await?;
        let result: Option<String> = redis::cmd("LPOP")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn rpop(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
    ) -> Result<Option<String>> {
        select_database(conn, database).await?;
        let result: Option<String> = redis::cmd("RPOP")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn lset(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        index: i64,
        value: &str,
    ) -> Result<()> {
        select_database(conn, database).await?;
        redis::cmd("LSET")
            .arg(key)
            .arg(index)
            .arg(value)
            .query_async::<_, ()>(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(())
    }

    pub async fn lrem(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        value: &str,
    ) -> Result<i64> {
        select_database(conn, database).await?;
        let result: i64 = redis::cmd("LREM")
            .arg(key)
            .arg(1)
            .arg(value)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn lrange(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<Vec<CellValue>> {
        select_database(conn, database).await?;
        let items: Vec<String> = redis::cmd("LRANGE")
            .arg(key)
            .arg(start)
            .arg(stop)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(items.into_iter().map(CellValue::Text).collect())
    }

    pub async fn sadd(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        select_database(conn, database).await?;
        let result: u32 = redis::cmd("SADD")
            .arg(key)
            .arg(member)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn srem(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        select_database(conn, database).await?;
        let result: u32 = redis::cmd("SREM")
            .arg(key)
            .arg(member)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn zadd(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        member: &str,
        score: f64,
    ) -> Result<u32> {
        select_database(conn, database).await?;
        let result: u32 = redis::cmd("ZADD")
            .arg(key)
            .arg(score)
            .arg(member)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn zrem(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        select_database(conn, database).await?;
        let result: u32 = redis::cmd("ZREM")
            .arg(key)
            .arg(member)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn zrange(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<Vec<(CellValue, f64)>> {
        select_database(conn, database).await?;
        let members: Vec<(String, f64)> = redis::cmd("ZRANGE")
            .arg(key)
            .arg(start)
            .arg(stop)
            .arg("WITHSCORES")
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(members
            .into_iter()
            .map(|(m, s)| (CellValue::Text(m), s))
            .collect())
    }

    pub async fn zscore(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<Option<f64>> {
        select_database(conn, database).await?;
        let result: Option<f64> = redis::cmd("ZSCORE")
            .arg(key)
            .arg(member)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn create_key(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        key: &str,
        key_type: &str,
        initial_value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        select_database(conn, database).await?;
        match key_type {
            "string" => {
                let mut cmd = redis::cmd("SET");
                cmd.arg(key).arg(initial_value);
                if let Some(ttl) = ttl_seconds {
                    if ttl > 0 {
                        cmd.arg("EX").arg(ttl);
                    }
                }
                cmd.query_async::<_, ()>(conn)
                    .await
                    .map_err(redis_query_error)?;
            }
            "hash" => {
                if !initial_value.is_empty() {
                    let parts: Vec<&str> = initial_value.splitn(2, ':').collect();
                    if parts.len() == 2 {
                        redis::cmd("HSET")
                            .arg(key)
                            .arg(parts[0])
                            .arg(parts[1])
                            .query_async::<_, ()>(conn)
                            .await
                            .map_err(redis_query_error)?;
                    }
                }
                if let Some(ttl) = ttl_seconds {
                    if ttl > 0 {
                        redis::cmd("EXPIRE")
                            .arg(key)
                            .arg(ttl)
                            .query_async::<_, ()>(conn)
                            .await
                            .map_err(redis_query_error)?;
                    }
                }
            }
            "list" => {
                if !initial_value.is_empty() {
                    redis::cmd("RPUSH")
                        .arg(key)
                        .arg(initial_value)
                        .query_async::<_, ()>(conn)
                        .await
                        .map_err(redis_query_error)?;
                }
                if let Some(ttl) = ttl_seconds {
                    if ttl > 0 {
                        redis::cmd("EXPIRE")
                            .arg(key)
                            .arg(ttl)
                            .query_async::<_, ()>(conn)
                            .await
                            .map_err(redis_query_error)?;
                    }
                }
            }
            "set" => {
                if !initial_value.is_empty() {
                    redis::cmd("SADD")
                        .arg(key)
                        .arg(initial_value)
                        .query_async::<_, ()>(conn)
                        .await
                        .map_err(redis_query_error)?;
                }
                if let Some(ttl) = ttl_seconds {
                    if ttl > 0 {
                        redis::cmd("EXPIRE")
                            .arg(key)
                            .arg(ttl)
                            .query_async::<_, ()>(conn)
                            .await
                            .map_err(redis_query_error)?;
                    }
                }
            }
            "zset" => {
                if !initial_value.is_empty() {
                    let parts: Vec<&str> = initial_value.splitn(2, ':').collect();
                    let score: f64 = if parts.len() == 2 {
                        parts[0].parse().unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    let member = if parts.len() == 2 {
                        parts[1]
                    } else {
                        initial_value
                    };
                    redis::cmd("ZADD")
                        .arg(key)
                        .arg(score)
                        .arg(member)
                        .query_async::<_, ()>(conn)
                        .await
                        .map_err(redis_query_error)?;
                }
                if let Some(ttl) = ttl_seconds {
                    if ttl > 0 {
                        redis::cmd("EXPIRE")
                            .arg(key)
                            .arg(ttl)
                            .query_async::<_, ()>(conn)
                            .await
                            .map_err(redis_query_error)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn execute_command(
        conn: &mut redis::aio::MultiplexedConnection,
        database: u8,
        command: &str,
    ) -> Result<RedisValue> {
        select_database(conn, database).await?;
        let tokens = split_command(command)?;
        let Some((head, tail)) = tokens.split_first() else {
            return Err(AppError::validation("Redis command cannot be empty"));
        };

        let mut cmd = redis::cmd(head);
        for token in tail {
            cmd.arg(token);
        }

        let value: redis::Value = cmd.query_async(conn).await.map_err(redis_query_error)?;
        redis_value_from_raw(value)
    }
}

fn split_command(command: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '\'' | '"' => {
                if let Some(quote) = in_quote {
                    if quote == ch {
                        in_quote = None;
                    } else {
                        current.push(ch);
                    }
                } else {
                    in_quote = Some(ch);
                }
            }
            c if c.is_whitespace() && in_quote.is_none() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if escaped || in_quote.is_some() {
        return Err(AppError::validation("Unterminated Redis command"));
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

fn redis_value_from_raw(value: redis::Value) -> Result<RedisValue> {
    match value {
        redis::Value::Nil => Ok(RedisValue::None),
        redis::Value::Int(number) => Ok(RedisValue::String(number.to_string().into_bytes())),
        redis::Value::Data(bytes) => Ok(RedisValue::String(bytes)),
        redis::Value::Status(text) => Ok(RedisValue::String(text.into_bytes())),
        redis::Value::Okay => Ok(RedisValue::String(b"OK".to_vec())),
        redis::Value::Bulk(items) => Ok(RedisValue::List(
            items
                .into_iter()
                .map(redis_cell_from_raw)
                .collect::<Result<Vec<_>>>()?,
        )),
        other => Ok(RedisValue::String(format!("{other:?}").into_bytes())),
    }
}

fn redis_cell_from_raw(value: redis::Value) -> Result<CellValue> {
    match value {
        redis::Value::Nil => Ok(CellValue::Null),
        redis::Value::Int(number) => Ok(CellValue::Int(number)),
        redis::Value::Data(bytes) => match String::from_utf8(bytes.clone()) {
            Ok(text) => Ok(CellValue::Text(text)),
            Err(_) => Ok(CellValue::Bytes(bytes)),
        },
        redis::Value::Status(text) => Ok(CellValue::Text(text)),
        redis::Value::Okay => Ok(CellValue::Text("OK".to_string())),
        redis::Value::Bulk(items) => Ok(CellValue::Json(serde_json::Value::Array(
            items
                .into_iter()
                .map(redis_json_from_raw)
                .collect::<Result<Vec<_>>>()?,
        ))),
        other => Ok(CellValue::Text(format!("{other:?}"))),
    }
}

fn redis_json_from_raw(value: redis::Value) -> Result<serde_json::Value> {
    match redis_cell_from_raw(value)? {
        CellValue::Null => Ok(serde_json::Value::Null),
        CellValue::Bool(value) => Ok(serde_json::Value::Bool(value)),
        CellValue::Int(value) => Ok(serde_json::json!(value)),
        CellValue::UInt(value) => Ok(serde_json::json!(value)),
        CellValue::Float(value) => Ok(serde_json::json!(value)),
        CellValue::Decimal(value) | CellValue::Text(value) => Ok(serde_json::Value::String(value)),
        CellValue::Bytes(bytes) => Ok(serde_json::Value::String(base64::encode(bytes))),
        CellValue::Date(value) => Ok(serde_json::Value::String(value.to_string())),
        CellValue::Time(value) => Ok(serde_json::Value::String(value.to_string())),
        CellValue::DateTime(value) => Ok(serde_json::Value::String(value.to_string())),
        CellValue::Timestamp(value) => Ok(serde_json::Value::String(value.to_rfc3339())),
        CellValue::Json(value) => Ok(value),
    }
}
