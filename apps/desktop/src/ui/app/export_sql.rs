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
    selected_database: Option<String>,
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
    let targets = database_export_targets(
        &metadata_service,
        &profile,
        connection_id,
        selected_database.as_deref(),
    )
    .await?;
    let mut output = String::new();
    let _ = writeln!(
        &mut output,
        "-- Orvyn {} export\n-- connection: {}\n",
        database_export_mode_label(mode),
        profile.name
    );
    if let Some(database) = selected_database.as_deref() {
        let _ = writeln!(&mut output, "-- database: {database}\n");
    }
    if targets.is_empty() {
        let _ = writeln!(&mut output, "-- no tables found\n");
    }

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
                        result_limit: None,
                        result_offset: 0,
                        result_total_rows: None,
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
    selected_database: Option<&str>,
) -> Result<Vec<DatabaseExportTarget>, AppError> {
    let mut targets = Vec::new();

    match profile.kind {
        DataSourceKind::MySql => {
            let database_names = if let Some(database) = selected_database {
                vec![database.to_string()]
            } else {
                let databases = metadata_service.list_databases(connection_id, None).await?;
                if databases.is_empty() {
                    profile.database.clone().into_iter().collect::<Vec<_>>()
                } else {
                    databases
                        .into_iter()
                        .map(|database| database.name)
                        .collect()
                }
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
            let database = selected_database
                .map(ToOwned::to_owned)
                .or_else(|| profile.database.clone());
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
        DataSourceKind::Redis | DataSourceKind::MongoDb => {}
        _ => {
            let database = selected_database.or(profile.database.as_deref());
            let tables = metadata_service
                .list_tables(connection_id, database, None, None)
                .await?;
            targets.extend(tables.into_iter().map(|table| DatabaseExportTarget {
                database: database.map(ToOwned::to_owned),
                schema: table.schema,
                table: table.name,
            }));
        }
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
        DataSourceKind::MySql | DataSourceKind::Doris => Box::new(BacktickDialect),
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

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::{storage::MemoryConnectionProfileStore, task::TaskManager};
    use db_common::{ConnectionPoolConfig, ConnectionProfile, PasswordUpdate};
    use secure_store::EncryptedFileStore;
    use std::{sync::Arc, time::SystemTime};

    struct TempExport {
        database: PathBuf,
        output: PathBuf,
        secrets: PathBuf,
    }

    impl TempExport {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos();
            let base =
                std::env::temp_dir().join(format!("orvyn-export-{}-{nonce}", std::process::id()));
            Self {
                database: base.with_extension("duckdb"),
                output: base.with_extension("sql"),
                secrets: base.with_extension("secrets.json"),
            }
        }
    }

    impl Drop for TempExport {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.database);
            let _ = std::fs::remove_file(&self.output);
            let _ = std::fs::remove_file(&self.secrets);
        }
    }

    #[tokio::test]
    async fn exports_selected_duckdb_structure_and_data_to_file() {
        let files = TempExport::new();
        let conn = duckdb::Connection::open(&files.database).expect("open temporary DuckDB");
        conn.execute_batch(
            "CREATE TABLE items (id INTEGER, name VARCHAR); \
             INSERT INTO items VALUES (1, 'first')",
        )
        .expect("seed temporary DuckDB");
        drop(conn);

        let connection_manager = ConnectionManager::new(
            Arc::new(MemoryConnectionProfileStore::default()),
            Arc::new(EncryptedFileStore::new(&files.secrets, "test-key")),
            Vec::new(),
        );
        let profile = ConnectionProfile::builder(DataSourceKind::DuckDb, "export test")
            .file_path(files.database.to_string_lossy())
            .build()
            .expect("build profile");
        let profile = connection_manager
            .create_profile(profile, PasswordUpdate::Keep)
            .await
            .expect("create profile");
        let metadata_service = MetadataService::with_default_readers(connection_manager.clone());
        let query_service = QueryService::new(
            connection_manager.clone(),
            vec![Arc::new(db_duckdb::DuckDbQueryExecutor::new())],
            ConnectionPoolConfig::default(),
            TaskManager::default(),
            None,
            None,
        );
        let payload = export_database_sql(
            metadata_service,
            query_service,
            connection_manager,
            profile.id,
            Some(files.database.to_string_lossy().into_owned()),
            DatabaseExportMode::StructureAndData,
            files.output.clone(),
        )
        .await
        .expect("export database");

        let sql = std::fs::read_to_string(&files.output).expect("read export file");
        assert_eq!(payload.table_count, 1);
        assert_eq!(payload.row_count, 1);
        assert!(sql.contains("CREATE TABLE \"main\".\"items\""));
        assert!(sql.contains("INSERT INTO \"main\".\"items\""));
        assert!(sql.contains("'first'"));
    }
}
