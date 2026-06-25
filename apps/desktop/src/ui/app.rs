use crate::ui::{
    connection_manager::ConnectionManagerUi,
    object_tree::{ObjectTree, ObjectTreeAction},
    query_editor::QueryEditor,
    result_grid::ResultSetGrid,
    samples::{sample_result_set, sample_table_columns},
    shortcuts::{matches_shortcut, AppShortcut, Shortcut},
    sql_formatter::format_sql,
    state::{AppState, AppTheme, TaskStatus, UiError},
    table_editor::TableEditor,
};
use app_core::{
    connection::ConnectionManager, metadata::MetadataService, query::QueryService,
    session::SessionManager,
};
use db_common::dialect::BacktickDialect;
use eframe::egui::{
    Align, Button, CentralPanel, Color32, Context, Frame, Key, Layout, Modifiers, RichText,
    SidePanel, TopBottomPanel, Window,
};
use tokio::runtime::Handle;

pub struct DesktopApp {
    app_name: String,
    connection_manager_ui: ConnectionManagerUi,
    object_tree: ObjectTree,
    query_editor: QueryEditor,
    state: AppState,
    result_grid: ResultSetGrid,
    table_editor: TableEditor,
    last_tree_status: String,
    error_window_open: bool,
}

impl DesktopApp {
    pub fn new(
        app_name: String,
        connection_manager: ConnectionManager,
        metadata_service: MetadataService,
        session_manager: SessionManager,
        query_service: QueryService,
        runtime: Handle,
    ) -> Self {
        let result_grid_result = sample_result_set();
        let sample = result_grid_result.clone();
        Self {
            app_name,
            connection_manager_ui: ConnectionManagerUi::new(connection_manager.clone(), runtime.clone()),
            object_tree: ObjectTree::new(
                metadata_service,
                session_manager,
                runtime.clone(),
            ),
            query_editor: QueryEditor::new(query_service, connection_manager, runtime),
            state: AppState::with_sample_workspace(),
            result_grid: ResultSetGrid::new(result_grid_result),
            table_editor: TableEditor::new(
                "main",
                "customers",
                sample_table_columns(),
                sample,
                Box::new(BacktickDialect),
            ),
            last_tree_status: String::new(),
            error_window_open: false,
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
        if matches_shortcut(&input, &AppShortcut::ExecuteCurrent.with_platform_modifier()) {
            if !self.query_editor.is_executing() {
                self.query_editor.execute_current_statement();
                self.state.set_status("Executing current statement (Ctrl+Enter)");
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
                self.state.set_status("Executing selection (Ctrl+Shift+Enter)");
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
        if matches_shortcut(&input, &AppShortcut::FocusObjectSearch.with_platform_modifier()) {
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
            self.state.set_status(format!("Theme: {} (Ctrl+T)", theme_name));
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
        // Handle keyboard shortcuts first
        self.handle_shortcuts(ctx);

        // Apply theme
        ctx.set_visuals(match self.state.theme {
            AppTheme::Light => eframe::egui::Visuals::light(),
            AppTheme::Dark => eframe::egui::Visuals::dark(),
        });

        self.top_toolbar(ctx);
        self.left_tree(ctx);
        self.central_panel(ctx);
        self.bottom_status(ctx);
        self.error_window(ctx);
    }
}

impl DesktopApp {
    fn central_panel(&mut self, ctx: &Context) {
        CentralPanel::default().show(ctx, |ui| {
            let query_status = self.query_editor.ui(ctx, ui);
            ui.separator();
            self.sync_result_grid();
            let grid_status = self.result_grid.ui(ui);
            if let Some(message) = query_status.or(grid_status) {
                if !message.is_empty() {
                    self.state.set_status(message);
                }
            }
        });
    }

    fn sync_result_grid(&mut self) {
        let latest = self.query_editor.latest_result().cloned();
        if let Some(new_result) = latest {
            if self.result_grid.result() != &new_result {
                self.result_grid.set_result(new_result);
            }
        }
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
                if ui.button(theme_icon).on_hover_text("Toggle Theme (Ctrl+T)").clicked() {
                    self.state.toggle_theme();
                }

                // Show error button for demo
                if ui.button("Show Error").clicked() {
                    self.state.push_error(UiError::new(
                        "Connection Error",
                        "Failed to connect to database server.",
                    ).with_detail(
                        "Error details:\n\
                         - Host: localhost:3306\n\
                         - Error code: 1045\n\
                         - Access denied for user 'root'@'localhost'\n\
                         - This may indicate incorrect password or insufficient privileges\n\
                         \n\
                         Stack trace:\n\
                         at Connection.connect() line 45\n\
                         at SessionManager.connect() line 78\n\
                         at DesktopApp.handle_connect() line 123"
                    ));
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
                        "Open table {}.{}{} ({})",
                        schema.as_deref().unwrap_or(""),
                        database.as_deref().unwrap_or(""),
                        table,
                        connection_id
                    ));
                }
                ObjectTreeAction::OpenQuery {
                    connection_id,
                    database,
                    schema,
                } => {
                    self.query_editor
                        .new_query_with_connection(connection_id);
                    self.state.set_status(format!(
                        "New query on {} (db={:?}, schema={:?})",
                        connection_id, database, schema
                    ));
                }
                ObjectTreeAction::ViewKey {
                    connection_id,
                    database,
                    key,
                } => {
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
                        "View structure {}.{}{} ({})",
                        schema.as_deref().unwrap_or(""),
                        database.as_deref().unwrap_or(""),
                        table,
                        connection_id
                    ));
                }
                ObjectTreeAction::EditConnection(id) => {
                    self.state
                        .set_status(format!("Edit connection: {}", id));
                }
                ObjectTreeAction::DeleteConnection(id) => {
                    self.state
                        .set_status(format!("Delete connection: {}", id));
                }
            }
        }
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
                                let toggle_text = if error.expanded { "▼ Hide Details" } else { "▶ Show Details" };
                                if ui.button(toggle_text).clicked() {
                                    error.expanded = !error.expanded;
                                }

                                // Show detail content if expanded
                                if error.expanded {
                                    ui.add_space(4.0);
                                    Frame::default()
                                        .fill(Color32::from_rgb(40, 40, 40))
                                        .show(ui, |ui| {
                                            ScrollArea::horizontal().show(ui, |ui| {
                                                ui.label(
                                                    RichText::new(detail)
                                                        .monospace()
                                                        .small()
                                                        .color(Color32::GRAY),
                                                );
                                            });
                                        });
                                }
                            }
                        });

                        ui.separator();
                    }

                    // Action buttons
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.state.errors.is_empty(), Button::new("Clear All")).clicked() {
                            self.state.errors.clear();
                            self.error_window_open = false;
                        }

                        if ui.button("Copy All").clicked() {
                            let error_text: String = self.state.errors.iter()
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
            ("Esc", "Close"),
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
