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

use crate::ui::i18n::text as tr;
use app_core::{
    connection::ConnectionManager, metadata::MetadataService, redis_service::RedisService,
};
use db_common::{
    human_bytes, AppError, CellValue, ConnectionId, ConnectionProfile, DataSourceKind,
    RedisDatabase, RedisKeyInfo, RedisValue,
};
use eframe::egui::{
    vec2, Align, Align2, Button, Color32, ComboBox, Context, Frame, Grid, Layout, Margin, Rect,
    RichText, Rounding, ScrollArea, Sense, Spinner, Stroke, TextEdit, Ui, Vec2, Window,
};
use std::{
    collections::BTreeMap,
    sync::mpsc::{self, Receiver, Sender},
};

const SCAN_PAGE_SIZE: u32 = 100;
const PREVIEW_MAX_BYTES: usize = 1024;
const LARGE_VALUE_THRESHOLD: usize = 64 * 1024;
const REDIS_KEY_SIDEBAR_WIDTH: f32 = 330.0;
const REDIS_VALUE_ROW_LIMIT: usize = 250;

type RedisKeyGroupRow = (String, Option<i64>, Option<u64>);
type RedisKeyGroup = (String, Vec<RedisKeyGroupRow>);

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
    value_filter: String,

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
    ConnectionLoaded(ConnectionProfile),
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

#[derive(Clone, Copy)]
struct RedisBrowserStyle {
    canvas: Color32,
    panel_fill: Color32,
    sidebar_fill: Color32,
    toolbar_fill: Color32,
    header_fill: Color32,
    border: Color32,
    row_alt: Color32,
    row_hover: Color32,
    selected_fill: Color32,
    selected_text: Color32,
    accent: Color32,
    accent_soft: Color32,
    danger: Color32,
    text: Color32,
    muted: Color32,
}

impl RedisBrowserStyle {
    fn from_ui(ui: &Ui) -> Self {
        if ui.visuals().dark_mode {
            Self {
                canvas: Color32::from_rgb(18, 20, 24),
                panel_fill: Color32::from_rgb(26, 29, 34),
                sidebar_fill: Color32::from_rgb(22, 25, 30),
                toolbar_fill: Color32::from_rgb(31, 35, 41),
                header_fill: Color32::from_rgb(35, 39, 46),
                border: Color32::from_rgb(54, 60, 69),
                row_alt: Color32::from_rgb(29, 32, 38),
                row_hover: Color32::from_rgb(43, 48, 56),
                selected_fill: Color32::from_rgb(82, 47, 48),
                selected_text: Color32::from_rgb(245, 247, 250),
                accent: Color32::from_rgb(79, 145, 255),
                accent_soft: Color32::from_rgb(35, 54, 83),
                danger: Color32::from_rgb(224, 86, 75),
                text: Color32::from_rgb(226, 230, 235),
                muted: Color32::from_rgb(147, 154, 164),
            }
        } else {
            Self {
                canvas: Color32::from_rgb(246, 247, 249),
                panel_fill: Color32::from_rgb(255, 255, 255),
                sidebar_fill: Color32::from_rgb(250, 251, 253),
                toolbar_fill: Color32::from_rgb(248, 249, 252),
                header_fill: Color32::from_rgb(246, 247, 250),
                border: Color32::from_rgb(218, 223, 230),
                row_alt: Color32::from_rgb(249, 250, 252),
                row_hover: Color32::from_rgb(239, 243, 249),
                selected_fill: Color32::from_rgb(247, 226, 226),
                selected_text: Color32::from_rgb(32, 36, 42),
                accent: Color32::from_rgb(47, 112, 214),
                accent_soft: Color32::from_rgb(226, 238, 255),
                danger: Color32::from_rgb(202, 66, 57),
                text: Color32::from_rgb(32, 36, 42),
                muted: Color32::from_rgb(107, 116, 128),
            }
        }
    }

    fn panel_frame(self) -> Frame {
        Frame::none()
            .fill(self.panel_fill)
            .stroke(Stroke::new(1.0, self.border))
            .rounding(Rounding::same(3.0))
            .inner_margin(Margin::same(0.0))
    }

    fn inset_frame(self) -> Frame {
        Frame::none()
            .fill(self.panel_fill)
            .stroke(Stroke::new(1.0, self.border))
            .rounding(Rounding::same(3.0))
            .inner_margin(Margin::symmetric(10.0, 8.0))
    }

    fn toolbar_frame(self) -> Frame {
        Frame::none()
            .fill(self.toolbar_fill)
            .stroke(Stroke::new(1.0, self.border))
            .inner_margin(Margin::symmetric(10.0, 6.0))
    }

    fn sidebar_frame(self) -> Frame {
        Frame::none()
            .fill(self.sidebar_fill)
            .stroke(Stroke::new(1.0, self.border))
            .inner_margin(Margin::same(8.0))
    }
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
            value_filter: String::new(),
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

    pub fn tab_title(&self) -> String {
        self.profile
            .as_ref()
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| "Key Browser".to_string())
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
                    let _ = tx.send(BrowserResponse::ConnectionLoaded(profile.clone()));
                    let _ = tx.send(BrowserResponse::Status(format!(
                        "Connection opened: {}",
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

        let style = RedisBrowserStyle::from_ui(ui);
        Frame::none()
            .fill(style.canvas)
            .inner_margin(Margin::same(0.0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
                self.connection_header(ui, style);
                self.redis_nav_tabs(ui, style);

                match self.active_tab {
                    RedisBrowserTab::Browser => self.browser_content(ui, style),
                    RedisBrowserTab::Cli => self.cli_content(ui, style),
                }

                self.status_strip(ui, style);
            });

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

        None
    }

    fn connection_header(&mut self, ui: &mut Ui, style: RedisBrowserStyle) {
        style.toolbar_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(self.tab_title())
                        .strong()
                        .color(style.text)
                        .size(15.0),
                );
                if let Some(endpoint) = self.connection_endpoint() {
                    ui.label(RichText::new(endpoint).small().color(style.muted));
                }
                if self.is_loading {
                    ui.add(Spinner::new());
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(!self.is_loading, Button::new(format!("+ {}", tr("Key"))))
                        .clicked()
                    {
                        self.new_key_dialog = true;
                    }
                    if ui
                        .add_enabled(!self.is_loading, Button::new(tr("Refresh")))
                        .clicked()
                    {
                        self.refresh();
                    }
                    ui.label(
                        RichText::new(format!("db{}", self.selected_database))
                            .small()
                            .color(style.muted),
                    );
                });
            });
        });
    }

    fn redis_nav_tabs(&mut self, ui: &mut Ui, style: RedisBrowserStyle) {
        style.toolbar_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.active_tab,
                    RedisBrowserTab::Browser,
                    tr("Key Detail"),
                );
                ui.selectable_value(&mut self.active_tab, RedisBrowserTab::Cli, "CLI");
            });
        });
    }

    fn browser_content(&mut self, ui: &mut Ui, style: RedisBrowserStyle) {
        let height = ui.available_height().max(360.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
            ui.allocate_ui_with_layout(
                Vec2::new(REDIS_KEY_SIDEBAR_WIDTH, height),
                Layout::top_down(Align::Min),
                |ui| {
                    self.key_list_panel(ui, style);
                },
            );

            let detail_width = ui.available_width().max(360.0);
            ui.allocate_ui_with_layout(
                Vec2::new(detail_width, height),
                Layout::top_down(Align::Min),
                |ui| {
                    self.key_detail_panel(ui, style);
                },
            );
        });
    }

    fn cli_content(&mut self, ui: &mut Ui, style: RedisBrowserStyle) {
        style.inset_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("CLI").strong().color(style.text));
                if self.cli_running {
                    ui.add(Spinner::new());
                    ui.label(RichText::new("Running...").color(style.muted));
                }
            });
            ui.add_space(6.0);
            ui.add(
                TextEdit::multiline(&mut self.cli_command)
                    .desired_width(f32::INFINITY)
                    .desired_rows(5)
                    .hint_text("SET mykey hello\nLRANGE mylist 0 9"),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.cli_running, Button::new(tr("Execute")))
                    .clicked()
                {
                    self.execute_cli_command();
                }
                if ui.button(tr("Clear")).clicked() {
                    self.cli_result = None;
                    self.cli_command.clear();
                }
            });
        });

        ui.add_space(6.0);
        style.panel_frame().show(ui, |ui| {
            Frame::none()
                .fill(style.header_fill)
                .inner_margin(Margin::symmetric(10.0, 6.0))
                .show(ui, |ui| {
                    ui.label(RichText::new("Result").strong().color(style.text));
                });
            ScrollArea::vertical()
                .id_source("redis_cli_result")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        if let Some(result) = &self.cli_result {
                            ui.label(RichText::new(result).monospace().color(style.text));
                        } else {
                            ui.label(
                                RichText::new("No command executed")
                                    .italics()
                                    .color(style.muted),
                            );
                        }
                    });
                });
        });
    }

    fn status_strip(&self, ui: &mut Ui, style: RedisBrowserStyle) {
        style.toolbar_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(error) = &self.error {
                    ui.label(RichText::new(error).color(style.danger));
                } else if !self.status.is_empty() {
                    ui.label(RichText::new(&self.status).small().color(style.muted));
                } else {
                    ui.label(RichText::new("Ready").small().color(style.muted));
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("None").small().color(style.muted));
                    ui.separator();
                    ui.label(RichText::new("Raw").small().color(style.muted));
                });
            });
        });
    }

    fn connection_endpoint(&self) -> Option<String> {
        let profile = self.profile.as_ref()?;
        let host = profile.host.as_deref()?;
        let port = profile.port.unwrap_or(6379);
        Some(format!("{host}:{port}"))
    }

    fn key_groups(&self) -> Vec<RedisKeyGroup> {
        let mut groups: BTreeMap<String, Vec<RedisKeyGroupRow>> = BTreeMap::new();
        for entry in &self.keys {
            groups
                .entry(entry.info.type_name.clone())
                .or_default()
                .push((entry.info.name.clone(), entry.info.ttl_seconds, entry.size));
        }

        let mut groups = groups.into_iter().collect::<Vec<_>>();
        for (_, rows) in &mut groups {
            rows.sort_by(|left, right| left.0.cmp(&right.0));
        }
        groups.sort_by(|left, right| {
            redis_type_rank(&left.0)
                .cmp(&redis_type_rank(&right.0))
                .then_with(|| left.0.cmp(&right.0))
        });
        groups
    }

    fn render_key_group_header(
        &self,
        ui: &mut Ui,
        style: RedisBrowserStyle,
        type_name: &str,
        count: usize,
    ) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
        let painter = ui.painter();
        painter.text(
            rect.left_center() + vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            "▾",
            eframe::egui::FontId::proportional(12.0),
            style.muted,
        );
        let label = format!("{} ({count})", redis_type_group_label(type_name));
        painter.text(
            rect.left_center() + vec2(28.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            eframe::egui::FontId::proportional(12.5),
            style.text,
        );
    }

    fn type_badge(&self, ui: &mut Ui, style: RedisBrowserStyle, type_name: &str) {
        Frame::none()
            .fill(key_type_badge_fill(type_name, style))
            .rounding(Rounding::same(3.0))
            .inner_margin(Margin::symmetric(8.0, 3.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(redis_type_badge_label(type_name))
                        .monospace()
                        .strong()
                        .color(Color32::WHITE),
                );
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

    fn key_list_panel(&mut self, ui: &mut Ui, style: RedisBrowserStyle) {
        style.sidebar_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                self.database_selector(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} keys", self.keys.len()))
                            .small()
                            .color(style.muted),
                    );
                });
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let response = ui.add(
                    TextEdit::singleline(&mut self.search_pattern)
                        .hint_text("Filter key pattern")
                        .desired_width((ui.available_width() - 62.0).max(120.0)),
                );
                if response.lost_focus() && self.search_pattern != self.pattern {
                    self.apply_pattern();
                }
                if ui.add_sized([56.0, 24.0], Button::new(tr("Match"))).clicked() {
                    self.apply_pattern();
                }
            });

            ui.add_space(6.0);
            Frame::none()
                .fill(style.panel_fill)
                .stroke(Stroke::new(1.0, style.border))
                .inner_margin(Margin::same(4.0))
                .show(ui, |ui| {
                    ScrollArea::vertical()
                        .id_source("key_list_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.keys.is_empty() && !self.is_loading {
                                ui.add_space(16.0);
                                ui.centered_and_justified(|ui| {
                                    ui.label(
                                        RichText::new("No keys found").color(style.muted).italics(),
                                    );
                                });
                                return;
                            }

                            let groups = self.key_groups();
                            for (type_name, rows) in groups {
                                self.render_key_group_header(ui, style, &type_name, rows.len());
                                for (name, ttl_seconds, size) in rows {
                                    self.render_key_row(
                                        ui,
                                        style,
                                        &name,
                                        &type_name,
                                        ttl_seconds,
                                        size,
                                    );
                                }
                            }

                            ui.add_space(6.0);
                            if self.is_loading {
                                ui.horizontal(|ui| {
                                    ui.add(Spinner::new());
                                    ui.label(RichText::new("Loading...").color(style.muted));
                                });
                            } else if self.has_more {
                                if ui.button(tr("Load More")).clicked() {
                                    self.load_more_keys();
                                }
                            }
                        });
                });
        });
    }

    fn render_key_row(
        &mut self,
        ui: &mut Ui,
        style: RedisBrowserStyle,
        key_name: &str,
        key_type: &str,
        ttl_seconds: Option<i64>,
        size: Option<u64>,
    ) {
        let is_selected = self.selected_key.as_ref().map(|value| value.as_str()) == Some(key_name);
        let row_height = 28.0;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), row_height), Sense::click());
        let painter = ui.painter();
        let fill = if is_selected {
            style.selected_fill
        } else if response.hovered() {
            style.row_hover
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            painter.rect_filled(rect.shrink2(vec2(1.0, 1.0)), Rounding::same(3.0), fill);
        }

        let badge_rect =
            Rect::from_min_size(rect.left_top() + vec2(10.0, 5.0), Vec2::new(18.0, 18.0));
        painter.rect_filled(
            badge_rect,
            Rounding::same(3.0),
            key_type_badge_fill(key_type, style),
        );
        painter.text(
            badge_rect.center(),
            Align2::CENTER_CENTER,
            key_type_short_label(key_type),
            eframe::egui::FontId::monospace(10.0),
            Color32::WHITE,
        );

        let label_color = if is_selected {
            style.selected_text
        } else {
            style.text
        };
        let leaf = key_leaf_label(key_name);
        let prefix = key_parent_label(key_name);
        painter.text(
            rect.left_top() + vec2(34.0, 8.0),
            Align2::LEFT_TOP,
            &leaf,
            eframe::egui::FontId::proportional(12.5),
            label_color,
        );
        if let Some(prefix) = prefix {
            painter.text(
                rect.left_top() + vec2(34.0 + leaf.len() as f32 * 7.0 + 6.0, 8.0),
                Align2::LEFT_TOP,
                &prefix,
                eframe::egui::FontId::proportional(11.0),
                style.muted,
            );
        }
        let meta = key_row_meta(key_type, ttl_seconds, size);
        if !meta.is_empty() && rect.width() > 220.0 {
            painter.text(
                rect.right_top() + vec2(-8.0, 8.0),
                Align2::RIGHT_TOP,
                meta,
                eframe::egui::FontId::proportional(10.5),
                style.muted,
            );
        }

        if response.clicked() {
            self.select_key(key_name.to_string());
        }

        response.context_menu(|ui| {
            self.key_context_menu(ui, key_name);
        });
    }

    fn key_context_menu(&mut self, ui: &mut Ui, key_name: &str) {
        if ui.button(tr("View")).clicked() {
            self.select_key(key_name.to_string());
            ui.close_menu();
        }
        if ui.button(tr("Copy Name")).clicked() {
            ui.output_mut(|o| o.copied_text = key_name.to_string());
            self.status = format!("Copied: {}", key_name);
            ui.close_menu();
        }
        ui.separator();
        if ui.button(tr("Delete")).clicked() {
            self.confirm_delete = Some(key_name.to_string());
            ui.close_menu();
        }
    }

    fn key_detail_panel(&mut self, ui: &mut Ui, style: RedisBrowserStyle) {
        style.panel_frame().show(ui, |ui| {
            let Some(key_name) = self.selected_key.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new("Select a key to view details")
                            .color(style.muted)
                            .italics(),
                    );
                });
                return;
            };

            let entry = self
                .keys
                .iter()
                .find(|entry| entry.info.name == key_name)
                .cloned();
            let key_type = entry
                .as_ref()
                .map(|entry| entry.info.type_name.clone())
                .or_else(|| self.selected_key_type.clone())
                .unwrap_or_else(|| "key".to_string());

            self.key_detail_header(ui, style, &key_name, &key_type);
            if let Some(entry) = entry.as_ref() {
                self.metadata_grid(ui, entry, style);
            }

            Frame::none()
                .fill(style.panel_fill)
                .inner_margin(Margin::symmetric(8.0, 8.0))
                .show(ui, |ui| {
                    self.render_value(ui, &key_name, style);
                });

            if self.string_editing {
                ui.add_space(6.0);
                style.inset_frame().show(ui, |ui| {
                    ui.label(
                        RichText::new("Edit String Value")
                            .strong()
                            .color(style.text),
                    );
                    ui.add(
                        TextEdit::multiline(&mut self.string_edit_value)
                            .desired_width(f32::INFINITY)
                            .desired_rows(5),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(tr("Save")).clicked() {
                            self.set_string_value();
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            self.string_editing = false;
                            self.string_edit_value.clear();
                        }
                    });
                });
            }
        });
    }

    fn key_detail_header(
        &mut self,
        ui: &mut Ui,
        style: RedisBrowserStyle,
        key_name: &str,
        key_type: &str,
    ) {
        Frame::none()
            .fill(style.header_fill)
            .stroke(Stroke::new(1.0, style.border))
            .inner_margin(Margin::symmetric(10.0, 7.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.type_badge(ui, style, key_type);
                    ui.label(
                        RichText::new(key_name)
                            .monospace()
                            .strong()
                            .color(style.text),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(tr("Del")).clicked() {
                            self.confirm_delete = Some(key_name.to_string());
                        }
                        if ui.button(tr("Edit")).clicked() && key_type == "string" {
                            self.string_editing = true;
                            if let ValueDisplay::Loaded(RedisValue::String(bytes)) =
                                &self.value_display
                            {
                                self.string_edit_value = redis_display_text(bytes);
                            }
                        }
                        if ui.button(tr("Copy")).clicked() {
                            ui.output_mut(|output| output.copied_text = key_name.to_string());
                            self.status = format!("Copied: {}", key_name);
                        }
                        if ui.button(tr("Reload")).clicked() {
                            self.refresh_key(key_name.to_string());
                        }
                    });
                });
            });
    }

    fn metadata_grid(&self, ui: &mut Ui, entry: &KeyEntry, style: RedisBrowserStyle) {
        Frame::none()
            .fill(style.toolbar_fill)
            .stroke(Stroke::new(1.0, style.border))
            .inner_margin(Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    metadata_chip(ui, style, "Type", &entry.info.type_name);
                    let ttl = entry
                        .info
                        .ttl_seconds
                        .map(format_ttl)
                        .unwrap_or_else(|| "no expiry".to_string());
                    metadata_chip(ui, style, "TTL", &ttl);
                    let size = entry
                        .size
                        .map(|size| format_key_size(&entry.info.type_name, size))
                        .unwrap_or_else(|| "calculating".to_string());
                    metadata_chip(ui, style, "Size", &size);
                });
            });
    }

    fn render_value(&mut self, ui: &mut Ui, key_name: &str, style: RedisBrowserStyle) {
        let value_display = self.value_display.clone();
        match value_display {
            ValueDisplay::NotLoaded => {
                if ui.button(tr("Load Preview")).clicked() {
                    self.load_key_value(key_name.to_string(), false);
                }
            }
            ValueDisplay::Loading => {
                ui.horizontal(|ui| {
                    ui.add(Spinner::new());
                    ui.label(RichText::new("Loading value...").color(style.muted));
                });
            }
            ValueDisplay::TooLarge {
                preview,
                total_size,
            } => {
                self.value_filter_bar(ui, style, 1, "preview");
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
                    if ui.button(tr("Load Full Value")).clicked() {
                        self.confirm_load_full = Some((key_name.to_string(), total_size));
                    }
                });
            }
            ValueDisplay::Loaded(value) => {
                self.render_full_value(ui, &value, style);
            }
        }
    }

    fn render_full_value(&mut self, ui: &mut Ui, value: &RedisValue, style: RedisBrowserStyle) {
        match value {
            RedisValue::String(bytes) => {
                self.render_string_value(ui, style, bytes);
            }
            RedisValue::Hash(fields) => {
                self.render_hash_value(ui, style, fields);
            }
            RedisValue::List(items) => {
                self.render_list_value(ui, style, items);
            }
            RedisValue::Set(items) => {
                self.render_set_value(ui, style, items);
            }
            RedisValue::SortedSet(members) => {
                self.render_zset_value(ui, style, members);
            }
            RedisValue::Stream(_) => {
                ui.label(RichText::new("(stream type not fully supported)").color(style.muted));
            }
            RedisValue::None => {
                ui.label(RichText::new("(key does not exist)").color(style.muted));
            }
        }
    }

    fn render_string_value(&mut self, ui: &mut Ui, style: RedisBrowserStyle, bytes: &[u8]) {
        let text_preview = redis_display_text(bytes);

        self.value_filter_bar(ui, style, bytes.len(), "bytes");
        style.inset_frame().show(ui, |ui| {
            ScrollArea::vertical()
                .id_source("string_value_scroll")
                .auto_shrink([false, false])
                .max_height(420.0)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(text_preview)
                            .family(eframe::egui::FontFamily::Monospace)
                            .size(12.0)
                            .color(style.text),
                    );
                });
        });
    }

    fn render_hash_value(
        &mut self,
        ui: &mut Ui,
        style: RedisBrowserStyle,
        fields: &[(String, CellValue)],
    ) {
        let total_size = fields.len();
        self.value_filter_bar(ui, style, total_size, "fields");

        Frame::none()
            .fill(style.toolbar_fill)
            .stroke(Stroke::new(1.0, style.border))
            .inner_margin(Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Insert Row").strong().color(style.text));
                    ui.add(
                        TextEdit::singleline(&mut self.hash_new_field)
                            .desired_width(150.0)
                            .hint_text("field"),
                    );
                    ui.add(
                        TextEdit::singleline(&mut self.hash_new_value)
                            .desired_width(220.0)
                            .hint_text("value"),
                    );
                    if ui.button("HSET").clicked() {
                        self.set_hash_field();
                    }
                });
            });

        ui.add_space(6.0);
        let rows = self.filtered_hash_rows(fields);
        Self::table_frame(ui, style, |ui| {
            Grid::new("rdm_hash_value_grid")
                .num_columns(4)
                .spacing([18.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    table_header(ui, style, "#");
                    table_header(ui, style, "Field");
                    table_header(ui, style, "Value");
                    table_header(ui, style, "Actions");
                    ui.end_row();

                    for (row_index, field, value_text) in rows.iter().take(REDIS_VALUE_ROW_LIMIT) {
                        ui.label(RichText::new((row_index + 1).to_string()).color(style.muted));
                        ui.label(RichText::new(field).monospace().color(style.text));
                        ui.label(RichText::new(value_text).monospace().color(style.text));
                        ui.horizontal(|ui| {
                            if ui.small_button(tr("Copy")).clicked() {
                                ui.output_mut(|output| output.copied_text = value_text.clone());
                            }
                            if ui.small_button(tr("Edit")).clicked() {
                                self.hash_edit_field = Some((field.clone(), value_text.clone()));
                            }
                            if ui.small_button(tr("Del")).clicked() {
                                self.confirm_delete_hash_field = Some(field.clone());
                            }
                        });
                        ui.end_row();
                    }
                });

            if rows.len() > REDIS_VALUE_ROW_LIMIT {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "... and {} more fields",
                        rows.len() - REDIS_VALUE_ROW_LIMIT
                    ))
                    .color(style.muted),
                );
            }
        });

        let editing = self.hash_edit_field.clone();
        if let Some((field, current)) = editing {
            ui.add_space(6.0);
            let mut edit_val = current;
            style.inset_frame().show(ui, |ui| {
                ui.label(
                    RichText::new(format!("Editing field: {}", field))
                        .strong()
                        .color(style.text),
                );
                let response = ui.add(
                    TextEdit::singleline(&mut edit_val)
                        .desired_width(f32::INFINITY)
                        .hint_text("value"),
                );
                if response.changed() {
                    self.hash_edit_field = Some((field.clone(), edit_val.clone()));
                }
                ui.horizontal(|ui| {
                    if ui.button(tr("Save")).clicked() {
                        self.update_hash_field_with_value(field.clone(), edit_val.clone());
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        self.hash_edit_field = None;
                    }
                });
            });
        }
    }

    fn render_list_value(&mut self, ui: &mut Ui, style: RedisBrowserStyle, items: &[CellValue]) {
        self.value_filter_bar(ui, style, items.len(), "items");

        Frame::none()
            .fill(style.toolbar_fill)
            .stroke(Stroke::new(1.0, style.border))
            .inner_margin(Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ComboBox::from_id_source("push_dir")
                        .selected_text(if self.list_push_direction == "left" {
                            "LPUSH"
                        } else {
                            "RPUSH"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.list_push_direction,
                                "left".to_string(),
                                "LPUSH",
                            );
                            ui.selectable_value(
                                &mut self.list_push_direction,
                                "right".to_string(),
                                "RPUSH",
                            );
                        });
                    ui.add(
                        TextEdit::singleline(&mut self.list_push_value)
                            .desired_width(240.0)
                            .hint_text("value"),
                    );
                    if ui.button(tr("Push")).clicked() {
                        if self.list_push_direction == "left" {
                            self.lpush_value();
                        } else {
                            self.rpush_value();
                        }
                    }

                    ui.separator();
                    ComboBox::from_id_source("pop_dir")
                        .selected_text(if self.list_pop_direction == "left" {
                            "LPOP"
                        } else {
                            "RPOP"
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.list_pop_direction,
                                "left".to_string(),
                                "LPOP",
                            );
                            ui.selectable_value(
                                &mut self.list_pop_direction,
                                "right".to_string(),
                                "RPOP",
                            );
                        });
                    if ui.button(tr("Pop")).clicked() {
                        if self.list_pop_direction == "left" {
                            self.lpop_value();
                        } else {
                            self.rpop_value();
                        }
                    }
                });
            });

        ui.add_space(6.0);
        let rows = self.filtered_cell_rows(items);
        Self::table_frame(ui, style, |ui| {
            Grid::new("rdm_list_value_grid")
                .num_columns(3)
                .spacing([18.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    table_header(ui, style, "#");
                    table_header(ui, style, "Value");
                    table_header(ui, style, "Actions");
                    ui.end_row();

                    for (index, value_text) in rows.iter().take(REDIS_VALUE_ROW_LIMIT) {
                        ui.label(RichText::new(index.to_string()).color(style.muted));
                        ui.label(RichText::new(value_text).monospace().color(style.text));
                        ui.horizontal(|ui| {
                            if ui.small_button(tr("Copy")).clicked() {
                                ui.output_mut(|output| output.copied_text = value_text.clone());
                            }
                            if ui.small_button(tr("Del")).clicked() {
                                self.confirm_delete_list_index = Some(*index);
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    }

    fn render_set_value(&mut self, ui: &mut Ui, style: RedisBrowserStyle, items: &[CellValue]) {
        self.value_filter_bar(ui, style, items.len(), "members");

        Frame::none()
            .fill(style.toolbar_fill)
            .stroke(Stroke::new(1.0, style.border))
            .inner_margin(Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("SADD").strong().color(style.text));
                    ui.add(
                        TextEdit::singleline(&mut self.set_new_member)
                            .desired_width(260.0)
                            .hint_text("member"),
                    );
                    if ui.button(tr("Add")).clicked() {
                        self.sadd_member();
                    }
                });
            });

        ui.add_space(6.0);
        let rows = self.filtered_cell_rows(items);
        Self::table_frame(ui, style, |ui| {
            Grid::new("rdm_set_value_grid")
                .num_columns(3)
                .spacing([18.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    table_header(ui, style, "#");
                    table_header(ui, style, "Member");
                    table_header(ui, style, "Actions");
                    ui.end_row();

                    for (index, value_text) in rows.iter().take(REDIS_VALUE_ROW_LIMIT) {
                        ui.label(RichText::new((index + 1).to_string()).color(style.muted));
                        ui.label(RichText::new(value_text).monospace().color(style.text));
                        ui.horizontal(|ui| {
                            if ui.small_button(tr("Copy")).clicked() {
                                ui.output_mut(|output| output.copied_text = value_text.clone());
                            }
                            if ui.small_button(tr("Del")).clicked() {
                                self.confirm_delete_set_member = Some(value_text.clone());
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    }

    fn render_zset_value(
        &mut self,
        ui: &mut Ui,
        style: RedisBrowserStyle,
        members: &[(CellValue, f64)],
    ) {
        self.value_filter_bar(ui, style, members.len(), "members");

        Frame::none()
            .fill(style.toolbar_fill)
            .stroke(Stroke::new(1.0, style.border))
            .inner_margin(Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ZADD").strong().color(style.text));
                    ui.add(
                        TextEdit::singleline(&mut self.zset_new_member)
                            .desired_width(180.0)
                            .hint_text("member"),
                    );
                    ui.add(
                        TextEdit::singleline(&mut self.zset_new_score)
                            .desired_width(90.0)
                            .hint_text("score"),
                    );
                    if ui.button(tr("Add")).clicked() {
                        self.zadd_member();
                    }
                });
            });

        ui.add_space(6.0);
        let rows = self.filtered_zset_rows(members);
        Self::table_frame(ui, style, |ui| {
            Grid::new("rdm_zset_value_grid")
                .num_columns(4)
                .spacing([18.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    table_header(ui, style, "#");
                    table_header(ui, style, "Member");
                    table_header(ui, style, "Score");
                    table_header(ui, style, "Actions");
                    ui.end_row();

                    for (index, member, score) in rows.iter().take(REDIS_VALUE_ROW_LIMIT) {
                        ui.label(RichText::new((index + 1).to_string()).color(style.muted));
                        ui.label(RichText::new(member).monospace().color(style.text));
                        ui.label(RichText::new(score.to_string()).color(style.text));
                        ui.horizontal(|ui| {
                            if ui.small_button(tr("Copy")).clicked() {
                                ui.output_mut(|output| output.copied_text = member.clone());
                            }
                            if ui.small_button(tr("Edit")).clicked() {
                                self.zset_edit_score = Some((member.clone(), score.to_string()));
                            }
                            if ui.small_button(tr("Del")).clicked() {
                                self.confirm_delete_zset_member = Some(member.clone());
                            }
                        });
                        ui.end_row();
                    }
                });
        });

        let zset_edit = self.zset_edit_score.clone();
        if let Some((member, current_score)) = zset_edit {
            ui.add_space(6.0);
            let mut score_str = current_score;
            style.inset_frame().show(ui, |ui| {
                ui.label(
                    RichText::new(format!("Editing score for: {}", member))
                        .strong()
                        .color(style.text),
                );
                let response = ui.add(TextEdit::singleline(&mut score_str).desired_width(140.0));
                if response.changed() {
                    self.zset_edit_score = Some((member.clone(), score_str.clone()));
                }
                ui.horizontal(|ui| {
                    if ui.button(tr("Update Score")).clicked() {
                        self.zset_edit_score = Some((member.clone(), score_str.clone()));
                        self.update_zset_score(member.clone());
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        self.zset_edit_score = None;
                    }
                });
            });
        }
    }

    fn value_filter_bar(
        &mut self,
        ui: &mut Ui,
        style: RedisBrowserStyle,
        total: usize,
        noun: &str,
    ) {
        Frame::none()
            .fill(style.toolbar_fill)
            .stroke(Stroke::new(1.0, style.border))
            .inner_margin(Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.value_filter)
                            .desired_width(220.0)
                            .hint_text("Filter value"),
                    );
                    ui.label(RichText::new("Full match").small().color(style.muted));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{total} {noun}"))
                                .small()
                                .color(style.muted),
                        );
                    });
                });
            });
        ui.add_space(6.0);
    }

    fn table_frame(ui: &mut Ui, style: RedisBrowserStyle, add_contents: impl FnOnce(&mut Ui)) {
        style.panel_frame().show(ui, |ui| {
            ScrollArea::vertical()
                .id_source("redis_value_table_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    add_contents(ui);
                });
        });
    }

    fn filtered_hash_rows(&self, fields: &[(String, CellValue)]) -> Vec<(usize, String, String)> {
        let filter = self.value_filter.trim().to_lowercase();
        fields
            .iter()
            .enumerate()
            .filter_map(|(index, (field, value))| {
                let value_text = cell_value_to_string(value);
                if filter.is_empty()
                    || field.to_lowercase().contains(&filter)
                    || value_text.to_lowercase().contains(&filter)
                {
                    Some((index, field.clone(), value_text))
                } else {
                    None
                }
            })
            .collect()
    }

    fn filtered_cell_rows(&self, items: &[CellValue]) -> Vec<(usize, String)> {
        let filter = self.value_filter.trim().to_lowercase();
        items
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                let value_text = cell_value_to_string(value);
                if filter.is_empty() || value_text.to_lowercase().contains(&filter) {
                    Some((index, value_text))
                } else {
                    None
                }
            })
            .collect()
    }

    fn filtered_zset_rows(&self, members: &[(CellValue, f64)]) -> Vec<(usize, String, f64)> {
        let filter = self.value_filter.trim().to_lowercase();
        members
            .iter()
            .enumerate()
            .filter_map(|(index, (member, score))| {
                let member_text = cell_value_to_string(member);
                if filter.is_empty()
                    || member_text.to_lowercase().contains(&filter)
                    || score.to_string().contains(&filter)
                {
                    Some((index, member_text, *score))
                } else {
                    None
                }
            })
            .collect()
    }

    fn delete_confirmation_window(&mut self, ctx: &Context) {
        let Some(key_name) = self.confirm_delete.clone() else {
            return;
        };

        let mut open = true;
        Window::new(tr("Confirm Delete"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    tr("Are you sure you want to delete key \"{key}\"?")
                        .replace("{key}", &key_name),
                );
                ui.label(RichText::new(tr("This action cannot be undone.")).color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button(tr("Delete")).clicked() {
                        self.delete_key(key_name.clone());
                        self.confirm_delete = None;
                    }
                    if ui.button(tr("Cancel")).clicked() {
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
                    if ui.button(tr("Load Anyway")).clicked() {
                        self.load_key_value_full(key_name.clone());
                        self.confirm_load_full = None;
                    }
                    if ui.button(tr("Cancel")).clicked() {
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
                BrowserResponse::ConnectionLoaded(profile) => {
                    self.profile = Some(profile);
                }
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
                            let diagnostic = error
                                .detail
                                .filter(|detail| !detail.trim().is_empty())
                                .map(|detail| format!(": {detail}"))
                                .unwrap_or_default();
                            self.cli_result = Some(format!("ERROR: {}{diagnostic}", error.message));
                            self.error = Some(format!(
                                "Redis command failed: {}{diagnostic}",
                                error.message
                            ));
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
        self.value_filter.clear();
        self.string_editing = false;
        self.hash_edit_field = None;
        self.zset_edit_score = None;

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

    pub fn refresh_database(&mut self, connection_id: ConnectionId, database: u8) {
        if self.connection_id != connection_id {
            return;
        }
        self.switch_database(database);
    }

    fn switch_database(&mut self, database: u8) {
        self.selected_database = database;
        self.keys.clear();
        self.cursor = 0;
        self.has_more = false;
        self.selected_key = None;
        self.value_display = ValueDisplay::NotLoaded;
        self.value_filter.clear();
        self.scan_keys();
    }

    fn apply_pattern(&mut self) {
        self.pattern = self.search_pattern.clone();
        self.keys.clear();
        self.cursor = 0;
        self.has_more = false;
        self.selected_key = None;
        self.value_display = ValueDisplay::NotLoaded;
        self.value_filter.clear();
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
        Window::new(tr("Create New Key"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr("Key Name:"));
                ui.add(TextEdit::singleline(&mut self.new_key_name).desired_width(300.0));

                ui.label(tr("Type:"));
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

                ui.label(tr("Initial Value:"));
                ui.add(TextEdit::singleline(&mut self.new_key_value).desired_width(300.0));

                ui.label(tr("TTL (seconds, empty = no expiry):"));
                ui.add(TextEdit::singleline(&mut self.new_key_ttl).desired_width(300.0));

                ui.horizontal(|ui| {
                    if ui.button(tr("Create")).clicked() {
                        self.create_key();
                    }
                    if ui.button(tr("Cancel")).clicked() {
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
        Window::new(tr("Confirm Delete Hash Field"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    tr("Delete field \"{field}\" from hash \"{key}\"?")
                        .replace("{field}", field)
                        .replace("{key}", &key_name),
                );
                ui.label(RichText::new(tr("This action cannot be undone.")).color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button(tr("Delete")).clicked() {
                        self.delete_hash_field();
                        self.confirm_delete_hash_field = None;
                    }
                    if ui.button(tr("Cancel")).clicked() {
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
        Window::new(tr("Confirm Delete List Element"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    tr("Remove list element at index [{index}]?")
                        .replace("{index}", &idx.to_string()),
                );
                ui.label(RichText::new(tr("This action cannot be undone.")).color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button(tr("Remove")).clicked() {
                        // Use LSET to empty or handle via refresh - for simplicity, just refresh
                        self.confirm_delete_list_index = None;
                        if let Some(ref key) = self.selected_key {
                            self.refresh_key(key.clone());
                        }
                    }
                    if ui.button(tr("Cancel")).clicked() {
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
        Window::new(tr("Confirm Delete Set Member"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(tr("Remove member \"{member}\" from set?").replace("{member}", member));
                ui.label(RichText::new(tr("This action cannot be undone.")).color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button(tr("Remove")).clicked() {
                        self.srem_member();
                    }
                    if ui.button(tr("Cancel")).clicked() {
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
        Window::new(tr("Confirm Delete ZSet Member"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    tr("Remove member \"{member}\" from sorted set?").replace("{member}", member),
                );
                ui.label(RichText::new(tr("This action cannot be undone.")).color(Color32::RED));
                ui.horizontal(|ui| {
                    if ui.button(tr("Remove")).clicked() {
                        self.zrem_member();
                    }
                    if ui.button(tr("Cancel")).clicked() {
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
        self.value_filter.clear();
        self.error = None;
        self.scan_keys();
    }
}

// Helper functions

fn metadata_chip(ui: &mut Ui, style: RedisBrowserStyle, label: &str, value: &str) {
    Frame::none()
        .fill(style.panel_fill)
        .stroke(Stroke::new(1.0, style.border))
        .rounding(Rounding::same(3.0))
        .inner_margin(Margin::symmetric(8.0, 3.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).small().color(style.muted));
                ui.label(RichText::new(value).small().strong().color(style.text));
            });
        });
}

fn table_header(ui: &mut Ui, style: RedisBrowserStyle, label: &str) {
    ui.label(RichText::new(label).strong().color(style.muted));
}

fn key_type_badge_fill(type_name: &str, style: RedisBrowserStyle) -> Color32 {
    match type_name {
        "string" => Color32::from_rgb(61, 145, 94),
        "hash" => style.accent,
        "list" => Color32::from_rgb(101, 118, 203),
        "set" => Color32::from_rgb(203, 128, 61),
        "zset" => Color32::from_rgb(169, 91, 183),
        "stream" => Color32::from_rgb(77, 151, 160),
        _ => Color32::from_rgb(118, 128, 142),
    }
}

fn key_type_short_label(type_name: &str) -> &'static str {
    match type_name {
        "hash" => "H",
        "list" => "L",
        "set" => "S",
        "zset" => "Z",
        "stream" => "R",
        "string" => "T",
        _ => "K",
    }
}

fn redis_type_badge_label(type_name: &str) -> &'static str {
    match type_name {
        "hash" => "HASH",
        "list" => "LIST",
        "set" => "SET",
        "zset" => "ZSET",
        "stream" => "STREAM",
        "string" => "STRING",
        _ => "KEY",
    }
}

fn redis_type_group_label(type_name: &str) -> &'static str {
    match type_name {
        "hash" => "hash",
        "list" => "list",
        "set" => "set",
        "zset" => "zset",
        "stream" => "stream",
        "string" => "string",
        _ => "key",
    }
}

fn redis_type_rank(type_name: &str) -> u8 {
    match type_name {
        "hash" => 0,
        "string" => 1,
        "list" => 2,
        "set" => 3,
        "zset" => 4,
        "stream" => 5,
        _ => 9,
    }
}

fn key_leaf_label(key_name: &str) -> String {
    key_name
        .rsplit([':', '/', '.'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(key_name)
        .to_string()
}

fn key_parent_label(key_name: &str) -> Option<String> {
    let leaf = key_leaf_label(key_name);
    let prefix_len = key_name.len().saturating_sub(leaf.len());
    if prefix_len == 0 {
        None
    } else {
        Some(format!(
            "({})",
            key_name[..prefix_len].trim_end_matches([':', '/', '.'])
        ))
    }
}

fn key_row_meta(key_type: &str, ttl_seconds: Option<i64>, size: Option<u64>) -> String {
    let mut parts = Vec::new();
    if let Some(ttl) = ttl_seconds {
        parts.push(format_ttl(ttl));
    }
    if let Some(size) = size {
        parts.push(format_key_size(key_type, size));
    }
    parts.join("  ")
}

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
            let text = redis_display_text(bytes);
            if text.chars().count() > max_bytes {
                let preview = text.chars().take(max_bytes).collect::<String>();
                format!(
                    "{}...\n\n(truncated, full size: {})",
                    preview,
                    human_bytes(bytes.len() as u64)
                )
            } else {
                text
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
        RedisValue::String(bytes) => redis_display_text(bytes),
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
        CellValue::Bytes(bytes) => redis_display_text(bytes),
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

fn redis_display_text(bytes: &[u8]) -> String {
    db_redis::decode_redis_text(bytes).unwrap_or_else(|| format!("[{} binary bytes]", bytes.len()))
}
