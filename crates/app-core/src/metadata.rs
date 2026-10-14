//! Metadata tree loading service.
//!
//! The service exposes lazy tree loading for the UI. It does not keep long
//! database sessions yet; each request delegates to the matching adapter.

use crate::connection::ConnectionManager;
use crate::registry::DriverRegistry;
use crate::task::TaskManager;
use db_common::{
    unsupported_kind, AppError, ConnectionId, DataSourceKind, DatabaseFunction, DatabaseObject,
    ErrorKind, MetadataNode, MetadataNodeKind, MetadataPath, ProtocolFamily, RedisKeyInfo,
    RedisMetadataReader, RelationalMetadataReader, Result, TableColumn, TableForeignKey,
    TableIndex, TableTrigger,
};
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct MetadataService {
    connections: ConnectionManager,
    registry: DriverRegistry,
    _task_manager: TaskManager,
}

impl MetadataService {
    pub fn new(
        connections: ConnectionManager,
        relational_readers: Vec<Arc<dyn RelationalMetadataReader>>,
        redis_readers: Vec<Arc<dyn RedisMetadataReader>>,
        task_manager: TaskManager,
    ) -> Self {
        let registry = DriverRegistry::default()
            .override_relational_metadata(relational_readers)
            .override_redis_metadata(redis_readers);
        Self::with_registry(connections, registry, task_manager)
    }

    /// Preferred production constructor. The registry already covers every
    /// [`DataSourceKind`] (brand-falling-back-to-protocol-family for metadata
    /// readers); callers only need to pass a task manager.
    pub fn with_registry(
        connections: ConnectionManager,
        registry: DriverRegistry,
        task_manager: TaskManager,
    ) -> Self {
        Self {
            connections,
            registry,
            _task_manager: task_manager,
        }
    }

    pub fn with_default_readers(connections: ConnectionManager) -> Self {
        Self::with_registry(
            connections,
            DriverRegistry::default(),
            TaskManager::default(),
        )
    }

    pub fn with_task_manager(
        connections: ConnectionManager,
        relational_readers: Vec<Arc<dyn RelationalMetadataReader>>,
        redis_readers: Vec<Arc<dyn RedisMetadataReader>>,
        task_manager: TaskManager,
    ) -> Self {
        Self::new(connections, relational_readers, redis_readers, task_manager)
    }

    /// Expose the underlying registry so the desktop shell can build sibling
    /// services (QueryService, SessionManager, ...) from the same instance.
    pub fn registry(&self) -> &DriverRegistry {
        &self.registry
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
            kind if kind.is_document() => {
                self.document_children(&profile, password.as_deref(), path)
                    .await
            }
            kind if kind.is_search() => {
                self.search_children(&profile, password.as_deref(), path)
                    .await
            }
            kind if kind.is_http_sql() => {
                self.http_sql_children(&profile, password.as_deref(), path)
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

    pub async fn list_databases(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader.list_databases(&profile, password.as_deref()).await
    }

    pub async fn list_schemas(
        &self,
        connection_id: ConnectionId,
        database: Option<&str>,
        runtime_password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader
            .list_schemas(&profile, password.as_deref(), database)
            .await
    }

    pub async fn list_tables(
        &self,
        connection_id: ConnectionId,
        database: Option<&str>,
        schema: Option<&str>,
        runtime_password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader
            .list_tables(&profile, password.as_deref(), database, schema)
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

    pub async fn table_foreign_keys(
        &self,
        connection_id: ConnectionId,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
        runtime_password: Option<&str>,
    ) -> Result<Vec<TableForeignKey>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader
            .list_foreign_keys(&profile, password.as_deref(), database, schema, table)
            .await
    }

    pub async fn table_triggers(
        &self,
        connection_id: ConnectionId,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
        runtime_password: Option<&str>,
    ) -> Result<Vec<TableTrigger>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader
            .list_triggers(&profile, password.as_deref(), database, schema, table)
            .await
    }

    pub async fn schema_functions(
        &self,
        connection_id: ConnectionId,
        database: Option<&str>,
        schema: Option<&str>,
        runtime_password: Option<&str>,
    ) -> Result<Vec<DatabaseFunction>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader
            .list_functions(&profile, password.as_deref(), database, schema)
            .await
    }

    pub async fn get_create_table_sql(
        &self,
        connection_id: ConnectionId,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
        runtime_password: Option<&str>,
    ) -> Result<String> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.relational_reader(profile.kind)?;
        reader
            .get_create_table_sql(&profile, password.as_deref(), database, schema, table)
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

    pub async fn list_redis_databases(
        &self,
        connection_id: ConnectionId,
        runtime_password: Option<&str>,
    ) -> Result<Vec<db_common::RedisDatabase>> {
        let (profile, password) = self
            .connections
            .profile_with_password(connection_id, runtime_password)
            .await?;
        let reader = self.redis_reader(profile.kind)?;
        reader.list_databases(&profile, password.as_deref()).await
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
            MetadataNodeKind::Database => self.database_object_nodes(profile, password, path).await,
            MetadataNodeKind::Schema => self.schema_object_nodes(profile, password, path).await,
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
            MetadataNodeKind::Table | MetadataNodeKind::View => Ok(Vec::new()),
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
            MetadataNodeKind::ForeignKeysFolder => {
                let table = required_object(path)?;
                let fks = reader
                    .list_foreign_keys(
                        profile,
                        password,
                        path.database.as_deref(),
                        path.schema.as_deref(),
                        table,
                    )
                    .await?;
                Ok(foreign_key_nodes(profile.id, path, fks))
            }
            MetadataNodeKind::TriggersFolder => {
                let table = required_object(path)?;
                let triggers = reader
                    .list_triggers(
                        profile,
                        password,
                        path.database.as_deref(),
                        path.schema.as_deref(),
                        table,
                    )
                    .await?;
                Ok(trigger_nodes(profile.id, path, triggers))
            }
            MetadataNodeKind::FunctionsFolder => {
                let functions = reader
                    .list_functions(
                        profile,
                        password,
                        path.database.as_deref(),
                        path.schema.as_deref(),
                    )
                    .await?;
                Ok(function_nodes(profile.id, path, functions))
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn database_object_nodes(
        &self,
        profile: &db_common::ConnectionProfile,
        password: Option<&str>,
        path: &MetadataPath,
    ) -> Result<Vec<MetadataNode>> {
        let reader = self.relational_reader(profile.kind)?;
        let schemas = reader
            .list_schemas(profile, password, path.database.as_deref())
            .await?;

        let mut nodes = Vec::new();
        if schemas.is_empty() {
            append_schema_objects(reader, profile, password, path, None, false, &mut nodes).await?;
        } else if profile.kind.protocol_family() == ProtocolFamily::PostgreSql
            && schemas.iter().any(|schema| schema.name == "public")
        {
            append_schema_objects(
                reader,
                profile,
                password,
                path,
                Some("public"),
                false,
                &mut nodes,
            )
            .await?;
            let remaining_schemas = schemas
                .into_iter()
                .filter(|schema| schema.name != "public")
                .collect::<Vec<_>>();
            nodes.extend(schema_nodes(profile.id, path, remaining_schemas));
        } else if schemas.len() == 1 {
            let schema = &schemas[0];
            append_schema_objects(
                reader,
                profile,
                password,
                path,
                Some(schema.name.as_str()),
                false,
                &mut nodes,
            )
            .await?;
        } else {
            nodes.extend(schema_nodes(profile.id, path, schemas));
        }

        nodes.sort_by(|left, right| {
            node_sort_group(left)
                .cmp(&node_sort_group(right))
                .then_with(|| left.label.cmp(&right.label))
        });
        Ok(nodes)
    }

    async fn schema_object_nodes(
        &self,
        profile: &db_common::ConnectionProfile,
        password: Option<&str>,
        path: &MetadataPath,
    ) -> Result<Vec<MetadataNode>> {
        let reader = self.relational_reader(profile.kind)?;
        let mut nodes = Vec::new();
        append_schema_objects(
            reader,
            profile,
            password,
            path,
            path.schema.as_deref(),
            false,
            &mut nodes,
        )
        .await?;
        Ok(nodes)
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

    async fn document_children(
        &self,
        profile: &db_common::ConnectionProfile,
        password: Option<&str>,
        path: &MetadataPath,
    ) -> Result<Vec<MetadataNode>> {
        let connector = self.registry.document(profile.kind)?;

        match path.kind {
            MetadataNodeKind::Connection => {
                let databases = connector.list_databases(profile, password).await?;
                Ok(databases
                    .into_iter()
                    .map(|database| {
                        let path = MetadataPath::connection(profile.id)
                            .child(MetadataNodeKind::Database)
                            .with_database(database.name.clone());
                        let mut node =
                            MetadataNode::new(path, database.name, MetadataNodeKind::Database);
                        if let Some(size_on_disk) = database.size_on_disk {
                            node = node.detail(format!("{size_on_disk} bytes"));
                        }
                        node
                    })
                    .collect())
            }
            MetadataNodeKind::Database => Ok(vec![folder_node(
                profile.id,
                path,
                MetadataNodeKind::TablesFolder,
                "Collections",
            )]),
            MetadataNodeKind::TablesFolder => {
                let database = required_database(profile.database.as_deref(), path)?;
                let collections = connector
                    .list_collections(profile, password, database)
                    .await?;
                Ok(collections
                    .into_iter()
                    .map(|collection| {
                        let path = MetadataPath::connection(profile.id)
                            .child(MetadataNodeKind::Table)
                            .with_database(collection.database.clone())
                            .with_object(collection.name.clone());
                        let mut node = MetadataNode::new(
                            path.clone(),
                            collection.name,
                            MetadataNodeKind::Table,
                        )
                        .leaf(true);
                        if let Some(document_count) = collection.document_count {
                            node = node.detail(format!("{document_count} docs"));
                        }
                        node
                    })
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn search_children(
        &self,
        profile: &db_common::ConnectionProfile,
        password: Option<&str>,
        path: &MetadataPath,
    ) -> Result<Vec<MetadataNode>> {
        let connector = self.registry.search(profile.kind)?;

        match path.kind {
            MetadataNodeKind::Connection => {
                let indices = connector.list_indices(profile, password).await?;
                Ok(indices
                    .into_iter()
                    .map(|index| {
                        let path = MetadataPath::connection(profile.id)
                            .child(MetadataNodeKind::Table)
                            .with_object(index.name.clone());
                        let mut node =
                            MetadataNode::new(path, index.name, MetadataNodeKind::Table).leaf(true);
                        if let Some(document_count) = index.document_count {
                            node = node.detail(format!("{document_count} docs"));
                        }
                        if let Some(health) = index.health {
                            node = node.detail(health.to_string());
                        }
                        node
                    })
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn http_sql_children(
        &self,
        profile: &db_common::ConnectionProfile,
        password: Option<&str>,
        path: &MetadataPath,
    ) -> Result<Vec<MetadataNode>> {
        let connector = self.registry.http_sql(profile.kind)?;

        match path.kind {
            MetadataNodeKind::Connection => {
                let databases = connector.list_databases(profile, password).await?;
                Ok(database_nodes(profile.id, databases))
            }
            MetadataNodeKind::Database => {
                let database = required_database(profile.database.as_deref(), path)?;
                let tables = connector
                    .list_tables(profile, password, Some(database))
                    .await?;
                Ok(object_nodes(
                    profile.id,
                    path,
                    tables,
                    MetadataNodeKind::Table,
                ))
            }
            _ => Ok(Vec::new()),
        }
    }

    fn relational_reader(
        &self,
        kind: DataSourceKind,
    ) -> Result<&Arc<dyn RelationalMetadataReader>> {
        self.registry.relational_metadata(kind)
    }

    fn redis_reader(&self, kind: DataSourceKind) -> Result<&Arc<dyn RedisMetadataReader>> {
        self.registry.redis_metadata(kind)
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
            MetadataNode::new(
                inherit_parent(connection_id, parent)
                    .child(MetadataNodeKind::Schema)
                    .with_schema(object.name.clone()),
                object.name,
                MetadataNodeKind::Schema,
            )
        })
        .collect()
}

async fn append_schema_objects(
    reader: &Arc<dyn RelationalMetadataReader>,
    profile: &db_common::ConnectionProfile,
    password: Option<&str>,
    parent: &MetadataPath,
    schema: Option<&str>,
    show_schema_prefix: bool,
    nodes: &mut Vec<MetadataNode>,
) -> Result<()> {
    let tables = reader
        .list_tables(profile, password, parent.database.as_deref(), schema)
        .await?;
    nodes.extend(direct_object_nodes(
        profile.id,
        parent,
        tables,
        MetadataNodeKind::Table,
        schema,
        show_schema_prefix,
    ));

    let views = reader
        .list_views(profile, password, parent.database.as_deref(), schema)
        .await?;
    if !views.is_empty() {
        nodes.push(folder_node(
            profile.id,
            parent,
            MetadataNodeKind::ViewsFolder,
            "Views",
        ));
    }

    Ok(())
}

fn direct_object_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    objects: Vec<DatabaseObject>,
    kind: MetadataNodeKind,
    schema: Option<&str>,
    show_schema_prefix: bool,
) -> Vec<MetadataNode> {
    objects
        .into_iter()
        .map(|object| {
            let object_schema = object
                .schema
                .or_else(|| schema.map(ToOwned::to_owned))
                .filter(|value| !value.is_empty());
            let label = if show_schema_prefix {
                object_schema
                    .as_ref()
                    .map(|schema| format!("{schema}.{}", object.name))
                    .unwrap_or_else(|| object.name.clone())
            } else {
                object.name.clone()
            };
            let mut path = inherit_parent(connection_id, parent)
                .child(kind)
                .with_object(object.name);
            if let Some(schema) = object_schema {
                path = path.with_schema(schema);
            }
            MetadataNode::new(path, label, kind).leaf(true)
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
            let leaf = matches!(
                kind,
                MetadataNodeKind::Table | MetadataNodeKind::View | MetadataNodeKind::Function
            );
            MetadataNode::new(path, object.name, kind).leaf(leaf)
        })
        .collect()
}

fn node_sort_group(node: &MetadataNode) -> u8 {
    match node.kind {
        MetadataNodeKind::Table => 0,
        MetadataNodeKind::View => 1,
        MetadataNodeKind::QueriesFolder | MetadataNodeKind::Query => 2,
        _ => 9,
    }
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

fn foreign_key_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    fks: Vec<TableForeignKey>,
) -> Vec<MetadataNode> {
    fks.into_iter()
        .map(|fk| {
            let detail = format!("→ {}", fk.referenced_table);
            MetadataNode::new(
                inherit_parent(connection_id, parent)
                    .child(MetadataNodeKind::ForeignKey)
                    .with_extra(fk.name.clone()),
                fk.name,
                MetadataNodeKind::ForeignKey,
            )
            .leaf(true)
            .detail(detail)
        })
        .collect()
}

fn trigger_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    triggers: Vec<TableTrigger>,
) -> Vec<MetadataNode> {
    triggers
        .into_iter()
        .map(|trigger| {
            let detail = format!("{} {}", trigger.timing, trigger.event);
            MetadataNode::new(
                inherit_parent(connection_id, parent)
                    .child(MetadataNodeKind::Trigger)
                    .with_extra(trigger.name.clone()),
                trigger.name,
                MetadataNodeKind::Trigger,
            )
            .leaf(true)
            .detail(detail)
        })
        .collect()
}

fn function_nodes(
    connection_id: ConnectionId,
    parent: &MetadataPath,
    functions: Vec<DatabaseFunction>,
) -> Vec<MetadataNode> {
    functions
        .into_iter()
        .map(|func| {
            let detail = func
                .return_type
                .clone()
                .unwrap_or_else(|| func.kind.clone());
            MetadataNode::new(
                inherit_parent(connection_id, parent)
                    .child(MetadataNodeKind::Function)
                    .with_extra(func.name.clone()),
                func.name,
                MetadataNodeKind::Function,
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

fn required_database<'a>(
    profile_database: Option<&'a str>,
    path: &'a MetadataPath,
) -> Result<&'a str> {
    path.database
        .as_deref()
        .or(profile_database)
        .filter(|database| !database.is_empty())
        .ok_or_else(|| AppError::new(ErrorKind::Validation, "metadata database is required"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connection::ConnectionManager, storage::MemoryConnectionProfileStore};
    use async_trait::async_trait;
    use db_common::{ConnectionProfile, CredentialRef, DataSourceKind, PasswordUpdate};
    use secure_store::SecureStore;
    use std::{collections::BTreeMap, sync::Mutex};

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

        let service = MetadataService::new(manager, Vec::new(), Vec::new(), TaskManager::default());
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
