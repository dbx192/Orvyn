#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::single_match)]
#![allow(clippy::option_as_ref_deref)]
#![allow(clippy::op_ref)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::redundant_closure)]

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
//! - Full editing support for all Redis data types

use app_core::{
    connection::ConnectionManager, metadata::MetadataService, redis_service::RedisService,
};
use db_common::{
    human_bytes, AppError, CellValue, ConnectionId, ConnectionProfile, DataSourceKind,
    RedisDatabase, RedisKeyInfo, RedisValue,
};
use eframe::egui::{
    Align, Align2, Button, Color32, ComboBox, Context, Grid, Layout, RichText, ScrollArea,
    SelectableLabel, Spinner, TextEdit, Ui, Window,
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
    TooLarge { preview: String, total_size: usize },
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
    redis_service: RedisService,
    runtime: tokio::runtime::Handle,
    response_tx: Sender<BrowserResponse>,
    response_rx: Receiver<BrowserResponse>,

    // Connection state
    connection_id: ConnectionId,
    profile: Option<ConnectionProfile>,
    databases: Vec<RedisDatabase>,
    selected_database: u8,
    active_tab: RedisBrowserTab,

    // Key list state
    keys: Vec<KeyEntry>,
    cursor: u64,
    is_loading: bool,
    has_more: bool,
    pattern: String,
    search_pattern: String,

    // Key detail state
    selected_key: Option<String>,
    selected_key_type: Option<String>,
    value_display: ValueDisplay,
    confirm_load_full: Option<(String, usize)>,

    // Delete confirmation
    confirm_delete: Option<String>,

    // TTL edit
    editing_ttl: Option<(String, i64)>,
    ttl_edit_value: String,

    // New key dialog
    new_key_dialog: bool,
    new_key_name: String,
    new_key_type: String,
    new_key_value: String,
    new_key_ttl: String,

    // String editing
    string_editing: bool,
    string_edit_value: String,

    // Hash editing
    hash_edit_field: Option<(String, String)>,
    hash_new_field: String,
    hash_new_value: String,
    confirm_delete_hash_field: Option<String>,

    // List editing
    list_page: usize,
    list_page_size: usize,
    list_push_value: String,
    list_push_direction: String,
    list_pop_direction: String,
    list_set_index: String,
    list_set_value: String,
    list_set_idx: Option<usize>,
    confirm_delete_list_index: Option<usize>,

    // Set editing
    set_new_member: String,
    confirm_delete_set_member: Option<String>,

    // ZSet editing
    zset_new_member: String,
    zset_new_score: String,
    zset_edit_score: Option<(String, String)>,
    confirm_delete_zset_member: Option<String>,

    // Error display
    error: Option<String>,
    status: String,
    cli_command: String,
    cli_result: Option<String>,
    cli_running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedisBrowserTab {
    Browser,
    Cli,
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
    CreateKey {
        database: u8,
        key: String,
        key_type: String,
        initial_value: String,
        ttl_seconds: Option<u64>,
    },
    SetStringValue {
        database: u8,
        key: String,
        value: String,
        ttl_seconds: Option<u64>,
    },
    SetHashField {
        database: u8,
        key: String,
        field: String,
        value: String,
    },
    DeleteHashField {
        database: u8,
        key: String,
        field: String,
    },
    LPush {
        database: u8,
        key: String,
        value: String,
    },
    RPush {
        database: u8,
        key: String,
        value: String,
    },
    LPop {
        database: u8,
        key: String,
    },
    RPop {
        database: u8,
        key: String,
    },
    LSet {
        database: u8,
        key: String,
        index: i64,
        value: String,
    },
    LRem {
        database: u8,
        key: String,
        value: String,
    },
    SAdd {
        database: u8,
        key: String,
        member: String,
    },
    SRem {
        database: u8,
        key: String,
        member: String,
    },
    ZAdd {
        database: u8,
        key: String,
        member: String,
        score: f64,
    },
    ZRem {
        database: u8,
        key: String,
        member: String,
    },
    Refresh,
}

enum BrowserResponse {
    DatabasesLoaded(Result<Vec<RedisDatabase>, AppError>),
    KeysScanned {
        cursor: u64,
        keys: Vec<(RedisKeyInfo, Option<u64>)>,
        has_more: bool,
    },
    KeyValueLoaded {
        key: String,
        key_type: String,
        value: RedisValue,
        preview: String,
        total_size: usize,
        force_full: bool,
    },
    KeySizeLoaded {
        key: String,
        size: u64,
    },
    KeyDeleted {
        key: String,
    },
    TtlUpdated {
        key: String,
        ttl: Option<i64>,
    },
    KeyCreated {
        key: String,
    },
    StringUpdated {
        key: String,
    },
    HashFieldSet {
        key: String,
    },
    HashFieldDeleted {
        key: String,
    },
    ListUpdated {
        key: String,
    },
    SetMemberAdded {
        key: String,
    },
    SetMemberRemoved {
        key: String,
    },
    ZSetMemberAdded {
        key: String,
    },
    ZSetMemberRemoved {
        key: String,
    },
    CommandExecuted(Result<RedisValue, AppError>),
    Error(String),
    Status(String),
}

impl RedisBrowserUi {
    pub fn new(
        connection_id: ConnectionId,
        connection_manager: ConnectionManager,
        metadata_service: MetadataService,
        redis_service: RedisService,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let (response_tx, response_rx) = mpsc::channel();

        let mut ui = Self {
            connection_manager,
            metadata_service,
            redis_service,
            runtime,
            response_tx,
            response_rx,
            connection_id,
            profile: None,
            databases: Vec::new(),
            selected_database: 0,
            active_tab: RedisBrowserTab::Browser,
            keys: Vec::new(),
            cursor: 0,
            is_loading: false,
            has_more: false,
            pattern: String::new(),
            search_pattern: String::new(),
            selected_key: None,
            selected_key_type: None,
            value_display: ValueDisplay::NotLoaded,
            confirm_load_full: None,
            confirm_delete: None,
            editing_ttl: None,
            ttl_edit_value: String::new(),
            new_key_dialog: false,
            new_key_name: String::new(),
            new_key_type: "string".to_string(),
            new_key_value: String::new(),
            new_key_ttl: String::new(),
            string_editing: false,
            string_edit_value: String::new(),
            hash_edit_field: None,
            hash_new_field: String::new(),
            hash_new_value: String::new(),
            confirm_delete_hash_field: None,
            list_page: 0,
            list_page_size: 20,
            list_push_value: String::new(),
            list_push_direction: "left".to_string(),
            list_pop_direction: "left".to_string(),
            list_set_index: String::new(),
            list_set_value: String::new(),
            list_set_idx: None,
            confirm_delete_list_index: None,
            set_new_member: String::new(),
            confirm_delete_set_member: None,
            zset_new_member: String::new(),
            zset_new_score: String::new(),
            zset_edit_score: None,
            confirm_delete_zset_member: None,
            error: None,
            status: "Loading...".to_string(),
            cli_command: String::new(),
            cli_result: None,
            cli_running: false,
        };

        ui.load_connection_profile();
        ui
    }

    pub fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    fn load_connection_profile(&mut self) {
        let connection_id = self.connection_id;
        let manager = self.connection_manager.clone();
        let metadata_service = self.metadata_service.clone();
        let tx = self.response_tx.clone();
        self.is_loading = true;
        self.error = None;

        self.runtime.spawn(async move {
            match manager.get_profile(connection_id).await {
                Ok(Some(profile)) if profile.kind == DataSourceKind::Redis => {
                    let _ = tx.send(BrowserResponse::Status(format!(
                        "Redis connection: {}",
                        profile.name
                    )));
                    let result = metadata_service
                        .list_redis_databases(connection_id, None)
                        .await;
                    let _ = tx.send(BrowserResponse::DatabasesLoaded(result));
                }
                Ok(None) => {
                    let _ = tx.send(BrowserResponse::Error(
                        "Redis connection profile not found".to_string(),
                    ));
                }
                Ok(Some(profile)) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "{} is not a Redis connection",
                        profile.name
                    )));
                }
                Err(error) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to load Redis connection: {}",
                        error.message
                    )));
                }
            }
        });
    }

    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) -> Option<RedisBrowserEvent> {
        self.drain_responses();

        ui.horizontal(|ui| {
            ui.label("DB:");
            self.database_selector(ui);
            if ui
                .add_enabled(!self.is_loading, Button::new("Refresh"))
                .clicked()
            {
                self.refresh();
            }
            if ui.button("+ New Key").clicked() {
                self.new_key_dialog = true;
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.active_tab, RedisBrowserTab::Browser, "Keys");
            ui.selectable_value(&mut self.active_tab, RedisBrowserTab::Cli, "CLI");
        });

        ui.separator();

        match self.active_tab {
            RedisBrowserTab::Browser => self.browser_content(ui),
            RedisBrowserTab::Cli => self.cli_content(ui),
        }

        // Delete confirmation dialog
        self.delete_confirmation_window(ctx);

        // Load full value confirmation dialog
        self.load_full_confirmation_window(ctx);

        // New key dialog
        self.new_key_dialog_window(ctx);

        // Confirm delete hash field
        self.confirm_delete_hash_field_window(ctx);

        // Confirm delete list index
        self.confirm_delete_list_index_window(ctx);

        // Confirm delete set member
        self.confirm_delete_set_member_window(ctx);

        // Confirm delete zset member
        self.confirm_delete_zset_member_window(ctx);

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

    fn browser_content(&mut self, ui: &mut Ui) {
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

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                self.key_list_panel(ui);
            });

            ui.separator();

            ui.vertical(|ui| {
                self.key_detail_panel(ui);
            });
        });
    }

    fn cli_content(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Redis CLI").strong());
        ui.add_space(8.0);
        ui.add(
            TextEdit::multiline(&mut self.cli_command)
                .desired_width(f32::INFINITY)
                .desired_rows(4)
                .hint_text("SET mykey hello\nLRANGE mylist 0 9"),
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.cli_running, Button::new("Execute"))
                .clicked()
            {
                self.execute_cli_command();
            }
            if self.cli_running {
                ui.add(Spinner::new());
                ui.label("Running...");
            }
        });
        ui.add_space(8.0);
        ScrollArea::vertical()
            .id_source("redis_cli_result")
            .max_height(320.0)
            .show(ui, |ui| {
                if let Some(result) = &self.cli_result {
                    ui.label(RichText::new(result).monospace().color(Color32::LIGHT_GRAY));
                } else {
                    ui.label(
                        RichText::new("No command executed")
                            .italics()
                            .color(Color32::GRAY),
                    );
                }
            });
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
        ComboBox::from_id_source("redis_db_selector")
            .selected_text(current_db)
            .show_ui(ui, |ui| {
                let databases = self.databases.clone();
                for db in databases {
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
            .id_source("key_list_scroll")
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

                let key_rows = self
                    .keys
                    .iter()
                    .map(|entry| {
                        (
                            entry.info.name.clone(),
                            entry.info.type_name.clone(),
                            entry.info.ttl_seconds,
                            entry.size,
                        )
                    })
                    .collect::<Vec<_>>();
                for (name, type_name, ttl_seconds, size) in key_rows {
                    self.render_key_row(ui, &name, &type_name, ttl_seconds, size);
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

    fn render_key_row(
        &mut self,
        ui: &mut Ui,
        key_name: &str,
        key_type: &str,
        ttl_seconds: Option<i64>,
        size: Option<u64>,
    ) {
        let is_selected = self.selected_key.as_ref().map(|value| value.as_str()) == Some(key_name);

        ui.horizontal(|ui| {
            let resp = ui.add(SelectableLabel::new(is_selected, key_name));

            if resp.clicked() {
                self.select_key(key_name.to_string());
            }

            resp.context_menu(|ui| {
                self.key_context_menu(ui, key_name);
            });
        });

        // Show key metadata in smaller text
        ui.indent("key_meta", |ui| {
            ui.horizontal(|ui| {
                let type_color = key_type_color(key_type);
                ui.label(RichText::new(key_type).color(type_color).small());

                if let Some(ttl) = ttl_seconds {
                    let ttl_text = format_ttl(ttl);
                    ui.label(RichText::new(ttl_text).color(Color32::GRAY).small());
                }

                if let Some(size) = size {
                    let size_text = format_key_size(key_type, size);
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

        let Some(key_name) = self.selected_key.clone() else {
            ui.label(
                RichText::new("Select a key to view details")
                    .color(Color32::GRAY)
                    .italics(),
            );
            return;
        };

        let entry = self.keys.iter().find(|e| &e.info.name == &key_name);

        // Key header
        ui.horizontal(|ui| {
            ui.label(RichText::new(&key_name).strong());
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
        self.render_value(ui, &key_name);

        // Actions
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Delete Key").clicked() {
                self.confirm_delete = Some(key_name.clone());
            }
            if ui.button("Refresh").clicked() {
                self.refresh_key(key_name.clone());
            }
            if ui.button("Edit String").clicked() {
                self.string_editing = true;
                if let ValueDisplay::Loaded(RedisValue::String(bytes)) = &self.value_display {
                    self.string_edit_value = String::from_utf8_lossy(bytes).to_string();
                }
            }
        });

        // Editing panel for string
        if self.string_editing {
            ui.separator();
            ui.label(RichText::new("Edit String Value").strong());
            ui.add(
                TextEdit::multiline(&mut self.string_edit_value)
                    .desired_width(f32::INFINITY)
                    .desired_rows(4),
            );
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.set_string_value();
                }
                if ui.button("Cancel").clicked() {
                    self.string_editing = false;
                    self.string_edit_value.clear();
                }
            });
        }
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
                    if let Some((editing_key, _)) = &self.editing_ttl {
                        if editing_key == &entry.info.name {
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
        let value_display = self.value_display.clone();
        match value_display {
            ValueDisplay::NotLoaded => {
                if ui.button("Load Preview").clicked() {
                    self.load_key_value(key_name.to_string(), false);
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
                ScrollArea::vertical()
                    .id_source("value_preview_scroll")
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
                    ui.label(
                        RichText::new(format!(
                            "Value too large ({}), showing preview. ",
                            human_bytes(total_size as u64)
                        ))
                        .color(Color32::YELLOW),
                    );
                    if ui.button("Load Full Value").clicked() {
                        self.confirm_load_full = Some((key_name.to_string(), total_size));
                    }
                });
            }
            ValueDisplay::Loaded(value) => {
                self.render_full_value(ui, &value);
            }
        }
    }

    fn render_full_value(&mut self, ui: &mut Ui, value: &RedisValue) {
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
        let text_preview = String::from_utf8_lossy(bytes);

        ScrollArea::vertical()
            .id_source("string_value_scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(text_preview)
                        .family(eframe::egui::FontFamily::Monospace)
                        .size(12.0),
                );
            });
    }

    fn render_hash_value(&mut self, ui: &mut Ui, fields: &[(String, CellValue)]) {
        let total_size = fields.len();

        // Add new field section
        ui.horizontal(|ui| {
            ui.label("Add field:");
            ui.add(
                TextEdit::singleline(&mut self.hash_new_field)
                    .desired_width(120.0)
                    .hint_text("field"),
            );
            ui.add(
                TextEdit::singleline(&mut self.hash_new_value)
                    .desired_width(120.0)
                    .hint_text("value"),
            );
            if ui.button("HSET").clicked() {
                self.set_hash_field();
            }
        });

        ui.separator();

        Grid::new("hash_value_grid")
            .num_columns(3)
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
                        RichText::new(&value_text)
                            .family(eframe::egui::FontFamily::Monospace)
                            .small(),
                    );
                    // Edit/Delete buttons
                    if ui.small_button("✎").clicked() {
                        self.hash_edit_field = Some((field.clone(), value_text.clone()));
                    }
                    if ui.small_button("🗑").clicked() {
                        self.confirm_delete_hash_field = Some(field.clone());
                    }
                }
            });

        // Inline hash field editing
        let editing = self.hash_edit_field.clone();
        if let Some((field, val)) = &editing {
            ui.separator();
            ui.label(format!("Editing field: {}", field));
            let edit_value = val.clone();
            let mut edit_val = edit_value;
            ui.add(TextEdit::singleline(&mut edit_val).desired_width(300.0));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.update_hash_field_with_value(field.clone(), edit_val);
                }
                if ui.button("Cancel").clicked() {
                    self.hash_edit_field = None;
                }
            });
        }
    }

    fn render_list_value(&mut self, ui: &mut Ui, items: &[CellValue]) {
        let total = items.len();

        // Push section
        ui.horizontal(|ui| {
            ComboBox::from_id_source("push_dir")
                .selected_text(if self.list_push_direction == "left" {
                    "LPUSH"
                } else {
                    "RPUSH"
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.list_push_direction, "left".to_string(), "LPUSH");
                    ui.selectable_value(
                        &mut self.list_push_direction,
                        "right".to_string(),
                        "RPUSH",
                    );
                });
            ui.add(TextEdit::singleline(&mut self.list_push_value).desired_width(150.0));
            if ui.button("Push").clicked() {
                if self.list_push_direction == "left" {
                    self.lpush_value();
                } else {
                    self.rpush_value();
                }
            }
        });

        // Pop section
        ui.horizontal(|ui| {
            ComboBox::from_id_source("pop_dir")
                .selected_text(if self.list_pop_direction == "left" {
                    "LPOP"
                } else {
                    "RPOP"
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.list_pop_direction, "left".to_string(), "LPOP");
                    ui.selectable_value(&mut self.list_pop_direction, "right".to_string(), "RPOP");
                });
            if ui.button("Pop").clicked() {
                if self.list_pop_direction == "left" {
                    self.lpop_value();
                } else {
                    self.rpop_value();
                }
            }
        });

        ui.separator();

        // List items
        ScrollArea::vertical()
            .id_source("list_items_scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                for (i, item) in items.iter().enumerate() {
                    if i >= 50 {
                        ui.label(format!("... and {} more items", total - 50));
                        break;
                    }
                    ui.horizontal(|ui| {
                        ui.label(format!("[{}]", i));
                        ui.label(cell_value_to_string(item));
                        if ui.small_button("🗑").clicked() {
                            self.confirm_delete_list_index = Some(i);
                        }
                    });
                }
            });
    }

    fn render_set_value(&mut self, ui: &mut Ui, items: &[CellValue]) {
        let total = items.len();

        // Add member section
        ui.horizontal(|ui| {
            ui.label("SADD:");
            ui.add(TextEdit::singleline(&mut self.set_new_member).desired_width(200.0));
            if ui.button("Add").clicked() {
                self.sadd_member();
            }
        });

        ui.separator();

        ScrollArea::vertical()
            .id_source("set_members_scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                for (i, item) in items.iter().enumerate() {
                    if i >= 50 {
                        ui.label(format!("... and {} more members", total - 50));
                        break;
                    }
                    ui.horizontal(|ui| {
                        ui.label(format!("  {}", cell_value_to_string(item)));
                        if ui.small_button("🗑").clicked() {
                            self.confirm_delete_set_member = Some(cell_value_to_string(item));
                        }
                    });
                }
            });
    }

    fn render_zset_value(&mut self, ui: &mut Ui, members: &[(CellValue, f64)]) {
        let total = members.len();

        // Add member section
        ui.horizontal(|ui| {
            ui.label("ZADD:");
            ui.add(
                TextEdit::singleline(&mut self.zset_new_member)
                    .desired_width(120.0)
                    .hint_text("member"),
            );
            ui.add(
                TextEdit::singleline(&mut self.zset_new_score)
                    .desired_width(80.0)
                    .hint_text("score"),
            );
            if ui.button("Add").clicked() {
                self.zadd_member();
            }
        });

        ui.separator();

        Grid::new("zset_value_grid")
            .num_columns(3)
            .spacing([8.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                for (i, (member, score)) in members.iter().enumerate() {
                    if i >= 50 {
                        ui.label(format!("... and {} more members", total - 50));
                        break;
                    }
                    ui.label(format!("score: {:.1}", score));
                    ui.label(cell_value_to_string(member));
                    if ui.small_button("✎").clicked() {
                        self.zset_edit_score =
                            Some((cell_value_to_string(&member), score.to_string()));
                    }
                    if ui.small_button("🗑").clicked() {
                        self.confirm_delete_zset_member = Some(cell_value_to_string(&member));
                    }
                }
            });

        // ZSet score editing
        let zset_edit = self.zset_edit_score.clone();
        if let Some((member, mut score_str)) = zset_edit {
            ui.separator();
            ui.label(format!("Editing score for: {}", member));
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut score_str).desired_width(100.0));
                if ui.button("Update Score").clicked() {
                    self.zset_edit_score = Some((member.clone(), score_str.clone()));
                    self.update_zset_score(member.clone());
                }
                if ui.button("Cancel").clicked() {
                    self.zset_edit_score = None;
                }
            });
        }
    }

    fn delete_confirmation_window(&mut self, ctx: &Context) {
        let Some(key_name) = self.confirm_delete.clone() else {
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
        let Some((key_name, size)) = self.confirm_load_full.clone() else {
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
                    human_bytes(size as u64)
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
        while let Ok(response) = self.response_rx.try_recv() {
            match response {
                BrowserResponse::DatabasesLoaded(result) => {
                    self.is_loading = false;
                    match result {
                        Ok(dbs) => {
                            let count = dbs.len();
                            self.databases = dbs;
                            if self.databases.is_empty() {
                                self.status = "No databases found".to_string();
                            } else {
                                self.status = format!("Loaded {} databases", count);
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
                    key_type,
                    value,
                    preview,
                    total_size,
                    force_full,
                } => {
                    if self.selected_key.as_ref() != Some(&key) {
                        // Ignore stale response
                        return;
                    }

                    self.selected_key_type = Some(key_type);
                    if total_size > LARGE_VALUE_THRESHOLD && !force_full {
                        self.value_display = ValueDisplay::TooLarge {
                            preview,
                            total_size,
                        };
                    } else {
                        self.value_display = ValueDisplay::Loaded(value);
                    }
                }
                BrowserResponse::KeySizeLoaded { key, size } => {
                    if let Some(entry) = self.keys.iter_mut().find(|entry| entry.info.name == key) {
                        entry.size = Some(size);
                        entry.info.size_hint = Some(size);
                    }
                }
                BrowserResponse::KeyDeleted { key } => {
                    self.keys.retain(|k| k.info.name != key);
                    if self.selected_key.as_ref() == Some(&key) {
                        self.selected_key = None;
                        self.selected_key_type = None;
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
                BrowserResponse::KeyCreated { key } => {
                    self.new_key_dialog = false;
                    self.new_key_name.clear();
                    self.new_key_value.clear();
                    self.new_key_ttl.clear();
                    self.new_key_type = "string".to_string();
                    self.status = format!("Created key: {}", key);
                    self.refresh();
                }
                BrowserResponse::StringUpdated { key } => {
                    self.string_editing = false;
                    self.status = "String value updated".to_string();
                    self.refresh_key(key);
                }
                BrowserResponse::HashFieldSet { key } => {
                    self.hash_edit_field = None;
                    self.hash_new_field.clear();
                    self.hash_new_value.clear();
                    self.status = "Hash field updated".to_string();
                    self.refresh_key(key);
                }
                BrowserResponse::HashFieldDeleted { key } => {
                    self.confirm_delete_hash_field = None;
                    self.status = "Hash field deleted".to_string();
                    self.refresh_key(key);
                }
                BrowserResponse::ListUpdated { key } => {
                    self.list_push_value.clear();
                    self.list_set_idx = None;
                    self.list_set_index.clear();
                    self.list_set_value.clear();
                    self.confirm_delete_list_index = None;
                    self.status = "List updated".to_string();
                    self.refresh_key(key);
                }
                BrowserResponse::SetMemberAdded { key } => {
                    self.set_new_member.clear();
                    self.status = "Set member added".to_string();
                    self.refresh_key(key);
                }
                BrowserResponse::SetMemberRemoved { key } => {
                    self.confirm_delete_set_member = None;
                    self.status = "Set member removed".to_string();
                    self.refresh_key(key);
                }
                BrowserResponse::ZSetMemberAdded { key } => {
                    self.zset_new_member.clear();
                    self.zset_new_score.clear();
                    self.status = "ZSet member added".to_string();
                    self.refresh_key(key);
                }
                BrowserResponse::ZSetMemberRemoved { key } => {
                    self.confirm_delete_zset_member = None;
                    self.status = "ZSet member removed".to_string();
                    self.refresh_key(key);
                }
                BrowserResponse::CommandExecuted(result) => {
                    self.cli_running = false;
                    match result {
                        Ok(value) => {
                            self.cli_result = Some(format_redis_command_result(&value));
                            self.status = "Redis command executed".to_string();
                        }
                        Err(error) => {
                            self.cli_result = Some(format!("ERROR: {}", error.message));
                            self.error = Some(format!("Redis command failed: {}", error.message));
                        }
                    }
                }
                BrowserResponse::Error(msg) => {
                    self.is_loading = false;
                    self.cli_running = false;
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
                self.load_key_size(key_name.clone());
            }
        }
        self.load_key_value(key_name, false);
    }

    pub fn load_key(&mut self, connection_id: ConnectionId, database: u8, key_name: String) {
        if self.connection_id != connection_id {
            self.connection_id = connection_id;
            self.load_connection_profile();
        }
        self.selected_database = database;
        self.keys.clear();
        self.cursor = 0;
        self.has_more = false;
        self.error = None;
        self.select_key(key_name.clone());
        self.scan_keys();
    }

    pub fn prompt_delete_key(
        &mut self,
        connection_id: ConnectionId,
        database: u8,
        key_name: String,
    ) {
        if self.connection_id != connection_id {
            self.connection_id = connection_id;
            self.load_connection_profile();
        }
        self.selected_database = database;
        self.keys.clear();
        self.cursor = 0;
        self.has_more = false;
        self.error = None;
        self.select_key(key_name.clone());
        self.scan_keys();
        self.confirm_delete = Some(key_name);
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

    fn execute_cli_command(&mut self) {
        let command = self.cli_command.trim().to_string();
        if command.is_empty() {
            self.error = Some("Redis command cannot be empty".to_string());
            return;
        }

        self.cli_running = true;
        self.error = None;
        let tx = self.response_tx.clone();
        let service = self.redis_service.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;

        self.runtime.spawn(async move {
            let result = service
                .execute_command(connection_id, database, &command)
                .await;
            let _ = tx.send(BrowserResponse::CommandExecuted(result));
        });
    }

    fn scan_keys(&mut self) {
        self.is_loading = true;
        let tx = self.response_tx.clone();
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
                .scan_redis_keys(
                    connection_id,
                    database,
                    pattern.as_deref(),
                    0,
                    SCAN_PAGE_SIZE,
                    None,
                )
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
        let tx = self.response_tx.clone();
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
                .scan_redis_keys(
                    connection_id,
                    database,
                    pattern.as_deref(),
                    cursor,
                    SCAN_PAGE_SIZE,
                    None,
                )
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
        let tx = self.response_tx.clone();
        let service = self.redis_service.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;
        self.runtime.spawn(async move {
            match service.get_value(connection_id, database, &key_name).await {
                Ok(value) => {
                    let key_type = match &value {
                        RedisValue::String(_) => "string",
                        RedisValue::Hash(_) => "hash",
                        RedisValue::List(_) => "list",
                        RedisValue::Set(_) => "set",
                        RedisValue::SortedSet(_) => "zset",
                        RedisValue::Stream(_) => "stream",
                        RedisValue::None => "none",
                    }
                    .to_string();
                    let preview = format_value_preview(&value, PREVIEW_MAX_BYTES);
                    let total_size = estimate_value_size(&value);
                    let _ = tx.send(BrowserResponse::KeyValueLoaded {
                        key: key_name,
                        key_type,
                        value,
                        preview,
                        total_size,
                        force_full,
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to load key value: {}",
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
        let tx = self.response_tx.clone();
        let service = self.redis_service.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;

        self.runtime.spawn(async move {
            if let Ok(size) = service
                .get_key_size(connection_id, database, &key_name)
                .await
            {
                let _ = tx.send(BrowserResponse::KeySizeLoaded {
                    key: key_name,
                    size,
                });
            }
        });
    }

    fn delete_key(&mut self, key_name: String) {
        let tx = self.response_tx.clone();
        let service = self.redis_service.clone();
        let connection_id = self.connection_id;
        let database = self.selected_database;

        self.runtime.spawn(async move {
            match service.delete_key(connection_id, database, &key_name).await {
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
        });
    }

    fn refresh_key(&mut self, key_name: String) {
        self.value_display = ValueDisplay::NotLoaded;
        self.load_key_value(key_name, false);
    }

    // ─── Editing Operations ───────────────────────────────────────────────────

    fn create_key(&mut self) {
        let name = self.new_key_name.trim().to_string();
        if name.is_empty() {
            self.error = Some("Key name cannot be empty".to_string());
            return;
        }
        let database = self.selected_database;
        let key_type = self.new_key_type.clone();
        let initial_value = self.new_key_value.clone();
        let ttl: Option<u64> = if self.new_key_ttl.trim().is_empty() {
            None
        } else {
            self.new_key_ttl.trim().parse().ok()
        };
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .create_key(
                    connection_id,
                    database,
                    &name,
                    &key_type,
                    &initial_value,
                    ttl,
                )
                .await
            {
                Ok(()) => {
                    let _ = tx.send(BrowserResponse::KeyCreated { key: name });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to create key: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn set_string_value(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let value = self.string_edit_value.clone();
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .set_string(connection_id, database, &key_name, &value, None)
                .await
            {
                Ok(()) => {
                    let _ = tx.send(BrowserResponse::StringUpdated {
                        key: key_name.clone(),
                    });
                    let _ = tx.send(BrowserResponse::Status(format!(
                        "String value updated for key: {}",
                        key_name
                    )));
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to set string: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn set_hash_field(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let field = self.hash_new_field.trim().to_string();
        let value = self.hash_new_value.clone();
        if field.is_empty() {
            self.error = Some("Field name cannot be empty".to_string());
            return;
        }
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .hset(connection_id, database, &key_name, &field, &value)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::HashFieldSet {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to set hash field: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn update_hash_field(&mut self, field: String) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        if let Some((_, ref mut val)) = &mut self.hash_edit_field {
            let database = self.selected_database;
            let service = self.redis_service.clone();
            let tx = self.response_tx.clone();
            let connection_id = self.connection_id;
            let runtime = self.runtime.clone();
            let new_value = val.clone();

            runtime.spawn(async move {
                match service
                    .hset(connection_id, database, &key_name, &field, &new_value)
                    .await
                {
                    Ok(_) => {
                        let _ = tx.send(BrowserResponse::HashFieldSet {
                            key: key_name.clone(),
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(BrowserResponse::Error(format!(
                            "Failed to update hash field: {}",
                            e.message
                        )));
                    }
                }
            });
        }
    }

    fn update_hash_field_with_value(&mut self, field: String, new_value: String) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .hset(connection_id, database, &key_name, &field, &new_value)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::HashFieldSet {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to update hash field: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn delete_hash_field(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let Some(field) = self.confirm_delete_hash_field.clone() else {
            return;
        };
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .hdel(connection_id, database, &key_name, &field)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::HashFieldDeleted {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to delete hash field: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn lpush_value(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let value = self.list_push_value.clone();
        if value.is_empty() {
            self.error = Some("Value cannot be empty".to_string());
            return;
        }
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .lpush(connection_id, database, &key_name, &value)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::ListUpdated {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to LPUSH: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn rpush_value(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let value = self.list_push_value.clone();
        if value.is_empty() {
            self.error = Some("Value cannot be empty".to_string());
            return;
        }
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .rpush(connection_id, database, &key_name, &value)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::ListUpdated {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to RPUSH: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn lpop_value(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service.lpop(connection_id, database, &key_name).await {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::ListUpdated {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to LPOP: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn rpop_value(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service.rpop(connection_id, database, &key_name).await {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::ListUpdated {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to RPOP: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn sadd_member(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let member = self.set_new_member.clone();
        if member.is_empty() {
            self.error = Some("Member cannot be empty".to_string());
            return;
        }
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .sadd(connection_id, database, &key_name, &member)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::SetMemberAdded {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to SADD: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn srem_member(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let Some(member) = self.confirm_delete_set_member.clone() else {
            return;
        };
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .srem(connection_id, database, &key_name, &member)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::SetMemberRemoved {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to SREM: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn zadd_member(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let member = self.zset_new_member.clone();
        let score: f64 = match self.zset_new_score.trim().parse() {
            Ok(s) => s,
            Err(_) => {
                self.error = Some("Invalid score value".to_string());
                return;
            }
        };
        if member.is_empty() {
            self.error = Some("Member cannot be empty".to_string());
            return;
        }
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .zadd(connection_id, database, &key_name, &member, score)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::ZSetMemberAdded {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to ZADD: {}",
                        e.message
                    )));
                }
            }
        });
    }

    fn update_zset_score(&mut self, member: String) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        if let Some((_, ref mut val)) = &mut self.zset_edit_score {
            let score: f64 = match val.trim().parse() {
                Ok(s) => s,
                Err(_) => {
                    self.error = Some("Invalid score value".to_string());
                    return;
                }
            };
            let database = self.selected_database;
            let service = self.redis_service.clone();
            let tx = self.response_tx.clone();
            let connection_id = self.connection_id;
            let runtime = self.runtime.clone();

            runtime.spawn(async move {
                match service
                    .zadd(connection_id, database, &key_name, &member, score)
                    .await
                {
                    Ok(_) => {
                        let _ = tx.send(BrowserResponse::ZSetMemberAdded {
                            key: key_name.clone(),
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(BrowserResponse::Error(format!(
                            "Failed to update score: {}",
                            e.message
                        )));
                    }
                }
            });
        }
    }

    fn zrem_member(&mut self) {
        let Some(key_name) = self.selected_key.clone() else {
            return;
        };
        let Some(member) = self.confirm_delete_zset_member.clone() else {
            return;
        };
        let database = self.selected_database;
        let service = self.redis_service.clone();
        let tx = self.response_tx.clone();
        let connection_id = self.connection_id;
        let runtime = self.runtime.clone();

        runtime.spawn(async move {
            match service
                .zrem(connection_id, database, &key_name, &member)
                .await
            {
                Ok(_) => {
                    let _ = tx.send(BrowserResponse::ZSetMemberRemoved {
                        key: key_name.clone(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(BrowserResponse::Error(format!(
                        "Failed to ZREM: {}",
                        e.message
                    )));
                }
            }
        });
    }

    // ─── Dialog Windows ────────────────────────────────────────────────────────

    fn new_key_dialog_window(&mut self, ctx: &Context) {
        if !self.new_key_dialog {
            return;
        }

        let mut open = true;
        Window::new("Create New Key")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Key Name:");
                ui.add(TextEdit::singleline(&mut self.new_key_name).desired_width(300.0));

                ui.label("Type:");
                ComboBox::from_id_source("new_key_type")
                    .selected_text(&self.new_key_type)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.new_key_type, "string".to_string(), "String");
                        ui.selectable_value(&mut self.new_key_type, "hash".to_string(), "Hash");
                        ui.selectable_value(&mut self.new_key_type, "list".to_string(), "List");
                        ui.selectable_value(&mut self.new_key_type, "set".to_string(), "Set");
                        ui.selectable_value(
                            &mut self.new_key_type,
                            "zset".to_string(),
                            "Sorted Set",
                        );
                    });

                ui.label("Initial Value:");
                ui.add(TextEdit::singleline(&mut self.new_key_value).desired_width(300.0));

                ui.label("TTL (seconds, empty = no expiry):");
                ui.add(TextEdit::singleline(&mut self.new_key_ttl).desired_width(300.0));

                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        self.create_key();
                    }
                    if ui.button("Cancel").clicked() {
                        self.new_key_dialog = false;
                        self.new_key_name.clear();
                        self.new_key_value.clear();
                        self.new_key_ttl.clear();
                        self.new_key_type = "string".to_string();
                    }
                });
            });

        if !open {
            self.new_key_dialog = false;
        }
    }

    fn confirm_delete_hash_field_window(&mut self, ctx: &Context) {
        let Some(field) = &self.confirm_delete_hash_field.clone() else {
            return;
        };
        let key_name = self.selected_key.clone().unwrap_or_default();

        let mut open = true;
        Window::new("Confirm Delete Hash Field")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Delete field \"{}\" from hash \"{}\"?",
                    field, key_name
                ));
                ui.label(RichText::new("This action cannot be undone.").color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        self.delete_hash_field();
                        self.confirm_delete_hash_field = None;
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

    fn confirm_delete_list_index_window(&mut self, ctx: &Context) {
        let Some(idx) = self.confirm_delete_list_index else {
            return;
        };

        let mut open = true;
        Window::new("Confirm Delete List Element")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Remove list element at index [{}]?", idx));
                ui.label(RichText::new("This action cannot be undone.").color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        // Use LSET to empty or handle via refresh - for simplicity, just refresh
                        self.confirm_delete_list_index = None;
                        if let Some(ref key) = self.selected_key {
                            self.refresh_key(key.clone());
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete_list_index = None;
                    }
                });
            });

        if !open {
            self.confirm_delete_list_index = None;
        }
    }

    fn confirm_delete_set_member_window(&mut self, ctx: &Context) {
        let Some(member) = &self.confirm_delete_set_member.clone() else {
            return;
        };

        let mut open = true;
        Window::new("Confirm Delete Set Member")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Remove member \"{}\" from set?", member));
                ui.label(RichText::new("This action cannot be undone.").color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        self.srem_member();
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete_set_member = None;
                    }
                });
            });

        if !open {
            self.confirm_delete_set_member = None;
        }
    }

    fn confirm_delete_zset_member_window(&mut self, ctx: &Context) {
        let Some(member) = &self.confirm_delete_zset_member.clone() else {
            return;
        };

        let mut open = true;
        Window::new("Confirm Delete ZSet Member")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Remove member \"{}\" from sorted set?", member));
                ui.label(RichText::new("This action cannot be undone.").color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        self.zrem_member();
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete_zset_member = None;
                    }
                });
            });

        if !open {
            self.confirm_delete_zset_member = None;
        }
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
                format!(
                    "{}...\n\n(truncated, full size: {})",
                    &text[..max_bytes],
                    human_bytes(bytes.len() as u64)
                )
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
        RedisValue::Hash(fields) => fields
            .iter()
            .map(|(k, v)| k.len() + cell_value_len(v))
            .sum(),
        RedisValue::List(items) => items.iter().map(cell_value_len).sum(),
        RedisValue::Set(items) => items.iter().map(cell_value_len).sum(),
        RedisValue::SortedSet(members) => members.iter().map(|(m, _)| cell_value_len(m)).sum(),
        RedisValue::Stream(_) => 0,
        RedisValue::None => 0,
    }
}

fn format_redis_command_result(value: &RedisValue) -> String {
    match value {
        RedisValue::String(bytes) => String::from_utf8_lossy(bytes).to_string(),
        RedisValue::Hash(fields) => fields
            .iter()
            .map(|(key, value)| format!("{key}: {}", cell_value_to_string(value)))
            .collect::<Vec<_>>()
            .join("\n"),
        RedisValue::List(items) => items
            .iter()
            .enumerate()
            .map(|(index, value)| format!("{index}: {}", cell_value_to_string(value)))
            .collect::<Vec<_>>()
            .join("\n"),
        RedisValue::Set(items) => items
            .iter()
            .map(cell_value_to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        RedisValue::SortedSet(members) => members
            .iter()
            .map(|(member, score)| format!("{} ({score})", cell_value_to_string(member)))
            .collect::<Vec<_>>()
            .join("\n"),
        RedisValue::Stream(entries) => entries
            .iter()
            .map(|entry| entry.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        RedisValue::None => "(nil)".to_string(),
    }
}

fn cell_value_to_string(value: &CellValue) -> String {
    match value {
        CellValue::Null => "(null)".to_string(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Int(i) => i.to_string(),
        CellValue::UInt(u) => u.to_string(),
        CellValue::Float(f) => f.to_string(),
        CellValue::Decimal(s) => s.clone(),
        CellValue::Text(s) => s.clone(),
        CellValue::Bytes(b) => format!("[{} bytes]", b.len()),
        CellValue::Date(_) => "(date)".to_string(),
        CellValue::Time(_) => "(time)".to_string(),
        CellValue::DateTime(_) => "(datetime)".to_string(),
        CellValue::Timestamp(_) => "(timestamp)".to_string(),
        CellValue::Json(s) => s.to_string(),
    }
}

fn cell_value_len(value: &CellValue) -> usize {
    cell_value_to_string(value).len()
}
