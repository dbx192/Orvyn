use super::{style::apply_app_style, ActivePanel, DesktopApp};
use eframe::egui::{
    Align, Button, CentralPanel, Context, Frame, Layout, Margin, RichText, Stroke, TopBottomPanel,
    Ui,
};
use std::time::Duration;

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.poll_pending_loads();
        if self.pending_table_load.is_some()
            || self.pending_structure_load.is_some()
            || self.pending_table_export.is_some()
            || self.pending_database_export.is_some()
            || self.pending_index_create.is_some()
            || self.pending_table_drop.is_some()
            || self.pending_clear.is_some()
            || self.pending_alter_table.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.handle_shortcuts(ctx);

        apply_app_style(ctx, self.state.theme);

        self.top_toolbar(ctx);
        self.left_tree(ctx);
        self.central_panel(ctx);
        self.connection_windows(ctx);
        self.table_export_window(ctx);
        self.import_table_window(ctx);
        self.drop_table_window(ctx);
        self.clear_window(ctx);
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
            if self.table_structure.is_some() || self.pending_structure_load.is_some() {
                self.workspace_tab(ui, ActivePanel::TableStructure, "Structure");
            }
            if self.import_wizard.is_some() {
                self.workspace_tab(ui, ActivePanel::ImportWizard, "Import");
            }
            if matches!(self.active_panel, ActivePanel::RedisBrowser) {
                let label = self
                    .redis_browser
                    .as_ref()
                    .map(|browser| browser.tab_title())
                    .unwrap_or_else(|| "Key Browser".to_string());
                self.workspace_tab(ui, ActivePanel::RedisBrowser, &label);
            }
        });
    }

    fn workspace_tab(&mut self, ui: &mut Ui, panel: ActivePanel, label: &str) {
        let selected = self.active_panel == panel;
        if ui.selectable_label(selected, label).clicked() {
            self.active_panel = panel;
            if panel == ActivePanel::TableStructure {
                self.ensure_structure_details_loaded();
            }
        }
    }

    fn ensure_structure_details_loaded(&mut self) {
        if self.pending_structure_load.is_some() {
            return;
        }
        let Some(view) = self.table_structure.as_ref() else {
            return;
        };
        if !view.needs_detail_load() {
            return;
        }
        let (connection_id, database, schema, table) = view.load_target();
        self.state
            .set_status(format!("Loading structure details for {table} ..."));
        self.spawn_structure_load(connection_id, database, schema, table);
    }

    pub(super) fn central_panel(&mut self, ctx: &Context) {
        CentralPanel::default()
            .frame(
                Frame::default()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(Margin::same(0.0)),
            )
            .show(ctx, |ui| {
                if matches!(self.active_panel, ActivePanel::RedisBrowser)
                    && self.redis_browser.is_none()
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
                                if matches!(
                                    message.as_str(),
                                    "Favorite saved" | "Favorite removed"
                                ) && self.state.status != message
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
                            let create_index = view.take_create_index_request();
                            let alter_request = view.take_alter_request();
                            if let Some(msg) = message {
                                if !msg.is_empty() {
                                    self.state.set_status(msg);
                                }
                            }
                            if let Some(request) = create_index {
                                self.spawn_index_create(request);
                            }
                            if let Some(request) = alter_request {
                                self.spawn_alter_table(request);
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

    pub(super) fn top_toolbar(&mut self, ctx: &Context) {
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
                            crate::ui::state::AppTheme::Light => "Dark",
                            crate::ui::state::AppTheme::Dark => "Light",
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
}
