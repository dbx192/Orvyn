use crate::{validation, ConnectionId, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

        Ok(())
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

    pub fn extra_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.profile.extra_params.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> Result<ConnectionProfile> {
        self.profile.validate()?;
        Ok(self.profile)
    }
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
}
