# Orvyn

[English](README.en.md)

Orvyn 是一个使用 Rust 构建的跨平台桌面数据库客户端，面向日常查询、数据浏览、轻量维护和 Redis key 管理。项目采用 `egui/eframe` 桌面界面与分层 Rust workspace 架构。

## 支持的数据源

- MySQL
- PostgreSQL
- SQLite
- DuckDB
- Redis

## 当前功能

### 连接与安全

- 新建、编辑、测试、分组和删除连接。
- 导入或导出全部或选中的连接；导入时可选择覆盖冲突项或自动重命名。
- 可选地在本地加密保存密码，使用 ChaCha20-Poly1305；默认不保存密码。
- 每个连接可设为只读。只读连接会拒绝写语句和启动事务。
- 支持 MySQL、PostgreSQL、SQLite、DuckDB 和 Redis 的连接测试。

### 关系型数据库

- 浏览数据库、schema、表、视图及其元数据。
- 查看列、索引、外键、触发器、函数和建表 SQL（实际可见对象取决于数据库适配器）。
- 单 SQL 工作区：执行全部语句、当前语句或选中 SQL；可取消执行。
- 查询历史、收藏查询、结果分页和列排序。
- 表数据浏览、筛选、排序及新增、编辑、删除行；无主键表默认只读。
- 表结构查看与编辑，包括列、索引和外键等操作；不同数据库的 DDL 能力存在差异。
- 导出表或数据库的结构 SQL，或结构及数据 SQL。

### Redis

- 浏览逻辑数据库，使用 `SCAN` 分页查找 key。
- 查看 key 类型、TTL、大小和值。
- 管理 String、Hash、List、Set、Sorted Set 等数据类型，并可执行 Redis 命令。

### 导入导出

- 将查询结果导出为 CSV、JSON 或 Excel 兼容的制表符分隔文本。
- CSV 文件使用 UTF-8 BOM，方便 Windows 上的 Excel 正确识别中文。
- 支持 CSV 预览、字段自动映射、类型校验和批量插入 SQL 生成。

## 从源码运行

前提：安装 Rust `1.78` 或更高版本及当前平台的 C/C++ 构建工具链。

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

该目录需要包含目标平台所需文件：

- Linux x64：`libduckdb.so`、`duckdb.h`
- Windows x64 MSVC：`duckdb.lib`、`duckdb.dll`、`duckdb.h`

更换 DuckDB 版本、目标平台或清理该目录后，请重新放入对应的本地库文件再构建。

## Windows x64 安装包

安装 Rust MSVC 工具链、`x86_64-pc-windows-msvc` target、Windows DuckDB 本地库与 Inno Setup 6 后，在仓库根目录运行：

```powershell
.\packaging\windows\build-installer.ps1
```

脚本构建 `orvyn`，然后生成安装包：

```text
dist/windows/orvyn-0.1.0-windows-x86_64-setup.exe
```

若直接分发可执行文件，请同时分发同目录下的 `duckdb.dll`；通常建议分发安装包。

## Workspace 结构

```text
apps/desktop           桌面应用入口和 egui 界面
crates/app-core        连接、会话、元数据、查询、Redis 和任务服务
crates/db-common       共享模型、trait、错误和工具
crates/db-mysql        MySQL 适配器
crates/db-postgres     PostgreSQL 适配器
crates/db-sqlite       SQLite 适配器
crates/db-duckdb       DuckDB 适配器
crates/db-redis        Redis 适配器
crates/import-export   CSV/JSON 导入导出能力
crates/secure-store    加密凭据存储
packaging/windows      Windows 安装包脚本和资源
```

## 许可证

本项目采用 [MIT OR Apache-2.0](Cargo.toml) 双许可证。
