use crate::ui::result_grid::ResultSetGrid;
use app_core::{connection::ConnectionManager, metadata::MetadataService, query::QueryService};
use db_common::{
    looks_like_unqualified_mutation, statement_at_offset, AppError, ConnectionId,
    ConnectionProfile, DataSourceKind, DatabaseObject, QueryExecutionMode,
    QueryExecutionReport, QueryExecutionRequest, QueryHistoryEntry, QueryId, SqlHighlightToken,
    SqlTokenKind, TextRange,
};
use eframe::egui::{
    text::LayoutJob, Button, Color32, ComboBox, FontId, RichText, ScrollArea, TextEdit,
    TextFormat, Ui, Window,
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    sync::mpsc::{self, Receiver, Sender},
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
    ConnectionList(Vec<ConnectionProfile>),
    DatabaseList(QueryId, Vec<String>),
    SchemaList(QueryId, Vec<String>),
    HistoryLoaded(Vec<QueryHistoryEntry>),
    MetadataLoadFailed(QueryId, String),
}

struct PendingDangerConfirm {
    mode: QueryExecutionMode,
    sql: String,
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
    results: Vec<QueryResultView>,
    selected_result: usize,
    execution_report: Option<QueryExecutionReport>,
    execution_running: bool,
    execution_error: Option<String>,
    highlighted_sql: Vec<SqlHighlightToken>,
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
            results: Vec::new(),
            selected_result: 0,
            execution_report: None,
            execution_running: false,
            execution_error: None,
            highlighted_sql: Vec::new(),
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
    show_close_confirm: Option<QueryId>,
    show_execute_all: bool,
    pending_danger_confirm: Option<PendingDangerConfirm>,
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
            show_close_confirm: None,
            show_execute_all: false,
            pending_danger_confirm: None,
            status: String::new(),
        };
        editor.load_connections();
        editor.load_history();
        editor
    }

    pub fn ui(&mut self, ctx: &eframe::egui::Context, ui: &mut Ui) -> Option<String> {
        self.drain_events();

        if self.tabs.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical(|ui| {
                    ui.label("No query tab open");
                    if ui.button("New Query").clicked() {
                        self.new_query();
                    }
                });
            });
            self.close_confirm_dialog(ctx);
            self.execute_all_dialog(ctx);
            return Some(self.status.clone());
        }

        self.toolbar(ui);
        ui.separator();
        self.tabs_bar(ui);
        ui.separator();
        self.active_tab_panel(ui);
        self.close_confirm_dialog(ctx);
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
        let profile = self
            .connections
            .iter()
            .find(|profile| profile.kind.is_relational())
            .cloned();

        if let Some(profile) = profile {
            self.open_new_tab(profile);
        } else {
            self.status = "No relational connections available".to_string();
        }
    }

    pub fn new_query_with_connection(
        &mut self,
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
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
            }
            self.refresh_metadata_for_tab(tab_id);
        } else {
            self.status = "Selected connection is unavailable".to_string();
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

        let service = self.query_service.clone();
        self.runtime.spawn(async move {
            let _ = service.stop_execution(tab_id).await;
        });

        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.execution_running = false;
            tab.execution_error = Some("Execution cancelled".to_string());
        }
        self.status = "Execution stopped".to_string();
    }

    fn open_new_tab(&mut self, profile: ConnectionProfile) -> QueryId {
        let tab = QueryEditorTab::new(&profile);
        let tab_id = tab.id;
        self.tabs.insert(tab_id, tab);
        self.active_tab = Some(tab_id);
        self.refresh_metadata_for_tab(tab_id);
        self.status = "New query tab created".to_string();
        tab_id
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("New Query").clicked() {
                self.new_query();
            }

            ui.separator();
            self.connection_selector(ui);
            self.database_selector(ui);
            self.schema_selector(ui);
            ui.separator();

            let active_tab_id = self.active_tab;
            let (is_running, has_selection) = active_tab_id
                .and_then(|id| self.tabs.get(&id))
                .map(|tab| (tab.execution_running, tab.has_selection()))
                .unwrap_or((false, false));

            if !is_running {
                if ui.button("Run Current").clicked() {
                    self.execute_current_statement();
                }
                if ui
                    .add_enabled(has_selection, Button::new("Run Selection"))
                    .clicked()
                {
                    self.execute_selection();
                }
                if ui.button("Run All").clicked() {
                    self.show_execute_all = true;
                }
            } else if ui.button("Stop").clicked() {
                self.stop_execution();
            }

            if let Some(tab) = active_tab_id.and_then(|id| self.tabs.get(&id)) {
                ui.separator();
                if tab.execution_running {
                    ui.label(RichText::new("Running").color(Color32::YELLOW).strong());
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
                if ui.selectable_label(false, "Default").clicked() {
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
                if ui.selectable_label(false, "Default").clicked() {
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

    fn tabs_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let mut close_tab = None;
            for (id, tab) in &self.tabs {
                let selected = self.active_tab == Some(*id);
                let title = if tab.dirty {
                    format!("{} *", tab.title)
                } else {
                    tab.title.clone()
                };

                if ui.selectable_label(selected, title).clicked() {
                    self.active_tab = Some(*id);
                }
                if ui.small_button("x").clicked() {
                    close_tab = Some(*id);
                }
            }

            if let Some(tab_id) = close_tab {
                self.close_tab_request(tab_id);
            }
        });
    }

    fn active_tab_panel(&mut self, ui: &mut Ui) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        let editor_height = (ui.available_height() * 0.42).max(180.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(300.0);
                self.history_panel(ui, tab_id);
            });
            ui.separator();
            ui.vertical(|ui| {
                let Some(tab) = self.tabs.get_mut(&tab_id) else {
                    return;
                };

                ScrollArea::vertical()
                    .id_source("query_editor_sql_scroll")
                    .max_height(editor_height)
                    .show(ui, |ui| {
                        let tokens = tab.highlighted_sql.clone();
                        let dialect = connection_kind_for(&self.connections, tab.connection_id);
                        let mut layouter =
                            move |ui: &Ui, text: &str, wrap_width: f32| -> Arc<eframe::egui::Galley> {
                                let mut job = highlight_layout_job(text, &tokens, dialect);
                                job.wrap.max_width = wrap_width;
                                ui.fonts(|fonts| fonts.layout_job(job))
                            };

                        let output = TextEdit::multiline(&mut tab.sql)
                            .font(FontId::monospace(14.0))
                            .desired_width(f32::INFINITY)
                            .desired_rows(14)
                            .code_editor()
                            .layouter(&mut layouter)
                            .show(ui);

                        if output.response.changed() {
                            tab.dirty = true;
                            tab.highlighted_sql = db_common::highlight_sql(&tab.sql);
                        }

                        if let Some(range) = output.cursor_range {
                            tab.cursor_offset = range.primary.ccursor.index;
                            let selection = TextRange::new(
                                range.primary.ccursor.index,
                                range.secondary.ccursor.index,
                            );
                            tab.selection = (!selection.is_empty()).then_some(selection);
                        }
                    });
            });
        });

        ui.separator();
        self.results_panel(ui, tab_id);
    }

    fn history_panel(&mut self, ui: &mut Ui, tab_id: QueryId) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("History").strong());
            ui.label(
                RichText::new(format!("{} entries", self.history_entries.len()))
                    .small()
                    .color(Color32::GRAY),
            );
        });
        ui.add_space(4.0);

        ScrollArea::vertical()
            .id_source("query_history_panel")
            .max_height(260.0)
            .show(ui, |ui| {
                if self.history_entries.is_empty() {
                    ui.label(RichText::new("No history yet").italics().color(Color32::GRAY));
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
                                RichText::new(entry.executed_at.format("%Y-%m-%d %H:%M:%S").to_string())
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
                            ui.label(RichText::new(if entry.success { "OK" } else { "ERR" }).small().color(success_color));
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

    fn results_panel(&mut self, ui: &mut Ui, tab_id: QueryId) {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };

        if tab.execution_running {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Executing...")
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
            ui.label("No query executed yet");
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

        let Some(result) = tab.results.get_mut(tab.selected_result) else {
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
        });

        for notice in &result.notices {
            ui.label(RichText::new(notice).small().color(Color32::LIGHT_BLUE));
        }

        ui.add_space(6.0);
        match &mut result.grid {
            Some(grid) => {
                let _ = grid.ui(ui);
            }
            None => {
                ui.label("Statement executed without a tabular result.");
            }
        }
    }

    fn close_tab_request(&mut self, tab_id: QueryId) {
        if let Some(tab) = self.tabs.get(&tab_id) {
            if tab.dirty {
                self.show_close_confirm = Some(tab_id);
            } else {
                self.close_tab(tab_id);
            }
        }
    }

    fn close_tab(&mut self, tab_id: QueryId) {
        self.tabs.remove(&tab_id);
        if self.active_tab == Some(tab_id) {
            self.active_tab = self.tabs.keys().next().copied();
        }
        self.status = "Tab closed".to_string();
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

    fn execute_active_confirmed(&mut self, mode: QueryExecutionMode) {
        let Some(tab_id) = self.active_tab else {
            return;
        };

        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return;
        };

        tab.execution_running = true;
        tab.execution_error = None;
        tab.results.clear();
        tab.execution_report = None;

        let tx = self.tx.clone();
        let service = self.query_service.clone();
        let request = QueryExecutionRequest {
            tab_id,
            connection_id: tab.connection_id,
            sql: tab.sql.clone(),
            mode,
            cursor_offset: tab.cursor_offset,
            selection: tab.selection,
            timeout_ms: if matches!(mode, QueryExecutionMode::All) {
                60_000
            } else {
                30_000
            },
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

    fn execute_all_statements(&mut self) {
        self.execute_active(QueryExecutionMode::All);
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
            tab.connection_id = connection_id;
            tab.database = profile.database.clone();
            tab.schema = None;
            tab.available_databases.clear();
            tab.available_schemas.clear();
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
            tab.database = database;
            tab.schema = None;
            tab.available_schemas.clear();
            tab.results.clear();
            tab.execution_report = None;
            self.status = "Database changed".to_string();
        }
        self.load_schemas_for_tab(tab_id);
    }

    fn set_active_schema(&mut self, schema: Option<String>) {
        let Some(tab_id) = self.active_tab else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.schema = schema;
            tab.results.clear();
            tab.execution_report = None;
            self.status = "Schema changed".to_string();
        }
    }

    fn refresh_metadata_for_tab(&mut self, tab_id: QueryId) {
        self.load_databases_for_tab(tab_id);
        self.load_schemas_for_tab(tab_id);
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

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                QueryEditorEvent::ExecutionCompleted(tab_id, result) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.execution_running = false;
                        match result {
                            Ok(report) => {
                                let results = report
                                    .results
                                    .iter()
                                    .enumerate()
                                    .map(|(index, result)| QueryResultView {
                                        statement: result.statement.clone(),
                                        summary: format!("Result {}", index + 1),
                                        grid: (!result.columns.is_empty()).then(|| {
                                            ResultSetGrid::new(result.clone())
                                                .with_runtime(self.runtime.clone())
                                        }),
                                        affected_rows: result.affected_rows,
                                        elapsed_ms: result.execution_time_ms,
                                        notices: result.notices.clone(),
                                    })
                                    .collect::<Vec<_>>();
                                tab.selected_result = 0;
                                tab.execution_report = Some(report.clone());
                                tab.results = results;
                                tab.dirty = false;
                                self.status = format!(
                                    "Execution completed in {} ms, {} rows affected",
                                    report.elapsed_ms, report.affected_rows
                                );
                            }
                            Err(error) => {
                                tab.execution_error = Some(error.message.clone());
                                self.status = format!("Execution failed: {}", error.message);
                            }
                        }
                    }
                    self.load_history();
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
                QueryEditorEvent::MetadataLoadFailed(tab_id, error) => {
                    if self.tabs.contains_key(&tab_id) {
                        self.status = format!("Metadata load failed: {}", error);
                    }
                }
            }
        }
    }

    fn close_confirm_dialog(&mut self, ctx: &eframe::egui::Context) {
        if let Some(tab_id) = self.show_close_confirm {
            let title = self
                .tabs
                .get(&tab_id)
                .map(|tab| tab.title.clone())
                .unwrap_or_else(|| "Query".to_string());

            let mut open = true;
            Window::new("Confirm Close")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("'{}' has unsaved changes. Close anyway?", title));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.show_close_confirm = None;
                        }
                        if ui.button("Close").clicked() {
                            self.close_tab(tab_id);
                            self.show_close_confirm = None;
                        }
                    });
                });

            if !open {
                self.show_close_confirm = None;
            }
        }
    }

    fn execute_all_dialog(&mut self, ctx: &eframe::egui::Context) {
        if self.show_execute_all {
            let mut open = true;
            Window::new("Execute All Statements")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("This will execute every statement in the current tab.");
                    ui.label("Default execution runs only the current statement.");
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.show_execute_all = false;
                        }
                        if ui.button("Execute All").clicked() {
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
        Window::new("Confirm Dangerous SQL")
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
                    if ui.button("Cancel").clicked() {
                        self.pending_danger_confirm = None;
                        self.status = "Dangerous SQL cancelled".to_string();
                    }
                    if ui.button("Execute Anyway").clicked() {
                        self.pending_danger_confirm = None;
                        self.execute_active_confirmed(mode);
                    }
                });
            });

        if !open {
            self.pending_danger_confirm = None;
        }
    }
}

fn connection_label(profile: &ConnectionProfile) -> String {
    format!("{} ({})", profile.name, profile.kind.as_str())
}

fn next_tab_title(connection_name: &str) -> String {
    format!("Query · {}", connection_name)
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
