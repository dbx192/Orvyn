//! Metadata reader for the ODBC bridge. Catalog reads use the ODBC catalog
//! functions (`SQLTables`/`SQLColumns`/`SQLPrimaryKeys`/`SQLForeignKeys`);
//! indexes have no odbc-api wrapper, so Oracle/DM fall back to their shared
//! `ALL_INDEXES` dictionary SQL and other dialects return an empty list.

use crate::connect::{self, driver_name_for, map_odbc_error_for};
use crate::kind::VendorDialect;
use async_trait::async_trait;
use db_common::{
    AppError, ConnectionProfile, DatabaseObject, DatabaseObjectKind, DataSourceKind, ErrorKind,
    RelationalMetadataReader, Result, TableColumn, TableForeignKey, TableIndex,
};
use odbc_api::parameter::VarCharArray;
use odbc_api::{ColumnsRow, Connection, Cursor, ResultSetMetadata};

pub struct OdbcMetadataReader {
    kind: DataSourceKind,
}

impl OdbcMetadataReader {
    pub const fn new(kind: DataSourceKind) -> Self {
        Self { kind }
    }
}

#[async_trait]
impl RelationalMetadataReader for OdbcMetadataReader {
    fn kind(&self) -> DataSourceKind {
        self.kind
    }

    async fn list_databases(
        &self,
        profile: &ConnectionProfile,
        _password: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        // ODBC has no "list databases" catalog function; the bridge exposes
        // the single database the profile connects to.
        let name = profile
            .database
            .clone()
            .filter(|database| !database.trim().is_empty())
            .unwrap_or_else(|| "default".to_string());
        Ok(vec![DatabaseObject {
            kind: DatabaseObjectKind::Database,
            name,
            schema: None,
            parent: None,
        }])
    }

    async fn list_schemas(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        let dialect = VendorDialect::of(self.kind)?;
        if !dialect.supports_schemas() {
            return Ok(Vec::new());
        }

        let mut schemas = Vec::new();
        if let Some(default_schema) = dialect.default_schema(profile) {
            schemas.push(default_schema);
        }

        // Oracle and DM both expose ALL_USERS; other dialects keep just the
        // default schema entry (the only one the login is guaranteed to see).
        if matches!(dialect, VendorDialect::Oracle | VendorDialect::Dm) {
            let driver_name = driver_name_for(profile);
            let kind = self.kind;
            let names = connect::with_connection(profile, password, move |connection| {
                fetch_text_rows(
                    connection,
                    "SELECT username FROM all_users ORDER BY username",
                    kind,
                    &driver_name,
                )
            })
            .await?;
            for row in names {
                if let Some(name) = row.first().filter(|name| !name.is_empty()) {
                    if !schemas.contains(name) {
                        schemas.push(name.clone());
                    }
                }
            }
        }

        Ok(schemas
            .into_iter()
            .map(|name| DatabaseObject {
                kind: DatabaseObjectKind::Schema,
                name,
                schema: None,
                parent: None,
            })
            .collect())
    }

    async fn list_tables(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        list_catalog_objects(self.kind, profile, password, schema, "TABLE").await
    }

    async fn list_views(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<DatabaseObject>> {
        list_catalog_objects(self.kind, profile, password, schema, "VIEW").await
    }

    async fn list_columns(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableColumn>> {
        let dialect = VendorDialect::of(self.kind)?;
        let schema_filter = resolve_schema(dialect, profile, schema).unwrap_or_default();
        let driver_name = driver_name_for(profile);
        let kind = self.kind;
        let table = table.to_string();

        connect::with_connection(profile, password, move |connection| {
            let iterator = connection
                .columns("", &schema_filter, &table, "")
                .map_err(|error| map_odbc_error_for(kind, &driver_name, error))?;

            let mut columns = Vec::new();
            for row in iterator {
                let row: ColumnsRow = row.map_err(|error| map_odbc_error_for(kind, &driver_name, error))?;
                let name = cell_value(&row.column_name);
                if name.is_empty() {
                    continue;
                }
                let nullable =
                    row.nullable != 0 || cell_value(&row.is_nullable).eq_ignore_ascii_case("YES");
                columns.push(TableColumn {
                    name,
                    database_type: column_type_label(&row),
                    nullable,
                    default_value: non_empty(cell_value(&row.column_default)),
                    primary_key: false,
                    comment: non_empty(cell_value(&row.remarks)),
                });
            }

            let primary_keys = primary_key_columns(connection, kind, &driver_name, &schema_filter, &table)?;
            for column in &mut columns {
                if primary_keys.contains(&column.name) {
                    column.primary_key = true;
                }
            }
            Ok(columns)
        })
        .await
    }

    async fn list_indexes(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableIndex>> {
        let dialect = VendorDialect::of(self.kind)?;
        if !matches!(dialect, VendorDialect::Oracle | VendorDialect::Dm) {
            // odbc-api has no SQLStatistics wrapper; Xugu/Access stay empty
            // until dialect SQL is contributed for them.
            return Ok(Vec::new());
        }

        let schema = resolve_schema(dialect, profile, schema).ok_or_else(|| {
            AppError::validation("schema is required to list indexes")
        })?;
        let sql = format!(
            "SELECT i.INDEX_NAME, i.UNIQUENESS, c.CONSTRAINT_TYPE, ic.COLUMN_NAME \
             FROM ALL_INDEXES i \
             JOIN ALL_IND_COLUMNS ic \
               ON ic.INDEX_OWNER = i.OWNER AND ic.INDEX_NAME = i.INDEX_NAME \
             LEFT JOIN ALL_CONSTRAINTS c \
               ON c.OWNER = i.OWNER AND c.CONSTRAINT_NAME = i.INDEX_NAME \
              AND c.CONSTRAINT_TYPE = 'P' \
             WHERE i.OWNER = {} AND i.TABLE_NAME = {} \
             ORDER BY i.INDEX_NAME, ic.COLUMN_POSITION",
            sql_quote(&schema),
            sql_quote(table),
        );

        let driver_name = driver_name_for(profile);
        let kind = self.kind;
        let rows = connect::with_connection(profile, password, move |connection| {
            fetch_text_rows(connection, &sql, kind, &driver_name)
        })
        .await?;

        let mut indexes: Vec<TableIndex> = Vec::new();
        for row in rows {
            let Some(name) = row.first().filter(|name| !name.is_empty()) else {
                continue;
            };
            let column = row.get(3).cloned().unwrap_or_default();
            if let Some(index) = indexes.iter_mut().find(|index| &index.name == name) {
                index.columns.push(column);
            } else {
                indexes.push(TableIndex {
                    name: name.clone(),
                    unique: row.get(1).is_some_and(|u| u.eq_ignore_ascii_case("UNIQUE")),
                    primary: row.get(2).is_some_and(|c| c.eq_ignore_ascii_case("P")),
                    columns: vec![column],
                });
            }
        }
        Ok(indexes)
    }

    async fn list_foreign_keys(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        _database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Vec<TableForeignKey>> {
        let dialect = VendorDialect::of(self.kind)?;
        let schema_filter = resolve_schema(dialect, profile, schema).unwrap_or_default();
        let driver_name = driver_name_for(profile);
        let kind = self.kind;
        let table = table.to_string();

        connect::with_connection(profile, password, move |connection| {
            // Empty PK filters + FK table filter: list foreign keys owned by
            // `table` (the child side).
            let iterator = connection
                .foreign_keys("", "", "", "", &schema_filter, &table)
                .map_err(|error| map_odbc_error_for(kind, &driver_name, error))?;

            let mut rows: Vec<(i16, odbc_api::ForeignKeysRow)> = Vec::new();
            for row in iterator {
                let row = row.map_err(|error| map_odbc_error_for(kind, &driver_name, error))?;
                rows.push((row.key_seq, row));
            }
            rows.sort_by_key(|(seq, _)| *seq);

            let mut keys: Vec<TableForeignKey> = Vec::new();
            for (_, row) in rows {
                let columns = cell_value(&row.fk_column);
                let referenced_table = cell_value(&row.pk_table);
                let entry = keys.iter_mut().find(|key| {
                    key.referenced_table == referenced_table && key.name == cell_value(&row.fk_name)
                });
                if let Some(key) = entry {
                    key.columns.push(columns);
                } else {
                    keys.push(TableForeignKey {
                        name: cell_value(&row.fk_name),
                        columns: vec![columns],
                        referenced_table,
                        referenced_columns: vec![cell_value(&row.pk_column)],
                        on_update: rule_label(row.update_rule.into_opt()),
                        on_delete: rule_label(row.delete_rule.into_opt()),
                    });
                }
            }
            Ok(keys)
        })
        .await
    }

    async fn get_create_table_sql(
        &self,
        profile: &ConnectionProfile,
        password: Option<&str>,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<String> {
        let dialect = VendorDialect::of(self.kind)?;
        let schema = resolve_schema(dialect, profile, schema);
        let columns = self
            .list_columns(profile, password, database, schema.as_deref(), table)
            .await?;
        if columns.is_empty() {
            return Err(AppError::new(
                ErrorKind::NotFound,
                format!("table {} not found", table),
            ));
        }

        let quote = |name: &str| {
            if matches!(dialect, VendorDialect::Access) {
                format!("[{name}]")
            } else {
                format!("\"{name}\"")
            }
        };

        let mut definitions: Vec<String> = Vec::with_capacity(columns.len() + 1);
        for column in &columns {
            let mut definition = format!("    {} {}", quote(&column.name), column.database_type);
            if let Some(default) = &column.default_value {
                definition.push_str(&format!(" DEFAULT {default}"));
            }
            if !column.nullable && !column.primary_key {
                definition.push_str(" NOT NULL");
            }
            definitions.push(definition);
        }
        let primary_keys: Vec<String> = columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| quote(&column.name))
            .collect();
        if !primary_keys.is_empty() {
            definitions.push(format!("    PRIMARY KEY ({})", primary_keys.join(", ")));
        }

        let qualified = match &schema {
            Some(schema) => format!("{}.{}", quote(schema), quote(table)),
            None => quote(table),
        };
        Ok(format!(
            "CREATE TABLE {qualified} (\n{}\n);",
            definitions.join(",\n")
        ))
    }
}

/// `SQLTables`-based object listing. The type filter is applied client-side:
/// drivers disagree about quoting `TABLE_TYPE` arguments, but the returned
/// rows always carry the canonical type string.
async fn list_catalog_objects(
    kind: DataSourceKind,
    profile: &ConnectionProfile,
    password: Option<&str>,
    schema: Option<&str>,
    wanted_type: &str,
) -> Result<Vec<DatabaseObject>> {
    let dialect = VendorDialect::of(kind)?;
    let schema_filter = resolve_schema(dialect, profile, schema).unwrap_or_default();
    let driver_name = driver_name_for(profile);
    let object_kind = if wanted_type.eq_ignore_ascii_case("VIEW") {
        DatabaseObjectKind::View
    } else {
        DatabaseObjectKind::Table
    };
    let wanted_type = wanted_type.to_string();

    connect::with_connection(profile, password, move |connection| {
        let iterator = connection
            .tables("", &schema_filter, "", "")
            .map_err(|error| map_odbc_error_for(kind, &driver_name, error))?;

        let mut objects = Vec::new();
        for row in iterator {
            let row: odbc_api::TablesRow =
                row.map_err(|error| map_odbc_error_for(kind, &driver_name, error))?;
            let table_type = cell_value(&row.table_type);
            if !table_type.eq_ignore_ascii_case(&wanted_type) {
                continue;
            }
            let name = cell_value(&row.table);
            if name.is_empty() {
                continue;
            }
            objects.push(DatabaseObject {
                kind: object_kind,
                name,
                schema: {
                    let schema = cell_value(&row.schema);
                    (!schema.is_empty()).then_some(schema)
                },
                parent: None,
            });
        }
        Ok(objects)
    })
    .await
}

/// Schema filter for catalog calls. `None` degrades to the dialect's default
/// schema (Oracle: upper-cased login user); an explicit selection wins.
fn resolve_schema(
    dialect: VendorDialect,
    profile: &ConnectionProfile,
    schema: Option<&str>,
) -> Option<String> {
    schema
        .map(str::to_string)
        .filter(|schema| !schema.trim().is_empty())
        .or_else(|| dialect.default_schema(profile))
}

fn cell_value<const N: usize>(cell: &VarCharArray<N>) -> String {
    cell.as_str()
        .ok()
        .flatten()
        .unwrap_or("")
        .to_string()
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn rule_label(rule: Option<i16>) -> Option<String> {
    rule.map(|code| {
        let label = match code {
            0 => "CASCADE",
            1 => "RESTRICT",
            2 => "SET NULL",
            3 => "NO ACTION",
            4 => "SET DEFAULT",
            _ => "NO ACTION",
        };
        label.to_string()
    })
}

fn sql_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Fetch every row of a result set as plain text. Used for dictionary SQL
/// (indexes, users); catalog-function reads go through their typed rows.
fn fetch_text_rows(
    connection: &Connection<'static>,
    sql: &str,
    kind: DataSourceKind,
    driver_name: &str,
) -> Result<Vec<Vec<String>>> {
    let mut cursor = connection
        .execute(sql, (), None)
        .map_err(|error| map_odbc_error_for(kind, driver_name, error))?
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Query,
                "metadata query returned no result set",
            )
        })?;

    let column_count = cursor
        .num_result_cols()
        .map_err(|error| map_odbc_error_for(kind, driver_name, error))?
        .max(0) as usize;

    let mut rows = Vec::new();
    let mut buffer = Vec::new();
    while let Some(mut row) = cursor
        .next_row()
        .map_err(|error| map_odbc_error_for(kind, driver_name, error))?
    {
        let mut values = Vec::with_capacity(column_count);
        for column in 1..=column_count as u16 {
            buffer.clear();
            let has_value = row
                .get_text(column, &mut buffer)
                .map_err(|error| map_odbc_error_for(kind, driver_name, error))?;
            values.push(if has_value {
                String::from_utf8_lossy(&buffer).into_owned()
            } else {
                String::new()
            });
        }
        rows.push(values);
    }
    Ok(rows)
}

fn primary_key_columns(
    connection: &Connection<'static>,
    kind: DataSourceKind,
    driver_name: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<String>> {
    let schema_param = if schema.is_empty() { None } else { Some(schema) };
    let iterator = connection
        .primary_keys(None, schema_param, table)
        .map_err(|error| map_odbc_error_for(kind, driver_name, error))?;

    let mut names = Vec::new();
    for row in iterator {
        let row: odbc_api::PrimaryKeysRow =
            row.map_err(|error| map_odbc_error_for(kind, driver_name, error))?;
        let name = cell_value(&row.column);
        if !name.is_empty() {
            names.push(name);
        }
    }
    Ok(names)
}

/// `TYPE_NAME` plus the conventional `(n)` / `(p,s)` suffix when the driver
/// reports a meaningful size, e.g. `VARCHAR2(100)` or `NUMBER(38,0)`.
fn column_type_label(row: &ColumnsRow) -> String {
    let base = cell_value(&row.type_name);
    if base.is_empty() {
        return "UNKNOWN".to_string();
    }
    let upper = base.to_ascii_uppercase();
    let size = row.column_size.as_opt().copied().unwrap_or(0);
    let digits = row.decimal_digits.as_opt().copied().unwrap_or(0);

    if (upper.contains("CHAR") || upper.contains("TEXT")) && size > 0 {
        format!("{base}({size})")
    } else if matches!(upper.as_str(), "NUMBER" | "NUMERIC" | "DECIMAL" | "DEC") && size > 0 {
        format!("{base}({size},{digits})")
    } else {
        base
    }
}
