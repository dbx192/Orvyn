//! CSV import wizard: step-by-step guide to import CSV data into a database table.

use app_core::{connection::ConnectionManager, metadata::MetadataService, query::QueryService};
use db_common::dialect::DoubleQuoteDialect;
use db_common::{
    ConnectionId, Dialect, Identifier, QueryExecutionMode, QueryExecutionRequest, QueryId,
    Result, TableColumn,
};
use eframe::egui::{Button, Color32, ComboBox, Frame, RichText, ScrollArea, Ui};
use import_export::import_csv::{
    auto_map_fields, generate_insert_batches, parse_csv_preview, validate_types, CsvImportOptions,
    CsvPreview, FieldMapping, ValidationReport,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;

/// Result of a completed import.
#[derive(Debug, Clone)]
pub struct ImportResult {
    pub total_rows: usize,
    pub success_rows: usize,
    pub failed_rows: usize,
    pub errors: Vec<String>,
    pub elapsed_secs: f64,
}

/// Steps of the import wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportStep {
    SelectFile,
    Preview,
    FieldMapping,
    Validate,
    Execute,
}

pub struct ImportWizard {
    // Target table info
    connection_id: ConnectionId,
    database: Option<String>,
    schema: Option<String>,
    table: String,

    // Step tracking
    step: ImportStep,

    // Step 1: File selection
    file_path: Option<PathBuf>,
    csv_options: CsvImportOptions,

    // Step 2-3: Preview & mapping
    preview: Option<CsvPreview>,
    table_columns: Vec<TableColumn>,
    mapping: Option<FieldMapping>,

    // Step 4: Validation
    validation_report: Option<ValidationReport>,

    // Step 5: Execute
    batch_size: usize,
    stop_on_error: bool,
    import_result: Option<ImportResult>,

    // Services
    metadata_service: MetadataService,
    query_service: QueryService,
    runtime: Handle,

    // Async pending
    pending_preview: Option<Arc<Mutex<Option<Result<CsvPreview>>>>>,
    pending_validation: Option<Arc<Mutex<Option<Result<ValidationReport>>>>>,
    pending_import: Option<Arc<Mutex<Option<Result<ImportResult>>>>>,

    // Status
    loading: bool,
    error: Option<String>,
    status: String,
}

impl ImportWizard {
    pub fn new(
        connection_id: ConnectionId,
        database: Option<String>,
        schema: Option<String>,
        table: String,
        metadata_service: MetadataService,
        query_service: QueryService,
        _connection_manager: ConnectionManager,
        runtime: Handle,
    ) -> Self {
        Self {
            connection_id,
            database,
            schema,
            table,
            step: ImportStep::SelectFile,
            file_path: None,
            csv_options: CsvImportOptions::default(),
            preview: None,
            table_columns: Vec::new(),
            mapping: None,
            validation_report: None,
            batch_size: 1000,
            stop_on_error: true,
            import_result: None,
            metadata_service,
            query_service,
            runtime,
            pending_preview: None,
            pending_validation: None,
            pending_import: None,
            loading: false,
            error: None,
            status: String::new(),
        }
    }

    pub fn table_name(&self) -> String {
        match self.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => format!("{}.{}", s, self.table),
            None => self.table.clone(),
        }
    }

    /// Main UI entry point. Returns an optional status message.
    pub fn ui(&mut self, ui: &mut Ui) -> Option<String> {
        let mut message: Option<String> = None;

        // Poll pending async tasks
        self.poll_pending();

        // Header
        ui.heading(format!("📥 Import CSV → {}", self.table_name()));
        ui.add_space(4.0);

        // Step indicator
        self.step_indicator(ui);
        ui.separator();
        ui.add_space(8.0);

        // Current step
        match self.step {
            ImportStep::SelectFile => self.step_select_file(ui, &mut message),
            ImportStep::Preview => self.step_preview(ui, &mut message),
            ImportStep::FieldMapping => self.step_field_mapping(ui, &mut message),
            ImportStep::Validate => self.step_validate(ui, &mut message),
            ImportStep::Execute => self.step_execute(ui, &mut message),
        }

        if let Some(ref err) = self.error {
            ui.add_space(8.0);
            ui.colored_label(Color32::RED, err);
        }

        if !self.status.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(&self.status).color(Color32::GRAY));
        }

        message
    }

    fn step_indicator(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (idx, (label, _step)) in [
                ("1. File", ImportStep::SelectFile),
                ("2. Preview", ImportStep::Preview),
                ("3. Map Fields", ImportStep::FieldMapping),
                ("4. Validate", ImportStep::Validate),
                ("5. Import", ImportStep::Execute),
            ]
            .iter()
            .enumerate()
            {
                if idx > 0 {
                    ui.label("→");
                }
                let active = self.step == *_step;
                let completed = self.step as usize > *_step as usize;
                let text = if active {
                    RichText::new(*label).strong()
                } else if completed {
                    RichText::new(format!("✓ {}", label)).color(Color32::GREEN)
                } else {
                    RichText::new(*label).color(Color32::GRAY)
                };
                ui.label(text);
            }
        });
    }

    // ── Step 1: File Selection ──

    fn step_select_file(&mut self, ui: &mut Ui, _message: &mut Option<String>) {
        ui.label("Choose a CSV file to import:");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if ui.button("📂 Browse...").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("CSV Files", &["csv", "tsv", "txt"])
                    .pick_file()
                {
                    self.file_path = Some(path);
                }
            }
            if let Some(ref path) = self.file_path {
                ui.label(format!("📄 {}", path.display()));
            } else {
                ui.label(RichText::new("No file selected").color(Color32::GRAY));
            }
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.csv_options.has_header, "First row is header");
            ui.add_space(16.0);
            ui.label("Delimiter:");
            ComboBox::from_id_source("csv_delimiter")
                .selected_text(match self.csv_options.delimiter {
                    b',' => "Comma (,)",
                    b'\t' => "Tab",
                    b';' => "Semicolon (;)",
                    b'|' => "Pipe (|)",
                    _other => "Custom",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.csv_options.delimiter, b',', "Comma (,)");
                    ui.selectable_value(&mut self.csv_options.delimiter, b'\t', "Tab");
                    ui.selectable_value(&mut self.csv_options.delimiter, b';', "Semicolon (;)");
                    ui.selectable_value(&mut self.csv_options.delimiter, b'|', "Pipe (|)");
                });
        });

        ui.add_space(16.0);
        if ui
            .add_enabled(
                self.file_path.is_some() && !self.loading,
                Button::new("Next: Preview →"),
            )
            .clicked()
        {
            self.load_preview();
        }

        if self.loading {
            ui.add_space(8.0);
            ui.label(RichText::new("Loading CSV...").color(Color32::YELLOW));
        }
    }

    fn load_preview(&mut self) {
        let path = match &self.file_path {
            Some(p) => p.clone(),
            None => return,
        };
        let options = self.csv_options.clone();
        self.loading = true;
        self.error = None;

        let cell: Arc<Mutex<Option<Result<CsvPreview>>>> = Arc::new(Mutex::new(None));
        self.pending_preview = Some(cell.clone());
        let rt = self.runtime.clone();

        rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || parse_csv_preview(&path, &options))
                .await
                .unwrap_or_else(|e| {
                    Err(db_common::AppError::validation(format!(
                        "Task panicked: {e}"
                    )))
                });
            *cell.lock().unwrap() = Some(result);
        });
    }

    // ── Step 2: Preview ──

    fn step_preview(&mut self, ui: &mut Ui, _message: &mut Option<String>) {
        if let Some(ref preview) = self.preview {
            ui.label(format!(
                "Found {} columns, ~{} data rows in {}",
                preview.headers.len(),
                preview.estimated_total_rows,
                preview.file_path,
            ));

            ui.add_space(8.0);
            ui.label(RichText::new("First 10 rows:").strong());

            // Table header
            Frame::group(ui.style()).show(ui, |ui| {
                ScrollArea::both().max_height(300.0).show(ui, |ui| {
                    use eframe::egui::Grid;
                    Grid::new("csv_preview_grid").striped(true).show(ui, |ui| {
                        // Column headers
                        for h in &preview.headers {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();

                        let show_rows = preview.sample_rows.len().min(10);
                        for row in preview.sample_rows.iter().take(show_rows) {
                            for val in row {
                                ui.label(val);
                            }
                            ui.end_row();
                        }
                    });
                });
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("← Back").clicked() {
                    self.step = ImportStep::SelectFile;
                }
                if ui.button("Next: Map Fields →").clicked() {
                    self.load_table_columns();
                    self.step = ImportStep::FieldMapping;
                }
            });
        } else {
            ui.label("Loading preview...");
        }
    }

    fn load_table_columns(&mut self) {
        // Load columns synchronously by spawning and awaiting.
        let ms = self.metadata_service.clone();
        let conn_id = self.connection_id;
        let db = self.database.clone();
        let sch = self.schema.clone();
        let tbl = self.table.clone();
        let rt = self.runtime.clone();

        let cell: Arc<Mutex<Option<Result<Vec<TableColumn>>>>> = Arc::new(Mutex::new(None));
        let cell_clone = cell.clone();
        self.loading = true;

        rt.spawn(async move {
            let result = ms
                .table_columns(conn_id, db.as_deref(), sch.as_deref(), &tbl, None)
                .await;
            *cell_clone.lock().unwrap() = Some(result);
        });

        // Block briefly to wait (in a real app this would be async).
        // For simplicity, poll a few times.
        for _ in 0..50 {
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    match result {
                        Ok(cols) => {
                            self.table_columns = cols;
                            // Auto-map fields
                            if let Some(ref preview) = self.preview {
                                self.mapping =
                                    Some(auto_map_fields(&preview.headers, &self.table_columns));
                            }
                        }
                        Err(e) => {
                            self.error =
                                Some(format!("Failed to load table columns: {}", e.message));
                        }
                    }
                    self.loading = false;
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.loading = false;
        self.error = Some("Timed out loading table columns".to_string());
    }

    // ── Step 3: Field Mapping ──

    fn step_field_mapping(&mut self, ui: &mut Ui, message: &mut Option<String>) {
        let preview = match &self.preview {
            Some(p) => p,
            None => {
                ui.label("No preview available.");
                return;
            }
        };
        let mapping = match &mut self.mapping {
            Some(m) => m,
            None => {
                ui.label("No mapping available.");
                return;
            }
        };

        ui.label("Map CSV columns to table columns:");
        ui.add_space(8.0);

        ScrollArea::vertical().max_height(350.0).show(ui, |ui| {
            for (col_idx, col) in self.table_columns.iter().enumerate() {
                ui.horizontal(|ui| {
                    let required = if !col.nullable { " *" } else { "" };
                    ui.label(format!("{}{} ({})", col.name, required, col.database_type));
                    ui.label("←");

                    let current = mapping.mappings[col_idx];
                    let mut selected_text = current
                        .and_then(|c| preview.headers.get(c))
                        .cloned()
                        .unwrap_or_else(|| "-- skip --".to_string());

                    ComboBox::from_id_source(format!("map_{}", col_idx))
                        .selected_text(&selected_text)
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut selected_text,
                                    "-- skip --".to_string(),
                                    "-- skip --",
                                )
                                .clicked()
                            {
                                mapping.mappings[col_idx] = None;
                            }
                            for (csv_idx, header) in preview.headers.iter().enumerate() {
                                if ui
                                    .selectable_value(&mut selected_text, header.clone(), header)
                                    .clicked()
                                {
                                    mapping.mappings[col_idx] = Some(csv_idx);
                                }
                            }
                        });
                });
                ui.add_space(2.0);
            }
        });

        // Auto-map hint
        ui.add_space(8.0);
        if ui.button("🔧 Auto-Map by Name").clicked() {
            self.mapping = Some(auto_map_fields(&preview.headers, &self.table_columns));
            *message = Some("Fields auto-mapped by case-insensitive name matching".to_string());
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("← Back").clicked() {
                self.step = ImportStep::Preview;
            }
            if ui.button("Next: Validate →").clicked() {
                self.run_validation();
                self.step = ImportStep::Validate;
            }
        });
    }

    fn run_validation(&mut self) {
        let preview = match &self.preview {
            Some(p) => p,
            None => return,
        };
        let mapping = match &self.mapping {
            Some(m) => m,
            None => return,
        };

        let headers = preview.headers.clone();
        let table_columns = self.table_columns.clone();
        let mapping_clone = mapping.clone();
        let sample = preview.sample_rows.clone();
        let rt = self.runtime.clone();

        let cell: Arc<Mutex<Option<Result<ValidationReport>>>> = Arc::new(Mutex::new(None));
        self.pending_validation = Some(cell.clone());

        rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                Ok::<_, db_common::AppError>(validate_types(
                    &sample,
                    &headers,
                    &table_columns,
                    &mapping_clone,
                ))
            })
            .await
            .unwrap_or_else(|e| {
                Err(db_common::AppError::validation(format!(
                    "Task panicked: {e}"
                )))
            });
            *cell.lock().unwrap() = Some(result);
        });
    }

    // ── Step 4: Validate ──

    fn step_validate(&mut self, ui: &mut Ui, _message: &mut Option<String>) {
        if let Some(ref report) = self.validation_report {
            if report.issues.is_empty() {
                ui.label(RichText::new("✅ All rows passed validation!").color(Color32::GREEN));
            } else {
                ui.label(
                    RichText::new(format!(
                        "⚠ {} type issues found in {} checked rows:",
                        report.issues.len(),
                        report.total_rows_checked
                    ))
                    .color(Color32::YELLOW),
                );

                ui.add_space(8.0);
                ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                    use eframe::egui::Grid;
                    Grid::new("validation_issues").striped(true).show(ui, |ui| {
                        ui.label(RichText::new("Row").strong());
                        ui.label(RichText::new("Column").strong());
                        ui.label(RichText::new("Value").strong());
                        ui.label(RichText::new("Issue").strong());
                        ui.end_row();
                        for issue in &report.issues {
                            ui.label(issue.row.to_string());
                            ui.label(&issue.column);
                            ui.label(&issue.value);
                            ui.label(RichText::new(&issue.reason).color(Color32::RED));
                            ui.end_row();
                        }
                    });
                });
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.stop_on_error, "Stop on first error");
                ui.add_space(16.0);
                ui.label("Batch size:");
                ui.add(
                    eframe::egui::DragValue::new(&mut self.batch_size)
                        .speed(100)
                        .clamp_range(100..=5000),
                );
            });
        } else {
            ui.label(RichText::new("Running validation...").color(Color32::YELLOW));
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("← Back").clicked() {
                self.step = ImportStep::FieldMapping;
            }
            if ui
                .add_enabled(
                    self.validation_report.is_some() && !self.loading,
                    Button::new("Next: Execute Import →"),
                )
                .clicked()
            {
                self.step = ImportStep::Execute;
            }
        });
    }

    // ── Step 5: Execute ──

    fn step_execute(&mut self, ui: &mut Ui, _message: &mut Option<String>) {
        if let Some(ref result) = self.import_result {
            // Show completed result
            ui.label(
                RichText::new("✅ Import complete!")
                    .color(Color32::GREEN)
                    .size(18.0),
            );
            ui.add_space(8.0);
            ui.label(format!("Total rows: {}", result.total_rows));
            ui.label(format!("Successful: {}", result.success_rows));
            if result.failed_rows > 0 {
                ui.label(
                    RichText::new(format!("Failed: {}", result.failed_rows)).color(Color32::RED),
                );
            }
            ui.label(format!("Time: {:.1}s", result.elapsed_secs));

            if !result.errors.is_empty() {
                ui.add_space(8.0);
                ui.collapsing("Error Details", |ui| {
                    for err in &result.errors {
                        ui.label(RichText::new(err).color(Color32::RED));
                    }
                });
            }
        } else if self.loading {
            ui.label(
                RichText::new("⏳ Import running...")
                    .color(Color32::YELLOW)
                    .size(16.0),
            );
            ui.add_space(8.0);
            ui.label("Executing batch INSERT statements in the background.");
        } else {
            let preview = match &self.preview {
                Some(p) => p,
                None => {
                    ui.label("No data to import.");
                    return;
                }
            };

            ui.label(format!(
                "Ready to import {} rows into {}.",
                preview.estimated_total_rows,
                self.table_name()
            ));
            ui.label(format!("Batch size: {}", self.batch_size));
            ui.label(format!(
                "Strategy: {}",
                if self.stop_on_error {
                    "Stop on first error"
                } else {
                    "Skip errors and continue"
                }
            ));

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui.button("← Back").clicked() {
                    self.step = ImportStep::Validate;
                }
                if ui
                    .add_enabled(!self.loading, Button::new("🚀 Start Import"))
                    .clicked()
                {
                    self.start_import();
                }
            });
        }

        if self.import_result.is_some() {
            ui.add_space(12.0);
            if ui.button("Done").clicked() {
                // Reset wizard to allow another import
                self.step = ImportStep::SelectFile;
                self.file_path = None;
                self.preview = None;
                self.mapping = None;
                self.validation_report = None;
                self.import_result = None;
            }
        }
    }

    fn start_import(&mut self) {
        let preview = match self.preview.clone() {
            Some(p) => p,
            None => return,
        };
        let mapping = match self.mapping.clone() {
            Some(m) => m,
            None => return,
        };
        let table_columns = self.table_columns.clone();

        let dialect: Box<dyn Dialect> = self.get_dialect();
        let qualified = self.qualified_table_name(&*dialect);
        let batch_size = self.batch_size;
        let stop_on_error = self.stop_on_error;
        let query_service = self.query_service.clone();
        let connection_id = self.connection_id;
        let rt = self.runtime.clone();

        self.loading = true;

        let cell: Arc<Mutex<Option<Result<ImportResult>>>> = Arc::new(Mutex::new(None));
        self.pending_import = Some(cell.clone());

        rt.spawn(async move {
            let start = std::time::Instant::now();
            let total = preview.sample_rows.len();
            let mut success = 0usize;
            let mut failed = 0usize;
            let mut errors = Vec::new();

            match generate_insert_batches(
                &preview.sample_rows,
                &preview.headers,
                &table_columns,
                &mapping,
                &qualified,
                &*dialect,
                batch_size,
            ) {
                Ok(batches) => {
                    for (i, batch_sql) in batches.iter().enumerate() {
                        match query_service
                            .execute(
                                QueryExecutionRequest {
                                    tab_id: QueryId::new(),
                                    connection_id,
                                    sql: batch_sql.clone(),
                                    mode: QueryExecutionMode::All,
                                    cursor_offset: 0,
                                    selection: None,
                                    timeout_ms: 60000,
                                },
                                None,
                            )
                            .await
                        {
                            Ok(report) => {
                                let affected: u64 =
                                    report.results.iter().map(|r| r.affected_rows).sum();
                                success += affected as usize;
                                if report.results.iter().any(|r| r.affected_rows == 0) {
                                    errors.push(format!(
                                        "Batch {}: some rows may not have been inserted",
                                        i + 1
                                    ));
                                }
                            }
                            Err(e) => {
                                let batch_rows = batch_size.min(total - i * batch_size);
                                failed += batch_rows;
                                errors.push(format!("Batch {}: {}", i + 1, e.message));
                                if stop_on_error {
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("SQL generation failed: {}", e.message));
                    failed = total;
                }
            }

            let result = ImportResult {
                total_rows: total,
                success_rows: success,
                failed_rows: failed,
                errors,
                elapsed_secs: start.elapsed().as_secs_f64(),
            };
            *cell.lock().unwrap() = Some(Ok(result));
        });
    }

    fn get_dialect(&self) -> Box<dyn Dialect> {
        // Default to DoubleQuoteDialect; in practice the connection profile would determine this.
        Box::new(DoubleQuoteDialect)
    }

    fn qualified_table_name(&self, dialect: &dyn Dialect) -> String {
        let table_ident = Identifier::new(self.table.as_str()).unwrap();
        let quoted_table = dialect.quote_identifier(&table_ident);
        match self.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(schema) => {
                let schema_ident = Identifier::new(schema).unwrap();
                format!(
                    "{}.{}",
                    dialect.quote_identifier(&schema_ident),
                    quoted_table
                )
            }
            None => quoted_table,
        }
    }

    // ── Polling ──

    fn poll_pending(&mut self) {
        // Preview
        if let Some(cell) = self.pending_preview.take() {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(preview) => {
                            self.preview = Some(preview);
                            self.step = ImportStep::Preview;
                        }
                        Err(e) => {
                            self.error = Some(format!("Failed to parse CSV: {}", e.message));
                        }
                    }
                    self.loading = false;
                }
            }
            if !done {
                self.pending_preview = Some(cell);
            }
        }

        // Validation
        if let Some(cell) = self.pending_validation.take() {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(report) => {
                            self.validation_report = Some(report);
                        }
                        Err(e) => {
                            self.error = Some(format!("Validation failed: {}", e.message));
                        }
                    }
                    self.loading = false;
                }
            }
            if !done {
                self.pending_validation = Some(cell);
            }
        }

        // Import
        if let Some(cell) = self.pending_import.take() {
            let mut done = false;
            if let Ok(mut guard) = cell.lock() {
                if let Some(result) = guard.take() {
                    done = true;
                    match result {
                        Ok(import_result) => {
                            self.import_result = Some(import_result);
                        }
                        Err(e) => {
                            self.error = Some(format!("Import failed: {}", e.message));
                        }
                    }
                    self.loading = false;
                }
            }
            if !done {
                self.pending_import = Some(cell);
            }
        }
    }
}
