use crate::{validation, AppError, ConnectionId, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DataSourceKind {
    MySql,
    PostgreSql,
    Sqlite,
    DuckDb,
    Redis,
}

impl DataSourceKind {
    pub const fn default_port(self) -> Option<u16> {
        match self {
            Self::MySql => Some(3306),
            Self::PostgreSql => Some(5432),
            Self::Redis => Some(6379),
            Self::Sqlite | Self::DuckDb => None,
        }
    }

    pub const fn is_file_based(self) -> bool {
        matches!(self, Self::Sqlite | Self::DuckDb)
    }

    pub const fn is_relational(self) -> bool {
        !matches!(self, Self::Redis)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MySql => "mysql",
            Self::PostgreSql => "postgresql",
            Self::Sqlite => "sqlite",
            Self::DuckDb => "duckdb",
            Self::Redis => "redis",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRef {
    pub provider: String,
    pub key: String,
}

impl CredentialRef {
    pub fn new(provider: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            key: key.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl Default for SslMode {
    fn default() -> Self {
        Self::Prefer
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionSecurity {
    pub ssl_mode: SslMode,
    pub read_only: bool,
    pub save_password: bool,
}

impl Default for ConnectionSecurity {
    fn default() -> Self {
        Self {
            ssl_mode: SslMode::Prefer,
            read_only: false,
            save_password: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: ConnectionId,
    pub name: String,
    pub group: Option<String>,
    pub kind: DataSourceKind,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password_ref: Option<CredentialRef>,
    pub database: Option<String>,
    pub file_path: Option<String>,
    pub security: ConnectionSecurity,
    pub extra_params: BTreeMap<String, String>,
}

impl ConnectionProfile {
    pub fn builder(kind: DataSourceKind, name: impl Into<String>) -> ConnectionProfileBuilder {
        ConnectionProfileBuilder::new(kind, name)
    }

    pub fn validate(&self) -> Result<()> {
        validation::ensure_not_blank("connection name", &self.name)?;

        if self.kind.is_file_based() {
            validation::ensure_file_path(
                "file_path",
                self.file_path.as_deref().unwrap_or_default(),
            )?;
            return Ok(());
        }

        validation::ensure_not_blank("host", self.host.as_deref().unwrap_or_default())?;
        validation::ensure_port(self.port.unwrap_or_default())?;

        if matches!(
            self.kind,
            DataSourceKind::MySql | DataSourceKind::PostgreSql
        ) {
            validation::ensure_not_blank("username", self.username.as_deref().unwrap_or_default())?;
        }

        Ok(())
    }

    pub fn required_host(&self) -> Result<&str> {
        self.host
            .as_deref()
            .filter(|host| !host.trim().is_empty())
            .ok_or_else(|| AppError::validation("host cannot be blank"))
    }

    pub fn required_port(&self) -> Result<u16> {
        self.port
            .filter(|port| *port > 0)
            .ok_or_else(|| AppError::validation("port must be between 1 and 65535"))
    }

    pub fn required_file_path(&self) -> Result<&str> {
        self.file_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| AppError::validation("file_path cannot be blank"))
    }

    pub fn connection_label(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.name)
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionProfileBuilder {
    profile: ConnectionProfile,
}

impl ConnectionProfileBuilder {
    pub fn new(kind: DataSourceKind, name: impl Into<String>) -> Self {
        Self {
            profile: ConnectionProfile {
                id: ConnectionId::new(),
                name: name.into(),
                group: None,
                kind,
                host: None,
                port: kind.default_port(),
                username: None,
                password_ref: None,
                database: None,
                file_path: None,
                security: ConnectionSecurity::default(),
                extra_params: BTreeMap::new(),
            },
        }
    }

    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.profile.host = Some(host.into());
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.profile.port = Some(port);
        self
    }

    pub fn username(mut self, username: impl Into<String>) -> Self {
        self.profile.username = Some(username.into());
        self
    }

    pub fn database(mut self, database: impl Into<String>) -> Self {
        self.profile.database = Some(database.into());
        self
    }

    pub fn file_path(mut self, file_path: impl Into<String>) -> Self {
        self.profile.file_path = Some(file_path.into());
        self
    }

    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.profile.group = Some(group.into());
        self
    }

    pub fn password_ref(mut self, credential_ref: CredentialRef) -> Self {
        self.profile.password_ref = Some(credential_ref);
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.profile.security.read_only = read_only;
        self
    }

    pub fn save_password(mut self, save_password: bool) -> Self {
        self.profile.security.save_password = save_password;
        self
    }

    pub fn ssl_mode(mut self, ssl_mode: SslMode) -> Self {
        self.profile.security.ssl_mode = ssl_mode;
        self
    }

    pub fn extra_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.profile.extra_params.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> Result<ConnectionProfile> {
        self.profile.validate()?;
        Ok(self.profile)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    NotConnected,
    Connecting,
    Connected,
    Failed,
    Disconnected,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self::NotConnected
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionTestReport {
    pub kind: DataSourceKind,
    pub success: bool,
    pub elapsed_ms: u128,
    pub server_version: Option<String>,
    pub message: String,
}

impl ConnectionTestReport {
    pub fn success(kind: DataSourceKind, elapsed_ms: u128) -> Self {
        Self {
            kind,
            success: true,
            elapsed_ms,
            server_version: None,
            message: "connection test succeeded".to_string(),
        }
    }

    pub fn with_server_version(mut self, server_version: impl Into<String>) -> Self {
        self.server_version = Some(server_version.into());
        self
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PasswordUpdate {
    Keep,
    Clear,
    Replace(String),
}

impl fmt::Debug for PasswordUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keep => formatter.write_str("Keep"),
            Self::Clear => formatter.write_str("Clear"),
            Self::Replace(_) => formatter.write_str("Replace(********)"),
        }
    }
}

pub fn unsupported_kind(kind: DataSourceKind) -> AppError {
    AppError::new(
        ErrorKind::Unsupported,
        format!("{} is not supported by this operation", kind.as_str()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_tcp_connection() {
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "local")
            .host("localhost")
            .username("postgres")
            .build();

        assert!(profile.is_ok());
    }

    #[test]
    fn rejects_file_connection_without_path() {
        let profile = ConnectionProfile::builder(DataSourceKind::Sqlite, "local").build();
        assert!(profile.is_err());
    }

    #[test]
    fn rejects_postgres_without_username() {
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "local")
            .host("localhost")
            .build();

        assert!(profile.is_err());
    }

    #[test]
    fn redacts_password_update_debug() {
        assert_eq!(
            format!("{:?}", PasswordUpdate::Replace("secret".to_string())),
            "Replace(********)"
        );
    }
}
