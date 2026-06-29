# Rust DB Client

Rust DB Client 是一个计划支持 Redis、MySQL、DuckDB、SQLite、PostgreSQL 的跨平台数据库客户端。

当前仓库处于项目骨架阶段，已包含：

- 开发计划：`dev.md`
- 编码规范：`rule.md`
- Rust workspace
- egui/eframe 桌面入口和 UI 模块：`apps/desktop`
- 通用模型和全局工具：`crates/db-common`
- 应用服务骨架：`crates/app-core`
- 数据库适配器占位 crate
- 安全存储和导入导出占位 crate

## 第一阶段能力

已实现连接管理后端基础能力：

- 连接配置 CRUD 服务：`app_core::connection::ConnectionManager`
- 连接配置文件持久化：`app_core::storage::FileConnectionProfileStore`
- 本地加密凭据存储：`secure_store::EncryptedFileStore`
- 五类数据源连接测试适配器：
  - `db_mysql::MySqlConnectionTester`
  - `db_postgres::PostgresConnectionTester`
  - `db_sqlite::SqliteConnectionTester`
  - `db_duckdb::DuckDbConnectionTester`
  - `db_redis::RedisConnectionTester`
- 密码保存策略：`PasswordUpdate::Keep`、`Clear`、`Replace`
- 默认不保存密码，只有 `ConnectionSecurity.save_password = true` 时才写入安全存储。

桌面入口使用 egui/eframe，当前会初始化连接管理服务，并包含结果集展示和表数据编辑原型。

## 第二阶段能力

已实现对象树和元数据后端基础能力：

- 对象树服务：`app_core::metadata::MetadataService`
- 统一对象树节点：`MetadataNode`、`MetadataPath`、`MetadataNodeKind`
- 关系型数据库元数据读取：
  - 数据库
  - schema
  - 表
  - 视图
  - 字段
  - 索引
- Redis 元数据读取：
  - DB 编号列表
  - DB key 数量
  - 使用 `SCAN` 浏览 key
  - key 类型和 TTL
- 对象树采用懒加载接口：`MetadataService::children`

当前限制：

- 没有实现前端对象树 UI。
- 每次元数据请求会短连接数据库，查询会话和连接池放到后续阶段。
- Redis key 浏览默认每次扫描 500 个 key。

## 第三阶段能力

已实现 SQL 查询和结果集展示的基础能力：

- 查询执行服务：`app_core::query::QueryService`
- MySQL、PostgreSQL、SQLite、DuckDB 查询执行适配器
- egui 结果集表格原型：
  - 分页和虚拟滚动
  - 列宽调整
  - 复制单元格、复制行、复制带表头结果
  - NULL、二进制、日期时间字段格式化展示
  - 按列排序
  - CSV、JSON、Excel 兼容格式导出到剪贴板

当前限制：

- 查询编辑器和真实查询结果 UI 接入还在后续阶段。
- 导出当前先复制到剪贴板，文件保存对话框后续补齐。

## 第四阶段能力

已建立桌面前端模块架构：

- `apps/desktop/src/main.rs`：启动、配置目录、服务注入
- `apps/desktop/src/ui/app.rs`：egui 应用壳和视图切换
- `apps/desktop/src/ui/result_grid.rs`：查询结果集展示
- `apps/desktop/src/ui/table_editor.rs`：表数据查看和编辑
- `apps/desktop/src/ui/format.rs`：单元格展示、复制、比较格式化

已实现表数据查看和编辑原型：

- 打开表数据视图
- 分页浏览
- 新增行
- 编辑单元格
- 删除行
- 提交生成 SQL 预览
- 回滚未提交变更
- 根据主键生成 UPDATE/DELETE
- 无主键表默认只读，可显式开启风险模式
- 简单过滤和排序

当前限制：

- 表数据视图当前使用示例数据。
- 提交动作先生成 SQL 预览，真实事务执行和刷新表数据后续接入 `QueryService`。

可选环境变量：

```bash
RUST_DB_CLIENT_CONFIG_DIR=.rust-db-client
RUST_DB_CLIENT_MASTER_KEY=change-this-master-key
```

## DuckDB 本地库

当前仓库默认使用已下载到仓库内的 DuckDB 本地库，不再在构建时联网下载。

- 本地库目录：`target/duckdb-download/x86_64-unknown-linux-gnu/1.5.4`
- 项目级 Cargo 配置：`.cargo/config.toml`

如果你更换了 DuckDB 版本、目标平台或删除了该目录，需要同步更新 `.cargo/config.toml` 中的：

- `DUCKDB_LIB_DIR`
- `DUCKDB_INCLUDE_DIR`

如果本地库不存在，`db-duckdb`、`app-core` 和 `desktop` 的相关构建会失败；当前默认策略是不再回退到自动下载。

## 常用命令

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

当前已验证的本地基线：

- `cargo check --workspace`
- `cargo test --workspace`

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
