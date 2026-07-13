#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

use db_common::{
    dialect::{BacktickDialect, Dialect, DoubleQuoteDialect, Identifier},
    AppError, ConnectionId, DataSourceKind, DatabaseFunction, TableColumn, TableForeignKey,
    TableIndex, TableTrigger,
};
use eframe::egui::{
    Button, Color32, ComboBox, Frame, Grid, Margin, RichText, ScrollArea, TextEdit, TextStyle, Ui,
    Window,
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

/// A request to alter a table's structure. The shell layer turns this into
/// a `QueryExecutionRequest` and re-loads the structure after it completes.
#[derive(Debug, Clone)]
pub enum AlterTableRequest {
    AddColumn {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        column_name: String,
        column_type: String,
        nullable: bool,
        default_value: Option<String>,
    },
    DropColumn {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        column_name: String,
    },
    ModifyColumn {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        column_name: String,
        new_type: String,
        nullable: bool,
        default_value: Option<String>,
    },
    DropIndex {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        index_name: String,
    },
    AddForeignKey {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        constraint_name: String,
        columns: Vec<String>,
        referenced_table: String,
        referenced_columns: Vec<String>,
        on_update: Option<String>,
        on_delete: Option<String>,
    },
}

impl AlterTableRequest {
    pub fn connection_id(&self) -> ConnectionId {
        match self {
            AlterTableRequest::AddColumn { connection_id, .. }
            | AlterTableRequest::DropColumn { connection_id, .. }
            | AlterTableRequest::ModifyColumn { connection_id, .. }
            | AlterTableRequest::DropIndex { connection_id, .. }
            | AlterTableRequest::AddForeignKey { connection_id, .. } => *connection_id,
        }
    }

    pub fn table(&self) -> &str {
        match self {
            AlterTableRequest::AddColumn { table, .. }
            | AlterTableRequest::DropColumn { table, .. }
            | AlterTableRequest::ModifyColumn { table, .. }
            | AlterTableRequest::DropIndex { table, .. }
            | AlterTableRequest::AddForeignKey { table, .. } => table,
        }
    }

    pub fn database(&self) -> Option<&str> {
        match self {
            AlterTableRequest::AddColumn { database, .. }
            | AlterTableRequest::DropColumn { database, .. }
            | AlterTableRequest::ModifyColumn { database, .. }
            | AlterTableRequest::DropIndex { database, .. }
            | AlterTableRequest::AddForeignKey { database, .. } => database.as_deref(),
        }
    }

    pub fn schema(&self) -> Option<&str> {
        match self {
            AlterTableRequest::AddColumn { schema, .. }
            | AlterTableRequest::DropColumn { schema, .. }
            | AlterTableRequest::ModifyColumn { schema, .. }
            | AlterTableRequest::DropIndex { schema, .. }
            | AlterTableRequest::AddForeignKey { schema, .. } => schema.as_deref(),
        }
    }
}

/// Generate the dialect-aware SQL for an `AlterTableRequest`.
pub fn build_alter_table_sql(
    request: &AlterTableRequest,
    kind: DataSourceKind,
) -> Result<String, AppError> {
    let dialect = dialect_for_kind(kind);
    match request {
        AlterTableRequest::AddColumn {
            table,
            column_name,
            column_type,
            nullable,
            default_value,
            ..
        } => {
            let table_sql = qualified_table_sql(
                dialect.as_ref(),
                kind,
                request.database(),
                request.schema(),
                table,
            )?;
            let column_sql = build_column_definition(
                dialect.as_ref(),
                column_name,
                column_type,
                *nullable,
                default_value.as_deref(),
            )?;
            Ok(format!("ALTER TABLE {table_sql} ADD COLUMN {column_sql}"))
        }
        AlterTableRequest::DropColumn {
            table, column_name, ..
        } => {
            let table_sql = qualified_table_sql(
                dialect.as_ref(),
                kind,
                request.database(),
                request.schema(),
                table,
            )?;
            let column_sql = quote_id(dialect.as_ref(), column_name)?;
            Ok(format!("ALTER TABLE {table_sql} DROP COLUMN {column_sql}"))
        }
        AlterTableRequest::ModifyColumn {
            table,
            column_name,
            new_type,
            nullable,
            default_value,
            ..
        } => {
            let table_sql = qualified_table_sql(
                dialect.as_ref(),
                kind,
                request.database(),
                request.schema(),
                table,
            )?;
            let column_sql = quote_id(dialect.as_ref(), column_name)?;
            let column_def = build_column_definition(
                dialect.as_ref(),
                column_name,
                new_type,
                *nullable,
                default_value.as_deref(),
            )?;
            // MySQL uses MODIFY COLUMN, PostgreSQL/SQLite/DuckDB use ALTER COLUMN ... TYPE
            match kind {
                DataSourceKind::MySql => Ok(format!(
                    "ALTER TABLE {table_sql} MODIFY COLUMN {column_def}"
                )),
                DataSourceKind::PostgreSql => {
                    let mut statements = vec![format!(
                        "ALTER TABLE {table_sql} ALTER COLUMN {column_sql} TYPE {new_type}"
                    )];
                    if *nullable {
                        statements.push(format!(
                            "ALTER TABLE {table_sql} ALTER COLUMN {column_sql} DROP NOT NULL"
                        ));
                    } else {
                        statements.push(format!(
                            "ALTER TABLE {table_sql} ALTER COLUMN {column_sql} SET NOT NULL"
                        ));
                    }
                    if let Some(default) = default_value {
                        statements.push(format!(
                            "ALTER TABLE {table_sql} ALTER COLUMN {column_sql} SET DEFAULT {default}"
                        ));
                    }
                    Ok(statements.join("; "))
                }
                DataSourceKind::Sqlite | DataSourceKind::DuckDb => {
                    // SQLite < 3.35 doesn't support full ALTER COLUMN; DuckDB does.
                    Ok(format!("ALTER TABLE {table_sql} ALTER COLUMN {column_sql} SET DATA TYPE {new_type}"))
                }
                DataSourceKind::Redis => Err(AppError::validation(
                    "unsupported data source for ALTER TABLE",
                )),
            }
        }
        AlterTableRequest::DropIndex {
            table, index_name, ..
        } => {
            let _ = table;
            let index_sql = quote_id(dialect.as_ref(), index_name)?;
            match kind {
                DataSourceKind::MySql => {
                    let table_sql = qualified_table_sql(
                        dialect.as_ref(),
                        kind,
                        request.database(),
                        request.schema(),
                        table,
                    )?;
                    Ok(format!("DROP INDEX {index_sql} ON {table_sql}"))
                }
                DataSourceKind::PostgreSql => {
                    // PostgreSQL: DROP INDEX [IF EXISTS] name [CASCADE|RESTRICT]
                    // Index name may be schema-qualified
                    let full_name = match request.schema() {
                        Some(schema) if !schema.is_empty() => {
                            format!("{}.{}", quote_id(dialect.as_ref(), schema)?, index_sql)
                        }
                        _ => index_sql,
                    };
                    Ok(format!("DROP INDEX IF EXISTS {full_name}"))
                }
                DataSourceKind::Sqlite | DataSourceKind::DuckDb => {
                    Ok(format!("DROP INDEX IF EXISTS {index_sql}"))
                }
                DataSourceKind::Redis => Err(AppError::validation(
                    "unsupported data source for DROP INDEX",
                )),
            }
        }
        AlterTableRequest::AddForeignKey {
            table,
            constraint_name,
            columns,
            referenced_table,
            referenced_columns,
            on_update,
            on_delete,
            ..
        } => {
            let table_sql = qualified_table_sql(
                dialect.as_ref(),
                kind,
                request.database(),
                request.schema(),
                table,
            )?;
            let constraint_sql = quote_id(dialect.as_ref(), constraint_name)?;
            let cols = columns
                .iter()
                .map(|c| quote_id(dialect.as_ref(), c))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            let ref_table_sql = qualified_table_sql(
                dialect.as_ref(),
                kind,
                request.database(),
                request.schema(),
                referenced_table,
            )?;
            let ref_cols = referenced_columns
                .iter()
                .map(|c| quote_id(dialect.as_ref(), c))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            let mut sql = format!(
                "ALTER TABLE {table_sql} ADD CONSTRAINT {constraint_sql} FOREIGN KEY ({cols}) REFERENCES {ref_table_sql} ({ref_cols})"
            );
            if let Some(action) = on_update {
                if !action.is_empty() {
                    sql.push_str(&format!(" ON UPDATE {action}"));
                }
            }
            if let Some(action) = on_delete {
                if !action.is_empty() {
                    sql.push_str(&format!(" ON DELETE {action}"));
                }
            }
            Ok(sql)
        }
    }
}

fn build_column_definition(
    dialect: &dyn Dialect,
    name: &str,
    column_type: &str,
    nullable: bool,
    default_value: Option<&str>,
) -> Result<String, AppError> {
    let name_sql = quote_id(dialect, name)?;
    let mut def = format!("{name_sql} {column_type}");
    if !nullable {
        def.push_str(" NOT NULL");
    }
    if let Some(default) = default_value {
        if !default.is_empty() {
            def.push_str(&format!(" DEFAULT {default}"));
        }
    }
    Ok(def)
}

fn quote_id(dialect: &dyn Dialect, value: &str) -> Result<String, AppError> {
    Ok(dialect.quote_identifier(&Identifier::new(value)?))
}

fn qualified_table_sql(
    dialect: &dyn Dialect,
    kind: DataSourceKind,
    database: Option<&str>,
    schema: Option<&str>,
    table: &str,
) -> Result<String, AppError> {
    let table_sql = quote_id(dialect, table)?;
    let qualifier = match kind {
        DataSourceKind::MySql => database.or(schema),
        _ => schema,
    };
    match qualifier.filter(|value| !value.is_empty()) {
        Some(qualifier) => Ok(format!("{}.{table_sql}", quote_id(dialect, qualifier)?)),
        None => Ok(table_sql),
    }
}

fn dialect_for_kind(kind: DataSourceKind) -> Box<dyn Dialect> {
    match kind {
        DataSourceKind::MySql => Box::new(BacktickDialect),
        _ => Box::new(DoubleQuoteDialect),
    }
}

/// A dedicated tab that displays the structure of a database table.
pub struct TableStructureView {
    connection_id: ConnectionId,
    kind: DataSourceKind,
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
    // Add-column dialog state
    show_add_column: bool,
    draft_column_name: String,
    draft_column_type: String,
    draft_column_nullable: bool,
    draft_column_default: String,
    add_column_error: Option<String>,
    // Drop-column confirmation
    pending_drop_column: Option<String>,
    // Modify-column dialog
    pending_modify_column: Option<String>,
    draft_modify_type: String,
    draft_modify_nullable: bool,
    draft_modify_default: String,
    modify_column_error: Option<String>,
    // Drop-index confirmation
    pending_drop_index: Option<String>,
    // Add-FK dialog
    show_add_fk: bool,
    draft_fk_name: String,
    draft_fk_columns: Vec<bool>,
    draft_fk_ref_table: String,
    draft_fk_ref_columns: Vec<String>,
    draft_fk_on_update: String,
    draft_fk_on_delete: String,
    add_fk_error: Option<String>,
    // The structure edit waiting to be picked up by the shell
    pending_alter: Option<AlterTableRequest>,
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
        kind: DataSourceKind,
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
            kind,
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
            show_add_column: false,
            draft_column_name: String::new(),
            draft_column_type: String::new(),
            draft_column_nullable: true,
            draft_column_default: String::new(),
            add_column_error: None,
            pending_drop_column: None,
            pending_modify_column: None,
            draft_modify_type: String::new(),
            draft_modify_nullable: true,
            draft_modify_default: String::new(),
            modify_column_error: None,
            pending_drop_index: None,
            show_add_fk: false,
            draft_fk_name: String::new(),
            draft_fk_columns: Vec::new(),
            draft_fk_ref_table: String::new(),
            draft_fk_ref_columns: Vec::new(),
            draft_fk_on_update: String::new(),
            draft_fk_on_delete: String::new(),
            add_fk_error: None,
            pending_alter: None,
        }
    }

    pub fn from_columns(
        connection_id: ConnectionId,
        kind: DataSourceKind,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        columns: Vec<TableColumn>,
    ) -> Self {
        let column_count = columns.len();
        Self {
            connection_id,
            kind,
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
            show_add_column: false,
            draft_column_name: String::new(),
            draft_column_type: String::new(),
            draft_column_nullable: true,
            draft_column_default: String::new(),
            add_column_error: None,
            pending_drop_column: None,
            pending_modify_column: None,
            draft_modify_type: String::new(),
            draft_modify_nullable: true,
            draft_modify_default: String::new(),
            modify_column_error: None,
            pending_drop_index: None,
            show_add_fk: false,
            draft_fk_name: String::new(),
            draft_fk_columns: Vec::new(),
            draft_fk_ref_table: String::new(),
            draft_fk_ref_columns: Vec::new(),
            draft_fk_on_update: String::new(),
            draft_fk_on_delete: String::new(),
            add_fk_error: None,
            pending_alter: None,
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

    pub fn take_alter_request(&mut self) -> Option<AlterTableRequest> {
        self.pending_alter.take()
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
        self.add_column_dialog(ui, &mut message);
        self.drop_column_dialog(ui, &mut message);
        self.modify_column_dialog(ui, &mut message);
        self.drop_index_dialog(ui, &mut message);
        self.add_fk_dialog(ui, &mut message);

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
                    Button::new("Add Column"),
                )
                .clicked()
            {
                self.prepare_add_column();
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
            if ui
                .add_enabled(
                    self.details_loaded && !self.columns.is_empty(),
                    Button::new("Add FK"),
                )
                .clicked()
            {
                self.prepare_add_fk();
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
                .filter(|(_, selected)| **selected)
                .map(|(column, _)| column.name.clone())
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

    // ---- Add Column ----

    fn prepare_add_column(&mut self) {
        self.active_tab = StructureTab::Columns;
        self.show_add_column = true;
        self.add_column_error = None;
        self.draft_column_name.clear();
        self.draft_column_type.clear();
        self.draft_column_nullable = true;
        self.draft_column_default.clear();
    }

    fn add_column_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        if !self.show_add_column {
            return;
        }
        let mut open = true;
        let mut create_requested = false;
        let mut cancel_requested = false;

        Window::new("Add Column")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(RichText::new(self.table_name()).strong());
                ui.add_space(8.0);
                Grid::new("add_column_form")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Column name");
                        ui.add(
                            TextEdit::singleline(&mut self.draft_column_name).desired_width(240.0),
                        );
                        ui.end_row();
                        ui.label("Type (e.g. VARCHAR(255))");
                        ui.add(
                            TextEdit::singleline(&mut self.draft_column_type).desired_width(240.0),
                        );
                        ui.end_row();
                        ui.label("Nullable");
                        ui.checkbox(&mut self.draft_column_nullable, "");
                        ui.end_row();
                        ui.label("Default (raw SQL, optional)");
                        ui.add(
                            TextEdit::singleline(&mut self.draft_column_default)
                                .desired_width(240.0),
                        );
                        ui.end_row();
                    });

                if let Some(error) = &self.add_column_error {
                    ui.colored_label(Color32::RED, error);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel_requested = true;
                    }
                    if ui.button("Add Column").clicked() {
                        create_requested = true;
                    }
                });
            });

        if cancel_requested || !open {
            self.show_add_column = false;
            return;
        }
        if create_requested {
            let name = self.draft_column_name.trim().to_string();
            let col_type = self.draft_column_type.trim().to_string();
            if name.is_empty() {
                self.add_column_error = Some("Column name is required".to_string());
            } else if col_type.is_empty() {
                self.add_column_error = Some("Column type is required".to_string());
            } else {
                self.pending_alter = Some(AlterTableRequest::AddColumn {
                    connection_id: self.connection_id,
                    database: self.database.clone(),
                    schema: self.schema.clone(),
                    table: self.table.clone(),
                    column_name: name,
                    column_type: col_type,
                    nullable: self.draft_column_nullable,
                    default_value: {
                        let d = self.draft_column_default.trim();
                        (!d.is_empty()).then(|| d.to_string())
                    },
                });
                self.show_add_column = false;
                *message = Some("Adding column...".to_string());
            }
        }
    }

    // ---- Drop Column ----

    fn drop_column_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        let Some(column_name) = self.pending_drop_column.take() else {
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        Window::new("Confirm Drop Column")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.colored_label(
                    Color32::from_rgb(220, 150, 60),
                    "Dropping a column permanently deletes its data.",
                );
                ui.add_space(8.0);
                ui.label(format!("Table: {}", self.table_name()));
                ui.label(format!("Column: {}", column_name));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Drop Column").clicked() {
                        confirm = true;
                    }
                });
            });
        if confirm {
            self.pending_alter = Some(AlterTableRequest::DropColumn {
                connection_id: self.connection_id,
                database: self.database.clone(),
                schema: self.schema.clone(),
                table: self.table.clone(),
                column_name: column_name.clone(),
            });
            *message = Some(format!("Dropping column {column_name}..."));
        }
        if !cancel && open {
            self.pending_drop_column = Some(column_name);
        }
    }

    // ---- Modify Column ----

    fn modify_column_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        let Some(column_name) = self.pending_modify_column.take() else {
            return;
        };
        // Sync draft fields with the existing column the first time.
        if self.draft_modify_type.is_empty() {
            if let Some(col) = self.columns.iter().find(|c| c.name == column_name) {
                self.draft_modify_type = col.database_type.clone();
                self.draft_modify_nullable = col.nullable;
                self.draft_modify_default = col.default_value.clone().unwrap_or_default();
            }
        }
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        Window::new("Modify Column")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(RichText::new(self.table_name()).strong());
                ui.label(format!("Column: {}", column_name));
                ui.add_space(8.0);
                Grid::new("modify_column_form")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("New type");
                        ui.add(
                            TextEdit::singleline(&mut self.draft_modify_type).desired_width(240.0),
                        );
                        ui.end_row();
                        ui.label("Nullable");
                        ui.checkbox(&mut self.draft_modify_nullable, "");
                        ui.end_row();
                        ui.label("Default (raw SQL, optional)");
                        ui.add(
                            TextEdit::singleline(&mut self.draft_modify_default)
                                .desired_width(240.0),
                        );
                        ui.end_row();
                    });
                if self.kind == DataSourceKind::Sqlite {
                    ui.colored_label(
                        Color32::from_rgb(220, 150, 60),
                        "SQLite has limited ALTER COLUMN support; type changes may fail.",
                    );
                }
                if let Some(error) = &self.modify_column_error {
                    ui.colored_label(Color32::RED, error);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Apply").clicked() {
                        confirm = true;
                    }
                });
            });
        if confirm {
            let new_type = self.draft_modify_type.trim().to_string();
            if new_type.is_empty() {
                self.modify_column_error = Some("Type is required".to_string());
                self.pending_modify_column = Some(column_name);
                return;
            }
            self.pending_alter = Some(AlterTableRequest::ModifyColumn {
                connection_id: self.connection_id,
                database: self.database.clone(),
                schema: self.schema.clone(),
                table: self.table.clone(),
                column_name: column_name.clone(),
                new_type,
                nullable: self.draft_modify_nullable,
                default_value: {
                    let d = self.draft_modify_default.trim();
                    (!d.is_empty()).then(|| d.to_string())
                },
            });
            self.draft_modify_type.clear();
            *message = Some(format!("Modifying column {column_name}..."));
        } else if !cancel && open {
            self.pending_modify_column = Some(column_name);
        } else {
            self.draft_modify_type.clear();
        }
    }

    // ---- Drop Index ----

    fn drop_index_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        let Some(index_name) = self.pending_drop_index.take() else {
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        Window::new("Confirm Drop Index")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(RichText::new("Drop index?").strong());
                ui.add_space(8.0);
                ui.label(format!("Table: {}", self.table_name()));
                ui.label(format!("Index: {}", index_name));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Drop Index").clicked() {
                        confirm = true;
                    }
                });
            });
        if confirm {
            self.pending_alter = Some(AlterTableRequest::DropIndex {
                connection_id: self.connection_id,
                database: self.database.clone(),
                schema: self.schema.clone(),
                table: self.table.clone(),
                index_name: index_name.clone(),
            });
            *message = Some(format!("Dropping index {index_name}..."));
        }
        if !cancel && open {
            self.pending_drop_index = Some(index_name);
        }
    }

    // ---- Add Foreign Key ----

    fn prepare_add_fk(&mut self) {
        self.active_tab = StructureTab::ForeignKeys;
        self.show_add_fk = true;
        self.add_fk_error = None;
        self.draft_fk_name.clear();
        self.draft_fk_columns = vec![false; self.columns.len()];
        self.draft_fk_ref_table.clear();
        self.draft_fk_ref_columns = Vec::new();
        self.draft_fk_on_update.clear();
        self.draft_fk_on_delete.clear();
    }

    fn add_fk_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        if !self.show_add_fk {
            return;
        }
        if self.draft_fk_columns.len() != self.columns.len() {
            self.draft_fk_columns.resize(self.columns.len(), false);
        }
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;

        Window::new("Add Foreign Key")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(RichText::new(self.table_name()).strong());
                ui.add_space(8.0);
                Grid::new("add_fk_form")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Constraint name");
                        ui.add(TextEdit::singleline(&mut self.draft_fk_name).desired_width(240.0));
                        ui.end_row();
                        ui.label("Referenced table");
                        ui.add(
                            TextEdit::singleline(&mut self.draft_fk_ref_table).desired_width(240.0),
                        );
                        ui.end_row();
                        ui.label("ON UPDATE");
                        ComboBox::from_id_source("fk_on_update")
                            .selected_text(
                                if self.draft_fk_on_update.is_empty() {
                                    "—"
                                } else {
                                    &self.draft_fk_on_update
                                }
                                .to_string(),
                            )
                            .show_ui(ui, |ui| {
                                let opts = [
                                    "",
                                    "CASCADE",
                                    "SET NULL",
                                    "RESTRICT",
                                    "NO ACTION",
                                    "SET DEFAULT",
                                ];
                                for opt in opts {
                                    ui.selectable_value(
                                        &mut self.draft_fk_on_update,
                                        opt.to_string(),
                                        opt,
                                    );
                                }
                            });
                        ui.end_row();
                        ui.label("ON DELETE");
                        ComboBox::from_id_source("fk_on_delete")
                            .selected_text(
                                if self.draft_fk_on_delete.is_empty() {
                                    "—"
                                } else {
                                    &self.draft_fk_on_delete
                                }
                                .to_string(),
                            )
                            .show_ui(ui, |ui| {
                                let opts = [
                                    "",
                                    "CASCADE",
                                    "SET NULL",
                                    "RESTRICT",
                                    "NO ACTION",
                                    "SET DEFAULT",
                                ];
                                for opt in opts {
                                    ui.selectable_value(
                                        &mut self.draft_fk_on_delete,
                                        opt.to_string(),
                                        opt,
                                    );
                                }
                            });
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.label(RichText::new("Local columns").strong());
                ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                    for (index, column) in self.columns.iter().enumerate() {
                        ui.checkbox(&mut self.draft_fk_columns[index], &column.name);
                    }
                });

                ui.add_space(4.0);
                ui.label(RichText::new(
                    "Referenced columns (comma-separated, one per selected local column)",
                ));
                let mut ref_cols_text = self.draft_fk_ref_columns.join(", ");
                ui.add(TextEdit::singleline(&mut ref_cols_text).desired_width(300.0));
                self.draft_fk_ref_columns = ref_cols_text
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                if let Some(error) = &self.add_fk_error {
                    ui.colored_label(Color32::RED, error);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Add Foreign Key").clicked() {
                        confirm = true;
                    }
                });
            });

        if cancel || !open {
            self.show_add_fk = false;
            return;
        }
        if confirm {
            let name = self.draft_fk_name.trim().to_string();
            let ref_table = self.draft_fk_ref_table.trim().to_string();
            let columns = self
                .columns
                .iter()
                .zip(self.draft_fk_columns.iter())
                .filter(|(_, sel)| **sel)
                .map(|(c, _)| c.name.clone())
                .collect::<Vec<_>>();

            if name.is_empty() {
                self.add_fk_error = Some("Constraint name is required".to_string());
            } else if ref_table.is_empty() {
                self.add_fk_error = Some("Referenced table is required".to_string());
            } else if columns.is_empty() {
                self.add_fk_error = Some("Select at least one local column".to_string());
            } else if columns.len() != self.draft_fk_ref_columns.len() {
                self.add_fk_error = Some(format!(
                    "Expected {} referenced column(s), got {}",
                    columns.len(),
                    self.draft_fk_ref_columns.len()
                ));
            } else {
                self.pending_alter = Some(AlterTableRequest::AddForeignKey {
                    connection_id: self.connection_id,
                    database: self.database.clone(),
                    schema: self.schema.clone(),
                    table: self.table.clone(),
                    constraint_name: name,
                    columns,
                    referenced_table: ref_table,
                    referenced_columns: self.draft_fk_ref_columns.clone(),
                    on_update: {
                        let v = self.draft_fk_on_update.trim().to_string();
                        (!v.is_empty()).then_some(v)
                    },
                    on_delete: {
                        let v = self.draft_fk_on_delete.trim().to_string();
                        (!v.is_empty()).then_some(v)
                    },
                });
                self.show_add_fk = false;
                *message = Some("Adding foreign key...".to_string());
            }
        }
    }

    /// Renders the columns section as a table.
    fn columns_table(&mut self, ui: &mut Ui) {
        if self.columns.is_empty() {
            empty_message(ui, "No columns");
            return;
        }

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new("structure_columns_grid")
                    .striped(true)
                    .num_columns(7)
                    .spacing([8.0, 0.0])
                    .show(ui, |ui| {
                        for label in &["Name", "Type", "Null", "Default", "Key", "", ""] {
                            ui.label(RichText::new(*label).strong());
                        }
                        ui.end_row();

                        let mut drop_requested = None;
                        let mut modify_requested = None;
                        for col in &self.columns {
                            ui.label(&col.name);
                            ui.label(type_without_field_width(&col.database_type))
                                .on_hover_text(&col.database_type);
                            ui.label(if col.nullable { "YES" } else { "NO" });
                            ui.label(col.default_value.as_deref().unwrap_or(""));
                            ui.label(if col.primary_key { "PK" } else { "" });
                            if ui.small_button("✎").clicked() {
                                modify_requested = Some(col.name.clone());
                            }
                            if ui.small_button("🗑").clicked() {
                                drop_requested = Some(col.name.clone());
                            }
                            ui.end_row();
                        }
                        if let Some(name) = drop_requested {
                            self.pending_drop_column = Some(name);
                        }
                        if let Some(name) = modify_requested {
                            // Reset draft fields so the dialog picks up the new column.
                            self.draft_modify_type.clear();
                            self.pending_modify_column = Some(name);
                        }
                    });
            });
    }

    /// Renders the indexes section as a table.
    fn indexes_table(&mut self, ui: &mut Ui) {
        if self.indexes.is_empty() {
            empty_message(ui, "No indexes");
            return;
        }

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new("structure_indexes_grid")
                    .striped(true)
                    .num_columns(5)
                    .spacing([8.0, 0.0])
                    .show(ui, |ui| {
                        for label in &["Name", "Columns", "Unique", "Primary", ""] {
                            ui.label(RichText::new(*label).strong());
                        }
                        ui.end_row();

                        let mut drop_requested = None;
                        for idx in &self.indexes {
                            ui.label(&idx.name);
                            ui.label(idx.columns.join(", "));
                            ui.label(if idx.unique { "YES" } else { "" });
                            ui.label(if idx.primary { "YES" } else { "" });
                            if !idx.primary {
                                if ui.small_button("🗑").clicked() {
                                    drop_requested = Some(idx.name.clone());
                                }
                            } else {
                                ui.label("");
                            }
                            ui.end_row();
                        }
                        if let Some(name) = drop_requested {
                            self.pending_drop_index = Some(name);
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
                    .spacing([8.0, 0.0])
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
                    .spacing([8.0, 0.0])
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
                    .spacing([8.0, 0.0])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> (AlterTableRequest, ConnectionId) {
        let conn = ConnectionId::new();
        let req = AlterTableRequest::AddColumn {
            connection_id: conn,
            database: None,
            schema: Some("public".to_string()),
            table: "users".to_string(),
            column_name: "email".to_string(),
            column_type: "VARCHAR(255)".to_string(),
            nullable: false,
            default_value: Some("'unknown'".to_string()),
        };
        (req, conn)
    }

    #[test]
    fn generates_add_column_for_postgres() {
        let (req, _) = sample_request();
        let sql = build_alter_table_sql(&req, DataSourceKind::PostgreSql).unwrap();
        // schema qualifier is prepended when present
        assert!(sql.contains("ALTER TABLE \"public\".\"users\""));
        assert!(sql.contains("ADD COLUMN \"email\" VARCHAR(255) NOT NULL DEFAULT 'unknown'"));
    }

    #[test]
    fn generates_add_column_for_mysql() {
        let (req, _) = sample_request();
        let sql = build_alter_table_sql(&req, DataSourceKind::MySql).unwrap();
        // MySQL falls back to schema when database is absent
        assert!(sql.contains("ALTER TABLE `public`.`users`"));
        assert!(sql.contains("ADD COLUMN `email` VARCHAR(255) NOT NULL DEFAULT 'unknown'"));
    }

    #[test]
    fn generates_drop_column() {
        let conn = ConnectionId::new();
        let req = AlterTableRequest::DropColumn {
            connection_id: conn,
            database: None,
            schema: None,
            table: "users".to_string(),
            column_name: "email".to_string(),
        };
        let sql = build_alter_table_sql(&req, DataSourceKind::PostgreSql).unwrap();
        assert_eq!(sql, "ALTER TABLE \"users\" DROP COLUMN \"email\"");
    }

    #[test]
    fn generates_modify_column_mysql() {
        let conn = ConnectionId::new();
        let req = AlterTableRequest::ModifyColumn {
            connection_id: conn,
            database: None,
            schema: None,
            table: "users".to_string(),
            column_name: "email".to_string(),
            new_type: "VARCHAR(512)".to_string(),
            nullable: true,
            default_value: None,
        };
        let sql = build_alter_table_sql(&req, DataSourceKind::MySql).unwrap();
        assert!(sql.contains("MODIFY COLUMN `email` VARCHAR(512)"));
    }

    #[test]
    fn generates_modify_column_postgres() {
        let conn = ConnectionId::new();
        let req = AlterTableRequest::ModifyColumn {
            connection_id: conn,
            database: None,
            schema: None,
            table: "users".to_string(),
            column_name: "email".to_string(),
            new_type: "VARCHAR(512)".to_string(),
            nullable: false,
            default_value: None,
        };
        let sql = build_alter_table_sql(&req, DataSourceKind::PostgreSql).unwrap();
        assert!(sql.contains("ALTER COLUMN \"email\" TYPE VARCHAR(512)"));
        assert!(sql.contains("SET NOT NULL"));
    }

    #[test]
    fn generates_drop_index_mysql() {
        let conn = ConnectionId::new();
        let req = AlterTableRequest::DropIndex {
            connection_id: conn,
            database: Some("mydb".to_string()),
            schema: None,
            table: "users".to_string(),
            index_name: "idx_email".to_string(),
        };
        let sql = build_alter_table_sql(&req, DataSourceKind::MySql).unwrap();
        assert!(sql.contains("DROP INDEX `idx_email` ON `mydb`.`users`"));
    }

    #[test]
    fn generates_drop_index_postgres() {
        let conn = ConnectionId::new();
        let req = AlterTableRequest::DropIndex {
            connection_id: conn,
            database: None,
            schema: Some("public".to_string()),
            table: "users".to_string(),
            index_name: "idx_email".to_string(),
        };
        let sql = build_alter_table_sql(&req, DataSourceKind::PostgreSql).unwrap();
        assert_eq!(sql, "DROP INDEX IF EXISTS \"public\".\"idx_email\"");
    }

    #[test]
    fn generates_add_foreign_key() {
        let conn = ConnectionId::new();
        let req = AlterTableRequest::AddForeignKey {
            connection_id: conn,
            database: None,
            schema: None,
            table: "orders".to_string(),
            constraint_name: "fk_orders_user".to_string(),
            columns: vec!["user_id".to_string()],
            referenced_table: "users".to_string(),
            referenced_columns: vec!["id".to_string()],
            on_update: Some("CASCADE".to_string()),
            on_delete: None,
        };
        let sql = build_alter_table_sql(&req, DataSourceKind::PostgreSql).unwrap();
        assert!(sql.contains("ADD CONSTRAINT \"fk_orders_user\""));
        assert!(sql.contains("FOREIGN KEY (\"user_id\") REFERENCES \"users\" (\"id\")"));
        assert!(sql.contains("ON UPDATE CASCADE"));
        assert!(!sql.contains("ON DELETE"));
    }
}
