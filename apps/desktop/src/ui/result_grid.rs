#![allow(dead_code)]

use crate::ui::format::{
    cell_rich_text, compare_cell_values, copy_cell_value, copy_to_clipboard, display_cell_value,
    hover_cell_value,
};
use crate::ui::grid::{
    estimate_text_width, grid_stroke, header_fill, label_cell, paint_cell_outline,
    resizable_header_cell, row_fill,
};
use db_common::{CellValue, Column, QueryResult};
use eframe::egui::{
    Button, Color32, DragValue, Frame, Margin, RichText, ScrollArea, Sense, Stroke, Ui,
};
use import_export::{export_query_result, export_query_result_to_file, ExportFormat};
use std::cmp::Ordering;
use tokio::runtime::Handle;

const DEFAULT_PAGE_SIZE: usize = 100;
const MIN_COLUMN_WIDTH: f32 = 72.0;
const MAX_COLUMN_WIDTH: f32 = 640.0;
const INITIAL_MAX_COLUMN_WIDTH: f32 = 360.0;
const ROW_HEIGHT: f32 = 26.0;
const HEADER_HEIGHT: f32 = 32.0;
const ROW_NUMBER_WIDTH: f32 = 64.0;

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

pub struct ResultSetGrid {
    result: QueryResult,
    column_widths: Vec<f32>,
    page: usize,
    page_size: usize,
    selected: Option<SelectedCell>,
    sort: Option<SortState>,
    runtime: Option<Handle>,
}

impl ResultSetGrid {
    pub fn new(result: QueryResult) -> Self {
        Self {
            column_widths: estimate_column_widths(&result),
            result,
            page: 0,
            page_size: DEFAULT_PAGE_SIZE,
            selected: None,
            sort: None,
            runtime: None,
        }
    }

    pub fn with_runtime(mut self, runtime: Handle) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn set_result(&mut self, result: QueryResult) {
        self.column_widths = estimate_column_widths(&result);
        self.result = result;
        self.page = 0;
        self.page_size = DEFAULT_PAGE_SIZE;
        self.selected = None;
        self.sort = None;
    }

    pub fn result(&self) -> &QueryResult {
        &self.result
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<String> {
        let mut message = None;
        self.toolbar(ui, &mut message);
        ui.add_space(4.0);

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
            return message;
        }

        let page_rows = self.page_rows();

        Frame::default()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(grid_stroke(ui))
            .inner_margin(Margin::same(0.0))
            .show(ui, |ui| {
                ScrollArea::horizontal()
                    .id_source("result_set_horizontal")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.header(ui);
                        ui.separator();
                        ScrollArea::vertical()
                            .id_source("result_set_vertical")
                            .max_height(ui.available_height().max(240.0))
                            .auto_shrink([false, false])
                            .show_rows(ui, ROW_HEIGHT, page_rows.len(), |ui, row_range| {
                                for visible_row_index in row_range {
                                    let Some(row_index) = page_rows.get(visible_row_index).copied()
                                    else {
                                        continue;
                                    };
                                    self.row(ui, row_index, visible_row_index);
                                }
                            });
                    });
            });

        message
    }

    fn toolbar(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} rows", self.result.rows.len())).strong());
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

            ui.separator();
            let old_page_size = self.page_size;
            ui.add(
                DragValue::new(&mut self.page_size)
                    .speed(10)
                    .clamp_range(25..=1000),
            );
            if self.page_size != old_page_size {
                self.page_size = self.page_size.clamp(25, 1000);
                self.page = self.page.min(self.total_pages().saturating_sub(1));
                self.selected = None;
            }

            ui.separator();
            let has_selection = self.selected.is_some();
            ui.menu_button("Copy", |ui| {
                if ui.add_enabled(has_selection, Button::new("Cell")).clicked() {
                    if let Some(text) = self.copy_cell_text() {
                        copy_to_clipboard(ui, text);
                        *message = Some("Cell copied".to_string());
                    }
                    ui.close_menu();
                }
                if ui.add_enabled(has_selection, Button::new("Row")).clicked() {
                    if let Some(text) = self.copy_row_text(false) {
                        copy_to_clipboard(ui, text);
                        *message = Some("Row copied".to_string());
                    }
                    ui.close_menu();
                }
                if ui
                    .add_enabled(has_selection, Button::new("Row with Header"))
                    .clicked()
                {
                    if let Some(text) = self.copy_row_text(true) {
                        copy_to_clipboard(ui, text);
                        *message = Some("Row copied with header".to_string());
                    }
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Page with Header").clicked() {
                    copy_to_clipboard(ui, self.copy_rows_text(self.page_rows(), true));
                    *message = Some("Current page copied with header".to_string());
                    ui.close_menu();
                }
                if ui.button("All with Header").clicked() {
                    copy_to_clipboard(ui, self.copy_rows_text(self.sorted_rows(), true));
                    *message = Some("All rows copied with header".to_string());
                    ui.close_menu();
                }
            });

            ui.menu_button("Export", |ui| {
                if ui.button("Copy CSV").clicked() {
                    self.copy_export(ui, ExportFormat::Csv, message);
                    ui.close_menu();
                }
                if ui.button("Copy JSON").clicked() {
                    self.copy_export(ui, ExportFormat::Json, message);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Save CSV").clicked() {
                    self.save_export(ExportFormat::Csv, message);
                    ui.close_menu();
                }
                if ui.button("Save JSON").clicked() {
                    self.save_export(ExportFormat::Json, message);
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
        ui.horizontal(|ui| {
            label_cell(
                ui,
                ROW_NUMBER_WIDTH,
                HEADER_HEIGHT,
                RichText::new("#").strong(),
                Sense::hover(),
                header_fill(ui),
                grid_stroke(ui),
            );

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
                    HEADER_HEIGHT,
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

    fn row(&mut self, ui: &mut Ui, row_index: usize, visible_row_index: usize) {
        ui.horizontal(|ui| {
            let row_selected = self
                .selected
                .map(|selected| selected.row == row_index)
                .unwrap_or(false);
            let row_number = self.page_start() + visible_row_index + 1;

            let row_cell = label_cell(
                ui,
                ROW_NUMBER_WIDTH,
                ROW_HEIGHT,
                RichText::new(format!("{row_number}"))
                    .monospace()
                    .color(if row_selected {
                        Color32::WHITE
                    } else {
                        Color32::GRAY
                    }),
                Sense::click(),
                row_fill(ui, visible_row_index, row_selected),
                grid_stroke(ui),
            );
            if row_cell.response.clicked() {
                self.selected = Some(SelectedCell {
                    row: row_index,
                    column: 0,
                });
            }

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
                    ROW_HEIGHT,
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
        std::iter::once("#".to_string())
            .chain(
                self.result
                    .columns
                    .iter()
                    .map(|column: &Column| column.name.clone()),
            )
            .collect::<Vec<_>>()
            .join("\t")
    }

    fn copy_row_values(&self, row_index: usize) -> Option<String> {
        let row = self.result.rows.get(row_index)?;
        Some(
            std::iter::once((row_index + 1).to_string())
                .chain((0..self.result.columns.len()).map(|column_index| {
                    row.values
                        .get(column_index)
                        .map(copy_cell_value)
                        .unwrap_or_else(|| "NULL".to_string())
                }))
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

    fn save_export(&self, format: ExportFormat, message: &mut Option<String>) {
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
                *message = Some(format!(
                    "{} export started: {}",
                    format_label,
                    path.display()
                ));
            }
        } else {
            *message = Some(format!(
                "{} export unavailable (runtime missing)",
                format_name
            ));
        }
    }
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
