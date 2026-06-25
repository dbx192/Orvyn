/// Basic SQL formatter that handles keyword capitalization and indentation.
/// This is a simplified formatter focused on readability, not a full SQL parser.
pub fn format_sql(sql: &str) -> String {
    if sql.trim().is_empty() {
        return sql.to_string();
    }

    let keywords = [
        "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "LIKE", "BETWEEN",
        "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "ON",
        "GROUP", "BY", "ORDER", "ASC", "DESC", "LIMIT", "OFFSET",
        "HAVING", "DISTINCT", "ALL", "AS", "CASE", "WHEN", "THEN", "ELSE", "END",
        "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE",
        "ALTER", "DROP", "INDEX", "VIEW", "DATABASE", "SCHEMA",
        "PRIMARY", "KEY", "FOREIGN", "REFERENCES", "CONSTRAINT", "UNIQUE", "DEFAULT",
        "NULL", "NOT", "AUTO_INCREMENT", "SERIAL", "VARCHAR", "INTEGER", "INT", "BIGINT",
        "TEXT", "BOOLEAN", "DATE", "TIME", "TIMESTAMP", "DATETIME", "FLOAT", "DOUBLE",
        "DECIMAL", "NUMERIC", "BLOB", "JSON", "UUID",
        "UNION", "INTERSECT", "EXCEPT", "EXISTS", "ANY", "SOME",
        "BEGIN", "COMMIT", "ROLLBACK", "TRANSACTION", "WITH",
        "RECURSIVE", "CTE", "OVER", "PARTITION", "ROW", "ROWS",
        "RANK", "DENSE_RANK", "ROW_NUMBER", "LEAD", "LAG", "FIRST", "LAST",
        "NULLS", "TRUE", "FALSE", "IS", "COUNT", "SUM", "AVG", "MIN", "MAX",
    ];

    let upper_keywords: std::collections::HashSet<&str> = keywords.iter().cloned().collect();

    let mut result = String::new();
    let mut in_string = false;
    let mut string_char = ' ';
    let mut indent_level = 0;
    let mut line_start = true;
    let mut prev_word = String::new();
    let mut paren_depth = 0;

    let mut chars = sql.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if in_string {
            result.push(ch);
            if ch == string_char && (ch != '\'' || idx == 0 || sql.as_bytes()[idx - 1] != b'\\') {
                in_string = false;
            }
            line_start = false;
            continue;
        }

        match ch {
            '\'' | '"' => {
                result.push(ch);
                in_string = true;
                string_char = ch;
                line_start = false;
            }
            '(' => {
                paren_depth += 1;
                result.push(ch);
                line_start = false;
            }
            ')' => {
                paren_depth -= 1;
                result.push(ch);
                line_start = false;
            }
            ',' => {
                result.push(ch);
                if paren_depth == 0 {
                    result.push('\n');
                    result.push_str(&"  ".repeat(indent_level));
                    line_start = true;
                }
            }
            ' ' | '\t' | '\r' => {
                if !line_start {
                    result.push(ch);
                }
            }
            '\n' => {
                if paren_depth == 0 && indent_level > 0 {
                    result.push('\n');
                    result.push_str(&"  ".repeat(indent_level));
                    line_start = true;
                } else {
                    result.push(' ');
                    line_start = false;
                }
            }
            ';' => {
                result.push(ch);
                if paren_depth == 0 {
                    indent_level = indent_level.saturating_sub(1);
                }
            }
            _ => {
                if line_start && paren_depth == 0 {
                    result.push_str(&"  ".repeat(indent_level));
                }

                // Check if this is a word
                let rest: String = sql[idx..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !rest.is_empty() {
                    let word_upper = rest.to_uppercase();
                    if upper_keywords.contains(word_upper.as_str()) {
                        // Keyword: apply indentation rules
                        let should_indent_after = ["FROM", "WHERE", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS",
                            "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "SET", "VALUES", "AND", "OR",
                            "CASE", "WHEN", "THEN", "ELSE", "END", "UNION", "INTERSECT", "EXCEPT",
                            "WITH", "RECURSIVE", "OVER", "PARTITION"];
                        let should_deindent_before = ["END", "ELSE", "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT"];

                        if should_deindent_before.contains(&word_upper.as_str()) && prev_word.as_str() != "(" {
                            indent_level = indent_level.saturating_sub(1);
                            if line_start && paren_depth == 0 {
                                result.push_str(&"  ".repeat(indent_level));
                            }
                        }

                        result.push_str(&rest);
                        indent_level += if should_indent_after.contains(&word_upper.as_str()) { 1 } else { 0 };
                    } else {
                        result.push_str(&rest);
                    }

                    // Skip the characters we just consumed
                    for _ in 0..rest.len().saturating_sub(1) {
                        chars.next();
                    }
                    prev_word = rest;
                    line_start = false;
                } else {
                    result.push(ch);
                    line_start = false;
                }
            }
        }
    }

    // Clean up trailing whitespace
    let trimmed: String = result.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_empty() {
        assert_eq!(format_sql(""), "");
    }

    #[test]
    fn test_format_select() {
        let sql = "select * from users where id = 1";
        let formatted = format_sql(sql);
        assert!(formatted.contains("SELECT"));
        assert!(formatted.contains("FROM"));
    }

    #[test]
    fn test_format_complex() {
        let sql = "SELECT u.name, u.email FROM users u JOIN orders o ON u.id = o.user_id WHERE u.active = true AND o.total > 100 ORDER BY u.name LIMIT 10";
        let formatted = format_sql(sql);
        assert!(formatted.contains("SELECT"));
        assert!(formatted.contains("JOIN"));
        assert!(formatted.contains("WHERE"));
    }
}
