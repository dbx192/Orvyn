#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod config;
mod ui;

use app_core::{
    connection::ConnectionManager,
    metadata::MetadataService,
    query::QueryService,
    redis_service::RedisService,
    session::SessionManager,
    storage::{FileConnectionProfileStore, FileFavoriteQueryStore, FileQueryHistoryStore},
};
use eframe::egui::{vec2, ViewportBuilder};
use eframe::NativeOptions;
use secure_store::EncryptedFileStore;
use std::{path::PathBuf, sync::Arc};
use tokio::runtime::Runtime;
use ui::app::DesktopApp;

fn main() -> eframe::Result<()> {
    tracing::info!("desktop application bootstrap");

    let app_name = app_core::app_name();
    let runtime = Arc::new(
        Runtime::new().expect("failed to create background async runtime for desktop app"),
    );
    let config_dir = config::data_dir();
    ui::i18n::initialize(&config_dir);
    let connection_manager = bootstrap_connection_manager(config_dir.clone());
    let session_manager = SessionManager::with_default_testers(connection_manager.clone());
    let metadata_service = MetadataService::with_default_readers(connection_manager.clone());
    let query_service = QueryService::with_stores(
        connection_manager.clone(),
        vec![
            Arc::new(db_mysql::MySqlQueryExecutor::new()),
            Arc::new(db_postgres::PostgresQueryExecutor::new()),
            Arc::new(db_sqlite::SqliteQueryExecutor::new()),
            Arc::new(db_duckdb::DuckDbQueryExecutor::new()),
        ],
        db_common::ConnectionPoolConfig::default(),
        app_core::task::TaskManager::default(),
        Arc::new(FileQueryHistoryStore::new(
            config_dir.join("query-history.json"),
        )),
        Arc::new(FileFavoriteQueryStore::new(
            config_dir.join("favorite-queries.json"),
        )),
    );
    let redis_service = RedisService::new(connection_manager.clone());
    let native_options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size(vec2(1280.0, 800.0))
            .with_min_inner_size(vec2(960.0, 640.0)),
        ..Default::default()
    };
    let runtime_handle = runtime.handle().clone();

    eframe::run_native(
        app_name,
        native_options,
        Box::new(move |cc| {
            ui::fonts::install_global_fonts(&cc.egui_ctx);
            Box::new(DesktopApp::new(
                app_name.to_string(),
                connection_manager,
                metadata_service,
                session_manager,
                query_service,
                redis_service,
                runtime_handle.clone(),
            ))
        }),
    )
}

fn bootstrap_connection_manager(config_dir: PathBuf) -> ConnectionManager {
    let profiles_path = config_dir.join("connections.json");
    let secrets_path = config_dir.join("secrets.json");
    let master_key =
        config::master_key(&config_dir).expect("failed to initialize Orvyn master key");

    ConnectionManager::with_default_testers(
        Arc::new(FileConnectionProfileStore::new(profiles_path)),
        Arc::new(EncryptedFileStore::new(secrets_path, master_key)),
    )
}
