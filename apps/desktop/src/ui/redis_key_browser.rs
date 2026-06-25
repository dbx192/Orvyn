use db_common::{CellValue, RedisKeyInfo, RedisValue};
use eframe::egui::{
    Color32, Frame, Label, Layout, RichText, ScrollArea, TextEdit, Ui,
};

#[derive(Debug, Default, Clone)]
struct PaginationState {
    offset: usize,
    page_size: usize,
    total: Option<usize>,
}

impl PaginationState {
    fn current_page(&self) -> usize {
        self.offset / self.page_size + 1
    }

    fn total_pages(&self) -> Option<usize> {
        self.total
            .map(|t| t.div_ceil(self.page_size.max(1)))
    }

    fn can_go_prev(&self) -> bool {
        self.offset > 0
    }

    fn can_go_next(&self) -> bool {
        match self.total {
            Some(total) => self.offset + self.page_size < total,
            None => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListPushDirection {
    Left,
    Right,
}

impl Default for ListPushDirection {
    fn default() -> Self {
        ListPushDirection::Right
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Browse,
    KeyDetail,
}

impl Default for ViewMode {
    fn default() -> Self {
        ViewMode::Browse
    }
}

pub struct RedisKeyBrowser {
    connection_label: String,
    database_index: u8,
    selected_key: Option<String>,
    selected_key_info: Option<RedisKeyInfo>,
    value: Option<RedisValue>,
    loading: bool,
    error: Option<String>,
    confirm_delete_key: bool,
    confirm_delete_hash_field: Option<String>,
    confirm_delete_set_member: Option<String>,
    confirm_delete_zset_member: Option<String>,
    confirm_delete_list_index: Option<usize>,
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
    list_pagination: PaginationState,
    list_new_value: String,
    list_push_direction: ListPushDirection,
    set_new_member: String,
    zset_pagination: PaginationState,
    zset_new_member: String,
    zset_new_score: String,
    zset_edit_score: Option<(String, String)>,
    view_mode: ViewMode,
}

impl RedisKeyBrowser {
    pub fn new(connection_label: impl Into<String>, database_index: u8) -> Self {
        Self {
            connection_label: connection_label.into(),
            database_index,
            selected_key: None,
            selected_key_info: None,
            value: None,
            loading: false,
            error: None,
            confirm_delete_key: false,
            confirm_delete_hash_field: None,
            confirm_delete_set_member: None,
            confirm_delete_zset_member: None,
            confirm_delete_list_index: None,
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
            list_pagination: PaginationState {
                offset: 0,
                page_size: 100,
                total: None,
            },
            list_new_value: String::new(),
            list_push_direction: ListPushDirection::default(),
            set_new_member: String::new(),
            zset_pagination: PaginationState {
                offset: 0,
                page_size: 100,
                total: None,
            },
            zset_new_member: String::new(),
            zset_new_score: String::new(),
            zset_edit_score: None,
            view_mode: ViewMode::Browse,
        }
    }

    pub fn with_sample_data(connection_label: impl Into<String>, database_index: u8) -> Self {
        let mut browser = Self::new(connection_label, database_index);
        browser.selected_key = Some("sample:user:1001".to_string());
        browser.selected_key_info = Some(RedisKeyInfo {
            name: "sample:user:1001".to_string(),
            type_name: "hash".to_string(),
            ttl_seconds: Some(3600),
            size_hint: Some(5),
        });
        browser.value = Some(RedisValue::Hash(vec![
            ("id".to_string(), CellValue::Text("1001".to_string())),
            ("name".to_string(), CellValue::Text("alice".to_string())),
            (
                "email".to_string(),
                CellValue::Text("alice@example.com".to_string()),
            ),
            (
                "created_at".to_string(),
                CellValue::Text("2024-01-15 10:30:00".to_string()),
            ),
            (
                "status".to_string(),
                CellValue::Text("active".to_string()),
            ),
        ]));
        browser.view_mode = ViewMode::KeyDetail;
        browser
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        match self.view_mode {
            ViewMode::Browse => self.browse_view(ui),
            ViewMode::KeyDetail => self.key_detail_view(ui),
        }
    }

    fn browse_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} — DB {}",
                    self.connection_label, self.database_index
                ))
                .strong(),
            );
            ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                if ui.button("＋ New Key").clicked() {
                    self.open_new_key_dialog();
                }
            });
        });
        ui.separator();

        ui.label(
            RichText::new("Click a key in the tree to view its value")
                .color(Color32::GRAY)
                .italics(),
        );

        // Sample key list for demo
        ui.add_space(16.0);
        ui.label(RichText::new("Sample Keys (demo)").strong().color(Color32::GRAY));
        ui.separator();

        let sample_keys = vec![
            ("sample:string", "string", Some(3600), Some(12)),
            ("sample:user:1001", "hash", Some(3600), Some(5)),
            ("sample:list:recent", "list", None, Some(50)),
            ("sample:set:tags", "set", Some(7200), Some(23)),
            ("sample:zset:leaderboard", "zset", None, Some(1000)),
        ];

        for (key, kind, ttl, size) in sample_keys {
            let response = ui.add(
                Label::new(
                    RichText::new(format!("🔑 {}  [{}]", key, kind))
                        .monospace(),
                )
                .sense(eframe::egui::Sense::click()),
            );
            if response.clicked() {
                self.load_sample_key(key);
            }
            response.clone().on_hover_text(format!(
                "Type: {}\nTTL: {}\nSize: {}",
                kind,
                ttl.map(|t| format!("{}s", t)).unwrap_or_else(|| "∞".to_string()),
                size.map(|s| s.to_string()).unwrap_or_else(|| "?".to_string()),
            ));
        }

        self.new_key_dialog(ui);
    }

    fn load_sample_key(&mut self, key: &str) {
        self.selected_key = Some(key.to_string());
        match key {
            "sample:string" => {
                self.selected_key_info = Some(RedisKeyInfo {
                    name: key.to_string(),
                    type_name: "string".to_string(),
                    ttl_seconds: Some(3600),
                    size_hint: Some(12),
                });
                self.value = Some(RedisValue::String(b"hello world".to_vec()));
            }
            "sample:user:1001" => {
                self.selected_key_info = Some(RedisKeyInfo {
                    name: key.to_string(),
                    type_name: "hash".to_string(),
                    ttl_seconds: Some(3600),
                    size_hint: Some(5),
                });
                self.value = Some(RedisValue::Hash(vec![
                    ("id".to_string(), CellValue::Text("1001".to_string())),
                    ("name".to_string(), CellValue::Text("alice".to_string())),
                    (
                        "email".to_string(),
                        CellValue::Text("alice@example.com".to_string()),
                    ),
                    (
                        "created_at".to_string(),
                        CellValue::Text("2024-01-15 10:30:00".to_string()),
                    ),
                    (
                        "status".to_string(),
                        CellValue::Text("active".to_string()),
                    ),
                ]));
            }
            "sample:list:recent" => {
                self.selected_key_info = Some(RedisKeyInfo {
                    name: key.to_string(),
                    type_name: "list".to_string(),
                    ttl_seconds: None,
                    size_hint: Some(50),
                });
                let mut items = Vec::new();
                for i in 1..=50 {
                    items.push(CellValue::Text(format!("item_{:03}", i)));
                }
                self.value = Some(RedisValue::List(items));
                self.list_pagination = PaginationState {
                    offset: 0,
                    page_size: 100,
                    total: Some(50),
                };
            }
            "sample:set:tags" => {
                self.selected_key_info = Some(RedisKeyInfo {
                    name: key.to_string(),
                    type_name: "set".to_string(),
                    ttl_seconds: Some(7200),
                    size_hint: Some(23),
                });
                let members = vec![
                    "rust", "python", "go", "java", "javascript", "typescript", "c++", "c#",
                    "ruby", "php", "swift", "kotlin", "dart", "elixir", "haskell", "scala",
                    "clojure", "erlang", "lua", "perl", "bash", "sql", "redis",
                ];
                self.value = Some(RedisValue::Set(
                    members.into_iter().map(|m| CellValue::Text(m.to_string())).collect(),
                ));
            }
            "sample:zset:leaderboard" => {
                self.selected_key_info = Some(RedisKeyInfo {
                    name: key.to_string(),
                    type_name: "zset".to_string(),
                    ttl_seconds: None,
                    size_hint: Some(1000),
                });
                let mut items = Vec::new();
                for i in 1..=100 {
                    items.push((
                        CellValue::Text(format!("user_{:04}", i)),
                        (1000 - i) as f64,
                    ));
                }
                self.value = Some(RedisValue::SortedSet(items));
                self.zset_pagination = PaginationState {
                    offset: 0,
                    page_size: 100,
                    total: Some(100),
                };
            }
            _ => {}
        }
        self.view_mode = ViewMode::KeyDetail;
        self.loading = false;
        self.error = None;
        self.string_editing = false;
        self.hash_edit_field = None;
        self.ttl_editing = false;
        self.zset_edit_score = None;
    }

    fn key_detail_view(&mut self, ui: &mut Ui) {
        self.key_header(ui);
        ui.separator();

        if let Some(value) = self.value.clone() {
            match value {
                RedisValue::String(bytes) => self.render_string_view(ui, bytes),
                RedisValue::Hash(fields) => self.render_hash_view(ui, fields),
                RedisValue::List(items) => self.render_list_view(ui, items),
                RedisValue::Set(members) => self.render_set_view(ui, members),
                RedisValue::SortedSet(items) => self.render_zset_view(ui, items),
                RedisValue::None => {
                    ui.label(RichText::new("Key does not exist").color(Color32::RED));
                }
                RedisValue::Stream(_) => {
                    ui.label("Stream type - preview not available");
                }
            }
        } else if self.loading {
            ui.label(
                RichText::new("Loading...")
                    .color(Color32::GRAY)
                    .italics(),
            );
        } else if let Some(err) = &self.error {
            ui.colored_label(Color32::RED, format!("Error: {}", err));
        } else {
            ui.label(
                RichText::new("Select a key to view")
                    .color(Color32::GRAY)
                    .italics(),
            );
        }

        self.new_key_dialog(ui);
        self.delete_confirm_dialogs(ui);
    }

    fn key_header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui.button("← Back").clicked() {
                self.view_mode = ViewMode::Browse;
            }
            ui.separator();
            if let Some(key_name) = &self.selected_key {
                ui.label(RichText::new(key_name).strong().monospace());
            }
            ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                if ui
                    .button("🗑 Delete")
                    .on_hover_text("Delete this key")
                    .clicked()
                {
                    self.confirm_delete_key = true;
                }
                if ui.button("⟳ Refresh").clicked() {
                    self.refresh_value();
                }
            });
        });

        ui.horizontal(|ui| {
            let (type_name, ttl, size) = if let Some(info) = &self.selected_key_info {
                (Some(info.type_name.clone()), info.ttl_seconds, info.size_hint)
            } else {
                (None, None, None)
            };

            if let Some(type_name) = type_name {
                ui.label(format!("Type: {}", type_name));
                ui.separator();
                if let Some(ttl) = ttl {
                    self.ttl_display(ui, ttl);
                } else {
                    ui.label("TTL: ∞ (persistent)");
                }
                ui.separator();
                if let Some(size) = size {
                    ui.label(format!("Size: {}", size));
                }
            }
        });
    }

    fn ttl_display(&mut self, ui: &mut Ui, ttl: i64) {
        if self.ttl_editing {
            ui.horizontal(|ui| {
                ui.label("TTL (sec):");
                ui.add(
                    TextEdit::singleline(&mut self.ttl_edit)
                        .desired_width(80.0)
                        .hint_text("seconds"),
                );
                if ui.button("Set").clicked() {
                    self.apply_ttl_edit();
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
        ui.label(RichText::new("String Value").strong());
        ui.add_space(4.0);

        let is_text = std::str::from_utf8(&bytes).is_ok();
        let display_text = if is_text {
            String::from_utf8_lossy(&bytes).to_string()
        } else {
            format!("[binary {} bytes]", bytes.len())
        };

        if self.string_editing {
            let mut text = self.string_edit_value.clone();
            let _response = ui.add(
                TextEdit::multiline(&mut text)
                    .desired_rows(10)
                    .desired_width(f32::INFINITY),
            );
            self.string_edit_value = text;

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
                ScrollArea::vertical()
                    .id_source("string_value_scroll")
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(&display_text).monospace());
                    });
            });

            ui.horizontal(|ui| {
                if ui.button("Edit").clicked() {
                    self.string_edit_value = display_text.clone();
                    self.string_editing = true;
                }
                if ui.button("Copy").clicked() {
                    ui.output_mut(|o| o.copied_text = display_text.clone());
                }
                if !is_text {
                    if ui.button("View as Base64").clicked() {
                        let b64 = base64_encode(&bytes);
                        ui.output_mut(|o| o.copied_text = b64);
                    }
                }
            });
        }
    }

    fn render_hash_view(&mut self, ui: &mut Ui, fields: Vec<(String, CellValue)>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Hash — {} fields", fields.len())).strong());
        });
        ui.add_space(4.0);

        let available_height = ui.available_height() - 120.0;

        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_source("hash_fields_scroll")
                .max_height(available_height.max(100.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (idx, (field, value)) in fields.iter().enumerate() {
                        self.render_hash_row(ui, field, value, idx);
                    }
                });
        });

        ui.add_space(4.0);

        // Add new field
        ui.horizontal(|ui| {
            ui.label("New:");
            ui.add(
                TextEdit::singleline(&mut self.hash_new_field)
                    .desired_width(150.0)
                    .hint_text("field name"),
            );
            ui.label("Value:");
            ui.add(
                TextEdit::singleline(&mut self.hash_new_value)
                    .desired_width(200.0)
                    .hint_text("value"),
            );
            if ui.button("+ Add").clicked() {
                self.add_hash_field();
            }
        });
    }

    fn render_hash_row(&mut self, ui: &mut Ui, field: &str, value: &CellValue, _idx: usize) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(field).strong().monospace());
            ui.separator();

            let is_editing = self
                .hash_edit_field
                .as_ref()
                .map(|(f, _)| f == field)
                .unwrap_or(false);

            if is_editing {
                if let Some((_f, ref mut val)) = &mut self.hash_edit_field {
                    ui.add(
                        TextEdit::singleline(val)
                            .desired_width(ui.available_width() - 120.0)
                            .hint_text("value"),
                    );
                }
                if ui.button("Save").clicked() {
                    self.save_hash_field_edit();
                }
                if ui.button("Cancel").clicked() {
                    self.hash_edit_field = None;
                }
            } else {
                let display = value.display();
                ui.label(
                    RichText::new(&display)
                        .monospace()
                        .color(if value.is_null() {
                            Color32::GRAY
                        } else {
                            Color32::default()
                        }),
                );
                ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                    if ui.small_button("🗑").on_hover_text("Delete field").clicked() {
                        self.confirm_delete_hash_field = Some(field.to_string());
                    }
                    if ui.small_button("Edit").clicked() {
                        self.start_hash_edit(field, value);
                    }
                });
            }
        });
    }

    fn render_list_view(&mut self, ui: &mut Ui, items: Vec<CellValue>) {
        let total = items.len();
        self.list_pagination.total = Some(total);

        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("List — {} items", total)).strong());
            ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                self.pagination_controls(ui);
            });
        });
        ui.add_space(4.0);

        let start = self.list_pagination.offset;
        let end = (start + self.list_pagination.page_size).min(total);
        let page_items: Vec<(usize, &CellValue)> = items[start..end]
            .iter()
            .enumerate()
            .map(|(i, v)| (start + i, v))
            .collect();

        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_source("list_items_scroll")
                .max_height(300.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (idx, value) in page_items {
                        self.render_list_row(ui, idx, value);
                    }
                });
        });

        ui.add_space(4.0);

        // Push controls
        ui.horizontal(|ui| {
            ui.label("Push:");
            ui.selectable_value(
                &mut self.list_push_direction,
                ListPushDirection::Left,
                "Left (LPUSH)",
            );
            ui.selectable_value(
                &mut self.list_push_direction,
                ListPushDirection::Right,
                "Right (RPUSH)",
            );
        });
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.list_new_value)
                    .desired_width(300.0)
                    .hint_text("new element value"),
            );
            if ui.button("Push").clicked() {
                self.push_list_element();
            }
            if ui.button("Pop Left").clicked() {
                self.pop_list_left();
            }
            if ui.button("Pop Right").clicked() {
                self.pop_list_right();
            }
        });
    }

    fn render_list_row(&mut self, ui: &mut Ui, index: usize, value: &CellValue) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("[{}]", index)).color(Color32::GRAY));
            ui.separator();
            let display = value.display();
            ui.label(RichText::new(&display).monospace());
            ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                if ui
                    .small_button("🗑")
                    .on_hover_text("Remove element")
                    .clicked()
                {
                    self.confirm_delete_list_index = Some(index);
                }
            });
        });
    }

    fn render_set_view(&mut self, ui: &mut Ui, members: Vec<CellValue>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Set — {} members", members.len())).strong());
        });
        ui.add_space(4.0);

        Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_source("set_members_scroll")
                .max_height(300.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for member in &members {
                        self.render_set_row(ui, member);
                    }
                });
        });

        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.set_new_member)
                    .desired_width(300.0)
                    .hint_text("new member"),
            );
            if ui.button("+ Add Member").clicked() {
                self.add_set_member();
            }
        });
    }

    fn render_set_row(&mut self, ui: &mut Ui, member: &CellValue) {
        ui.horizontal(|ui| {
            let display = member.display();
            ui.label(RichText::new(&display).monospace());
            ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                if ui
                    .small_button("🗑")
                    .on_hover_text("Remove member")
                    .clicked()
                {
                    self.confirm_delete_set_member = Some(display.clone());
                }
            });
        });
    }

    fn render_zset_view(&mut self, ui: &mut Ui, items: Vec<(CellValue, f64)>) {
        let total = items.len();
        self.zset_pagination.total = Some(total);

        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Sorted Set — {} members", total)).strong());
            ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                self.zset_pagination_controls(ui);
            });
        });
        ui.add_space(4.0);

        let start = self.zset_pagination.offset;
        let end = (start + self.zset_pagination.page_size).min(total);
        let page_items: Vec<(usize, &(CellValue, f64))> = items[start..end]
            .iter()
            .enumerate()
            .map(|(i, v)| (start + i, v))
            .collect();

        Frame::group(ui.style()).show(ui, |ui| {
            // Header row
            ui.horizontal(|ui| {
                ui.label(RichText::new("Rank").strong().color(Color32::GRAY));
                ui.separator();
                ui.label(RichText::new("Member").strong().color(Color32::GRAY));
                ui.separator();
                ui.label(RichText::new("Score").strong().color(Color32::GRAY));
            });
            ui.separator();

            ScrollArea::vertical()
                .id_source("zset_members_scroll")
                .max_height(260.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (rank, (member, score)) in page_items {
                        self.render_zset_row(ui, rank, member, *score);
                    }
                });
        });

        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Member:");
            ui.add(
                TextEdit::singleline(&mut self.zset_new_member)
                    .desired_width(200.0)
                    .hint_text("member name"),
            );
            ui.label("Score:");
            ui.add(
                TextEdit::singleline(&mut self.zset_new_score)
                    .desired_width(100.0)
                    .hint_text("0.0"),
            );
            if ui.button("+ Add").clicked() {
                self.add_zset_member();
            }
        });
    }

    fn render_zset_row(&mut self, ui: &mut Ui, rank: usize, member: &CellValue, score: f64) {
        let member_str = member.display();
        let is_editing = self
            .zset_edit_score
            .as_ref()
            .map(|(m, _)| m == &member_str)
            .unwrap_or(false);

        ui.horizontal(|ui| {
            ui.label(format!("#{}", rank + 1));
            ui.separator();
            ui.label(RichText::new(&member_str).monospace());
            ui.separator();

            if is_editing {
                if let Some((_m, ref mut val)) = &mut self.zset_edit_score {
                    ui.add(
                        TextEdit::singleline(val)
                            .desired_width(100.0)
                            .hint_text("score"),
                    );
                }
                if ui.button("Save").clicked() {
                    self.save_zset_score_edit();
                }
                if ui.button("Cancel").clicked() {
                    self.zset_edit_score = None;
                }
            } else {
                ui.label(format!("{}", score));
                ui.with_layout(Layout::right_to_left(eframe::egui::Align::Center), |ui| {
                    if ui
                        .small_button("🗑")
                        .on_hover_text("Remove member")
                        .clicked()
                    {
                        self.confirm_delete_zset_member = Some(member_str.clone());
                    }
                    if ui.small_button("Edit").clicked() {
                        self.zset_edit_score = Some((member_str.clone(), score.to_string()));
                    }
                });
            }
        });
    }

    fn pagination_controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.list_pagination.can_go_prev(), eframe::egui::Button::new("◀"))
                .clicked()
            {
                self.list_pagination.offset = self
                    .list_pagination
                    .offset
                    .saturating_sub(self.list_pagination.page_size);
            }
            let page_info = match self.list_pagination.total_pages() {
                Some(total) => format!(
                    "{}/{}",
                    self.list_pagination.current_page(),
                    total
                ),
                None => format!("{}", self.list_pagination.current_page()),
            };
            ui.label(RichText::new(page_info).small());
            if ui
                .add_enabled(
                    self.list_pagination.can_go_next(),
                    eframe::egui::Button::new("▶"),
                )
                .clicked()
            {
                self.list_pagination.offset += self.list_pagination.page_size;
            }
        });
    }

    fn zset_pagination_controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.zset_pagination.can_go_prev(),
                    eframe::egui::Button::new("◀"),
                )
                .clicked()
            {
                self.zset_pagination.offset = self
                    .zset_pagination
                    .offset
                    .saturating_sub(self.zset_pagination.page_size);
            }
            let page_info = match self.zset_pagination.total_pages() {
                Some(total) => format!(
                    "{}/{}",
                    self.zset_pagination.current_page(),
                    total
                ),
                None => format!("{}", self.zset_pagination.current_page()),
            };
            ui.label(RichText::new(page_info).small());
            if ui
                .add_enabled(
                    self.zset_pagination.can_go_next(),
                    eframe::egui::Button::new("▶"),
                )
                .clicked()
            {
                self.zset_pagination.offset += self.zset_pagination.page_size;
            }
        });
    }

    fn open_new_key_dialog(&mut self) {
        self.new_key_dialog = true;
        self.new_key_name = String::new();
        self.new_key_type = "string".to_string();
        self.new_key_value = String::new();
        self.new_key_ttl = String::new();
    }

    fn new_key_dialog(&mut self, ui: &mut Ui) {
        if !self.new_key_dialog {
            return;
        }

        let mut open = self.new_key_dialog;
        eframe::egui::Window::new("New Redis Key")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ui.ctx(), |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("Key Name:");
                        ui.add(
                            TextEdit::singleline(&mut self.new_key_name)
                                .desired_width(250.0)
                                .hint_text("my:key:name"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Type:");
                        eframe::egui::ComboBox::from_id_source("new_key_type")
                            .selected_text(&self.new_key_type)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.new_key_type,
                                    "string".to_string(),
                                    "String",
                                );
                                ui.selectable_value(
                                    &mut self.new_key_type,
                                    "hash".to_string(),
                                    "Hash",
                                );
                                ui.selectable_value(
                                    &mut self.new_key_type,
                                    "list".to_string(),
                                    "List",
                                );
                                ui.selectable_value(
                                    &mut self.new_key_type,
                                    "set".to_string(),
                                    "Set",
                                );
                                ui.selectable_value(
                                    &mut self.new_key_type,
                                    "zset".to_string(),
                                    "Sorted Set",
                                );
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Value:");
                        ui.add(
                            TextEdit::singleline(&mut self.new_key_value)
                                .desired_width(250.0)
                                .hint_text("initial value"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("TTL (sec):");
                        ui.add(
                            TextEdit::singleline(&mut self.new_key_ttl)
                                .desired_width(100.0)
                                .hint_text("empty = persistent"),
                        );
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
            });
        self.new_key_dialog = open;
    }

    fn delete_confirm_dialogs(&mut self, ui: &mut Ui) {
        // Delete key confirmation
        if self.confirm_delete_key {
            let mut open = self.confirm_delete_key;
            eframe::egui::Window::new("Confirm Delete")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    if let Some(key) = &self.selected_key {
                        ui.label(
                            RichText::new(format!("Delete key \"{}\"?", key))
                                .color(Color32::RED),
                        );
                        ui.label("This action cannot be undone.");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Cancel").clicked() {
                                self.confirm_delete_key = false;
                            }
                            if ui.button("Delete").clicked() {
                                self.do_delete_key();
                            }
                        });
                    }
                });
            self.confirm_delete_key = open;
        }

        // Delete hash field confirmation
        if let Some(field) = self.confirm_delete_hash_field.clone() {
            let mut open = true;
            eframe::egui::Window::new("Confirm Delete Field")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(
                        RichText::new(format!("Delete field \"{}\"?", field))
                            .color(Color32::RED),
                    );
                    ui.label("This action cannot be undone.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete_hash_field = None;
                        }
                        if ui.button("Delete").clicked() {
                            self.do_delete_hash_field(field.clone());
                        }
                    });
                });
            if !open {
                self.confirm_delete_hash_field = None;
            }
        }

        // Delete set member confirmation
        if let Some(member) = self.confirm_delete_set_member.clone() {
            let mut open = true;
            eframe::egui::Window::new("Confirm Remove Member")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(
                        RichText::new(format!("Remove member \"{}\"?", member))
                            .color(Color32::RED),
                    );
                    ui.label("This action cannot be undone.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete_set_member = None;
                        }
                        if ui.button("Remove").clicked() {
                            self.do_delete_set_member(member.clone());
                        }
                    });
                });
            if !open {
                self.confirm_delete_set_member = None;
            }
        }

        // Delete zset member confirmation
        if let Some(member) = self.confirm_delete_zset_member.clone() {
            let mut open = true;
            eframe::egui::Window::new("Confirm Remove Member")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(
                        RichText::new(format!("Remove member \"{}\"?", member))
                            .color(Color32::RED),
                    );
                    ui.label("This action cannot be undone.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete_zset_member = None;
                        }
                        if ui.button("Remove").clicked() {
                            self.do_delete_zset_member(member.clone());
                        }
                    });
                });
            if !open {
                self.confirm_delete_zset_member = None;
            }
        }

        // Delete list element confirmation
        if let Some(idx) = self.confirm_delete_list_index {
            let mut open = true;
            eframe::egui::Window::new("Confirm Remove Element")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(
                        RichText::new(format!("Remove element at index {}?", idx))
                            .color(Color32::RED),
                    );
                    ui.label("This action cannot be undone.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete_list_index = None;
                        }
                        if ui.button("Remove").clicked() {
                            self.do_delete_list_index(idx);
                        }
                    });
                });
            if !open {
                self.confirm_delete_list_index = None;
            }
        }
    }

    fn refresh_value(&mut self) {
        self.loading = true;
        self.error = None;
    }

    fn save_string_value(&mut self) {
        self.string_editing = false;
    }

    fn start_hash_edit(&mut self, field: &str, value: &CellValue) {
        self.hash_edit_field = Some((field.to_string(), value.display()));
    }

    fn save_hash_field_edit(&mut self) {
        self.hash_edit_field = None;
    }

    fn add_hash_field(&mut self) {
        self.hash_new_field.clear();
        self.hash_new_value.clear();
    }

    fn apply_ttl_edit(&mut self) {
        self.ttl_editing = false;
    }

    fn remove_ttl(&mut self) {
        self.ttl_editing = false;
    }

    fn push_list_element(&mut self) {
        self.list_new_value.clear();
    }

    fn pop_list_left(&mut self) {}

    fn pop_list_right(&mut self) {}

    fn add_set_member(&mut self) {
        self.set_new_member.clear();
    }

    fn add_zset_member(&mut self) {
        self.zset_new_member.clear();
        self.zset_new_score.clear();
    }

    fn save_zset_score_edit(&mut self) {
        self.zset_edit_score = None;
    }

    fn create_new_key(&mut self) {
        self.new_key_dialog = false;
    }

    fn do_delete_key(&mut self) {
        self.confirm_delete_key = false;
        self.selected_key = None;
        self.selected_key_info = None;
        self.value = None;
        self.view_mode = ViewMode::Browse;
    }

    fn do_delete_hash_field(&mut self, field: String) {
        let _ = field;
        self.confirm_delete_hash_field = None;
    }

    fn do_delete_set_member(&mut self, member: String) {
        let _ = member;
        self.confirm_delete_set_member = None;
    }

    fn do_delete_zset_member(&mut self, member: String) {
        let _ = member;
        self.confirm_delete_zset_member = None;
    }

    fn do_delete_list_index(&mut self, index: usize) {
        let _ = index;
        self.confirm_delete_list_index = None;
    }

    pub fn set_selected_key(
        &mut self,
        key: String,
        info: RedisKeyInfo,
        value: RedisValue,
    ) {
        self.selected_key = Some(key);
        self.selected_key_info = Some(info);
        self.value = Some(value);
        self.view_mode = ViewMode::KeyDetail;
        self.loading = false;
        self.error = None;
        self.string_editing = false;
        self.hash_edit_field = None;
        self.ttl_editing = false;
        self.zset_edit_score = None;
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = if i + 1 < bytes.len() {
            bytes[i + 1]
        } else {
            0
        };
        let b2 = if i + 2 < bytes.len() {
            bytes[i + 2]
        } else {
            0
        };
        result.push(CHARS[((b0 >> 2) & 0x3F) as usize]);
        result.push(CHARS[((((b0 & 0x03) << 4) | ((b1 >> 4) & 0x0F))) as usize]);
        if i + 1 < bytes.len() {
            result.push(CHARS[((((b1 & 0x0F) << 2) | ((b2 >> 6) & 0x03))) as usize]);
        } else {
            result.push(b'=');
        }
        if i + 2 < bytes.len() {
            result.push(CHARS[(b2 & 0x3F) as usize]);
        } else {
            result.push(b'=');
        }
        i += 3;
    }
    String::from_utf8(result).unwrap_or_default()
}
