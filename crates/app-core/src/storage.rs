//! Connection profile persistence.

use async_trait::async_trait;
use db_common::{AppError, ConnectionId, ConnectionProfile, ErrorKind, FavoriteQuery, QueryHistoryEntry, QueryId, Result};
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

#[async_trait]
pub trait QueryHistoryStore: Send + Sync {
    async fn list(&self) -> Result<Vec<QueryHistoryEntry>>;

    async fn add(&self, entry: QueryHistoryEntry) -> Result<()>;

    async fn clear(&self) -> Result<()>;

    async fn remove(&self, id: QueryId) -> Result<()>;
}

#[async_trait]
pub trait FavoriteQueryStore: Send + Sync {
    async fn list(&self) -> Result<Vec<FavoriteQuery>>;

    async fn get(&self, id: QueryId) -> Result<Option<FavoriteQuery>>;

    async fn upsert(&self, favorite: FavoriteQuery) -> Result<()>;

    async fn delete(&self, id: QueryId) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct FileQueryHistoryStore {
    path: PathBuf,
}

impl FileQueryHistoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    async fn read_file(&self) -> Result<QueryHistoryFile> {
        let path = self.path.clone();
        task::spawn_blocking(move || read_history_file(path))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "query history read task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn write_file(&self, file: QueryHistoryFile) -> Result<()> {
        let path = self.path.clone();
        task::spawn_blocking(move || write_history_file(path, &file))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "query history write task failed")
                    .with_detail(error.to_string())
            })?
    }
}

fn read_history_file(path: PathBuf) -> Result<QueryHistoryFile> {
    if !path.exists() {
        return Ok(QueryHistoryFile::default());
    }

    let bytes = fs::read(&path)?;
    if bytes.is_empty() {
        return Ok(QueryHistoryFile::default());
    }

    Ok(serde_json::from_slice(&bytes)?)
}

fn write_history_file(path: PathBuf, file: &QueryHistoryFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(file)?;
    fs::write(path, bytes)?;
    Ok(())
}

#[async_trait]
impl QueryHistoryStore for FileQueryHistoryStore {
    async fn list(&self) -> Result<Vec<QueryHistoryEntry>> {
        Ok(self.read_file().await?.entries)
    }

    async fn add(&self, entry: QueryHistoryEntry) -> Result<()> {
        let mut file = self.read_file().await?;
        file.entries.insert(0, entry);
        while file.entries.len() > 100 {
            file.entries.pop();
        }
        self.write_file(file).await
    }

    async fn clear(&self) -> Result<()> {
        let file = QueryHistoryFile::default();
        self.write_file(file).await
    }

    async fn remove(&self, id: QueryId) -> Result<()> {
        let mut file = self.read_file().await?;
        file.entries.retain(|e| e.id != id);
        self.write_file(file).await
    }
}

#[derive(Debug, Clone)]
pub struct FileFavoriteQueryStore {
    path: PathBuf,
}

impl FileFavoriteQueryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    async fn read_file(&self) -> Result<FavoriteQueryFile> {
        let path = self.path.clone();
        task::spawn_blocking(move || read_favorite_file(path))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "favorite query read task failed")
                    .with_detail(error.to_string())
            })?
    }

    async fn write_file(&self, file: FavoriteQueryFile) -> Result<()> {
        let path = self.path.clone();
        task::spawn_blocking(move || write_favorite_file(path, &file))
            .await
            .map_err(|error| {
                AppError::new(ErrorKind::Internal, "favorite query write task failed")
                    .with_detail(error.to_string())
            })?
    }
}

fn read_favorite_file(path: PathBuf) -> Result<FavoriteQueryFile> {
    if !path.exists() {
        return Ok(FavoriteQueryFile::default());
    }

    let bytes = fs::read(&path)?;
    if bytes.is_empty() {
        return Ok(FavoriteQueryFile::default());
    }

    Ok(serde_json::from_slice(&bytes)?)
}

fn write_favorite_file(path: PathBuf, file: &FavoriteQueryFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(file)?;
    fs::write(path, bytes)?;
    Ok(())
}

#[async_trait]
impl FavoriteQueryStore for FileFavoriteQueryStore {
    async fn list(&self) -> Result<Vec<FavoriteQuery>> {
        Ok(self.read_file().await?.favorites.into_values().collect())
    }

    async fn get(&self, id: QueryId) -> Result<Option<FavoriteQuery>> {
        let file = self.read_file().await?;
        Ok(file.favorites.get(&id).cloned())
    }

    async fn upsert(&self, favorite: FavoriteQuery) -> Result<()> {
        let mut file = self.read_file().await?;
        file.favorites.insert(favorite.id, favorite);
        self.write_file(file).await
    }

    async fn delete(&self, id: QueryId) -> Result<()> {
        let mut file = self.read_file().await?;
        if file.favorites.remove(&id).is_none() {
            return Err(AppError::new(
                ErrorKind::Validation,
                "favorite query not found",
            ));
        }
        self.write_file(file).await
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct QueryHistoryFile {
    #[serde(default)]
    entries: Vec<QueryHistoryEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FavoriteQueryFile {
    #[serde(default)]
    favorites: BTreeMap<QueryId, FavoriteQuery>,
}

#[derive(Debug, Default)]
pub struct MemoryQueryHistoryStore {
    entries: Mutex<Vec<QueryHistoryEntry>>,
}

#[async_trait]
impl QueryHistoryStore for MemoryQueryHistoryStore {
    async fn list(&self) -> Result<Vec<QueryHistoryEntry>> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| AppError::internal("history store lock poisoned"))?
            .clone())
    }

    async fn add(&self, entry: QueryHistoryEntry) -> Result<()> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| AppError::internal("history store lock poisoned"))?;
        entries.insert(0, entry);
        while entries.len() > 100 {
            entries.pop();
        }
        Ok(())
    }

    async fn clear(&self) -> Result<()> {
        self.entries
            .lock()
            .map_err(|_| AppError::internal("history store lock poisoned"))?
            .clear();
        Ok(())
    }

    async fn remove(&self, id: QueryId) -> Result<()> {
        self.entries
            .lock()
            .map_err(|_| AppError::internal("history store lock poisoned"))?
            .retain(|e| e.id != id);
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct MemoryFavoriteQueryStore {
    favorites: Mutex<BTreeMap<QueryId, FavoriteQuery>>,
}

#[async_trait]
impl FavoriteQueryStore for MemoryFavoriteQueryStore {
    async fn list(&self) -> Result<Vec<FavoriteQuery>> {
        Ok(self
            .favorites
            .lock()
            .map_err(|_| AppError::internal("favorite store lock poisoned"))?
            .values()
            .cloned()
            .collect())
    }

    async fn get(&self, id: QueryId) -> Result<Option<FavoriteQuery>> {
        Ok(self
            .favorites
            .lock()
            .map_err(|_| AppError::internal("favorite store lock poisoned"))?
            .get(&id)
            .cloned())
    }

    async fn upsert(&self, favorite: FavoriteQuery) -> Result<()> {
        self.favorites
            .lock()
            .map_err(|_| AppError::internal("favorite store lock poisoned"))?
            .insert(favorite.id, favorite);
        Ok(())
    }

    async fn delete(&self, id: QueryId) -> Result<()> {
        self.favorites
            .lock()
            .map_err(|_| AppError::internal("favorite store lock poisoned"))?
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

    #[tokio::test]
    async fn adds_and_lists_history() {
        let store = MemoryQueryHistoryStore::default();
        let entry = QueryHistoryEntry {
            id: QueryId::new(),
            connection_id: ConnectionId::new(),
            summary: "SELECT 1".to_string(),
            sql: "SELECT 1".to_string(),
            executed_at: db_common::now_utc(),
            elapsed_ms: 10,
            affected_rows: 1,
            success: true,
            error_message: None,
        };

        store.add(entry).await.expect("add");
        let history = store.list().await.expect("list");
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn adds_and_lists_favorites() {
        let store = MemoryFavoriteQueryStore::default();
        let favorite = FavoriteQuery {
            id: QueryId::new(),
            connection_id: None,
            title: "Test".to_string(),
            sql: "SELECT 1".to_string(),
            created_at: db_common::now_utc(),
            updated_at: db_common::now_utc(),
        };
        let id = favorite.id;

        store.upsert(favorite).await.expect("upsert");

        assert!(store.get(id).await.expect("get").is_some());
        assert_eq!(store.list().await.expect("list").len(), 1);
    }
}
