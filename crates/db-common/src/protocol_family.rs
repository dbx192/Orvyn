//! Protocol family classification for `DataSourceKind`.
//!
//! Multiple database brands can share one wire protocol (e.g. TiDB, MariaDB,
//! OceanBase MySQL mode, Doris, StarRocks all speak the MySQL protocol). The
//! `DriverRegistry` uses the protocol family to route brand kinds to a single
//! shared protocol-driver instance while keeping brand-specific tweaks.

use crate::DataSourceKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProtocolFamily {
    /// MySQL wire protocol.
    MySql,
    /// PostgreSQL wire protocol.
    PostgreSql,
    Sqlite,
    DuckDb,
    ClickHouse,
    SqlServer,
    Tdengine,
    Mongo,
    Redis,
    Elasticsearch,
    /// HTTP-based SQL services (Cloudflare D1).
    HttpSql,
    /// No reliable pure-Rust driver; config-only with restricted capabilities.
    Restricted,
}

impl ProtocolFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MySql => "MySQL protocol",
            Self::PostgreSql => "PostgreSQL protocol",
            Self::Sqlite => "SQLite",
            Self::DuckDb => "DuckDB",
            Self::ClickHouse => "ClickHouse",
            Self::SqlServer => "SQL Server (TDS)",
            Self::Tdengine => "TDengine",
            Self::Mongo => "MongoDB",
            Self::Redis => "Redis (RESP)",
            Self::Elasticsearch => "Elasticsearch HTTP",
            Self::HttpSql => "HTTP SQL",
            Self::Restricted => "Restricted",
        }
    }

    pub const fn display_label(self) -> &'static str {
        match self {
            Self::MySql => "MySQL Family",
            Self::PostgreSql => "PostgreSQL Family",
            Self::Sqlite => "SQLite",
            Self::DuckDb => "DuckDB",
            Self::ClickHouse => "ClickHouse",
            Self::SqlServer => "SQL Server",
            Self::Tdengine => "TDengine",
            Self::Mongo => "MongoDB",
            Self::Redis => "Redis",
            Self::Elasticsearch => "Search (Elasticsearch)",
            Self::HttpSql => "HTTP SQL",
            Self::Restricted => "Restricted",
        }
    }

    /// Kinds belonging to this family, in the canonical `DataSourceKind::ALL`
    /// order. Used by the UI type picker to group entries.
    pub fn kinds(self) -> Vec<DataSourceKind> {
        DataSourceKind::ALL
            .iter()
            .copied()
            .filter(|kind| kind.protocol_family() == self)
            .collect()
    }

    /// All protocol families in a stable display order matching the plan's
    /// grouping (relational first, then document/kv, search, http, restricted).
    pub const ALL: &'static [ProtocolFamily] = &[
        Self::MySql,
        Self::PostgreSql,
        Self::Sqlite,
        Self::DuckDb,
        Self::ClickHouse,
        Self::SqlServer,
        Self::Tdengine,
        Self::Mongo,
        Self::Redis,
        Self::Elasticsearch,
        Self::HttpSql,
        Self::Restricted,
    ];
}

impl DataSourceKind {
    pub const fn protocol_family(self) -> ProtocolFamily {
        match self {
            Self::MySql
            | Self::MariaDb
            | Self::TiDb
            | Self::OceanBase
            | Self::Doris
            | Self::SelectDb
            | Self::StarRocks
            | Self::GoldenDb => ProtocolFamily::MySql,
            Self::PostgreSql
            | Self::TimescaleDb
            | Self::Redshift
            | Self::CockroachDb
            | Self::OpenGauss
            | Self::GaussDb
            | Self::KwDb
            | Self::KingbaseEs
            | Self::Vastbase
            | Self::HighGo
            | Self::UxDb => ProtocolFamily::PostgreSql,
            Self::Sqlite => ProtocolFamily::Sqlite,
            Self::DuckDb => ProtocolFamily::DuckDb,
            Self::ClickHouse => ProtocolFamily::ClickHouse,
            Self::SqlServer => ProtocolFamily::SqlServer,
            Self::TDengine => ProtocolFamily::Tdengine,
            Self::MongoDb => ProtocolFamily::Mongo,
            Self::Redis => ProtocolFamily::Redis,
            Self::Elasticsearch | Self::Easysearch | Self::Meilisearch | Self::ManticoreSearch => {
                ProtocolFamily::Elasticsearch
            }
            Self::CloudflareD1 => ProtocolFamily::HttpSql,
            Self::Access | Self::Oracle | Self::Dm | Self::XuguDb => ProtocolFamily::Restricted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_maps_to_a_family() {
        for kind in DataSourceKind::ALL {
            let family = kind.protocol_family();
            assert!(family.kinds().contains(kind), "{kind:?} not in its family");
        }
    }

    #[test]
    fn family_partition_is_exhaustive() {
        let mut all: Vec<DataSourceKind> = ProtocolFamily::ALL
            .iter()
            .flat_map(|family| family.kinds())
            .collect();
        all.sort();
        let mut expected: Vec<DataSourceKind> = DataSourceKind::ALL.to_vec();
        expected.sort();
        assert_eq!(all, expected);
    }
}
