# Audit remediation

This report distinguishes implemented fixes from runtime validation and new product features.

## Implemented

- [x] Encrypt sensitive connection extras, migrate existing profiles, redact Debug output and default connection exports. Explicit secret export remains opt-in.
- [x] Apply conservative read-only statement checks to SQL and supported document mutations, including HTTP SQL paths.
- [x] Use locked read/modify/write and atomic replacement for local profiles, history, favorites and secret storage. Preserve existing master keys on invalid/empty input.
- [x] Register cancellation and timeout handling for ordinary and transactional query service paths. Implement SQLite/DuckDB interrupts, PostgreSQL cancel requests, and MySQL KILL QUERY for normal pooled queries.
- [x] Implement TDengine REST SQL and metadata with accurate restricted capabilities.
- [x] Implement MongoDB query envelopes for CRUD/aggregation, projection/sort and stable pagination; reject invalid write aggregation paging.
- [x] Preserve raw search responses, including aggregation, timing and hit metadata.
- [x] Add search/document paging, Elasticsearch PIT lifecycle, cancellation and asynchronous atomic current-page JSON export.
- [x] Fetch real PostgreSQL/MySQL routine definitions, disable editing when unavailable, preserve PostgreSQL overload signatures and distinguish procedures when dropping.
- [x] Add mapping-aware search suggestions and SQL dialect keywords, aliases and current-statement completion, including tables after the cursor.
- [x] Add native streaming XLSX output, preserve large numeric values as text, prevent formula interpretation, and replace the output atomically after successful writing.
- [x] Add bounded CSV streaming/snapshots, encoding conversion, batches, progress/cancellation and failure output.
- [x] Cache HTTP/Mongo clients with configuration-aware keys and bounded Mongo client retention.
- [x] Gate metadata actions using adapter capabilities and separate completion/import execution into modules.
- [x] Add three-platform CI, native packaging scripts and opt-in disposable service integration tests.
- [x] Correct README build/dependency/support claims and unify platform configuration directories.
- [x] Align text line endings with host Git autocrlf=true; retain LF for Unix shell scripts.

## Verification

Local Windows checks:

- `cargo test --workspace --locked --offline`: passed (external service tests are intentionally ignored).
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: passed.
- Targeted table structure tests: 16 passed, including procedure parsing and PostgreSQL routine identity handling.
- Import/export tests: 15 passed, including multi-page native XLSX, numeric/text preservation, 10,001-row snapshot streaming, UTF-16 and cancellation.
- SQL completion regression tests and SQLite interrupt/pool reuse tests passed in the workspace run.
- Packaging Python AST, CI YAML parsing, and Git Bash shell syntax checks passed.

Not validated locally: Linux/macOS compilation or packages; live PostgreSQL/MySQL/Redis/Elasticsearch integration tests; declared Rust 1.88 minimum on that exact compiler. The local compiler is 1.96.1. Docker Desktop's Linux engine was unavailable; CI defines disposable services for live tests. These checks must succeed on their target environments before release readiness is claimed.

## Remaining limits

- Transactional PostgreSQL/MySQL queries and drivers without an interrupt implementation report client cancellation without promising server termination. Transaction acquisition itself is not yet interruptible. MySQL analytical protocol variants use the client cancellation fallback.
- Read-only statement classification is conservative, not a database security boundary; enforce database privileges for untrusted SQL and side-effecting functions.
- SQL completion uses lexical/context rules; complete nested-query/CTE scope resolution is not implemented.
- Search JSON export saves the current page. MongoDB offset paging can shift if documents change concurrently; Elasticsearch PIT provides snapshot paging where supported.
- A local master key protects stored credential files, not an attacker with access to the complete user configuration directory. File locks serialize storage updates; independent application instances may still overwrite concurrent logical edits of the same profile.
- macOS packaging uses ad hoc signing; Developer ID signing/notarization and target-system runtime validation remain release tasks. ODBC database types require installed vendor drivers.

## Product roadmap (not implemented)

- [ ] SSH tunnels.
- [ ] Schema and data comparison/synchronization.
- [ ] ER diagrams.
- [ ] Database user/permission administration.
- [ ] Automatic application updates.

These are substantial new features identified during the review. They remain outstanding and are not counted as completed bug fixes.
