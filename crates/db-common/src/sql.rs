pub fn trim_sql(sql: &str) -> &str {
    sql.trim_matches(|ch: char| ch.is_whitespace() || ch == ';')
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
}
