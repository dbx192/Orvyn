#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

use db_common::{
    ConnectionId, DatabaseFunction, TableColumn, TableForeignKey, TableIndex, TableTrigger,
};
use eframe::egui::{
    Button, Color32, Frame, Grid, Margin, RichText, ScrollArea, TextEdit, TextStyle, Ui, Window,
};

#[derive(Debug, Clone)]
pub struct CreateIndexRequest {
    pub connection_id: ConnectionId,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub table: String,
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

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
    details_loaded: bool,
    show_add_index: bool,
    draft_index_name: String,
    draft_index_unique: bool,
    draft_index_columns: Vec<bool>,
    add_index_error: Option<String>,
    pending_create_index: Option<CreateIndexRequest>,
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
        let column_count = columns.len();
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
            details_loaded: true,
            show_add_index: false,
            draft_index_name: String::new(),
            draft_index_unique: false,
            draft_index_columns: vec![false; column_count],
            add_index_error: None,
            pending_create_index: None,
        }
    }

    pub fn from_columns(
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        columns: Vec<TableColumn>,
    ) -> Self {
        let column_count = columns.len();
        Self {
            connection_id,
            database,
            schema,
            table,
            columns,
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            triggers: Vec::new(),
            functions: Vec::new(),
            create_sql: String::new(),
            active_tab: StructureTab::Columns,
            details_loaded: false,
            show_add_index: false,
            draft_index_name: String::new(),
            draft_index_unique: false,
            draft_index_columns: vec![false; column_count],
            add_index_error: None,
            pending_create_index: None,
        }
    }

    pub fn needs_detail_load(&self) -> bool {
        !self.details_loaded
    }

    pub fn load_target(&self) -> (ConnectionId, Option<String>, Option<String>, String) {
        (
            self.connection_id,
            self.database.clone(),
            self.schema.clone(),
            self.table.clone(),
        )
    }

    pub fn take_create_index_request(&mut self) -> Option<CreateIndexRequest> {
        self.pending_create_index.take()
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
        self.add_index_dialog(ui, &mut message);

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
            if !self.details_loaded {
                ui.label(
                    RichText::new("details lazy-loaded")
                        .small()
                        .color(Color32::GRAY),
                );
            }
            ui.separator();
            if ui
                .add_enabled(self.details_loaded, eframe::egui::Button::new("Copy DDL"))
                .clicked()
            {
                ui.output_mut(|o| o.copied_text = self.create_sql.clone());
                message = Some("CREATE TABLE DDL copied to clipboard".to_string());
            }
            if ui
                .add_enabled(
                    self.details_loaded && !self.columns.is_empty(),
                    Button::new("Add Index"),
                )
                .clicked()
            {
                self.prepare_add_index();
            }
        });

        ui.add_space(4.0);
        self.tabs(ui);
        ui.add_space(2.0);

        Frame::default()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .inner_margin(Margin::same(0.0))
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

    fn prepare_add_index(&mut self) {
        self.active_tab = StructureTab::Indexes;
        self.show_add_index = true;
        self.add_index_error = None;
        self.draft_index_unique = false;
        self.draft_index_columns = vec![false; self.columns.len()];
        let base = self
            .columns
            .first()
            .map(|column| column.name.as_str())
            .unwrap_or("column");
        self.draft_index_name = format!("idx_{}_{}", self.table, base);
    }

    fn add_index_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        if !self.show_add_index {
            return;
        }

        if self.draft_index_columns.len() != self.columns.len() {
            self.draft_index_columns.resize(self.columns.len(), false);
        }

        let mut open = true;
        let mut create_requested = false;
        let mut cancel_requested = false;

        Window::new("Add Index")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(RichText::new(self.table_name()).strong());
                ui.add_space(8.0);
                Grid::new("add_index_form")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.add(
                            TextEdit::singleline(&mut self.draft_index_name).desired_width(260.0),
                        );
                        ui.end_row();
                        ui.label("Unique");
                        ui.checkbox(&mut self.draft_index_unique, "");
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.label(RichText::new("Columns").strong());
                ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for (index, column) in self.columns.iter().enumerate() {
                        ui.checkbox(
                            &mut self.draft_index_columns[index],
                            format!(
                                "{}  {}",
                                column.name,
                                type_without_field_width(&column.database_type)
                            ),
                        );
                    }
                });

                if let Some(error) = &self.add_index_error {
                    ui.colored_label(Color32::RED, error);
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel_requested = true;
                    }
                    if ui.button("Create").clicked() {
                        create_requested = true;
                    }
                });
            });

        if cancel_requested || !open {
            self.show_add_index = false;
            return;
        }

        if create_requested {
            let name = self.draft_index_name.trim().to_string();
            let columns = self
                .columns
                .iter()
                .zip(self.draft_index_columns.iter())
                .filter_map(|(column, selected)| selected.then(|| column.name.clone()))
                .collect::<Vec<_>>();

            if name.is_empty() {
                self.add_index_error = Some("Index name is required".to_string());
            } else if columns.is_empty() {
                self.add_index_error = Some("Select at least one column".to_string());
            } else {
                self.pending_create_index = Some(CreateIndexRequest {
                    connection_id: self.connection_id,
                    database: self.database.clone(),
                    schema: self.schema.clone(),
                    table: self.table.clone(),
                    name,
                    columns,
                    unique: self.draft_index_unique,
                });
                self.show_add_index = false;
                self.add_index_error = None;
                *message = Some("Creating index...".to_string());
            }
        }
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
                    .spacing([14.0, 2.0])
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
                    .spacing([14.0, 2.0])
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
                    .spacing([14.0, 2.0])
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
                    .spacing([14.0, 2.0])
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
                    .spacing([14.0, 2.0])
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
