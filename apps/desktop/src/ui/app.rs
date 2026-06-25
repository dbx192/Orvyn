use crate::ui::{
    connection_manager::ConnectionManagerUi,
    object_tree::{ObjectTree, ObjectTreeAction},
    query_editor::QueryEditor,
    result_grid::ResultSetGrid,
    samples::{sample_result_set, sample_table_columns},
    state::{AppState, TaskStatus, UiError},
    table_editor::TableEditor,
};
use app_core::{
    connection::ConnectionManager, metadata::MetadataService, query::QueryService,
    session::SessionManager,
};
use db_common::dialect::BacktickDialect;
use eframe::egui::{
    Align, Button, CentralPanel, Color32, Context, Layout, RichText, SidePanel, TopBottomPanel, Window,
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
        }
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
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
            // Update the result grid if the latest result is different
            // from what it currently shows.
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
                if ui.button("Show Error").clicked() {
                    self.state.push_error(UiError::new(
                        "Example error",
                        "Errors from app-core tasks will appear here.",
                    ));
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                // Always show last tree status hint at the bottom of the panel.
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
                    ui.label(format!("Errors: {}", self.state.errors.len()));
                });
            });
        });
    }

    fn error_window(&mut self, ctx: &Context) {
        if self.state.errors.is_empty() {
            return;
        }

        let mut open = true;
        Window::new("Errors")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                for error in &self.state.errors {
                    ui.label(RichText::new(&error.title).strong());
                    ui.label(&error.message);
                    if let Some(detail) = &error.detail {
                        ui.label(detail);
                    }
                    ui.separator();
                }
                if ui.add(Button::new("Clear")).clicked() {
                    self.state.errors.clear();
                }
            });

        if !open {
            self.state.errors.clear();
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
