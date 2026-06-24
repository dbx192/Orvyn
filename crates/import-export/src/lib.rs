//! Import and export workflow primitives.

use db_common::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
}

pub fn validate_export_file_name(file_name: &str) -> Result<()> {
    db_common::validation::ensure_not_blank("file_name", file_name)
}
