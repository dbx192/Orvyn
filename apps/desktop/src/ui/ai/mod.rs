//! AI 助手（Ask 模式）。
pub mod client;
pub mod conversation;
pub mod panel;
pub mod prompts;
mod schema;
pub mod settings;

use db_common::ConnectionId;

/// SQL 编辑器传递给 AI 面板的当前上下文。
#[derive(Clone, Debug, Default)]
pub struct AiEditorContext {
    pub connection_id: Option<ConnectionId>,
    pub connection_name: Option<String>,
    pub db_type: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    /// 当前活动 tab 的 SQL（有选区时为选区文本）。
    pub current_sql: Option<String>,
    /// 最近一次执行报错。
    pub last_error: Option<String>,
    /// 最近一次查询结果预览（前几行）。
    pub last_result_preview: Option<String>,
    pub cached_tables: Vec<prompts::AiSchemaTable>,
    pub tables: Vec<AiTableTarget>,
    pub preferred_table: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AiTableTarget {
    pub name: String,
    pub schema: Option<String>,
    pub is_view: bool,
}

impl AiEditorContext {
    pub fn database_label(&self) -> Option<&str> {
        let database = self.database.as_deref()?;
        let path = std::path::Path::new(database);
        if matches!(self.db_type.as_deref(), Some("sqlite" | "duckdb")) && path.is_absolute() {
            path.file_name().and_then(|name| name.to_str())
        } else {
            Some(database)
        }
    }

    pub fn for_table(
        target: (ConnectionId, Option<String>, Option<String>, String),
        kind: db_common::DataSourceKind,
        columns: &[db_common::TableColumn],
    ) -> Self {
        let (connection_id, database, schema, table) = target;
        Self {
            connection_id: Some(connection_id),
            database,
            schema,
            db_type: Some(kind.as_str().to_owned()),
            cached_tables: vec![prompts::AiSchemaTable::from_columns(
                table.clone(),
                false,
                columns,
            )],
            preferred_table: Some(table),
            ..Default::default()
        }
    }
}
