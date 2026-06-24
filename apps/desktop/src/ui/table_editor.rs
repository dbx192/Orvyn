use crate::ui::format::{cell_rich_text, copy_cell_value, display_cell_value};
use db_common::{CellValue, Dialect, Identifier, QueryResult, Row, TableColumn};
use eframe::egui::{
    Align, Button, DragValue, Frame, Label, Layout, RichText, ScrollArea, Sense, TextEdit, Ui, Vec2,
};
use std::{collections::BTreeMap, fmt::Write};

const DEFAULT_PAGE_SIZE: usize = 100;
const MIN_COLUMN_WIDTH: f32 = 72.0;
const MAX_COLUMN_WIDTH: f32 = 640.0;
const ROW_HEIGHT: f32 = 28.0;
const ACTION_WIDTH: f32 = 96.0;

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
    edits: BTreeMap<CellKey, String>,
    new_rows: Vec<NewRow>,
    deleted_rows: BTreeMap<usize, Row>,
    allow_risk_without_pk: bool,
    last_sql_preview: String,
}

impl TableEditor {
    pub fn new(
        schema: impl Into<String>,
        table: impl Into<String>,
        columns: Vec<TableColumn>,
        result: QueryResult,
        dialect: Box<dyn Dialect>,
    ) -> Self {
        let column_widths = columns
            .iter()
            .map(|column| ((column.name.len() as f32 * 8.0) + 72.0).clamp(MIN_COLUMN_WIDTH, 180.0))
            .collect();

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
            edits: BTreeMap::new(),
            new_rows: Vec::new(),
            deleted_rows: BTreeMap::new(),
            allow_risk_without_pk: false,
            last_sql_preview: String::new(),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<String> {
        let mut message = None;
        self.toolbar(ui, &mut message);
        ui.add_space(8.0);

        if !self.has_primary_key() && !self.allow_risk_without_pk {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "No primary key detected. Existing rows are read-only until risk mode is enabled.",
            );
            ui.add_space(8.0);
        }

        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::horizontal()
                .id_source("table_editor_horizontal")
                .show(ui, |ui| {
                    self.header(ui);
                    ui.separator();
                    ScrollArea::vertical()
                        .id_source("table_editor_vertical")
                        .max_height(ui.available_height().max(280.0))
                        .auto_shrink([false, false])
                        .show_rows(
                            ui,
                            ROW_HEIGHT,
                            self.visible_rows().len(),
                            |ui, row_range| {
                                let visible_rows = self.visible_rows();
                                for visible_index in row_range {
                                    let Some(row_key) = visible_rows.get(visible_index).copied()
                                    else {
                                        continue;
                                    };
                                    self.row(ui, row_key);
                                }
                            },
                        );
                });
        });

        if !self.last_sql_preview.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Pending SQL preview").strong());
            ui.add(
                TextEdit::multiline(&mut self.last_sql_preview)
                    .desired_rows(6)
                    .code_editor()
                    .interactive(false),
            );
        }

        message
    }

    fn toolbar(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{}.{}",
                self.schema.as_deref().unwrap_or(""),
                self.table
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

            ui.label("Rows/page");
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
            if ui.button("New Row").clicked() {
                self.new_rows.push(NewRow {
                    values: vec![String::new(); self.columns.len()],
                });
                *message = Some("New row staged".to_string());
            }
            if ui
                .add_enabled(self.has_changes(), Button::new("Commit"))
                .clicked()
            {
                match self.build_commit_sql() {
                    Ok(sql) => {
                        self.last_sql_preview = sql;
                        self.clear_changes();
                        *message =
                            Some("Changes converted to SQL preview and marked clean".to_string());
                    }
                    Err(error) => {
                        *message = Some(error);
                    }
                }
            }
            if ui
                .add_enabled(self.has_changes(), Button::new("Rollback"))
                .clicked()
            {
                self.clear_changes();
                self.last_sql_preview.clear();
                *message = Some("Uncommitted table changes rolled back".to_string());
            }

            ui.separator();
            ui.checkbox(&mut self.allow_risk_without_pk, "Risk mode");
            ui.label(format!("{} pending", self.pending_change_count()));
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("Filter");
            if !self.columns.is_empty() {
                self.filter_column = self.filter_column.min(self.columns.len() - 1);
                let selected = &self.columns[self.filter_column].name;
                eframe::egui::ComboBox::from_id_source("table_filter_column")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (index, column) in self.columns.iter().enumerate() {
                            ui.selectable_value(&mut self.filter_column, index, &column.name);
                        }
                    });
            }
            let response = ui.add(
                TextEdit::singleline(&mut self.filter_text)
                    .desired_width(220.0)
                    .hint_text("contains text"),
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
        });
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(ACTION_WIDTH, 46.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.label(RichText::new("Actions").strong());
                },
            );

            for column_index in 0..self.columns.len() {
                let column = &self.columns[column_index];
                let column_name = column.name.clone();
                let column_hover = format!(
                    "{}{}{}",
                    column.database_type,
                    if column.nullable { ", nullable" } else { "" },
                    if column.primary_key {
                        ", primary key"
                    } else {
                        ""
                    }
                );
                let width = self.column_widths[column_index];

                ui.allocate_ui_with_layout(
                    Vec2::new(width, 46.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let title = format!("{}{}", column_name, self.sort_marker(column_index));
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

    fn row(&mut self, ui: &mut Ui, row_key: RowKey) {
        let deleted =
            matches!(row_key, RowKey::Existing(index) if self.deleted_rows.contains_key(&index));

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(ACTION_WIDTH, ROW_HEIGHT),
                Layout::left_to_right(Align::Center),
                |ui| match row_key {
                    RowKey::Existing(row_index) => {
                        let can_delete = self.can_edit_existing_rows() && !deleted;
                        if ui.add_enabled(can_delete, Button::new("Delete")).clicked() {
                            if let Some(row) = self.result.rows.get(row_index).cloned() {
                                self.deleted_rows.insert(row_index, row);
                            }
                        }
                        if deleted && ui.button("Undo").clicked() {
                            self.deleted_rows.remove(&row_index);
                        }
                    }
                    RowKey::New(row_index) => {
                        if ui.button("Remove").clicked() && row_index < self.new_rows.len() {
                            self.new_rows.remove(row_index);
                        }
                    }
                },
            );

            for column_index in 0..self.columns.len() {
                let width = self.column_widths[column_index];
                let editable = match row_key {
                    RowKey::Existing(_) => self.can_edit_existing_rows() && !deleted,
                    RowKey::New(_) => true,
                };

                match row_key {
                    RowKey::Existing(row_index) => {
                        self.existing_cell(ui, row_index, column_index, width, editable, deleted);
                    }
                    RowKey::New(row_index) => {
                        self.new_cell(ui, row_index, column_index, width);
                    }
                }
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
    ) {
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

        if editable {
            let mut value = self
                .edits
                .get(&key)
                .cloned()
                .unwrap_or_else(|| copy_cell_value(original));
            let response = ui.add_sized(
                [width, ROW_HEIGHT],
                TextEdit::singleline(&mut value).desired_width(width),
            );
            if response.changed() {
                if value == copy_cell_value(original) {
                    self.edits.remove(&key);
                } else {
                    self.edits.insert(key, value);
                }
            }
            if self.edits.contains_key(&key) {
                let rect = response.rect;
                ui.painter()
                    .rect_stroke(rect, 2.0, (1.0, ui.visuals().selection.stroke.color));
            }
        } else {
            let mut rich = cell_rich_text(original, display_cell_value(original));
            if deleted {
                rich = rich.strikethrough();
            }
            ui.add_sized([width, ROW_HEIGHT], Label::new(rich).sense(Sense::hover()))
                .on_hover_text(copy_cell_value(original));
        }
    }

    fn new_cell(&mut self, ui: &mut Ui, row_index: usize, column_index: usize, width: f32) {
        if let Some(row) = self.new_rows.get_mut(row_index) {
            if let Some(value) = row.values.get_mut(column_index) {
                ui.add_sized(
                    [width, ROW_HEIGHT],
                    TextEdit::singleline(value).desired_width(width),
                );
            }
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
                .map(|(index, _)| self.quote_identifier(&self.columns[*index].name))
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
                        self.quote_identifier(&self.columns[*column_index].name)?,
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
        let key_columns = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .collect::<Vec<_>>();

        let columns = if key_columns.is_empty() {
            self.columns.iter().enumerate().collect::<Vec<_>>()
        } else {
            key_columns
        };

        if columns.is_empty() {
            return Err("Cannot build WHERE clause without columns".to_string());
        }

        let mut predicates = Vec::new();
        for (index, column) in columns {
            let identifier = self.quote_identifier(&column.name)?;
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

fn sql_literal(value: &str) -> String {
    if value == "NULL" {
        "NULL".to_string()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::{dialect::BacktickDialect, QueryResult};

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
