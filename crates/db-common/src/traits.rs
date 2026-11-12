use crate::{
    CellValue, ConnectionId, ConnectionPoolConfig, ConnectionPoolStats, ConnectionProfile,
    ConnectionTestReport, DataSourceKind, PageRequest, QueryId, QueryResult, Result,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait ConnectionTester: Send + Sync {
    fn kind(&self) -> DataSourceKind;

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<ConnectionTestReport>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DatabaseObjectKind {
    Database,
    Schema,
    Table,
    View,
    Column,
    Index,
    ForeignKey,
    Function,
    Trigger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseObject {
    pub kind: DatabaseObjectKind,
    pub name: String,
    pub schema: Option<String>,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableColumn {
    pub name: String,
    pub database_type: String,
    pub nullable: bool,
    pub default_value: Option<String>,
    pub primary_key: bool,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableIndex {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
    pub on_update: Option<String>,
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableTrigger {
    pub name: String,
    pub event: String,
    pub timing: String,
    pub statement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseFunction {
    pub name: String,
    pub return_type: Option<String>,
    pub language: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub definition: Option<String>,
    #[serde(default)]
    pub identity_arguments: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MetadataNodeKind {
    Connection,
    Database,
    Schema,
    TablesFolder,
    ViewsFolder,
    FunctionsFolder,
    QueriesFolder,
    ColumnsFolder,
    IndexesFolder,
    ForeignKeysFolder,
    TriggersFolder,
    Table,
    View,
    Column,
    Index,
    ForeignKey,
    Trigger,
    Function,
    Query,
    RedisDatabase,
    RedisKeysFolder,
    RedisKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataPath {
    pub connection_id: ConnectionId,
    pub kind: MetadataNodeKind,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub object: Option<String>,
    pub extra: Option<String>,
}

impl MetadataPath {
    pub fn connection(connection_id: ConnectionId) -> Self {
        Self {
            connection_id,
            kind: MetadataNodeKind::Connection,
            database: None,
            schema: None,
            object: None,
            extra: None,
        }
    }

    pub fn child(mut self, kind: MetadataNodeKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    pub fn with_object(mut self, object: impl Into<String>) -> Self {
        self.object = Some(object.into());
        self
    }

    pub fn with_extra(mut self, extra: impl Into<String>) -> Self {
        self.extra = Some(extra.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataNode {
    pub path: MetadataPath,
    pub label: String,
    pub kind: MetadataNodeKind,
    pub leaf: bool,
    pub detail: Option<String>,
}

impl MetadataNode {
    pub fn new(path: MetadataPath, label: impl Into<String>, kind: MetadataNodeKind) -> Self {
        Self {
            path,
            label: label.into(),
            kind,
            leaf: false,
            detail: None,
        }
    }

    pub fn leaf(mut self, leaf: bool) -> Self {
        self.leaf = leaf;
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[async_trait]
pub trait RelationalMetadataReader: Send + Sync {
    fn kind(&self) -> DataSourceKind;

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>>;

    async fn list_schemas(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>>;

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>>;

    async fn list_views(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>>;

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>>;

    async fn list_indexes(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableIndex>>;

    async fn list_foreign_keys(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableForeignKey>> {
        let _ = (profile, password, database, schema, table);
        Ok(Vec::new())
    }

    async fn list_triggers(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableTrigger>> {
        let _ = (profile, password, database, schema, table);
        Ok(Vec::new())
    }

    async fn list_functions(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseFunction>> {
        let _ = (profile, password, database, schema);
        Ok(Vec::new())
    }

    async fn get_create_table_sql(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<String> {
        let _ = (profile, password, database, schema, table);
        Ok(String::new())
    }
}

#[async_trait]
pub trait RelationalQueryExecutor: Send + Sync {
    fn kind(&self) -> DataSourceKind;

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        pool_config: ConnectionPoolConfig,
    ) -> Result<Vec<QueryResult>>;

    async fn execute_cancellable(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        statements: &[String],
        config: ConnectionPoolConfig,
        cancellation: crate::QueryCancellation,
    ) -> Result<Vec<QueryResult>> {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(crate::AppError::new(crate::ErrorKind::Cancelled,
                "Client request cancelled; this driver cannot confirm server termination")),
            result = self.execute(profile, password, statements, config) => result,
        }
    }
    async fn execute_in_transaction_cancellable(
        &self,
        id: QueryId,
        statements: &[String],
        cancellation: crate::QueryCancellation,
    ) -> Result<Vec<QueryResult>> {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(crate::AppError::new(crate::ErrorKind::Cancelled,
                "Client request cancelled; transaction state must be checked before reuse")),
            result = self.execute_in_transaction(id, statements) => result,
        }
    }

    async fn pool_stats(&self, connection_id: ConnectionId) -> Result<Option<ConnectionPoolStats>>;

    async fn close_pool(&self, connection_id: ConnectionId) -> Result<()>;

    async fn begin_transaction(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        transaction_id: QueryId,
        pool_config: ConnectionPoolConfig,
    ) -> Result<()>;

    async fn execute_in_transaction(
        &self,
        transaction_id: QueryId,
        statements: &[String],
    ) -> Result<Vec<QueryResult>>;

    async fn commit_transaction(&self, transaction_id: QueryId) -> Result<()>;

    async fn rollback_transaction(&self, transaction_id: QueryId) -> Result<()>;
}

#[async_trait]
pub trait RelationalConnector: Send + Sync {
    async fn test_connection(&self, profile: &ConnectionProfile) -> Result<()>;

    async fn list_databases(&self) -> Result<Vec<DatabaseObject>>;

    async fn list_schemas(&self, database: Option<&str>) -> Result<Vec<DatabaseObject>>;

    async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<DatabaseObject>>;

    async fn list_columns(&self, schema: Option<&str>, table: &str) -> Result<Vec<TableColumn>>;

    async fn list_indexes(&self, schema: Option<&str>, table: &str) -> Result<Vec<TableIndex>>;

    async fn get_create_sql(&self, object: &DatabaseObject) -> Result<String>;

    async fn execute(&self, sql: &str) -> Result<Vec<QueryResult>>;

    async fn query_page(&self, sql: &str, page: PageRequest) -> Result<QueryResult>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedisDatabase {
    pub index: u8,
    pub key_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedisKeyInfo {
    pub name: String,
    pub type_name: String,
    pub ttl_seconds: Option<i64>,
    pub size_hint: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedisKeyScan {
    pub next_cursor: u64,
    pub keys: Vec<RedisKeyInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RedisValue {
    String(Vec<u8>),
    Hash(Vec<(String, CellValue)>),
    List(Vec<CellValue>),
    Set(Vec<CellValue>),
    SortedSet(Vec<(CellValue, f64)>),
    Stream(Vec<serde_json::Value>),
    None,
}

#[async_trait]
pub trait RedisConnector: Send + Sync {
    async fn test_connection(&self, profile: &ConnectionProfile) -> Result<()>;

    async fn list_databases(&self) -> Result<Vec<RedisDatabase>>;

    async fn scan_keys(
        &self,
        database: u8,
        pattern: Option<&str>,
        cursor: u64,
        limit: u32,
    ) -> Result<(u64, Vec<RedisKeyInfo>)>;

    async fn get_value(&self, database: u8, key: &str) -> Result<RedisValue>;

    async fn set_value(
        &self,
        database: u8,
        key: &str,
        value: RedisValue,
        ttl_seconds: Option<u64>,
    ) -> Result<()>;

    async fn delete_key(&self, database: u8, key: &str) -> Result<()>;

    async fn execute_command(&self, database: u8, command: &str) -> Result<RedisValue>;
}

#[async_trait]
pub trait RedisMetadataReader: Send + Sync {
    fn kind(&self) -> DataSourceKind;

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<RedisDatabase>>;

    async fn scan_keys(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: u8,
        pattern: Option<&str>,
        cursor: u64,
        limit: u32,
    ) -> Result<RedisKeyScan>;
}

// ---- Document / search / HTTP-SQL connectors (Phases 4-5) ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentDatabase {
    pub name: String,
    pub size_on_disk: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentCollection {
    pub name: String,
    pub database: String,
    pub document_count: Option<u64>,
}

/// Document store connector (MongoDB). Implemented in Phase 4.
#[allow(clippy::too_many_arguments)]
#[async_trait]
pub trait DocumentConnector: Send + Sync {
    fn kind(&self) -> DataSourceKind;

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<()>;

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DocumentDatabase>>;

    async fn list_collections(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
    ) -> Result<Vec<DocumentCollection>>;

    async fn find_documents(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
        collection: &str,
        filter: Option<&serde_json::Value>,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>>;

    /// Extended document query: find, aggregate, insert, update or delete.
    /// Connectors validate the envelope and enforce read-only mutation policy.
    async fn execute_document_query(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
        collection: &str,
        query: &serde_json::Value,
        limit: u32,
        offset: u64,
    ) -> Result<serde_json::Value> {
        let _ = (
            profile, password, database, collection, query, limit, offset,
        );
        Err(crate::AppError::unsupported(
            "extended document queries are not supported",
        ))
    }

    async fn delete_document(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: &str,
        collection: &str,
        document_id: &serde_json::Value,
    ) -> Result<u64>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndex {
    pub name: String,
    pub document_count: Option<u64>,
    pub health: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub total: Option<u64>,
    pub documents: Vec<serde_json::Value>,
    /// Complete server response, retaining hit IDs/scores/sort keys,
    /// aggregations, timing, shard failures and total relation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_response: Option<serde_json::Value>,
}

/// Search-service connector (Elasticsearch / Easysearch / Meilisearch /
/// Manticore). Implemented in Phase 5.
#[async_trait]
pub trait SearchConnector: Send + Sync {
    fn kind(&self) -> DataSourceKind;

    async fn open_point_in_time(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
    ) -> Result<String> {
        let _ = (profile, password, index);
        Err(crate::AppError::unsupported(
            "point-in-time search is unavailable",
        ))
    }
    async fn close_point_in_time(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        id: &str,
    ) -> Result<()> {
        let _ = (profile, password, id);
        Err(crate::AppError::unsupported(
            "point-in-time search is unavailable",
        ))
    }

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<()>;

    async fn list_indices(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<SearchIndex>>;

    async fn get_mapping(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
    ) -> Result<serde_json::Value>;

    async fn search(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        index: &str,
        query: &serde_json::Value,
        limit: u32,
    ) -> Result<SearchResult>;
}

/// HTTP-based SQL connector (Cloudflare D1). Implemented in Phase 5.
#[async_trait]
pub trait HttpSqlConnector: Send + Sync {
    fn kind(&self) -> DataSourceKind;

    async fn test_connection(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<()>;

    async fn execute(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>>;

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>>;

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>>;

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>>;
}
