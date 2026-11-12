//! Connection management use cases.
//!
//! This service owns profile CRUD, password persistence policy, and connection
//! test dispatch. UI code should call this layer instead of database drivers.

use crate::registry::DriverRegistry;
use crate::storage::ConnectionProfileStore;
use db_common::{
    AppError, ConnectionId, ConnectionProfile, ConnectionStatus, ConnectionTestReport,
    ConnectionTester, ErrorKind, PasswordUpdate, Result,
};
use secure_store::SecureStore;
use std::{sync::Arc, time::Duration};
use tracing::{debug, info, instrument};

/// A connection test must always return control to the UI. Individual drivers
/// may use shorter transport timeouts, but this covers every registered tester
/// and prevents an unresponsive server from leaving the editor permanently
/// busy.
#[cfg(not(test))]
const CONNECTION_TEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(not(test))]
const CONNECTION_TEST_RETRIES: u8 = 1;
#[cfg(not(test))]
const CONNECTION_TEST_RETRY_DELAY: Duration = Duration::from_millis(250);

#[cfg(test)]
const CONNECTION_TEST_TIMEOUT: Duration = Duration::from_millis(20);
#[cfg(test)]
const CONNECTION_TEST_RETRIES: u8 = 0;
#[cfg(test)]
const CONNECTION_TEST_RETRY_DELAY: Duration = Duration::from_millis(1);

#[derive(Clone)]
pub struct ConnectionManager {
    store: Arc<dyn ConnectionProfileStore>,
    secure_store: Arc<dyn SecureStore>,
    registry: DriverRegistry,
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
}

impl ConnectionManager {
    pub fn new(
        store: Arc<dyn ConnectionProfileStore>,
        secure_store: Arc<dyn SecureStore>,
        testers: Vec<Arc<dyn ConnectionTester>>,
    ) -> Self {
        Self {
            store,
            secure_store,
            registry: DriverRegistry::from_testers(testers),
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Build a `ConnectionManager` that routes connection tests through the
    /// supplied [`DriverRegistry`]. This is the preferred production path:
    /// every brand (TiDB, MariaDB, openGauss, ...) gets a tester even when no
    /// dedicated brand adapter exists, because the registry falls back to the
    /// shared protocol-family driver.
    pub fn with_registry(
        store: Arc<dyn ConnectionProfileStore>,
        secure_store: Arc<dyn SecureStore>,
        registry: DriverRegistry,
    ) -> Self {
        Self {
            store,
            secure_store,
            registry,
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn with_default_testers(
        store: Arc<dyn ConnectionProfileStore>,
        secure_store: Arc<dyn SecureStore>,
    ) -> Self {
        Self::with_registry(store, secure_store, DriverRegistry::default())
    }

    /// Expose the underlying registry so other services (SessionManager,
    /// MetadataService, ...) can share the exact same driver instances.
    pub fn registry(&self) -> &DriverRegistry {
        &self.registry
    }

    #[instrument(skip(self, profile, password), fields(kind = ?profile.kind, profile_id = %profile.id))]
    pub async fn create_profile(
        &self,
        mut profile: ConnectionProfile,
        password: PasswordUpdate,
    ) -> Result<ConnectionProfile> {
        let _guard = self.mutation_lock.lock().await;
        profile.validate()?;
        profile.extra_secret_refs.clear();
        self.protect_extra_secrets(&mut profile).await?;
        profile.password_ref = self.apply_password_update(None, &profile, password).await?;
        self.store.upsert(profile.clone()).await?;
        info!(connection_id = %profile.id, "connection profile created");
        Ok(profile)
    }

    #[instrument(skip(self, profile, password), fields(profile_id = %profile.id))]
    pub async fn update_profile(
        &self,
        mut profile: ConnectionProfile,
        password: PasswordUpdate,
    ) -> Result<ConnectionProfile> {
        let _guard = self.mutation_lock.lock().await;
        profile.validate()?;
        let previous =
            self.store.get(profile.id).await?.ok_or_else(|| {
                AppError::new(ErrorKind::Validation, "connection profile not found")
            })?;
        profile
            .extra_secret_refs
            .retain(|key, reference| previous.extra_secret_refs.get(key) == Some(reference));
        self.protect_extra_secrets(&mut profile).await?;
        profile.password_ref = self
            .apply_password_update(previous.password_ref.as_ref(), &profile, password)
            .await?;
        self.store.upsert(profile.clone()).await?;
        // Retire old credentials only after the new profile is durable.
        self.retire_secrets(&previous, Some(&profile)).await;
        info!(connection_id = %profile.id, "connection profile updated");
        Ok(profile)
    }

    pub async fn delete_profile(&self, id: ConnectionId) -> Result<()> {
        let _guard = self.mutation_lock.lock().await;
        let previous = self.store.get(id).await?;
        self.store.delete(id).await?;
        if let Some(previous) = previous {
            self.retire_secrets(&previous, None).await;
        }
        info!(connection_id = %id, "connection profile deleted");
        Ok(())
    }

    pub async fn get_profile(&self, id: ConnectionId) -> Result<Option<ConnectionProfile>> {
        let _guard = self.mutation_lock.lock().await;
        match self.store.get(id).await? {
            Some(profile) => Ok(Some(self.migrate_profile(profile).await?)),
            None => Ok(None),
        }
    }

    pub async fn list_profiles(&self) -> Result<Vec<ConnectionProfile>> {
        let _guard = self.mutation_lock.lock().await;
        let mut profiles = Vec::new();
        for profile in self.store.list().await? {
            profiles.push(self.migrate_profile(profile).await?);
        }
        Ok(profiles)
    }

    async fn protect_extra_secrets(&self, profile: &mut ConnectionProfile) -> Result<()> {
        for key in profile.sensitive_extra_keys() {
            if let Some(secret) = profile.extra_params.remove(key) {
                profile.extra_secret_refs.remove(key);
                if !secret.is_empty() {
                    let reference = self
                        .secure_store
                        .put_secret(&profile.connection_label(), &secret)
                        .await?;
                    profile.extra_secret_refs.insert(key.to_string(), reference);
                }
            }
        }
        Ok(())
    }

    async fn migrate_profile(&self, mut profile: ConnectionProfile) -> Result<ConnectionProfile> {
        if profile
            .sensitive_extra_keys()
            .iter()
            .any(|key| profile.extra_params.contains_key(*key))
        {
            self.protect_extra_secrets(&mut profile).await?;
            self.store.upsert(profile.clone()).await?;
        }
        Ok(profile)
    }

    async fn resolve_extra_secrets(&self, profile: &mut ConnectionProfile) -> Result<()> {
        for (key, reference) in &profile.extra_secret_refs {
            if profile.extra_params.contains_key(key) {
                continue;
            }
            let secret = self
                .secure_store
                .get_secret(reference)
                .await?
                .ok_or_else(|| {
                    AppError::new(
                        ErrorKind::Credential,
                        "stored connection credential is missing",
                    )
                })?;
            profile.extra_params.insert(key.clone(), secret);
        }
        Ok(())
    }

    async fn retire_secrets(
        &self,
        previous: &ConnectionProfile,
        current: Option<&ConnectionProfile>,
    ) {
        for reference in previous
            .password_ref
            .iter()
            .chain(previous.extra_secret_refs.values())
        {
            let retained = current.is_some_and(|p| {
                p.password_ref.as_ref() == Some(reference)
                    || p.extra_secret_refs.values().any(|r| r == reference)
            });
            if !retained {
                // An orphan is recoverable; a failed save must never destroy a usable credential.
                if self.secure_store.delete_secret(reference).await.is_err() {
                    tracing::warn!("failed to retire an unreferenced credential");
                }
            }
        }
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
        let tester = self.registry.tester(profile.kind)?.clone();
        // Callers may pass an editor-built profile with no password_ref. In
        // that case resolve the persisted profile by id before testing.
        let persisted_profile = self.get_profile(profile.id).await?;
        let mut resolved = profile.clone();
        if let Some(saved) = &persisted_profile {
            for (key, reference) in &saved.extra_secret_refs {
                resolved
                    .extra_secret_refs
                    .entry(key.clone())
                    .or_insert(reference.clone());
            }
        }
        self.resolve_extra_secrets(&mut resolved).await?;
        let stored_password = match persisted_profile.as_ref() {
            Some(saved) => self.load_password(saved).await?,
            None => self.load_password(profile).await?,
        };
        let password = runtime_password.or(stored_password.as_deref());

        debug!(kind = ?profile.kind, "testing connection");
        for attempt in 0..=CONNECTION_TEST_RETRIES {
            let result = match tokio::time::timeout(
                CONNECTION_TEST_TIMEOUT,
                tester.test_connection(&resolved, password),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(AppError::new(
                    ErrorKind::Timeout,
                    format!(
                        "{} connection test timed out after {} seconds",
                        profile.kind.display_label(),
                        CONNECTION_TEST_TIMEOUT.as_secs()
                    ),
                )),
            };

            match result {
                Ok(report) => return Ok(report),
                Err(error)
                    if usize::from(attempt) < usize::from(CONNECTION_TEST_RETRIES)
                        && is_transient_connection_error(&error) =>
                {
                    debug!(
                        kind = ?profile.kind,
                        attempt = attempt + 1,
                        "connection test failed transiently; retrying"
                    );
                    tokio::time::sleep(CONNECTION_TEST_RETRY_DELAY).await;
                }
                Err(error) => return Err(error),
            }
        }

        unreachable!("connection test retry loop always returns")
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
            PasswordUpdate::Clear => Ok(None),
            PasswordUpdate::Replace(secret) => {
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
        let profile = self
            .get_profile(id)
            .await?
            .ok_or_else(|| AppError::new(ErrorKind::Validation, "connection profile not found"))?;
        let password = match runtime_password {
            Some(password) => Some(password.to_string()),
            None => self.load_password(&profile).await?,
        };

        let mut profile = profile;
        self.resolve_extra_secrets(&mut profile).await?;
        Ok((profile, password))
    }
}

fn is_transient_connection_error(error: &AppError) -> bool {
    matches!(
        error.kind,
        ErrorKind::Connection | ErrorKind::Timeout | ErrorKind::Io
    )
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
    use db_common::{CredentialRef, DataSourceKind, SslMode};
    use secure_store::SecureStore;
    use std::{collections::BTreeMap, sync::Mutex};

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

    #[tokio::test]
    async fn test_profile_times_out_unresponsive_drivers() {
        let manager = ConnectionManager::new(
            Arc::new(MemoryConnectionProfileStore::default()),
            Arc::new(MemorySecureStore::default()),
            vec![Arc::new(UnresponsiveTester)],
        );
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "slow")
            .host("localhost")
            .username("postgres")
            .build()
            .expect("profile");

        let error = manager
            .test_profile(&profile, None)
            .await
            .expect_err("unresponsive tester should time out");

        assert_eq!(error.kind, ErrorKind::Timeout);
        assert!(error
            .message
            .contains("PostgreSQL connection test timed out"));
    }

    #[tokio::test]
    async fn encrypts_migrates_and_resolves_extra_credentials() {
        let store = Arc::new(MemoryConnectionProfileStore::default());
        let manager = ConnectionManager::new(
            store.clone(),
            Arc::new(MemorySecureStore::default()),
            Vec::new(),
        );
        let mut profile = ConnectionProfile::builder(DataSourceKind::Elasticsearch, "search")
            .host("localhost")
            .build()
            .unwrap();
        profile.set_extra_param("es_api_key", "private-api-key");
        let id = profile.id;
        store.upsert(profile).await.unwrap(); // Simulate the legacy plaintext format.
        let migrated = manager.get_profile(id).await.unwrap().unwrap();
        assert!(!serde_json::to_string(&migrated)
            .unwrap()
            .contains("private-api-key"));
        assert!(!format!("{migrated:?}").contains("private-api-key"));
        assert!(!store
            .get(id)
            .await
            .unwrap()
            .unwrap()
            .extra_params
            .contains_key("es_api_key"));
        assert_eq!(
            manager
                .profile_with_password(id, None)
                .await
                .unwrap()
                .0
                .extra_param("es_api_key"),
            Some("private-api-key")
        );
        let updated = manager
            .update_profile(migrated, PasswordUpdate::Keep)
            .await
            .unwrap();
        assert!(updated.extra_secret_refs.contains_key("es_api_key"));
        let mut cleared = updated;
        cleared.set_extra_param("es_api_key", "");
        manager
            .update_profile(cleared, PasswordUpdate::Keep)
            .await
            .unwrap();
        assert_eq!(
            manager
                .profile_with_password(id, None)
                .await
                .unwrap()
                .0
                .extra_param("es_api_key"),
            None
        );
    }

    fn test_manager() -> ConnectionManager {
        ConnectionManager::new(
            Arc::new(MemoryConnectionProfileStore::default()),
            Arc::new(MemorySecureStore::default()),
            Vec::new(),
        )
    }

    struct UnresponsiveTester;

    #[async_trait]
    impl ConnectionTester for UnresponsiveTester {
        fn kind(&self) -> DataSourceKind {
            DataSourceKind::PostgreSql
        }

        async fn test_connection(
            &self,
            _profile: &ConnectionProfile,
            _password: Option<&str>,
        ) -> Result<ConnectionTestReport> {
            tokio::time::sleep(Duration::from_secs(1)).await;
            unreachable!("the test timeout should cancel this future")
        }
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
