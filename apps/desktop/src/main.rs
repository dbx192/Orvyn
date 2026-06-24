use app_core::{
    connection::ConnectionManager, query::QueryService, storage::FileConnectionProfileStore,
};
use db_common::{base64_preview, human_bytes, CellValue, Column, LogicalType, QueryResult, Row};
use eframe::{
    egui::{
        Align, Button, CentralPanel, Color32, Context, DragValue, Frame, Label, Layout, RichText,
        ScrollArea, Sense, TopBottomPanel, Ui, Vec2,
    },
    NativeOptions,
};
use import_export::{export_query_result, ExportFormat};
use secure_store::EncryptedFileStore;
use std::{cmp::Ordering, path::PathBuf, sync::Arc};

const DEFAULT_PAGE_SIZE: usize = 100;
const MIN_COLUMN_WIDTH: f32 = 64.0;
const MAX_COLUMN_WIDTH: f32 = 640.0;
const ROW_HEIGHT: f32 = 26.0;
const MAX_DISPLAY_CHARS: usize = 96;

fn main() -> eframe::Result<()> {
    tracing::info!("desktop application bootstrap");

    let app_name = app_core::app_name();
    let connection_manager = bootstrap_connection_manager(default_config_dir());
    let query_service = QueryService::with_default_executors(connection_manager.clone());
    let native_options = NativeOptions::default();

    eframe::run_native(
        app_name,
        native_options,
        Box::new(move |_| {
            Box::new(DesktopApp::new(
                app_name.to_string(),
                connection_manager,
                query_service,
            ))
        }),
    )
}

fn bootstrap_connection_manager(config_dir: PathBuf) -> ConnectionManager {
    let profiles_path = config_dir.join("connections.json");
    let secrets_path = config_dir.join("secrets.json");
    let master_key = std::env::var("RUST_DB_CLIENT_MASTER_KEY")
        .unwrap_or_else(|_| "development-master-key-change-me".to_string());

    ConnectionManager::with_default_testers(
        Arc::new(FileConnectionProfileStore::new(profiles_path)),
        Arc::new(EncryptedFileStore::new(secrets_path, master_key)),
    )
}

fn default_config_dir() -> PathBuf {
    std::env::var_os("RUST_DB_CLIENT_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".rust-db-client"))
}

struct DesktopApp {
    app_name: String,
    _connection_manager: ConnectionManager,
    _query_service: QueryService,
    result_grid: ResultSetGrid,
    status: String,
}

impl DesktopApp {
    fn new(
        app_name: String,
        connection_manager: ConnectionManager,
        query_service: QueryService,
    ) -> Self {
        Self {
            app_name,
            _connection_manager: connection_manager,
            _query_service: query_service,
            result_grid: ResultSetGrid::new(sample_result_set()),
            status: "Ready".to_string(),
        }
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(&self.app_name);
                ui.separator();
                ui.label("egui desktop");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(&self.status);
                });
            });
        });

        CentralPanel::default().show(ctx, |ui| {
            ui.heading("Query Result");
            ui.add_space(6.0);
            if let Some(message) = self.result_grid.ui(ui) {
                self.status = message;
            }
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SortState {
    column: usize,
    direction: SortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectedCell {
    row: usize,
    column: usize,
}

struct ResultSetGrid {
    result: QueryResult,
    column_widths: Vec<f32>,
    page: usize,
    page_size: usize,
    selected: Option<SelectedCell>,
    sort: Option<SortState>,
}

impl ResultSetGrid {
    fn new(result: QueryResult) -> Self {
        let column_widths = result
            .columns
            .iter()
            .map(|column| ((column.name.len() as f32 * 8.0) + 56.0).clamp(MIN_COLUMN_WIDTH, 180.0))
            .collect();

        Self {
            result,
            column_widths,
            page: 0,
            page_size: DEFAULT_PAGE_SIZE,
            selected: None,
            sort: None,
        }
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<String> {
        let mut message = None;
        self.toolbar(ui, &mut message);
        ui.add_space(8.0);

        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::horizontal()
                .id_source("result_set_horizontal")
                .show(ui, |ui| {
                    self.header(ui);
                    ui.separator();
                    ScrollArea::vertical()
                        .id_source("result_set_vertical")
                        .max_height(ui.available_height().max(240.0))
                        .auto_shrink([false, false])
                        .show_rows(ui, ROW_HEIGHT, self.page_rows().len(), |ui, row_range| {
                            let page_rows = self.page_rows();
                            for visible_row_index in row_range {
                                let Some(row_index) = page_rows.get(visible_row_index).copied()
                                else {
                                    continue;
                                };
                                self.row(ui, row_index);
                            }
                        });
                });
        });

        message
    }

    fn toolbar(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} rows, {} columns",
                self.result.rows.len(),
                self.result.columns.len()
            ));
            ui.separator();
            ui.label(format!(
                "Page {} / {}",
                self.page + 1,
                self.total_pages().max(1)
            ));

            if ui.add_enabled(self.page > 0, Button::new("Prev")).clicked() {
                self.page -= 1;
            }
            if ui
                .add_enabled(self.page + 1 < self.total_pages(), Button::new("Next"))
                .clicked()
            {
                self.page += 1;
            }

            ui.separator();
            ui.label("Rows/page");
            let old_page_size = self.page_size;
            ui.add(
                DragValue::new(&mut self.page_size)
                    .speed(10)
                    .clamp_range(25..=1000),
            );
            if self.page_size != old_page_size {
                self.page_size = self.page_size.clamp(25, 1000);
                self.page = self.page.min(self.total_pages().saturating_sub(1));
            }

            ui.separator();
            let has_selection = self.selected.is_some();
            if ui
                .add_enabled(has_selection, Button::new("Copy Cell"))
                .clicked()
            {
                if let Some(text) = self.copy_cell_text() {
                    copy_to_clipboard(ui, text);
                    *message = Some("Cell copied".to_string());
                }
            }
            if ui
                .add_enabled(has_selection, Button::new("Copy Row"))
                .clicked()
            {
                if let Some(text) = self.copy_row_text(false) {
                    copy_to_clipboard(ui, text);
                    *message = Some("Row copied".to_string());
                }
            }
            if ui
                .add_enabled(has_selection, Button::new("Copy Row + Header"))
                .clicked()
            {
                if let Some(text) = self.copy_row_text(true) {
                    copy_to_clipboard(ui, text);
                    *message = Some("Row copied with header".to_string());
                }
            }
            if ui.button("Copy Page + Header").clicked() {
                copy_to_clipboard(ui, self.copy_rows_text(self.page_rows(), true));
                *message = Some("Visible page copied with header".to_string());
            }
            if ui.button("Copy All + Header").clicked() {
                copy_to_clipboard(ui, self.copy_rows_text(self.sorted_rows(), true));
                *message = Some("All rows copied with header".to_string());
            }

            ui.separator();
            if ui.button("Export CSV").clicked() {
                self.copy_export(ui, ExportFormat::Csv, message);
            }
            if ui.button("Export JSON").clicked() {
                self.copy_export(ui, ExportFormat::Json, message);
            }
            if ui.button("Export Excel").clicked() {
                self.copy_export(ui, ExportFormat::ExcelCompatible, message);
            }
        });
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for column_index in 0..self.result.columns.len() {
                let column = &self.result.columns[column_index];
                let column_name = column.name.clone();
                let column_hover = format!(
                    "{}{}",
                    column.database_type,
                    if column.nullable { ", nullable" } else { "" }
                );
                let width = self.column_widths[column_index];
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 46.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let marker = self.sort_marker(column_index);
                        let title = format!("{column_name}{marker}");
                        if ui
                            .add_sized([width, 22.0], Button::new(RichText::new(title).strong()))
                            .on_hover_text(column_hover)
                            .clicked()
                        {
                            self.toggle_sort(column_index);
                            self.page = 0;
                        }
                        ui.horizontal(|ui| {
                            ui.label("W");
                            ui.add(
                                DragValue::new(&mut self.column_widths[column_index])
                                    .speed(2.0)
                                    .clamp_range(MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH),
                            );
                        });
                    },
                );
            }
        });
    }

    fn row(&mut self, ui: &mut Ui, row_index: usize) {
        let row = &self.result.rows[row_index];
        ui.horizontal(|ui| {
            for column_index in 0..self.result.columns.len() {
                let width = self.column_widths[column_index];
                let value = row.values.get(column_index).unwrap_or(&CellValue::Null);
                let selected = self.selected
                    == Some(SelectedCell {
                        row: row_index,
                        column: column_index,
                    });
                let text = display_cell_value(value);
                let rich_text = cell_rich_text(value, text);
                let response = ui
                    .allocate_ui_with_layout(
                        Vec2::new(width, ROW_HEIGHT),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            let label = Label::new(rich_text).sense(Sense::click());
                            ui.add_sized([width, ROW_HEIGHT], label)
                        },
                    )
                    .inner
                    .on_hover_text(copy_cell_value(value));

                if selected {
                    let rect = response.rect;
                    ui.painter()
                        .rect_stroke(rect, 2.0, (1.0, ui.visuals().selection.stroke.color));
                }
                if response.clicked() {
                    self.selected = Some(SelectedCell {
                        row: row_index,
                        column: column_index,
                    });
                }
            }
        });
    }

    fn page_rows(&self) -> Vec<usize> {
        let start = self.page.saturating_mul(self.page_size);
        self.sorted_rows()
            .into_iter()
            .skip(start)
            .take(self.page_size)
            .collect()
    }

    fn sorted_rows(&self) -> Vec<usize> {
        let mut row_indexes = (0..self.result.rows.len()).collect::<Vec<_>>();
        if let Some(sort) = self.sort {
            row_indexes.sort_by(|left, right| self.compare_rows(*left, *right, sort));
        }
        row_indexes
    }

    fn total_pages(&self) -> usize {
        self.result.rows.len().div_ceil(self.page_size.max(1))
    }

    fn toggle_sort(&mut self, column: usize) {
        self.sort = match self.sort {
            Some(sort) if sort.column == column && sort.direction == SortDirection::Asc => {
                Some(SortState {
                    column,
                    direction: SortDirection::Desc,
                })
            }
            Some(sort) if sort.column == column && sort.direction == SortDirection::Desc => None,
            _ => Some(SortState {
                column,
                direction: SortDirection::Asc,
            }),
        };
    }

    fn sort_marker(&self, column: usize) -> &'static str {
        match self.sort {
            Some(SortState {
                column: sort_column,
                direction: SortDirection::Asc,
            }) if sort_column == column => " ^",
            Some(SortState {
                column: sort_column,
                direction: SortDirection::Desc,
            }) if sort_column == column => " v",
            _ => "",
        }
    }

    fn compare_rows(&self, left: usize, right: usize, sort: SortState) -> Ordering {
        let left_value = self
            .result
            .rows
            .get(left)
            .and_then(|row| row.values.get(sort.column))
            .unwrap_or(&CellValue::Null);
        let right_value = self
            .result
            .rows
            .get(right)
            .and_then(|row| row.values.get(sort.column))
            .unwrap_or(&CellValue::Null);
        let ordering = compare_cell_values(left_value, right_value);

        match sort.direction {
            SortDirection::Asc => ordering,
            SortDirection::Desc => ordering.reverse(),
        }
    }

    fn copy_cell_text(&self) -> Option<String> {
        let selected = self.selected?;
        let row = self.result.rows.get(selected.row)?;
        let value = row.values.get(selected.column)?;
        Some(copy_cell_value(value))
    }

    fn copy_row_text(&self, include_header: bool) -> Option<String> {
        let selected = self.selected?;
        let mut lines = Vec::new();
        if include_header {
            lines.push(self.copy_header_text());
        }
        lines.push(self.copy_row_values(selected.row)?);
        Some(lines.join("\n"))
    }

    fn copy_rows_text(&self, row_indexes: Vec<usize>, include_header: bool) -> String {
        let mut lines = Vec::new();
        if include_header {
            lines.push(self.copy_header_text());
        }
        lines.extend(
            row_indexes
                .into_iter()
                .filter_map(|row_index| self.copy_row_values(row_index)),
        );
        lines.join("\n")
    }

    fn copy_header_text(&self) -> String {
        self.result
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>()
            .join("\t")
    }

    fn copy_row_values(&self, row_index: usize) -> Option<String> {
        let row = self.result.rows.get(row_index)?;
        Some(
            (0..self.result.columns.len())
                .map(|column_index| {
                    row.values
                        .get(column_index)
                        .map(copy_cell_value)
                        .unwrap_or_else(|| "NULL".to_string())
                })
                .collect::<Vec<_>>()
                .join("\t"),
        )
    }

    fn copy_export(&self, ui: &mut Ui, format: ExportFormat, message: &mut Option<String>) {
        match export_query_result(&self.result, format) {
            Ok(text) => {
                copy_to_clipboard(ui, text);
                *message = Some(match format {
                    ExportFormat::Csv => "CSV export copied to clipboard".to_string(),
                    ExportFormat::Json => "JSON export copied to clipboard".to_string(),
                    ExportFormat::ExcelCompatible => {
                        "Excel-compatible export copied to clipboard".to_string()
                    }
                });
            }
            Err(error) => {
                *message = Some(format!("Export failed: {}", error.message));
            }
        }
    }
}

fn copy_to_clipboard(ui: &mut Ui, text: String) {
    ui.output_mut(|output| {
        output.copied_text = text;
    });
}

fn cell_rich_text(value: &CellValue, text: String) -> RichText {
    match value {
        CellValue::Null => RichText::new(text)
            .italics()
            .color(Color32::from_rgb(175, 92, 64)),
        CellValue::Bytes(_) => RichText::new(text).color(Color32::from_rgb(85, 108, 166)),
        _ => RichText::new(text),
    }
}

fn display_cell_value(value: &CellValue) -> String {
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

fn truncate_display(value: String) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(MAX_DISPLAY_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn copy_cell_value(value: &CellValue) -> String {
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

fn compare_cell_values(left: &CellValue, right: &CellValue) -> Ordering {
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

fn sample_result_set() -> QueryResult {
    let rows = (0..250)
        .map(|index| {
            Row::new(vec![
                CellValue::Int(index),
                if index % 9 == 0 {
                    CellValue::Null
                } else {
                    CellValue::Text(format!("customer_{index:03}"))
                },
                CellValue::Bool(index % 2 == 0),
                CellValue::Bytes(vec![0x52, 0x53, 0x54, index as u8, 0, 255]),
                CellValue::Timestamp(
                    chrono::DateTime::from_timestamp(1_718_000_000 + i64::from(index) * 3600, 0)
                        .unwrap_or_else(db_common::now_utc),
                ),
            ])
        })
        .collect();

    QueryResult {
        statement: "select id, name, active, payload, updated_at from customers".to_string(),
        columns: vec![
            column("id", "BIGINT", LogicalType::Int, false),
            column("name", "VARCHAR", LogicalType::Text, true),
            column("active", "BOOLEAN", LogicalType::Bool, false),
            column("payload", "BLOB", LogicalType::Bytes, true),
            column("updated_at", "TIMESTAMP", LogicalType::DateTime, false),
        ],
        rows,
        affected_rows: 0,
        execution_time_ms: 18,
        notices: Vec::new(),
        metadata: Default::default(),
    }
}

fn column(name: &str, database_type: &str, logical_type: LogicalType, nullable: bool) -> Column {
    Column {
        name: name.to_string(),
        database_type: database_type.to_string(),
        logical_type,
        nullable,
        primary_key: name == "id",
    }
}
