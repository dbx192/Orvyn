use app_core::{metadata::MetadataService, session::SessionManager};
use db_common::{
    AppError, ConnectionId, ConnectionStatus, MetadataNode, MetadataNodeKind, MetadataPath,
};
use eframe::egui::{
    vec2, Align, Button, Color32, Frame, Id, Label, Layout, RichText, ScrollArea, Sense,
    TextEdit, Ui, Widget,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc::{self, Receiver, Sender},
};
use tokio::runtime::Handle;

#[derive(Clone, PartialEq, Eq, Hash)]
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
        };
        ui.load_root();
        ui
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Option<String> {
        self.drain_events();

        let mut message = None;

        self.search_bar(ui);

        ui.separator();

        ScrollArea::vertical().id_source("object_tree_scroll").show(ui, |ui| {
            if self.search_results.is_some() {
                self.search_results_ui(ui, &mut message);
            } else {
                self.tree_ui(ui, &mut message);
            }
        });

        message
    }

    fn search_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("🔍");
            let response = ui.add(
                TextEdit::singleline(&mut self.search_query)
                    .hint_text("Search (min 2 chars)...")
                    .desired_width(180.0),
            );
            if response.changed() {
                self.search_results = None;
                if self.search_query.len() >= 2 {
                    self.perform_search();
                }
            }
            if ui.small_button("✕").clicked() {
                self.search_query.clear();
                self.search_results = None;
            }
        });
    }

    fn tree_ui(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        let connection_ids: Vec<ConnectionId> = self.connections.keys().copied().collect();

        for conn_id in connection_ids {
            let (tree_node, status) = match self.connections.get(&conn_id) {
                Some(t) => (t.0.clone(), t.1),
                None => continue,
            };

            let key = tree_node.key();
            let key_str = node_key_id(&key);
            let expanded = self.expanded.contains(&key_str);
            let status_color = connection_status_color(&status);

            let node = tree_node.node.clone();
            let is_selected = self.selected_node == Some(key.clone());

            let header_response = ui.horizontal(|ui| {
                let arrow = if expanded { "▼" } else { "▶" };
                let arrow_resp = ui.add(
                    Label::new(RichText::new(arrow).color(Color32::GRAY).size(10.0))
                        .sense(Sense::click()),
                );

                ui.add(
                    Label::new(RichText::new("●").color(status_color).size(10.0))
                        .sense(Sense::click()),
                );

                let label_resp = ui.add(
                    Label::new(
                        RichText::new(&node.label)
                            .color(if is_selected {
                                ui.visuals().selection.bg_fill
                            } else {
                                Color32::default()
                            }),
                    )
                    .sense(Sense::click()),
                );

                if let Some(detail) = &node.detail {
                    ui.label(
                        RichText::new(format!(" ({})", detail))
                            .color(Color32::GRAY)
                            .size(10.0),
                    );
                }

                arrow_resp.clicked() || label_resp.clicked()
            });

            let clicked = header_response.inner;
            let full_resp = header_response.response.interact(Sense::click());

            let mut show_menu = false;
            full_resp.context_menu(|ui| {
                show_menu = true;
                self.connection_context_menu(ui, conn_id, message);
            });

            if clicked && !show_menu {
                self.toggle_connection(conn_id);
            }

            if full_resp.clicked() && !show_menu {
                self.selected_node = Some(key.clone());
            }

            if expanded {
                ui.indent("conn_children", |ui| {
                    if let Some(children) = &tree_node.children {
                        self.render_children(ui, children, message);
                    } else if tree_node.loading {
                        ui.label(RichText::new("Loading...").color(Color32::GRAY));
                    } else if let Some(error) = &tree_node.load_error {
                        ui.colored_label(Color32::RED, format!("Error: {}", error));
                        if ui.button("Retry").clicked() {
                            self.refresh_connection(conn_id);
                        }
                    }
                });
            }

            ui.separator();
        }
    }

    fn render_children(&mut self, ui: &mut Ui, children: &[MetadataNode], message: &mut Option<String>) {
        for child in children {
            self.render_tree_node(ui, child, message);
        }
    }

    fn render_tree_node(&mut self, ui: &mut Ui, node: &MetadataNode, message: &mut Option<String>) {
        let key = NodeKey::from(&node.path);
        let key_str = node_key_id(&key);
        let expanded = self.expanded.contains(&key_str);
        let is_leaf = node.leaf;
        let is_selected = self.selected_node == Some(key.clone());
        let has_children = self.child_cache.contains_key(&key);
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
                Label::new(
                    RichText::new(&node.label)
                        .color(if is_selected {
                            ui.visuals().selection.bg_fill
                        } else {
                            Color32::default()
                        }),
                )
                .sense(Sense::click()),
            );

            let resp = label_resp.interact(Sense::click());
            let mut show_menu = false;
            resp.context_menu(|ui| {
                show_menu = true;
                self.node_context_menu(ui, &node_clone, message);
            });

            if resp.clicked() && !show_menu {
                self.selected_node = Some(key);
                *message = Some(format!("Selected: {}", node.label));
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
            ui.indent("node_children", |ui| {
                if let Some(children) = children {
                    if children.is_empty() {
                        ui.label(RichText::new("(empty)").color(Color32::GRAY));
                    } else {
                        self.render_children(ui, &children, message);
                    }
                } else {
                    ui.label(RichText::new("Loading...").color(Color32::GRAY));
                }
            });
        }
    }

    fn search_results_ui(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        if let Some(results) = &self.search_results {
            if results.is_empty() {
                ui.label(RichText::new("No results found").color(Color32::GRAY));
            } else {
                for (result, depth) in results {
                    let key = NodeKey::from(&result.path);
                    let is_selected = self.selected_node == Some(key.clone());

                    let indent = *depth as f32 * 12.0;
                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        let icon = node_kind_icon(result.kind);
                        ui.label(RichText::new(icon).size(12.0));

                        let resp = ui.add(
                            Label::new(
                                RichText::new(&result.label)
                                    .color(if is_selected {
                                        ui.visuals().selection.bg_fill
                                    } else {
                                        Color32::default()
                                    }),
                            )
                            .sense(Sense::click()),
                        );

                        if resp.clicked() {
                            self.selected_node = Some(key.clone());
                            *message = Some(format!("Selected: {}", result.label));
                        }
                    });

                    let path_desc = format!(
                        "  {}",
                        result_path_description(result)
                    );
                    ui.label(
                        RichText::new(path_desc)
                            .color(Color32::GRAY)
                            .size(10.0),
                    );
                    ui.add_space(2.0);
                }
            }
        }
    }

    fn connection_context_menu(
        &mut self,
        ui: &mut Ui,
        connection_id: ConnectionId,
        message: &mut Option<String>,
    ) {
        if ui.button("Connect").clicked() {
            self.connect(connection_id);
            *message = Some("Connecting...".to_string());
            ui.close_menu();
        }
        if ui.button("Disconnect").clicked() {
            self.disconnect(connection_id);
            *message = Some("Disconnected".to_string());
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Refresh").clicked() {
            self.refresh_connection(connection_id);
            *message = Some("Refreshing...".to_string());
            ui.close_menu();
        }
        if ui.button("Edit Connection").clicked() {
            *message = Some("Edit connection".to_string());
            ui.close_menu();
        }
        ui.separator();
        if ui.button("New Query").clicked() {
            *message = Some("New query tab".to_string());
            ui.close_menu();
        }
    }

    fn node_context_menu(
        &mut self,
        ui: &mut Ui,
        node: &MetadataNode,
        message: &mut Option<String>,
    ) {
        match node.kind {
            MetadataNodeKind::Table | MetadataNodeKind::View => {
                if ui.button("Open Table Data").clicked() {
                    *message = Some(format!("Open table: {}", node.label));
                    ui.close_menu();
                }
                if ui.button("View Structure").clicked() {
                    *message = Some(format!("View structure: {}", node.label));
                    ui.close_menu();
                }
                ui.separator();
            }
            MetadataNodeKind::RedisKey => {
                if ui.button("View Key").clicked() {
                    *message = Some(format!("View key: {}", node.label));
                    ui.close_menu();
                }
                ui.separator();
            }
            _ => {}
        }

        if ui.button("Refresh").clicked() {
            self.refresh_node(node.clone());
            ui.close_menu();
        }

        if ui.button("Copy Name").clicked() {
            ui.output_mut(|o| o.copied_text = node.label.clone());
            ui.close_menu();
        }
    }

    fn toggle_connection(&mut self, connection_id: ConnectionId) {
        let key_str = if let Some((tree_node, _)) = self.connections.get(&connection_id) {
            node_key_id(&tree_node.key())
        } else {
            return;
        };

        let expanded = self.expanded.contains(&key_str);

        if expanded {
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
            self.expanded.insert(key_str.clone());
            self.load_children(node);
        }
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

        let tx = self.tx.clone();
        let service = self.metadata_service.clone();
        let path = node.path;

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

        for (conn_id, (tree_node, _)) in &self.connections {
            if tree_node.node.label.to_lowercase().contains(&query) {
                results.push((tree_node.node.clone(), 0));
            }

            if let Some(children) = &tree_node.children {
                self.search_in_children(children, &query, 1, &mut results);
            }
        }

        results.truncate(100);
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

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                TreeEvent::RootLoaded(result) => {
                    match result {
                        Ok(nodes) => {
                            self.connections = nodes
                                .into_iter()
                                .map(|n| (n.path.connection_id, (TreeNode::new(n), ConnectionStatus::NotConnected)))
                                .collect();
                            self.status = format!("Loaded {} connections", self.connections.len());
                        }
                        Err(error) => {
                            self.status = format!("Failed to load: {}", error.message);
                        }
                    }
                }
                TreeEvent::ChildrenLoaded(key, result) => {
                    match result {
                        Ok(children) => {
                            let is_connection = matches!(key.path_kind, MetadataNodeKind::Connection);

                            if is_connection {
                                if let Some((tree_node, _)) =
                                    self.connections.get_mut(&key.connection_id)
                                {
                                    tree_node.children = Some(children.clone());
                                    tree_node.loading = false;
                                    tree_node.load_error = None;
                                }
                            } else {
                                self.child_cache.insert(key, children);
                            }
                        }
                        Err(error) => {
                            let is_connection = matches!(key.path_kind, MetadataNodeKind::Connection);

                            if is_connection {
                                if let Some((tree_node, _)) =
                                    self.connections.get_mut(&key.connection_id)
                                {
                                    tree_node.loading = false;
                                    tree_node.load_error = Some(error.message.clone());
                                }
                            }
                        }
                    }
                }
                TreeEvent::Connected(id, result) => {
                    if let Some((_, status)) = self.connections.get_mut(&id) {
                        match &result {
                            Ok(_) => *status = ConnectionStatus::Connected,
                            Err(_) => *status = ConnectionStatus::Failed,
                        }
                    }
                    self.status = match result {
                        Ok(_) => "Connected".to_string(),
                        Err(e) => format!("Connection failed: {}", e.message),
                    };
                }
            }
        }
    }
}

fn node_kind_icon(kind: MetadataNodeKind) -> &'static str {
    match kind {
        MetadataNodeKind::Connection => "🗄",
        MetadataNodeKind::Database => "🗄",
        MetadataNodeKind::Schema => "📁",
        MetadataNodeKind::TablesFolder => "📁",
        MetadataNodeKind::ViewsFolder => "📁",
        MetadataNodeKind::ColumnsFolder => "📁",
        MetadataNodeKind::IndexesFolder => "📁",
        MetadataNodeKind::Table => "📋",
        MetadataNodeKind::View => "👁",
        MetadataNodeKind::Column => "🔤",
        MetadataNodeKind::Index => "🔢",
        MetadataNodeKind::RedisDatabase => "🗄",
        MetadataNodeKind::RedisKeysFolder => "📁",
        MetadataNodeKind::RedisKey => "🔑",
    }
}

fn connection_status_color(status: &ConnectionStatus) -> Color32 {
    match status {
        ConnectionStatus::NotConnected => Color32::GRAY,
        ConnectionStatus::Connecting => Color32::from_rgb(255, 200, 0),
        ConnectionStatus::Connected => Color32::GREEN,
        ConnectionStatus::Failed => Color32::RED,
        ConnectionStatus::Disconnected => Color32::LIGHT_GRAY,
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
        MetadataNodeKind::RedisKey => "Key",
        MetadataNodeKind::RedisDatabase => "Redis DB",
        _ => "",
    };

    if parts.is_empty() {
        kind.to_string()
    } else {
        format!("{} - {}", kind, parts.join("."))
    }
}