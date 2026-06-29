#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

use db_common::{
    ConnectionId, DatabaseFunction, TableColumn, TableForeignKey, TableIndex, TableTrigger,
};
use eframe::egui::{CollapsingHeader, Color32, Grid, RichText, ScrollArea, TextStyle, Ui};

/// A dedicated tab that displays the full structure of a database table:
/// columns, indexes, foreign keys, triggers, functions, and DDL.
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
    columns_expanded: bool,
    indexes_expanded: bool,
    fks_expanded: bool,
    triggers_expanded: bool,
    functions_expanded: bool,
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
            columns_expanded: true,
            indexes_expanded: true,
            fks_expanded: false,
            triggers_expanded: false,
            functions_expanded: false,
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

        // ── Toolbar ──
        ui.horizontal(|ui| {
            ui.heading(format!("📐 Structure: {}", self.table_name()));
            ui.separator();
            if ui.button("📋 Copy DDL").clicked() {
                ui.output_mut(|o| o.copied_text = self.create_sql.clone());
                message = Some("CREATE TABLE DDL copied to clipboard".to_string());
            }
        });
        ui.add_space(8.0);

        // ── Columns ──
        CollapsingHeader::new("📋 Columns")
            .default_open(self.columns_expanded)
            .show(ui, |ui| {
                self.columns_expanded = true;
                self.columns_table(ui);
            });

        // ── Indexes ──
        CollapsingHeader::new(format!("🔍 Indexes ({})", self.indexes.len()))
            .default_open(self.indexes_expanded)
            .show(ui, |ui| {
                self.indexes_expanded = true;
                self.indexes_table(ui);
            });

        // ── Foreign Keys ──
        CollapsingHeader::new(format!("🔗 Foreign Keys ({})", self.foreign_keys.len()))
            .default_open(self.fks_expanded)
            .show(ui, |ui| {
                self.fks_expanded = true;
                self.foreign_keys_table(ui);
            });

        // ── Triggers ──
        CollapsingHeader::new(format!("⚡ Triggers ({})", self.triggers.len()))
            .default_open(self.triggers_expanded)
            .show(ui, |ui| {
                self.triggers_expanded = true;
                self.triggers_table(ui);
            });

        // ── Functions ──
        CollapsingHeader::new(format!("📦 Functions ({})", self.functions.len()))
            .default_open(self.functions_expanded)
            .show(ui, |ui| {
                self.functions_expanded = true;
                self.functions_table(ui);
            });

        ui.add_space(12.0);

        // ── CREATE TABLE DDL ──
        CollapsingHeader::new("📝 CREATE TABLE DDL")
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(4.0);
                let code_style = TextStyle::Monospace;
                ScrollArea::vertical()
                    .max_height(300.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let mut sql_display = self.create_sql.clone();
                        ui.add(
                            eframe::egui::TextEdit::multiline(&mut sql_display)
                                .font(code_style)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
            });

        message
    }

    /// Renders the columns section as a table.
    fn columns_table(&self, ui: &mut Ui) {
        if self.columns.is_empty() {
            ui.label(RichText::new("No columns").color(Color32::GRAY));
            return;
        }

        Grid::new("structure_columns_grid")
            .striped(true)
            .num_columns(6)
            .show(ui, |ui| {
                // Header
                for label in &["#", "Name", "Type", "Nullable", "Default", "PK"] {
                    ui.label(RichText::new(*label).strong());
                }
                ui.end_row();

                for (idx, col) in self.columns.iter().enumerate() {
                    ui.label(idx.to_string());
                    ui.label(&col.name);
                    ui.label(&col.database_type);
                    ui.label(if col.nullable { "✓" } else { "" });
                    ui.label(col.default_value.as_deref().unwrap_or(""));
                    ui.label(if col.primary_key {
                        RichText::new("🔑").strong()
                    } else {
                        RichText::new("")
                    });
                    ui.end_row();
                }
            });
    }

    /// Renders the indexes section as a table.
    fn indexes_table(&self, ui: &mut Ui) {
        if self.indexes.is_empty() {
            ui.label(RichText::new("No indexes").color(Color32::GRAY));
            return;
        }

        Grid::new("structure_indexes_grid")
            .striped(true)
            .num_columns(4)
            .show(ui, |ui| {
                for label in &["Name", "Columns", "Unique", "Primary"] {
                    ui.label(RichText::new(*label).strong());
                }
                ui.end_row();

                for idx in &self.indexes {
                    ui.label(&idx.name);
                    ui.label(idx.columns.join(", "));
                    ui.label(if idx.unique { "✓" } else { "" });
                    ui.label(if idx.primary { "✓" } else { "" });
                    ui.end_row();
                }
            });
    }

    /// Renders the foreign keys section as a table.
    fn foreign_keys_table(&self, ui: &mut Ui) {
        if self.foreign_keys.is_empty() {
            ui.label(RichText::new("No foreign keys").color(Color32::GRAY));
            return;
        }

        Grid::new("structure_fks_grid")
            .striped(true)
            .num_columns(6)
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
    }

    /// Renders the triggers section as a table.
    fn triggers_table(&self, ui: &mut Ui) {
        if self.triggers.is_empty() {
            ui.label(RichText::new("No triggers").color(Color32::GRAY));
            return;
        }

        Grid::new("structure_triggers_grid")
            .striped(true)
            .num_columns(4)
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
                    let preview = if stmt.len() > 80 {
                        format!("{}...", &stmt[..80])
                    } else {
                        stmt.to_string()
                    };
                    ui.label(preview).on_hover_text(stmt);
                    ui.end_row();
                }
            });
    }

    /// Renders the functions section as a table.
    fn functions_table(&self, ui: &mut Ui) {
        if self.functions.is_empty() {
            ui.label(RichText::new("No functions").color(Color32::GRAY));
            return;
        }

        Grid::new("structure_functions_grid")
            .striped(true)
            .num_columns(3)
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
    }
}
