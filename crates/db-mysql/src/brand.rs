//! Brand identity for the MySQL protocol family.
//!
//! Several database products speak the MySQL wire protocol but differ in
//! the SQL surface they expose: Doris/SelectDB/StarRocks share Apache Doris
//! internals and reject `COM_STMT_PREPARE`, ignore `information_schema`, and
//! do not support transactions/triggers/foreign keys; the rest (MySQL,
//! MariaDB, TiDB, OceanBase MySQL mode, GoldenDB) implement the full MySQL
//! feature set. Centralising those differences here lets `MySqlMetadataReader`
//! and `MySqlQueryExecutor` branch on semantic capabilities instead of
//! sprinkling `kind == DataSourceKind::Doris` checks across the codebase.

use db_common::{ConnectionProfile, DataSourceKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MysqlBrand {
    MySql,
    MariaDb,
    TiDb,
    OceanBase,
    GoldenDb,
    Doris,
    SelectDb,
    StarRocks,
}

impl MysqlBrand {
    /// Maps a [`DataSourceKind`] to its MySQL-family brand. Returns `None` for
    /// non-MySQL kinds so callers can guard with `is_some_and`.
    pub fn of(kind: DataSourceKind) -> Option<Self> {
        match kind {
            DataSourceKind::MySql => Some(Self::MySql),
            DataSourceKind::MariaDb => Some(Self::MariaDb),
            DataSourceKind::TiDb => Some(Self::TiDb),
            DataSourceKind::OceanBase => Some(Self::OceanBase),
            DataSourceKind::GoldenDb => Some(Self::GoldenDb),
            DataSourceKind::Doris => Some(Self::Doris),
            DataSourceKind::SelectDb => Some(Self::SelectDb),
            DataSourceKind::StarRocks => Some(Self::StarRocks),
            _ => None,
        }
    }

    pub fn kind(self) -> DataSourceKind {
        match self {
            Self::MySql => DataSourceKind::MySql,
            Self::MariaDb => DataSourceKind::MariaDb,
            Self::TiDb => DataSourceKind::TiDb,
            Self::OceanBase => DataSourceKind::OceanBase,
            Self::GoldenDb => DataSourceKind::GoldenDb,
            Self::Doris => DataSourceKind::Doris,
            Self::SelectDb => DataSourceKind::SelectDb,
            Self::StarRocks => DataSourceKind::StarRocks,
        }
    }

    /// Returns the brand behind a Doris-derived product. Doris/SelectDB/
    /// StarRocks share Apache Doris internals and therefore share the same
    /// quirks (no prepared statements, no information_schema, no routines).
    pub fn is_doris_family(self) -> bool {
        matches!(self, Self::Doris | Self::SelectDb | Self::StarRocks)
    }

    /// Uses the text protocol (`COM_QUERY` / `sqlx::raw_sql`) instead of
    /// prepared statements. Doris-family servers do not implement
    /// `COM_STMT_PREPARE`, so any path that binds parameters must be
    /// replaced with literal SQL.
    pub fn uses_text_protocol(self) -> bool {
        self.is_doris_family()
    }

    /// Whether `information_schema` queries are available. Doris-family
    /// servers expose `SHOW` commands but not the ANSI information_schema
    /// tables; metadata must use SHOW TABLES / SHOW COLUMNS / SHOW INDEX.
    pub fn supports_information_schema(self) -> bool {
        !self.is_doris_family()
    }

    /// Whether the server supports transactions (`BEGIN`/`COMMIT`/`ROLLBACK`).
    pub fn supports_transactions(self) -> bool {
        !self.is_doris_family()
    }

    /// Whether the server exposes referential constraints, triggers and
    /// routines through `information_schema`.
    pub fn supports_routines_metadata(self) -> bool {
        !self.is_doris_family()
    }

    /// Returns false when this brand cannot be reached over the MySQL wire
    /// protocol for the given profile. OceanBase exposes both a MySQL and an
    /// Oracle tenant mode; only MySQL mode is reachable from this adapter
    /// because Oracle mode speaks Oracle's proprietary protocol. The UI lets
    /// users save Oracle-mode configs, but every operation returns
    /// `ErrorKind::Unsupported` via this check.
    pub fn is_connectable(self, profile: &ConnectionProfile) -> bool {
        match self {
            Self::OceanBase => match profile.extra_params.get("ob_mode") {
                Some(mode) if !mode.is_empty() => mode == "mysql",
                _ => true,
            },
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_mysql_family_kind() {
        for kind in [
            DataSourceKind::MySql,
            DataSourceKind::MariaDb,
            DataSourceKind::TiDb,
            DataSourceKind::OceanBase,
            DataSourceKind::GoldenDb,
            DataSourceKind::Doris,
            DataSourceKind::SelectDb,
            DataSourceKind::StarRocks,
        ] {
            let brand = MysqlBrand::of(kind).expect("mysql family brand");
            assert_eq!(brand.kind(), kind);
        }
    }

    #[test]
    fn rejects_non_mysql_kinds() {
        assert!(MysqlBrand::of(DataSourceKind::PostgreSql).is_none());
        assert!(MysqlBrand::of(DataSourceKind::Redis).is_none());
        assert!(MysqlBrand::of(DataSourceKind::CloudflareD1).is_none());
    }

    #[test]
    fn doris_family_groups_doris_selectdb_starrocks() {
        for kind in [
            DataSourceKind::Doris,
            DataSourceKind::SelectDb,
            DataSourceKind::StarRocks,
        ] {
            let brand = MysqlBrand::of(kind).unwrap();
            assert!(brand.is_doris_family(), "{kind:?} should be Doris family");
            assert!(brand.uses_text_protocol());
            assert!(!brand.supports_information_schema());
            assert!(!brand.supports_transactions());
            assert!(!brand.supports_routines_metadata());
        }
    }

    #[test]
    fn full_family_brands_keep_full_capability() {
        for kind in [
            DataSourceKind::MySql,
            DataSourceKind::MariaDb,
            DataSourceKind::TiDb,
            DataSourceKind::OceanBase,
            DataSourceKind::GoldenDb,
        ] {
            let brand = MysqlBrand::of(kind).unwrap();
            assert!(!brand.is_doris_family(), "{kind:?} should be full family");
            assert!(!brand.uses_text_protocol());
            assert!(brand.supports_information_schema());
            assert!(brand.supports_transactions());
            assert!(brand.supports_routines_metadata());
        }
    }
}
