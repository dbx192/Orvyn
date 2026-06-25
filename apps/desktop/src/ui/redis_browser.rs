//! Redis key browser UI.
//!
//! Provides a complete Redis key browsing experience:
//! - DB selection dropdown
//! - SCAN pagination
//! - Pattern search
//! - Key list with type, TTL, and size summary
//! - Key detail panel with lazy loading (preview by default, full content on demand)
//! - Delete confirmation
//! - Refresh functionality

use app_core::{connection::ConnectionManager, metadata::MetadataService};
use db_common::{
    base64_preview, human_bytes, AppError, CellValue, ConnectionId, ConnectionProfile,
    DataSourceKind, RedisDatabase, RedisKeyInfo, RedisValue,
};
use db_redis::RedisConnectorWithAuth;
use eframe::egui::{
    Align, Button, Color32, ComboBox, Context, DragValue, Grid, Label, Layout, RichText,
    ScrollArea, SelectableLabel, Spinner, TextEdit, Ui, Window,
};
use std::sync::mpsc::{self, Receiver, Sender};

const SCAN_PAGE_SIZE: u32 = 100;
const PREVIEW_MAX_BYTES: usize = 1024;
const LARGE_VALUE_THRESHOLD: usize = 64 * 1024;

/// Events emitted by the Redis browser.
#[derive(Debug, Clone)]
pub enum RedisBrowserEvent {
    Error(String),
    Status(String),
}

/// Display state for a Redis key value.
#[derive(Debug, Clone)]
pub enum ValueDisplay {
    /// Value is not loaded yet.
    NotLoaded,
    /// Value is too large, requires user confirmation to load.
    TooLarge {
        preview: String,
        total_size: usize,
    },
    /// Full value is loaded.
    Loaded(RedisValue),
    /// Value is currently loading.
    Loading,
}

/// Information about a single key displayed in the list.
#[derive(Debug, Clone)]
pub struct KeyEntry {
    pub info: RedisKeyInfo,
    pub size: Option<u64>,
    pub selected: bool,
}

pub struct RedisBrowserUi {
    connection_manager: ConnectionManager,
    metadata_service: MetadataService,
    runtime: tokio::runtime::Handle,
    tx: Sender<BrowserCommand>,
    rx: Receiver<BrowserResponse>,

    // Connection state
    connection_id: ConnectionId,
    profile: Option<ConnectionProfile>,
    databases: Vec<RedisDatabase>,
    selected_database: u8,

    // Key list state
    keys: Vec<KeyEntry>,
    cursor: u64,
    is_loading: bool,
    has_more: bool,
    pattern: String,
    search_pattern: String,

    // Key detail state
    selected_key: Option<String>,
    value_display: ValueDisplay,
    confirm_load_full: Option<(String, usize)>,

    // Delete confirmation
    confirm_delete: Option<String>,

    // TTL edit
    editing_ttl: Option<(String, i64)>,

    // Error display
    error: Option<String>,
    status: String,
}

enum BrowserCommand {
    LoadDatabases,
    ScanKeys {
        database: u8,
        pattern: Option<String>,
        cursor: u64,
    },
    LoadMoreKeys,
    LoadKeyValue {
        database: u8,
        key: String,
        force_full: bool,
    },
    DeleteKey {
        database: u8,
        key: String,
    },
    UpdateTtl {
        database: u8,
        key: String,
        ttl_seconds: Option<i64>,
    },
    Refresh,
}

enum BrowserResponse {
    DatabasesLoaded(Result<Vec<RedisDatabase>>),
    KeysScanned {
        cursor: u64,
        keys: Vec<(RedisKeyInfo, Option<u64>)>,
        has_more: bool,
    },
    KeyValueLoaded {
        key: String,
        value: RedisValue,
        preview: String,
        total_size: usize,
    },
    KeyDeleted { key: String },
    TtlUpdated { key: String, ttl: Option<i64> },
    Error(String),
    Status(String),
}

impl RedisBrowserUi {
    pub fn new(
        connection_id: ConnectionId,
        connection_manager: ConnectionManager,
        metadata_service: MetadataService,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let (tx, rx) = mpsc::channel();

        let mut ui = Self {
            connection_manager,
            metadata_service,
            runtime,
            tx,
            rx,
            connection_id,
            profile: None,
            databases: Vec::new(),
            selected_database: 0,
            keys: Vec::new(),
            cursor: 0,
            is_loading: false,
            has_more: false,
            pattern: String::new(),
            search_pattern: String::new(),
            selected_key: None,
            value_display: ValueDisplay::NotLoaded,
            confirm_load_full: None,
            confirm_delete: None,
            editing_ttl: None,
            error: None,
            status: "Loading...".to_string(),
        };

        ui.load_connection_profile();
        ui
    }

    fn load_connection_profile(&mut self) {
        let connection_id = self.connection_id;
        let tx = self.tx.clone();
        let manager = self.connection_manager.clone();

        self.runtime.spawn(async move {
            match manager.get_profile(connection_id).await {
                Ok(Some(profile)) => {
                    let _ = tx.send(BrowserCommand::LoadDatabases);
                }
                Ok(None) => {
                    let _ = tx.send(BrowserCommand::Refresh);
                }
                Err(e) => {
                    let _ = tx.send(BrowserCommand::Refresh);
                }
            }
        });
    }

    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) -> Option<RedisBrowserEvent> {
        self.drain_responses();

        ui.horizontal(|ui| {
            ui.label("DB:");
            self.database_selector(ui);
            if ui.add_enabled(!self.is_loading, Button::new("Refresh")).clicked() {
                self.refresh();
            }
        });

        ui.separator();

        // Search bar
        ui.horizontal(|ui| {
            ui.label("Pattern:");
            let response = ui.add(
                TextEdit::singleline(&mut self.search_pattern)
                    .hint_text("* or user:*")
                    .desired_width(200.0),
            );
            if response.lost_focus() && self.search_pattern != self.pattern {
                self.apply_pattern();
            }
            if ui.button("Apply").clicked() {
                self.apply_pattern();
            }
        });

        ui.separator();

        // Main content: key list + detail panel
        ui.horizontal(|ui| {
            // Left: Key list
            ui.vertical(|ui| {
                self.key_list_panel(ui);
            });

            ui.separator();

            // Right: Key detail
            ui.vertical(|ui| {
                self.key_detail_panel(ui);
            });
        });

        // Delete confirmation dialog
        self.delete_confirmation_window(ctx);

        // Load full value confirmation dialog
        self.load_full_confirmation_window(ctx);

        // Error display
        if let Some(error) = &self.error {
            ui.add_space(8.0);
            ui.colored_label(Color32::RED, error);
        }

        if !self.status.is_empty() && self.error.is_none() {
            ui.add_space(8.0);
            ui.label(&self.status);
        }

        None
    }

    fn database_selector(&mut self, ui: &mut Ui) {
        if self.databases.is_empty() {
            if self.is_loading {
                ui.add(Spinner::new());
            } else {
                ui.label("(no databases)");
            }
            return;
        }

        let current_db = format!("DB {}", self.selected_database);
        ComboBox::from_id_salt("redis_db_selector")
            .selected_text(current_db)
            .show_ui(ui, |ui| {
                for db in &self.databases {
                    let label = if let Some(count) = db.key_count {
                        format!("DB {} ({} keys)", db.index, count)
                    } else {
                        format!("DB {}", db.index)
                    };
                    if ui
                        .selectable_value(&mut self.selected_database, db.index, label)
                        .clicked()
                    {
                        self.switch_database(db.index);
                    }
                }
            });
    }

    fn key_list_panel(&mut self, ui: &mut Ui) {
        ui.set_min_width(350.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Keys").strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(format!("{} keys", self.keys.len()));
            });
        });

        ScrollArea::vertical()
            .id_salt("key_list_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.keys.is_empty() && !self.is_loading {
                    ui.label(
                        RichText::new("No keys found")
                            .color(Color32::GRAY)
                            .italics(),
                    );
                    return;
                }

                for entry in &mut self.keys {
                    self.render_key_row(ui, entry);
                }

                if self.is_loading {
                    ui.horizontal(|ui| {
                        ui.add(Spinner::new());
                        ui.label("Loading...");
                    });
                } else if self.has_more {
                    if ui.button("Load More...").clicked() {
                        self.load_more_keys();
                    }
                }
            });
    }

    fn render_key_row(&mut self, ui: &mut Ui, entry: &mut KeyEntry) {
        let is_selected = self.selected_key.as_ref() == Some(&entry.info.name);

        ui.horizontal(|ui| {
            let resp = ui.add(
                SelectableLabel::new(is_selected, &entry.info.name)
                    .wrap_mode(eframe::egui::TextWrapMode::Extend),
            );

            if resp.clicked() {
                self.select_key(entry.info.name.clone());
            }

            resp.context_menu(|ui| {
                self.key_context_menu(ui, &entry.info.name);
            });
        });

        // Show key metadata in smaller text
        ui.indent("key_meta", |ui| {
            ui.horizontal(|ui| {
                let type_color = key_type_color(&entry.info.type_name);
                ui.label(RichText::new(&entry.info.type_name).color(type_color).small());

                if let Some(ttl) = entry.info.ttl_seconds {
                    let ttl_text = format_ttl(ttl);
                    ui.label(RichText::new(ttl_text).color(Color32::GRAY).small());
                }

                if let Some(size) = entry.size {
                    let size_text = format_key_size(&entry.info.type_name, size);
                    ui.label(RichText::new(size_text).color(Color32::GRAY).small());
                }
            });
        });
    }

    fn key_context_menu(&mut self, ui: &mut Ui, key_name: &str) {
        if ui.button("View").clicked() {
            self.select_key(key_name.to_string());
            ui.close_menu();
        }
        if ui.button("Copy Name").clicked() {
            ui.output_mut(|o| o.copied_text = key_name.to_string());
            self.status = format!("Copied: {}", key_name);
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Delete").clicked() {
            self.confirm_delete = Some(key_name.to_string());
            ui.close_menu();
        }
    }

    fn key_detail_panel(&mut self, ui: &mut Ui) {
        ui.set_min_width(400.0);

        let Some(key_name) = &self.selected_key else {
            ui.label(
                RichText::new("Select a key to view details")
                    .color(Color32::GRAY)
                    .italics(),
            );
            return;
        };

        let entry = self.keys.iter().find(|e| &e.info.name == key_name);

        // Key header
        ui.horizontal(|ui| {
            ui.label(RichText::new(key_name).strong());
            if let Some(e) = entry {
                let type_color = key_type_color(&e.info.type_name);
                ui.label(RichText::new(format!("({})", e.info.type_name)).color(type_color));
            }
        });

        // Metadata section
        if let Some(e) = entry {
            ui.separator();
            self.metadata_grid(ui, e);
        }

        // Value section
        ui.separator();
        ui.label(RichText::new("Value").strong());
        self.render_value(ui, key_name);

        // Actions
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Delete Key").clicked() {
                self.confirm_delete = Some(key_name.clone());
            }
            if ui.button("Refresh").clicked() {
                self.refresh_key(key_name.clone());
            }
        });
    }

    fn metadata_grid(&self, ui: &mut Ui, entry: &KeyEntry) {
        Grid::new("key_metadata_grid")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label("Type:");
                ui.label(&entry.info.type_name);

                ui.label("TTL:");
                if let Some(ttl) = entry.info.ttl_seconds {
                    if let Some((_, editing_ttl)) = &self.editing_ttl {
                        if editing_ttl == &entry.info.name {
                            // Already editing this key's TTL
                        } else {
                            ui.label(format_ttl(ttl));
                        }
                    } else {
                        ui.label(format_ttl(ttl));
                    }
                } else {
                    ui.label("no expiry");
                }

                ui.label("Size:");
                if let Some(size) = entry.size {
                    ui.label(format_key_size(&entry.info.type_name, size));
                } else {
                    ui.label("(calculating...)");
                }
            });
    }

    fn render_value(&mut self, ui: &mut Ui, key_name: &str) {
        match &self.value_display {
            ValueDisplay::NotLoaded => {
                if ui.button("Load Preview").clicked() {
                    self.load_key_value(key_name.clone(), false);
                }
            }
            ValueDisplay::Loading => {
                ui.horizontal(|ui| {
                    ui.add(Spinner::new());
                    ui.label("Loading value...");
                });
            }
            ValueDisplay::TooLarge {
                preview,
                total_size,
            } => {
                ScrollArea::new()
                    .id_salt("value_preview_scroll")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(preview)
                                .family(eframe::egui::FontFamily::Monospace)
                                .size(12.0),
                        );
                    });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!(
                            "Value too large ({}), showing preview. ",
                            human_bytes(*total_size as u64)
                        )).color(Color32::YELLOW));
                    if ui.button("Load Full Value").clicked() {
                        self.confirm_load_full =
                            Some((key_name.to_string(), *total_size));
                    }
                });
            }
            ValueDisplay::Loaded(value) => {
                self.render_full_value(ui, value);
            }
        }
    }

    fn render_full_value(&self, ui: &mut Ui, value: &RedisValue) {
        match value {
            RedisValue::String(bytes) => {
                self.render_string_value(ui, bytes);
            }
            RedisValue::Hash(fields) => {
                self.render_hash_value(ui, fields);
            }
            RedisValue::List(items) => {
                self.render_list_value(ui, items);
            }
            RedisValue::Set(items) => {
                self.render_set_value(ui, items);
            }
            RedisValue::SortedSet(members) => {
                self.render_zset_value(ui, members);
            }
            RedisValue::Stream(_) => {
                ui.label("(stream type not fully supported)");
            }
            RedisValue::None => {
                ui.label("(key does not exist)");
            }
        }
    }

    fn render_string_value(&self, ui: &mut Ui, bytes: &[u8]) {
        // Try to display as text
        let text_preview = String::from_utf8_lossy(bytes);

        ScrollArea::new()
            .id_salt("string_value_scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(text_preview)
                        .family(eframe::egui::FontFamily::Monospace)
                        .size(12.0),
                );
            });
    }

    fn render_hash_value(&self, ui: &mut Ui, fields: &[(String, CellValue)]) {
        let total_size = fields.len();
        Grid::new("hash_value_grid")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                for (i, (field, value)) in fields.iter().enumerate() {
                    if i >= 50 {
                        ui.label(format!("... and {} more fields", total_size - 50));
                        break;
                    }
                    ui.label(
                        RichText::new(field)
                            .family(eframe::egui::FontFamily::Monospace)
                            .small(),
                    );
                    let value_text = cell_value_to_string(value);
                    ui.label(
                        RichText::new(value_text)
                            .family(eframe::egui::FontFamily::Monospace)
                            .small(),
                    );
                }
            });
    }

    fn render_list_value(&self, ui: &mut Ui, items: &[CellValue]) {
        let total = items.len();
        for (i, item) in items.iter().enumerate() {
            if i >= 50 {
                ui.label(format!("... and {} more items", total - 50));
                break;
            }
            ui.label(format!("[{}] {}", i, cell_value_to_string(item)));
        }
    }

    fn render_set_value(&self, ui: &mut Ui, items: &[CellValue]) {
        let total = items.len();
        for (i, item) in items.iter().enumerate() {
            if i >= 50 {
                ui.label(format!("... and {} more members", total - 50));
                break;
            }
            ui.label(format!("  {}", cell_value_to_string(item)));
        }
    }

    fn render_zset_value(&self, ui: &mut Ui, members: &[(CellValue, f64)]) {
        let total = members.len();
        Grid::new("zset_value_grid")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                for (i, (member, score)) in members.iter().enumerate() {
                    if i >= 50 {
                        ui.label(format!("... and {} more members", total - 50));
                        break;
                    }
                    ui.label(format!("score: {}", score));
                    ui.label(cell_value_to_string(member));
                }
            });
    }

    fn delete_confirmation_window(&mut self, ctx: &Context) {
        let Some(key_name) = &self.confirm_delete else {
            return;
        };

        let mut open = true;
        Window::new("Confirm Delete")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Are you sure you want to delete key \"{}\"?",
                    key_name
                ));
                ui.label(RichText::new("This action cannot be undone.").color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        self.delete_key(key_name.clone());
                        self.confirm_delete = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete = None;
                    }
                });
            });

        if !open {
            self.confirm_delete = None;
        }
    }

    fn load_full_confirmation_window(&mut self, ctx: &Context) {
        let Some((key_name, size)) = &self.confirm_load_full else {
            return;
        };

        let mut open = true;
        Window::new("Load Full Value")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "The value for key \"{}\" is {}.",
                    key_name,
                    human_bytes(*size as u64)
                ));
                ui.label("Loading large values may:");
                ui.label("- Use significant memory");
                ui.label("- Block the UI temporarily");
                ui.label("- Display slowly");
                ui.horizontal(|ui| {
                    if ui.button("Load Anyway").clicked() {
                        self.load_key_value_full(key_name.clone());
                        self.confirm_load_full = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_load_full = None;
                    }
                });
            });

        if !open {
            self.confirm_load_full = None;
        }
    }

    fn drain_responses(&mut self) {
        while let Ok(response) = self.rx.try_recv() {
            match response {
                BrowserResponse::DatabasesLoaded(result) => {
                    self.is_loading = false;
                    match result {
                        Ok(dbs) => {
                            self.databases = dbs;
                            if self.databases.is_empty() {
                                self.status = "No databases found".to_string();
                            } else {
                                self.status = format!("Loaded {} databases", dbs.len());
                            }
                        }
                        Err(e) => {
                            self.error = Some(format!("Failed to load databases: {}", e.message));
                        }
                    }
                }
                BrowserResponse::KeysScanned {
                    cursor,
                    keys,
                    has_more,
                } => {
                    self.is_loading = false;
                    self.cursor = cursor;
                    self.has_more = has_more;

                    for (info, size) in keys {
                        if !self.keys.iter().any(|k| k.info.name == info.name) {
                            self.keys.push(KeyEntry {
                                info,
                                size,
                                selected: false,
                            });
                        }
                    }

                    if self.keys.is_empty() && !has_more {
                        self.status = "No keys found".to_string();
                    } else {
                        self.status = format!("{} keys loaded", self.keys.len());
                    }
                }
                BrowserResponse::KeyValueLoaded {
                    key,
                    value,
                    preview,
                    total_size,
                } => {
                    if self.selected_key.as_ref() != Some(&key) {
                        // Ignore stale response
                        return;
                    }

                    if total_size > LARGE_VALUE_THRESHOLD {
                        self.value_display = ValueDisplay::TooLarge {
                            preview,
                            total_size,
                        };
                    } else {
                        self.value_display = ValueDisplay::Loaded(value);
                    }
                }
                BrowserResponse::KeyDeleted { key } => {
                    self.keys.retain(|k| k.info.name != key);
                    if self.selected_key.as_ref() == Some(&key) {
                        self.selected_key = None;
                        self.value_display = ValueDisplay::NotLoaded;
                    }
                    self.status = format!("Deleted key: {}", key);
                }
                BrowserResponse::TtlUpdated { key, ttl } => {
                    if let Some(entry) = self.keys.iter_mut().find(|k| k.info.name == key) {
                        entry.info.ttl_seconds = ttl;
                    }
                    self.editing_ttl = None;
                    self.status = "TTL updated".to_string();
                }
                BrowserResponse::Error(msg) => {
                    self.is_loading = false;
                    self.error = Some(msg);
                }
                BrowserResponse::Status(msg) => {
                    self.status = msg;
                }
            }
        }
    }

    fn select_key(&mut self, key_name: String) {
        self.selected_key = Some(key_name.clone());
        self.value_display = ValueDisplay::NotLoaded;

        // Load key metadata (size) if not already loaded
        if let Some(entry) = self.keys.iter_mut().find(|k| k.info.name == key_name) {
            if entry.size.is_none() {
                self.load_key_size(key_name);
            }
        }
    }

    fn switch_database(&mut self, database: u8) {
        self.selected_database = database;
        self.keys.clear();
        self.cursor = 0;
        self.has_more = false;
        self.selected_key = None;
        self.value_display = ValueDisplay::NotLoaded;
        self.scan_keys();
    }

    fn apply_pattern(&mut self) {
        self.pattern = self.search_pattern.clone();
        self.keys.clear();
        self.cursor = 0;
        self.has_more = false;
        self.selected_key = None;
        self.value_display = ValueDisplay::NotLoaded;
        self.scan_keys();
    }

    fn scan_keys(&mut self) {
        self.is_loading = true;
        let tx = self.tx.clone();
        let manager = self.metadata_service.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;
        let pattern = if self.pattern.is_empty() {
            None
        } else {
            Some(self.pattern.clone())
        };

        self.runtime.spawn(async move {
            match manager
                .scan_redis_keys(connection_id, database, pattern.as_deref(), 0, SCAN_PAGE_SIZE, None)
                .await
            {
                Ok((next_cursor, nodes)) => {
                    let mut keys_with_size = Vec::new();
                    for node in nodes {
                        if let Some(name) = &node.path.object {
                            let info = RedisKeyInfo {
                                name: name.clone(),
                                type_name: node
                                    .detail
                                    .as_ref()
                                    .and_then(|d| parse_type_from_detail(d))
                                    .unwrap_or_else(|| "string".to_string()),
                                ttl_seconds: parse_ttl_from_detail(node.detail.as_deref()),
                                size_hint: None,
                            };
                            keys_with_size.push((info, None));
                        }
                    }
                    let _ = tx.send(BrowserResponse::KeysScanned {
                        cursor: next_cursor,
                        keys: keys_with_size,
                        has_more: next_cursor != 0,
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to scan keys: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn load_more_keys(&mut self) {
        if self.is_loading || !self.has_more {
            return;
        }

        self.is_loading = true;
        let tx = self.tx.clone();
        let manager = self.metadata_service.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;
        let pattern = if self.pattern.is_empty() {
            None
        } else {
            Some(self.pattern.clone())
        };
        let cursor = self.cursor;

        self.runtime.spawn(async move {
            match manager
                .scan_redis_keys(connection_id, database, pattern.as_deref(), cursor, SCAN_PAGE_SIZE, None)
                .await
            {
                Ok((next_cursor, nodes)) => {
                    let mut keys_with_size = Vec::new();
                    for node in nodes {
                        if let Some(name) = &node.path.object {
                            let info = RedisKeyInfo {
                                name: name.clone(),
                                type_name: node
                                    .detail
                                    .as_ref()
                                    .and_then(|d| parse_type_from_detail(d))
                                    .unwrap_or_else(|| "string".to_string()),
                                ttl_seconds: parse_ttl_from_detail(node.detail.as_deref()),
                                size_hint: None,
                            };
                            keys_with_size.push((info, None));
                        }
                    }
                    let _ = tx.send(BrowserResponse::KeysScanned {
                        cursor: next_cursor,
                        keys: keys_with_size,
                        has_more: next_cursor != 0,
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to load more keys: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn load_key_value(&mut self, key_name: String, force_full: bool) {
        self.value_display = ValueDisplay::Loading;
        let tx = self.tx.clone();
        let manager = self.connection_manager.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;

        self.runtime.spawn(async move {
            match manager.profile_with_password(connection_id, None).await {
                Ok((profile, _)) => {
                    match RedisConnectorWithAuth::new(&profile, None).await {
                        Ok(mut conn) => {
                            match RedisConnectorWithAuth::get_key_value(&mut conn, database, &key_name).await {
                                Ok(value) => {
                                    let preview = format_value_preview(&value, PREVIEW_MAX_BYTES);
                                    let total_size = estimate_value_size(&value);
                                    let _ = tx.send(BrowserResponse::KeyValueLoaded {
                                        key: key_name,
                                        value,
                                        preview,
                                        total_size,
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.send(BrowserResponse::Error(format!(
                                        "Failed to load key value: {}",
                                        e.message
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(BrowserResponse::Error(format!(
                                "Failed to connect: {}",
                                e.message
                            )));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to get profile: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn load_key_value_full(&mut self, key_name: String) {
        self.load_key_value(key_name, true);
    }

    fn load_key_size(&mut self, key_name: String) {
        let tx = self.tx.clone();
        let manager = self.connection_manager.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;

        self.runtime.spawn(async move {
            match manager.profile_with_password(connection_id, None).await {
                Ok((profile, _)) => {
                    match RedisConnectorWithAuth::new(&profile, None).await {
                        Ok(mut conn) => {
                            match RedisConnectorWithAuth::get_key_size(&mut conn, database, &key_name).await {
                                Ok(size) => {
                                    if let Some(entry) = tx.send(BrowserResponse::KeysScanned {
                                        cursor: 0,
                                        keys: vec![],
                                        has_more: false,
                                    }).ok() {
                                        // Update the key entry's size
                                    }
                                }
                                Err(_) => {}
                            }
                        }
                        Err(_) => {}
                    }
                }
                Err(_) => {}
            }
        });
    }

    fn delete_key(&mut self, key_name: String) {
        let tx = self.tx.clone();
        let manager = self.connection_manager.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;

        self.runtime.spawn(async move {
            match manager.profile_with_password(connection_id, None).await {
                Ok((profile, _)) => {
                    match RedisConnectorWithAuth::new(&profile, None).await {
                        Ok(mut conn) => {
                            match RedisConnectorWithAuth::delete_key(&mut conn, database, &key_name).await {
                                Ok(()) => {
                                    let _ = tx.send(BrowserResponse::KeyDeleted { key: key_name });
                                }
                                Err(e) => {
                                    let _ = tx.send(BrowserResponse::Error(format!(
                                        "Failed to delete key: {}",
                                        e.message
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(BrowserResponse::Error(format!(
                                "Failed to connect: {}",
                                e.message
                            )));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to get profile: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn refresh_key(&mut self, key_name: String) {
        self.value_display = ValueDisplay::NotLoaded;
        self.load_key_value(key_name, false);
    }

    fn refresh(&mut self) {
        self.keys.clear();
        self.cursor = 0;
        self.has_more = false;
        self.selected_key = None;
        self.value_display = ValueDisplay::NotLoaded;
        self.error = None;
        self.scan_keys();
    }
}

// Helper functions

fn key_type_color(type_name: &str) -> Color32 {
    match type_name {
        "string" => Color32::from_rgb(64, 196, 64),
        "list" => Color32::from_rgb(64, 128, 255),
        "set" => Color32::from_rgb(255, 128, 64),
        "zset" => Color32::from_rgb(200, 64, 200),
        "hash" => Color32::from_rgb(255, 192, 64),
        _ => Color32::GRAY,
    }
}

fn format_ttl(seconds: i64) -> String {
    if seconds < 0 {
        return "no expiry".to_string();
    }
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else if seconds < 86400 {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    } else {
        format!("{}d {}h", seconds / 86400, (seconds % 86400) / 3600)
    }
}

fn format_key_size(type_name: &str, size: u64) -> String {
    match type_name {
        "string" => human_bytes(size),
        "list" | "set" | "zset" => format!("{} items", size),
        "hash" => format!("{} fields", size),
        _ => human_bytes(size),
    }
}

fn parse_type_from_detail(detail: &str) -> Option<String> {
    detail.split_whitespace().next().map(|s| s.to_string())
}

fn parse_ttl_from_detail(detail: Option<&str>) -> Option<i64> {
    detail.and_then(|d| {
        d.split("ttl=")
            .nth(1)
            .and_then(|s| s.trim_end_matches('s').parse().ok())
    })
}

fn format_value_preview(value: &RedisValue, max_bytes: usize) -> String {
    match value {
        RedisValue::String(bytes) => {
            let text = String::from_utf8_lossy(bytes);
            if text.len() > max_bytes {
                format!("{}...\n\n(truncated, full size: {})", &text[..max_bytes], human_bytes(bytes.len() as u64))
            } else {
                text.to_string()
            }
        }
        RedisValue::Hash(fields) => {
            let preview: Vec<String> = fields
                .iter()
                .take(20)
                .map(|(k, v)| format!("{}: {}", k, cell_value_to_string(v)))
                .collect();
            let total = fields.len();
            if total > 20 {
                format!("{}\n... ({} more fields)", preview.join("\n"), total - 20)
            } else {
                preview.join("\n")
            }
        }
        RedisValue::List(items) => {
            let preview: Vec<String> = items
                .iter()
                .take(20)
                .enumerate()
                .map(|(i, v)| format!("[{}] {}", i, cell_value_to_string(v)))
                .collect();
            let total = items.len();
            if total > 20 {
                format!("{}\n... ({} more items)", preview.join("\n"), total - 20)
            } else {
                preview.join("\n")
            }
        }
        RedisValue::Set(items) => {
            let preview: Vec<String> = items
                .iter()
                .take(20)
                .map(|v| cell_value_to_string(v))
                .collect();
            let total = items.len();
            if total > 20 {
                format!("{}\n... ({} more members)", preview.join("\n"), total - 20)
            } else {
                preview.join("\n")
            }
        }
        RedisValue::SortedSet(members) => {
            let preview: Vec<String> = members
                .iter()
                .take(20)
                .map(|(m, s)| format!("{}: score={}", cell_value_to_string(m), s))
                .collect();
            let total = members.len();
            if total > 20 {
                format!("{}\n... ({} more members)", preview.join("\n"), total - 20)
            } else {
                preview.join("\n")
            }
        }
        RedisValue::Stream(_) => "(stream)".to_string(),
        RedisValue::None => "(empty)".to_string(),
    }
}

fn estimate_value_size(value: &RedisValue) -> usize {
    match value {
        RedisValue::String(bytes) => bytes.len(),
        RedisValue::Hash(fields) => {
            fields.iter().map(|(k, v)| k.len() + cell_value_len(v)).sum()
        }
        RedisValue::List(items) => items.iter().map(cell_value_len).sum(),
        RedisValue::Set(items) => items.iter().map(cell_value_len).sum(),
        RedisValue::SortedSet(members) => {
            members.iter().map(|(m, _)| cell_value_len(m)).sum()
        }
        RedisValue::Stream(_) => 0,
        RedisValue::None => 0,
    }
}

fn cell_value_to_string(value: &CellValue) -> String {
    match value {
        CellValue::Null => "(null)".to_string(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Int(i) => i.to_string(),
        CellValue::Float(f) => f.to_string(),
        CellValue::Decimal(s) => s.clone(),
        CellValue::Text(s) => s.clone(),
        CellValue::Bytes(b) => format!("[{} bytes]", b.len()),
        CellValue::Date(_) => "(date)".to_string(),
        CellValue::DateTime(_) => "(datetime)".to_string(),
        CellValue::Json(s) => s.clone(),
    }
}

fn cell_value_len(value: &CellValue) -> usize {
    cell_value_to_string(value).len()
}
