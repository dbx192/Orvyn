use crate::ui::i18n::text as tr;
use crate::ui::result_grid::{ResultSetGrid, ResultSetGridAction};
use crate::ui::table_editor::TableEditor;
use app_core::{connection::ConnectionManager, metadata::MetadataService, query::QueryService};
use db_common::{
    dialect::{BacktickDialect, BracketDialect, DoubleQuoteDialect},
    looks_like_unqualified_mutation, split_statements, statement_at_offset, AppError, ConnectionId,
    ConnectionProfile, DataSourceKind, DatabaseObject, Dialect, ErrorKind, FavoriteQuery,
    QueryExecutionMode, QueryExecutionReport, QueryExecutionRequest, QueryHistoryEntry, QueryId,
    SqlHighlightToken, SqlTokenKind, TableColumn, TextRange, DEFAULT_QUERY_RESULT_LIMIT,
    QUERY_RESULT_METADATA_TOTAL_ROWS,
};
use eframe::egui::{
    text::LayoutJob, vec2, Align, Area, Button, Color32, ComboBox, FontId, Frame, Key, Layout,
    Margin, Order, Pos2, RichText, ScrollArea, Stroke, TextEdit, TextFormat, Ui, Window,
};
use import_export::{export_query_result_to_file, ExportFormat};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc::{self, Receiver, Sender},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::runtime::Handle;

struct QueryResultView {
    statement: String,
    summary: String,
    grid: Option<ResultSetGrid>,
    table_editor: Option<TableEditor>,
    affected_rows: u64,
    elapsed_ms: u128,
    notices: Vec<String>,
}

enum QueryEditorEvent {
    ExecutionCompleted(QueryId, Result<QueryExecutionReport, AppError>),
    ResultPageCompleted(QueryId, usize, Result<QueryExecutionReport, AppError>),
    StopCompleted(QueryId, Result<(), AppError>),
    ConnectionList(Vec<ConnectionProfile>),
    DatabaseList(QueryId, Vec<String>),
    SchemaList(QueryId, Vec<String>),
    TableList(QueryId, Vec<String>),
    ColumnList(QueryId, String, Vec<TableColumn>),
    ResultTableColumns(QueryId, usize, String, Option<String>, Vec<TableColumn>),
    HistoryLoaded(Vec<QueryHistoryEntry>),
    FavoritesLoaded(Vec<FavoriteQuery>),
    FavoriteSaved(Result<FavoriteQuery, AppError>),
    FavoriteUpdated(Result<FavoriteQuery, AppError>),
    FavoriteRemoved(Result<QueryId, AppError>),
    TransactionCompleted(QueryId, TransactionAction, Result<(), AppError>),
    MetadataLoadFailed(QueryId, String),
}

struct PendingDangerConfirm {
    mode: QueryExecutionMode,
    sql: String,
}

const LARGE_SQL_HIGHLIGHT_THRESHOLD: usize = 20_000;
const SQL_HIGHLIGHT_DEBOUNCE: Duration = Duration::from_millis(140);
const MAX_COMPLETION_ITEMS: usize = 24;
const SQL_EDITOR_VIEWPORT_HEIGHT: f32 = 240.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionAction {
    Begin,
    Commit,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionKind {
    Keyword,
    Table,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionCommand {
    None,
    Next,
    Previous,
    Accept,
    Dismiss,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionContext {
    None,
    Keyword,
    Table,
    Column { qualifier: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionTableRef {
    table: String,
    alias: Option<String>,
}

#[derive(Debug, Clone)]
struct CompletionItem {
    label: String,
    insert_text: String,
    kind: CompletionKind,
    detail: String,
}

#[derive(Debug, Clone, Default)]
struct CompletionState {
    visible: bool,
    replacement: Option<TextRange>,
    items: Vec<CompletionItem>,
    selected: usize,
    waiting_for_metadata: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QuerySideTab {
    History,
    Favorites,
}

struct QueryEditorTab {
    id: QueryId,
    title: String,
    sql: String,
    cursor_offset: usize,
    selection: Option<TextRange>,
    dirty: bool,
    connection_id: ConnectionId,
    database: Option<String>,
    schema: Option<String>,
    available_databases: Vec<String>,
    available_schemas: Vec<String>,
    available_tables: Vec<String>,
    columns_by_table: BTreeMap<String, Vec<TableColumn>>,
    pending_column_loads: BTreeSet<String>,
    results: Vec<QueryResultView>,
    selected_result: usize,
    execution_report: Option<QueryExecutionReport>,
    execution_running: bool,
    execution_cancelling: bool,
    page_loading: Option<usize>,
    execution_error: Option<String>,
    highlighted_sql: Vec<SqlHighlightToken>,
    highlight_due_at: Option<Instant>,
    completion: CompletionState,
    transaction_active: bool,
    transaction_busy: bool,
    transaction_error: Option<String>,
}

impl QueryEditorTab {
    fn new(profile: &ConnectionProfile) -> Self {
        Self {
            id: QueryId::new(),
            title: next_tab_title(&profile.name),
            sql: String::new(),
            cursor_offset: 0,
            selection: None,
            dirty: false,
            connection_id: profile.id,
            database: profile.database.clone(),
            schema: None,
            available_databases: Vec::new(),
            available_schemas: Vec::new(),
            available_tables: Vec::new(),
            columns_by_table: BTreeMap::new(),
            pending_column_loads: BTreeSet::new(),
            results: Vec::new(),
            selected_result: 0,
            execution_report: None,
            execution_running: false,
            execution_cancelling: false,
            page_loading: None,
            execution_error: None,
            highlighted_sql: Vec::new(),
            highlight_due_at: None,
            completion: CompletionState::default(),
            transaction_active: false,
            transaction_busy: false,
            transaction_error: None,
        }
    }

    fn has_selection(&self) -> bool {
        self.selection
            .map(|range| !range.is_empty())
            .unwrap_or(false)
    }
}

pub struct QueryEditor {
    query_service: QueryService,
    connection_manager: ConnectionManager,
    metadata_service: MetadataService,
    runtime: Handle,
    tx: Sender<QueryEditorEvent>,
    rx: Receiver<QueryEditorEvent>,
    tabs: BTreeMap<QueryId, QueryEditorTab>,
    active_tab: Option<QueryId>,
    connections: Vec<ConnectionProfile>,
    history_entries: Vec<QueryHistoryEntry>,
    favorite_entries: Vec<FavoriteQuery>,
    show_execute_all: bool,
    pending_danger_confirm: Option<PendingDangerConfirm>,
    side_tab: QuerySideTab,
    side_panel_open: bool,
    side_panel_width: f32,
    /// Filter text for the history side panel.
    history_search: String,
    /// Filter text for the favorites side panel.
    favorite_search: String,
    /// Favorite currently being edited (updated in place on save).
    editing_favorite: Option<QueryId>,
    status: String,
    /// Completion messages for async exports; polled into `status`.
    export_status: Arc<Mutex<Option<String>>>,
}

impl QueryEditor {
    pub fn new(
        query_service: QueryService,
        connection_manager: ConnectionManager,
        metadata_service: MetadataService,
        runtime: Handle,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut editor = Self {
            query_service,
            connection_manager,
            metadata_service,
            runtime,
            tx,
            rx,
            tabs: BTreeMap::new(),
            active_tab: None,
            connections: Vec::new(),
            history_entries: Vec::new(),
            favorite_entries: Vec::new(),
            show_execute_all: false,
            pending_danger_confirm: None,
            side_tab: QuerySideTab::History,
            side_panel_open: false,
            side_panel_width: 360.0,
            history_search: String::new(),
            favorite_search: String::new(),
            editing_favorite: None,
            status: String::new(),
            export_status: Arc::new(Mutex::new(None)),
        };
        editor.load_connections();
        editor.load_history();
        editor.load_favorites();
        editor
    }

    pub fn ui(&mut self, ctx: &eframe::egui::Context, ui: &mut Ui) -> Option<String> {
        self.drain_events();

        if let Some(message) = self.export_status.lock().unwrap().take() {
            self.status = message;
        }

        if self.tabs.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical(|ui| {
                    ui.label(tr("No SQL workspace open"));
                    if ui.button(tr("New Query")).clicked() {
                        self.new_query();
                    }
                });
            });
            self.execute_all_dialog(ctx);
            return Some(self.status.clone());
        }

        self.toolbar(ui);
        ui.separator();
        self.active_tab_panel(ui);
        self.execute_all_dialog(ctx);
        self.dangerous_sql_dialog(ctx);

        Some(self.status.clone())
    }

    pub fn is_executing(&self) -> bool {
        self.active_tab
            .and_then(|id| self.tabs.get(&id))
            .map(|tab| tab.execution_running)
            .unwrap_or(false)
    }

    pub fn format_current_sql(&mut self) -> Option<String> {
        let tab_id = self.active_tab?;
        let tab = self.tabs.get_mut(&tab_id)?;
        if tab.sql.is_empty() {
            return None;
        }
        let formatted = crate::ui::sql_formatter::format_sql(&tab.sql);
        if formatted != tab.sql {
            tab.sql = formatted.clone();
            tab.dirty = true;
            return Some(formatted);
        }
        None
    }

    pub fn new_query(&mut self) {
        let Some(profile) = self
            .connections
            .iter()
            .find(|profile| profile.kind.is_relational())
            .cloned()
        else {
            self.status = "No relational connections available".to_string();
            return;
        };
        self.open_new_tab(profile);
    }

    pub fn show_history(&mut self) {
        self.side_tab = QuerySideTab::History;
        self.side_panel_open = true;
        if self.tabs.is_empty() {
            self.new_query();
        }
        self.status = "History opened".to_string();
    }

    pub fn show_favorites(&mut self) {
        self.side_tab = QuerySideTab::Favorites;
        self.side_panel_open = true;
        if self.tabs.is_empty() {
            self.new_query();
        }
        self.status = "Favorites opened".to_string();
    }

    pub fn new_query_with_connection(
        &mut self,
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: Option<String>,
    ) {
        let profile = self
            .connections
            .iter()
            .find(|profile| profile.id == connection_id && profile.kind.is_relational())
            .cloned();

        if let Some(profile) = profile {
            let target_database = database.clone().or(profile.database.clone());

            // Reuse a table query tab when returning from the table-data view.
            if let Some(table_name) = table.as_deref() {
                if let Some(tab_id) = self
                    .tabs
                    .iter()
                    .find(|(_, tab)| {
                        tab.connection_id == connection_id
                            && tab.database == target_database
                            && tab.schema == schema
                            && tab.title == table_name
                    })
                    .map(|(id, _)| *id)
                {
                    self.active_tab = Some(tab_id);
                    self.refresh_metadata_for_tab(tab_id);
                    return;
                }
            }

            let tab_id = self.open_new_tab(profile.clone());
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                tab.database = target_database;
                tab.schema = schema;
                if let Some(table) = table {
                    tab.title = table.clone();
                    tab.sql = format!(
                        "SELECT * FROM {};",
                        qualified_query_table(
                            profile.kind,
                            tab.database.as_deref(),
                            tab.schema.as_deref(),
                            &table,
                        )
                    );
                    tab.cursor_offset = tab.sql.len();
                    tab.highlighted_sql = db_common::highlight_sql(&tab.sql);
                    tab.dirty = false;
                }
            }
            self.refresh_metadata_for_tab(tab_id);
        } else {
            self.status = "Selected connection is unavailable".to_string();
        }
    }

    pub fn sync_active_context(
        &mut self,
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
    ) {
        if self.tabs.is_empty() {
            self.new_query_with_connection(connection_id, database, schema, None);
            return;
        }

        let Some(tab_id) = self.active_tab else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };

        tab.connection_id = connection_id;
        tab.database = database;
        tab.schema = schema;
        tab.available_databases.clear();
        tab.available_schemas.clear();
        tab.available_tables.clear();
        tab.columns_by_table.clear();
        tab.pending_column_loads.clear();
        tab.completion.visible = false;
        self.refresh_metadata_for_tab(tab_id);
    }

    pub fn open_saved_query(
        &mut self,
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        title: String,
        sql: String,
    ) {
        let profile = self
            .connections
            .iter()
            .find(|profile| profile.id == connection_id && profile.kind.is_relational())
            .cloned();

        if let Some(profile) = profile {
            let tab_id = self.open_new_tab(profile.clone());
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                tab.title = title;
                tab.database = database.or(profile.database.clone());
                tab.schema = schema;
                tab.sql = sql;
                tab.highlighted_sql = db_common::highlight_sql(&tab.sql);
                tab.dirty = false;
            }
            self.refresh_metadata_for_tab(tab_id);
            self.status = "Saved query opened".to_string();
        } else {
            self.status = "Saved query connection is unavailable".to_string();
        }
    }

    pub fn execute_current_statement(&mut self) {
        self.execute_active(QueryExecutionMode::CurrentStatement);
    }

    pub fn execute_selection(&mut self) {
        self.execute_active(QueryExecutionMode::Selection);
    }

    pub fn stop_execution(&mut self) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            if !tab.execution_running || tab.execution_cancelling {
                return;
            }
            tab.execution_cancelling = true;
            tab.execution_error = None;
        }

        let service = self.query_service.clone();
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = service.stop_execution(tab_id).await;
            let _ = tx.send(QueryEditorEvent::StopCompleted(tab_id, result));
        });
        self.status = "Cancelling execution...".to_string();
    }

    fn open_new_tab(&mut self, profile: ConnectionProfile) -> QueryId {
        let tab = QueryEditorTab::new(&profile);
        let tab_id = tab.id;
        self.tabs.insert(tab_id, tab);
        self.active_tab = Some(tab_id);
        self.refresh_metadata_for_tab(tab_id);
        tab_id
    }

    fn close_tab(&mut self, tab_id: QueryId) {
        let Some(_tab) = self.tabs.remove(&tab_id) else {
            return;
        };
        if self.active_tab == Some(tab_id) {
            self.active_tab = self.tabs.keys().next_back().copied();
        }
    }

    fn close_all_tabs(&mut self) {
        self.tabs.clear();
        self.active_tab = None;
    }

    fn close_other_tabs(&mut self, keep: QueryId) {
        self.tabs.retain(|id, _| *id == keep);
        self.active_tab = Some(keep);
    }

    fn tab_bar(&mut self, ui: &mut Ui) {
        let mut ids: Vec<QueryId> = self.tabs.keys().copied().collect();
        // Keep the untitled/default query workspace at the leading edge of
        // the tab strip regardless of the generated QueryId ordering.
        let default_query_ids = ids
            .iter()
            .copied()
            .filter(|id| self.tabs.get(id).is_some_and(|tab| tab.title == "Query"))
            .collect::<Vec<_>>();
        ids.retain(|id| !default_query_ids.contains(id));
        ids.splice(0..0, default_query_ids);
        let mut close_tab = None;
        ui.horizontal_wrapped(|ui| {
            for id in ids {
                let Some((title, dirty)) = self.tabs.get(&id).map(|tab| {
                    if tab.dirty {
                        (format!("* {}", tab.title), true)
                    } else {
                        (tab.title.clone(), false)
                    }
                }) else {
                    continue;
                };
                let selected = self.active_tab == Some(id);
                let response = ui.selectable_label(selected, title);
                if response.clicked() {
                    self.active_tab = Some(id);
                }
                response.context_menu(|ui| {
                    if ui.button(tr("Close")).clicked() {
                        self.close_tab(id);
                        ui.close_menu();
                    }
                    if ui.button(tr("Close Other Tabs")).clicked() {
                        self.close_other_tabs(id);
                        ui.close_menu();
                    }
                    if ui.button(tr("Close All Tabs")).clicked() {
                        self.close_all_tabs();
                        ui.close_menu();
                    }
                });
                if selected || response.hovered() {
                    // Dirty state is already carried by the "* " title prefix,
                    // so the dot variant would be redundant here.
                    let close = crate::ui::app::style::close_icon_button(ui, 18.0, false);
                    let close = if dirty {
                        close.on_hover_text(tr("Save or rollback changes before closing"))
                    } else {
                        close.on_hover_text(tr("Close query"))
                    };
                    if close.clicked() && !dirty {
                        close_tab = Some(id);
                    }
                } else {
                    ui.allocate_exact_size(vec2(18.0, 18.0), eframe::egui::Sense::hover());
                }
            }
        });
        if let Some(id) = close_tab {
            self.close_tab(id);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let mut transaction_action = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("New Query")).clicked() {
                self.new_query();
            }

            ui.separator();
            self.connection_selector(ui);
            self.database_selector(ui);
            if self
                .active_tab
                .and_then(|id| self.tabs.get(&id))
                .and_then(|tab| connection_kind_for(&self.connections, tab.connection_id))
                .map(supports_schema_selector)
                .unwrap_or(false)
            {
                self.schema_selector(ui);
            }
            ui.separator();

            let active_tab_id = self.active_tab;
            let (is_running, is_cancelling, has_selection, transaction_active, transaction_busy) =
                active_tab_id
                    .and_then(|id| self.tabs.get(&id))
                    .map(|tab| {
                        (
                            tab.execution_running,
                            tab.execution_cancelling,
                            tab.has_selection(),
                            tab.transaction_active,
                            tab.transaction_busy,
                        )
                    })
                    .unwrap_or((false, false, false, false, false));

            if !is_running {
                if ui
                    .button(tr("Run Current"))
                    .on_hover_text("Ctrl+R")
                    .clicked()
                {
                    self.execute_current_statement();
                }
                if ui
                    .add_enabled(has_selection, Button::new(tr("Run Selection")))
                    .clicked()
                {
                    self.execute_selection();
                }
                if ui
                    .button(tr("Run All"))
                    .on_hover_text("Ctrl+Shift+R")
                    .clicked()
                {
                    self.show_execute_all = true;
                }
                if ui
                    .button(tr("Format SQL"))
                    .on_hover_text("Ctrl+Shift+F")
                    .clicked()
                {
                    if self.format_current_sql().is_some() {
                        self.status = "SQL formatted".to_string();
                    } else {
                        self.status = "SQL already formatted".to_string();
                    }
                }
                if ui.button(tr("Explain")).clicked() {
                    self.execute_explain(false);
                }
                if ui.button(tr("Explain Analyze")).clicked() {
                    self.execute_explain(true);
                }
            } else if is_cancelling {
                ui.add_enabled(false, Button::new(tr("Cancelling...")));
            } else if ui.button(tr("Stop")).clicked() {
                self.stop_execution();
            }

            if let Some(tab) = active_tab_id.and_then(|id| self.tabs.get(&id)) {
                ui.separator();
                let tx_label = if tab.transaction_active {
                    "Txn: active"
                } else {
                    "Txn: idle"
                };
                let tx_color = if tab.transaction_active {
                    Color32::from_rgb(220, 160, 60)
                } else {
                    Color32::GRAY
                };
                ui.label(RichText::new(tx_label).small().color(tx_color));
                let can_tx = !is_running && !transaction_busy;
                if ui
                    .add_enabled(can_tx && !transaction_active, Button::new("BEGIN"))
                    .clicked()
                {
                    transaction_action = Some(TransactionAction::Begin);
                }
                if ui
                    .add_enabled(can_tx && transaction_active, Button::new("COMMIT"))
                    .clicked()
                {
                    transaction_action = Some(TransactionAction::Commit);
                }
                if ui
                    .add_enabled(can_tx && transaction_active, Button::new("ROLLBACK"))
                    .clicked()
                {
                    transaction_action = Some(TransactionAction::Rollback);
                }
                if let Some(error) = &tab.transaction_error {
                    ui.colored_label(Color32::RED, RichText::new(error).small());
                }

                ui.separator();
                if tab.execution_running {
                    ui.label(
                        RichText::new(if tab.execution_cancelling {
                            "Cancelling..."
                        } else {
                            "Running"
                        })
                        .color(Color32::YELLOW)
                        .strong(),
                    );
                } else if let Some(report) = &tab.execution_report {
                    ui.label(
                        RichText::new(format!("{} ms", report.elapsed_ms))
                            .color(Color32::GREEN)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format!("{} rows affected", report.affected_rows))
                            .color(Color32::from_rgb(70, 110, 190))
                            .strong(),
                    );
                    ui.label(format!("{} result set(s)", report.results.len()));
                }
            }
        });
        match transaction_action {
            Some(TransactionAction::Begin) => self.begin_transaction(),
            Some(TransactionAction::Commit) => self.commit_transaction(),
            Some(TransactionAction::Rollback) => self.rollback_transaction(),
            None => {}
        }
    }

    fn connection_selector(&mut self, ui: &mut Ui) {
        let active_tab_id = self.active_tab;
        let selected_label = active_tab_id
            .and_then(|id| self.tabs.get(&id))
            .and_then(|tab| {
                self.connections
                    .iter()
                    .find(|conn| conn.id == tab.connection_id)
                    .map(connection_label)
            })
            .unwrap_or_else(|| "Select connection".to_string());

        ComboBox::from_id_source("query_connection_selector")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                let mut next_connection = None;
                for conn in &self.connections {
                    if !conn.kind.is_relational() {
                        continue;
                    }
                    let selected = active_tab_id
                        .and_then(|id| self.tabs.get(&id))
                        .map(|tab| tab.connection_id == conn.id)
                        .unwrap_or(false);
                    if ui
                        .selectable_label(selected, connection_label(conn))
                        .clicked()
                    {
                        next_connection = Some(conn.id);
                    }
                }

                if let Some(connection_id) = next_connection {
                    self.set_active_connection(connection_id);
                }
            });
    }

    fn database_selector(&mut self, ui: &mut Ui) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        let selected_text = self
            .tabs
            .get(&tab_id)
            .and_then(|tab| tab.database.clone())
            .unwrap_or_else(|| "Database".to_string());

        ComboBox::from_id_source("query_database_selector")
            .selected_text(selected_text)
            .width(150.0)
            .show_ui(ui, |ui| {
                let options = self
                    .tabs
                    .get(&tab_id)
                    .map(|tab| tab.available_databases.clone())
                    .unwrap_or_default();

                let mut changed = None;
                if ui.selectable_label(false, tr("Default")).clicked() {
                    changed = Some(None);
                }
                for database in options {
                    let selected = self
                        .tabs
                        .get(&tab_id)
                        .and_then(|tab| tab.database.as_ref())
                        .map(|value| value == &database)
                        .unwrap_or(false);
                    if ui.selectable_label(selected, &database).clicked() {
                        changed = Some(Some(database));
                    }
                }

                if let Some(database) = changed {
                    self.set_active_database(database);
                }
            });
    }

    fn schema_selector(&mut self, ui: &mut Ui) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        let selected_text = self
            .tabs
            .get(&tab_id)
            .and_then(|tab| tab.schema.clone())
            .unwrap_or_else(|| "Schema".to_string());

        ComboBox::from_id_source("query_schema_selector")
            .selected_text(selected_text)
            .width(150.0)
            .show_ui(ui, |ui| {
                let options = self
                    .tabs
                    .get(&tab_id)
                    .map(|tab| tab.available_schemas.clone())
                    .unwrap_or_default();

                let mut changed = None;
                if ui.selectable_label(false, tr("Default")).clicked() {
                    changed = Some(None);
                }
                for schema in options {
                    let selected = self
                        .tabs
                        .get(&tab_id)
                        .and_then(|tab| tab.schema.as_ref())
                        .map(|value| value == &schema)
                        .unwrap_or(false);
                    if ui.selectable_label(selected, &schema).clicked() {
                        changed = Some(Some(schema));
                    }
                }

                if let Some(schema) = changed {
                    self.set_active_schema(schema);
                }
            });
    }

    fn active_tab_panel(&mut self, ui: &mut Ui) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        self.refresh_deferred_highlight(tab_id);
        self.tab_bar(ui);
        ui.separator();
        let side_width = self.side_panel_width;
        let connections_snapshot = self.connections.clone();
        let mut column_loads = Vec::new();
        // Navicat-style shortcuts detected inside the SQL editor; dispatched
        // here because the tab borrow spans the whole editor block.
        let mut shortcut_run_current = false;
        let mut shortcut_run_all = false;
        let mut shortcut_format = false;
        ui.horizontal(|ui| {
            let main_width = if self.side_panel_open {
                (ui.available_width() - side_width - 12.0).max(360.0)
            } else {
                ui.available_width()
            };
            // Pin the row to the full remaining height (down to the status
            // bar) so the results area and the history/favorites side panel
            // both stretch to the bottom.
            ui.set_min_height(ui.available_height());

            ui.vertical(|ui| {
                ui.set_width(main_width);
                let Some(tab) = self.tabs.get_mut(&tab_id) else {
                    return;
                };

                Frame::default()
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(Stroke::new(
                        1.0,
                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                    ))
                    .inner_margin(Margin::same(6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("SQL").strong().monospace());
                            ui.label(
                                RichText::new(format!(
                                    "{} statement(s)",
                                    split_statements(&tab.sql).len()
                                ))
                                .small()
                                .color(Color32::GRAY),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let selected = tab.selection.is_some();
                                ui.label(
                                    RichText::new(if selected { "selection" } else { "cursor" })
                                        .small()
                                        .color(Color32::GRAY),
                                );
                            });
                        });
                        ui.separator();
                        ScrollArea::vertical()
                            .id_source("query_editor_sql_scroll")
                            .min_scrolled_height(SQL_EDITOR_VIEWPORT_HEIGHT)
                            .max_height(SQL_EDITOR_VIEWPORT_HEIGHT)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_height(SQL_EDITOR_VIEWPORT_HEIGHT);
                                let tokens = tab.highlighted_sql.clone();
                                let editor_id = ui.make_persistent_id(("sql_editor", tab.id));
                                let editor_has_focus =
                                    ui.memory(|memory| memory.has_focus(editor_id));
                                // Consume completion keys before TextEdit so
                                // accepting a candidate cannot also edit SQL.
                                let completion_command =
                                    take_completion_command(ui, &tab.completion);
                                let tab_requested_completion = completion_command
                                    == CompletionCommand::None
                                    && !tab.completion.visible
                                    && editor_has_focus
                                    && ui.input_mut(|input| {
                                        input.consume_key(eframe::egui::Modifiers::NONE, Key::Tab)
                                    });
                                let dialect =
                                    connection_kind_for(&connections_snapshot, tab.connection_id);
                                let mut layouter =
                                    move |ui: &Ui,
                                          text: &str,
                                          wrap_width: f32|
                                          -> Arc<eframe::egui::Galley> {
                                        let mut job = highlight_layout_job(text, &tokens, dialect);
                                        job.wrap.max_width = wrap_width;
                                        ui.fonts(|fonts| fonts.layout_job(job))
                                    };

                                let mut output = TextEdit::multiline(&mut tab.sql)
                                    .id(editor_id)
                                    .font(FontId::monospace(14.0))
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(32)
                                    .code_editor()
                                    .frame(false)
                                    .layouter(&mut layouter)
                                    .show(ui);
                                let pointer_repositioned = !output.response.changed()
                                    && (output.response.clicked()
                                        || (output.response.hovered()
                                            && ui.input(|input| input.pointer.any_pressed())));

                                let mut refresh_completion = output.response.has_focus()
                                    && ui.input(|input| {
                                        input.key_pressed(Key::Space) && input.modifiers.ctrl
                                    });
                                refresh_completion |= tab_requested_completion;

                                if output.response.changed() {
                                    tab.dirty = true;
                                    schedule_sql_highlight(tab);
                                    refresh_completion = true;
                                }

                                // Navicat-style shortcuts while the editor
                                // holds focus. Shift variants are consumed
                                // first so Ctrl+Shift+R cannot leak into the
                                // Ctrl+R check.
                                if output.response.has_focus() {
                                    let ctrl = eframe::egui::Modifiers::CTRL;
                                    let ctrl_shift =
                                        eframe::egui::Modifiers::CTRL
                                            | eframe::egui::Modifiers::SHIFT;
                                    if ui.input_mut(|input| {
                                        input.consume_key(ctrl_shift, Key::R)
                                    }) {
                                        shortcut_run_all = true;
                                    }
                                    if ui.input_mut(|input| {
                                        input.consume_key(ctrl_shift, Key::F)
                                    }) {
                                        shortcut_format = true;
                                    }
                                    if ui
                                        .input_mut(|input| input.consume_key(ctrl, Key::R))
                                    {
                                        shortcut_run_current = true;
                                    }
                                }

                                let previous_cursor_offset = tab.cursor_offset;
                                if let Some(range) = output.cursor_range {
                                    let primary = char_index_to_byte_offset(
                                        &tab.sql,
                                        range.primary.ccursor.index,
                                    );
                                    let secondary = char_index_to_byte_offset(
                                        &tab.sql,
                                        range.secondary.ccursor.index,
                                    );
                                    tab.cursor_offset = primary;
                                    let selection = TextRange::new(primary, secondary);
                                    tab.selection = (!selection.is_empty()).then_some(selection);
                                    refresh_completion |= output.response.has_focus()
                                        && !pointer_repositioned
                                        && primary != previous_cursor_offset;
                                }

                                if pointer_repositioned {
                                    tab.completion.visible = false;
                                    refresh_completion = false;
                                }

                                let mut completion_pos = output
                                    .cursor_range
                                    .map(|range| {
                                        let cursor_rect =
                                            output.galley.pos_from_cursor(&range.primary);
                                        Pos2::new(
                                            output.galley_pos.x + cursor_rect.left(),
                                            output.galley_pos.y + cursor_rect.bottom() + 2.0,
                                        )
                                    })
                                    .unwrap_or_else(|| output.response.rect.left_bottom());
                                completion_pos.x = completion_pos.x.clamp(
                                    output.text_clip_rect.left(),
                                    (output.text_clip_rect.right() - 260.0)
                                        .max(output.text_clip_rect.left()),
                                );
                                completion_pos.y = completion_pos.y.clamp(
                                    output.text_clip_rect.top(),
                                    output.text_clip_rect.bottom(),
                                );

                                if !output.response.has_focus() {
                                    tab.completion.visible = false;
                                }

                                if refresh_completion {
                                    update_completion_for_tab(tab);
                                    column_loads.extend(missing_completion_columns(tab));
                                }

                                // Completion is tied to the current caret. Once
                                // the editor scrolls, its old layout position is
                                // no longer meaningful, so dismiss it.
                                if tab.completion.visible
                                    && ui.input(|input| input.raw_scroll_delta.y != 0.0)
                                {
                                    tab.completion.visible = false;
                                }

                                let inserted = output.response.has_focus().then(|| {
                                    completion_panel(ui, completion_pos, tab, completion_command)
                                });
                                if let Some(inserted) = inserted.flatten() {
                                    tab.sql = inserted;
                                    let char_cursor = tab.sql[..tab.cursor_offset].chars().count();
                                    output.state.cursor.set_char_range(Some(
                                        eframe::egui::text::CCursorRange::one(
                                            eframe::egui::text::CCursor::new(char_cursor),
                                        ),
                                    ));
                                    output.state.store(ui.ctx(), output.response.id);
                                    tab.dirty = true;
                                    schedule_sql_highlight(tab);
                                }
                            });
                    });

                ui.separator();
                self.results_panel(ui, tab_id);
            });

            if self.side_panel_open {
                // Stretch the divider and the side panel down to the status
                // bar; the row is pinned to full height via set_min_height.
                let (divider_rect, divider_response) = ui.allocate_exact_size(
                    vec2(8.0, ui.available_height()),
                    eframe::egui::Sense::drag(),
                );
                if divider_response.dragged() {
                    self.side_panel_width = (self.side_panel_width
                        - divider_response.drag_delta().x)
                        .clamp(280.0, 640.0);
                    ui.ctx().request_repaint();
                }
                ui.painter().line_segment(
                    [divider_rect.center_top(), divider_rect.center_bottom()],
                    Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                );
                ui.vertical(|ui| {
                    ui.set_width(side_width);
                    ui.set_min_height(ui.available_height());
                    self.history_panel(ui, tab_id);
                });
            }
        });

        for table in column_loads {
            self.load_columns_for_tab(tab_id, table);
        }

        if shortcut_run_current {
            self.execute_current_statement();
        }
        if shortcut_run_all {
            self.show_execute_all = true;
        }
        if shortcut_format {
            if self.format_current_sql().is_some() {
                self.status = "SQL formatted".to_string();
            } else {
                self.status = "SQL already formatted".to_string();
            }
        }
    }

    fn history_panel(&mut self, ui: &mut Ui, tab_id: QueryId) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.side_tab, QuerySideTab::History, tr("History"));
            ui.selectable_value(&mut self.side_tab, QuerySideTab::Favorites, tr("Favorites"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if crate::ui::app::style::close_icon_button(ui, 18.0, false).clicked() {
                    self.side_panel_open = false;
                }
            });
        });
        ui.add_space(2.0);

        match self.side_tab {
            QuerySideTab::History => self.history_list(ui, tab_id),
            QuerySideTab::Favorites => self.favorites_panel(ui, tab_id),
        }
    }

    fn results_panel(&mut self, ui: &mut Ui, tab_id: QueryId) {
        let mut export_request = None;
        let mut page_request = None;
        {
            let Some(tab) = self.tabs.get_mut(&tab_id) else {
                return;
            };

            if tab.execution_running {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new(if tab.execution_cancelling {
                            "Cancelling query..."
                        } else {
                            "Executing..."
                        })
                        .color(Color32::YELLOW)
                        .strong(),
                    );
                });
                return;
            }

            if let Some(error) = &tab.execution_error {
                eframe::egui::Frame::none()
                    .fill(Color32::from_rgb(64, 28, 30))
                    .stroke(eframe::egui::Stroke::new(
                        1.0,
                        Color32::from_rgb(190, 74, 78),
                    ))
                    .inner_margin(eframe::egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Query error").strong().color(Color32::RED));
                        ui.add_space(4.0);
                        let mut diagnostic = error.clone();
                        eframe::egui::TextEdit::multiline(&mut diagnostic)
                            .code_editor()
                            .interactive(false)
                            .desired_width(f32::INFINITY)
                            .show(ui);
                    });
                return;
            }

            if tab.results.is_empty() {
                ui.label(tr("No query executed yet"));
                return;
            }

            ui.horizontal_wrapped(|ui| {
                for (index, result) in tab.results.iter().enumerate() {
                    let selected = tab.selected_result == index;
                    if ui.selectable_label(selected, &result.summary).clicked() {
                        tab.selected_result = index;
                    }
                }
            });
            ui.separator();

            let result_index = tab.selected_result;
            let Some(result) = tab.results.get_mut(result_index) else {
                return;
            };

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{} ms", result.elapsed_ms))
                        .color(Color32::GREEN)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!("{} rows affected", result.affected_rows))
                        .color(Color32::from_rgb(70, 110, 190))
                        .strong(),
                );
                if tab.page_loading == Some(result_index) {
                    ui.label(
                        RichText::new("Loading page...")
                            .small()
                            .color(Color32::YELLOW),
                    );
                }
                ui.separator();
                if ui.button(tr("Export CSV")).clicked() {
                    let query_result = result
                        .grid
                        .as_ref()
                        .map(|grid| grid.result().clone())
                        .or_else(|| {
                            result
                                .table_editor
                                .as_ref()
                                .map(|editor| editor.result().clone())
                        });
                    export_request = Some((query_result, ExportFormat::Csv));
                }
                if ui.button(tr("Export JSON")).clicked() {
                    let query_result = result
                        .grid
                        .as_ref()
                        .map(|grid| grid.result().clone())
                        .or_else(|| {
                            result
                                .table_editor
                                .as_ref()
                                .map(|editor| editor.result().clone())
                        });
                    export_request = Some((query_result, ExportFormat::Json));
                }
            });

            for notice in &result.notices {
                ui.label(RichText::new(notice).small().color(Color32::LIGHT_BLUE));
            }

            if let Some(editor) = &mut result.table_editor {
                if let Some(message) = editor.ui(ui) {
                    self.status = message;
                }
            } else {
                match &mut result.grid {
                    Some(grid) => match grid.ui(ui) {
                        Some(ResultSetGridAction::Message(message)) => {
                            self.status = message;
                        }
                        Some(ResultSetGridAction::PageRequested { offset, page_size }) => {
                            page_request =
                                Some((result_index, result.statement.clone(), offset, page_size));
                        }
                        None => {}
                    },
                    None => {
                        ui.label(tr("Statement executed without a tabular result."));
                    }
                }
            }
        }

        if let Some((query_result, format)) = export_request {
            self.export_query_result(query_result, format);
        }
        if let Some((result_index, statement, offset, page_size)) = page_request {
            self.execute_result_page(tab_id, result_index, statement, offset, page_size);
        }
    }

    fn execute_active(&mut self, mode: QueryExecutionMode) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };

        if tab.sql.trim().is_empty() {
            self.status = "No SQL to execute".to_string();
            return;
        }

        if matches!(mode, QueryExecutionMode::Selection) && !tab.has_selection() {
            self.status = "No selected SQL".to_string();
            return;
        }

        let sql_to_check = selected_sql_for_mode(tab, mode).unwrap_or_else(|| tab.sql.clone());
        if should_confirm_dangerous_sql(&sql_to_check) {
            self.pending_danger_confirm = Some(PendingDangerConfirm {
                mode,
                sql: sql_to_check,
            });
            self.status = "Dangerous SQL requires confirmation".to_string();
            return;
        }

        self.execute_active_confirmed(mode);
    }

    fn execute_explain(&mut self, analyze: bool) {
        let Some(tab_id) = self.active_tab else {
            return;
        };
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        let Some(statement) = selected_sql_for_mode(
            tab,
            if tab.has_selection() {
                QueryExecutionMode::Selection
            } else {
                QueryExecutionMode::CurrentStatement
            },
        ) else {
            self.status = "No statement available for EXPLAIN".to_string();
            return;
        };

        let prefix = if analyze {
            "EXPLAIN ANALYZE"
        } else {
            "EXPLAIN"
        };
        self.execute_custom_sql(tab_id, format!("{prefix} {}", statement.trim()), prefix);
    }

    fn execute_active_confirmed(&mut self, mode: QueryExecutionMode) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };

        tab.execution_running = true;
        tab.execution_cancelling = false;
        tab.page_loading = None;
        tab.execution_error = None;
        tab.results.clear();
        tab.execution_report = None;

        let tx = self.tx.clone();
        let service = self.query_service.clone();
        let request = QueryExecutionRequest {
            tab_id,
            connection_id: tab.connection_id,
            database: tab.database.clone(),
            schema: tab.schema.clone(),
            sql: tab.sql.clone(),
            mode,
            cursor_offset: tab.cursor_offset,
            selection: tab.selection,
            timeout_ms: if matches!(mode, QueryExecutionMode::All) {
                60_000
            } else {
                30_000
            },
            result_limit: Some(DEFAULT_QUERY_RESULT_LIMIT),
            result_offset: 0,
            result_total_rows: None,
        };

        self.status = match mode {
            QueryExecutionMode::CurrentStatement => "Executing current statement...".to_string(),
            QueryExecutionMode::Selection => "Executing selected SQL...".to_string(),
            QueryExecutionMode::All => "Executing all statements...".to_string(),
        };

        self.runtime.spawn(async move {
            let result = service.execute(request, None).await;
            let _ = tx.send(QueryEditorEvent::ExecutionCompleted(tab_id, result));
        });
    }

    fn execute_custom_sql(&mut self, tab_id: QueryId, sql: String, label: &str) {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };

        tab.execution_running = true;
        tab.execution_cancelling = false;
        tab.page_loading = None;
        tab.execution_error = None;
        tab.results.clear();
        tab.execution_report = None;

        let tx = self.tx.clone();
        let service = self.query_service.clone();
        let request = QueryExecutionRequest {
            tab_id,
            connection_id: tab.connection_id,
            database: tab.database.clone(),
            schema: tab.schema.clone(),
            sql,
            mode: QueryExecutionMode::CurrentStatement,
            cursor_offset: 0,
            selection: None,
            timeout_ms: 60_000,
            result_limit: None,
            result_offset: 0,
            result_total_rows: None,
        };
        self.status = format!("{label} running...");
        self.runtime.spawn(async move {
            let result = service.execute(request, None).await;
            let _ = tx.send(QueryEditorEvent::ExecutionCompleted(tab_id, result));
        });
    }

    fn execute_result_page(
        &mut self,
        tab_id: QueryId,
        result_index: usize,
        sql: String,
        offset: u64,
        page_size: u32,
    ) {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };

        if tab.page_loading.is_some() {
            return;
        }
        tab.page_loading = Some(result_index);

        let tx = self.tx.clone();
        let service = self.query_service.clone();
        let request = QueryExecutionRequest {
            tab_id,
            connection_id: tab.connection_id,
            database: tab.database.clone(),
            schema: tab.schema.clone(),
            sql,
            mode: QueryExecutionMode::All,
            cursor_offset: 0,
            selection: None,
            timeout_ms: 120_000,
            result_limit: Some(page_size),
            result_offset: offset,
            result_total_rows: tab
                .results
                .get(result_index)
                .and_then(|view| view.grid.as_ref())
                .and_then(|grid| {
                    grid.result()
                        .metadata
                        .get(QUERY_RESULT_METADATA_TOTAL_ROWS)
                        .and_then(|value| value.parse::<u64>().ok())
                }),
        };
        self.status = "Loading result page...".to_string();
        self.runtime.spawn(async move {
            let result = service.execute(request, None).await;
            let _ = tx.send(QueryEditorEvent::ResultPageCompleted(
                tab_id,
                result_index,
                result,
            ));
        });
    }

    fn execute_all_statements(&mut self) {
        self.execute_active(QueryExecutionMode::All);
    }

    /// Apply a deferred SQL highlight if its debounce window has elapsed.
    fn refresh_deferred_highlight(&mut self, tab_id: QueryId) {
        let due = self
            .tabs
            .get(&tab_id)
            .and_then(|tab| tab.highlight_due_at)
            .is_some_and(|deadline| Instant::now() >= deadline);
        if !due {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.highlight_due_at = None;
            tab.highlighted_sql = db_common::highlight_sql(&tab.sql);
        }
    }

    fn begin_transaction(&mut self) {
        let Some(tab_id) = self.active_tab else {
            return;
        };
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        if tab.transaction_active || tab.transaction_busy {
            return;
        }
        let connection_id = tab.connection_id;
        let database = tab.database.clone();
        let schema = tab.schema.clone();
        let service = self.query_service.clone();
        let tx = self.tx.clone();
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.transaction_busy = true;
            tab.transaction_error = None;
        }
        self.runtime.spawn(async move {
            let result = service
                .begin_transaction(tab_id, connection_id, database, schema, None)
                .await;
            let _ = tx.send(QueryEditorEvent::TransactionCompleted(
                tab_id,
                TransactionAction::Begin,
                result,
            ));
        });
        self.status = "Starting transaction...".to_string();
    }

    fn commit_transaction(&mut self) {
        let Some(tab_id) = self.active_tab else {
            return;
        };
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        if !tab.transaction_active || tab.transaction_busy {
            return;
        }
        let service = self.query_service.clone();
        let tx = self.tx.clone();
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.transaction_busy = true;
            tab.transaction_error = None;
        }
        self.runtime.spawn(async move {
            let result = service.commit_transaction(tab_id).await;
            let _ = tx.send(QueryEditorEvent::TransactionCompleted(
                tab_id,
                TransactionAction::Commit,
                result,
            ));
        });
        self.status = "Committing transaction...".to_string();
    }

    fn rollback_transaction(&mut self) {
        let Some(tab_id) = self.active_tab else {
            return;
        };
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        if !tab.transaction_active || tab.transaction_busy {
            return;
        }
        let service = self.query_service.clone();
        let tx = self.tx.clone();
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.transaction_busy = true;
            tab.transaction_error = None;
        }
        self.runtime.spawn(async move {
            let result = service.rollback_transaction(tab_id).await;
            let _ = tx.send(QueryEditorEvent::TransactionCompleted(
                tab_id,
                TransactionAction::Rollback,
                result,
            ));
        });
        self.status = "Rolling back transaction...".to_string();
    }

    fn set_active_connection(&mut self, connection_id: ConnectionId) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        let profile = self
            .connections
            .iter()
            .find(|profile| profile.id == connection_id)
            .cloned();

        let Some(profile) = profile else {
            return;
        };

        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            if tab.transaction_active {
                self.status =
                    "Commit or rollback the active transaction before changing connection"
                        .to_string();
                return;
            }
            tab.connection_id = connection_id;
            tab.database = profile.database.clone();
            tab.schema = None;
            tab.available_databases.clear();
            tab.available_schemas.clear();
            tab.available_tables.clear();
            tab.columns_by_table.clear();
            tab.pending_column_loads.clear();
            tab.completion.visible = false;
            tab.results.clear();
            tab.execution_report = None;
            tab.execution_error = None;
            self.status = format!("Connection changed to {}", profile.name);
        }
        self.refresh_metadata_for_tab(tab_id);
    }

    fn set_active_database(&mut self, database: Option<String>) {
        let Some(tab_id) = self.active_tab else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            if tab.transaction_active {
                self.status = "Commit or rollback the active transaction before changing database"
                    .to_string();
                return;
            }
            tab.database = database;
            tab.schema = None;
            tab.available_schemas.clear();
            tab.available_tables.clear();
            tab.columns_by_table.clear();
            tab.pending_column_loads.clear();
            tab.completion.visible = false;
            tab.results.clear();
            tab.execution_report = None;
            self.status = "Database changed".to_string();
        }
        self.load_schemas_for_tab(tab_id);
        self.load_tables_for_tab(tab_id);
    }

    fn set_active_schema(&mut self, schema: Option<String>) {
        let Some(tab_id) = self.active_tab else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            if tab.transaction_active {
                self.status =
                    "Commit or rollback the active transaction before changing schema".to_string();
                return;
            }
            tab.schema = schema;
            tab.available_tables.clear();
            tab.columns_by_table.clear();
            tab.pending_column_loads.clear();
            tab.completion.visible = false;
            tab.results.clear();
            tab.execution_report = None;
            self.status = "Schema changed".to_string();
        }
        self.load_tables_for_tab(tab_id);
    }

    fn refresh_metadata_for_tab(&mut self, tab_id: QueryId) {
        self.load_databases_for_tab(tab_id);
        self.load_schemas_for_tab(tab_id);
        self.load_tables_for_tab(tab_id);
    }

    fn load_connections(&mut self) {
        let tx = self.tx.clone();
        let manager = self.connection_manager.clone();
        self.runtime.spawn(async move {
            if let Ok(profiles) = manager.list_profiles().await {
                let _ = tx.send(QueryEditorEvent::ConnectionList(profiles));
            }
        });
    }

    fn load_history(&mut self) {
        let tx = self.tx.clone();
        let service = self.query_service.clone();
        self.runtime.spawn(async move {
            let history = service.history().await;
            let _ = tx.send(QueryEditorEvent::HistoryLoaded(history));
        });
    }

    fn load_favorites(&mut self) {
        let tx = self.tx.clone();
        let service = self.query_service.clone();
        self.runtime.spawn(async move {
            let favorites = service.favorites().await;
            let _ = tx.send(QueryEditorEvent::FavoritesLoaded(favorites));
        });
    }

    fn load_databases_for_tab(&mut self, tab_id: QueryId) {
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let connection_id = tab.connection_id;
        self.runtime.spawn(async move {
            match service.list_databases(connection_id, None).await {
                Ok(databases) => {
                    let _ = tx.send(QueryEditorEvent::DatabaseList(
                        tab_id,
                        database_names(databases),
                    ));
                }
                Err(error) => {
                    let _ = tx.send(QueryEditorEvent::MetadataLoadFailed(tab_id, error.message));
                }
            }
        });
    }

    fn load_schemas_for_tab(&mut self, tab_id: QueryId) {
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        let kind = self
            .connections
            .iter()
            .find(|profile| profile.id == tab.connection_id)
            .map(|profile| profile.kind);
        if tab.database.is_none() && kind.is_some_and(mysql_metadata_requires_database) {
            return;
        }
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let connection_id = tab.connection_id;
        let database = tab.database.clone();
        self.runtime.spawn(async move {
            match service
                .list_schemas(connection_id, database.as_deref(), None)
                .await
            {
                Ok(schemas) => {
                    let _ = tx.send(QueryEditorEvent::SchemaList(
                        tab_id,
                        database_names(schemas),
                    ));
                }
                Err(error) => {
                    let _ = tx.send(QueryEditorEvent::MetadataLoadFailed(tab_id, error.message));
                }
            }
        });
    }

    fn load_tables_for_tab(&mut self, tab_id: QueryId) {
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        let kind = self
            .connections
            .iter()
            .find(|profile| profile.id == tab.connection_id)
            .map(|profile| profile.kind);
        if tab.database.is_none() && kind.is_some_and(mysql_metadata_requires_database) {
            return;
        }
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let connection_id = tab.connection_id;
        let database = tab.database.clone();
        let schema = tab.schema.clone();
        self.runtime.spawn(async move {
            match service
                .list_tables(connection_id, database.as_deref(), schema.as_deref(), None)
                .await
            {
                Ok(tables) => {
                    let _ = tx.send(QueryEditorEvent::TableList(tab_id, database_names(tables)));
                }
                Err(error) => {
                    let _ = tx.send(QueryEditorEvent::MetadataLoadFailed(tab_id, error.message));
                }
            }
        });
    }

    fn load_columns_for_tab(&mut self, tab_id: QueryId, table: String) {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };
        let kind = self
            .connections
            .iter()
            .find(|profile| profile.id == tab.connection_id)
            .map(|profile| profile.kind);
        if tab.database.is_none() && kind.is_some_and(mysql_metadata_requires_database) {
            return;
        }
        let table_key = normalize_identifier(&table);
        if tab.columns_by_table.contains_key(&table_key)
            || !tab.pending_column_loads.insert(table_key)
        {
            return;
        }
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let connection_id = tab.connection_id;
        let database = tab.database.clone();
        let schema = tab.schema.clone();
        self.runtime.spawn(async move {
            match service
                .table_columns(
                    connection_id,
                    database.as_deref(),
                    schema.as_deref(),
                    &table,
                    None,
                )
                .await
            {
                Ok(columns) => {
                    let _ = tx.send(QueryEditorEvent::ColumnList(tab_id, table, columns));
                }
                Err(error) => {
                    let _ = tx.send(QueryEditorEvent::MetadataLoadFailed(tab_id, error.message));
                }
            }
        });
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                QueryEditorEvent::ExecutionCompleted(tab_id, result) => {
                    let mut editable_requests = Vec::new();
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.execution_running = false;
                        tab.execution_cancelling = false;
                        match result {
                            Ok(report) => {
                                let results = report
                                    .results
                                    .iter()
                                    .enumerate()
                                    .map(|(index, result)| {
                                        query_result_view(index, result, self.runtime.clone())
                                    })
                                    .collect::<Vec<_>>();
                                tab.selected_result = 0;
                                tab.execution_report = Some(report.clone());
                                tab.results = results;
                                for (index, view) in tab.results.iter().enumerate() {
                                    if let Some((table, schema)) =
                                        editable_result_source(&view.statement)
                                    {
                                        editable_requests.push((
                                            index,
                                            table,
                                            schema.or_else(|| tab.schema.clone()),
                                            tab.connection_id,
                                            tab.database.clone(),
                                        ));
                                    }
                                }
                                tab.dirty = false;
                                self.status = format!(
                                    "Execution completed in {} ms, {} statement result(s), {} rows affected",
                                    report.elapsed_ms,
                                    report.results.len(),
                                    report.affected_rows
                                );
                            }
                            Err(error) => {
                                if error.kind == ErrorKind::Cancelled {
                                    tab.execution_error = Some("Execution cancelled".to_string());
                                    self.status = "Execution cancelled".to_string();
                                } else {
                                    let diagnostic = error.diagnostic_message();
                                    tab.execution_error = Some(diagnostic.clone());
                                    self.status = format!("Execution failed: {}", error.message);
                                }
                            }
                        }
                    }
                    for (index, table, schema, connection_id, database) in editable_requests {
                        let metadata = self.metadata_service.clone();
                        let tx = self.tx.clone();
                        self.runtime.spawn(async move {
                            if let Ok(columns) = metadata
                                .table_columns(
                                    connection_id,
                                    database.as_deref(),
                                    schema.as_deref(),
                                    &table,
                                    None,
                                )
                                .await
                            {
                                let _ = tx.send(QueryEditorEvent::ResultTableColumns(
                                    tab_id, index, table, schema, columns,
                                ));
                            }
                        });
                    }
                    self.load_history();
                }
                QueryEditorEvent::ResultPageCompleted(tab_id, result_index, result) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.page_loading = None;
                        match result {
                            Ok(report) => {
                                if let Some(result) = report.results.first() {
                                    if let Some(slot) = tab.results.get_mut(result_index) {
                                        *slot = query_result_view(
                                            result_index,
                                            result,
                                            self.runtime.clone(),
                                        );
                                        tab.selected_result = result_index;
                                        tab.execution_report = Some(report.clone());
                                        self.status = format!(
                                            "Loaded result page in {} ms",
                                            report.elapsed_ms
                                        );
                                    }
                                } else {
                                    self.status = "Result page returned no data".to_string();
                                }
                            }
                            Err(error) => {
                                self.status =
                                    format!("Page load failed: {}", error.diagnostic_message());
                            }
                        }
                    }
                    self.load_history();
                }
                QueryEditorEvent::StopCompleted(tab_id, result) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.execution_cancelling = false;
                        if let Err(error) = result {
                            let diagnostic = error.diagnostic_message();
                            tab.execution_error = Some(diagnostic.clone());
                            self.status = format!("Stop failed: {diagnostic}");
                        } else {
                            self.status = "Cancellation requested".to_string();
                        }
                    }
                }
                QueryEditorEvent::ConnectionList(profiles) => {
                    self.connections = profiles
                        .into_iter()
                        .filter(|profile| profile.kind.is_relational())
                        .collect();
                }
                QueryEditorEvent::HistoryLoaded(entries) => {
                    self.history_entries = entries;
                }
                QueryEditorEvent::FavoritesLoaded(entries) => {
                    self.favorite_entries = entries;
                }
                QueryEditorEvent::FavoriteSaved(result) => match result {
                    Ok(favorite) => {
                        upsert_favorite(&mut self.favorite_entries, favorite);
                        self.status = "Favorite saved".to_string();
                    }
                    Err(error) => {
                        self.status = format!("Save favorite failed: {}", error.message);
                    }
                },
                QueryEditorEvent::FavoriteUpdated(result) => match result {
                    Ok(favorite) => {
                        upsert_favorite(&mut self.favorite_entries, favorite);
                        self.editing_favorite = None;
                        self.status = "Favorite updated".to_string();
                    }
                    Err(error) => {
                        self.status = format!("Update favorite failed: {}", error.message);
                    }
                },
                QueryEditorEvent::FavoriteRemoved(result) => match result {
                    Ok(id) => {
                        self.favorite_entries.retain(|entry| entry.id != id);
                        self.status = "Favorite removed".to_string();
                    }
                    Err(error) => {
                        self.status = format!("Remove favorite failed: {}", error.message);
                    }
                },
                QueryEditorEvent::DatabaseList(tab_id, databases) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.available_databases = databases;
                        if tab.database.is_none() {
                            tab.database = tab.available_databases.first().cloned();
                        }
                    }
                }
                QueryEditorEvent::SchemaList(tab_id, schemas) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.available_schemas = schemas;
                        if tab.schema.is_none() {
                            tab.schema = tab.available_schemas.first().cloned();
                        }
                    }
                }
                QueryEditorEvent::TableList(tab_id, tables) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.available_tables = tables;
                        refresh_active_completion(tab);
                    }
                }
                QueryEditorEvent::ColumnList(tab_id, table, columns) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.pending_column_loads
                            .remove(&normalize_identifier(&table));
                        tab.columns_by_table
                            .insert(normalize_identifier(&table), columns);
                        refresh_active_completion(tab);
                    }
                }
                QueryEditorEvent::ResultTableColumns(
                    tab_id,
                    result_index,
                    table,
                    schema,
                    columns,
                ) => {
                    let Some(tab) = self.tabs.get_mut(&tab_id) else {
                        continue;
                    };
                    let Some(kind) = connection_kind_for(&self.connections, tab.connection_id)
                    else {
                        continue;
                    };
                    let has_primary_key = columns.iter().any(|column| column.primary_key);
                    let Some(view) = tab.results.get_mut(result_index) else {
                        continue;
                    };
                    let Some(result) = view.grid.as_ref().map(|grid| grid.result().clone()) else {
                        continue;
                    };
                    let result_columns = result
                        .columns
                        .iter()
                        .map(|column| normalize_identifier(&column.name))
                        .collect::<BTreeSet<_>>();
                    let includes_all_keys = columns
                        .iter()
                        .filter(|column| column.primary_key)
                        .all(|column| result_columns.contains(&normalize_identifier(&column.name)));
                    if has_primary_key && includes_all_keys {
                        view.table_editor = Some(TableEditor::with_services(
                            schema,
                            table,
                            columns,
                            result,
                            query_dialect(kind),
                            self.query_service.clone(),
                            self.metadata_service.clone(),
                            tab.connection_id,
                            kind,
                            tab.database.clone(),
                            self.runtime.clone(),
                        ));
                        view.grid = None;
                    }
                }
                QueryEditorEvent::TransactionCompleted(tab_id, action, result) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.transaction_busy = false;
                        match (action, result) {
                            (TransactionAction::Begin, Ok(())) => {
                                tab.transaction_active = true;
                                tab.transaction_error = None;
                                self.status = "Transaction started".to_string();
                            }
                            (TransactionAction::Commit, Ok(())) => {
                                tab.transaction_active = false;
                                tab.transaction_error = None;
                                self.status = "Transaction committed".to_string();
                            }
                            (TransactionAction::Rollback, Ok(())) => {
                                tab.transaction_active = false;
                                tab.transaction_error = None;
                                self.status = "Transaction rolled back".to_string();
                            }
                            (_, Err(error)) => {
                                tab.transaction_error = Some(error.message.clone());
                                self.status =
                                    format!("Transaction operation failed: {}", error.message);
                            }
                        }
                    }
                }
                QueryEditorEvent::MetadataLoadFailed(tab_id, error) => {
                    if self.tabs.contains_key(&tab_id) {
                        self.status = format!("Metadata load failed: {}", error);
                    }
                }
            }
        }
    }

    fn execute_all_dialog(&mut self, ctx: &eframe::egui::Context) {
        if self.show_execute_all {
            let mut open = true;
            Window::new(tr("Execute All Statements"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("This will execute every statement in the current workspace.");
                    ui.label("Default execution runs only the current statement.");
                    if let Some(tab) = self.active_tab.and_then(|id| self.tabs.get(&id)) {
                        ui.label(
                            RichText::new(format!(
                                "{} statement(s) detected",
                                split_statements(&tab.sql).len()
                            ))
                            .small()
                            .color(Color32::GRAY),
                        );
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("Cancel")).clicked() {
                            self.show_execute_all = false;
                        }
                        if ui.button(tr("Execute All")).clicked() {
                            self.execute_all_statements();
                            self.show_execute_all = false;
                        }
                    });
                });

            if !open {
                self.show_execute_all = false;
            }
        }
    }

    fn dangerous_sql_dialog(&mut self, ctx: &eframe::egui::Context) {
        let Some(pending) = &self.pending_danger_confirm else {
            return;
        };

        let mut open = true;
        let mode = pending.mode;
        let sql = pending.sql.clone();
        Window::new(tr("Confirm Dangerous SQL"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .min_width(640.0)
            .show(ctx, |ui| {
                ui.colored_label(
                    Color32::from_rgb(220, 150, 60),
                    "UPDATE/DELETE/TRUNCATE without WHERE detected. Review the full SQL before continuing.",
                );
                ui.add_space(8.0);
                ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    ui.code(&sql);
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        self.pending_danger_confirm = None;
                        self.status = "Dangerous SQL cancelled".to_string();
                    }
                    if ui.button(tr("Execute Anyway")).clicked() {
                        self.pending_danger_confirm = None;
                        self.execute_active_confirmed(mode);
                    }
                });
            });

        if !open {
            self.pending_danger_confirm = None;
        }
    }

    fn export_query_result(
        &mut self,
        query_result: Option<db_common::QueryResult>,
        format: ExportFormat,
    ) {
        let Some(query_result) = query_result else {
            self.status = "This result set cannot be exported".to_string();
            return;
        };

        let (label, ext) = match format {
            ExportFormat::Csv => ("CSV", "csv"),
            ExportFormat::Json => ("JSON", "json"),
            ExportFormat::ExcelCompatible => ("Excel", "tsv"),
        };

        // The native save dialog must not run synchronously inside the egui
        // frame: on Windows the modal COM dialog can silently fail when
        // opened from the winit event loop callback. Pick the path on a
        // background thread instead and export once it is known.
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(format!("Export {label}"))
            .add_filter(label, &[ext]);
        let status_cell = self.export_status.clone();
        *status_cell.lock().unwrap() = Some(format!("{label} export: choose a save location..."));
        self.runtime.spawn(async move {
            let Some(file) = dialog.save_file().await else {
                *status_cell.lock().unwrap() = Some(format!("{label} export cancelled"));
                return;
            };
            let path = file.path().to_path_buf();
            let path_for_task = path.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                export_query_result_to_file(&query_result, format, &path_for_task)
            })
            .await;
            let message = match outcome {
                Ok(Ok(())) => format!("{label} export finished: {}", path.display()),
                Ok(Err(error)) => format!("{label} export failed: {}", error.diagnostic_message()),
                Err(error) => format!("{label} export task failed: {error}"),
            };
            *status_cell.lock().unwrap() = Some(message);
        });
    }

    fn history_list(&mut self, ui: &mut Ui, tab_id: QueryId) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("History")).strong());
            ui.label(
                RichText::new(format!("{} entries", self.history_entries.len()))
                    .small()
                    .color(Color32::GRAY),
            );
            side_search_box(ui, &mut self.history_search);
        });
        ui.add_space(2.0);
        ScrollArea::vertical()
            .id_source("query_history_panel")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.history_entries.is_empty() {
                    ui.label(
                        RichText::new("No history yet")
                            .italics()
                            .color(Color32::GRAY),
                    );
                    return;
                }

                let query = self.history_search.trim().to_lowercase();
                let entries: Vec<QueryHistoryEntry> = self
                    .history_entries
                    .iter()
                    .filter(|entry| {
                        query.is_empty()
                            || entry.summary.to_lowercase().contains(&query)
                            || entry.sql.to_lowercase().contains(&query)
                    })
                    .take(100)
                    .cloned()
                    .collect();
                if entries.is_empty() {
                    ui.label(
                        RichText::new("No matching history")
                            .italics()
                            .color(Color32::GRAY),
                    );
                    return;
                }

                // Compact one-line rows instead of grouped cards.
                ui.style_mut().spacing.item_spacing.y = 1.0;
                let mut picked_sql = None;
                for entry in &entries {
                    let conn_name = self
                        .connections
                        .iter()
                        .find(|conn| conn.id == entry.connection_id)
                        .map(|conn| conn.name.as_str())
                        .unwrap_or("Unknown");
                    let success_color = if entry.success {
                        Color32::from_rgb(70, 150, 70)
                    } else {
                        Color32::from_rgb(200, 80, 80)
                    };
                    let response = ui.add_sized(
                        [ui.available_width(), 20.0],
                        eframe::egui::SelectableLabel::new(
                            false,
                            RichText::new(&entry.summary).small().monospace(),
                        ),
                    );
                    if response.clicked() {
                        picked_sql = Some(entry.sql.clone());
                    }
                    response.on_hover_text(format!(
                        "{}\n{} · {} ms · {} rows",
                        entry.sql,
                        conn_name,
                        entry.elapsed_ms,
                        entry.affected_rows
                    ));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(conn_name).small().color(Color32::GRAY));
                        ui.label(
                            RichText::new(entry.executed_at.format("%Y-%m-%d %H:%M:%S").to_string())
                                .small()
                                .color(Color32::GRAY),
                        );
                        ui.label(
                            RichText::new(if entry.success { "OK" } else { "ERR" })
                                .small()
                                .color(success_color),
                        );
                    });
                    ui.separator();
                }

                if let Some(sql) = picked_sql {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.sql = sql;
                        tab.highlighted_sql = db_common::highlight_sql(&tab.sql);
                        tab.dirty = true;
                        self.status = "Loaded SQL from history".to_string();
                    }
                }
            });
    }

    fn favorites_panel(&mut self, ui: &mut Ui, tab_id: QueryId) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Favorites")).strong());
            ui.label(
                RichText::new(format!("{} saved", self.favorite_entries.len()))
                    .small()
                    .color(Color32::GRAY),
            );
            side_search_box(ui, &mut self.favorite_search);
            let editing = self.editing_favorite.is_some();
            if ui
                .button(if editing { tr("Update") } else { tr("Save Current") })
                .clicked()
            {
                self.save_current_as_favorite(tab_id);
            }
            if editing && ui.button(tr("Cancel")).clicked() {
                self.editing_favorite = None;
            }
        });
        ui.add_space(2.0);
        ScrollArea::vertical()
            .id_source("query_favorites_panel")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.favorite_entries.is_empty() {
                    ui.label(
                        RichText::new("No favorites yet")
                            .italics()
                            .color(Color32::GRAY),
                    );
                    return;
                }

                let query = self.favorite_search.trim().to_lowercase();
                let entries: Vec<FavoriteQuery> = self
                    .favorite_entries
                    .iter()
                    .filter(|entry| {
                        query.is_empty()
                            || entry.title.to_lowercase().contains(&query)
                            || entry.sql.to_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect();
                if entries.is_empty() {
                    ui.label(
                        RichText::new("No matching favorites")
                            .italics()
                            .color(Color32::GRAY),
                    );
                    return;
                }

                // Compact four-column list: name | time | edit | delete.
                ui.style_mut().spacing.item_spacing.y = 1.0;
                ui.style_mut().spacing.item_spacing.x = 4.0;
                ui.style_mut().spacing.button_padding = vec2(4.0, 2.0);
                let time_width = 96.0;
                let action_width = 34.0;
                let name_width = (ui.available_width()
                    - time_width
                    - action_width * 2.0
                    - 12.0)
                .max(60.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [name_width, 16.0],
                        eframe::egui::Label::new(
                            RichText::new(tr("Name")).small().strong().color(Color32::GRAY),
                        ),
                    );
                    ui.add_sized(
                        [time_width, 16.0],
                        eframe::egui::Label::new(
                            RichText::new(tr("Time")).small().strong().color(Color32::GRAY),
                        ),
                    );
                    ui.add_sized(
                        [action_width, 16.0],
                        eframe::egui::Label::new(
                            RichText::new(tr("Edit")).small().strong().color(Color32::GRAY),
                        ),
                    );
                    ui.add_sized(
                        [action_width, 16.0],
                        eframe::egui::Label::new(
                            RichText::new(tr("Delete")).small().strong().color(Color32::GRAY),
                        ),
                    );
                });
                ui.separator();

                let mut picked_sql = None;
                let mut edit_target = None;
                let mut remove_id = None;
                for entry in &entries {
                    ui.horizontal(|ui| {
                        let name = ui.add_sized(
                            [name_width, 20.0],
                            eframe::egui::SelectableLabel::new(
                                false,
                                RichText::new(&entry.title).small(),
                            ),
                        );
                        if name.clicked() {
                            picked_sql = Some(entry.sql.clone());
                        }
                        name.on_hover_text(&entry.sql);
                        ui.add_sized(
                            [time_width, 20.0],
                            eframe::egui::Label::new(
                                RichText::new(entry.updated_at.format("%Y-%m-%d %H:%M").to_string())
                                    .small()
                                    .color(Color32::GRAY),
                            ),
                        );
                        if ui
                            .add_sized(
                                [action_width, 20.0],
                                eframe::egui::Button::new(RichText::new(tr("Edit")).small()),
                            )
                            .clicked()
                        {
                            edit_target = Some(entry.clone());
                        }
                        if ui
                            .add_sized(
                                [action_width, 20.0],
                                eframe::egui::Button::new(RichText::new(tr("Delete")).small()),
                            )
                            .clicked()
                        {
                            remove_id = Some(entry.id);
                        }
                    });
                }

                if let Some(entry) = edit_target {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.sql = entry.sql.clone();
                        tab.highlighted_sql = db_common::highlight_sql(&tab.sql);
                        tab.dirty = true;
                    }
                    self.editing_favorite = Some(entry.id);
                    self.status = "Loaded SQL from favorites".to_string();
                }
                if let Some(sql) = picked_sql {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.sql = sql;
                        tab.highlighted_sql = db_common::highlight_sql(&tab.sql);
                        tab.dirty = true;
                        self.status = "Loaded SQL from favorites".to_string();
                    }
                }
                if let Some(id) = remove_id {
                    self.remove_favorite(id);
                }
            });
    }

    fn save_current_as_favorite(&mut self, tab_id: QueryId) {
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        if tab.sql.trim().is_empty() {
            self.status = "No SQL to save".to_string();
            return;
        }
        // No dedicated name field: new favorites are titled from their first
        // statement, updates keep the existing title.
        let title = match self.editing_favorite {
            Some(id) => self
                .favorite_entries
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.title.clone())
                .unwrap_or_else(|| statement_title(&tab.sql)),
            None => statement_title(&tab.sql),
        };
        let service = self.query_service.clone();
        let tx = self.tx.clone();
        let sql = tab.sql.clone();
        let connection_id = Some(tab.connection_id);
        let database = tab.database.clone();
        let schema = tab.schema.clone();
        if let Some(id) = self.editing_favorite {
            self.runtime.spawn(async move {
                let result = service
                    .update_favorite(id, connection_id, database, schema, title, sql)
                    .await;
                let _ = tx.send(QueryEditorEvent::FavoriteUpdated(result));
            });
        } else {
            self.runtime.spawn(async move {
                let result = service
                    .add_favorite(connection_id, database, schema, title, sql)
                    .await;
                let _ = tx.send(QueryEditorEvent::FavoriteSaved(result));
            });
        }
    }

    fn remove_favorite(&mut self, id: QueryId) {
        let service = self.query_service.clone();
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = service.remove_favorite(id).await.map(|_| id);
            let _ = tx.send(QueryEditorEvent::FavoriteRemoved(result));
        });
    }
}

fn connection_label(profile: &ConnectionProfile) -> String {
    format!("{} ({})", profile.name, profile.kind.as_str())
}

/// Compact inline filter input shared by the history/favorites panel headers.
fn side_search_box(ui: &mut Ui, text: &mut String) {
    ui.horizontal(|ui| {
        let row_height = 22.0;
        let clear_width = 22.0;
        ui.add_sized(
            [(ui.available_width() - clear_width).max(80.0), row_height],
            TextEdit::singleline(text).hint_text(tr("Search")),
        );
        if crate::ui::app::style::close_icon_button(ui, row_height, false)
            .on_hover_text(tr("Clear"))
            .clicked()
        {
            text.clear();
        }
    });
}

fn next_tab_title(_connection_name: &str) -> String {
    "Query".to_string()
}

fn mysql_metadata_requires_database(kind: DataSourceKind) -> bool {
    matches!(
        kind,
        DataSourceKind::MySql
            | DataSourceKind::MariaDb
            | DataSourceKind::TiDb
            | DataSourceKind::OceanBase
            | DataSourceKind::Doris
            | DataSourceKind::SelectDb
            | DataSourceKind::StarRocks
            | DataSourceKind::GoldenDb
    )
}

fn qualified_query_table(
    kind: DataSourceKind,
    database: Option<&str>,
    schema: Option<&str>,
    table: &str,
) -> String {
    let quote = |value: &str| match kind {
        DataSourceKind::MySql | DataSourceKind::TiDb => {
            format!("`{}`", value.replace('`', "``"))
        }
        _ => format!("\"{}\"", value.replace('"', "\"\"")),
    };
    let qualifier = match kind {
        DataSourceKind::MySql | DataSourceKind::TiDb => database.or(schema),
        _ => schema.or(database),
    };
    qualifier
        .map(|value| format!("{}.{}", quote(value), quote(table)))
        .unwrap_or_else(|| quote(table))
}

fn statement_title(sql: &str) -> String {
    let trimmed = sql.trim();
    let preview = trimmed.lines().next().unwrap_or("SQL");
    let short = preview.chars().take(48).collect::<String>();
    if short.is_empty() {
        "SQL".to_string()
    } else {
        short
    }
}

fn database_names(objects: Vec<DatabaseObject>) -> Vec<String> {
    objects.into_iter().map(|object| object.name).collect()
}

fn connection_kind_for(
    connections: &[ConnectionProfile],
    connection_id: ConnectionId,
) -> Option<DataSourceKind> {
    connections
        .iter()
        .find(|conn| conn.id == connection_id)
        .map(|conn| conn.kind)
}

fn supports_schema_selector(kind: DataSourceKind) -> bool {
    matches!(kind, DataSourceKind::PostgreSql | DataSourceKind::DuckDb)
}

fn selected_sql_for_mode(tab: &QueryEditorTab, mode: QueryExecutionMode) -> Option<String> {
    match mode {
        QueryExecutionMode::CurrentStatement => statement_at_offset(&tab.sql, tab.cursor_offset),
        QueryExecutionMode::Selection => tab.selection.map(|selection| {
            tab.sql
                .get(selection.start..selection.end)
                .unwrap_or("")
                .to_string()
        }),
        QueryExecutionMode::All => Some(tab.sql.clone()),
    }
}

fn query_result_view(
    index: usize,
    result: &db_common::QueryResult,
    runtime: Handle,
) -> QueryResultView {
    QueryResultView {
        statement: result.statement.clone(),
        summary: result_tab_summary(index, result),
        grid: (!result.columns.is_empty())
            .then(|| ResultSetGrid::new(result.clone()).with_runtime(runtime)),
        table_editor: None,
        affected_rows: result.affected_rows,
        elapsed_ms: result.execution_time_ms,
        notices: result.notices.clone(),
    }
}

fn result_tab_summary(index: usize, result: &db_common::QueryResult) -> String {
    let statement = compact_statement_preview(&result.statement, 32);
    if !result.columns.is_empty() {
        if let Some(total_rows) = result
            .metadata
            .get(QUERY_RESULT_METADATA_TOTAL_ROWS)
            .and_then(|value| value.parse::<u64>().ok())
        {
            format!(
                "{} {} ({} / {} rows)",
                index + 1,
                statement,
                result.rows.len(),
                total_rows
            )
        } else {
            format!("{} {} ({} rows)", index + 1, statement, result.rows.len())
        }
    } else {
        format!(
            "{} {} ({} affected)",
            index + 1,
            statement,
            result.affected_rows
        )
    }
}

fn query_dialect(kind: DataSourceKind) -> Box<dyn Dialect> {
    match kind {
        DataSourceKind::MySql | DataSourceKind::Doris => Box::new(BacktickDialect),
        DataSourceKind::Access => Box::new(BracketDialect),
        _ => Box::new(DoubleQuoteDialect),
    }
}

/// Returns a table only for simple, single-table SELECT statements. Results
/// from joins, derived tables, and aggregates remain read-only.
fn editable_result_source(statement: &str) -> Option<(String, Option<String>)> {
    let normalized = statement.trim().trim_end_matches(';').trim();
    let upper = normalized.to_ascii_uppercase();
    if !upper.starts_with("SELECT ")
        || [
            " JOIN ",
            " GROUP ",
            " HAVING ",
            " UNION ",
            " INTERSECT ",
            " EXCEPT ",
        ]
        .iter()
        .any(|keyword| upper.contains(keyword))
    {
        return None;
    }
    let from = upper.find(" FROM ")?;
    let source = normalized[from + 6..].trim_start();
    if source.starts_with('(') {
        return None;
    }
    let source = source.split_whitespace().next()?.trim_end_matches(',');
    if source.contains(',') {
        return None;
    }
    let parts = source
        .split('.')
        .map(|part| part.trim_matches(|ch| ch == '`' || ch == '"'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [table] => Some(((*table).to_string(), None)),
        [schema, table] => Some(((*table).to_string(), Some((*schema).to_string()))),
        _ => None,
    }
}

fn compact_statement_preview(statement: &str, max_chars: usize) -> String {
    let compact = statement.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return "Statement".to_string();
    }
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut preview = compact
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    preview.push_str("...");
    preview
}

fn should_confirm_dangerous_sql(sql: &str) -> bool {
    let normalized = sql.trim().to_ascii_lowercase();
    normalized.starts_with("truncate") || looks_like_unqualified_mutation(sql)
}

fn highlight_layout_job(
    text: &str,
    tokens: &[SqlHighlightToken],
    dialect: Option<DataSourceKind>,
) -> LayoutJob {
    let default_format = TextFormat {
        font_id: FontId::monospace(14.0),
        color: Color32::from_rgb(220, 220, 220),
        ..Default::default()
    };

    let mut job = LayoutJob::default();
    let mut cursor = 0;
    for token in tokens {
        // Highlighting is debounced, so tokens can briefly belong to the
        // previous editor value. Clamp stale byte ranges to valid UTF-8
        // boundaries before slicing the current text.
        let start = floor_char_boundary(text, token.range.start).max(cursor);
        let end = floor_char_boundary(text, token.range.end);
        if start > cursor {
            job.append(&text[cursor..start], 0.0, default_format.clone());
        }
        if start < end {
            job.append(&text[start..end], 0.0, token_format(token.kind, dialect));
        }
        cursor = cursor.max(end);
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, default_format);
    }
    job
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn token_format(kind: SqlTokenKind, dialect: Option<DataSourceKind>) -> TextFormat {
    let color = match kind {
        SqlTokenKind::Keyword => Color32::from_rgb(86, 156, 214),
        SqlTokenKind::Identifier => Color32::from_rgb(220, 220, 220),
        SqlTokenKind::QuotedIdentifier => match dialect {
            Some(DataSourceKind::MySql) => Color32::from_rgb(156, 220, 254),
            Some(DataSourceKind::PostgreSql) => Color32::from_rgb(197, 134, 192),
            _ => Color32::from_rgb(156, 220, 254),
        },
        SqlTokenKind::StringLiteral => Color32::from_rgb(206, 145, 120),
        SqlTokenKind::Number => Color32::from_rgb(181, 206, 168),
        SqlTokenKind::Comment => Color32::from_rgb(106, 153, 85),
        SqlTokenKind::Parameter => Color32::from_rgb(78, 201, 176),
        SqlTokenKind::Function => Color32::from_rgb(220, 220, 170),
        SqlTokenKind::Operator | SqlTokenKind::Punctuation => Color32::from_rgb(212, 212, 212),
    };

    TextFormat {
        font_id: FontId::monospace(14.0),
        color,
        ..Default::default()
    }
}

fn upsert_favorite(favorites: &mut Vec<FavoriteQuery>, favorite: FavoriteQuery) {
    if let Some(existing) = favorites.iter_mut().find(|entry| entry.id == favorite.id) {
        *existing = favorite;
    } else {
        favorites.push(favorite);
        favorites.sort_by_key(|favorite| std::cmp::Reverse(favorite.updated_at));
    }
}

/// Convert a character index (used by egui's `ccursor`) into a byte offset
/// (used by our SQL helpers). Safe against non-ASCII text.
fn char_index_to_byte_offset(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte_offset, _)| byte_offset)
        .unwrap_or_else(|| text.len())
}

/// Normalise an identifier for comparison so that case differences between
/// the user's SQL and the metadata catalog don't prevent matching. Lowercases
/// the string and strips surrounding quotes/backticks.
fn normalize_identifier(value: &str) -> String {
    let trimmed = value.trim();
    let unquoted = trimmed
        .trim_start_matches(['"', '`', '['])
        .trim_end_matches(['"', '`', ']']);
    unquoted.to_ascii_lowercase()
}

/// Schedule a debounced SQL highlight pass. Large SQL documents are expensive
/// to re-tokenise, so we coalesce rapid edits and skip highlighting entirely
/// past the large-SQL threshold until the editor settles.
fn schedule_sql_highlight(tab: &mut QueryEditorTab) {
    if tab.sql.len() > LARGE_SQL_HIGHLIGHT_THRESHOLD {
        tab.highlighted_sql.clear();
        tab.highlight_due_at = None;
        return;
    }
    tab.highlight_due_at = Some(Instant::now() + SQL_HIGHLIGHT_DEBOUNCE);
}

/// A short list of SQL keywords offered when no table/column context is
/// available. Kept lowercase to match the user's typing style.
const SQL_KEYWORD_SUGGESTIONS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "TABLE",
    "ALTER",
    "DROP",
    "INDEX",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "ON",
    "GROUP",
    "BY",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "UNION",
    "AS",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "IN",
    "BETWEEN",
    "LIKE",
    "DISTINCT",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "TRANSACTION",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "DEFAULT",
    "CASCADE",
    "EXISTS",
    "EXPLAIN",
    "ANALYZE",
    "WITH",
    "RETURNING",
    "TRUNCATE",
    "GRANT",
    "REVOKE",
    "VIEW",
    "SCHEMA",
    "DATABASE",
    "IF",
    "EXISTS",
];

/// Refresh context-sensitive completion for the SQL immediately before the
/// caret. Empty/editor-click contexts deliberately produce no suggestions.
fn update_completion_for_tab(tab: &mut QueryEditorTab) {
    if tab.sql.is_empty() {
        tab.completion = CompletionState::default();
        return;
    }

    let cursor = tab.cursor_offset.min(tab.sql.len());
    let before = &tab.sql[..cursor];
    let word_start = before
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_completion_char(*ch))
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(cursor);

    let prefix = &before[word_start..];
    let context = completion_context(before, word_start, prefix);
    if context == CompletionContext::None {
        tab.completion = CompletionState::default();
        return;
    }

    let mut items = Vec::new();
    let replacement = TextRange::new(word_start, cursor);

    match &context {
        CompletionContext::None => {}
        CompletionContext::Keyword => {
            for keyword in SQL_KEYWORD_SUGGESTIONS {
                if matches_prefix(keyword, prefix) {
                    items.push(CompletionItem {
                        label: keyword.to_string(),
                        insert_text: keyword.to_string(),
                        kind: CompletionKind::Keyword,
                        detail: String::new(),
                    });
                }
            }
        }
        CompletionContext::Table => {
            for table in &tab.available_tables {
                if matches_prefix(table, prefix) {
                    items.push(CompletionItem {
                        label: table.clone(),
                        insert_text: table.clone(),
                        kind: CompletionKind::Table,
                        detail: String::new(),
                    });
                }
            }
        }
        CompletionContext::Column { qualifier } => {
            let references = referenced_tables(before);
            let table_names = completion_column_tables(&references, qualifier.as_deref());
            let mut seen = BTreeSet::new();
            for table_name in table_names {
                let key = normalize_identifier(&table_name);
                let Some(columns) = tab.columns_by_table.get(&key) else {
                    continue;
                };
                for column in columns {
                    let normalized = normalize_identifier(&column.name);
                    if matches_prefix(&column.name, prefix) && seen.insert(normalized) {
                        items.push(CompletionItem {
                            label: column.name.clone(),
                            insert_text: column.name.clone(),
                            kind: CompletionKind::Column,
                            detail: format!("{} - {}", table_name, column.database_type),
                        });
                    }
                }
            }
        }
    }

    let waiting_for_metadata = matches!(context, CompletionContext::Column { .. })
        && !missing_completion_columns_for_context(tab, before, &context).is_empty();
    if items.is_empty() && !waiting_for_metadata {
        tab.completion.visible = false;
        tab.completion.items.clear();
        return;
    }

    items.truncate(MAX_COMPLETION_ITEMS);

    // Keep the current selection if still valid, otherwise reset.
    if tab.completion.selected >= items.len() {
        tab.completion.selected = 0;
    }
    tab.completion.items = items;
    tab.completion.replacement = Some(replacement);
    tab.completion.visible = true;
    tab.completion.waiting_for_metadata = waiting_for_metadata;
}

/// Metadata can arrive after the user has clicked outside the SQL editor.
/// Only an existing completion request is allowed to consume it.
fn refresh_active_completion(tab: &mut QueryEditorTab) {
    if tab.completion.visible {
        update_completion_for_tab(tab);
    }
}

/// Return tables referenced by the current completion that have no loaded
/// columns yet. The caller spawns metadata loads for each.
fn missing_completion_columns(tab: &QueryEditorTab) -> Vec<String> {
    if !tab.completion.visible {
        return Vec::new();
    }

    let cursor = tab.cursor_offset.min(tab.sql.len());
    let before = &tab.sql[..cursor];
    let word_start = before
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_completion_char(*ch))
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(cursor);
    let prefix = &before[word_start..];
    let context = completion_context(before, word_start, prefix);
    missing_completion_columns_for_context(tab, before, &context)
}

fn completion_context(before: &str, word_start: usize, prefix: &str) -> CompletionContext {
    let clause = before[..word_start].trim_end();
    if let Some(stripped) = clause.strip_suffix('.') {
        let qualifier = last_sql_identifier(stripped).map(ToOwned::to_owned);
        return CompletionContext::Column { qualifier };
    }

    let previous = last_sql_identifier(clause)
        .map(str::to_ascii_uppercase)
        .unwrap_or_default();
    if matches!(previous.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
        CompletionContext::Table
    } else if matches!(
        previous.as_str(),
        "SELECT" | "WHERE" | "ON" | "HAVING" | "SET" | "AND" | "OR" | "BY"
    ) {
        CompletionContext::Column { qualifier: None }
    } else if !prefix.is_empty() && (clause.is_empty() || clause.ends_with(';')) {
        CompletionContext::Keyword
    } else {
        CompletionContext::None
    }
}

fn last_sql_identifier(sql: &str) -> Option<&str> {
    let end = sql
        .char_indices()
        .rev()
        .find(|(_, ch)| is_completion_char(*ch))
        .map(|(index, ch)| index + ch.len_utf8())?;
    let start = sql[..end]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_completion_char(*ch))
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0);
    Some(&sql[start..end])
}

fn referenced_tables(sql: &str) -> Vec<CompletionTableRef> {
    let tokens = sql_completion_tokens(sql);
    let mut references = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let keyword = tokens[index].to_ascii_uppercase();
        if !matches!(keyword.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
            index += 1;
            continue;
        }

        let mut object_index = index + 1;
        if object_index >= tokens.len() || tokens[object_index] == "(" {
            index += 1;
            continue;
        }
        let mut table = tokens[object_index].clone();
        while object_index + 2 < tokens.len()
            && tokens[object_index + 1] == "."
            && is_sql_identifier_token(&tokens[object_index + 2])
        {
            object_index += 2;
            table = tokens[object_index].clone();
        }

        if !is_sql_identifier_token(&table) {
            index += 1;
            continue;
        }

        let mut alias = None;
        let mut alias_index = object_index + 1;
        if tokens
            .get(alias_index)
            .is_some_and(|token| token.eq_ignore_ascii_case("AS"))
        {
            alias_index += 1;
        }
        if let Some(candidate) = tokens.get(alias_index) {
            if is_sql_identifier_token(candidate) && !is_sql_clause_keyword(candidate) {
                alias = Some(candidate.clone());
            }
        }

        if !references.iter().any(|reference: &CompletionTableRef| {
            normalize_identifier(&reference.table) == normalize_identifier(&table)
        }) {
            references.push(CompletionTableRef { table, alias });
        }
        index = object_index + 1;
    }
    references
}

fn completion_column_tables(
    references: &[CompletionTableRef],
    qualifier: Option<&str>,
) -> Vec<String> {
    let qualifier = qualifier.map(normalize_identifier);
    references
        .iter()
        .filter(|reference| match qualifier.as_ref() {
            None => true,
            Some(qualifier) => {
                normalize_identifier(&reference.table) == *qualifier
                    || reference
                        .alias
                        .as_deref()
                        .is_some_and(|alias| normalize_identifier(alias) == *qualifier)
            }
        })
        .map(|reference| reference.table.clone())
        .collect()
}

fn missing_completion_columns_for_context(
    tab: &QueryEditorTab,
    before: &str,
    context: &CompletionContext,
) -> Vec<String> {
    let CompletionContext::Column { qualifier } = context else {
        return Vec::new();
    };
    let references = referenced_tables(before);
    completion_column_tables(&references, qualifier.as_deref())
        .into_iter()
        .filter(|table| {
            let key = normalize_identifier(table);
            !tab.columns_by_table.contains_key(&key)
                && !tab.pending_column_loads.contains(&key)
                && tab
                    .available_tables
                    .iter()
                    .any(|name| normalize_identifier(name) == key)
        })
        .collect()
}

fn sql_completion_tokens(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if is_completion_char(ch) {
            current.push(ch);
            while chars.peek().is_some_and(|next| is_completion_char(*next)) {
                current.push(chars.next().unwrap_or_default());
            }
            tokens.push(std::mem::take(&mut current));
        } else if matches!(ch, '`' | '"' | '[') {
            let closing = if ch == '[' { ']' } else { ch };
            for next in chars.by_ref() {
                if next == closing {
                    break;
                }
                current.push(next);
            }
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else if matches!(ch, '.' | ',' | ';' | '(' | ')') {
            tokens.push(ch.to_string());
        } else if ch == '\'' {
            // Single-quoted string with `''` escape: skip the closing quote,
            // consume the doubled quote, continue scanning. `for` doesn't work
            // here because we need `peek()` + `next()` for the escape.
            #[allow(clippy::while_let_on_iterator)]
            while let Some(next) = chars.next() {
                if next == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
        }
    }
    tokens
}

fn is_sql_identifier_token(token: &str) -> bool {
    !token.is_empty() && token.chars().all(is_completion_char)
}

fn is_sql_clause_keyword(token: &str) -> bool {
    matches!(
        token.to_ascii_uppercase().as_str(),
        "WHERE"
            | "JOIN"
            | "ON"
            | "GROUP"
            | "ORDER"
            | "HAVING"
            | "LIMIT"
            | "OFFSET"
            | "UNION"
            | "LEFT"
            | "RIGHT"
            | "INNER"
            | "OUTER"
            | "CROSS"
            | "SET"
            | "VALUES"
            | "RETURNING"
    )
}

fn is_completion_char(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_alphanumeric()
}

fn matches_prefix(candidate: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    candidate
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
}

fn take_completion_command(ui: &mut Ui, completion: &CompletionState) -> CompletionCommand {
    if !completion.visible || completion.items.is_empty() {
        return CompletionCommand::None;
    }

    ui.input_mut(|input| {
        let modifiers = eframe::egui::Modifiers::NONE;
        if input.consume_key(modifiers, Key::Escape) {
            CompletionCommand::Dismiss
        } else if input.consume_key(modifiers, Key::ArrowDown) {
            CompletionCommand::Next
        } else if input.consume_key(modifiers, Key::ArrowUp) {
            CompletionCommand::Previous
        } else if input.consume_key(modifiers, Key::Enter) || input.consume_key(modifiers, Key::Tab)
        {
            CompletionCommand::Accept
        } else {
            CompletionCommand::None
        }
    })
}

/// Render a foreground completion popup anchored to the caret and return the
/// SQL with the selected item inserted when the user accepts a suggestion.
fn completion_panel(
    ui: &mut Ui,
    popup_pos: Pos2,
    tab: &mut QueryEditorTab,
    command: CompletionCommand,
) -> Option<String> {
    if !tab.completion.visible {
        return None;
    }

    if command == CompletionCommand::Dismiss {
        tab.completion.visible = false;
        return None;
    }

    let count = tab.completion.items.len();
    if count > 0 {
        match command {
            CompletionCommand::Next => {
                tab.completion.selected = (tab.completion.selected + 1) % count;
            }
            CompletionCommand::Previous => {
                tab.completion.selected = (tab.completion.selected + count - 1) % count;
            }
            _ => {}
        }
    }

    let mut clicked = None;
    Area::new(ui.make_persistent_id(("sql_completion", tab.id)))
        .order(Order::Foreground)
        .fixed_pos(popup_pos)
        .constrain(true)
        .show(ui.ctx(), |ui| {
            Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(260.0);
                if tab.completion.waiting_for_metadata {
                    ui.label("Loading column suggestions...");
                }
                ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (index, item) in tab.completion.items.iter().enumerate() {
                            let selected = index == tab.completion.selected;
                            let kind = match item.kind {
                                CompletionKind::Keyword => "keyword",
                                CompletionKind::Table => "table",
                                CompletionKind::Column => "column",
                            };
                            let detail = if item.detail.is_empty() {
                                String::new()
                            } else {
                                format!(" - {}", item.detail)
                            };
                            let response = ui.selectable_label(
                                selected,
                                RichText::new(format!("{}  [{}]{}", item.label, kind, detail))
                                    .color(if selected {
                                        ui.visuals().selection.stroke.color
                                    } else {
                                        ui.visuals().text_color()
                                    }),
                            );
                            if selected {
                                response.scroll_to_me(Some(Align::Center));
                            }
                            if response.clicked() {
                                clicked = Some(index);
                            }
                        }
                    });
            });
        });

    if count == 0 {
        return None;
    }

    if let Some(index) = clicked {
        tab.completion.selected = index;
    }
    let accept = command == CompletionCommand::Accept || clicked.is_some();
    if !accept {
        return None;
    }

    let replacement = tab.completion.replacement?;
    let selected = tab.completion.selected.min(tab.completion.items.len() - 1);
    let item = tab.completion.items.get(selected)?.clone();
    let mut sql = tab.sql.clone();
    let start = replacement.start.min(sql.len());
    let end = replacement.end.min(sql.len());
    sql.replace_range(start..end, &item.insert_text);
    tab.cursor_offset = start + item.insert_text.len();
    tab.selection = None;
    tab.completion.visible = false;
    Some(sql)
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use eframe::egui::{CentralPanel, Context, Event, Modifiers, RawInput};

    fn completion_state() -> CompletionState {
        CompletionState {
            visible: true,
            replacement: Some(TextRange::new(0, 1)),
            items: vec![CompletionItem {
                label: "SELECT".to_string(),
                insert_text: "SELECT".to_string(),
                kind: CompletionKind::Keyword,
                detail: String::new(),
            }],
            selected: 0,
            waiting_for_metadata: false,
        }
    }

    fn command_for(key: Key) -> CompletionCommand {
        let ctx = Context::default();
        let mut input = RawInput::default();
        input.events.push(Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        ctx.begin_frame(input);
        let mut command = CompletionCommand::None;
        CentralPanel::default().show(&ctx, |ui| {
            command = take_completion_command(ui, &completion_state());
        });
        let _ = ctx.end_frame();
        command
    }

    fn context_for(sql: &str) -> CompletionContext {
        let word_start = sql
            .char_indices()
            .rev()
            .take_while(|(_, ch)| is_completion_char(*ch))
            .last()
            .map(|(index, _)| index)
            .unwrap_or(sql.len());
        completion_context(sql, word_start, &sql[word_start..])
    }

    #[test]
    fn tab_and_enter_accept_visible_completion() {
        assert_eq!(command_for(Key::Tab), CompletionCommand::Accept);
        assert_eq!(command_for(Key::Enter), CompletionCommand::Accept);
    }

    #[test]
    fn arrows_and_escape_control_visible_completion() {
        assert_eq!(command_for(Key::ArrowDown), CompletionCommand::Next);
        assert_eq!(command_for(Key::ArrowUp), CompletionCommand::Previous);
        assert_eq!(command_for(Key::Escape), CompletionCommand::Dismiss);
    }

    #[test]
    fn completion_is_scoped_by_sql_clause() {
        assert_eq!(context_for(""), CompletionContext::None);
        assert_eq!(context_for("SELECT * FROM bo"), CompletionContext::Table);
        assert_eq!(
            context_for("SELECT * FROM books WHERE au"),
            CompletionContext::Column { qualifier: None }
        );
        assert_eq!(
            context_for("SELECT * FROM books b WHERE b."),
            CompletionContext::Column {
                qualifier: Some("b".to_string())
            }
        );
        assert_eq!(context_for("SELECT * FROM books;"), CompletionContext::None);
        assert_eq!(
            context_for("SELECT * FROM books LIMIT 10"),
            CompletionContext::None
        );
    }

    #[test]
    fn metadata_only_refreshes_an_active_completion() {
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "completion test")
            .host("localhost")
            .username("postgres")
            .build()
            .expect("valid profile");
        let mut tab = QueryEditorTab::new(&profile);
        tab.sql = "SELECT * FROM bo".to_string();
        tab.cursor_offset = tab.sql.len();
        tab.available_tables = vec!["books".to_string()];

        refresh_active_completion(&mut tab);
        assert!(!tab.completion.visible);

        tab.completion.visible = true;
        refresh_active_completion(&mut tab);
        assert!(tab.completion.visible);
        assert_eq!(tab.completion.items.len(), 1);
        assert_eq!(tab.completion.items[0].label, "books");
    }

    #[test]
    fn table_references_support_qualified_names_and_aliases() {
        assert_eq!(
            referenced_tables("SELECT * FROM `askme` . `books` b JOIN authors AS a ON a.id=b.id"),
            vec![
                CompletionTableRef {
                    table: "books".to_string(),
                    alias: Some("b".to_string()),
                },
                CompletionTableRef {
                    table: "authors".to_string(),
                    alias: Some("a".to_string()),
                },
            ]
        );
    }

    #[test]
    fn identifies_single_table_results_as_editable_candidates() {
        assert_eq!(
            editable_result_source(
                "SELECT id, name FROM \"base\".\"country\" WHERE id > 1 ORDER BY name LIMIT 10"
            ),
            Some(("country".to_string(), Some("base".to_string())))
        );
    }

    #[test]
    fn keeps_joined_and_aggregate_results_read_only() {
        assert_eq!(
            editable_result_source(
                "SELECT * FROM books JOIN authors ON authors.id = books.author_id"
            ),
            None
        );
        assert_eq!(
            editable_result_source("SELECT category, COUNT(*) FROM books GROUP BY category"),
            None
        );
    }
}
