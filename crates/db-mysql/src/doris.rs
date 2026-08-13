//! Apache Doris compatibility for the MySQL wire adapter.

use crate::{
    is_result_set_statement, mysql_query_error, mysql_rows_to_query_result, QueryResult, Result,
};
use sqlx::MySqlPool;
use std::time::Instant;

/// Executes literal SQL using COM_QUERY. Doris does not implement
/// COM_STMT_PREPARE, so SQLx's normal prepared-query path cannot be used.
pub(super) async fn execute(pool: MySqlPool, statements: Vec<String>) -> Result<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());

    for statement in statements {
        let started_at = Instant::now();
        tracing::info!(statement = %statement, "executing Doris text-protocol query");
        if is_result_set_statement(&statement) {
            let rows = sqlx::raw_sql(&statement)
                .fetch_all(&pool)
                .await
                .map_err(|error| {
                    tracing::error!(statement = %statement, error = %error, "Doris query failed");
                    mysql_query_error(error)
                })?;
            results.push(
                mysql_rows_to_query_result(&statement, rows).with_elapsed(started_at.elapsed()),
            );
        } else {
            let done = sqlx::raw_sql(&statement)
                .execute(&pool)
                .await
                .map_err(mysql_query_error)?;
            let mut result = QueryResult::empty(&statement).with_elapsed(started_at.elapsed());
            result.affected_rows = done.rows_affected();
            results.push(result);
        }
    }

    Ok(results)
}
