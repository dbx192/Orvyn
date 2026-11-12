# Orvyn

[中文](README.md)

A native Rust and egui/eframe database client using native protocols, HTTP, vendor ODBC drivers and a separate DuckDB dynamic library.

## Features

- SQL highlighting, formatting, keyword/table/column completion, aliases, history and favorites.
- Server pagination, filters, sorting and primary-key editing.
- CSV, JSON, Excel-compatible TSV and XLSX export, preserving large integers and decimals as text.
- CSV encoding detection, bounded import batches, progress, cancellation and failed/unconfirmed-row output.
- MongoDB filters, projection, sorting, pagination, aggregation and single-document CRUD.
- Mapping-aware Elasticsearch/Easysearch completion, complete responses, PIT/search_after paging where supported, cancellation and current-page JSON export.
- Meilisearch, Manticore, Cloudflare D1 and Redis operations; D1 preserves each statement result.
- Actual routine definitions for editing, including PostgreSQL overload signatures.
- Encrypted sensitive connection parameters, legacy migration, exports omitting secrets by default, locked atomic storage updates.

## Limits

MySQL/PostgreSQL brands share drivers; versions and permissions may limit support. SQLite/DuckDB use local engines; DuckDB needs matching native libraries. SQL Server/ClickHouse use dedicated connectors; ClickHouse has no client transactions. TDengine supports REST SQL and table/stable metadata, without transactions, views or an index designer. Oracle, DM, XuguDB and Access require matching vendor ODBC drivers. MongoDB UI has no multi-document transactions; D1 has no client-managed transactions.

SQLite/DuckDB support engine interrupts. Ordinary PostgreSQL/MySQL queries send server cancellation. Other connectors, analytical variants and some transaction paths may only stop client waiting; diagnostics identify unconfirmed server termination. Cancellation never guarantees rollback of committed work. Read-only validation conservatively blocks writes; database permissions must constrain server-function side effects.

## Build and verify

Use Rust stable (dependencies require at least 1.88), Python 3 and the host C/C++ toolchain.

```text
python packaging/prepare-duckdb.py
cargo run -p orvyn --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Downloads use a GitHub proxy; set `ORVYN_GITHUB_PROXY` to an empty string for direct access. DuckDB 1.5.4 libraries live in `packaging/duckdb-download/local/1.5.4/`. Prepare again when changing version or architecture.

Linux dependencies: `build-essential pkg-config unixodbc-dev libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev`. macOS: Xcode command line tools and `brew install unixodbc`. Windows: Visual Studio C++ Build Tools and Rust MSVC.

## Packaging and CI

Build on the target OS:

- Windows: `packaging/windows/build-installer.ps1` (Inno Setup 6).
- Linux: `bash packaging/linux/package.sh` produces a `.deb` including DuckDB.
- macOS: `bash packaging/macos/package.sh` produces an `.app`/`.dmg` with corrected library paths and ad hoc signing. Developer ID signing/notarization remains a release step.

`.github/workflows/ci.yml` defines three-platform tests, strict Clippy, packages and disposable PostgreSQL/MySQL/Redis/Elasticsearch integration services. Runner success is required before claiming release readiness. Integration tests are opt-in and read explicit `ORVYN_TEST_*` variables, never local credential files.

## Configuration

- Windows: `%APPDATA%\Orvyn`
- Linux: `$XDG_CONFIG_HOME/orvyn` or `$HOME/.config/orvyn`
- macOS: `$HOME/Library/Application Support/Orvyn`

Override with `ORVYN_CONFIG_DIR`. Supply `ORVYN_MASTER_KEY` or retain the atomically created private `master.key`. Back up the key with encrypted credentials. Invalid existing keys are never silently replaced. A local key cannot protect secrets from someone able to read the entire configuration directory.

See [remediation](docs/REMEDIATION.md) for verification and limits. SSH tunnels, schema/data comparison, ER diagrams, user/permission administration and automatic updates remain roadmap features.

Licensed under MIT OR Apache-2.0; see package metadata.
