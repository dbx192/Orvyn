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
use app_core::search_service::SearchService;
use db_common::{ConnectionId, DataSourceKind};
use eframe::egui::{Color32, Context, RichText, ScrollArea, Spinner, TextEdit, Ui};
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
    runtime: Handle,
    connection_id: ConnectionId,
    kind: DataSourceKind,
    database: Option<String>,
    object: String,
    query_text: String,
    running: bool,
    result_text: String,
    error: Option<String>,
    cell: Option<SharedQueryCell>,
}

impl SearchBrowser {
    pub fn new(
        search_service: SearchService,
        runtime: Handle,
        connection_id: ConnectionId,
        kind: DataSourceKind,
        database: Option<String>,
        object: String,
    ) -> Self {
        Self {
            search_service,
            runtime,
            connection_id,
            kind,
            database,
            object,
            query_text: default_query(kind),
            running: false,
            result_text: String::new(),
            error: None,
            cell: None,
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
        let status = self.poll();
        if self.running {
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
        });
        ui.add_space(6.0);

        ui.label(RichText::new(tr("Query (JSON DSL or SQL)")).strong());
        TextEdit::multiline(&mut self.query_text)
            .code_editor()
            .desired_rows(8)
            .desired_width(f32::INFINITY)
            .show(ui);
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
                    let mut text = self.result_text.clone();
                    TextEdit::multiline(&mut text)
                        .code_editor()
                        .interactive(false)
                        .desired_width(f32::INFINITY)
                        .show(ui);
                } else if !self.running {
                    ui.label(RichText::new(tr("No results yet")).color(Color32::GRAY));
                }
            });

        status
    }
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
