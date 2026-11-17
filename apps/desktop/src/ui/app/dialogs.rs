use super::{
    export_sql::{database_export_mode_label, qualified_table_label},
    payloads::{ClearRequest, TableExportProgress},
    style::{current_palette, Palette},
    ActivePanel, DesktopApp,
};
use crate::ui::{
    i18n::text as tr,
    import_wizard::ImportWizard,
    object_tree::DatabaseExportMode,
    state::{TaskState, TaskStatus},
};
use db_common::ConnectionId;
use eframe::egui::{
    vec2, Align, Button, Color32, Context, Frame, Layout, Margin, ProgressBar, RichText, Spinner,
    Stroke, TextEdit, TopBottomPanel, Window,
};
use import_export::ExportFormat;

impl DesktopApp {
    pub(super) fn import_table_window(&mut self, ctx: &Context) {
        let Some(request) = self.import_table_request.as_mut() else {
            return;
        };

        let mut open = true;
        let mut start_import = false;
        let mut cancel = false;

        Window::new("Import Table")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(tr("Target table"));
                ui.add_space(6.0);
                eframe::egui::Grid::new("connection_import_target_grid")
                    .num_columns(2)
                    .spacing(vec2(8.0, 6.0))
                    .show(ui, |ui| {
                        ui.label(tr("Database"));
                        ui.add(TextEdit::singleline(&mut request.database).desired_width(220.0));
                        ui.end_row();
                        ui.label(tr("Schema"));
                        ui.add(TextEdit::singleline(&mut request.schema).desired_width(220.0));
                        ui.end_row();
                        ui.label(tr("Table"));
                        ui.add(TextEdit::singleline(&mut request.table).desired_width(220.0));
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Leave database/schema empty to use the connection default.")
                        .small()
                        .color(Color32::GRAY),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled(!request.table.trim().is_empty(), Button::new(tr("Continue")))
                        .clicked()
                    {
                        start_import = true;
                    }
                });
            });

        if start_import {
            if let Some(request) = self.import_table_request.take() {
                let table = request.table.trim().to_string();
                let wizard = ImportWizard::new(
                    request.connection_id,
                    trimmed_option(request.database),
                    trimmed_option(request.schema),
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
        }

        if cancel || !open {
            self.import_table_request = None;
        }
    }

    pub(super) fn request_database_export(
        &mut self,
        connection_id: ConnectionId,
        database: Option<String>,
        mode: DatabaseExportMode,
    ) {
        let mode_label = database_export_mode_label(mode);
        let target_name = database
            .as_deref()
            .map(safe_export_file_stem)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "database".to_string());
        let file_name = match mode {
            DatabaseExportMode::Structure => format!("{target_name}_structure.sql"),
            DatabaseExportMode::StructureAndData => {
                format!("{target_name}_structure_data.sql")
            }
        };
        let path = rfd::FileDialog::new()
            .set_title(mode_label)
            .set_file_name(file_name)
            .add_filter("SQL", &["sql"])
            .save_file();

        if let Some(path) = path {
            self.spawn_database_export(connection_id, database, mode, path);
        } else {
            self.state.set_status("Database export canceled");
        }
    }

    pub(super) fn table_export_window(&mut self, ctx: &Context) {
        // The async file dialog landed a save path: hand it to the export task.
        let pending_path = self.pending_table_export_path.lock().unwrap().take();
        if let Some(path) = pending_path {
            if let Some(export_request) = self.export_request.take() {
                self.spawn_table_export(export_request, path);
            }
            return;
        }

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
        let mut launch_file_dialog = false;
        let mut export_format = request.format;
        let mut cancel_export = false;

        Window::new(tr("Export Table Data"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("{}: {}", tr("Table"), table_name));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(tr("Format"));
                    eframe::egui::ComboBox::from_id_source("table_export_format")
                        .selected_text(match request.format {
                            ExportFormat::Xlsx => "XLSX",
                            ExportFormat::Csv => "CSV",
                            ExportFormat::Json => "JSON",
                            ExportFormat::ExcelCompatible => "TSV",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut request.format, ExportFormat::Csv, "CSV");
                            ui.selectable_value(&mut request.format, ExportFormat::Json, "JSON");
                            ui.selectable_value(&mut request.format, ExportFormat::Xlsx, "XLSX");
                        });
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(tr(
                        "Export runs in the background and writes directly to file.",
                    ))
                    .small()
                    .color(Color32::GRAY),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel_export = true;
                    }
                    if ui.button(tr("Export")).clicked() {
                        export_format = request.format;
                        launch_file_dialog = true;
                    }
                });
            });
            // The native save dialog must not run synchronously inside the
            // egui frame: on Windows the modal COM dialog can silently fail
            // when opened from the winit event loop callback. Pick the path
            // on a background thread instead.
            if launch_file_dialog {
                let (label, ext) = match export_format {
                    ExportFormat::Xlsx => ("XLSX", "xlsx"),
                    ExportFormat::Csv => ("CSV", "csv"),
                    ExportFormat::Json => ("JSON", "json"),
                    ExportFormat::ExcelCompatible => ("Excel", "tsv"),
                };
                let dialog = rfd::AsyncFileDialog::new()
                    .set_title("Export Table Data")
                    .set_file_name(format!("{}.{ext}", table_file_name))
                    .add_filter(label, &[ext]);
                let pending_path = self.pending_table_export_path.clone();
                self.runtime.spawn(async move {
                    if let Some(file) = dialog.save_file().await {
                        *pending_path.lock().unwrap() = Some(file.path().to_path_buf());
                    }
                });
            }

        if cancel_export {
            self.export_request = None;
        }

        if !open {
            self.export_request = None;
        }
    }

    pub(super) fn drop_table_window(&mut self, ctx: &Context) {
        let Some(request) = self.drop_table_request.as_ref() else {
            return;
        };

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
        let object_name = match request.object_kind {
            super::payloads::DropObjectKind::View => tr("View"),
            super::payloads::DropObjectKind::Database => tr("Database"),
            super::payloads::DropObjectKind::Table => tr("Table"),
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;

        Window::new(format!("{} {}", tr("Delete"), object_name))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("{} {}?", tr("Delete"), object_name)).strong());
                ui.add_space(8.0);
                ui.label(RichText::new(&table_name).monospace());
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "This will execute DROP {} and cannot be undone here.",
                        object_name.to_uppercase()
                    ))
                    .small()
                    .color(ui.visuals().warn_fg_color),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("Delete")).clicked() {
                        confirm = true;
                    }
                });
            });

        if confirm {
            if let Some(request) = self.drop_table_request.take() {
                self.spawn_table_drop(request);
            }
        }

        if cancel || !open {
            self.drop_table_request = None;
        }
    }

    pub(super) fn clear_window(&mut self, ctx: &Context) {
        let Some(request) = self.clear_request.as_ref() else {
            return;
        };

        let (title, target, warning, confirm_label) = match request {
            ClearRequest::Table {
                database,
                schema,
                table,
                ..
            } => (
                tr("Clear Table"),
                qualified_table_label(
                    schema
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .or_else(|| database.as_deref().filter(|value| !value.is_empty())),
                    table,
                ),
                tr("All rows will be removed while the table structure is kept. This cannot be undone here."),
                tr("Clear Table"),
            ),
            ClearRequest::RedisDatabase { database, .. } => (
                tr("Clear Redis Database"),
                format!("Redis DB {database}"),
                tr("FLUSHDB will permanently remove every key in this Redis database only."),
                tr("Clear Database"),
            ),
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;

        Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new(tr("Clear this target?")).strong());
                ui.add_space(8.0);
                ui.label(RichText::new(target).monospace());
                ui.add_space(8.0);
                ui.label(
                    RichText::new(warning)
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(confirm_label).clicked() {
                        confirm = true;
                    }
                });
            });

        if confirm {
            if let Some(request) = self.clear_request.take() {
                self.spawn_clear(request);
            }
        } else if cancel || !open {
            self.clear_request = None;
        }
    }

    pub(super) fn bottom_status(&mut self, ctx: &Context) {
        let palette = current_palette(ctx);
        TopBottomPanel::bottom("bottom_status")
            .frame(
                Frame::default()
                    .fill(palette.surface)
                    .inner_margin(Margin::symmetric(12.0, 5.0)),
            )
            .show(ctx, |ui| {
                if ui.visuals().dark_mode {
                    ui.painter().line_segment(
                        [ui.max_rect().left_top(), ui.max_rect().right_top()],
                        Stroke::new(1.0, palette.border),
                    );
                }
                ui.horizontal(|ui| {
                    // While a table export is running, replace the status text
                    // with live row progress so it is not overwritten by panels.
                    let export_progress = self
                        .table_export_progress
                        .as_ref()
                        .and_then(|cell| cell.lock().ok());
                    if let Some(progress) = export_progress.as_ref() {
                        if let Some((ok, message, _)) = &progress.outcome {
                            // Finished result: survives the panels' per-frame
                            // status overwrites for a few seconds.
                            let text = format!("{} {message}", if *ok { "✓" } else { "✗" });
                            let color = if *ok {
                                Color32::from_rgb(94, 200, 120)
                            } else {
                                palette.danger
                            };
                            ui.label(RichText::new(&text).small().color(color))
                                .on_hover_text(&text);
                        } else {
                            self.render_export_running_progress(ui, palette, progress);
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(selected_count) = self
                            .active_table_editor
                            .and_then(|id| self.table_editors.get(&id))
                            .map(|editor| editor.selected_row_count())
                            .filter(|count| *count > 0)
                        {
                            ui.label(
                                RichText::new(format!("{selected_count} rows selected"))
                                    .small()
                                    .color(palette.fg_muted),
                            );
                            ui.separator();
                        }
                        ui.label(
                            RichText::new(match self.state.errors.len() {
                                0 => tr("No errors").to_string(),
                                count => {
                                    format!("⚠ {count} {}", tr("errors"))
                                }
                            })
                            .small()
                            .color(if self.state.errors.is_empty() {
                                palette.fg_muted
                            } else {
                                palette.danger
                            }),
                        );
                        if !self.state.errors.is_empty()
                            && ui
                                .small_button(tr("View"))
                                .on_hover_text(tr("Open error details"))
                                .clicked()
                        {
                            self.error_window_open = true;
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(active_panel_label(self.active_panel))
                                .small()
                                .color(palette.fg_muted),
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(task_summary(&self.state.tasks))
                                .small()
                                .color(palette.fg_muted),
                        );
                    });
                });
            });
    }

    /// Spinner + live row counter (plus a bar once the total is known) for
    /// a running table export.
    fn render_export_running_progress(
        &self,
        ui: &mut eframe::egui::Ui,
        palette: Palette,
        progress: &TableExportProgress,
    ) {
        ui.add(Spinner::new());
        let progress_label = match progress.total_rows {
            Some(total) if total > 0 => format!(
                "{} export {}: {}/{} rows",
                progress.format_label, progress.table_name, progress.rows_done, total
            ),
            _ => format!(
                "{} export {}: {} rows",
                progress.format_label, progress.table_name, progress.rows_done
            ),
        };
        ui.label(RichText::new(&progress_label).small().color(palette.fg))
            .on_hover_text(&progress_label);
        if let Some(fraction) = progress
            .total_rows
            .filter(|total| *total > 0)
            .map(|total| (progress.rows_done as f32 / total as f32).min(1.0))
        {
            ui.add_sized([140.0, 16.0], ProgressBar::new(fraction));
        }
    }

    pub(super) fn error_window(&mut self, ctx: &Context) {
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

                        Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("●").color(Color32::RED).strong());
                                ui.label(RichText::new(&error.title).strong());
                            });

                            ui.separator();
                            ui.label(RichText::new(&error.message).color(Color32::WHITE));

                            if let Some(detail) = &error.detail {
                                ui.add_space(8.0);

                                let toggle_text = if error.expanded {
                                    "▼ Hide Details"
                                } else {
                                    "▶ Show Details"
                                };
                                if ui.button(toggle_text).clicked() {
                                    error.expanded = !error.expanded;
                                }

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

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.state.errors.is_empty(), Button::new(tr("Clear All")))
                            .clicked()
                        {
                            self.state.errors.clear();
                            self.error_window_open = false;
                        }

                        if ui.button(tr("Copy All")).clicked() {
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

fn trimmed_option(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn safe_export_file_stem(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
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
        ActivePanel::SearchBrowser => "Browse",
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
