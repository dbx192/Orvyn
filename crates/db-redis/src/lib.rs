//! Redis adapter crate.

pub use redis;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use db_common::{
    redact_connection_url, AppError, CellValue, ConnectionProfile, ConnectionTestReport,
    ConnectionTester, DataSourceKind, ErrorKind, RedisConnector, RedisDatabase, RedisKeyInfo,
    RedisKeyScan, RedisMetadataReader, RedisValue, Result,
};
use encoding_rs::GB18030;
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
        let url = redis_url(profile, password, profile_database(profile)?)?;
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
        let client = redis_client(profile, password, 0)?;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_connection_error)?;
        let count = redis_database_count(&mut conn).await.unwrap_or(16);

        // A single pipeline keeps SELECT/DBSIZE ordering on one connection,
        // so the whole database listing costs one round trip.
        let mut pipe = redis::pipe();
        for index in 0..count {
            pipe.cmd("SELECT").arg(index).ignore();
            pipe.cmd("DBSIZE");
        }
        let sizes: Vec<u64> = pipe
            .query_async(&mut conn)
            .await
            .map_err(redis_query_error)?;

        Ok((0..count)
            .zip(sizes)
            .map(|(index, key_count)| RedisDatabase {
                index,
                key_count: Some(key_count),
            })
            .collect())
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
        let client = redis_client(profile, password, database)?;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_connection_error)?;
        let (next_cursor, keys) = pipeline_scan(&mut conn, pattern, cursor, limit).await?;

        Ok(RedisKeyScan {
            next_cursor,
            keys,
        })
    }
}

/// SCAN one page and resolve each key's TYPE/TTL in a single pipelined round
/// trip instead of two blocking commands per key.
pub async fn pipeline_scan(
    conn: &mut redis::aio::MultiplexedConnection,
    pattern: Option<&str>,
    cursor: u64,
    limit: u32,
) -> Result<(u64, Vec<RedisKeyInfo>)> {
    let mut command = redis::cmd("SCAN");
    command.arg(cursor).arg("COUNT").arg(limit.max(1));
    if let Some(pattern) = pattern.filter(|value| !value.is_empty()) {
        command.arg("MATCH").arg(pattern);
    }

    let (next_cursor, keys): (u64, Vec<String>) = command
        .query_async(conn)
        .await
        .map_err(redis_query_error)?;
    if keys.is_empty() {
        return Ok((next_cursor, Vec::new()));
    }

    let mut pipe = redis::pipe();
    for key in &keys {
        pipe.cmd("TYPE").arg(key);
        pipe.cmd("TTL").arg(key);
    }
    let replies: Vec<redis::Value> = match pipe.query_async(conn).await {
        Ok(replies) => replies,
        Err(_) => {
            let infos = scan_page_sequential(conn, &keys).await?;
            return Ok((next_cursor, infos));
        }
    };

    if replies.len() != keys.len() * 2 {
        let infos = scan_page_sequential(conn, &keys).await?;
        return Ok((next_cursor, infos));
    }

    let mut infos = Vec::with_capacity(keys.len());
    for (key, pair) in keys.iter().zip(replies.chunks_exact(2)) {
        let Some(type_name) = decode_type_reply(&pair[0]) else {
            let infos = scan_page_sequential(conn, &keys).await?;
            return Ok((next_cursor, infos));
        };
        let Some(ttl) = decode_int_reply(&pair[1]) else {
            let infos = scan_page_sequential(conn, &keys).await?;
            return Ok((next_cursor, infos));
        };
        infos.push(RedisKeyInfo {
            name: key.clone(),
            type_name,
            ttl_seconds: (ttl >= 0).then_some(ttl),
            size_hint: None,
        });
    }

    Ok((next_cursor, infos))
}

fn decode_type_reply(value: &redis::Value) -> Option<String> {
    match value {
        redis::Value::Status(text) => Some(text.clone()),
        redis::Value::Data(bytes) => String::from_utf8(bytes.clone()).ok(),
        _ => None,
    }
}

fn decode_int_reply(value: &redis::Value) -> Option<i64> {
    match value {
        redis::Value::Int(number) => Some(*number),
        redis::Value::Data(bytes) => String::from_utf8(bytes.clone()).ok()?.parse().ok(),
        _ => None,
    }
}

async fn scan_page_sequential(
    conn: &mut redis::aio::MultiplexedConnection,
    keys: &[String],
) -> Result<Vec<RedisKeyInfo>> {
    let mut infos = Vec::with_capacity(keys.len());
    for key in keys {
        let type_name: String = redis::cmd("TYPE")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        let ttl_seconds: i64 = redis::cmd("TTL")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        infos.push(RedisKeyInfo {
            name: key.clone(),
            type_name,
            ttl_seconds: (ttl_seconds >= 0).then_some(ttl_seconds),
            size_hint: None,
        });
    }
    Ok(infos)
}

fn profile_database(profile: &ConnectionProfile) -> Result<u8> {
    profile
        .database
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("0")
        .parse::<u8>()
        .map_err(|error| {
            AppError::new(
                ErrorKind::Validation,
                "Redis database must be between 0 and 255",
            )
            .with_detail(error.to_string())
        })
}

fn redis_url(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: u8,
) -> Result<String> {
    let host = profile.required_host()?;
    let port = profile.required_port()?;
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

    Ok(format!("redis://{auth}{host}:{port}/{database}"))
}

fn redis_client(
    profile: &ConnectionProfile,
    password: Option<&str>,
    database: u8,
) -> Result<redis::Client> {
    let url = redis_url(profile, password, database)?;
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
    let kind = if error.is_connection_dropped() || error.is_io_error() || error.is_timeout() {
        ErrorKind::Connection
    } else {
        ErrorKind::Query
    };
    AppError::new(kind, "failed to query Redis").with_detail(redact_connection_url(&error.to_string()))
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

fn redis_cell(bytes: Vec<u8>) -> CellValue {
    decode_redis_text(&bytes)
        .map(CellValue::Text)
        .unwrap_or(CellValue::Bytes(bytes))
}

fn redis_field_name(bytes: Vec<u8>) -> String {
    decode_redis_text(&bytes).unwrap_or_else(|| BASE64.encode(bytes))
}

pub fn decode_redis_text(bytes: &[u8]) -> Option<String> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return is_textual(text).then(|| text.to_string());
    }

    let (decoded, had_errors) = GB18030.decode_without_bom_handling(bytes);
    (!had_errors && is_textual(&decoded)).then(|| decoded.into_owned())
}

fn is_textual(value: &str) -> bool {
    !value.contains('\0')
        && value
            .chars()
            .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
}

#[async_trait]
impl RedisConnector for RedisConnectorImpl {
    async fn test_connection(&self, profile: &ConnectionProfile) -> Result<()> {
        profile.validate()?;
        let url = redis_url(profile, None, profile_database(profile)?)?;
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
        _database: u8,
        _pattern: Option<&str>,
        _cursor: u64,
        _limit: u32,
    ) -> Result<(u64, Vec<RedisKeyInfo>)> {
        Err(AppError::internal(
            "RedisConnector::scan_keys requires profile and password",
        ))
    }

    async fn get_value(&self, _database: u8, _key: &str) -> Result<RedisValue> {
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
    pub async fn connect(
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: u8,
    ) -> Result<redis::aio::MultiplexedConnection> {
        profile.validate()?;
        let client = redis_client(profile, password, database)?;
        client
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_connection_error)
    }

    pub async fn get_key_value(
        conn: &mut redis::aio::MultiplexedConnection,
        key: &str,
    ) -> Result<RedisValue> {
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
                let items: Vec<Vec<u8>> = redis::cmd("LRANGE")
                    .arg(key)
                    .arg(0)
                    .arg(len.saturating_sub(1))
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                Ok(RedisValue::List(
                    items.into_iter().map(redis_cell).collect(),
                ))
            }
            "set" => {
                let members: Vec<Vec<u8>> = redis::cmd("SMEMBERS")
                    .arg(key)
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                Ok(RedisValue::Set(
                    members.into_iter().map(redis_cell).collect(),
                ))
            }
            "zset" => {
                let members: Vec<(Vec<u8>, f64)> = redis::cmd("ZRANGE")
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
                        .map(|(m, s)| (redis_cell(m), s))
                        .collect(),
                ))
            }
            "hash" => {
                let entries: Vec<(Vec<u8>, Vec<u8>)> = redis::cmd("HGETALL")
                    .arg(key)
                    .query_async(conn)
                    .await
                    .map_err(redis_query_error)?;
                Ok(RedisValue::Hash(
                    entries
                        .into_iter()
                        .map(|(k, v)| (redis_field_name(k), redis_cell(v)))
                        .collect(),
                ))
            }
            "none" => Ok(RedisValue::None),
            _ => Ok(RedisValue::None),
        }
    }

    pub async fn get_key_info(
        conn: &mut redis::aio::MultiplexedConnection,
        key: &str,
    ) -> Result<RedisKeyInfo> {
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
        let size_hint = Some(Self::get_key_size(conn, key).await?);

        Ok(RedisKeyInfo {
            name: key.to_string(),
            type_name,
            ttl_seconds: (ttl_seconds >= 0).then_some(ttl_seconds),
            size_hint,
        })
    }

    pub async fn delete_key(
        conn: &mut redis::aio::MultiplexedConnection,
        key: &str,
    ) -> Result<()> {
        redis::cmd("DEL")
            .arg(key)
            .query_async::<_, i32>(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(())
    }

    pub async fn delete_keys(
        conn: &mut redis::aio::MultiplexedConnection,
        keys: &[String],
    ) -> Result<u64> {
        let mut cmd = redis::cmd("DEL");
        for key in keys {
            cmd.arg(key);
        }
        let deleted: u64 = cmd.query_async(conn).await.map_err(redis_query_error)?;
        Ok(deleted)
    }

    pub async fn rename_key(
        conn: &mut redis::aio::MultiplexedConnection,
        from: &str,
        to: &str,
    ) -> Result<()> {
        redis::cmd("RENAME")
            .arg(from)
            .arg(to)
            .query_async::<_, ()>(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(())
    }

    pub async fn update_ttl(
        conn: &mut redis::aio::MultiplexedConnection,
        key: &str,
        ttl_seconds: Option<i64>,
    ) -> Result<()> {
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
        key: &str,
    ) -> Result<u64> {
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
        key: &str,
        value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
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
        key: &str,
        field: &str,
        value: &str,
    ) -> Result<u32> {
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
        key: &str,
        field: &str,
    ) -> Result<u32> {
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
        key: &str,
        value: &str,
    ) -> Result<i64> {
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
        key: &str,
        value: &str,
    ) -> Result<i64> {
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
        key: &str,
    ) -> Result<Option<String>> {
        let result: Option<String> = redis::cmd("LPOP")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn rpop(
        conn: &mut redis::aio::MultiplexedConnection,
        key: &str,
    ) -> Result<Option<String>> {
        let result: Option<String> = redis::cmd("RPOP")
            .arg(key)
            .query_async(conn)
            .await
            .map_err(redis_query_error)?;
        Ok(result)
    }

    pub async fn lset(
        conn: &mut redis::aio::MultiplexedConnection,
        key: &str,
        index: i64,
        value: &str,
    ) -> Result<()> {
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
        key: &str,
        value: &str,
    ) -> Result<i64> {
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
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<Vec<CellValue>> {
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
        key: &str,
        member: &str,
    ) -> Result<u32> {
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
        key: &str,
        member: &str,
    ) -> Result<u32> {
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
        key: &str,
        member: &str,
        score: f64,
    ) -> Result<u32> {
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
        key: &str,
        member: &str,
    ) -> Result<u32> {
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
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<Vec<(CellValue, f64)>> {
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
        key: &str,
        member: &str,
    ) -> Result<Option<f64>> {
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
        key: &str,
        key_type: &str,
        initial_value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
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
        command: &str,
    ) -> Result<RedisValue> {
        let tokens = normalize_cli_tokens(split_command(command)?);
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
    let mut token_started = false;

    for ch in command.trim_end().trim_end_matches(';').chars() {
        if escaped {
            token_started = true;
            current.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
            continue;
        }

        match ch {
            '\\' => {
                token_started = true;
                escaped = true;
            }
            '\'' | '"' => {
                token_started = true;
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
                if token_started {
                    tokens.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            _ => {
                token_started = true;
                current.push(ch);
            }
        }
    }

    if escaped {
        return Err(AppError::validation("Redis command ends with an escape"));
    }
    if in_quote.is_some() {
        return Err(AppError::validation(
            "Redis command has an unterminated quote",
        ));
    }
    if token_started {
        tokens.push(current);
    }

    Ok(tokens)
}

/// The console accepts the common `SET key=value` shorthand in addition to
/// Redis CLI's canonical `SET key value` syntax.
fn normalize_cli_tokens(mut tokens: Vec<String>) -> Vec<String> {
    if !tokens
        .first()
        .is_some_and(|command| command.eq_ignore_ascii_case("SET"))
    {
        return tokens;
    }

    let Some((key, value)) = tokens
        .get(1)
        .and_then(|argument| argument.split_once('='))
        .filter(|(key, _)| !key.is_empty())
    else {
        return tokens;
    };

    tokens.splice(1..2, [key.to_string(), value.to_string()]);
    tokens
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
    }
}

fn redis_cell_from_raw(value: redis::Value) -> Result<CellValue> {
    match value {
        redis::Value::Nil => Ok(CellValue::Null),
        redis::Value::Int(number) => Ok(CellValue::Int(number)),
        redis::Value::Data(bytes) => Ok(redis_cell(bytes)),
        redis::Value::Status(text) => Ok(CellValue::Text(text)),
        redis::Value::Okay => Ok(CellValue::Text("OK".to_string())),
        redis::Value::Bulk(items) => Ok(CellValue::Json(serde_json::Value::Array(
            items
                .into_iter()
                .map(redis_json_from_raw)
                .collect::<Result<Vec<_>>>()?,
        ))),
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
        CellValue::Bytes(bytes) => Ok(serde_json::Value::String(BASE64.encode(bytes))),
        CellValue::Date(value) => Ok(serde_json::Value::String(value.to_string())),
        CellValue::Time(value) => Ok(serde_json::Value::String(value.to_string())),
        CellValue::DateTime(value) => Ok(serde_json::Value::String(value.to_string())),
        CellValue::Timestamp(value) => Ok(serde_json::Value::String(value.to_rfc3339())),
        CellValue::Json(value) => Ok(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_and_gbk_redis_text() {
        assert_eq!(
            decode_redis_text("中文".as_bytes()).as_deref(),
            Some("中文")
        );
        assert_eq!(
            decode_redis_text(&[0xD6, 0xD0, 0xCE, 0xC4]).as_deref(),
            Some("中文")
        );
    }

    #[test]
    fn keeps_binary_redis_values_as_bytes() {
        assert_eq!(decode_redis_text(&[0, 0xFF, 0x01]), None);
        assert_eq!(
            redis_cell(vec![0, 0xFF, 0x01]),
            CellValue::Bytes(vec![0, 0xFF, 0x01])
        );
    }

    #[test]
    fn parses_cli_arguments_with_quotes_escapes_and_semicolon() {
        assert_eq!(
            split_command(r#"SET "" "Ada \"Lovelace\"";"#).unwrap(),
            vec!["SET", "", "Ada \"Lovelace\""]
        );
        assert_eq!(
            split_command(r#"SET note "line\nnext""#).unwrap(),
            vec!["SET", "note", "line\nnext"]
        );
    }

    #[test]
    fn rejects_malformed_cli_arguments() {
        assert_eq!(
            split_command(r#"GET "unterminated"#).unwrap_err().message,
            "Redis command has an unterminated quote"
        );
        assert_eq!(
            split_command("GET key\\").unwrap_err().message,
            "Redis command ends with an escape"
        );
    }

    #[test]
    fn supports_set_key_value_shorthand() {
        assert_eq!(
            normalize_cli_tokens(split_command("set aa=22").unwrap()),
            vec!["set", "aa", "22"]
        );
        assert_eq!(
            normalize_cli_tokens(split_command("SET aa=hello=world EX 10").unwrap()),
            vec!["SET", "aa", "hello=world", "EX", "10"]
        );
        assert_eq!(
            normalize_cli_tokens(split_command("GET aa=22").unwrap()),
            vec!["GET", "aa=22"]
        );
    }
}
