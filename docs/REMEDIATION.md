# Audit remediation

User-authorized scope: fix the audit findings and complete the missing capabilities.
Status is based on implemented and verified behavior, not planned work.

- [ ] Encrypt sensitive connection parameters, migrate existing profiles, redact exports/debug output.
- [ ] Enforce read-only SQL consistently, including HTTP SQL.
- [ ] Atomic persistence and serialized read/modify/write for profiles, history, favorites and secrets.
- [ ] Query cancellation: driver support and accurate UI semantics.
- [ ] TDengine SQL execution and metadata.
- [ ] MongoDB CRUD, aggregation, projection, sorting and pagination.
- [ ] Preserve search aggregation, timing and hit metadata.
- [ ] Search/document paging, cancellation and export.
- [ ] Fetch real stored routine definitions for editing.
- [ ] Metadata-aware search completion and SQL dialect/scopes.
- [ ] Native XLSX export.
- [ ] Bounded streaming CSV import, progress/cancel and failure output.
- [ ] Reuse HTTP/MongoDB clients with configuration-aware invalidation.
- [ ] More precise capability declarations.
- [ ] Split large UI modules at coherent boundaries.
- [ ] CI, cross-platform build/package checks and service integration tests.
- [ ] Correct README support and packaging claims.

Additional roadmap features identified by the audit (not existing bugs): SSH tunnels,
schema/data comparison, ER diagrams, user/permission management and automatic updates.
These remain part of the requested review scope; none is claimed implemented here.

## Verification

Pending. Real database validation requires suitable service instances; never include
credentials or local private configuration in this document.
