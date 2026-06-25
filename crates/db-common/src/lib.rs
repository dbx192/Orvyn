//! Shared primitives for the database client workspace.
//!
//! This crate intentionally contains only cross-cutting types and stateless
//! helpers. Concrete database drivers belong in their own adapter crates.

pub mod bytes;
pub mod connection;
pub mod dialect;
pub mod error;
pub mod id;
pub mod paging;
pub mod pool;
pub mod query;
pub mod redaction;
pub mod result_set;
pub mod session;
pub mod sql;
pub mod time;
pub mod traits;
pub mod validation;

pub use bytes::{base64_preview, human_bytes};
pub use connection::{
    unsupported_kind, ConnectionProfile, ConnectionProfileBuilder, ConnectionSecurity,
    ConnectionStatus, ConnectionTestReport, CredentialRef, DataSourceKind, PasswordUpdate, SslMode,
};
pub use dialect::{Dialect, Identifier};
pub use error::{AppError, ErrorKind, Result};
pub use id::{ConnectionId, ObjectId, QueryId, SessionId, TaskId};
pub use paging::{PageRequest, PageSize};
pub use pool::{ConnectionPoolConfig, ConnectionPoolStats};
pub use query::{
    FavoriteQuery, QueryExecutionMode, QueryExecutionReport, QueryExecutionRequest,
    QueryHistoryEntry, QueryTab, SqlHighlightToken, SqlTokenKind, TextRange,
    DEFAULT_QUERY_TIMEOUT_MS,
};
pub use redaction::{mask_secret, redact_connection_url, SecretString};
pub use result_set::{CellValue, Column, LogicalType, QueryResult, Row};
pub use sql::{
    highlight_sql, is_result_set_statement, looks_like_unqualified_mutation, split_statements,
    statement_at_offset, statement_summary, trim_sql,
};
pub use time::{elapsed_ms, now_utc};
pub use session::{
    SessionInfo, SessionState, TaskInfo, TaskKind, TaskProgress, TaskStatus,
};
pub use traits::{
    ConnectionTester, DatabaseFunction, DatabaseObject, DatabaseObjectKind, MetadataNode,
    MetadataNodeKind, MetadataPath, RedisConnector, RedisDatabase, RedisKeyInfo, RedisKeyScan,
    RedisMetadataReader, RedisValue, RelationalConnector, RelationalMetadataReader,
    RelationalQueryExecutor, TableColumn, TableForeignKey, TableIndex, TableTrigger,
};
