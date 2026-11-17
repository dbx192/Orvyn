use crate::ui::object_tree::DatabaseExportMode;
use db_common::{
    AppError, ConnectionId, DataSourceKind, DatabaseFunction, Dialect, QueryResult, TableColumn,
    TableForeignKey, TableIndex, TableTrigger,
};
use import_export::ExportFormat;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

pub(super) type PendingTask<T> = Arc<Mutex<Option<Result<T, AppError>>>>;

/// Payload produced by the table-data async loader.
pub(super) struct TableDataPayload {
    pub(super) connection_id: ConnectionId,
    pub(super) kind: DataSourceKind,
    pub(super) database: Option<String>,
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) columns: Vec<TableColumn>,
    pub(super) result: QueryResult,
    pub(super) dialect: Box<dyn Dialect>,
}

/// Payload produced by the table-structure async loader.
pub(super) struct TableStructurePayload {
    pub(super) connection_id: ConnectionId,
    pub(super) kind: DataSourceKind,
    pub(super) database: Option<String>,
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) columns: Vec<TableColumn>,
    pub(super) indexes: Vec<TableIndex>,
    pub(super) foreign_keys: Vec<TableForeignKey>,
    pub(super) triggers: Vec<TableTrigger>,
    pub(super) functions: Vec<DatabaseFunction>,
    pub(super) create_sql: String,
}

pub(super) struct TableExportRequest {
    pub(super) connection_id: ConnectionId,
    pub(super) database: Option<String>,
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) format: ExportFormat,
}

pub(super) struct TableExportPayload {
    pub(super) table_name: String,
    pub(super) path: PathBuf,
    pub(super) format: ExportFormat,
    pub(super) row_count: usize,
}

/// Live progress of the in-flight table export, shared with the UI thread.
pub(super) struct TableExportProgress {
    pub(super) table_name: String,
    pub(super) format_label: &'static str,
    pub(super) rows_done: u64,
    pub(super) total_rows: Option<u64>,
    /// Final result kept in the status bar after completion:
    /// (success, message, finished at).
    pub(super) outcome: Option<(bool, String, Instant)>,
}

pub(super) struct DatabaseExportPayload {
    pub(super) path: PathBuf,
    pub(super) mode: DatabaseExportMode,
    pub(super) table_count: usize,
    pub(super) row_count: usize,
}

pub(super) struct ImportTableRequest {
    pub(super) connection_id: ConnectionId,
    pub(super) database: String,
    pub(super) schema: String,
    pub(super) table: String,
}

pub(super) struct IndexCreatePayload {
    pub(super) connection_id: ConnectionId,
    pub(super) database: Option<String>,
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) name: String,
}

/// Payload produced after a batch of raw DDL statements (index / FK reshapes,
/// trigger & function SQL) has run; triggers a structure reload.
pub(super) struct StructDdlPayload {
    pub(super) connection_id: ConnectionId,
    pub(super) database: Option<String>,
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) label: String,
}

pub(super) struct DropTableRequest {
    pub(super) connection_id: ConnectionId,
    pub(super) database: Option<String>,
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) object_kind: DropObjectKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DropObjectKind {
    Table,
    View,
    Database,
}

pub(super) struct DropTablePayload {
    pub(super) database: Option<String>,
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) object_kind: DropObjectKind,
}

#[derive(Clone)]
pub(super) enum ClearRequest {
    Table {
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    },
    RedisDatabase {
        connection_id: ConnectionId,
        database: u8,
    },
}

pub(super) struct ClearPayload {
    pub(super) request: ClearRequest,
}

/// Payload produced by the alter-table async executor.
pub(super) struct AlterTablePayload {
    pub(super) connection_id: ConnectionId,
    pub(super) database: Option<String>,
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) label: String,
}
