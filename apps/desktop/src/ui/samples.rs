#![allow(dead_code)]

use db_common::{CellValue, Column, LogicalType, QueryResult, Row, TableColumn};

pub fn sample_result_set() -> QueryResult {
    let rows = (0..250)
        .map(|index| {
            Row::new(vec![
                CellValue::Int(index),
                if index % 9 == 0 {
                    CellValue::Null
                } else {
                    CellValue::Text(format!("customer_{index:03}"))
                },
                CellValue::Bool(index % 2 == 0),
                CellValue::Bytes(vec![0x52, 0x53, 0x54, index as u8, 0, 255]),
                CellValue::Timestamp(
                    chrono::DateTime::from_timestamp(1_718_000_000 + index * 3600, 0)
                        .unwrap_or_else(db_common::now_utc),
                ),
            ])
        })
        .collect();

    QueryResult {
        statement: "select id, name, active, payload, updated_at from customers".to_string(),
        columns: vec![
            result_column("id", "BIGINT", LogicalType::Int, false),
            result_column("name", "VARCHAR", LogicalType::Text, true),
            result_column("active", "BOOLEAN", LogicalType::Bool, false),
            result_column("payload", "BLOB", LogicalType::Bytes, true),
            result_column("updated_at", "TIMESTAMP", LogicalType::DateTime, false),
        ],
        rows,
        affected_rows: 0,
        execution_time_ms: 18,
        notices: Vec::new(),
        metadata: Default::default(),
    }
}

pub fn sample_table_columns() -> Vec<TableColumn> {
    vec![
        table_column("id", "BIGINT", false, true),
        table_column("name", "VARCHAR", true, false),
        table_column("active", "BOOLEAN", false, false),
        table_column("payload", "BLOB", true, false),
        table_column("updated_at", "TIMESTAMP", false, false),
    ]
}

fn result_column(
    name: &str,
    database_type: &str,
    logical_type: LogicalType,
    nullable: bool,
) -> Column {
    Column {
        name: name.to_string(),
        database_type: database_type.to_string(),
        logical_type,
        nullable,
        primary_key: name == "id",
    }
}

fn table_column(name: &str, database_type: &str, nullable: bool, primary_key: bool) -> TableColumn {
    TableColumn {
        name: name.to_string(),
        database_type: database_type.to_string(),
        nullable,
        default_value: None,
        primary_key,
        comment: None,
    }
}
