#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

use db_common::{
    ConnectionId, DatabaseFunction, TableColumn, TableForeignKey, TableIndex, TableTrigger,
};
use eframe::egui::{Color32, Frame, Grid, Margin, RichText, ScrollArea, TextEdit, TextStyle, Ui};

/// A dedicated tab that displays the structure of a database table.
pub struct TableStructureView {
    connection_id: ConnectionId,
    database: Option<String>,
    schema: Option<String>,
    table: String,
    columns: Vec<TableColumn>,
    indexes: Vec<TableIndex>,
    foreign_keys: Vec<TableForeignKey>,
    triggers: Vec<TableTrigger>,
    functions: Vec<DatabaseFunction>,
    create_sql: String,
    active_tab: StructureTab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StructureTab {
    Columns,
    Indexes,
    ForeignKeys,
    Triggers,
    Functions,
    Ddl,
}

impl TableStructureView {
    /// Create a view with all structure data already loaded.
    pub fn new(
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        columns: Vec<TableColumn>,
        indexes: Vec<TableIndex>,
        foreign_keys: Vec<TableForeignKey>,
        triggers: Vec<TableTrigger>,
        functions: Vec<DatabaseFunction>,
        create_sql: String,
    ) -> Self {
        Self {
            connection_id,
            database,
            schema,
            table,
            columns,
            indexes,
            foreign_keys,
            triggers,
            functions,
            create_sql,
            active_tab: StructureTab::Columns,
        }
    }

    /// Return the fully qualified table name.
    pub fn table_name(&self) -> String {
        match self.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(schema) => format!("{}.{}", schema, self.table),
            None => self.table.clone(),
        }
    }

    /// Render the structure view. Returns an optional status message.
    pub fn ui(&mut self, ui: &mut Ui) -> Option<String> {
        let mut message: Option<String> = None;

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(self.table_name()).strong().size(15.0));
            ui.separator();
            ui.label(
                RichText::new(format!("{} columns", self.columns.len()))
                    .small()
                    .color(Color32::GRAY),
            );
            ui.label(
                RichText::new(format!("{} indexes", self.indexes.len()))
                    .small()
                    .color(Color32::GRAY),
            );
            if !self.foreign_keys.is_empty() {
                ui.label(
                    RichText::new(format!("{} foreign keys", self.foreign_keys.len()))
                        .small()
                        .color(Color32::GRAY),
                );
            }
            ui.separator();
            if ui.button("Copy DDL").clicked() {
                ui.output_mut(|o| o.copied_text = self.create_sql.clone());
                message = Some("CREATE TABLE DDL copied to clipboard".to_string());
            }
        });

        ui.add_space(4.0);
        self.tabs(ui);
        ui.add_space(2.0);

        Frame::default()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .inner_margin(Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| match self.active_tab {
                StructureTab::Columns => self.columns_table(ui),
                StructureTab::Indexes => self.indexes_table(ui),
                StructureTab::ForeignKeys => self.foreign_keys_table(ui),
                StructureTab::Triggers => self.triggers_table(ui),
                StructureTab::Functions => self.functions_table(ui),
                StructureTab::Ddl => self.ddl_view(ui),
            });

        message
    }

    fn tabs(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::Columns,
                format!("Columns {}", self.columns.len()),
            );
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::Indexes,
                format!("Indexes {}", self.indexes.len()),
            );
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::ForeignKeys,
                format!("Foreign Keys {}", self.foreign_keys.len()),
            );
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::Triggers,
                format!("Triggers {}", self.triggers.len()),
            );
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::Functions,
                format!("Functions {}", self.functions.len()),
            );
            ui.selectable_value(&mut self.active_tab, StructureTab::Ddl, "DDL");
        });
    }

    /// Renders the columns section as a table.
    fn columns_table(&self, ui: &mut Ui) {
        if self.columns.is_empty() {
            empty_message(ui, "No columns");
            return;
        }

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new("structure_columns_grid")
                    .striped(true)
                    .num_columns(5)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for label in &["Name", "Type", "Null", "Default", "Key"] {
                            ui.label(RichText::new(*label).strong());
                        }
                        ui.end_row();

                        for col in &self.columns {
                            ui.label(&col.name);
                            ui.label(type_without_field_width(&col.database_type))
                                .on_hover_text(&col.database_type);
                            ui.label(if col.nullable { "YES" } else { "NO" });
                            ui.label(col.default_value.as_deref().unwrap_or(""));
                            ui.label(if col.primary_key { "PK" } else { "" });
                            ui.end_row();
                        }
                    });
            });
    }

    /// Renders the indexes section as a table.
    fn indexes_table(&self, ui: &mut Ui) {
        if self.indexes.is_empty() {
            empty_message(ui, "No indexes");
            return;
        }

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new("structure_indexes_grid")
                    .striped(true)
                    .num_columns(4)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for label in &["Name", "Columns", "Unique", "Primary"] {
                            ui.label(RichText::new(*label).strong());
                        }
                        ui.end_row();

                        for idx in &self.indexes {
                            ui.label(&idx.name);
                            ui.label(idx.columns.join(", "));
                            ui.label(if idx.unique { "YES" } else { "" });
                            ui.label(if idx.primary { "YES" } else { "" });
                            ui.end_row();
                        }
                    });
            });
    }

    /// Renders the foreign keys section as a table.
    fn foreign_keys_table(&self, ui: &mut Ui) {
        if self.foreign_keys.is_empty() {
            empty_message(ui, "No foreign keys");
            return;
        }

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new("structure_fks_grid")
                    .striped(true)
                    .num_columns(6)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for label in &[
                            "Name",
                            "Columns",
                            "Referenced Table",
                            "Referenced Columns",
                            "On Update",
                            "On Delete",
                        ] {
                            ui.label(RichText::new(*label).strong());
                        }
                        ui.end_row();

                        for fk in &self.foreign_keys {
                            ui.label(&fk.name);
                            ui.label(fk.columns.join(", "));
                            ui.label(&fk.referenced_table);
                            ui.label(fk.referenced_columns.join(", "));
                            ui.label(fk.on_update.as_deref().unwrap_or(""));
                            ui.label(fk.on_delete.as_deref().unwrap_or(""));
                            ui.end_row();
                        }
                    });
            });
    }

    /// Renders the triggers section as a table.
    fn triggers_table(&self, ui: &mut Ui) {
        if self.triggers.is_empty() {
            empty_message(ui, "No triggers");
            return;
        }

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new("structure_triggers_grid")
                    .striped(true)
                    .num_columns(4)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for label in &["Name", "Event", "Timing", "Statement"] {
                            ui.label(RichText::new(*label).strong());
                        }
                        ui.end_row();

                        for trg in &self.triggers {
                            ui.label(&trg.name);
                            ui.label(&trg.event);
                            ui.label(&trg.timing);
                            let stmt = trg.statement.as_deref().unwrap_or("");
                            let preview = if stmt.chars().count() > 100 {
                                format!("{}...", stmt.chars().take(100).collect::<String>())
                            } else {
                                stmt.to_string()
                            };
                            ui.label(preview).on_hover_text(stmt);
                            ui.end_row();
                        }
                    });
            });
    }

    /// Renders the functions section as a table.
    fn functions_table(&self, ui: &mut Ui) {
        if self.functions.is_empty() {
            empty_message(ui, "No functions");
            return;
        }

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new("structure_functions_grid")
                    .striped(true)
                    .num_columns(3)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for label in &["Name", "Return Type", "Kind"] {
                            ui.label(RichText::new(*label).strong());
                        }
                        ui.end_row();

                        for func in &self.functions {
                            ui.label(&func.name);
                            ui.label(func.return_type.as_deref().unwrap_or(""));
                            ui.label(&func.kind);
                            ui.end_row();
                        }
                    });
            });
    }

    fn ddl_view(&self, ui: &mut Ui) {
        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut sql_display = self.create_sql.clone();
                ui.add(
                    TextEdit::multiline(&mut sql_display)
                        .font(TextStyle::Monospace)
                        .desired_width(ui.available_width().max(720.0))
                        .desired_rows(28)
                        .interactive(false),
                );
            });
    }
}

fn empty_message(ui: &mut Ui, text: &str) {
    ui.add_space(32.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(text).color(Color32::GRAY).italics());
    });
}

fn type_without_field_width(value: &str) -> String {
    let trimmed = value.trim();
    let Some(open) = trimmed.find('(') else {
        return trimmed.to_string();
    };
    let Some(close_offset) = trimmed[open..].find(')') else {
        return trimmed.to_string();
    };

    let close = open + close_offset;
    let base = trimmed[..open].trim();
    let suffix = trimmed[close + 1..].trim();

    if base.is_empty() {
        trimmed.to_string()
    } else if suffix.is_empty() {
        base.to_string()
    } else {
        format!("{base} {suffix}")
    }
}
