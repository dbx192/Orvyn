use app_core::connection::ConnectionManager;
use db_common::{
    redact_connection_url, AppError, ConnectionId, ConnectionProfile, ConnectionSecurity,
    ConnectionTestReport, DataSourceKind, PasswordUpdate, SslMode,
};
use eframe::egui::{
    Button, Checkbox, ComboBox, DragValue, Grid, RichText, ScrollArea, TextEdit, Ui, Window,
};
use std::{
    collections::BTreeMap,
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
            save_password: false,
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

    pub fn ui_window(&mut self, ctx: &eframe::egui::Context) -> Option<String> {
        self.drain_events();
        if !self.window_open {
            return None;
        }

        let mut open = true;
        let mut message = None;
        let title = match self.form.mode {
            EditorMode::New => "New Connection",
            EditorMode::Edit(_) => "Edit Connection",
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

        message.or_else(|| (!self.status.is_empty()).then(|| self.status.clone()))
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
        ui.heading("Connections");
        ui.horizontal(|ui| {
            if ui.button("New").clicked() {
                self.selected = None;
                self.form = ConnectionForm::new();
                self.test_report = None;
                self.error = None;
            }
            if ui.add_enabled(!self.busy, Button::new("Refresh")).clicked() {
                self.load_profiles();
            }
        });

        if !self.recent.is_empty() {
            ui.separator();
            ui.label(RichText::new("Recent").strong());
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
            EditorMode::New => "New Connection",
            EditorMode::Edit(_) => "Edit Connection",
        });

        Grid::new("connection_editor_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Type");
                let old_kind = self.form.kind;
                ComboBox::from_id_source("connection_kind")
                    .selected_text(self.form.kind.as_str())
                    .show_ui(ui, |ui| {
                        for kind in [
                            DataSourceKind::Redis,
                            DataSourceKind::MySql,
                            DataSourceKind::PostgreSql,
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

                ui.label("Name");
                ui.add(TextEdit::singleline(&mut self.form.name).desired_width(260.0));
                ui.end_row();

                ui.label("Group");
                ui.add(TextEdit::singleline(&mut self.form.group).desired_width(260.0));
                ui.end_row();

                if self.form.kind.is_file_based() {
                    ui.label("File path");
                    ui.add(TextEdit::singleline(&mut self.form.file_path).desired_width(360.0));
                    ui.end_row();
                } else {
                    ui.label("Host");
                    ui.add(TextEdit::singleline(&mut self.form.host).desired_width(260.0));
                    ui.end_row();

                    ui.label("Port");
                    ui.add(DragValue::new(&mut self.form.port).clamp_range(1..=65535));
                    ui.end_row();
                }

                if matches!(
                    self.form.kind,
                    DataSourceKind::MySql | DataSourceKind::PostgreSql
                ) {
                    ui.label("Username");
                    ui.add(TextEdit::singleline(&mut self.form.username).desired_width(260.0));
                    ui.end_row();
                }

                if !self.form.kind.is_file_based() {
                    ui.label("Password");
                    let response = ui.add(
                        TextEdit::singleline(&mut self.form.password)
                            .password(true)
                            .hint_text(if self.form.has_stored_password && !self.form.password_modified {
                                "Saved password"
                            } else {
                                ""
                            })
                            .desired_width(260.0),
                    );
                    if response.changed() {
                        self.form.password_modified = true;
                    }
                    ui.end_row();
                }

                if self.form.kind.is_relational() && !self.form.kind.is_file_based() {
                    ui.label("Database");
                    ui.add(TextEdit::singleline(&mut self.form.database).desired_width(260.0));
                    ui.end_row();
                }

                ui.label("SSL");
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

                ui.label("Options");
                ui.horizontal(|ui| {
                    ui.add(Checkbox::new(&mut self.form.read_only, "Read only"));
                    ui.add(Checkbox::new(&mut self.form.save_password, "Save password"));
                });
                ui.end_row();
            });

        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.busy, Button::new("Test Connection"))
                .clicked()
            {
                self.test_connection();
            }
            if ui.add_enabled(!self.busy, Button::new("Save")).clicked() {
                self.save_connection();
            }
            if as_window && ui.add_enabled(!self.busy, Button::new("Cancel")).clicked() {
                self.window_open = false;
                self.confirm_delete = None;
                self.error = None;
                self.test_report = None;
            }
            if let EditorMode::Edit(id) = self.form.mode {
                if ui.add_enabled(!self.busy, Button::new("Delete")).clicked() {
                    self.confirm_delete = Some(id);
                }
            }
        });

        if let Some(id) = self.confirm_delete {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, "Confirm delete connection?");
                if ui.button("Delete").clicked() {
                    self.delete_connection(id);
                    self.confirm_delete = None;
                }
                if ui.button("Cancel").clicked() {
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

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.busy = false;
            match event {
                ConnectionEvent::Loaded(result) => match result {
                    Ok(profiles) => {
                        self.profiles = profiles;
                        self.status = format!("Loaded {} connections", self.profiles.len());
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
