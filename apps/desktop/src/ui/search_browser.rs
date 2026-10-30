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
use db_common::{ConnectionId, DataSourceKind, FavoriteQuery};
use eframe::egui::{Button, Color32, Context, Key, RichText, ScrollArea, Spinner, TextEdit, Ui};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::runtime::Handle;

const SEARCH_LIMIT: u32 = 100;

/// Shared async slot for streaming query output.
type SharedQueryCell = Arc<Mutex<Option<Result<String, String>>>>;

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
    save_cell: Option<Arc<Mutex<Option<Result<FavoriteQuery, String>>>>>,
    completion_visible: bool,
    completion_cursor: usize,
    saved_queries: Vec<FavoriteQuery>,
    show_saved: bool,
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
            completion_visible: false,
            completion_cursor: 0,
            saved_queries: Vec::new(),
            show_saved: false,
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
        let limit = SEARCH_LIMIT;

        let cell: Arc<Mutex<Option<Result<String, String>>>> = Arc::new(Mutex::new(None));
        self.cell = Some(cell.clone());

        runtime.spawn(async move {
            let result = run_query(
                service,
                kind,
                connection_id,
                database.as_deref(),
                &object,
                &query_text,
                limit,
            )
            .await;
            *cell.lock().unwrap() = Some(result);
        });
    }

    fn format_query(&mut self) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.query_text) {
            if let Ok(formatted) = serde_json::to_string_pretty(&value) {
                self.query_text = formatted;
            }
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
        let cell = Arc::new(Mutex::new(None));
        self.save_cell = Some(cell.clone());
        self.runtime.spawn(async move {
            let result = service
                .add_favorite(Some(connection_id), None, None, title, sql)
                .await
                .map_err(|error| error.diagnostic_message());
            *cell.lock().unwrap() = Some(result);
        });
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
                Some(Ok(text)) => {
                    self.result_text = text;
                    return Some(tr("Query complete"));
                }
                Some(Err(message)) => {
                    self.error = Some(message.clone());
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
        if self.running || self.save_cell.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.running, eframe::egui::Button::new(tr("Run")))
                .clicked()
            {
                self.run();
            }
            if self.running {
                ui.add(Spinner::new().size(16.0));
                ui.label(tr("Running..."));
            }
            if ui.add_enabled(!self.running, Button::new(tr("Format"))).clicked() {
                self.format_query();
            }
            if ui.add_enabled(self.save_cell.is_none(), Button::new(tr("Save"))).clicked() {
                self.save_query();
            }
            if self.kind.is_search()
                && ui.selectable_label(self.show_saved, tr("Saved")).clicked()
            {
                self.show_saved = !self.show_saved;
            }
        });
        ui.add_space(6.0);

        ui.label(RichText::new(tr("Query (JSON DSL or SQL)")).strong());
        let query_output = TextEdit::multiline(&mut self.query_text)
            .code_editor()
            .desired_rows(8)
            .desired_width(f32::INFINITY)
            .show(ui);
        let query_response = &query_output.response;
        if let Some(range) = query_output.cursor_range {
            self.completion_cursor = char_to_byte_offset(
                &self.query_text,
                range.primary.ccursor.index,
            );
        }
        self.completion_visible = query_response.has_focus()
            && (query_response.changed()
                || ui.input(|input| input.key_pressed(Key::Space) && input.modifiers.ctrl));
        if self.completion_visible && self.kind.is_search() {
            ui.horizontal_wrapped(|ui| {
                for (label, snippet) in [
                    ("match_all", "\"match_all\": {}"),
                    ("match", "\"match\": { \"field\": \"value\" }"),
                    ("term", "\"term\": { \"field\": \"value\" }"),
                    ("bool", "\"bool\": { \"must\": [], \"filter\": [] }"),
                ] {
                    if ui.small_button(label).clicked() {
                        self.query_text = insert_completion(
                            &self.query_text,
                            self.completion_cursor,
                            snippet,
                        );
                        self.completion_visible = false;
                    }
                }
            });
        }
        if self.show_saved && self.kind.is_search() {
            ui.horizontal_wrapped(|ui| {
                let prefix = format!("{} [", self.object);
                for favorite in self
                    .saved_queries
                    .iter()
                    .filter(|favorite| favorite.title.starts_with(&prefix))
                {
                    if ui.small_button(&favorite.title).clicked() {
                        self.query_text = favorite.sql.clone();
                    }
                }
            });
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

fn char_to_byte_offset(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

fn insert_completion(text: &str, cursor: usize, snippet: &str) -> String {
    let cursor = cursor.min(text.len());
    format!("{}{}{}", &text[..cursor], snippet, &text[cursor..])
}

async fn run_query(
    service: SearchService,
    kind: DataSourceKind,
    connection_id: ConnectionId,
    database: Option<&str>,
    object: &str,
    query_text: &str,
    limit: u32,
) -> Result<String, String> {
    if kind.is_search() {
        let query: serde_json::Value = serde_json::from_str(query_text)
            .map_err(|error| format!("Invalid search JSON: {error}"))?;
        let result = service
            .search(connection_id, None, object, &query, limit)
            .await
            .map_err(|error| error.diagnostic_message())?;
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
    } else if kind.is_document() {
        let filter: serde_json::Value = serde_json::from_str(query_text)
            .map_err(|error| format!("Invalid filter JSON: {error}"))?;
        let documents = service
            .find_documents(
                connection_id,
                None,
                database.unwrap_or(""),
                object,
                Some(&filter),
                limit,
            )
            .await
            .map_err(|error| error.diagnostic_message())?;
        serde_json::to_string_pretty(&documents).map_err(|error| error.to_string())
    } else if kind.is_http_sql() {
        let results = service
            .execute_sql(connection_id, None, database, query_text)
            .await
            .map_err(|error| error.diagnostic_message())?;
        serde_json::to_string_pretty(&results).map_err(|error| error.to_string())
    } else {
        Err(format!(
            "Unsupported connection type for the browser: {}",
            kind.as_str()
        ))
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
