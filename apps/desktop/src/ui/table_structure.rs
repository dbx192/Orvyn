#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

use crate::ui::i18n::text as tr;
use db_common::{
    dialect::{BacktickDialect, Dialect, DoubleQuoteDialect, Identifier},
    AppError, ConnectionId, DataSourceKind, DatabaseFunction, ProtocolFamily, TableColumn,
    TableForeignKey, TableIndex, TableTrigger,
};
use eframe::egui::{
    self, Button, Color32, ComboBox, Frame, Grid, Margin, RichText, ScrollArea, TextEdit, TextStyle,
    Ui, Window,
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

/// A column definition that can be copied on the clipboard for Navicat-style
/// copy / paste inside the structure designer.
#[derive(Debug, Clone, Default)]
pub struct StagedColumn {
    pub column_type: String,
    pub nullable: bool,
    pub default_value: Option<String>,
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
    RenameColumn {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        old_name: String,
        new_name: String,
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
            | AlterTableRequest::RenameColumn { connection_id, .. }
            | AlterTableRequest::DropIndex { connection_id, .. }
            | AlterTableRequest::AddForeignKey { connection_id, .. } => *connection_id,
        }
    }

    pub fn table(&self) -> &str {
        match self {
            AlterTableRequest::AddColumn { table, .. }
            | AlterTableRequest::DropColumn { table, .. }
            | AlterTableRequest::ModifyColumn { table, .. }
            | AlterTableRequest::RenameColumn { table, .. }
            | AlterTableRequest::DropIndex { table, .. }
            | AlterTableRequest::AddForeignKey { table, .. } => table,
        }
    }

    pub fn database(&self) -> Option<&str> {
        match self {
            AlterTableRequest::AddColumn { database, .. }
            | AlterTableRequest::DropColumn { database, .. }
            | AlterTableRequest::ModifyColumn { database, .. }
            | AlterTableRequest::RenameColumn { database, .. }
            | AlterTableRequest::DropIndex { database, .. }
            | AlterTableRequest::AddForeignKey { database, .. } => database.as_deref(),
        }
    }

    pub fn schema(&self) -> Option<&str> {
        match self {
            AlterTableRequest::AddColumn { schema, .. }
            | AlterTableRequest::DropColumn { schema, .. }
            | AlterTableRequest::ModifyColumn { schema, .. }
            | AlterTableRequest::RenameColumn { schema, .. }
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
            match kind.protocol_family() {
                ProtocolFamily::MySql => Ok(format!(
                    "ALTER TABLE {table_sql} MODIFY COLUMN {column_def}"
                )),
                ProtocolFamily::PostgreSql => {
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
                ProtocolFamily::Sqlite | ProtocolFamily::DuckDb => {
                    // SQLite < 3.35 doesn't support full ALTER COLUMN; DuckDB does.
                    Ok(format!("ALTER TABLE {table_sql} ALTER COLUMN {column_sql} SET DATA TYPE {new_type}"))
                }
                ProtocolFamily::SqlServer
                | ProtocolFamily::ClickHouse
                | ProtocolFamily::Tdengine => Err(AppError::unsupported(
                    "ALTER COLUMN is not available for this data source yet",
                )),
                _ => Err(AppError::validation(
                    "unsupported data source for ALTER TABLE",
                )),
            }
        }
        AlterTableRequest::RenameColumn {
            table,
            old_name,
            new_name,
            ..
        } => {
            let table_sql = qualified_table_sql(
                dialect.as_ref(),
                kind,
                request.database(),
                request.schema(),
                table,
            )?;
            let old_sql = quote_id(dialect.as_ref(), old_name)?;
            let new_sql = quote_id(dialect.as_ref(), new_name)?;
            match kind.protocol_family() {
                // MySQL and Postgres both support `RENAME COLUMN old TO new`.
                ProtocolFamily::MySql | ProtocolFamily::PostgreSql => Ok(format!(
                    "ALTER TABLE {table_sql} RENAME COLUMN {old_sql} TO {new_sql}"
                )),
                ProtocolFamily::Sqlite | ProtocolFamily::DuckDb => Ok(format!(
                    "ALTER TABLE {table_sql} RENAME COLUMN {old_sql} TO {new_sql}"
                )),
                ProtocolFamily::SqlServer => Ok(format!(
                    "EXEC sp_rename '{}.{}', '{}', 'COLUMN'",
                    qualified_table_sql(dialect.as_ref(), kind, None, None, table)?,
                    old_name,
                    new_name,
                )),
                ProtocolFamily::ClickHouse
                | ProtocolFamily::Tdengine => Err(AppError::unsupported(
                    "RENAME COLUMN is not available for this data source yet",
                )),
                _ => Err(AppError::validation(
                    "unsupported data source for RENAME COLUMN",
                )),
            }
        }
        AlterTableRequest::DropIndex {
            table, index_name, ..
        } => {
            let _ = table;
            let index_sql = quote_id(dialect.as_ref(), index_name)?;
            match kind.protocol_family() {
                ProtocolFamily::MySql => {
                    let table_sql = qualified_table_sql(
                        dialect.as_ref(),
                        kind,
                        request.database(),
                        request.schema(),
                        table,
                    )?;
                    Ok(format!("DROP INDEX {index_sql} ON {table_sql}"))
                }
                ProtocolFamily::PostgreSql => {
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
                ProtocolFamily::Sqlite | ProtocolFamily::DuckDb => {
                    Ok(format!("DROP INDEX IF EXISTS {index_sql}"))
                }
                ProtocolFamily::SqlServer
                | ProtocolFamily::ClickHouse
                | ProtocolFamily::Tdengine => Err(AppError::unsupported(
                    "DROP INDEX is not available for this data source yet",
                )),
                _ => Err(AppError::validation(
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
    let qualifier = match kind.protocol_family() {
        ProtocolFamily::MySql => database.or(schema),
        _ => schema,
    };
    match qualifier.filter(|value| !value.is_empty()) {
        Some(qualifier) => Ok(format!("{}.{table_sql}", quote_id(dialect, qualifier)?)),
        None => Ok(table_sql),
    }
}

fn dialect_for_kind(kind: DataSourceKind) -> Box<dyn Dialect> {
    match kind.protocol_family() {
        ProtocolFamily::MySql => Box::new(BacktickDialect),
        _ => Box::new(DoubleQuoteDialect),
    }
}

/// Serialize a column to a single text line for the OS clipboard, in a form
/// `parse_clipboard_column` understands, so a column copied in the designer can
/// be pasted back (or into a SQL editor / another table designer).
fn column_to_clipboard_text(col: &TableColumn) -> String {
    let mut s = format!("{} {}", col.name, col.database_type);
    if !col.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(default) = &col.default_value {
        s.push_str(" DEFAULT ");
        s.push_str(default);
    }
    s
}

/// Best-effort parse of clipboard text into a column definition.
/// Returns `(name, type, nullable, default_value)`.
fn parse_clipboard_column(text: &str) -> Option<(String, String, bool, Option<String>)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }
    let name = tokens[0].to_string();
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    // The type runs from the second token up to the first SQL keyword.
    const KEYWORDS: [&str; 9] = [
        "NOT", "NULL", "DEFAULT", "PRIMARY", "KEY", "UNIQUE", "AUTO_INCREMENT", "COMMENT",
        "REFERENCES",
    ];
    let upper: Vec<String> = tokens.iter().map(|t| t.to_ascii_uppercase()).collect();
    let mut type_tokens: Vec<&str> = Vec::new();
    for (i, token) in tokens.iter().enumerate().skip(1) {
        if KEYWORDS.contains(&upper[i].as_str()) {
            break;
        }
        type_tokens.push(token);
    }
    if type_tokens.is_empty() {
        return None;
    }
    let nullable = !(upper.iter().any(|t| t == "NOT") && upper.iter().any(|t| t == "NULL"));
    let default_value = upper.iter().position(|t| t == "DEFAULT").and_then(|pos| {
        tokens.get(pos + 1).map(|v| {
            v.trim_matches(|c| c == '\'' || c == '"')
                .to_string()
        })
    });
    Some((name, type_tokens.join(" "), nullable, default_value))
}

/// A dedicated tab that displays the structure of a database table.
pub struct TableStructureView {
    connection_id: ConnectionId,
    kind: DataSourceKind,
    database: Option<String>,
    schema: Option<String>,
    table: String,
    columns: Vec<TableColumn>,
    /// Snapshot of the columns as loaded, used to diff staged edits on Save.
    original_columns: Vec<TableColumn>,
    /// Per-row identity of each column in `columns`: `Some(original_name)`
    /// for columns that existed at load, `None` for rows added since (e.g. via
    /// paste or the Add-Column dialog), so the Save diff can detect adds,
    /// renames and drops.
    col_original_name: Vec<Option<String>>,
    /// Last column copied via the context menu (for Paste).
    copied_column: Option<StagedColumn>,
    /// Row whose Name cell is being edited inline (Navicat designer style).
    editing_name: Option<usize>,
    /// Currently selected row in the field designer.
    selected_column: Option<usize>,
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
    // Drop-column confirmation
    pending_drop_column: Option<String>,
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
    // The structure edits waiting to be picked up by the shell. A Save runs
    // a whole batch (add / rename / modify / drop) in one go.
    pending_alter_batch: Vec<AlterTableRequest>,
    /// Set when the user clicks Refresh and there are no staged changes; the
    /// shell picks it up and re-loads the structure from the database.
    refresh_requested: bool,
    /// Set when Refresh is clicked with staged changes, asking for confirmation
    /// before discarding them.
    confirm_refresh: bool,
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
        let col_original_name = columns.iter().map(|c| Some(c.name.clone())).collect();
        Self {
            connection_id,
            kind,
            database,
            schema,
            table,
            columns: columns.clone(),
            original_columns: columns,
            col_original_name,
            copied_column: None,
            editing_name: None,
            selected_column: (column_count > 0).then_some(0),
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
            pending_drop_column: None,
            pending_drop_index: None,
            show_add_fk: false,
            draft_fk_name: String::new(),
            draft_fk_columns: Vec::new(),
            draft_fk_ref_table: String::new(),
            draft_fk_ref_columns: Vec::new(),
            draft_fk_on_update: String::new(),
            draft_fk_on_delete: String::new(),
            add_fk_error: None,
            pending_alter_batch: Vec::new(),
            refresh_requested: false,
            confirm_refresh: false,
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
        let col_original_name = columns.iter().map(|c| Some(c.name.clone())).collect();
        Self {
            connection_id,
            kind,
            database,
            schema,
            table,
            columns: columns.clone(),
            original_columns: columns,
            col_original_name,
            copied_column: None,
            editing_name: None,
            selected_column: (column_count > 0).then_some(0),
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
            pending_drop_column: None,
            pending_drop_index: None,
            show_add_fk: false,
            draft_fk_name: String::new(),
            draft_fk_columns: Vec::new(),
            draft_fk_ref_table: String::new(),
            draft_fk_ref_columns: Vec::new(),
            draft_fk_on_update: String::new(),
            draft_fk_on_delete: String::new(),
            add_fk_error: None,
            pending_alter_batch: Vec::new(),
            refresh_requested: false,
            confirm_refresh: false,
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

    pub fn take_alter_requests(&mut self) -> Vec<AlterTableRequest> {
        std::mem::take(&mut self.pending_alter_batch)
    }

    /// True once the user asked to re-load the structure from the database.
    pub fn take_refresh_request(&mut self) -> bool {
        let requested = self.refresh_requested;
        self.refresh_requested = false;
        requested
    }

    /// Pick a column name that does not collide with any row currently in the
    /// designer, derived from `base` (e.g. `name_2`, `name_3`, ...).
    fn unique_column_name(&self, base: &str) -> String {
        if !self.columns.iter().any(|c| c.name == base) {
            return base.to_string();
        }
        let mut counter = 2;
        loop {
            let candidate = format!("{base}_{counter}");
            if !self.columns.iter().any(|c| c.name == candidate) {
                return candidate;
            }
            counter += 1;
        }
    }

    fn queue_alter(&mut self, request: AlterTableRequest) {
        self.pending_alter_batch.push(request);
    }

    /// Diff the staged columns against the original snapshot and produce the
    /// ordered ALTER requests (adds / renames / modifies first, drops last)
    /// that persist the current design.
    fn staged_requests(&self) -> Vec<AlterTableRequest> {
        use std::collections::{HashMap, HashSet};
        let c = self.connection_id;
        let db = self.database.clone();
        let sc = self.schema.clone();
        let tbl = self.table.clone();

        let orig_by_name: HashMap<&str, &TableColumn> = self
            .original_columns
            .iter()
            .map(|col| (col.name.as_str(), col))
            .collect();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut changes: Vec<AlterTableRequest> = Vec::new();
        let mut drops: Vec<AlterTableRequest> = Vec::new();

        for (idx, col) in self.columns.iter().enumerate() {
            match self.col_original_name[idx].as_deref() {
                Some(orig) => {
                    if orig != col.name {
                        changes.push(AlterTableRequest::RenameColumn {
                            connection_id: c,
                            database: db.clone(),
                            schema: sc.clone(),
                            table: tbl.clone(),
                            old_name: orig.to_string(),
                            new_name: col.name.clone(),
                        });
                    }
                    if let Some(original) = orig_by_name.get(orig) {
                        seen.insert(orig);
                        if original.database_type != col.database_type
                            || original.nullable != col.nullable
                            || original.default_value != col.default_value
                        {
                            changes.push(AlterTableRequest::ModifyColumn {
                                connection_id: c,
                                database: db.clone(),
                                schema: sc.clone(),
                                table: tbl.clone(),
                                column_name: col.name.clone(),
                                new_type: col.database_type.clone(),
                                nullable: col.nullable,
                                default_value: col.default_value.clone(),
                            });
                        }
                    }
                }
                None => {
                    changes.push(AlterTableRequest::AddColumn {
                        connection_id: c,
                        database: db.clone(),
                        schema: sc.clone(),
                        table: tbl.clone(),
                        column_name: col.name.clone(),
                        column_type: col.database_type.clone(),
                        nullable: col.nullable,
                        default_value: col.default_value.clone(),
                    });
                }
            }
        }
        for original in &self.original_columns {
            if !seen.contains(original.name.as_str()) {
                drops.push(AlterTableRequest::DropColumn {
                    connection_id: c,
                    database: db.clone(),
                    schema: sc.clone(),
                    table: tbl.clone(),
                    column_name: original.name.clone(),
                });
            }
        }
        changes.extend(drops);
        changes
    }

    /// True when the designer has staged column edits waiting to be saved.
    fn has_pending_changes(&self) -> bool {
        !self.staged_requests().is_empty()
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
        self.drop_column_dialog(ui, &mut message);
        self.drop_index_dialog(ui, &mut message);
        self.add_fk_dialog(ui, &mut message);
        self.refresh_dialog(ui, &mut message);

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(self.table_name()).strong().size(15.0));
            if !self.details_loaded {
                ui.label(
                    RichText::new(tr("Details load on demand"))
                        .small()
                        .color(Color32::GRAY),
                );
            }
            ui.separator();
            if ui
                .add_enabled(
                    self.details_loaded,
                    eframe::egui::Button::new(tr("Copy DDL")),
                )
                .clicked()
            {
                ui.output_mut(|o| o.copied_text = self.create_sql.clone());
                message = Some("CREATE TABLE DDL copied to clipboard".to_string());
            }
            // Designer field actions: add / delete / primary key / order.
            let selected = self.selected_column.filter(|i| *i < self.columns.len());
            if ui
                .add_enabled(self.details_loaded, Button::new(tr("Add Column")))
                .on_hover_text(tr("Add a column to the table"))
                .clicked()
            {
                self.add_inline_column(&mut message);
            }
            if ui
                .add_enabled(
                    self.details_loaded && selected.is_some(),
                    Button::new(tr("Delete")),
                )
                .on_hover_text(tr("Delete the selected column"))
                .clicked()
            {
                if let Some(index) = selected {
                    self.pending_drop_column = Some(self.columns[index].name.clone());
                }
            }
            ui.separator();
            if ui
                .add_enabled(
                    self.details_loaded
                        && selected.is_some_and(|i| i > 0),
                    Button::new("↑").min_size(egui::vec2(20.0, 18.0)),
                )
                .on_hover_text(tr("Move selected column up"))
                .clicked()
            {
                self.move_selected_column(-1, &mut message);
            }
            if ui
                .add_enabled(
                    self.details_loaded
                        && selected.is_some_and(|i| i + 1 < self.columns.len()),
                    Button::new("↓").min_size(egui::vec2(20.0, 18.0)),
                )
                .on_hover_text(tr("Move selected column down"))
                .clicked()
            {
                self.move_selected_column(1, &mut message);
            }
            if ui
                .add_enabled(
                    self.details_loaded && selected.is_some(),
                    Button::new(tr("Primary key")),
                )
                .on_hover_text(tr("Toggle the selected column as primary key"))
                .clicked()
            {
                if let Some(index) = selected {
                    self.columns[index].primary_key = !self.columns[index].primary_key;
                }
            }
            ui.separator();
            if ui
                .add_enabled(
                    self.details_loaded && !self.columns.is_empty(),
                    Button::new(tr("Add Index")),
                )
                .clicked()
            {
                self.prepare_add_index();
            }
            if ui
                .add_enabled(
                    self.details_loaded && !self.columns.is_empty(),
                    Button::new(tr("Add FK")),
                )
                .clicked()
            {
                self.prepare_add_fk();
            }
            ui.separator();
            let has_changes = self.has_pending_changes();
            if ui
                .add_enabled(self.details_loaded, Button::new(tr("Refresh")))
                .on_hover_text(tr("Reload the table structure from the database"))
                .clicked()
            {
                if self.has_pending_changes() {
                    self.confirm_refresh = true;
                } else {
                    self.refresh_requested = true;
                    message = Some("Refreshing structure...".to_string());
                }
            }
            if ui
                .add_enabled(has_changes, Button::new(tr("Save")))
                .on_hover_text(tr("Apply all pending structure changes"))
                .clicked()
            {
                let requests = self.staged_requests();
                let count = requests.len();
                self.pending_alter_batch.extend(requests);
                message = Some(format!("Saving {} structure change(s)...", count));
            }
        });

        ui.add_space(4.0);
        self.tabs(ui);
        ui.add_space(2.0);

        Frame::default()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .inner_margin(Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| match self.active_tab {
                StructureTab::Columns => {
                    if let Some(m) = self.columns_table(ui) {
                        message = Some(m);
                    }
                    self.column_properties(ui);
                }
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
                format!("{} {}", tr("Columns"), self.columns.len()),
            );
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::Indexes,
                format!("{} {}", tr("Indexes"), self.indexes.len()),
            );
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::ForeignKeys,
                format!("{} {}", tr("Foreign Keys"), self.foreign_keys.len()),
            );
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::Triggers,
                format!("{} {}", tr("Triggers"), self.triggers.len()),
            );
            ui.selectable_value(
                &mut self.active_tab,
                StructureTab::Functions,
                format!("{} {}", tr("Functions"), self.functions.len()),
            );
            ui.selectable_value(&mut self.active_tab, StructureTab::Ddl, tr("DDL"));
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

        Window::new(tr("Add Index"))
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
                        ui.label(tr("Name"));
                        ui.add(
                            TextEdit::singleline(&mut self.draft_index_name).desired_width(260.0),
                        );
                        ui.end_row();
                        ui.label(tr("Unique"));
                        ui.checkbox(&mut self.draft_index_unique, "");
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.label(RichText::new(tr("Columns")).strong());
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
                    if ui.button(tr("Cancel")).clicked() {
                        cancel_requested = true;
                    }
                    if ui.button(tr("Create")).clicked() {
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

    // ---- Add Column (inline designer) ----

    /// Insert a new blank row into the field grid for inline editing, selecting
    /// it immediately so the user can type a name. The column is only persisted
    /// when the designer is saved.
    fn add_inline_column(&mut self, message: &mut Option<String>) {
        self.active_tab = StructureTab::Columns;
        let default_type = common_column_types(self.kind)
            .first()
            .map(|s| type_without_field_width(s))
            .unwrap_or_else(|| "TEXT".to_string());
        let name = self.unique_column_name("new_column");
        self.columns.push(TableColumn {
            name,
            database_type: default_type,
            nullable: true,
            default_value: None,
            primary_key: false,
            comment: None,
        });
        self.col_original_name.push(None);
        self.editing_name = Some(self.columns.len() - 1);
        self.selected_column = Some(self.columns.len() - 1);
        *message = Some(
            tr("Column added; edit its properties inline and click Save")
                .to_string(),
        );
    }

    /// Move the currently selected column up or down in the designer display
    /// order (and keep the selection tracking the moved row).
    fn move_selected_column(&mut self, delta: i32, message: &mut Option<String>) {
        let Some(from) = self.selected_column.filter(|i| *i < self.columns.len()) else {
            return;
        };
        let to = from as i32 + delta;
        if to < 0 || (to as usize) >= self.columns.len() {
            return;
        }
        self.columns.swap(from, to as usize);
        self.col_original_name.swap(from, to as usize);
        self.selected_column = Some(to as usize);
        if self.editing_name == Some(from) {
            self.editing_name = Some(to as usize);
        } else if self.editing_name == Some(to as usize) {
            self.editing_name = Some(from);
        }
        *message = Some(
            if delta < 0 {
                "Moved column up".to_string()
            } else {
                "Moved column down".to_string()
            },
        );
    }

    // ---- Drop Column ----

    fn drop_column_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        let Some(column_name) = self.pending_drop_column.take() else {
            return;
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        Window::new(tr("Confirm Drop Column"))
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
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("Drop Column")).clicked() {
                        confirm = true;
                    }
                });
            });
        if confirm {
            if let Some(index) = self.columns.iter().position(|col| col.name == column_name) {
                self.columns.remove(index);
                self.col_original_name.remove(index);
                self.selected_column =
                    self.selected_column
                        .and_then(|selected| match selected.cmp(&index) {
                            std::cmp::Ordering::Less => Some(selected),
                            std::cmp::Ordering::Equal => (!self.columns.is_empty())
                                .then_some(index.min(self.columns.len() - 1)),
                            std::cmp::Ordering::Greater => Some(selected - 1),
                        });
                *message = Some(format!("Column {column_name} marked for deletion"));
            }
        }
        if !cancel && open {
            self.pending_drop_column = Some(column_name);
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
        Window::new(tr("Confirm Drop Index"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(RichText::new(tr("Drop index?")).strong());
                ui.add_space(8.0);
                ui.label(format!("Table: {}", self.table_name()));
                ui.label(format!("Index: {}", index_name));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("Drop Index")).clicked() {
                        confirm = true;
                    }
                });
            });
        if confirm {
            self.queue_alter(AlterTableRequest::DropIndex {
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

        Window::new(tr("Add Foreign Key"))
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
                        ui.label(tr("Constraint name"));
                        ui.add(TextEdit::singleline(&mut self.draft_fk_name).desired_width(240.0));
                        ui.end_row();
                        ui.label(tr("Referenced table"));
                        ui.add(
                            TextEdit::singleline(&mut self.draft_fk_ref_table).desired_width(240.0),
                        );
                        ui.end_row();
                        ui.label(tr("ON UPDATE"));
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
                        ui.label(tr("ON DELETE"));
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
                ui.label(RichText::new(tr("Local columns")).strong());
                ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                    for (index, column) in self.columns.iter().enumerate() {
                        ui.checkbox(&mut self.draft_fk_columns[index], &column.name);
                    }
                });

                ui.add_space(4.0);
                ui.label(RichText::new(tr(
                    "Referenced columns (comma-separated, one per selected local column)",
                )));
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
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("Add Foreign Key")).clicked() {
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
                self.queue_alter(AlterTableRequest::AddForeignKey {
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

    /// Confirmation shown when Refresh is clicked while there are staged
    /// (unsaved) structure changes, since reloading would discard them.
    fn refresh_dialog(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        if !self.confirm_refresh {
            return;
        }
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        Window::new(tr("Confirm Refresh"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.colored_label(
                    Color32::from_rgb(220, 150, 60),
                    tr("Discard unsaved changes and reload the table structure?"),
                );
                ui.add_space(8.0);
                ui.label(format!("Table: {}", self.table_name()));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("Refresh")).clicked() {
                        confirm = true;
                    }
                });
            });
        if confirm {
            self.refresh_requested = true;
            *message = Some("Refreshing structure...".to_string());
        }
        self.confirm_refresh = !confirm && !cancel && open;
    }

    /// Renders the columns section as a table, returning an optional status
    /// message (e.g. when the user copies a name or reorders a column).
    fn columns_table(&mut self, ui: &mut Ui) -> Option<String> {
        if self.columns.is_empty() {
            empty_message(ui, "No columns");
            return None;
        }
        let mut status: Option<String> = None;
        let column_count = self.columns.len();

        let copy_label = tr("Copy");
        let paste_label = tr("Paste");
        let copy_name_label = tr("Copy Name");
        let delete_label = tr("Delete");
        let move_up_label = tr("Move Up");
        let move_down_label = tr("Move Down");

        ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Grid::new("structure_columns_grid")
                    .striped(true)
                    .num_columns(8)
                    .spacing([8.0, 0.0])
                    .show(ui, |ui| {
                        // Fixed-size header cells drive the column widths; the
                        // label widget would otherwise keep the "Name" column at
                        // ~40px and ignore the wider inline editor below.
                        for (label, width) in [
                            ("Name", 150.0),
                            ("Type", 150.0),
                            ("Length", 90.0),
                            ("Unsigned", 80.0),
                            ("Null", 50.0),
                            ("Default", 150.0),
                            ("Key", 50.0),
                            ("Comment", 220.0),
                        ] {
                            ui.add_sized(
                                [width, 20.0],
                                egui::Label::new(RichText::new(tr(label)).strong()),
                            );
                        }
                        ui.end_row();

                        let mut copy_requested: Option<usize> = None;
                        let mut paste_requested = false;
                        let mut drop_requested: Option<usize> = None;
                        let mut move_requested: Option<(usize, i32)> = None;

                        for idx in 0..column_count {
                            let name = self.columns[idx].name.clone();

                            // ---- Name: inline editable, focused after copy/paste ----
                            let name_resp = ui.add_sized(
                                [150.0, 22.0],
                                TextEdit::singleline(&mut self.columns[idx].name),
                            );
                            if name_resp.clicked() {
                                self.selected_column = Some(idx);
                            }
                            if self.editing_name == Some(idx) {
                                name_resp.request_focus();
                                // Navicat-style: select the whole (generated) name
                                // so typing immediately replaces it.
                                let char_count = self.columns[idx].name.chars().count();
                                if let Some(mut state) =
                                    TextEdit::load_state(ui.ctx(), name_resp.id)
                                {
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(0),
                                            egui::text::CCursor::new(char_count),
                                        ),
                                    ));
                                    state.store(ui.ctx(), name_resp.id);
                                }
                                self.editing_name = None;
                            }
                            name_resp.context_menu(|ui| {
                                if ui.button(&copy_label).clicked() {
                                    copy_requested = Some(idx);
                                    ui.close_menu();
                                }
                                if ui.button(&paste_label).clicked() {
                                    paste_requested = true;
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button(&copy_name_label).clicked() {
                                    ui.ctx().output_mut(|o| o.copied_text = name.clone());
                                    status = Some("Column name copied".to_string());
                                    ui.close_menu();
                                }
                                if ui.button(&delete_label).clicked() {
                                    drop_requested = Some(idx);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui
                                    .add_enabled(idx > 0, Button::new(&move_up_label))
                                    .clicked()
                                {
                                    move_requested = Some((idx, -1));
                                    ui.close_menu();
                                }
                                if ui
                                    .add_enabled(
                                        idx + 1 < column_count,
                                        Button::new(&move_down_label),
                                    )
                                    .clicked()
                                {
                                    move_requested = Some((idx, 1));
                                    ui.close_menu();
                                }
                            });

                            // ---- Type (dropdown, base type only; length/precision in its own column) ----
                            let was_unsigned = type_has_unsigned(&self.columns[idx].database_type);
                            let type_combo = ComboBox::from_id_source(("col_type", idx))
                                .selected_text(type_label(&self.columns[idx].database_type))
                                .width(140.0)
                                .show_ui(ui, |ui| {
                                    for candidate in common_column_types(self.kind) {
                                        let base = type_without_field_width(candidate);
                                        let target = if was_unsigned {
                                            format!("{base} UNSIGNED")
                                        } else {
                                            base.clone()
                                        };
                                        ui.selectable_value(
                                            &mut self.columns[idx].database_type,
                                            target,
                                            &base,
                                        );
                                    }
                                });
                            if type_combo.response.clicked() || type_combo.response.changed() {
                                self.selected_column = Some(idx);
                            }

                            // ---- Length / precision (editable) ----
                            let mut width_buf =
                                type_field_width(&self.columns[idx].database_type);
                            let width_response = ui.add(
                                TextEdit::singleline(&mut width_buf)
                                    .desired_width(90.0)
                                    .hint_text("size"),
                            );
                            if width_response.changed() {
                                self.columns[idx].database_type = set_type_field_width(
                                    &self.columns[idx].database_type,
                                    &width_buf,
                                );
                            }
                            if width_response.clicked() {
                                self.selected_column = Some(idx);
                            }

                            // ---- Unsigned ----
                            let mut unsigned =
                                type_has_unsigned(&self.columns[idx].database_type);
                            let supports_unsigned =
                                type_supports_unsigned(&self.columns[idx].database_type);
                            let unsigned_resp = ui.add_enabled(
                                supports_unsigned,
                                egui::Checkbox::new(&mut unsigned, ""),
                            );
                            if unsigned_resp.changed() {
                                self.columns[idx].database_type =
                                    set_type_unsigned(&self.columns[idx].database_type, unsigned);
                            }
                            if unsigned_resp.clicked() {
                                self.selected_column = Some(idx);
                            }

                            // ---- Nullable ----
                            if ui.checkbox(&mut self.columns[idx].nullable, "").clicked() {
                                self.selected_column = Some(idx);
                            }

                            // ---- Default ----
                            let mut default_buf =
                                self.columns[idx].default_value.clone().unwrap_or_default();
                            let default_edit =
                                TextEdit::singleline(&mut default_buf).desired_width(120.0);
                            let default_response = ui.add_sized([120.0, 22.0], default_edit);
                            if default_response.clicked() {
                                self.selected_column = Some(idx);
                            }
                            let trimmed_default = default_buf.trim().to_string();
                            self.columns[idx].default_value = if trimmed_default.is_empty() {
                                None
                            } else {
                                Some(default_buf)
                            };

                            // ---- Key ----
                            ui.label(if self.columns[idx].primary_key { "PK" } else { "" });

                            // ---- Comment (editable) ----
                            let mut comment_buf =
                                self.columns[idx].comment.clone().unwrap_or_default();
                            let comment_edit = TextEdit::singleline(&mut comment_buf)
                                .desired_width(160.0)
                                .hint_text(tr("Comment (optional)"));
                            let comment_response = ui.add_sized([160.0, 22.0], comment_edit);
                            if comment_response.changed() {
                                let value = comment_buf.trim();
                                self.columns[idx].comment =
                                    (!value.is_empty()).then(|| value.to_string());
                            }
                            if comment_response.clicked() {
                                self.selected_column = Some(idx);
                            }

                            ui.end_row();
                        }

                        // Apply row actions after the loop (borrow of `columns` released).
                        if let Some((from, delta)) = move_requested {
                            let to = from as i32 + delta;
                            if to >= 0 && (to as usize) < column_count {
                                self.columns.swap(from, to as usize);
                                self.col_original_name.swap(from, to as usize);
                                if self.selected_column == Some(from) {
                                    self.selected_column = Some(to as usize);
                                } else if self.selected_column == Some(to as usize) {
                                    self.selected_column = Some(from);
                                }
                                status = Some(
                                    if delta < 0 {
                                        "Moved column up (display order)"
                                    } else {
                                        "Moved column down (display order)"
                                    }
                                    .to_string(),
                                );
                            }
                        }
                        if let Some(idx) = drop_requested {
                            self.pending_drop_column = Some(self.columns[idx].name.clone());
                        }
                        if let Some(idx) = copy_requested {
                            let col = self.columns[idx].clone();
                            self.copied_column = Some(StagedColumn {
                                column_type: col.database_type.clone(),
                                nullable: col.nullable,
                                default_value: col.default_value.clone(),
                            });
                            // Mirror the definition to the OS clipboard too, so it
                            // can be pasted into a SQL editor or another table.
                            ui.ctx().output_mut(|o| {
                                o.copied_text = column_to_clipboard_text(&col);
                            });
                            // Insert a duplicate right below the source and focus
                            // its name cell so the user can rename it immediately.
                            let new_name = self.unique_column_name(&col.name);
                            let insert_at = idx + 1;
                            self.columns.insert(
                                insert_at,
                                TableColumn {
                                    name: new_name,
                                    database_type: col.database_type,
                                    nullable: col.nullable,
                                    default_value: col.default_value,
                                    primary_key: false,
                                    comment: None,
                                },
                            );
                            self.col_original_name.insert(insert_at, None);
                            self.editing_name = Some(insert_at);
                            status = Some("Column duplicated; rename the new column".to_string());
                        }
                        if paste_requested {
                            // Prefer a definition parsed from the OS clipboard
                            // (egui 0.27 does not expose clipboard reading, so we
                            // read it directly with arboard, the same backend
                            // eframe uses for clipboard writes).
                            let mut pasted: Option<(String, String, bool, Option<String>)> =
                                arboard::Clipboard::new()
                                    .and_then(|mut clipboard| clipboard.get_text())
                                    .ok()
                                    .and_then(|text| parse_clipboard_column(&text));
                            // Fall back to the last column copied in the designer.
                            if pasted.is_none() {
                                if let Some(staged) = self.copied_column.clone() {
                                    pasted = Some((
                                        self.unique_column_name("column"),
                                        staged.column_type,
                                        staged.nullable,
                                        staged.default_value,
                                    ));
                                }
                            }
                            if let Some((name, col_type, nullable, default_value)) = pasted {
                                let new_name = self.unique_column_name(&name);
                                self.columns.push(TableColumn {
                                    name: new_name,
                                    database_type: col_type,
                                    nullable,
                                    default_value,
                                    primary_key: false,
                                    comment: None,
                                });
                                self.col_original_name.push(None);
                                self.editing_name = Some(self.columns.len() - 1);
                                status = Some("Column pasted from clipboard; rename it".to_string());
                            }
                        }
                    });
            });
        status
    }

    /// Shows the editable properties for the selected field, matching the
    /// lower inspector area of a desktop table designer.
    fn column_properties(&mut self, ui: &mut Ui) {
        let Some(index) = self
            .selected_column
            .filter(|index| *index < self.columns.len())
        else {
            Frame::default()
                .fill(ui.visuals().faint_bg_color)
                .inner_margin(Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.label(tr("Select a column to view its properties"))
                });
            return;
        };

        let column = &mut self.columns[index];
        Frame::default()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .inner_margin(Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} properties", column.name)).strong());
                    if self.col_original_name[index].is_none() {
                        ui.label(
                            RichText::new(tr("New"))
                                .small()
                                .color(Color32::from_rgb(40, 140, 90)),
                        );
                    }
                });
                ui.add_space(4.0);
                Grid::new("structure_column_properties")
                    .num_columns(2)
                    .spacing([12.0, 5.0])
                    .show(ui, |ui| {
                        ui.label(tr("Default"));
                        let mut default = column.default_value.clone().unwrap_or_default();
                        if ui
                            .add(TextEdit::singleline(&mut default).desired_width(260.0))
                            .changed()
                        {
                            let value = default.trim();
                            column.default_value = (!value.is_empty()).then(|| default);
                        }
                        ui.end_row();
                        ui.label(tr("Nullable"));
                        ui.checkbox(&mut column.nullable, tr("Allow NULL"));
                        ui.end_row();
                        ui.label(tr("Unsigned"));
                        let mut unsigned = type_has_unsigned(&column.database_type);
                        if ui.checkbox(&mut unsigned, "").changed() {
                            column.database_type = set_type_unsigned(&column.database_type, unsigned);
                        }
                        ui.end_row();
                        ui.label(tr("Primary key"));
                        ui.checkbox(&mut column.primary_key, "");
                        ui.end_row();
                        ui.label(tr("Comment"));
                        let mut comment = column.comment.clone().unwrap_or_default();
                        if ui
                            .add(
                                TextEdit::singleline(&mut comment)
                                    .desired_width(320.0)
                                    .hint_text(tr("Comment (optional)")),
                            )
                            .changed()
                        {
                            let value = comment.trim();
                            column.comment = (!value.is_empty()).then(|| value.to_string());
                        }
                        ui.end_row();
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
                            ui.label(
                                RichText::new(if label.is_empty() {
                                    String::new()
                                } else {
                                    tr(label)
                                })
                                .strong(),
                            );
                        }
                        ui.end_row();

                        let mut drop_requested = None;
                        for idx in &self.indexes {
                            ui.label(&idx.name);
                            ui.label(idx.columns.join(", "));
                            ui.label(if idx.unique { "YES" } else { "" });
                            ui.label(if idx.primary { "YES" } else { "" });
                            if !idx.primary {
                                if ui
                                    .small_button(tr("Delete"))
                                    .on_hover_text(tr("Drop index"))
                                    .clicked()
                                {
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
            empty_message(ui, &tr("No foreign keys"));
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
                            ui.label(RichText::new(tr(label)).strong());
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
                            ui.label(RichText::new(tr(label)).strong());
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
                            ui.label(RichText::new(tr(label)).strong());
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
                        .interactive(true),
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

/// Common column type candidates offered in the Add-Column dropdown, grouped
/// by the target protocol family so users pick from the right vocabulary
/// instead of typing a type by hand.
fn common_column_types(kind: DataSourceKind) -> &'static [&'static str] {
    match kind.protocol_family() {
        ProtocolFamily::MySql => &[
            "INT",
            "BIGINT",
            "TINYINT",
            "SMALLINT",
            "VARCHAR(255)",
            "CHAR(64)",
            "TEXT",
            "DATETIME",
            "TIMESTAMP",
            "DATE",
            "TIME",
            "DECIMAL(10,2)",
            "FLOAT",
            "DOUBLE",
            "BOOLEAN",
            "JSON",
            "BLOB",
            "ENUM",
        ],
        ProtocolFamily::PostgreSql => &[
            "INTEGER",
            "BIGINT",
            "SMALLINT",
            "SERIAL",
            "BIGSERIAL",
            "VARCHAR(255)",
            "TEXT",
            "TIMESTAMP",
            "TIMESTAMPTZ",
            "DATE",
            "TIME",
            "NUMERIC(10,2)",
            "DOUBLE PRECISION",
            "BOOLEAN",
            "JSONB",
            "UUID",
            "BYTEA",
            "ENUM",
        ],
        ProtocolFamily::SqlServer => &[
            "INT",
            "BIGINT",
            "SMALLINT",
            "TINYINT",
            "NVARCHAR(255)",
            "VARCHAR(255)",
            "NCHAR(64)",
            "CHAR(64)",
            "TEXT",
            "NTEXT",
            "DATETIME",
            "DATETIME2",
            "DATE",
            "TIME",
            "DECIMAL(18,2)",
            "NUMERIC",
            "FLOAT",
            "REAL",
            "BIT",
            "UNIQUEIDENTIFIER",
            "BINARY(16)",
            "ENUM",
        ],
        ProtocolFamily::ClickHouse => &[
            "Int32",
            "Int64",
            "UInt32",
            "Float64",
            "String",
            "FixedString(16)",
            "Date",
            "DateTime",
            "DateTime64(3)",
            "Decimal(18,6)",
            "Nullable(Int64)",
            "Array(Int32)",
            "UUID",
            "JSON",
            "ENUM",
        ],
        ProtocolFamily::Sqlite | ProtocolFamily::DuckDb => &[
            "INTEGER",
            "BIGINT",
            "TEXT",
            "REAL",
            "DOUBLE",
            "DECIMAL",
            "BOOLEAN",
            "DATE",
            "DATETIME",
            "TIMESTAMP",
            "TIME",
            "BLOB",
            "JSON",
            "UUID",
            "ENUM",
        ],
        _ => &[
            "INT",
            "BIGINT",
            "VARCHAR(255)",
            "TEXT",
            "DATE",
            "DATETIME",
            "TIMESTAMP",
            "DECIMAL(10,2)",
            "BOOLEAN",
            "JSON",
            "ENUM",
        ],
    }
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

/// Extract the inner length/precision text of a type declaration, e.g.
/// `20` for `VARCHAR(20)` or `10,2` for `DECIMAL(10,2)`, or "" when absent.
fn type_field_width(value: &str) -> String {
    let trimmed = value.trim();
    let Some(open) = trimmed.find('(') else {
        return String::new();
    };
    let Some(close_offset) = trimmed[open..].find(')') else {
        return String::new();
    };
    let close = open + close_offset;
    trimmed[open + 1..close].trim().to_string()
}

/// Set (or clear) the length/precision part of a type declaration while
/// preserving the base type, rebuilding e.g. `VARCHAR` + `255` -> `VARCHAR(255)`.
fn set_type_field_width(value: &str, new_width: &str) -> String {
    let unsigned = type_has_unsigned(value);
    let no_unsigned = set_type_unsigned(value, false);
    let base = type_without_field_width(&no_unsigned);
    let width = new_width.trim();
    let rebuilt = if width.is_empty() {
        base
    } else {
        format!("{base}({width})")
    };
    set_type_unsigned(&rebuilt, unsigned)
}

/// Whether a type declaration carries the MySQL `UNSIGNED` attribute.
fn type_has_unsigned(value: &str) -> bool {
    value.to_uppercase().contains("UNSIGNED")
}

/// Numeric types that support the `UNSIGNED` attribute; the inline grid only
/// enables the checkbox for these (mirrors Navicat).
fn type_supports_unsigned(value: &str) -> bool {
    let base = type_label(value).to_ascii_lowercase();
    !base.is_empty()
        && [
            "int",
            "integer",
            "tinyint",
            "smallint",
            "mediumint",
            "bigint",
            "smallint unsigned",
            "decimal",
            "numeric",
            "float",
            "float8",
            "double",
            "double precision",
            "real",
            "year",
        ]
        .contains(&base.as_str())
}

/// Add or remove the `UNSIGNED` attribute while normalizing spacing, e.g.
/// `INT(11) UNSIGNED` <-> `INT(11)`.
fn set_type_unsigned(value: &str, unsigned: bool) -> String {
    let cleaned = value
        .replace("UNSIGNED", "")
        .replace("unsigned", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if unsigned && !cleaned.is_empty() {
        format!("{cleaned} UNSIGNED")
    } else {
        cleaned
    }
}

/// Friendly label for a type: strips `UNSIGNED` and any length parens, e.g.
/// `INT(11) UNSIGNED` -> `INT`.
fn type_label(value: &str) -> String {
    let no_unsigned = set_type_unsigned(value, false);
    type_without_field_width(&no_unsigned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str) -> TableColumn {
        TableColumn {
            name: name.to_string(),
            database_type: "VARCHAR(255)".to_string(),
            nullable: true,
            default_value: None,
            primary_key: false,
            comment: None,
        }
    }

    fn structure(columns: Vec<TableColumn>) -> TableStructureView {
        TableStructureView::from_columns(
            ConnectionId::new(),
            DataSourceKind::MySql,
            Some("app".to_string()),
            None,
            "users".to_string(),
            columns,
        )
    }

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

    #[test]
    fn staged_changes_batch_add_rename_modify_then_drop() {
        let mut view = structure(vec![column("id"), column("email"), column("legacy")]);
        view.columns[1].name = "email_address".to_string();
        view.columns[1].database_type = "VARCHAR(512)".to_string();
        view.columns[1].nullable = false;
        view.columns[1].default_value = Some("'unknown'".to_string());
        view.columns.remove(2);
        view.col_original_name.remove(2);
        view.columns.push(column("display_name"));
        view.col_original_name.push(None);

        let requests = view.staged_requests();
        assert_eq!(requests.len(), 4);
        assert!(matches!(
            &requests[0],
            AlterTableRequest::RenameColumn { old_name, new_name, .. }
                if old_name == "email" && new_name == "email_address"
        ));
        assert!(matches!(
            &requests[1],
            AlterTableRequest::ModifyColumn { column_name, new_type, nullable, default_value, .. }
                if column_name == "email_address"
                    && new_type == "VARCHAR(512)"
                    && !nullable
                    && default_value.as_deref() == Some("'unknown'")
        ));
        assert!(matches!(
            &requests[2],
            AlterTableRequest::AddColumn { column_name, .. } if column_name == "display_name"
        ));
        assert!(matches!(
            &requests[3],
            AlterTableRequest::DropColumn { column_name, .. } if column_name == "legacy"
        ));
    }

    #[test]
    fn removing_a_new_draft_column_does_not_create_a_request() {
        let mut view = structure(vec![column("id")]);
        view.columns.push(column("draft"));
        view.col_original_name.push(None);
        view.columns.pop();
        view.col_original_name.pop();

        assert!(view.staged_requests().is_empty());
    }
}
