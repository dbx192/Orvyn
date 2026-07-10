use crate::connection::ConnectionManager;
use db_common::{
    unsupported_kind, AppError, ConnectionId, ConnectionProfile, ConnectionStatus,
    ConnectionTestReport, ConnectionTester, DataSourceKind, ErrorKind, Result, SessionInfo,
    SessionState,
};
use std::{collections::BTreeMap, sync::Arc};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument};

const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct SessionManager {
    connections: ConnectionManager,
    testers: Arc<BTreeMap<DataSourceKind, Arc<dyn ConnectionTester>>>,
    sessions: Arc<RwLock<BTreeMap<ConnectionId, SessionInfo>>>,
}

impl SessionManager {
    pub fn new(connections: ConnectionManager, testers: Vec<Arc<dyn ConnectionTester>>) -> Self {
        let testers = testers
            .into_iter()
            .map(|tester| (tester.kind(), tester))
            .collect();

        Self {
            connections,
            testers: Arc::new(testers),
            sessions: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub fn with_default_testers(connections: ConnectionManager) -> Self {
        Self::new(
            connections,
            vec![
                Arc::new(db_mysql::MySqlConnectionTester),
                Arc::new(db_postgres::PostgresConnectionTester),
                Arc::new(db_sqlite::SqliteConnectionTester),
                Arc::new(db_duckdb::DuckDbConnectionTester),
                Arc::new(db_redis::RedisConnectionTester),
            ],
        )
    }

    #[instrument(skip(self, runtime_password), fields(connection_id = %id))]
    pub async fn connect(
        &self,
        id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<SessionInfo> {
        let (profile, password) = self
            .connections
            .profile_with_password(id, runtime_password)
            .await?;

        let tester = self
            .testers
            .get(&profile.kind)
            .ok_or_else(|| unsupported_kind(profile.kind))?;

        let mut session = self.get_or_create_session(id).await;
        session.mark_connecting();
        self.update_session(session.clone()).await;

        info!(connection_id = %id, "connecting to database");

        match tester.test_connection(&profile, password.as_deref()).await {
            Ok(report) => {
                if report.success {
                    session.mark_connected();
                    self.update_session(session.clone()).await;
                    info!(connection_id = %id, "connection established");
                    Ok(session)
                } else {
                    session.mark_broken(AppError::new(ErrorKind::Connection, &report.message));
                    self.update_session(session.clone()).await;
                    Err(AppError::new(ErrorKind::Connection, &report.message))
                }
            }
            Err(error) => {
                session.mark_broken(error.clone());
                self.update_session(session.clone()).await;
                Err(error)
            }
        }
    }

    #[instrument(skip(self), fields(connection_id = %id))]
    pub async fn disconnect(&self, id: ConnectionId) -> Result<()> {
        let mut session = self
            .get_session(id)
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Validation, "session not found"))?;

        if !session.state.is_connected() && !session.state.is_connecting() {
            return Ok(());
        }

        session.mark_disconnected();
        self.update_session(session).await;
        info!(connection_id = %id, "connection disconnected");
        Ok(())
    }

    #[instrument(skip(self, runtime_password), fields(connection_id = %id))]
    pub async fn reconnect(
        &self,
        id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<SessionInfo> {
        let mut session = self
            .get_session(id)
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Validation, "session not found"))?;

        if !session.can_reconnect() {
            return Err(AppError::new(
                ErrorKind::Connection,
                "max reconnect attempts exceeded",
            ));
        }

        session.mark_reconnecting();
        self.update_session(session.clone()).await;

        // Exponential backoff: base * 2^(attempts-1), capped at MAX_RECONNECT_DELAY.
        // This grows faster than the previous linear scheme, so transient outages
        // are retried quickly while sustained outages don't hammer the server.
        let exponent = session.reconnect_attempts.saturating_sub(1);
        let multiplier = 1u32.checked_shl(exponent).unwrap_or(u32::MAX);
        let delay = session
            .reconnect_delay
            .checked_mul(multiplier)
            .unwrap_or(MAX_RECONNECT_DELAY)
            .min(MAX_RECONNECT_DELAY);
        debug!(connection_id = %id, delay_ms = %delay.as_millis(), "waiting before reconnect");
        tokio::time::sleep(delay).await;

        self.connect(id, runtime_password).await
    }

    pub async fn get_session(&self, id: ConnectionId) -> Result<Option<SessionInfo>> {
        let sessions = self.sessions.read().await;
        Ok(sessions.get(&id).cloned())
    }

    pub async fn get_or_create_session(&self, id: ConnectionId) -> SessionInfo {
        let mut sessions = self.sessions.write().await;
        sessions
            .entry(id)
            .or_insert_with(|| SessionInfo::new(id))
            .clone()
    }

    pub async fn update_session(&self, session: SessionInfo) {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.connection_id, session);
    }

    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }

    pub async fn status_for(&self, id: ConnectionId) -> ConnectionStatus {
        match self.get_session(id).await {
            Ok(Some(session)) => match session.state {
                SessionState::Disconnected => ConnectionStatus::NotConnected,
                SessionState::Connecting | SessionState::Reconnecting => {
                    ConnectionStatus::Connecting
                }
                SessionState::Connected => ConnectionStatus::Connected,
                SessionState::Broken => ConnectionStatus::Failed,
            },
            _ => ConnectionStatus::NotConnected,
        }
    }

    pub async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        let tester = self
            .testers
            .get(&profile.kind)
            .ok_or_else(|| unsupported_kind(profile.kind))?;
        tester.test_connection(profile, password).await
    }

    pub async fn test_saved_profile(
        &self,
        id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        let (profile, password) = self
            .connections
            .profile_with_password(id, runtime_password)
            .await?;
        self.test_connection(&profile, password.as_deref()).await
    }

    #[instrument(skip(self), fields(connection_id = %id))]
    pub async fn notify_broken(&self, id: ConnectionId, error: AppError) {
        if let Ok(Some(mut session)) = self.get_session(id).await {
            if session.state.is_connected() {
                session.mark_broken(error);
                self.update_session(session).await;
                error!(connection_id = %id, "connection broken");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryConnectionProfileStore;
    use async_trait::async_trait;
    use db_common::{CredentialRef, DataSourceKind, PasswordUpdate};
    use secure_store::SecureStore;
    use std::sync::Mutex;

    #[tokio::test]
    async fn connect_and_disconnect() {
        let manager = test_session_manager();
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "test")
            .host("localhost")
            .username("postgres")
            .build()
            .expect("profile");
        manager
            .connections
            .create_profile(profile.clone(), PasswordUpdate::Keep)
            .await
            .expect("created profile");

        let session = manager.connect(profile.id, None).await;
        assert!(session.is_err());

        let status = manager.status_for(profile.id).await;
        assert_eq!(status, ConnectionStatus::Failed);
    }

    #[tokio::test]
    async fn status_reflects_state() {
        let manager = test_session_manager();
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "test")
            .host("localhost")
            .username("postgres")
            .build()
            .expect("profile");
        manager
            .connections
            .create_profile(profile.clone(), PasswordUpdate::Keep)
            .await
            .expect("created profile");

        assert_eq!(
            manager.status_for(profile.id).await,
            ConnectionStatus::NotConnected
        );

        let _ = manager.connect(profile.id, None).await;
        assert_eq!(
            manager.status_for(profile.id).await,
            ConnectionStatus::Failed
        );
    }

    fn test_session_manager() -> SessionManager {
        let connection_manager = ConnectionManager::new(
            Arc::new(MemoryConnectionProfileStore::default()),
            Arc::new(MemorySecureStore::default()),
            vec![Arc::new(db_postgres::PostgresConnectionTester)],
        );
        SessionManager::new(
            connection_manager,
            vec![Arc::new(db_postgres::PostgresConnectionTester)],
        )
    }

    #[derive(Default)]
    struct MemorySecureStore {
        values: Mutex<BTreeMap<String, String>>,
    }

    #[async_trait]
    impl SecureStore for MemorySecureStore {
        async fn put_secret(&self, _label: &str, secret: &str) -> Result<CredentialRef> {
            let mut values = self
                .values
                .lock()
                .map_err(|_| AppError::internal("secure store lock poisoned"))?;
            let key = format!("key-{}", values.len());
            values.insert(key.clone(), secret.to_string());
            Ok(CredentialRef::new("memory", key))
        }

        async fn get_secret(&self, credential_ref: &CredentialRef) -> Result<Option<String>> {
            Ok(self
                .values
                .lock()
                .map_err(|_| AppError::internal("secure store lock poisoned"))?
                .get(&credential_ref.key)
                .cloned())
        }

        async fn delete_secret(&self, credential_ref: &CredentialRef) -> Result<()> {
            self.values
                .lock()
                .map_err(|_| AppError::internal("secure store lock poisoned"))?
                .remove(&credential_ref.key);
            Ok(())
        }
    }
}
