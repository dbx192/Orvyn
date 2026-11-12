use db_common::{highlight_sql, SqlTokenKind};

/// Format token boundaries only; quoted values, identifiers and comments stay intact.
pub fn format_sql(sql: &str) -> String {
    if sql.trim().is_empty() {
        return sql.to_owned();
    }
    let mut result = String::new();
    let mut end = 0;
    let mut depth = 0usize;
    let mut previous = String::new();
    for token in highlight_sql(sql) {
        let value = &sql[token.range.start..token.range.end];
        let keyword = token.kind == SqlTokenKind::Keyword;
        let upper = value.to_ascii_uppercase();
        let clause = keyword
            && matches!(
                upper.as_str(),
                "SELECT"
                    | "FROM"
                    | "WHERE"
                    | "HAVING"
                    | "LIMIT"
                    | "OFFSET"
                    | "GROUP"
                    | "ORDER"
                    | "SET"
                    | "VALUES"
                    | "UNION"
                    | "RETURNING"
                    | "LEFT"
                    | "RIGHT"
                    | "INNER"
                    | "FULL"
                    | "CROSS"
                    | "JOIN"
            )
            && !(upper == "JOIN"
                && matches!(
                    previous.as_str(),
                    "LEFT" | "RIGHT" | "INNER" | "FULL" | "CROSS" | "OUTER"
                ));
        if clause && !result.is_empty() {
            newline(&mut result, depth);
        } else if !sql[end..token.range.start].is_empty()
            && !result.is_empty()
            && !result.ends_with([' ', '\n'])
        {
            result.push(' ');
        }
        if keyword {
            result.push_str(&upper);
        } else {
            result.push_str(value);
        }
        match value {
            "(" => depth += 1,
            ")" => depth = depth.saturating_sub(1),
            "," if depth == 0 => newline(&mut result, depth),
            ";" => newline(&mut result, 0),
            _ if token.kind == SqlTokenKind::Comment && value.starts_with("--") => {
                newline(&mut result, depth)
            }
            _ => {}
        }
        previous = upper;
        end = token.range.end;
    }
    result.trim_end().to_owned()
}

fn newline(result: &mut String, depth: usize) {
    while result.ends_with([' ', '\t']) {
        result.pop();
    }
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&"  ".repeat(depth));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_clauses_and_is_stable() {
        let formatted = format_sql("select a, b from users where id = 1 order by a");
        assert!(formatted.contains("\nFROM users\nWHERE"));
        assert_eq!(format_sql(&formatted), formatted);
    }
    #[test]
    fn preserves_unicode_quotes_comments_and_dialect_literals() {
        let sql = "select \u{4e2d}\u{6587}, `order`, [from], 'it''s \u{4e2d}\u{6587}', $$a; from\nb$$ -- where \u{4e2d}\u{6587}\nfrom \u{8868} where x = :value";
        let before = highlight_sql(sql);
        let formatted = format_sql(sql);
        let after = highlight_sql(&formatted);
        let tokens = |s: &str, ts: Vec<db_common::SqlHighlightToken>| {
            ts.into_iter()
                .map(|t| (t.kind, s[t.range.start..t.range.end].to_ascii_uppercase()))
                .collect::<Vec<_>>()
        };
        assert_eq!(tokens(sql, before), tokens(&formatted, after));
        assert!(formatted.contains("-- where \u{4e2d}\u{6587}\n"));
    }
}
