use crate::connection::ConnectionManager;
use db_common::{
    CellValue, ConnectionId, ErrorKind, RedisKeyInfo, RedisKeyScan, RedisValue, Result,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct RedisKeyLoad {
    pub connection_label: String,
    pub database: u8,
    pub info: RedisKeyInfo,
    pub value: RedisValue,
}

/// One multiplexed connection per (connection profile, database index).
/// The database is baked into the connection URL, so concurrent operations
/// never race a shared SELECT between databases.
type RedisConnectionCache =
    BTreeMap<(ConnectionId, u8), db_redis::redis::aio::MultiplexedConnection>;

/// Run a Redis operation on a cached connection. On transport failure the
/// cached connection is evicted, re-established, and the operation (which may
/// borrow the caller's arguments) is re-expanded against the fresh connection.
macro_rules! redis_op {
    ($self:expr, $connection_id:expr, $database:expr, |$conn:ident| $body:expr) => {{
        let mut $conn = $self.get_connection($connection_id, $database).await?;
        match async { $body }.await {
            Ok(value) => Ok(value),
            Err(error) if error.kind == ErrorKind::Connection => {
                {
                    let mut cache = $self.connections_cache.lock().await;
                    cache.remove(&($connection_id, $database));
                }
                let mut $conn = $self.get_connection($connection_id, $database).await?;
                async { $body }.await
            }
            Err(error) => Err(error),
        }
    }};
}

pub struct RedisService {
    connections: ConnectionManager,
    connections_cache: Arc<Mutex<RedisConnectionCache>>,
}

impl RedisService {
    pub fn new(connections: ConnectionManager) -> Self {
        Self {
            connections,
            connections_cache: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    async fn get_connection(
        &self,
        connection_id: ConnectionId,
        database: u8,
    ) -> Result<db_redis::redis::aio::MultiplexedConnection> {
        let mut cache = self.connections_cache.lock().await;
        if let Some(conn) = cache.get(&(connection_id, database)) {
            return Ok(conn.clone());
        }
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, None)
            .await?;
        let conn = db_redis::RedisConnectorWithAuth::connect(
            &profile,
            password.as_deref(),
            database,
        )
        .await?;
        cache.insert((connection_id, database), conn.clone());
        Ok(conn)
    }

    /// Drop cached connections for a profile (e.g. after edits or disconnect).
    pub async fn invalidate(&self, connection_id: ConnectionId) {
        let mut cache = self.connections_cache.lock().await;
        let stale: Vec<(ConnectionId, u8)> = cache
            .keys()
            .filter(|(id, _)| *id == connection_id)
            .copied()
            .collect();
        for key in stale {
            cache.remove(&key);
        }
    }

    pub async fn scan_keys(
        &self,
        connection_id: ConnectionId,
        database: u8,
        pattern: Option<&str>,
        cursor: u64,
        limit: u32,
    ) -> Result<RedisKeyScan> {
        redis_op!(self, connection_id, database, |conn| {
            let (next_cursor, keys) =
                db_redis::pipeline_scan(&mut conn, pattern, cursor, limit).await?;
            Ok::<_, db_common::AppError>(RedisKeyScan { next_cursor, keys })
        })
    }

    pub async fn get_value(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<RedisValue> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::get_key_value(&mut conn, key).await
        })
    }

    pub async fn get_key_info(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<RedisKeyInfo> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::get_key_info(&mut conn, key).await
        })
    }

    pub async fn get_key_size(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<u64> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::get_key_size(&mut conn, key).await
        })
    }

    pub async fn load_key(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<RedisKeyLoad> {
        let profile = self
            .connections
            .get_profile(connection_id)
            .await?
            .ok_or_else(|| db_common::AppError::validation("connection profile not found"))?;
        let mut load = redis_op!(self, connection_id, database, |conn| {
            let info = db_redis::RedisConnectorWithAuth::get_key_info(&mut conn, key).await?;
            let value = db_redis::RedisConnectorWithAuth::get_key_value(&mut conn, key).await?;
            Ok::<_, db_common::AppError>(RedisKeyLoad {
                connection_label: profile.name.clone(),
                database,
                info,
                value,
            })
        })?;
        load.connection_label = profile.name;
        Ok(load)
    }

    pub async fn delete_key(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<()> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::delete_key(&mut conn, key).await
        })
    }

    pub async fn delete_keys(
        &self,
        connection_id: ConnectionId,
        database: u8,
        keys: &[String],
    ) -> Result<u64> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::delete_keys(&mut conn, keys).await
        })
    }

    pub async fn rename_key(
        &self,
        connection_id: ConnectionId,
        database: u8,
        from: &str,
        to: &str,
    ) -> Result<()> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::rename_key(&mut conn, from, to).await
        })
    }

    pub async fn update_ttl(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        ttl_seconds: Option<i64>,
    ) -> Result<()> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::update_ttl(&mut conn, key, ttl_seconds).await
        })
    }

    pub async fn set_string(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::set_string_value(&mut conn, key, value, ttl_seconds)
                .await
        })
    }

    pub async fn hset(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        field: &str,
        value: &str,
    ) -> Result<u32> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::hset_field(&mut conn, key, field, value).await
        })
    }

    pub async fn hdel(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        field: &str,
    ) -> Result<u32> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::hdel_field(&mut conn, key, field).await
        })
    }

    pub async fn lpush(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        value: &str,
    ) -> Result<i64> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::lpush(&mut conn, key, value).await
        })
    }

    pub async fn rpush(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        value: &str,
    ) -> Result<i64> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::rpush(&mut conn, key, value).await
        })
    }

    pub async fn lpop(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<Option<String>> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::lpop(&mut conn, key).await
        })
    }

    pub async fn rpop(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<Option<String>> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::rpop(&mut conn, key).await
        })
    }

    pub async fn lset(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        index: i64,
        value: &str,
    ) -> Result<()> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::lset(&mut conn, key, index, value).await
        })
    }

    pub async fn lrange(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<Vec<CellValue>> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::lrange(&mut conn, key, start, stop).await
        })
    }

    pub async fn sadd(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::sadd(&mut conn, key, member).await
        })
    }

    pub async fn srem(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::srem(&mut conn, key, member).await
        })
    }

    pub async fn zadd(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        member: &str,
        score: f64,
    ) -> Result<u32> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::zadd(&mut conn, key, member, score).await
        })
    }

    pub async fn zrem(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::zrem(&mut conn, key, member).await
        })
    }

    pub async fn zrange(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<Vec<(CellValue, f64)>> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::zrange(&mut conn, key, start, stop).await
        })
    }

    pub async fn create_key(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        key_type: &str,
        initial_value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::create_key(
                &mut conn,
                key,
                key_type,
                initial_value,
                ttl_seconds,
            )
            .await
        })
    }

    pub async fn execute_command(
        &self,
        connection_id: ConnectionId,
        database: u8,
        command: &str,
    ) -> Result<RedisValue> {
        redis_op!(self, connection_id, database, |conn| {
            db_redis::RedisConnectorWithAuth::execute_command(&mut conn, command).await
        })
    }
}

impl Clone for RedisService {
    fn clone(&self) -> Self {
        Self {
            connections: self.connections.clone(),
            connections_cache: self.connections_cache.clone(),
        }
    }
}
