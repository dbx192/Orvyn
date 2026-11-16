//! Minimal search / document / HTTP-SQL query panel.
//!
//! A single simple browser for the Phase-5 connectors:
//! - search-service connections (Elasticsearch/Easysearch/Meilisearch/Manticore)
//! - document connections (MongoDB)
//! - HTTP-SQL connections (Cloudflare D1)
//!
//! The user types a JSON DSL (or, for D1, a SQL statement), presses Run, and
//! the pretty-serialized result is shown in a read-only code editor inside a
//! scroll area. Following [`super::redis_browser::RedisBrowserUi`], the async
//! call is dispatched on a `tokio` handle and polled each frame.

use crate::ui::i18n::text as tr;
use app_core::{query::QueryService, search_service::SearchService};
use db_common::{ConnectionId, DataSourceKind, FavoriteQuery, QueryHistoryEntry};
use eframe::egui::{Button, Color32, Context, Key, RichText, ScrollArea, Spinner, TextEdit, Ui};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::runtime::Handle;

const SEARCH_LIMIT: u32 = 100;

/// Shared async slot for streaming query output.
#[derive(Default)]
struct QueryPage {
    text: String,
    pit: Option<String>,
    next_cursor: Option<serde_json::Value>,
    count: usize,
    notice: Option<String>,
}
type AsyncCell<T> = Arc<Mutex<Option<T>>>;
type ResultCell<T> = AsyncCell<Result<T, String>>;
type SharedQueryCell = ResultCell<QueryPage>;

pub struct SearchBrowser {
    search_service: SearchService,
    query_service: QueryService,
    runtime: Handle,
    connection_id: ConnectionId,
    kind: DataSourceKind,
    database: Option<String>,
    object: String,
    query_text: String,
    running: bool,
    result_text: String,
    error: Option<String>,
    notice: Option<String>,
    cell: Option<SharedQueryCell>,
    save_cell: Option<ResultCell<FavoriteQuery>>,
    history_cell: Option<ResultCell<()>>,
    saved_queries: Vec<FavoriteQuery>,
    history_entries: Vec<QueryHistoryEntry>,
    load_cell: Option<AsyncCell<(Vec<QueryHistoryEntry>, Vec<FavoriteQuery>)>>,
    history_search: String,
    saved_search: String,
    show_saved: bool,
    page_size: u32,
    offset: u64,
    has_more: bool,
    executed_query: String,
    pit: Option<String>,
    cursors: std::collections::BTreeMap<u64, serde_json::Value>,
    task: Option<tokio::task::AbortHandle>,
    mapping_cell: ResultCell<Vec<(String, String)>>,
    mapping_fields: Vec<(String, String)>,
    mapping_pending: bool,
    export_cell: Option<ResultCell<()>>,
}

impl SearchBrowser {
    pub fn new(
        search_service: SearchService,
        query_service: QueryService,
        runtime: Handle,
        connection_id: ConnectionId,
        kind: DataSourceKind,
        database: Option<String>,
        object: String,
    ) -> Self {
        let load_cell = Arc::new(Mutex::new(None));
        let load_cell_for_task = load_cell.clone();
        let load_service = query_service.clone();
        let load_connection = connection_id;
        runtime.spawn(async move {
            let history = load_service.history().await;
            let favorites = load_service
                .favorites()
                .await
                .into_iter()
                .filter(|entry| entry.connection_id == Some(load_connection))
                .collect();
            *load_cell_for_task.lock().unwrap() = Some((history, favorites));
        });
        let mapping_cell = Arc::new(Mutex::new(None));
        if matches!(
            kind,
            DataSourceKind::Elasticsearch | DataSourceKind::Easysearch
        ) {
            let mapping = mapping_cell.clone();
            let service = search_service.clone();
            let index = object.clone();
            runtime.spawn(async move {
                *mapping.lock().unwrap() = Some(
                    service
                        .get_mapping(connection_id, None, &index)
                        .await
                        .map(|value| crate::ui::query_input::mapping_fields(&value))
                        .map_err(|error| error.diagnostic_message()),
                );
            });
        }
        Self {
            search_service,
            query_service,
            runtime,
            connection_id,
            kind,
            database,
            object,
            query_text: default_query(kind),
            running: false,
            result_text: String::new(),
            error: None,
            notice: None,
            cell: None,
            save_cell: None,
            history_cell: None,
            saved_queries: Vec::new(),
            history_entries: Vec::new(),
            load_cell: Some(load_cell),
            history_search: String::new(),
            saved_search: String::new(),
            show_saved: false,
            page_size: SEARCH_LIMIT,
            offset: 0,
            has_more: false,
            executed_query: String::new(),
            pit: None,
            cursors: Default::default(),
            task: None,
            mapping_cell,
            mapping_fields: Vec::new(),
            mapping_pending: matches!(
                kind,
                DataSourceKind::Elasticsearch | DataSourceKind::Easysearch
            ),
            export_cell: None,
        }
    }

    pub fn title(&self) -> String {
        format!("{} · {}", tr("Browse"), self.object)
    }

    pub fn is_busy(&self) -> bool {
        self.running
    }

    fn run(&mut self) {
        if self.running {
            return;
        }
        if self.executed_query != self.query_text {
            self.reset_pages();
        }
        self.executed_query = self.query_text.clone();
        self.result_text.clear();
        self.error = None;
        self.running = true;

        let service = self.search_service.clone();
        let runtime = self.runtime.clone();
        let connection_id = self.connection_id;
        let kind = self.kind;
        let database = self.database.clone();
        let object = self.object.clone();
        let query_text = self.query_text.clone();
        let history = self.query_service.clone();
        let limit = self.page_size;
        let offset = self.offset;
        let pit = self.pit.clone();
        let cursor = self.cursors.get(&offset).cloned();
        let started_at = std::time::Instant::now();

        let cell: SharedQueryCell = Arc::new(Mutex::new(None));
        self.cell = Some(cell.clone());
        let history_cell = Arc::new(Mutex::new(None));
        self.history_cell = Some(history_cell.clone());

        let task = runtime.spawn(async move {
            let result = run_query(
                service,
                kind,
                connection_id,
                database.as_deref(),
                &object,
                &query_text,
                limit,
                offset,
                pit,
                cursor,
            )
            .await;
            let history_result = history
                .record_external_history(
                    connection_id,
                    database,
                    object,
                    query_text,
                    started_at.elapsed().as_millis(),
                    result.as_ref().err().cloned(),
                )
                .await
                .map(|_| ())
                .map_err(|error| error.diagnostic_message());
            *history_cell.lock().unwrap() = Some(history_result);
            *cell.lock().unwrap() = Some(result);
        });
        self.task = Some(task.abort_handle());
    }

    fn reset_pages(&mut self) {
        self.release_pit();
        self.offset = 0;
        self.has_more = false;
        self.cursors.clear();
    }
    fn release_pit(&mut self) {
        if let Some(pit) = self.pit.take() {
            let service = self.search_service.clone();
            let id = self.connection_id;
            self.runtime.spawn(async move {
                let _ = service.close_point_in_time(id, &pit).await;
            });
        }
    }
    fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.running = false;
        self.cell = None;
        self.history_cell = None;
        self.notice =
            Some("Request cancelled; server work may continue until its timeout.".to_string());
        self.reset_pages();
    }

    fn format_query(&mut self) {
        if self.kind.is_search() || self.kind.is_document() {
            match serde_json::from_str::<serde_json::Value>(&self.query_text) {
                Ok(value) => {
                    self.query_text = serde_json::to_string_pretty(&value)
                        .unwrap_or_else(|_| self.query_text.clone());
                    self.error = None;
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        } else {
            self.query_text = crate::ui::sql_formatter::format_sql(&self.query_text);
        }
    }

    fn save_query(&mut self) {
        if self.query_text.trim().is_empty() || self.save_cell.is_some() {
            return;
        }
        let service = self.query_service.clone();
        let sql = self.query_text.clone();
        let title = format!("{} [{}]", self.object, self.kind.as_str());
        let connection_id = self.connection_id;
        let database = if self.kind.is_search() {
            None
        } else {
            Some(self.object.clone())
        };
        let schema = self.kind.is_search().then(|| self.object.clone());
        let cell = Arc::new(Mutex::new(None));
        self.save_cell = Some(cell.clone());
        self.runtime.spawn(async move {
            let result = service
                .add_favorite(Some(connection_id), database, schema, title, sql)
                .await
                .map_err(|error| error.diagnostic_message());
            *cell.lock().unwrap() = Some(result);
        });
    }

    fn saved_query_panel(&mut self, ui: &mut Ui) {
        ui.label(RichText::new(tr("Saved queries")).strong());
        search_filter(ui, &mut self.saved_search);
        let query = self.saved_search.to_lowercase();
        let mut selected = None;
        ScrollArea::vertical()
            .id_source("es_saved_query_list")
            .max_height(120.0)
            .show(ui, |ui| {
                for favorite in self.saved_queries.iter().filter(|favorite| {
                    favorite.connection_id == Some(self.connection_id)
                        && (favorite.database.is_some() || favorite.schema.is_some())
                        && (query.is_empty()
                            || favorite.title.to_lowercase().contains(&query)
                            || favorite
                                .schema
                                .as_deref()
                                .unwrap_or_default()
                                .to_lowercase()
                                .contains(&query)
                            || favorite
                                .database
                                .as_deref()
                                .unwrap_or_default()
                                .to_lowercase()
                                .contains(&query)
                            || favorite.sql.to_lowercase().contains(&query))
                }) {
                    if ui
                        .selectable_label(false, &favorite.title)
                        .on_hover_text(&favorite.sql)
                        .clicked()
                    {
                        selected = Some(favorite.clone());
                    }
                }
            });
        if let Some(favorite) = selected {
            self.object = favorite
                .schema
                .clone()
                .or(favorite.database.clone())
                .unwrap_or_default();
            self.query_text = favorite.sql;
        }
    }

    fn history_panel(&mut self, ui: &mut Ui) {
        ui.label(RichText::new(tr("History")).strong());
        search_filter(ui, &mut self.history_search);
        let query = self.history_search.to_lowercase();
        let mut selected = None;
        ScrollArea::vertical()
            .id_source("es_query_history_list")
            .max_height(140.0)
            .show(ui, |ui| {
                for entry in self.history_entries.iter().filter(|entry| {
                    entry.connection_id == self.connection_id
                        && entry.object.is_some()
                        && (query.is_empty()
                            || entry.summary.to_lowercase().contains(&query)
                            || entry
                                .object
                                .as_deref()
                                .unwrap_or_default()
                                .to_lowercase()
                                .contains(&query)
                            || entry.sql.to_lowercase().contains(&query))
                }) {
                    if ui
                        .selectable_label(false, &entry.summary)
                        .on_hover_text(format!("{}\n{}", entry.sql, entry.executed_at))
                        .clicked()
                    {
                        selected = Some(entry.clone());
                    }
                }
            });
        if let Some(entry) = selected {
            if let Some(object) = entry.object {
                self.object = object;
            }
            self.database = entry.database;
            self.query_text = entry.sql;
        }
    }

    fn poll(&mut self) -> Option<String> {
        let cell = self.cell.take()?;
        let is_done = cell.lock().map(|guard| guard.is_some()).unwrap_or(false);
        if !is_done {
            self.cell = Some(cell);
            return None;
        }
        self.running = false;
        if let Ok(mut guard) = cell.lock() {
            match guard.take() {
                Some(Ok(page)) => {
                    self.task = None;
                    self.has_more = page.count >= self.page_size as usize;
                    self.result_text = page.text;
                    self.pit = page.pit;
                    if let Some(cursor) = page.next_cursor {
                        self.cursors
                            .insert(self.offset + u64::from(self.page_size), cursor);
                    }
                    if page.notice.is_some() {
                        self.notice = page.notice;
                    }
                    return Some(tr("Query complete"));
                }
                Some(Err(message)) => {
                    self.error = Some(message.clone());
                    self.task = None;
                    self.reset_pages();
                    self.result_text.clear();
                    return Some(message);
                }
                None => {}
            }
        }
        None
    }

    pub fn ui(&mut self, ctx: &Context, ui: &mut Ui) -> Option<String> {
        let status = self.poll().or_else(|| self.notice.take());
        if let Some(cell) = self.load_cell.take() {
            let loaded = { cell.lock().unwrap().take() };
            if let Some((history, favorites)) = loaded {
                self.history_entries = history;
                self.saved_queries = favorites;
            } else {
                self.load_cell = Some(cell);
            }
        }
        if let Some(cell) = self.save_cell.take() {
            let result = { cell.lock().unwrap().take() };
            if let Some(result) = result {
                match result {
                    Ok(favorite) => {
                        self.saved_queries.insert(0, favorite);
                        self.notice = Some(tr("Query saved"));
                    }
                    Err(error) => self.error = Some(error),
                }
            } else {
                self.save_cell = Some(cell);
            }
        }
        if self.running
            || self.save_cell.is_some()
            || self.history_cell.is_some()
            || self.load_cell.is_some()
            || self.mapping_pending
            || self.export_cell.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some(cell) = self.history_cell.take() {
            let result = { cell.lock().unwrap().take() };
            if let Some(result) = result {
                if let Err(error) = result {
                    self.notice = Some(error);
                }
            } else {
                self.history_cell = Some(cell);
            }
        }

        if let Some(cell) = self.export_cell.take() {
            let result = cell.lock().unwrap().take();
            match result {
                Some(Ok(())) => self.notice = Some("Current page exported".into()),
                Some(Err(error)) => self.error = Some(error),
                None => self.export_cell = Some(cell),
            }
        }
        if let Some(result) = self.mapping_cell.lock().unwrap().take() {
            self.mapping_pending = false;
            match result {
                Ok(fields) => self.mapping_fields = fields,
                Err(error) => self.notice = Some(format!("Field suggestions unavailable: {error}")),
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.running, eframe::egui::Button::new(tr("Run")))
                .clicked()
            {
                self.reset_pages();
                self.run();
            }
            if self.running {
                if ui.button(tr("Stop")).clicked() {
                    self.cancel();
                }
                ui.add(Spinner::new().size(16.0));
                ui.label(tr("Running..."));
            }
            if ui
                .add_enabled(!self.running, Button::new(tr("Format")))
                .clicked()
            {
                self.format_query();
            }
            if ui
                .add_enabled(self.save_cell.is_none(), Button::new(tr("Save")))
                .clicked()
            {
                self.save_query();
            }
            if self.kind.is_search() {
                ui.selectable_value(&mut self.show_saved, false, tr("History"));
                ui.selectable_value(&mut self.show_saved, true, tr("Favorites"));
            }
        });
        ui.horizontal(|ui| {
            let pageable = self.kind.is_search() || self.kind.is_document();
            if pageable {
                ui.label(tr("Page size"));
                if ui
                    .add_enabled(
                        !self.running,
                        eframe::egui::DragValue::new(&mut self.page_size).clamp_range(1..=1000),
                    )
                    .changed()
                {
                    self.reset_pages();
                }
                if ui
                    .add_enabled(
                        !self.running && self.offset > 0,
                        Button::new(tr("Previous")),
                    )
                    .clicked()
                {
                    self.offset = self.offset.saturating_sub(u64::from(self.page_size));
                    self.run();
                }
                if ui
                    .add_enabled(!self.running && self.has_more, Button::new(tr("Next")))
                    .clicked()
                {
                    self.offset += u64::from(self.page_size);
                    self.run();
                }
                ui.label(format!("Offset: {}", self.offset));
            }
            if ui
                .add_enabled(
                    !self.running && !self.result_text.is_empty() && self.export_cell.is_none(),
                    Button::new(tr("Export current page JSON")),
                )
                .clicked()
            {
                let text = self.result_text.clone();
                let cell = Arc::new(Mutex::new(None));
                self.export_cell = Some(cell.clone());
                self.runtime.spawn(async move {
                    let result = if let Some(file) = rfd::AsyncFileDialog::new()
                        .add_filter("JSON", &["json"])
                        .set_file_name("results.json")
                        .save_file()
                        .await
                    {
                        let path = file.path().to_path_buf();
                        tokio::task::spawn_blocking(move || {
                            db_common::atomic_file::replace(&path, text.as_bytes())
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|result| result.map_err(|e| e.diagnostic_message()))
                    } else {
                        Ok(())
                    };
                    *cell.lock().unwrap() = Some(result);
                });
            }
        });
        if self.kind.is_document() {
            ui.collapsing("MongoDB query syntax", |ui| {
                ui.label(tr("Use a filter object, or an envelope with operation: find / aggregate / insert / update / delete."));
                ui.code(r#"{"operation":"find","filter":{},"sort":{"_id":1},"projection":{"name":1}}"#);
                ui.code(r#"{"operation":"aggregate","pipeline":[{"$match":{}}]}"#);
                ui.code(r#"{"operation":"insert","document":{"name":"example"}}"#);
                ui.code(r#"{"operation":"update","filter":{"name":"example"},"update":{"$set":{"active":true}}}"#);
                ui.code(r#"{"operation":"delete","filter":{"_id":{"$oid":"507f1f77bcf86cd799439011"}}}"#);
                ui.label(tr("Update and delete affect one matching document; a non-empty filter is required."));
            });
        }
        ui.add_space(6.0);

        if self.kind.is_search() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Index")).strong());
                ui.label(RichText::new(&self.object).monospace());
            });
        }
        ui.label(RichText::new(tr("Query (JSON DSL or SQL)")).strong());
        let editor_id = ui.make_persistent_id(("search_query", self.connection_id, &self.object));
        crate::ui::query_input::editor_with_fields(
            ui,
            editor_id,
            &mut self.query_text,
            self.kind,
            &self.object,
            8,
            &self.mapping_fields,
        );
        if ui.memory(|memory| memory.has_focus(editor_id))
            && ui.input_mut(|input| {
                input.consume_key(
                    eframe::egui::Modifiers::CTRL | eframe::egui::Modifiers::SHIFT,
                    Key::F,
                )
            })
        {
            self.format_query();
        }
        if self.kind.is_search() {
            if self.show_saved {
                self.saved_query_panel(ui);
            } else {
                self.history_panel(ui);
            }
        }
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        ScrollArea::vertical()
            .id_source((
                "search_browser_results",
                self.connection_id.as_uuid(),
                &self.object,
            ))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(error) = &self.error {
                    ui.label(RichText::new(error).color(Color32::RED));
                } else if !self.result_text.is_empty() {
                    TextEdit::multiline(&mut self.result_text)
                        .code_editor()
                        .interactive(true)
                        .desired_width(f32::INFINITY)
                        .show(ui);
                } else if !self.running {
                    ui.label(RichText::new(tr("No results yet")).color(Color32::GRAY));
                }
            });

        status
    }
}

fn search_filter(ui: &mut Ui, text: &mut String) {
    ui.horizontal_wrapped(|ui| {
        let row_height = 22.0;
        let clear_width = 22.0;
        ui.add_sized(
            [(ui.available_width() - clear_width).max(40.0), row_height],
            TextEdit::singleline(text).hint_text(tr("Search")),
        );
        if crate::ui::app::style::close_icon_button(ui, row_height, false)
            .on_hover_text(tr("Clear"))
            .clicked()
        {
            text.clear();
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn run_query(
    service: SearchService,
    kind: DataSourceKind,
    connection_id: ConnectionId,
    database: Option<&str>,
    object: &str,
    query_text: &str,
    limit: u32,
    offset: u64,
    pit: Option<String>,
    cursor: Option<serde_json::Value>,
) -> Result<QueryPage, String> {
    let mut page = QueryPage::default();
    if kind.is_search() {
        let mut query: serde_json::Value = serde_json::from_str(query_text)
            .map_err(|error| format!("Invalid search JSON: {error}"))?;
        if !query.is_object() {
            return Err("Search query must be a JSON object".to_string());
        }
        if matches!(
            kind,
            DataSourceKind::Elasticsearch | DataSourceKind::Easysearch
        ) {
            let pit = if offset == 0 && pit.is_none() {
                match service.open_point_in_time(connection_id, object).await {
                    Ok(id) => Some(id),
                    Err(_) => {
                        page.notice = Some("Point-in-time paging unavailable; using offset paging (up to the server result window).".to_string());
                        None
                    }
                }
            } else {
                pit
            };
            if let Some(id) = &pit {
                query["pit"] = serde_json::json!({"id":id,"keep_alive":"1m"});
                query.as_object_mut().unwrap().remove("from");
                if query.get("sort").is_none() {
                    query["sort"] = serde_json::json!(["_shard_doc"]);
                }
                query.as_object_mut().unwrap().remove("search_after");
                if let Some(cursor) = cursor {
                    query["search_after"] = cursor;
                } else if offset > 0 {
                    return Err("Paging cursor expired; run the query again.".to_string());
                }
            } else {
                query["from"] = serde_json::json!(offset);
            }
            page.pit = pit;
            query["size"] = serde_json::json!(limit);
        } else {
            query["offset"] = serde_json::json!(offset);
            query["limit"] = serde_json::json!(limit);
        }
        let result = match service
            .search(connection_id, None, object, &query, limit)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if let Some(id) = &page.pit {
                    let _ = service.close_point_in_time(connection_id, id).await;
                }
                return Err(error.diagnostic_message());
            }
        };
        page.count = result.documents.len();
        if let Some(raw) = &result.raw_response {
            if let Some(id) = raw.get("pit_id").and_then(|v| v.as_str()) {
                page.pit = Some(id.to_string());
            }
            page.next_cursor = raw
                .pointer("/hits/hits")
                .and_then(|v| v.as_array())
                .and_then(|hits| hits.last())
                .and_then(|hit| hit.get("sort"))
                .cloned();
        }
        page.text = serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?;
    } else if kind.is_document() {
        let query: serde_json::Value = serde_json::from_str(query_text)
            .map_err(|error| format!("Invalid MongoDB JSON: {error}"))?;
        let result = service
            .execute_document_query(
                connection_id,
                None,
                database.unwrap_or(""),
                object,
                &query,
                limit,
                offset,
            )
            .await
            .map_err(|error| error.diagnostic_message())?;
        page.count = result
            .get("documents")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len);
        page.text = serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?;
    } else if kind.is_http_sql() {
        let results = service
            .execute_sql(connection_id, None, database, query_text)
            .await
            .map_err(|error| error.diagnostic_message())?;
        page.text = serde_json::to_string_pretty(&results).map_err(|error| error.to_string())?;
    } else {
        return Err(format!(
            "Unsupported connection type for the browser: {}",
            kind.as_str()
        ));
    }
    Ok(page)
}

impl Drop for SearchBrowser {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.release_pit();
    }
}

fn default_query(kind: DataSourceKind) -> String {
    if kind.is_search() {
        if kind == DataSourceKind::Meilisearch {
            "{\"q\":\"\",\"limit\":10}".to_string()
        } else {
            "{\"query\":{\"match_all\":{}}}".to_string()
        }
    } else if kind.is_document() {
        "{}".to_string()
    } else {
        "SELECT 1".to_string()
    }
}
