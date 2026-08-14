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
        let statement = normalize_doris_identifiers(&statement);
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

/// Doris speaks the MySQL dialect and treats double quotes as string literals.
/// The desktop UI may produce ANSI-quoted identifiers, so normalize only those
/// quoted sections before sending literal SQL over COM_QUERY.
fn normalize_doris_identifiers(sql: &str) -> String {
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
