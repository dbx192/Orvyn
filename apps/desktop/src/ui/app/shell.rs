use super::{style::apply_app_style, ActivePanel, DesktopApp};
use crate::ui::i18n::{self, text as tr, Locale};
use eframe::egui::{
    vec2, Align, Button, CentralPanel, Context, Frame, Layout, Margin, RichText, Spinner, Stroke,
    TopBottomPanel, Ui,
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
            || self
                .search_browser
                .as_ref()
                .is_some_and(|browser| browser.is_busy())
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
        self.table_tab_order
            .retain(|id| self.table_editors.contains_key(id));
        for id in self.table_editors.keys() {
            if !self.table_tab_order.contains(id) {
                self.table_tab_order.push(*id);
            }
        }

        ui.horizontal_wrapped(|ui| {
            let query_selected = self.active_panel == ActivePanel::QueryEditor;
            if ui.selectable_label(query_selected, tr("Query")).clicked() {
                if !query_selected {
                    if let Some((connection_id, database, schema, table)) = self
                        .active_table_editor
                        .and_then(|id| self.table_editors.get(&id))
                        .and_then(|editor| editor.query_context())
                    {
                        self.query_editor.new_query_with_connection(
                            connection_id,
                            database,
                            schema,
                            Some(table),
                        );
                    }
                }
                self.active_panel = ActivePanel::QueryEditor;
            }
            if self.active_structure.is_some() || self.pending_structure_load.is_some() {
                self.workspace_tab(ui, ActivePanel::TableStructure, &tr("Structure"));
            }
            let tabs = self
                .table_tab_order
                .iter()
                .filter_map(|id| {
                    self.table_editors
                        .get(id)
                        .map(|editor| (*id, editor.table_name(), editor.has_unsaved_changes()))
                })
                .collect::<Vec<_>>();
            let dirty_tabs = tabs
                .iter()
                .filter_map(|(id, _, dirty)| dirty.then_some(*id))
                .collect::<Vec<_>>();
            let ordered_ids = tabs.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
            let mut close_tabs = None;
            for (id, title, dirty) in tabs {
                let selected = self.active_panel == ActivePanel::TableEditor
                    && self.active_table_editor == Some(id);
                let tab_response = ui.selectable_label(selected, title);
                if tab_response.clicked() {
                    self.active_panel = ActivePanel::TableEditor;
                    self.active_table_editor = Some(id);
                    self.active_structure = Some(id);
                }
                let tab_index = ordered_ids
                    .iter()
                    .position(|tab_id| *tab_id == id)
                    .unwrap_or(0);
                tab_response.context_menu(|ui| {
                    let left = ordered_ids[..tab_index].to_vec();
                    let right = ordered_ids[tab_index + 1..].to_vec();
                    let others = ordered_ids
                        .iter()
                        .copied()
                        .filter(|tab_id| *tab_id != id)
                        .collect::<Vec<_>>();

                    close_menu_item(ui, &tr("Close"), vec![id], &dirty_tabs, &mut close_tabs);
                    close_menu_item(
                        ui,
                        &tr("Close Other Tabs"),
                        others,
                        &dirty_tabs,
                        &mut close_tabs,
                    );
                    ui.separator();
                    close_menu_item(
                        ui,
                        &tr("Close Tabs to the Left"),
                        left,
                        &dirty_tabs,
                        &mut close_tabs,
                    );
                    close_menu_item(
                        ui,
                        &tr("Close Tabs to the Right"),
                        right,
                        &dirty_tabs,
                        &mut close_tabs,
                    );
                    ui.separator();
                    close_menu_item(
                        ui,
                        &tr("Close All Tabs"),
                        ordered_ids.clone(),
                        &dirty_tabs,
                        &mut close_tabs,
                    );
                });
                if ui
                    .add_enabled(!dirty, Button::new("x"))
                    .on_hover_text(if dirty {
                        tr("Save or rollback changes before closing")
                    } else {
                        tr("Close table")
                    })
                    .clicked()
                {
                    close_tabs = Some(vec![id]);
                }
            }
            if let Some(ids) = close_tabs {
                self.close_table_tabs(&ids);
            }
            if self.import_wizard.is_some() {
                self.workspace_tab(ui, ActivePanel::ImportWizard, &tr("Import"));
            }
            if matches!(self.active_panel, ActivePanel::RedisBrowser) {
                let label = self
                    .redis_browser
                    .as_ref()
                    .map(|browser| browser.tab_title())
                    .unwrap_or_else(|| "Key Browser".to_string());
                self.workspace_tab(ui, ActivePanel::RedisBrowser, &label);
            }
            if matches!(self.active_panel, ActivePanel::SearchBrowser) {
                let label = self
                    .search_browser
                    .as_ref()
                    .map(|browser| browser.title())
                    .unwrap_or_else(|| "Browse".to_string());
                self.workspace_tab(ui, ActivePanel::SearchBrowser, &label);
            }
        });
    }

    fn close_table_tabs(&mut self, ids: &[eframe::egui::Id]) {
        let fallback_index = ids
            .iter()
            .filter_map(|id| self.table_tab_order.iter().position(|tab_id| tab_id == id))
            .min()
            .unwrap_or_default();
        for id in ids {
            self.table_editors.remove(id);
            self.table_structures.remove(id);
        }
        self.table_tab_order.retain(|id| !ids.contains(id));

        if self
            .active_table_editor
            .is_some_and(|active| ids.contains(&active))
        {
            self.active_table_editor = self
                .table_tab_order
                .get(fallback_index.min(self.table_tab_order.len().saturating_sub(1)))
                .copied();
            self.active_structure = self.active_table_editor;
            self.active_panel = if self.active_table_editor.is_some() {
                ActivePanel::TableEditor
            } else {
                ActivePanel::QueryEditor
            };
        } else if self
            .active_structure
            .is_some_and(|active| ids.contains(&active))
        {
            self.active_structure = self.active_table_editor;
        }
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
        let Some(view) = self
            .active_structure
            .and_then(|id| self.table_structures.get(&id))
        else {
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
                if matches!(self.active_panel, ActivePanel::SearchBrowser)
                    && self.search_browser.is_none()
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
                    ActivePanel::TableEditor => {
                        if self.pending_table_load.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(72.0);
                                ui.add(Spinner::new().size(28.0));
                                ui.add_space(14.0);
                                ui.label(RichText::new(tr("Loading table data..."))
                                    .size(18.0)
                                    .strong());
                                ui.add_space(6.0);
                                ui.label(RichText::new(tr("Fetching the first rows and table columns"))
                                    .small()
                                    .color(eframe::egui::Color32::GRAY));
                            });
                        } else {
                            match self
                                .active_table_editor
                                .and_then(|id| self.table_editors.get_mut(&id))
                            {
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
                                        ui.label(RichText::new(tr("Table Data")).size(24.0).strong());
                                        ui.add_space(12.0);
                                        ui.label(tr("No table open. Double-click a table in the object tree to load data."));
                                        ui.add_space(8.0);
                                        if ui.button(tr("Switch to Query Editor")).clicked() {
                                            self.active_panel = ActivePanel::QueryEditor;
                                        }
                                    });
                                }
                            }
                        }
                    }
                    ActivePanel::TableStructure => match self
                        .active_structure
                        .and_then(|id| self.table_structures.get_mut(&id)) {
                        Some(view) => {
                            let message = view.ui(ui);
                            let create_index = view.take_create_index_request();
                            let alter_requests = view.take_alter_requests();
                            if let Some(msg) = message {
                                if !msg.is_empty() {
                                    self.state.set_status(msg);
                                }
                            }
                            if let Some(request) = create_index {
                                self.spawn_index_create(request);
                            }
                            if !alter_requests.is_empty() {
                                self.alter_queue.extend(alter_requests);
                                self.pump_alter_queue();
                            }
                        }
                        None => {
                            ui.vertical_centered(|ui| {
                                ui.add_space(60.0);
                                ui.label(RichText::new(tr("Table Structure")).size(24.0).strong());
                                ui.add_space(12.0);
                                ui.label(tr("Loading table structure..."));
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
                                ui.label(tr("No import wizard active."));
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
                    ActivePanel::SearchBrowser => {
                        if let Some(browser) = &mut self.search_browser {
                            let status = browser.ui(ctx, ui);
                            if let Some(message) = status {
                                if !message.is_empty() {
                                    self.state.set_status(message);
                                }
                            }
                        } else {
                            self.active_panel = ActivePanel::QueryEditor;
                        }
                    }
                }
            });
    }

    pub(super) fn top_toolbar(&mut self, ctx: &Context) {
        use super::style::current_palette;
        use eframe::egui::{Align2, FontId, Rounding, Sense};
        let palette = current_palette(ctx);
        TopBottomPanel::top("top_toolbar")
            .frame(
                Frame::default()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.border))
                    .inner_margin(Margin::symmetric(12.0, 8.0)),
            )
            .show(ctx, |ui| {
                let compact = ui.available_width() < 960.0;
                ui.horizontal(|ui| {
                    // ---- Brand ----
                    let (brand_rect, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                    ui.painter()
                        .rect_filled(brand_rect, Rounding::same(6.0), palette.info);
                    ui.painter().text(
                        brand_rect.center(),
                        Align2::CENTER_CENTER,
                        "O",
                        FontId::proportional(15.0),
                        palette.accent_fg,
                    );
                    ui.label(RichText::new("Orvyn").strong().size(15.0));
                    if !compact {
                        ui.label(
                            RichText::new(tr("Multi-database workbench"))
                                .small()
                                .color(palette.fg_muted),
                        );
                    }
                    ui.separator();

                    // ---- Connection group ----
                    let connection_label = if compact {
                        tr("Connect")
                    } else {
                        tr("Connections")
                    };
                    let mut connection_actions = Vec::new();
                    ui.menu_button(RichText::new(connection_label).strong(), |ui| {
                        if ui.button(tr("New")).clicked() {
                            self.connection_manager_ui.new_connection();
                            self.state.set_status(tr("New connection editor opened"));
                            ui.close_menu();
                        }
                        ui.separator();
                        self.object_tree
                            .connection_transfer_menu(ui, &mut connection_actions);
                    });
                    if !connection_actions.is_empty() {
                        self.handle_tree_actions(connection_actions);
                    }
                    if ui
                        .add(Button::new(tr("History")))
                        .on_hover_text(tr("Show executed SQL history"))
                        .clicked()
                    {
                        self.query_editor.show_history();
                        self.active_panel = ActivePanel::QueryEditor;
                        self.state.set_status(tr("History opened"));
                    }
                    if ui
                        .add(Button::new(tr("Favorites")))
                        .on_hover_text(tr("Show saved SQL snippets"))
                        .clicked()
                    {
                        self.query_editor.show_favorites();
                        self.active_panel = ActivePanel::QueryEditor;
                        self.state.set_status(tr("Favorites opened"));
                    }

                    // ---- Right-aligned actions ----
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let theme_icon = if ui.visuals().dark_mode { "◐" } else { "◑" };
                        if ui
                            .add_sized([34.0, 28.0], Button::new(theme_icon).frame(false))
                            .on_hover_text(tr("Toggle theme (Ctrl+T)"))
                            .clicked()
                        {
                            self.state.toggle_theme();
                        }

                        let error_count = self.state.errors.len();
                        if error_count > 0
                            && ui
                                .add_sized(
                                    [78.0, 28.0],
                                    Button::new(
                                        RichText::new(format!("⚠ {error_count}"))
                                            .color(palette.danger)
                                            .strong(),
                                    ),
                                )
                                .on_hover_text(tr("Open error details"))
                                .clicked()
                        {
                            self.error_window_open = true;
                        }

                        let lang_badge = i18n::locale()
                            .tag()
                            .split('-')
                            .next()
                            .unwrap_or("EN")
                            .to_uppercase();
                        ui.menu_button(RichText::new(lang_badge).strong(), |ui| {
                            for locale in Locale::ALL {
                                if ui
                                    .selectable_label(
                                        i18n::locale() == locale,
                                        format!("{}  ({})", locale.native_name(), locale.tag()),
                                    )
                                    .clicked()
                                {
                                    if i18n::set_locale(locale).is_ok() {
                                        self.state.set_status(i18n::format(
                                            "Language changed to {language}",
                                            &[("language", locale.native_name().to_string())],
                                        ));
                                    }
                                    ui.close_menu();
                                }
                            }
                        })
                        .response
                        .on_hover_text(format!(
                            "{} · {}",
                            tr("Language"),
                            i18n::locale().tag()
                        ));

                        ui.separator();
                        ui.label(
                            RichText::new(&self.state.status)
                                .small()
                                .color(palette.fg_muted)
                                .strong(),
                        )
                        .on_hover_text(&self.state.status);
                    });
                });
            });
    }
}

fn close_menu_item(
    ui: &mut Ui,
    label: &str,
    targets: Vec<eframe::egui::Id>,
    dirty_tabs: &[eframe::egui::Id],
    close_tabs: &mut Option<Vec<eframe::egui::Id>>,
) {
    let can_close = !targets.is_empty() && !targets.iter().any(|id| dirty_tabs.contains(id));
    let response = ui.add_enabled(can_close, Button::new(label));
    let response = if !targets.is_empty() && !can_close {
        response.on_hover_text("Save or rollback changes before closing")
    } else {
        response
    };
    if response.clicked() {
        *close_tabs = Some(targets);
        ui.close_menu();
    }
}
