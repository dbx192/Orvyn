use app_core::{connection::ConnectionManager, query::QueryService};
use db_common::{
    AppError, ConnectionId, ConnectionProfile, DataSourceKind, QueryExecutionMode,
    QueryExecutionReport, QueryExecutionRequest, QueryId, QueryResult, ResultSet, SqlHighlightToken,
    TextRange,
};
use eframe::egui::{
    Align, Button, CentralPanel, Checkbox, Color32, ComboBox, Frame, Label, Layout, RichText,
    ScrollArea, Separator, Sense, TextEdit, Ui, Window,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::mpsc::{self, Receiver, Sender},
};
use tokio::runtime::Handle;

enum QueryEditorEvent {
    TabOpened(QueryId),
    TabUpdated(QueryId, String, usize, Option<TextRange>),
    TabClosed(QueryId),
    ExecutionStarted(QueryId),
    ExecutionCompleted(QueryId, Result<QueryExecutionReport, AppError>),
    ExecutionCancelled(QueryId),
    ConnectionList(Vec<ConnectionProfile>),
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
    highlighted_tokens: Vec<SqlHighlightToken>,
    results: Option<QueryExecutionReport>,
    execution_running: bool,
    execution_error: Option<String>,
}

impl QueryEditorTab {
    fn new(connection_id: ConnectionId) -> Self {
        Self {
            id: QueryId::new(),
            title: "Query".to_string(),
            sql: String::new(),
            cursor_offset: 0,
            selection: None,
            dirty: false,
            connection_id,
            database: None,
            schema: None,
            highlighted_tokens: Vec::new(),
            results: None,
            execution_running: false,
            execution_error: None,
        }
    }
}

pub struct QueryEditor {
    query_service: QueryService,
    connection_manager: ConnectionManager,
    runtime: Handle,
    tx: Sender<QueryEditorEvent>,
    rx: Receiver<QueryEditorEvent>,
    tabs: BTreeMap<QueryId, QueryEditorTab>,
    active_tab: Option<QueryId>,
    connections: Vec<ConnectionProfile>,
    show_close_confirm: Option<QueryId>,
    show_execute_all: bool,
    status: String,
}

impl QueryEditor {
    pub fn new(
        query_service: QueryService,
        connection_manager: ConnectionManager,
        runtime: Handle,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut editor = Self {
            query_service,
            connection_manager,
            runtime,
            tx,
            rx,
            tabs: BTreeMap::new(),
            active_tab: None,
            connections: Vec::new(),
            show_close_confirm: None,
            show_execute_all: false,
            status: String::new(),
        };
        editor.load_connections();
        editor
    }

    pub fn ui(&mut self, ctx: &eframe::egui::Context) -> Option<String> {
        self.drain_events();

        if self.tabs.is_empty() {
            CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label("No query tab open");
                    if ui.button("New Query").clicked() {
                        self.new_query();
                    }
                });
            });
            return None;
        }

        CentralPanel::default().show(ctx, |ui| {
            self.toolbar(ui);
            ui.separator();
            self.tabs_bar(ui);
            ui.separator();
            if let Some(tab) = self.active_tab.as_ref().and_then(|id| self.tabs.get_mut(id)) {
                self.editor_panel(ui, tab);
                ui.separator();
                self.results_panel(ui, tab);
            }
        });

        self.close_confirm_dialog(ctx);
        self.execute_all_dialog(ctx);

        Some(self.status.clone())
    }

    pub fn new_query_with_connection(&mut self, connection_id: ConnectionId) {
        let tab = QueryEditorTab::new(connection_id);
        self.active_tab = Some(tab.id);
        self.tabs.insert(tab.id, tab);
        self.status = "New query tab created".to_string();
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.button("New Query").on_click(|_| {
                self.new_query();
            });

            ui.separator();

            self.connection_selector(ui);

            ui.separator();

            let active_tab = self.active_tab.as_ref().and_then(|id| self.tabs.get(id));
            let has_selection = active_tab.map(|t| t.selection.is_some()).unwrap_or(false);
            let is_running = active_tab.map(|t| t.execution_running).unwrap_or(false);

            if !is_running {
                if ui.button("▶ Run").clicked() {
                    self.execute_current_statement();
                }
                if ui.add_enabled(has_selection, Button::new("▶ Run Selection")).clicked() {
                    self.execute_selection();
                }
                if ui.button("▶ Run All").clicked() {
                    self.show_execute_all = true;
                }
            } else {
                if ui.button("■ Stop").clicked() {
                    self.stop_execution();
                }
            }

            ui.separator();

            if let Some(tab) = active_tab {
                if let Some(report) = &tab.results {
                    ui.label(
                        RichText::new(format!("{} ms", report.elapsed_ms))
                            .color(Color32::GREEN)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format!("{} rows affected", report.affected_rows))
                            .color(Color32::BLUE)
                            .strong(),
                    );
                }
            }
        });
    }

    fn connection_selector(&mut self, ui: &mut Ui) {
        let active_tab = self.active_tab.as_ref().and_then(|id| self.tabs.get(id));
        let current_connection = active_tab.map(|t| t.connection_id);

        ComboBox::from_id_source("connection_selector")
            .selected_text(
                current_connection
                    .and_then(|id| {
                        self.connections
                            .iter()
                            .find(|c| c.id == id)
                            .map(|c| format!("{} ({})", c.name, c.kind.as_str()))
                    })
                    .unwrap_or_else(|| "Select connection".to_string()),
            )
            .show_ui(ui, |ui| {
                for conn in &self.connections {
                    if conn.kind.is_relational() {
                        let selected = current_connection == Some(conn.id);
                        if ui.selectable_label(selected, format!("{} ({})", conn.name, conn.kind.as_str())).clicked() {
                            if let Some(id) = self.active_tab {
                                if let Some(tab) = self.tabs.get_mut(&id) {
                                    tab.connection_id = conn.id;
                                    tab.results = None;
                                    tab.database = None;
                                    tab.schema = None;
                                    self.status = format!("Connection changed to {}", conn.name);
                                }
                            }
                        }
                    }
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

                let resp = ui.selectable_label(selected, title);
                if resp.clicked() {
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

    fn editor_panel(&mut self, ui: &mut Ui, tab: &mut QueryEditorTab) {
        let height = ui.available_height() / 2.0;

        ScrollArea::vertical().max_height(height).show(ui, |ui| {
            let text_edit = TextEdit::multiline(&mut tab.sql)
                .font(eframe::egui::FontId::monospace(14.0))
                .desired_width(f32::INFINITY)
                .code_editor()
                .show_lines(30);

            let response = text_edit.response;

            if response.changed() {
                tab.dirty = true;
                self.update_highlight(tab);
            }

            tab.cursor_offset = response.cursor_range().map(|r| r.min()).unwrap_or(0);
            tab.selection = response.cursor_range().map(|r| {
                let min = r.min();
                let max = r.max();
                TextRange { min, max }
            });
        });
    }

    fn results_panel(&mut self, ui: &mut Ui, tab: &QueryEditorTab) {
        let height = ui.available_height();

        ScrollArea::vertical().max_height(height).show(ui, |ui| {
            if tab.execution_running {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Executing...").color(Color32::YELLOW).strong());
                });
                return;
            }

            if let Some(error) = &tab.execution_error {
                ui.colored_label(Color32::RED, error);
                return;
            }

            if let Some(report) = &tab.results {
                if report.results.is_empty() {
                    ui.label("No results");
                    return;
                }

                for (idx, result) in report.results.iter().enumerate() {
                    ui.heading(format!("Result Set {}", idx + 1));

                    if !result.columns.is_empty() || !result.rows.is_empty() {
                        self.render_result(ui, result);
                    } else {
                        ui.label(format!("Affected rows: {}", result.affected_rows));
                    }

                    if idx < report.results.len() - 1 {
                        ui.separator();
                    }
                }
            } else {
                ui.label("No query executed yet");
            }
        });
    }

    fn render_result(&mut self, ui: &mut Ui, result: &QueryResult) {
        let row_count = result.rows.len();
        let col_count = result.columns.len();

        if col_count == 0 {
            ui.label("Empty result");
            return;
        }

        ScrollArea::horizontal().show(ui, |ui| {
            eframe::egui::Grid::new("result_grid")
                .num_columns(col_count)
                .spacing([8.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for col in &result.columns {
                        ui.label(RichText::new(&col.name).bold());
                    }
                    ui.end_row();

                    for row in &result.rows {
                        for val in &row.values {
                            ui.label(val.to_string());
                        }
                        ui.end_row();
                    }
                });

            ui.add_space(8.0);
            ui.label(format!("{} rows", row_count));
        });
    }

    fn new_query(&mut self) {
        let connection_id = self.connections.first().map(|c| c.id);
        if let Some(conn_id) = connection_id {
            self.new_query_with_connection(conn_id);
        } else {
            self.status = "No connections available".to_string();
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

    fn execute_current_statement(&mut self) {
        if let Some(tab_id) = self.active_tab {
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                if tab.sql.is_empty() {
                    self.status = "No SQL to execute".to_string();
                    return;
                }

                tab.execution_running = true;
                tab.execution_error = None;
                tab.results = None;

                let tx = self.tx.clone();
                let service = self.query_service.clone();
                let request = QueryExecutionRequest {
                    tab_id,
                    connection_id: tab.connection_id,
                    sql: tab.sql.clone(),
                    mode: QueryExecutionMode::CurrentStatement,
                    cursor_offset: tab.cursor_offset,
                    selection: None,
                    timeout_ms: 30000,
                };

                self.status = "Executing current statement...".to_string();

                self.runtime.spawn(async move {
                    let result = service.execute(request, None).await;
                    let _ = tx.send(QueryEditorEvent::ExecutionCompleted(tab_id, result));
                });
            }
        }
    }

    fn execute_selection(&mut self) {
        if let Some(tab_id) = self.active_tab {
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                if tab.selection.is_none() {
                    self.status = "No selection".to_string();
                    return;
                }

                tab.execution_running = true;
                tab.execution_error = None;
                tab.results = None;

                let tx = self.tx.clone();
                let service = self.query_service.clone();
                let request = QueryExecutionRequest {
                    tab_id,
                    connection_id: tab.connection_id,
                    sql: tab.sql.clone(),
                    mode: QueryExecutionMode::Selection,
                    cursor_offset: tab.cursor_offset,
                    selection: tab.selection.clone(),
                    timeout_ms: 30000,
                };

                self.status = "Executing selection...".to_string();

                self.runtime.spawn(async move {
                    let result = service.execute(request, None).await;
                    let _ = tx.send(QueryEditorEvent::ExecutionCompleted(tab_id, result));
                });
            }
        }
    }

    fn stop_execution(&mut self) {
        if let Some(tab_id) = self.active_tab {
            let _ = self.query_service.close_tab(tab_id);

            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                tab.execution_running = false;
                tab.execution_error = Some("Execution cancelled".to_string());
            }

            self.status = "Execution stopped".to_string();
        }
    }

    fn update_highlight(&mut self, tab: &mut QueryEditorTab) {
        let tx = self.tx.clone();
        let service = self.query_service.clone();
        let tab_id = tab.id;

        self.runtime.spawn(async move {
            if let Ok(tokens) = service.highlight_tab_sql(tab_id).await {
                let _ = tx.send(QueryEditorEvent::TabUpdated(tab_id, String::new(), 0, None));
            }
        });
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

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                QueryEditorEvent::ExecutionCompleted(tab_id, result) => {
                    if let Some(tab) = self.tabs.get_mut(&tab_id) {
                        tab.execution_running = false;

                        match result {
                            Ok(report) => {
                                tab.results = Some(report.clone());
                                self.status = format!(
                                    "Execution completed in {} ms, {} rows affected",
                                    report.elapsed_ms, report.affected_rows
                                );
                            }
                            Err(error) => {
                                tab.execution_error = Some(error.message);
                                self.status = format!("Execution failed: {}", error.message);
                            }
                        }
                    }
                }
                QueryEditorEvent::ConnectionList(profiles) => {
                    self.connections = profiles;
                }
                _ => {}
            }
        }
    }

    fn close_confirm_dialog(&mut self, ctx: &eframe::egui::Context) {
        if let Some(tab_id) = self.show_close_confirm {
            let tab = self.tabs.get(&tab_id);
            let title = tab.map(|t| &t.title).unwrap_or("Query");

            let mut open = true;
            Window::new("Confirm Close")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("'{}' has unsaved changes. Close anyway?", title));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            open = false;
                        }
                        if ui.button("Close").clicked() {
                            self.close_tab(tab_id);
                            open = false;
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
                    ui.label("Warning: This will execute ALL statements in the editor.");
                    ui.label("This may modify data or have unexpected side effects.");
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            open = false;
                        }
                        if ui.button("Execute All").clicked() {
                            self.execute_all_statements();
                            open = false;
                        }
                    });
                });

            if !open {
                self.show_execute_all = false;
            }
        }
    }

    fn execute_all_statements(&mut self) {
        if let Some(tab_id) = self.active_tab {
            if let Some(tab) = self.tabs.get_mut(&tab_id) {
                if tab.sql.is_empty() {
                    self.status = "No SQL to execute".to_string();
                    return;
                }

                tab.execution_running = true;
                tab.execution_error = None;
                tab.results = None;

                let tx = self.tx.clone();
                let service = self.query_service.clone();
                let request = QueryExecutionRequest {
                    tab_id,
                    connection_id: tab.connection_id,
                    sql: tab.sql.clone(),
                    mode: QueryExecutionMode::All,
                    cursor_offset: tab.cursor_offset,
                    selection: None,
                    timeout_ms: 60000,
                };

                self.status = "Executing all statements...".to_string();

                self.runtime.spawn(async move {
                    let result = service.execute(request, None).await;
                    let _ = tx.send(QueryEditorEvent::ExecutionCompleted(tab_id, result));
                });
            }
        }
    }
}