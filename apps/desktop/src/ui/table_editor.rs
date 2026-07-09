#![allow(dead_code)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

use crate::ui::format::{cell_rich_text, copy_cell_value, display_cell_value};
use crate::ui::grid::{
    estimate_text_width, grid_stroke, label_cell, paint_cell_frame, paint_cell_outline,
    resizable_header_cell, row_fill, text_edit_cell, GridCellResponse, TABLE_HEADER_HEIGHT,
    TABLE_ROW_HEIGHT,
};
use app_core::{metadata::MetadataService, query::QueryService};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use db_common::{
    CellValue, ConnectionId, Dialect, Identifier, LogicalType, QueryExecutionMode,
    QueryExecutionRequest, QueryId, QueryResult, Result, Row, TableColumn,
};
use eframe::egui::{
    pos2, vec2, Align, Button, Color32, DragValue, Frame, Key, Layout, Margin, Modifiers, Rect,
    Response, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, Window,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    sync::{Arc, Mutex},
};
use tokio::runtime::Handle;

const DEFAULT_PAGE_SIZE: usize = 500;
const MIN_COLUMN_WIDTH: f32 = 56.0;
const MAX_COLUMN_WIDTH: f32 = 640.0;
const INITIAL_MAX_COLUMN_WIDTH: f32 = 360.0;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CellKey {
    row: usize,
    column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RowKey {
    Existing(usize),
    New(usize),
}

#[derive(Debug, Clone)]
struct NewRow {
    values: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemporalKind {
    Date,
    Time,
    DateTime,
}

#[derive(Debug, Clone)]
struct TimePickerState {
    row: RowKey,
    column: usize,
    kind: TemporalKind,
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

#[derive(Debug, Clone)]
struct ChangeSummary {
    inserts: Vec<(usize, Vec<(String, String)>)>,
    updates: Vec<(usize, Vec<(String, String, String)>)>,
    deletes: Vec<(usize, Vec<(String, String)>)>,
}

impl ChangeSummary {
    fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.updates.is_empty() && self.deletes.is_empty()
    }

    fn total_changes(&self) -> usize {
        self.inserts.len() + self.updates.len() + self.deletes.len()
    }
}

/// Tracks the state of an async commit operation.
enum CommitState {
    /// No commit in progress.
    Idle,
    /// Commit task is running; the Arc<Mutex<...>> will receive the result.
    Committing(Arc<Mutex<Option<Result<db_common::QueryExecutionReport>>>>),
    /// Previous commit succeeded; will be reset on next poll.
    Success,
    /// Previous commit failed with a reason.
    Failed(String),
    /// Commit succeeded and refresh is running.
    Refreshing(Arc<Mutex<Option<Result<(Vec<TableColumn>, QueryResult)>>>>),
}

pub struct TableEditor {
    schema: Option<String>,
    table: String,
    columns: Vec<TableColumn>,
    result: QueryResult,
    dialect: Box<dyn Dialect>,
    column_widths: Vec<f32>,
    page: usize,
    page_size: usize,
    filter_column: usize,
    filter_text: String,
    sort: Option<SortState>,
    scroll_x: f32,
    edits: BTreeMap<CellKey, String>,
    new_rows: Vec<NewRow>,
    deleted_rows: BTreeMap<usize, Row>,
    selected_row: Option<RowKey>,
    selected_rows: BTreeSet<RowKey>,
    allow_risk_without_pk: bool,
    query_service: Option<QueryService>,
    metadata_service: Option<MetadataService>,
    connection_id: Option<ConnectionId>,
    database: Option<String>,
    runtime: Option<Handle>,
    show_commit_dialog: bool,
    show_close_warning: bool,
    time_picker: Option<TimePickerState>,
    commit_state: CommitState,
    commit_error: Option<String>,
    error_row_index: Option<usize>,
    last_commit_result: Option<(usize, Vec<usize>)>,
    pending_refresh_page: Option<usize>,
}

impl TableEditor {
    pub fn new(
        schema: impl Into<String>,
        table: impl Into<String>,
        columns: Vec<TableColumn>,
        result: QueryResult,
        dialect: Box<dyn Dialect>,
    ) -> Self {
        let column_widths = estimate_column_widths(&columns, &result);

        Self {
            schema: Some(schema.into()),
            table: table.into(),
            columns,
            result,
            dialect,
            column_widths,
            page: 0,
            page_size: DEFAULT_PAGE_SIZE,
            filter_column: 0,
            filter_text: String::new(),
            sort: None,
            scroll_x: 0.0,
            edits: BTreeMap::new(),
            new_rows: Vec::new(),
            deleted_rows: BTreeMap::new(),
            selected_row: None,
            selected_rows: BTreeSet::new(),
            allow_risk_without_pk: false,
            query_service: None,
            metadata_service: None,
            connection_id: None,
            database: None,
            runtime: None,
            show_commit_dialog: false,
            show_close_warning: false,
            time_picker: None,
            commit_state: CommitState::Idle,
            commit_error: None,
            error_row_index: None,
            last_commit_result: None,
            pending_refresh_page: None,
        }
    }

    pub fn with_services(
        schema: Option<String>,
        table: impl Into<String>,
        columns: Vec<TableColumn>,
        result: QueryResult,
        dialect: Box<dyn Dialect>,
        query_service: QueryService,
        metadata_service: MetadataService,
        connection_id: ConnectionId,
        database: Option<String>,
        runtime: Handle,
    ) -> Self {
        let mut editor = Self::new(
            schema.clone().unwrap_or_default(),
            table,
            columns,
            result,
            dialect,
        );
        editor.schema = schema;
        editor.query_service = Some(query_service);
        editor.metadata_service = Some(metadata_service);
        editor.connection_id = Some(connection_id);
        editor.database = database;
        editor.runtime = Some(runtime);
        editor
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<String> {
        let mut message = self.poll_commit();

        if self.show_commit_dialog {
            self.commit_dialog(ui, &mut message);
        }

        if self.show_close_warning {
            self.close_warning_dialog(ui);
        }

        self.time_picker_window(ui, &mut message);

        if let Some(error) = self.commit_error.clone() {
            ui.colored_label(Color32::RED, error);
            if ui.button("Clear Error").clicked() {
                self.commit_error = None;
                self.error_row_index = None;
            }
        }

        self.toolbar(ui, &mut message);
        self.handle_row_shortcuts(ui, &mut message);

        if !self.has_primary_key() && !self.allow_risk_without_pk {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "No primary key detected. Existing rows are read-only until risk mode is enabled.",
            );
        }

        let grid_height = (ui.available_height() - 30.0).max(0.0);
        let visible_rows = self.visible_rows();
        Frame::default()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(grid_stroke(ui))
            .inner_margin(Margin::same(0.0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                self.header(ui);
                let content_width = self.content_width();
                let output = if visible_rows.is_empty() {
                    ScrollArea::both()
                        .id_source("table_editor_body")
                        .max_height(grid_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_min_width(content_width);
                            self.empty_rows_body(ui, content_width, grid_height, &mut message);
                        })
                } else {
                    ScrollArea::both()
                        .id_source("table_editor_body")
                        .max_height(grid_height)
                        .auto_shrink([false, false])
                        .show_rows(ui, TABLE_ROW_HEIGHT, visible_rows.len(), |ui, row_range| {
                            ui.set_min_width(content_width);
                            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                            for visible_index in row_range {
                                let Some(row_key) = visible_rows.get(visible_index).copied() else {
                                    continue;
                                };
                                self.row(ui, row_key, visible_index, &visible_rows);
                            }
                        })
                };
                self.scroll_x = output.state.offset.x;
            });

        self.row_actions_bar(ui, &mut message);

        message
    }

    pub fn request_close(&mut self) -> bool {
        if self.has_changes() {
            self.show_close_warning = true;
            true
        } else {
            false
        }
    }

    /// Poll the async commit task. Returns a status message if the commit completed this frame.
    fn poll_commit(&mut self) -> Option<String> {
        // Take ownership of the current state to avoid borrowing conflicts.
        let state = std::mem::replace(&mut self.commit_state, CommitState::Idle);
        match state {
            CommitState::Idle => None,
            CommitState::Success => Some("Table changes committed successfully".to_string()),
            CommitState::Failed(msg) => {
                // Keep the failed state so the error remains visible.
                self.commit_state = CommitState::Failed(msg);
                None
            }
            CommitState::Committing(cell) => {
                let poll_result = if let Ok(mut guard) = cell.lock() {
                    guard.take()
                } else {
                    let msg = "Internal error: commit result lock poisoned".to_string();
                    self.commit_state = CommitState::Failed(msg.clone());
                    self.commit_error = Some(msg.clone());
                    return Some(msg);
                };

                if let Some(result) = poll_result {
                    match result {
                        Ok(report) => {
                            let affected = report.affected_rows;
                            self.last_commit_result = Some((affected as usize, Vec::new()));
                            self.pending_refresh_page = Some(self.page);
                            self.start_refresh_after_commit();
                            Some(format!(
                                "Commit successful — {} row(s) affected, refreshing...",
                                affected
                            ))
                        }
                        Err(e) => {
                            let msg = format!("Commit failed: {}", e.message);
                            self.commit_state = CommitState::Failed(msg.clone());
                            self.commit_error = Some(msg.clone());
                            Some(msg)
                        }
                    }
                } else {
                    // Still waiting — put the cell back.
                    self.commit_state = CommitState::Committing(cell);
                    None
                }
            }
            CommitState::Refreshing(cell) => {
                let poll_result = if let Ok(mut guard) = cell.lock() {
                    guard.take()
                } else {
                    let msg = "Internal error: refresh result lock poisoned".to_string();
                    self.commit_state = CommitState::Failed(msg.clone());
                    self.commit_error = Some(msg.clone());
                    return Some(msg);
                };

                if let Some(result) = poll_result {
                    match result {
                        Ok((columns, query_result)) => {
                            let table = self.table.clone();
                            let schema = self.schema.clone();
                            self.load_data(schema, table, columns, query_result);
                            Some("Table data refreshed after commit".to_string())
                        }
                        Err(error) => {
                            let msg =
                                format!("Commit succeeded but refresh failed: {}", error.message);
                            self.commit_state = CommitState::Failed(msg.clone());
                            self.commit_error = Some(msg.clone());
                            Some(msg)
                        }
                    }
                } else {
                    self.commit_state = CommitState::Refreshing(cell);
                    None
                }
            }
        }
    }

    /// Replace the current dataset with freshly loaded data from the backend.
    pub fn load_data(
        &mut self,
        schema: Option<String>,
        table: String,
        columns: Vec<TableColumn>,
        result: QueryResult,
    ) {
        self.column_widths = estimate_column_widths(&columns, &result);
        self.schema = schema;
        self.table = table;
        self.columns = columns;
        self.result = result;
        self.page = self
            .pending_refresh_page
            .take()
            .unwrap_or(0)
            .min(self.total_pages().saturating_sub(1));
        self.filter_text.clear();
        self.sort = None;
        self.scroll_x = 0.0;
        self.clear_changes();
        self.commit_state = CommitState::Idle;
        self.last_commit_result = None;
    }

    /// Return the fully qualified table name.
    pub fn table_name(&self) -> String {
        match self.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(schema) => format!("{}.{}", schema, self.table),
            None => self.table.clone(),
        }
    }

    /// Returns true when a commit is in flight and UI actions that mutate data should be disabled.
    fn is_committing(&self) -> bool {
        matches!(&self.commit_state, CommitState::Committing(_))
    }

    fn toolbar(&mut self, ui: &mut Ui, _message: &mut Option<String>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(self.table_name()).strong());
            ui.label(
                RichText::new(format!("{} rows", self.filtered_existing_rows().len()))
                    .small()
                    .color(Color32::GRAY),
            );
            ui.separator();
            if ui.add_enabled(self.page > 0, Button::new("Prev")).clicked() {
                self.page -= 1;
            }
            ui.label(format!("{} / {}", self.page + 1, self.total_pages().max(1)));
            if ui
                .add_enabled(self.page + 1 < self.total_pages(), Button::new("Next"))
                .clicked()
            {
                self.page += 1;
            }

            let old_page_size = self.page_size;
            ui.add(
                DragValue::new(&mut self.page_size)
                    .speed(10)
                    .clamp_range(25..=1000),
            );
            if old_page_size != self.page_size {
                self.page_size = self.page_size.clamp(25, 1000);
                self.page = self.page.min(self.total_pages().saturating_sub(1));
            }

            ui.separator();
            ui.label("Filter");
            let filter_options = (0..self.display_column_count())
                .map(|index| self.display_column_name(index))
                .collect::<Vec<_>>();
            if !filter_options.is_empty() {
                self.filter_column = self.filter_column.min(filter_options.len() - 1);
                let selected = filter_options
                    .get(self.filter_column)
                    .cloned()
                    .unwrap_or_default();
                eframe::egui::ComboBox::from_id_source("table_filter_column")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (index, label) in filter_options.iter().enumerate() {
                            ui.selectable_value(&mut self.filter_column, index, label);
                        }
                    });
            }
            let response = ui.add(
                TextEdit::singleline(&mut self.filter_text)
                    .desired_width(180.0)
                    .hint_text("contains"),
            );
            if response.changed() {
                self.page = 0;
            }
            if ui
                .add_enabled(!self.filter_text.is_empty(), Button::new("Clear"))
                .clicked()
            {
                self.filter_text.clear();
                self.page = 0;
            }

            if !self.has_primary_key() {
                ui.separator();
                ui.checkbox(&mut self.allow_risk_without_pk, "Risk mode");
            }
            if self.is_committing() {
                ui.colored_label(Color32::YELLOW, "Committing...");
            }
            let pending = self.pending_change_count();
            if pending > 0 {
                ui.label(
                    RichText::new(format!("{pending} pending"))
                        .small()
                        .color(Color32::from_rgb(70, 110, 190)),
                );
            }
        });
    }

    fn row_actions_bar(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.is_committing(), Button::new("+"))
                .on_hover_text("Append row")
                .clicked()
            {
                self.append_row(message);
            }

            let selected_rows = self.selected_rows.iter().copied().collect::<Vec<_>>();
            let selected_count = selected_rows.len();
            let can_remove = !self.is_committing()
                && selected_rows
                    .iter()
                    .any(|row| self.can_remove_or_restore_row(*row));
            let delete_label = if selected_count > 1 {
                format!("- Rows ({selected_count})")
            } else {
                "- Row".to_string()
            };
            if ui
                .add_enabled(can_remove, Button::new(delete_label))
                .on_hover_text("Delete selected row(s), restore delete mark, or remove staged rows")
                .clicked()
            {
                self.remove_or_restore_rows(selected_rows, message);
            }

            ui.separator();
            if ui
                .add_enabled(
                    self.has_changes() && !self.is_committing(),
                    Button::new("Commit"),
                )
                .clicked()
            {
                self.show_commit_dialog = true;
            }
            if ui
                .add_enabled(
                    self.has_changes() && !self.is_committing(),
                    Button::new("Rollback"),
                )
                .clicked()
            {
                self.clear_changes();
                self.last_commit_result = None;
                *message = Some("Uncommitted table changes rolled back".to_string());
            }

            ui.separator();
            let selected_label = if selected_count > 1 {
                format!("{selected_count} rows selected")
            } else {
                self.selected_row
                    .map(|row| self.row_label(row))
                    .unwrap_or_else(|| "No row selected".to_string())
            };
            ui.label(RichText::new(selected_label).small().color(Color32::GRAY));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pending = self.pending_change_count();
                if pending > 0 {
                    ui.label(
                        RichText::new(format!("{pending} pending"))
                            .small()
                            .color(Color32::from_rgb(70, 110, 190)),
                    );
                }
            });
        });
    }

    fn append_row(&mut self, message: &mut Option<String>) {
        let index = self.new_rows.len();
        let row = RowKey::New(index);
        self.new_rows.push(NewRow {
            values: vec![String::new(); self.display_column_count()],
        });
        self.selected_row = Some(row);
        self.selected_rows.clear();
        self.selected_rows.insert(row);
        *message = Some("New row staged".to_string());
    }

    fn can_remove_or_restore_row(&self, row: RowKey) -> bool {
        match row {
            RowKey::Existing(row_index) => {
                self.can_edit_existing_rows() || self.deleted_rows.contains_key(&row_index)
            }
            RowKey::New(row_index) => row_index < self.new_rows.len(),
        }
    }

    fn row_is_deleted(&self, row: RowKey) -> bool {
        matches!(row, RowKey::Existing(index) if self.deleted_rows.contains_key(&index))
    }

    fn remove_or_restore_rows(&mut self, rows: Vec<RowKey>, message: &mut Option<String>) {
        if rows.is_empty() {
            return;
        }

        let mut marked = 0usize;
        let mut restored = 0usize;
        let mut new_rows_to_remove = Vec::new();

        for row in rows {
            match row {
                RowKey::Existing(row_index) => {
                    if self.deleted_rows.remove(&row_index).is_some() {
                        restored += 1;
                    } else if self.can_edit_existing_rows() {
                        if let Some(row) = self.result.rows.get(row_index).cloned() {
                            self.deleted_rows.insert(row_index, row);
                            marked += 1;
                        }
                    }
                }
                RowKey::New(row_index) => {
                    if row_index < self.new_rows.len() {
                        new_rows_to_remove.push(row_index);
                    }
                }
            }
        }

        new_rows_to_remove.sort_unstable();
        new_rows_to_remove.dedup();
        for row_index in new_rows_to_remove.iter().rev() {
            self.new_rows.remove(*row_index);
        }
        if !new_rows_to_remove.is_empty() {
            self.selected_row = None;
            self.selected_rows.clear();
        }

        let removed = new_rows_to_remove.len();
        let mut parts = Vec::new();
        if marked > 0 {
            parts.push(format!("{marked} marked for delete"));
        }
        if restored > 0 {
            parts.push(format!("{restored} delete mark removed"));
        }
        if removed > 0 {
            parts.push(format!("{removed} staged row removed"));
        }
        if !parts.is_empty() {
            *message = Some(parts.join(", "));
        }
    }

    fn display_column_count(&self) -> usize {
        if self.result.columns.is_empty() {
            self.columns.len()
        } else {
            self.result.columns.len()
        }
    }

    fn display_column_name(&self, index: usize) -> String {
        self.result
            .columns
            .get(index)
            .map(|column| column.name.clone())
            .or_else(|| self.columns.get(index).map(|column| column.name.clone()))
            .unwrap_or_else(|| format!("column_{}", index + 1))
    }

    fn metadata_column_for_display_index(&self, index: usize) -> Option<&TableColumn> {
        let display_name = self.display_column_name(index);
        self.columns
            .iter()
            .find(|column| column.name == display_name)
            .or_else(|| self.columns.get(index))
    }

    fn primary_key_display_indices(&self) -> Vec<usize> {
        (0..self.display_column_count())
            .filter(|index| {
                self.metadata_column_for_display_index(*index)
                    .map(|column| column.primary_key)
                    .unwrap_or(false)
            })
            .collect()
    }

    fn display_column_hover(&self, index: usize) -> String {
        let metadata = self.metadata_column_for_display_index(index);
        let database_type = metadata
            .map(|column| column.database_type.as_str())
            .or_else(|| {
                self.result
                    .columns
                    .get(index)
                    .map(|column| column.database_type.as_str())
            })
            .unwrap_or("");
        let nullable = metadata.map(|column| column.nullable).unwrap_or(true);
        let primary_key = metadata.map(|column| column.primary_key).unwrap_or(false);
        format!(
            "{}{}{}",
            database_type,
            if nullable { ", nullable" } else { "" },
            if primary_key { ", primary key" } else { "" }
        )
    }

    fn row_label(&self, row: RowKey) -> String {
        match row {
            RowKey::Existing(index) => {
                if self.deleted_rows.contains_key(&index) {
                    format!("{}-", index + 1)
                } else {
                    (index + 1).to_string()
                }
            }
            RowKey::New(index) => format!("*{}", index + 1),
        }
    }

    fn handle_row_shortcuts(&mut self, ui: &Ui, message: &mut Option<String>) {
        let has_text_focus = ui.memory(|memory| memory.focused().is_some());
        if has_text_focus || self.is_committing() {
            return;
        }

        let input = ui.input(|input| {
            (
                input.key_pressed(Key::Insert),
                input.key_pressed(Key::Delete),
                input.key_pressed(Key::A),
                input.modifiers,
            )
        });
        let (insert_pressed, delete_pressed, a_pressed, modifiers) = input;

        if insert_pressed {
            self.append_row(message);
        }
        if delete_pressed {
            let rows = self.selected_rows.iter().copied().collect::<Vec<_>>();
            self.remove_or_restore_rows(rows, message);
        }
        if a_pressed && command_modifier(modifiers) {
            self.selected_rows = self.visible_rows().into_iter().collect();
            self.selected_row = self.selected_rows.iter().next_back().copied();
            *message = Some(format!("{} rows selected", self.selected_rows.len()));
        }
    }

    fn select_row(&mut self, row_key: RowKey, visible_rows: &[RowKey], modifiers: Modifiers) {
        if modifiers.shift {
            if let Some(anchor) = self.selected_row {
                if let (Some(start), Some(end)) = (
                    visible_rows.iter().position(|row| *row == anchor),
                    visible_rows.iter().position(|row| *row == row_key),
                ) {
                    let (start, end) = if start <= end {
                        (start, end)
                    } else {
                        (end, start)
                    };
                    self.selected_rows.clear();
                    for row in &visible_rows[start..=end] {
                        self.selected_rows.insert(*row);
                    }
                    self.selected_row = Some(row_key);
                    return;
                }
            }
        }

        if command_modifier(modifiers) {
            if !self.selected_rows.remove(&row_key) {
                self.selected_rows.insert(row_key);
                self.selected_row = Some(row_key);
            } else if self.selected_row == Some(row_key) {
                self.selected_row = self.selected_rows.iter().next_back().copied();
            }
            return;
        }

        self.selected_rows.clear();
        self.selected_rows.insert(row_key);
        self.selected_row = Some(row_key);
    }

    fn commit_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        let summary = self.build_change_summary();

        let mut open = true;
        Window::new("Commit Changes")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .min_width(500.0)
            .min_height(400.0)
            .show(ui.ctx(), |ui| {
                ui.heading(format!("{} pending changes", summary.total_changes()));
                ui.add_space(16.0);

                ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    if !summary.inserts.is_empty() {
                        ui.label(
                            RichText::new(format!("📥 {} INSERT(s)", summary.inserts.len()))
                                .strong(),
                        );
                        for (idx, row) in &summary.inserts {
                            ui.label(format!("  Row {}", idx + 1));
                            for (col, val) in row {
                                ui.label(format!("    {} = {}", col, val));
                            }
                        }
                        ui.add_space(8.0);
                    }

                    if !summary.updates.is_empty() {
                        ui.label(
                            RichText::new(format!("🔄 {} UPDATE(s)", summary.updates.len()))
                                .strong(),
                        );
                        for (idx, changes) in &summary.updates {
                            ui.label(format!("  Row {}", idx + 1));
                            for (col, old_val, new_val) in changes {
                                ui.label(format!("    {}: {} -> {}", col, old_val, new_val));
                            }
                        }
                        ui.add_space(8.0);
                    }

                    if !summary.deletes.is_empty() {
                        ui.label(
                            RichText::new(format!("🗑 {} DELETE(s)", summary.deletes.len()))
                                .strong(),
                        );
                        for (idx, keys) in &summary.deletes {
                            ui.label(format!("  Row {}", idx + 1));
                            for (col, val) in keys {
                                ui.label(format!("    {} = {}", col, val));
                            }
                        }
                    }
                });

                ui.add_space(16.0);

                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.show_commit_dialog = false;
                    }
                    if ui
                        .add_enabled(self.can_commit(), Button::new("Commit"))
                        .clicked()
                    {
                        self.execute_commit(message);
                        self.show_commit_dialog = false;
                    }
                });
            });

        if !open {
            self.show_commit_dialog = false;
        }
    }

    fn close_warning_dialog(&mut self, ui: &mut Ui) {
        let mut open = true;
        Window::new("Close Confirmation")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(format!(
                    "There are {} pending changes. Close without saving?",
                    self.pending_change_count()
                ));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.show_close_warning = false;
                    }
                    if ui.button("Close Anyway").clicked() {
                        self.clear_changes();
                        self.show_close_warning = false;
                    }
                });
            });

        if !open {
            self.show_close_warning = false;
        }
    }

    fn execute_commit(&mut self, message: &mut Option<String>) {
        if self.query_service.is_none() || self.connection_id.is_none() {
            *message = Some("Query service not configured".to_string());
            return;
        }

        let sql = match self.build_commit_sql() {
            Ok(sql) => sql,
            Err(e) => {
                *message = Some(e);
                return;
            }
        };

        let service = self.query_service.clone().unwrap();
        let connection_id = self.connection_id.unwrap();
        let runtime = self.runtime.clone().unwrap();
        let insert_count = self.new_rows.len();

        let result_cell: Arc<Mutex<Option<Result<db_common::QueryExecutionReport>>>> =
            Arc::new(Mutex::new(None));
        let result_for_task = result_cell.clone();

        runtime.spawn(async move {
            let result = service
                .execute(
                    db_common::QueryExecutionRequest {
                        tab_id: QueryId::new(),
                        connection_id,
                        sql,
                        mode: db_common::QueryExecutionMode::All,
                        cursor_offset: 0,
                        selection: None,
                        timeout_ms: 30000,
                    },
                    None,
                )
                .await;

            match result {
                Ok(report) => {
                    let mut success = true;
                    let mut error_msg = String::new();

                    for (idx, stmt_result) in report.results.iter().enumerate() {
                        let is_insert = idx < insert_count;
                        if stmt_result.affected_rows == 0 {
                            success = false;
                            let label = if is_insert { "INSERT" } else { "Statement" };
                            write!(
                                &mut error_msg,
                                "{} {} affected 0 rows (conflict?); ",
                                label,
                                idx + 1
                            )
                            .unwrap();
                        } else if stmt_result.affected_rows > 1 && !is_insert {
                            success = false;
                            write!(
                                &mut error_msg,
                                "Statement {} affected {} rows (danger); ",
                                idx + 1,
                                stmt_result.affected_rows
                            )
                            .unwrap();
                        }
                    }

                    if success {
                        *result_for_task.lock().unwrap() = Some(Ok(report));
                    } else {
                        *result_for_task.lock().unwrap() =
                            Some(Err(db_common::AppError::validation(error_msg)));
                    }
                }
                Err(e) => {
                    *result_for_task.lock().unwrap() = Some(Err(e));
                }
            }
        });

        self.commit_state = CommitState::Committing(result_cell);
    }

    fn start_refresh_after_commit(&mut self) {
        let Some(query_service) = self.query_service.clone() else {
            return;
        };
        let Some(metadata_service) = self.metadata_service.clone() else {
            return;
        };
        let Some(connection_id) = self.connection_id else {
            return;
        };
        let Some(runtime) = self.runtime.clone() else {
            return;
        };

        let database = self.database.clone();
        let schema = self.schema.clone();
        let table = self.table.clone();
        let qualified = match schema.as_deref().filter(|value| !value.is_empty()) {
            Some(schema_name) => {
                format!(
                    "{}.{}",
                    self.dialect
                        .quote_identifier(&Identifier::new(schema_name).unwrap()),
                    self.dialect
                        .quote_identifier(&Identifier::new(&table).unwrap()),
                )
            }
            None => self
                .dialect
                .quote_identifier(&Identifier::new(&table).unwrap()),
        };
        let sql = format!("SELECT * FROM {} LIMIT 500", qualified);

        let cell = Arc::new(Mutex::new(None::<Result<(Vec<TableColumn>, QueryResult)>>));
        let cell_for_task = cell.clone();

        runtime.spawn(async move {
            let columns_future = metadata_service.table_columns(
                connection_id,
                database.as_deref(),
                schema.as_deref(),
                &table,
                None,
            );
            let query_future = query_service.execute(
                QueryExecutionRequest {
                    tab_id: QueryId::new(),
                    connection_id,
                    sql,
                    mode: QueryExecutionMode::All,
                    cursor_offset: 0,
                    selection: None,
                    timeout_ms: 30_000,
                },
                None,
            );
            let result = match tokio::join!(columns_future, query_future) {
                (Ok(cols), Ok(report)) => Ok((
                    cols,
                    report
                        .results
                        .into_iter()
                        .next()
                        .unwrap_or(QueryResult::empty("")),
                )),
                (Err(error), _) => Err(error),
                (_, Err(error)) => Err(error),
            };
            *cell_for_task.lock().unwrap() = Some(result);
        });
        self.commit_state = CommitState::Refreshing(cell);
    }

    fn build_change_summary(&self) -> ChangeSummary {
        let mut summary = ChangeSummary {
            inserts: Vec::new(),
            updates: Vec::new(),
            deletes: Vec::new(),
        };

        for (idx, row) in self.new_rows.iter().enumerate() {
            let mut values = Vec::new();
            for (col_idx, val) in row.values.iter().enumerate() {
                if !val.trim().is_empty() {
                    values.push((self.display_column_name(col_idx), val.clone()));
                }
            }
            if !values.is_empty() {
                summary.inserts.push((idx, values));
            }
        }

        let mut by_row: BTreeMap<usize, Vec<(usize, String)>> = BTreeMap::new();
        for (key, value) in &self.edits {
            by_row
                .entry(key.row)
                .or_default()
                .push((key.column, value.clone()));
        }

        for (row_index, changes) in by_row {
            let mut updates = Vec::new();
            for (col_idx, new_val) in changes {
                let original = self
                    .result
                    .rows
                    .get(row_index)
                    .and_then(|row| row.values.get(col_idx))
                    .unwrap_or(&CellValue::Null);
                let old_val = copy_cell_value(original);
                if old_val != new_val {
                    updates.push((self.display_column_name(col_idx), old_val, new_val));
                }
            }
            if !updates.is_empty() {
                summary.updates.push((row_index, updates));
            }
        }

        for (row_index, row) in &self.deleted_rows {
            let mut keys = Vec::new();
            let key_indices = self.primary_key_display_indices();
            let indices = if key_indices.is_empty() {
                if self.has_primary_key() {
                    continue;
                }
                (0..self.display_column_count()).collect::<Vec<_>>()
            } else {
                key_indices
            };

            for col_idx in indices {
                let val = row.values.get(col_idx).unwrap_or(&CellValue::Null);
                keys.push((self.display_column_name(col_idx), copy_cell_value(val)));
            }
            summary.deletes.push((*row_index, keys));
        }

        summary
    }

    fn can_commit(&self) -> bool {
        if !self.has_changes() {
            return false;
        }
        if !self.can_edit_existing_rows()
            && (!self.edits.is_empty() || !self.deleted_rows.is_empty())
        {
            return false;
        }
        true
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
            for column_index in 0..self.display_column_count() {
                let column_name = self.display_column_name(column_index);
                let column_hover = self.display_column_hover(column_index);
                let title = format!("{}{}", column_name, self.sort_marker(column_index));
                let header = resizable_header_cell(
                    ui,
                    &mut self.column_widths[column_index],
                    TABLE_HEADER_HEIGHT,
                    RichText::new(title).strong(),
                    format!("{column_hover}\nClick to sort"),
                    ui.make_persistent_id(("table_column_resize", column_index)),
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

    fn empty_rows_body(
        &mut self,
        ui: &mut Ui,
        content_width: f32,
        grid_height: f32,
        message: &mut Option<String>,
    ) {
        let height = grid_height.max(TABLE_ROW_HEIGHT * 6.0);
        let (rect, response) = ui.allocate_exact_size(
            vec2(content_width.max(ui.available_width()), height),
            Sense::click(),
        );
        ui.painter().rect_filled(rect, 0.0, row_fill(ui, 0, false));
        ui.painter()
            .rect_stroke(rect.shrink(0.5), 0.0, grid_stroke(ui));
        response.context_menu(|ui| {
            if ui.button("Insert Row").clicked() {
                self.append_row(message);
                ui.close_menu();
            }
        });
    }

    fn row(&mut self, ui: &mut Ui, row_key: RowKey, visible_index: usize, visible_rows: &[RowKey]) {
        let deleted =
            matches!(row_key, RowKey::Existing(index) if self.deleted_rows.contains_key(&index));
        let row_selected = self.selected_rows.contains(&row_key);

        let is_error_row = self.error_row_index
            == Some(match row_key {
                RowKey::Existing(i) => i,
                RowKey::New(i) => self.result.rows.len() + i,
            });

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

            if is_error_row {
                ui.painter().rect_stroke(
                    ui.available_rect_before_wrap(),
                    2.0,
                    Stroke::new(2.0, Color32::RED),
                );
            }

            let mut append_requested = false;
            let mut remove_requested = false;
            let mut clear_selection_requested = false;

            for column_index in 0..self.display_column_count() {
                let width = self.column_widths[column_index];
                let editable = match row_key {
                    RowKey::Existing(_) => self.can_edit_existing_rows() && !deleted,
                    RowKey::New(_) => true,
                };

                let cell = match row_key {
                    RowKey::Existing(row_index) => self.existing_cell(
                        ui,
                        row_index,
                        column_index,
                        width,
                        editable,
                        deleted,
                        row_selected,
                        visible_index,
                    ),
                    RowKey::New(row_index) => self.new_cell(
                        ui,
                        row_index,
                        column_index,
                        width,
                        row_selected,
                        visible_index,
                    ),
                };
                self.handle_row_cell_response(
                    ui,
                    &cell.response,
                    row_key,
                    visible_rows,
                    &mut append_requested,
                    &mut remove_requested,
                    &mut clear_selection_requested,
                );
            }

            if append_requested {
                let mut ignored = None;
                self.append_row(&mut ignored);
            }
            if remove_requested {
                let mut ignored = None;
                let rows = self.selected_rows.iter().copied().collect::<Vec<_>>();
                self.remove_or_restore_rows(rows, &mut ignored);
            }
            if clear_selection_requested {
                self.selected_row = None;
                self.selected_rows.clear();
            }
        });
    }

    fn handle_row_cell_response(
        &mut self,
        ui: &Ui,
        response: &Response,
        row_key: RowKey,
        visible_rows: &[RowKey],
        append_requested: &mut bool,
        remove_requested: &mut bool,
        clear_selection_requested: &mut bool,
    ) {
        if response.clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            self.select_row(row_key, visible_rows, modifiers);
        }

        response.context_menu(|ui| {
            if !self.selected_rows.contains(&row_key) {
                self.selected_rows.clear();
                self.selected_rows.insert(row_key);
                self.selected_row = Some(row_key);
            }

            if ui.button("Insert Row").clicked() {
                *append_requested = true;
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    self.can_remove_or_restore_row(row_key),
                    Button::new(if self.row_is_deleted(row_key) {
                        "Undo Delete"
                    } else {
                        "Delete Row"
                    }),
                )
                .clicked()
            {
                *remove_requested = true;
                ui.close_menu();
            }
            if self.selected_rows.len() > 1 && ui.button("Delete Selected Rows").clicked() {
                *remove_requested = true;
                ui.close_menu();
            }
            if ui.button("Clear Selection").clicked() {
                *clear_selection_requested = true;
                ui.close_menu();
            }
        });
    }

    fn existing_cell(
        &mut self,
        ui: &mut Ui,
        row_index: usize,
        column_index: usize,
        width: f32,
        editable: bool,
        deleted: bool,
        row_selected: bool,
        visible_index: usize,
    ) -> GridCellResponse {
        let key = CellKey {
            row: row_index,
            column: column_index,
        };
        let original = self
            .result
            .rows
            .get(row_index)
            .and_then(|row| row.values.get(column_index))
            .unwrap_or(&CellValue::Null);
        let original_text = copy_cell_value(original);

        if editable {
            let mut value = self
                .edits
                .get(&key)
                .cloned()
                .unwrap_or_else(|| original_text.clone());
            let cell = if self.temporal_kind_for_column(column_index).is_some() {
                self.temporal_edit_cell(
                    ui,
                    width,
                    &mut value,
                    RowKey::Existing(row_index),
                    column_index,
                    table_cell_fill(ui, visible_index, deleted, row_selected),
                )
            } else {
                text_edit_cell(
                    ui,
                    width,
                    TABLE_ROW_HEIGHT,
                    &mut value,
                    table_cell_fill(ui, visible_index, deleted, row_selected),
                    grid_stroke(ui),
                )
            };
            if cell.response.changed() {
                if value == original_text {
                    self.edits.remove(&key);
                } else {
                    self.edits.insert(key, value);
                }
            }
            if self.edits.contains_key(&key) {
                paint_cell_outline(ui, cell.rect, Stroke::new(1.0, Color32::YELLOW));
            }
            cell
        } else {
            let mut rich = cell_rich_text(original, display_cell_value(original));
            if deleted {
                rich = rich.strikethrough();
            }
            let cell = label_cell(
                ui,
                width,
                TABLE_ROW_HEIGHT,
                rich,
                Sense::click(),
                table_cell_fill(ui, visible_index, deleted, row_selected),
                grid_stroke(ui),
            );
            GridCellResponse {
                response: cell.response.on_hover_text(copy_cell_value(original)),
                rect: cell.rect,
            }
        }
    }

    fn new_cell(
        &mut self,
        ui: &mut Ui,
        row_index: usize,
        column_index: usize,
        width: f32,
        row_selected: bool,
        visible_index: usize,
    ) -> GridCellResponse {
        let current_value = self
            .new_rows
            .get(row_index)
            .and_then(|row| row.values.get(column_index))
            .cloned()
            .unwrap_or_default();

        let mut value = current_value.clone();
        let cell = if self.temporal_kind_for_column(column_index).is_some() {
            self.temporal_edit_cell(
                ui,
                width,
                &mut value,
                RowKey::New(row_index),
                column_index,
                table_cell_fill(ui, visible_index, false, row_selected),
            )
        } else {
            text_edit_cell(
                ui,
                width,
                TABLE_ROW_HEIGHT,
                &mut value,
                table_cell_fill(ui, visible_index, false, row_selected),
                grid_stroke(ui),
            )
        };

        if cell.response.changed() && value != current_value {
            if let Some(row) = self.new_rows.get_mut(row_index) {
                if let Some(target) = row.values.get_mut(column_index) {
                    *target = value;
                }
            }
        }

        cell
    }

    fn temporal_edit_cell(
        &mut self,
        ui: &mut Ui,
        width: f32,
        value: &mut String,
        row: RowKey,
        column_index: usize,
        fill: Color32,
    ) -> GridCellResponse {
        let (rect, background_response) =
            ui.allocate_exact_size(vec2(width, TABLE_ROW_HEIGHT), Sense::hover());
        paint_cell_frame(ui, rect, fill, grid_stroke(ui));

        let button_width = 16.0;
        let edit_rect = Rect::from_min_max(
            rect.left_top() + vec2(crate::ui::grid::CELL_PADDING_X, 0.0),
            rect.right_bottom() + vec2(-(button_width + crate::ui::grid::CELL_PADDING_X), 0.0),
        );
        let button_rect = Rect::from_min_max(
            rect.right_top() + vec2(-button_width, 0.0),
            rect.right_bottom(),
        );

        let mut edit_ui = ui.child_ui(edit_rect, Layout::left_to_right(Align::Center));
        edit_ui.set_clip_rect(edit_rect);
        edit_ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        edit_ui.spacing_mut().interact_size = vec2(0.0, TABLE_ROW_HEIGHT);
        let edit_response = edit_ui.add_sized(
            [edit_rect.width(), TABLE_ROW_HEIGHT],
            TextEdit::singleline(value)
                .desired_width(edit_rect.width())
                .frame(false),
        );

        let button_response = ui
            .allocate_ui_at_rect(button_rect, |ui| {
                ui.add_sized(
                    [button_rect.width(), button_rect.height()],
                    Button::new("..."),
                )
            })
            .inner
            .on_hover_text("Choose date/time");

        if button_response.clicked() {
            if let Some(kind) = self.temporal_kind_for_column(column_index) {
                self.time_picker =
                    Some(TimePickerState::from_value(row, column_index, kind, value));
            }
        }

        if edit_response.has_focus() {
            paint_cell_outline(
                ui,
                rect,
                Stroke::new(1.5, ui.visuals().selection.stroke.color),
            );
        }

        GridCellResponse {
            response: edit_response
                .union(button_response)
                .union(background_response),
            rect,
        }
    }

    fn time_picker_window(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        let Some(mut picker) = self.time_picker.take() else {
            return;
        };

        let title = match picker.kind {
            TemporalKind::Date => "Choose Date",
            TemporalKind::Time => "Choose Time",
            TemporalKind::DateTime => "Choose Date/Time",
        };
        let mut open = true;
        let mut apply_value = None;
        let mut set_null = false;
        let mut close_requested = false;

        Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                if matches!(picker.kind, TemporalKind::Date | TemporalKind::DateTime) {
                    ui.horizontal(|ui| {
                        ui.label("Date");
                        ui.add(DragValue::new(&mut picker.year).speed(1));
                        ui.add(DragValue::new(&mut picker.month).clamp_range(1..=12));
                        let max_day = days_in_month(picker.year, picker.month);
                        picker.day = picker.day.clamp(1, max_day);
                        ui.add(DragValue::new(&mut picker.day).clamp_range(1..=max_day));
                    });
                }

                if matches!(picker.kind, TemporalKind::Time | TemporalKind::DateTime) {
                    ui.horizontal(|ui| {
                        ui.label("Time");
                        ui.add(DragValue::new(&mut picker.hour).clamp_range(0..=23));
                        ui.add(DragValue::new(&mut picker.minute).clamp_range(0..=59));
                        ui.add(DragValue::new(&mut picker.second).clamp_range(0..=59));
                    });
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Now").clicked() {
                        picker.set_now();
                    }
                    if ui.button("NULL").clicked() {
                        set_null = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close_requested = true;
                    }
                    if ui.button("Apply").clicked() {
                        apply_value = Some(picker.format_value());
                    }
                });
            });

        if close_requested {
            open = false;
        }
        if set_null {
            apply_value = Some("NULL".to_string());
        }
        if let Some(value) = apply_value {
            self.set_cell_text(picker.row, picker.column, value);
            *message = Some("Cell date/time updated".to_string());
        } else if open {
            self.time_picker = Some(picker);
        }
    }

    fn set_cell_text(&mut self, row: RowKey, column: usize, value: String) {
        match row {
            RowKey::Existing(row_index) => {
                let key = CellKey {
                    row: row_index,
                    column,
                };
                let original = self
                    .result
                    .rows
                    .get(row_index)
                    .and_then(|row| row.values.get(column))
                    .map(copy_cell_value)
                    .unwrap_or_else(|| "NULL".to_string());
                if value == original {
                    self.edits.remove(&key);
                } else {
                    self.edits.insert(key, value);
                }
            }
            RowKey::New(row_index) => {
                if let Some(row) = self.new_rows.get_mut(row_index) {
                    if let Some(target) = row.values.get_mut(column) {
                        *target = value;
                    }
                }
            }
        }
    }

    fn temporal_kind_for_column(&self, column_index: usize) -> Option<TemporalKind> {
        match self
            .result
            .columns
            .get(column_index)
            .map(|column| column.logical_type.clone())
        {
            Some(LogicalType::Date) => return Some(TemporalKind::Date),
            Some(LogicalType::Time) => return Some(TemporalKind::Time),
            Some(LogicalType::DateTime) => return Some(TemporalKind::DateTime),
            _ => {}
        }

        let db_type = self
            .metadata_column_for_display_index(column_index)
            .map(|column| column.database_type.to_ascii_lowercase())
            .or_else(|| {
                self.result
                    .columns
                    .get(column_index)
                    .map(|column| column.database_type.to_ascii_lowercase())
            })
            .unwrap_or_default();

        if db_type.contains("timestamp") || db_type.contains("datetime") {
            Some(TemporalKind::DateTime)
        } else if db_type.contains("date") {
            Some(TemporalKind::Date)
        } else if db_type.contains("time") {
            Some(TemporalKind::Time)
        } else {
            None
        }
    }

    fn visible_rows(&self) -> Vec<RowKey> {
        let mut rows = self.filtered_existing_rows();
        if let Some(sort) = self.sort {
            rows.sort_by(|left, right| self.compare_rows(*left, *right, sort));
        }

        let start = self.page.saturating_mul(self.page_size);
        rows.into_iter()
            .skip(start)
            .take(self.page_size)
            .map(RowKey::Existing)
            .chain((0..self.new_rows.len()).map(RowKey::New))
            .collect()
    }

    fn filtered_existing_rows(&self) -> Vec<usize> {
        let filter = self.filter_text.trim().to_ascii_lowercase();
        (0..self.result.rows.len())
            .filter(|row_index| {
                if filter.is_empty() {
                    return true;
                }
                self.result
                    .rows
                    .get(*row_index)
                    .and_then(|row| row.values.get(self.filter_column))
                    .map(copy_cell_value)
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&filter)
            })
            .collect()
    }

    fn total_pages(&self) -> usize {
        self.filtered_existing_rows()
            .len()
            .div_ceil(self.page_size.max(1))
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

    fn compare_rows(&self, left: usize, right: usize, sort: SortState) -> std::cmp::Ordering {
        let left_value = self
            .result
            .rows
            .get(left)
            .and_then(|row| row.values.get(sort.column))
            .map(copy_cell_value)
            .unwrap_or_default();
        let right_value = self
            .result
            .rows
            .get(right)
            .and_then(|row| row.values.get(sort.column))
            .map(copy_cell_value)
            .unwrap_or_default();
        let ordering = left_value.cmp(&right_value);

        match sort.direction {
            SortDirection::Asc => ordering,
            SortDirection::Desc => ordering.reverse(),
        }
    }

    fn can_edit_existing_rows(&self) -> bool {
        self.has_primary_key() || self.allow_risk_without_pk
    }

    fn has_primary_key(&self) -> bool {
        self.columns.iter().any(|column| column.primary_key)
    }

    fn has_changes(&self) -> bool {
        !self.edits.is_empty() || !self.new_rows.is_empty() || !self.deleted_rows.is_empty()
    }

    fn pending_change_count(&self) -> usize {
        self.edits.len() + self.new_rows.len() + self.deleted_rows.len()
    }

    fn clear_changes(&mut self) {
        self.edits.clear();
        self.new_rows.clear();
        self.deleted_rows.clear();
        self.selected_row = None;
        self.selected_rows.clear();
        self.time_picker = None;
        self.commit_error = None;
        self.error_row_index = None;
    }

    fn build_commit_sql(&self) -> std::result::Result<String, String> {
        if !self.can_edit_existing_rows()
            && (!self.edits.is_empty() || !self.deleted_rows.is_empty())
        {
            return Err(
                "Table has no primary key; enable risk mode before editing existing rows"
                    .to_string(),
            );
        }

        let mut statements = Vec::new();
        statements.extend(self.build_insert_sql()?);
        statements.extend(self.build_update_sql()?);
        statements.extend(self.build_delete_sql()?);

        if statements.is_empty() {
            return Err("No pending table changes".to_string());
        }

        Ok(statements.join("\n"))
    }

    fn build_insert_sql(&self) -> std::result::Result<Vec<String>, String> {
        let mut statements = Vec::new();
        for row in &self.new_rows {
            let values = row
                .values
                .iter()
                .enumerate()
                .filter(|(_, value)| !value.trim().is_empty())
                .collect::<Vec<_>>();
            if values.is_empty() {
                continue;
            }

            let columns = values
                .iter()
                .map(|(index, _)| self.quote_identifier(&self.display_column_name(*index)))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let sql_values = values
                .iter()
                .map(|(_, value)| sql_literal(value))
                .collect::<Vec<_>>();

            statements.push(format!(
                "INSERT INTO {} ({}) VALUES ({});",
                self.qualified_table_name()?,
                columns.join(", "),
                sql_values.join(", ")
            ));
        }
        Ok(statements)
    }

    fn build_update_sql(&self) -> std::result::Result<Vec<String>, String> {
        let mut by_row: BTreeMap<usize, Vec<(usize, String)>> = BTreeMap::new();
        for (key, value) in &self.edits {
            by_row
                .entry(key.row)
                .or_default()
                .push((key.column, value.clone()));
        }

        let mut statements = Vec::new();
        for (row_index, changes) in by_row {
            let row = self
                .result
                .rows
                .get(row_index)
                .ok_or_else(|| format!("Missing row {row_index}"))?;
            let assignments = changes
                .iter()
                .map(|(column_index, value)| {
                    Ok(format!(
                        "{} = {}",
                        self.quote_identifier(&self.display_column_name(*column_index))?,
                        sql_literal(value)
                    ))
                })
                .collect::<std::result::Result<Vec<_>, String>>()?;
            statements.push(format!(
                "UPDATE {} SET {} WHERE {};",
                self.qualified_table_name()?,
                assignments.join(", "),
                self.where_clause(row)?
            ));
        }
        Ok(statements)
    }

    fn build_delete_sql(&self) -> std::result::Result<Vec<String>, String> {
        let mut statements = Vec::new();
        for row in self.deleted_rows.values() {
            statements.push(format!(
                "DELETE FROM {} WHERE {};",
                self.qualified_table_name()?,
                self.where_clause(row)?
            ));
        }
        Ok(statements)
    }

    fn where_clause(&self, row: &Row) -> std::result::Result<String, String> {
        let key_columns = self.primary_key_display_indices();

        let columns = if key_columns.is_empty() {
            (0..self.display_column_count()).collect::<Vec<_>>()
        } else {
            key_columns
        };

        if columns.is_empty() {
            return Err("Cannot build WHERE clause without columns".to_string());
        }

        let mut predicates = Vec::new();
        for index in columns {
            let identifier = self.quote_identifier(&self.display_column_name(index))?;
            let value = row.values.get(index).unwrap_or(&CellValue::Null);
            if matches!(value, CellValue::Null) {
                predicates.push(format!("{identifier} IS NULL"));
            } else {
                predicates.push(format!(
                    "{identifier} = {}",
                    sql_literal(&copy_cell_value(value))
                ));
            }
        }
        Ok(predicates.join(" AND "))
    }

    fn qualified_table_name(&self) -> std::result::Result<String, String> {
        let mut output = String::new();
        if let Some(schema) = self.schema.as_deref().filter(|schema| !schema.is_empty()) {
            write!(&mut output, "{}.", self.quote_identifier(schema)?)
                .map_err(|error| error.to_string())?;
        }
        write!(&mut output, "{}", self.quote_identifier(&self.table)?)
            .map_err(|error| error.to_string())?;
        Ok(output)
    }

    fn quote_identifier(&self, value: &str) -> std::result::Result<String, String> {
        let identifier = Identifier::new(value.to_string()).map_err(|error| error.message)?;
        Ok(self.dialect.quote_identifier(&identifier))
    }
}

impl TimePickerState {
    fn from_value(row: RowKey, column: usize, kind: TemporalKind, value: &str) -> Self {
        let now = Local::now().naive_local();
        let mut state = Self {
            row,
            column,
            kind,
            year: now.year(),
            month: now.month(),
            day: now.day(),
            hour: now.hour(),
            minute: now.minute(),
            second: now.second(),
        };

        match kind {
            TemporalKind::Date => {
                if let Some(date) = parse_date_value(value) {
                    state.year = date.year();
                    state.month = date.month();
                    state.day = date.day();
                }
            }
            TemporalKind::Time => {
                if let Some(time) = parse_time_value(value) {
                    state.hour = time.hour();
                    state.minute = time.minute();
                    state.second = time.second();
                }
            }
            TemporalKind::DateTime => {
                if let Some(datetime) = parse_datetime_value(value) {
                    state.year = datetime.date().year();
                    state.month = datetime.date().month();
                    state.day = datetime.date().day();
                    state.hour = datetime.time().hour();
                    state.minute = datetime.time().minute();
                    state.second = datetime.time().second();
                }
            }
        }

        state
    }

    fn set_now(&mut self) {
        let now = Local::now().naive_local();
        self.year = now.year();
        self.month = now.month();
        self.day = now.day();
        self.hour = now.hour();
        self.minute = now.minute();
        self.second = now.second();
    }

    fn format_value(&self) -> String {
        match self.kind {
            TemporalKind::Date => format!("{:04}-{:02}-{:02}", self.year, self.month, self.day),
            TemporalKind::Time => format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second),
            TemporalKind::DateTime => format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                self.year, self.month, self.day, self.hour, self.minute, self.second
            ),
        }
    }
}

fn sql_literal(value: &str) -> String {
    if value == "NULL" {
        "NULL".to_string()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

fn parse_date_value(value: &str) -> Option<NaiveDate> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        return None;
    }
    NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .ok()
        .or_else(|| parse_datetime_value(trimmed).map(|value| value.date()))
}

fn parse_time_value(value: &str) -> Option<NaiveTime> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        return None;
    }
    for format in ["%H:%M:%S%.f", "%H:%M:%S", "%H:%M"] {
        if let Ok(time) = NaiveTime::parse_from_str(trimmed, format) {
            return Some(time);
        }
    }
    parse_datetime_value(trimmed).map(|value| value.time())
}

fn parse_datetime_value(value: &str) -> Option<NaiveDateTime> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        return None;
    }
    if let Ok(datetime) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Some(datetime.naive_local());
    }
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
    ] {
        if let Ok(datetime) = NaiveDateTime::parse_from_str(trimmed, format) {
            return Some(datetime);
        }
    }
    NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let month = month.clamp(1, 12);
    let Some(first_of_next_month) = (if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }) else {
        return 31;
    };
    first_of_next_month
        .pred_opt()
        .map(|date| date.day())
        .unwrap_or(31)
}

fn estimate_column_widths(columns: &[TableColumn], result: &QueryResult) -> Vec<f32> {
    let display_count = if result.columns.is_empty() {
        columns.len()
    } else {
        result.columns.len()
    };

    (0..display_count)
        .map(|column_index| {
            let column_name = result
                .columns
                .get(column_index)
                .map(|column| column.name.as_str())
                .or_else(|| columns.get(column_index).map(|column| column.name.as_str()))
                .unwrap_or("");
            let database_type = columns
                .iter()
                .find(|column| column.name == column_name)
                .or_else(|| columns.get(column_index))
                .map(|column| column.database_type.as_str())
                .or_else(|| {
                    result
                        .columns
                        .get(column_index)
                        .map(|column| column.database_type.as_str())
                })
                .unwrap_or("");
            let mut estimated = estimate_text_width(column_name)
                .max(estimate_text_width(database_type))
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

fn table_cell_fill(ui: &Ui, row: usize, deleted: bool, selected: bool) -> Color32 {
    if selected {
        row_fill(ui, row, true)
    } else if deleted {
        if ui.visuals().dark_mode {
            Color32::from_rgb(42, 30, 30)
        } else {
            Color32::from_rgb(255, 238, 238)
        }
    } else {
        row_fill(ui, row, false)
    }
}

fn command_modifier(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.command
    } else {
        modifiers.ctrl || modifiers.command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::{dialect::BacktickDialect, Column, QueryResult};

    #[test]
    fn builds_update_and_delete_by_primary_key() {
        let mut editor = test_editor(true);
        editor
            .edits
            .insert(CellKey { row: 0, column: 1 }, "Bob".to_string());
        editor.deleted_rows.insert(1, editor.result.rows[1].clone());

        let sql = editor.build_commit_sql().unwrap();

        assert!(sql.contains("UPDATE `main`.`customers` SET `name` = 'Bob' WHERE `id` = '1';"));
        assert!(sql.contains("DELETE FROM `main`.`customers` WHERE `id` = '2';"));
    }

    #[test]
    fn rejects_existing_row_edits_without_primary_key_until_risk_mode() {
        let mut editor = test_editor(false);
        editor
            .edits
            .insert(CellKey { row: 0, column: 1 }, "Bob".to_string());

        assert!(editor.build_commit_sql().is_err());

        editor.allow_risk_without_pk = true;
        let sql = editor.build_commit_sql().unwrap();
        assert!(sql.contains("WHERE `id` = '1' AND `name` = 'Alice';"));
    }

    #[test]
    fn builds_change_summary() {
        let mut editor = test_editor(true);
        editor
            .edits
            .insert(CellKey { row: 0, column: 1 }, "Bob".to_string());
        editor.deleted_rows.insert(1, editor.result.rows[1].clone());
        editor.new_rows.push(NewRow {
            values: vec!["3".to_string(), "Charlie".to_string()],
        });

        let summary = editor.build_change_summary();
        assert_eq!(summary.inserts.len(), 1);
        assert_eq!(summary.updates.len(), 1);
        assert_eq!(summary.deletes.len(), 1);
        assert_eq!(summary.total_changes(), 3);
    }

    #[test]
    fn aligns_result_columns_with_row_values_when_metadata_order_differs() {
        let columns = vec![
            TableColumn {
                name: "id".to_string(),
                database_type: "BIGINT".to_string(),
                nullable: false,
                default_value: None,
                primary_key: true,
                comment: None,
            },
            TableColumn {
                name: "name".to_string(),
                database_type: "VARCHAR".to_string(),
                nullable: true,
                default_value: None,
                primary_key: false,
                comment: None,
            },
        ];
        let result = QueryResult {
            statement: "select name, id from customers".to_string(),
            columns: vec![
                Column {
                    name: "name".to_string(),
                    database_type: "VARCHAR".to_string(),
                    logical_type: LogicalType::Text,
                    nullable: true,
                    primary_key: false,
                },
                Column {
                    name: "id".to_string(),
                    database_type: "BIGINT".to_string(),
                    logical_type: LogicalType::Int,
                    nullable: false,
                    primary_key: true,
                },
            ],
            rows: vec![Row::new(vec![
                CellValue::Text("Alice".to_string()),
                CellValue::Int(1),
            ])],
            affected_rows: 0,
            execution_time_ms: 0,
            notices: Vec::new(),
            metadata: Default::default(),
        };
        let mut editor = TableEditor::new(
            "main",
            "customers",
            columns,
            result,
            Box::new(BacktickDialect),
        );

        assert_eq!(editor.display_column_name(0), "name");
        assert_eq!(editor.display_column_name(1), "id");
        assert_eq!(editor.primary_key_display_indices(), vec![1]);

        editor
            .edits
            .insert(CellKey { row: 0, column: 0 }, "Bob".to_string());
        editor.deleted_rows.insert(0, editor.result.rows[0].clone());

        let sql = editor.build_commit_sql().unwrap();
        assert!(sql.contains("UPDATE `main`.`customers` SET `name` = 'Bob' WHERE `id` = '1';"));
        assert!(sql.contains("DELETE FROM `main`.`customers` WHERE `id` = '1';"));

        let summary = editor.build_change_summary();
        assert_eq!(
            summary.deletes[0].1,
            vec![("id".to_string(), "1".to_string())]
        );
    }

    fn test_editor(primary_key: bool) -> TableEditor {
        let columns = vec![
            TableColumn {
                name: "id".to_string(),
                database_type: "BIGINT".to_string(),
                nullable: false,
                default_value: None,
                primary_key,
                comment: None,
            },
            TableColumn {
                name: "name".to_string(),
                database_type: "VARCHAR".to_string(),
                nullable: true,
                default_value: None,
                primary_key: false,
                comment: None,
            },
        ];
        let result = QueryResult {
            statement: "select id, name from customers".to_string(),
            columns: Vec::new(),
            rows: vec![
                Row::new(vec![
                    CellValue::Int(1),
                    CellValue::Text("Alice".to_string()),
                ]),
                Row::new(vec![
                    CellValue::Int(2),
                    CellValue::Text("Carol".to_string()),
                ]),
            ],
            affected_rows: 0,
            execution_time_ms: 0,
            notices: Vec::new(),
            metadata: Default::default(),
        };

        TableEditor::new(
            "main",
            "customers",
            columns,
            result,
            Box::new(BacktickDialect),
        )
    }
}
