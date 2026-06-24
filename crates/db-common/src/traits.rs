use crate::{CellValue, ConnectionProfile, PageRequest, QueryResult, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
