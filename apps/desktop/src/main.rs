#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod config;
mod ui;

use app_core::{
    connection::ConnectionManager,
    metadata::MetadataService,
    query::QueryService,
    redis_service::RedisService,
    search_service::SearchService,
    session::SessionManager,
    storage::{FileConnectionProfileStore, FileFavoriteQueryStore, FileQueryHistoryStore},
    DriverRegistry,
};
use eframe::egui::{vec2, ViewportBuilder};
use eframe::NativeOptions;
use secure_store::EncryptedFileStore;
use std::{path::PathBuf, sync::Arc};
use tokio::runtime::Runtime;
use ui::app::DesktopApp;

fn init_logging(config_dir: &std::path::Path) -> tracing_appender::non_blocking::WorkerGuard {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info,sqlx=warn".to_string());
    let file_appender = tracing_appender::rolling::never(config_dir, "orvyn.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
    let env_filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_ansi(true))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .init();
    guard
}

fn main() -> eframe::Result<()> {
    let app_name = app_core::app_name();
    let config_dir = config::data_dir();
    if let Err(error) = std::fs::create_dir_all(&config_dir) {
        eprintln!(
            "failed to create log directory {}: {error}",
            config_dir.display()
        );
    }
    let _log_guard = init_logging(&config_dir);
    tracing::info!(log_file = %config_dir.join("orvyn.log").display(), "desktop application bootstrap");
    let runtime = Arc::new(
        Runtime::new().expect("failed to create background async runtime for desktop app"),
    );
    ui::i18n::initialize(&config_dir);
    let registry = DriverRegistry::default();
    let connection_manager = bootstrap_connection_manager(config_dir.clone(), registry.clone());
    let session_manager =
        SessionManager::with_registry(connection_manager.clone(), registry.clone());
    let metadata_service = MetadataService::with_registry(
        connection_manager.clone(),
        registry.clone(),
        app_core::task::TaskManager::default(),
    );
    let query_service = QueryService::with_registry(
        connection_manager.clone(),
        registry.clone(),
        db_common::ConnectionPoolConfig::default(),
        app_core::task::TaskManager::default(),
        Some(Arc::new(FileQueryHistoryStore::new(
            config_dir.join("query-history.json"),
        ))),
        Some(Arc::new(FileFavoriteQueryStore::new(
            config_dir.join("favorite-queries.json"),
        ))),
    );
    let redis_service = RedisService::new(connection_manager.clone());
    let search_service = SearchService::new(connection_manager.clone(), Arc::new(registry));
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
                search_service,
                runtime_handle.clone(),
            ))
        }),
    )
}

fn bootstrap_connection_manager(
    config_dir: PathBuf,
    registry: DriverRegistry,
) -> ConnectionManager {
    let profiles_path = config_dir.join("connections.json");
    let secrets_path = config_dir.join("secrets.json");
    let master_key =
        config::master_key(&config_dir).expect("failed to initialize Orvyn master key");

    ConnectionManager::with_registry(
        Arc::new(FileConnectionProfileStore::new(profiles_path)),
        Arc::new(EncryptedFileStore::new(secrets_path, master_key)),
        registry,
    )
}
