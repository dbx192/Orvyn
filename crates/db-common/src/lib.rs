//! Shared primitives for the database client workspace.
//!
//! This crate intentionally contains only cross-cutting types and stateless
//! helpers. Concrete database drivers belong in their own adapter crates.

pub mod connection;
pub mod bytes;
pub mod dialect;
pub mod error;
pub mod id;
pub mod paging;
pub mod redaction;
pub mod result_set;
pub mod sql;
pub mod time;
pub mod traits;
pub mod validation;

pub use bytes::{base64_preview, human_bytes};
pub use connection::{
    ConnectionProfile, ConnectionProfileBuilder, ConnectionSecurity, ConnectionStatus,
    ConnectionTestReport, CredentialRef, DataSourceKind, PasswordUpdate, SslMode,
};
pub use dialect::{Dialect, Identifier};
pub use error::{AppError, ErrorKind, Result};
pub use id::{ConnectionId, ObjectId, QueryId, TaskId};
pub use paging::{PageRequest, PageSize};
pub use redaction::{mask_secret, redact_connection_url, SecretString};
pub use result_set::{CellValue, Column, LogicalType, QueryResult, Row};
pub use sql::{looks_like_unqualified_mutation, statement_summary, trim_sql};
pub use time::{elapsed_ms, now_utc};
pub use traits::{
    ConnectionTester, DatabaseObject, DatabaseObjectKind, RedisConnector, RedisDatabase,
    RedisKeyInfo, RedisValue, RelationalConnector, TableColumn, TableIndex,
};
