use app_core::{
    connection::ConnectionManager, query::QueryService, storage::FileConnectionProfileStore,
};
use secure_store::EncryptedFileStore;
use std::{path::PathBuf, sync::Arc};

fn main() {
    tracing::info!("desktop application bootstrap");

    let app_name = app_core::app_name();
    let connection_manager = bootstrap_connection_manager(default_config_dir());
    let _query_service = QueryService::with_default_executors(connection_manager.clone());
    println!("{app_name} desktop skeleton");
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
