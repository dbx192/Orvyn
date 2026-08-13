use crate::ui::i18n::text as tr;
use crate::ui::result_grid::{ResultSetGrid, ResultSetGridAction};
use app_core::{connection::ConnectionManager, metadata::MetadataService, query::QueryService};
use db_common::{
    looks_like_unqualified_mutation, split_statements, statement_at_offset, AppError, ConnectionId,
    ConnectionProfile, DataSourceKind, DatabaseObject, ErrorKind, FavoriteQuery,
    QueryExecutionMode, QueryExecutionReport, QueryExecutionRequest, QueryHistoryEntry, QueryId,
    SqlHighlightToken, SqlTokenKind, TableColumn, TextRange, DEFAULT_QUERY_RESULT_LIMIT,
    QUERY_RESULT_METADATA_TOTAL_ROWS,
};
use eframe::egui::{
    text::LayoutJob, vec2, Align, Button, Color32, ComboBox, FontId, Frame, Key, Layout, Margin,
    RichText, ScrollArea, Stroke, TextEdit, TextFormat, Ui, Window,
};
use import_export::{export_query_result_to_file, ExportFormat};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc::{self, Receiver, Sender},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::runtime::Handle;

struct QueryResultView {
    statement: String,
    summary: String,
    grid: Option<ResultSetGrid>,
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
    HistoryLoaded(Vec<QueryHistoryEntry>),
    FavoritesLoaded(Vec<FavoriteQuery>),
    FavoriteSaved(Result<FavoriteQuery, AppError>),
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
    favorite_name_input: String,
    status: String,
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
            side_panel_width: 280.0,
            favorite_name_input: String::new(),
            status: String::new(),
        };
        editor.load_connections();
        editor.load_history();
        editor.load_favorites();
        editor
    }

    pub fn ui(&mut self, ctx: &eframe::egui::Context, ui: &mut Ui) -> Option<String> {
        self.drain_events();

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
            let tab_id = self.open_new_tab(profile.clone());
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                tab.database = database.or(profile.database.clone());
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
        self.status = "SQL workspace ready".to_string();
        tab_id
    }

    fn close_tab(&mut self, tab_id: QueryId) {
        let Some(tab) = self.tabs.remove(&tab_id) else {
            return;
        };
        if self.active_tab == Some(tab_id) {
            self.active_tab = self.tabs.keys().next_back().copied();
        }
        self.status = if tab.dirty {
            "Query tab closed; unsaved SQL was discarded".to_string()
        } else {
            "Query tab closed".to_string()
        };
    }

    fn close_all_tabs(&mut self) {
        self.tabs.clear();
        self.active_tab = None;
        self.status = "All query tabs closed".to_string();
    }

    fn close_other_tabs(&mut self, keep: QueryId) {
        self.tabs.retain(|id, _| *id == keep);
        self.active_tab = Some(keep);
        self.status = "Other query tabs closed".to_string();
    }

    fn tab_bar(&mut self, ui: &mut Ui) {
        let ids: Vec<QueryId> = self.tabs.keys().copied().collect();
        ui.horizontal_wrapped(|ui| {
            for id in ids {
                let Some(title) = self.tabs.get(&id).map(|tab| {
                    if tab.dirty {
                        format!("* {}", tab.title)
                    } else {
                        tab.title.clone()
                    }
                }) else {
                    continue;
                };
                let response = ui.selectable_label(self.active_tab == Some(id), title);
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
            }
        });
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
                if ui.button(tr("Run Current")).clicked() {
                    self.execute_current_statement();
                }
                if ui
                    .add_enabled(has_selection, Button::new("Run Selection"))
                    .clicked()
                {
                    self.execute_selection();
                }
                if ui.button(tr("Run All")).clicked() {
                    self.show_execute_all = true;
                }
                if ui.button(tr("Format SQL")).clicked() {
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
                ui.add_enabled(false, Button::new("Cancelling..."));
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
        let editor_height = (ui.available_height() * 0.42).max(180.0);
        let side_width = self.side_panel_width;
        let connections_snapshot = self.connections.clone();
        let mut column_loads = Vec::new();
        ui.horizontal(|ui| {
            let main_width = if self.side_panel_open {
                (ui.available_width() - side_width - 12.0).max(360.0)
            } else {
                ui.available_width()
            };

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
                            .max_height((editor_height - 32.0).max(120.0))
                            .show(ui, |ui| {
                                let tokens = tab.highlighted_sql.clone();
                                let tab_pressed = ui.input_mut(|input| {
                                    if tab.completion.visible
                                        && !tab.completion.items.is_empty()
                                        && input.key_pressed(Key::Tab)
                                    {
                                        input.consume_key(eframe::egui::Modifiers::NONE, Key::Tab)
                                    } else {
                                        false
                                    }
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
                                    .font(FontId::monospace(14.0))
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(16)
                                    .code_editor()
                                    .frame(false)
                                    .layouter(&mut layouter)
                                    .show(ui);

                                let mut refresh_completion = output.response.has_focus()
                                    && ui.input(|input| {
                                        input.key_pressed(Key::Space) && input.modifiers.ctrl
                                    });

                                if output.response.changed() {
                                    tab.dirty = true;
                                    schedule_sql_highlight(tab);
                                    refresh_completion = true;
                                }

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
                                }

                                if refresh_completion {
                                    update_completion_for_tab(tab);
                                    column_loads.extend(missing_completion_columns(tab));
                                }

                                if output.response.has_focus()
                                    && tab.completion.visible
                                    && ui.input(|input| input.key_pressed(Key::Escape))
                                {
                                    tab.completion.visible = false;
                                }

                                if let Some(inserted) = completion_panel(ui, tab, tab_pressed) {
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
                                    update_completion_for_tab(tab);
                                }
                            });
                    });
            });

            if self.side_panel_open {
                let (divider_rect, divider_response) = ui.allocate_exact_size(
                    vec2(8.0, ui.available_height()),
                    eframe::egui::Sense::drag(),
                );
                if divider_response.dragged() {
                    self.side_panel_width = (self.side_panel_width
                        - divider_response.drag_delta().x)
                        .clamp(200.0, 520.0);
                    ui.ctx().request_repaint();
                }
                ui.painter().line_segment(
                    [divider_rect.center_top(), divider_rect.center_bottom()],
                    Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                );
                ui.vertical(|ui| {
                    ui.set_width(side_width);
                    self.history_panel(ui, tab_id);
                });
            }
        });

        ui.separator();
        self.results_panel(ui, tab_id);
        for table in column_loads {
            self.load_columns_for_tab(tab_id, table);
        }
    }

    fn history_panel(&mut self, ui: &mut Ui, tab_id: QueryId) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.side_tab, QuerySideTab::History, tr("History"));
            ui.selectable_value(&mut self.side_tab, QuerySideTab::Favorites, tr("Favorites"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("x").clicked() {
                    self.side_panel_open = false;
                }
            });
        });
        ui.add_space(4.0);

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
                ui.colored_label(Color32::RED, error);
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
                ui.label(
                    RichText::new(result.statement.trim())
                        .small()
                        .color(Color32::GRAY),
                );
                ui.separator();
                if ui.button("Export CSV").clicked() {
                    export_request = Some((
                        result.grid.as_ref().map(|grid| grid.result().clone()),
                        ExportFormat::Csv,
                    ));
                }
                if ui.button("Export JSON").clicked() {
                    export_request = Some((
                        result.grid.as_ref().map(|grid| grid.result().clone()),
                        ExportFormat::Json,
                    ));
                }
            });

            for notice in &result.notices {
                ui.label(RichText::new(notice).small().color(Color32::LIGHT_BLUE));
            }

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

        tab.execution_running = true;
        tab.execution_cancelling = false;
        tab.execution_error = None;

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
            timeout_ms: 30_000,
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
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
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
                                    tab.execution_error = Some(error.message.clone());
                                    self.status = format!("Execution failed: {}", error.message);
                                }
                            }
                        }
                    }
                    self.load_history();
                }
                QueryEditorEvent::ResultPageCompleted(tab_id, result_index, result) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.execution_running = false;
                        tab.execution_cancelling = false;
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
                                self.status = format!("Page load failed: {}", error.message);
                            }
                        }
                    }
                    self.load_history();
                }
                QueryEditorEvent::StopCompleted(tab_id, result) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.execution_cancelling = false;
                        if let Err(error) = result {
                            tab.execution_error = Some(error.message.clone());
                            self.status = format!("Stop failed: {}", error.message);
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
                        self.favorite_name_input.clear();
                        self.status = "Favorite saved".to_string();
                    }
                    Err(error) => {
                        self.status = format!("Save favorite failed: {}", error.message);
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
                        update_completion_for_tab(tab);
                    }
                }
                QueryEditorEvent::ColumnList(tab_id, table, columns) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.pending_column_loads
                            .remove(&normalize_identifier(&table));
                        tab.columns_by_table
                            .insert(normalize_identifier(&table), columns);
                        update_completion_for_tab(tab);
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

        let Some(path) = rfd::FileDialog::new()
            .set_title(format!("Export {label}"))
            .add_filter(label, &[ext])
            .save_file()
        else {
            return;
        };

        let path_for_task = path.clone();
        self.status = format!("{label} export started: {}", path.display());
        self.runtime.spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                export_query_result_to_file(&query_result, format, &path_for_task)
            })
            .await;
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
        });
        ScrollArea::vertical()
            .id_source("query_history_panel")
            .max_height(260.0)
            .show(ui, |ui| {
                if self.history_entries.is_empty() {
                    ui.label(
                        RichText::new("No history yet")
                            .italics()
                            .color(Color32::GRAY),
                    );
                    return;
                }

                let mut picked_sql = None;
                for entry in self.history_entries.iter().take(100) {
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

                    eframe::egui::Frame::group(ui.style()).show(ui, |ui| {
                        if ui
                            .selectable_label(false, &entry.summary)
                            .on_hover_text(&entry.sql)
                            .clicked()
                        {
                            picked_sql = Some(entry.sql.clone());
                        }
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(conn_name).small().color(Color32::GRAY));
                            ui.label(
                                RichText::new(
                                    entry.executed_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                                )
                                .small()
                                .color(Color32::GRAY),
                            );
                            ui.label(
                                RichText::new(format!("{} ms", entry.elapsed_ms))
                                    .small()
                                    .color(Color32::GRAY),
                            );
                            ui.label(
                                RichText::new(format!("{} rows", entry.affected_rows))
                                    .small()
                                    .color(Color32::GRAY),
                            );
                            ui.label(
                                RichText::new(if entry.success { "OK" } else { "ERR" })
                                    .small()
                                    .color(success_color),
                            );
                        });
                    });
                    ui.add_space(4.0);
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
        });
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.favorite_name_input)
                    .hint_text("Favorite name")
                    .desired_width(180.0),
            );
            if ui.button("Save Current").clicked() {
                self.save_current_as_favorite(tab_id);
            }
        });
        ui.add_space(4.0);
        ScrollArea::vertical()
            .id_source("query_favorites_panel")
            .max_height(260.0)
            .show(ui, |ui| {
                if self.favorite_entries.is_empty() {
                    ui.label(
                        RichText::new("No favorites yet")
                            .italics()
                            .color(Color32::GRAY),
                    );
                    return;
                }

                let mut picked_sql = None;
                let mut remove_id = None;
                for entry in &self.favorite_entries {
                    eframe::egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.selectable_label(false, &entry.title).clicked() {
                                picked_sql = Some(entry.sql.clone());
                            }
                            if ui.small_button("Delete").clicked() {
                                remove_id = Some(entry.id);
                            }
                        });
                        ui.label(
                            RichText::new(entry.updated_at.format("%Y-%m-%d %H:%M:%S").to_string())
                                .small()
                                .color(Color32::GRAY),
                        );
                    });
                    ui.add_space(4.0);
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
        let title = if self.favorite_name_input.trim().is_empty() {
            statement_title(&tab.sql)
        } else {
            self.favorite_name_input.trim().to_string()
        };
        let service = self.query_service.clone();
        let tx = self.tx.clone();
        let sql = tab.sql.clone();
        let connection_id = Some(tab.connection_id);
        let database = tab.database.clone();
        let schema = tab.schema.clone();
        self.runtime.spawn(async move {
            let result = service
                .add_favorite(connection_id, database, schema, title, sql)
                .await;
            let _ = tx.send(QueryEditorEvent::FavoriteSaved(result));
        });
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

fn next_tab_title(connection_name: &str) -> String {
    format!("Query · {}", connection_name)
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
        let start = token.range.start.min(text.len());
        let end = token.range.end.min(text.len());
        if start > cursor {
            job.append(&text[cursor..start], 0.0, default_format.clone());
        }
        if start < end {
            job.append(&text[start..end], 0.0, token_format(token.kind, dialect));
        }
        cursor = end;
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, default_format);
    }
    job
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

/// Determine what the user is currently typing and refresh the completion list.
///
/// The heuristic walks backwards from the cursor and collects the partial
/// identifier being typed. If the partial identifier follows a dot we try to
/// resolve it as a column of that table; otherwise we offer tables and
/// keywords. Column lookups trigger lazy metadata loads via
/// `missing_completion_columns`.
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

    // If we are right after a dot, offer columns of the preceding identifier.
    let dot_context = before[..word_start].ends_with('.').then(|| {
        // Walk back to the identifier before the dot.
        let before_dot = &before[..word_start - 1];
        let id_start = before_dot
            .char_indices()
            .rev()
            .take_while(|(_, ch)| is_completion_char(*ch))
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(before_dot.len());
        before_dot[id_start..].to_string()
    });

    let mut items = Vec::new();
    let replacement = TextRange::new(word_start, cursor);

    if let Some(table_token) = dot_context {
        let key = normalize_identifier(&table_token);
        if let Some(columns) = tab.columns_by_table.get(&key) {
            for column in columns {
                if matches_prefix(&column.name, prefix) {
                    items.push(CompletionItem {
                        label: column.name.clone(),
                        insert_text: column.name.clone(),
                        kind: CompletionKind::Column,
                        detail: column.database_type.clone(),
                    });
                }
            }
        }
    } else {
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

    if items.is_empty() && !prefix.is_empty() {
        tab.completion.visible = false;
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
    tab.completion.waiting_for_metadata = tab.completion.items.is_empty();
}

/// Return tables referenced by the current completion that have no loaded
/// columns yet. The caller spawns metadata loads for each.
fn missing_completion_columns(tab: &QueryEditorTab) -> Vec<String> {
    if !tab.completion.visible {
        return Vec::new();
    }
    // If the user just typed a table name with a trailing dot, try to load its
    // columns so the next keystroke offers column completions.
    let cursor = tab.cursor_offset.min(tab.sql.len());
    let before = &tab.sql[..cursor];
    let word_start = before
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_completion_char(*ch))
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(cursor);
    let mut needed = Vec::new();
    // Detect "table." (with or without a partial column name) and queue a
    // column load. This makes completion available immediately after typing '.'.
    let before_word = &before[..word_start];
    if let Some(stripped) = before_word.strip_suffix('.') {
        let id_end = stripped.len();
        let id_start = stripped
            .char_indices()
            .rev()
            .take_while(|(_, ch)| is_completion_char(*ch))
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(id_end);
        let table_token = &stripped[id_start..];
        if !table_token.is_empty() {
            let key = normalize_identifier(table_token);
            if !tab.columns_by_table.contains_key(&key)
                && !tab.pending_column_loads.contains(&key)
                && tab
                    .available_tables
                    .iter()
                    .any(|name| normalize_identifier(name) == key)
            {
                needed.push(table_token.to_string());
            }
        }
    }

    needed
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

/// Render the completion popup and return the SQL with the selected item
/// inserted if the user accepted a suggestion.
fn completion_panel(ui: &mut Ui, tab: &mut QueryEditorTab, tab_pressed: bool) -> Option<String> {
    if !tab.completion.visible {
        return None;
    }

    let mut clicked = None;
    Frame::popup(ui.style()).show(ui, |ui| {
        if tab.completion.waiting_for_metadata {
            ui.label("Loading column suggestions...");
        }
        for (index, item) in tab.completion.items.iter().take(8).enumerate() {
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
            if ui
                .selectable_label(
                    selected,
                    RichText::new(format!("{}  [{}]{}", item.label, kind, detail)).color(
                        if selected {
                            ui.visuals().selection.stroke.color
                        } else {
                            ui.visuals().text_color()
                        },
                    ),
                )
                .clicked()
            {
                clicked = Some(index);
            }
        }
    });

    let input = ui.input(|input| {
        (
            input.key_pressed(Key::ArrowDown),
            input.key_pressed(Key::ArrowUp),
            input.key_pressed(Key::Enter),
            input.key_pressed(Key::Escape),
        )
    });
    let (down, up, enter, escape) = input;

    if escape {
        tab.completion.visible = false;
        return None;
    }

    let count = tab.completion.items.len();
    if count == 0 {
        return None;
    }
    if down {
        tab.completion.selected = (tab.completion.selected + 1) % count;
    }
    if up {
        tab.completion.selected = (tab.completion.selected + count - 1) % count;
    }

    if let Some(index) = clicked {
        tab.completion.selected = index;
    }
    let accept = enter || clicked.is_some() || (tab_pressed && count == 1);
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
