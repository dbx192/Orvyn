# Rust DB Client

Rust DB Client 是一个使用 Rust 编写、面向 Redis、MySQL、DuckDB、SQLite、PostgreSQL 的跨平台数据库客户端。

当前仓库已经具备可构建、可测试的 workspace，以及可运行的桌面端主流程：

- 开发计划：`dev.md`
- 编码规范：`rule.md`
- Rust workspace
- egui/eframe 桌面入口和 UI 模块：`apps/desktop`
- 通用模型和全局工具：`crates/db-common`
- 应用服务层：`crates/app-core`
- 五类数据库适配 crate；
- 
- 安全存储与导入导出 crate

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

桌面入口使用 egui/eframe，启动后会初始化连接管理、对象树、查询、Redis 管理、导入导出等服务。

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

- 元数据请求当前仍以短连接/短会话为主，长期会话与更细粒度缓存仍可继续优化。
- Redis key 浏览默认每次扫描 500 个 key。

## 第三阶段能力

已实现 SQL 查询和结果集展示的基础能力：

- 查询执行服务：`app_core::query::QueryService`
- MySQL、PostgreSQL、SQLite、DuckDB 查询执行适配器
- egui 结果集表格：
  - 分页和虚拟滚动
  - 列宽调整
  - 复制单元格、复制行、复制带表头结果
  - NULL、二进制、日期时间字段格式化展示
  - 按列排序
  - CSV、JSON、Excel 兼容格式导出到剪贴板

当前限制：

- SQL 格式化与编辑体验仍是轻量实现，不是完整 SQL IDE。

## 第四阶段能力

已建立桌面前端模块架构：

- `apps/desktop/src/main.rs`：启动、配置目录、服务注入
- `apps/desktop/src/ui/app.rs`：egui 应用壳和视图切换
- `apps/desktop/src/ui/result_grid.rs`：查询结果集展示
- `apps/desktop/src/ui/table_editor.rs`：表数据查看和编辑
- `apps/desktop/src/ui/format.rs`：单元格展示、复制、比较格式化

已实现表数据查看和编辑：

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

- 表数据加载当前默认走 `SELECT * ... LIMIT 500`。
- 更完整的事务控制、批量编辑保护和增量刷新仍可继续增强。

可选环境变量：

```bash
RUST_DB_CLIENT_CONFIG_DIR=.rust-db-client
RUST_DB_CLIENT_MASTER_KEY=change-this-master-key
```

## DuckDB 本地库

当前仓库默认让 `libduckdb-sys` 直接链接本地 DuckDB 运行库，不触发在线下载。

- 项目使用的统一本地库目录：`target/duckdb-download/local/1.5.4`
- 项目级 Cargo 配置：`.cargo/config.toml`

该目录需要包含当前构建目标对应的 DuckDB 库文件：

- Linux x64：`libduckdb.so`、`duckdb.h`
- Windows x64 MSVC：`duckdb.lib`、`duckdb.dll`、`duckdb.h`

如果更换 DuckDB 版本、切换目标平台或删除了该目录，需要先把对应平台的 DuckDB 本地库放回去，再运行 `cargo check`/`cargo build`。

## Windows 11 x64 打包

当前桌面端可以作为普通 Windows 桌面程序构建，目标三元组建议使用：

- `x86_64-pc-windows-msvc`

基础步骤：

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build -p desktop --release --target x86_64-pc-windows-msvc
```

生成产物：

- `target/x86_64-pc-windows-msvc/release/desktop.exe`

当前 `.cargo/config.toml` 固定指向统一本地库目录。为 Windows 目标构建前，需要确认 Windows x64 的 DuckDB 本地库已经放到：

- `target/duckdb-download/local/1.5.4`

其中至少需要包含 `duckdb.lib`、`duckdb.dll` 和 `duckdb.h`。

如果你要把程序拷贝到另一台 Windows 11 64 位机器使用，需要同时带上与目标构建匹配的 DuckDB 动态库文件。

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
- `cargo clippy --workspace --all-targets -- -D warnings`

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
