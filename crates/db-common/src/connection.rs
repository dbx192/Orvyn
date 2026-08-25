use crate::{validation, AppError, ConnectionId, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum DataSourceKind {
    // MySQL protocol family
    MySql,
    MariaDb,
    TiDb,
    OceanBase,
    Doris,
    SelectDb,
    StarRocks,
    GoldenDb,
    // PostgreSQL protocol family
    PostgreSql,
    TimescaleDb,
    Redshift,
    CockroachDb,
    OpenGauss,
    GaussDb,
    KwDb,
    KingbaseEs,
    Vastbase,
    HighGo,
    UxDb,
    // Independent relational / analytical
    Sqlite,
    DuckDb,
    ClickHouse,
    SqlServer,
    TDengine,
    // Document / key-value / search
    MongoDb,
    Redis,
    Elasticsearch,
    Easysearch,
    Meilisearch,
    ManticoreSearch,
    CloudflareD1,
    // Restricted (no reliable pure-Rust driver)
    Access,
    Oracle,
    Dm,
    XuguDb,
}

impl DataSourceKind {
    /// All known kinds in a stable, family-grouped order. Used by registries
    /// and the UI type picker.
    pub const ALL: &'static [DataSourceKind] = &[
        // MySQL family
        Self::MySql,
        Self::MariaDb,
        Self::TiDb,
        Self::OceanBase,
        Self::Doris,
        Self::SelectDb,
        Self::StarRocks,
        Self::GoldenDb,
        // PostgreSQL family
        Self::PostgreSql,
        Self::TimescaleDb,
        Self::Redshift,
        Self::CockroachDb,
        Self::OpenGauss,
        Self::GaussDb,
        Self::KwDb,
        Self::KingbaseEs,
        Self::Vastbase,
        Self::HighGo,
        Self::UxDb,
        // Independent relational / analytical
        Self::Sqlite,
        Self::DuckDb,
        Self::ClickHouse,
        Self::SqlServer,
        Self::TDengine,
        // Document / key-value / search
        Self::MongoDb,
        Self::Redis,
        Self::Elasticsearch,
        Self::Easysearch,
        Self::Meilisearch,
        Self::ManticoreSearch,
        Self::CloudflareD1,
        // Restricted
        Self::Access,
        Self::Oracle,
        Self::Dm,
        Self::XuguDb,
    ];

    pub const fn default_port(self) -> Option<u16> {
        match self {
            // MySQL family
            Self::MySql | Self::MariaDb => Some(3306),
            Self::TiDb => Some(4000),
            Self::OceanBase | Self::GoldenDb => Some(2883),
            Self::Doris | Self::SelectDb | Self::StarRocks => Some(9030),
            // PostgreSQL family
            Self::PostgreSql
            | Self::TimescaleDb
            | Self::Redshift
            | Self::OpenGauss
            | Self::GaussDb
            | Self::KwDb
            | Self::Vastbase
            | Self::HighGo
            | Self::UxDb => Some(5432),
            Self::CockroachDb => Some(26257),
            Self::KingbaseEs => Some(54321),
            // Independent relational / analytical
            Self::SqlServer => Some(1433),
            Self::ClickHouse => Some(8123),
            Self::TDengine => Some(6041),
            Self::Oracle => Some(1521),
            Self::Dm => Some(5236),
            Self::XuguDb => Some(5136),
            // Document / key-value / search
            Self::MongoDb => Some(27017),
            Self::Redis => Some(6379),
            Self::Elasticsearch | Self::Easysearch => Some(9200),
            Self::Meilisearch => Some(7700),
            Self::ManticoreSearch => Some(9306),
            // File-based or HTTP-only (no TCP port)
            Self::Sqlite | Self::DuckDb | Self::Access | Self::CloudflareD1 => None,
        }
    }

    pub const fn is_file_based(self) -> bool {
        matches!(self, Self::Sqlite | Self::DuckDb | Self::Access)
    }

    pub const fn is_document(self) -> bool {
        matches!(self, Self::MongoDb)
    }

    pub const fn is_kv(self) -> bool {
        matches!(self, Self::Redis)
    }

    pub const fn is_search(self) -> bool {
        matches!(
            self,
            Self::Elasticsearch | Self::Easysearch | Self::Meilisearch | Self::ManticoreSearch
        )
    }

    pub const fn is_http_sql(self) -> bool {
        matches!(self, Self::CloudflareD1)
    }

    pub const fn is_restricted(self) -> bool {
        matches!(self, Self::Access | Self::Oracle | Self::Dm | Self::XuguDb)
    }

    /// Relational SQL databases with a query editor. Excludes document,
    /// key-value, search, HTTP-SQL and restricted types.
    pub const fn is_relational(self) -> bool {
        !(self.is_document()
            || self.is_kv()
            || self.is_search()
            || self.is_http_sql()
            || self.is_restricted())
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            // MySQL family
            Self::MySql => "mysql",
            Self::MariaDb => "mariadb",
            Self::TiDb => "tidb",
            Self::OceanBase => "oceanbase",
            Self::Doris => "doris",
            Self::SelectDb => "selectdb",
            Self::StarRocks => "starrocks",
            Self::GoldenDb => "goldendb",
            // PostgreSQL family
            Self::PostgreSql => "postgresql",
            Self::TimescaleDb => "timescaledb",
            Self::Redshift => "redshift",
            Self::CockroachDb => "cockroachdb",
            Self::OpenGauss => "opengauss",
            Self::GaussDb => "gaussdb",
            Self::KwDb => "kwdb",
            Self::KingbaseEs => "kingbasees",
            Self::Vastbase => "vastbase",
            Self::HighGo => "highgo",
            Self::UxDb => "uxdb",
            // Independent relational / analytical
            Self::Sqlite => "sqlite",
            Self::DuckDb => "duckdb",
            Self::ClickHouse => "clickhouse",
            Self::SqlServer => "sqlserver",
            Self::TDengine => "tdengine",
            // Document / key-value / search
            Self::MongoDb => "mongodb",
            Self::Redis => "redis",
            Self::Elasticsearch => "elasticsearch",
            Self::Easysearch => "easysearch",
            Self::Meilisearch => "meilisearch",
            Self::ManticoreSearch => "manticoresearch",
            Self::CloudflareD1 => "cloudflare_d1",
            // Restricted
            Self::Access => "access",
            Self::Oracle => "oracle",
            Self::Dm => "dm",
            Self::XuguDb => "xugudb",
        }
    }

    /// Human-friendly display label (title-cased brand name).
    pub const fn display_label(self) -> &'static str {
        match self {
            Self::MySql => "MySQL",
            Self::MariaDb => "MariaDB",
            Self::TiDb => "TiDB",
            Self::OceanBase => "OceanBase",
            Self::Doris => "Apache Doris",
            Self::SelectDb => "SelectDB",
            Self::StarRocks => "StarRocks",
            Self::GoldenDb => "GoldenDB",
            Self::PostgreSql => "PostgreSQL",
            Self::TimescaleDb => "TimescaleDB",
            Self::Redshift => "Redshift",
            Self::CockroachDb => "CockroachDB",
            Self::OpenGauss => "openGauss",
            Self::GaussDb => "GaussDB",
            Self::KwDb => "KWDB",
            Self::KingbaseEs => "KingbaseES",
            Self::Vastbase => "Vastbase",
            Self::HighGo => "HighGo",
            Self::UxDb => "UXDB",
            Self::Sqlite => "SQLite",
            Self::DuckDb => "DuckDB",
            Self::ClickHouse => "ClickHouse",
            Self::SqlServer => "SQL Server",
            Self::TDengine => "TDengine",
            Self::MongoDb => "MongoDB",
            Self::Redis => "Redis",
            Self::Elasticsearch => "Elasticsearch",
            Self::Easysearch => "Easysearch",
            Self::Meilisearch => "Meilisearch",
            Self::ManticoreSearch => "Manticore Search",
            Self::CloudflareD1 => "Cloudflare D1",
            Self::Access => "Microsoft Access",
            Self::Oracle => "Oracle",
            Self::Dm => "DM (达梦)",
            Self::XuguDb => "XuguDB (虚谷)",
        }
    }

    /// Parse a kind from its serialized form. Accepts both the variant name
    /// (e.g. `"MySql"`, the historical on-disk format) and the canonical
    /// `as_str()` value (e.g. `"mysql"`). Returns `None` for unknown strings.
    pub fn from_serialized(value: &str) -> Option<Self> {
        // First try the canonical as_str mapping; then fall back to the
        // PascalCase variant names used by serde's default derive.
        for &kind in Self::ALL {
            if kind.as_str() == value {
                return Some(kind);
            }
        }
        match value {
            "MySql" => Some(Self::MySql),
            "MariaDb" => Some(Self::MariaDb),
            "TiDb" => Some(Self::TiDb),
            "OceanBase" => Some(Self::OceanBase),
            "Doris" => Some(Self::Doris),
            "SelectDb" => Some(Self::SelectDb),
            "StarRocks" => Some(Self::StarRocks),
            "GoldenDb" => Some(Self::GoldenDb),
            "PostgreSql" => Some(Self::PostgreSql),
            "TimescaleDb" => Some(Self::TimescaleDb),
            "Redshift" => Some(Self::Redshift),
            "CockroachDb" => Some(Self::CockroachDb),
            "OpenGauss" => Some(Self::OpenGauss),
            "GaussDb" => Some(Self::GaussDb),
            "KwDb" => Some(Self::KwDb),
            "KingbaseEs" => Some(Self::KingbaseEs),
            "Vastbase" => Some(Self::Vastbase),
            "HighGo" => Some(Self::HighGo),
            "UxDb" => Some(Self::UxDb),
            "Sqlite" => Some(Self::Sqlite),
            "DuckDb" => Some(Self::DuckDb),
            "ClickHouse" => Some(Self::ClickHouse),
            "SqlServer" => Some(Self::SqlServer),
            "TDengine" => Some(Self::TDengine),
            "MongoDb" => Some(Self::MongoDb),
            "Redis" => Some(Self::Redis),
            "Elasticsearch" => Some(Self::Elasticsearch),
            "Easysearch" => Some(Self::Easysearch),
            "Meilisearch" => Some(Self::Meilisearch),
            "ManticoreSearch" => Some(Self::ManticoreSearch),
            "CloudflareD1" => Some(Self::CloudflareD1),
            "Access" => Some(Self::Access),
            "Oracle" => Some(Self::Oracle),
            "Dm" => Some(Self::Dm),
            "XuguDb" => Some(Self::XuguDb),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for DataSourceKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        DataSourceKind::from_serialized(&value).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown database kind '{value}'; supported kinds: mysql, mariadb, tidb, oceanbase, doris, selectdb, starrocks, goldendb, postgresql, timescaledb, redshift, cockroachdb, opengauss, gaussdb, kwdb, kingbasees, vastbase, highgo, uxdb, sqlite, duckdb, clickhouse, sqlserver, tdengine, mongodb, redis, elasticsearch, easysearch, meilisearch, manticoresearch, cloudflare_d1, access, oracle, dm, xugudb"
            ))
        })
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
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

        // Cloudflare D1 reaches the Cloudflare HTTP API using an account id,
        // database id and token; it has no host/port.
        if self.kind.is_http_sql() {
            let missing = match self.kind {
                DataSourceKind::CloudflareD1 => {
                    let mut missing = Vec::new();
                    if self.extra_param("cf_account").is_none() {
                        missing.push("cf_account");
                    }
                    if self.extra_param("cf_database_id").is_none() {
                        missing.push("cf_database_id");
                    }
                    missing
                }
                _ => Vec::new(),
            };
            if !missing.is_empty() {
                return Err(AppError::validation(format!(
                    "{} requires extra params: {}",
                    self.kind.display_label(),
                    missing.join(", ")
                )));
            }
            return Ok(());
        }

        validation::ensure_not_blank("host", self.host.as_deref().unwrap_or_default())?;
        validation::ensure_port(self.port.unwrap_or_default())?;

        // Relational TCP and document stores require a username. Search,
        // key-value and HTTP-SQL services authenticate via tokens/API keys.
        if (self.kind.is_relational() || self.kind.is_document()) && !self.kind.is_file_based() {
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

    /// Read a brand-specific extra parameter (TLS, HTTP path, Cloudflare
    /// account/token, MongoDB auth source, ClickHouse protocol, OceanBase
    /// mode, ES API keys, ...).
    pub fn extra_param(&self, key: &str) -> Option<&str> {
        self.extra_params.get(key).map(String::as_str)
    }

    pub fn set_extra_param(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.extra_params.insert(key.into(), value.into());
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    #[default]
    NotConnected,
    Connecting,
    Connected,
    Failed,
    Disconnected,
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

    #[test]
    fn supports_new_database_defaults() {
        assert_eq!(DataSourceKind::MongoDb.default_port(), Some(27017));
        assert_eq!(DataSourceKind::SqlServer.default_port(), Some(1433));
        assert_eq!(DataSourceKind::ClickHouse.default_port(), Some(8123));
        assert_eq!(DataSourceKind::TiDb.default_port(), Some(4000));
        assert_eq!(DataSourceKind::TimescaleDb.default_port(), Some(5432));
        assert!(!DataSourceKind::MongoDb.is_relational());
        assert!(DataSourceKind::TiDb.is_relational());
    }

    #[test]
    fn deserializes_legacy_variant_name_and_as_str() {
        // Historical on-disk format stored the PascalCase variant name.
        assert_eq!(
            DataSourceKind::from_serialized("MySql"),
            Some(DataSourceKind::MySql)
        );
        assert_eq!(
            DataSourceKind::from_serialized("TiDb"),
            Some(DataSourceKind::TiDb)
        );
        // Canonical as_str form is also accepted.
        assert_eq!(
            DataSourceKind::from_serialized("redis"),
            Some(DataSourceKind::Redis)
        );
        assert_eq!(
            DataSourceKind::from_serialized("cloudflare_d1"),
            Some(DataSourceKind::CloudflareD1)
        );
    }

    #[test]
    fn unknown_kind_is_none() {
        assert!(DataSourceKind::from_serialized("Cassandra").is_none());
        assert!(DataSourceKind::from_serialized("").is_none());
    }

    #[test]
    fn serde_round_trips_all_kinds() {
        for &kind in DataSourceKind::ALL {
            let json = serde_json::to_string(&kind).expect("serialize");
            let back: DataSourceKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(kind, back, "round-trip failed for {kind:?}: {json}");
        }
    }

    #[test]
    fn serde_rejects_unknown_kind_with_message() {
        let result = serde_json::from_str::<DataSourceKind>("\"Cassandra\"");
        assert!(result.is_err());
        let message = result.unwrap_err().to_string().to_lowercase();
        assert!(message.contains("unknown database kind"), "{message}");
        assert!(message.contains("cassandra"), "{message}");
    }

    #[test]
    fn cloudflare_d1_requires_account_and_database_id() {
        let profile = ConnectionProfile::builder(DataSourceKind::CloudflareD1, "d1").build();
        assert!(profile.is_err());

        let profile = ConnectionProfile::builder(DataSourceKind::CloudflareD1, "d1")
            .extra_param("cf_account", "abc")
            .extra_param("cf_database_id", "def")
            .build();
        assert!(profile.is_ok());
    }
}
