use crate::{
    CellValue, ConnectionId, ConnectionPoolConfig, ConnectionPoolStats, ConnectionProfile,
    ConnectionTestReport, DataSourceKind, PageRequest, QueryResult, Result,
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

    async fn pool_stats(&self, connection_id: ConnectionId) -> Result<Option<ConnectionPoolStats>>;

    async fn close_pool(&self, connection_id: ConnectionId) -> Result<()>;
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
