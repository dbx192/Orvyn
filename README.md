# Rust DB Client

Rust DB Client 是一个计划支持 Redis、MySQL、DuckDB、SQLite、PostgreSQL 的跨平台数据库客户端。

当前仓库处于项目骨架阶段，已包含：

- 开发计划：`dev.md`
- 编码规范：`rule.md`
- Rust workspace
- 通用模型和全局工具：`crates/db-common`
- 应用服务骨架：`crates/app-core`
- 数据库适配器占位 crate
- 安全存储和导入导出占位 crate

## 常用命令

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## 目录

```text
apps/desktop           桌面应用入口
crates/app-core        应用服务层
crates/db-common       通用模型、错误、trait、工具方法
crates/db-mysql        MySQL 适配器
crates/db-postgres     PostgreSQL 适配器
crates/db-sqlite       SQLite 适配器
crates/db-duckdb       DuckDB 适配器
crates/db-redis        Redis 适配器
crates/secure-store    凭据安全存储
crates/import-export   导入导出
```
