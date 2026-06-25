mod ui;

use app_core::{
    connection::ConnectionManager, metadata::MetadataService, query::QueryService,
    redis_service::RedisService, session::SessionManager, storage::FileConnectionProfileStore,
};
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
    let connection_manager = bootstrap_connection_manager(default_config_dir());
    let session_manager = SessionManager::with_default_testers(connection_manager.clone());
    let metadata_service = MetadataService::with_default_readers(connection_manager.clone());
    let query_service = QueryService::with_default_executors(connection_manager.clone());
    let redis_service = RedisService::new(connection_manager.clone());
    let native_options = NativeOptions::default();
    let runtime_handle = runtime.handle().clone();

    eframe::run_native(
        app_name,
        native_options,
        Box::new(move |_| {
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
    let master_key = std::env::var("RUST_DB_CLIENT_MASTER_KEY")
        .unwrap_or_else(|_| "development-master-key-change-me".to_string());

    ConnectionManager::with_default_testers(
        Arc::new(FileConnectionProfileStore::new(profiles_path)),
        Arc::new(EncryptedFileStore::new(secrets_path, master_key)),
    )
}

fn default_config_dir() -> PathBuf {
    std::env::var_os("RUST_DB_CLIENT_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".rust-db-client"))
}
