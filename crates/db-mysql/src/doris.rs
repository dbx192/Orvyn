//! Doris-family compatibility for the MySQL wire adapter.
//!
//! Doris, SelectDB and StarRocks all derive from Apache Doris and share its
//! MySQL-protocol quirks: no `COM_STMT_PREPARE`, no `information_schema`, no
//! routines / triggers / foreign keys, and the session `sql_mode` must be a
//! literal instead of the SQLx-default `SELECT CONCAT(...)` expression. The
//! helpers in this module are invoked from `MySqlMetadataReader` and
//! `MySqlQueryExecutor` whenever [`crate::MysqlBrand::is_doris_family`] is
//! true, so all three brands are covered by the same code path.

use crate::{
    is_result_set_statement, mysql_query_error, mysql_rows_to_query_result, mysql_string_at,
    QueryResult, Result, TableIndex,
};
use sqlx::{
    mysql::{MySqlConnectOptions, MySqlRow},
    MySqlPool, Row as SqlxRow,
};
use std::time::Instant;

/// Executes literal SQL using COM_QUERY. Doris-family servers do not
/// implement `COM_STMT_PREPARE`, so SQLx's normal prepared-query path cannot
/// be used.
pub(super) async fn execute(pool: MySqlPool, statements: Vec<String>) -> Result<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());

    for statement in statements {
        let statement = normalize_doris_identifiers(&statement);
        let started_at = Instant::now();
        tracing::info!(statement = %statement, "executing Doris-family text-protocol query");
        if is_result_set_statement(&statement) {
            let rows = sqlx::raw_sql(&statement)
                .fetch_all(&pool)
                .await
                .map_err(|error| {
                    tracing::error!(statement = %statement, error = %error, "Doris-family query failed");
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

pub(super) fn configure_connect_options(options: MySqlConnectOptions) -> MySqlConnectOptions {
    options.pipes_as_concat(false).no_engine_substitution(false)
}

pub(super) fn group_indexes(rows: Vec<MySqlRow>) -> Result<Vec<TableIndex>> {
    let mut indexes = Vec::new();
    for row in rows {
        let name = mysql_string_at(&row, 2)?;
        let column = mysql_string_at(&row, 4)?;
        let non_unique: i8 = row.try_get(1).map_err(mysql_query_error)?;
        if let Some(index) = indexes
            .iter_mut()
            .find(|index: &&mut TableIndex| index.name == name)
        {
            index.columns.push(column);
        } else {
            indexes.push(TableIndex {
                primary: name.eq_ignore_ascii_case("PRIMARY"),
                name,
                columns: vec![column],
                unique: non_unique == 0,
            });
        }
    }
    Ok(indexes)
}

/// Doris-family servers speak the MySQL dialect and treat double quotes as
/// string literals. The desktop UI may produce ANSI-quoted identifiers, so
/// normalize only those quoted sections before sending literal SQL over
/// COM_QUERY.
pub(super) fn normalize_doris_identifiers(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut single_quoted = false;
    let mut backtick_quoted = false;
    let mut line_comment = false;
    let mut block_comment = false;

    while let Some(ch) = chars.next() {
        if line_comment {
            output.push(ch);
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            output.push(ch);
            if ch == '*' && chars.peek() == Some(&'/') {
                output.push(chars.next().unwrap());
                block_comment = false;
            }
            continue;
        }
        if single_quoted {
            output.push(ch);
            if ch == '\\' {
                if let Some(escaped) = chars.next() {
                    output.push(escaped);
                }
            } else if ch == '\'' {
                if chars.peek() == Some(&'\'') {
                    output.push(chars.next().unwrap());
                } else {
                    single_quoted = false;
                }
            }
            continue;
        }
        if backtick_quoted {
            output.push(ch);
            if ch == '`' {
                if chars.peek() == Some(&'`') {
                    output.push(chars.next().unwrap());
                } else {
                    backtick_quoted = false;
                }
            }
            continue;
        }

        match ch {
            '-' if chars.peek() == Some(&'-') => {
                output.push(ch);
                output.push(chars.next().unwrap());
                line_comment = true;
            }
            '#' => {
                output.push(ch);
                line_comment = true;
            }
            '/' if chars.peek() == Some(&'*') => {
                output.push(ch);
                output.push(chars.next().unwrap());
                block_comment = true;
            }
            '\'' => {
                output.push(ch);
                single_quoted = true;
            }
            '`' => {
                output.push(ch);
                backtick_quoted = true;
            }
            '"' => {
                output.push('`');
                while let Some(identifier_ch) = chars.next() {
                    match identifier_ch {
                        '"' if chars.peek() == Some(&'"') => {
                            output.push('`');
                            chars.next();
                            output.push('`');
                        }
                        '"' => {
                            output.push('`');
                            break;
                        }
                        '`' => output.push_str("``"),
                        _ => output.push(identifier_ch),
                    }
                }
            }
            _ => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::normalize_doris_identifiers;

    #[test]
    fn converts_ansi_quoted_identifiers_for_doris() {
        assert_eq!(
            normalize_doris_identifiers(
                "SELECT * FROM \"base\".\"country\" ORDER BY \"zh_name\" LIMIT 10"
            ),
            "SELECT * FROM `base`.`country` ORDER BY `zh_name` LIMIT 10"
        );
    }

    #[test]
    fn preserves_literals_and_comments() {
        assert_eq!(
            normalize_doris_identifiers("SELECT '\"name\"' -- \"comment\"\nFROM \"items\""),
            "SELECT '\"name\"' -- \"comment\"\nFROM `items`"
        );
    }
}
