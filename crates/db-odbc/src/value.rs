//! Mapping between ODBC column types and the app's `CellValue`/`LogicalType`.
//!
//! Values are fetched as text (see `executor::fetch_cursor`) and re-typed
//! here. Text-first fetching sidesteps driver quirks (Oracle's ODBC driver
//! rejects 64-bit integer binds, and NUMBER binds would lose precision) at
//! the cost of a conversion per cell, which is fine for GUI row volumes.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use db_common::{CellValue, LogicalType};
use odbc_api::DataType;
use std::num::NonZeroUsize;

/// Human-readable column type label, e.g. `VARCHAR(255)` or `DECIMAL(10,2)`.
pub fn database_type_label(data_type: DataType) -> String {
    fn fmt_len(length: Option<NonZeroUsize>) -> String {
        length
            .map(|value| value.get().to_string())
            .unwrap_or_else(|| "max".to_string())
    }

    match data_type {
        DataType::Unknown => "UNKNOWN".to_string(),
        DataType::Char { length } => format!("CHAR({})", fmt_len(length)),
        DataType::WChar { length } => format!("NCHAR({})", fmt_len(length)),
        DataType::Numeric { precision, scale } => format!("NUMERIC({precision},{scale})"),
        DataType::Decimal { precision, scale } => format!("DECIMAL({precision},{scale})"),
        DataType::Integer => "INTEGER".to_string(),
        DataType::SmallInt => "SMALLINT".to_string(),
        DataType::Float { precision } => format!("FLOAT({precision})"),
        DataType::Real => "REAL".to_string(),
        DataType::Double => "DOUBLE".to_string(),
        DataType::Varchar { length } => format!("VARCHAR({})", fmt_len(length)),
        DataType::WVarchar { length } => format!("NVARCHAR({})", fmt_len(length)),
        DataType::LongVarchar { .. } => "TEXT".to_string(),
        DataType::WLongVarchar { .. } => "NTEXT".to_string(),
        DataType::LongVarbinary { .. } => "BLOB".to_string(),
        DataType::Date => "DATE".to_string(),
        DataType::Time { .. } => "TIME".to_string(),
        DataType::Timestamp { precision } => {
            if precision > 0 {
                format!("TIMESTAMP({precision})")
            } else {
                "TIMESTAMP".to_string()
            }
        }
        DataType::BigInt => "BIGINT".to_string(),
        DataType::TinyInt => "TINYINT".to_string(),
        DataType::Bit => "BIT".to_string(),
        DataType::Varbinary { length } => format!("VARBINARY({})", fmt_len(length)),
        DataType::Binary { length } => format!("BINARY({})", fmt_len(length)),
        DataType::Other { .. } => "OTHER".to_string(),
    }
}

/// Coarse logical type used by the grid for alignment/formatting decisions.
pub fn logical_type(data_type: DataType) -> LogicalType {
    match data_type {
        DataType::Bit => LogicalType::Bool,
        DataType::Integer
        | DataType::SmallInt
        | DataType::BigInt
        | DataType::TinyInt => LogicalType::Int,
        DataType::Real | DataType::Float { .. } | DataType::Double => LogicalType::Float,
        DataType::Numeric { .. } | DataType::Decimal { .. } => LogicalType::Decimal,
        DataType::Date => LogicalType::Date,
        DataType::Time { .. } => LogicalType::Time,
        DataType::Timestamp { .. } => LogicalType::DateTime,
        DataType::Char { .. }
        | DataType::WChar { .. }
        | DataType::Varchar { .. }
        | DataType::WVarchar { .. }
        | DataType::LongVarchar { .. }
        | DataType::WLongVarchar { .. } => LogicalType::Text,
        DataType::Binary { .. }
        | DataType::Varbinary { .. }
        | DataType::LongVarbinary { .. } => LogicalType::Bytes,
        DataType::Unknown | DataType::Other { .. } => LogicalType::Unknown,
    }
}

/// Convert one fetched text cell into a typed [`CellValue`] according to the
/// column's ODBC type. `None` means the value was SQL NULL. Unparseable
/// values fall back to `Text` instead of failing the whole result set.
pub fn cell_from_text(data_type: DataType, text: Option<&str>) -> CellValue {
    let Some(text) = text else {
        return CellValue::Null;
    };

    match data_type {
        DataType::Integer | DataType::SmallInt | DataType::BigInt | DataType::TinyInt => text
            .parse::<i64>()
            .map(CellValue::Int)
            .unwrap_or_else(|_| CellValue::Decimal(text.to_string())),
        DataType::Real | DataType::Float { .. } | DataType::Double => text
            .parse::<f64>()
            .map(CellValue::Float)
            .unwrap_or_else(|_| CellValue::Text(text.to_string())),
        DataType::Numeric { .. } | DataType::Decimal { .. } => {
            CellValue::Decimal(text.to_string())
        }
        DataType::Bit => parse_bit(text),
        DataType::Date => NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .map(CellValue::Date)
            .unwrap_or_else(|_| CellValue::Text(text.to_string())),
        DataType::Time { .. } => parse_time(text)
            .map(CellValue::Time)
            .unwrap_or_else(|| CellValue::Text(text.to_string())),
        DataType::Timestamp { .. } => parse_timestamp(text)
            .map(CellValue::DateTime)
            .unwrap_or_else(|| CellValue::Text(text.to_string())),
        _ => CellValue::Text(text.to_string()),
    }
}

fn parse_bit(text: &str) -> CellValue {
    let normalized = text.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "1" | "true" | "t" | "yes" | "y" => CellValue::Bool(true),
        "0" | "false" | "f" | "no" | "n" => CellValue::Bool(false),
        _ => text
            .parse::<i64>()
            .map(CellValue::Int)
            .unwrap_or_else(|_| CellValue::Text(text.to_string())),
    }
}

fn parse_time(text: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(text.trim(), "%H:%M:%S%.f")
        .or_else(|_| NaiveTime::parse_from_str(text.trim(), "%H:%M:%S"))
        .ok()
}

fn parse_timestamp(text: &str) -> Option<NaiveDateTime> {
    let trimmed = text.trim();
    NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S"))
        .ok()
        .or_else(|| {
            let date = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").ok()?;
            date.and_hms_opt(0, 0, 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn integers_become_int_or_decimal_fallback() {
        assert_eq!(
            cell_from_text(DataType::Integer, Some("42")),
            CellValue::Int(42)
        );
        // Oracle NUMBER columns report as DECIMAL but hold big integers.
        assert_eq!(
            cell_from_text(DataType::Decimal { precision: 38, scale: 0 }, Some("123456789012345678901234567890")),
            CellValue::Decimal("123456789012345678901234567890".to_string())
        );
    }

    #[test]
    fn unparseable_values_fall_back_gracefully() {
        // Integer columns fall back to Decimal (Oracle NUMBER quirk safety).
        assert_eq!(
            cell_from_text(DataType::Integer, Some("abc")),
            CellValue::Decimal("abc".to_string())
        );
        // Text columns keep the raw text.
        assert_eq!(
            cell_from_text(
                DataType::Varchar {
                    length: NonZeroUsize::new(10)
                },
                Some("abc")
            ),
            CellValue::Text("abc".to_string())
        );
    }

    #[test]
    fn timestamps_parse_with_and_without_fraction() {
        let expected = NaiveDate::from_ymd_opt(2026, 1, 2)
            .unwrap()
            .and_hms_opt(3, 4, 5)
            .unwrap();
        assert_eq!(
            cell_from_text(DataType::Timestamp { precision: 0 }, Some("2026-01-02 03:04:05")),
            CellValue::DateTime(expected)
        );
        let fractional = expected + chrono::Duration::microseconds(123456);
        assert_eq!(
            cell_from_text(
                DataType::Timestamp { precision: 6 },
                Some("2026-01-02 03:04:05.123456")
            ),
            CellValue::DateTime(fractional)
        );
    }

    #[test]
    fn bit_values_parse() {
        assert_eq!(cell_from_text(DataType::Bit, Some("1")), CellValue::Bool(true));
        assert_eq!(
            cell_from_text(DataType::Bit, Some("false")),
            CellValue::Bool(false)
        );
    }

    #[test]
    fn null_becomes_null_regardless_of_type() {
        assert_eq!(cell_from_text(DataType::Date, None), CellValue::Null);
    }
}
