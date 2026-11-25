use super::{
    prompts::{AiContext, AiSchemaTable},
    AiTableTarget,
};
use app_core::metadata::MetadataService;
use db_common::DatabaseObjectKind;
use futures::{stream, StreamExt};
use std::{collections::BTreeSet, time::Duration};

const MAX_TABLES: usize = 12;
const MAX_COLUMNS: usize = 40;
const MAX_TABLE_NAMES: usize = 160;

impl AiTableTarget {
    pub fn display_name(&self, current_schema: Option<&str>) -> String {
        match self.schema.as_deref() {
            Some(schema) if Some(schema) != current_schema => format!("{schema}.{}", self.name),
            _ => self.name.clone(),
        }
    }
}

pub async fn load(metadata: &MetadataService, context: &mut AiContext, question: &str) {
    let editor = &mut context.editor;
    let Some(connection_id) = editor.connection_id else {
        return;
    };
    let database = editor.database.as_deref();
    let schema = editor.schema.as_deref();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut targets: BTreeSet<_> = std::mem::take(&mut editor.tables).into_iter().collect();
    if !targets.iter().any(|table| !table.is_view) {
        if let Ok(Ok(tables)) = tokio::time::timeout_at(
            deadline,
            metadata.list_tables(connection_id, database, schema, None),
        )
        .await
        {
            targets.extend(tables.into_iter().filter_map(|table| {
                let is_view = match table.kind {
                    DatabaseObjectKind::Table => false,
                    DatabaseObjectKind::View => true,
                    _ => return None,
                };
                Some(AiTableTarget {
                    name: table.name,
                    schema: table.schema.or_else(|| editor.schema.clone()),
                    is_view,
                })
            }));
        } else {
            context.schema_truncated = true;
        }
    }

    let question = question.to_lowercase();
    let mut candidates: Vec<_> = targets
        .into_iter()
        .map(|target| {
            let name = target.display_name(schema);
            let rank = relevance(&target.name, &question, editor.preferred_table.as_deref());
            (rank, name, Some(target))
        })
        .collect();
    for table in &editor.cached_tables {
        if !candidates.iter().any(|(_, name, _)| *name == table.name) {
            let rank = relevance(&table.name, &question, editor.preferred_table.as_deref());
            candidates.push((rank, table.name.clone(), None));
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    context.table_names = candidates
        .iter()
        .take(MAX_TABLE_NAMES)
        .map(|(_, name, _)| name.chars().take(96).collect())
        .collect();
    context.schema_truncated |= candidates.len() > MAX_TABLES;

    let cached_tables = &editor.cached_tables;
    let mut loads = stream::iter(candidates.into_iter().take(MAX_TABLES))
        .map(|(_, name, target)| async move {
            if let Some(table) = cached_tables
                .iter()
                .find(|table| table.name == name && !table.columns.is_empty())
            {
                return Some(table.clone());
            }
            let target = target?;
            let columns = metadata
                .table_columns(
                    connection_id,
                    database,
                    target.schema.as_deref().or(schema),
                    &target.name,
                    None,
                )
                .await
                .ok()?;
            (!columns.is_empty())
                .then(|| AiSchemaTable::from_columns(name, target.is_view, &columns))
        })
        .buffered(4);
    loop {
        match tokio::time::timeout_at(deadline, loads.next()).await {
            Ok(Some(Some(mut table))) => {
                context.schema_truncated |= table.columns.len() > MAX_COLUMNS;
                table.columns.truncate(MAX_COLUMNS);
                context.tables.push(table);
            }
            Ok(Some(None)) => context.schema_truncated = true,
            Ok(None) => break,
            Err(_) => {
                context.schema_truncated = true;
                break;
            }
        }
    }
}

fn relevance(name: &str, question: &str, preferred: Option<&str>) -> u8 {
    let exact = preferred == Some(name);
    let name = name.to_lowercase();
    let mentioned = name.len() >= 3 && question.contains(&name);
    let topic = name.split(|c: char| !c.is_alphanumeric()).any(|part| {
        part.chars().count() >= 3
            && (question.contains(part)
                || ((question.contains("评论") || question.contains("留言"))
                    && matches!(
                        part,
                        "comment" | "comments" | "reply" | "replies" | "review" | "reviews"
                    )))
    });
    4 * u8::from(mentioned) + 2 * u8::from(exact) + u8::from(topic)
}
