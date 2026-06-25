use crate::ui::{
    connection_manager::ConnectionManagerUi,
    object_tree::ObjectTree,
    query_editor::QueryEditor,
    result_grid::ResultSetGrid,
    samples::{sample_result_set, sample_table_columns},
    state::{AppState, TaskStatus, UiError, WorkspaceTabKind},
    table_editor::TableEditor,
};
use app_core::{
    connection::ConnectionManager, metadata::MetadataService, query::QueryService,
    session::SessionManager,
};
use db_common::dialect::BacktickDialect;
use eframe::egui::{
    Align, Button, CentralPanel, Context, Layout, RichText, ScrollArea, SidePanel, TopBottomPanel,
    Ui, Window,
};
use tokio::runtime::Handle;

pub struct DesktopApp {
    app_name: String,
    connection_manager_ui: ConnectionManagerUi,
    object_tree: ObjectTree,
    query_editor: QueryEditor,
    _metadata_service: MetadataService,
    _session_manager: SessionManager,
    state: AppState,
    result_grid: ResultSetGrid,
    table_editor: TableEditor,
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
        Self {
            app_name,
            connection_manager_ui: ConnectionManagerUi::new(connection_manager.clone(), runtime.clone()),
            object_tree: ObjectTree::new(metadata_service.clone(), session_manager.clone(), runtime.clone()),
            query_editor: QueryEditor::new(query_service, connection_manager, runtime),
            _metadata_service: metadata_service,
            _session_manager: session_manager,
            state: AppState::with_sample_workspace(),
            result_grid: ResultSetGrid::new(sample_result_set()),
            table_editor: TableEditor::new(
                "main",
                "customers",
                sample_table_columns(),
                sample_result_set(),
                Box::new(BacktickDialect),
            ),
        }
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.top_toolbar(ctx);
        self.left_tree(ctx);
        self.bottom_status(ctx);
        self.error_window(ctx);

        if let Some(message) = self.query_editor.ui(ctx) {
            self.state.set_status(message);
        }
    }
}

impl DesktopApp {
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
            .default_width(260.0)
            .width_range(200.0..=420.0)
            .show(ctx, |ui| {
                if let Some(message) = self.object_tree.ui(ui) {
                    self.state.set_status(message);
                }
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