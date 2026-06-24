//! Metadata tree loading service.
//!
//! The service exposes lazy tree loading for the UI. It does not keep long
//! database sessions yet; each request delegates to the matching adapter.

use crate::connection::ConnectionManager;
use db_common::{
    unsupported_kind, AppError, ConnectionId, DataSourceKind, DatabaseObject, ErrorKind,
    MetadataNode, MetadataNodeKind, MetadataPath, RedisKeyInfo, RedisMetadataReader,
    RelationalMetadataReader, Result, TableColumn, TableIndex,
};
use std::{collections::BTreeMap, sync::Arc};
use tracing::instrument;

#[derive(Clone)]
pub struct MetadataService {
    connections: ConnectionManager,
    relational_readers: Arc<BTreeMap<DataSourceKind, Arc<dyn RelationalMetadataReader>>>,
    redis_readers: Arc<BTreeMap<DataSourceKind, Arc<dyn RedisMetadataReader>>>,
}

impl MetadataService {
    pub fn new(
        connections: ConnectionManager,
        relational_readers: Vec<Arc<dyn RelationalMetadataReader>>,
        redis_readers: Vec<Arc<dyn RedisMetadataReader>>,
    ) -> Self {
        Self {
            connections,
            relational_readers: Arc::new(
                relational_readers
                    .into_iter()
                    .map(|reader| (reader.kind(), reader))
                    .collect(),
            ),
            redis_readers: Arc::new(
                redis_readers
                    .into_iter()
                    .map(|reader| (reader.kind(), reader))
                    .collect(),
            ),
        }
    }

    pub fn with_default_readers(connections: ConnectionManager) -> Self {
        Self::new(
            connections,
            vec![
                Arc::new(db_mysql::MySqlMetadataReader),
                Arc::new(db_postgres::PostgresMetadataReader),
                Arc::new(db_sqlite::SqliteMetadataReader),
                Arc::new(db_duckdb::DuckDbMetadataReader),
            ],
            vec![Arc::new(db_redis::RedisMetadataReaderImpl)],
        )
    }

    pub async fn root_nodes(&self) -> Result<Vec<MetadataNode>> {
        let profiles = self.connections.list_profiles().await?;
        Ok(profiles
            .into_iter()
            .map(|profile| {
                MetadataNode::new(
                    MetadataPath::connection(profile.id),
                    profile.name,
                    MetadataNodeKind::Connection,
                )
                .detail(profile.kind.as_str())
            })
            .collect())
    }

    #[instrument(skip(self, runtime_password), fields(connection_id = %path.connection_id))]
    pub async fn children(
        &self,
        path: &MetadataPath,
        runtime_password: Option<&str>,
    ) -> Result<Vec<MetadataNode>> {
        let (profile, password) = self
            .connections
            .profile_with_password(path.connection_id, runtime_password)
            .await?;

        match profile.kind {
            DataSourceKind::Redis => {
                self.redis_children(&profile, password.as_deref(), path)
                    .await
            }
            kind if kind.is_relational() => {
                self.relational_children(&profile, password.as_deref(), path)
                    .await
            }
            kind => Err(unsupported_kind(kind)),
        }
    }

    pub async fn table_columns(
        &self,
        connection_id: ConnectionId,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
        runtime_password: Option<&str>,
    ) -> Result<Vec<TableColumn>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader
            .list_columns(&profile, password.as_deref(), database, schema, table)
            .await
    }

    pub async fn table_indexes(
        &self,
        connection_id: ConnectionId,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
        runtime_password: Option<&str>,
    ) -> Result<Vec<TableIndex>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader
            .list_indexes(&profile, password.as_deref(), database, schema, table)
            .await
    }

    pub async fn scan_redis_keys(
        &self,
        connection_id: ConnectionId,
        database: u8,
        pattern: Option<&str>,
        cursor: u64,
        limit: u32,
        runtime_password: Option<&str>,
    ) -> Result<(u64, Vec<MetadataNode>)> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.redis_reader(profile.kind)?;
        let scan = reader
            .scan_keys(
                &profile,
                password.as_deref(),
                database,
                pattern,
                cursor,
                limit,
            )
            .await?;
        let path = MetadataPath::connection(connection_id)
            .child(MetadataNodeKind::RedisKeysFolder)
            .with_database(database.to_string());

        Ok((
            scan.next_cursor,
            redis_key_nodes(connection_id, &path, scan.keys),
        ))
    }

    async fn relational_children(
        &self,
        profile: &db_common::ConnectionProfile,
        password: Option<&str>,
        path: &MetadataPath,
    ) -> Result<Vec<MetadataNode>> {
        let reader = self.relational_reader(profile.kind)?;

        match path.kind {
            MetadataNodeKind::Connection => {
                let databases = reader.list_databases(profile, password).await?;
                Ok(database_nodes(profile.id, databases))
            }
            MetadataNodeKind::Database => {
                let schemas = reader
                    .list_schemas(profile, password, path.database.as_deref())
                    .await?;
                Ok(schema_nodes(profile.id, path, schemas))
            }
            MetadataNodeKind::Schema => Ok(vec![
                folder_node(profile.id, path, MetadataNodeKind::TablesFolder, "Tables"),
                folder_node(profile.id, path, MetadataNodeKind::ViewsFolder, "Views"),
            ]),
            MetadataNodeKind::TablesFolder => {
                let tables = reader
                    .list_tables(
                        profile,
                        password,
                        path.database.as_deref(),
                        path.schema.as_deref(),
                    )
                    .await?;
                Ok(object_nodes(
                    profile.id,
                    path,
                    tables,
                    MetadataNodeKind::Table,
                ))
            }
            MetadataNodeKind::ViewsFolder => {
                let views = reader
                    .list_views(
                        profile,
                        password,
                        path.database.as_deref(),
                        path.schema.as_deref(),
                    )
                    .await?;
                Ok(object_nodes(
                    profile.id,
                    path,
                    views,
                    MetadataNodeKind::View,
                ))
            }
            MetadataNodeKind::Table => Ok(vec![
                folder_node(profile.id, path, MetadataNodeKind::ColumnsFolder, "Columns"),
                folder_node(profile.id, path, MetadataNodeKind::IndexesFolder, "Indexes"),
            ]),
            MetadataNodeKind::ColumnsFolder => {
                let table = required_object(path)?;
                let columns = reader
                    .list_columns(
                        profile,
                        password,
                        path.database.as_deref(),
                        path.schema.as_deref(),
                        table,
                    )
                    .await?;
                Ok(column_nodes(profile.id, path, columns))
            }
            MetadataNodeKind::IndexesFolder => {
                let table = required_object(path)?;
                let indexes = reader
                    .list_indexes(
                        profile,
                        password,
                        path.database.as_deref(),
                        path.schema.as_deref(),
                        table,
                    )
                    .await?;
                Ok(index_nodes(profile.id, path, indexes))
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn redis_children(
        &self,
        profile: &db_common::ConnectionProfile,
        password: Option<&str>,
        path: &MetadataPath,
    ) -> Result<Vec<MetadataNode>> {
        let reader = self.redis_reader(profile.kind)?;

        match path.kind {
            MetadataNodeKind::Connection => {
                let databases = reader.list_databases(profile, password).await?;
                Ok(databases
                    .into_iter()
                    .map(|database| {
                        let label = format!("DB {}", database.index);
                        let path = MetadataPath::connection(profile.id)
                            .child(MetadataNodeKind::RedisDatabase)
                            .with_database(database.index.to_string());
                        let mut node =
                            MetadataNode::new(path, label, MetadataNodeKind::RedisDatabase);
                        if let Some(key_count) = database.key_count {
                            node = node.detail(format!("{key_count} keys"));
                        }
                        node
                    })
                    .collect())
            }
            MetadataNodeKind::RedisDatabase => Ok(vec![folder_node(
                profile.id,
                path,
                MetadataNodeKind::RedisKeysFolder,
                "Keys",
            )]),
            MetadataNodeKind::RedisKeysFolder => {
                let database = path
                    .database
                    .as_deref()
                    .unwrap_or("0")
                    .parse::<u8>()
                    .map_err(|error| {
                        AppError::new(ErrorKind::Validation, "invalid Redis database index")
                            .with_detail(error.to_string())
                    })?;
                let pattern = path.extra.as_deref();
                let scan = reader
                    .scan_keys(profile, password, database, pattern, 0, 500)
                    .await?;
                Ok(redis_key_nodes(profile.id, path, scan.keys))
            }
            _ => Ok(Vec::new()),
        }
    }

    fn relational_reader(
        &self,
        kind: DataSourceKind,
    ) -> Result<&Arc<dyn RelationalMetadataReader>> {
        self.relational_readers
            .get(&kind)
            .ok_or_else(|| unsupported_kind(kind))
    }

    fn redis_reader(&self, kind: DataSourceKind) -> Result<&Arc<dyn RedisMetadataReader>> {
        self.redis_readers
            .get(&kind)
            .ok_or_else(|| unsupported_kind(kind))
    }
}

fn database_nodes(connection_id: ConnectionId, objects: Vec<DatabaseObject>) -> Vec<MetadataNode> {
    objects
        .into_iter()
        .map(|object| {
            let path = MetadataPath::connection(connection_id)
                .child(MetadataNodeKind::Database)
                .with_database(object.name.clone());
            MetadataNode::new(path, object.name, MetadataNodeKind::Database)
        })
        .collect()
}

fn schema_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    objects: Vec<DatabaseObject>,
) -> Vec<MetadataNode> {
    objects
        .into_iter()
        .map(|object| {
            let mut path = MetadataPath::connection(connection_id).child(MetadataNodeKind::Schema);
            if let Some(database) = parent.database.clone() {
                path = path.with_database(database);
            }
            path = path.with_schema(object.name.clone());
            MetadataNode::new(path, object.name, MetadataNodeKind::Schema)
        })
        .collect()
}

fn object_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    objects: Vec<DatabaseObject>,
    kind: MetadataNodeKind,
) -> Vec<MetadataNode> {
    objects
        .into_iter()
        .map(|object| {
            let mut path = inherit_parent(connection_id, parent)
                .child(kind)
                .with_object(object.name.clone());
            if let Some(schema) = object.schema {
                path = path.with_schema(schema);
            }
            MetadataNode::new(path, object.name, kind)
        })
        .collect()
}

fn column_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    columns: Vec<TableColumn>,
) -> Vec<MetadataNode> {
    columns
        .into_iter()
        .map(|column| {
            let detail = format!(
                "{}{}",
                column.database_type,
                if column.nullable { "" } else { " not null" }
            );
            MetadataNode::new(
                inherit_parent(connection_id, parent)
                    .child(MetadataNodeKind::Column)
                    .with_extra(column.name.clone()),
                column.name,
                MetadataNodeKind::Column,
            )
            .leaf(true)
            .detail(detail)
        })
        .collect()
}

fn index_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    indexes: Vec<TableIndex>,
) -> Vec<MetadataNode> {
    indexes
        .into_iter()
        .map(|index| {
            let detail = if index.columns.is_empty() {
                String::new()
            } else {
                index.columns.join(", ")
            };
            MetadataNode::new(
                inherit_parent(connection_id, parent)
                    .child(MetadataNodeKind::Index)
                    .with_extra(index.name.clone()),
                index.name,
                MetadataNodeKind::Index,
            )
            .leaf(true)
            .detail(detail)
        })
        .collect()
}

fn redis_key_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    keys: Vec<RedisKeyInfo>,
) -> Vec<MetadataNode> {
    keys.into_iter()
        .map(|key| {
            let mut detail = key.type_name;
            if let Some(ttl_seconds) = key.ttl_seconds {
                detail = format!("{detail} ttl={ttl_seconds}s");
            }
            let mut path = inherit_parent(connection_id, parent)
                .child(MetadataNodeKind::RedisKey)
                .with_object(key.name.clone());
            if let Some(database) = parent.database.clone() {
                path = path.with_database(database);
            }

            MetadataNode::new(path, key.name, MetadataNodeKind::RedisKey)
                .leaf(true)
                .detail(detail)
        })
        .collect()
}

fn folder_node(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    kind: MetadataNodeKind,
    label: &str,
) -> MetadataNode {
    MetadataNode::new(
        inherit_parent(connection_id, parent).child(kind),
        label,
        kind,
    )
}

fn inherit_parent(connection_id: ConnectionId, parent: &MetadataPath) -> MetadataPath {
    let mut path = MetadataPath::connection(connection_id);
    if let Some(database) = parent.database.clone() {
        path = path.with_database(database);
    }
    if let Some(schema) = parent.schema.clone() {
        path = path.with_schema(schema);
    }
    if let Some(object) = parent.object.clone() {
        path = path.with_object(object);
    }
    if let Some(extra) = parent.extra.clone() {
        path = path.with_extra(extra);
    }
    path
}

fn required_object(path: &MetadataPath) -> Result<&str> {
    path.object
        .as_deref()
        .ok_or_else(|| AppError::new(ErrorKind::Validation, "metadata object is required"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connection::ConnectionManager, storage::MemoryConnectionProfileStore};
    use async_trait::async_trait;
    use db_common::{ConnectionProfile, CredentialRef, PasswordUpdate};
    use secure_store::SecureStore;
    use std::sync::Mutex;

    #[tokio::test]
    async fn root_nodes_include_saved_connections() {
        let store = Arc::new(MemoryConnectionProfileStore::default());
        let manager =
            ConnectionManager::new(store, Arc::new(MemorySecureStore::default()), Vec::new());
        let profile = ConnectionProfile::builder(DataSourceKind::Redis, "local redis")
            .host("localhost")
            .build()
            .expect("profile");
        manager
            .create_profile(profile, PasswordUpdate::Keep)
            .await
            .expect("created");

        let service = MetadataService::new(manager, Vec::new(), Vec::new());
        let nodes = service.root_nodes().await.expect("nodes");

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].label, "local redis");
        assert_eq!(nodes[0].kind, MetadataNodeKind::Connection);
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
