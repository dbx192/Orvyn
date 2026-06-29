use app_core::{metadata::MetadataService, session::SessionManager};
use db_common::{
    AppError, ConnectionId, ConnectionStatus, MetadataNode, MetadataNodeKind, MetadataPath,
};
use eframe::egui::{
    vec2, Align, Button, Color32, Context, Frame, Label, Layout, RichText, ScrollArea, Sense,
    TextEdit, Ui,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc::{self, Receiver, Sender},
};
use tokio::runtime::Handle;

const SEARCH_MIN_CHARS: usize = 2;
const SEARCH_RESULT_LIMIT: usize = 100;
const STATUS_POLL_INTERVAL_MS: u64 = 1500;

#[derive(Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
struct NodeKey {
    connection_id: ConnectionId,
    path_kind: MetadataNodeKind,
    database: Option<String>,
    schema: Option<String>,
    object: Option<String>,
    extra: Option<String>,
}

impl From<&MetadataPath> for NodeKey {
    fn from(path: &MetadataPath) -> Self {
        Self {
            connection_id: path.connection_id,
            path_kind: path.kind,
            database: path.database.clone(),
            schema: path.schema.clone(),
            object: path.object.clone(),
            extra: path.extra.clone(),
        }
    }
}

fn node_key_id(key: &NodeKey) -> String {
    format!(
        "{:?}-{:?}-{}-{}-{}-{}",
        key.connection_id.as_uuid(),
        key.path_kind,
        key.database.as_deref().unwrap_or(""),
        key.schema.as_deref().unwrap_or(""),
        key.object.as_deref().unwrap_or(""),
        key.extra.as_deref().unwrap_or("")
    )
}

#[derive(Clone)]
struct TreeNode {
    node: MetadataNode,
    children: Option<Vec<MetadataNode>>,
    loading: bool,
    load_error: Option<String>,
}

impl TreeNode {
    fn new(node: MetadataNode) -> Self {
        Self {
            node,
            children: None,
            loading: false,
            load_error: None,
        }
    }

    fn key(&self) -> NodeKey {
        NodeKey::from(&self.node.path)
    }
}

enum TreeEvent {
    RootLoaded(Result<Vec<MetadataNode>, AppError>),
    ChildrenLoaded(NodeKey, Result<Vec<MetadataNode>, AppError>),
    Connected(ConnectionId, Result<(), AppError>),
    StatusPolled(Vec<(ConnectionId, ConnectionStatus)>),
}

/// Action surfaced to the parent app when a node is activated from the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectTreeAction {
    OpenTableData {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    },
    OpenQuery {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
    },
    ViewKey {
        connection_id: ConnectionId,
        database: Option<String>,
        key: String,
    },
    ViewStructure {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    },
    ImportCsv {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    },
    EditConnection(ConnectionId),
    DeleteConnection(ConnectionId),
}

pub struct ObjectTree {
    metadata_service: MetadataService,
    session_manager: SessionManager,
    runtime: Handle,
    tx: Sender<TreeEvent>,
    rx: Receiver<TreeEvent>,
    connections: BTreeMap<ConnectionId, (TreeNode, ConnectionStatus)>,
    child_cache: BTreeMap<NodeKey, Vec<MetadataNode>>,
    expanded: BTreeSet<String>,
    search_query: String,
    search_results: Option<Vec<(MetadataNode, usize)>>,
    selected_node: Option<NodeKey>,
    status: String,
    redis_pattern: BTreeMap<NodeKey, String>,
    last_status_poll_ms: u64,
}

impl ObjectTree {
    pub fn new(
        metadata_service: MetadataService,
        session_manager: SessionManager,
        runtime: Handle,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut ui = Self {
            metadata_service,
            session_manager,
            runtime,
            tx,
            rx,
            connections: BTreeMap::new(),
            child_cache: BTreeMap::new(),
            expanded: BTreeSet::new(),
            search_query: String::new(),
            search_results: None,
            selected_node: None,
            status: String::new(),
            redis_pattern: BTreeMap::new(),
            last_status_poll_ms: 0,
        };
        ui.load_root();
        ui
    }

    /// Render the object tree. Returns a status message and a list of
    /// navigation actions the parent app should consume (e.g. open a table
    /// tab, open a query tab).
    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) -> (Option<String>, Vec<ObjectTreeAction>) {
        self.drain_events(ctx);
        self.maybe_poll_status(ctx);

        self.search_bar(ui);
        ui.separator();

        let mut message = None;
        let mut actions = Vec::new();

        ScrollArea::vertical()
            .id_source("object_tree_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.search_results.is_some() {
                    self.search_results_ui(ui, &mut message, &mut actions);
                } else {
                    self.tree_ui(ui, &mut message, &mut actions);
                }
            });

        (message, actions)
    }

    fn search_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("🔍").color(Color32::GRAY));
            let response = ui.add(
                TextEdit::singleline(&mut self.search_query)
                    .hint_text(format!("Search object (min {SEARCH_MIN_CHARS} chars)..."))
                    .desired_width(180.0),
            );
            if response.changed() {
                self.search_results = None;
                if self.search_query.chars().count() >= SEARCH_MIN_CHARS {
                    self.perform_search();
                }
            }
            if ui.small_button("✕").clicked() {
                self.search_query.clear();
                self.search_results = None;
            }
        });
    }

    fn tree_ui(
        &mut self,
        ui: &mut Ui,
        message: &mut Option<String>,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        if self.connections.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("No connections yet")
                        .color(Color32::GRAY)
                        .italics(),
                );
                ui.label(
                    RichText::new("Click \"New Connection\" to add one")
                        .color(Color32::GRAY)
                        .small(),
                );
            });
            return;
        }

        let connection_ids: Vec<ConnectionId> = self.connections.keys().copied().collect();

        for conn_id in connection_ids {
            let (tree_node, status) = match self.connections.get(&conn_id) {
                Some(t) => (t.0.clone(), t.1),
                None => continue,
            };

            self.render_connection(ui, conn_id, &tree_node, status, message, actions);
            ui.add_space(2.0);
        }
    }

    fn render_connection(
        &mut self,
        ui: &mut Ui,
        connection_id: ConnectionId,
        tree_node: &TreeNode,
        status: ConnectionStatus,
        message: &mut Option<String>,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        let key = tree_node.key();
        let key_str = node_key_id(&key);
        let expanded = self.expanded.contains(&key_str);
        let status_color = connection_status_color(&status);
        let status_label = connection_status_label(&status);

        let node = tree_node.node.clone();
        let is_selected = self.selected_node == Some(key.clone());

        Frame::group(ui.style())
            .fill(ui.style().visuals.faint_bg_color)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let arrow = if expanded { "▼" } else { "▶" };
                    let arrow_resp = ui.add(
                        Label::new(RichText::new(arrow).color(Color32::GRAY).size(10.0))
                            .sense(Sense::click()),
                    );
                    if arrow_resp.clicked() {
                        self.toggle_connection(connection_id);
                    }

                    let dot_resp = ui
                        .add(
                            Label::new(RichText::new("●").color(status_color).size(11.0).strong())
                                .sense(Sense::click()),
                        )
                        .on_hover_text(status_label);
                    if dot_resp.clicked() {
                        self.toggle_connection(connection_id);
                    }

                    let label_text = format!("{}  ", node.label);
                    let label_resp = ui.add(
                        Label::new(RichText::new(label_text).strong().color(if is_selected {
                            ui.visuals().selection.bg_fill
                        } else {
                            Color32::default()
                        }))
                        .sense(Sense::click()),
                    );

                    if let Some(detail) = &node.detail {
                        ui.label(
                            RichText::new(format!("({})", detail))
                                .color(Color32::GRAY)
                                .size(10.0),
                        );
                    }

                    let mut show_menu = false;
                    label_resp.context_menu(|ui| {
                        show_menu = true;
                        self.connection_context_menu(ui, connection_id, message, actions);
                    });

                    if label_resp.clicked() && !show_menu {
                        self.toggle_connection(connection_id);
                        self.selected_node = Some(key.clone());
                    }
                });

                if expanded {
                    ui.indent("conn_children", |ui| {
                        match (&tree_node.children, tree_node.loading) {
                            (Some(children), _) => {
                                if children.is_empty() {
                                    ui.label(
                                        RichText::new("(empty)").color(Color32::GRAY).italics(),
                                    );
                                } else {
                                    for child in children {
                                        self.render_tree_node(ui, child, message, actions);
                                    }
                                }
                            }
                            (None, true) => {
                                ui.label(
                                    RichText::new("Loading...").color(Color32::GRAY).italics(),
                                );
                            }
                            (None, false) => {
                                if let Some(error) = &tree_node.load_error {
                                    ui.colored_label(Color32::RED, format!("Error: {}", error));
                                    if ui.button("Retry").clicked() {
                                        self.refresh_connection(connection_id);
                                    }
                                }
                            }
                        }
                    });
                }
            });
    }

    fn render_tree_node(
        &mut self,
        ui: &mut Ui,
        node: &MetadataNode,
        message: &mut Option<String>,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        let key = NodeKey::from(&node.path);
        let key_str = node_key_id(&key);
        let expanded = self.expanded.contains(&key_str);
        let is_leaf = node.leaf;
        let is_selected = self.selected_node == Some(key.clone());
        let children = self.child_cache.get(&key).cloned();

        let node_clone = node.clone();

        ui.horizontal(|ui| {
            if !is_leaf {
                let arrow = if expanded { "▼" } else { "▶" };
                if ui
                    .add(
                        Label::new(RichText::new(arrow).color(Color32::GRAY).size(10.0))
                            .sense(Sense::click()),
                    )
                    .clicked()
                {
                    self.toggle_node(node_clone.clone());
                }
            } else {
                ui.add_space(12.0);
            }

            let icon = node_kind_icon(node.kind);
            ui.label(RichText::new(icon).size(12.0));

            let label_resp = ui.add(
                Label::new(RichText::new(&node.label).color(if is_selected {
                    ui.visuals().selection.bg_fill
                } else {
                    Color32::default()
                }))
                .sense(Sense::click()),
            );

            let mut show_menu = false;
            label_resp.context_menu(|ui| {
                show_menu = true;
                self.node_context_menu(ui, &node_clone, message, actions);
            });

            let double_clicked = label_resp.double_clicked();
            if (label_resp.clicked() || double_clicked) && !show_menu {
                self.selected_node = Some(key.clone());
                if double_clicked && !is_leaf {
                    self.toggle_node(node_clone.clone());
                }
                if double_clicked && is_leaf {
                    self.activate_node(&node_clone, message, actions);
                } else {
                    *message = Some(format!("Selected: {}", node.label));
                }
            }

            if let Some(detail) = &node.detail {
                ui.label(
                    RichText::new(format!(" ({})", detail))
                        .color(Color32::GRAY)
                        .size(10.0),
                );
            }
        });

        if expanded && !is_leaf {
            ui.indent("node_children", |ui| match children {
                Some(list) if list.is_empty() => {
                    ui.label(RichText::new("(empty)").color(Color32::GRAY).italics());
                }
                Some(list) => {
                    for child in &list {
                        self.render_tree_node(ui, child, message, actions);
                    }
                }
                None => {
                    if matches!(node.kind, MetadataNodeKind::RedisKeysFolder)
                        && self.redis_pattern.contains_key(&key)
                    {
                        ui.label(RichText::new("Loading...").color(Color32::GRAY).italics());
                    } else {
                        ui.label(RichText::new("Loading...").color(Color32::GRAY).italics());
                    }
                }
            });
        }
    }

    fn search_results_ui(
        &mut self,
        ui: &mut Ui,
        message: &mut Option<String>,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        let results = self.search_results.clone();
        if let Some(results) = &results {
            if results.is_empty() {
                ui.label(
                    RichText::new("No results found")
                        .color(Color32::GRAY)
                        .italics(),
                );
                return;
            }
            ui.label(
                RichText::new(format!("{} matches", results.len()))
                    .color(Color32::GRAY)
                    .small(),
            );
            for (result, depth) in results {
                let key = NodeKey::from(&result.path);
                let is_selected = self.selected_node == Some(key.clone());
                let indent = (*depth as f32) * 14.0;

                let result_clone = result.clone();
                let label_text = format!("{} {}", node_kind_icon(result.kind), result.label);
                let resp = ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 20.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.add_space(indent);
                        ui.add(
                            Label::new(RichText::new(label_text).color(if is_selected {
                                ui.visuals().selection.bg_fill
                            } else {
                                Color32::default()
                            }))
                            .sense(Sense::click()),
                        )
                    },
                );
                if resp.inner.clicked() {
                    self.selected_node = Some(key);
                    *message = Some(format!("Selected: {}", result.label));
                }
                if resp.inner.double_clicked() {
                    self.activate_node(&result_clone, message, actions);
                }

                let path_desc = format!("    {}", result_path_description(result));
                ui.label(RichText::new(path_desc).color(Color32::GRAY).size(10.0));
            }
        }
    }

    fn connection_context_menu(
        &mut self,
        ui: &mut Ui,
        connection_id: ConnectionId,
        message: &mut Option<String>,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        let status = self
            .connections
            .get(&connection_id)
            .map(|(_, s)| *s)
            .unwrap_or_default();

        match status {
            ConnectionStatus::NotConnected | ConnectionStatus::Disconnected => {
                if ui.button("Connect").clicked() {
                    self.connect(connection_id);
                    *message = Some("Connecting...".to_string());
                    ui.close_menu();
                }
            }
            ConnectionStatus::Connecting => {
                ui.add_enabled(false, Button::new("Connecting..."));
            }
            ConnectionStatus::Connected => {
                if ui.button("Disconnect").clicked() {
                    self.disconnect(connection_id);
                    *message = Some("Disconnected".to_string());
                    ui.close_menu();
                }
                if ui.button("New Query").clicked() {
                    let node = self.connection_node(connection_id);
                    if let Some(node) = node {
                        actions.push(ObjectTreeAction::OpenQuery {
                            connection_id,
                            database: node.path.database.clone(),
                            schema: node.path.schema.clone(),
                        });
                    }
                    *message = Some("Opening new query tab".to_string());
                    ui.close_menu();
                }
            }
            ConnectionStatus::Failed => {
                if ui.button("Retry Connect").clicked() {
                    self.connect(connection_id);
                    *message = Some("Retrying connection".to_string());
                    ui.close_menu();
                }
            }
        }

        ui.separator();
        if ui.button("Refresh").clicked() {
            self.refresh_connection(connection_id);
            *message = Some("Refreshing...".to_string());
            ui.close_menu();
        }
        if ui.button("Edit Connection...").clicked() {
            actions.push(ObjectTreeAction::EditConnection(connection_id));
            ui.close_menu();
        }
        if ui.button("Delete Connection").clicked() {
            actions.push(ObjectTreeAction::DeleteConnection(connection_id));
            ui.close_menu();
        }
    }

    fn node_context_menu(
        &mut self,
        ui: &mut Ui,
        node: &MetadataNode,
        message: &mut Option<String>,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        match node.kind {
            MetadataNodeKind::Table => {
                if ui.button("Open Table Data").clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::OpenTableData {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                if ui.button("View Structure").clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::ViewStructure {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                if ui.button("New Query on Table").clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, _table| ObjectTreeAction::OpenQuery {
                            connection_id: conn_id,
                            database: db,
                            schema,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                if ui.button("Import CSV...").clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::ImportCsv {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                ui.separator();
            }
            MetadataNodeKind::View => {
                if ui.button("Open View Data").clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::OpenTableData {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                if ui.button("View Structure").clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::ViewStructure {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                ui.separator();
            }
            MetadataNodeKind::RedisKeysFolder => {
                let key = NodeKey::from(&node.path);
                let current = self.redis_pattern.get(&key).cloned().unwrap_or_default();
                let mut input = current.clone();
                ui.label("Pattern (glob):");
                let resp = ui.add(
                    TextEdit::singleline(&mut input)
                        .hint_text("user:*")
                        .desired_width(180.0),
                );
                if resp.lost_focus() && input != current {
                    self.apply_redis_pattern(key.clone(), input.clone());
                }
                if resp.changed() {
                    self.redis_pattern.insert(key.clone(), input.clone());
                }
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        self.apply_redis_pattern(key.clone(), input.clone());
                        ui.close_menu();
                    }
                    if ui.button("Clear").clicked() {
                        self.apply_redis_pattern(key.clone(), String::new());
                        ui.close_menu();
                    }
                });
                ui.separator();
                if ui.button("Refresh").clicked() {
                    self.refresh_node(node.clone());
                    ui.close_menu();
                }
            }
            MetadataNodeKind::RedisKey => {
                if ui.button("View Key").clicked() {
                    self.emit_key_action(
                        node,
                        |conn_id, db, key| ObjectTreeAction::ViewKey {
                            connection_id: conn_id,
                            database: db,
                            key,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                ui.separator();
            }
            _ => {}
        }

        if !matches!(node.kind, MetadataNodeKind::RedisKeysFolder) {
            if ui.button("Refresh").clicked() {
                self.refresh_node(node.clone());
                ui.close_menu();
            }
        }

        if ui.button("Copy Name").clicked() {
            ui.output_mut(|o| o.copied_text = node.label.clone());
            *message = Some(format!("Copied: {}", node.label));
            ui.close_menu();
        }

        if let Some(database) = &node.path.database {
            if ui.button("Copy Full Path").clicked() {
                let path_text = build_full_path(node);
                ui.output_mut(|o| o.copied_text = format!("{path_text} ({database})"));
                *message = Some("Path copied".to_string());
                ui.close_menu();
            }
        }
    }

    fn activate_node(
        &mut self,
        node: &MetadataNode,
        message: &mut Option<String>,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        match node.kind {
            MetadataNodeKind::Table => {
                self.emit_table_action(
                    node,
                    |conn_id, db, schema, table| ObjectTreeAction::OpenTableData {
                        connection_id: conn_id,
                        database: db,
                        schema,
                        table,
                    },
                    actions,
                );
                *message = Some(format!("Opening {}", node.label));
            }
            MetadataNodeKind::View => {
                self.emit_table_action(
                    node,
                    |conn_id, db, schema, table| ObjectTreeAction::OpenTableData {
                        connection_id: conn_id,
                        database: db,
                        schema,
                        table,
                    },
                    actions,
                );
                *message = Some(format!("Opening view {}", node.label));
            }
            MetadataNodeKind::RedisKey => {
                self.emit_key_action(
                    node,
                    |conn_id, db, key| ObjectTreeAction::ViewKey {
                        connection_id: conn_id,
                        database: db,
                        key,
                    },
                    actions,
                );
                *message = Some(format!("Viewing key {}", node.label));
            }
            _ => {
                *message = Some(format!("Selected: {}", node.label));
            }
        }
    }

    fn emit_table_action(
        &self,
        node: &MetadataNode,
        ctor: impl FnOnce(ConnectionId, Option<String>, Option<String>, String) -> ObjectTreeAction,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        let table = node
            .path
            .object
            .clone()
            .unwrap_or_else(|| node.label.clone());
        actions.push(ctor(
            node.path.connection_id,
            node.path.database.clone(),
            node.path.schema.clone(),
            table,
        ));
    }

    fn emit_key_action(
        &self,
        node: &MetadataNode,
        ctor: impl FnOnce(ConnectionId, Option<String>, String) -> ObjectTreeAction,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        let key = node
            .path
            .object
            .clone()
            .unwrap_or_else(|| node.label.clone());
        actions.push(ctor(
            node.path.connection_id,
            node.path.database.clone(),
            key,
        ));
    }

    fn connection_node(&self, connection_id: ConnectionId) -> Option<MetadataNode> {
        self.connections
            .get(&connection_id)
            .map(|(n, _)| n.node.clone())
    }

    fn toggle_connection(&mut self, connection_id: ConnectionId) {
        let key_str = if let Some((tree_node, _)) = self.connections.get(&connection_id) {
            node_key_id(&tree_node.key())
        } else {
            return;
        };

        if self.expanded.contains(&key_str) {
            self.expanded.remove(&key_str);
        } else {
            self.expanded.insert(key_str.clone());
            let has_children = self
                .connections
                .get(&connection_id)
                .map(|(n, _)| n.children.is_some())
                .unwrap_or(false);
            if !has_children {
                self.load_connection_children(connection_id);
            }
        }
    }

    fn toggle_node(&mut self, node: MetadataNode) {
        let key = NodeKey::from(&node.path);
        let key_str = node_key_id(&key);

        if self.expanded.contains(&key_str) {
            self.expanded.remove(&key_str);
        } else {
            self.expanded.insert(key_str);
            self.load_children(node);
        }
    }

    fn apply_redis_pattern(&mut self, key: NodeKey, pattern: String) {
        let pattern_trimmed = pattern.trim().to_string();
        let is_empty = pattern_trimmed.is_empty();

        if is_empty {
            self.redis_pattern.remove(&key);
        } else {
            self.redis_pattern.insert(key.clone(), pattern);
        }
        // Invalidate and re-fetch with the new pattern.
        self.child_cache.remove(&key);

        // Recompute the path with extra set to the pattern.
        if let Some((conn_id, _)) = self.find_connection_id_for(&key) {
            if let Some(node) = self.connection_node(conn_id) {
                let mut path = node.path.child(MetadataNodeKind::RedisKeysFolder);
                if let Some(database) = &key.database {
                    path = path.with_database(database.clone());
                }
                if !is_empty {
                    path = path.with_extra(&pattern_trimmed);
                }
                self.load_children_for_path(path);
            }
        }
    }

    fn find_connection_id_for(&self, key: &NodeKey) -> Option<(ConnectionId, MetadataPath)> {
        self.connections
            .get(&key.connection_id)
            .map(|(n, _)| (key.connection_id, n.node.path.clone()))
    }

    fn load_root(&mut self) {
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();

        self.runtime.spawn(async move {
            let result = service.root_nodes().await;
            let _ = tx.send(TreeEvent::RootLoaded(result));
        });
    }

    fn load_connection_children(&mut self, connection_id: ConnectionId) {
        if let Some((tree_node, _)) = self.connections.get_mut(&connection_id) {
            tree_node.loading = true;
            tree_node.load_error = None;
        }

        let node = match self.connections.get(&connection_id) {
            Some((t, _)) => t.node.clone(),
            None => return,
        };

        let key = NodeKey::from(&node.path);
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let path = node.path;

        self.runtime.spawn(async move {
            let result = service.children(&path, None).await;
            let _ = tx.send(TreeEvent::ChildrenLoaded(key, result));
        });
    }

    fn load_children(&mut self, node: MetadataNode) {
        let key = NodeKey::from(&node.path);
        if self.child_cache.contains_key(&key) {
            return;
        }
        self.load_children_for_path(node.path);
    }

    fn load_children_for_path(&mut self, path: MetadataPath) {
        let key = NodeKey::from(&path);
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();

        self.runtime.spawn(async move {
            let result = service.children(&path, None).await;
            let _ = tx.send(TreeEvent::ChildrenLoaded(key, result));
        });
    }

    fn refresh_connection(&mut self, connection_id: ConnectionId) {
        if let Some((tree_node, _)) = self.connections.get_mut(&connection_id) {
            tree_node.children = None;
            tree_node.loading = true;
            tree_node.load_error = None;
        }
        // Drop caches that are descendants of this connection.
        self.child_cache
            .retain(|k, _| k.connection_id != connection_id);
        self.expanded
            .retain(|s| !s.contains(&connection_id.as_uuid().to_string()));

        let node = match self.connections.get(&connection_id) {
            Some((t, _)) => t.node.clone(),
            None => return,
        };

        let key = NodeKey::from(&node.path);
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let path = node.path;

        self.runtime.spawn(async move {
            let result = service.children(&path, None).await;
            let _ = tx.send(TreeEvent::ChildrenLoaded(key, result));
        });
    }

    fn refresh_node(&mut self, node: MetadataNode) {
        let key = NodeKey::from(&node.path);
        self.child_cache.remove(&key);

        // Drop descendant caches for non-leaf refresh.
        self.child_cache.retain(|k, _| !is_descendant(k, &key));

        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let path = node.path;

        self.runtime.spawn(async move {
            let result = service.children(&path, None).await;
            let _ = tx.send(TreeEvent::ChildrenLoaded(key, result));
        });
    }

    fn connect(&mut self, connection_id: ConnectionId) {
        if let Some((_, status)) = self.connections.get_mut(&connection_id) {
            *status = ConnectionStatus::Connecting;
        }
        let tx = self.tx.clone();
        let manager = self.session_manager.clone();

        self.runtime.spawn(async move {
            let result = manager.connect(connection_id, None).await.map(|_| ());
            let _ = tx.send(TreeEvent::Connected(connection_id, result));
        });
    }

    fn disconnect(&mut self, connection_id: ConnectionId) {
        if let Some((_, status)) = self.connections.get_mut(&connection_id) {
            *status = ConnectionStatus::Disconnected;
        }
        let manager = self.session_manager.clone();
        self.runtime.spawn(async move {
            let _ = manager.disconnect(connection_id).await;
        });
    }

    fn perform_search(&mut self) {
        let mut results = Vec::new();
        let query = self.search_query.to_lowercase();

        for (_, (tree_node, _)) in &self.connections {
            if tree_node.node.label.to_lowercase().contains(&query) {
                results.push((tree_node.node.clone(), 0));
            }
            if let Some(children) = &tree_node.children {
                self.search_in_children(children, &query, 1, &mut results);
            }
        }

        results.truncate(SEARCH_RESULT_LIMIT);
        self.search_results = Some(results);
    }

    fn search_in_children(
        &self,
        children: &[MetadataNode],
        query: &str,
        depth: usize,
        results: &mut Vec<(MetadataNode, usize)>,
    ) {
        for child in children {
            if child.label.to_lowercase().contains(query) {
                results.push((child.clone(), depth));
            }
            let key = NodeKey::from(&child.path);
            if let Some(grandchildren) = self.child_cache.get(&key) {
                self.search_in_children(grandchildren, query, depth + 1, results);
            }
        }
    }

    fn maybe_poll_status(&mut self, ctx: &Context) {
        if self.connections.is_empty() {
            return;
        }
        let now = ctx.input(|i| i.time) * 1000.0;
        if now - (self.last_status_poll_ms as f64) < STATUS_POLL_INTERVAL_MS as f64 {
            return;
        }
        self.last_status_poll_ms = now as u64;
        let tx = self.tx.clone();
        let manager = self.session_manager.clone();
        let ids: Vec<ConnectionId> = self.connections.keys().copied().collect();
        self.runtime.spawn(async move {
            let mut updates = Vec::new();
            for id in ids {
                let status = manager.status_for(id).await;
                updates.push((id, status));
            }
            let _ = tx.send(TreeEvent::StatusPolled(updates));
        });
    }

    fn drain_events(&mut self, _ctx: &Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                TreeEvent::RootLoaded(result) => match result {
                    Ok(nodes) => {
                        self.connections = nodes
                            .into_iter()
                            .map(|n| {
                                (
                                    n.path.connection_id,
                                    (TreeNode::new(n), ConnectionStatus::NotConnected),
                                )
                            })
                            .collect();
                        self.status = format!("Loaded {} connections", self.connections.len());
                    }
                    Err(error) => {
                        self.status = format!("Failed to load tree: {}", error.message);
                    }
                },
                TreeEvent::ChildrenLoaded(key, result) => {
                    let is_connection = matches!(key.path_kind, MetadataNodeKind::Connection);
                    match result {
                        Ok(children) => {
                            if is_connection {
                                if let Some((tree_node, _)) =
                                    self.connections.get_mut(&key.connection_id)
                                {
                                    tree_node.children = Some(children);
                                    tree_node.loading = false;
                                    tree_node.load_error = None;
                                }
                            } else {
                                self.child_cache.insert(key, children);
                            }
                        }
                        Err(error) => {
                            if is_connection {
                                if let Some((tree_node, _)) =
                                    self.connections.get_mut(&key.connection_id)
                                {
                                    tree_node.loading = false;
                                    tree_node.load_error = Some(error.message.clone());
                                }
                            } else {
                                self.status = format!("Failed to load children: {}", error.message);
                            }
                        }
                    }
                }
                TreeEvent::Connected(id, result) => {
                    if let Some((_, status)) = self.connections.get_mut(&id) {
                        *status = match &result {
                            Ok(_) => ConnectionStatus::Connected,
                            Err(_) => ConnectionStatus::Failed,
                        };
                    }
                    self.status = match result {
                        Ok(_) => "Connected".to_string(),
                        Err(e) => format!("Connection failed: {}", e.message),
                    };
                }
                TreeEvent::StatusPolled(updates) => {
                    for (id, new_status) in updates {
                        // Only update if the polled status reflects a real state.
                        if matches!(
                            new_status,
                            ConnectionStatus::Connected
                                | ConnectionStatus::Failed
                                | ConnectionStatus::Disconnected
                                | ConnectionStatus::NotConnected
                        ) {
                            if let Some((_, status)) = self.connections.get_mut(&id) {
                                *status = new_status;
                            }
                        }
                    }
                }
            }
        }
    }
}

fn is_descendant(candidate: &NodeKey, ancestor: &NodeKey) -> bool {
    if candidate.connection_id != ancestor.connection_id {
        return false;
    }
    if ancestor.path_kind == MetadataNodeKind::Connection {
        return true;
    }
    // Strict descendant: candidate must be deeper in the tree.
    if let (Some(db_a), Some(db_c)) = (&ancestor.database, &candidate.database) {
        if db_a != db_c {
            return false;
        }
    }
    if let (Some(s_a), Some(s_c)) = (&ancestor.schema, &candidate.schema) {
        if s_a != s_c {
            return false;
        }
    }
    true
}

fn node_kind_icon(kind: MetadataNodeKind) -> &'static str {
    match kind {
        MetadataNodeKind::Connection => "🗄",
        MetadataNodeKind::Database => "🗄",
        MetadataNodeKind::Schema => "📁",
        MetadataNodeKind::TablesFolder => "📁",
        MetadataNodeKind::ViewsFolder => "👁",
        MetadataNodeKind::ColumnsFolder => "🔤",
        MetadataNodeKind::IndexesFolder => "🔢",
        MetadataNodeKind::ForeignKeysFolder => "🔗",
        MetadataNodeKind::TriggersFolder => "⚡",
        MetadataNodeKind::FunctionsFolder => "ƒ",
        MetadataNodeKind::Table => "📋",
        MetadataNodeKind::View => "👁",
        MetadataNodeKind::Column => "🔤",
        MetadataNodeKind::Index => "🔢",
        MetadataNodeKind::ForeignKey => "🔗",
        MetadataNodeKind::Trigger => "⚡",
        MetadataNodeKind::Function => "ƒ",
        MetadataNodeKind::RedisDatabase => "🗄",
        MetadataNodeKind::RedisKeysFolder => "🔑",
        MetadataNodeKind::RedisKey => "🔑",
    }
}

fn connection_status_color(status: &ConnectionStatus) -> Color32 {
    match status {
        ConnectionStatus::NotConnected => Color32::GRAY,
        ConnectionStatus::Connecting => Color32::from_rgb(255, 200, 0),
        ConnectionStatus::Connected => Color32::from_rgb(64, 180, 64),
        ConnectionStatus::Failed => Color32::from_rgb(220, 60, 60),
        ConnectionStatus::Disconnected => Color32::from_rgb(160, 160, 160),
    }
}

fn connection_status_label(status: &ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::NotConnected => "Not connected",
        ConnectionStatus::Connecting => "Connecting",
        ConnectionStatus::Connected => "Connected",
        ConnectionStatus::Failed => "Connection failed",
        ConnectionStatus::Disconnected => "Disconnected",
    }
}

fn result_path_description(node: &MetadataNode) -> String {
    let mut parts = Vec::new();
    if let Some(db) = &node.path.database {
        parts.push(db.clone());
    }
    if let Some(schema) = &node.path.schema {
        parts.push(schema.clone());
    }
    let kind = match node.kind {
        MetadataNodeKind::Table => "Table",
        MetadataNodeKind::View => "View",
        MetadataNodeKind::Column => "Column",
        MetadataNodeKind::Index => "Index",
        MetadataNodeKind::Schema => "Schema",
        MetadataNodeKind::Database => "Database",
        MetadataNodeKind::RedisKey => "Redis Key",
        MetadataNodeKind::RedisDatabase => "Redis DB",
        MetadataNodeKind::RedisKeysFolder => "Keys",
        MetadataNodeKind::TablesFolder => "Tables",
        MetadataNodeKind::ViewsFolder => "Views",
        MetadataNodeKind::ColumnsFolder => "Columns",
        MetadataNodeKind::IndexesFolder => "Indexes",
        MetadataNodeKind::ForeignKeysFolder => "Foreign Keys",
        MetadataNodeKind::TriggersFolder => "Triggers",
        MetadataNodeKind::FunctionsFolder => "Functions",
        MetadataNodeKind::Connection => "Connection",
        MetadataNodeKind::ForeignKey => "Foreign Key",
        MetadataNodeKind::Trigger => "Trigger",
        MetadataNodeKind::Function => "Function",
    };
    if parts.is_empty() {
        kind.to_string()
    } else {
        format!("{} - {}", kind, parts.join("."))
    }
}

fn build_full_path(node: &MetadataNode) -> String {
    let mut parts = Vec::new();
    if let Some(db) = &node.path.database {
        parts.push(db.clone());
    }
    if let Some(schema) = &node.path.schema {
        parts.push(schema.clone());
    }
    if let Some(object) = &node.path.object {
        parts.push(object.clone());
    } else {
        parts.push(node.label.clone());
    }
    parts.join(".")
}
