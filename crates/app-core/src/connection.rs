//! Connection management use cases.
//!
//! This service owns profile CRUD, password persistence policy, and connection
//! test dispatch. UI code should call this layer instead of database drivers.

use crate::storage::ConnectionProfileStore;
use db_common::{
    unsupported_kind, AppError, ConnectionId, ConnectionProfile, ConnectionStatus,
    ConnectionTestReport, ConnectionTester, DataSourceKind, ErrorKind, PasswordUpdate, Result,
};
use secure_store::SecureStore;
use std::{collections::BTreeMap, sync::Arc};
use tracing::{debug, info, instrument};

#[derive(Clone)]
pub struct ConnectionManager {
    store: Arc<dyn ConnectionProfileStore>,
    secure_store: Arc<dyn SecureStore>,
    testers: Arc<BTreeMap<DataSourceKind, Arc<dyn ConnectionTester>>>,
}

impl ConnectionManager {
    pub fn new(
        store: Arc<dyn ConnectionProfileStore>,
        secure_store: Arc<dyn SecureStore>,
        testers: Vec<Arc<dyn ConnectionTester>>,
    ) -> Self {
        let testers = testers
            .into_iter()
            .map(|tester| (tester.kind(), tester))
            .collect();

        Self {
            store,
            secure_store,
            testers: Arc::new(testers),
        }
    }

    pub fn with_default_testers(
        store: Arc<dyn ConnectionProfileStore>,
        secure_store: Arc<dyn SecureStore>,
    ) -> Self {
        Self::new(
            store,
            secure_store,
            vec![
                Arc::new(db_mysql::MySqlConnectionTester),
                Arc::new(db_mysql::OceanBaseConnectionTester),
                Arc::new(db_mysql::DorisConnectionTester),
                Arc::new(db_mysql::TiDbConnectionTester),
                Arc::new(db_postgres::PostgresConnectionTester),
                Arc::new(db_postgres::TimescaleDbConnectionTester),
                Arc::new(db_mongodb::MongoDbConnectionTester),
                Arc::new(db_sqlserver::SqlServerConnectionTester),
                Arc::new(db_clickhouse::ClickHouseConnectionTester),
                Arc::new(db_sqlite::SqliteConnectionTester),
                Arc::new(db_duckdb::DuckDbConnectionTester),
                Arc::new(db_redis::RedisConnectionTester),
            ],
        )
    }

    #[instrument(skip(self, password), fields(kind = ?profile.kind, profile_id = %profile.id))]
    pub async fn create_profile(
        &self,
        mut profile: ConnectionProfile,
        password: PasswordUpdate,
    ) -> Result<ConnectionProfile> {
        profile.validate()?;
        profile.password_ref = self.apply_password_update(None, &profile, password).await?;
        self.store.upsert(profile.clone()).await?;
        info!(connection_id = %profile.id, "connection profile created");
        Ok(profile)
    }

    #[instrument(skip(self, password), fields(profile_id = %profile.id))]
    pub async fn update_profile(
        &self,
        mut profile: ConnectionProfile,
        password: PasswordUpdate,
    ) -> Result<ConnectionProfile> {
        profile.validate()?;
        let previous =
            self.store.get(profile.id).await?.ok_or_else(|| {
                AppError::new(ErrorKind::Validation, "connection profile not found")
            })?;
        profile.password_ref = self
            .apply_password_update(previous.password_ref.as_ref(), &profile, password)
            .await?;
        self.store.upsert(profile.clone()).await?;
        info!(connection_id = %profile.id, "connection profile updated");
        Ok(profile)
    }

    pub async fn delete_profile(&self, id: ConnectionId) -> Result<()> {
        if let Some(profile) = self.store.get(id).await? {
            if let Some(credential_ref) = profile.password_ref.as_ref() {
                self.secure_store.delete_secret(credential_ref).await?;
            }
        }

        self.store.delete(id).await?;
        info!(connection_id = %id, "connection profile deleted");
        Ok(())
    }

    pub async fn get_profile(&self, id: ConnectionId) -> Result<Option<ConnectionProfile>> {
        self.store.get(id).await
    }

    pub async fn list_profiles(&self) -> Result<Vec<ConnectionProfile>> {
        self.store.list().await
    }

    #[instrument(skip(self, runtime_password), fields(profile_id = %id))]
    pub async fn test_saved_profile(
        &self,
        id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        let profile =
            self.store.get(id).await?.ok_or_else(|| {
                AppError::new(ErrorKind::Validation, "connection profile not found")
            })?;
        self.test_profile(&profile, runtime_password).await
    }

    #[instrument(skip(self, profile, runtime_password), fields(kind = ?profile.kind))]
    pub async fn test_profile(
        &self,
        profile: &ConnectionProfile,
        runtime_password: Option<&str>,
    ) -> Result<ConnectionTestReport> {
        profile.validate()?;
        let tester = self
            .testers
            .get(&profile.kind)
            .ok_or_else(|| unsupported_kind(profile.kind))?;
        // Callers may pass an editor-built profile with no password_ref. In
        // that case resolve the persisted profile by id before testing.
        let persisted_profile = self.store.get(profile.id).await?;
        let stored_password = match persisted_profile.as_ref() {
            Some(saved) => self.load_password(saved).await?,
            None => self.load_password(profile).await?,
        };
        let password = runtime_password.or(stored_password.as_deref());

        debug!(kind = ?profile.kind, "testing connection");
        tester.test_connection(profile, password).await
    }

    pub async fn status_for(&self, id: ConnectionId) -> Result<ConnectionStatus> {
        Ok(if self.store.get(id).await?.is_some() {
            ConnectionStatus::NotConnected
        } else {
            ConnectionStatus::Failed
        })
    }

    async fn apply_password_update(
        &self,
        previous: Option<&db_common::CredentialRef>,
        profile: &ConnectionProfile,
        password: PasswordUpdate,
    ) -> Result<Option<db_common::CredentialRef>> {
        match password {
            PasswordUpdate::Keep => Ok(previous.cloned().or_else(|| profile.password_ref.clone())),
            PasswordUpdate::Clear => {
                if let Some(previous) = previous {
                    self.secure_store.delete_secret(previous).await?;
                }
                Ok(None)
            }
            PasswordUpdate::Replace(secret) => {
                if let Some(previous) = previous {
                    self.secure_store.delete_secret(previous).await?;
                }

                if secret.is_empty() || !profile.security.save_password {
                    return Ok(None);
                }

                let credential_ref = self
                    .secure_store
                    .put_secret(&profile.connection_label(), &secret)
                    .await?;
                Ok(Some(credential_ref))
            }
        }
    }

    async fn load_password(&self, profile: &ConnectionProfile) -> Result<Option<String>> {
        let Some(credential_ref) = profile.password_ref.as_ref() else {
            return Ok(None);
        };

        self.secure_store.get_secret(credential_ref).await
    }

    pub async fn profile_with_password(
        &self,
        id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<(ConnectionProfile, Option<String>)> {
        let profile =
            self.store.get(id).await?.ok_or_else(|| {
                AppError::new(ErrorKind::Validation, "connection profile not found")
            })?;
        let password = match runtime_password {
            Some(password) => Some(password.to_string()),
            None => self.load_password(&profile).await?,
        };

        Ok((profile, password))
    }
}

/// In-memory helper kept for tests and early UI prototypes.
#[derive(Debug, Default)]
pub struct ConnectionRegistry {
    profiles: Vec<ConnectionProfile>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, profile: ConnectionProfile) {
        self.profiles.push(profile);
    }

    pub fn list(&self) -> &[ConnectionProfile] {
        &self.profiles
    }

    pub fn find(&self, id: ConnectionId) -> Option<&ConnectionProfile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    pub fn validate(profile: &ConnectionProfile) -> Result<()> {
        profile.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryConnectionProfileStore;
    use async_trait::async_trait;
    use db_common::{CredentialRef, SslMode};
    use secure_store::SecureStore;
    use std::sync::Mutex;

    #[tokio::test]
    async fn creates_profile_without_password_when_save_is_disabled() {
        let manager = test_manager();
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "local")
            .host("localhost")
            .username("postgres")
            .build()
            .expect("profile");

        let created = manager
            .create_profile(profile, PasswordUpdate::Replace("secret".to_string()))
            .await
            .expect("created");

        assert!(created.password_ref.is_none());
        assert_eq!(manager.list_profiles().await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn saves_password_when_enabled() {
        let manager = test_manager();
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "local")
            .host("localhost")
            .username("postgres")
            .save_password(true)
            .ssl_mode(SslMode::Disable)
            .build()
            .expect("profile");

        let created = manager
            .create_profile(profile, PasswordUpdate::Replace("secret".to_string()))
            .await
            .expect("created");

        assert!(created.password_ref.is_some());
    }

    fn test_manager() -> ConnectionManager {
        ConnectionManager::new(
            Arc::new(MemoryConnectionProfileStore::default()),
            Arc::new(MemorySecureStore::default()),
            Vec::new(),
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
