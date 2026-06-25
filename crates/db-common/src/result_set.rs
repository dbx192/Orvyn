use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogicalType {
    Null,
    Bool,
    Int,
    UInt,
    Float,
    Decimal,
    Text,
    Bytes,
    Date,
    Time,
    DateTime,
    Json,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CellValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    Date(NaiveDate),
    Time(NaiveTime),
    DateTime(NaiveDateTime),
    Timestamp(DateTime<Utc>),
    Json(serde_json::Value),
}

impl CellValue {
    pub const fn logical_type(&self) -> LogicalType {
        match self {
            Self::Null => LogicalType::Null,
            Self::Bool(_) => LogicalType::Bool,
            Self::Int(_) => LogicalType::Int,
            Self::UInt(_) => LogicalType::UInt,
            Self::Float(_) => LogicalType::Float,
            Self::Decimal(_) => LogicalType::Decimal,
            Self::Text(_) => LogicalType::Text,
            Self::Bytes(_) => LogicalType::Bytes,
            Self::Date(_) => LogicalType::Date,
            Self::Time(_) => LogicalType::Time,
            Self::DateTime(_) | Self::Timestamp(_) => LogicalType::DateTime,
            Self::Json(_) => LogicalType::Json,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Null => "NULL".to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Int(i) => i.to_string(),
            Self::UInt(u) => u.to_string(),
            Self::Float(f) => f.to_string(),
            Self::Decimal(s) => s.clone(),
            Self::Text(s) => s.clone(),
            Self::Bytes(b) => format!("<{} bytes>", b.len()),
            Self::Date(d) => d.to_string(),
            Self::Time(t) => t.to_string(),
            Self::DateTime(dt) => dt.to_string(),
            Self::Timestamp(ts) => ts.to_string(),
            Self::Json(j) => j.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub database_type: String,
    pub logical_type: LogicalType,
    pub nullable: bool,
    pub primary_key: bool,
}

impl Column {
    pub fn new(name: impl Into<String>, database_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            database_type: database_type.into(),
            logical_type: LogicalType::Unknown,
            nullable: true,
            primary_key: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub values: Vec<CellValue>,
}

impl Row {
    pub fn new(values: Vec<CellValue>) -> Self {
        Self { values }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub statement: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    pub affected_rows: u64,
    pub execution_time_ms: u128,
    pub notices: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

impl QueryResult {
    pub fn empty(statement: impl Into<String>) -> Self {
        Self {
            statement: statement.into(),
            columns: Vec::new(),
            rows: Vec::new(),
            affected_rows: 0,
            execution_time_ms: 0,
            notices: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.execution_time_ms = elapsed.as_millis();
        self
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}
