use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    Validation,
    Connection,
    Authentication,
    Permission,
    Timeout,
    Query,
    TypeConversion,
    Io,
    Serialization,
    Credential,
    Cancelled,
    Unsupported,
    Internal,
    NotFound,
}

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
#[error("{message}")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    pub detail: Option<String>,
    pub code: Option<String>,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: None,
            code: None,
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Validation, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// User-facing diagnostic that keeps server and driver context.
    pub fn diagnostic_message(&self) -> String {
        let mut lines = vec![self.message.clone()];
        if let Some(code) = self
            .code
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            lines.push(format!("Code: {code}"));
        }
        if let Some(detail) = self
            .detail
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            lines.push(detail.to_string());
        }
        lines.join("\n")
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorKind::Io, "I/O operation failed").with_detail(error.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(ErrorKind::Serialization, "serialization failed").with_detail(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_message_keeps_code_and_server_detail() {
        let error = AppError::new(ErrorKind::Query, "Query failed")
            .with_code("42601")
            .with_detail("syntax error at character 18");

        assert_eq!(
            error.diagnostic_message(),
            "Query failed\nCode: 42601\nsyntax error at character 18"
        );
    }
}
