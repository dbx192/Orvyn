use db_common::{base64_preview, human_bytes, CellValue};
use eframe::egui::{Color32, RichText, Ui};
use std::cmp::Ordering;

const MAX_DISPLAY_CHARS: usize = 96;

pub fn copy_to_clipboard(ui: &mut Ui, text: String) {
    ui.output_mut(|output| {
        output.copied_text = text;
    });
}

pub fn cell_rich_text(value: &CellValue, text: String) -> RichText {
    match value {
        CellValue::Null => RichText::new(text)
            .italics()
            .color(Color32::from_rgb(175, 92, 64)),
        CellValue::Bytes(_) => RichText::new(text).color(Color32::from_rgb(85, 108, 166)),
        _ => RichText::new(text),
    }
}

pub fn display_cell_value(value: &CellValue) -> String {
    truncate_display(match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bytes(bytes) => format!(
            "<binary: {}, base64 {}>",
            human_bytes(bytes.len() as u64),
            base64_preview(bytes, 12)
        ),
        CellValue::Date(value) => value.format("%Y-%m-%d").to_string(),
        CellValue::Time(value) => value.format("%H:%M:%S").to_string(),
        CellValue::DateTime(value) => value.format("%Y-%m-%d %H:%M:%S").to_string(),
        CellValue::Timestamp(value) => value.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        CellValue::Json(value) => value.to_string(),
        _ => copy_cell_value(value),
    })
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
