//! Unified driver registry.
//!
//! Replaces the scattered `match kind { TiDb|OceanBase|Doris => MySql }`
//! routing that previously lived in `SessionManager`, `MetadataService` and
//! `QueryService`. Each `DataSourceKind` is explicitly registered against the
//! shared protocol-driver instance for its family, so multiple brands (TiDB,
//! MariaDB, OceanBase, ...) reuse one `MySqlMetadataReader` / pool map while
//! keeping their own `kind()` identity for accurate reports and UI labels.

use async_trait::async_trait;
use db_common::{
    unsupported_kind, AppError, ConnectionTester, DataSourceKind, DocumentConnector, ErrorKind,
    HttpSqlConnector, RedisMetadataReader, RelationalMetadataReader, RelationalQueryExecutor,
    Result, SearchConnector,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct DriverRegistry {
    testers: Arc<BTreeMap<DataSourceKind, Arc<dyn ConnectionTester>>>,
    relational_metadata: Arc<BTreeMap<DataSourceKind, Arc<dyn RelationalMetadataReader>>>,
    relational_query: Arc<BTreeMap<DataSourceKind, Arc<dyn RelationalQueryExecutor>>>,
    redis_metadata: Arc<BTreeMap<DataSourceKind, Arc<dyn RedisMetadataReader>>>,
    document: Arc<BTreeMap<DataSourceKind, Arc<dyn DocumentConnector>>>,
    search: Arc<BTreeMap<DataSourceKind, Arc<dyn SearchConnector>>>,
    http_sql: Arc<BTreeMap<DataSourceKind, Arc<dyn HttpSqlConnector>>>,
}

impl Default for DriverRegistry {
    fn default() -> Self {
        Self::build()
    }
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn build() -> Self {
        let mut testers: BTreeMap<DataSourceKind, Arc<dyn ConnectionTester>> = BTreeMap::new();
        let mut relational_metadata: BTreeMap<DataSourceKind, Arc<dyn RelationalMetadataReader>> =
            BTreeMap::new();
        let mut relational_query: BTreeMap<DataSourceKind, Arc<dyn RelationalQueryExecutor>> =
            BTreeMap::new();
        let mut redis_metadata: BTreeMap<DataSourceKind, Arc<dyn RedisMetadataReader>> =
            BTreeMap::new();
        let mut document: BTreeMap<DataSourceKind, Arc<dyn DocumentConnector>> = BTreeMap::new();
        let mut search: BTreeMap<DataSourceKind, Arc<dyn SearchConnector>> = BTreeMap::new();
        let mut http_sql: BTreeMap<DataSourceKind, Arc<dyn HttpSqlConnector>> = BTreeMap::new();

        // ---- MySQL family: shared driver instances across all brands ----
        let mysql_tester: Arc<dyn ConnectionTester> = Arc::new(db_mysql::MySqlConnectionTester);
        let mysql_meta: Arc<dyn RelationalMetadataReader> =
            Arc::new(db_mysql::MySqlMetadataReader::new());
        let mysql_query: Arc<dyn RelationalQueryExecutor> =
            Arc::new(db_mysql::MySqlQueryExecutor::new());

        // Every MySQL-family brand delegates to the shared MySQL tester; the
        // brand wrapper re-tags the report so the UI shows the right kind.
        testers.insert(DataSourceKind::MySql, mysql_tester.clone());
        for brand in [
            DataSourceKind::MariaDb,
            DataSourceKind::TiDb,
            DataSourceKind::OceanBase,
            DataSourceKind::Doris,
            DataSourceKind::SelectDb,
            DataSourceKind::StarRocks,
            DataSourceKind::GoldenDb,
        ] {
            testers.insert(
                brand,
                Arc::new(BrandConnectionTester::new(brand, mysql_tester.clone())),
            );
        }

        for kind in [
            DataSourceKind::MySql,
            DataSourceKind::MariaDb,
            DataSourceKind::TiDb,
            DataSourceKind::OceanBase,
            DataSourceKind::Doris,
            DataSourceKind::SelectDb,
            DataSourceKind::StarRocks,
            DataSourceKind::GoldenDb,
        ] {
            relational_metadata.insert(kind, mysql_meta.clone());
            relational_query.insert(kind, mysql_query.clone());
        }

        // ---- PostgreSQL family ----
        let pg_tester: Arc<dyn ConnectionTester> = Arc::new(db_postgres::PostgresConnectionTester);
        let pg_meta: Arc<dyn RelationalMetadataReader> =
            Arc::new(db_postgres::PostgresMetadataReader);
        let pg_query: Arc<dyn RelationalQueryExecutor> =
            Arc::new(db_postgres::PostgresQueryExecutor::new());

        testers.insert(DataSourceKind::PostgreSql, pg_tester.clone());
        for brand in [
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
            testers.insert(
                brand,
                Arc::new(BrandConnectionTester::new(brand, pg_tester.clone())),
            );
        }

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
            relational_metadata.insert(kind, pg_meta.clone());
            relational_query.insert(kind, pg_query.clone());
        }

        // ---- Independent relational / analytical ----
        testers.insert(
            DataSourceKind::Sqlite,
            Arc::new(db_sqlite::SqliteConnectionTester),
        );
        testers.insert(
            DataSourceKind::DuckDb,
            Arc::new(db_duckdb::DuckDbConnectionTester),
        );
        relational_metadata.insert(
            DataSourceKind::Sqlite,
            Arc::new(db_sqlite::SqliteMetadataReader),
        );
        relational_metadata.insert(
            DataSourceKind::DuckDb,
            Arc::new(db_duckdb::DuckDbMetadataReader),
        );
        relational_query.insert(
            DataSourceKind::Sqlite,
            Arc::new(db_sqlite::SqliteQueryExecutor::new()),
        );
        relational_query.insert(
            DataSourceKind::DuckDb,
            Arc::new(db_duckdb::DuckDbQueryExecutor::new()),
        );

        // SQL Server and ClickHouse: independent relational drivers. Each has
        // its own metadata reader and query executor in addition to the tester.
        let sqlserver_tester: Arc<dyn ConnectionTester> =
            Arc::new(db_sqlserver::SqlServerConnectionTester);
        let sqlserver_meta: Arc<dyn RelationalMetadataReader> =
            Arc::new(db_sqlserver::SqlServerMetadataReader);
        let sqlserver_query: Arc<dyn RelationalQueryExecutor> =
            Arc::new(db_sqlserver::SqlServerQueryExecutor::new());
        testers.insert(DataSourceKind::SqlServer, sqlserver_tester);
        relational_metadata.insert(DataSourceKind::SqlServer, sqlserver_meta);
        relational_query.insert(DataSourceKind::SqlServer, sqlserver_query);

        let clickhouse_tester: Arc<dyn ConnectionTester> =
            Arc::new(db_clickhouse::ClickHouseConnectionTester);
        let clickhouse_meta: Arc<dyn RelationalMetadataReader> =
            Arc::new(db_clickhouse::ClickHouseMetadataReader);
        let clickhouse_query: Arc<dyn RelationalQueryExecutor> =
            Arc::new(db_clickhouse::ClickHouseQueryExecutor);
        testers.insert(DataSourceKind::ClickHouse, clickhouse_tester);
        relational_metadata.insert(DataSourceKind::ClickHouse, clickhouse_meta);
        relational_query.insert(DataSourceKind::ClickHouse, clickhouse_query);

        // ---- Document / key-value ----
        testers.insert(
            DataSourceKind::MongoDb,
            Arc::new(db_mongodb::MongoDbConnectionTester),
        );
        document.insert(
            DataSourceKind::MongoDb,
            Arc::new(db_mongodb::MongoDbDocumentConnector),
        );
        testers.insert(
            DataSourceKind::Redis,
            Arc::new(db_redis::RedisConnectionTester),
        );
        redis_metadata.insert(
            DataSourceKind::Redis,
            Arc::new(db_redis::RedisMetadataReaderImpl),
        );

        // TDengine REST SQL and metadata; transactions remain unsupported.
        testers.insert(
            DataSourceKind::TDengine,
            Arc::new(db_tdengine::TdengineConnectionTester),
        );
        relational_query.insert(DataSourceKind::TDengine, Arc::new(db_tdengine::TdengineQueryExecutor));
        relational_metadata.insert(DataSourceKind::TDengine, Arc::new(db_tdengine::TdengineMetadataReader));

        // ---- Search services ----
        let es_tester: Arc<dyn ConnectionTester> = Arc::new(db_http::ElasticsearchConnectionTester);
        testers.insert(DataSourceKind::Elasticsearch, es_tester.clone());
        testers.insert(
            DataSourceKind::Easysearch,
            Arc::new(BrandConnectionTester::new(
                DataSourceKind::Easysearch,
                es_tester,
            )),
        );
        testers.insert(
            DataSourceKind::Meilisearch,
            Arc::new(db_http::MeilisearchConnectionTester),
        );
        testers.insert(
            DataSourceKind::ManticoreSearch,
            Arc::new(db_http::ManticoreConnectionTester),
        );
        search.insert(
            DataSourceKind::Elasticsearch,
            Arc::new(db_http::ElasticsearchSearchConnector::new()),
        );
        search.insert(
            DataSourceKind::Easysearch,
            Arc::new(db_http::ElasticsearchSearchConnector::easysearch()),
        );
        search.insert(
            DataSourceKind::Meilisearch,
            Arc::new(db_http::MeilisearchSearchConnector),
        );
        search.insert(
            DataSourceKind::ManticoreSearch,
            Arc::new(db_http::ManticoreSearchConnector),
        );

        // ---- HTTP-SQL (Cloudflare D1) ----
        testers.insert(
            DataSourceKind::CloudflareD1,
            Arc::new(db_http::CloudflareD1ConnectionTester),
        );
        http_sql.insert(
            DataSourceKind::CloudflareD1,
            Arc::new(db_http::CloudflareD1HttpConnector),
        );

        // ---- Vendor ODBC bridge (Oracle, DM, XuguDB, Access) ----
        // One tester/metadata/executor instance per kind: the ODBC bridge
        // carries its kind identity for reports, and each executor owns a
        // separate pool map.
        for kind in [
            DataSourceKind::Oracle,
            DataSourceKind::Dm,
            DataSourceKind::XuguDb,
            DataSourceKind::Access,
        ] {
            testers.insert(kind, Arc::new(db_odbc::OdbcConnectionTester::new(kind)));
            relational_metadata.insert(
                kind,
                Arc::new(db_odbc::OdbcMetadataReader::new(kind)),
            );
            relational_query.insert(
                kind,
                Arc::new(db_odbc::OdbcQueryExecutor::new(kind)),
            );
        }

        Self {
            testers: Arc::new(testers),
            relational_metadata: Arc::new(relational_metadata),
            relational_query: Arc::new(relational_query),
            redis_metadata: Arc::new(redis_metadata),
            document: Arc::new(document),
            search: Arc::new(search),
            http_sql: Arc::new(http_sql),
        }
    }

    /// Builder from explicit driver lists. Used by tests that inject custom
    /// drivers; production code should prefer [`DriverRegistry::default`].
    pub fn from_testers(testers: Vec<Arc<dyn ConnectionTester>>) -> Self {
        let mut map = BTreeMap::new();
        for tester in testers {
            map.insert(tester.kind(), tester);
        }
        Self {
            testers: Arc::new(map),
            relational_metadata: Arc::new(BTreeMap::new()),
            relational_query: Arc::new(BTreeMap::new()),
            redis_metadata: Arc::new(BTreeMap::new()),
            document: Arc::new(BTreeMap::new()),
            search: Arc::new(BTreeMap::new()),
            http_sql: Arc::new(BTreeMap::new()),
        }
    }

    /// Clone the registry and replace the listed relational metadata readers.
    /// Lets a caller start from `DriverRegistry::default()` (which already
    /// covers all brands) and override individual kinds without rebuilding the
    /// entire map.
    pub fn override_relational_metadata(
        mut self,
        readers: Vec<Arc<dyn RelationalMetadataReader>>,
    ) -> Self {
        if readers.is_empty() {
            return self;
        }
        let mut map = (*self.relational_metadata).clone();
        for reader in readers {
            map.insert(reader.kind(), reader);
        }
        self.relational_metadata = Arc::new(map);
        self
    }

    /// Clone the registry and replace the listed relational query executors.
    pub fn override_relational_query(
        mut self,
        executors: Vec<Arc<dyn RelationalQueryExecutor>>,
    ) -> Self {
        if executors.is_empty() {
            return self;
        }
        let mut map = (*self.relational_query).clone();
        for executor in executors {
            map.insert(executor.kind(), executor);
        }
        self.relational_query = Arc::new(map);
        self
    }

    /// Clone the registry and replace the listed Redis metadata readers.
    pub fn override_redis_metadata(mut self, readers: Vec<Arc<dyn RedisMetadataReader>>) -> Self {
        if readers.is_empty() {
            return self;
        }
        let mut map = (*self.redis_metadata).clone();
        for reader in readers {
            map.insert(reader.kind(), reader);
        }
        self.redis_metadata = Arc::new(map);
        self
    }

    /// Clone the registry and replace the listed connection testers. Used by
    /// tests that want to substitute a mock tester for a single kind while
    /// keeping the rest of the default routing intact.
    pub fn override_testers(mut self, testers: Vec<Arc<dyn ConnectionTester>>) -> Self {
        if testers.is_empty() {
            return self;
        }
        let mut map = (*self.testers).clone();
        for tester in testers {
            map.insert(tester.kind(), tester);
        }
        self.testers = Arc::new(map);
        self
    }

    pub fn tester(&self, kind: DataSourceKind) -> Result<&Arc<dyn ConnectionTester>> {
        self.testers
            .get(&kind)
            .ok_or_else(|| unsupported_kind(kind))
    }

    pub fn relational_metadata(
        &self,
        kind: DataSourceKind,
    ) -> Result<&Arc<dyn RelationalMetadataReader>> {
        self.relational_metadata
            .get(&kind)
            .ok_or_else(|| unsupported_kind(kind))
    }

    pub fn relational_query(
        &self,
        kind: DataSourceKind,
    ) -> Result<&Arc<dyn RelationalQueryExecutor>> {
        self.relational_query
            .get(&kind)
            .ok_or_else(|| unsupported_kind(kind))
    }

    pub fn redis_metadata(&self, kind: DataSourceKind) -> Result<&Arc<dyn RedisMetadataReader>> {
        self.redis_metadata
            .get(&kind)
            .ok_or_else(|| unsupported_kind(kind))
    }

    pub fn document(&self, kind: DataSourceKind) -> Result<&Arc<dyn DocumentConnector>> {
        self.document
            .get(&kind)
            .ok_or_else(|| unsupported_kind(kind))
    }

    pub fn search(&self, kind: DataSourceKind) -> Result<&Arc<dyn SearchConnector>> {
        self.search.get(&kind).ok_or_else(|| unsupported_kind(kind))
    }

    pub fn http_sql(&self, kind: DataSourceKind) -> Result<&Arc<dyn HttpSqlConnector>> {
        self.http_sql
            .get(&kind)
            .ok_or_else(|| unsupported_kind(kind))
    }
}

/// Wraps any `ConnectionTester` and re-tags the resulting report with the
/// brand kind, so MariaDB/TiDB/etc. tests report their own type instead of the
/// underlying protocol driver's kind.
pub struct BrandConnectionTester {
    kind: DataSourceKind,
    inner: Arc<dyn ConnectionTester>,
}

impl BrandConnectionTester {
    pub fn new(kind: DataSourceKind, inner: Arc<dyn ConnectionTester>) -> Self {
        Self { kind, inner }
    }
}

#[async_trait]
impl ConnectionTester for BrandConnectionTester {
    fn kind(&self) -> DataSourceKind {
        self.kind
    }

    async fn test_connection(
        &self,
        profile: &db_common::ConnectionProfile,
        password: Option<&str>,
    ) -> Result<db_common::ConnectionTestReport> {
        let mut report = self.inner.test_connection(profile, password).await?;
        report.kind = self.kind;
        Ok(report)
    }
}

/// Returns a structured `Unsupported` error for kinds without a pure-Rust
/// driver. Lets the user save and edit the config while making the support
/// level explicit at test time.
pub struct RestrictedConnectionTester {
    kind: DataSourceKind,
}

impl RestrictedConnectionTester {
    pub fn new(kind: DataSourceKind) -> Self {
        Self { kind }
    }
}

#[async_trait]
impl ConnectionTester for RestrictedConnectionTester {
    fn kind(&self) -> DataSourceKind {
        self.kind
    }

    async fn test_connection(
        &self,
        profile: &db_common::ConnectionProfile,
        _password: Option<&str>,
    ) -> Result<db_common::ConnectionTestReport> {
        let caps = profile.kind.capabilities();
        let detail = caps
            .notes
            .unwrap_or("no reliable pure-Rust driver available");
        Err(AppError::new(
            ErrorKind::Unsupported,
            format!(
                "{} is {} in this build: {detail}",
                profile.kind.display_label(),
                caps.support.as_str()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_covers_all_kinds_for_testers() {
        let registry = DriverRegistry::default();
        for &kind in DataSourceKind::ALL {
            assert!(
                registry.tester(kind).is_ok(),
                "{kind:?} has no registered tester"
            );
        }
    }

    #[test]
    fn mysql_family_shares_one_driver_instance() {
        let registry = DriverRegistry::default();
        let mysql = registry
            .relational_metadata(DataSourceKind::MySql)
            .expect("mysql metadata");
        for brand in [
            DataSourceKind::MariaDb,
            DataSourceKind::TiDb,
            DataSourceKind::OceanBase,
            DataSourceKind::Doris,
            DataSourceKind::SelectDb,
            DataSourceKind::StarRocks,
            DataSourceKind::GoldenDb,
        ] {
            assert!(
                Arc::ptr_eq(
                    registry.relational_metadata(brand).expect("brand metadata"),
                    mysql
                ),
                "{brand:?} should share the MySQL metadata reader instance"
            );
        }
    }

    #[test]
    fn postgres_family_shares_one_driver_instance() {
        let registry = DriverRegistry::default();
        let pg = registry
            .relational_metadata(DataSourceKind::PostgreSql)
            .expect("pg metadata");
        for brand in [
            DataSourceKind::Redshift,
            DataSourceKind::CockroachDb,
            DataSourceKind::OpenGauss,
            DataSourceKind::KingbaseEs,
        ] {
            assert!(
                Arc::ptr_eq(
                    registry.relational_metadata(brand).expect("brand metadata"),
                    pg
                ),
                "{brand:?} should share the PostgreSQL metadata reader instance"
            );
        }
    }

    #[tokio::test]
    async fn odbc_kinds_route_to_odbc_driver() {
        let registry = DriverRegistry::default();
        for kind in [
            DataSourceKind::Oracle,
            DataSourceKind::Dm,
            DataSourceKind::XuguDb,
            DataSourceKind::Access,
        ] {
            let tester = registry.tester(kind).expect("tester");
            assert_eq!(tester.kind(), kind);
            // Access is file-based so it needs a file_path to pass validation;
            // the other ODBC kinds just need a host.
            let mut builder = db_common::ConnectionProfile::builder(kind, "x");
            if kind.is_file_based() {
                builder = builder.file_path("/tmp/fake");
            } else {
                builder = builder.host("localhost").username("u");
            }
            let profile = builder.build().expect("profile");
            // No vendor ODBC driver is installed in CI, so a connection
            // attempt must fail — but with a Connection/Query error carrying
            // the driver-manager diagnostics, not an Unsupported rejection.
            let result = tester.test_connection(&profile, None).await;
            let err = match result {
                Ok(_) => continue, // a driver happened to be present
                Err(err) => err,
            };
            assert_ne!(
                err.kind,
                ErrorKind::Unsupported,
                "{kind:?} must not be rejected as unsupported anymore"
            );
            assert!(
                matches!(err.kind, ErrorKind::Connection | ErrorKind::Query),
                "{kind:?} unexpected error kind: {:?} ({})",
                err.kind,
                err.message
            );

            let metadata = registry.relational_metadata(kind).expect("metadata");
            assert_eq!(metadata.kind(), kind);
            let query = registry.relational_query(kind).expect("query executor");
            assert_eq!(query.kind(), kind);
        }
    }
}
