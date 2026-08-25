# Orvyn

[English](README.en.md)

Orvyn 是一个使用 Rust 构建的跨平台桌面数据库客户端，覆盖关系型、文档、键值与搜索数据库的日常查询、数据浏览和轻量维护。界面采用 `egui/eframe` 原生实现，核心为一个分层的 Rust workspace，无 ODBC/JDBC/sidecar 依赖。

## 功能亮点

- **纯 Rust**：原生 `egui/eframe` 界面 + Tokio 异步核心。
- **统一驱动注册表**：35 种 `DataSourceKind` 按 `ProtocolFamily` 路由到共享协议驱动，并带能力矩阵（`SupportLevel` / `ConnectorCapabilities`）。
- **品牌共享**：MySQL 族、PostgreSQL 族的多品牌数据库共享同一底层驱动，以品牌开关处理方言、端口和能力差异。
- **能力驱动的 UI**：连接表单字段、查询编辑器可用性、专用面板均由能力矩阵驱动。

## 支持的数据源

按协议族分组：

- **MySQL 族**：MySQL、TiDB、MariaDB、OceanBase、Doris、SelectDB、StarRocks、GoldenDB
- **PostgreSQL 族**：PostgreSQL、TimescaleDB、Redshift、CockroachDB、OpenGauss、GaussDB、KDB、KingbaseES、Vastbase、HighGo、UXDB
- **独立关系/分析**：SQLite、DuckDB、ClickHouse、SQL Server
- **文档/键值**：MongoDB、Redis
- **搜索（HTTP）**：Elasticsearch、Easysearch、Meilisearch、Manticore Search
- **HTTP-SQL**：Cloudflare D1
- **受限**：TDengine（仅连接探测）；Access、Oracle、达梦（DM）、虚谷（XuguDB）——可创建/保存，测试返回结构化“暂不支持”提示

本地集成测试可复用 `F:\dnmp\docker-compose.orvyn.yml`（路径以你的环境为准）：在 DNMP 目录执行
`docker compose -f docker-compose.yml -f docker-compose.orvyn.yml up -d mongodb clickhouse sqlserver tidb`。
TimescaleDB 通过现有 PostgreSQL 服务配置，TiDB 使用 `127.0.0.1:4000`，ClickHouse 同时提供 `8123/9000`。

## 架构

- `DriverRegistry`（app-core）：集中注册并路由各协议族驱动，消除散落的 `match kind` 分支。
- `ProtocolFamily` + `ConnectorCapabilities`（db-common）：表达协议族归属与支持级别，未知类型有清晰错误。
- 查询/文档/搜索/HTTP-SQL 分别走 `RelationalQueryExecutor`、`DocumentConnector`、`SearchConnector`、`HttpSqlConnector` trait，UI 与底层驱动解耦。

## 当前功能

### 连接与安全

- 新建、编辑、测试、分组和删除连接；导入或导出全部或选中的连接（导入时可覆盖冲突项或自动重命名）。
- 可选地在本地加密保存密码（ChaCha20-Poly1305）；默认不保存密码。
- 每个连接可设为只读；只读连接拒绝写语句与启动事务。
- 类型选择器按协议族分组并显示支持级别，表单字段由能力矩阵驱动。

### 关系型数据库

- 浏览数据库、schema、表、视图及其元数据（列、索引、外键、触发器、函数、建表 SQL，视适配器而定）。
- 多标签 SQL 编辑器：执行全部语句、当前语句或选中 SQL；可取消执行；查询历史、收藏、结果分页与列排序。
- 表数据浏览、筛选、排序及新增、编辑、删除行；无主键表默认只读。
- 表结构查看与编辑；导出表或数据库的结构 SQL、或结构及数据 SQL。
- SQL Server（tiberius）与 ClickHouse（HTTP）提供查询与元数据支持。

### 文档 / 键值

- **MongoDB**：浏览数据库与集合、查询文档、基础 CRUD。
- **Redis**：逻辑库浏览、`SCAN` 分页查找 key、类型/TTL/大小/值查看、String/Hash/List/Set/Sorted Set 管理及命令执行。

### 搜索 / HTTP-SQL

- **Elasticsearch / Easysearch / Meilisearch / Manticore Search**：索引浏览与 JSON/DSL 查询面板。
- **Cloudflare D1**：通过 HTTP API 执行 SQL。

### 导入导出

- 将查询结果导出为 CSV、JSON 或 Excel 兼容的制表符分隔文本；CSV 使用 UTF-8 BOM，便于 Windows 上的 Excel 识别中文。
- 支持 CSV 预览、字段自动映射、类型校验和批量插入 SQL 生成。

## 从源码运行

前提：安装 Rust `1.78` 或更高版本，以及当前平台的 C/C++ 构建工具链。

```powershell
cargo run -p orvyn
```

常用检查命令：

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## 配置与凭据

默认配置位置：

- Windows：`%APPDATA%\Orvyn`
- 其他平台：`$XDG_CONFIG_HOME/orvyn`；未设置时使用当前目录下的 `orvyn`

目录内包含连接配置、查询历史、收藏查询及加密密钥。可通过环境变量调整：

```powershell
$env:ORVYN_CONFIG_DIR = "D:\\path\\to\\orvyn-config"
$env:ORVYN_MASTER_KEY = "replace-with-a-strong-private-key"
```

未设置 `ORVYN_MASTER_KEY` 时，应用会在配置目录创建并使用 `master.key`。请妥善保护配置目录和该密钥。

## DuckDB 本地库

项目通过 `libduckdb-sys` 链接本地 DuckDB 库，不在构建时自动下载。默认位置为：

```text
target/duckdb-download/local/1.5.4
```

该目录需包含目标平台所需文件：

- Linux x64：`libduckdb.so`、`duckdb.h`
- Windows x64 MSVC：`duckdb.lib`、`duckdb.dll`、`duckdb.h`

更换 DuckDB 版本、目标平台或清理该目录后，请重新放入对应的本地库文件再构建。

## 发布与打包

不同平台应在各自的原生系统上构建；不要在 Linux 上做三系统交叉编译（GUI、系统 SDK、签名与 DuckDB 动态库会让方案不稳定）。发布产物需携带对应平台的 DuckDB 动态库，并构建相应安装包：

- **Windows x64**：安装 Rust MSVC 工具链、`x86_64-pc-windows-msvc` target、Windows DuckDB 本地库与 Inno Setup 6 后，在仓库根目录运行 `.\packaging\windows\build-installer.ps1`，产物为 `dist/windows/orvyn-0.1.0-windows-x86_64-setup.exe`。
- **Linux x86_64**：在尽量老的基础发行版（如 Ubuntu 22.04）上用 `--locked` 构建，可产出便携 tar.gz、`.AppImage` 或 `.deb`。
- **macOS**：Apple Silicon / Intel 分别产出 `.app` 与 `.dmg`，面向用户分发需签名与公证。

若直接分发可执行文件，务必同目录携带对应 DuckDB 动态库（`duckdb.dll` / `libduckdb.so` / `libduckdb.dylib`），通常建议直接分发安装包。

## Workspace 结构

```text
apps/desktop           桌面应用入口和 egui 界面
crates/app-core        连接、会话、元数据、查询、Redis/Search 服务和 DriverRegistry
crates/db-common       共享模型、trait、错误、能力矩阵和工具
crates/db-mysql        MySQL 协议族（品牌共享：MariaDB/TiDB/OceanBase/Doris/SelectDB/StarRocks/GoldenDB）
crates/db-postgres     PostgreSQL 协议族（品牌共享：TimescaleDB/Redshift/CockroachDB/OpenGauss/...）
crates/db-sqlite       SQLite 适配器
crates/db-duckdb       DuckDB 适配器
crates/db-redis        Redis 适配器
crates/db-sqlserver    SQL Server 适配器（tiberius）
crates/db-clickhouse   ClickHouse HTTP 适配器
crates/db-mongodb      MongoDB 文档适配器
crates/db-http         HTTP 搜索（Elasticsearch/Easysearch/Meilisearch/Manticore）与 HTTP-SQL（Cloudflare D1）
crates/db-tdengine     TDengine 受限 HTTP 连接探测
crates/import-export   CSV/JSON 导入导出能力
crates/secure-store    加密凭据存储
packaging/windows      Windows 安装包脚本和资源
```

## 许可证

本项目采用 [MIT OR Apache-2.0](Cargo.toml) 双许可证。