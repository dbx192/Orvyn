use crate::ConnectionId;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_MAX_CONNECTIONS: u32 = 8;
pub const DEFAULT_MIN_CONNECTIONS: u32 = 0;
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_ACQUIRE_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 300_000;
pub const DEFAULT_MAX_LIFETIME_MS: u64 = 1_800_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionPoolConfig {
    pub min_connections: u32,
    pub max_connections: u32,
    pub connect_timeout_ms: u64,
    pub acquire_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub max_lifetime_ms: u64,
}

impl ConnectionPoolConfig {
    pub fn connect_timeout(self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms.max(1))
    }

    pub fn acquire_timeout(self) -> Duration {
        Duration::from_millis(self.acquire_timeout_ms.max(1))
    }

    pub fn idle_timeout(self) -> Duration {
        Duration::from_millis(self.idle_timeout_ms.max(1))
    }

    pub fn max_lifetime(self) -> Duration {
        Duration::from_millis(self.max_lifetime_ms.max(1))
    }
}

impl Default for ConnectionPoolConfig {
    fn default() -> Self {
        Self {
            min_connections: DEFAULT_MIN_CONNECTIONS,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            acquire_timeout_ms: DEFAULT_ACQUIRE_TIMEOUT_MS,
            idle_timeout_ms: DEFAULT_IDLE_TIMEOUT_MS,
            max_lifetime_ms: DEFAULT_MAX_LIFETIME_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionPoolStats {
    pub connection_id: ConnectionId,
    pub size: u32,
    pub idle: u32,
    pub max_connections: u32,
}
