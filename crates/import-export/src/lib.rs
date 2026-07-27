//! Import and export workflow primitives.

pub mod import_csv;

use db_common::{base64_preview, human_bytes, CellValue, QueryResult, Result};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

pub struct QueryResultFileWriter {
    output: ExportWriter,
    columns: Vec<String>,
    rows_written: usize,
}

enum ExportWriter {
    Delimited(Box<csv::Writer<File>>),
    Json(BufWriter<File>),
}

impl QueryResultFileWriter {
    pub fn create(format: ExportFormat, path: &Path, columns: Vec<String>) -> Result<Self> {
        let output = match format {
            ExportFormat::Csv | ExportFormat::ExcelCompatible => {
                let delimiter = if format == ExportFormat::Csv {
                    b','
                } else {
                    b'\t'
                };
                let mut file = File::create(path).map_err(db_common::AppError::from)?;
                // Excel and many Windows tools otherwise interpret an UTF-8 CSV using the
                // current ANSI code page, which corrupts Chinese text.
                if format == ExportFormat::Csv {
                    file.write_all(b"\xEF\xBB\xBF")
                        .map_err(db_common::AppError::from)?;
                }
                let mut writer = csv::WriterBuilder::new()
                    .delimiter(delimiter)
                    .from_writer(file);
                writer
                    .write_record(columns.iter().map(String::as_str))
                    .map_err(|error| db_common::AppError::internal(error.to_string()))?;
                ExportWriter::Delimited(Box::new(writer))
            }
            ExportFormat::Json => {
                let file = File::create(path).map_err(db_common::AppError::from)?;
                let mut writer = BufWriter::new(file);
                writer
                    .write_all(b"[\n")
                    .map_err(db_common::AppError::from)?;
                ExportWriter::Json(writer)
            }
        };
        Ok(Self {
            output,
            columns,
            rows_written: 0,
        })
    }

    pub fn write_page(&mut self, result: &QueryResult) -> Result<usize> {
        let page_rows = result.rows.len();
        match &mut self.output {
            ExportWriter::Delimited(writer) => {
                for row in &result.rows {
                    let record = (0..self.columns.len())
                        .map(|index| {
                            row.values
                                .get(index)
                                .map(export_cell_value)
                                .unwrap_or_else(|| "NULL".to_string())
                        })
                        .collect::<Vec<_>>();
                    writer
                        .write_record(&record)
                        .map_err(|error| db_common::AppError::internal(error.to_string()))?;
                }
            }
            ExportWriter::Json(writer) => {
                for row in &result.rows {
                    if self.rows_written > 0 {
                        writer
                            .write_all(b",\n")
                            .map_err(db_common::AppError::from)?;
                    }
                    let object = self
                        .columns
                        .iter()
                        .enumerate()
                        .map(|(index, column)| {
                            let value = row.values.get(index).unwrap_or(&CellValue::Null);
                            (column.clone(), json_cell_value(value))
                        })
                        .collect::<serde_json::Map<_, _>>();
                    serde_json::to_writer(&mut *writer, &serde_json::Value::Object(object))
                        .map_err(db_common::AppError::from)?;
                    self.rows_written += 1;
                }
                return Ok(page_rows);
            }
        }
        self.rows_written += page_rows;
        Ok(page_rows)
    }

    pub fn finish(mut self) -> Result<usize> {
        match &mut self.output {
            ExportWriter::Delimited(writer) => writer.flush().map_err(db_common::AppError::from)?,
            ExportWriter::Json(writer) => {
                writer
                    .write_all(b"\n]\n")
                    .map_err(db_common::AppError::from)?;
                writer.flush().map_err(db_common::AppError::from)?;
            }
        }
        Ok(self.rows_written)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
    ExcelCompatible,
}

pub fn validate_export_file_name(file_name: &str) -> Result<()> {
    db_common::validation::ensure_not_blank("file_name", file_name)
}

pub fn export_query_result(result: &QueryResult, format: ExportFormat) -> Result<String> {
    match format {
        ExportFormat::Csv => Ok(export_delimited(result, ',')),
        ExportFormat::Json => export_json(result),
        ExportFormat::ExcelCompatible => Ok(export_delimited(result, '\t')),
    }
}

pub fn export_query_result_to_file(
    result: &QueryResult,
    format: ExportFormat,
    path: &Path,
) -> Result<()> {
    let columns = result
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect();
    let mut writer = QueryResultFileWriter::create(format, path, columns)?;
    writer.write_page(result)?;
    writer.finish()?;
    Ok(())
}

fn export_delimited(result: &QueryResult, delimiter: char) -> String {
    let mut output = String::new();
    let headers = result
        .columns
        .iter()
        .map(|column| escape_delimited(&column.name, delimiter))
        .collect::<Vec<_>>();
    output.push_str(&headers.join(&delimiter.to_string()));
    output.push('\n');

    for row in &result.rows {
        let values = row
            .values
            .iter()
            .map(|value| escape_delimited(&export_cell_value(value), delimiter))
            .collect::<Vec<_>>();
        output.push_str(&values.join(&delimiter.to_string()));
        output.push('\n');
    }

    output
}

fn export_json(result: &QueryResult) -> Result<String> {
    let rows = result
        .rows
        .iter()
        .map(|row| {
            result
                .columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    let value = row
                        .values
                        .get(index)
                        .map(json_cell_value)
                        .unwrap_or(serde_json::Value::Null);
                    (column.name.clone(), value)
                })
                .collect::<serde_json::Map<String, serde_json::Value>>()
        })
        .map(serde_json::Value::Object)
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&rows).map_err(Into::into)
}

fn json_cell_value(value: &CellValue) -> serde_json::Value {
    match value {
        CellValue::Null => serde_json::Value::Null,
        CellValue::Bool(value) => serde_json::Value::Bool(*value),
        CellValue::Int(value) => serde_json::json!(value),
        CellValue::UInt(value) => serde_json::json!(value),
        CellValue::Float(value) => serde_json::json!(value),
        CellValue::Decimal(value) | CellValue::Text(value) => {
            serde_json::Value::String(value.clone())
        }
        CellValue::Bytes(value) => serde_json::json!({
            "type": "bytes",
            "length": value.len(),
            "base64_preview": base64_preview(value, 24),
        }),
        CellValue::Date(value) => serde_json::Value::String(value.format("%Y-%m-%d").to_string()),
        CellValue::Time(value) => {
            serde_json::Value::String(value.format("%H:%M:%S%.3f").to_string())
        }
        CellValue::DateTime(value) => {
            serde_json::Value::String(value.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        }
        CellValue::Timestamp(value) => serde_json::Value::String(value.to_rfc3339()),
        CellValue::Json(value) => value.clone(),
    }
}

fn export_cell_value(value: &CellValue) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(value) => value.to_string(),
        CellValue::Int(value) => value.to_string(),
        CellValue::UInt(value) => value.to_string(),
        CellValue::Float(value) => value.to_string(),
        CellValue::Decimal(value) | CellValue::Text(value) => value.clone(),
        CellValue::Bytes(value) => format!(
            "<binary: {}, base64 {}>",
            human_bytes(value.len() as u64),
            base64_preview(value, 24)
        ),
        CellValue::Date(value) => value.format("%Y-%m-%d").to_string(),
        CellValue::Time(value) => value.format("%H:%M:%S%.3f").to_string(),
        CellValue::DateTime(value) => value.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        CellValue::Timestamp(value) => value.to_rfc3339(),
        CellValue::Json(value) => value.to_string(),
    }
}

fn escape_delimited(value: &str, delimiter: char) -> String {
    if value.contains(delimiter)
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r')
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::{Column, LogicalType, Row};
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_test_file_path(extension: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "rust-db-client-export-{}-{}.{}",
            std::process::id(),
            nanos,
            extension
        ))
    }

    #[test]
    fn exports_csv_with_headers_and_null_marker() {
        let result = QueryResult {
            statement: "select 1".to_string(),
            columns: vec![
                typed_column("id", LogicalType::Int),
                typed_column("name", LogicalType::Text),
            ],
            rows: vec![Row::new(vec![
                CellValue::Int(1),
                CellValue::Text("a,b".to_string()),
            ])],
            affected_rows: 0,
            execution_time_ms: 1,
            notices: Vec::new(),
            metadata: Default::default(),
        };

        assert_eq!(
            export_query_result(&result, ExportFormat::Csv).unwrap(),
            "id,name\n1,\"a,b\"\n"
        );
    }

    #[test]
    fn exports_json_rows_by_column_name() {
        let result = QueryResult {
            statement: "select null".to_string(),
            columns: vec![typed_column("value", LogicalType::Null)],
            rows: vec![Row::new(vec![CellValue::Null])],
            affected_rows: 0,
            execution_time_ms: 1,
            notices: Vec::new(),
            metadata: Default::default(),
        };

        assert_eq!(
            export_query_result(&result, ExportFormat::Json).unwrap(),
            "[\n  {\n    \"value\": null\n  }\n]"
        );
    }

    #[test]
    fn exports_csv_file_with_headers() {
        let result = QueryResult {
            statement: "select 1".to_string(),
            columns: vec![
                typed_column("id", LogicalType::Int),
                typed_column("name", LogicalType::Text),
            ],
            rows: vec![Row::new(vec![
                CellValue::Int(1),
                CellValue::Text("中文姓名".to_string()),
            ])],
            affected_rows: 0,
            execution_time_ms: 1,
            notices: Vec::new(),
            metadata: Default::default(),
        };

        let path = temp_test_file_path("csv");
        export_query_result_to_file(&result, ExportFormat::Csv, &path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let preview = import_csv::parse_csv_preview(&path, &Default::default()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(content, "\u{feff}id,name\n1,中文姓名\n");
        assert_eq!(preview.encoding, "UTF-8 BOM");
        assert_eq!(preview.sample_rows[0][1], "中文姓名");
    }

    #[test]
    fn streams_multiple_pages_into_one_export_file() {
        let path = temp_test_file_path("csv");
        let mut writer = QueryResultFileWriter::create(
            ExportFormat::Csv,
            &path,
            vec!["id".to_string(), "name".to_string()],
        )
        .unwrap();
        let page = |id, name: &str| QueryResult {
            statement: "select id, name from users".to_string(),
            columns: Vec::new(),
            rows: vec![Row::new(vec![
                CellValue::Int(id),
                CellValue::Text(name.to_string()),
            ])],
            affected_rows: 0,
            execution_time_ms: 0,
            notices: Vec::new(),
            metadata: Default::default(),
        };

        writer.write_page(&page(1, "alice")).unwrap();
        writer.write_page(&page(2, "bob")).unwrap();
        assert_eq!(writer.finish().unwrap(), 2);

        let content = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(content, "\u{feff}id,name\n1,alice\n2,bob\n");
    }

    fn typed_column(name: &str, logical_type: LogicalType) -> Column {
        Column {
            name: name.to_string(),
            database_type: String::new(),
            logical_type,
            nullable: true,
            primary_key: false,
        }
    }
}
