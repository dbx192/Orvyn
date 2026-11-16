#![allow(clippy::if_same_then_else)]
#![allow(clippy::collapsible_if)]

use crate::ui::i18n::text as tr;
use app_core::{metadata::MetadataService, query::QueryService, session::SessionManager};
use db_common::{
    AppError, ConnectionId, ConnectionStatus, DataSourceKind, FavoriteQuery, MetadataNode,
    MetadataNodeKind, MetadataPath,
};
use eframe::egui::{
    containers::scroll_area::ScrollBarVisibility, pos2, vec2, Align2, Button, Color32, Context,
    FontId, Rect, Response, RichText, Rounding, ScrollArea, Sense, Stroke, TextEdit, Ui, Vec2,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
use tokio::runtime::Handle;

const SEARCH_MIN_CHARS: usize = 2;
const STATUS_POLL_INTERVAL_MS: u64 = 1500;
const RECONNECT_BASE_DELAY_MS: u64 = 3000;
const MAX_RECONNECT_ATTEMPTS: u8 = 3;
const TREE_ROW_HEIGHT: f32 = 24.0;
const TREE_ROW_RADIUS: f32 = 4.0;
const TREE_INDENT: f32 = 14.0;
const TREE_MAX_INDENT: f32 = 96.0;
const TREE_ARROW_WIDTH: f32 = 15.0;
const TREE_BADGE_GAP: f32 = 8.0;
const TREE_STATUS_DOT_WIDTH: f32 = 16.0;

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

fn localized_node_label(node: &MetadataNode) -> String {
    match node.kind {
        MetadataNodeKind::ViewsFolder => tr("Views"),
        MetadataNodeKind::QueriesFolder => tr("Queries"),
        _ => node.label.clone(),
    }
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
    FavoritesLoaded(Vec<FavoriteQuery>),
    FavoriteDeleted(Result<(), AppError>),
    Connected(ConnectionId, Result<(), AppError>),
    StatusPolled(Vec<(ConnectionId, ConnectionStatus)>),
}

#[derive(Clone, Copy, Debug)]
struct ReconnectState {
    attempts: u8,
    next_retry_ms: u64,
    in_flight: bool,
}

/// Action surfaced to the parent app when a node is activated from the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseExportMode {
    Structure,
    StructureAndData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectTreeAction {
    ImportConnections,
    ExportConnections {
        connection_ids: Option<Vec<ConnectionId>>,
        include_passwords: bool,
    },
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
        table: Option<String>,
    },
    /// Open the search/document/http-sql browser for a non-relational node
    /// (search index, Mongo collection or D1 table).
    OpenDataBrowser {
        connection_id: ConnectionId,
        kind: DataSourceKind,
        database: Option<String>,
        object: String,
    },
    OpenSavedQuery {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        title: String,
        sql: String,
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
    ExportTableData {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    },
    DropTable {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    },
    DropView {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        view: String,
    },
    DropDatabase {
        connection_id: ConnectionId,
        database: String,
    },
    ClearTable {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    },
    ClearRedisDatabase {
        connection_id: ConnectionId,
        database: u8,
    },
    ExportDatabase {
        connection_id: ConnectionId,
        database: Option<String>,
        mode: DatabaseExportMode,
    },
    ImportTable {
        connection_id: ConnectionId,
    },
    EditConnection(ConnectionId),
    DeleteConnection(ConnectionId),
    DeleteRedisKey {
        connection_id: ConnectionId,
        database: Option<String>,
        key: String,
    },
}

pub struct ObjectTree {
    metadata_service: MetadataService,
    query_service: QueryService,
    session_manager: SessionManager,
    runtime: Handle,
    tx: Sender<TreeEvent>,
    rx: Receiver<TreeEvent>,
    connections: BTreeMap<ConnectionId, (TreeNode, ConnectionStatus)>,
    connection_kinds: BTreeMap<ConnectionId, DataSourceKind>,
    child_cache: BTreeMap<NodeKey, Vec<MetadataNode>>,
    child_load_errors: BTreeMap<NodeKey, String>,
    loading_children: BTreeSet<NodeKey>,
    favorite_queries: Vec<FavoriteQuery>,
    expanded: BTreeSet<String>,
    search_query: String,
    selected_node: Option<NodeKey>,
    selected_connections: BTreeSet<ConnectionId>,
    export_include_passwords: bool,
    status: String,
    redis_pattern: BTreeMap<NodeKey, String>,
    last_status_poll_ms: u64,
    reconnects: BTreeMap<ConnectionId, ReconnectState>,
    reconnect_suppressed: BTreeSet<ConnectionId>,
}

impl ObjectTree {
    pub fn new(
        metadata_service: MetadataService,
        query_service: QueryService,
        session_manager: SessionManager,
        runtime: Handle,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut ui = Self {
            metadata_service,
            query_service,
            session_manager,
            runtime,
            tx,
            rx,
            connections: BTreeMap::new(),
            connection_kinds: BTreeMap::new(),
            child_cache: BTreeMap::new(),
            child_load_errors: BTreeMap::new(),
            loading_children: BTreeSet::new(),
            favorite_queries: Vec::new(),
            expanded: BTreeSet::new(),
            search_query: String::new(),
            selected_node: None,
            selected_connections: BTreeSet::new(),
            export_include_passwords: false,
            status: String::new(),
            redis_pattern: BTreeMap::new(),
            last_status_poll_ms: 0,
            reconnects: BTreeMap::new(),
            reconnect_suppressed: BTreeSet::new(),
        };
        ui.load_root();
        ui.load_favorites();
        ui
    }

    /// Render the object tree. Returns a status message and a list of
    /// navigation actions the parent app should consume (e.g. open a table
    /// tab, open a query tab).
    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) -> (Option<String>, Vec<ObjectTreeAction>) {
        self.drain_events(ctx);
        self.maybe_poll_status(ctx);
        self.maybe_retry_reconnects(ctx);
        if self.has_pending_work() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        ui.spacing_mut().item_spacing.y = 4.0;
        let mut message = None;
        let mut actions = Vec::new();
        self.search_bar(ui);

        let tree_width = ui.available_width();
        ScrollArea::vertical()
            .id_source("object_tree_scroll")
            .max_width(tree_width)
            .auto_shrink([false, false])
            .scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| {
                ui.set_width(tree_width);
                self.tree_ui(ui, &mut message, &mut actions);
            });

        (message, actions)
    }

    pub fn connection_transfer_menu(&mut self, ui: &mut Ui, actions: &mut Vec<ObjectTreeAction>) {
        if ui.button(format!("{}...", tr("Import"))).clicked() {
            actions.push(ObjectTreeAction::ImportConnections);
            ui.close_menu();
        }

        let selected_count = self.selected_connections.len();
        ui.menu_button(tr("Export"), |ui| {
            ui.checkbox(
                &mut self.export_include_passwords,
                tr("Include saved passwords"),
            )
            .on_hover_text(tr("Passwords are written as plain text in the backup file"));
            ui.separator();
            if ui.button(tr("Export All...")).clicked() {
                actions.push(ObjectTreeAction::ExportConnections {
                    connection_ids: None,
                    include_passwords: self.export_include_passwords,
                });
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    selected_count > 0,
                    Button::new(crate::ui::i18n::format(
                        "Export Selected ({count})...",
                        &[("count", selected_count.to_string())],
                    )),
                )
                .clicked()
            {
                actions.push(ObjectTreeAction::ExportConnections {
                    connection_ids: Some(self.selected_connections.iter().copied().collect()),
                    include_passwords: self.export_include_passwords,
                });
                ui.close_menu();
            }
        });
    }

    pub fn reload_root(&mut self) {
        self.child_cache.clear();
        self.child_load_errors.clear();
        self.loading_children.clear();
        self.expanded.clear();
        self.redis_pattern.clear();
        self.load_favorites();
        self.selected_node = None;
        self.status = "Loading connections...".to_string();
        self.load_root();
    }

    pub fn reload_favorites(&mut self) {
        self.child_cache
            .retain(|key, _| key.path_kind != MetadataNodeKind::QueriesFolder);
        self.child_load_errors
            .retain(|key, _| key.path_kind != MetadataNodeKind::QueriesFolder);
        self.loading_children
            .retain(|key| key.path_kind != MetadataNodeKind::QueriesFolder);
        self.load_favorites();
    }

    pub fn mark_connected(&mut self, connection_id: ConnectionId) {
        if let Some((_, status)) = self.connections.get_mut(&connection_id) {
            *status = ConnectionStatus::Connected;
        }
        self.reconnects.remove(&connection_id);
        self.reconnect_suppressed.remove(&connection_id);
    }

    fn search_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            // Input and reset control share one exact height so the row looks
            // like a single aligned field instead of mismatched widgets.
            let row_height = 22.0;
            let clear_width = 22.0;
            let edit_width = (ui.available_width() - clear_width).max(80.0);
            let response = ui.add_sized(
                [edit_width, row_height],
                TextEdit::singleline(&mut self.search_query).hint_text(tr(
                    "Search databases, tables, queries (at least 2 characters)",
                )),
            );
            let _ = response.changed();
            if crate::ui::app::style::close_icon_button(ui, row_height, false)
                .on_hover_text(tr("Clear"))
                .clicked()
            {
                self.search_query.clear();
            }
        });
        ui.separator();
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

            if self.search_active() && !self.connection_matches_search(&tree_node) {
                continue;
            }

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
        let expanded =
            self.expanded.contains(&key_str) || self.connection_force_expanded(tree_node);
        let status_color = connection_status_color(&status);

        let node = tree_node.node.clone();
        let is_selected = self.selected_connections.contains(&connection_id);
        let matched = self.node_matches_search(&node);

        let response = tree_row(
            ui,
            TreeRowSpec {
                id_source: &key_str,
                depth: 0,
                kind: node.kind,
                label: &localized_node_label(&node),
                detail: node.detail.as_deref(),
                expanded,
                leaf: false,
                selected: is_selected,
                matched,
                strong: true,
                connection_status: Some(status_color),
            },
        );

        let mut show_menu = false;
        response.context_menu(|ui| {
            show_menu = true;
            self.connection_context_menu(ui, connection_id, message, actions);
        });

        if response.clicked() && !show_menu {
            let multi_select = ui.input(|input| input.modifiers.ctrl || input.modifiers.command);
            if multi_select {
                if !self.selected_connections.insert(connection_id) {
                    self.selected_connections.remove(&connection_id);
                }
                self.selected_node = self
                    .selected_connections
                    .contains(&connection_id)
                    .then_some(key.clone());
                *message = Some(format!(
                    "Selected {} connection(s)",
                    self.selected_connections.len()
                ));
            } else {
                self.selected_connections.clear();
                self.selected_connections.insert(connection_id);
                self.selected_node = Some(key.clone());
                self.toggle_connection(connection_id);
            }
        }

        if expanded {
            match (&tree_node.children, tree_node.loading) {
                (Some(children), _) => {
                    if children.is_empty() {
                        tree_message_row(ui, 1, "(empty)", false);
                    } else {
                        for child in children {
                            if self.search_active() && !self.node_or_descendant_matches(child) {
                                continue;
                            }
                            self.render_tree_node(ui, child, 1, message, actions);
                        }
                    }
                }
                (None, true) => {
                    tree_message_row(ui, 1, "Loading...", false);
                }
                (None, false) => {
                    if let Some(error) = &tree_node.load_error {
                        tree_message_row(ui, 1, &format!("Error: {}", error), true);
                        ui.horizontal(|ui| {
                            ui.add_space(TREE_INDENT + TREE_ARROW_WIDTH);
                            if ui.small_button(tr("Retry")).clicked() {
                                self.refresh_connection(connection_id);
                            }
                        });
                    }
                }
            }
        }
    }

    fn render_tree_node(
        &mut self,
        ui: &mut Ui,
        node: &MetadataNode,
        depth: usize,
        message: &mut Option<String>,
        actions: &mut Vec<ObjectTreeAction>,
    ) {
        let key = NodeKey::from(&node.path);
        let key_str = node_key_id(&key);
        let expanded = self.expanded.contains(&key_str) || self.force_expanded(node);
        let is_leaf = node.leaf;
        let is_selected = self.selected_node == Some(key.clone());
        let children = self.children_for_node(node, &key);
        let matched = self.node_matches_search(node);

        let node_clone = node.clone();

        let response = tree_row(
            ui,
            TreeRowSpec {
                id_source: &key_str,
                depth,
                kind: node.kind,
                label: &localized_node_label(node),
                detail: node.detail.as_deref(),
                expanded,
                leaf: is_leaf,
                selected: is_selected,
                matched,
                strong: matches!(
                    node.kind,
                    MetadataNodeKind::Database
                        | MetadataNodeKind::Schema
                        | MetadataNodeKind::Table
                        | MetadataNodeKind::View
                        | MetadataNodeKind::QueriesFolder
                        | MetadataNodeKind::RedisDatabase
                ),
                connection_status: None,
            },
        );

        let mut show_menu = false;
        response.context_menu(|ui| {
            show_menu = true;
            self.node_context_menu(ui, &node_clone, message, actions);
        });

        let double_clicked = response.double_clicked();
        if (response.clicked() || double_clicked) && !show_menu {
            self.selected_connections.clear();
            self.selected_node = Some(key.clone());
            if !is_leaf && (clicked_in_arrow(&response, depth) || double_clicked) {
                self.toggle_node(node_clone.clone());
            }
            if should_activate_leaf(is_leaf, double_clicked) {
                self.activate_node(&node_clone, message, actions);
            } else {
                *message = Some(format!("Selected: {}", node.label));
            }
        }

        if expanded && !is_leaf {
            match children {
                Some(list) if list.is_empty() => {
                    tree_message_row(ui, depth + 1, "(empty)", false);
                }
                Some(list) => {
                    for child in &list {
                        if self.search_active() && !self.node_or_descendant_matches(child) {
                            continue;
                        }
                        self.render_tree_node(ui, child, depth + 1, message, actions);
                    }
                }
                None => {
                    if let Some(error) = self.child_load_errors.get(&key).cloned() {
                        tree_message_row(ui, depth + 1, &format!("Error: {error}"), true);
                        ui.horizontal(|ui| {
                            ui.add_space(((depth + 1) as f32 * TREE_INDENT) + TREE_ARROW_WIDTH);
                            if ui.small_button(tr("Retry")).clicked() {
                                self.load_children(node.clone());
                            }
                        });
                    } else if self.loading_children.contains(&key) {
                        tree_message_row(ui, depth + 1, "Loading...", false);
                    } else {
                        self.load_children(node.clone());
                        tree_message_row(ui, depth + 1, "Loading...", false);
                    }
                }
            }
        }
    }

    fn children_for_node(&self, node: &MetadataNode, key: &NodeKey) -> Option<Vec<MetadataNode>> {
        if node.kind == MetadataNodeKind::QueriesFolder {
            return Some(self.query_nodes_for_folder(node));
        }

        let mut children = self.child_cache.get(key).cloned()?;
        if node.kind == MetadataNodeKind::Database {
            if let Some(folder) = self.query_folder_for_database(node) {
                children.push(folder);
            }
        }
        Some(children)
    }

    fn query_folder_for_database(&self, node: &MetadataNode) -> Option<MetadataNode> {
        let query_count = self
            .favorite_queries
            .iter()
            .filter(|favorite| self.favorite_belongs_to_database(favorite, node))
            .count();
        if query_count == 0 {
            return None;
        }

        let mut path = MetadataPath::connection(node.path.connection_id)
            .child(MetadataNodeKind::QueriesFolder);
        if let Some(database) = node.path.database.clone() {
            path = path.with_database(database);
        }
        if let Some(schema) = node.path.schema.clone() {
            path = path.with_schema(schema);
        }

        Some(MetadataNode::new(
            path,
            "Queries",
            MetadataNodeKind::QueriesFolder,
        ))
    }

    fn query_nodes_for_folder(&self, node: &MetadataNode) -> Vec<MetadataNode> {
        let mut nodes = self
            .favorite_queries
            .iter()
            .filter(|favorite| self.favorite_belongs_to_database(favorite, node))
            .map(|favorite| {
                let mut path = MetadataPath::connection(node.path.connection_id)
                    .child(MetadataNodeKind::Query);
                if let Some(database) = favorite.database.clone() {
                    path = path.with_database(database);
                }
                if let Some(schema) = favorite.schema.clone() {
                    path = path.with_schema(schema);
                }
                path = path.with_extra(favorite.id.to_string());

                MetadataNode::new(path, &favorite.title, MetadataNodeKind::Query)
                    .leaf(true)
                    .detail(favorite.updated_at.format("%Y-%m-%d").to_string())
            })
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| left.label.cmp(&right.label));
        nodes
    }

    fn favorite_belongs_to_database(&self, favorite: &FavoriteQuery, node: &MetadataNode) -> bool {
        if favorite.connection_id != Some(node.path.connection_id) {
            return false;
        }
        favorite.database.as_deref() == node.path.database.as_deref()
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
                if ui.button(tr("Connect")).clicked() {
                    self.connect(connection_id);
                    *message = Some("Connecting...".to_string());
                    ui.close_menu();
                }
            }
            ConnectionStatus::Connecting => {
                ui.add_enabled(false, Button::new(tr("Connecting...")));
            }
            ConnectionStatus::Connected => {
                if ui.button(tr("Disconnect")).clicked() {
                    self.disconnect(connection_id);
                    *message = Some("Disconnected".to_string());
                    ui.close_menu();
                }
                if ui.button(tr("New Query")).clicked() {
                    let node = self.connection_node(connection_id);
                    if let Some(node) = node {
                        actions.push(ObjectTreeAction::OpenQuery {
                            connection_id,
                            database: node.path.database.clone(),
                            schema: node.path.schema.clone(),
                            table: None,
                        });
                    }
                    *message = Some("Opening new query tab".to_string());
                    ui.close_menu();
                }
            }
            ConnectionStatus::Failed => {
                if ui.button(tr("Retry Connect")).clicked() {
                    self.connect(connection_id);
                    *message = Some("Retrying connection".to_string());
                    ui.close_menu();
                }
            }
        }

        ui.separator();
        ui.add_enabled_ui(!matches!(status, ConnectionStatus::Connecting), |ui| {
            if ui.button(tr("Import Table...")).clicked() {
                actions.push(ObjectTreeAction::ImportTable { connection_id });
                *message = Some("Opening table import".to_string());
                ui.close_menu();
            }
            ui.menu_button(tr("Export Database"), |ui| {
                if ui.button(tr("Structure Only...")).clicked() {
                    actions.push(ObjectTreeAction::ExportDatabase {
                        connection_id,
                        database: None,
                        mode: DatabaseExportMode::Structure,
                    });
                    *message = Some("Export database structure".to_string());
                    ui.close_menu();
                }
                if ui.button(tr("Structure and Data...")).clicked() {
                    actions.push(ObjectTreeAction::ExportDatabase {
                        connection_id,
                        database: None,
                        mode: DatabaseExportMode::StructureAndData,
                    });
                    *message = Some("Export database structure and data".to_string());
                    ui.close_menu();
                }
            });
        });

        ui.separator();
        if ui.button(tr("Refresh")).clicked() {
            self.refresh_connection(connection_id);
            *message = Some("Refreshing...".to_string());
            ui.close_menu();
        }
        if ui.button(tr("Edit Connection...")).clicked() {
            actions.push(ObjectTreeAction::EditConnection(connection_id));
            ui.close_menu();
        }
        if ui.button(tr("Copy Name")).clicked() {
            let name = self
                .connection_node(connection_id)
                .map(|node| node.label)
                .unwrap_or_default();
            ui.output_mut(|o| o.copied_text = name.clone());
            *message = Some(format!("Copied: {name}"));
            ui.close_menu();
        }
        if ui.button(tr("Delete Connection")).clicked() {
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
            MetadataNodeKind::Database => {
                ui.menu_button(tr("Export Database"), |ui| {
                    if ui.button(tr("Structure Only...")).clicked() {
                        actions.push(ObjectTreeAction::ExportDatabase {
                            connection_id: node.path.connection_id,
                            database: node.path.database.clone(),
                            mode: DatabaseExportMode::Structure,
                        });
                        *message = Some(format!("Export structure: {}", node.label));
                        ui.close_menu();
                    }
                    if ui.button(tr("Structure and Data...")).clicked() {
                        actions.push(ObjectTreeAction::ExportDatabase {
                            connection_id: node.path.connection_id,
                            database: node.path.database.clone(),
                            mode: DatabaseExportMode::StructureAndData,
                        });
                        *message = Some(format!("Export structure and data: {}", node.label));
                        ui.close_menu();
                    }
                });
                ui.separator();
                if ui.button(tr("Delete Database...")).clicked() {
                    actions.push(ObjectTreeAction::DropDatabase {
                        connection_id: node.path.connection_id,
                        database: node
                            .path
                            .database
                            .clone()
                            .unwrap_or_else(|| node.label.clone()),
                    });
                    ui.close_menu();
                }
                ui.separator();
            }
            MetadataNodeKind::Table => {
                if let Some(kind) = self.connection_kinds.get(&node.path.connection_id).copied() {
                    if let Some(action) = self.open_data_browser_action(node, kind) {
                        if ui.button(tr("Browse")).clicked() {
                            actions.push(action);
                            *message = Some(format!("Opening {}", node.label));
                            ui.close_menu();
                        }
                        return;
                    }
                }
                if ui.button(tr("Open Table Data")).clicked() {
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
                if ui.button(tr("View Structure")).clicked() {
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
                if ui.button(tr("Export Data")).clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::ExportTableData {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                if ui.button(tr("New Query on Table")).clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::OpenQuery {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table: Some(table),
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                if ui.button(tr("Import CSV...")).clicked() {
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
                if ui.button(tr("Clear Table...")).clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::ClearTable {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
                if ui.button(tr("Drop Table...")).clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, table| ObjectTreeAction::DropTable {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            table,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
            }
            MetadataNodeKind::View => {
                if ui.button(tr("Open View Data")).clicked() {
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
                if ui.button(tr("View Structure")).clicked() {
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
                if ui.button(tr("Delete")).clicked() {
                    self.emit_table_action(
                        node,
                        |conn_id, db, schema, view| ObjectTreeAction::DropView {
                            connection_id: conn_id,
                            database: db,
                            schema,
                            view,
                        },
                        actions,
                    );
                    ui.close_menu();
                }
            }
            MetadataNodeKind::Query => {
                if ui.button(tr("Open Query")).clicked() {
                    self.emit_saved_query_action(node, actions);
                    *message = Some(format!("Opening query {}", node.label));
                    ui.close_menu();
                }
                if ui.button(tr("Delete")).clicked() {
                    if let Some(favorite) = self.favorite_for_node(node) {
                        self.delete_favorite(favorite.id);
                        *message = Some(format!("Deleting query {}", node.label));
                        ui.close_menu();
                    }
                }
                ui.separator();
            }
            MetadataNodeKind::RedisKeysFolder => {
                let key = NodeKey::from(&node.path);
                let current = self.redis_pattern.get(&key).cloned().unwrap_or_default();
                let mut input = current.clone();
                ui.label(tr("Pattern (glob):"));
                let resp = ui.add(
                    TextEdit::singleline(&mut input)
                        .hint_text(tr("user:*"))
                        .desired_width(180.0),
                );
                if resp.lost_focus() && input != current {
                    self.apply_redis_pattern(key.clone(), input.clone());
                }
                if resp.changed() {
                    self.redis_pattern.insert(key.clone(), input.clone());
                }
                ui.horizontal(|ui| {
                    if ui.button(tr("Apply")).clicked() {
                        self.apply_redis_pattern(key.clone(), input.clone());
                        ui.close_menu();
                    }
                    if ui.button(tr("Clear")).clicked() {
                        self.apply_redis_pattern(key.clone(), String::new());
                        ui.close_menu();
                    }
                });
                ui.separator();
                if ui.button(tr("Refresh")).clicked() {
                    self.refresh_node(node.clone());
                    ui.close_menu();
                }
            }
            MetadataNodeKind::RedisDatabase => {
                if ui.button(tr("Clear Database...")).clicked() {
                    let database = node
                        .path
                        .database
                        .as_deref()
                        .and_then(|value| value.parse::<u8>().ok())
                        .unwrap_or(0);
                    actions.push(ObjectTreeAction::ClearRedisDatabase {
                        connection_id: node.path.connection_id,
                        database,
                    });
                    ui.close_menu();
                }
                ui.separator();
            }
            MetadataNodeKind::RedisKey => {
                if ui.button(tr("View Key")).clicked() {
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
                if ui.button(tr("Delete Key")).clicked() {
                    self.emit_key_action(
                        node,
                        |conn_id, db, key| ObjectTreeAction::DeleteRedisKey {
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
            if ui.button(tr("Refresh")).clicked() {
                self.refresh_node(node.clone());
                ui.close_menu();
            }
        }

        if ui.button(tr("Copy Name")).clicked() {
            ui.output_mut(|o| o.copied_text = node.label.clone());
            *message = Some(format!("Copied: {}", node.label));
            ui.close_menu();
        }

        if let Some(database) = &node.path.database {
            if ui.button(tr("Copy Full Path")).clicked() {
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
                if let Some(kind) = self.connection_kinds.get(&node.path.connection_id).copied() {
                    if let Some(action) = self.open_data_browser_action(node, kind) {
                        actions.push(action);
                        *message = Some(format!("Opening {}", node.label));
                        return;
                    }
                }
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
            MetadataNodeKind::Query => {
                self.emit_saved_query_action(node, actions);
                *message = Some(format!("Opening query {}", node.label));
            }
            _ => {
                *message = Some(format!("Selected: {}", node.label));
            }
        }
    }

    /// Build an `OpenDataBrowser` action for a search/document/http-sql tree
    /// node, or `None` when the connection kind is relational.
    fn open_data_browser_action(
        &self,
        node: &MetadataNode,
        kind: DataSourceKind,
    ) -> Option<ObjectTreeAction> {
        if !(kind.is_search() || kind.is_document() || kind.is_http_sql()) {
            return None;
        }
        let object = node
            .path
            .object
            .clone()
            .unwrap_or_else(|| node.label.clone());
        Some(ObjectTreeAction::OpenDataBrowser {
            connection_id: node.path.connection_id,
            kind,
            database: node.path.database.clone(),
            object,
        })
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

    fn emit_saved_query_action(&self, node: &MetadataNode, actions: &mut Vec<ObjectTreeAction>) {
        let Some(favorite) = self.favorite_for_node(node) else {
            return;
        };
        let Some(connection_id) = favorite.connection_id else {
            return;
        };
        actions.push(ObjectTreeAction::OpenSavedQuery {
            connection_id,
            database: favorite.database.clone(),
            schema: favorite.schema.clone(),
            title: favorite.title.clone(),
            sql: favorite.sql.clone(),
        });
    }

    fn favorite_for_node(&self, node: &MetadataNode) -> Option<&FavoriteQuery> {
        let favorite_id = node.path.extra.as_deref()?;
        self.favorite_queries
            .iter()
            .find(|favorite| favorite.id.to_string() == favorite_id)
    }

    fn delete_favorite(&self, id: db_common::QueryId) {
        let tx = self.tx.clone();
        let service = self.query_service.clone();
        self.runtime.spawn(async move {
            let result = service.remove_favorite(id).await;
            let _ = tx.send(TreeEvent::FavoriteDeleted(result));
        });
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
        self.child_load_errors.remove(&key);
        self.loading_children.remove(&key);

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
                self.load_children_for_path_with_key(path, key.clone());
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

    fn load_favorites(&mut self) {
        let tx = self.tx.clone();
        let service = self.query_service.clone();

        self.runtime.spawn(async move {
            let favorites = service.favorites().await;
            let _ = tx.send(TreeEvent::FavoritesLoaded(favorites));
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
        self.child_load_errors.remove(&key);
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let path = node.path;

        self.runtime.spawn(async move {
            let result = service.children(&path, None).await;
            let _ = tx.send(TreeEvent::ChildrenLoaded(key, result));
        });
    }

    fn load_children(&mut self, node: MetadataNode) {
        let (path, key) = self.child_request_path(&node);
        if self.child_cache.contains_key(&key) || self.loading_children.contains(&key) {
            return;
        }
        if node.kind == MetadataNodeKind::QueriesFolder {
            let children = self.query_nodes_for_folder(&node);
            self.child_cache.insert(key, children);
            return;
        }
        self.load_children_for_path_with_key(path, key);
    }

    fn load_children_for_path_with_key(&mut self, path: MetadataPath, key: NodeKey) {
        self.loading_children.insert(key.clone());
        self.child_load_errors.remove(&key);
        let tx = self.tx.clone();
        let service = self.metadata_service.clone();

        self.runtime.spawn(async move {
            let result = service.children(&path, None).await;
            let _ = tx.send(TreeEvent::ChildrenLoaded(key, result));
        });
    }

    fn child_request_path(&self, node: &MetadataNode) -> (MetadataPath, NodeKey) {
        let key = NodeKey::from(&node.path);
        if node.kind != MetadataNodeKind::RedisKeysFolder {
            return (node.path.clone(), key);
        }

        let mut path = node.path.clone();
        if let Some(pattern) = self
            .redis_pattern
            .get(&key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            path = path.with_extra(pattern.to_string());
        }
        (path, key)
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
        self.child_load_errors
            .retain(|k, _| k.connection_id != connection_id);
        self.loading_children
            .retain(|k| k.connection_id != connection_id);
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
        let mut path = refresh_path_for_node(&node);
        let key = NodeKey::from(&path);
        if matches!(
            node.kind,
            MetadataNodeKind::RedisKeysFolder | MetadataNodeKind::RedisKey
        ) {
            if let Some(pattern) = self
                .redis_pattern
                .get(&key)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| node.path.extra.clone())
            {
                path = path.with_extra(pattern);
            }
        }
        self.child_cache.remove(&key);
        self.child_load_errors.remove(&key);
        self.loading_children.remove(&key);

        // Drop descendant caches for non-leaf refresh.
        self.child_cache.retain(|k, _| !is_descendant(k, &key));
        self.child_load_errors
            .retain(|k, _| !is_descendant(k, &key));
        self.loading_children.retain(|k| !is_descendant(k, &key));

        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        self.loading_children.insert(key.clone());

        self.runtime.spawn(async move {
            let result = service.children(&path, None).await;
            let _ = tx.send(TreeEvent::ChildrenLoaded(key, result));
        });
    }

    fn connect(&mut self, connection_id: ConnectionId) {
        self.reconnect_suppressed.remove(&connection_id);
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
        self.reconnect_suppressed.insert(connection_id);
        self.reconnects.remove(&connection_id);
        if let Some((_, status)) = self.connections.get_mut(&connection_id) {
            *status = ConnectionStatus::Disconnected;
        }
        let manager = self.session_manager.clone();
        self.runtime.spawn(async move {
            let _ = manager.disconnect(connection_id).await;
        });
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

    fn maybe_retry_reconnects(&mut self, ctx: &Context) {
        let now_ms = (ctx.input(|i| i.time) * 1000.0) as u64;
        let retry_ids = self
            .reconnects
            .iter()
            .filter_map(|(id, state)| {
                (!state.in_flight
                    && state.attempts < MAX_RECONNECT_ATTEMPTS
                    && now_ms >= state.next_retry_ms)
                    .then_some(*id)
            })
            .collect::<Vec<_>>();

        for connection_id in retry_ids {
            if let Some(state) = self.reconnects.get_mut(&connection_id) {
                state.in_flight = true;
            }
            self.connect(connection_id);
            self.status = format!("Reconnecting {}...", connection_id.as_uuid());
        }
    }

    fn drain_events(&mut self, _ctx: &Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                TreeEvent::RootLoaded(result) => match result {
                    Ok(nodes) => {
                        let previous_connections = std::mem::take(&mut self.connections);
                        self.connection_kinds = nodes
                            .iter()
                            .filter_map(|n| {
                                n.detail
                                    .as_deref()
                                    .and_then(DataSourceKind::from_serialized)
                                    .map(|kind| (n.path.connection_id, kind))
                            })
                            .collect();
                        self.connections = nodes
                            .into_iter()
                            .map(|n| {
                                let status = previous_connections
                                    .get(&n.path.connection_id)
                                    .map(|(_, status)| *status)
                                    .unwrap_or(ConnectionStatus::NotConnected);
                                (n.path.connection_id, (TreeNode::new(n), status))
                            })
                            .collect();
                        self.selected_connections
                            .retain(|id| self.connections.contains_key(id));
                        self.status = format!("Loaded {} connections", self.connections.len());
                    }
                    Err(error) => {
                        self.status = format!("Failed to load tree: {}", error.message);
                    }
                },
                TreeEvent::ChildrenLoaded(key, result) => {
                    let is_connection = matches!(key.path_kind, MetadataNodeKind::Connection);
                    self.loading_children.remove(&key);
                    match result {
                        Ok(children) => {
                            self.child_load_errors.remove(&key);
                            if is_connection {
                                if let Some((tree_node, status)) =
                                    self.connections.get_mut(&key.connection_id)
                                {
                                    tree_node.children = Some(children);
                                    tree_node.loading = false;
                                    tree_node.load_error = None;
                                    *status = ConnectionStatus::Connected;
                                    self.reconnects.remove(&key.connection_id);
                                    self.reconnect_suppressed.remove(&key.connection_id);
                                }
                            } else {
                                self.child_cache.insert(key, children);
                            }
                        }
                        Err(error) => {
                            let error_text = tree_error_text(&error);
                            if is_connection {
                                if let Some((tree_node, _)) =
                                    self.connections.get_mut(&key.connection_id)
                                {
                                    tree_node.loading = false;
                                    tree_node.load_error = Some(error_text);
                                }
                            } else {
                                self.child_load_errors.insert(key, error_text.clone());
                                self.status = format!("Failed to load children: {}", error_text);
                            }
                        }
                    }
                }
                TreeEvent::FavoritesLoaded(favorites) => {
                    self.favorite_queries = favorites;
                    self.child_cache
                        .retain(|key, _| key.path_kind != MetadataNodeKind::QueriesFolder);
                }
                TreeEvent::FavoriteDeleted(result) => match result {
                    Ok(()) => {
                        self.status = "Saved query deleted".to_string();
                        self.reload_favorites();
                    }
                    Err(error) => {
                        self.status = format!("Delete saved query failed: {}", error.message);
                    }
                },
                TreeEvent::Connected(id, result) => {
                    if let Some((_, status)) = self.connections.get_mut(&id) {
                        *status = match &result {
                            Ok(_) => ConnectionStatus::Connected,
                            Err(_) => ConnectionStatus::Failed,
                        };
                    }
                    match result {
                        Ok(_) => {
                            self.reconnects.remove(&id);
                            self.refresh_connection(id);
                            self.status = "Connected".to_string();
                        }
                        Err(e) => {
                            let error_text = tree_error_text(&e);
                            self.schedule_reconnect(id, ConnectionStatus::Failed, _ctx);
                            self.status = format!("Connection failed: {}", error_text);
                        }
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
                            let previous = self.connections.get(&id).map(|(_, status)| *status);
                            if previous == Some(ConnectionStatus::Connected)
                                && new_status == ConnectionStatus::NotConnected
                            {
                                continue;
                            }
                            if let Some((_, status)) = self.connections.get_mut(&id) {
                                *status = new_status;
                            }
                            if matches!(new_status, ConnectionStatus::Connected) {
                                self.reconnects.remove(&id);
                                self.reconnect_suppressed.remove(&id);
                            } else if matches!(
                                new_status,
                                ConnectionStatus::Disconnected | ConnectionStatus::Failed
                            ) && previous != Some(new_status)
                            {
                                self.schedule_reconnect(id, new_status, _ctx);
                            }
                        }
                    }
                }
            }
        }
    }

    fn search_active(&self) -> bool {
        self.search_query.chars().count() >= SEARCH_MIN_CHARS
    }

    fn search_term(&self) -> Option<String> {
        self.search_active()
            .then(|| self.search_query.trim().to_lowercase())
            .filter(|query| !query.is_empty())
    }

    fn node_matches_search(&self, node: &MetadataNode) -> bool {
        let Some(query) = self.search_term() else {
            return false;
        };
        fuzzy_matches(&node.label.to_lowercase(), &query)
    }

    fn node_or_descendant_matches(&self, node: &MetadataNode) -> bool {
        if !self.search_active() {
            return true;
        }
        if self.node_matches_search(node) {
            return true;
        }
        let key = NodeKey::from(&node.path);
        self.child_cache
            .get(&key)
            .map(|children| {
                children
                    .iter()
                    .any(|child| self.node_or_descendant_matches(child))
            })
            .unwrap_or(false)
    }

    fn connection_matches_search(&self, tree_node: &TreeNode) -> bool {
        self.node_matches_search(&tree_node.node)
            || tree_node
                .children
                .as_ref()
                .map(|children| {
                    children
                        .iter()
                        .any(|child| self.node_or_descendant_matches(child))
                })
                .unwrap_or(false)
    }

    fn force_expanded(&self, node: &MetadataNode) -> bool {
        self.search_active() && self.node_or_descendant_matches(node) && !node.leaf
    }

    fn connection_force_expanded(&self, tree_node: &TreeNode) -> bool {
        self.search_active() && self.connection_matches_search(tree_node)
    }

    fn has_pending_work(&self) -> bool {
        self.connections
            .values()
            .any(|(node, status)| node.loading || matches!(status, ConnectionStatus::Connecting))
            || !self.loading_children.is_empty()
            || self.reconnects.values().any(|state| state.in_flight)
    }

    fn schedule_reconnect(
        &mut self,
        connection_id: ConnectionId,
        status: ConnectionStatus,
        ctx: &Context,
    ) {
        if self.reconnect_suppressed.contains(&connection_id)
            || !matches!(
                status,
                ConnectionStatus::Disconnected | ConnectionStatus::Failed
            )
        {
            return;
        }

        let now_ms = (ctx.input(|i| i.time) * 1000.0) as u64;
        let state = self
            .reconnects
            .entry(connection_id)
            .or_insert(ReconnectState {
                attempts: 0,
                next_retry_ms: now_ms + RECONNECT_BASE_DELAY_MS,
                in_flight: false,
            });

        if state.attempts >= MAX_RECONNECT_ATTEMPTS || state.in_flight {
            return;
        }

        if state.attempts == 0 || state.next_retry_ms <= now_ms {
            state.next_retry_ms =
                now_ms + RECONNECT_BASE_DELAY_MS * u64::from(state.attempts.saturating_add(1));
        }
        state.attempts = state.attempts.saturating_add(1);
        state.in_flight = false;
    }
}

fn should_activate_leaf(is_leaf: bool, double_clicked: bool) -> bool {
    is_leaf && double_clicked
}

struct TreeRowSpec<'a> {
    id_source: &'a str,
    depth: usize,
    kind: MetadataNodeKind,
    label: &'a str,
    detail: Option<&'a str>,
    expanded: bool,
    leaf: bool,
    selected: bool,
    matched: bool,
    strong: bool,
    /// Status shown as a small colored dot next to the connection name.
    connection_status: Option<Color32>,
}

fn tree_row(ui: &mut Ui, spec: TreeRowSpec<'_>) -> Response {
    let width = ui.available_width().max(1.0);
    let (_, rect) = ui.allocate_space(Vec2::new(width, TREE_ROW_HEIGHT));
    let response = ui.interact(
        rect,
        ui.id().with(("object_tree_row", spec.id_source)),
        Sense::click(),
    );
    let painter = ui.painter();

    let bg_fill = if spec.selected {
        ui.visuals().selection.bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    if bg_fill != Color32::TRANSPARENT {
        painter.rect_filled(
            rect.expand2(vec2(1.0, 0.0)),
            Rounding::same(TREE_ROW_RADIUS),
            bg_fill,
        );
    }
    if spec.selected {
        painter.rect_stroke(
            rect.expand2(vec2(1.0, 0.0)).shrink(0.5),
            Rounding::same(TREE_ROW_RADIUS),
            Stroke::new(1.0, ui.visuals().selection.stroke.color),
        );
    }

    draw_tree_guides(ui, rect, spec.depth);

    let center_y = rect.center().y;
    let mut x = rect.left() + 5.0 + tree_indent(spec.depth);

    if !spec.leaf {
        let cx = x + TREE_ARROW_WIDTH * 0.5;
        let points = if spec.expanded {
            vec![
                pos2(cx - 4.0, center_y - 2.0),
                pos2(cx + 4.0, center_y - 2.0),
                pos2(cx, center_y + 3.0),
            ]
        } else {
            vec![
                pos2(cx - 2.0, center_y - 4.0),
                pos2(cx - 2.0, center_y + 4.0),
                pos2(cx + 3.0, center_y),
            ]
        };
        painter.add(eframe::egui::epaint::Shape::convex_polygon(
            points,
            muted_text_color(ui),
            Stroke::NONE,
        ));
    }
    x += TREE_ARROW_WIDTH;

    if let Some(status_color) = spec.connection_status {
        painter.circle_filled(pos2(x + 6.0, center_y), 4.0, status_color);
        x += TREE_STATUS_DOT_WIDTH;
    }

    let (badge, badge_color) = node_kind_badge(spec.kind, spec.detail);
    let uses_object_icon = matches!(
        spec.kind,
        MetadataNodeKind::Database | MetadataNodeKind::RedisDatabase | MetadataNodeKind::Table
    );
    let badge_width = if uses_object_icon {
        23.0
    } else if badge.len() > 2 {
        28.0
    } else {
        23.0
    };
    let badge_rect = Rect::from_center_size(
        pos2(x + badge_width * 0.5, center_y),
        vec2(badge_width, 15.0),
    );
    match spec.kind {
        MetadataNodeKind::Database | MetadataNodeKind::RedisDatabase => {
            paint_database_icon(painter, badge_rect, badge_color);
        }
        MetadataNodeKind::Table => paint_table_icon(painter, badge_rect, badge_color),
        _ => {
            painter.rect_filled(badge_rect, Rounding::same(3.0), badge_color);
            painter.text(
                badge_rect.center(),
                Align2::CENTER_CENTER,
                badge,
                FontId::monospace(9.0),
                badge_text_color(badge_color),
            );
        }
    }
    x += badge_width + TREE_BADGE_GAP;

    let right_label = spec.detail.filter(|value| !value.trim().is_empty());
    let right_reserved = if right_label.is_some() { 92.0 } else { 6.0 };
    let text_rect = Rect::from_min_max(
        pos2(x, rect.top()),
        pos2((rect.right() - right_reserved).max(x + 24.0), rect.bottom()),
    );
    let text_painter = painter.with_clip_rect(text_rect);
    text_painter.text(
        pos2(x, center_y),
        Align2::LEFT_CENTER,
        spec.label,
        FontId::proportional(if spec.strong { 13.0 } else { 12.5 }),
        node_label_color(ui, spec.selected, spec.matched, spec.strong),
    );

    if let Some(label) = right_label {
        if rect.width() > 260.0 {
            painter.text(
                pos2(rect.right() - 7.0, center_y),
                Align2::RIGHT_CENTER,
                label,
                FontId::proportional(10.5),
                muted_text_color(ui),
            );
        }
    }

    response
}

fn paint_database_icon(painter: &eframe::egui::Painter, rect: Rect, color: Color32) {
    let body = rect.shrink2(vec2(2.5, 0.5));
    let cap_height = 5.0;
    let middle = Rect::from_min_max(
        pos2(body.left(), body.top() + cap_height * 0.5),
        pos2(body.right(), body.bottom() - cap_height * 0.5),
    );
    let top = Rect::from_center_size(
        pos2(body.center().x, body.top() + cap_height * 0.5),
        vec2(body.width(), cap_height),
    );
    let bottom = Rect::from_center_size(
        pos2(body.center().x, body.bottom() - cap_height * 0.5),
        vec2(body.width(), cap_height),
    );
    painter.rect_filled(middle, Rounding::ZERO, color);
    painter.rect_filled(bottom, Rounding::same(cap_height * 0.5), color);
    painter.rect_filled(top, Rounding::same(cap_height * 0.5), color);
    painter.rect_stroke(
        top,
        Rounding::same(cap_height * 0.5),
        Stroke::new(1.0, Color32::WHITE),
    );
    let divider = Color32::from_rgba_unmultiplied(255, 255, 255, 95);
    for y in [body.top() + 5.0, body.top() + 9.5] {
        painter.line_segment(
            [pos2(body.left() + 1.0, y), pos2(body.right() - 1.0, y)],
            Stroke::new(0.8, divider),
        );
    }
}

fn paint_table_icon(painter: &eframe::egui::Painter, rect: Rect, color: Color32) {
    let body = rect.shrink2(vec2(1.5, 0.5));
    painter.rect_filled(body, Rounding::same(2.0), color);
    painter.rect_stroke(
        body,
        Rounding::same(2.0),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 120)),
    );
    let grid = Color32::from_rgba_unmultiplied(255, 255, 255, 150);
    let header_y = body.top() + 4.5;
    painter.line_segment(
        [
            pos2(body.left() + 1.5, header_y),
            pos2(body.right() - 1.5, header_y),
        ],
        Stroke::new(1.0, grid),
    );
    painter.line_segment(
        [
            pos2(body.center().x, header_y + 0.5),
            pos2(body.center().x, body.bottom() - 1.5),
        ],
        Stroke::new(1.0, grid),
    );
    painter.line_segment(
        [
            pos2(body.left() + 1.5, body.top() + 9.0),
            pos2(body.right() - 1.5, body.top() + 9.0),
        ],
        Stroke::new(1.0, grid),
    );
}

fn tree_message_row(ui: &mut Ui, depth: usize, text: &str, error: bool) {
    let width = ui.available_width().max(1.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, TREE_ROW_HEIGHT), Sense::hover());
    draw_tree_guides(ui, rect, depth);
    let color = if error {
        ui.visuals().error_fg_color
    } else {
        muted_text_color(ui)
    };
    ui.painter().text(
        pos2(
            rect.left() + 5.0 + tree_indent(depth) + TREE_ARROW_WIDTH + 4.0,
            rect.center().y,
        ),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.0),
        color,
    );
}

fn clicked_in_arrow(response: &Response, depth: usize) -> bool {
    response
        .interact_pointer_pos()
        .map(|pos| {
            let left = response.rect.left() + 5.0 + tree_indent(depth);
            pos.x >= left && pos.x <= left + TREE_ARROW_WIDTH + 2.0
        })
        .unwrap_or(false)
}

fn draw_tree_guides(ui: &Ui, rect: Rect, depth: usize) {
    if depth == 0 {
        return;
    }
    let stroke = Stroke::new(1.0, tree_guide_color(ui));
    for level in 0..depth {
        let x = rect.left() + 5.0 + tree_indent(level) + TREE_ARROW_WIDTH * 0.5;
        ui.painter()
            .line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], stroke);
    }
}

fn tree_indent(depth: usize) -> f32 {
    (depth as f32 * TREE_INDENT).min(TREE_MAX_INDENT)
}

fn node_kind_badge(kind: MetadataNodeKind, detail: Option<&str>) -> (&'static str, Color32) {
    if kind == MetadataNodeKind::Connection {
        let detail = detail.unwrap_or_default().to_ascii_lowercase();
        if detail.contains("postgres") {
            return ("PG", Color32::from_rgb(58, 116, 178));
        }
        if detail.contains("mysql") {
            return ("MY", Color32::from_rgb(27, 131, 151));
        }
        if detail.contains("sqlite") {
            return ("SL", Color32::from_rgb(110, 112, 118));
        }
        if detail.contains("duck") {
            return ("DK", Color32::from_rgb(188, 139, 37));
        }
        if detail.contains("redis") {
            return ("RD", Color32::from_rgb(192, 60, 54));
        }
    }

    match kind {
        MetadataNodeKind::Connection => ("CN", Color32::from_rgb(94, 107, 123)),
        MetadataNodeKind::Database | MetadataNodeKind::RedisDatabase => {
            ("DB", Color32::from_rgb(79, 132, 196))
        }
        MetadataNodeKind::Schema => ("SC", Color32::from_rgb(100, 151, 96)),
        MetadataNodeKind::TablesFolder => ("TBL", Color32::from_rgb(86, 117, 151)),
        MetadataNodeKind::ViewsFolder => ("VW", Color32::from_rgb(121, 104, 173)),
        MetadataNodeKind::FunctionsFolder => ("FN", Color32::from_rgb(154, 111, 65)),
        MetadataNodeKind::QueriesFolder => ("Q", Color32::from_rgb(70, 124, 180)),
        MetadataNodeKind::ColumnsFolder => ("COL", Color32::from_rgb(94, 122, 122)),
        MetadataNodeKind::IndexesFolder => ("IX", Color32::from_rgb(113, 116, 160)),
        MetadataNodeKind::ForeignKeysFolder => ("FK", Color32::from_rgb(126, 133, 88)),
        MetadataNodeKind::TriggersFolder => ("TR", Color32::from_rgb(157, 102, 91)),
        MetadataNodeKind::Table => ("T", Color32::from_rgb(47, 128, 197)),
        MetadataNodeKind::View => ("V", Color32::from_rgb(118, 88, 177)),
        MetadataNodeKind::Column => ("C", Color32::from_rgb(63, 143, 136)),
        MetadataNodeKind::Index => ("IX", Color32::from_rgb(93, 96, 167)),
        MetadataNodeKind::ForeignKey => ("FK", Color32::from_rgb(114, 124, 71)),
        MetadataNodeKind::Trigger => ("TR", Color32::from_rgb(174, 92, 76)),
        MetadataNodeKind::Function => ("ƒ", Color32::from_rgb(161, 100, 48)),
        MetadataNodeKind::Query => ("Q", Color32::from_rgb(52, 111, 176)),
        MetadataNodeKind::RedisKeysFolder | MetadataNodeKind::RedisKey => {
            ("K", Color32::from_rgb(190, 66, 62))
        }
    }
}

fn badge_text_color(color: Color32) -> Color32 {
    let luminance =
        0.299 * f32::from(color.r()) + 0.587 * f32::from(color.g()) + 0.114 * f32::from(color.b());
    if luminance > 150.0 {
        Color32::from_rgb(24, 28, 32)
    } else {
        Color32::WHITE
    }
}

fn muted_text_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(150, 155, 162)
    } else {
        Color32::from_rgb(96, 103, 112)
    }
}

fn refresh_path_for_node(node: &MetadataNode) -> MetadataPath {
    let base = MetadataPath::connection(node.path.connection_id);
    let mut path = match node.kind {
        MetadataNodeKind::Table | MetadataNodeKind::View => {
            if let Some(schema) = node.path.schema.clone() {
                base.child(MetadataNodeKind::Schema).with_schema(schema)
            } else {
                base.child(MetadataNodeKind::Database)
            }
        }
        MetadataNodeKind::Column => base
            .child(MetadataNodeKind::ColumnsFolder)
            .with_object(node.path.object.clone().unwrap_or_default()),
        MetadataNodeKind::Index => base
            .child(MetadataNodeKind::IndexesFolder)
            .with_object(node.path.object.clone().unwrap_or_default()),
        MetadataNodeKind::ForeignKey => base
            .child(MetadataNodeKind::ForeignKeysFolder)
            .with_object(node.path.object.clone().unwrap_or_default()),
        MetadataNodeKind::Trigger => base
            .child(MetadataNodeKind::TriggersFolder)
            .with_object(node.path.object.clone().unwrap_or_default()),
        MetadataNodeKind::RedisKey => base.child(MetadataNodeKind::RedisKeysFolder),
        _ => node.path.clone(),
    };

    if let Some(database) = node.path.database.clone() {
        path = path.with_database(database);
    }
    if !matches!(node.kind, MetadataNodeKind::Table | MetadataNodeKind::View) {
        if let Some(schema) = node.path.schema.clone() {
            path = path.with_schema(schema);
        }
    }
    path
}

fn tree_guide_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(48, 52, 58)
    } else {
        Color32::from_rgb(218, 223, 229)
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

fn connection_status_color(status: &ConnectionStatus) -> Color32 {
    match status {
        ConnectionStatus::NotConnected => Color32::from_rgb(220, 60, 60),
        ConnectionStatus::Connecting => Color32::from_rgb(255, 200, 0),
        ConnectionStatus::Connected => Color32::from_rgb(64, 180, 64),
        ConnectionStatus::Failed => Color32::from_rgb(220, 60, 60),
        ConnectionStatus::Disconnected => Color32::from_rgb(220, 60, 60),
    }
}

fn tree_error_text(error: &AppError) -> String {
    match error.detail.as_deref() {
        Some(detail) if !detail.is_empty() => format!("{} - {}", error.message, detail),
        _ => error.message.clone(),
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

fn fuzzy_matches(label: &str, query: &str) -> bool {
    if label.contains(query) {
        return true;
    }

    let mut query_chars = query.chars();
    let mut current = query_chars.next();
    for ch in label.chars() {
        if current == Some(ch) {
            current = query_chars.next();
            if current.is_none() {
                return true;
            }
        }
    }
    false
}

fn node_label_color(ui: &Ui, selected: bool, matched: bool, strong: bool) -> Color32 {
    if selected {
        if ui.visuals().dark_mode {
            Color32::WHITE
        } else {
            Color32::from_rgb(18, 24, 30)
        }
    } else if matched {
        Color32::from_rgb(255, 210, 90)
    } else if strong {
        if ui.visuals().dark_mode {
            Color32::from_rgb(238, 241, 245)
        } else {
            Color32::from_rgb(24, 30, 36)
        }
    } else {
        ui.visuals().text_color()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_nodes_do_not_activate_on_single_click() {
        assert!(!should_activate_leaf(true, false));
    }

    #[test]
    fn tables_and_views_activate_on_double_click() {
        assert!(should_activate_leaf(true, true));
    }

    #[test]
    fn other_leaf_nodes_still_require_double_click() {
        assert!(!should_activate_leaf(true, false));
        assert!(should_activate_leaf(true, true));
        assert!(!should_activate_leaf(false, true));
    }
}
