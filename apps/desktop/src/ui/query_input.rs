//! Persistent completion for the JSON/HTTP-SQL and Redis query editors.
use db_common::DataSourceKind;
use eframe::egui::{self, Area, Frame, Id, Key, Modifiers, Order, ScrollArea, TextEdit, Ui};

#[derive(Clone, Default)]
struct Completion {
    visible: bool,
    selected: usize,
    start: usize,
    end: usize,
    cursor: usize,
    source: String,
    items: Vec<(String, String)>,
}

pub fn editor(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    kind: DataSourceKind,
    object: &str,
    rows: usize,
) {
    editor_with_fields(ui, id, text, kind, object, rows, &[]);
}

pub fn editor_with_fields(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    kind: DataSourceKind,
    object: &str,
    rows: usize,
    fields: &[(String, String)],
) {
    let state_id = id.with("completion");
    let mut state = ui
        .ctx()
        .data_mut(|data| data.get_temp::<Completion>(state_id))
        .unwrap_or_default();
    let focused = ui.memory(|memory| memory.has_focus(id));
    let mut accept = false;
    let mut dismissed = false;
    let mut requested = false;
    if focused {
        ui.input_mut(|input| {
            requested = input.consume_key(Modifiers::CTRL, Key::Space);
            if state.visible && !state.items.is_empty() {
                accept = input.consume_key(Modifiers::NONE, Key::Tab)
                    || input.consume_key(Modifiers::NONE, Key::Enter);
                dismissed = input.consume_key(Modifiers::NONE, Key::Escape);
                if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    state.selected = (state.selected + 1) % state.items.len();
                }
                if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    state.selected = (state.selected + state.items.len() - 1) % state.items.len();
                }
            }
        });
    }
    let mut output = TextEdit::multiline(text)
        .id(id)
        .code_editor()
        .desired_rows(rows)
        .desired_width(f32::INFINITY)
        .show(ui);
    if let Some(range) = output.cursor_range {
        let cursor = byte_offset(text, range.primary.ccursor.index);
        if output.response.has_focus() && (output.response.changed() || requested) {
            state = candidates(text, cursor, kind, object);
            add_field_candidates(text, cursor, &mut state, fields);
        } else if (cursor != state.cursor || *text != state.source) && output.response.has_focus() {
            state.visible = false;
        }
    }
    if dismissed {
        state.visible = false;
    }
    // Keep the popup alive across idle frames and pointer-down on a candidate.
    let popup_id = id.with("popup");
    let popup_pressed = ui
        .ctx()
        .layer_id_at(ui.input(|i| i.pointer.interact_pos()).unwrap_or_default())
        == Some(egui::LayerId::new(Order::Foreground, popup_id));
    if !output.response.has_focus() && !popup_pressed {
        state.visible = false;
    }
    if state.visible && !state.items.is_empty() {
        let pos = output
            .cursor_range
            .map(|range| {
                let rect = output.galley.pos_from_cursor(&range.primary);
                output.galley_pos + rect.left_bottom().to_vec2() + egui::vec2(0.0, 3.0)
            })
            .unwrap_or(output.response.rect.left_bottom());
        Area::new(popup_id)
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(220.0);
                    ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for (index, (label, _)) in state.items.iter().enumerate() {
                            let response = ui.selectable_label(index == state.selected, label);
                            if index == state.selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                state.selected = index;
                                accept = true;
                            }
                        }
                    });
                });
            });
        if accept {
            let insert = &state.items[state.selected].1;
            text.replace_range(state.start..state.end, insert);
            let cursor = text[..state.start + insert.len()].chars().count();
            output
                .state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(cursor),
                )));
            output.state.store(ui.ctx(), id);
            output.response.request_focus();
            state.visible = false;
        }
    }
    if state.visible && output.response.has_focus() {
        // egui processes Escape focus navigation before widget key handlers.
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
    }
    ui.ctx().data_mut(|data| data.insert_temp(state_id, state));
}

fn byte_offset(text: &str, offset: usize) -> usize {
    text.char_indices()
        .nth(offset)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

fn candidates(text: &str, cursor: usize, kind: DataSourceKind, object: &str) -> Completion {
    let mut end = cursor.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let start = text[..end]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '$'))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(end);
    let prefix = text[start..end].to_ascii_lowercase();
    let json = kind.is_search() || kind.is_document();
    let list = if kind == DataSourceKind::Redis {
        REDIS
    } else if kind == DataSourceKind::Meilisearch {
        MEILI
    } else if kind.is_document() {
        MONGO
    } else if json {
        ES
    } else {
        SQL
    };
    let mut state = Completion {
        start,
        end,
        cursor: end,
        source: text.to_owned(),
        ..Default::default()
    };
    let quoted = json && text[..start].ends_with('"');
    let mut replacement_end = end;
    if json {
        let key_start = if quoted { start - 1 } else { start };
        if !text[..key_start].trim_end().ends_with(['{', ',']) {
            return state;
        }
        // Replace the entire key (including its quotes), preserving an existing value.
        while text[replacement_end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
        {
            replacement_end += text[replacement_end..].chars().next().unwrap().len_utf8();
        }
        let suffix = &text[replacement_end..];
        let has_quote = suffix.starts_with('"');
        let has_colon = suffix.trim_start_matches('"').trim_start().starts_with(':');
        if quoted {
            state.start = start - 1;
        }
        state.end = replacement_end + usize::from(has_quote);
        for (label, snippet) in list {
            if label.to_ascii_lowercase().starts_with(&prefix) {
                let insert = if has_colon {
                    format!("\"{label}\"")
                } else {
                    snippet.to_string()
                };
                state.items.push((label.to_string(), insert));
            }
        }
    } else {
        for (label, snippet) in list {
            if label.to_ascii_lowercase().starts_with(&prefix) {
                state.items.push((label.to_string(), snippet.to_string()));
            }
        }
        if !object.is_empty() && object.to_ascii_lowercase().starts_with(&prefix) {
            state.items.push((
                object.to_owned(),
                format!("\"{}\"", object.replace('"', "\"\"")),
            ));
        }
    }
    state.visible = !state.items.is_empty();
    state
}

fn add_field_candidates(
    text: &str,
    cursor: usize,
    state: &mut Completion,
    fields: &[(String, String)],
) {
    if fields.is_empty() {
        return;
    }
    let end = cursor.min(text.len());
    if !text.is_char_boundary(end) {
        return;
    }
    let start = text[..end]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(end);
    let quoted = text[..start].ends_with('"');
    let before = text[..start.saturating_sub(usize::from(quoted))].trim_end();
    let key = before.ends_with(['{', ',']);
    let value = quoted && before.ends_with([':', '[', ',']);
    if !key && !value {
        return;
    }
    let mut replacement_end = end;
    while let Some(c) = text[replacement_end..].chars().next() {
        if !(c.is_alphanumeric() || matches!(c, '_' | '.' | '-')) {
            break;
        }
        replacement_end += c.len_utf8();
    }
    let has_quote = text[replacement_end..].starts_with('"');
    let has_colon = text[replacement_end + usize::from(has_quote)..]
        .trim_start()
        .starts_with(':');
    let prefix = text[start..end].to_lowercase();
    state.start = start.saturating_sub(usize::from(quoted));
    state.end = replacement_end + usize::from(has_quote);
    for (field, ty) in fields {
        if !field.to_lowercase().starts_with(&prefix) {
            continue;
        }
        let name = serde_json::to_string(field).unwrap_or_default();
        let insert = if key && !has_colon {
            format!("{name}: null")
        } else {
            name
        };
        state.items.push((format!("{field} ({ty})"), insert));
    }
    state.visible = !state.items.is_empty();
}

pub fn mapping_fields(mapping: &serde_json::Value) -> Vec<(String, String)> {
    fn properties(value: &serde_json::Value, prefix: &str, result: &mut Vec<(String, String)>) {
        for key in ["properties", "fields"] {
            if let Some(fields) = value.get(key).and_then(|v| v.as_object()) {
                for (name, field) in fields {
                    let name = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    result.push((
                        name.clone(),
                        field
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("object")
                            .to_string(),
                    ));
                    properties(field, &name, result);
                }
            }
        }
    }
    let mut result = Vec::new();
    properties(mapping, "", &mut result);
    if let Some(indices) = mapping.as_object() {
        for index in indices.values() {
            let mappings = index.get("mappings").unwrap_or(index);
            properties(mappings, "", &mut result);
            if let Some(types) = mappings.as_object() {
                for typed in types.values() {
                    properties(typed, "", &mut result);
                }
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

const ES: &[(&str, &str)] = &[
    ("query", "\"query\": {}"),
    ("match_all", "\"match_all\": {}"),
    ("match", "\"match\": { \"field\": \"value\" }"),
    ("match_phrase", "\"match_phrase\": { \"field\": \"value\" }"),
    ("term", "\"term\": { \"field\": \"value\" }"),
    ("terms", "\"terms\": { \"field\": [] }"),
    ("bool", "\"bool\": { \"must\": [], \"filter\": [] }"),
    ("must", "\"must\": []"),
    ("filter", "\"filter\": []"),
    ("should", "\"should\": []"),
    ("must_not", "\"must_not\": []"),
    ("range", "\"range\": { \"field\": { \"gte\": 0 } }"),
    ("exists", "\"exists\": { \"field\": \"name\" }"),
    ("sort", "\"sort\": []"),
    ("size", "\"size\": 10"),
    ("from", "\"from\": 0"),
    ("_source", "\"_source\": []"),
    ("aggs", "\"aggs\": {}"),
    ("track_total_hits", "\"track_total_hits\": true"),
];
const MEILI: &[(&str, &str)] = &[
    ("q", "\"q\": \"\""),
    ("filter", "\"filter\": \"\""),
    ("sort", "\"sort\": []"),
    ("limit", "\"limit\": 10"),
    ("offset", "\"offset\": 0"),
    ("facets", "\"facets\": []"),
    ("attributesToRetrieve", "\"attributesToRetrieve\": []"),
];
const MONGO: &[(&str, &str)] = &[
    ("operation", "\"operation\": \"find\""),
    ("filter", "\"filter\": {}"),
    ("projection", "\"projection\": {}"),
    ("sort", "\"sort\": {}"),
    ("pipeline", "\"pipeline\": []"),
    ("document", "\"document\": {}"),
    ("update", "\"update\": {\"$set\": {}}"),
    ("$eq", "\"$eq\": null"),
    ("$ne", "\"$ne\": null"),
    ("$gt", "\"$gt\": 0"),
    ("$gte", "\"$gte\": 0"),
    ("$lt", "\"$lt\": 0"),
    ("$lte", "\"$lte\": 0"),
    ("$in", "\"$in\": []"),
    ("$nin", "\"$nin\": []"),
    ("$and", "\"$and\": []"),
    ("$or", "\"$or\": []"),
    ("$exists", "\"$exists\": true"),
    ("$regex", "\"$regex\": \"\""),
];
const SQL: &[(&str, &str)] = &[
    ("SELECT", "SELECT"),
    ("FROM", "FROM"),
    ("WHERE", "WHERE"),
    ("JOIN", "JOIN"),
    ("ON", "ON"),
    ("GROUP BY", "GROUP BY"),
    ("ORDER BY", "ORDER BY"),
    ("HAVING", "HAVING"),
    ("LIMIT", "LIMIT"),
    ("INSERT INTO", "INSERT INTO"),
    ("UPDATE", "UPDATE"),
    ("DELETE FROM", "DELETE FROM"),
    ("VALUES", "VALUES"),
    ("SET", "SET"),
    ("AND", "AND"),
    ("OR", "OR"),
    ("COUNT", "COUNT(*)"),
];
const REDIS: &[(&str, &str)] = &[
    ("GET", "GET"),
    ("SET", "SET"),
    ("DEL", "DEL"),
    ("EXISTS", "EXISTS"),
    ("SCAN", "SCAN"),
    ("TTL", "TTL"),
    ("EXPIRE", "EXPIRE"),
    ("TYPE", "TYPE"),
    ("HGET", "HGET"),
    ("HGETALL", "HGETALL"),
    ("HSET", "HSET"),
    ("LRANGE", "LRANGE"),
    ("LPUSH", "LPUSH"),
    ("SADD", "SADD"),
    ("SMEMBERS", "SMEMBERS"),
    ("ZADD", "ZADD"),
    ("ZRANGE", "ZRANGE"),
    ("PING", "PING"),
];

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(ctx: &egui::Context, id: Id, text: &mut String, events: Vec<egui::Event>) {
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    editor(ui, id, text, DataSourceKind::Elasticsearch, "", 8);
                });
            },
        );
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn mapping_completion_preserves_nested_field_names_and_existing_values() {
        let mapping = serde_json::json!({"index":{"mappings":{"properties":{
            "user":{"properties":{"name":{"type":"text","fields":{"keyword":{"type":"keyword"}}}}}
        }}}});
        let fields = mapping_fields(&mapping);
        assert!(fields.contains(&("user.name.keyword".into(), "keyword".into())));
        for text in [r#"{"user.na": 1}"#, r#"{"exists":{"field":"user.na"}}"#] {
            let cursor = text.find("user.na").unwrap() + "user.na".len();
            let mut state = Completion::default();
            add_field_candidates(text, cursor, &mut state, &fields);
            let insert = &state
                .items
                .iter()
                .find(|(label, _)| label == "user.name (text)")
                .unwrap()
                .1;
            let mut result = text.to_string();
            result.replace_range(state.start..state.end, insert);
            assert!(serde_json::from_str::<serde_json::Value>(&result).is_ok());
            assert!(result.contains("user.name"));
        }
    }

    #[test]
    fn popup_survives_idle_frames_and_accepts_without_inserting_tab_or_newline() {
        for accept in [Key::Tab, Key::Enter] {
            let ctx = egui::Context::default();
            let id = Id::new("query");
            let mut text = "{\"mat\": {}}".to_owned();
            frame(&ctx, id, &mut text, vec![]);
            ctx.memory_mut(|memory| memory.request_focus(id));
            let mut edit = egui::text_edit::TextEditState::load(&ctx, id).unwrap();
            edit.cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(5),
                )));
            edit.store(&ctx, id);
            frame(&ctx, id, &mut text, vec![key(Key::Space, Modifiers::CTRL)]);
            for _ in 0..3 {
                frame(&ctx, id, &mut text, vec![]);
                assert!(
                    ctx.data_mut(|data| data.get_temp::<Completion>(id.with("completion")))
                        .unwrap()
                        .visible
                );
            }
            frame(&ctx, id, &mut text, vec![key(accept, Modifiers::NONE)]);
            assert_eq!(text, "{\"match_all\": {}}");
            assert!(ctx.memory(|memory| memory.has_focus(id)));
            frame(&ctx, id, &mut text, vec![]);
            assert!(
                !ctx.data_mut(|data| data.get_temp::<Completion>(id.with("completion")))
                    .unwrap()
                    .visible
            );
        }
    }

    #[test]
    fn arrows_choose_candidates_escape_dismisses_and_typing_reopens() {
        let ctx = egui::Context::default();
        let id = Id::new("query");
        let mut text = String::new();
        frame(&ctx, id, &mut text, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(
            &ctx,
            id,
            &mut text,
            vec![egui::Event::Text("{\"mat".to_owned())],
        );
        frame(
            &ctx,
            id,
            &mut text,
            vec![key(Key::ArrowDown, Modifiers::NONE)],
        );
        frame(&ctx, id, &mut text, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(text, "{\"match\": { \"field\": \"value\" }");
        let ctx = egui::Context::default();
        let mut text = String::new();
        frame(&ctx, id, &mut text, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(
            &ctx,
            id,
            &mut text,
            vec![egui::Event::Text("{\"mat".to_owned())],
        );
        frame(&ctx, id, &mut text, vec![key(Key::Escape, Modifiers::NONE)]);
        assert!(
            !ctx.data_mut(|data| data.get_temp::<Completion>(id.with("completion")))
                .unwrap()
                .visible
        );
        frame(&ctx, id, &mut text, vec![egui::Event::Text("c".to_owned())]);
        assert!(
            ctx.data_mut(|data| data.get_temp::<Completion>(id.with("completion")))
                .unwrap()
                .visible
        );
    }

    #[test]
    fn json_values_do_not_offer_key_snippets() {
        let text = "{\"field\": \"mat";
        assert!(!candidates(text, text.len(), DataSourceKind::Elasticsearch, "").visible);
    }

    #[test]
    fn replaces_partial_json_key_without_duplicate_quotes_or_values() {
        for text in ["{\"mat", "{\"mat\"", "{\"mat\": {}}"] {
            let state = candidates(text, 5, DataSourceKind::Elasticsearch, "");
            let insert = &state
                .items
                .iter()
                .find(|(name, _)| name == "match_all")
                .unwrap()
                .1;
            let mut result = text.to_string();
            result.replace_range(state.start..state.end, insert);
            if text.ends_with("{}}") {
                assert_eq!(result, "{\"match_all\": {}}");
            } else {
                assert_eq!(result, "{\"match_all\": {}");
            }
        }
    }
    #[test]
    fn uses_each_query_language_and_handles_unicode_offsets() {
        for (kind, text, expected) in [
            (DataSourceKind::MongoDb, "{\"$g", "$gte"),
            (DataSourceKind::Meilisearch, "{\"li", "limit"),
            (DataSourceKind::CloudflareD1, "SEL", "SELECT"),
            (DataSourceKind::Redis, "HG", "HGET"),
            (
                DataSourceKind::Elasticsearch,
                "{\"\u{4e2d}\u{6587}\": {}, \"mat",
                "match_all",
            ),
        ] {
            let state = candidates(text, text.len(), kind, "");
            assert!(state.visible && state.items.iter().any(|(label, _)| label == expected));
            assert!(text.is_char_boundary(state.start) && text.is_char_boundary(state.end));
        }
    }
}
