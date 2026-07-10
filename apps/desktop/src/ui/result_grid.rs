#![allow(dead_code)]

use crate::ui::format::{
    cell_rich_text, compare_cell_values, copy_cell_value, copy_to_clipboard, display_cell_value,
    hover_cell_value,
};
use crate::ui::grid::{
    estimate_text_width, grid_stroke, label_cell, paint_cell_outline, resizable_header_cell,
    row_fill, TABLE_HEADER_HEIGHT, TABLE_ROW_HEIGHT,
};
use db_common::{
    CellValue, QueryResult, QUERY_RESULT_METADATA_PAGE_LIMIT, QUERY_RESULT_METADATA_PAGE_OFFSET,
    QUERY_RESULT_METADATA_TOTAL_ROWS,
};
use eframe::egui::{
    pos2, vec2, Align, Button, Color32, DragValue, Frame, Layout, Margin, Rect, RichText,
    ScrollArea, Sense, Stroke, Ui,
};
use import_export::{export_query_result, export_query_result_to_file, ExportFormat};
use std::cmp::Ordering;
use tokio::runtime::Handle;

const DEFAULT_PAGE_SIZE: usize = 500;
const MIN_COLUMN_WIDTH: f32 = 56.0;
const MAX_COLUMN_WIDTH: f32 = 640.0;
const INITIAL_MAX_COLUMN_WIDTH: f32 = 320.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultSetGridAction {
    Message(String),
    PageRequested { offset: u64, page_size: u32 },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemotePage {
    limit: usize,
    offset: u64,
    total_rows: u64,
}

pub struct ResultSetGrid {
    result: QueryResult,
    column_widths: Vec<f32>,
    page: usize,
    page_size: usize,
    selected: Option<SelectedCell>,
    sort: Option<SortState>,
    runtime: Option<Handle>,
    scroll_x: f32,
}

impl ResultSetGrid {
    pub fn new(result: QueryResult) -> Self {
        let page_size = page_size_from_result(&result);
        Self {
            column_widths: estimate_column_widths(&result),
            result,
            page: 0,
            page_size,
            selected: None,
            sort: None,
            runtime: None,
            scroll_x: 0.0,
        }
    }

    pub fn with_runtime(mut self, runtime: Handle) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn set_result(&mut self, result: QueryResult) {
        self.column_widths = estimate_column_widths(&result);
        self.page_size = page_size_from_result(&result);
        self.result = result;
        self.page = 0;
        self.selected = None;
        self.sort = None;
        self.scroll_x = 0.0;
    }

    pub fn result(&self) -> &QueryResult {
        &self.result
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<ResultSetGridAction> {
        let mut action = None;
        self.toolbar(ui, &mut action);

        if self.result.columns.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new("No columns in result")
                        .color(Color32::GRAY)
                        .italics(),
                );
                if !self.result.rows.is_empty() {
                    ui.label(
                        RichText::new(format!("{} row(s) returned", self.result.rows.len()))
                            .color(Color32::GRAY),
                    );
                }
            });
            return action;
        }

        let page_rows = self.page_rows();

        Frame::default()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(grid_stroke(ui))
            .inner_margin(Margin::same(0.0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                self.header(ui);
                let body_height = ui.available_height().max(0.0);
                let content_width = self.content_width();
                let output = ScrollArea::both()
                    .id_source("result_set_body")
                    .max_height(body_height)
                    .auto_shrink([false, false])
                    .show_rows(ui, TABLE_ROW_HEIGHT, page_rows.len(), |ui, row_range| {
                        ui.set_min_width(content_width);
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        ui.spacing_mut().interact_size = vec2(0.0, TABLE_ROW_HEIGHT);
                        for visible_row_index in row_range {
                            let Some(row_index) = page_rows.get(visible_row_index).copied() else {
                                continue;
                            };
                            self.row(ui, row_index, visible_row_index);
                        }
                    });
                self.scroll_x = output.state.offset.x;
            });

        action
    }

    fn toolbar(&mut self, ui: &mut Ui, action: &mut Option<ResultSetGridAction>) {
        ui.horizontal_wrapped(|ui| {
            if let Some(remote_page) = self.remote_page() {
                ui.label(
                    RichText::new(format!(
                        "{} of {} rows",
                        self.result.rows.len(),
                        remote_page.total_rows
                    ))
                    .strong(),
                );
            } else {
                ui.label(RichText::new(format!("{} rows", self.result.rows.len())).strong());
            }
            ui.label(
                RichText::new(format!("{} columns", self.result.columns.len()))
                    .small()
                    .color(Color32::GRAY),
            );
            if self.result.execution_time_ms > 0 {
                ui.separator();
                ui.label(
                    RichText::new(format!("{} ms", self.result.execution_time_ms))
                        .color(Color32::from_rgb(60, 140, 60)),
                );
            }

            ui.separator();
            if let Some(remote_page) = self.remote_page() {
                self.remote_pager(ui, remote_page, action);
            } else {
                self.local_pager(ui);
            }

            ui.separator();
            let old_page_size = self.page_size;
            ui.add(
                DragValue::new(&mut self.page_size)
                    .speed(10)
                    .clamp_range(25..=1000),
            );
            if self.page_size != old_page_size {
                self.page_size = self.page_size.clamp(25, 1000);
                self.selected = None;
                if self.remote_page().is_some() {
                    *action = Some(ResultSetGridAction::PageRequested {
                        offset: 0,
                        page_size: self.page_size as u32,
                    });
                } else {
                    self.page = self.page.min(self.total_pages().saturating_sub(1));
                }
            }

            ui.separator();
            let has_selection = self.selected.is_some();
            ui.menu_button("Copy", |ui| {
                if ui.add_enabled(has_selection, Button::new("Cell")).clicked() {
                    if let Some(text) = self.copy_cell_text() {
                        copy_to_clipboard(ui, text);
                        *action = Some(ResultSetGridAction::Message("Cell copied".to_string()));
                    }
                    ui.close_menu();
                }
                if ui.add_enabled(has_selection, Button::new("Row")).clicked() {
                    if let Some(text) = self.copy_row_text(false) {
                        copy_to_clipboard(ui, text);
                        *action = Some(ResultSetGridAction::Message("Row copied".to_string()));
                    }
                    ui.close_menu();
                }
                if ui
                    .add_enabled(has_selection, Button::new("Row with Header"))
                    .clicked()
                {
                    if let Some(text) = self.copy_row_text(true) {
                        copy_to_clipboard(ui, text);
                        *action = Some(ResultSetGridAction::Message(
                            "Row copied with header".to_string(),
                        ));
                    }
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Page with Header").clicked() {
                    copy_to_clipboard(ui, self.copy_rows_text(self.page_rows(), true));
                    *action = Some(ResultSetGridAction::Message(
                        "Current page copied with header".to_string(),
                    ));
                    ui.close_menu();
                }
                if ui.button("All with Header").clicked() {
                    copy_to_clipboard(ui, self.copy_rows_text(self.sorted_rows(), true));
                    *action = Some(ResultSetGridAction::Message(
                        "All rows copied with header".to_string(),
                    ));
                    ui.close_menu();
                }
            });

            ui.menu_button("Export", |ui| {
                if ui.button("Copy CSV").clicked() {
                    self.copy_export(ui, ExportFormat::Csv, action);
                    ui.close_menu();
                }
                if ui.button("Copy JSON").clicked() {
                    self.copy_export(ui, ExportFormat::Json, action);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Save CSV").clicked() {
                    self.save_export(ExportFormat::Csv, action);
                    ui.close_menu();
                }
                if ui.button("Save JSON").clicked() {
                    self.save_export(ExportFormat::Json, action);
                    ui.close_menu();
                }
            });
        });

        if let Some(sort) = self.sort {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Sorted by {}{}",
                        self.result.columns[sort.column].name,
                        match sort.direction {
                            SortDirection::Asc => " ↑",
                            SortDirection::Desc => " ↓",
                        }
                    ))
                    .color(Color32::from_rgb(80, 80, 180))
                    .small(),
                );
                if ui.small_button("Clear sort").clicked() {
                    self.sort = None;
                    self.page = 0;
                    self.selected = None;
                }
            });
        }

        if let Some(selected) = self.selected {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Selected R{} C{}",
                        selected.row + 1,
                        selected.column + 1
                    ))
                    .small()
                    .color(Color32::from_rgb(70, 110, 190)),
                );
                if let Some(value) = self
                    .result
                    .rows
                    .get(selected.row)
                    .and_then(|row| row.values.get(selected.column))
                {
                    ui.label(
                        RichText::new(display_cell_value(value))
                            .small()
                            .color(Color32::GRAY),
                    );
                }
            });
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        let viewport_width = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(vec2(viewport_width, TABLE_HEADER_HEIGHT), Sense::hover());
        let header_rect = Rect::from_min_size(
            pos2(rect.left() - self.scroll_x, rect.top()),
            vec2(self.content_width(), TABLE_HEADER_HEIGHT),
        );
        let mut header_ui = ui.child_ui(header_rect, Layout::left_to_right(Align::Center));
        header_ui.set_clip_rect(rect);
        header_ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

        header_ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for column_index in 0..self.result.columns.len() {
                let column = self.result.columns[column_index].clone();
                let sort_dir = self.sort_direction_for(column_index);
                let column_hover = format!(
                    "{}{}{}",
                    column.database_type,
                    if column.nullable { " · nullable" } else { "" },
                    if column.primary_key { " · PK" } else { "" }
                );

                let marker = match sort_dir {
                    Some(SortDirection::Asc) => " ↑",
                    Some(SortDirection::Desc) => " ↓",
                    None => "",
                };
                let title = format!("{}{}", column.name, marker);
                let title = if sort_dir.is_some() {
                    RichText::new(title)
                        .strong()
                        .color(Color32::from_rgb(90, 150, 230))
                } else {
                    RichText::new(title).strong()
                };
                let header = resizable_header_cell(
                    ui,
                    &mut self.column_widths[column_index],
                    TABLE_HEADER_HEIGHT,
                    title,
                    format!("{column_hover}\nClick to sort"),
                    ui.make_persistent_id(("result_column_resize", column_index)),
                    MIN_COLUMN_WIDTH,
                    MAX_COLUMN_WIDTH,
                );

                if header.response.clicked() && !header.resizing {
                    self.toggle_sort(column_index);
                    self.page = 0;
                }
            }
        });
    }

    fn content_width(&self) -> f32 {
        self.column_widths.iter().sum()
    }

    fn local_pager(&mut self, ui: &mut Ui) {
        if ui.add_enabled(self.page > 0, Button::new("<")).clicked() {
            self.page -= 1;
            self.selected = None;
        }
        ui.label(format!("{} / {}", self.page + 1, self.total_pages().max(1)));
        if ui
            .add_enabled(self.page + 1 < self.total_pages(), Button::new(">"))
            .clicked()
        {
            self.page += 1;
            self.selected = None;
        }
        let page_end = self.page_end();
        let range_text = if page_end == 0 {
            "0-0".to_string()
        } else {
            format!("{}-{page_end}", self.page_start() + 1)
        };
        ui.label(RichText::new(range_text).small().color(Color32::GRAY));
    }

    fn remote_pager(
        &mut self,
        ui: &mut Ui,
        remote_page: RemotePage,
        action: &mut Option<ResultSetGridAction>,
    ) {
        let page_size = remote_page.limit.max(1) as u64;
        let current_page = (remote_page.offset / page_size) + 1;
        let total_pages = remote_page.total_rows.div_ceil(page_size).max(1);
        if ui
            .add_enabled(remote_page.offset > 0, Button::new("<"))
            .clicked()
        {
            *action = Some(ResultSetGridAction::PageRequested {
                offset: remote_page.offset.saturating_sub(page_size),
                page_size: remote_page.limit as u32,
            });
        }
        ui.label(format!("{current_page} / {total_pages}"));
        let loaded_rows = self.result.rows.len() as u64;
        let has_next = remote_page.offset + loaded_rows < remote_page.total_rows;
        if ui.add_enabled(has_next, Button::new(">")).clicked() {
            *action = Some(ResultSetGridAction::PageRequested {
                offset: remote_page.offset + page_size,
                page_size: remote_page.limit as u32,
            });
        }
        let range_text = if loaded_rows == 0 {
            "0-0".to_string()
        } else {
            format!(
                "{}-{}",
                remote_page.offset + 1,
                remote_page.offset + loaded_rows
            )
        };
        ui.label(RichText::new(range_text).small().color(Color32::GRAY));
    }

    fn row(&mut self, ui: &mut Ui, row_index: usize, visible_row_index: usize) {
        ui.spacing_mut().interact_size = vec2(0.0, TABLE_ROW_HEIGHT);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.spacing_mut().interact_size = vec2(0.0, TABLE_ROW_HEIGHT);

            for column_index in 0..self.result.columns.len() {
                let width = self.column_widths[column_index];
                let value = self
                    .result
                    .rows
                    .get(row_index)
                    .and_then(|row| row.values.get(column_index))
                    .unwrap_or(&CellValue::Null);
                let selected = self.selected
                    == Some(SelectedCell {
                        row: row_index,
                        column: column_index,
                    });

                let text = display_cell_value(value);
                let rich_text = cell_rich_text(value, text);
                let cell = label_cell(
                    ui,
                    width,
                    TABLE_ROW_HEIGHT,
                    rich_text,
                    Sense::click(),
                    row_fill(ui, visible_row_index, selected),
                    grid_stroke(ui),
                );
                let response = cell.response.on_hover_text(hover_cell_value(value));

                if selected {
                    paint_cell_outline(
                        ui,
                        cell.rect,
                        Stroke::new(1.5, ui.visuals().selection.stroke.color),
                    );
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
        if self.remote_page().is_some() {
            return self.sorted_rows();
        }

        let start = self.page_start();
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

    fn page_start(&self) -> usize {
        self.page.saturating_mul(self.page_size)
    }

    fn page_end(&self) -> usize {
        let rows = self.page_rows();
        if rows.is_empty() {
            0
        } else {
            self.page_start() + rows.len()
        }
    }

    fn sort_direction_for(&self, column: usize) -> Option<SortDirection> {
        match self.sort {
            Some(SortState {
                column: sort_column,
                direction,
            }) if sort_column == column => Some(direction),
            _ => None,
        }
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
        self.selected = None;
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
            .map(|column| column.name.clone())
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

    fn remote_page(&self) -> Option<RemotePage> {
        let limit = self
            .result
            .metadata
            .get(QUERY_RESULT_METADATA_PAGE_LIMIT)?
            .parse::<usize>()
            .ok()?;
        let offset = self
            .result
            .metadata
            .get(QUERY_RESULT_METADATA_PAGE_OFFSET)?
            .parse::<u64>()
            .ok()?;
        let total_rows = self
            .result
            .metadata
            .get(QUERY_RESULT_METADATA_TOTAL_ROWS)?
            .parse::<u64>()
            .ok()?;

        Some(RemotePage {
            limit,
            offset,
            total_rows,
        })
    }

    fn copy_export(
        &self,
        ui: &mut Ui,
        format: ExportFormat,
        action: &mut Option<ResultSetGridAction>,
    ) {
        match export_query_result(&self.result, format) {
            Ok(text) => {
                copy_to_clipboard(ui, text);
                *action = Some(ResultSetGridAction::Message(match format {
                    ExportFormat::Csv => "CSV export copied to clipboard".to_string(),
                    ExportFormat::Json => "JSON export copied to clipboard".to_string(),
                    ExportFormat::ExcelCompatible => {
                        "Excel-compatible export copied to clipboard".to_string()
                    }
                }));
            }
            Err(error) => {
                *action = Some(ResultSetGridAction::Message(format!(
                    "Export failed: {}",
                    error.message
                )));
            }
        }
    }

    fn save_export(&self, format: ExportFormat, action: &mut Option<ResultSetGridAction>) {
        let ext = match format {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::ExcelCompatible => "tsv",
        };
        let format_name = match format {
            ExportFormat::Csv => "CSV",
            ExportFormat::Json => "JSON",
            ExportFormat::ExcelCompatible => "Excel",
        };

        if let Some(rt) = &self.runtime {
            let rt = rt.clone();
            let format_label = format_name.to_string();
            let file_ext = ext.to_string();
            let path = rfd::FileDialog::new()
                .set_title(format!("Save as {}", format_label))
                .add_filter(&format_label, &[file_ext.as_str()])
                .save_file();

            if let Some(path) = path {
                let path_for_task = path.clone();
                let result = self.result.clone();
                let format_for_task = format;
                let format_label_for_task = format_label.clone();
                let display_path = path.display().to_string();
                rt.spawn(async move {
                    let write_result = tokio::task::spawn_blocking(move || {
                        export_query_result_to_file(&result, format_for_task, &path_for_task)
                    })
                    .await;

                    match write_result {
                        Ok(Ok(())) => {
                            tracing::info!(
                                "Exported {} to {}",
                                format_label_for_task,
                                display_path
                            );
                        }
                        Ok(Err(error)) => {
                            tracing::error!("Failed to export file: {}", error.message);
                        }
                        Err(error) => {
                            tracing::error!("Failed to join export task: {error}");
                        }
                    }
                });
                *action = Some(ResultSetGridAction::Message(format!(
                    "{} export started: {}",
                    format_label,
                    path.display()
                )));
            }
        } else {
            *action = Some(ResultSetGridAction::Message(format!(
                "{} export unavailable (runtime missing)",
                format_name
            )));
        }
    }
}

fn page_size_from_result(result: &QueryResult) -> usize {
    result
        .metadata
        .get(QUERY_RESULT_METADATA_PAGE_LIMIT)
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.clamp(25, 1000))
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

fn estimate_column_widths(result: &QueryResult) -> Vec<f32> {
    result
        .columns
        .iter()
        .enumerate()
        .map(|(column_index, column)| {
            let mut estimated = estimate_text_width(&column.name)
                .max(estimate_text_width(&column.database_type))
                .max(MIN_COLUMN_WIDTH);

            for row in result.rows.iter().take(200) {
                if let Some(value) = row.values.get(column_index) {
                    estimated = estimated.max(estimate_text_width(&display_cell_value(value)));
                }
            }

            estimated.clamp(MIN_COLUMN_WIDTH, INITIAL_MAX_COLUMN_WIDTH)
        })
        .collect()
}
