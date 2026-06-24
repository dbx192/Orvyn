use crate::{validation, AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Identifier(String);

impl Identifier {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validation::ensure_not_blank("identifier", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub trait Dialect: Send + Sync {
    fn quote_identifier(&self, identifier: &Identifier) -> String;

    fn limit_offset(&self, sql: &str, limit: u32, offset: u64) -> String {
        format!("{sql} LIMIT {limit} OFFSET {offset}")
    }

    fn placeholder(&self, index: usize) -> String;

    fn validate_identifier(&self, value: &str) -> Result<Identifier> {
        if value.contains('\0') {
            return Err(AppError::validation(
                "identifier contains invalid null byte",
            ));
        }

        Identifier::new(value)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DoubleQuoteDialect;

impl Dialect for DoubleQuoteDialect {
    fn quote_identifier(&self, identifier: &Identifier) -> String {
        format!("\"{}\"", identifier.as_str().replace('"', "\"\""))
    }

    fn placeholder(&self, index: usize) -> String {
        format!("${index}")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BacktickDialect;

impl Dialect for BacktickDialect {
    fn quote_identifier(&self, identifier: &Identifier) -> String {
        format!("`{}`", identifier.as_str().replace('`', "``"))
    }

    fn placeholder(&self, _index: usize) -> String {
        "?".to_string()
    }
}
