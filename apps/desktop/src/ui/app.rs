use crate::ui::{
    result_grid::ResultSetGrid,
    samples::{sample_result_set, sample_table_columns},
    table_editor::TableEditor,
};
use app_core::{connection::ConnectionManager, query::QueryService};
use db_common::dialect::BacktickDialect;
use eframe::egui::{Align, CentralPanel, Context, Layout, TopBottomPanel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceView {
    QueryResult,
    TableData,
}

pub struct DesktopApp {
    app_name: String,
    _connection_manager: ConnectionManager,
    _query_service: QueryService,
    current_view: WorkspaceView,
    result_grid: ResultSetGrid,
    table_editor: TableEditor,
    status: String,
}

impl DesktopApp {
    pub fn new(
        app_name: String,
        connection_manager: ConnectionManager,
        query_service: QueryService,
    ) -> Self {
        Self {
            app_name,
            _connection_manager: connection_manager,
            _query_service: query_service,
            current_view: WorkspaceView::TableData,
            result_grid: ResultSetGrid::new(sample_result_set()),
            table_editor: TableEditor::new(
                "main",
                "customers",
                sample_table_columns(),
                sample_result_set(),
                Box::new(BacktickDialect),
            ),
            status: "Ready".to_string(),
        }
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(&self.app_name);
                ui.separator();
                ui.selectable_value(
                    &mut self.current_view,
                    WorkspaceView::TableData,
                    "Table Data",
                );
                ui.selectable_value(
                    &mut self.current_view,
                    WorkspaceView::QueryResult,
                    "Query Result",
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(&self.status);
                });
            });
        });

        CentralPanel::default().show(ctx, |ui| {
            let message = match self.current_view {
                WorkspaceView::QueryResult => {
                    ui.heading("Query Result");
                    ui.add_space(6.0);
                    self.result_grid.ui(ui)
                }
                WorkspaceView::TableData => {
                    ui.heading("Table Data");
                    ui.add_space(6.0);
                    self.table_editor.ui(ui)
                }
            };

            if let Some(message) = message {
                self.status = message;
            }
        });
    }
}
