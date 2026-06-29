use db_common::{base64_preview, human_bytes, CellValue};
use eframe::egui::{Color32, FontFamily, RichText, Ui};
use std::cmp::Ordering;

const MAX_DISPLAY_CHARS: usize = 96;
/// Sentinel we use when an empty string should be visually distinguished from
/// "no value" without losing the actual content during copy.
const EMPTY_STRING_SENTINEL: &str = "\"\"";

pub fn copy_to_clipboard(ui: &mut Ui, text: String) {
    ui.output_mut(|output| {
        output.copied_text = text;
    });
}

/// Render a cell value as `RichText` so that NULL, empty strings, binary, and
/// date/time values are visually unambiguous.
///
/// Visual rules:
/// - `NULL`  → dimmed italic "NULL" with a warning color.
/// - `""`    → dimmed italic `""` to make empty-string clearly distinct from
///   NULL and from regular text.
/// - `Bytes` → blue monospace-ish summary like `<binary 12 B base64 AABB...>`.
/// - Date/Time/DateTime/Timestamp → time-aware coloring, but otherwise plain.
/// - `Json`  → light gray to indicate structured payload.
pub fn cell_rich_text(value: &CellValue, text: String) -> RichText {
    match value {
        CellValue::Null => RichText::new(text)
            .family(FontFamily::Monospace)
            .italics()
            .color(Color32::from_rgb(170, 90, 60))
            .strong(),
        CellValue::Text(s) if s.is_empty() => RichText::new(text)
            .family(FontFamily::Monospace)
            .italics()
            .color(Color32::from_rgb(150, 150, 150))
            .weak(),
        CellValue::Bytes(_) => RichText::new(text)
            .family(FontFamily::Monospace)
            .color(Color32::from_rgb(70, 110, 175)),
        CellValue::Date(_)
        | CellValue::Time(_)
        | CellValue::DateTime(_)
        | CellValue::Timestamp(_) => RichText::new(text)
            .family(FontFamily::Monospace)
            .color(Color32::from_rgb(70, 110, 70)),
        CellValue::Json(_) => RichText::new(text)
            .family(FontFamily::Monospace)
            .color(Color32::from_rgb(140, 100, 150)),
        _ => RichText::new(text),
    }
}

pub fn display_cell_value(value: &CellValue) -> String {
    truncate_display(match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Text(s) if s.is_empty() => EMPTY_STRING_SENTINEL.to_string(),
        CellValue::Bytes(bytes) => format!(
            "<binary: {}, base64 {}>",
            human_bytes(bytes.len() as u64),
            base64_preview(bytes, 16)
        ),
        CellValue::Date(value) => format!("<date {}>", value.format("%Y-%m-%d")),
        CellValue::Time(value) => format!("<time {}>", value.format("%H:%M:%S")),
        CellValue::DateTime(value) => {
            format!("<datetime {}>", value.format("%Y-%m-%d %H:%M:%S"))
        }
        CellValue::Timestamp(value) => {
            format!("<timestamp {}>", value.format("%Y-%m-%d %H:%M:%S UTC"))
        }
        CellValue::Json(value) => format!("<json {}>", value),
        _ => copy_cell_value(value),
    })
}

pub fn hover_cell_value(value: &CellValue) -> String {
    match value {
        CellValue::Null => "Type: NULL\nValue: NULL".to_string(),
        CellValue::Text(text) if text.is_empty() => "Type: TEXT\nValue: empty string".to_string(),
        CellValue::Text(text) => format!("Type: TEXT\nValue: {}", text),
        CellValue::Bytes(bytes) => format!(
            "Type: BINARY\nSize: {}\nBase64: {}",
            human_bytes(bytes.len() as u64),
            base64_preview(bytes, bytes.len().min(128))
        ),
        CellValue::Date(value) => format!("Type: DATE\nValue: {}", value.format("%Y-%m-%d")),
        CellValue::Time(value) => format!("Type: TIME\nValue: {}", value.format("%H:%M:%S%.3f")),
        CellValue::DateTime(value) => {
            format!(
                "Type: DATETIME\nValue: {}",
                value.format("%Y-%m-%d %H:%M:%S%.3f")
            )
        }
        CellValue::Timestamp(value) => format!("Type: TIMESTAMP\nValue: {}", value.to_rfc3339()),
        CellValue::Json(value) => format!("Type: JSON\nValue: {}", value),
        CellValue::Bool(value) => format!("Type: BOOL\nValue: {}", value),
        CellValue::Int(value) => format!("Type: INT\nValue: {}", value),
        CellValue::UInt(value) => format!("Type: UINT\nValue: {}", value),
        CellValue::Float(value) => format!("Type: FLOAT\nValue: {}", value),
        CellValue::Decimal(value) => format!("Type: DECIMAL\nValue: {}", value),
    }
}

pub fn truncate_display(value: String) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(MAX_DISPLAY_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

/// Convert a cell value to its clipboard-friendly form.
///
/// Distinct outputs:
/// - `NULL` → literal text `NULL`.
/// - empty string → empty string (no quotes), distinct from NULL when pasted.
/// - `Bytes` → full base64 representation, so nothing is lost on copy.
/// - `Date` → `YYYY-MM-DD`.
/// - `Time` → `HH:MM:SS.fff`.
/// - `DateTime` → `YYYY-MM-DD HH:MM:SS.fff`.
/// - `Timestamp` → RFC3339.
pub fn copy_cell_value(value: &CellValue) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(value) => value.to_string(),
        CellValue::Int(value) => value.to_string(),
        CellValue::UInt(value) => value.to_string(),
        CellValue::Float(value) => value.to_string(),
        CellValue::Decimal(value) | CellValue::Text(value) => value.clone(),
        CellValue::Bytes(bytes) => base64_preview(bytes, bytes.len()),
        CellValue::Date(value) => value.format("%Y-%m-%d").to_string(),
        CellValue::Time(value) => value.format("%H:%M:%S%.3f").to_string(),
        CellValue::DateTime(value) => value.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        CellValue::Timestamp(value) => value.to_rfc3339(),
        CellValue::Json(value) => value.to_string(),
    }
}

pub fn compare_cell_values(left: &CellValue, right: &CellValue) -> Ordering {
    match (left, right) {
        (CellValue::Null, CellValue::Null) => Ordering::Equal,
        (CellValue::Null, _) => Ordering::Less,
        (_, CellValue::Null) => Ordering::Greater,
        (CellValue::Bool(left), CellValue::Bool(right)) => left.cmp(right),
        (CellValue::Int(left), CellValue::Int(right)) => left.cmp(right),
        (CellValue::UInt(left), CellValue::UInt(right)) => left.cmp(right),
        (CellValue::Float(left), CellValue::Float(right)) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (CellValue::Decimal(left), CellValue::Decimal(right)) => left.cmp(right),
        (CellValue::Text(left), CellValue::Text(right)) => left.cmp(right),
        (CellValue::Bytes(left), CellValue::Bytes(right)) => left.len().cmp(&right.len()),
        (CellValue::Date(left), CellValue::Date(right)) => left.cmp(right),
        (CellValue::Time(left), CellValue::Time(right)) => left.cmp(right),
        (CellValue::DateTime(left), CellValue::DateTime(right)) => left.cmp(right),
        (CellValue::Timestamp(left), CellValue::Timestamp(right)) => left.cmp(right),
        (CellValue::Json(left), CellValue::Json(right)) => left.to_string().cmp(&right.to_string()),
        _ => copy_cell_value(left).cmp(&copy_cell_value(right)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

    #[test]
    fn null_renders_as_distinct_marker() {
        let text = display_cell_value(&CellValue::Null);
        assert_eq!(text, "NULL");
    }

    #[test]
    fn empty_string_renders_as_distinct_marker() {
        let text = display_cell_value(&CellValue::Text(String::new()));
        assert_eq!(text, EMPTY_STRING_SENTINEL);
    }

    #[test]
    fn copy_distinguishes_null_from_empty_string() {
        assert_eq!(copy_cell_value(&CellValue::Null), "NULL");
        assert_eq!(copy_cell_value(&CellValue::Text(String::new())), "");
    }

    #[test]
    fn binary_marker_contains_size() {
        let text = display_cell_value(&CellValue::Bytes(vec![0u8; 32]));
        assert!(text.contains("32 B"));
        assert!(text.contains("base64"));
    }

    #[test]
    fn date_renders_in_iso() {
        let text = display_cell_value(&CellValue::Date(
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
        ));
        assert_eq!(text, "<date 2024-01-02>");
    }

    #[test]
    fn datetime_renders_in_iso() {
        let text = display_cell_value(&CellValue::DateTime(NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            NaiveTime::from_hms_opt(3, 4, 5).unwrap(),
        )));
        assert_eq!(text, "<datetime 2024-01-02 03:04:05>");
    }
}
