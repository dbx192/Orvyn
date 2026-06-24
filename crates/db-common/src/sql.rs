use crate::{SqlHighlightToken, SqlTokenKind, TextRange};

pub fn trim_sql(sql: &str) -> &str {
    sql.trim_matches(|ch: char| ch.is_whitespace() || ch == ';')
}

pub fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = sql.char_indices().peekable();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut line_comment = false;
    let mut block_comment = false;

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
            ('$', _) => {
                if let Some(delimiter) = dollar_quote_delimiter(sql, index) {
                    current.push(ch);
                    consume_dollar_quoted_into(sql, &mut chars, &delimiter, &mut current);
                } else {
                    current.push(ch);
                }
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
            ('$', _) => {
                if let Some(delimiter) = dollar_quote_delimiter(sql, index) {
                    current.push(ch);
                    consume_dollar_quoted_into(sql, &mut chars, &delimiter, &mut current);
                } else {
                    current.push(ch);
                }
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

pub fn highlight_sql(sql: &str) -> Vec<SqlHighlightToken> {
    let mut tokens = Vec::new();
    let mut chars = sql.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }

        let next = chars.peek().map(|(_, ch)| *ch);
        match (ch, next) {
            ('-', Some('-')) => {
                chars.next();
                let end = consume_until_line_end(sql, &mut chars);
                tokens.push(highlight_token(start, end, SqlTokenKind::Comment));
            }
            ('/', Some('*')) => {
                chars.next();
                let end = consume_block_comment(sql, &mut chars);
                tokens.push(highlight_token(start, end, SqlTokenKind::Comment));
            }
            ('\'', _) => {
                let end = consume_quoted(sql, &mut chars, '\'');
                tokens.push(highlight_token(start, end, SqlTokenKind::StringLiteral));
            }
            ('"', _) | ('`', _) => {
                let end = consume_quoted(sql, &mut chars, ch);
                tokens.push(highlight_token(start, end, SqlTokenKind::QuotedIdentifier));
            }
            ('[', _) => {
                let end = consume_bracket_identifier(sql, &mut chars);
                tokens.push(highlight_token(start, end, SqlTokenKind::QuotedIdentifier));
            }
            ('$', _) if dollar_quote_delimiter(sql, start).is_some() => {
                let Some(delimiter) = dollar_quote_delimiter(sql, start) else {
                    continue;
                };
                let end = consume_dollar_quoted_range(sql, &mut chars, &delimiter);
                tokens.push(highlight_token(start, end, SqlTokenKind::StringLiteral));
            }
            (':', Some(next)) if is_identifier_start(next) => {
                let end = consume_identifier(sql, &mut chars);
                tokens.push(highlight_token(start, end, SqlTokenKind::Parameter));
            }
            ('?', _) | ('$', _) => {
                let end = consume_parameter(sql, &mut chars, start, ch);
                tokens.push(highlight_token(start, end, SqlTokenKind::Parameter));
            }
            (ch, _) if ch.is_ascii_digit() => {
                let end = consume_number(sql, &mut chars);
                tokens.push(highlight_token(start, end, SqlTokenKind::Number));
            }
            (ch, _) if is_identifier_start(ch) => {
                let end = consume_identifier(sql, &mut chars);
                let value = &sql[start..end];
                let kind = if is_sql_keyword(value) {
                    SqlTokenKind::Keyword
                } else if next_non_whitespace(sql, end) == Some('(') {
                    SqlTokenKind::Function
                } else {
                    SqlTokenKind::Identifier
                };
                tokens.push(highlight_token(start, end, kind));
            }
            (ch, _) if is_punctuation(ch) => {
                let end = start + ch.len_utf8();
                tokens.push(highlight_token(start, end, SqlTokenKind::Punctuation));
            }
            _ => {
                let end = consume_operator(&mut chars, start, ch);
                tokens.push(highlight_token(start, end, SqlTokenKind::Operator));
            }
        }
    }

    tokens
}

fn non_empty_statement(sql: &str) -> Option<String> {
    let statement = trim_sql(sql);
    (!statement.is_empty()).then(|| statement.to_string())
}

fn highlight_token(start: usize, end: usize, kind: SqlTokenKind) -> SqlHighlightToken {
    SqlHighlightToken {
        range: TextRange::new(start, end),
        kind,
    }
}

fn consume_until_line_end(
    sql: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> usize {
    while let Some((index, ch)) = chars.next() {
        if ch == '\n' {
            return index;
        }
    }
    sql.len()
}

fn consume_block_comment(
    sql: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> usize {
    while let Some((index, ch)) = chars.next() {
        if ch == '*' && chars.peek().map(|(_, ch)| *ch) == Some('/') {
            chars.next();
            return index + 2;
        }
    }
    sql.len()
}

fn dollar_quote_delimiter(sql: &str, start: usize) -> Option<String> {
    let rest = sql.get(start..)?;
    if !rest.starts_with('$') {
        return None;
    }

    let mut end = start + 1;
    for ch in rest[1..].chars() {
        if ch == '$' {
            return Some(sql[start..end + ch.len_utf8()].to_string());
        }
        if !(ch == '_' || ch.is_ascii_alphanumeric()) {
            return None;
        }
        end += ch.len_utf8();
    }
    None
}

fn consume_dollar_quoted_into(
    sql: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    delimiter: &str,
    current: &mut String,
) {
    for _ in 1..delimiter.chars().count() {
        if let Some((_, ch)) = chars.next() {
            current.push(ch);
        }
    }

    while let Some((index, ch)) = chars.next() {
        if sql[index..].starts_with(delimiter) {
            current.push(ch);
            for _ in 1..delimiter.chars().count() {
                if let Some((_, delimiter_ch)) = chars.next() {
                    current.push(delimiter_ch);
                }
            }
            return;
        }
        current.push(ch);
    }
}

fn consume_dollar_quoted_range(
    sql: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    delimiter: &str,
) -> usize {
    for _ in 1..delimiter.chars().count() {
        chars.next();
    }

    while let Some((index, _)) = chars.next() {
        if sql[index..].starts_with(delimiter) {
            let end = index + delimiter.len();
            for _ in 1..delimiter.chars().count() {
                chars.next();
            }
            return end;
        }
    }
    sql.len()
}

fn consume_quoted(
    sql: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    quote: char,
) -> usize {
    while let Some((index, ch)) = chars.next() {
        if ch == quote {
            if chars.peek().map(|(_, next)| *next) == Some(quote) {
                chars.next();
                continue;
            }
            return index + ch.len_utf8();
        }
    }
    sql.len()
}

fn consume_bracket_identifier(
    sql: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> usize {
    while let Some((index, ch)) = chars.next() {
        if ch == ']' {
            return index + ch.len_utf8();
        }
    }
    sql.len()
}

fn consume_identifier(
    sql: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> usize {
    while let Some((index, ch)) = chars.peek().copied() {
        if is_identifier_continue(ch) {
            chars.next();
            continue;
        }
        return index;
    }
    sql.len()
}

fn consume_parameter(
    sql: &str,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    start: usize,
    prefix: char,
) -> usize {
    if prefix == '?' {
        return chars
            .next_if(|(_, ch)| ch.is_ascii_digit())
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(start + prefix.len_utf8());
    }

    while let Some((index, ch)) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            chars.next();
            continue;
        }
        return index;
    }
    sql.len()
}

fn consume_number(sql: &str, chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> usize {
    while let Some((index, ch)) = chars.peek().copied() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_') {
            chars.next();
            continue;
        }
        return index;
    }
    sql.len()
}

fn consume_operator(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    start: usize,
    first: char,
) -> usize {
    if let Some((index, ch)) = chars.peek().copied() {
        let two_char_operator = matches!(
            (first, ch),
            ('<', '=') | ('>', '=') | ('!', '=') | ('<', '>') | ('|', '|') | (':', ':')
        );
        if two_char_operator {
            chars.next();
            return index + ch.len_utf8();
        }
    }

    start + first.len_utf8()
}

fn next_non_whitespace(sql: &str, offset: usize) -> Option<char> {
    sql.get(offset..)?.chars().find(|ch| !ch.is_whitespace())
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_alphanumeric()
}

fn is_punctuation(ch: char) -> bool {
    matches!(ch, '(' | ')' | ',' | ';' | '.')
}

fn is_sql_keyword(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "ADD"
            | "ALL"
            | "ALTER"
            | "AND"
            | "AS"
            | "ASC"
            | "BEGIN"
            | "BETWEEN"
            | "BY"
            | "CASE"
            | "COMMIT"
            | "CREATE"
            | "CROSS"
            | "DATABASE"
            | "DELETE"
            | "DESC"
            | "DESCRIBE"
            | "DISTINCT"
            | "DROP"
            | "ELSE"
            | "END"
            | "EXISTS"
            | "EXPLAIN"
            | "FALSE"
            | "FROM"
            | "FULL"
            | "GROUP"
            | "HAVING"
            | "IF"
            | "IN"
            | "INDEX"
            | "INNER"
            | "INSERT"
            | "INTO"
            | "IS"
            | "JOIN"
            | "KEY"
            | "LEFT"
            | "LIKE"
            | "LIMIT"
            | "NOT"
            | "NULL"
            | "OFFSET"
            | "ON"
            | "OR"
            | "ORDER"
            | "OUTER"
            | "PRIMARY"
            | "PRAGMA"
            | "REFERENCES"
            | "RETURNING"
            | "RIGHT"
            | "ROLLBACK"
            | "SELECT"
            | "SET"
            | "SHOW"
            | "TABLE"
            | "THEN"
            | "TRUE"
            | "UNION"
            | "UPDATE"
            | "VALUES"
            | "VIEW"
            | "WHEN"
            | "WHERE"
            | "WITH"
    )
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
    fn splits_statements_without_splitting_dollar_quoted_strings() {
        assert_eq!(
            split_statements("select $$a;b$$; select $tag$c;d$tag$; select 3"),
            vec!["select $$a;b$$", "select $tag$c;d$tag$", "select 3"]
        );
    }

    #[test]
    fn finds_statement_at_offset() {
        let sql = "select 1;\nselect 2;";
        assert_eq!(statement_at_offset(sql, 0).as_deref(), Some("select 1"));
        assert_eq!(statement_at_offset(sql, 12).as_deref(), Some("select 2"));
    }

    #[test]
    fn finds_statement_at_offset_inside_dollar_quoted_string() {
        let sql = "select $$a;b$$;\nselect 2;";
        assert_eq!(
            statement_at_offset(sql, 10).as_deref(),
            Some("select $$a;b$$")
        );
    }

    #[test]
    fn highlights_sql_tokens_with_byte_ranges() {
        let sql = "select count(*) from users where name = 'a;'; -- tail";
        let tokens = highlight_sql(sql);

        assert_eq!(tokens[0].kind, SqlTokenKind::Keyword);
        assert_eq!(&sql[tokens[0].range.start..tokens[0].range.end], "select");
        assert!(tokens
            .iter()
            .any(|token| token.kind == SqlTokenKind::Function
                && &sql[token.range.start..token.range.end] == "count"));
        assert!(tokens
            .iter()
            .any(|token| token.kind == SqlTokenKind::StringLiteral
                && &sql[token.range.start..token.range.end] == "'a;'"));
        assert!(tokens
            .iter()
            .any(|token| token.kind == SqlTokenKind::Comment
            && &sql[token.range.start..token.range.end] == "-- tail"));
    }

    #[test]
    fn highlights_dollar_quoted_string_as_one_token() {
        let sql = "select $tag$a;b$tag$";
        let tokens = highlight_sql(sql);

        assert!(tokens
            .iter()
            .any(|token| token.kind == SqlTokenKind::StringLiteral
                && &sql[token.range.start..token.range.end] == "$tag$a;b$tag$"));
    }

    #[test]
    fn highlights_parameters_and_quoted_identifiers() {
        let sql = r#"select "user", `group` from [table] where id = $1 and name = :name and x = ?"#;
        let tokens = highlight_sql(sql);

        assert!(tokens
            .iter()
            .any(|token| token.kind == SqlTokenKind::QuotedIdentifier
                && &sql[token.range.start..token.range.end] == r#""user""#));
        assert_eq!(
            tokens
                .iter()
                .filter(|token| token.kind == SqlTokenKind::Parameter)
                .count(),
            3
        );
    }
}
