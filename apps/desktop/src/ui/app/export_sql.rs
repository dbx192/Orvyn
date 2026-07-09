use super::payloads::DatabaseExportPayload;
use crate::ui::{object_tree::DatabaseExportMode, table_structure::CreateIndexRequest};
use app_core::{connection::ConnectionManager, metadata::MetadataService, query::QueryService};
use db_common::{
    dialect::{BacktickDialect, DoubleQuoteDialect},
    AppError, CellValue, ConnectionId, DataSourceKind, Dialect, Identifier, QueryExecutionMode,
    QueryExecutionRequest, QueryId, QueryResult,
};
use std::{fmt::Write as _, path::PathBuf};

struct DatabaseExportTarget {
    database: Option<String>,
    schema: Option<String>,
    table: String,
}

pub(super) async fn export_database_sql(
    metadata_service: MetadataService,
    query_service: QueryService,
    connection_manager: ConnectionManager,
    connection_id: ConnectionId,
    mode: DatabaseExportMode,
    path: PathBuf,
) -> Result<DatabaseExportPayload, AppError> {
    let profile = connection_manager
        .get_profile(connection_id)
        .await?
        .ok_or_else(|| AppError::validation("connection profile not found"))?;
    if !profile.kind.is_relational() {
        return Err(AppError::validation(
            "database SQL export is only available for relational connections",
        ));
    }

    let dialect = dialect_for_kind(profile.kind);
    let targets = database_export_targets(&metadata_service, &profile, connection_id).await?;
    let mut output = String::new();
    let _ = writeln!(
        &mut output,
        "-- rust-db-client {} export\n-- connection: {}\n",
        database_export_mode_label(mode),
        profile.name
    );

    let mut row_count = 0usize;
    let mut current_mysql_database: Option<String> = None;

    for target in &targets {
        if matches!(profile.kind, DataSourceKind::MySql)
            && current_mysql_database.as_deref() != target.database.as_deref()
        {
            if let Some(database) = target.database.as_deref() {
                let database_sql = quote_identifier(dialect.as_ref(), database)?;
                let _ = writeln!(
                    &mut output,
                    "CREATE DATABASE IF NOT EXISTS {database_sql};\nUSE {database_sql};\n"
                );
                current_mysql_database = Some(database.to_string());
            }
        }

        let qualified_table = qualified_table_sql(
            dialect.as_ref(),
            profile.kind,
            target.database.as_deref(),
            target.schema.as_deref(),
            &target.table,
        )?;
        let label = qualified_table_label(
            target
                .schema
                .as_deref()
                .filter(|value| !value.is_empty())
                .or_else(|| target.database.as_deref().filter(|value| !value.is_empty())),
            &target.table,
        );
        let _ = writeln!(&mut output, "-- table: {label}");

        let ddl = metadata_service
            .get_create_table_sql(
                connection_id,
                target.database.as_deref(),
                target.schema.as_deref(),
                &target.table,
                None,
            )
            .await?;
        let ddl = ddl.trim().trim_end_matches(';').trim();
        if ddl.is_empty() {
            let _ = writeln!(
                &mut output,
                "-- no CREATE TABLE SQL available for {label};\n"
            );
        } else {
            let _ = writeln!(&mut output, "{ddl};\n");
        }

        if mode == DatabaseExportMode::StructureAndData {
            let sql = format!("SELECT * FROM {qualified_table}");
            let report = query_service
                .execute(
                    QueryExecutionRequest {
                        tab_id: QueryId::new(),
                        connection_id,
                        database: target.database.clone(),
                        schema: target.schema.clone(),
                        sql,
                        mode: QueryExecutionMode::All,
                        cursor_offset: 0,
                        selection: None,
                        timeout_ms: 300_000,
                    },
                    None,
                )
                .await?;
            let result = report
                .results
                .into_iter()
                .next()
                .unwrap_or_else(|| QueryResult::empty(""));
            row_count += append_insert_sql(
                &mut output,
                &qualified_table,
                &result,
                dialect.as_ref(),
                profile.kind,
            )?;
        }

        output.push('\n');
    }

    let path_for_write = path.clone();
    tokio::fs::write(&path_for_write, output)
        .await
        .map_err(AppError::from)?;

    Ok(DatabaseExportPayload {
        path,
        mode,
        table_count: targets.len(),
        row_count,
    })
}

async fn database_export_targets(
    metadata_service: &MetadataService,
    profile: &db_common::ConnectionProfile,
    connection_id: ConnectionId,
) -> Result<Vec<DatabaseExportTarget>, AppError> {
    let mut targets = Vec::new();

    match profile.kind {
        DataSourceKind::MySql => {
            let databases = metadata_service.list_databases(connection_id, None).await?;
            let database_names = if databases.is_empty() {
                profile.database.clone().into_iter().collect::<Vec<_>>()
            } else {
                databases
                    .into_iter()
                    .map(|database| database.name)
                    .collect()
            };

            for database in database_names {
                let tables = metadata_service
                    .list_tables(connection_id, Some(database.as_str()), None, None)
                    .await?;
                targets.extend(tables.into_iter().map(|table| DatabaseExportTarget {
                    database: Some(database.clone()),
                    schema: None,
                    table: table.name,
                }));
            }
        }
        DataSourceKind::PostgreSql | DataSourceKind::Sqlite | DataSourceKind::DuckDb => {
            let database = profile.database.clone();
            let schemas = metadata_service
                .list_schemas(connection_id, database.as_deref(), None)
                .await?;

            if schemas.is_empty() {
                let tables = metadata_service
                    .list_tables(connection_id, database.as_deref(), None, None)
                    .await?;
                targets.extend(tables.into_iter().map(|table| DatabaseExportTarget {
                    database: database.clone(),
                    schema: table.schema,
                    table: table.name,
                }));
            } else {
                for schema in schemas {
                    let tables = metadata_service
                        .list_tables(
                            connection_id,
                            database.as_deref(),
                            Some(schema.name.as_str()),
                            None,
                        )
                        .await?;
                    targets.extend(tables.into_iter().map(|table| DatabaseExportTarget {
                        database: database.clone(),
                        schema: table.schema.or_else(|| Some(schema.name.clone())),
                        table: table.name,
                    }));
                }
            }
        }
        DataSourceKind::Redis => {}
    }

    Ok(targets)
}

fn append_insert_sql(
    output: &mut String,
    table_sql: &str,
    result: &QueryResult,
    dialect: &dyn Dialect,
    kind: DataSourceKind,
) -> Result<usize, AppError> {
    if result.rows.is_empty() || result.columns.is_empty() {
        return Ok(0);
    }

    let columns = result
        .columns
        .iter()
        .map(|column| quote_identifier(dialect, &column.name))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");

    for chunk in result.rows.chunks(200) {
        let _ = writeln!(output, "INSERT INTO {table_sql} ({columns}) VALUES");
        for (row_index, row) in chunk.iter().enumerate() {
            let values = (0..result.columns.len())
                .map(|index| {
                    let value = row.values.get(index).unwrap_or(&CellValue::Null);
                    sql_literal_for_cell(value, kind)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let suffix = if row_index + 1 == chunk.len() {
                ";"
            } else {
                ","
            };
            let _ = writeln!(output, "({values}){suffix}");
        }
        output.push('\n');
    }

    Ok(result.rows.len())
}

pub(super) fn build_create_index_sql(
    request: &CreateIndexRequest,
    kind: DataSourceKind,
) -> Result<String, AppError> {
    let dialect = dialect_for_kind(kind);
    let table_sql = qualified_table_sql(
        dialect.as_ref(),
        kind,
        request.database.as_deref(),
        request.schema.as_deref(),
        &request.table,
    )?;
    let index_sql = quote_identifier(dialect.as_ref(), &request.name)?;
    let columns = request
        .columns
        .iter()
        .map(|column| quote_identifier(dialect.as_ref(), column))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let unique = if request.unique { "UNIQUE " } else { "" };

    Ok(format!(
        "CREATE {unique}INDEX {index_sql} ON {table_sql} ({columns})"
    ))
}

pub(super) fn dialect_for_kind(kind: DataSourceKind) -> Box<dyn Dialect> {
    match kind {
        DataSourceKind::MySql => Box::new(BacktickDialect),
        _ => Box::new(DoubleQuoteDialect),
    }
}

pub(super) fn qualified_table_sql(
    dialect: &dyn Dialect,
    kind: DataSourceKind,
    database: Option<&str>,
    schema: Option<&str>,
    table: &str,
) -> Result<String, AppError> {
    let table_sql = quote_identifier(dialect, table)?;
    let qualifier = match kind {
        DataSourceKind::MySql => database.or(schema),
        _ => schema,
    };

    match qualifier.filter(|value| !value.is_empty()) {
        Some(qualifier) => Ok(format!(
            "{}.{table_sql}",
            quote_identifier(dialect, qualifier)?
        )),
        None => Ok(table_sql),
    }
}

fn quote_identifier(dialect: &dyn Dialect, value: &str) -> Result<String, AppError> {
    Ok(dialect.quote_identifier(&Identifier::new(value)?))
}

fn sql_literal_for_cell(value: &CellValue, kind: DataSourceKind) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(value) => {
            if *value {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        CellValue::Int(value) => value.to_string(),
        CellValue::UInt(value) => value.to_string(),
        CellValue::Float(value) => {
            if value.is_finite() {
                value.to_string()
            } else {
                sql_string_literal(&value.to_string())
            }
        }
        CellValue::Decimal(value) => value.clone(),
        CellValue::Text(value) => sql_string_literal(value),
        CellValue::Bytes(value) => match kind {
            DataSourceKind::PostgreSql => format!("'\\\\x{}'", hex_bytes(value)),
            _ => format!("X'{}'", hex_bytes(value)),
        },
        CellValue::Date(value) => sql_string_literal(&value.format("%Y-%m-%d").to_string()),
        CellValue::Time(value) => sql_string_literal(&value.format("%H:%M:%S%.3f").to_string()),
        CellValue::DateTime(value) => {
            sql_string_literal(&value.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        }
        CellValue::Timestamp(value) => sql_string_literal(&value.to_rfc3339()),
        CellValue::Json(value) => sql_string_literal(&value.to_string()),
    }
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn hex_bytes(value: &[u8]) -> String {
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value {
        let _ = write!(&mut output, "{byte:02X}");
    }
    output
}

pub(super) fn database_export_mode_label(mode: DatabaseExportMode) -> &'static str {
    match mode {
        DatabaseExportMode::Structure => "Database structure",
        DatabaseExportMode::StructureAndData => "Database structure and data",
    }
}

pub(super) fn qualified_table_label(qualifier: Option<&str>, table: &str) -> String {
    match qualifier.filter(|value| !value.is_empty()) {
        Some(qualifier) => format!("{qualifier}.{table}"),
        None => table.to_string(),
    }
}
