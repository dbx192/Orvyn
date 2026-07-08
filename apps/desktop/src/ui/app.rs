#![allow(clippy::type_complexity)]

use crate::ui::{
    connection_manager::{ConnectionChange, ConnectionManagerUi},
    import_wizard::ImportWizard,
    object_tree::{ObjectTree, ObjectTreeAction},
    query_editor::QueryEditor,
    redis_browser::RedisBrowserUi,
    shortcuts::{matches_shortcut, AppShortcut, Shortcut},
    state::{AppState, AppTheme, TaskState, TaskStatus},
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
    AppError, ConnectionId, DataSourceKind, Dialect, QueryExecutionMode, QueryExecutionRequest,
    QueryId, QueryResult, TableColumn, TableForeignKey, TableIndex, TableTrigger,
};
use eframe::egui::{
    vec2, Align, Button, CentralPanel, Color32, Context, Frame, Key, Layout, Margin, Modifiers,
    RichText, Rounding, SidePanel, Stroke, TopBottomPanel, Ui, Visuals, Window,
};
use import_export::{export_query_result_to_file, ExportFormat};
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
    redis_browser: Option<RedisBrowserUi>,
    last_tree_status: String,
    error_window_open: bool,
    active_panel: ActivePanel,
    /// Pending async load for table data (Arc<Mutex> polled each frame).
    pending_table_load: Option<Arc<Mutex<Option<Result<TableDataPayload, db_common::AppError>>>>>,
    /// Pending async load for table structure.
    pending_structure_load:
        Option<Arc<Mutex<Option<Result<TableStructurePayload, db_common::AppError>>>>>,
    /// Pending async export query load and file write.
    pending_table_export:
        Option<Arc<Mutex<Option<Result<TableExportPayload, db_common::AppError>>>>>,
    export_request: Option<TableExportRequest>,
    // Cloned service handles for spawning async loads.
    connection_manager: ConnectionManager,
    metadata_service: MetadataService,
    query_service: QueryService,
    redis_service: RedisService,
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

struct TableExportRequest {
    connection_id: ConnectionId,
    database: Option<String>,
    schema: Option<String>,
    table: String,
    format: ExportFormat,
}

struct TableExportPayload {
    table_name: String,
    path: std::path::PathBuf,
    format: ExportFormat,
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
        let rs = redis_service.clone();
        let rt_cm_ui = runtime.clone();
        let rt_field = runtime.clone();
        Self {
            app_name,
            connection_manager_ui: ConnectionManagerUi::new(connection_manager.clone(), rt_cm_ui),
            object_tree: ObjectTree::new(
                metadata_service.clone(),
                query_service.clone(),
                session_manager,
                runtime.clone(),
            ),
            query_editor: QueryEditor::new(
                query_service,
                connection_manager.clone(),
                metadata_service.clone(),
                runtime,
            ),
            state: AppState::default(),
            table_editor: None,
            table_structure: None,
            import_wizard: None,
            redis_browser: None,
            last_tree_status: String::new(),
            error_window_open: false,
            active_panel: ActivePanel::QueryEditor,
            pending_table_load: None,
            pending_structure_load: None,
            pending_table_export: None,
            export_request: None,
            connection_manager: cm,
            metadata_service: ms,
            query_service: qs,
            redis_service: rs,
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
        ) && !self.query_editor.is_executing()
        {
            self.query_editor.execute_current_statement();
            self.state
                .set_status("Executing current statement (Ctrl+Enter)");
        }

        // Ctrl/Cmd+Shift+Enter: Execute selected SQL
        let shift_enter = Shortcut {
            modifiers: platform_modifiers(),
            key: Key::Enter,
        };
        if input.modifiers.shift
            && matches_shortcut(&input, &shift_enter)
            && !self.query_editor.is_executing()
        {
            self.query_editor.execute_selection();
            self.state
                .set_status("Executing selection (Ctrl+Shift+Enter)");
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
        if input.modifiers.shift
            && matches_shortcut(&input, &shift_f)
            && self.query_editor.format_current_sql().is_some()
        {
            self.state.set_status("SQL formatted (Ctrl+Shift+F)");
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

fn apply_app_style(ctx: &Context, theme: AppTheme) {
    let mut visuals = match theme {
        AppTheme::Light => Visuals::light(),
        AppTheme::Dark => Visuals::dark(),
    };

    match theme {
        AppTheme::Light => {
            visuals.panel_fill = Color32::from_rgb(246, 247, 249);
            visuals.extreme_bg_color = Color32::from_rgb(238, 240, 244);
            visuals.faint_bg_color = Color32::from_rgb(232, 236, 241);
            visuals.widgets.noninteractive.bg_stroke =
                Stroke::new(1.0, Color32::from_rgb(210, 216, 224));
            visuals.selection.bg_fill = Color32::from_rgb(45, 103, 168);
        }
        AppTheme::Dark => {
            visuals.panel_fill = Color32::from_rgb(20, 23, 28);
            visuals.extreme_bg_color = Color32::from_rgb(13, 15, 19);
            visuals.faint_bg_color = Color32::from_rgb(31, 36, 43);
            visuals.widgets.noninteractive.bg_stroke =
                Stroke::new(1.0, Color32::from_rgb(50, 58, 68));
            visuals.selection.bg_fill = Color32::from_rgb(48, 108, 172);
        }
    }

    visuals.window_rounding = Rounding::same(6.0);
    visuals.menu_rounding = Rounding::same(6.0);
    visuals.widgets.noninteractive.rounding = Rounding::same(4.0);
    visuals.widgets.inactive.rounding = Rounding::same(4.0);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);
    visuals.widgets.active.rounding = Rounding::same(4.0);
    visuals.widgets.open.rounding = Rounding::same(4.0);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(8.0, 3.0);
    style.spacing.interact_size = vec2(44.0, 24.0);
    style.spacing.window_margin = Margin::symmetric(10.0, 8.0);
    ctx.set_style(style);
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Poll any pending async data loads first
        self.poll_pending_loads();

        // Handle keyboard shortcuts
        self.handle_shortcuts(ctx);

        apply_app_style(ctx, self.state.theme);

        self.top_toolbar(ctx);
        self.left_tree(ctx);
        self.central_panel(ctx);
        self.connection_windows(ctx);
        self.table_export_window(ctx);
        self.bottom_status(ctx);
        self.error_window(ctx);
    }
}

impl DesktopApp {
    fn workspace_tabs(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            self.workspace_tab(ui, ActivePanel::QueryEditor, "Query");
            if self.table_editor.is_some() {
                self.workspace_tab(ui, ActivePanel::TableEditor, "Data");
            }
            if self.table_structure.is_some() {
                self.workspace_tab(ui, ActivePanel::TableStructure, "Structure");
            }
            if self.import_wizard.is_some() {
                self.workspace_tab(ui, ActivePanel::ImportWizard, "Import");
            }
            if self.redis_browser.is_some() {
                self.workspace_tab(ui, ActivePanel::RedisBrowser, "Redis");
            }
        });
    }

    fn workspace_tab(&mut self, ui: &mut Ui, panel: ActivePanel, label: &str) {
        let selected = self.active_panel == panel;
        if ui.selectable_label(selected, label).clicked() {
            self.active_panel = panel;
        }
    }

    fn central_panel(&mut self, ctx: &Context) {
        CentralPanel::default()
            .frame(
                Frame::default()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(Margin::symmetric(6.0, 4.0)),
            )
            .show(ctx, |ui| {
            if matches!(self.active_panel, ActivePanel::RedisBrowser) && self.redis_browser.is_none()
            {
                self.active_panel = ActivePanel::QueryEditor;
            }

            self.workspace_tabs(ui);
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(2.0);

            match self.active_panel {
                ActivePanel::QueryEditor => {
                    let query_status = self.query_editor.ui(ctx, ui);
                    if let Some(message) = query_status {
                        if !message.is_empty() {
                            if matches!(message.as_str(), "Favorite saved" | "Favorite removed")
                                && self.state.status != message
                            {
                                self.object_tree.reload_favorites();
                            }
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
                            ui.label(RichText::new("Table Data").size(24.0).strong());
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
                            ui.label(RichText::new("Table Structure").size(24.0).strong());
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
                    if let Some(redis_browser) = &mut self.redis_browser {
                        redis_browser.ui(ctx, ui);
                    } else {
                        self.active_panel = ActivePanel::QueryEditor;
                    }
                }
            }
            });
    }

    fn top_toolbar(&mut self, ctx: &Context) {
        let stroke = ctx.style().visuals.widgets.noninteractive.bg_stroke;
        TopBottomPanel::top("top_toolbar")
            .frame(
                Frame::default()
                    .fill(ctx.style().visuals.panel_fill)
                    .stroke(Stroke::new(1.0, stroke.color))
                    .inner_margin(Margin::symmetric(10.0, 6.0)),
            )
            .show(ctx, |ui| {
                let compact = ui.available_width() < 920.0;
                ui.horizontal(|ui| {
                    ui.heading(RichText::new(&self.app_name).size(18.0));
                    ui.separator();

                    let connection_label = if compact { "Connect" } else { "New Connection" };
                    if ui
                        .add_sized([112.0, 28.0], Button::new(connection_label))
                        .on_hover_text("Create or edit saved database connections")
                        .clicked()
                    {
                        self.connection_manager_ui.new_connection();
                        self.state.set_status("New connection editor opened");
                    }
                    if ui
                        .add_sized([92.0, 28.0], Button::new("New Query"))
                        .on_hover_text("Open a new SQL editor tab")
                        .clicked()
                    {
                        self.query_editor.new_query();
                        self.state.set_status("New query tab opened");
                    }
                    if ui
                        .add_sized([72.0, 28.0], Button::new("History"))
                        .on_hover_text("Show executed SQL history")
                        .clicked()
                    {
                        self.query_editor.show_history();
                        self.active_panel = ActivePanel::QueryEditor;
                        self.state.set_status("History opened");
                    }
                    if ui
                        .add_sized([82.0, 28.0], Button::new("Favorites"))
                        .on_hover_text("Show saved SQL snippets")
                        .clicked()
                    {
                        self.query_editor.show_favorites();
                        self.active_panel = ActivePanel::QueryEditor;
                        self.state.set_status("Favorites opened");
                    }
                    if ui
                        .add_sized([76.0, 28.0], Button::new("Refresh"))
                        .on_hover_text("Refresh the active browser or workspace")
                        .clicked()
                    {
                        self.state.set_status("Refresh requested");
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let theme_label = match self.state.theme {
                            AppTheme::Light => "Dark",
                            AppTheme::Dark => "Light",
                        };
                        if ui
                            .add_sized([58.0, 28.0], Button::new(theme_label))
                            .on_hover_text("Toggle theme")
                            .clicked()
                        {
                            self.state.toggle_theme();
                        }

                        let error_count = self.state.errors.len();
                        if error_count > 0
                            && ui
                                .add_sized(
                                    [86.0, 28.0],
                                    Button::new(format!("Errors {error_count}")),
                                )
                                .on_hover_text("Open error details")
                                .clicked()
                        {
                            self.error_window_open = true;
                        }

                        ui.separator();
                        ui.label(RichText::new(&self.state.status).small().strong())
                            .on_hover_text(&self.state.status);
                    });
                });
            });
    }

    fn left_tree(&mut self, ctx: &Context) {
        SidePanel::left("connection_object_tree")
            .resizable(true)
            .default_width(250.0)
            .width_range(200.0..=420.0)
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
                        let status = compact_status_text(&self.last_tree_status, 72);
                        ui.label(RichText::new(status).small().color(Color32::GRAY))
                            .on_hover_text(&self.last_tree_status);
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
                ObjectTreeAction::OpenSavedQuery {
                    connection_id,
                    database,
                    schema,
                    title,
                    sql,
                } => {
                    let status_title = title.clone();
                    self.query_editor
                        .open_saved_query(connection_id, database, schema, title, sql);
                    self.active_panel = ActivePanel::QueryEditor;
                    self.state
                        .set_status(format!("Opened saved query: {status_title}"));
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
                    self.ensure_redis_browser(connection_id);
                    if let Some(redis_browser) = &mut self.redis_browser {
                        redis_browser.load_key(connection_id, database_index, key.clone());
                    }
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
                    let table_name =
                        qualified_table_label(schema.as_deref().or(database.as_deref()), &table);
                    self.export_request = Some(TableExportRequest {
                        connection_id,
                        database,
                        schema,
                        table,
                        format: ExportFormat::Csv,
                    });
                    self.state
                        .set_status(format!("Export table: {}", table_name));
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
                    self.ensure_redis_browser(connection_id);
                    if let Some(redis_browser) = &mut self.redis_browser {
                        redis_browser.prompt_delete_key(connection_id, database_index, key.clone());
                    }
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
        for change in self.connection_manager_ui.take_changes() {
            self.handle_connection_change(change);
        }
    }

    fn handle_connection_change(&mut self, change: ConnectionChange) {
        self.object_tree.reload_root();
        match change {
            ConnectionChange::Saved(_) => {
                self.state
                    .set_status("Connection saved; object tree refreshed");
            }
            ConnectionChange::Deleted(id) => {
                if self
                    .redis_browser
                    .as_ref()
                    .is_some_and(|browser| browser.connection_id() == id)
                {
                    self.redis_browser = None;
                    if matches!(self.active_panel, ActivePanel::RedisBrowser) {
                        self.active_panel = ActivePanel::QueryEditor;
                    }
                }
                self.state
                    .set_status("Connection deleted; object tree refreshed");
            }
        }
    }

    fn ensure_redis_browser(&mut self, connection_id: ConnectionId) {
        let needs_new = self
            .redis_browser
            .as_ref()
            .map(|browser| browser.connection_id() != connection_id)
            .unwrap_or(true);

        if needs_new {
            self.redis_browser = Some(RedisBrowserUi::new(
                connection_id,
                self.connection_manager.clone(),
                self.metadata_service.clone(),
                self.redis_service.clone(),
                self.runtime.clone(),
            ));
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

        // ── Table export ──
        let export_cell = self.pending_table_export.take();
        if let Some(cell) = export_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            let format_label = match payload.format {
                                ExportFormat::Csv => "CSV",
                                ExportFormat::Json => "JSON",
                                ExportFormat::ExcelCompatible => "Excel",
                            };
                            self.state.set_status(format!(
                                "{} export completed: {} -> {}",
                                format_label,
                                payload.table_name,
                                payload.path.display()
                            ));
                        }
                        Err(error) => {
                            self.state
                                .set_status(format!("Export failed: {}", error.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_table_export = Some(cell);
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

    fn table_export_window(&mut self, ctx: &Context) {
        let Some(request) = self.export_request.as_mut() else {
            return;
        };

        let mut open = true;
        let table_name = qualified_table_label(
            request
                .schema
                .as_deref()
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    request
                        .database
                        .as_deref()
                        .filter(|value| !value.is_empty())
                }),
            &request.table,
        );
        let table_file_name = request.table.clone();
        let mut submit_export = None;
        let mut cancel_export = false;

        Window::new("Export Table Data")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Table: {}", table_name));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Format");
                    eframe::egui::ComboBox::from_id_source("table_export_format")
                        .selected_text(match request.format {
                            ExportFormat::Csv => "CSV",
                            ExportFormat::Json => "JSON",
                            ExportFormat::ExcelCompatible => "Excel",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut request.format, ExportFormat::Csv, "CSV");
                            ui.selectable_value(&mut request.format, ExportFormat::Json, "JSON");
                        });
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Export runs in the background and writes directly to file.")
                        .small()
                        .color(Color32::GRAY),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel_export = true;
                    }
                    if ui.button("Export").clicked() {
                        let format = request.format;
                        let ext = match format {
                            ExportFormat::Csv => "csv",
                            ExportFormat::Json => "json",
                            ExportFormat::ExcelCompatible => "tsv",
                        };
                        let path = rfd::FileDialog::new()
                            .set_title("Export Table Data")
                            .set_file_name(format!("{}.{}", table_file_name, ext))
                            .add_filter(
                                match format {
                                    ExportFormat::Csv => "CSV",
                                    ExportFormat::Json => "JSON",
                                    ExportFormat::ExcelCompatible => "Excel",
                                },
                                &[ext],
                            )
                            .save_file();

                        if let Some(path) = path {
                            submit_export = Some(path);
                        }
                    }
                });
            });

        if let Some(path) = submit_export {
            if let Some(export_request) = self.export_request.take() {
                self.spawn_table_export(export_request, path);
            }
        }

        if cancel_export {
            self.export_request = None;
        }

        if !open {
            self.export_request = None;
        }
    }

    fn spawn_table_export(&mut self, request: TableExportRequest, path: std::path::PathBuf) {
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();
        let cell: Arc<Mutex<Option<Result<TableExportPayload, AppError>>>> =
            Arc::new(Mutex::new(None));
        self.pending_table_export = Some(cell.clone());

        runtime.spawn(async move {
            let profile_kind = connection_manager
                .get_profile(request.connection_id)
                .await
                .ok()
                .flatten()
                .map(|profile| profile.kind);
            let dialect: Box<dyn Dialect> = match profile_kind {
                Some(DataSourceKind::MySql) => Box::new(BacktickDialect),
                _ => Box::new(DoubleQuoteDialect),
            };

            let qualifier = request
                .schema
                .as_deref()
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    matches!(profile_kind, Some(DataSourceKind::MySql))
                        .then_some(request.database.as_deref())
                        .flatten()
                        .filter(|value| !value.is_empty())
                });
            let qualified = match qualifier {
                Some(schema) => format!(
                    "{}.{}",
                    dialect.quote_identifier(&db_common::Identifier::new(schema).unwrap()),
                    dialect.quote_identifier(&db_common::Identifier::new(&request.table).unwrap()),
                ),
                None => {
                    dialect.quote_identifier(&db_common::Identifier::new(&request.table).unwrap())
                }
            };
            let sql = format!("SELECT * FROM {}", qualified);
            let execute_result = query_service
                .execute(
                    QueryExecutionRequest {
                        tab_id: QueryId::new(),
                        connection_id: request.connection_id,
                        sql,
                        mode: QueryExecutionMode::All,
                        cursor_offset: 0,
                        selection: None,
                        timeout_ms: 60_000,
                    },
                    None,
                )
                .await;

            let result = match execute_result {
                Ok(report) => {
                    let result = report
                        .results
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| QueryResult::empty(""));
                    let path_for_write = path.clone();
                    let format = request.format;
                    let table_name = qualified_table_label(qualifier, &request.table);
                    let write_result = tokio::task::spawn_blocking(move || {
                        export_query_result_to_file(&result, format, &path_for_write).map(|_| {
                            TableExportPayload {
                                table_name,
                                path,
                                format,
                            }
                        })
                    })
                    .await;

                    match write_result {
                        Ok(payload) => payload,
                        Err(error) => Err(AppError::internal(error.to_string())),
                    }
                }
                Err(error) => Err(error),
            };

            *cell.lock().unwrap() = Some(result);
        });

        self.state.set_status("Table export started");
    }

    fn bottom_status(&mut self, ctx: &Context) {
        TopBottomPanel::bottom("bottom_status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.state.status).small())
                    .on_hover_text(&self.state.status);

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let error_count = self.state.errors.len();
                    if error_count > 0 {
                        if ui.button(format!("Errors: {}", error_count)).clicked() {
                            self.error_window_open = true;
                        }
                    } else {
                        ui.label(format!("Errors: {}", error_count));
                    }
                    ui.separator();
                    ui.label(format!(
                        "Workspace: {}",
                        active_panel_label(self.active_panel)
                    ));
                    ui.separator();
                    ui.label(task_summary(&self.state.tasks));
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

fn active_panel_label(panel: ActivePanel) -> &'static str {
    match panel {
        ActivePanel::QueryEditor => "Query",
        ActivePanel::TableEditor => "Table Data",
        ActivePanel::TableStructure => "Structure",
        ActivePanel::ImportWizard => "Import",
        ActivePanel::RedisBrowser => "Redis",
    }
}

fn task_summary(tasks: &[TaskState]) -> String {
    if let Some(task) = tasks
        .iter()
        .find(|task| matches!(task.status, TaskStatus::Queued | TaskStatus::Running))
    {
        return format!("{}: {}", task_status_label(task.status), task.label);
    }

    let failed = tasks
        .iter()
        .filter(|task| matches!(task.status, TaskStatus::Failed))
        .count();
    if failed > 0 {
        return format!("Tasks: {} failed", failed);
    }

    format!("Tasks: {}", tasks.len())
}

fn compact_status_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut text = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    text.push_str("...");
    text
}

fn qualified_table_label(qualifier: Option<&str>, table: &str) -> String {
    match qualifier.filter(|value| !value.is_empty()) {
        Some(qualifier) => format!("{qualifier}.{table}"),
        None => table.to_string(),
    }
}
