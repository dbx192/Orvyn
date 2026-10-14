//! Connection establishment shared by the tester, executor and metadata
//! reader. Every entry point is `async` and wraps the synchronous odbc-api
//! calls in `tokio::task::spawn_blocking`.

use crate::kind::{VendorDialect, ODBC_DRIVER_PARAM};
use db_common::{AppError, ConnectionProfile, DataSourceKind, ErrorKind, Result};
use odbc_api::{Connection, ConnectionOptions};

/// Login timeout handed to the driver manager so a dead host cannot hang the
/// UI forever (drivers may enforce their own minimum).
pub const LOGIN_TIMEOUT_SEC: u32 = 15;

/// Resolve the driver name the connection string will use (for error text).
pub fn driver_name_for(profile: &ConnectionProfile) -> String {
    profile
        .extra_param(ODBC_DRIVER_PARAM)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("vendor default")
        .to_string()
}

/// Map an [`odbc_api::Error`] for a profile of the given kind.
pub fn map_odbc_error_for(
    kind: DataSourceKind,
    driver_name: &str,
    error: odbc_api::Error,
) -> AppError {
    match VendorDialect::of(kind) {
        Ok(dialect) => crate::map_odbc_error(dialect, driver_name, error),
        Err(_) => AppError::new(ErrorKind::Connection, "ODBC operation failed")
            .with_detail(error.to_string()),
    }
}

/// Synchronous connect. Runs on a blocking thread: either through the async
/// [`connect`] wrapper or directly inside a caller-owned `spawn_blocking`
/// closure (the executor's pooled connections reconnect this way).
pub fn connect_blocking(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<Connection<'static>> {
    let kind = profile.kind;
    let dialect = VendorDialect::of(kind)?;
    let connection_string = dialect.connection_string(profile, password)?;
    let driver_name = driver_name_for(profile);
    let options = ConnectionOptions {
        login_timeout_sec: Some(LOGIN_TIMEOUT_SEC),
        ..ConnectionOptions::default()
    };

    let environment = odbc_api::environment()
        .map_err(|error| crate::map_odbc_error(dialect, "environment", error))?;
    environment
        .connect_with_connection_string(&connection_string, options)
        .map_err(|error| map_odbc_error_for(kind, &driver_name, error))
}

/// Open a `'static` ODBC connection on the blocking pool.
pub async fn connect(
    profile: &ConnectionProfile,
    password: Option<&str>,
) -> Result<Connection<'static>> {
    let profile = profile.clone();
    let password = password.map(str::to_string);
    tokio::task::spawn_blocking(move || connect_blocking(&profile, password.as_deref()))
        .await
        .map_err(|join| AppError::internal(format!("ODBC blocking task failed: {join}")))?
}

/// Run a blocking closure against a fresh connection. Metadata reads use this;
/// the executor keeps pooled connections instead.
pub async fn with_connection<T, F>(
    profile: &ConnectionProfile,
    password: Option<&str>,
    f: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Connection<'static>) -> Result<T> + Send + 'static,
{
    let connection = connect(profile, password).await?;
    tokio::task::spawn_blocking(move || f(&connection))
        .await
        .map_err(|join| AppError::internal(format!("ODBC blocking task failed: {join}")))?
}
