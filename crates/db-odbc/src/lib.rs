//! Generic ODBC bridge adapter.
//!
//! Covers data sources without a pure-Rust wire-protocol driver (Oracle,
//! DM/达梦, XuguDb/虚谷, MS Access) by delegating to vendor ODBC drivers
//! through the platform driver manager (`odbc32.dll` on Windows) — the same
//! "universal driver bridge" idea DBeaver implements over JDBC. All odbc-api
//! calls are synchronous, so every connect / query / catalog operation runs
//! inside `tokio::task::spawn_blocking`.

use db_common::{AppError, ErrorKind};
use odbc_api::Error as OdbcError;

pub mod connect;
pub mod executor;
pub mod kind;
pub mod metadata;
pub mod tester;
pub mod value;

pub use executor::OdbcQueryExecutor;
pub use kind::VendorDialect;
pub use metadata::OdbcMetadataReader;
pub use tester::OdbcConnectionTester;

pub const ADAPTER_NAME: &str = "odbc";

/// SQLSTATE prefix meaning the driver manager could not find/load a driver.
const DRIVER_NOT_FOUND_STATES: [&str; 4] = ["IM001", "IM002", "IM003", "IM004"];
/// Driver / application architecture (32 vs 64 bit) mismatch.
const ARCH_MISMATCH_STATES: [&str; 2] = ["IM014", "IM015"];
/// Authentication rejected by the data source.
const AUTH_STATES: [&str; 3] = ["28000", "01017", "08004"];
/// Login / query timeouts.
const TIMEOUT_STATES: [&str; 2] = ["HYT00", "HYT01"];

/// Map an [`odc_api::Error`] into an [`AppError`], turning driver-manager
/// SQLSTATEs into actionable messages. `driver_name` is the driver the
/// connection string asked for, so "not installed" errors can name it.
pub fn map_odbc_error(
    dialect: VendorDialect,
    driver_name: &str,
    error: OdbcError,
) -> AppError {
    let label = dialect.display_label();
    match error {
        OdbcError::Diagnostics { record, function } => {
            let state = record.state.as_str().to_string();
            let detail = record.to_string();
            if DRIVER_NOT_FOUND_STATES.contains(&state.as_str()) {
                return AppError::new(
                    ErrorKind::Connection,
                    format!(
                        "{label} ODBC driver is not installed or not visible to this \
                         process. Install the vendor driver and retry. Looked for \
                         driver: {driver_name:?} (override via the ODBC driver name \
                         or raw connection string options)."
                    ),
                )
                .with_code(state)
                .with_detail(detail);
            }
            if ARCH_MISMATCH_STATES.contains(&state.as_str()) {
                return AppError::new(
                    ErrorKind::Connection,
                    format!(
                        "ODBC driver and application architecture mismatch ({state}). \
                         This application is 64-bit; install the 64-bit {label} ODBC \
                         driver."
                    ),
                )
                .with_code(state)
                .with_detail(detail);
            }
            if AUTH_STATES.contains(&state.as_str()) {
                return AppError::new(
                    ErrorKind::Authentication,
                    format!("{label} rejected the credentials"),
                )
                .with_code(state)
                .with_detail(detail);
            }
            if TIMEOUT_STATES.contains(&state.as_str()) {
                return AppError::new(ErrorKind::Timeout, format!("{label} operation timed out"))
                    .with_code(state)
                    .with_detail(detail);
            }
            AppError::new(ErrorKind::Query, format!("{label} query failed"))
                .with_code(state)
                .with_detail(format!("{function}: {detail}"))
        }
        OdbcError::OracleOdbcDriverDoesNotSupport64Bit(record) => AppError::new(
            ErrorKind::Query,
            "Oracle ODBC driver reported an invalid SQL data type. This usually \
             means the driver does not support the requested 64-bit fetch; retry \
             or update the Oracle Instant Client ODBC driver.",
        )
        .with_detail(record.to_string()),
        error => AppError::new(ErrorKind::Connection, "ODBC operation failed")
            .with_detail(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_not_found_maps_to_friendly_message() {
        // Constructing a real Diagnostics error requires a live ODBC handle,
        // so exercise the state-matching sets directly instead.
        assert!(DRIVER_NOT_FOUND_STATES.contains(&"IM002"));
        assert!(ARCH_MISMATCH_STATES.contains(&"IM014"));
        assert!(AUTH_STATES.contains(&"01017"));
        assert!(TIMEOUT_STATES.contains(&"HYT00"));
    }
}
