//! CSV import: parse, preview, field mapping, type validation, and batch INSERT generation.

use db_common::{Dialect, Identifier, Result, TableColumn};
use encoding_rs::{GB18030, UTF_16BE, UTF_16LE};
use std::path::Path;

/// Preview of a CSV file after parsing the header and first N rows.
#[derive(Debug, Clone)]
pub struct CsvPreview {
    /// Column names from the first row (if has_header), else "Column_1", "Column_2", ...
    pub headers: Vec<String>,
    /// First N data rows (as raw strings).
    pub sample_rows: Vec<Vec<String>>,
    /// Estimated total row count (including header if present). May be approximate.
    pub estimated_total_rows: u64,
    /// Whether the first row was treated as a header.
    pub has_header: bool,
    /// Detected delimiter.
    pub delimiter: u8,
    /// Full file path.
    pub file_path: String,
    /// Character encoding used to decode the file.
    pub encoding: String,
}

/// Field mapping: which CSV column maps to which target table column.
#[derive(Debug, Clone)]
pub struct FieldMapping {
    /// For each target table column index, the corresponding CSV header index (if mapped).
    pub mappings: Vec<Option<usize>>,
}

/// A single type-validation issue.
#[derive(Debug, Clone)]
pub struct TypeIssue {
    pub row: usize,
    pub column: String,
    pub value: String,
    pub reason: String,
}

/// Result of type pre-validation.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub issues: Vec<TypeIssue>,
    pub total_rows_checked: usize,
}

/// Options for CSV import.
#[derive(Debug, Clone)]
pub struct CsvImportOptions {
    pub has_header: bool,
    pub delimiter: u8,
    pub batch_size: usize,
}

impl Default for CsvImportOptions {
    fn default() -> Self {
        Self {
            has_header: true,
            delimiter: b',',
            batch_size: 1000,
        }
    }
}

/// Parse a CSV file and return a preview with the first `sample_limit` data rows.
pub fn parse_csv_preview(path: &Path, options: &CsvImportOptions) -> Result<CsvPreview> {
    let bytes = std::fs::read(path)
        .map_err(|e| db_common::AppError::validation(format!("Failed to open CSV: {e}")))?;
    let (text, encoding) = decode_csv_text(&bytes)?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(options.delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());

    let mut all_records: Vec<Vec<String>> = Vec::new();
    for result in reader.records() {
        let record =
            result.map_err(|e| db_common::AppError::validation(format!("CSV parse error: {e}")))?;
        all_records.push(record.iter().map(|s| s.to_string()).collect());
    }

    if all_records.is_empty() {
        return Err(db_common::AppError::validation("CSV file is empty"));
    }

    let max_cols = all_records.iter().map(|r| r.len()).max().unwrap_or(0);

    let (headers, data_start) = if options.has_header {
        let hdrs = all_records[0].clone();
        // Pad headers to match max columns
        let mut padded = hdrs;
        while padded.len() < max_cols {
            padded.push(format!("Column_{}", padded.len() + 1));
        }
        (padded, 1usize)
    } else {
        let hdrs: Vec<String> = (1..=max_cols).map(|i| format!("Column_{i}")).collect();
        (hdrs, 0usize)
    };

    // Normalize all rows to have the same number of columns as headers
    let data_rows: Vec<Vec<String>> = all_records[data_start..]
        .iter()
        .map(|row| {
            let mut padded = row.clone();
            padded.resize(headers.len(), String::new());
            padded
        })
        .collect();

    let total = data_rows.len() as u64;

    Ok(CsvPreview {
        headers,
        sample_rows: data_rows,
        estimated_total_rows: total,
        has_header: options.has_header,
        delimiter: options.delimiter,
        file_path: path.display().to_string(),
        encoding: encoding.to_string(),
    })
}

fn decode_csv_text(bytes: &[u8]) -> Result<(String, &'static str)> {
    if let Some(content) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return std::str::from_utf8(content)
            .map(|text| (text.to_string(), "UTF-8 BOM"))
            .map_err(|error| {
                db_common::AppError::validation(format!(
                    "CSV has an invalid UTF-8 byte sequence: {error}"
                ))
            });
    }

    if let Some(content) = bytes.strip_prefix(b"\xFF\xFE") {
        return decode_legacy_csv(content, UTF_16LE, "UTF-16 LE");
    }
    if let Some(content) = bytes.strip_prefix(b"\xFE\xFF") {
        return decode_legacy_csv(content, UTF_16BE, "UTF-16 BE");
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok((text.to_string(), "UTF-8"));
    }

    decode_legacy_csv(bytes, GB18030, "GB18030/GBK")
}

fn decode_legacy_csv(
    bytes: &[u8],
    encoding: &'static encoding_rs::Encoding,
    label: &'static str,
) -> Result<(String, &'static str)> {
    let (text, had_errors) = encoding.decode_without_bom_handling(bytes);
    if had_errors {
        return Err(db_common::AppError::validation(
            "CSV is not valid UTF-8, UTF-16, or GB18030 near an invalid byte sequence",
        ));
    }
    Ok((text.into_owned(), label))
}

/// Auto-map CSV headers to table columns by case-insensitive name matching.
pub fn auto_map_fields(csv_headers: &[String], table_columns: &[TableColumn]) -> FieldMapping {
    let mappings: Vec<Option<usize>> = table_columns
        .iter()
        .map(|col| {
            csv_headers
                .iter()
                .position(|h| h.eq_ignore_ascii_case(&col.name))
        })
        .collect();
    FieldMapping { mappings }
}

/// Validate that CSV values can be reasonably inserted into the target columns.
///
/// An empty CSV field is an empty string, not SQL `NULL`.  This distinction is
/// important for `NOT NULL` character columns, where `''` is a valid value.  A
/// literal `NULL` is treated as SQL `NULL` and is rejected for non-nullable
/// columns.  Empty values in nullable columns are converted to SQL `NULL` when
/// the INSERT statement is generated (see [`generate_insert_batches`]).
pub fn validate_types(
    rows: &[Vec<String>],
    _headers: &[String],
    table_columns: &[TableColumn],
    mapping: &FieldMapping,
) -> ValidationReport {
    let mut issues = Vec::new();

    for (row_idx, row) in rows.iter().enumerate() {
        for (col_idx, col) in table_columns.iter().enumerate() {
            let csv_col = match mapping.mappings.get(col_idx).and_then(|&m| m) {
                Some(c) => c,
                None => continue, // unmapped column, skip validation
            };

            let value = row.get(csv_col).map(|s| s.as_str()).unwrap_or("");

            let trimmed = value.trim();
            let db_type = col.database_type.to_lowercase();

            // The exporter writes NULL as a literal marker.  Do not let it
            // reach the database as a quoted string, and report it clearly
            // when the destination column is NOT NULL.
            if is_null_marker(trimmed) {
                if !col.nullable {
                    issues.push(TypeIssue {
                        row: row_idx + 1,
                        column: col.name.clone(),
                        value: value.to_string(),
                        reason: "Column is NOT NULL but value is NULL".to_string(),
                    });
                }
                continue;
            }

            // Empty strings are valid for character columns even when they
            // are NOT NULL.  For non-character columns an empty field cannot
            // provide a value, so retain the useful validation error.
            if !col.nullable && trimmed.is_empty() && !is_character_type(&db_type) {
                issues.push(TypeIssue {
                    row: row_idx + 1,
                    column: col.name.clone(),
                    value: value.to_string(),
                    reason: "Column is NOT NULL but value is empty".to_string(),
                });
                continue;
            }

            // Skip further validation for empty nullable values
            if trimmed.is_empty() {
                continue;
            }

            // Basic type checks based on database_type hints
            if db_type.contains("int") || db_type.contains("bigint") || db_type.contains("smallint")
            {
                if value.trim().parse::<i64>().is_err() {
                    issues.push(TypeIssue {
                        row: row_idx + 1,
                        column: col.name.clone(),
                        value: value.to_string(),
                        reason: format!("Expected integer for type '{}'", col.database_type),
                    });
                }
            } else if db_type.contains("float")
                || db_type.contains("double")
                || db_type.contains("decimal")
                || db_type.contains("numeric")
            {
                if value.trim().parse::<f64>().is_err() {
                    issues.push(TypeIssue {
                        row: row_idx + 1,
                        column: col.name.clone(),
                        value: value.to_string(),
                        reason: format!("Expected number for type '{}'", col.database_type),
                    });
                }
            } else if db_type.contains("bool") {
                let lower = value.trim().to_lowercase();
                if lower != "true"
                    && lower != "false"
                    && lower != "1"
                    && lower != "0"
                    && lower != "t"
                    && lower != "f"
                {
                    issues.push(TypeIssue {
                        row: row_idx + 1,
                        column: col.name.clone(),
                        value: value.to_string(),
                        reason: format!("Expected boolean for type '{}'", col.database_type),
                    });
                }
            }
        }
    }

    ValidationReport {
        issues,
        total_rows_checked: rows.len(),
    }
}

/// Generate batch INSERT SQL statements for the given rows and field mapping.
pub fn generate_insert_batches(
    rows: &[Vec<String>],
    _headers: &[String],
    table_columns: &[TableColumn],
    mapping: &FieldMapping,
    qualified_table: &str,
    dialect: &dyn Dialect,
    batch_size: usize,
) -> Result<Vec<String>> {
    let mut batches = Vec::new();
    let mapped_columns: Vec<(usize, &TableColumn)> = mapping
        .mappings
        .iter()
        .enumerate()
        .filter_map(|(col_idx, csv_idx)| csv_idx.map(|c| (c, &table_columns[col_idx])))
        .collect();

    if mapped_columns.is_empty() {
        return Err(db_common::AppError::validation(
            "No columns mapped for import",
        ));
    }

    let col_names: Vec<String> = mapped_columns
        .iter()
        .map(|(_, col)| {
            let ident = Identifier::new(col.name.clone())?;
            Ok(dialect.quote_identifier(&ident))
        })
        .collect::<Result<Vec<_>>>()?;

    for chunk in rows.chunks(batch_size) {
        let mut values_clauses = Vec::new();
        for row in chunk {
            let vals: Vec<String> = mapped_columns
                .iter()
                .map(|(csv_idx, col)| {
                    let raw = row.get(*csv_idx).map(|s| s.as_str()).unwrap_or("");
                    if raw.trim().is_empty() && col.nullable {
                        "NULL".to_string()
                    } else if raw.trim().is_empty() && !col.nullable {
                        // Use empty string as fallback for non-nullable columns
                        "''".to_string()
                    } else {
                        sql_literal_for_column(raw, col)
                    }
                })
                .collect();
            values_clauses.push(format!("({})", vals.join(", ")));
        }
        batches.push(format!(
            "INSERT INTO {} ({}) VALUES\n{};",
            qualified_table,
            col_names.join(", "),
            values_clauses.join(",\n")
        ));
    }

    Ok(batches)
}

/// Escape a value for use as a SQL string literal.
fn sql_literal(value: &str) -> String {
    if value.eq_ignore_ascii_case("NULL") {
        "NULL".to_string()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

fn sql_literal_for_column(value: &str, column: &TableColumn) -> String {
    let trimmed = value.trim();
    let db_type = column.database_type.to_ascii_lowercase();

    if trimmed.eq_ignore_ascii_case("NULL") {
        return "NULL".to_string();
    }

    if is_integer_type(&db_type) && trimmed.parse::<i64>().is_ok() {
        return trimmed.to_string();
    }

    if is_numeric_type(&db_type) && trimmed.parse::<f64>().is_ok() {
        return trimmed.to_string();
    }

    if is_boolean_type(&db_type) {
        match trimmed.to_ascii_lowercase().as_str() {
            "true" | "false" => return trimmed.to_ascii_lowercase(),
            "1" | "t" => return "TRUE".to_string(),
            "0" | "f" => return "FALSE".to_string(),
            _ => {}
        }
    }

    sql_literal(value)
}

fn is_null_marker(value: &str) -> bool {
    value.eq_ignore_ascii_case("NULL")
}

fn is_character_type(db_type: &str) -> bool {
    db_type.contains("char")
        || db_type.contains("text")
        || db_type.contains("clob")
        || db_type.contains("string")
}

fn is_integer_type(db_type: &str) -> bool {
    db_type.contains("bigint")
        || db_type.contains("smallint")
        || db_type.contains("tinyint")
        || (db_type.contains("int") && !db_type.contains("point"))
}

fn is_numeric_type(db_type: &str) -> bool {
    is_integer_type(db_type)
        || db_type.contains("float")
        || db_type.contains("double")
        || db_type.contains("decimal")
        || db_type.contains("numeric")
        || db_type.contains("real")
}

fn is_boolean_type(db_type: &str) -> bool {
    db_type.contains("bool")
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::dialect::DoubleQuoteDialect;
    use std::{
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn make_csv_bytes(content: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!("test_import_{}-{}.csv", std::process::id(), nanos));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content).unwrap();
        path
    }

    fn make_csv(content: &str) -> std::path::PathBuf {
        make_csv_bytes(content.as_bytes())
    }

    #[test]
    fn parses_csv_with_header() {
        let path = make_csv("name,age\nAlice,30\nBob,25\n");
        let opts = CsvImportOptions::default();
        let preview = parse_csv_preview(&path, &opts).unwrap();
        assert_eq!(preview.headers, vec!["name", "age"]);
        assert_eq!(preview.sample_rows.len(), 2);
        assert_eq!(preview.sample_rows[0][0], "Alice");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_gb18030_chinese_csv() {
        let (encoded, _, had_errors) = GB18030.encode("姓名,城市\n张三,上海\n");
        assert!(!had_errors);
        let path = make_csv_bytes(encoded.as_ref());
        let preview = parse_csv_preview(&path, &Default::default()).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(preview.encoding, "GB18030/GBK");
        assert_eq!(preview.headers, vec!["姓名", "城市"]);
        assert_eq!(preview.sample_rows[0], vec!["张三", "上海"]);
    }

    #[test]
    fn strips_utf8_bom_from_first_header() {
        let path = make_csv_bytes(b"\xEF\xBB\xBFname,city\nAlice,Shanghai\n");
        let preview = parse_csv_preview(&path, &Default::default()).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(preview.encoding, "UTF-8 BOM");
        assert_eq!(preview.headers[0], "name");
    }

    #[test]
    fn auto_maps_by_case_insensitive_name() {
        let headers = vec!["NAME".to_string(), "Age".to_string()];
        let columns = vec![
            TableColumn {
                name: "name".into(),
                database_type: "VARCHAR".into(),
                nullable: true,
                default_value: None,
                primary_key: false,
                comment: None,
            },
            TableColumn {
                name: "age".into(),
                database_type: "INT".into(),
                nullable: false,
                default_value: None,
                primary_key: false,
                comment: None,
            },
        ];
        let mapping = auto_map_fields(&headers, &columns);
        assert_eq!(mapping.mappings[0], Some(0));
        assert_eq!(mapping.mappings[1], Some(1));
    }

    #[test]
    fn validates_not_null_and_type() {
        let headers = vec!["name".to_string(), "age".to_string()];
        let columns = vec![
            TableColumn {
                name: "name".into(),
                database_type: "VARCHAR".into(),
                nullable: true,
                default_value: None,
                primary_key: false,
                comment: None,
            },
            TableColumn {
                name: "age".into(),
                database_type: "INT".into(),
                nullable: false,
                default_value: None,
                primary_key: false,
                comment: None,
            },
        ];
        let mapping = auto_map_fields(&headers, &columns);
        let rows = vec![
            vec!["Alice".to_string(), "".to_string()], // age is empty → NOT NULL error
            vec!["Bob".to_string(), "abc".to_string()], // age is not int → type error
        ];
        let report = validate_types(&rows, &headers, &columns, &mapping);
        assert_eq!(report.issues.len(), 2);
    }

    #[test]
    fn allows_empty_string_for_not_null_text_column() {
        let headers = vec!["name".to_string()];
        let columns = vec![TableColumn {
            name: "name".into(),
            database_type: "VARCHAR".into(),
            nullable: false,
            default_value: None,
            primary_key: false,
            comment: None,
        }];
        let mapping = auto_map_fields(&headers, &columns);
        let report = validate_types(&[vec![String::new()]], &headers, &columns, &mapping);
        assert!(report.issues.is_empty());

        let batches = generate_insert_batches(
            &[vec![String::new()]],
            &headers,
            &columns,
            &mapping,
            "\"users\"",
            &DoubleQuoteDialect,
            100,
        )
        .unwrap();
        assert!(batches[0].contains("VALUES\n('')"));
    }

    #[test]
    fn rejects_explicit_null_for_not_null_column() {
        let headers = vec!["id".to_string()];
        let columns = vec![TableColumn {
            name: "id".into(),
            database_type: "INTEGER".into(),
            nullable: false,
            default_value: None,
            primary_key: false,
            comment: None,
        }];
        let mapping = auto_map_fields(&headers, &columns);
        let report = validate_types(&[vec![" NULL ".to_string()]], &headers, &columns, &mapping);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(
            report.issues[0].reason,
            "Column is NOT NULL but value is NULL"
        );
    }

    #[test]
    fn generates_insert_batches() {
        let headers = vec!["name".to_string(), "age".to_string()];
        let columns = vec![
            TableColumn {
                name: "name".into(),
                database_type: "VARCHAR".into(),
                nullable: true,
                default_value: None,
                primary_key: false,
                comment: None,
            },
            TableColumn {
                name: "age".into(),
                database_type: "INT".into(),
                nullable: false,
                default_value: None,
                primary_key: false,
                comment: None,
            },
        ];
        let mapping = auto_map_fields(&headers, &columns);
        let rows: Vec<Vec<String>> = (1..=5)
            .map(|i| vec![format!("User{i}"), format!("{}", i * 10)])
            .collect();
        let dialect = DoubleQuoteDialect;
        let batches = generate_insert_batches(
            &rows,
            &headers,
            &columns,
            &mapping,
            "\"public\".\"users\"",
            &dialect,
            2,
        )
        .unwrap();
        assert_eq!(batches.len(), 3); // 5 rows / batch_size 2 = 3 batches
        assert!(batches[0].starts_with("INSERT INTO \"public\".\"users\""));
        assert!(batches[0].contains("'User1', 10"));
        assert!(batches[0].contains("'User2', 20"));
    }
}
