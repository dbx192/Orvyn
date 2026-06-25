use crate::ui::format::{copy_cell_value, display_cell_value};
use app_core::redis_service::{RedisKeyLoad, RedisService};
use db_common::{AppError, CellValue, ConnectionId, RedisKeyInfo, RedisValue};
use eframe::egui::{Color32, Context, Frame, Layout, RichText, ScrollArea, TextEdit, Ui, Window};
use std::sync::mpsc::{self, Receiver, Sender};
use tokio::runtime::Handle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Empty,
    KeyDetail,
}

impl Default for ViewMode {
    fn default() -> Self {
        Self::Empty
    }
}

enum RedisBrowserCommand {
    LoadKey {
        connection_id: ConnectionId,
        database: u8,
        key: String,
    },
    Refresh,
    SetString {
        connection_id: ConnectionId,
        database: u8,
        key: String,
        value: String,
        ttl_seconds: Option<u64>,
    },
    SetHashField {
        connection_id: ConnectionId,
        database: u8,
        key: String,
        field: String,
        value: String,
    },
    DeleteHashField {
        connection_id: ConnectionId,
        database: u8,
        key: String,
        field: String,
    },
    DeleteKey {
        connection_id: ConnectionId,
        database: u8,
        key: String,
    },
    UpdateTtl {
        connection_id: ConnectionId,
        database: u8,
        key: String,
        ttl_seconds: Option<i64>,
    },
    CreateKey {
        connection_id: ConnectionId,
        database: u8,
        key: String,
        key_type: String,
        initial_value: String,
        ttl_seconds: Option<u64>,
    },
}

enum RedisBrowserResponse {
    KeyLoaded(RedisKeyLoad),
    KeyDeleted {
        key: String,
    },
    Error(AppError),
    Status(String),
}

pub struct RedisKeyBrowser {
    redis_service: RedisService,
    runtime: Handle,
    tx: Sender<RedisBrowserResponse>,
    rx: Receiver<RedisBrowserResponse>,
    active_connection_id: Option<ConnectionId>,
    connection_label: String,
    database_index: u8,
    selected_key: Option<String>,
    selected_key_info: Option<RedisKeyInfo>,
    value: Option<RedisValue>,
    loading: bool,
    error: Option<String>,
    status: Option<String>,
    confirm_delete_key: bool,
    confirm_delete_hash_field: Option<String>,
    new_key_dialog: bool,
    new_key_name: String,
    new_key_type: String,
    new_key_value: String,
    new_key_ttl: String,
    ttl_edit: String,
    ttl_editing: bool,
    hash_edit_field: Option<(String, String)>,
    hash_new_field: String,
    hash_new_value: String,
    string_editing: bool,
    string_edit_value: String,
    view_mode: ViewMode,
}

impl RedisKeyBrowser {
    pub fn new(redis_service: RedisService, runtime: Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            redis_service,
            runtime,
            tx,
            rx,
            active_connection_id: None,
            connection_label: String::new(),
            database_index: 0,
            selected_key: None,
            selected_key_info: None,
            value: None,
            loading: false,
            error: None,
            status: None,
            confirm_delete_key: false,
            confirm_delete_hash_field: None,
            new_key_dialog: false,
            new_key_name: String::new(),
            new_key_type: "string".to_string(),
            new_key_value: String::new(),
            new_key_ttl: String::new(),
            ttl_edit: String::new(),
            ttl_editing: false,
            hash_edit_field: None,
            hash_new_field: String::new(),
            hash_new_value: String::new(),
            string_editing: false,
            string_edit_value: String::new(),
            view_mode: ViewMode::Empty,
        }
    }

    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) -> Option<String> {
        self.drain_events();

        match self.view_mode {
            ViewMode::Empty => self.empty_view(ui),
            ViewMode::KeyDetail => self.key_detail_view(ui),
        }

        self.new_key_dialog(ctx);
        self.delete_confirm_dialogs(ctx);
        self.status.take()
    }

    pub fn load_key(
        &mut self,
        connection_id: ConnectionId,
        database: u8,
        key: String,
    ) {
        self.active_connection_id = Some(connection_id);
        self.database_index = database;
        self.selected_key = Some(key.clone());
        self.value = None;
        self.error = None;
        self.loading = true;
        self.view_mode = ViewMode::KeyDetail;
        self.string_editing = false;
        self.hash_edit_field = None;
        self.ttl_editing = false;
        self.send_command(RedisBrowserCommand::LoadKey {
            connection_id,
            database,
            key,
        });
    }

    fn empty_view(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(
                RichText::new("Select a Redis key from the object tree")
                    .color(Color32::GRAY)
                    .italics(),
            );
            if let Some(error) = &self.error {
                ui.add_space(8.0);
                ui.colored_label(Color32::RED, error);
            }
        });
    }

    fn key_detail_view(&mut self, ui: &mut Ui) {
        self.key_header(ui);
        ui.separator();

        if self.loading {
            ui.label(RichText::new("Loading...").color(Color32::GRAY).italics());
            return;
        }

        if let Some(err) = &self.error {
            ui.colored_label(Color32::RED, err);
            return;
        }

        let Some(value) = self.value.clone() else {
            ui.label(RichText::new("Key not loaded").color(Color32::GRAY).italics());
            return;
        };

        match value {
            RedisValue::String(bytes) => self.render_string_view(ui, bytes),
            RedisValue::Hash(fields) => self.render_hash_view(ui, fields),
            RedisValue::List(items) => self.render_list_view(ui, items),
            RedisValue::Set(items) => self.render_set_view(ui, items),
            RedisValue::SortedSet(items) => self.render_zset_view(ui, items),
            RedisValue::None => {
                ui.label(RichText::new("Key does not exist").color(Color32::RED));
            }
            RedisValue::Stream(_) => {
                ui.label("Stream preview is not implemented");
            }
        }
    }

    fn key_header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui.button("Back").clicked() {
                self.view_mode = ViewMode::Empty;
            }
            ui.separator();
            if let Some(key_name) = &self.selected_key {
                ui.label(RichText::new(key_name).strong().monospace());
            }
            ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                if ui.button("Delete").clicked() {
                    self.confirm_delete_key = true;
                }
                if ui.button("Refresh").clicked() {
                    self.refresh_value();
                }
            });
        });

        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.connection_label).small().color(Color32::GRAY));
            ui.separator();
            ui.label(RichText::new(format!("DB {}", self.database_index)).small().color(Color32::GRAY));
            let key_info = self.selected_key_info.clone();
            if let Some(info) = &key_info {
                ui.separator();
                ui.label(format!("Type: {}", info.type_name));
                ui.separator();
                if let Some(ttl) = info.ttl_seconds {
                    self.ttl_display(ui, ttl);
                } else {
                    ui.label("TTL: persistent");
                    if ui.small_button("Set TTL").clicked() {
                        self.ttl_edit.clear();
                        self.ttl_editing = true;
                    }
                }
                if let Some(size) = info.size_hint {
                    ui.separator();
                    ui.label(format!("Size: {}", size));
                }
            }
        });
    }

    fn ttl_display(&mut self, ui: &mut Ui, ttl: i64) {
        if self.ttl_editing {
            ui.horizontal(|ui| {
                ui.label("TTL:");
                ui.add(TextEdit::singleline(&mut self.ttl_edit).desired_width(80.0));
                if ui.button("Save").clicked() {
                    self.apply_ttl_edit();
                }
                if ui.button("Persist").clicked() {
                    self.remove_ttl();
                }
                if ui.button("Cancel").clicked() {
                    self.ttl_editing = false;
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.label(format!("TTL: {}s", ttl));
                if ui.small_button("Edit").clicked() {
                    self.ttl_edit = ttl.to_string();
                    self.ttl_editing = true;
                }
                if ui.small_button("Persist").clicked() {
                    self.remove_ttl();
                }
            });
        }
    }

    fn render_string_view(&mut self, ui: &mut Ui, bytes: Vec<u8>) {
        ui.label(RichText::new("String").strong());
        ui.add_space(4.0);

        let text_value = String::from_utf8_lossy(&bytes).to_string();
        if self.string_editing {
            ui.add(
                TextEdit::multiline(&mut self.string_edit_value)
                    .desired_rows(10)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.save_string_value();
                }
                if ui.button("Cancel").clicked() {
                    self.string_editing = false;
                }
            });
        } else {
            Frame::group(ui.style()).show(ui, |ui| {
                ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    ui.label(RichText::new(&text_value).monospace());
                });
            });

            ui.horizontal(|ui| {
                if ui.button("Edit").clicked() {
                    self.string_edit_value = text_value.clone();
                    self.string_editing = true;
                }
                if ui.button("Copy").clicked() {
                    ui.output_mut(|o| o.copied_text = text_value.clone());
                }
            });
        }
    }

    fn render_hash_view(&mut self, ui: &mut Ui, fields: Vec<(String, CellValue)>) {
        ui.label(RichText::new(format!("Hash ({})", fields.len())).strong());
        ui.add_space(4.0);

        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (field, value) in &fields {
                    self.render_hash_row(ui, field, value);
                }
            });
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Field:");
            ui.add(TextEdit::singleline(&mut self.hash_new_field).desired_width(160.0));
            ui.label("Value:");
            ui.add(TextEdit::singleline(&mut self.hash_new_value).desired_width(220.0));
            if ui.button("Add").clicked() {
                self.add_hash_field();
            }
        });
    }

    fn render_hash_row(&mut self, ui: &mut Ui, field: &str, value: &CellValue) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(field).strong().monospace());
            ui.separator();

            let is_editing = self
                .hash_edit_field
                .as_ref()
                .map(|(current, _)| current == field)
                .unwrap_or(false);

            if is_editing {
                if let Some((_, edit_value)) = &mut self.hash_edit_field {
                    ui.add(TextEdit::singleline(edit_value).desired_width(280.0));
                }
                if ui.button("Save").clicked() {
                    self.save_hash_field_edit();
                }
                if ui.button("Cancel").clicked() {
                    self.hash_edit_field = None;
                }
            } else {
                let display = display_cell_value(value);
                ui.label(RichText::new(&display).monospace());
                ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                    if ui.small_button("Delete").clicked() {
                        self.confirm_delete_hash_field = Some(field.to_string());
                    }
                    if ui.small_button("Copy").clicked() {
                        ui.output_mut(|o| o.copied_text = copy_cell_value(value));
                    }
                    if ui.small_button("Edit").clicked() {
                        self.start_hash_edit(field, value);
                    }
                });
            }
        });
    }

    fn render_list_view(&mut self, ui: &mut Ui, items: Vec<CellValue>) {
        ui.label(RichText::new(format!("List ({})", items.len())).strong());
        ui.add_space(4.0);
        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (idx, value) in items.iter().enumerate().take(200) {
                    ui.label(format!("[{}] {}", idx, display_cell_value(value)));
                }
            });
        });
    }

    fn render_set_view(&mut self, ui: &mut Ui, items: Vec<CellValue>) {
        ui.label(RichText::new(format!("Set ({})", items.len())).strong());
        ui.add_space(4.0);
        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for value in items.iter().take(200) {
                    ui.label(display_cell_value(value));
                }
            });
        });
    }

    fn render_zset_view(&mut self, ui: &mut Ui, items: Vec<(CellValue, f64)>) {
        ui.label(RichText::new(format!("Sorted Set ({})", items.len())).strong());
        ui.add_space(4.0);
        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (member, score) in items.iter().take(200) {
                    ui.label(format!("{}  score={}", display_cell_value(member), score));
                }
            });
        });
    }

    fn refresh_value(&mut self) {
        let Some(connection_id) = self.active_connection_id else {
            return;
        };
        let Some(key) = self.selected_key.clone() else {
            return;
        };
        self.loading = true;
        self.error = None;
        self.send_command(RedisBrowserCommand::LoadKey {
            connection_id,
            database: self.database_index,
            key,
        });
    }

    fn save_string_value(&mut self) {
        let Some(connection_id) = self.active_connection_id else {
            return;
        };
        let Some(key) = self.selected_key.clone() else {
            return;
        };
        let ttl = self.selected_key_info.as_ref().and_then(|info| info.ttl_seconds).and_then(|ttl| {
            if ttl > 0 {
                Some(ttl as u64)
            } else {
                None
            }
        });
        self.loading = true;
        self.error = None;
        self.send_command(RedisBrowserCommand::SetString {
            connection_id,
            database: self.database_index,
            key,
            value: self.string_edit_value.clone(),
            ttl_seconds: ttl,
        });
    }

    fn start_hash_edit(&mut self, field: &str, value: &CellValue) {
        self.hash_edit_field = Some((field.to_string(), copy_cell_value(value)));
    }

    fn save_hash_field_edit(&mut self) {
        let Some(connection_id) = self.active_connection_id else {
            return;
        };
        let Some(key) = self.selected_key.clone() else {
            return;
        };
        let Some((field, value)) = self.hash_edit_field.clone() else {
            return;
        };
        self.loading = true;
        self.error = None;
        self.send_command(RedisBrowserCommand::SetHashField {
            connection_id,
            database: self.database_index,
            key,
            field,
            value,
        });
    }

    fn add_hash_field(&mut self) {
        if self.hash_new_field.trim().is_empty() {
            self.error = Some("Hash field name cannot be empty".to_string());
            return;
        }
        let Some(connection_id) = self.active_connection_id else {
            return;
        };
        let Some(key) = self.selected_key.clone() else {
            return;
        };
        self.loading = true;
        self.error = None;
        self.send_command(RedisBrowserCommand::SetHashField {
            connection_id,
            database: self.database_index,
            key,
            field: self.hash_new_field.trim().to_string(),
            value: self.hash_new_value.clone(),
        });
    }

    fn apply_ttl_edit(&mut self) {
        let Some(connection_id) = self.active_connection_id else {
            return;
        };
        let Some(key) = self.selected_key.clone() else {
            return;
        };
        let ttl = match self.ttl_edit.trim().parse::<i64>() {
            Ok(value) if value > 0 => value,
            _ => {
                self.error = Some("TTL must be a positive integer".to_string());
                return;
            }
        };
        self.loading = true;
        self.error = None;
        self.send_command(RedisBrowserCommand::UpdateTtl {
            connection_id,
            database: self.database_index,
            key,
            ttl_seconds: Some(ttl),
        });
    }

    fn remove_ttl(&mut self) {
        let Some(connection_id) = self.active_connection_id else {
            return;
        };
        let Some(key) = self.selected_key.clone() else {
            return;
        };
        self.loading = true;
        self.error = None;
        self.send_command(RedisBrowserCommand::UpdateTtl {
            connection_id,
            database: self.database_index,
            key,
            ttl_seconds: None,
        });
    }

    fn create_new_key(&mut self) {
        if self.new_key_name.trim().is_empty() {
            self.error = Some("Key name cannot be empty".to_string());
            return;
        }
        let Some(connection_id) = self.active_connection_id else {
            self.error = Some("Select a Redis connection first".to_string());
            return;
        };
        let ttl_seconds = parse_optional_u64(&self.new_key_ttl).map_err(|msg| {
            self.error = Some(msg);
        });
        if ttl_seconds.is_err() {
            return;
        }
        self.loading = true;
        self.error = None;
        self.send_command(RedisBrowserCommand::CreateKey {
            connection_id,
            database: self.database_index,
            key: self.new_key_name.trim().to_string(),
            key_type: self.new_key_type.clone(),
            initial_value: self.new_key_value.clone(),
            ttl_seconds: ttl_seconds.ok().flatten(),
        });
    }

    fn send_command(&self, command: RedisBrowserCommand) {
        let service = self.redis_service.clone();
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let response = match command {
                RedisBrowserCommand::LoadKey {
                    connection_id,
                    database,
                    key,
                } => match service.load_key(connection_id, database, &key).await {
                    Ok(load) => RedisBrowserResponse::KeyLoaded(load),
                    Err(error) => RedisBrowserResponse::Error(error),
                },
                RedisBrowserCommand::Refresh => {
                    RedisBrowserResponse::Status("Refreshed".to_string())
                }
                RedisBrowserCommand::SetString {
                    connection_id,
                    database,
                    key,
                    value,
                    ttl_seconds,
                } => match service
                    .set_string(connection_id, database, &key, &value, ttl_seconds)
                    .await
                {
                    Ok(()) => match service.load_key(connection_id, database, &key).await {
                        Ok(load) => RedisBrowserResponse::KeyLoaded(load),
                        Err(error) => RedisBrowserResponse::Error(error),
                    },
                    Err(error) => RedisBrowserResponse::Error(error),
                },
                RedisBrowserCommand::SetHashField {
                    connection_id,
                    database,
                    key,
                    field,
                    value,
                } => match service.hset(connection_id, database, &key, &field, &value).await {
                    Ok(_) => match service.load_key(connection_id, database, &key).await {
                        Ok(load) => RedisBrowserResponse::KeyLoaded(load),
                        Err(error) => RedisBrowserResponse::Error(error),
                    },
                    Err(error) => RedisBrowserResponse::Error(error),
                },
                RedisBrowserCommand::DeleteHashField {
                    connection_id,
                    database,
                    key,
                    field,
                } => match service.hdel(connection_id, database, &key, &field).await {
                    Ok(_) => match service.load_key(connection_id, database, &key).await {
                        Ok(load) => RedisBrowserResponse::KeyLoaded(load),
                        Err(error) => RedisBrowserResponse::Error(error),
                    },
                    Err(error) => RedisBrowserResponse::Error(error),
                },
                RedisBrowserCommand::DeleteKey {
                    connection_id,
                    database,
                    key,
                } => match service.delete_key(connection_id, database, &key).await {
                    Ok(()) => RedisBrowserResponse::KeyDeleted { key },
                    Err(error) => RedisBrowserResponse::Error(error),
                },
                RedisBrowserCommand::UpdateTtl {
                    connection_id,
                    database,
                    key,
                    ttl_seconds,
                } => match service.update_ttl(connection_id, database, &key, ttl_seconds).await {
                    Ok(()) => match service.load_key(connection_id, database, &key).await {
                        Ok(load) => RedisBrowserResponse::KeyLoaded(load),
                        Err(error) => RedisBrowserResponse::Error(error),
                    },
                    Err(error) => RedisBrowserResponse::Error(error),
                },
                RedisBrowserCommand::CreateKey {
                    connection_id,
                    database,
                    key,
                    key_type,
                    initial_value,
                    ttl_seconds,
                } => match service
                    .create_key(
                        connection_id,
                        database,
                        &key,
                        &key_type,
                        &initial_value,
                        ttl_seconds,
                    )
                    .await
                {
                    Ok(()) => match service.load_key(connection_id, database, &key).await {
                        Ok(load) => RedisBrowserResponse::KeyLoaded(load),
                        Err(error) => RedisBrowserResponse::Error(error),
                    },
                    Err(error) => RedisBrowserResponse::Error(error),
                },
            };
            let _ = tx.send(response);
        });
    }

    fn drain_events(&mut self) {
        while let Ok(response) = self.rx.try_recv() {
            match response {
                RedisBrowserResponse::KeyLoaded(load) => {
                    self.set_loaded_key(load);
                }
                RedisBrowserResponse::KeyDeleted { key } => {
                    self.loading = false;
                    self.confirm_delete_key = false;
                    self.status = Some(format!("Deleted key {}", key));
                    if self.selected_key.as_deref() == Some(key.as_str()) {
                        self.selected_key = None;
                        self.selected_key_info = None;
                        self.value = None;
                        self.view_mode = ViewMode::Empty;
                    }
                }
                RedisBrowserResponse::Error(error) => {
                    self.loading = false;
                    self.error = Some(error.message);
                }
                RedisBrowserResponse::Status(status) => {
                    self.status = Some(status);
                }
            }
        }
    }

    fn set_loaded_key(&mut self, load: RedisKeyLoad) {
        self.connection_label = load.connection_label;
        self.database_index = load.database;
        self.selected_key = Some(load.info.name.clone());
        self.selected_key_info = Some(load.info);
        self.value = Some(load.value.clone());
        self.loading = false;
        self.error = None;
        self.view_mode = ViewMode::KeyDetail;
        self.string_editing = false;
        self.hash_edit_field = None;
        self.ttl_editing = false;
        self.hash_new_field.clear();
        self.hash_new_value.clear();
        self.new_key_dialog = false;
        self.status = Some("Redis key loaded".to_string());
    }

    fn new_key_dialog(&mut self, ctx: &Context) {
        if !self.new_key_dialog {
            return;
        }

        let mut open = self.new_key_dialog;
        Window::new("New Redis Key")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Key:");
                    ui.add(TextEdit::singleline(&mut self.new_key_name).desired_width(240.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Type:");
                    eframe::egui::ComboBox::from_id_source("redis_new_key_type")
                        .selected_text(&self.new_key_type)
                        .show_ui(ui, |ui| {
                            for key_type in ["string", "hash", "list", "set", "zset"] {
                                ui.selectable_value(
                                    &mut self.new_key_type,
                                    key_type.to_string(),
                                    key_type,
                                );
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Value:");
                    ui.add(TextEdit::singleline(&mut self.new_key_value).desired_width(240.0));
                });
                ui.horizontal(|ui| {
                    ui.label("TTL:");
                    ui.add(TextEdit::singleline(&mut self.new_key_ttl).desired_width(100.0));
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        self.create_new_key();
                    }
                    if ui.button("Cancel").clicked() {
                        self.new_key_dialog = false;
                    }
                });
            });
        self.new_key_dialog = open;
    }

    fn delete_confirm_dialogs(&mut self, ctx: &Context) {
        if self.confirm_delete_key {
            let selected_key = self.selected_key.clone();
            let mut open = self.confirm_delete_key;
            Window::new("Confirm Delete")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    if let Some(key) = &selected_key {
                        ui.colored_label(Color32::RED, format!("Delete key \"{}\"?", key));
                        ui.label("This action cannot be undone.");
                        ui.horizontal(|ui| {
                            if ui.button("Delete").clicked() {
                                if let Some(connection_id) = self.active_connection_id {
                                    self.loading = true;
                                    self.error = None;
                                    self.send_command(RedisBrowserCommand::DeleteKey {
                                        connection_id,
                                        database: self.database_index,
                                        key: key.clone(),
                                    });
                                }
                            }
                            if ui.button("Cancel").clicked() {
                                self.confirm_delete_key = false;
                            }
                        });
                    }
                });
            self.confirm_delete_key = open;
        }

        if let Some(field) = self.confirm_delete_hash_field.clone() {
            let selected_key = self.selected_key.clone();
            let mut open = true;
            Window::new("Delete Hash Field")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.colored_label(Color32::RED, format!("Delete field \"{}\"?", field));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            if let (Some(connection_id), Some(key)) =
                                (self.active_connection_id, selected_key.clone())
                            {
                                self.loading = true;
                                self.error = None;
                                self.send_command(RedisBrowserCommand::DeleteHashField {
                                    connection_id,
                                    database: self.database_index,
                                    key,
                                    field: field.clone(),
                                });
                                self.confirm_delete_hash_field = None;
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete_hash_field = None;
                        }
                    });
                });
            if !open {
                self.confirm_delete_hash_field = None;
            }
        }
    }
}

fn parse_optional_u64(value: &str) -> Result<Option<u64>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| "TTL must be a positive integer".to_string())
}
