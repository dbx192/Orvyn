pub fn trim_sql(sql: &str) -> &str {
    sql.trim_matches(|ch: char| ch.is_whitespace() || ch == ';')
}

pub fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut line_comment = false;
    let mut block_comment = false;

    while let Some(ch) = chars.next() {
        let next = chars.peek().copied();

        if line_comment {
            current.push(ch);
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }

        if block_comment {
            current.push(ch);
            if ch == '*' && next == Some('/') {
                current.push('/');
                chars.next();
                block_comment = false;
            }
            continue;
        }

        if single_quoted {
            current.push(ch);
            if ch == '\'' {
                if next == Some('\'') {
                    current.push('\'');
                    chars.next();
                } else {
                    single_quoted = false;
                }
            }
            continue;
        }

        if double_quoted {
            current.push(ch);
            if ch == '"' {
                if next == Some('"') {
                    current.push('"');
                    chars.next();
                } else {
                    double_quoted = false;
                }
            }
            continue;
        }

        match (ch, next) {
            ('-', Some('-')) => {
                current.push(ch);
                current.push('-');
                chars.next();
                line_comment = true;
            }
            ('/', Some('*')) => {
                current.push(ch);
                current.push('*');
                chars.next();
                block_comment = true;
            }
            ('\'', _) => {
                current.push(ch);
                single_quoted = true;
            }
            ('"', _) => {
                current.push(ch);
                double_quoted = true;
            }
            (';', _) => {
                let statement = trim_sql(&current);
                if !statement.is_empty() {
                    statements.push(statement.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let statement = trim_sql(&current);
    if !statement.is_empty() {
        statements.push(statement.to_string());
    }

    statements
}

pub fn statement_at_offset(sql: &str, byte_offset: usize) -> Option<String> {
    let mut current_start = 0;
    let mut current = String::new();
    let mut chars = sql.char_indices().peekable();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let offset = byte_offset.min(sql.len());

    while let Some((index, ch)) = chars.next() {
        let next = chars.peek().map(|(_, ch)| *ch);

        if line_comment {
            current.push(ch);
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }

        if block_comment {
            current.push(ch);
            if ch == '*' && next == Some('/') {
                current.push('/');
                chars.next();
                block_comment = false;
            }
            continue;
        }

        if single_quoted {
            current.push(ch);
            if ch == '\'' {
                if next == Some('\'') {
                    current.push('\'');
                    chars.next();
                } else {
                    single_quoted = false;
                }
            }
            continue;
        }

        if double_quoted {
            current.push(ch);
            if ch == '"' {
                if next == Some('"') {
                    current.push('"');
                    chars.next();
                } else {
                    double_quoted = false;
                }
            }
            continue;
        }

        match (ch, next) {
            ('-', Some('-')) => {
                current.push(ch);
                current.push('-');
                chars.next();
                line_comment = true;
            }
            ('/', Some('*')) => {
                current.push(ch);
                current.push('*');
                chars.next();
                block_comment = true;
            }
            ('\'', _) => {
                current.push(ch);
                single_quoted = true;
            }
            ('"', _) => {
                current.push(ch);
                double_quoted = true;
            }
            (';', _) => {
                if current_start <= offset && offset <= index {
                    return non_empty_statement(&current);
                }
                current.clear();
                current_start = index + ch.len_utf8();
            }
            _ => current.push(ch),
        }
    }

    if current_start <= offset {
        return non_empty_statement(&current);
    }

    None
}

pub fn is_result_set_statement(sql: &str) -> bool {
    let first = trim_sql(sql)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    matches!(
        first.as_str(),
        "select" | "show" | "describe" | "desc" | "explain" | "with" | "pragma" | "values"
    )
}

pub fn statement_summary(sql: &str, max_chars: usize) -> String {
    let normalized = trim_sql(sql)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if normalized.chars().count() <= max_chars {
        return normalized;
    }

    let mut summary = normalized.chars().take(max_chars).collect::<String>();
    summary.push_str("...");
    summary
}

pub fn looks_like_unqualified_mutation(sql: &str) -> bool {
    let normalized = trim_sql(sql).to_ascii_lowercase();
    let mut words = normalized.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };

    matches!(first, "update" | "delete") && !normalized.contains(" where ")
}

fn non_empty_statement(sql: &str) -> Option<String> {
    let statement = trim_sql(sql);
    (!statement.is_empty()).then(|| statement.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unqualified_mutation() {
        assert!(looks_like_unqualified_mutation(
            "update users set name = 'a'"
        ));
        assert!(looks_like_unqualified_mutation("delete from users"));
        assert!(!looks_like_unqualified_mutation(
            "delete from users where id = 1"
        ));
    }

    #[test]
    fn summarizes_statement() {
        assert_eq!(
            statement_summary(" select  *  from users ; ", 30),
            "select * from users"
        );
    }

    #[test]
    fn splits_statements_without_splitting_quoted_semicolons() {
        assert_eq!(
            split_statements("select ';'; select \"a;\"; -- ;\nselect 3")
                .into_iter()
                .map(|sql| statement_summary(&sql, 100))
                .collect::<Vec<_>>(),
            vec!["select ';'", "select \"a;\"", "-- ; select 3"]
        );
    }

    #[test]
    fn finds_statement_at_offset() {
        let sql = "select 1;\nselect 2;";
        assert_eq!(statement_at_offset(sql, 0).as_deref(), Some("select 1"));
        assert_eq!(statement_at_offset(sql, 12).as_deref(), Some("select 2"));
    }
}
