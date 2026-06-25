use crate::ui::format::{
    cell_rich_text, compare_cell_values, copy_cell_value, copy_to_clipboard, display_cell_value,
};
use db_common::{CellValue, Column, QueryResult};
use eframe::egui::{
    Align, Button, Color32, DragValue, Frame, Label, Layout, RichText, ScrollArea, Sense, Ui, Vec2,
};
use import_export::{export_query_result, ExportFormat};
use std::cmp::Ordering;

const DEFAULT_PAGE_SIZE: usize = 100;
const MIN_COLUMN_WIDTH: f32 = 72.0;
const MAX_COLUMN_WIDTH: f32 = 640.0;
const ROW_HEIGHT: f32 = 26.0;

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
}

impl ResultSetGrid {
    pub fn new(result: QueryResult) -> Self {
        let column_widths = result
            .columns
            .iter()
            .map(|column| {
                let estimated = (column.name.chars().count() as f32 * 8.0)
                    + (column.database_type.chars().count() as f32 * 7.0)
                    + 32.0;
                estimated.clamp(MIN_COLUMN_WIDTH, 220.0)
            })
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

    pub fn set_result(&mut self, result: QueryResult) {
        self.result = result;
        self.selected = None;
        self.sort = None;
        self.page = 0;
        self.column_widths = self
            .result
            .columns
            .iter()
            .map(|column| {
                let estimated = (column.name.chars().count() as f32 * 8.0)
                    + (column.database_type.chars().count() as f32 * 7.0)
                    + 32.0;
                estimated.clamp(MIN_COLUMN_WIDTH, 220.0)
            })
            .collect();
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

        Frame::group(ui.style()).show(ui, |ui| {
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
                        .show_rows(
                            ui,
                            ROW_HEIGHT,
                            self.page_rows().len(),
                            |ui, row_range| {
                                let page_rows = self.page_rows();
                                for visible_row_index in row_range {
                                    let Some(row_index) = page_rows.get(visible_row_index).copied()
                                    else {
                                        continue;
                                    };
                                    self.row(ui, row_index);
                                }
                            },
                        );
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
            if self.result.execution_time_ms > 0 {
                ui.separator();
                ui.label(
                    RichText::new(format!("{} ms", self.result.execution_time_ms))
                        .color(Color32::from_rgb(60, 140, 60)),
                );
            }
            ui.separator();
            ui.label(format!(
                "Page {} / {}",
                self.page + 1,
                self.total_pages().max(1)
            ));

            if ui
                .add_enabled(self.page > 0, Button::new("◀ Prev"))
                .clicked()
            {
                self.page -= 1;
            }
            if ui
                .add_enabled(self.page + 1 < self.total_pages(), Button::new("Next ▶"))
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
            ui.add_enabled_ui(has_selection, |ui| {
                if ui.button("Copy Cell").clicked() {
                    if let Some(text) = self.copy_cell_text() {
                        copy_to_clipboard(ui, text);
                        *message = Some("Cell copied".to_string());
                    }
                }
                if ui.button("Copy Row").clicked() {
                    if let Some(text) = self.copy_row_text(false) {
                        copy_to_clipboard(ui, text);
                        *message = Some("Row copied".to_string());
                    }
                }
                if ui.button("Copy Row + Header").clicked() {
                    if let Some(text) = self.copy_row_text(true) {
                        copy_to_clipboard(ui, text);
                        *message = Some("Row copied with header".to_string());
                    }
                }
            });
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
                }
            });
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for column_index in 0..self.result.columns.len() {
                let column = &self.result.columns[column_index];
                let column_name = column.name.clone();
                let column_hover = format!(
                    "{}{}{}",
                    column.database_type,
                    if column.nullable { " · nullable" } else { "" },
                    if column.primary_key { " · PK" } else { "" }
                );
                let width = self.column_widths[column_index];
                let sort_dir = self.sort_direction_for(column_index);

                ui.allocate_ui_with_layout(
                    Vec2::new(width, 56.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let marker = match sort_dir {
                            Some(SortDirection::Asc) => " ↑",
                            Some(SortDirection::Desc) => " ↓",
                            None => "",
                        };
                        let title = format!("{column_name}{marker}");
                        let title_btn = if sort_dir.is_some() {
                            Button::new(RichText::new(title).strong().color(Color32::from_rgb(
                                60,
                                100,
                                180,
                            )))
                        } else {
                            Button::new(RichText::new(title).strong())
                        };
                        if ui
                            .add_sized([width, 22.0], title_btn)
                            .on_hover_text(format!("{column_hover}\nClick to sort"))
                            .clicked()
                        {
                            self.toggle_sort(column_index);
                            self.page = 0;
                        }
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("↔")
                                    .color(Color32::GRAY)
                                    .size(10.0),
                            );
                            ui.add(
                                DragValue::new(&mut self.column_widths[column_index])
                                    .speed(2.0)
                                    .clamp_range(MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH),
                            );
                        });
                        ui.label(
                            RichText::new(&column.database_type)
                                .color(Color32::GRAY)
                                .size(10.0),
                        );
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
                            let label = Label::new(rich_text)
                                .sense(Sense::click())
                                .truncate(true);
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
            .map(|column: &Column| column.name.as_str())
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
