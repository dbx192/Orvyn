# Orvyn

[中文](README.md)

Orvyn is a cross-platform desktop database client built with Rust. It supports everyday querying, data browsing, light data maintenance, and Redis key management through an `egui/eframe` interface and a layered Rust workspace.

## Supported Data Sources

- MySQL
- PostgreSQL
- SQLite
- DuckDB
- Redis

## Current Capabilities

### Connections and Security

- Create, edit, test, group, and delete connections.
- Import or export all or selected connections. Imports can overwrite conflicts or rename them automatically.
- Optionally store passwords locally with ChaCha20-Poly1305 encryption. Passwords are not saved by default.
- Mark individual connections as read-only. Read-only connections reject write statements and transactions.
- Test connections for MySQL, PostgreSQL, SQLite, DuckDB, and Redis.

### Relational Databases

- Browse databases, schemas, tables, views, and metadata.
- Inspect columns, indexes, foreign keys, triggers, functions, and CREATE TABLE SQL. Available objects depend on the database adapter.
- Use a single-SQL workspace to execute all SQL, the current statement, or selected SQL; running queries can be cancelled.
- Keep query history and favorites; page and sort result sets.
- Browse, filter, sort, insert, edit, and delete table rows. Tables without a primary key are read-only by default.
- Inspect and edit table structure, including columns, indexes, and foreign keys. DDL support varies by database.
- Export database or table SQL as structure only, or structure plus data.

### Redis

- Browse logical databases and find keys through paginated `SCAN` operations.
- Inspect key type, TTL, size, and value.
- Manage String, Hash, List, Set, and Sorted Set values, and execute Redis commands.

### Import and Export

- Export query results as CSV, JSON, or Excel-compatible tab-separated text.
- CSV output includes a UTF-8 BOM so Excel on Windows recognizes Chinese text correctly.
- Preview CSV files, map fields automatically, validate types, and generate batched INSERT SQL.

## Run from Source

Prerequisites: Rust `1.78` or newer and the C/C++ build toolchain for the current platform.

```powershell
cargo run -p orvyn
```

Useful checks:

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Configuration and Credentials

Default configuration location:

- Windows: `%APPDATA%\Orvyn`
- Other platforms: `$XDG_CONFIG_HOME/orvyn`; when it is unset, `orvyn` under the current directory

The directory holds connection profiles, query history, favorites, and the encryption key. Override it with environment variables:

```powershell
$env:ORVYN_CONFIG_DIR = "D:\\path\\to\\orvyn-config"
$env:ORVYN_MASTER_KEY = "replace-with-a-strong-private-key"
```

When `ORVYN_MASTER_KEY` is not set, the app creates and uses `master.key` in the configuration directory. Protect that directory and key.

## Local DuckDB Library

The project links a local DuckDB library through `libduckdb-sys`; it does not download DuckDB during builds. The default location is:

```text
target/duckdb-download/local/1.5.4
```

The directory must contain the files for the target platform:

- Linux x64: `libduckdb.so`, `duckdb.h`
- Windows x64 MSVC: `duckdb.lib`, `duckdb.dll`, `duckdb.h`

After changing DuckDB version or target platform, or removing this directory, restore the matching local library before building.

## Windows x64 Installer

Install the Rust MSVC toolchain, the `x86_64-pc-windows-msvc` target, the Windows DuckDB local library, and Inno Setup 6. Then run this from the repository root:

```powershell
.\packaging\windows\build-installer.ps1
```

The script builds `orvyn` and produces:

```text
dist/windows/orvyn-0.1.0-windows-x86_64-setup.exe
```

When distributing the executable directly, distribute `duckdb.dll` from the same directory as well. The installer is the recommended distribution format.

## Workspace Layout

```text
apps/desktop           Desktop entry point and egui UI
crates/app-core        Connection, session, metadata, query, Redis, and task services
crates/db-common       Shared models, traits, errors, and utilities
crates/db-mysql        MySQL adapter
crates/db-postgres     PostgreSQL adapter
crates/db-sqlite       SQLite adapter
crates/db-duckdb       DuckDB adapter
crates/db-redis        Redis adapter
crates/import-export   CSV/JSON import and export utilities
crates/secure-store    Encrypted credential storage
packaging/windows      Windows installer scripts and assets
```

## License

Licensed under [MIT OR Apache-2.0](Cargo.toml).
