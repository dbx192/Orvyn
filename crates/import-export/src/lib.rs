//! Import and export workflow primitives.

pub mod import_csv;

use db_common::{base64_preview, human_bytes, CellValue, QueryResult, Result};

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
