//! Opt-in, read-only tests against disposable CI services. No local config is read.
use db_common::{
    CellValue, ConnectionPoolConfig, ConnectionProfile, ConnectionTester, DataSourceKind,
    RelationalQueryExecutor, SearchConnector,
};
use std::time::Duration;

fn profile(kind: DataSourceKind, prefix: &str) -> ConnectionProfile {
    let host = std::env::var(format!("{prefix}_HOST"))
        .expect("Set the integration service host explicitly");
    let mut profile = ConnectionProfile::builder(kind, "CI service integration")
        .host(host)
        .build()
        .unwrap();
    if let Ok(port) = std::env::var(format!("{prefix}_PORT")) {
        profile.port = Some(port.parse().unwrap());
    }
    if let Ok(user) = std::env::var(format!("{prefix}_USER")) {
        profile.username = Some(user);
    }
    if let Ok(database) = std::env::var(format!("{prefix}_DATABASE")) {
        profile.database = Some(database);
    }
    profile
}
fn password(prefix: &str) -> Option<String> {
    std::env::var(format!("{prefix}_PASSWORD")).ok()
}

#[tokio::test]
#[ignore = "requires explicit ORVYN_TEST_PG_* disposable service variables"]
async fn postgres_query_cancel_and_reuse() {
    let profile = profile(DataSourceKind::PostgreSql, "ORVYN_TEST_PG");
    let password = password("ORVYN_TEST_PG");
    let executor = db_postgres::PostgresQueryExecutor::new();
    let config = ConnectionPoolConfig {
        max_connections: 1,
        ..Default::default()
    };
    let cancellation = db_common::QueryCancellation::default();
    let trigger = cancellation.clone();
    let statements = ["SELECT pg_sleep(30)".into()];
    let query = executor.execute_cancellable(
        &profile,
        password.as_deref(),
        &statements,
        config,
        cancellation,
    );
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        trigger.cancel();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(query, cancel)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    let rows = executor
        .execute(&profile, password.as_deref(), &["SELECT 42".into()], config)
        .await
        .unwrap();
    assert_eq!(rows[0].rows[0].values[0], CellValue::Int(42));
}

#[tokio::test]
#[ignore = "requires explicit ORVYN_TEST_MYSQL_* disposable service variables"]
async fn mysql_query_and_transaction() {
    let profile = profile(DataSourceKind::MySql, "ORVYN_TEST_MYSQL");
    let password = password("ORVYN_TEST_MYSQL");
    let executor = db_mysql::MySqlQueryExecutor::new();
    let config = ConnectionPoolConfig::default();
    let id = db_common::QueryId::new();
    executor
        .begin_transaction(&profile, password.as_deref(), id, config)
        .await
        .unwrap();
    let result = executor
        .execute_in_transaction(id, &["SELECT 42".into()])
        .await
        .unwrap();
    assert_eq!(result[0].rows.len(), 1);
    executor.rollback_transaction(id).await.unwrap();
}

#[tokio::test]
#[ignore = "requires explicit ORVYN_TEST_REDIS_* disposable service variables"]
async fn redis_connection() {
    let profile = profile(DataSourceKind::Redis, "ORVYN_TEST_REDIS");
    let password = password("ORVYN_TEST_REDIS");
    db_redis::RedisConnectionTester
        .test_connection(&profile, password.as_deref())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires explicit ORVYN_TEST_ES_* disposable service variables"]
async fn elasticsearch_preserves_raw_response_and_aggregations() {
    let profile = profile(DataSourceKind::Elasticsearch, "ORVYN_TEST_ES");
    let password = password("ORVYN_TEST_ES");
    let result = db_http::ElasticsearchSearchConnector::new()
        .search(
            &profile,
            password.as_deref(),
            "_all",
            &serde_json::json!({"size":0,"aggs":{"indices":{"terms":{"field":"_index"}}}}),
            10,
        )
        .await
        .unwrap();
    let raw = result.raw_response.expect("raw ES response");
    assert!(raw.get("took").is_some());
    assert!(raw.get("aggregations").is_some());
}
