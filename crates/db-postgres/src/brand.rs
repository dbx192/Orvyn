//! Brand identity for the PostgreSQL protocol family.
//!
//! Several products speak the PostgreSQL wire protocol but differ in the SQL
//! surface and deployment semantics they expose: Redshift is a read-only
//! analytical warehouse, CockroachDB is a distributed store whose only
//! isolation level is `SERIALIZABLE` (and which may abort transactions with a
//! retryable serialisation error), and the domestic PG-protocol products
//! (openGauss, GaussDB, KWDB, KingbaseES, Vastbase, HighGo, UXDB) implement
//! the standard `information_schema`/`pg_catalog` surface with brand-specific
//! ports and minor DDL differences. Centralising those differences here lets
//! `PostgresMetadataReader` and `PostgresQueryExecutor` branch on semantic
//! capabilities instead of sprinkling `kind == DataSourceKind::Redshift`
//! checks across the codebase.

use db_common::{ConnectionProfile, DataSourceKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostgresBrand {
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
}

impl PostgresBrand {
    /// Maps a [`DataSourceKind`] to its PostgreSQL-family brand. Returns
    /// `None` for non-PostgreSQL kinds so callers can guard with `is_some_and`.
    pub fn of(kind: DataSourceKind) -> Option<Self> {
        match kind {
            DataSourceKind::PostgreSql => Some(Self::PostgreSql),
            DataSourceKind::TimescaleDb => Some(Self::TimescaleDb),
            DataSourceKind::Redshift => Some(Self::Redshift),
            DataSourceKind::CockroachDb => Some(Self::CockroachDb),
            DataSourceKind::OpenGauss => Some(Self::OpenGauss),
            DataSourceKind::GaussDb => Some(Self::GaussDb),
            DataSourceKind::KwDb => Some(Self::KwDb),
            DataSourceKind::KingbaseEs => Some(Self::KingbaseEs),
            DataSourceKind::Vastbase => Some(Self::Vastbase),
            DataSourceKind::HighGo => Some(Self::HighGo),
            DataSourceKind::UxDb => Some(Self::UxDb),
            _ => None,
        }
    }

    pub fn kind(self) -> DataSourceKind {
        match self {
            Self::PostgreSql => DataSourceKind::PostgreSql,
            Self::TimescaleDb => DataSourceKind::TimescaleDb,
            Self::Redshift => DataSourceKind::Redshift,
            Self::CockroachDb => DataSourceKind::CockroachDb,
            Self::OpenGauss => DataSourceKind::OpenGauss,
            Self::GaussDb => DataSourceKind::GaussDb,
            Self::KwDb => DataSourceKind::KwDb,
            Self::KingbaseEs => DataSourceKind::KingbaseEs,
            Self::Vastbase => DataSourceKind::Vastbase,
            Self::HighGo => DataSourceKind::HighGo,
            Self::UxDb => DataSourceKind::UxDb,
        }
    }

    /// Analytical / distributed PostgreSQL-protocol stores with a reduced
    /// SQL surface compared with stock PostgreSQL.
    pub fn is_analytic(self) -> bool {
        matches!(self, Self::Redshift | Self::CockroachDb)
    }

    /// Offline analytical warehouses default the connection to read-only.
    /// Read slicing is still enforced later by `profile.security.read_only`;
    /// this drives the UI default and the documented capability.
    pub fn read_only_default(self) -> bool {
        matches!(self, Self::Redshift)
    }

    /// CockroachDB supports only the `SERIALIZABLE` isolation level and may
    /// abort an in-flight transaction with a retryable serialisation error
    /// (SQLSTATE `40001`). The executor pins the isolation explicitly so a
    /// client that observes the abort can retry the whole transaction.
    pub fn uses_serializable_transactions(self) -> bool {
        matches!(self, Self::CockroachDb)
    }
}

/// Resolves the brand for a profile from the profile's kind; returns `None`
/// for non-PostgreSQL kinds (which should not reach this adapter at all).
pub(super) fn brand_for(profile: &ConnectionProfile) -> Option<PostgresBrand> {
    PostgresBrand::of(profile.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_postgres_family_kind() {
        for kind in [
            DataSourceKind::PostgreSql,
            DataSourceKind::TimescaleDb,
            DataSourceKind::Redshift,
            DataSourceKind::CockroachDb,
            DataSourceKind::OpenGauss,
            DataSourceKind::GaussDb,
            DataSourceKind::KwDb,
            DataSourceKind::KingbaseEs,
            DataSourceKind::Vastbase,
            DataSourceKind::HighGo,
            DataSourceKind::UxDb,
        ] {
            let brand = PostgresBrand::of(kind).expect("postgres family brand");
            assert_eq!(brand.kind(), kind);
        }
    }

    #[test]
    fn rejects_non_postgres_kinds() {
        assert!(PostgresBrand::of(DataSourceKind::MySql).is_none());
        assert!(PostgresBrand::of(DataSourceKind::TimescaleDb).is_some());
        assert!(PostgresBrand::of(DataSourceKind::Redis).is_none());
    }

    #[test]
    fn analytic_brands_group_redshift_and_cockroach() {
        for kind in [DataSourceKind::Redshift, DataSourceKind::CockroachDb] {
            let brand = PostgresBrand::of(kind).unwrap();
            assert!(brand.is_analytic(), "{kind:?} should be analytic");
        }
        let brand = PostgresBrand::of(DataSourceKind::PostgreSql).unwrap();
        assert!(!brand.is_analytic());
    }

    #[test]
    fn redshift_defaults_read_only() {
        let brand = PostgresBrand::of(DataSourceKind::Redshift).unwrap();
        assert!(brand.read_only_default());
        assert!(!brand.is_analytic() || brand.read_only_default());
        for kind in [
            DataSourceKind::PostgreSql,
            DataSourceKind::TimescaleDb,
            DataSourceKind::CockroachDb,
        ] {
            let brand = PostgresBrand::of(kind).unwrap();
            assert!(
                !brand.read_only_default(),
                "{kind:?} should not default read-only"
            );
        }
    }

    #[test]
    fn only_cockroach_uses_serializable_transactions() {
        for kind in [
            DataSourceKind::PostgreSql,
            DataSourceKind::TimescaleDb,
            DataSourceKind::Redshift,
            DataSourceKind::OpenGauss,
        ] {
            let brand = PostgresBrand::of(kind).unwrap();
            assert!(!brand.uses_serializable_transactions(), "{kind:?}");
        }
        let brand = PostgresBrand::of(DataSourceKind::CockroachDb).unwrap();
        assert!(brand.uses_serializable_transactions());
    }
}
