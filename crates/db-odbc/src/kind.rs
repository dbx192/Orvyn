//! Vendor-specific connection details for the ODBC bridge.
//!
//! All per-brand quirks (driver name, connection-string keywords, default
//! schema, identifier quoting) live here so the rest of the crate is generic.
//! Connection-string keyword differences between driver versions are the most
//! likely thing to need a fix; it is a one-line change in `connection_string`.

use db_common::{AppError, ConnectionProfile, DataSourceKind, ErrorKind, Result};
use odbc_api::escape_attribute_value;
use std::borrow::Cow;

/// Extra param holding the vendor driver name override.
pub const ODBC_DRIVER_PARAM: &str = "odbc_driver";
/// Extra param holding a raw connection string; overrides everything else.
pub const ODBC_CONNECTION_STRING_PARAM: &str = "odbc_connection_string";
/// Oracle service name (shared with historical config-only profiles).
pub const ORACLE_SERVICE_PARAM: &str = "service_name";
/// Oracle SID, alternative to the service name.
pub const ORACLE_SID_PARAM: &str = "oracle_sid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorDialect {
    Oracle,
    Dm,
    Xugu,
    Access,
}

impl VendorDialect {
    pub fn of(kind: DataSourceKind) -> Result<Self> {
        match kind {
            DataSourceKind::Oracle => Ok(Self::Oracle),
            DataSourceKind::Dm => Ok(Self::Dm),
            DataSourceKind::XuguDb => Ok(Self::Xugu),
            DataSourceKind::Access => Ok(Self::Access),
            other => Err(AppError::new(
                ErrorKind::Unsupported,
                format!("{} is not served by the ODBC bridge", other.display_label()),
            )),
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            Self::Oracle => "Oracle",
            Self::Dm => "DM (达梦)",
            Self::Xugu => "XuguDB (虚谷)",
            Self::Access => "MS Access",
        }
    }

    /// Driver name used when the profile does not override `odbc_driver`.
    /// These are conventional names; users almost always need to override
    /// Oracle's (it embeds the client home name) via the extra param.
    pub fn default_driver_name(self) -> &'static str {
        match self {
            Self::Oracle => "Oracle in OraDB21Home1",
            Self::Dm => "DM ODBC DRIVER",
            Self::Xugu => "Xugu ODBC Driver",
            Self::Access => "Microsoft Access Driver (*.mdb, *.accdb)",
        }
    }

    /// Whether the DBMS organizes objects into schemas below a catalog.
    pub fn supports_schemas(self) -> bool {
        !matches!(self, Self::Access)
    }

    /// Whether ODBC-level transactions are usable. Jet (Access) via ODBC has
    /// no reliable transaction support.
    pub fn supports_transactions(self) -> bool {
        !matches!(self, Self::Access)
    }

    /// Default schema for catalog calls. `None` means the driver's default.
    pub fn default_schema(self, profile: &ConnectionProfile) -> Option<String> {
        match self {
            Self::Access => None,
            // Oracle stores unquoted identifiers uppercased.
            Self::Oracle => profile
                .username
                .as_deref()
                .map(|user| user.to_uppercase()),
            Self::Dm | Self::Xugu => profile.username.clone(),
        }
    }

    /// Build a DSN-less connection string from the profile. A raw
    /// `odbc_connection_string` extra param wins over everything.
    pub fn connection_string(
        self,
        profile: &ConnectionProfile,
        password: Option<&str>,
    ) -> Result<String> {
        if let Some(raw) = profile.extra_param(ODBC_CONNECTION_STRING_PARAM) {
            if !raw.trim().is_empty() {
                return Ok(raw.to_string());
            }
        }

        let driver = profile
            .extra_param(ODBC_DRIVER_PARAM)
            .filter(|name| !name.trim().is_empty())
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Owned(self.default_driver_name().to_string()));
        let mut cs = format!("Driver={{{driver}}};");

        match self {
            Self::Oracle => {
                let host = profile.required_host()?;
                let port = profile.required_port()?;
                let endpoint = if let Some(sid) = non_empty(profile.extra_param(ORACLE_SID_PARAM)) {
                    format!("{host}:{port}:{sid}")
                } else {
                    let service = non_empty(profile.extra_param(ORACLE_SERVICE_PARAM))
                        .or_else(|| non_empty(profile.database.as_deref()))
                        .map(str::to_string);
                    match service {
                        Some(service) => format!("{host}:{port}/{service}"),
                        None => format!("{host}:{port}"),
                    }
                };
                cs.push_str(&format!("DBQ={endpoint};"));
                if let Some(user) = non_empty(profile.username.as_deref()) {
                    cs.push_str(&format!("UID={};", escape_attribute_value(user)));
                }
            }
            Self::Dm => {
                let host = profile.required_host()?;
                let port = profile.required_port()?;
                cs.push_str(&format!("SERVER={host};PORT={port};"));
                if let Some(db) = non_empty(profile.database.as_deref()) {
                    cs.push_str(&format!("DATABASE={};", escape_attribute_value(db)));
                }
                if let Some(user) = non_empty(profile.username.as_deref()) {
                    cs.push_str(&format!("UID={};", escape_attribute_value(user)));
                }
            }
            Self::Xugu => {
                let host = profile.required_host()?;
                let port = profile.required_port()?;
                cs.push_str(&format!("SERVER={host};PORT={port};"));
                if let Some(db) = non_empty(profile.database.as_deref()) {
                    cs.push_str(&format!("DATABASE={};", escape_attribute_value(db)));
                }
                if let Some(user) = non_empty(profile.username.as_deref()) {
                    cs.push_str(&format!("UID={};", escape_attribute_value(user)));
                }
            }
            Self::Access => {
                let path = profile.required_file_path()?;
                cs.push_str(&format!("DBQ={};", escape_attribute_value(path)));
            }
        }

        if let Some(password) = password.filter(|pwd| !pwd.is_empty()) {
            cs.push_str(&format!("PWD={};", escape_attribute_value(password)));
        }
        Ok(cs)
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle_profile() -> ConnectionProfile {
        ConnectionProfile::builder(DataSourceKind::Oracle, "prod")
            .host("db.example.test")
            .port(1521)
            .username("scott")
            .extra_param("service_name", "orcl")
            .build()
            .expect("profile")
    }

    #[test]
    fn oracle_connection_string_uses_service_name() {
        let profile = oracle_profile();
        let cs = VendorDialect::Oracle
            .connection_string(&profile, Some("tiger"))
            .expect("connection string");
        assert_eq!(
            cs,
            "Driver={Oracle in OraDB21Home1};DBQ=db.example.test:1521/orcl;UID=scott;PWD=tiger;"
        );
    }

    #[test]
    fn oracle_sid_takes_precedence_and_password_is_escaped() {
        let profile = ConnectionProfile::builder(DataSourceKind::Oracle, "prod")
            .host("db")
            .port(1521)
            .username("u")
            .extra_param("oracle_sid", "XE")
            .extra_param(ODBC_DRIVER_PARAM, "Oracle in MyHome")
            .build()
            .expect("profile");
        let cs = VendorDialect::Oracle
            .connection_string(&profile, Some("p+w;d"))
            .expect("connection string");
        assert_eq!(
            cs,
            "Driver={Oracle in MyHome};DBQ=db:1521:XE;UID=u;PWD={p+w;d};"
        );
    }

    #[test]
    fn raw_connection_string_overrides_everything() {
        let profile = ConnectionProfile::builder(DataSourceKind::Oracle, "prod")
            .host("db")
            .port(1521)
            .username("u")
            .extra_param(ODBC_CONNECTION_STRING_PARAM, "DSN=my oracle")
            .build()
            .expect("profile");
        let cs = VendorDialect::Oracle
            .connection_string(&profile, None)
            .expect("connection string");
        assert_eq!(cs, "DSN=my oracle");
    }

    #[test]
    fn access_uses_file_path_and_has_no_schema() {
        let profile = ConnectionProfile::builder(DataSourceKind::Access, "northwind")
            .file_path(r"C:\data\northwind.accdb")
            .build()
            .expect("profile");
        let dialect = VendorDialect::Access;
        let cs = dialect
            .connection_string(&profile, None)
            .expect("connection string");
        assert!(cs.starts_with("Driver={Microsoft Access Driver (*.mdb, *.accdb)};DBQ=C:\\data\\northwind.accdb;"));
        assert_eq!(dialect.default_schema(&profile), None);
        assert!(!dialect.supports_schemas());
        assert!(!dialect.supports_transactions());
    }

    #[test]
    fn oracle_default_schema_is_uppercased_username() {
        let profile = oracle_profile();
        assert_eq!(
            VendorDialect::Oracle.default_schema(&profile).as_deref(),
            Some("SCOTT")
        );
        assert!(VendorDialect::Dm.supports_schemas());
        assert!(VendorDialect::Xugu.supports_transactions());
    }
}
