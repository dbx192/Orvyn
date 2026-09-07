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
    let mut backtick_quoted = false;
    let mut line_comment = false;
    let mut block_comment = false;
    // mysql CLI / Navicat `DELIMITER` is a client command, not server SQL.
    // Track it so trigger bodies terminated with `//` (etc.) stay intact and
    // the DELIMITER lines are never sent to the server.
    let mut terminator = ";".to_string();

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

        if backtick_quoted {
            current.push(ch);
            if ch == '`' {
                if next == Some('`') {
                    current.push('`');
                    chars.next();
                } else {
                    backtick_quoted = false;
                }
            }
            continue;
        }

        if current.trim().is_empty() {
            if let Some((end, new_terminator)) = parse_delimiter_command(sql, index) {
                terminator = new_terminator;
                current.clear();
                while chars.peek().is_some_and(|(next_index, _)| *next_index < end) {
                    chars.next();
                }
                continue;
            }
        }

        if !terminator.is_empty() && sql[index..].starts_with(&terminator) {
            if terminator == ";" && is_unfinished_procedural_ddl(&current) {
                current.push(ch);
                continue;
            }
            let statement = trim_sql(&current);
            if !statement.is_empty() {
                statements.push(statement.to_string());
            }
            current.clear();
            let mut skipped = ch.len_utf8();
            while skipped < terminator.len() {
                match chars.next() {
                    Some((_, extra)) => skipped += extra.len_utf8(),
                    None => break,
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
            ('`', _) => {
                current.push(ch);
                backtick_quoted = true;
            }
            ('$', _) => {
                if let Some(delimiter) = dollar_quote_delimiter(sql, index) {
                    current.push(ch);
                    consume_dollar_quoted_into(sql, &mut chars, &delimiter, &mut current);
                } else {
                    current.push(ch);
                }
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

fn parse_delimiter_command(sql: &str, index: usize) -> Option<(usize, String)> {
    let rest = sql.get(index..)?;
    const KEYWORD: &str = "delimiter";
    let prefix = rest.get(..KEYWORD.len())?;
    if !prefix.eq_ignore_ascii_case(KEYWORD) {
        return None;
    }
    let mut pos = index + KEYWORD.len();
    let bytes = sql.as_bytes();
    if pos < sql.len() {
        let next = bytes[pos];
        if next.is_ascii_alphanumeric() || next == b'_' {
            return None;
        }
    }
    while pos < sql.len() && matches!(bytes[pos], b' ' | b'\t') {
        pos += 1;
    }
    if pos >= sql.len() || matches!(bytes[pos], b'\n' | b'\r') {
        return None;
    }
    let delim_start = pos;
    while pos < sql.len() && !matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    let new_terminator = sql.get(delim_start..pos)?.to_string();
    if new_terminator.is_empty() {
        return None;
    }
    while pos < sql.len() {
        let c = bytes[pos];
        pos += 1;
        if c == b'\n' {
            break;
        }
        if c == b'\r' {
            if pos < sql.len() && bytes[pos] == b'\n' {
                pos += 1;
            }
            break;
        }
    }
    Some((pos, new_terminator))
}

fn is_unfinished_procedural_ddl(current: &str) -> bool {
    is_procedural_ddl(current) && procedural_begin_depth(current) > 0
}

fn is_procedural_ddl(sql: &str) -> bool {
    let tokens = sql_keyword_tokens(sql);
    let mut rest = tokens.as_slice();
    if rest.first().map(String::as_str) != Some("CREATE") {
        return false;
    }
    rest = &rest[1..];
    if rest.len() >= 2 && rest[0] == "OR" && rest[1] == "REPLACE" {
        rest = &rest[2..];
    }
    matches!(
        rest.first().map(String::as_str),
        Some("TRIGGER" | "FUNCTION" | "PROCEDURE" | "EVENT")
    )
}

fn procedural_begin_depth(sql: &str) -> i32 {
    let tokens = sql_keyword_tokens(sql);
    let mut depth = 0i32;
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "BEGIN" => depth += 1,
            "END" => {
                let next = tokens.get(i + 1).map(String::as_str).unwrap_or("");
                if !matches!(next, "IF" | "LOOP" | "CASE" | "WHILE" | "REPEAT") {
                    depth -= 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    depth
}

fn sql_keyword_tokens(sql: &str) -> Vec<String> {
    sql.to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn statement_at_offset(sql: &str, byte_offset: usize) -> Option<String> {
    let mut current_start = 0;
    let mut current = String::new();
    let mut chars = sql.char_indices().peekable();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut previous_statement = None;
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
                let statement = non_empty_statement(&current);
                if current_start <= offset && offset <= index {
                    return statement;
                }
                if statement.is_some() {
                    previous_statement = statement;
                }
                current.clear();
                current_start = index + ch.len_utf8();
            }
            _ => current.push(ch),
        }
    }

    if current_start <= offset {
        if let Some(statement) = non_empty_statement(&current) {
            return Some(statement);
        }

        if sql
            .get(current_start..offset)
            .map(|value| value.trim().is_empty())
            .unwrap_or(false)
        {
            return previous_statement;
        }
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

pub fn limit_result_preview_statement(sql: &str, limit: u32) -> Option<String> {
    if limit == 0 {
        return None;
    }

    let statement = trim_sql(sql);
    if !is_limitable_result_query(statement) {
        return None;
    }

    let words = top_level_sql_words(statement);
    if words.iter().any(|word| {
        matches!(
            word.value.as_str(),
            "LIMIT" | "OFFSET" | "FETCH" | "FOR" | "INTO"
        )
    }) {
        return None;
    }

    Some(format!("{statement}\nLIMIT {limit}"))
}

pub fn page_result_statement(sql: &str, limit: u32, offset: u64) -> Option<String> {
    if limit == 0 {
        return None;
    }

    let statement = trim_sql(sql);
    if !is_pageable_result_query(statement) {
        return None;
    }
    if has_explicit_page_clause(statement) {
        return None;
    }

    Some(format!(
        "SELECT * FROM ({statement}) AS __query_page LIMIT {limit} OFFSET {offset}"
    ))
}

pub fn count_result_page_statement(sql: &str) -> Option<String> {
    let statement = trim_sql(sql);
    if !is_pageable_result_query(statement) {
        return None;
    }
    if has_explicit_page_clause(statement) {
        return None;
    }

    Some(format!(
        "SELECT COUNT(*) AS total_rows FROM ({statement}) AS __query_count"
    ))
}

fn has_explicit_page_clause(statement: &str) -> bool {
    top_level_sql_words(statement)
        .iter()
        .any(|word| matches!(word.value.as_str(), "LIMIT" | "OFFSET" | "FETCH"))
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
    for (index, ch) in chars.by_ref() {
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
    for (index, ch) in chars.by_ref() {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct SqlWord {
    value: String,
}

fn is_limitable_result_query(sql: &str) -> bool {
    let words = top_level_sql_words(sql);
    let Some(first) = words.first() else {
        return false;
    };

    if first.value == "SELECT" || first.value == "VALUES" {
        return true;
    }

    first.value == "WITH"
        && words
            .iter()
            .skip(1)
            .any(|word| word.value == "SELECT" || word.value == "VALUES")
}

fn is_pageable_result_query(sql: &str) -> bool {
    let words = top_level_sql_words(sql);
    let Some(first) = words.first() else {
        return false;
    };

    if words
        .iter()
        .any(|word| matches!(word.value.as_str(), "FOR" | "INTO"))
    {
        return false;
    }

    let is_select_query = first.value == "SELECT"
        || (first.value == "WITH" && words.iter().any(|word| word.value == "SELECT"));

    is_select_query && !is_single_row_result_query(&words)
}

fn is_single_row_result_query(words: &[SqlWord]) -> bool {
    let Some(select_index) = words.iter().position(|word| word.value == "SELECT") else {
        return false;
    };

    if words.iter().any(|word| {
        matches!(
            word.value.as_str(),
            "GROUP" | "HAVING" | "UNION" | "INTERSECT" | "EXCEPT"
        )
    }) {
        return false;
    }

    let from_index = words
        .iter()
        .enumerate()
        .skip(select_index + 1)
        .find_map(|(index, word)| (word.value == "FROM").then_some(index));

    let projection = match from_index {
        Some(from_index) => &words[select_index + 1..from_index],
        None => return true,
    };

    projection
        .iter()
        .any(|word| matches!(word.value.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX"))
        && !projection.iter().any(|word| word.value == "OVER")
}

fn top_level_sql_words(sql: &str) -> Vec<SqlWord> {
    let mut words = Vec::new();
    let mut chars = sql.char_indices().peekable();
    let mut depth = 0usize;

    while let Some((index, ch)) = chars.next() {
        let next = chars.peek().map(|(_, ch)| *ch);

        match (ch, next) {
            ('-', Some('-')) => {
                chars.next();
                consume_until_line_end(sql, &mut chars);
            }
            ('/', Some('*')) => {
                chars.next();
                consume_block_comment(sql, &mut chars);
            }
            ('\'', _) | ('"', _) | ('`', _) => {
                consume_quoted(sql, &mut chars, ch);
            }
            ('[', _) => {
                consume_bracket_identifier(sql, &mut chars);
            }
            ('$', _) if dollar_quote_delimiter(sql, index).is_some() => {
                let Some(delimiter) = dollar_quote_delimiter(sql, index) else {
                    continue;
                };
                consume_dollar_quoted_range(sql, &mut chars, &delimiter);
            }
            ('(', _) => {
                depth = depth.saturating_add(1);
            }
            (')', _) => {
                depth = depth.saturating_sub(1);
            }
            (ch, _) if depth == 0 && is_identifier_start(ch) => {
                let end = consume_identifier(sql, &mut chars);
                words.push(SqlWord {
                    value: sql[index..end].to_ascii_uppercase(),
                });
            }
            _ => {}
        }
    }

    words
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
    fn adds_preview_limit_to_unlimited_select() {
        assert_eq!(
            limit_result_preview_statement("SELECT * FROM books;", 500).as_deref(),
            Some("SELECT * FROM books\nLIMIT 500")
        );
    }

    #[test]
    fn adds_preview_limit_after_trailing_line_comment() {
        assert_eq!(
            limit_result_preview_statement("SELECT * FROM books -- sample", 500).as_deref(),
            Some("SELECT * FROM books -- sample\nLIMIT 500")
        );
    }

    #[test]
    fn adds_preview_limit_to_cte_select() {
        assert_eq!(
            limit_result_preview_statement(
                "WITH recent AS (SELECT * FROM books LIMIT 10) SELECT * FROM recent",
                500
            )
            .as_deref(),
            Some("WITH recent AS (SELECT * FROM books LIMIT 10) SELECT * FROM recent\nLIMIT 500")
        );
    }

    #[test]
    fn skips_preview_limit_for_explicit_top_level_limits() {
        assert_eq!(
            limit_result_preview_statement("SELECT * FROM books LIMIT 25", 500),
            None
        );
        assert_eq!(
            limit_result_preview_statement("SELECT * FROM books OFFSET 25", 500),
            None
        );
        assert_eq!(
            limit_result_preview_statement("SELECT * FROM books FETCH FIRST 25 ROWS ONLY", 500),
            None
        );
    }

    #[test]
    fn skips_preview_limit_for_non_select_cte_and_locking_select() {
        assert_eq!(
            limit_result_preview_statement(
                "WITH stale AS (SELECT id FROM books) DELETE FROM books WHERE id IN (SELECT id FROM stale)",
                500
            ),
            None
        );
        assert_eq!(
            limit_result_preview_statement("SELECT * FROM books FOR UPDATE", 500),
            None
        );
    }

    #[test]
    fn builds_page_and_count_wrappers_for_selects() {
        assert_eq!(
            page_result_statement("SELECT * FROM books;", 500, 1_000).as_deref(),
            Some("SELECT * FROM (SELECT * FROM books) AS __query_page LIMIT 500 OFFSET 1000")
        );
        assert_eq!(
            count_result_page_statement("SELECT * FROM books;").as_deref(),
            Some("SELECT COUNT(*) AS total_rows FROM (SELECT * FROM books) AS __query_count")
        );
    }

    #[test]
    fn skips_page_wrappers_for_explicit_top_level_limits() {
        let sql = "SELECT * FROM `base`.`country` ORDER BY `zh_name` LIMIT 10";
        assert_eq!(page_result_statement(sql, 100, 0), None);
        assert_eq!(count_result_page_statement(sql), None);
    }

    #[test]
    fn skips_page_wrappers_for_unsafe_select_forms() {
        assert_eq!(
            page_result_statement("SELECT * INTO new_books FROM books", 500, 0),
            None
        );
        assert_eq!(
            count_result_page_statement("SELECT * FROM books FOR UPDATE"),
            None
        );
    }

    #[test]
    fn skips_page_wrappers_for_single_row_aggregate_queries() {
        assert_eq!(
            page_result_statement("SELECT COUNT(*) FROM books", 500, 0),
            None
        );
        assert_eq!(
            count_result_page_statement("SELECT COUNT(*) FROM books"),
            None
        );
        assert_eq!(page_result_statement("SELECT 1", 500, 0), None);
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
    fn keeps_trigger_body_semicolons_together() {
        assert_eq!(
            split_statements("CREATE TRIGGER t AFTER INSERT ON tab BEGIN SELECT 1; SELECT 2; END; SELECT 3"),
            vec![
                "CREATE TRIGGER t AFTER INSERT ON tab BEGIN SELECT 1; SELECT 2; END",
                "SELECT 3"
            ]
        );
    }

    #[test]
    fn keeps_mysql_trigger_begin_end_together() {
        assert_eq!(
            split_statements(
                "CREATE TRIGGER t BEFORE INSERT ON tab FOR EACH ROW BEGIN SET NEW.x = 1; SET NEW.y = 2; END; SELECT 1"
            ),
            vec![
                "CREATE TRIGGER t BEFORE INSERT ON tab FOR EACH ROW BEGIN SET NEW.x = 1; SET NEW.y = 2; END",
                "SELECT 1"
            ]
        );
    }

    #[test]
    fn keeps_end_if_inside_trigger_body() {
        assert_eq!(
            split_statements(
                "CREATE TRIGGER t BEFORE INSERT ON tab FOR EACH ROW BEGIN IF NEW.x IS NULL THEN SET NEW.x = 0; END IF; END"
            ),
            vec![
                "CREATE TRIGGER t BEFORE INSERT ON tab FOR EACH ROW BEGIN IF NEW.x IS NULL THEN SET NEW.x = 0; END IF; END"
            ]
        );
    }

    #[test]
    fn splits_mysql_set_form_trigger() {
        assert_eq!(
            split_statements(
                "CREATE TRIGGER t BEFORE INSERT ON tab FOR EACH ROW SET NEW.x = NEW.x; SELECT 1"
            ),
            vec![
                "CREATE TRIGGER t BEFORE INSERT ON tab FOR EACH ROW SET NEW.x = NEW.x",
                "SELECT 1"
            ]
        );
    }

    #[test]
    fn strips_mysql_client_delimiter_around_trigger() {
        let sql = "DELIMITER //
CREATE TRIGGER trg_articles_update_title BEFORE UPDATE ON `articles`
FOR EACH ROW
BEGIN
    IF NEW.content IS NOT NULL AND NEW.content <> '' THEN
        SET NEW.title = LEFT(NEW.content, 10);
    ELSE
        SET NEW.title = '';
    END IF;
END //
DELIMITER ;";
        let statements = split_statements(sql);
        assert_eq!(statements.len(), 1);
        assert!(
            statements[0].starts_with("CREATE TRIGGER trg_articles_update_title"),
            "{}",
            statements[0]
        );
        assert!(statements[0].contains("SET NEW.title = LEFT(NEW.content, 10);"));
        assert!(statements[0].trim_end().ends_with("END"));
        assert!(!statements[0].to_ascii_uppercase().contains("DELIMITER"));
        assert!(!statements[0].contains("//"));
    }

    #[test]
    fn delimiter_script_keeps_leading_drop_separate() {
        let sql = "DROP TRIGGER IF EXISTS `app`.`trg_articles_update_title`;
DELIMITER //
CREATE TRIGGER trg_articles_update_title BEFORE UPDATE ON `articles`
FOR EACH ROW
BEGIN
    SET NEW.title = LEFT(NEW.content, 10);
END //
DELIMITER ;";
        let statements = split_statements(sql);
        assert_eq!(statements.len(), 2);
        assert_eq!(
            statements[0],
            "DROP TRIGGER IF EXISTS `app`.`trg_articles_update_title`"
        );
        assert!(statements[1].starts_with("CREATE TRIGGER trg_articles_update_title"));
        assert!(statements[1].contains("SET NEW.title = LEFT(NEW.content, 10);"));
        assert!(statements[1].trim_end().ends_with("END"));
        assert!(!statements[1].contains("DELIMITER"));
        assert!(!statements[1].contains("//"));
    }

    #[test]
    fn finds_statement_at_offset() {
        let sql = "select 1;\nselect 2;";
        assert_eq!(statement_at_offset(sql, 0).as_deref(), Some("select 1"));
        assert_eq!(statement_at_offset(sql, 12).as_deref(), Some("select 2"));
    }

    #[test]
    fn finds_statement_at_offset_after_trailing_semicolon() {
        let sql = "SELECT * FROM books;";
        assert_eq!(
            statement_at_offset(sql, sql.len()).as_deref(),
            Some("SELECT * FROM books")
        );

        let sql = "SELECT * FROM books;\n\n";
        assert_eq!(
            statement_at_offset(sql, sql.len()).as_deref(),
            Some("SELECT * FROM books")
        );
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
    fn splits_empty_statements() {
        assert_eq!(split_statements(""), Vec::<String>::new());
        assert_eq!(split_statements("   "), Vec::<String>::new());
        assert_eq!(split_statements(";;"), Vec::<String>::new());
        assert_eq!(split_statements("; select 1 ; ;"), vec!["select 1"]);
    }

    #[test]
    fn splits_single_statement() {
        assert_eq!(split_statements("select 1"), vec!["select 1"]);
        assert_eq!(split_statements("select 1;"), vec!["select 1"]);
    }

    #[test]
    fn splits_multiple_statements() {
        assert_eq!(
            split_statements("select 1; select 2"),
            vec!["select 1", "select 2"]
        );
        assert_eq!(
            split_statements("select 1;\nselect 2;\nselect 3;"),
            vec!["select 1", "select 2", "select 3"]
        );
    }

    #[test]
    fn handles_single_line_comments() {
        assert_eq!(
            split_statements("-- this is a comment\nselect 1"),
            vec!["-- this is a comment\nselect 1"]
        );
        assert_eq!(
            split_statements("select 1 -- comment;\nselect 2"),
            vec!["select 1 -- comment;\nselect 2"]
        );
        assert_eq!(
            split_statements("-- comment with ; semicolon\nselect 1;"),
            vec!["-- comment with ; semicolon\nselect 1"]
        );
    }

    #[test]
    fn handles_multi_line_comments() {
        assert_eq!(
            split_statements("/* comment */ select 1"),
            vec!["/* comment */ select 1"]
        );
        assert_eq!(
            split_statements("select /* comment */ 1; select 2"),
            vec!["select /* comment */ 1", "select 2"]
        );
        assert_eq!(
            split_statements("select /* comment ; with ; semicolons */ 1"),
            vec!["select /* comment ; with ; semicolons */ 1"]
        );
    }

    #[test]
    fn handles_single_quoted_strings() {
        assert_eq!(split_statements("select 'a';"), vec!["select 'a'"]);
        assert_eq!(split_statements("select 'a;b';"), vec!["select 'a;b'"]);
        assert_eq!(split_statements("select 'a''b';"), vec!["select 'a''b'"]);
        assert_eq!(split_statements("select ';';"), vec!["select ';'"]);
    }

    #[test]
    fn handles_double_quoted_strings() {
        assert_eq!(split_statements(r#"select "a";"#), vec![r#"select "a""#]);
        assert_eq!(
            split_statements(r#"select "a;b";"#),
            vec![r#"select "a;b""#]
        );
        assert_eq!(
            split_statements(r#"select "a""b";"#),
            vec![r#"select "a""b""#]
        );
        assert_eq!(split_statements(r#"select ";";"#), vec![r#"select ";""#]);
    }

    #[test]
    fn handles_mixed_quotes_and_comments() {
        assert_eq!(
            split_statements("select 'a'; /* comment */ select \"b\"; -- comment\nselect 3"),
            vec![
                "select 'a'",
                "/* comment */ select \"b\"",
                "-- comment\nselect 3"
            ]
        );
    }

    #[test]
    fn handles_statement_at_offset_with_comments() {
        let sql = "-- comment\nselect 1;\n/* comment */\nselect 2;";
        assert_eq!(
            statement_at_offset(sql, 20).as_deref(),
            Some("/* comment */\nselect 2")
        );
        assert_eq!(
            statement_at_offset(sql, 40).as_deref(),
            Some("/* comment */\nselect 2")
        );
    }

    #[test]
    fn handles_statement_at_offset_with_strings() {
        let sql = "select 'a;b'; select 2;";
        assert_eq!(statement_at_offset(sql, 5).as_deref(), Some("select 'a;b'"));
        assert_eq!(statement_at_offset(sql, 15).as_deref(), Some("select 2"));
    }

    #[test]
    fn handles_trailing_semicolon() {
        assert_eq!(split_statements("select 1;;"), vec!["select 1"]);
        assert_eq!(split_statements("select 1;\n;"), vec!["select 1"]);
    }

    #[test]
    fn handles_whitespace_around_semicolons() {
        assert_eq!(
            split_statements("select 1 ; select 2"),
            vec!["select 1", "select 2"]
        );
        assert_eq!(
            split_statements("select 1 \n ; \n select 2"),
            vec!["select 1", "select 2"]
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
