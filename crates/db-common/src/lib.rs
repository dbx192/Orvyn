//! Shared primitives for the database client workspace.
//!
//! This crate intentionally contains only cross-cutting types and stateless
//! helpers. Concrete database drivers belong in their own adapter crates.

pub mod bytes;
pub mod capabilities;
pub mod connection;
pub mod dialect;
pub mod error;
pub mod id;
pub mod paging;
pub mod pool;
pub mod protocol_family;
pub mod query;
pub mod redaction;
pub mod result_set;
pub mod session;
pub mod sql;
pub mod time;
pub mod traits;
pub mod validation;

pub use bytes::{base64_preview, human_bytes};
pub use capabilities::{ConnectorCapabilities, ExtraParamField, ExtraParamKind, SupportLevel};
pub use connection::{
    unsupported_kind, ConnectionProfile, ConnectionProfileBuilder, ConnectionSecurity,
    ConnectionStatus, ConnectionTestReport, CredentialRef, DataSourceKind, PasswordUpdate, SslMode,
};
pub use dialect::{Dialect, Identifier};
pub use error::{AppError, ErrorKind, Result};
pub use id::{ConnectionId, ObjectId, QueryId, SessionId, TaskId};
pub use paging::{PageRequest, PageSize};
pub use pool::{ConnectionPoolConfig, ConnectionPoolStats};
pub use protocol_family::ProtocolFamily;
pub use query::{
    FavoriteQuery, QueryExecutionMode, QueryExecutionReport, QueryExecutionRequest,
    QueryHistoryEntry, QueryTab, SqlHighlightToken, SqlTokenKind, TextRange,
    DEFAULT_QUERY_RESULT_LIMIT, DEFAULT_QUERY_TIMEOUT_MS,
};
pub use redaction::{mask_secret, redact_connection_url, SecretString};
pub use result_set::{
    CellValue, Column, LogicalType, QueryResult, Row, QUERY_RESULT_METADATA_PAGE_LIMIT,
    QUERY_RESULT_METADATA_PAGE_OFFSET, QUERY_RESULT_METADATA_TOTAL_ROWS,
};
pub use session::{SessionInfo, SessionState, TaskInfo, TaskKind, TaskProgress, TaskStatus};
pub use sql::{
    count_result_page_statement, highlight_sql, is_result_set_statement,
    limit_result_preview_statement, looks_like_unqualified_mutation, page_result_statement,
    split_statements, statement_at_offset, statement_summary, trim_sql,
};
pub use time::{elapsed_ms, now_utc};
pub use traits::{
    ConnectionTester, DatabaseFunction, DatabaseObject, DatabaseObjectKind, DocumentCollection,
    DocumentConnector, DocumentDatabase, HttpSqlConnector, MetadataNode, MetadataNodeKind,
    MetadataPath, RedisConnector, RedisDatabase, RedisKeyInfo, RedisKeyScan, RedisMetadataReader,
    RedisValue, RelationalConnector, RelationalMetadataReader, RelationalQueryExecutor,
    SearchConnector, SearchIndex, SearchResult, TableColumn, TableForeignKey, TableIndex,
    TableTrigger,
};
