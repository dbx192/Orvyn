use super::DesktopApp;
use crate::ui::i18n::text as tr;
use db_common::{ConnectionId, QueryCancellation, Result};
use db_mysql::{run_sql_file, ScriptEncoding, ScriptProgress};
use eframe::egui::{Context, ProgressBar, Window};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const ENCODINGS: [&str; 9] = [
    "",
    "UTF-8",
    "GB18030",
    "Big5",
    "UTF-16LE",
    "UTF-16BE",
    "Windows-1252",
    "Shift_JIS",
    "EUC-KR",
];

pub(super) struct SqlFileJob {
    path: PathBuf,
    connection_id: ConnectionId,
    database: String,
    encoding: &'static str,
    progress: Arc<Mutex<ScriptProgress>>,
    cancellation: QueryCancellation,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
    outcome: Option<String>,
}

impl DesktopApp {
    pub(super) fn choose_sql_file(&mut self, connection_id: ConnectionId, database: String) {
        if self.sql_file.is_some() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title(tr("Run SQL File..."))
            .add_filter("SQL", &["sql"])
            .pick_file()
        {
            self.sql_file = Some(SqlFileJob {
                path,
                connection_id,
                database,
                encoding: "",
                progress: Arc::default(),
                cancellation: QueryCancellation::default(),
                task: None,
                outcome: None,
            });
        }
    }

    pub(super) fn sql_file_window(&mut self, ctx: &Context) {
        let Some(job) = self.sql_file.as_mut() else {
            return;
        };
        if job.task.as_ref().is_some_and(|task| task.is_finished()) {
            let task = job.task.take().expect("finished SQL file task");
            let outcome = match self.runtime.block_on(task) {
                Ok(Ok(())) => tr("SQL file completed"),
                Ok(Err(error)) => error.diagnostic_message(),
                Err(error) => error.to_string(),
            };
            self.state.set_status(outcome.clone());
            self.object_tree
                .refresh_database(job.connection_id, &job.database);
            job.outcome = Some(outcome);
        }
        let running = job.task.is_some();
        let mut open = true;
        let mut start = false;
        let mut close = false;
        Window::new(tr("Run SQL File..."))
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(job.path.display().to_string());
                ui.label(format!("{}  {}", tr("Database"), job.database));
                if running || job.outcome.is_some() {
                    let progress = *job.progress.lock().unwrap_or_else(|error| error.into_inner());
                    let fraction = if progress.total_bytes > 0 {
                        (progress.bytes_read as f64 / progress.total_bytes as f64) as f32
                    } else {
                        0.0
                    };
                    ui.add(ProgressBar::new(fraction).show_percentage());
                    ui.label(format!("{}  {}", tr("Statements executed"), progress.statements));
                }
                if let Some(outcome) = &job.outcome {
                    ui.label(outcome);
                    close = ui.button(tr("Close")).clicked();
                } else if running {
                    ui.add_enabled_ui(!job.cancellation.is_cancelled(), |ui| {
                        if ui.button(tr("Stop after current statement")).clicked() {
                            job.cancellation.cancel();
                        }
                    });
                } else {
                    eframe::egui::ComboBox::from_label(tr("File encoding"))
                        .selected_text(encoding_name(job.encoding))
                        .show_ui(ui, |ui| {
                            for encoding in ENCODINGS {
                                ui.selectable_value(&mut job.encoding, encoding, encoding_name(encoding));
                            }
                        });
                    ui.label(tr("The script runs on the selected database. Completed changes may persist if it stops."));
                    start = ui.button(tr("Run")).clicked();
                }
            });
        if start {
            let manager = self.connection_manager.clone();
            let connection_id = job.connection_id;
            let database = job.database.clone();
            let path = job.path.clone();
            let encoding = job.encoding;
            let cancellation = job.cancellation.clone();
            let progress = job.progress.clone();
            job.task = Some(self.runtime.spawn(async move {
                let (mut profile, password) =
                    manager.profile_with_password(connection_id, None).await?;
                profile.database = Some(database);
                run_sql_file(
                    profile,
                    password,
                    path,
                    ScriptEncoding::from_label(encoding)?,
                    cancellation,
                    move |value| {
                        *progress.lock().unwrap_or_else(|error| error.into_inner()) = value;
                    },
                )
                .await
            }));
        }
        if running || start {
            ctx.request_repaint_after(Duration::from_millis(100));
            if !open {
                job.cancellation.cancel();
            }
        } else if close || !open {
            self.sql_file = None;
        }
    }
}

fn encoding_name(encoding: &str) -> String {
    if encoding.is_empty() {
        tr("Auto (BOM or UTF-8)")
    } else {
        encoding.to_owned()
    }
}
