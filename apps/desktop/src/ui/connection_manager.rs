use crate::ui::i18n::text as tr;
use app_core::connection::ConnectionManager;
use db_common::{
    redact_connection_url, AppError, ConnectionId, ConnectionProfile, ConnectionSecurity,
    ConnectionTestReport, DataSourceKind, PasswordUpdate, SslMode,
};
use eframe::egui::{
    Button, Checkbox, ComboBox, DragValue, Grid, RichText, ScrollArea, TextEdit, Ui, Window,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};
use tokio::runtime::Handle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorMode {
    New,
    Edit(ConnectionId),
}

#[derive(Debug, Clone)]
struct ConnectionForm {
    mode: EditorMode,
    name: String,
    group: String,
    kind: DataSourceKind,
    host: String,
    port: u16,
    username: String,
    password: String,
    database: String,
    file_path: String,
    save_password: bool,
    has_stored_password: bool,
    password_modified: bool,
    read_only: bool,
    ssl_mode: SslMode,
}

impl ConnectionForm {
    fn new() -> Self {
        let kind = DataSourceKind::PostgreSql;
        Self {
            mode: EditorMode::New,
            name: String::new(),
            group: String::new(),
            kind,
            host: "localhost".to_string(),
            port: kind.default_port().unwrap_or(0),
            username: String::new(),
            password: String::new(),
            database: String::new(),
            file_path: String::new(),
            save_password: true,
            has_stored_password: false,
            password_modified: false,
            read_only: false,
            ssl_mode: SslMode::Prefer,
        }
    }

    fn from_profile(profile: &ConnectionProfile) -> Self {
        Self {
            mode: EditorMode::Edit(profile.id),
            name: profile.name.clone(),
            group: profile.group.clone().unwrap_or_default(),
            kind: profile.kind,
            host: profile.host.clone().unwrap_or_default(),
            port: profile
                .port
                .unwrap_or_else(|| profile.kind.default_port().unwrap_or(0)),
            username: profile.username.clone().unwrap_or_default(),
            password: String::new(),
            database: profile.database.clone().unwrap_or_default(),
            file_path: profile.file_path.clone().unwrap_or_default(),
            save_password: profile.security.save_password,
            has_stored_password: profile.password_ref.is_some() && profile.security.save_password,
            password_modified: false,
            read_only: profile.security.read_only,
            ssl_mode: profile.security.ssl_mode,
        }
    }

    fn apply_kind_defaults(&mut self) {
        self.port = self.kind.default_port().unwrap_or(0);
        if self.kind.is_file_based() {
            self.host.clear();
            self.username.clear();
            self.database.clear();
            self.password.clear();
            self.save_password = false;
        } else if self.host.is_empty() {
            self.host = "localhost".to_string();
        }
    }

    fn to_profile(&self) -> ConnectionProfile {
        ConnectionProfile {
            id: match self.mode {
                EditorMode::New => ConnectionId::new(),
                EditorMode::Edit(id) => id,
            },
            name: self.name.trim().to_string(),
            group: non_empty(&self.group),
            kind: self.kind,
            host: if self.kind.is_file_based() {
                None
            } else {
                non_empty(&self.host)
            },
            port: if self.kind.is_file_based() || self.port == 0 {
                None
            } else {
                Some(self.port)
            },
            username: non_empty(&self.username),
            password_ref: None,
            database: non_empty(&self.database),
            file_path: if self.kind.is_file_based() {
                non_empty(&self.file_path)
            } else {
                None
            },
            security: ConnectionSecurity {
                ssl_mode: self.ssl_mode,
                read_only: self.read_only,
                save_password: self.save_password,
            },
            extra_params: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
enum ConnectionEvent {
    Loaded(Result<Vec<ConnectionProfile>, AppError>),
    Saved(Result<ConnectionProfile, AppError>),
    Deleted(Result<ConnectionId, AppError>),
    Tested(Result<ConnectionTestReport, AppError>),
    Exported(Result<(PathBuf, usize), AppError>),
    ImportInspected(Result<PendingConnectionImport, AppError>),
    Imported(Result<ConnectionImportResult, AppError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionChange {
    Saved(ConnectionId),
    Deleted(ConnectionId),
}

pub struct ConnectionManagerUi {
    manager: ConnectionManager,
    runtime: Handle,
    tx: Sender<ConnectionEvent>,
    rx: Receiver<ConnectionEvent>,
    profiles: Vec<ConnectionProfile>,
    recent: Vec<ConnectionId>,
    selected: Option<ConnectionId>,
    form: ConnectionForm,
    busy: bool,
    status: String,
    error: Option<String>,
    test_report: Option<ConnectionTestReport>,
    confirm_delete: Option<ConnectionId>,
    window_open: bool,
    pending_changes: Vec<ConnectionChange>,
    pending_import: Option<PendingConnectionImport>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConnectionExportFile {
    version: u32,
    profiles: Vec<ConnectionExportEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConnectionExportEntry {
    profile: ConnectionProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

#[derive(Debug)]
struct PendingConnectionImport {
    file: ConnectionExportFile,
    conflicting_names: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum ImportConflictPolicy {
    Overwrite,
    Rename,
}

#[derive(Debug)]
struct ConnectionImportResult {
    ids: Vec<ConnectionId>,
    overwritten: usize,
    renamed: usize,
}

impl ConnectionManagerUi {
    pub fn new(manager: ConnectionManager, runtime: Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut ui = Self {
            manager,
            runtime,
            tx,
            rx,
            profiles: Vec::new(),
            recent: Vec::new(),
            selected: None,
            form: ConnectionForm::new(),
            busy: false,
            status: "Connections ready".to_string(),
            error: None,
            test_report: None,
            confirm_delete: None,
            window_open: false,
            pending_changes: Vec::new(),
            pending_import: None,
        };
        ui.load_profiles();
        ui
    }

    pub fn new_connection(&mut self) {
        self.window_open = true;
        self.selected = None;
        self.form = ConnectionForm::new();
        self.test_report = None;
        self.error = None;
    }

    pub fn edit_connection(&mut self, id: ConnectionId) {
        self.window_open = true;
        self.select_profile(id);
    }

    pub fn delete_connection_prompt(&mut self, id: ConnectionId) {
        self.window_open = true;
        self.select_profile(id);
        self.confirm_delete = Some(id);
    }

    pub fn choose_import_file(&mut self) {
        if self.busy || self.pending_import.is_some() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Import Connections")
            .add_filter("Connection JSON", &["json"])
            .pick_file()
        {
            self.inspect_import(path);
        }
    }

    pub fn choose_export_file(
        &mut self,
        connection_ids: Option<Vec<ConnectionId>>,
        include_passwords: bool,
    ) {
        if self.busy || connection_ids.as_ref().is_some_and(Vec::is_empty) {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Export Connections")
            .set_file_name("orvyn-connections.json")
            .add_filter("Connection JSON", &["json"])
            .save_file()
        {
            self.export_connections(path, connection_ids, include_passwords);
        }
    }

    pub fn ui_window(&mut self, ctx: &eframe::egui::Context) -> Option<String> {
        self.drain_events();
        let mut message = None;

        if self.window_open {
            let mut open = true;
            let title = match self.form.mode {
                EditorMode::New => tr("New Connection"),
                EditorMode::Edit(_) => tr("Edit Connection"),
            };

            Window::new(title)
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(560.0)
                .show(ctx, |ui| {
                    self.editor_panel(ui, &mut message, true);
                });

            if !open {
                self.window_open = false;
                self.confirm_delete = None;
                self.error = None;
                self.test_report = None;
            }
        }

        self.import_conflict_window(ctx, &mut message);

        message.or_else(|| (!self.status.is_empty()).then(|| self.status.clone()))
    }

    pub fn take_changes(&mut self) -> Vec<ConnectionChange> {
        mem::take(&mut self.pending_changes)
    }

    #[allow(dead_code)]
    pub fn ui(&mut self, ui: &mut Ui) -> Option<String> {
        self.drain_events();
        let mut message = None;

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                self.list_panel(ui);
            });
            ui.separator();
            ui.vertical(|ui| {
                self.editor_panel(ui, &mut message, false);
            });
        });

        message
    }

    #[allow(dead_code)]
    fn list_panel(&mut self, ui: &mut Ui) {
        ui.heading(tr("Connections"));
        ui.horizontal(|ui| {
            if ui.button(tr("New")).clicked() {
                self.selected = None;
                self.form = ConnectionForm::new();
                self.test_report = None;
                self.error = None;
            }
            if ui
                .add_enabled(!self.busy, Button::new(tr("Refresh")))
                .clicked()
            {
                self.load_profiles();
            }
        });

        if !self.recent.is_empty() {
            ui.separator();
            ui.label(RichText::new(tr("Recent")).strong());
            for id in self.recent.clone() {
                if let Some(profile) = self.profiles.iter().find(|profile| profile.id == id) {
                    if ui
                        .button(format!("{} ({})", profile.name, profile.kind.as_str()))
                        .clicked()
                    {
                        self.select_profile(id);
                    }
                }
            }
        }

        ui.separator();
        ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for (group, ids) in self.grouped_profiles() {
                eframe::egui::CollapsingHeader::new(group)
                    .default_open(true)
                    .show(ui, |ui| {
                        for id in ids {
                            let Some(profile) =
                                self.profiles.iter().find(|profile| profile.id == id)
                            else {
                                continue;
                            };
                            let selected = self.selected == Some(id);
                            if ui
                                .selectable_label(
                                    selected,
                                    format!("{} ({})", profile.name, profile.kind.as_str()),
                                )
                                .clicked()
                            {
                                self.select_profile(id);
                            }
                        }
                    });
            }
        });
    }

    fn editor_panel(&mut self, ui: &mut Ui, message: &mut Option<String>, as_window: bool) {
        ui.heading(match self.form.mode {
            EditorMode::New => tr("New Connection"),
            EditorMode::Edit(_) => tr("Edit Connection"),
        });

        Grid::new("connection_editor_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(tr("Type"));
                let old_kind = self.form.kind;
                ComboBox::from_id_source("connection_kind")
                    .selected_text(self.form.kind.as_str())
                    .show_ui(ui, |ui| {
                        for kind in [
                            DataSourceKind::Redis,
                            DataSourceKind::MySql,
                            DataSourceKind::TiDb,
                            DataSourceKind::PostgreSql,
                            DataSourceKind::TimescaleDb,
                            DataSourceKind::SqlServer,
                            DataSourceKind::ClickHouse,
                            DataSourceKind::MongoDb,
                            DataSourceKind::Sqlite,
                            DataSourceKind::DuckDb,
                        ] {
                            ui.selectable_value(&mut self.form.kind, kind, kind.as_str());
                        }
                    });
                if old_kind != self.form.kind {
                    self.form.apply_kind_defaults();
                }
                ui.end_row();

                ui.label(tr("Name"));
                ui.add(TextEdit::singleline(&mut self.form.name).desired_width(260.0));
                ui.end_row();

                ui.label(tr("Group"));
                ui.add(TextEdit::singleline(&mut self.form.group).desired_width(260.0));
                ui.end_row();

                if self.form.kind.is_file_based() {
                    ui.label(tr("File path"));
                    ui.horizontal(|ui| {
                        ui.add(TextEdit::singleline(&mut self.form.file_path).desired_width(290.0));
                        if ui.button(tr("Browse...")).clicked() {
                            let mut dialog =
                                rfd::FileDialog::new().set_title(match self.form.kind {
                                    DataSourceKind::DuckDb => "Select DuckDB database",
                                    DataSourceKind::Sqlite => "Select SQLite database",
                                    _ => "Select database file",
                                });
                            if matches!(self.form.kind, DataSourceKind::DuckDb) {
                                dialog = dialog.add_filter("DuckDB database", &["duckdb", "db"]);
                            } else if matches!(self.form.kind, DataSourceKind::Sqlite) {
                                dialog = dialog
                                    .add_filter("SQLite database", &["sqlite", "sqlite3", "db"]);
                            }
                            if let Some(path) = dialog.pick_file() {
                                self.form.file_path = path.to_string_lossy().into_owned();
                                if self.form.name.trim().is_empty() {
                                    self.form.name = path
                                        .file_stem()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or_default()
                                        .to_string();
                                }
                            }
                        }
                    });
                    ui.end_row();
                } else {
                    ui.label(tr("Host"));
                    ui.add(TextEdit::singleline(&mut self.form.host).desired_width(260.0));
                    ui.end_row();

                    ui.label(tr("Port"));
                    ui.add(DragValue::new(&mut self.form.port).clamp_range(1..=65535));
                    ui.end_row();
                }

                if matches!(
                    self.form.kind,
                    DataSourceKind::MySql | DataSourceKind::PostgreSql
                ) {
                    ui.label(tr("Username"));
                    ui.add(TextEdit::singleline(&mut self.form.username).desired_width(260.0));
                    ui.end_row();
                }

                if !self.form.kind.is_file_based() {
                    ui.label(tr("Password"));
                    let response = ui.add(
                        TextEdit::singleline(&mut self.form.password)
                            .password(true)
                            .hint_text(
                                if self.form.has_stored_password && !self.form.password_modified {
                                    "Saved password"
                                } else {
                                    ""
                                },
                            )
                            .desired_width(260.0),
                    );
                    if response.changed() {
                        self.form.password_modified = true;
                        if !self.form.password.is_empty() {
                            self.form.save_password = true;
                        }
                    }
                    ui.end_row();
                }

                if self.form.kind.is_relational() && !self.form.kind.is_file_based() {
                    ui.label(tr("Database"));
                    ui.add(TextEdit::singleline(&mut self.form.database).desired_width(260.0));
                    ui.end_row();
                }

                ui.label(tr("SSL"));
                ComboBox::from_id_source("connection_ssl_mode")
                    .selected_text(format!("{:?}", self.form.ssl_mode))
                    .show_ui(ui, |ui| {
                        for mode in [
                            SslMode::Disable,
                            SslMode::Prefer,
                            SslMode::Require,
                            SslMode::VerifyCa,
                            SslMode::VerifyFull,
                        ] {
                            ui.selectable_value(&mut self.form.ssl_mode, mode, format!("{mode:?}"));
                        }
                    });
                ui.end_row();

                ui.label(tr("Options"));
                ui.horizontal(|ui| {
                    ui.add(Checkbox::new(&mut self.form.read_only, tr("Read only")));
                    ui.add(Checkbox::new(
                        &mut self.form.save_password,
                        tr("Save password"),
                    ));
                });
                ui.end_row();
            });

        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.busy, Button::new(tr("Test Connection")))
                .clicked()
            {
                self.test_connection();
            }
            if ui
                .add_enabled(!self.busy, Button::new(tr("Save")))
                .clicked()
            {
                self.save_connection();
            }
            if as_window
                && ui
                    .add_enabled(!self.busy, Button::new(tr("Cancel")))
                    .clicked()
            {
                self.window_open = false;
                self.confirm_delete = None;
                self.error = None;
                self.test_report = None;
            }
            if let EditorMode::Edit(id) = self.form.mode {
                if ui
                    .add_enabled(!self.busy, Button::new(tr("Delete")))
                    .clicked()
                {
                    self.confirm_delete = Some(id);
                }
            }
        });

        if let Some(id) = self.confirm_delete {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, "Confirm delete connection?");
                if ui.button(tr("Delete")).clicked() {
                    self.delete_connection(id);
                    self.confirm_delete = None;
                }
                if ui.button(tr("Cancel")).clicked() {
                    self.confirm_delete = None;
                }
            });
        }

        if let Some(report) = &self.test_report {
            ui.add_space(8.0);
            let label = if report.success {
                format!(
                    "Test succeeded in {} ms{}",
                    report.elapsed_ms,
                    report
                        .server_version
                        .as_ref()
                        .map(|version| format!(" ({version})"))
                        .unwrap_or_default()
                )
            } else {
                format!("Test failed: {}", report.message)
            };
            ui.label(label);
        }

        if let Some(error) = &self.error {
            ui.add_space(8.0);
            ui.colored_label(ui.visuals().error_fg_color, error);
        }

        if !self.status.is_empty() {
            ui.add_space(8.0);
            ui.label(&self.status);
            *message = Some(self.status.clone());
        }
    }

    fn import_conflict_window(
        &mut self,
        ctx: &eframe::egui::Context,
        message: &mut Option<String>,
    ) {
        let Some(pending) = self.pending_import.as_ref() else {
            return;
        };
        let names = pending.conflicting_names.clone();
        let total = pending.file.profiles.len();
        let mut action = None;
        let mut open = true;
        let mut cancel = false;

        Window::new(tr("Connection Name Conflicts"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} of {total} imported connection name(s) are already in use:",
                    names.len()
                ));
                ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    for name in &names {
                        ui.label(format!("- {name}"));
                    }
                });
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr("Overwrite Existing")).clicked() {
                        action = Some(ImportConflictPolicy::Overwrite);
                    }
                    if ui.button(tr("Keep Both and Rename")).clicked() {
                        action = Some(ImportConflictPolicy::Rename);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if let Some(policy) = action {
            let pending = self.pending_import.take().expect("pending import");
            self.start_import(pending.file, policy);
            *message = Some("Importing connections...".to_string());
        } else if !open || cancel {
            self.pending_import = None;
            self.status = "Connection import cancelled".to_string();
        }
    }

    fn load_profiles(&mut self) {
        self.busy = true;
        let tx = self.tx.clone();
        let manager = self.manager.clone();
        self.runtime.spawn(async move {
            let result = manager.list_profiles().await;
            let _ = tx.send(ConnectionEvent::Loaded(result));
        });
    }

    fn save_connection(&mut self) {
        self.busy = true;
        self.error = None;
        let profile = self.form.to_profile();
        let password = if self.form.save_password {
            if self.form.password_modified {
                PasswordUpdate::Replace(self.form.password.clone())
            } else if self.form.has_stored_password {
                PasswordUpdate::Keep
            } else {
                PasswordUpdate::Replace(String::new())
            }
        } else {
            PasswordUpdate::Clear
        };
        let mode = self.form.mode;
        let tx = self.tx.clone();
        let manager = self.manager.clone();

        self.runtime.spawn(async move {
            let result = match mode {
                EditorMode::New => manager.create_profile(profile, password).await,
                EditorMode::Edit(_) => manager.update_profile(profile, password).await,
            };
            let _ = tx.send(ConnectionEvent::Saved(result));
        });
    }

    fn test_connection(&mut self) {
        self.busy = true;
        self.error = None;
        self.test_report = None;
        let profile = self.form.to_profile();
        let runtime_password = if self.form.password.is_empty() {
            None
        } else {
            Some(self.form.password.clone())
        };
        let tx = self.tx.clone();
        let manager = self.manager.clone();

        self.runtime.spawn(async move {
            let result = manager
                .test_profile(&profile, runtime_password.as_deref())
                .await;
            let _ = tx.send(ConnectionEvent::Tested(result));
        });
    }

    fn delete_connection(&mut self, id: ConnectionId) {
        self.busy = true;
        self.error = None;
        let tx = self.tx.clone();
        let manager = self.manager.clone();

        self.runtime.spawn(async move {
            let result = manager.delete_profile(id).await.map(|_| id);
            let _ = tx.send(ConnectionEvent::Deleted(result));
        });
    }

    fn export_connections(
        &mut self,
        path: PathBuf,
        connection_ids: Option<Vec<ConnectionId>>,
        include_passwords: bool,
    ) {
        self.busy = true;
        self.error = None;
        self.status = "Exporting connections...".to_string();
        let tx = self.tx.clone();
        let manager = self.manager.clone();

        self.runtime.spawn(async move {
            let result = export_connections_to_file(
                manager,
                path.clone(),
                connection_ids,
                include_passwords,
            )
            .await
            .map(|count| (path, count));
            let _ = tx.send(ConnectionEvent::Exported(result));
        });
    }

    fn inspect_import(&mut self, path: PathBuf) {
        self.busy = true;
        self.error = None;
        self.status = "Checking connection backup...".to_string();
        let tx = self.tx.clone();
        let manager = self.manager.clone();

        self.runtime.spawn(async move {
            let result = inspect_connections_file(manager, path).await;
            let _ = tx.send(ConnectionEvent::ImportInspected(result));
        });
    }

    fn start_import(&mut self, file: ConnectionExportFile, policy: ImportConflictPolicy) {
        self.busy = true;
        self.error = None;
        self.status = "Importing connections...".to_string();
        let tx = self.tx.clone();
        let manager = self.manager.clone();

        self.runtime.spawn(async move {
            let result = import_connections(manager, file, policy).await;
            let _ = tx.send(ConnectionEvent::Imported(result));
        });
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.busy = false;
            match event {
                ConnectionEvent::Loaded(result) => match result {
                    Ok(profiles) => {
                        self.profiles = profiles;
                        self.status = crate::ui::i18n::format(
                            "Loaded {count} connections",
                            &[("count", self.profiles.len().to_string())],
                        );
                    }
                    Err(error) => {
                        self.error = Some(actionable_error(
                            "Load connections failed",
                            error,
                            &self.form.password,
                        ))
                    }
                },
                ConnectionEvent::Saved(result) => match result {
                    Ok(profile) => {
                        let id = profile.id;
                        upsert_profile(&mut self.profiles, profile.clone());
                        self.selected = Some(id);
                        self.form = ConnectionForm::from_profile(&profile);
                        self.remember_recent(id);
                        self.pending_changes.push(ConnectionChange::Saved(id));
                        self.status = "Connection saved".to_string();
                        self.window_open = false;
                    }
                    Err(error) => {
                        self.error = Some(actionable_error(
                            "Save connection failed",
                            error,
                            &self.form.password,
                        ))
                    }
                },
                ConnectionEvent::Deleted(result) => match result {
                    Ok(id) => {
                        self.profiles.retain(|profile| profile.id != id);
                        self.recent.retain(|recent_id| *recent_id != id);
                        self.selected = None;
                        self.form = ConnectionForm::new();
                        self.pending_changes.push(ConnectionChange::Deleted(id));
                        self.status = "Connection deleted".to_string();
                        self.window_open = false;
                    }
                    Err(error) => {
                        self.error = Some(actionable_error(
                            "Delete connection failed",
                            error,
                            &self.form.password,
                        ))
                    }
                },
                ConnectionEvent::Tested(result) => match result {
                    Ok(report) => {
                        self.status = if report.success {
                            if !self.form.password.is_empty() {
                                self.form.save_password = true;
                            }
                            "Connection test succeeded".to_string()
                        } else {
                            "Connection test failed".to_string()
                        };
                        self.test_report = Some(report);
                    }
                    Err(error) => {
                        self.error = Some(actionable_error(
                            "Test connection failed",
                            error,
                            &self.form.password,
                        ))
                    }
                },
                ConnectionEvent::Exported(result) => match result {
                    Ok((path, count)) => {
                        self.status = format!("Exported {count} connection(s): {}", path.display());
                    }
                    Err(error) => {
                        self.error = Some(actionable_error(
                            "Export connections failed",
                            error,
                            &self.form.password,
                        ));
                    }
                },
                ConnectionEvent::ImportInspected(result) => match result {
                    Ok(pending) if pending.conflicting_names.is_empty() => {
                        self.start_import(pending.file, ImportConflictPolicy::Rename);
                    }
                    Ok(pending) => {
                        self.status = format!(
                            "{} connection name conflict(s) require a choice",
                            pending.conflicting_names.len()
                        );
                        self.pending_import = Some(pending);
                    }
                    Err(error) => {
                        self.error = Some(actionable_error(
                            "Import connections failed",
                            error,
                            &self.form.password,
                        ));
                    }
                },
                ConnectionEvent::Imported(result) => match result {
                    Ok(result) => {
                        self.status = format!(
                            "Imported {} connection(s), overwrote {}, renamed {}",
                            result.ids.len(),
                            result.overwritten,
                            result.renamed
                        );
                        if let Some(id) = result.ids.first().copied() {
                            self.pending_changes.push(ConnectionChange::Saved(id));
                        }
                        self.load_profiles();
                    }
                    Err(error) => {
                        self.error = Some(actionable_error(
                            "Import connections failed",
                            error,
                            &self.form.password,
                        ));
                    }
                },
            }
        }
    }

    fn select_profile(&mut self, id: ConnectionId) {
        if let Some(profile) = self.profiles.iter().find(|profile| profile.id == id) {
            self.selected = Some(id);
            self.form = ConnectionForm::from_profile(profile);
            self.test_report = None;
            self.error = None;
            self.remember_recent(id);
        }
    }

    fn remember_recent(&mut self, id: ConnectionId) {
        self.recent.retain(|recent_id| *recent_id != id);
        self.recent.insert(0, id);
        self.recent.truncate(8);
    }

    #[allow(dead_code)]
    fn grouped_profiles(&self) -> BTreeMap<String, Vec<ConnectionId>> {
        let mut groups = BTreeMap::<String, Vec<ConnectionId>>::new();
        for profile in &self.profiles {
            groups
                .entry(
                    profile
                        .group
                        .clone()
                        .unwrap_or_else(|| "Ungrouped".to_string()),
                )
                .or_default()
                .push(profile.id);
        }
        groups
    }
}

fn upsert_profile(profiles: &mut Vec<ConnectionProfile>, profile: ConnectionProfile) {
    if let Some(existing) = profiles
        .iter_mut()
        .find(|existing| existing.id == profile.id)
    {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
}

async fn export_connections_to_file(
    manager: ConnectionManager,
    path: PathBuf,
    connection_ids: Option<Vec<ConnectionId>>,
    include_passwords: bool,
) -> Result<usize, AppError> {
    let mut profiles = manager.list_profiles().await?;
    if let Some(connection_ids) = connection_ids {
        let selected = connection_ids.into_iter().collect::<BTreeSet<_>>();
        profiles.retain(|profile| selected.contains(&profile.id));
    }
    if profiles.is_empty() {
        return Err(AppError::validation("no connections selected for export"));
    }
    let mut entries = Vec::with_capacity(profiles.len());

    for mut profile in profiles {
        let password = if include_passwords {
            manager
                .profile_with_password(profile.id, None)
                .await?
                .1
                .filter(|password| !password.is_empty())
        } else {
            None
        };

        profile.password_ref = None;
        if password.is_none() {
            profile.security.save_password = false;
        }
        entries.push(ConnectionExportEntry { profile, password });
    }

    let file = ConnectionExportFile {
        version: 1,
        profiles: entries,
    };
    let bytes = serde_json::to_vec_pretty(&file)?;
    tokio::fs::write(path, bytes).await?;
    Ok(file.profiles.len())
}

async fn inspect_connections_file(
    manager: ConnectionManager,
    path: PathBuf,
) -> Result<PendingConnectionImport, AppError> {
    let bytes = tokio::fs::read(path).await?;
    let file: ConnectionExportFile = serde_json::from_slice(&bytes)?;
    if file.version != 1 {
        return Err(AppError::validation(format!(
            "unsupported connection export version {}",
            file.version
        )));
    }

    let existing = manager.list_profiles().await?;
    let mut used_names = existing
        .iter()
        .map(|profile| normalized_connection_name(&profile.name))
        .collect::<BTreeSet<_>>();
    let mut conflicts = BTreeSet::new();
    for entry in &file.profiles {
        let key = normalized_connection_name(&entry.profile.name);
        if !used_names.insert(key) {
            conflicts.insert(entry.profile.name.clone());
        }
    }

    Ok(PendingConnectionImport {
        file,
        conflicting_names: conflicts.into_iter().collect(),
    })
}

async fn import_connections(
    manager: ConnectionManager,
    file: ConnectionExportFile,
    policy: ImportConflictPolicy,
) -> Result<ConnectionImportResult, AppError> {
    let mut existing = manager.list_profiles().await?;
    let mut used_names = existing
        .iter()
        .map(|profile| normalized_connection_name(&profile.name))
        .collect::<BTreeSet<_>>();
    let mut used_ids = existing
        .iter()
        .map(|profile| profile.id)
        .collect::<BTreeSet<_>>();

    let mut imported = Vec::with_capacity(file.profiles.len());
    let mut overwritten = 0;
    let mut renamed = 0;
    for entry in file.profiles {
        let mut profile = entry.profile;
        profile.password_ref = None;
        let password = entry.password.unwrap_or_default();
        let name_key = normalized_connection_name(&profile.name);
        let name_match = existing
            .iter()
            .position(|item| normalized_connection_name(&item.name) == name_key);

        if let (Some(index), ImportConflictPolicy::Overwrite) = (name_match, policy) {
            let previous = existing[index].clone();
            profile.id = previous.id;
            let password_update = if password.is_empty() {
                profile.security.save_password = previous.security.save_password;
                PasswordUpdate::Keep
            } else {
                profile.security.save_password = true;
                PasswordUpdate::Replace(password)
            };
            let id = profile.id;
            manager.update_profile(profile, password_update).await?;
            existing[index] = manager
                .get_profile(id)
                .await?
                .expect("updated connection profile");
            imported.push(id);
            overwritten += 1;
        } else {
            if name_match.is_some() {
                profile.name = unique_import_name(&profile.name, &used_names);
                renamed += 1;
            }
            if used_ids.contains(&profile.id) {
                profile.id = ConnectionId::new();
            }
            let password_update = if password.is_empty() {
                profile.security.save_password = false;
                PasswordUpdate::Clear
            } else {
                profile.security.save_password = true;
                PasswordUpdate::Replace(password)
            };
            let id = profile.id;
            manager.create_profile(profile, password_update).await?;
            let created = manager
                .get_profile(id)
                .await?
                .expect("created connection profile");
            used_names.insert(normalized_connection_name(&created.name));
            used_ids.insert(id);
            existing.push(created);
            imported.push(id);
        }
    }

    Ok(ConnectionImportResult {
        ids: imported,
        overwritten,
        renamed,
    })
}

fn normalized_connection_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn unique_import_name(name: &str, used_names: &BTreeSet<String>) -> String {
    let base = name.trim();
    let mut suffix = 2;
    loop {
        let candidate = format!("{base} ({suffix})");
        if !used_names.contains(&normalized_connection_name(&candidate)) {
            return candidate;
        }
        suffix += 1;
    }
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn actionable_error(context: &str, error: AppError, password: &str) -> String {
    let mut text = format!("{context}: {}", error.message);
    if let Some(detail) = error.detail {
        text.push_str(" - ");
        text.push_str(&detail);
    }
    let text = redact_ui_text(&text, password);
    format!("{text}. Check required fields, network reachability, credentials, and file paths.")
}

fn redact_ui_text(value: &str, password: &str) -> String {
    let mut text = redact_connection_url(value);
    if !password.is_empty() {
        text = text.replace(password, "********");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::storage::MemoryConnectionProfileStore;
    use secure_store::EncryptedFileStore;
    use std::sync::Arc;

    fn profile(name: &str) -> ConnectionProfile {
        ConnectionProfile::builder(DataSourceKind::PostgreSql, name)
            .host("localhost")
            .username("postgres")
            .build()
            .expect("valid profile")
    }

    fn manager() -> (ConnectionManager, PathBuf) {
        let secrets_path = std::env::temp_dir().join(format!(
            "orvyn-connection-import-test-{}.json",
            ConnectionId::new()
        ));
        let manager = ConnectionManager::new(
            Arc::new(MemoryConnectionProfileStore::default()),
            Arc::new(EncryptedFileStore::new(&secrets_path, "test-key")),
            Vec::new(),
        );
        (manager, secrets_path)
    }

    #[test]
    fn import_names_are_made_unique_case_insensitively() {
        let used = ["production".to_string(), "production (2)".to_string()]
            .into_iter()
            .collect();

        assert_eq!(unique_import_name("Production", &used), "Production (3)");
    }

    #[tokio::test]
    async fn overwrite_by_name_keeps_existing_password_when_backup_omits_it() {
        let (manager, secrets_path) = manager();
        let mut existing = profile("Production");
        existing.security.save_password = true;
        let existing_id = existing.id;
        manager
            .create_profile(existing, PasswordUpdate::Replace("secret".to_string()))
            .await
            .expect("existing profile");

        let mut imported = profile("production");
        imported.host = Some("db.example.com".to_string());
        let file = ConnectionExportFile {
            version: 1,
            profiles: vec![ConnectionExportEntry {
                profile: imported,
                password: None,
            }],
        };

        let result = import_connections(manager.clone(), file, ImportConflictPolicy::Overwrite)
            .await
            .expect("import");
        let (saved, password) = manager
            .profile_with_password(existing_id, None)
            .await
            .expect("saved profile");

        assert_eq!(result.overwritten, 1);
        assert_eq!(saved.host.as_deref(), Some("db.example.com"));
        assert_eq!(password.as_deref(), Some("secret"));
        let _ = std::fs::remove_file(secrets_path);
    }

    #[tokio::test]
    async fn rename_policy_keeps_both_connections() {
        let (manager, secrets_path) = manager();
        manager
            .create_profile(profile("Local"), PasswordUpdate::Clear)
            .await
            .expect("existing profile");
        let file = ConnectionExportFile {
            version: 1,
            profiles: vec![ConnectionExportEntry {
                profile: profile("local"),
                password: None,
            }],
        };

        let result = import_connections(manager.clone(), file, ImportConflictPolicy::Rename)
            .await
            .expect("import");
        let profiles = manager.list_profiles().await.expect("profiles");

        assert_eq!(result.renamed, 1);
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().any(|profile| profile.name == "local (2)"));
        let _ = std::fs::remove_file(secrets_path);
    }

    #[tokio::test]
    async fn inspection_reports_existing_connection_names() {
        let (manager, secrets_path) = manager();
        manager
            .create_profile(profile("Production"), PasswordUpdate::Clear)
            .await
            .expect("existing profile");
        let import_path = std::env::temp_dir().join(format!(
            "orvyn-connection-import-source-{}.json",
            ConnectionId::new()
        ));
        let file = ConnectionExportFile {
            version: 1,
            profiles: vec![ConnectionExportEntry {
                profile: profile("production"),
                password: None,
            }],
        };
        std::fs::write(&import_path, serde_json::to_vec(&file).expect("json"))
            .expect("write import");

        let pending = inspect_connections_file(manager, import_path.clone())
            .await
            .expect("inspect");

        assert_eq!(pending.conflicting_names, vec!["production"]);
        let _ = std::fs::remove_file(import_path);
        let _ = std::fs::remove_file(secrets_path);
    }

    #[tokio::test]
    async fn export_can_be_limited_to_selected_connections() {
        let (manager, secrets_path) = manager();
        let selected = manager
            .create_profile(profile("Selected"), PasswordUpdate::Clear)
            .await
            .expect("selected profile");
        manager
            .create_profile(profile("Other"), PasswordUpdate::Clear)
            .await
            .expect("other profile");
        let export_path = std::env::temp_dir().join(format!(
            "orvyn-connection-export-selected-{}.json",
            ConnectionId::new()
        ));

        let count = export_connections_to_file(
            manager,
            export_path.clone(),
            Some(vec![selected.id]),
            false,
        )
        .await
        .expect("export");
        let bytes = std::fs::read(&export_path).expect("read export");
        let file: ConnectionExportFile = serde_json::from_slice(&bytes).expect("export json");

        assert_eq!(count, 1);
        assert_eq!(file.profiles.len(), 1);
        assert_eq!(file.profiles[0].profile.id, selected.id);
        let _ = std::fs::remove_file(export_path);
        let _ = std::fs::remove_file(secrets_path);
    }
}
