# Rust 多数据库客户端开发计划

## 1. 项目目标

开发一个使用 Rust 编写的桌面数据库客户端，支持连接和管理 Redis、MySQL、DuckDB、SQLite、PostgreSQL。目标是覆盖 Navicat 类工具约 80% 的高频功能，优先保证连接稳定、对象浏览、SQL 查询、表数据查看编辑、导入导出、Redis key 管理等核心工作流完整可用。

本计划随实现同步更新。当前桌面前端采用 egui/eframe 原生 Rust UI。

## 2. 产品定位

### 2.1 目标用户

- 后端工程师：日常查询、排障、查看表结构、修改少量数据。
- 数据分析人员：连接本地或远程数据库，编写 SQL，导出结果。
- 运维/DBA：管理连接、查看对象、执行脚本、基础数据维护。
- Rust 项目团队：希望有一个可控、可扩展、跨平台的数据库客户端。

### 2.2 支持平台

- Windows
- macOS
- Linux

### 2.3 推荐技术形态

采用桌面应用架构：

- UI 层：egui/eframe 原生 Rust UI。
- 核心层：Rust workspace，数据库能力放在独立 crate 中。
- 异步运行时：Tokio。
- 本地配置：SQLite 或文件加密存储。

egui 层只负责交互和渲染，连接、查询、安全和数据转换继续由 Rust 应用服务和数据库适配 crate 承担。

当前前端模块划分：

```text
apps/desktop/src/
  main.rs                 # 桌面启动、服务注入
  ui/
    app.rs                # egui 应用壳、顶部导航、视图切换
    result_grid.rs        # 查询结果集展示
    table_editor.rs       # 表数据查看和编辑
    format.rs             # CellValue 展示、复制、排序格式化
    samples.rs            # UI 测试/演示数据辅助
```

UI 模块只持有界面状态和用户操作状态，真实连接、查询、元数据和导出能力必须通过 `app-core`、`db-common` 和各数据库适配 crate 暴露的接口进入。

### 2.4 本地构建约束

- DuckDB 运行库通过 `.cargo/config.toml` 显式指向统一本地目录 `target/duckdb-download/local/1.5.4`。
- 项目强制设置 `DUCKDB_DOWNLOAD_LIB=0`，避免 `libduckdb-sys` 从 GitHub Releases 下载运行库。
- 当前目录同时放置 Linux x64 和 Windows x64 MSVC 的 DuckDB 本地库文件；切换其他目标平台时，需要把对应平台库文件同步放入该目录。
- 如果升级 DuckDB crate 版本、切换目标平台、或清理掉 `target/duckdb-download`，需要同步更新 `.cargo/config.toml` 和 README 中的构建说明。

## 3. 功能范围

### 3.1 第一优先级：必须实现

这些能力构成 Navicat 类客户端的主流程。

#### 连接管理

- 新建连接：Redis、MySQL、DuckDB、SQLite、PostgreSQL。
- 编辑连接：主机、端口、用户名、密码、数据库、连接参数。
- 测试连接。
- 保存连接配置。
- 删除连接。
- 连接分组。
- 最近使用连接。
- 连接状态显示。
- 断线重连。

#### 数据库对象浏览

- 左侧树形导航。
- 展示数据库、schema、表、视图、索引、函数、触发器等对象。
- 不同数据库使用统一对象模型。
- 支持刷新单个节点。
- 支持搜索对象名称。

#### SQL 查询编辑器

- 多标签查询窗口。
- SQL 高亮。
- 执行当前语句。
- 执行选中语句。
- 停止执行。
- 查询耗时显示。
- 查询影响行数显示。
- 多结果集展示。
- 查询历史。
- 常用 SQL 收藏。

#### 结果集展示

- egui 表格展示查询结果。
- 分页加载和虚拟滚动结合，避免一次渲染全部行。
- 列宽可调整。
- 支持复制单元格、复制行、复制单行带表头、复制当前页带表头、复制完整结果带表头。
- NULL 值必须与空字符串明显区分。
- 二进制字段默认显示长度和 base64 摘要。
- 日期时间字段按统一格式展示，复制和导出保留更完整精度。
- 支持按列升序、降序、取消排序。
- 结果集导出 CSV、JSON、Excel 兼容格式；当前 egui 桌面端先复制到剪贴板，后续接入文件保存对话框。

#### 表数据查看和编辑

- 打开表数据。
- 分页浏览。
- 新增行。
- 编辑单元格。
- 删除行。
- 提交变更。
- 回滚未提交变更。
- 根据主键生成 UPDATE/DELETE。
- 无主键表默认只读，可显式进入风险模式后按整行条件生成 UPDATE/DELETE。
- 简单过滤条件。
- 简单排序。
- 简单过滤条件。
- 简单排序。

#### 表结构查看

- 查看字段名、类型、是否为空、默认值、主键、注释。
- 查看索引。
- 查看外键。
- 查看建表 SQL。
- 刷新结构。

#### Redis 管理

- 展示 Redis 数据库编号。
- 浏览 key。
- 按 pattern 搜索 key。
- 查看 key 类型、TTL、大小摘要。
- 支持 String、Hash、List、Set、Sorted Set 的查看和基础编辑。
- 设置 TTL。
- 删除 key。
- 新增 key。
- 执行 Redis 命令。

#### 导入导出

- 查询结果导出 CSV。
- 查询结果导出 JSON。
- 表数据导出 CSV。
- CSV 导入到表。
- 导入前字段映射。
- 导入预览。
- 导入错误报告。

#### 本地安全

- 连接密码加密保存。
- 支持不保存密码。
- 支持启动后首次访问连接时输入密码。
- 配置文件权限检查。
- 日志中禁止输出密码和 token。

### 3.2 第二优先级：重要但可稍后实现

- ER 图基础展示。
- 表结构编辑器。
- 创建/删除/重命名数据库对象。
- SQL 格式化。
- SQL 自动补全。
- SQL 片段模板。
- 查询计划查看。
- 事务控制面板：BEGIN、COMMIT、ROLLBACK。
- SSH tunnel。
- SSL/TLS 高级配置。
- 数据库备份与恢复入口。
- 批量执行 SQL 文件。
- 任务调度。
- 主题切换。
- 快捷键自定义。

### 3.3 暂不实现

这些功能成本高或专业场景较窄，暂不纳入 80% 主体范围。

- 完整数据库迁移建模工具。
- 复杂权限管理 UI。
- 商业级数据同步。
- 商业级结构同步。
- 高级 BI 报表。
- 分布式数据库集群管理。
- 图数据库、MongoDB 等非本次指定数据源。

## 4. 总体架构

### 4.1 分层设计

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

### 4.2 Rust workspace 建议

```text
db-client/
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
  apps/
    desktop/           # Tauri 或原生 UI 应用
  tests/
    integration/       # 多数据库集成测试
```

### 4.3 核心抽象

#### 数据源类型

```text
DataSourceKind:
  - MySQL
  - PostgreSQL
  - SQLite
  - DuckDB
  - Redis
```

#### 连接配置

```text
ConnectionProfile:
  id
  name
  group
  kind
  host
  port
  username
  password_ref
  database
  file_path
  ssl_config
  ssh_tunnel_config
  extra_params
  created_at
  updated_at
```

#### 关系型数据库统一接口

```text
RelationalConnector:
  connect(profile)
  test_connection(profile)
  list_databases()
  list_schemas(database)
  list_tables(schema)
  list_views(schema)
  list_columns(table)
  list_indexes(table)
  get_create_sql(object)
  execute(sql, params, options)
  query(sql, params, page)
  begin_transaction()
  commit()
  rollback()
```

#### Redis 统一接口

```text
RedisConnector:
  connect(profile)
  test_connection(profile)
  list_databases()
  scan_keys(db, pattern, cursor, limit)
  get_key_info(key)
  get_value(key)
  set_value(key, value, ttl)
  delete_key(key)
  update_ttl(key, ttl)
  execute_command(command)
```

#### 统一结果模型

```text
QueryResult:
  statement
  columns
  rows
  affected_rows
  execution_time_ms
  notices
  warnings
  error

Column:
  name
  database_type
  logical_type
  nullable
  is_primary_key

CellValue:
  Null
  Bool
  Int
  Float
  Decimal
  Text
  Bytes
  Date
  DateTime
  Json
```

## 5. 数据库驱动选择

### 5.1 MySQL

候选 crate：

- `mysql_async`
- `sqlx` MySQL feature

建议：

- 如果强调运行时动态 SQL 和手动元数据，优先 `mysql_async`。
- 如果希望复用连接池和统一异步接口，可考虑 `sqlx`，但 SQLx 编译期检查对动态客户端价值有限。

### 5.2 PostgreSQL

候选 crate：

- `tokio-postgres`
- `sqlx` PostgreSQL feature

建议：

- 优先 `tokio-postgres`，动态查询和协议控制更直接。

### 5.3 SQLite

候选 crate：

- `rusqlite`
- `sqlx` SQLite feature

建议：

- 优先 `rusqlite`。SQLite 常作为本地文件访问，同步接口可通过专用线程池封装。

### 5.4 DuckDB

候选 crate：

- `duckdb`

建议：

- 使用官方生态的 `duckdb` crate。
- DuckDB 偏分析型、本地文件型，适配方式接近 SQLite。

### 5.5 Redis

候选 crate：

- `redis`

建议：

- 使用 `redis` crate 的 async tokio 支持。
- 对大 key 空间必须使用 SCAN，不使用 KEYS 作为默认浏览方式。

## 6. 主要界面布局

### 6.1 整体布局

```text
┌──────────────────────────────────────────────────────────────┐
│ 顶部工具栏：新建连接 / 查询 / 导入 / 导出 / 设置              │
├───────────────┬──────────────────────────────────────────────┤
│ 左侧连接树    │ 主工作区                                      │
│               │ ┌──────────────────────────────────────────┐ │
│ 连接分组      │ │ 标签页：表数据 / 查询编辑器 / Redis Key   │ │
│ 数据库对象树  │ └──────────────────────────────────────────┘ │
│ 搜索          │                                              │
├───────────────┴──────────────────────────────────────────────┤
│ 底部状态栏：连接状态 / 查询耗时 / 当前任务 / 错误提示         │
└──────────────────────────────────────────────────────────────┘
```

### 6.2 连接树

交互：

1. 点击连接，若未连接则建立连接。
2. 展开连接后加载数据库或 schema。
3. 展开表节点后加载列、索引、外键。
4. 右键连接显示操作菜单。
5. 右键表显示打开数据、查看结构、复制名称、导出等操作。

右键菜单：

- 连接
- 断开
- 编辑连接
- 删除连接
- 新建查询
- 刷新
- 打开表
- 查看结构
- 复制对象名
- 导出数据

### 6.3 主工作区标签页

标签页类型：

- 查询编辑器
- 表数据
- 表结构
- Redis Key 详情
- Redis 命令行
- 导入向导
- 导出任务

标签页行为：

- 每个标签页绑定一个连接会话。
- 查询标签页可切换数据库/schema。
- 关闭有未保存变更的标签页时必须确认。
- 支持固定标签页。

## 7. 主要交互流程

### 7.1 新建连接流程

```text
用户点击“新建连接”
  -> 选择数据源类型
  -> 填写连接信息
  -> 点击“测试连接”
  -> 测试成功后启用“保存”
  -> 保存到本地配置
  -> 左侧连接树出现新连接
```

关键校验：

- MySQL/PostgreSQL 必填 host、port、username。
- SQLite/DuckDB 必填 file_path。
- Redis 必填 host、port。
- 密码可选保存。
- 连接名称不可为空。

### 7.2 打开关系型数据库表

```text
用户展开连接
  -> 展开 database/schema
  -> 双击表
  -> 打开表数据标签页
  -> 自动执行分页查询
  -> 展示前 100 或 500 行
```

默认策略：

- 使用分页查询，避免一次加载全表。
- 默认限制 500 行。
- 根据数据库方言生成分页 SQL。
- 若表有主键，允许编辑。
- 若表无主键，默认只读。

### 7.3 编辑表数据

```text
用户修改单元格
  -> 单元格进入 dirty 状态
  -> 工具栏显示提交/回滚
  -> 用户点击提交
  -> 根据主键生成 UPDATE
  -> 执行事务
  -> 成功后清除 dirty 状态
```

冲突处理：

- 提交前可选检查原值是否变化。
- 更新影响行数为 0 时提示数据可能已被修改。
- 更新影响行数大于 1 时视为危险，回滚并提示。

### 7.4 执行 SQL

```text
用户打开查询标签页
  -> 输入 SQL
  -> 点击执行或使用快捷键
  -> 后端拆分语句
  -> 执行当前语句或选中语句
  -> 返回一个或多个 QueryResult
  -> UI 展示结果表格、影响行数、耗时和错误
```

注意点：

- 默认只执行当前语句，避免误执行整页脚本。
- 多语句执行需要明显入口。
- 长查询必须支持取消。
- 错误定位到具体语句。

### 7.5 Redis key 浏览

```text
用户展开 Redis 连接
  -> 选择 DB 编号
  -> 默认使用 SCAN 加载 key
  -> 输入 pattern 搜索
  -> 点击 key
  -> 展示类型、TTL、值预览
```

Redis 类型交互：

- String：文本/二进制预览，允许编辑。
- Hash：表格展示 field/value，允许增删改。
- List：按 index 分页展示，允许 push/pop/set。
- Set：分页展示 member，允许添加/删除。
- Sorted Set：展示 member/score，允许编辑 score。

### 7.6 导出流程

```text
用户在结果集或表上点击导出
  -> 选择格式 CSV/JSON
  -> 选择目标路径
  -> 配置编码、分隔符、是否包含表头
  -> 开始导出
  -> 后台流式读取
  -> 展示进度
  -> 完成后显示文件路径
```

### 7.7 导入流程

```text
用户选择目标表
  -> 点击导入 CSV
  -> 选择文件
  -> 自动解析表头和样例
  -> 字段映射
  -> 预校验类型
  -> 开始导入
  -> 批量写入
  -> 展示成功行数和失败行数
```

导入策略：

- 每批 500 到 5000 行。
- 使用事务。
- 失败可选择停止或跳过。
- 错误行输出报告。

## 8. 数据库方言差异处理

### 8.1 方言模块

为每种关系型数据库实现方言能力：

```text
Dialect:
  quote_identifier(name)
  build_limit_offset(sql, limit, offset)
  list_tables_sql(schema)
  list_columns_sql(table)
  list_indexes_sql(table)
  get_create_table_sql(table)
  parameter_placeholder(index)
```

### 8.2 标识符引用

- MySQL：反引号，例如 `table_name`
- PostgreSQL：双引号，例如 "table_name"
- SQLite：双引号为主
- DuckDB：双引号为主

### 8.3 分页

- MySQL：`LIMIT offset, limit` 或 `LIMIT limit OFFSET offset`
- PostgreSQL：`LIMIT limit OFFSET offset`
- SQLite：`LIMIT limit OFFSET offset`
- DuckDB：`LIMIT limit OFFSET offset`

### 8.4 元数据查询

优先使用各数据库官方系统表：

- MySQL：`information_schema`
- PostgreSQL：`information_schema`、`pg_catalog`
- SQLite：`sqlite_master`、`PRAGMA`
- DuckDB：`information_schema`、`duckdb_*` 函数

## 9. 本地数据存储

### 9.1 保存内容

- 连接配置。
- 连接分组。
- 查询历史。
- 查询收藏。
- UI 布局偏好。
- 最近打开的对象。
- 导入导出任务历史。

### 9.2 敏感信息

密码保存策略：

- Windows：优先使用系统凭据管理器。
- macOS：优先使用 Keychain。
- Linux：优先使用 Secret Service。
- 无系统安全存储可用时，使用主密码派生密钥加密。

禁止：

- 明文保存密码。
- 在日志中打印完整连接字符串。
- 在错误信息中回显密码。

## 10. 错误处理和日志

### 10.1 错误分类

- 连接错误。
- 认证错误。
- 网络超时。
- SQL 语法错误。
- 权限错误。
- 类型转换错误。
- 文件读写错误。
- 导入导出错误。
- 内部错误。

### 10.2 UI 展示

- 短错误在状态栏或 toast 显示。
- 详细错误在可展开面板中展示。
- SQL 错误需要展示数据库返回的 message、code、position。
- 导入错误需要关联行号和字段。

### 10.3 日志级别

- error：功能失败。
- warn：可恢复问题。
- info：连接、导入导出、任务状态。
- debug：开发调试信息。
- trace：默认关闭。

## 11. 性能策略

### 11.1 查询结果

- 大结果集分页或流式读取。
- 表格使用虚拟滚动。
- 单次默认最多显示 500 行，可手动调整。
- 导出不依赖 UI 表格数据，直接从数据库流式读取。

### 11.2 对象树

- 懒加载节点。
- 单节点刷新。
- 元数据缓存。
- 连接断开后清理缓存。

### 11.3 Redis

- 默认 SCAN。
- key 值按类型分页读取。
- 大 value 只展示预览，用户确认后加载完整内容。

### 11.4 并发

- 每个连接维护独立会话。
- 查询任务可取消。
- 导入导出走后台任务。
- UI 不直接阻塞等待数据库 IO。

## 12. 安全策略

- 默认禁止无确认执行危险批量操作，例如清空表、删除数据库。
- DELETE/UPDATE 无 WHERE 时弹出高风险确认。
- 表数据编辑必须展示待提交变更数量。
- Redis 删除 key 需要确认。
- 支持只读连接模式。
- 支持查询超时设置。
- 支持连接级最大返回行数。

## 13. 测试计划

### 13.1 单元测试

- 方言 SQL 生成。
- 连接配置校验。
- 结果集类型转换。
- CSV 解析和导出。
- SQL 语句拆分。
- Redis key pattern 处理。

### 13.2 集成测试

使用 Docker 或本地服务：

- MySQL 测试容器。
- PostgreSQL 测试容器。
- Redis 测试容器。
- SQLite 临时文件。
- DuckDB 临时文件。

覆盖：

- 连接测试。
- 元数据读取。
- 查询执行。
- 表数据编辑。
- 导入导出。
- Redis 各类型读写。

### 13.3 UI 测试

- 新建连接流程。
- 打开表流程。
- 执行 SQL 流程。
- 编辑表数据提交流程。
- 导入导出向导。
- 错误提示展示。

## 14. 开发里程碑

### 里程碑 0：项目初始化

目标：

- 创建 Rust workspace。
- 确定 UI 技术。
- 建立基础错误类型、日志、配置模型。
- 建立 CI。

产出：

- 可启动空应用。
- 本地配置读写。
- 基础窗口布局。

### 里程碑 1：连接管理

目标：

- 实现连接配置 CRUD。
- 实现 MySQL、PostgreSQL、SQLite、DuckDB、Redis 测试连接。
- 实现密码安全存储。

产出：

- 新建连接窗口。
- 左侧连接列表。
- 连接测试结果。

当前实现状态：

- 已实现连接管理后端服务 `ConnectionManager`。
- 已实现连接配置文件持久化 `FileConnectionProfileStore`。
- 已实现本地加密凭据存储 `EncryptedFileStore`。
- 已实现 MySQL、PostgreSQL、SQLite、DuckDB、Redis 的连接测试适配器。
- 已实现密码保存策略，默认不保存密码。
- UI 的新建连接窗口和左侧连接列表尚未实现。

### 里程碑 2：对象树和元数据

目标：

- 实现关系型数据库对象浏览。
- 实现 Redis DB/key 初步浏览。
- 实现懒加载和刷新。

产出：

- 可展开数据库对象树。
- 可查看表字段和基础结构。

当前实现状态：

- 已实现后端对象树服务 `MetadataService`。
- 已实现统一对象树节点模型 `MetadataNode`、`MetadataPath`。
- 已实现 MySQL、PostgreSQL、SQLite、DuckDB 的数据库、schema、表、视图、字段、索引元数据读取。
- 已实现 Redis DB 列表和 SCAN key 浏览。
- 已实现 Redis key 类型和 TTL 基础信息读取。
- 对象树 UI 尚未实现。
- 当前元数据请求使用短连接，连接池和查询会话留到后续阶段。

### 里程碑 3：SQL 查询编辑器

目标：

- 多标签查询编辑器。
- 执行 SQL。
- 展示结果集。
- 查询历史。

产出：

- 可用于日常查询的核心版本。

当前实现状态：

- 已实现查询执行服务 `QueryService` 和多数据库查询执行适配器。
- 已实现 egui 结果集展示，包含分页、虚拟滚动、列宽调整、复制、排序、NULL/二进制/日期时间格式化展示，以及 CSV/JSON/Excel 兼容格式导出。
- 查询编辑器、多结果集标签页和真实查询 UI 接入仍需继续完成。

### 里程碑 4：表数据编辑

目标：

- 表数据分页浏览。
- 新增、修改、删除行。
- 提交和回滚。
- 无主键表只读保护。

产出：

- 可替代大部分手工数据维护场景。

当前实现状态：

- 已建立 egui 前端模块架构，`main.rs` 只负责启动和服务注入，具体视图拆分到 `apps/desktop/src/ui/`。
- 已实现表数据查看和编辑 UI `TableEditor`，支持分页浏览、新增行、编辑单元格、删除行、回滚未提交变更、简单过滤和简单排序。
- 已实现基于主键生成 UPDATE/DELETE SQL；无主键表默认只读，显式开启风险模式后按整行条件生成 UPDATE/DELETE。
- 当前提交动作先生成 SQL 预览并清理 UI dirty 状态，真实数据库事务执行和刷新表数据需要接入 `QueryService` 与表元数据加载流程。

### 里程碑 5：Redis 完整基础管理

目标：

- 支持 Redis 五大常用类型查看和编辑。
- 支持 TTL 设置。
- 支持 Redis 命令执行。

产出：

- Redis 日常排障和维护可用。

### 里程碑 6：导入导出

目标：

- CSV/JSON 导出。
- CSV 导入。
- 字段映射和错误报告。

产出：

- 可完成常见数据交换任务。

### 里程碑 7：增强体验

目标：

- SQL 格式化。
- SQL 自动补全基础版。
- 查询计划。
- ER 图基础版。
- 主题和快捷键。

产出：

- 更接近成熟 Navicat 类工具体验。

## 15. 关键技术风险

### 15.1 多数据库统一抽象过度复杂

风险：

- 不同数据库元数据和类型差异大，过早抽象会导致接口臃肿。

对策：

- 只抽象高频能力。
- 方言差异放到独立模块。
- 不强行让 Redis 复用关系型数据库接口。

### 15.2 大数据量导致 UI 卡顿

风险：

- 查询结果、Redis key、导出任务可能阻塞 UI。

对策：

- 后台任务。
- 分页和流式读取。
- 虚拟滚动。
- 默认限制返回行数。

### 15.3 数据编辑误操作

风险：

- 数据库客户端具有直接修改生产数据的能力。

对策：

- 危险 SQL 检测。
- 只读模式。
- 提交前变更预览。
- 无主键表编辑限制。
- 默认不开启自动提交批量修改。

### 15.4 密码安全

风险：

- 跨平台凭据存储行为不一致。

对策：

- 优先系统凭据管理器。
- 设计 `secure-store` 抽象。
- 明确降级策略。

### 15.5 驱动异步模型不一致

风险：

- SQLite 和 DuckDB 常用同步接口，MySQL/PostgreSQL/Redis 使用异步接口。

对策：

- UI 和应用服务统一使用异步 command。
- 同步数据库操作放入 blocking thread pool。

## 16. 首版最小可用范围

首版 MVP 建议严格控制范围：

- 支持 5 类连接的新建、测试、保存。
- 支持关系型数据库对象树。
- 支持 SQL 查询和结果表格。
- 支持表数据分页查看。
- 支持 Redis key 搜索和 String/Hash 基础查看。
- 支持 CSV 导出。
- 支持连接密码安全保存。

首版暂缓：

- 表结构编辑。
- ER 图。
- 自动补全。
- SSH tunnel。
- CSV 导入。
- Redis List/Set/ZSet 编辑。

## 17. 后续实现顺序建议

1. 建立 workspace 和通用模型。
2. 实现配置和安全存储。
3. 实现各数据库 test connection。
4. 实现对象树元数据读取。
5. 实现查询执行和结果集模型。
6. 实现 UI 三栏布局。
7. 实现表数据分页。
8. 实现表数据编辑。
9. 实现 Redis 管理。
10. 实现导入导出。
11. 做性能优化和错误体验。
12. 做打包发布。

## 18. 主要快捷键

- `Ctrl+N`：新建查询。
- `Ctrl+Enter`：执行当前 SQL。
- `Ctrl+Shift+Enter`：执行选中 SQL。
- `Ctrl+S`：保存当前查询或提交编辑。
- `Ctrl+R`：刷新当前对象。
- `Ctrl+F`：搜索当前结果或对象。
- `Ctrl+L`：聚焦对象搜索。
- `Esc`：关闭弹窗或取消当前编辑。

macOS 对应使用 `Cmd`。

## 19. 交互状态设计

### 19.1 连接状态

- 未连接：灰色。
- 连接中：加载状态。
- 已连接：正常图标。
- 连接失败：红色错误标记。
- 断线：黄色警告标记。

### 19.2 查询状态

- 空闲。
- 执行中。
- 已完成。
- 已取消。
- 执行失败。

### 19.3 数据编辑状态

- 未修改。
- 已修改未提交。
- 提交中。
- 提交成功。
- 提交失败。
- 冲突。

## 20. 完成标准

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
