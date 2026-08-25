# Orvyn

[中文](README.md)

Orvyn is a cross-platform desktop database client built with Rust, covering daily querying, data browsing, and light maintenance across relational, document, key-value, and search databases. The UI is a native `egui/eframe` implementation on top of a layered Rust workspace, with no ODBC/JDBC/sidecar dependencies.

## Highlights

- **Pure Rust**: native `egui/eframe` interface backed by a Tokio async core.
- **Unified driver registry**: 35 `DataSourceKind` variants are routed over `ProtocolFamily` to shared protocol drivers, with a capability matrix (`SupportLevel` / `ConnectorCapabilities`).
- **Brand sharing**: multiple brands inside the MySQL family and PostgreSQL family share one underlying driver, with brand switches for dialect, port, and capability differences.
- **Capability-driven UI**: connection form fields, query editor availability, and dedicated panels are all driven by the capability matrix.

## Supported Data Sources

Grouped by protocol family:

- **MySQL family**: MySQL, TiDB, MariaDB, OceanBase, Doris, SelectDB, StarRocks, GoldenDB
- **PostgreSQL family**: PostgreSQL, TimescaleDB, Redshift, CockroachDB, OpenGauss, GaussDB, KDB, KingbaseES, Vastbase, HighGo, UXDB
- **Standalone relational/analytics**: SQLite, DuckDB, ClickHouse, SQL Server
- **Document/key-value**: MongoDB, Redis
- **Search (HTTP)**: Elasticsearch, Easysearch, Meilisearch, Manticore Search
- **HTTP-SQL**: Cloudflare D1
- **Restricted**: TDengine (connection probe only); Access, Oracle, Dameng (DM), XuguDB — can be created/saved, tests return a structured "not yet supported" message

Local integration services can reuse `F:\dnmp\docker-compose.orvyn.yml` from the DNMP directory:
`docker compose -f docker-compose.yml -f docker-compose.orvyn.yml up -d mongodb clickhouse sqlserver tidb`.
TimescaleDB uses the existing PostgreSQL service; TiDB listens on `127.0.0.1:4000`, ClickHouse exposes `8123/9000`.

## Architecture

- `DriverRegistry` (app-core): centrally registers and routes each protocol family, removing scattered `match kind` branches.
- `ProtocolFamily` + `ConnectorCapabilities` (db-common): express protocol-family membership and support levels, with clear errors for unknown types.
- Query/document/search/HTTP-SQL go through `RelationalQueryExecutor`, `DocumentConnector`, `SearchConnector`, and `HttpSqlConnector` traits respectively, decoupling the UI from the underlying drivers.

## Current Capabilities

### Connections and Security

- Create, edit, test, group, and delete connections; import or export all or selected connections (imports can overwrite conflicts or rename automatically).
- Optionally store passwords locally with ChaCha20-Poly1305 encryption. Passwords are not saved by default.
- Mark individual connections as read-only. Read-only connections reject write statements and transactions.
- The type selector groups by protocol family and shows support level; form fields are driven by the capability matrix.

### Relational Databases

- Browse databases, schemas, tables, views, and their metadata (columns, indexes, foreign keys, triggers, functions, CREATE TABLE SQL — adapter-dependent).
- Multi-tab SQL editor: execute all statements, the current statement, or the selected SQL; cancellable execution; query history, favorites, result paging and column sorting.
- Table data browse, filter, sort, insert, edit, and delete rows; tables without a primary key are read-only by default.
- Inspect and edit table structure; export structure SQL, or structure-and-data SQL, for a table or database.
- SQL Server (tiberius) and ClickHouse (HTTP) provide query and metadata support.

### Document / Key-Value

- **MongoDB**: browse databases and collections, query documents, basic CRUD.
- **Redis**: logical-database browsing, paginated `SCAN` key lookup, type/TTL/size/value inspection, String/Hash/List/Set/Sorted Set management, and command execution.

### Search / HTTP-SQL

- **Elasticsearch / Easysearch / Meilisearch / Manticore Search**: index browsing and a JSON/DSL query panel.
- **Cloudflare D1**: executes SQL over the HTTP API.

### Import and Export

- Export query results as CSV, JSON, or Excel-compatible tab-separated text; CSV uses a UTF-8 BOM so Excel on Windows recognizes Chinese correctly.
- CSV preview, automatic field mapping, type validation, and batched INSERT SQL generation.

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

After changing the DuckDB version or target platform, or removing this directory, restore the matching local library before building.

## Release and Packaging

Build each platform on its native OS; do not cross-compile all three systems on Linux (GUI, system SDKs, signing, and the DuckDB dynamic library make that unreliable). Release artifacts must carry the matching DuckDB dynamic library and an appropriate installer:

- **Windows x64**: install the Rust MSVC toolchain, the `x86_64-pc-windows-msvc` target, the Windows DuckDB local library, and Inno Setup 6, then run `.\packaging\windows\build-installer.ps1` from the repository root — output is `dist/windows/orvyn-0.1.0-windows-x86_64-setup.exe`.
- **Linux x86_64**: build with `--locked` on an as-old-as-possible base distribution (e.g., Ubuntu 22.04); produces a portable tar.gz, `.AppImage`, or `.deb`.
- **macOS**: produce `.app` and `.dmg` for Apple Silicon and Intel separately; sign and notarize for end-user distribution.

When distributing an executable directly, ship the matching DuckDB dynamic library in the same directory (`duckdb.dll` / `libduckdb.so` / `libduckdb.dylib`). Distributing an installer is generally recommended.

## Workspace Layout

```text
apps/desktop           Desktop entry point and egui UI
crates/app-core        Connection, session, metadata, query, Redis/Search services, and DriverRegistry
crates/db-common       Shared models, traits, errors, capability matrix, and utilities
crates/db-mysql        MySQL protocol family (brand sharing: MariaDB/TiDB/OceanBase/Doris/SelectDB/StarRocks/GoldenDB)
crates/db-postgres     PostgreSQL protocol family (brand sharing: TimescaleDB/Redshift/CockroachDB/OpenGauss/...)
crates/db-sqlite       SQLite adapter
crates/db-duckdb       DuckDB adapter
crates/db-redis        Redis adapter
crates/db-sqlserver    SQL Server adapter (tiberius)
crates/db-clickhouse   ClickHouse HTTP adapter
crates/db-mongodb      MongoDB document adapter
crates/db-http         HTTP search (Elasticsearch/Easysearch/Meilisearch/Manticore) and HTTP-SQL (Cloudflare D1)
crates/db-tdengine     TDengine restricted HTTP connection probe
crates/import-export   CSV/JSON import and export utilities
crates/secure-store    Encrypted credential storage
packaging/windows      Windows installer scripts and assets
```

## License

Licensed under [MIT OR Apache-2.0](Cargo.toml).