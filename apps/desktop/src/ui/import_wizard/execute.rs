//! Import execution is separate from wizard rendering and retains one bounded batch.
use super::*;
use db_common::{QueryExecutionRequest, QueryId};
use import_export::import_csv::{generate_insert_batches, CsvRowReader};
use std::io::Write;

#[derive(Clone, Default)]
pub(super) struct ImportProgress {
    pub processed: usize,
}
pub(super) struct ImportJob {
    pub preview: CsvPreview,
    pub mapping: FieldMapping,
    pub table_columns: Vec<TableColumn>,
    pub batch_size: usize,
    pub stop_on_error: bool,
    pub options: CsvImportOptions,
    pub query_service: QueryService,
    pub connection_manager: ConnectionManager,
    pub connection_id: ConnectionId,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub table: String,
    pub cancellation: db_common::QueryCancellation,
    pub progress: Arc<Mutex<ImportProgress>>,
}
impl ImportJob {
    pub async fn run(self) -> Result<ImportResult> {
        let start = std::time::Instant::now();
        let profile = self
            .connection_manager
            .get_profile(self.connection_id)
            .await?
            .ok_or_else(|| db_common::AppError::validation("connection profile not found"))?;
        if profile.security.read_only {
            return Err(db_common::AppError::validation("connection is read-only"));
        }
        let dialect = import_dialect(profile.kind);
        let qualified = qualified_import_table_name(
            profile.kind,
            self.database.as_deref(),
            self.schema.as_deref(),
            &self.table,
            dialect.as_ref(),
        )?;
        let batch_size = self.batch_size.clamp(1, 5000);
        let path = PathBuf::from(&self.preview.file_path);
        let options = self.options.clone();
        let cancellation = self.cancellation.clone();
        // Backpressure keeps at most two batches in memory, including the active batch.
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let producer = tokio::task::spawn_blocking(move || {
            let result = (|| -> Result<()> {
                let mut reader = CsvRowReader::open(&path, &options)?;
                while !cancellation.is_cancelled() {
                    let rows = reader.read_batch(batch_size)?;
                    if rows.is_empty() {
                        break;
                    }
                    if sender.blocking_send(Ok(rows)).is_err() {
                        break;
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = sender.blocking_send(Err(error));
            }
        });
        let mut result = ImportResult {
            total_rows: self.preview.estimated_total_rows as usize,
            success_rows: 0,
            failed_rows: 0,
            errors: Vec::new(),
            elapsed_secs: 0.0,
            cancelled: false,
            failure_file: None,
        };
        let mut failure_writer: Option<std::io::BufWriter<std::fs::File>> = None;
        let mut processed = 0usize;
        let tab = QueryId::new();
        loop {
            let next = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => break,
                next = receiver.recv() => next,
            };
            let Some(rows) = next else {
                break;
            };
            let rows = rows?;
            if self.cancellation.is_cancelled() {
                break;
            }
            let validation = validate_types(
                &rows,
                &self.preview.headers,
                &self.table_columns,
                &self.mapping,
            );
            let execution = if let Some(issue) = validation.issues.first() {
                Err(db_common::AppError::validation(format!(
                    "Row {}, column {}: {}",
                    processed + issue.row,
                    issue.column,
                    issue.reason
                )))
            } else {
                let batches = generate_insert_batches(
                    &rows,
                    &self.preview.headers,
                    &self.table_columns,
                    &self.mapping,
                    &qualified,
                    dialect.as_ref(),
                    batch_size,
                )?;
                let mut request =
                    QueryExecutionRequest::all(tab, self.connection_id, batches.join("\n"));
                request.database = self.database.clone();
                request.schema = self.schema.clone();
                request.timeout_ms = 60_000;
                self.query_service
                    .execute_cancellable(request, None, self.cancellation.clone())
                    .await
            };
            match execution {
                Ok(report) => {
                    result.success_rows += report.affected_rows as usize;
                }
                Err(error) => {
                    result.failed_rows += rows.len();
                    if result.errors.len() < 100 {
                        result.errors.push(error.diagnostic_message());
                    }
                    if failure_writer.is_none() {
                        let filename = format!("orvyn-import-failures-{}.jsonl", QueryId::new());
                        let path = std::env::temp_dir().join(filename);
                        let file = std::fs::OpenOptions::new()
                            .create_new(true)
                            .write(true)
                            .open(&path)?;
                        failure_writer = Some(std::io::BufWriter::new(file));
                        result.failure_file = Some(path);
                    }
                    let writer = failure_writer.as_mut().unwrap();
                    for (index, row) in rows.iter().enumerate() {
                        serde_json::to_writer(
                            &mut *writer,
                            &serde_json::json!({
                                "source_row": processed + index + 1, "values": row,
                                "error": error.diagnostic_message(), "status": "failed_or_unconfirmed",
                            }),
                        )?;
                        writer.write_all(b"\n")?;
                    }
                    if self.stop_on_error || self.cancellation.is_cancelled() {
                        processed += rows.len();
                        self.progress.lock().unwrap().processed = processed;
                        break;
                    }
                }
            }
            processed += rows.len();
            self.progress.lock().unwrap().processed = processed;
        }
        drop(receiver); // Unblocks a producer waiting to send the next batch.
        producer
            .await
            .map_err(|e| db_common::AppError::internal(e.to_string()))?;
        if let Some(mut writer) = failure_writer {
            writer.flush()?;
            writer.get_ref().sync_all()?;
        }
        result.cancelled = self.cancellation.is_cancelled();
        result.elapsed_secs = start.elapsed().as_secs_f64();
        Ok(result)
    }
}
