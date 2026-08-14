use super::{
    export_sql::qualified_table_label,
    payloads::{ClearRequest, DropTableRequest, ImportTableRequest, TableExportRequest},
    ActivePanel, DesktopApp,
};
use crate::ui::{
    connection_manager::ConnectionChange, import_wizard::ImportWizard,
    object_tree::ObjectTreeAction, redis_browser::RedisBrowserUi,
};
use db_common::ConnectionId;
use eframe::egui::{Align, Color32, Context, Layout, RichText, SidePanel};
use import_export::ExportFormat;

const LEFT_PANEL_MIN_WIDTH: f32 = 200.0;
const LEFT_PANEL_MAX_WIDTH: f32 = 280.0;

impl DesktopApp {
    pub(super) fn left_tree(&mut self, ctx: &Context) {
        // Use a fresh persistence id so widths inflated by the previous
        // content-driven sizing implementation are not restored.
        SidePanel::left("connection_object_tree_resizable_v2")
            .resizable(true)
            .default_width(200.0)
            .width_range(LEFT_PANEL_MIN_WIDTH..=LEFT_PANEL_MAX_WIDTH)
            .show(ctx, |ui| {
                let (message, actions) = self.object_tree.ui(ctx, ui);
                if let Some(message) = message {
                    if !message.is_empty() {
                        self.last_tree_status = message.clone();
                        self.state.set_status(message);
                    }
                }
                self.handle_tree_actions(actions);
                if !self.last_tree_status.is_empty() {
                    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                        ui.label(
                            RichText::new(&self.last_tree_status)
                                .small()
                                .color(Color32::GRAY),
                        );
                    });
                }
            });
    }

    pub(super) fn handle_tree_actions(&mut self, actions: Vec<ObjectTreeAction>) {
        for action in actions {
            match action {
                ObjectTreeAction::ImportConnections => {
                    self.connection_manager_ui.choose_import_file();
                }
                ObjectTreeAction::ExportConnections {
                    connection_ids,
                    include_passwords,
                } => {
                    self.connection_manager_ui
                        .choose_export_file(connection_ids, include_passwords);
                }
                ObjectTreeAction::OpenTableData {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    self.query_editor.sync_active_context(
                        connection_id,
                        database.clone(),
                        schema.clone(),
                    );
                    self.state.set_status(format!(
                        "Loading table {}.{} ...",
                        schema.as_deref().unwrap_or(""),
                        table
                    ));
                    self.spawn_table_data_load(
                        connection_id,
                        database.clone(),
                        schema.clone(),
                        table.clone(),
                    );
                    self.active_panel = ActivePanel::TableEditor;
                }
                ObjectTreeAction::OpenQuery {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    self.query_editor.new_query_with_connection(
                        connection_id,
                        database,
                        schema,
                        table,
                    );
                    self.active_panel = ActivePanel::QueryEditor;
                    self.state
                        .set_status(format!("New query on {}", connection_id));
                }
                ObjectTreeAction::OpenSavedQuery {
                    connection_id,
                    database,
                    schema,
                    title,
                    sql,
                } => {
                    let status_title = title.clone();
                    self.query_editor
                        .open_saved_query(connection_id, database, schema, title, sql);
                    self.active_panel = ActivePanel::QueryEditor;
                    self.state
                        .set_status(format!("Opened saved query: {status_title}"));
                }
                ObjectTreeAction::ViewKey {
                    connection_id,
                    database,
                    key,
                } => {
                    let database_index = database
                        .as_deref()
                        .and_then(|value| value.parse::<u8>().ok())
                        .unwrap_or(0);
                    self.ensure_redis_browser(connection_id);
                    if let Some(redis_browser) = &mut self.redis_browser {
                        redis_browser.load_key(connection_id, database_index, key.clone());
                    }
                    self.active_panel = ActivePanel::RedisBrowser;
                    self.state.set_status(format!(
                        "View Redis key {key} (db={:?}, conn={})",
                        database, connection_id
                    ));
                }
                ObjectTreeAction::ViewStructure {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    self.query_editor.sync_active_context(
                        connection_id,
                        database.clone(),
                        schema.clone(),
                    );
                    self.state.set_status(format!(
                        "Loading structure for {}.{} ...",
                        schema.as_deref().unwrap_or(""),
                        table
                    ));
                    self.spawn_structure_load(connection_id, database, schema, table);
                    self.active_panel = ActivePanel::TableStructure;
                }
                ObjectTreeAction::ImportCsv {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    let wizard = ImportWizard::new(
                        connection_id,
                        database,
                        schema,
                        table,
                        self.metadata_service.clone(),
                        self.query_service.clone(),
                        self.connection_manager.clone(),
                        self.runtime.clone(),
                    );
                    let table_name = wizard.table_name();
                    self.import_wizard = Some(wizard);
                    self.active_panel = ActivePanel::ImportWizard;
                    self.state
                        .set_status(format!("Import CSV to {}", table_name));
                }
                ObjectTreeAction::ExportTableData {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    let table_name =
                        qualified_table_label(schema.as_deref().or(database.as_deref()), &table);
                    self.export_request = Some(TableExportRequest {
                        connection_id,
                        database,
                        schema,
                        table,
                        format: ExportFormat::Csv,
                    });
                    self.state
                        .set_status(format!("Export table: {}", table_name));
                }
                ObjectTreeAction::DropTable {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    let table_name =
                        qualified_table_label(schema.as_deref().or(database.as_deref()), &table);
                    self.drop_table_request = Some(DropTableRequest {
                        connection_id,
                        database,
                        schema,
                        table,
                    });
                    self.state
                        .set_status(format!("Confirm drop table: {}", table_name));
                }
                ObjectTreeAction::ClearTable {
                    connection_id,
                    database,
                    schema,
                    table,
                } => {
                    let table_name =
                        qualified_table_label(schema.as_deref().or(database.as_deref()), &table);
                    self.clear_request = Some(ClearRequest::Table {
                        connection_id,
                        database,
                        schema,
                        table,
                    });
                    self.state
                        .set_status(format!("Confirm clear table: {table_name}"));
                }
                ObjectTreeAction::ClearRedisDatabase {
                    connection_id,
                    database,
                } => {
                    self.clear_request = Some(ClearRequest::RedisDatabase {
                        connection_id,
                        database,
                    });
                    self.state
                        .set_status(format!("Confirm clear Redis DB {database}"));
                }
                ObjectTreeAction::ExportDatabase {
                    connection_id,
                    database,
                    mode,
                } => {
                    self.request_database_export(connection_id, database, mode);
                }
                ObjectTreeAction::ImportTable { connection_id } => {
                    self.import_table_request = Some(ImportTableRequest {
                        connection_id,
                        database: String::new(),
                        schema: String::new(),
                        table: String::new(),
                    });
                    self.state.set_status("Choose import target table");
                }
                ObjectTreeAction::EditConnection(id) => {
                    self.connection_manager_ui.edit_connection(id);
                    self.state.set_status(format!("Edit connection: {}", id));
                }
                ObjectTreeAction::DeleteConnection(id) => {
                    self.connection_manager_ui.delete_connection_prompt(id);
                    self.state.set_status(format!("Delete connection: {}", id));
                }
                ObjectTreeAction::DeleteRedisKey {
                    connection_id,
                    database,
                    key,
                } => {
                    let database_index = database
                        .as_deref()
                        .and_then(|value| value.parse::<u8>().ok())
                        .unwrap_or(0);
                    self.ensure_redis_browser(connection_id);
                    if let Some(redis_browser) = &mut self.redis_browser {
                        redis_browser.prompt_delete_key(connection_id, database_index, key.clone());
                    }
                    self.active_panel = ActivePanel::RedisBrowser;
                    self.state.set_status(format!("Delete Redis key {}", key));
                }
            }
        }
    }

    pub(super) fn connection_windows(&mut self, ctx: &Context) {
        if let Some(message) = self.connection_manager_ui.ui_window(ctx) {
            if !message.is_empty() {
                self.state.set_status(message);
            }
        }
        for change in self.connection_manager_ui.take_changes() {
            self.handle_connection_change(change);
        }
    }

    fn handle_connection_change(&mut self, change: ConnectionChange) {
        self.object_tree.reload_root();
        match change {
            ConnectionChange::Saved(_) => {
                self.state
                    .set_status("Connection saved; object tree refreshed");
            }
            ConnectionChange::Deleted(id) => {
                if self
                    .redis_browser
                    .as_ref()
                    .is_some_and(|browser| browser.connection_id() == id)
                {
                    self.redis_browser = None;
                    if matches!(self.active_panel, ActivePanel::RedisBrowser) {
                        self.active_panel = ActivePanel::QueryEditor;
                    }
                }
                self.state
                    .set_status("Connection deleted; object tree refreshed");
            }
        }
    }

    fn ensure_redis_browser(&mut self, connection_id: ConnectionId) {
        let needs_new = self
            .redis_browser
            .as_ref()
            .map(|browser| browser.connection_id() != connection_id)
            .unwrap_or(true);

        if needs_new {
            self.redis_browser = Some(RedisBrowserUi::new(
                connection_id,
                self.connection_manager.clone(),
                self.metadata_service.clone(),
                self.redis_service.clone(),
                self.runtime.clone(),
            ));
        }
    }
}
