use crate::connection::ConnectionManager;
use db_common::{
    CellValue, ConnectionId, RedisValue, Result,
};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct RedisService {
    connections: ConnectionManager,
    redis_impl: Arc<db_redis::RedisConnectorWithAuth>,
    connections_cache: Arc<Mutex<std::collections::BTreeMap<ConnectionId, db_redis::redis::aio::MultiplexedConnection>>>,
}

impl RedisService {
    pub fn new(connections: ConnectionManager) -> Self {
        Self {
            connections,
            redis_impl: Arc::new(db_redis::RedisConnectorWithAuth),
            connections_cache: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
        }
    }

    async fn get_connection(
        &self,
        connection_id: ConnectionId,
    ) -> Result<db_redis::redis::aio::MultiplexedConnection> {
        let mut cache = self.connections_cache.lock().await;
        if let Some(conn) = cache.get(&connection_id) {
            Ok(conn.clone())
        } else {
            let (profile, password) = self
                .connections
                .profile_with_password(connection_id, None)
                .await?;
            let conn = db_redis::RedisConnectorWithAuth::new(&profile, password.as_deref()).await?;
            cache.insert(connection_id, conn.clone());
            Ok(conn)
        }
    }

    pub async fn get_value(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<RedisValue> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::get_key_value(&mut conn, database, key).await
    }

    pub async fn delete_key(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<()> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::delete_key(&mut conn, database, key).await
    }

    pub async fn update_ttl(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        ttl_seconds: Option<i64>,
    ) -> Result<()> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::update_ttl(&mut conn, database, key, ttl_seconds).await
    }

    pub async fn set_string(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::set_string_value(&mut conn, database, key, value, ttl_seconds).await
    }

    pub async fn hset(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        field: &str,
        value: &str,
    ) -> Result<u32> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::hset_field(&mut conn, database, key, field, value).await
    }

    pub async fn hdel(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        field: &str,
    ) -> Result<u32> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::hdel_field(&mut conn, database, key, field).await
    }

    pub async fn lpush(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        value: &str,
    ) -> Result<i64> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::lpush(&mut conn, database, key, value).await
    }

    pub async fn rpush(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        value: &str,
    ) -> Result<i64> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::rpush(&mut conn, database, key, value).await
    }

    pub async fn lpop(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<Option<String>> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::lpop(&mut conn, database, key).await
    }

    pub async fn rpop(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
    ) -> Result<Option<String>> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::rpop(&mut conn, database, key).await
    }

    pub async fn lset(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        index: i64,
        value: &str,
    ) -> Result<()> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::lset(&mut conn, database, key, index, value).await
    }

    pub async fn lrange(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<Vec<CellValue>> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::lrange(&mut conn, database, key, start, stop).await
    }

    pub async fn sadd(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::sadd(&mut conn, database, key, member).await
    }

    pub async fn srem(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::srem(&mut conn, database, key, member).await
    }

    pub async fn zadd(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        member: &str,
        score: f64,
    ) -> Result<u32> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::zadd(&mut conn, database, key, member, score).await
    }

    pub async fn zrem(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        member: &str,
    ) -> Result<u32> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::zrem(&mut conn, database, key, member).await
    }

    pub async fn zrange(
        &self,
        connection_id: ConnectionId,
        database: u8,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<Vec<(CellValue, f64)>> {
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::zrange(&mut conn, database, key, start, stop).await
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
        let mut conn = self.get_connection(connection_id).await?;
        db_redis::RedisConnectorWithAuth::create_key(
            &mut conn,
            database,
            key,
            key_type,
            initial_value,
            ttl_seconds,
        )
        .await
    }
}

impl Clone for RedisService {
    fn clone(&self) -> Self {
        Self {
            connections: self.connections.clone(),
            redis_impl: self.redis_impl.clone(),
            connections_cache: self.connections_cache.clone(),
        }
    }
}
