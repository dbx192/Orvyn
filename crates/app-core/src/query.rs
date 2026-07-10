//! SQL query editor orchestration.
//!
//! This service keeps editor state out of concrete database drivers: tabs,
//! execution selection, cancellation, history, favorites, and pool lifecycle.

use crate::connection::ConnectionManager;
use crate::storage::{FavoriteQueryStore, QueryHistoryStore};
use crate::task::TaskManager;
use db_common::{
    count_result_page_statement, highlight_sql, page_result_statement, split_statements,
    statement_at_offset, statement_summary, unsupported_kind, AppError, CellValue, ConnectionId,
    ConnectionPoolConfig, ConnectionPoolStats, DataSourceKind, ErrorKind, FavoriteQuery, PageSize,
    QueryExecutionMode, QueryExecutionReport, QueryExecutionRequest, QueryHistoryEntry, QueryId,
    QueryResult, QueryTab, RelationalQueryExecutor, Result, SqlHighlightToken, TextRange,
    QUERY_RESULT_METADATA_PAGE_LIMIT, QUERY_RESULT_METADATA_PAGE_OFFSET,
    QUERY_RESULT_METADATA_TOTAL_ROWS,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Instant,
};
use tokio::{
    runtime::{Handle, Runtime},
    sync::Mutex,
    task::AbortHandle,
};
use tracing::{info, instrument};

pub const DEFAULT_HISTORY_LIMIT: usize = 500;

#[derive(Clone)]
pub struct QueryService {
    connections: ConnectionManager,
    executors: Arc<BTreeMap<DataSourceKind, Arc<dyn RelationalQueryExecutor>>>,
    pool_config: ConnectionPoolConfig,
    _task_manager: TaskManager,
    history_store: Option<Arc<dyn QueryHistoryStore>>,
    favorite_store: Option<Arc<dyn FavoriteQueryStore>>,
    state: Arc<Mutex<QueryServiceState>>,
}

impl QueryService {
    pub fn new(
        connections: ConnectionManager,
        executors: Vec<Arc<dyn RelationalQueryExecutor>>,
        pool_config: ConnectionPoolConfig,
        task_manager: TaskManager,
        history_store: Option<Arc<dyn QueryHistoryStore>>,
        favorite_store: Option<Arc<dyn FavoriteQueryStore>>,
    ) -> Self {
        let service = Self {
            connections,
            executors: Arc::new(
                executors
                    .into_iter()
                    .map(|executor| (executor.kind(), executor))
                    .collect(),
            ),
            pool_config,
            _task_manager: task_manager,
            history_store,
            favorite_store,
            state: Arc::new(Mutex::new(QueryServiceState::default())),
        };
        service.load_history_and_favorites().unwrap();
        service
    }

    pub fn with_default_executors(connections: ConnectionManager) -> Self {
        Self::new(
            connections,
            vec![
                Arc::new(db_mysql::MySqlQueryExecutor::new()),
                Arc::new(db_postgres::PostgresQueryExecutor::new()),
                Arc::new(db_sqlite::SqliteQueryExecutor::new()),
                Arc::new(db_duckdb::DuckDbQueryExecutor::new()),
            ],
            ConnectionPoolConfig::default(),
            TaskManager::default(),
            None,
            None,
        )
    }

    pub fn with_task_manager(
        connections: ConnectionManager,
        executors: Vec<Arc<dyn RelationalQueryExecutor>>,
        pool_config: ConnectionPoolConfig,
        task_manager: TaskManager,
    ) -> Self {
        Self::new(
            connections,
            executors,
            pool_config,
            task_manager,
            None,
            None,
        )
    }

    pub fn with_stores(
        connections: ConnectionManager,
        executors: Vec<Arc<dyn RelationalQueryExecutor>>,
        pool_config: ConnectionPoolConfig,
        task_manager: TaskManager,
        history_store: Arc<dyn QueryHistoryStore>,
        favorite_store: Arc<dyn FavoriteQueryStore>,
    ) -> Self {
        Self::new(
            connections,
            executors,
            pool_config,
            task_manager,
            Some(history_store),
            Some(favorite_store),
        )
    }

    fn load_history_and_favorites(&self) -> Result<()> {
        let history_store = self.history_store.clone();
        let favorite_store = self.favorite_store.clone();
        let state = self.state.clone();

        run_blocking_on_runtime(async move {
            if let Some(store) = history_store {
                match store.list().await {
                    Ok(entries) => {
                        let mut state = state.lock().await;
                        state.history.extend(entries);
                    }
                    Err(e) => {
                        tracing::warn!("failed to load query history: {}", e.message);
                    }
                }
            }

            if let Some(store) = favorite_store {
                match store.list().await {
                    Ok(favorites) => {
                        let mut state = state.lock().await;
                        for fav in favorites {
                            state.favorites.insert(fav.id, fav);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("failed to load favorites: {}", e.message);
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn open_tab(
        &self,
        connection_id: ConnectionId,
        title: Option<String>,
    ) -> Result<QueryTab> {
        let profile = self
            .connections
            .get_profile(connection_id)
            .await?
            .ok_or_else(|| AppError::validation("connection profile not found"))?;
        if !profile.kind.is_relational() {
            return Err(unsupported_kind(profile.kind));
        }

        let title = title.unwrap_or_else(|| format!("Query {}", profile.name));
        let tab = QueryTab::new(connection_id, title);
        self.state.lock().await.tabs.insert(tab.id, tab.clone());
        Ok(tab)
    }

    pub async fn update_tab_sql(
        &self,
        tab_id: QueryId,
        sql: String,
        cursor_offset: usize,
        selection: Option<TextRange>,
    ) -> Result<QueryTab> {
        let mut state = self.state.lock().await;
        let tab = state
            .tabs
            .get_mut(&tab_id)
            .ok_or_else(|| AppError::validation("query tab not found"))?;
        tab.sql = sql;
        tab.cursor_offset = cursor_offset;
        tab.selection = selection;
        tab.dirty = true;
        Ok(tab.clone())
    }

    pub async fn current_statement(&self, tab_id: QueryId) -> Result<Option<String>> {
        let state = self.state.lock().await;
        let tab = state
            .tabs
            .get(&tab_id)
            .ok_or_else(|| AppError::validation("query tab not found"))?;
        Ok(statement_at_offset(&tab.sql, tab.cursor_offset))
    }

    pub async fn highlight_tab_sql(&self, tab_id: QueryId) -> Result<Vec<SqlHighlightToken>> {
        let state = self.state.lock().await;
        let tab = state
            .tabs
            .get(&tab_id)
            .ok_or_else(|| AppError::validation("query tab not found"))?;
        Ok(highlight_sql(&tab.sql))
    }

    pub async fn close_tab(&self, tab_id: QueryId) -> Result<()> {
        let mut state = self.state.lock().await;
        state.tabs.remove(&tab_id);
        if let Some(handle) = state.running.remove(&tab_id) {
            handle.abort();
        }
        Ok(())
    }

    pub async fn tabs(&self) -> Vec<QueryTab> {
        self.state.lock().await.tabs.values().cloned().collect()
    }

    pub async fn open_history_entry(&self, history_id: QueryId) -> Result<QueryTab> {
        let entry = {
            let state = self.state.lock().await;
            state
                .history
                .iter()
                .find(|entry| entry.id == history_id)
                .cloned()
                .ok_or_else(|| AppError::validation("query history entry not found"))?
        };
        let tab = self
            .open_tab(entry.connection_id, Some(entry.summary.clone()))
            .await?;
        self.update_tab_sql(tab.id, entry.sql, 0, None).await
    }

    pub async fn open_favorite(&self, favorite_id: QueryId) -> Result<QueryTab> {
        let favorite = {
            let state = self.state.lock().await;
            state
                .favorites
                .get(&favorite_id)
                .cloned()
                .ok_or_else(|| AppError::validation("favorite query not found"))?
        };
        let connection_id = favorite
            .connection_id
            .ok_or_else(|| AppError::validation("favorite query has no connection"))?;
        let tab = self.open_tab(connection_id, Some(favorite.title)).await?;
        self.update_tab_sql(tab.id, favorite.sql, 0, None).await
    }

    #[instrument(skip(self, request, runtime_password), fields(tab_id = %request.tab_id))]
    pub async fn execute(
        &self,
        request: QueryExecutionRequest,
        runtime_password: Option<&str>,
    ) -> Result<QueryExecutionReport> {
        let statements = statements_for_request(&request)?;
        let execution_plan =
            execution_plan_for(
                &statements,
                request.result_limit,
                request.result_offset,
                request.result_total_rows,
            )?;
        let (mut profile, password) = self
            .connections
            .profile_with_password(request.connection_id, runtime_password)
            .await?;
        if let Some(database) = request
            .database
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            profile.database = Some(database.to_string());
        }
        let executor = self.executor(profile.kind)?;
        let execution_id = QueryId::new();
        let started_at = Instant::now();
        let record_history = request.result_offset == 0;
        let request_for_task = request.clone();
        let statements_for_task = execution_plan.execution_statements.clone();
        let pool_config = self.pool_config;
        let password_for_task = password;

        let handle = tokio::spawn(async move {
            tokio::time::timeout(
                request_for_task.timeout(),
                executor.execute(
                    &profile,
                    password_for_task.as_deref(),
                    &statements_for_task,
                    pool_config,
                ),
            )
            .await
            .map_err(|_| AppError::new(ErrorKind::Timeout, "query execution timed out"))?
        });
        let abort_handle = handle.abort_handle();

        self.state
            .lock()
            .await
            .running
            .insert(request.tab_id, abort_handle);

        let result = handle.await.map_err(|error| {
            if error.is_cancelled() {
                AppError::new(ErrorKind::Cancelled, "query execution cancelled")
            } else {
                AppError::new(ErrorKind::Internal, "query task failed")
                    .with_detail(error.to_string())
            }
        });

        let elapsed_ms = started_at.elapsed().as_millis();
        match result {
            Ok(Ok(results)) => {
                self.state.lock().await.running.remove(&request.tab_id);
                let results = execution_plan.into_report_results(results)?;
                let report = QueryExecutionReport::new(
                    execution_id,
                    &request,
                    statements,
                    results,
                    elapsed_ms,
                );
                if record_history {
                    self.record_history(&request, &report, None).await;
                }
                info!(execution_id = %execution_id, elapsed_ms, "query execution finished");
                Ok(report)
            }
            Ok(Err(error)) | Err(error) => {
                self.state.lock().await.running.remove(&request.tab_id);
                if record_history {
                    self.record_failed_history(&request, &statements, elapsed_ms, &error)
                        .await;
                }
                Err(error)
            }
        }
    }

    pub async fn execute_tab(
        &self,
        tab_id: QueryId,
        mode: QueryExecutionMode,
        timeout_ms: u64,
        runtime_password: Option<&str>,
    ) -> Result<QueryExecutionReport> {
        let tab = self
            .state
            .lock()
            .await
            .tabs
            .get(&tab_id)
            .cloned()
            .ok_or_else(|| AppError::validation("query tab not found"))?;
        let request = QueryExecutionRequest {
            tab_id: tab.id,
            connection_id: tab.connection_id,
            database: None,
            schema: None,
            sql: tab.sql,
            mode,
            cursor_offset: tab.cursor_offset,
            selection: tab.selection,
            timeout_ms,
            result_limit: None,
            result_offset: 0,
            result_total_rows: None,
        };
        self.execute(request, runtime_password).await
    }

    pub async fn execute_current_statement(
        &self,
        tab_id: QueryId,
        timeout_ms: u64,
        runtime_password: Option<&str>,
    ) -> Result<QueryExecutionReport> {
        self.execute_tab(
            tab_id,
            QueryExecutionMode::CurrentStatement,
            timeout_ms,
            runtime_password,
        )
        .await
    }

    pub async fn execute_selection(
        &self,
        tab_id: QueryId,
        timeout_ms: u64,
        runtime_password: Option<&str>,
    ) -> Result<QueryExecutionReport> {
        self.execute_tab(
            tab_id,
            QueryExecutionMode::Selection,
            timeout_ms,
            runtime_password,
        )
        .await
    }

    pub async fn stop_execution(&self, tab_id: QueryId) -> Result<()> {
        let handle = self.state.lock().await.running.remove(&tab_id);
        if let Some(handle) = handle {
            handle.abort();
            return Ok(());
        }

        Err(AppError::validation("query tab has no running execution"))
    }

    pub async fn history(&self) -> Vec<QueryHistoryEntry> {
        self.state.lock().await.history.iter().cloned().collect()
    }

    pub async fn clear_history(&self) -> Result<()> {
        self.state.lock().await.history.clear();
        if let Some(store) = &self.history_store {
            store.clear().await?;
        }
        Ok(())
    }

    pub async fn add_favorite(
        &self,
        connection_id: Option<ConnectionId>,
        database: Option<String>,
        schema: Option<String>,
        title: String,
        sql: String,
    ) -> Result<FavoriteQuery> {
        let now = db_common::now_utc();
        let favorite = FavoriteQuery {
            id: QueryId::new(),
            connection_id,
            database,
            schema,
            title,
            sql,
            created_at: now,
            updated_at: now,
        };
        self.state
            .lock()
            .await
            .favorites
            .insert(favorite.id, favorite.clone());
        if let Some(store) = &self.favorite_store {
            store.upsert(favorite.clone()).await?;
        }
        Ok(favorite)
    }

    pub async fn update_favorite(
        &self,
        id: QueryId,
        connection_id: Option<ConnectionId>,
        database: Option<String>,
        schema: Option<String>,
        title: String,
        sql: String,
    ) -> Result<FavoriteQuery> {
        let mut state = self.state.lock().await;
        let favorite = state
            .favorites
            .get_mut(&id)
            .ok_or_else(|| AppError::validation("favorite query not found"))?;
        favorite.connection_id = connection_id;
        favorite.database = database;
        favorite.schema = schema;
        favorite.title = title;
        favorite.sql = sql;
        favorite.updated_at = db_common::now_utc();
        let updated = favorite.clone();
        if let Some(store) = &self.favorite_store {
            store.upsert(updated.clone()).await?;
        }
        Ok(updated)
    }

    pub async fn remove_favorite(&self, id: QueryId) -> Result<()> {
        self.state.lock().await.favorites.remove(&id);
        if let Some(store) = &self.favorite_store {
            store.delete(id).await?;
        }
        Ok(())
    }

    pub async fn favorites(&self) -> Vec<FavoriteQuery> {
        self.state
            .lock()
            .await
            .favorites
            .values()
            .cloned()
            .collect()
    }

    pub async fn pool_stats(
        &self,
        connection_id: ConnectionId,
    ) -> Result<Option<ConnectionPoolStats>> {
        let profile = self
            .connections
            .get_profile(connection_id)
            .await?
            .ok_or_else(|| AppError::validation("connection profile not found"))?;
        self.executor(profile.kind)?.pool_stats(connection_id).await
    }

    pub async fn close_pool(&self, connection_id: ConnectionId) -> Result<()> {
        let profile = self
            .connections
            .get_profile(connection_id)
            .await?
            .ok_or_else(|| AppError::validation("connection profile not found"))?;
        self.executor(profile.kind)?.close_pool(connection_id).await
    }

    fn executor(&self, kind: DataSourceKind) -> Result<Arc<dyn RelationalQueryExecutor>> {
        self.executors
            .get(&kind)
            .cloned()
            .ok_or_else(|| unsupported_kind(kind))
    }

    async fn record_history(
        &self,
        request: &QueryExecutionRequest,
        report: &QueryExecutionReport,
        error_message: Option<String>,
    ) {
        let sql = report.statements.join(";\n");
        let entry = QueryHistoryEntry {
            id: report.execution_id,
            connection_id: request.connection_id,
            summary: statement_summary(&sql, 120),
            sql,
            executed_at: db_common::now_utc(),
            elapsed_ms: report.elapsed_ms,
            affected_rows: report.affected_rows,
            success: error_message.is_none(),
            error_message,
        };
        push_history(&mut self.state.lock().await.history, entry.clone());

        if let Some(store) = &self.history_store {
            if let Err(e) = store.add(entry).await {
                tracing::warn!("failed to write history entry: {}", e.message);
            }
        }
    }

    async fn record_failed_history(
        &self,
        request: &QueryExecutionRequest,
        statements: &[String],
        elapsed_ms: u128,
        error: &AppError,
    ) {
        let sql = statements.join(";\n");
        let entry = QueryHistoryEntry {
            id: QueryId::new(),
            connection_id: request.connection_id,
            summary: statement_summary(&sql, 120),
            sql,
            executed_at: db_common::now_utc(),
            elapsed_ms,
            affected_rows: 0,
            success: false,
            error_message: Some(error.message.clone()),
        };
        push_history(&mut self.state.lock().await.history, entry.clone());

        if let Some(store) = &self.history_store {
            if let Err(e) = store.add(entry).await {
                tracing::warn!("failed to write history entry: {}", e.message);
            }
        }
    }
}

fn run_blocking_on_runtime<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match Handle::try_current() {
        Ok(handle) => handle.block_on(future),
        Err(_) => Runtime::new()
            .expect("failed to create runtime for query service initialization")
            .block_on(future),
    }
}

#[derive(Default)]
struct QueryServiceState {
    tabs: BTreeMap<QueryId, QueryTab>,
    running: BTreeMap<QueryId, AbortHandle>,
    history: VecDeque<QueryHistoryEntry>,
    favorites: BTreeMap<QueryId, FavoriteQuery>,
}

fn statements_for_request(request: &QueryExecutionRequest) -> Result<Vec<String>> {
    let sql = match request.mode {
        QueryExecutionMode::All => request.sql.clone(),
        QueryExecutionMode::Selection => {
            let Some(range) = request.selection.filter(|range| !range.is_empty()) else {
                return Err(AppError::validation("SQL selection is empty"));
            };
            request
                .sql
                .get(range.start..range.end)
                .ok_or_else(|| AppError::validation("SQL selection is not on character boundary"))?
                .to_string()
        }
        QueryExecutionMode::CurrentStatement => {
            statement_at_offset(&request.sql, request.cursor_offset)
                .ok_or_else(|| AppError::validation("no SQL statement at cursor"))?
        }
    };

    let statements = split_statements(&sql);
    if statements.is_empty() {
        return Err(AppError::validation("SQL is empty"));
    }
    Ok(statements)
}

struct ExecutionPlan {
    execution_statements: Vec<String>,
    result_plans: Vec<ResultPlan>,
}

enum ResultPlan {
    Direct {
        statement: String,
    },
    Paged {
        statement: String,
        limit: u32,
        offset: u64,
        total_rows: Option<u64>,
    },
}

impl ExecutionPlan {
    fn into_report_results(self, execution_results: Vec<QueryResult>) -> Result<Vec<QueryResult>> {
        let mut results = Vec::with_capacity(self.result_plans.len());
        let mut execution_results = execution_results.into_iter();

        for plan in self.result_plans {
            match plan {
                ResultPlan::Direct { statement } => {
                    let mut result = execution_results.next().ok_or_else(|| {
                        AppError::new(ErrorKind::Internal, "query result count mismatch")
                    })?;
                    result.statement = statement;
                    results.push(result);
                }
                ResultPlan::Paged {
                    statement,
                    limit,
                    offset,
                    total_rows,
                } => {
                    let (total_rows, count_execution_time_ms) = if let Some(total_rows) = total_rows
                    {
                        (total_rows, 0)
                    } else {
                        let count_result = execution_results.next().ok_or_else(|| {
                            AppError::new(ErrorKind::Internal, "query count result missing")
                        })?;
                        let total_rows = query_count_from_result(&count_result)?;
                        (total_rows, count_result.execution_time_ms)
                    };
                    let mut page_result = execution_results.next().ok_or_else(|| {
                        AppError::new(ErrorKind::Internal, "query page result missing")
                    })?;
                    page_result.statement = statement;
                    page_result.execution_time_ms += count_execution_time_ms;
                    page_result.metadata.insert(
                        QUERY_RESULT_METADATA_PAGE_LIMIT.to_string(),
                        limit.to_string(),
                    );
                    page_result.metadata.insert(
                        QUERY_RESULT_METADATA_PAGE_OFFSET.to_string(),
                        offset.to_string(),
                    );
                    page_result.metadata.insert(
                        QUERY_RESULT_METADATA_TOTAL_ROWS.to_string(),
                        total_rows.to_string(),
                    );
                    page_result.notices.push(page_notice(
                        offset,
                        page_result.rows.len() as u64,
                        total_rows,
                    ));
                    results.push(page_result);
                }
            }
        }

        if execution_results.next().is_some() {
            return Err(AppError::new(
                ErrorKind::Internal,
                "query returned more result sets than expected",
            ));
        }

        Ok(results)
    }
}

fn execution_plan_for(
    statements: &[String],
    result_limit: Option<u32>,
    result_offset: u64,
    result_total_rows: Option<u64>,
) -> Result<ExecutionPlan> {
    let mut execution_statements = Vec::with_capacity(statements.len());
    let mut result_plans = Vec::with_capacity(statements.len());

    for statement in statements {
        let Some(limit) = result_limit else {
            execution_statements.push(statement.clone());
            result_plans.push(ResultPlan::Direct {
                statement: statement.clone(),
            });
            continue;
        };

        let limit = PageSize::new(limit)?.get();
        let Some(page_statement) = page_result_statement(statement, limit, result_offset) else {
            execution_statements.push(statement.clone());
            result_plans.push(ResultPlan::Direct {
                statement: statement.clone(),
            });
            continue;
        };

        let total_rows = if result_offset == 0 {
            None
        } else {
            result_total_rows
        };
        if total_rows.is_none() {
            let Some(count_statement) = count_result_page_statement(statement) else {
                execution_statements.push(statement.clone());
                result_plans.push(ResultPlan::Direct {
                    statement: statement.clone(),
                });
                continue;
            };
            execution_statements.push(count_statement);
        }
        execution_statements.push(page_statement);
        result_plans.push(ResultPlan::Paged {
            statement: statement.clone(),
            limit,
            offset: result_offset,
            total_rows,
        });
    }

    Ok(ExecutionPlan {
        execution_statements,
        result_plans,
    })
}

fn query_count_from_result(result: &QueryResult) -> Result<u64> {
    let value = result
        .rows
        .first()
        .and_then(|row| row.values.first())
        .ok_or_else(|| AppError::new(ErrorKind::Query, "failed to read query row count"))?;

    match value {
        CellValue::UInt(value) => Ok(*value),
        CellValue::Int(value) if *value >= 0 => Ok(*value as u64),
        CellValue::Text(value) | CellValue::Decimal(value) => {
            value.parse::<u64>().map_err(|error| {
                AppError::new(ErrorKind::Query, "failed to parse query row count")
                    .with_detail(error.to_string())
            })
        }
        _ => Err(AppError::new(
            ErrorKind::Query,
            "query row count had an unsupported type",
        )),
    }
}

fn page_notice(offset: u64, row_count: u64, total_rows: u64) -> String {
    if row_count == 0 || total_rows == 0 {
        return format!("Showing 0 rows of {total_rows}.");
    }

    format!(
        "Showing rows {}-{} of {total_rows}.",
        offset + 1,
        offset + row_count
    )
}

fn push_history(history: &mut VecDeque<QueryHistoryEntry>, entry: QueryHistoryEntry) {
    history.push_front(entry);
    while history.len() > DEFAULT_HISTORY_LIMIT {
        history.pop_back();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::QueryExecutionRequest;

    #[test]
    fn selects_current_statement() {
        let connection_id = ConnectionId::new();
        let request = QueryExecutionRequest {
            tab_id: QueryId::new(),
            connection_id,
            database: None,
            schema: None,
            sql: "select 1;\nselect 2;".to_string(),
            mode: QueryExecutionMode::CurrentStatement,
            cursor_offset: 12,
            selection: None,
            timeout_ms: 1000,
            result_limit: None,
            result_offset: 0,
            result_total_rows: None,
        };

        assert_eq!(statements_for_request(&request).unwrap(), vec!["select 2"]);
    }

    #[test]
    fn selects_highlighted_sql() {
        let connection_id = ConnectionId::new();
        let request = QueryExecutionRequest {
            tab_id: QueryId::new(),
            connection_id,
            database: None,
            schema: None,
            sql: "select 1;\nselect 2;".to_string(),
            mode: QueryExecutionMode::Selection,
            cursor_offset: 0,
            selection: Some(TextRange::new(10, 18)),
            timeout_ms: 1000,
            result_limit: None,
            result_offset: 0,
            result_total_rows: None,
        };

        assert_eq!(statements_for_request(&request).unwrap(), vec!["select 2"]);
    }

    #[test]
    fn builds_count_and_page_execution_plan_for_selects() {
        let statements = vec!["select * from books".to_string()];

        let plan = execution_plan_for(&statements, Some(500), 1_000, None).unwrap();

        assert_eq!(
            plan.execution_statements,
            vec![
                "SELECT COUNT(*) AS total_rows FROM (select * from books) AS __query_count",
                "SELECT * FROM (select * from books) AS __query_page LIMIT 500 OFFSET 1000"
            ]
        );
    }

    #[test]
    fn direct_execution_plan_when_query_is_not_pageable() {
        let statements = vec!["select * from books limit 25".to_string()];

        let plan = execution_plan_for(&statements, None, 0, None).unwrap();

        assert_eq!(plan.execution_statements, statements);
    }
}
