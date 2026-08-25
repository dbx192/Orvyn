#![allow(dead_code)]

use db_common::{ConnectionId, DataSourceKind, QueryId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceTabKind {
    Query,
    TableData,
    ResultSet,
    /// Search-index browser (Elasticsearch/Easysearch/Meilisearch/Manticore).
    Search,
    /// Document-collection browser (MongoDB).
    Document,
    /// HTTP-SQL browser (Cloudflare D1).
    HttpSql,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceTab {
    pub id: QueryId,
    pub title: String,
    pub kind: WorkspaceTabKind,
    pub dirty: bool,
    /// Connection the tab is bound to (set when opened from the object tree).
    pub connection_id: Option<ConnectionId>,
    /// Index / collection / table name the tab targets.
    pub object_name: Option<String>,
    /// Optional database qualifier (MongoDB / D1).
    pub database: Option<String>,
    /// The query JSON DSL or SQL entered by the user.
    pub query_text: String,
    /// Local JSON results rendered in the panel.
    pub results: Vec<serde_json::Value>,
    /// The pretty-printed result string shown to the user.
    pub result_text: String,
    /// Latest execution error, if any.
    pub error: Option<String>,
}

impl WorkspaceTab {
    pub fn new(title: impl Into<String>, kind: WorkspaceTabKind) -> Self {
        Self {
            id: QueryId::new(),
            title: title.into(),
            kind,
            dirty: false,
            connection_id: None,
            object_name: None,
            database: None,
            query_text: String::new(),
            results: Vec::new(),
            result_text: String::new(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TaskState {
    pub id: TaskId,
    pub label: String,
    pub status: TaskStatus,
}

impl TaskState {
    pub fn running(label: impl Into<String>) -> Self {
        Self {
            id: TaskId::new(),
            label: label.into(),
            status: TaskStatus::Running,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiError {
    pub title: String,
    pub message: String,
    pub detail: Option<String>,
    pub expanded: bool,
}

impl UiError {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            detail: None,
            expanded: false,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionTreeNode {
    pub label: String,
    pub kind: DataSourceKind,
    pub connection_id: Option<ConnectionId>,
    pub children: Vec<String>,
}

/// Application theme options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppTheme {
    Light,
    #[default]
    Dark,
}

impl AppTheme {
    pub fn toggle(&mut self) {
        *self = match self {
            AppTheme::Light => AppTheme::Dark,
            AppTheme::Dark => AppTheme::Light,
        };
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub active_tab: Option<QueryId>,
    pub tabs: Vec<WorkspaceTab>,
    pub tasks: Vec<TaskState>,
    pub errors: Vec<UiError>,
    pub connection_tree: Vec<ConnectionTreeNode>,
    pub status: String,
    pub theme: AppTheme,
    pub search_focused: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active_tab: None,
            tabs: Vec::new(),
            tasks: Vec::new(),
            errors: Vec::new(),
            connection_tree: Vec::new(),
            status: crate::ui::i18n::text("Ready"),
            theme: AppTheme::Dark,
            search_focused: false,
        }
    }
}

impl AppState {
    pub fn with_sample_workspace() -> Self {
        let table_tab = WorkspaceTab::new("customers", WorkspaceTabKind::TableData);
        let result_tab = WorkspaceTab::new("Query Result", WorkspaceTabKind::ResultSet);
        let active_tab = Some(table_tab.id);

        Self {
            active_tab,
            tabs: vec![table_tab, result_tab],
            tasks: vec![TaskState::running("UI ready")],
            errors: Vec::new(),
            connection_tree: vec![
                ConnectionTreeNode {
                    label: "Local DuckDB".to_string(),
                    kind: DataSourceKind::DuckDb,
                    connection_id: None,
                    children: vec!["main.customers".to_string(), "main.orders".to_string()],
                },
                ConnectionTreeNode {
                    label: "Local SQLite".to_string(),
                    kind: DataSourceKind::Sqlite,
                    connection_id: None,
                    children: vec!["main.users".to_string(), "main.audit_log".to_string()],
                },
            ],
            status: "Ready".to_string(),
            theme: AppTheme::Dark,
            search_focused: false,
        }
    }

    pub fn active_tab(&self) -> Option<&WorkspaceTab> {
        let active_tab = self.active_tab?;
        self.tabs.iter().find(|tab| tab.id == active_tab)
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub fn push_error(&mut self, error: UiError) {
        self.errors.push(error);
    }

    pub fn toggle_theme(&mut self) {
        self.theme.toggle();
    }

    pub fn set_search_focused(&mut self, focused: bool) {
        self.search_focused = focused;
    }
}
