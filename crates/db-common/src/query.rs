use crate::{ConnectionId, QueryId, QueryResult};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_QUERY_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_QUERY_RESULT_LIMIT: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

impl TextRange {
    pub fn new(start: usize, end: usize) -> Self {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        Self { start, end }
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Semantic class for a SQL editor highlight span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SqlTokenKind {
    Keyword,
    Identifier,
    QuotedIdentifier,
    StringLiteral,
    Number,
    Operator,
    Comment,
    Parameter,
    Punctuation,
    Function,
}

/// Byte range and class for a SQL editor highlight span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlHighlightToken {
    pub range: TextRange,
    pub kind: SqlTokenKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryTab {
    pub id: QueryId,
    pub connection_id: ConnectionId,
    pub title: String,
    pub sql: String,
    pub cursor_offset: usize,
    pub selection: Option<TextRange>,
    pub dirty: bool,
}

impl QueryTab {
    pub fn new(connection_id: ConnectionId, title: impl Into<String>) -> Self {
        Self {
            id: QueryId::new(),
            connection_id,
            title: title.into(),
            sql: String::new(),
            cursor_offset: 0,
            selection: None,
            dirty: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryExecutionMode {
    All,
    CurrentStatement,
    Selection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryExecutionRequest {
    pub tab_id: QueryId,
    pub connection_id: ConnectionId,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    pub sql: String,
    pub mode: QueryExecutionMode,
    pub cursor_offset: usize,
    pub selection: Option<TextRange>,
    pub timeout_ms: u64,
    #[serde(default)]
    pub result_limit: Option<u32>,
    #[serde(default)]
    pub result_offset: u64,
    #[serde(default)]
    pub result_total_rows: Option<u64>,
}

impl QueryExecutionRequest {
    pub fn all(tab_id: QueryId, connection_id: ConnectionId, sql: impl Into<String>) -> Self {
        Self {
            tab_id,
            connection_id,
            database: None,
            schema: None,
            sql: sql.into(),
            mode: QueryExecutionMode::All,
            cursor_offset: 0,
            selection: None,
            timeout_ms: DEFAULT_QUERY_TIMEOUT_MS,
            result_limit: None,
            result_offset: 0,
            result_total_rows: None,
        }
    }

    pub fn timeout(self) -> Duration {
        Duration::from_millis(self.timeout_ms.max(1))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryExecutionReport {
    pub execution_id: QueryId,
    pub tab_id: QueryId,
    pub connection_id: ConnectionId,
    pub statements: Vec<String>,
    pub results: Vec<QueryResult>,
    pub elapsed_ms: u128,
    pub affected_rows: u64,
}

impl QueryExecutionReport {
    pub fn new(
        execution_id: QueryId,
        request: &QueryExecutionRequest,
        statements: Vec<String>,
        results: Vec<QueryResult>,
        elapsed_ms: u128,
    ) -> Self {
        let affected_rows = results.iter().map(|result| result.affected_rows).sum();
        Self {
            execution_id,
            tab_id: request.tab_id,
            connection_id: request.connection_id,
            statements,
            results,
            elapsed_ms,
            affected_rows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryHistoryEntry {
    pub id: QueryId,
    pub connection_id: ConnectionId,
    pub sql: String,
    pub summary: String,
    pub executed_at: chrono::DateTime<chrono::Utc>,
    pub elapsed_ms: u128,
    pub affected_rows: u64,
    pub success: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoriteQuery {
    pub id: QueryId,
    pub connection_id: Option<ConnectionId>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    pub title: String,
    pub sql: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
