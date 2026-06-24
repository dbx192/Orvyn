//! Connection profile persistence.

use async_trait::async_trait;
use db_common::{AppError, ConnectionId, ConnectionProfile, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Mutex};
use tokio::task;

#[async_trait]
pub trait ConnectionProfileStore: Send + Sync {
    async fn list(&self) -> Result<Vec<ConnectionProfile>>;

    async fn get(&self, id: ConnectionId) -> Result<Option<ConnectionProfile>>;

    async fn upsert(&self, profile: ConnectionProfile) -> Result<()>;

    async fn delete(&self, id: ConnectionId) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct FileConnectionProfileStore {
    path: PathBuf,
}

impl FileConnectionProfileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    async fn read_file(&self) -> Result<ConnectionProfileFile> {
        let path = self.path.clone();
        task::spawn_blocking(move || read_profile_file(path))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "connection profile read task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn write_file(&self, file: ConnectionProfileFile) -> Result<()> {
        let path = self.path.clone();
        task::spawn_blocking(move || write_profile_file(path, &file))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "connection profile write task failed")
                    .with_detail(error.to_string())
            })?
    }
}

fn read_profile_file(path: PathBuf) -> Result<ConnectionProfileFile> {
    if !path.exists() {
        return Ok(ConnectionProfileFile::default());
    }

    let bytes = fs::read(&path)?;
    if bytes.is_empty() {
        return Ok(ConnectionProfileFile::default());
    }

    Ok(serde_json::from_slice(&bytes)?)
}

fn write_profile_file(path: PathBuf, file: &ConnectionProfileFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(file)?;
    fs::write(path, bytes)?;
    Ok(())
}

#[async_trait]
impl ConnectionProfileStore for FileConnectionProfileStore {
    async fn list(&self) -> Result<Vec<ConnectionProfile>> {
        Ok(self.read_file().await?.profiles.into_values().collect())
    }

    async fn get(&self, id: ConnectionId) -> Result<Option<ConnectionProfile>> {
        let file = self.read_file().await?;
        Ok(file.profiles.get(&id).cloned())
    }

    async fn upsert(&self, profile: ConnectionProfile) -> Result<()> {
        profile.validate()?;
        let mut file = self.read_file().await?;
        file.profiles.insert(profile.id, profile);
        self.write_file(file).await
    }

    async fn delete(&self, id: ConnectionId) -> Result<()> {
        let mut file = self.read_file().await?;
        if file.profiles.remove(&id).is_none() {
            return Err(AppError::new(
                ErrorKind::Validation,
                "connection profile not found",
            ));
        }
        self.write_file(file).await
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConnectionProfileFile {
    #[serde(default)]
    profiles: BTreeMap<ConnectionId, ConnectionProfile>,
}

#[derive(Debug, Default)]
pub struct MemoryConnectionProfileStore {
    profiles: Mutex<BTreeMap<ConnectionId, ConnectionProfile>>,
}

#[async_trait]
impl ConnectionProfileStore for MemoryConnectionProfileStore {
    async fn list(&self) -> Result<Vec<ConnectionProfile>> {
        Ok(self
            .profiles
            .lock()
            .map_err(|_| AppError::internal("connection store lock poisoned"))?
            .values()
            .cloned()
            .collect())
    }

    async fn get(&self, id: ConnectionId) -> Result<Option<ConnectionProfile>> {
        Ok(self
            .profiles
            .lock()
            .map_err(|_| AppError::internal("connection store lock poisoned"))?
            .get(&id)
            .cloned())
    }

    async fn upsert(&self, profile: ConnectionProfile) -> Result<()> {
        profile.validate()?;
        self.profiles
            .lock()
            .map_err(|_| AppError::internal("connection store lock poisoned"))?
            .insert(profile.id, profile);
        Ok(())
    }

    async fn delete(&self, id: ConnectionId) -> Result<()> {
        self.profiles
            .lock()
            .map_err(|_| AppError::internal("connection store lock poisoned"))?
            .remove(&id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::DataSourceKind;

    #[tokio::test]
    async fn upserts_and_lists_profile() {
        let store = MemoryConnectionProfileStore::default();
        let profile = ConnectionProfile::builder(DataSourceKind::Redis, "redis")
            .host("localhost")
            .port(6379)
            .build()
            .expect("profile");
        let id = profile.id;

        store.upsert(profile).await.expect("upsert");

        assert!(store.get(id).await.expect("get").is_some());
        assert_eq!(store.list().await.expect("list").len(), 1);
    }
}
