//! Opt-in, read-only tests against disposable CI services. No local config is read.
use db_common::{
    CellValue, ConnectionPoolConfig, ConnectionProfile, ConnectionTester, DataSourceKind,
    RelationalQueryExecutor, SearchConnector, SslMode,
};
use std::time::Duration;

fn profile(kind: DataSourceKind, prefix: &str) -> ConnectionProfile {
    let host = std::env::var(format!("{prefix}_HOST"))
        .expect("Set the integration service host explicitly");
    let mut builder = ConnectionProfile::builder(kind, "CI service integration").host(host);
    if let Ok(port) = std::env::var(format!("{prefix}_PORT")) {
        builder = builder.port(port.parse().unwrap());
    }
    if let Ok(user) = std::env::var(format!("{prefix}_USER")) {
        builder = builder.username(user);
    }
    if let Ok(database) = std::env::var(format!("{prefix}_DATABASE")) {
        builder = builder.database(database);
    }
    if let Ok(mode) = std::env::var(format!("{prefix}_SSL_MODE")) {
        builder = builder.ssl_mode(match mode.as_str() {
            "disable" => SslMode::Disable,
            "prefer" => SslMode::Prefer,
            "require" => SslMode::Require,
            "verify_ca" => SslMode::VerifyCa,
            "verify_full" => SslMode::VerifyFull,
            other => panic!("unsupported {prefix}_SSL_MODE: {other}"),
        });
    }
    builder.build().unwrap()
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

/// Stream a result larger than two batches and verify that aborting a consumer
/// releases a pool of size one for the next query.
async fn check_export_stream(
    executor: &dyn RelationalQueryExecutor,
    profile: &ConnectionProfile,
    password: Option<&str>,
    sql: &str,
) {
    let config = ConnectionPoolConfig {
        max_connections: 1,
        ..Default::default()
    };
    let mut sizes = Vec::new();
    let mut count = 0;
    executor
        .stream_export(
            profile,
            password,
            sql,
            config,
            Default::default(),
            &mut |batch| {
                sizes.push(batch.rows.len());
                count += batch.rows.len();
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(sizes, [5000, 5000, 2005]);
    assert_eq!(count, 12005);
    let error = executor
        .stream_export(
            profile,
            password,
            sql,
            config,
            Default::default(),
            &mut |_| Err(db_common::AppError::internal("consumer stopped")),
        )
        .await
        .unwrap_err();
    assert_eq!(error.message, "consumer stopped");
    let cancellation = db_common::QueryCancellation::default();
    let mut batches = 0;
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        executor.stream_export(
            profile,
            password,
            sql,
            config,
            cancellation.clone(),
            &mut |_| {
                batches += 1;
                cancellation.cancel();
                Ok(())
            },
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind, db_common::ErrorKind::Cancelled);
    assert_eq!(batches, 1);

    // Cancel while waiting for the server, before any batch is available.
    let cancellation = db_common::QueryCancellation::default();
    let slow_sql = match executor.kind() {
        DataSourceKind::MySql => "SELECT SLEEP(30)",
        DataSourceKind::PostgreSql => "SELECT pg_sleep(30)",
        _ => unreachable!(),
    };
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancellation.cancel();
    };
    let stream = async {
        executor
            .stream_export(
                profile,
                password,
                slow_sql,
                config,
                cancellation.clone(),
                &mut |_| panic!("cancelled slow query produced a batch"),
            )
            .await
    };
    let (_, result) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(cancel, stream)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().kind, db_common::ErrorKind::Cancelled);
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        executor.execute(profile, password, &["SELECT 42".into()], config),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result[0].rows[0].values[0], CellValue::Int(42));
}

#[tokio::test]
#[ignore = "requires explicit ORVYN_TEST_MYSQL_* disposable service variables"]
async fn mysql_streams_export_batches_and_recovers_after_write_failure() {
    let profile = profile(DataSourceKind::MySql, "ORVYN_TEST_MYSQL");
    let password = password("ORVYN_TEST_MYSQL");
    let sql = "WITH d AS (SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) SELECT a.n + b.n*10 + c.n*100 + e.n*1000 + f.n*10000 AS n FROM d a CROSS JOIN d b CROSS JOIN d c CROSS JOIN d e CROSS JOIN d f LIMIT 12005";
    check_export_stream(
        &db_mysql::MySqlQueryExecutor::new(),
        &profile,
        password.as_deref(),
        sql,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires explicit ORVYN_TEST_PG_* disposable service variables"]
async fn postgres_streams_export_batches_and_recovers_after_write_failure() {
    let profile = profile(DataSourceKind::PostgreSql, "ORVYN_TEST_PG");
    let password = password("ORVYN_TEST_PG");
    check_export_stream(
        &db_postgres::PostgresQueryExecutor::new(),
        &profile,
        password.as_deref(),
        "SELECT generate_series(1, 12005)",
    )
    .await;
}
