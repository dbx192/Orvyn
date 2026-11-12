//! Capability matrix and brand-specific form schema.
//!
//! `ConnectorCapabilities` describes what a `DataSourceKind` can actually do
//! today (full / compatible / restricted / unsupported) plus the granular
//! capability flags the UI uses to show, hide or disable actions. The matrix
//! reflects the currently registered native and ODBC drivers, not aspirational
//! support, so the UI never advertises an action that will return
//! `ErrorKind::Unsupported`.

use crate::{DataSourceKind, ProtocolFamily};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupportLevel {
    /// Full native support: connection test, query, metadata, CRUD, DDL.
    Full,
    /// Speaks a shared wire protocol (MySQL/PostgreSQL) with brand-specific
    /// tweaks; most operations work but some DDL/transaction semantics differ.
    Compatible,
    /// Configurable and partially working (e.g. connection test only, or
    /// read-only analysis). Unsupported operations return `ErrorKind::Unsupported`.
    Restricted,
    /// No reliable pure-Rust driver yet. Config can be saved but no operations
    /// are available; `RestrictedConnectionTester` returns structured Unsupported.
    Unsupported,
}

impl SupportLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compatible => "compatible",
            Self::Restricted => "restricted",
            Self::Unsupported => "unsupported",
        }
    }

    pub const fn display_label(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::Compatible => "Compatible",
            Self::Restricted => "Restricted",
            Self::Unsupported => "Unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectorCapabilities {
    pub support: SupportLevel,
    pub protocol_family: ProtocolFamily,
    pub connection_test: bool,
    pub sql_query: bool,
    pub metadata: bool,
    pub crud: bool,
    pub transactions: bool,
    pub http_json: bool,
    pub document_kv: bool,
    pub search: bool,
    pub ddl: bool,
    pub read_only_default: bool,
    pub notes: Option<&'static str>,
}

impl ConnectorCapabilities {
    const fn full_relational(family: ProtocolFamily) -> Self {
        Self {
            support: SupportLevel::Full,
            protocol_family: family,
            connection_test: true,
            sql_query: true,
            metadata: true,
            crud: true,
            transactions: true,
            http_json: false,
            document_kv: false,
            search: false,
            ddl: true,
            read_only_default: false,
            notes: None,
        }
    }

    /// Vendor-driver brands reached through the ODBC bridge: full relational
    /// surface, but only when the matching vendor ODBC driver is installed,
    /// hence `Restricted` rather than `Full`.
    const fn odbc_bridge(notes: &'static str, transactions: bool) -> Self {
        Self {
            support: SupportLevel::Restricted,
            protocol_family: ProtocolFamily::Odbc,
            connection_test: true,
            sql_query: true,
            metadata: true,
            crud: true,
            transactions,
            http_json: false,
            document_kv: false,
            search: false,
            ddl: true,
            read_only_default: false,
            notes: Some(notes),
        }
    }

    const fn mysql_compatible(notes: &'static str) -> Self {
        Self {
            support: SupportLevel::Compatible,
            protocol_family: ProtocolFamily::MySql,
            connection_test: true,
            sql_query: true,
            metadata: true,
            crud: true,
            transactions: true,
            http_json: false,
            document_kv: false,
            search: false,
            ddl: true,
            read_only_default: false,
            notes: Some(notes),
        }
    }

    const fn postgres_compatible(notes: &'static str) -> Self {
        Self {
            support: SupportLevel::Compatible,
            protocol_family: ProtocolFamily::PostgreSql,
            connection_test: true,
            sql_query: true,
            metadata: true,
            crud: true,
            transactions: true,
            http_json: false,
            document_kv: false,
            search: false,
            ddl: true,
            read_only_default: false,
            notes: Some(notes),
        }
    }

    /// Analytical MySQL-protocol stores without transactions / foreign keys.
    const fn mysql_analytic(notes: &'static str) -> Self {
        Self {
            support: SupportLevel::Compatible,
            protocol_family: ProtocolFamily::MySql,
            connection_test: true,
            sql_query: true,
            metadata: true,
            crud: true,
            transactions: false,
            http_json: false,
            document_kv: false,
            search: false,
            ddl: false,
            read_only_default: false,
            notes: Some(notes),
        }
    }

    /// Document store with full native support (MongoDB).
    const fn document_full(family: ProtocolFamily, notes: &'static str) -> Self {
        Self {
            support: SupportLevel::Full,
            protocol_family: family,
            connection_test: true,
            sql_query: false,
            metadata: true,
            crud: true,
            transactions: false,
            http_json: false,
            document_kv: true,
            search: false,
            ddl: false,
            read_only_default: false,
            notes: Some(notes),
        }
    }

    /// Search service with full native support (Elasticsearch, ...).
    const fn search_full(family: ProtocolFamily, notes: &'static str) -> Self {
        Self {
            support: SupportLevel::Full,
            protocol_family: family,
            connection_test: true,
            sql_query: false,
            metadata: true,
            crud: false,
            transactions: false,
            http_json: true,
            document_kv: false,
            search: true,
            ddl: false,
            read_only_default: false,
            notes: Some(notes),
        }
    }

    /// HTTP-based SQL store (Cloudflare D1).
    const fn http_sql_full(family: ProtocolFamily, notes: &'static str) -> Self {
        Self {
            support: SupportLevel::Full,
            protocol_family: family,
            connection_test: true,
            sql_query: true,
            metadata: true,
            crud: true,
            transactions: false,
            http_json: true,
            document_kv: false,
            search: false,
            ddl: true,
            read_only_default: false,
            notes: Some(notes),
        }
    }
}

/// Metadata actually exposed by the registered connector, not server feature claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MetadataCapabilities {
    pub views: bool,
    pub indexes: bool,
    pub foreign_keys: bool,
    pub triggers: bool,
    pub routines: bool,
    pub routine_definitions: bool,
}

impl DataSourceKind {
    pub const fn metadata_capabilities(self) -> MetadataCapabilities {
        let family = self.protocol_family();
        let analytic = matches!(self, Self::Doris | Self::SelectDb | Self::StarRocks);
        MetadataCapabilities {
            views: self.is_relational() && !matches!(self, Self::TDengine),
            indexes: self.is_relational()
                && !matches!(self, Self::ClickHouse | Self::TDengine)
                && !analytic,
            foreign_keys: matches!(
                family,
                ProtocolFamily::PostgreSql
                    | ProtocolFamily::Sqlite
                    | ProtocolFamily::DuckDb
                    | ProtocolFamily::Odbc
            ) || (matches!(family, ProtocolFamily::MySql) && !analytic),
            triggers: matches!(family, ProtocolFamily::PostgreSql | ProtocolFamily::Sqlite)
                || (matches!(family, ProtocolFamily::MySql) && !analytic),
            routines: matches!(family, ProtocolFamily::PostgreSql | ProtocolFamily::DuckDb)
                || (matches!(family, ProtocolFamily::MySql) && !analytic),
            routine_definitions: matches!(family, ProtocolFamily::PostgreSql)
                || (matches!(family, ProtocolFamily::MySql) && !analytic),
        }
    }

    pub const fn capabilities(self) -> ConnectorCapabilities {
        match self {
            Self::MySql => ConnectorCapabilities::full_relational(ProtocolFamily::MySql),
            Self::MariaDb => ConnectorCapabilities::mysql_compatible("MySQL protocol (MariaDB)"),
            Self::TiDb => ConnectorCapabilities::mysql_compatible("MySQL protocol (TiDB)"),
            Self::OceanBase => {
                ConnectorCapabilities::mysql_compatible("MySQL protocol (OceanBase MySQL mode)")
            }
            Self::Doris => {
                ConnectorCapabilities::mysql_analytic("MySQL protocol (Doris), no transactions")
            }
            Self::SelectDb => {
                ConnectorCapabilities::mysql_analytic("MySQL protocol (SelectDB), no transactions")
            }
            Self::StarRocks => {
                ConnectorCapabilities::mysql_analytic("MySQL protocol (StarRocks), no transactions")
            }
            Self::GoldenDb => ConnectorCapabilities::mysql_compatible("MySQL protocol (GoldenDB)"),

            Self::PostgreSql => ConnectorCapabilities::full_relational(ProtocolFamily::PostgreSql),
            Self::TimescaleDb => {
                ConnectorCapabilities::postgres_compatible("PostgreSQL protocol (TimescaleDB)")
            }
            Self::Redshift => ConnectorCapabilities {
                read_only_default: true,
                ..ConnectorCapabilities::postgres_compatible(
                    "PostgreSQL protocol (Redshift, analytical)",
                )
            },
            Self::CockroachDb => ConnectorCapabilities::postgres_compatible(
                "PostgreSQL protocol (CockroachDB, retry on serialization)",
            ),
            Self::OpenGauss => {
                ConnectorCapabilities::postgres_compatible("PostgreSQL protocol (openGauss)")
            }
            Self::GaussDb => {
                ConnectorCapabilities::postgres_compatible("PostgreSQL protocol (GaussDB)")
            }
            Self::KwDb => ConnectorCapabilities::postgres_compatible("PostgreSQL protocol (KWDB)"),
            Self::KingbaseEs => {
                ConnectorCapabilities::postgres_compatible("PostgreSQL protocol (KingbaseES)")
            }
            Self::Vastbase => {
                ConnectorCapabilities::postgres_compatible("PostgreSQL protocol (Vastbase)")
            }
            Self::HighGo => {
                ConnectorCapabilities::postgres_compatible("PostgreSQL protocol (HighGo)")
            }
            Self::UxDb => ConnectorCapabilities::postgres_compatible("PostgreSQL protocol (UXDB)"),

            Self::Sqlite => ConnectorCapabilities::full_relational(ProtocolFamily::Sqlite),
            Self::DuckDb => ConnectorCapabilities::full_relational(ProtocolFamily::DuckDb),
            Self::ClickHouse => ConnectorCapabilities {
                transactions: false,
                notes: Some("ClickHouse HTTP query/metadata, no client transactions"),
                ..ConnectorCapabilities::full_relational(ProtocolFamily::ClickHouse)
            },
            Self::SqlServer => ConnectorCapabilities::full_relational(ProtocolFamily::SqlServer),
            Self::TDengine => ConnectorCapabilities {
                support: SupportLevel::Restricted,
                transactions: false,
                notes: Some(
                    "TDengine REST SQL and table/stable metadata; no transactions or views",
                ),
                ..ConnectorCapabilities::full_relational(ProtocolFamily::Tdengine)
            },

            Self::MongoDb => ConnectorCapabilities::document_full(
                ProtocolFamily::Mongo,
                "MongoDB document store",
            ),
            Self::Redis => ConnectorCapabilities {
                support: SupportLevel::Full,
                protocol_family: ProtocolFamily::Redis,
                connection_test: true,
                sql_query: false,
                metadata: true,
                crud: true,
                transactions: false,
                http_json: false,
                document_kv: true,
                search: false,
                ddl: false,
                read_only_default: false,
                notes: None,
            },
            Self::Elasticsearch => ConnectorCapabilities::search_full(
                ProtocolFamily::Elasticsearch,
                "Elasticsearch REST search",
            ),
            Self::Easysearch => ConnectorCapabilities::search_full(
                ProtocolFamily::Elasticsearch,
                "Easysearch (Elasticsearch-compatible) REST search",
            ),
            Self::Meilisearch => ConnectorCapabilities::search_full(
                ProtocolFamily::Elasticsearch,
                "Meilisearch REST search",
            ),
            Self::ManticoreSearch => ConnectorCapabilities::search_full(
                ProtocolFamily::Elasticsearch,
                "Manticore Search HTTP/MySQL-protocol search",
            ),
            Self::CloudflareD1 => ConnectorCapabilities::http_sql_full(
                ProtocolFamily::HttpSql,
                "Cloudflare D1 in-browser SQL via HTTP API",
            ),

            Self::Access => ConnectorCapabilities::odbc_bridge(
                "via ODBC; requires the 64-bit Microsoft Access Driver (*.mdb, *.accdb)",
                false,
            ),
            Self::Oracle => ConnectorCapabilities::odbc_bridge(
                "via ODBC; requires an Oracle Instant Client ODBC driver",
                true,
            ),
            Self::Dm => {
                ConnectorCapabilities::odbc_bridge("via ODBC; requires the DM ODBC DRIVER", true)
            }
            Self::XuguDb => {
                ConnectorCapabilities::odbc_bridge("via ODBC; requires the Xugu ODBC driver", true)
            }
        }
    }
}

/// A brand-specific form field rendered from `extra_params`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraParamField {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: ExtraParamKind,
    pub required: bool,
    pub placeholder: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraParamKind {
    Text,
    Password,
    Choice(&'static [&'static str]),
}

impl DataSourceKind {
    /// Brand-specific fields the connection form renders beyond the common
    /// host/port/database/file_path/ssl block. Drives `extra_params`.
    pub fn extra_params_schema(self) -> Vec<ExtraParamField> {
        match self {
            Self::MySql
            | Self::MariaDb
            | Self::TiDb
            | Self::Doris
            | Self::SelectDb
            | Self::StarRocks
            | Self::GoldenDb
            | Self::PostgreSql
            | Self::TimescaleDb
            | Self::Redshift
            | Self::CockroachDb
            | Self::OpenGauss
            | Self::GaussDb
            | Self::KwDb
            | Self::KingbaseEs
            | Self::Vastbase
            | Self::HighGo
            | Self::UxDb
            | Self::SqlServer => vec![ExtraParamField {
                key: "tls_ca_path",
                label: "TLS CA certificate path",
                kind: ExtraParamKind::Text,
                required: false,
                placeholder: Some("C:\\certs\\ca.pem"),
            }],
            Self::OceanBase => vec![
                ExtraParamField {
                    key: "tls_ca_path",
                    label: "TLS CA certificate path",
                    kind: ExtraParamKind::Text,
                    required: false,
                    placeholder: Some("C:\\certs\\ca.pem"),
                },
                ExtraParamField {
                    key: "ob_mode",
                    label: "OceanBase mode",
                    kind: ExtraParamKind::Choice(&["mysql", "oracle"]),
                    required: false,
                    placeholder: Some("mysql"),
                },
            ],
            Self::ClickHouse => vec![ExtraParamField {
                key: "clickhouse_protocol",
                label: "ClickHouse protocol",
                kind: ExtraParamKind::Choice(&["http", "native"]),
                required: false,
                placeholder: Some("http"),
            }],
            Self::MongoDb => vec![ExtraParamField {
                key: "mongo_auth_source",
                label: "Auth source",
                kind: ExtraParamKind::Text,
                required: false,
                placeholder: Some("admin"),
            }],
            Self::Elasticsearch | Self::Easysearch => vec![
                ExtraParamField {
                    key: "http_path",
                    label: "HTTP path",
                    kind: ExtraParamKind::Text,
                    required: false,
                    placeholder: Some("/"),
                },
                ExtraParamField {
                    key: "es_api_id",
                    label: "API key id",
                    kind: ExtraParamKind::Text,
                    required: false,
                    placeholder: None,
                },
                ExtraParamField {
                    key: "es_api_key",
                    label: "API key",
                    kind: ExtraParamKind::Password,
                    required: false,
                    placeholder: None,
                },
            ],
            Self::Meilisearch => vec![ExtraParamField {
                key: "meili_api_key",
                label: "Master key",
                kind: ExtraParamKind::Password,
                required: false,
                placeholder: None,
            }],
            Self::TDengine => vec![ExtraParamField {
                key: "http_path",
                label: "HTTP path",
                kind: ExtraParamKind::Text,
                required: false,
                placeholder: Some("/rest/sql"),
            }],
            Self::CloudflareD1 => vec![
                ExtraParamField {
                    key: "cf_account",
                    label: "Cloudflare account id",
                    kind: ExtraParamKind::Text,
                    required: true,
                    placeholder: None,
                },
                ExtraParamField {
                    key: "cf_database_id",
                    label: "Database id",
                    kind: ExtraParamKind::Text,
                    required: true,
                    placeholder: None,
                },
                ExtraParamField {
                    key: "cf_token",
                    label: "API token",
                    kind: ExtraParamKind::Password,
                    required: true,
                    placeholder: None,
                },
            ],
            Self::Oracle => vec![
                ExtraParamField {
                    key: "service_name",
                    label: "Service name",
                    kind: ExtraParamKind::Text,
                    required: false,
                    placeholder: Some("orcl"),
                },
                ExtraParamField {
                    key: "oracle_sid",
                    label: "SID (alternative to service name)",
                    kind: ExtraParamKind::Text,
                    required: false,
                    placeholder: None,
                },
                ExtraParamField {
                    key: "odbc_driver",
                    label: "ODBC driver name",
                    kind: ExtraParamKind::Text,
                    required: false,
                    placeholder: Some("Oracle in instantclient_21_13"),
                },
                ExtraParamField {
                    key: "odbc_connection_string",
                    label: "Raw ODBC connection string (overrides all)",
                    kind: ExtraParamKind::Password,
                    required: false,
                    placeholder: None,
                },
            ],
            Self::Dm | Self::XuguDb => vec![
                ExtraParamField {
                    key: "odbc_driver",
                    label: "ODBC driver name",
                    kind: ExtraParamKind::Text,
                    required: false,
                    placeholder: None,
                },
                ExtraParamField {
                    key: "odbc_connection_string",
                    label: "Raw ODBC connection string (overrides all)",
                    kind: ExtraParamKind::Password,
                    required: false,
                    placeholder: None,
                },
            ],
            Self::Access => vec![
                ExtraParamField {
                    key: "odbc_driver",
                    label: "ODBC driver name",
                    kind: ExtraParamKind::Text,
                    required: false,
                    placeholder: Some("Microsoft Access Driver (*.mdb, *.accdb)"),
                },
                ExtraParamField {
                    key: "odbc_connection_string",
                    label: "Raw ODBC connection string (overrides all)",
                    kind: ExtraParamKind::Password,
                    required: false,
                    placeholder: None,
                },
            ],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_capabilities() {
        for kind in DataSourceKind::ALL {
            let caps = kind.capabilities();
            assert_eq!(caps.protocol_family, kind.protocol_family());
            // Unsupported kinds must not advertise any operation.
            if caps.support == SupportLevel::Unsupported {
                assert!(!caps.connection_test);
                assert!(!caps.sql_query);
                assert!(!caps.metadata);
            }
            // Full relational kinds must advertise query + metadata.
            if caps.support == SupportLevel::Full && kind.is_relational() {
                assert!(caps.sql_query && caps.metadata && caps.crud);
            }
        }
    }

    #[test]
    fn odbc_kinds_are_restricted_but_queryable() {
        for kind in [
            DataSourceKind::Oracle,
            DataSourceKind::Dm,
            DataSourceKind::XuguDb,
            DataSourceKind::Access,
        ] {
            let caps = kind.capabilities();
            assert_eq!(caps.support, SupportLevel::Restricted, "{kind:?}");
            assert_eq!(caps.protocol_family, ProtocolFamily::Odbc);
            assert!(caps.sql_query && caps.metadata && caps.connection_test);
            assert!(caps.notes.is_some(), "{kind:?} should name its driver");
        }
    }
}
