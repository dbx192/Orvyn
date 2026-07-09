#![allow(clippy::type_complexity)]

mod dialogs;
mod export_sql;
mod payloads;
mod shell;
mod style;
mod tasks;
mod tree;

use self::payloads::{
    DatabaseExportPayload, DropTablePayload, DropTableRequest, ImportTableRequest,
    IndexCreatePayload, PendingTask, TableDataPayload, TableExportPayload, TableExportRequest,
    TableStructurePayload,
};
use crate::ui::{
    connection_manager::ConnectionManagerUi,
    import_wizard::ImportWizard,
    object_tree::ObjectTree,
    query_editor::QueryEditor,
    redis_browser::RedisBrowserUi,
    shortcuts::{matches_shortcut, AppShortcut, Shortcut},
    state::{AppState, AppTheme},
    table_editor::TableEditor,
    table_structure::TableStructureView,
};
use app_core::{
    connection::ConnectionManager, metadata::MetadataService, query::QueryService,
    redis_service::RedisService, session::SessionManager,
};
use eframe::egui::{Context, Key, Modifiers};
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
    pending_table_load: Option<PendingTask<TableDataPayload>>,
    /// Pending async load for table structure.
    pending_structure_load: Option<PendingTask<TableStructurePayload>>,
    /// Pending async export query load and file write.
    pending_table_export: Option<PendingTask<TableExportPayload>>,
    pending_database_export: Option<PendingTask<DatabaseExportPayload>>,
    pending_index_create: Option<PendingTask<IndexCreatePayload>>,
    pending_table_drop: Option<PendingTask<DropTablePayload>>,
    export_request: Option<TableExportRequest>,
    import_table_request: Option<ImportTableRequest>,
    drop_table_request: Option<DropTableRequest>,
    // Cloned service handles for spawning async loads.
    connection_manager: ConnectionManager,
    metadata_service: MetadataService,
    query_service: QueryService,
    redis_service: RedisService,
    runtime: Handle,
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
            pending_database_export: None,
            pending_index_create: None,
            pending_table_drop: None,
            export_request: None,
            import_table_request: None,
            drop_table_request: None,
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
