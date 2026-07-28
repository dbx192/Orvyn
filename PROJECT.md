# Orvyn 项目文档

## 1. 项目目标

开发一个使用 Rust 编写的桌面数据库客户端，支持连接和管理 Redis、MySQL、DuckDB、SQLite、PostgreSQL。目标是覆盖 Navicat 类工具约 80% 的高频功能，优先保证连接稳定、对象浏览、SQL 查询、表数据查看编辑、导入导出、Redis key 管理等核心工作流完整可用。

### 1.1 目标用户

- 后端工程师：日常查询、排障、查看表结构、修改少量数据。
- 数据分析人员：连接本地或远程数据库，编写 SQL，导出结果。
- 运维/DBA：管理连接、查看对象、执行脚本、基础数据维护。
- Rust 项目团队：希望有一个可控、可扩展、跨平台的数据库客户端。

### 1.2 支持平台

- Windows
- macOS
- Linux

### 1.3 技术形态

采用桌面应用架构：
- UI 层：egui/eframe 原生 Rust UI。
- 核心层：Rust workspace，数据库能力放在独立 crate 中。
- 异步运行时：Tokio。
- 本地配置：文件加密存储。

## 2. 架构设计

### 2.1 分层设计

```text
┌─────────────────────────────────────────────┐
│ UI 层                                       │
│ 连接列表 / 对象树 / 查询编辑器 / 结果表格   │
└─────────────────────┬───────────────────────┘
                      │ Command/Event API
┌─────────────────────▼───────────────────────┐
│ 应用服务层                                  │
│ 连接管理 / 查询会话 / 导入导出 / 历史记录   │
└─────────────────────┬───────────────────────┘
                      │ Trait API
┌─────────────────────▼───────────────────────┐
│ 数据源适配层                                │
│ MySQL / PostgreSQL / SQLite / DuckDB / Redis │
└─────────────────────┬───────────────────────┘
                      │ Driver
┌─────────────────────▼───────────────────────┐
│ 外部数据库                                  │
└─────────────────────────────────────────────┘
```

### 2.2 Workspace 结构

```text
apps/
  desktop/           # 桌面应用入口
crates/
  app-core/          # 应用业务核心
  db-common/         # 通用模型、trait、错误类型
  db-mysql/          # MySQL 适配器
  db-postgres/       # PostgreSQL 适配器
  db-sqlite/         # SQLite 适配器
  db-duckdb/         # DuckDB 适配器
  db-redis/          # Redis 适配器
  secure-store/      # 本地加密存储
  import-export/     # CSV/JSON/Excel 兼容导入导出
tests/
  integration/       # 多数据库集成测试（预留）
```

### 2.3 模块边界

- `db-common`：跨 crate 共享的模型、trait、错误和无状态工具方法。
- `app-core`：业务流程编排，连接管理、查询会话、任务管理。
- 数据库 crate：对应数据库驱动、方言和元数据实现。
- `secure-store`：凭据保存和读取，不参与连接逻辑。
- `import-export`：文件解析、格式转换、流式读写。
- UI 层：只调用应用服务，不直接调用数据库驱动。

## 3. 核心抽象

### 3.1 数据源类型

```text
DataSourceKind: MySQL, PostgreSQL, SQLite, DuckDB, Redis
```

### 3.2 连接配置

```text
ConnectionProfile:
  id, name, group, kind, host, port, username, password_ref
  database, file_path, ssl_config, ssh_tunnel_config, extra_params
  created_at, updated_at
```

### 3.3 关系型数据库接口

```text
RelationalConnector:
  connect(profile), test_connection(profile)
  list_databases(), list_schemas(), list_tables(), list_views()
  list_columns(), list_indexes(), get_create_sql()
  execute(sql, params), query(sql, params, page)
  begin_transaction(), commit(), rollback()
```

### 3.4 Redis 接口

```text
RedisConnector:
  connect(profile), test_connection(profile)
  list_databases(), scan_keys(db, pattern, cursor, limit)
  get_key_info(key), get_value(key), set_value(key, value, ttl)
  delete_key(key), update_ttl(key, ttl), execute_command(command)
```

### 3.5 统一结果模型

```text
QueryResult: statement, columns, rows, affected_rows, execution_time_ms
Column: name, database_type, logical_type, nullable, is_primary_key
CellValue: Null, Bool, Int, Float, Decimal, Text, Bytes, Date, DateTime, Json
```

## 4. 功能范围

### 4.1 第一优先级（必须实现）

| 功能模块 | 核心能力 |
|---------|---------|
| 连接管理 | 新建/编辑/测试/保存/删除连接，密码安全存储，连接状态显示 |
| 对象浏览 | 左侧树形导航，数据库/schema/表/视图/索引/函数/触发器，懒加载，搜索 |
| SQL 查询 | 多标签编辑器，SQL 高亮，执行/停止，多结果集，查询历史，收藏 |
| 结果展示 | 分页/虚拟滚动，列宽调整，复制（单元格/行/带表头），NULL/二进制/日期格式化，排序，CSV/JSON/Excel 导出 |
| 表数据编辑 | 分页浏览，新增/编辑/删除行，提交/回滚，主键保护，简单过滤排序 |
| 表结构查看 | 字段/索引/外键信息，建表 SQL |
| Redis 管理 | DB 浏览，SCAN key，类型/TLL/大小，String/Hash/List/Set/ZSet 编辑，TTL 设置，命令执行 |
| 导入导出 | CSV/JSON 导出，CSV 导入（字段映射、预览、错误报告） |
| 本地安全 | 密码加密保存，支持不保存密码，日志脱敏 |

### 4.2 第二优先级（重要但可稍后）

- ER 图基础展示
- 表结构编辑器
- 对象 CRUD（创建/删除/重命名）
- SQL 格式化与自动补全
- 查询计划查看
- 事务控制面板（BEGIN/COMMIT/ROLLBACK）
- SSH tunnel
- SSL/TLS 高级配置
- 数据库备份与恢复入口
- 批量执行 SQL 文件
- 主题切换
- 快捷键自定义

### 4.3 暂不实现

- 完整数据库迁移建模工具
- 复杂权限管理 UI
- 商业级数据/结构同步
- 高级 BI 报表
- 分布式数据库集群管理
- 图数据库、MongoDB 等非本次指定数据源

## 5. 开发里程碑

| 里程碑 | 目标 | 当前状态 |
|--------|------|---------|
| 0：项目初始化 | 建立 workspace，基础模型，CI | ✅ 已完成 |
| 1：连接管理 | 连接配置 CRUD，测试连接，密码存储 | ✅ 已完成 |
| 2：对象树和元数据 | 对象浏览，懒加载，元数据读取 | ✅ 已完成 |
| 3：SQL 查询编辑器 | 多标签编辑器，执行 SQL，结果展示 | ✅ 已完成 |
| 4：表数据编辑 | 分页浏览，新增/修改/删除，提交/回滚 | ✅ 已完成 |
| 5：Redis 完整管理 | 五大类型编辑，TTL，命令执行 | ✅ 已完成 |
| 6：导入导出 | CSV/JSON 导出，CSV 导入 | ✅ 已完成 |
| 7：增强体验 | SQL 格式化，自动补全，查询计划，ER 图，主题 | ⏳ 进行中 |

## 6. 编码规范

### 6.1 Rust 版本和格式化

- 使用 Rust 2021 edition。
- 必须通过 `cargo fmt`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。

### 6.2 命名规范

- crate：kebab-case（如 `db-common`）
- 模块/文件：snake_case
- 类型/trait/enum：PascalCase
- 函数/变量/字段：snake_case
- 常量：SCREAMING_SNAKE_CASE
- ID 字段：统一使用 `Id` 新类型（如 `ConnectionId`）

### 6.3 错误处理

- 禁止在库代码中使用 `unwrap()` 和 `expect()`（测试除外）。
- 禁止吞掉错误，必须返回、记录或转换为用户可理解的错误。
- 使用 `thiserror` 定义 crate 内部错误。
- 顶层应用可使用 `anyhow`，但不得暴露到公共 API。
- 敏感信息不得出现在错误信息中。

### 6.4 日志规范

- 使用 `tracing`。
- 日志字段使用结构化字段。
- 禁止记录密码、token、完整连接字符串。
- SQL 日志默认只记录摘要和耗时。

### 6.5 异步和并发

- 异步运行时使用 Tokio。
- UI 线程不得执行数据库 IO、文件 IO 或耗时计算。
- 长任务必须可取消。
- 同步驱动（SQLite、DuckDB）使用 blocking thread pool 封装。

### 6.6 数据库安全规范

- 用户输入值必须使用参数绑定，禁止字符串拼接。
- 标识符拼接必须经过方言模块的 `quote_identifier`。
- 默认限制查询结果行数。
- `UPDATE`/`DELETE` 无 `WHERE` 时必须触发高风险确认。
- 无主键表默认只读。
- Redis key 浏览必须使用 `SCAN`。

### 6.7 凭据处理

- 密码只保存到 `secure-store`，连接配置中只能保存 `password_ref`。
- 日志、错误、调试输出中必须脱敏。
- 支持不保存密码和只读连接配置。

### 6.8 数据模型规范

- 跨数据库的通用数据使用 `CellValue` 表达。
- 货币、高精度数值优先保留为字符串或 decimal 类型。
- 时间类型必须保留时区语义。
- 二进制字段默认只展示摘要。
- NULL 必须与空字符串区分。

### 6.9 UI 交互规范

- 所有高风险操作必须确认。
- 表格编辑必须有明确 dirty 状态。
- 查询执行中必须显示状态，并允许取消。
- 大结果集使用分页或虚拟滚动。

## 7. 性能策略

- 查询结果：大结果集分页或流式读取，表格使用虚拟滚动，单次默认最多显示 500 行。
- 对象树：懒加载节点，单节点刷新，元数据缓存。
- Redis：默认 SCAN，key 值按类型分页读取，大 value 只展示预览。
- 并发：每个连接维护独立会话，查询任务可取消，导入导出走后台任务。

## 8. 安全策略

- 默认禁止无确认执行危险批量操作。
- DELETE/UPDATE 无 WHERE 时弹出高风险确认。
- 表数据编辑必须展示待提交变更数量。
- Redis 删除 key 需要确认。
- 支持只读连接模式、查询超时设置、连接级最大返回行数。

## 9. 测试规范

- 方言 SQL 生成、错误脱敏、连接配置校验、数据类型转换必须有单元测试。
- 数据库适配器必须有集成测试。
- 导入导出必须覆盖空值、引号、换行、大文件、类型错误。
- 修复 bug 时必须补充回归测试。

## 10. 依赖规范

- 引入依赖前优先检查是否已有 workspace 依赖可复用。
- 避免重量级依赖进入底层公共 crate。
- 数据库驱动依赖只允许出现在对应数据库 crate。
- UI 依赖只允许出现在应用入口或 UI crate。
- 依赖版本统一放在 workspace dependencies。

## 11. Git 和提交规范

- 单次提交只解决一个明确问题。
- 提交前必须运行格式化、clippy 和测试。
- 不提交本地数据库文件、日志、临时导出文件、凭据文件。
- 提交信息建议使用：`feat:`、`fix:`、`docs:`、`refactor:`、`test:`、`chore:`。

## 12. 完成标准

项目达到以下标准可认为完成“Navicat 80% 主要功能”的阶段目标：

- 可以稳定保存并连接 Redis、MySQL、DuckDB、SQLite、PostgreSQL。
- 可以浏览主要数据库对象。
- 可以打开多标签 SQL 编辑器并执行查询。
- 可以展示和导出结果集。
- 可以分页查看并安全编辑关系型数据库表数据。
- 可以查看和编辑 Redis 常用 key 类型。
- 可以安全保存密码。
- 常见错误有明确提示。
- 大表和大量 Redis key 不会明显卡死 UI。
- 核心功能有单元测试和集成测试覆盖。

## 13. 常用命令

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## 14. DuckDB 本地库

- 项目使用的统一本地库目录：`target/duckdb-download/local/1.5.4`
- 项目级 Cargo 配置：`.cargo/config.toml`
- 该目录需要包含当前构建目标对应的 DuckDB 库文件（`libduckdb.so`/`duckdb.lib`/`duckdb.dll` + `duckdb.h`）
