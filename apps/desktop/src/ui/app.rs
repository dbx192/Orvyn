use crate::ui::{
    connection_manager::ConnectionManagerUi,
    import_wizard::ImportWizard,
    object_tree::{ObjectTree, ObjectTreeAction},
    query_editor::QueryEditor,
    redis_browser::RedisBrowserUi,
    shortcuts::{matches_shortcut, AppShortcut, Shortcut},
    state::{AppState, AppTheme, TaskStatus, UiError},
    table_editor::TableEditor,
    table_structure::TableStructureView,
};
use app_core::{
    connection::ConnectionManager, metadata::MetadataService, query::QueryService,
    redis_service::RedisService, session::SessionManager,
};
use db_common::DatabaseFunction;
use db_common::{
    dialect::{BacktickDialect, DoubleQuoteDialect},
    ConnectionId, DataSourceKind, Dialect, QueryExecutionMode, QueryExecutionRequest, QueryId,
    QueryResult, TableColumn, TableForeignKey, TableIndex, TableTrigger,
};
use eframe::egui::{
    Align, Button, CentralPanel, Color32, Context, Frame, Key, Layout, Modifiers, RichText,
    SidePanel, TopBottomPanel, Window,
};
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

pub struct DesktopApp {
    app_name: String,
    connection_manager_ui: ConnectionManagerUi,
    object_tree: ObjectTree,
    query_editor: QueryEditor,
    state: AppState,
    table_editor: Option<TableEditor>,
    table_structure: Option<TableStructureView>,
    import_wizard: Option<ImportWizard>,
    redis_browser: RedisBrowserUi,
    last_tree_status: String,
    error_window_open: bool,
    active_panel: ActivePanel,
    /// Pending async load for table data (Arc<Mutex> polled each frame).
    pending_table_load: Option<Arc<Mutex<Option<Result<TableDataPayload, db_common::AppError>>>>>,
    /// Pending async load for table structure.
    pending_structure_load:
        Option<Arc<Mutex<Option<Result<TableStructurePayload, db_common::AppError>>>>>,
    // Cloned service handles for spawning async loads.
    connection_manager: ConnectionManager,
    metadata_service: MetadataService,
    query_service: QueryService,
    runtime: Handle,
}

/// Payload produced by the table-data async loader.
struct TableDataPayload {
    connection_id: ConnectionId,
    database: Option<String>,
    schema: Option<String>,
    table: String,
    columns: Vec<TableColumn>,
    result: QueryResult,
    dialect: Box<dyn Dialect>,
}

/// Payload produced by the table-structure async loader.
struct TableStructurePayload {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivePanel {
    QueryEditor,
    TableEditor,
    TableStructure,
    ImportWizard,
    RedisBrowser,
}

impl DesktopApp {
    pub fn new(
        app_name: String,
        connection_manager: ConnectionManager,
        metadata_service: MetadataService,
        session_manager: SessionManager,
        query_service: QueryService,
        redis_service: RedisService,
        runtime: Handle,
    ) -> Self {
        let cm = connection_manager.clone();
        let ms = metadata_service.clone();
        let qs = query_service.clone();
        let rt_cm_ui = runtime.clone();
        let rt_redis = runtime.clone();
        let rt_field = runtime.clone();
        Self {
            app_name,
            connection_manager_ui: ConnectionManagerUi::new(connection_manager.clone(), rt_cm_ui),
            object_tree: ObjectTree::new(
                metadata_service.clone(),
                session_manager,
                runtime.clone(),
            ),
            query_editor: QueryEditor::new(
                query_service,
                connection_manager.clone(),
                metadata_service.clone(),
                runtime,
            ),
            state: AppState::with_sample_workspace(),
            table_editor: None,
            table_structure: None,
            import_wizard: None,
            redis_browser: {
                let redis_conn_id = ConnectionId::new();
                RedisBrowserUi::new(
                    redis_conn_id,
                    connection_manager.clone(),
                    metadata_service.clone(),
                    redis_service.clone(),
                    rt_redis,
                )
            },
            last_tree_status: String::new(),
            error_window_open: false,
            active_panel: ActivePanel::QueryEditor,
            pending_table_load: None,
            pending_structure_load: None,
            connection_manager: cm,
            metadata_service: ms,
            query_service: qs,
            runtime: rt_field,
        }
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let input = ctx.input(|i| i.clone());

        // Ctrl/Cmd+N: New query
        if matches_shortcut(&input, &AppShortcut::NewQuery.with_platform_modifier()) {
            self.query_editor.new_query();
            self.state.set_status("New query (Ctrl+N)");
        }

        // Ctrl/Cmd+Enter: Execute current SQL
        if matches_shortcut(
            &input,
            &AppShortcut::ExecuteCurrent.with_platform_modifier(),
        ) {
            if !self.query_editor.is_executing() {
                self.query_editor.execute_current_statement();
                self.state
                    .set_status("Executing current statement (Ctrl+Enter)");
            }
        }

        // Ctrl/Cmd+Shift+Enter: Execute selected SQL
        let shift_enter = Shortcut {
            modifiers: platform_modifiers(),
            key: Key::Enter,
        };
        if input.modifiers.shift && matches_shortcut(&input, &shift_enter) {
            if !self.query_editor.is_executing() {
                self.query_editor.execute_selection();
                self.state
                    .set_status("Executing selection (Ctrl+Shift+Enter)");
            }
        }

        // Ctrl/Cmd+S: Save/Submit
        if matches_shortcut(&input, &AppShortcut::Save.with_platform_modifier()) {
            self.state.set_status("Save/Submit (Ctrl+S)");
        }

        // Ctrl/Cmd+R: Refresh
        if matches_shortcut(&input, &AppShortcut::Refresh.with_platform_modifier()) {
            self.state.set_status("Refresh (Ctrl+R)");
        }

        // Ctrl/Cmd+F: Search (in result grid or text)
        if matches_shortcut(&input, &AppShortcut::Search.with_platform_modifier()) {
            self.state.set_status("Search (Ctrl+F)");
        }

        // Ctrl/Cmd+L: Focus object search
        if matches_shortcut(
            &input,
            &AppShortcut::FocusObjectSearch.with_platform_modifier(),
        ) {
            self.state.set_search_focused(true);
            self.state.set_status("Focus object search (Ctrl+L)");
        }

        // Esc: Close popup or cancel
        if matches_shortcut(&input, &AppShortcut::ClosePopup.with_platform_modifier()) {
            if self.error_window_open {
                self.error_window_open = false;
                self.state.errors.clear();
            } else {
                self.state.set_status("Esc - cancel/close");
            }
        }

        // Ctrl/Cmd+T: Toggle theme
        if matches_shortcut(&input, &AppShortcut::ToggleTheme.with_platform_modifier()) {
            self.state.toggle_theme();
            let theme_name = match self.state.theme {
                AppTheme::Light => "Light",
                AppTheme::Dark => "Dark",
            };
            self.state
                .set_status(format!("Theme: {} (Ctrl+T)", theme_name));
        }

        // Ctrl/Cmd+Shift+F: Format SQL
        let shift_f = Shortcut {
            modifiers: platform_modifiers(),
            key: Key::F,
        };
        if input.modifiers.shift && matches_shortcut(&input, &shift_f) {
            if let Some(_) = self.query_editor.format_current_sql() {
                self.state.set_status("SQL formatted (Ctrl+Shift+F)");
            }
        }
    }
}

fn platform_modifiers() -> Modifiers {
    #[cfg(target_os = "macos")]
    {
        Modifiers::COMMAND
    }
    #[cfg(not(target_os = "macos"))]
    {
        Modifiers::CTRL
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Poll any pending async data loads first
        self.poll_pending_loads();

        // Handle keyboard shortcuts
        self.handle_shortcuts(ctx);

        // Apply theme
        ctx.set_visuals(match self.state.theme {
            AppTheme::Light => eframe::egui::Visuals::light(),
            AppTheme::Dark => eframe::egui::Visuals::dark(),
        });

        self.top_toolbar(ctx);
        self.left_tree(ctx);
        self.central_panel(ctx);
        self.connection_windows(ctx);
        self.bottom_status(ctx);
        self.error_window(ctx);
    }
}

impl DesktopApp {
    fn central_panel(&mut self, ctx: &Context) {
        CentralPanel::default().show(ctx, |ui| {
            // Tab bar to switch between panels
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.active_panel, ActivePanel::QueryEditor, "📝 Query Editor");
                if self.table_editor.is_some() {
                    ui.selectable_value(&mut self.active_panel, ActivePanel::TableEditor, "📋 Table Editor");
                }
                if self.table_structure.is_some() {
                    ui.selectable_value(&mut self.active_panel, ActivePanel::TableStructure, "📐 Table Structure");
                }
                if self.import_wizard.is_some() {
                    ui.selectable_value(&mut self.active_panel, ActivePanel::ImportWizard, "📥 Import CSV");
                }
                ui.selectable_value(&mut self.active_panel, ActivePanel::RedisBrowser, "🔑 Redis Browser");
            });
            ui.separator();

            match self.active_panel {
                ActivePanel::QueryEditor => {
                    let query_status = self.query_editor.ui(ctx, ui);
                    if let Some(message) = query_status {
                        if !message.is_empty() {
                            self.state.set_status(message);
                        }
                    }
                }
                ActivePanel::TableEditor => match &mut self.table_editor {
                    Some(editor) => {
                        let message = editor.ui(ui);
                        if let Some(msg) = message {
                            if !msg.is_empty() {
                                self.state.set_status(msg);
                            }
                        }
                    }
                    None => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(60.0);
                            ui.label(RichText::new("📋").size(48.0));
                            ui.add_space(12.0);
                            ui.label("No table open. Double-click a table in the object tree to load data.");
                            ui.add_space(8.0);
                            if ui.button("Switch to Query Editor").clicked() {
                                self.active_panel = ActivePanel::QueryEditor;
                            }
                        });
                    }
                },
                ActivePanel::TableStructure => match &mut self.table_structure {
                    Some(view) => {
                        let message = view.ui(ui);
                        if let Some(msg) = message {
                            if !msg.is_empty() {
                                self.state.set_status(msg);
                            }
                        }
                    }
                    None => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(60.0);
                            ui.label(RichText::new("📐").size(48.0));
                            ui.add_space(12.0);
                            ui.label("Loading table structure...");
                        });
                    }
                },
                ActivePanel::ImportWizard => match &mut self.import_wizard {
                    Some(wizard) => {
                        let message = wizard.ui(ui);
                        if let Some(msg) = message {
                            if !msg.is_empty() {
                                self.state.set_status(msg);
                            }
                        }
                    }
                    None => {
                        ui.vertical_centered(|ui| {
                            ui.label("No import wizard active.");
                        });
                    }
                },
                ActivePanel::RedisBrowser => {
                    self.redis_browser.ui(ctx, ui);
                }
            }
        });
    }

    fn top_toolbar(&mut self, ctx: &Context) {
        TopBottomPanel::top("top_toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(&self.app_name);
                ui.separator();

                if ui.button("New Connection").clicked() {
                    self.connection_manager_ui.new_connection();
                    self.state.set_status("New connection editor opened");
                }
                if ui.button("New Query").clicked() {
                    self.query_editor.new_query();
                    self.state.set_status("New query tab opened");
                }
                if ui.button("Refresh").clicked() {
                    self.state.set_status("Refresh requested");
                }

                // Theme toggle button
                let theme_icon = match self.state.theme {
                    AppTheme::Light => "🌙",
                    AppTheme::Dark => "☀️",
                };
                if ui
                    .button(theme_icon)
                    .on_hover_text("Toggle Theme (Ctrl+T)")
                    .clicked()
                {
                    self.state.toggle_theme();
                }

                // Show error button for demo
                if ui.button("Show Error").clicked() {
                    self.state.push_error(
                        UiError::new("Connection Error", "Failed to connect to database server.")
                            .with_detail(
                                "Error details:\n\
                         - Host: localhost:3306\n\
                         - Error code: 1045\n\
                         - Access denied for user 'root'@'localhost'\n\
                         - This may indicate incorrect password or insufficient privileges\n\
                         \n\
                         Stack trace:\n\
                         at Connection.connect() line 45\n\
                         at SessionManager.connect() line 78\n\
                         at DesktopApp.handle_connect() line 123",
                            ),
                    );
                    self.error_window_open = true;
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.shortcut_label(ui);
                    ui.separator();
                    ui.label(RichText::new(&self.state.status).strong());
                });
            });
        });
    }

    fn left_tree(&mut self, ctx: &Context) {
        SidePanel::left("connection_object_tree")
            .resizable(true)
            .default_width(280.0)
            .width_range(220.0..=440.0)
            .show(ctx, |ui| {
                let (message, actions) = self.object_tree.ui(ctx, ui);
                if let Some(message) = message {
                    if !message.is_empty() {
                        self.last_tree_status = message.clone();
                        self.state.set_status(message);
                    }
                }
                self.handle_tree_actions(actions);
                if !self.last_tree_status.is_empty() {
                    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                        ui.label(
                            RichText::new(&self.last_tree_status)
                                .small()
                                .color(Color32::GRAY),
                        );
                    });
                }
            });
    }

    fn handle_tree_actions(&mut self, actions: Vec<ObjectTreeAction>) {
        for action in actions {
            match action {
                ObjectTreeAction::OpenTableData {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    self.state.set_status(format!(
                        "Loading table {}.{} ...",
                        schema.as_deref().unwrap_or(""),
                        table
                    ));
                    self.spawn_table_data_load(connection_id, database, schema, table);
                    self.active_panel = ActivePanel::TableEditor;
                }
                ObjectTreeAction::OpenQuery {
                    connection_id,
                    database,
                    schema,
                } => {
                    self.query_editor
                        .new_query_with_connection(connection_id, database, schema);
                    self.state
                        .set_status(format!("New query on {}", connection_id));
                }
                ObjectTreeAction::ViewKey {
                    connection_id,
                    database,
                    key,
                } => {
                    let database_index = database
                        .as_deref()
                        .and_then(|value| value.parse::<u8>().ok())
                        .unwrap_or(0);
                    self.redis_browser
                        .load_key(connection_id, database_index, key.clone());
                    self.active_panel = ActivePanel::RedisBrowser;
                    self.state.set_status(format!(
                        "View Redis key {key} (db={:?}, conn={})",
                        database, connection_id
                    ));
                }
                ObjectTreeAction::ViewStructure {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    self.state.set_status(format!(
                        "Loading structure for {}.{} ...",
                        schema.as_deref().unwrap_or(""),
                        table
                    ));
                    self.spawn_structure_load(connection_id, database, schema, table);
                    self.active_panel = ActivePanel::TableStructure;
                }
                ObjectTreeAction::ImportCsv {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    let wizard = ImportWizard::new(
                        connection_id,
                        database,
                        schema,
                        table,
                        self.metadata_service.clone(),
                        self.query_service.clone(),
                        self.connection_manager.clone(),
                        self.runtime.clone(),
                    );
                    let table_name = wizard.table_name();
                    self.import_wizard = Some(wizard);
                    self.active_panel = ActivePanel::ImportWizard;
                    self.state
                        .set_status(format!("Import CSV to {}", table_name));
                }
                ObjectTreeAction::ExportTableData {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    self.state.set_status(format!(
                        "Loading table {}.{} for export ...",
                        schema.as_deref().unwrap_or(""),
                        table
                    ));
                    self.spawn_table_data_load(connection_id, database, schema, table);
                    self.active_panel = ActivePanel::TableEditor;
                }
                ObjectTreeAction::EditConnection(id) => {
                    self.connection_manager_ui.edit_connection(id);
                    self.state.set_status(format!("Edit connection: {}", id));
                }
                ObjectTreeAction::DeleteConnection(id) => {
                    self.connection_manager_ui.delete_connection_prompt(id);
                    self.state.set_status(format!("Delete connection: {}", id));
                }
                ObjectTreeAction::DeleteRedisKey {
                    connection_id,
                    database,
                    key,
                } => {
                    let database_index = database
                        .as_deref()
                        .and_then(|value| value.parse::<u8>().ok())
                        .unwrap_or(0);
                    self.redis_browser
                        .prompt_delete_key(connection_id, database_index, key.clone());
                    self.active_panel = ActivePanel::RedisBrowser;
                    self.state.set_status(format!("Delete Redis key {}", key));
                }
            }
        }
    }

    fn connection_windows(&mut self, ctx: &Context) {
        if let Some(message) = self.connection_manager_ui.ui_window(ctx) {
            if !message.is_empty() {
                self.state.set_status(message);
            }
        }
    }

    /// Poll pending async loads. Called at the top of each frame.
    fn poll_pending_loads(&mut self) {
        // ── Table data load ──
        let table_cell = self.pending_table_load.take();
        if let Some(cell) = table_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            let row_count = payload.result.rows.len();
                            let table_name = payload.table.clone();
                            let conn_id = payload.connection_id;
                            let editor = TableEditor::with_services(
                                payload.schema.clone(),
                                &payload.table,
                                payload.columns,
                                payload.result,
                                payload.dialect,
                                self.query_service.clone(),
                                self.metadata_service.clone(),
                                conn_id,
                                payload.database,
                                self.runtime.clone(),
                            );
                            self.table_editor = Some(editor);
                            self.state.set_status(format!(
                                "Loaded table {} ({} rows)",
                                table_name, row_count
                            ));
                        }
                        Err(e) => {
                            self.state
                                .set_status(format!("Failed to load table: {}", e.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_table_load = Some(cell);
            }
        }

        // ── Table structure load ──
        let structure_cell = self.pending_structure_load.take();
        if let Some(cell) = structure_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            let view = TableStructureView::new(
                                payload.connection_id,
                                payload.database,
                                payload.schema,
                                payload.table,
                                payload.columns,
                                payload.indexes,
                                payload.foreign_keys,
                                payload.triggers,
                                payload.functions,
                                payload.create_sql,
                            );
                            let table_name = view.table_name();
                            self.table_structure = Some(view);
                            self.state
                                .set_status(format!("Loaded structure for {}", table_name));
                        }
                        Err(e) => {
                            self.state
                                .set_status(format!("Failed to load structure: {}", e.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_structure_load = Some(cell);
            }
        }
    }

    /// Spawn an async task to load table data (columns + paginated SELECT).
    fn spawn_table_data_load(
        &mut self,
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    ) {
        let metadata_service = self.metadata_service.clone();
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();

        let cell: Arc<Mutex<Option<Result<TableDataPayload, db_common::AppError>>>> =
            Arc::new(Mutex::new(None));
        self.pending_table_load = Some(cell.clone());

        // Clone for the async task — these are used in futures AND in the payload.
        let db = database.clone();
        let sch = schema.clone();
        let tbl = table.clone();

        runtime.spawn(async move {
            // Resolve the connection profile to determine the dialect.
            let dialect: Box<dyn Dialect> =
                match connection_manager.get_profile(connection_id).await {
                    Ok(Some(profile)) => match profile.kind {
                        DataSourceKind::MySql => Box::new(BacktickDialect),
                        _ => Box::new(DoubleQuoteDialect),
                    },
                    _ => Box::new(DoubleQuoteDialect),
                };

            // Build a safe SELECT with LIMIT.
            let qualified = match &sch {
                Some(s) if !s.is_empty() => {
                    format!(
                        "{}.{}",
                        dialect.quote_identifier(&db_common::Identifier::new(s.as_str()).unwrap()),
                        dialect
                            .quote_identifier(&db_common::Identifier::new(tbl.as_str()).unwrap()),
                    )
                }
                _ => dialect.quote_identifier(&db_common::Identifier::new(tbl.as_str()).unwrap()),
            };
            let sql = format!("SELECT * FROM {} LIMIT 500", qualified);

            // Fetch columns and data in parallel.
            let columns_future = metadata_service.table_columns(
                connection_id,
                db.as_deref(),
                sch.as_deref(),
                &tbl,
                None,
            );
            let request = QueryExecutionRequest {
                tab_id: QueryId::new(),
                connection_id,
                sql,
                mode: QueryExecutionMode::All,
                cursor_offset: 0,
                selection: None,
                timeout_ms: 30000,
            };
            let query_future = query_service.execute(request, None);

            let (columns_result, query_result) = tokio::join!(columns_future, query_future);

            let result = match (columns_result, query_result) {
                (Ok(columns), Ok(report)) => {
                    let data = report
                        .results
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| QueryResult {
                            statement: String::new(),
                            columns: Vec::new(),
                            rows: Vec::new(),
                            affected_rows: 0,
                            execution_time_ms: 0,
                            notices: Vec::new(),
                            metadata: Default::default(),
                        });
                    Ok(TableDataPayload {
                        connection_id,
                        database: db,
                        schema: sch,
                        table: tbl,
                        columns,
                        result: data,
                        dialect,
                    })
                }
                (Err(e), _) => Err(e),
                (_, Err(e)) => Err(e),
            };

            *cell.lock().unwrap() = Some(result);
        });
    }

    /// Spawn an async task to load full table structure metadata.
    fn spawn_structure_load(
        &mut self,
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    ) {
        let metadata_service = self.metadata_service.clone();
        let runtime = self.runtime.clone();

        let cell: Arc<Mutex<Option<Result<TableStructurePayload, db_common::AppError>>>> =
            Arc::new(Mutex::new(None));
        self.pending_structure_load = Some(cell.clone());

        let db = database.clone();
        let sch = schema.clone();
        let tbl = table.clone();

        runtime.spawn(async move {
            let db_ref = db.as_deref();
            let sch_ref = sch.as_deref();

            let (
                columns_result,
                indexes_result,
                fks_result,
                triggers_result,
                functions_result,
                sql_result,
            ) = tokio::join!(
                metadata_service.table_columns(connection_id, db_ref, sch_ref, &tbl, None),
                metadata_service.table_indexes(connection_id, db_ref, sch_ref, &tbl, None),
                metadata_service.table_foreign_keys(connection_id, db_ref, sch_ref, &tbl, None),
                metadata_service.table_triggers(connection_id, db_ref, sch_ref, &tbl, None),
                metadata_service.schema_functions(connection_id, db_ref, sch_ref, None),
                metadata_service.get_create_table_sql(connection_id, db_ref, sch_ref, &tbl, None),
            );

            let result = match (
                columns_result,
                indexes_result,
                fks_result,
                triggers_result,
                functions_result,
                sql_result,
            ) {
                (
                    Ok(columns),
                    Ok(indexes),
                    Ok(foreign_keys),
                    Ok(triggers),
                    Ok(functions),
                    Ok(create_sql),
                ) => Ok(TableStructurePayload {
                    connection_id,
                    database: db,
                    schema: sch,
                    table: tbl,
                    columns,
                    indexes,
                    foreign_keys,
                    triggers,
                    functions,
                    create_sql,
                }),
                (Err(e), ..) => Err(e),
                (_, Err(e), ..) => Err(e),
                (_, _, Err(e), ..) => Err(e),
                (_, _, _, Err(e), ..) => Err(e),
                (_, _, _, _, Err(e), _) => Err(e),
                (_, _, _, _, _, Err(e)) => Err(e),
            };

            *cell.lock().unwrap() = Some(result);
        });
    }

    fn bottom_status(&mut self, ctx: &Context) {
        TopBottomPanel::bottom("bottom_status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Tabs: {}", self.state.tabs.len()));
                ui.separator();
                ui.label(format!("Tasks: {}", self.state.tasks.len()));
                for task in &self.state.tasks {
                    ui.separator();
                    ui.label(format!(
                        "{}: {}",
                        task_status_label(task.status),
                        task.label
                    ));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let error_count = self.state.errors.len();
                    if error_count > 0 {
                        if ui.button(format!("⚠ Errors: {}", error_count)).clicked() {
                            self.error_window_open = true;
                        }
                    } else {
                        ui.label(format!("Errors: {}", error_count));
                    }
                });
            });
        });
    }

    fn error_window(&mut self, ctx: &Context) {
        if self.state.errors.is_empty() && !self.error_window_open {
            return;
        }

        let mut open = self.error_window_open;
        Window::new("Error Details")
            .open(&mut open)
            .collapsible(true)
            .resizable(true)
            .default_width(500.0)
            .show(ctx, |ui| {
                use eframe::egui::ScrollArea;
                ScrollArea::vertical().show(ui, |ui| {
                    for error in &mut self.state.errors {
                        ui.add_space(4.0);

                        // Error header with expandable detail
                        Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("●").color(Color32::RED).strong());
                                ui.label(RichText::new(&error.title).strong());
                            });

                            ui.separator();

                            // Error message
                            ui.label(RichText::new(&error.message).color(Color32::WHITE));

                            // Expandable detail section
                            if let Some(detail) = &error.detail {
                                ui.add_space(8.0);

                                // Toggle button for detail
                                let toggle_text = if error.expanded {
                                    "▼ Hide Details"
                                } else {
                                    "▶ Show Details"
                                };
                                if ui.button(toggle_text).clicked() {
                                    error.expanded = !error.expanded;
                                }

                                // Show detail content if expanded
                                if error.expanded {
                                    ui.add_space(4.0);
                                    Frame::default().fill(Color32::from_rgb(40, 40, 40)).show(
                                        ui,
                                        |ui| {
                                            ScrollArea::horizontal().show(ui, |ui| {
                                                ui.label(
                                                    RichText::new(detail)
                                                        .monospace()
                                                        .small()
                                                        .color(Color32::GRAY),
                                                );
                                            });
                                        },
                                    );
                                }
                            }
                        });

                        ui.separator();
                    }

                    // Action buttons
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.state.errors.is_empty(), Button::new("Clear All"))
                            .clicked()
                        {
                            self.state.errors.clear();
                            self.error_window_open = false;
                        }

                        if ui.button("Copy All").clicked() {
                            let error_text: String = self
                                .state
                                .errors
                                .iter()
                                .map(|e| {
                                    let detail = e.detail.as_deref().unwrap_or("");
                                    format!("{}: {}\n{}", e.title, e.message, detail)
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n");
                            ui.output_mut(|o| o.copied_text = error_text);
                            self.state.set_status("Errors copied to clipboard");
                        }
                    });
                });
            });

        self.error_window_open = open;
        if !open && self.state.errors.is_empty() {
            self.error_window_open = false;
        }
    }

    fn shortcut_label(&self, ui: &mut eframe::egui::Ui) {
        #[cfg(target_os = "macos")]
        let modifier = "⌘";
        #[cfg(not(target_os = "macos"))]
        let modifier = "Ctrl";

        let shortcuts = [
            (format!("{}N", modifier), "New"),
            (format!("{}Enter", modifier), "Run"),
            (format!("{}⇧↵", modifier), "RunSel"),
            (format!("{}S", modifier), "Save"),
            (format!("{}R", modifier), "Refresh"),
            (format!("{}F", modifier), "Search"),
            (format!("{}L", modifier), "Focus"),
            ("Esc".to_string(), "Close"),
            (format!("{}T", modifier), "Theme"),
        ];

        for (key, label) in shortcuts {
            ui.label(
                RichText::new(format!("{} {}", key, label))
                    .small()
                    .color(Color32::GRAY),
            );
        }
    }
}

fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "Queued",
        TaskStatus::Running => "Running",
        TaskStatus::Completed => "Completed",
        TaskStatus::Failed => "Failed",
        TaskStatus::Cancelled => "Cancelled",
    }
}
