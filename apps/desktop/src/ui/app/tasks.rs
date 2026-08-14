use super::{
    export_sql::{
        build_create_index_sql, database_export_mode_label, dialect_for_kind, export_database_sql,
        qualified_table_label, qualified_table_sql,
    },
    payloads::{
        AlterTablePayload, ClearPayload, ClearRequest, DatabaseExportPayload, DropTablePayload,
        DropTableRequest, IndexCreatePayload, PendingTask, TableDataPayload, TableExportPayload,
        TableExportRequest, TableStructurePayload,
    },
    ActivePanel, DesktopApp,
};
use crate::ui::{
    table_editor::TableEditor,
    table_structure::{
        build_alter_table_sql, AlterTableRequest, CreateIndexRequest, TableStructureView,
    },
};
use db_common::{
    AppError, DataSourceKind, Dialect, QueryExecutionMode, QueryExecutionRequest, QueryId,
    QueryResult, QUERY_RESULT_METADATA_PAGE_LIMIT, QUERY_RESULT_METADATA_PAGE_OFFSET,
    QUERY_RESULT_METADATA_TOTAL_ROWS,
};
use import_export::{ExportFormat, QueryResultFileWriter};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

impl DesktopApp {
    /// Poll pending async loads. Called at the top of each frame.
    pub(super) fn poll_pending_loads(&mut self) {
        let table_cell = self.pending_table_load.take();
        if let Some(cell) = table_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            let row_count = payload.result.rows.len();
                            let table_name = payload.table.clone();
                            let conn_id = payload.connection_id;
                            self.object_tree.mark_connected(conn_id);
                            let structure = TableStructureView::from_columns(
                                conn_id,
                                payload.kind,
                                payload.database.clone(),
                                payload.schema.clone(),
                                payload.table.clone(),
                                payload.columns.clone(),
                            );
                            let editor = TableEditor::with_services(
                                payload.schema.clone(),
                                &payload.table,
                                payload.columns,
                                payload.result,
                                payload.dialect,
                                self.query_service.clone(),
                                self.metadata_service.clone(),
                                conn_id,
                                payload.kind,
                                payload.database.clone(),
                                self.runtime.clone(),
                            );
                            let editor_id = eframe::egui::Id::new((
                                "table-editor",
                                conn_id.to_string(),
                                payload.database.as_deref().unwrap_or_default(),
                                payload.schema.as_deref().unwrap_or_default(),
                                payload.table.as_str(),
                            ));
                            self.table_structures.insert(editor_id, structure);
                            self.table_editors.insert(editor_id, editor);
                            if !self.table_tab_order.contains(&editor_id) {
                                self.table_tab_order.push(editor_id);
                            }
                            self.active_table_editor = Some(editor_id);
                            self.active_structure = Some(editor_id);
                            self.state.set_status(format!(
                                "Loaded table {} ({} rows)",
                                table_name, row_count
                            ));
                        }
                        Err(e) => {
                            let detail = e
                                .detail
                                .as_deref()
                                .filter(|detail| !detail.trim().is_empty())
                                .map(|detail| format!(": {detail}"))
                                .unwrap_or_default();
                            self.state.set_status(format!(
                                "Failed to load table: {}{}",
                                e.message, detail
                            ));
                        }
                    }
                }
            }
            if !done {
                self.pending_table_load = Some(cell);
            }
        }

        let structure_cell = self.pending_structure_load.take();
        if let Some(cell) = structure_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            self.object_tree.mark_connected(payload.connection_id);
                            let structure_id = eframe::egui::Id::new((
                                "table-editor",
                                payload.connection_id.to_string(),
                                payload.database.as_deref().unwrap_or_default(),
                                payload.schema.as_deref().unwrap_or_default(),
                                payload.table.as_str(),
                            ));
                            let view = TableStructureView::new(
                                payload.connection_id,
                                payload.kind,
                                payload.database,
                                payload.schema,
                                payload.table,
                                payload.columns,
                                payload.indexes,
                                payload.foreign_keys,
                                payload.triggers,
                                payload.functions,
                                payload.create_sql,
                            );
                            let table_name = view.table_name();
                            self.table_structures.insert(structure_id, view);
                            self.active_structure = Some(structure_id);
                            self.state
                                .set_status(format!("Loaded structure for {}", table_name));
                        }
                        Err(e) => {
                            self.state
                                .set_status(format!("Failed to load structure: {}", e.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_structure_load = Some(cell);
            }
        }

        let export_cell = self.pending_table_export.take();
        if let Some(cell) = export_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            let format_label = match payload.format {
                                ExportFormat::Csv => "CSV",
                                ExportFormat::Json => "JSON",
                                ExportFormat::ExcelCompatible => "Excel",
                            };
                            self.state.set_status(format!(
                                "{} export completed: {} ({} rows) -> {}",
                                format_label,
                                payload.table_name,
                                payload.row_count,
                                payload.path.display()
                            ));
                        }
                        Err(error) => {
                            self.state
                                .set_status(format!("Export failed: {}", error.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_table_export = Some(cell);
            }
        }

        let database_export_cell = self.pending_database_export.take();
        if let Some(cell) = database_export_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            let mode_label = database_export_mode_label(payload.mode);
                            self.state.set_status(format!(
                                "{} export completed: {} table(s), {} row(s) -> {}",
                                mode_label,
                                payload.table_count,
                                payload.row_count,
                                payload.path.display()
                            ));
                        }
                        Err(error) => {
                            self.state
                                .set_status(format!("Database export failed: {}", error.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_database_export = Some(cell);
            }
        }

        let index_create_cell = self.pending_index_create.take();
        if let Some(cell) = index_create_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            self.state
                                .set_status(format!("Index created: {}", payload.name));
                            self.spawn_structure_load(
                                payload.connection_id,
                                payload.database,
                                payload.schema,
                                payload.table,
                            );
                            self.active_panel = ActivePanel::TableStructure;
                        }
                        Err(error) => {
                            self.state
                                .set_status(format!("Create index failed: {}", error.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_index_create = Some(cell);
            }
        }

        let table_drop_cell = self.pending_table_drop.take();
        if let Some(cell) = table_drop_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            let table_name = qualified_table_label(
                                payload
                                    .schema
                                    .as_deref()
                                    .filter(|value| !value.is_empty())
                                    .or_else(|| {
                                        payload
                                            .database
                                            .as_deref()
                                            .filter(|value| !value.is_empty())
                                    }),
                                &payload.table,
                            );
                            let closed_editor_ids = self
                                .table_editors
                                .iter()
                                .filter_map(|(id, editor)| {
                                    (editor.table_name() == table_name).then_some(*id)
                                })
                                .collect::<Vec<_>>();
                            self.table_editors
                                .retain(|id, _| !closed_editor_ids.contains(id));
                            self.table_structures
                                .retain(|id, _| !closed_editor_ids.contains(id));
                            self.table_tab_order
                                .retain(|id| !closed_editor_ids.contains(id));
                            if self.table_editors.is_empty() {
                                self.table_structures.clear();
                                self.table_tab_order.clear();
                                self.active_structure = None;
                                self.active_panel = ActivePanel::QueryEditor;
                                self.active_table_editor = None;
                            } else if self
                                .active_table_editor
                                .is_some_and(|id| !self.table_editors.contains_key(&id))
                            {
                                self.active_table_editor =
                                    self.table_editors.keys().next().copied();
                                self.active_structure = self.active_table_editor;
                            }
                            self.object_tree.reload_root();
                            self.state
                                .set_status(format!("Dropped table: {table_name}"));
                        }
                        Err(error) => {
                            self.state
                                .set_status(format!("Drop table failed: {}", error.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_table_drop = Some(cell);
            }
        }

        let clear_cell = self.pending_clear.take();
        if let Some(cell) = clear_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            self.object_tree.reload_root();
                            match payload.request {
                                ClearRequest::Table {
                                    connection_id,
                                    database,
                                    schema,
                                    table,
                                } => {
                                    let table_name = qualified_table_label(
                                        schema.as_deref().or(database.as_deref()),
                                        &table,
                                    );
                                    let is_open = self
                                        .table_editors
                                        .values()
                                        .any(|editor| editor.table_name() == table_name);
                                    if is_open {
                                        self.spawn_table_data_load(
                                            connection_id,
                                            database,
                                            schema,
                                            table,
                                        );
                                        self.active_panel = ActivePanel::TableEditor;
                                    }
                                    self.state
                                        .set_status(format!("Cleared table: {table_name}"));
                                }
                                ClearRequest::RedisDatabase {
                                    connection_id,
                                    database,
                                } => {
                                    if let Some(browser) = &mut self.redis_browser {
                                        browser.refresh_database(connection_id, database);
                                    }
                                    self.state
                                        .set_status(format!("Cleared Redis DB {database}"));
                                }
                            }
                        }
                        Err(error) => {
                            self.state
                                .set_status(format!("Clear failed: {}", error.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_clear = Some(cell);
            }
        }

        let alter_cell = self.pending_alter_table.take();
        if let Some(cell) = alter_cell {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(payload) => {
                            self.state
                                .set_status(format!("Structure updated: {}", payload.label));
                            self.spawn_structure_load(
                                payload.connection_id,
                                payload.database,
                                payload.schema,
                                payload.table,
                            );
                            self.active_panel = ActivePanel::TableStructure;
                        }
                        Err(error) => {
                            self.state
                                .set_status(format!("Alter table failed: {}", error.message));
                        }
                    }
                }
            }
            if !done {
                self.pending_alter_table = Some(cell);
            }
        }
    }

    /// Spawn an async task to load table data (columns + paginated SELECT).
    pub(super) fn spawn_table_data_load(
        &mut self,
        connection_id: db_common::ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    ) {
        let metadata_service = self.metadata_service.clone();
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();

        let cell: PendingTask<TableDataPayload> = Arc::new(Mutex::new(None));
        self.pending_table_load = Some(cell.clone());

        let db = database.clone();
        let sch = schema.clone();
        let tbl = table.clone();

        runtime.spawn(async move {
            let result: Result<TableDataPayload, AppError> = async {
                let profile = connection_manager
                    .get_profile(connection_id)
                    .await?
                    .ok_or_else(|| AppError::validation("connection profile not found"))?;
                let kind = profile.kind;
                let dialect = dialect_for_kind(kind);
                let qualified = qualified_table_sql(
                    dialect.as_ref(),
                    kind,
                    db.as_deref(),
                    sch.as_deref(),
                    &tbl,
                )?;
                tracing::info!(connection_id = %connection_id, database = ?db, schema = ?sch, table = %tbl, sql = %qualified, "loading table data");

                let columns = metadata_service
                    .table_columns(connection_id, db.as_deref(), sch.as_deref(), &tbl, None)
                    .await?;
                let base_sql = ordered_table_select_sql(dialect.as_ref(), &qualified, &columns)?;
                let uses_direct_paging = matches!(kind, DataSourceKind::DuckDb | DataSourceKind::Doris);
                let sql = if uses_direct_paging {
                    format!("{base_sql} LIMIT 100 OFFSET 0")
                } else {
                    base_sql
                };
                let report = query_service
                    .execute(
                        QueryExecutionRequest {
                            tab_id: QueryId::new(),
                            connection_id,
                            database: db.clone(),
                            schema: sch.clone(),
                            sql,
                            mode: QueryExecutionMode::All,
                            cursor_offset: 0,
                            selection: None,
                            timeout_ms: 30_000,
                            result_limit: (!uses_direct_paging).then_some(100),
                            result_offset: 0,
                            result_total_rows: None,
                        },
                        None,
                    )
                    .await?;
                let mut data = report
                    .results
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| QueryResult {
                        statement: String::new(),
                        columns: Vec::new(),
                        rows: Vec::new(),
                        affected_rows: 0,
                        execution_time_ms: 0,
                        notices: Vec::new(),
                        metadata: Default::default(),
                    });
                // DuckDB and Doris use explicit paging instead of the
                // generic SELECT wrapper. This guarantees consistent metadata
                // for the UI and avoids planner-dependent wrapper behavior.
                if uses_direct_paging {
                    let count_report = query_service
                        .execute(
                            QueryExecutionRequest {
                                tab_id: QueryId::new(),
                                connection_id,
                                database: db.clone(),
                                schema: sch.clone(),
                                sql: format!("SELECT COUNT(*) AS total_rows FROM {qualified}"),
                                mode: QueryExecutionMode::All,
                                cursor_offset: 0,
                                selection: None,
                                timeout_ms: 30_000,
                                result_limit: None,
                                result_offset: 0,
                                result_total_rows: None,
                            },
                            None,
                        )
                        .await?;
                    let total_rows = count_report
                        .results
                        .first()
                        .and_then(|result| result.rows.first())
                        .and_then(|row| row.values.first())
                        .and_then(|value| match value {
                            db_common::CellValue::Int(value) if *value >= 0 => Some(*value as u64),
                            db_common::CellValue::UInt(value) => Some(*value),
                            db_common::CellValue::Text(value)
                            | db_common::CellValue::Decimal(value) => value.parse().ok(),
                            _ => None,
                        })
                        .ok_or_else(|| AppError::internal("table row count was invalid"))?;
                    data.metadata.insert(
                        QUERY_RESULT_METADATA_PAGE_LIMIT.to_string(),
                        "100".to_string(),
                    );
                    data.metadata.insert(
                        QUERY_RESULT_METADATA_PAGE_OFFSET.to_string(),
                        "0".to_string(),
                    );
                    data.metadata.insert(
                        QUERY_RESULT_METADATA_TOTAL_ROWS.to_string(),
                        total_rows.to_string(),
                    );
                }
                Ok(TableDataPayload {
                    connection_id,
                    kind,
                    database: db,
                    schema: sch,
                    table: tbl,
                    columns,
                    result: data,
                    dialect,
                })
            }
            .await;

            if let Err(error) = &result {
                tracing::error!(connection_id = %connection_id, table = %table, message = %error.message, detail = ?error.detail, "table data load failed");
            }
            *cell.lock().unwrap() = Some(result);
        });
    }

    /// Spawn an async task to load full table structure metadata.
    pub(super) fn spawn_structure_load(
        &mut self,
        connection_id: db_common::ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    ) {
        let metadata_service = self.metadata_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();

        let cell: PendingTask<TableStructurePayload> = Arc::new(Mutex::new(None));
        self.pending_structure_load = Some(cell.clone());

        let db = database.clone();
        let sch = schema.clone();
        let tbl = table.clone();

        runtime.spawn(async move {
            let db_ref = db.as_deref();
            let sch_ref = sch.as_deref();

            let kind = connection_manager
                .get_profile(connection_id)
                .await
                .ok()
                .flatten()
                .map(|profile| profile.kind)
                .unwrap_or(DataSourceKind::PostgreSql);

            let (
                columns_result,
                indexes_result,
                fks_result,
                triggers_result,
                functions_result,
                sql_result,
            ) = tokio::join!(
                metadata_service.table_columns(connection_id, db_ref, sch_ref, &tbl, None),
                metadata_service.table_indexes(connection_id, db_ref, sch_ref, &tbl, None),
                metadata_service.table_foreign_keys(connection_id, db_ref, sch_ref, &tbl, None),
                metadata_service.table_triggers(connection_id, db_ref, sch_ref, &tbl, None),
                metadata_service.schema_functions(connection_id, db_ref, sch_ref, None),
                metadata_service.get_create_table_sql(connection_id, db_ref, sch_ref, &tbl, None),
            );

            let result = match (
                columns_result,
                indexes_result,
                fks_result,
                triggers_result,
                functions_result,
                sql_result,
            ) {
                (
                    Ok(columns),
                    Ok(indexes),
                    Ok(foreign_keys),
                    Ok(triggers),
                    Ok(functions),
                    Ok(create_sql),
                ) => Ok(TableStructurePayload {
                    connection_id,
                    kind,
                    database: db,
                    schema: sch,
                    table: tbl,
                    columns,
                    indexes,
                    foreign_keys,
                    triggers,
                    functions,
                    create_sql,
                }),
                (Err(e), ..) => Err(e),
                (_, Err(e), ..) => Err(e),
                (_, _, Err(e), ..) => Err(e),
                (_, _, _, Err(e), ..) => Err(e),
                (_, _, _, _, Err(e), _) => Err(e),
                (_, _, _, _, _, Err(e)) => Err(e),
            };

            *cell.lock().unwrap() = Some(result);
        });
    }

    pub(super) fn spawn_table_export(&mut self, request: TableExportRequest, path: PathBuf) {
        const EXPORT_PAGE_SIZE: u32 = 5_000;

        let metadata_service = self.metadata_service.clone();
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();
        let cell: PendingTask<TableExportPayload> = Arc::new(Mutex::new(None));
        self.pending_table_export = Some(cell.clone());

        runtime.spawn(async move {
            let result = async {
                let profile = connection_manager
                    .get_profile(request.connection_id)
                    .await?
                    .ok_or_else(|| AppError::validation("connection profile not found"))?;
                let dialect = dialect_for_kind(profile.kind);
                let qualified = qualified_table_sql(
                    dialect.as_ref(),
                    profile.kind,
                    request.database.as_deref(),
                    request.schema.as_deref(),
                    &request.table,
                )?;
                let columns = metadata_service
                    .table_columns(
                        request.connection_id,
                        request.database.as_deref(),
                        request.schema.as_deref(),
                        &request.table,
                        None,
                    )
                    .await?;
                let sql = ordered_table_select_sql(dialect.as_ref(), &qualified, &columns)?;
                let headers = columns.iter().map(|column| column.name.clone()).collect();
                let mut writer = QueryResultFileWriter::create(request.format, &path, headers)?;
                let mut offset = 0u64;
                let mut total_rows = None;

                loop {
                    let report = query_service
                        .execute(
                            QueryExecutionRequest {
                                tab_id: QueryId::new(),
                                connection_id: request.connection_id,
                                database: request.database.clone(),
                                schema: request.schema.clone(),
                                sql: sql.clone(),
                                mode: QueryExecutionMode::All,
                                cursor_offset: 0,
                                selection: None,
                                timeout_ms: 300_000,
                                result_limit: Some(EXPORT_PAGE_SIZE),
                                result_offset: offset,
                                result_total_rows: total_rows,
                            },
                            None,
                        )
                        .await?;
                    let page = report
                        .results
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| QueryResult::empty(""));
                    total_rows = total_rows.or_else(|| {
                        page.metadata
                            .get(db_common::QUERY_RESULT_METADATA_TOTAL_ROWS)
                            .and_then(|value| value.parse::<u64>().ok())
                    });
                    let page_rows = writer.write_page(&page)?;
                    offset = offset.saturating_add(page_rows as u64);
                    if page_rows < EXPORT_PAGE_SIZE as usize
                        || total_rows.is_some_and(|total| offset >= total)
                    {
                        break;
                    }
                }

                let row_count = writer.finish()?;
                let qualifier = request
                    .schema
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        request
                            .database
                            .as_deref()
                            .filter(|value| !value.is_empty())
                    });
                Ok(TableExportPayload {
                    table_name: qualified_table_label(qualifier, &request.table),
                    path,
                    format: request.format,
                    row_count,
                })
            }
            .await;

            *cell.lock().unwrap() = Some(result);
        });

        self.state.set_status("Table export started");
    }

    pub(super) fn spawn_database_export(
        &mut self,
        connection_id: db_common::ConnectionId,
        database: Option<String>,
        mode: crate::ui::object_tree::DatabaseExportMode,
        path: PathBuf,
    ) {
        let metadata_service = self.metadata_service.clone();
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();
        let cell: PendingTask<DatabaseExportPayload> = Arc::new(Mutex::new(None));
        self.pending_database_export = Some(cell.clone());

        runtime.spawn(async move {
            let result = export_database_sql(
                metadata_service,
                query_service,
                connection_manager,
                connection_id,
                database,
                mode,
                path,
            )
            .await;
            *cell.lock().unwrap() = Some(result);
        });

        self.state.set_status(format!(
            "{} export started",
            database_export_mode_label(mode)
        ));
    }

    pub(super) fn spawn_index_create(&mut self, request: CreateIndexRequest) {
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();
        let cell: PendingTask<IndexCreatePayload> = Arc::new(Mutex::new(None));
        self.pending_index_create = Some(cell.clone());

        runtime.spawn(async move {
            let result = async {
                let profile = connection_manager
                    .get_profile(request.connection_id)
                    .await?
                    .ok_or_else(|| AppError::validation("connection profile not found"))?;
                let sql = build_create_index_sql(&request, profile.kind)?;
                query_service
                    .execute(
                        QueryExecutionRequest {
                            tab_id: QueryId::new(),
                            connection_id: request.connection_id,
                            database: request.database.clone(),
                            schema: request.schema.clone(),
                            sql,
                            mode: QueryExecutionMode::All,
                            cursor_offset: 0,
                            selection: None,
                            timeout_ms: 60_000,
                            result_limit: None,
                            result_offset: 0,
                            result_total_rows: None,
                        },
                        None,
                    )
                    .await?;
                Ok(IndexCreatePayload {
                    connection_id: request.connection_id,
                    database: request.database,
                    schema: request.schema,
                    table: request.table,
                    name: request.name,
                })
            }
            .await;
            *cell.lock().unwrap() = Some(result);
        });

        self.state.set_status("Creating index...");
    }

    pub(super) fn spawn_table_drop(&mut self, request: DropTableRequest) {
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();
        let cell: PendingTask<DropTablePayload> = Arc::new(Mutex::new(None));
        self.pending_table_drop = Some(cell.clone());

        runtime.spawn(async move {
            let result = async {
                let profile = connection_manager
                    .get_profile(request.connection_id)
                    .await?
                    .ok_or_else(|| AppError::validation("connection profile not found"))?;
                let dialect = dialect_for_kind(profile.kind);
                let table_sql = qualified_table_sql(
                    dialect.as_ref(),
                    profile.kind,
                    request.database.as_deref(),
                    request.schema.as_deref(),
                    &request.table,
                )?;
                query_service
                    .execute(
                        QueryExecutionRequest {
                            tab_id: QueryId::new(),
                            connection_id: request.connection_id,
                            database: request.database.clone(),
                            schema: request.schema.clone(),
                            sql: format!("DROP TABLE {table_sql}"),
                            mode: QueryExecutionMode::All,
                            cursor_offset: 0,
                            selection: None,
                            timeout_ms: 60_000,
                            result_limit: None,
                            result_offset: 0,
                            result_total_rows: None,
                        },
                        None,
                    )
                    .await?;
                Ok(DropTablePayload {
                    database: request.database,
                    schema: request.schema,
                    table: request.table,
                })
            }
            .await;
            *cell.lock().unwrap() = Some(result);
        });

        self.state.set_status("Dropping table...");
    }

    pub(super) fn spawn_clear(&mut self, request: ClearRequest) {
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let redis_service = self.redis_service.clone();
        let runtime = self.runtime.clone();
        let cell: PendingTask<ClearPayload> = Arc::new(Mutex::new(None));
        self.pending_clear = Some(cell.clone());

        runtime.spawn(async move {
            let result = async {
                match &request {
                    ClearRequest::Table {
                        connection_id,
                        database,
                        schema,
                        table,
                    } => {
                        let profile = connection_manager
                            .get_profile(*connection_id)
                            .await?
                            .ok_or_else(|| AppError::validation("connection profile not found"))?;
                        let dialect = dialect_for_kind(profile.kind);
                        let table_sql = qualified_table_sql(
                            dialect.as_ref(),
                            profile.kind,
                            database.as_deref(),
                            schema.as_deref(),
                            table,
                        )?;
                        let sql = clear_table_sql(profile.kind, &table_sql);
                        query_service
                            .execute(
                                QueryExecutionRequest {
                                    tab_id: QueryId::new(),
                                    connection_id: *connection_id,
                                    database: database.clone(),
                                    schema: schema.clone(),
                                    sql,
                                    mode: QueryExecutionMode::All,
                                    cursor_offset: 0,
                                    selection: None,
                                    timeout_ms: 300_000,
                                    result_limit: None,
                                    result_offset: 0,
                                    result_total_rows: None,
                                },
                                None,
                            )
                            .await?;
                    }
                    ClearRequest::RedisDatabase {
                        connection_id,
                        database,
                    } => {
                        redis_service
                            .execute_command(*connection_id, *database, "FLUSHDB")
                            .await?;
                    }
                }
                Ok(ClearPayload { request })
            }
            .await;
            *cell.lock().unwrap() = Some(result);
        });

        self.state.set_status("Clearing data...");
    }

    /// Spawn an async task to execute an ALTER TABLE request.
    pub(super) fn spawn_alter_table(&mut self, request: AlterTableRequest) {
        let query_service = self.query_service.clone();
        let connection_manager = self.connection_manager.clone();
        let runtime = self.runtime.clone();
        let cell: PendingTask<AlterTablePayload> = Arc::new(Mutex::new(None));
        self.pending_alter_table = Some(cell.clone());

        let connection_id = request.connection_id();
        let database = request.database().map(|s| s.to_string());
        let schema = request.schema().map(|s| s.to_string());
        let table = request.table().to_string();
        let label = match &request {
            AlterTableRequest::AddColumn { column_name, .. } => {
                format!("add column {column_name} on {table}")
            }
            AlterTableRequest::DropColumn { column_name, .. } => {
                format!("drop column {column_name} on {table}")
            }
            AlterTableRequest::ModifyColumn { column_name, .. } => {
                format!("modify column {column_name} on {table}")
            }
            AlterTableRequest::DropIndex { index_name, .. } => {
                format!("drop index {index_name} on {table}")
            }
            AlterTableRequest::AddForeignKey {
                constraint_name, ..
            } => {
                format!("add foreign key {constraint_name} on {table}")
            }
        };

        runtime.spawn(async move {
            let result = async {
                let profile = connection_manager
                    .get_profile(connection_id)
                    .await?
                    .ok_or_else(|| AppError::validation("connection profile not found"))?;
                let sql = build_alter_table_sql(&request, profile.kind)?;
                query_service
                    .execute(
                        QueryExecutionRequest {
                            tab_id: QueryId::new(),
                            connection_id,
                            database: database.clone(),
                            schema: schema.clone(),
                            sql,
                            mode: QueryExecutionMode::All,
                            cursor_offset: 0,
                            selection: None,
                            timeout_ms: 60_000,
                            result_limit: None,
                            result_offset: 0,
                            result_total_rows: None,
                        },
                        None,
                    )
                    .await?;
                Ok(AlterTablePayload {
                    connection_id,
                    database,
                    schema,
                    table,
                    label,
                })
            }
            .await;
            *cell.lock().unwrap() = Some(result);
        });

        self.state.set_status("Altering table...");
    }
}

fn clear_table_sql(kind: DataSourceKind, table_sql: &str) -> String {
    match kind {
        DataSourceKind::Sqlite => format!("DELETE FROM {table_sql}"),
        DataSourceKind::MySql | DataSourceKind::PostgreSql | DataSourceKind::DuckDb => {
            format!("TRUNCATE TABLE {table_sql}")
        }
        DataSourceKind::Redis | DataSourceKind::MongoDb => {
            unreachable!("non-relational databases are cleared separately")
        }
        _ => format!("TRUNCATE TABLE {table_sql}"),
    }
}

fn ordered_table_select_sql(
    dialect: &dyn Dialect,
    table_sql: &str,
    columns: &[db_common::TableColumn],
) -> Result<String, AppError> {
    let time_columns = columns
        .iter()
        .filter(|column| {
            let ty = column.database_type.to_ascii_lowercase();
            ty.contains("date") || ty.contains("time") || ty.contains("timestamp")
        })
        .map(|column| {
            db_common::Identifier::new(&column.name)
                .map(|identifier| dialect.quote_identifier(&identifier))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let primary_keys = columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| {
            db_common::Identifier::new(&column.name)
                .map(|identifier| dialect.quote_identifier(&identifier))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let order = if !time_columns.is_empty() {
        format!(" ORDER BY {}", time_columns.join(", "))
    } else if !primary_keys.is_empty() {
        format!(" ORDER BY {}", primary_keys.join(", "))
    } else if let Some(first_column) = columns.first() {
        // Doris requires ORDER BY whenever OFFSET is used. A first-column
        // fallback keeps pagination valid for tables without a primary key.
        format!(
            " ORDER BY {}",
            dialect.quote_identifier(&db_common::Identifier::new(&first_column.name)?)
        )
    } else {
        String::new()
    };
    Ok(format!("SELECT * FROM {table_sql}{order}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::dialect::DoubleQuoteDialect;

    #[test]
    fn builds_clear_sql_for_relational_engines() {
        assert_eq!(
            clear_table_sql(DataSourceKind::Sqlite, "\"users\""),
            "DELETE FROM \"users\""
        );
        assert_eq!(
            clear_table_sql(DataSourceKind::DuckDb, "\"main\".\"users\""),
            "TRUNCATE TABLE \"main\".\"users\""
        );
        assert_eq!(
            clear_table_sql(DataSourceKind::MySql, "`app`.`users`"),
            "TRUNCATE TABLE `app`.`users`"
        );
    }

    #[test]
    fn orders_table_reads_by_primary_key() {
        let columns = vec![db_common::TableColumn {
            name: "id".to_string(),
            database_type: "INTEGER".to_string(),
            nullable: false,
            default_value: None,
            primary_key: true,
            comment: None,
        }];
        assert_eq!(
            ordered_table_select_sql(&DoubleQuoteDialect, "\"main\".\"users\"", &columns).unwrap(),
            "SELECT * FROM \"main\".\"users\" ORDER BY \"id\""
        );
    }
}
