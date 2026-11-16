//! SQL completion context, metadata selection and popup interaction.
use super::*;

fn keyword_suggestions(kind: DataSourceKind) -> impl Iterator<Item = &'static str> {
    let dialect: &[&str] = match kind.protocol_family() {
        db_common::ProtocolFamily::PostgreSql => &[
            "RETURNING",
            "ILIKE",
            "DISTINCT ON",
            "ON CONFLICT",
            "JSONB_BUILD_OBJECT",
        ],
        db_common::ProtocolFamily::MySql => &[
            "SHOW TABLES",
            "SHOW CREATE TABLE",
            "ON DUPLICATE KEY UPDATE",
            "IFNULL",
            "GROUP_CONCAT",
        ],
        db_common::ProtocolFamily::SqlServer => &[
            "TOP",
            "OUTPUT",
            "CROSS APPLY",
            "ISNULL",
            "OFFSET",
            "FETCH NEXT",
        ],
        db_common::ProtocolFamily::Sqlite => &["PRAGMA", "ON CONFLICT", "RETURNING", "IFNULL"],
        db_common::ProtocolFamily::DuckDb => &["QUALIFY", "EXCLUDE", "UNNEST", "PIVOT", "READ_CSV"],
        db_common::ProtocolFamily::ClickHouse => {
            &["PREWHERE", "ARRAY JOIN", "FINAL", "FORMAT", "LIMIT BY"]
        }
        _ => &[],
    };
    SQL_KEYWORD_SUGGESTIONS
        .iter()
        .copied()
        .chain(dialect.iter().copied())
}

fn completion_statement(tab: &QueryEditorTab) -> &str {
    let cursor = tab.cursor_offset.min(tab.sql.len());
    let mut start = 0;
    let mut end = tab.sql.len();
    for token in db_common::highlight_sql(&tab.sql) {
        if &tab.sql[token.range.start..token.range.end] != ";" {
            continue;
        }
        if token.range.end <= cursor {
            start = token.range.end;
        } else {
            end = token.range.start;
            break;
        }
    }
    &tab.sql[start..end]
}

pub(super) fn update_completion_for_tab(tab: &mut QueryEditorTab) {
    if tab.sql.is_empty() {
        tab.completion = CompletionState::default();
        return;
    }

    let cursor = tab.cursor_offset.min(tab.sql.len());
    let before = &tab.sql[..cursor];
    let word_start = before
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_completion_char(*ch))
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(cursor);

    let prefix = &before[word_start..];
    let context = completion_context(before, word_start, prefix);
    if context == CompletionContext::None {
        tab.completion = CompletionState::default();
        return;
    }

    let mut items = Vec::new();
    let replacement = TextRange::new(word_start, cursor);

    match &context {
        CompletionContext::None => {}
        CompletionContext::Keyword => {
            for keyword in keyword_suggestions(tab.kind) {
                if matches_prefix(keyword, prefix) {
                    items.push(CompletionItem {
                        label: keyword.to_string(),
                        insert_text: keyword.to_string(),
                        kind: CompletionKind::Keyword,
                        detail: String::new(),
                    });
                }
            }
        }
        CompletionContext::Table => {
            for table in &tab.available_tables {
                if matches_prefix(table, prefix) {
                    items.push(CompletionItem {
                        label: table.clone(),
                        insert_text: table.clone(),
                        kind: CompletionKind::Table,
                        detail: String::new(),
                    });
                }
            }
        }
        CompletionContext::Column { qualifier } => {
            let references = referenced_tables(completion_statement(tab));
            let table_names = completion_column_tables(&references, qualifier.as_deref());
            let mut seen = BTreeSet::new();
            for table_name in table_names {
                let key = normalize_identifier(&table_name);
                let Some(columns) = tab.columns_by_table.get(&key) else {
                    continue;
                };
                for column in columns {
                    let normalized = normalize_identifier(&column.name);
                    if matches_prefix(&column.name, prefix) && seen.insert(normalized) {
                        items.push(CompletionItem {
                            label: column.name.clone(),
                            insert_text: column.name.clone(),
                            kind: CompletionKind::Column,
                            detail: format!("{} - {}", table_name, column.database_type),
                        });
                    }
                }
            }
        }
    }

    if !prefix.is_empty()
        && !matches!(
            context,
            CompletionContext::Keyword | CompletionContext::Table
        )
    {
        for keyword in keyword_suggestions(tab.kind) {
            if matches_prefix(keyword, prefix)
                && !items
                    .iter()
                    .any(|item| item.label.eq_ignore_ascii_case(keyword))
            {
                items.push(CompletionItem {
                    label: keyword.to_string(),
                    insert_text: keyword.to_string(),
                    kind: CompletionKind::Keyword,
                    detail: String::new(),
                });
            }
        }
    }
    let waiting_for_metadata = matches!(context, CompletionContext::Column { .. })
        && !missing_completion_columns_for_context(tab, before, &context).is_empty();
    if items.is_empty() && !waiting_for_metadata {
        tab.completion.visible = false;
        tab.completion.items.clear();
        return;
    }

    items.truncate(MAX_COMPLETION_ITEMS);

    // Keep the current selection if still valid, otherwise reset.
    if tab.completion.selected >= items.len() {
        tab.completion.selected = 0;
    }
    tab.completion.items = items;
    tab.completion.replacement = Some(replacement);
    tab.completion.visible = true;
    tab.completion.waiting_for_metadata = waiting_for_metadata;
}

/// Metadata can arrive after the user has clicked outside the SQL editor.
/// Only an existing completion request is allowed to consume it.
pub(super) fn refresh_active_completion(tab: &mut QueryEditorTab) {
    if tab.completion.visible {
        update_completion_for_tab(tab);
    }
}

/// Return tables referenced by the current completion that have no loaded
/// columns yet. The caller spawns metadata loads for each.
pub(super) fn missing_completion_columns(tab: &QueryEditorTab) -> Vec<String> {
    if !tab.completion.visible {
        return Vec::new();
    }

    let cursor = tab.cursor_offset.min(tab.sql.len());
    let before = &tab.sql[..cursor];
    let word_start = before
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_completion_char(*ch))
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(cursor);
    let prefix = &before[word_start..];
    let context = completion_context(before, word_start, prefix);
    missing_completion_columns_for_context(tab, before, &context)
}

pub(super) fn completion_context(
    before: &str,
    word_start: usize,
    prefix: &str,
) -> CompletionContext {
    if db_common::highlight_sql(before)
        .last()
        .is_some_and(|token| {
            token.range.end == before.len()
                && matches!(
                    token.kind,
                    SqlTokenKind::StringLiteral | SqlTokenKind::Comment
                )
        })
    {
        return CompletionContext::None;
    }
    let clause = before[..word_start].trim_end();
    if let Some(stripped) = clause.strip_suffix('.') {
        let qualifier = last_sql_identifier(stripped).map(ToOwned::to_owned);
        return CompletionContext::Column { qualifier };
    }

    let previous = last_sql_identifier(clause)
        .map(str::to_ascii_uppercase)
        .unwrap_or_default();
    if matches!(previous.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
        CompletionContext::Table
    } else if matches!(
        previous.as_str(),
        "SELECT" | "WHERE" | "ON" | "HAVING" | "SET" | "AND" | "OR" | "BY"
    ) {
        CompletionContext::Column { qualifier: None }
    } else if !prefix.is_empty() && prefix.chars().next().is_some_and(|ch| ch.is_alphabetic()) {
        CompletionContext::Keyword
    } else {
        CompletionContext::None
    }
}

pub(super) fn last_sql_identifier(sql: &str) -> Option<&str> {
    let end = sql
        .char_indices()
        .rev()
        .find(|(_, ch)| is_completion_char(*ch))
        .map(|(index, ch)| index + ch.len_utf8())?;
    let start = sql[..end]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_completion_char(*ch))
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0);
    Some(&sql[start..end])
}

pub(super) fn referenced_tables(sql: &str) -> Vec<CompletionTableRef> {
    let tokens = sql_completion_tokens(sql);
    let mut references = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let keyword = tokens[index].to_ascii_uppercase();
        if !matches!(keyword.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
            index += 1;
            continue;
        }

        let mut object_index = index + 1;
        if object_index >= tokens.len() || tokens[object_index] == "(" {
            index += 1;
            continue;
        }
        let mut table = tokens[object_index].clone();
        while object_index + 2 < tokens.len()
            && tokens[object_index + 1] == "."
            && is_sql_identifier_token(&tokens[object_index + 2])
        {
            object_index += 2;
            table = tokens[object_index].clone();
        }

        if !is_sql_identifier_token(&table) {
            index += 1;
            continue;
        }

        let mut alias = None;
        let mut alias_index = object_index + 1;
        if tokens
            .get(alias_index)
            .is_some_and(|token| token.eq_ignore_ascii_case("AS"))
        {
            alias_index += 1;
        }
        if let Some(candidate) = tokens.get(alias_index) {
            if is_sql_identifier_token(candidate) && !is_sql_clause_keyword(candidate) {
                alias = Some(candidate.clone());
            }
        }

        if !references.iter().any(|reference: &CompletionTableRef| {
            normalize_identifier(&reference.table) == normalize_identifier(&table)
                && reference.alias == alias
        }) {
            references.push(CompletionTableRef { table, alias });
        }
        index = object_index + 1;
    }
    references
}

pub(super) fn completion_column_tables(
    references: &[CompletionTableRef],
    qualifier: Option<&str>,
) -> Vec<String> {
    let qualifier = qualifier.map(normalize_identifier);
    references
        .iter()
        .filter(|reference| match qualifier.as_ref() {
            None => true,
            Some(qualifier) => {
                normalize_identifier(&reference.table) == *qualifier
                    || reference
                        .alias
                        .as_deref()
                        .is_some_and(|alias| normalize_identifier(alias) == *qualifier)
            }
        })
        .map(|reference| reference.table.clone())
        .collect()
}

pub(super) fn missing_completion_columns_for_context(
    tab: &QueryEditorTab,
    _before: &str,
    context: &CompletionContext,
) -> Vec<String> {
    let CompletionContext::Column { qualifier } = context else {
        return Vec::new();
    };
    let references = referenced_tables(completion_statement(tab));
    completion_column_tables(&references, qualifier.as_deref())
        .into_iter()
        .filter(|table| {
            let key = normalize_identifier(table);
            !tab.columns_by_table.contains_key(&key)
                && !tab.pending_column_loads.contains(&key)
                && tab
                    .available_tables
                    .iter()
                    .any(|name| normalize_identifier(name) == key)
        })
        .collect()
}

pub(super) fn sql_completion_tokens(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if is_completion_char(ch) {
            current.push(ch);
            while chars.peek().is_some_and(|next| is_completion_char(*next)) {
                current.push(chars.next().unwrap_or_default());
            }
            tokens.push(std::mem::take(&mut current));
        } else if matches!(ch, '`' | '"' | '[') {
            let closing = if ch == '[' { ']' } else { ch };
            for next in chars.by_ref() {
                if next == closing {
                    break;
                }
                current.push(next);
            }
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else if matches!(ch, '.' | ',' | ';' | '(' | ')') {
            tokens.push(ch.to_string());
        } else if ch == '\'' {
            // Single-quoted string with `''` escape: skip the closing quote,
            // consume the doubled quote, continue scanning. `for` doesn't work
            // here because we need `peek()` + `next()` for the escape.
            #[allow(clippy::while_let_on_iterator)]
            while let Some(next) = chars.next() {
                if next == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
        }
    }
    tokens
}

pub(super) fn is_sql_identifier_token(token: &str) -> bool {
    !token.is_empty() && token.chars().all(is_completion_char)
}

pub(super) fn is_sql_clause_keyword(token: &str) -> bool {
    matches!(
        token.to_ascii_uppercase().as_str(),
        "WHERE"
            | "JOIN"
            | "ON"
            | "GROUP"
            | "ORDER"
            | "HAVING"
            | "LIMIT"
            | "OFFSET"
            | "UNION"
            | "LEFT"
            | "RIGHT"
            | "INNER"
            | "OUTER"
            | "CROSS"
            | "SET"
            | "VALUES"
            | "RETURNING"
    )
}

pub(super) fn is_completion_char(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_alphanumeric()
}

pub(super) fn matches_prefix(candidate: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    candidate
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
}

pub(super) fn take_completion_command(
    ui: &mut Ui,
    completion: &CompletionState,
) -> CompletionCommand {
    if !completion.visible || completion.items.is_empty() {
        return CompletionCommand::None;
    }

    ui.input_mut(|input| {
        let modifiers = eframe::egui::Modifiers::NONE;
        if input.consume_key(modifiers, Key::Escape) {
            CompletionCommand::Dismiss
        } else if input.consume_key(modifiers, Key::ArrowDown) {
            CompletionCommand::Next
        } else if input.consume_key(modifiers, Key::ArrowUp) {
            CompletionCommand::Previous
        } else if input.consume_key(modifiers, Key::Enter) || input.consume_key(modifiers, Key::Tab)
        {
            CompletionCommand::Accept
        } else {
            CompletionCommand::None
        }
    })
}

/// Render a foreground completion popup anchored to the caret and return the
/// SQL with the selected item inserted when the user accepts a suggestion.
pub(super) fn completion_panel(
    ui: &mut Ui,
    popup_pos: Pos2,
    tab: &mut QueryEditorTab,
    command: CompletionCommand,
) -> Option<String> {
    if !tab.completion.visible {
        return None;
    }

    if command == CompletionCommand::Dismiss {
        tab.completion.visible = false;
        return None;
    }

    let count = tab.completion.items.len();
    if count > 0 {
        match command {
            CompletionCommand::Next => {
                tab.completion.selected = (tab.completion.selected + 1) % count;
            }
            CompletionCommand::Previous => {
                tab.completion.selected = (tab.completion.selected + count - 1) % count;
            }
            _ => {}
        }
    }

    let mut clicked = None;
    Area::new(ui.make_persistent_id(("sql_completion", tab.id)))
        .order(Order::Foreground)
        .fixed_pos(popup_pos)
        .constrain(true)
        .show(ui.ctx(), |ui| {
            Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(260.0);
                if tab.completion.waiting_for_metadata {
                    ui.label(tr("Loading column suggestions..."));
                }
                ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (index, item) in tab.completion.items.iter().enumerate() {
                            let selected = index == tab.completion.selected;
                            let kind = match item.kind {
                                CompletionKind::Keyword => "keyword",
                                CompletionKind::Table => "table",
                                CompletionKind::Column => "column",
                            };
                            let detail = if item.detail.is_empty() {
                                String::new()
                            } else {
                                format!(" - {}", item.detail)
                            };
                            let response = ui.selectable_label(
                                selected,
                                RichText::new(format!("{}  [{}]{}", item.label, kind, detail))
                                    .color(if selected {
                                        ui.visuals().selection.stroke.color
                                    } else {
                                        ui.visuals().text_color()
                                    }),
                            );
                            if selected {
                                response.scroll_to_me(Some(Align::Center));
                            }
                            if response.clicked() {
                                clicked = Some(index);
                            }
                        }
                    });
            });
        });

    if count == 0 {
        return None;
    }

    if let Some(index) = clicked {
        tab.completion.selected = index;
    }
    let accept = command == CompletionCommand::Accept || clicked.is_some();
    if !accept {
        return None;
    }

    let replacement = tab.completion.replacement?;
    let selected = tab.completion.selected.min(tab.completion.items.len() - 1);
    let item = tab.completion.items.get(selected)?.clone();
    let mut sql = tab.sql.clone();
    let start = replacement.start.min(sql.len());
    let end = replacement.end.min(sql.len());
    sql.replace_range(start..end, &item.insert_text);
    tab.cursor_offset = start + item.insert_text.len();
    tab.selection = None;
    tab.completion.visible = false;
    Some(sql)
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use eframe::egui::{CentralPanel, Context, Event, Modifiers, RawInput};

    fn completion_state() -> CompletionState {
        CompletionState {
            visible: true,
            replacement: Some(TextRange::new(0, 1)),
            items: vec![CompletionItem {
                label: "SELECT".to_string(),
                insert_text: "SELECT".to_string(),
                kind: CompletionKind::Keyword,
                detail: String::new(),
            }],
            selected: 0,
            waiting_for_metadata: false,
        }
    }

    fn command_for(key: Key) -> CompletionCommand {
        let ctx = Context::default();
        let mut input = RawInput::default();
        input.events.push(Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        ctx.begin_frame(input);
        let mut command = CompletionCommand::None;
        CentralPanel::default().show(&ctx, |ui| {
            command = take_completion_command(ui, &completion_state());
        });
        let _ = ctx.end_frame();
        command
    }

    fn context_for(sql: &str) -> CompletionContext {
        let word_start = sql
            .char_indices()
            .rev()
            .take_while(|(_, ch)| is_completion_char(*ch))
            .last()
            .map(|(index, _)| index)
            .unwrap_or(sql.len());
        completion_context(sql, word_start, &sql[word_start..])
    }

    #[test]
    fn select_list_uses_following_from_and_excludes_other_statements() {
        let profile = ConnectionProfile::builder(DataSourceKind::Sqlite, "completion")
            .file_path(":memory:")
            .build()
            .unwrap();
        let mut tab = QueryEditorTab::new(&profile);
        tab.sql = "SELECT * FROM unrelated; SELECT b.na FROM books b".into();
        tab.cursor_offset = tab.sql.find("b.na").unwrap() + 4;
        tab.columns_by_table.insert(
            "books".into(),
            vec![TableColumn {
                name: "name".into(),
                database_type: "TEXT".into(),
                nullable: true,
                default_value: None,
                primary_key: false,
                comment: None,
            }],
        );
        update_completion_for_tab(&mut tab);
        assert!(tab.completion.items.iter().any(|item| item.label == "name"));
        assert_eq!(
            referenced_tables(completion_statement(&tab))[0].table,
            "books"
        );
        assert!(keyword_suggestions(DataSourceKind::PostgreSql).any(|word| word == "ILIKE"));
        assert!(!keyword_suggestions(DataSourceKind::MySql).any(|word| word == "ILIKE"));
    }

    #[test]
    fn tab_and_enter_accept_visible_completion() {
        assert_eq!(command_for(Key::Tab), CompletionCommand::Accept);
        assert_eq!(command_for(Key::Enter), CompletionCommand::Accept);
    }

    #[test]
    fn arrows_and_escape_control_visible_completion() {
        assert_eq!(command_for(Key::ArrowDown), CompletionCommand::Next);
        assert_eq!(command_for(Key::ArrowUp), CompletionCommand::Previous);
        assert_eq!(command_for(Key::Escape), CompletionCommand::Dismiss);
    }

    #[test]
    fn completion_is_scoped_by_sql_clause() {
        assert_eq!(context_for(""), CompletionContext::None);
        assert_eq!(context_for("SELECT * FROM bo"), CompletionContext::Table);
        assert_eq!(
            context_for("SELECT * FROM books WHERE au"),
            CompletionContext::Column { qualifier: None }
        );
        assert_eq!(
            context_for("SELECT * FROM books b WHERE b."),
            CompletionContext::Column {
                qualifier: Some("b".to_string())
            }
        );
        assert_eq!(context_for("SELECT * FROM books;"), CompletionContext::None);
        assert_eq!(
            context_for("SELECT * FROM books LIMIT 10"),
            CompletionContext::None
        );
    }

    #[test]
    fn suggests_keywords_after_expressions_but_not_inside_literals() {
        assert_eq!(
            context_for("SELECT * FROM books WH"),
            CompletionContext::Keyword
        );
        assert_eq!(context_for("SELECT 'WH"), CompletionContext::None);
        assert_eq!(context_for("SELECT * -- WH"), CompletionContext::None);
        let profile = ConnectionProfile::builder(DataSourceKind::Sqlite, "completion")
            .file_path(":memory:")
            .build()
            .unwrap();
        let mut tab = QueryEditorTab::new(&profile);
        tab.sql = "SELECT COU".to_owned();
        tab.cursor_offset = tab.sql.len();
        update_completion_for_tab(&mut tab);
        assert!(tab
            .completion
            .items
            .iter()
            .any(|item| item.label == "COUNT"));
    }

    #[test]
    fn metadata_only_refreshes_an_active_completion() {
        let profile = ConnectionProfile::builder(DataSourceKind::PostgreSql, "completion test")
            .host("localhost")
            .username("postgres")
            .build()
            .expect("valid profile");
        let mut tab = QueryEditorTab::new(&profile);
        tab.sql = "SELECT * FROM bo".to_string();
        tab.cursor_offset = tab.sql.len();
        tab.available_tables = vec!["books".to_string()];

        refresh_active_completion(&mut tab);
        assert!(!tab.completion.visible);

        tab.completion.visible = true;
        refresh_active_completion(&mut tab);
        assert!(tab.completion.visible);
        assert_eq!(tab.completion.items.len(), 1);
        assert_eq!(tab.completion.items[0].label, "books");
    }

    #[test]
    fn table_references_support_qualified_names_and_aliases() {
        assert_eq!(
            referenced_tables("SELECT * FROM `askme` . `books` b JOIN authors AS a ON a.id=b.id"),
            vec![
                CompletionTableRef {
                    table: "books".to_string(),
                    alias: Some("b".to_string()),
                },
                CompletionTableRef {
                    table: "authors".to_string(),
                    alias: Some("a".to_string()),
                },
            ]
        );
    }

    #[test]
    fn identifies_single_table_results_as_editable_candidates() {
        assert_eq!(
            editable_result_source(
                "SELECT id, name FROM \"base\".\"country\" WHERE id > 1 ORDER BY name LIMIT 10"
            ),
            Some(("country".to_string(), Some("base".to_string())))
        );
    }

    #[test]
    fn keeps_joined_and_aggregate_results_read_only() {
        assert_eq!(
            editable_result_source(
                "SELECT * FROM books JOIN authors ON authors.id = books.author_id"
            ),
            None
        );
        assert_eq!(
            editable_result_source("SELECT category, COUNT(*) FROM books GROUP BY category"),
            None
        );
    }
}
