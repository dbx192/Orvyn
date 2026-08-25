# Orvyn 多数据库连接架构扩展（Plan.md 实施方案）
不要写兼容逻辑，这是个全新项目，还没上线，按照rust最佳实践实现。
## Context

`Plan.md` 要求把 Orvyn 从当前 12 个 `DataSourceKind` 扩展为“品牌类型 + 协议族驱动 + 能力矩阵”架构，覆盖 30+ 数据库，纯 Rust、无 ODBC/JDBC/sidecar。当前现状（已勘探确认）：

- `crates/db-common/src/connection.rs` `DataSourceKind` 12 变体，无 `#[serde]` tag，按变体名（`"MySql"`/`"TiDb"`/`"Redis"`）序列化；`as_str()` 仅用于展示。
- `crates/app-core/src/storage.rs` 用 `serde_json` 直读直写 `connections.json`/`orvyn-connections.json`，**无版本字段、无迁移**；未知 kind 字符串会以 serde 原始报错漏出。
- `SessionManager`/`MetadataService`/`QueryService` 各自持有 `BTreeMap<DataSourceKind, Arc<dyn …>>`，并用散落的 `match kind { TiDb|OceanBase|Doris => MySql, TimescaleDb => PostgreSql }` 做协议族路由（`session.rs`、`metadata.rs:relational_reader`、`query.rs:executor`）。
- `crates/db-mysql/src/lib.rs` 已用 `profile.kind == DataSourceKind::Doris` 做品牌分支（sql_mode、SHOW 元数据、无事务/FK/触发器）；`doris.rs` 子模块已存在。这是品牌共享模式的雏形。
- `apps/desktop/src/ui/connection_manager.rs` 类型选择器是硬编码 12 项的 `ComboBox`（L408-427），表单字段按 `is_file_based()`/`is_relational()`/具体 kind 分支。
- 参考项目 `D:\ProjectsRust\dbx` 的 `db/mysql_compatible.rs`、`db/doris.rs`、`db/starrocks.rs`、`db/oceanbase_mysql.rs`、`db/tidb.rs` 验证了“底层协议驱动共享 + `driver_profile` 品牌开关”模式；`sql_dialect/capabilities.rs` 提供能力矩阵范式。

目标产出：统一的 `DriverRegistry` + `ProtocolFamily` + `ConnectorCapabilities`，多品牌共享一个协议驱动实例，UI 按协议族分组并显示支持级别，旧配置可读且未知类型有清晰错误。按 Plan.md 6 阶段逐步交付。

## Architecture

### 1. 类型模型（db-common）

新增 `crates/db-common/src/protocol_family.rs`：
```rust
pub enum ProtocolFamily { MySql, PostgreSql, Sqlite, DuckDb, ClickHouse,
    SqlServer, Tdengine, Mongo, Redis, Elasticsearch, HttpSql, Restricted }
```
`DataSourceKind::protocol_family(self) -> ProtocolFamily`。

新增 `crates/db-common/src/capabilities.rs`：
```rust
pub enum SupportLevel { Full, Compatible, Restricted, Unsupported }
pub struct ConnectorCapabilities {
    pub support: SupportLevel,
    pub protocol_family: ProtocolFamily,
    pub connection_test: bool, pub sql_query: bool, pub metadata: bool,
    pub crud: bool, pub transactions: bool, pub http_json: bool,
    pub document_kv: bool, pub search: bool, pub ddl: bool,
    pub read_only_default: bool,
    pub notes: Option<&'static str>, // 如 "MySQL-compatible (Doris), no transactions"
}
impl DataSourceKind { pub fn capabilities(self) -> ConnectorCapabilities }
```

`DataSourceKind` 扩展到 35 变体（保留旧名不变以保序列化兼容）：
- MySQL 族：`MySql`(旧)、`MariaDb`、`TiDb`(旧)、`OceanBase`(旧)、`Doris`(旧)、`SelectDb`、`StarRocks`、`GoldenDb`
- PG 族：`PostgreSql`(旧)、`TimescaleDb`(旧)、`Redshift`、`CockroachDb`、`OpenGauss`、`GaussDb`、`KwDb`、`KingbaseEs`、`Vastbase`、`HighGo`、`UxDb`
- 独立关系/分析：`Sqlite`(旧)、`DuckDb`(旧)、`ClickHouse`(旧)、`SqlServer`(旧)、`TDengine`
- 文档/键值/搜索：`MongoDb`(旧)、`Redis`(旧)、`Elasticsearch`、`Easysearch`、`Meilisearch`、`ManticoreSearch`、`CloudflareD1`
- 受限：`Access`、`Oracle`、`Dm`、`XuguDb`

序列化兼容：实现手写 `Deserialize`，接受变体名（`"MySql"`）与 `as_str()`（`"mysql"`，大小写敏感匹配现有磁盘格式），未知字符串返回 `serde::de::Error::custom("unknown database kind '<x>'; supported: …")`。`Serialize` 保持 derive。同步扩展 `default_port`/`as_str`/`is_file_based`/`is_relational`，并新增 `is_document()`/`is_search()`/`is_http_sql()`/`is_restricted()`。

`ConnectionProfile.extra_params`（已存在 `BTreeMap<String,String>`）承载品牌专属参数：`tls`、`http_path`、`cf_account`/`cf_database_id`/`cf_token`、`mongo_auth_source`、`clickhouse_protocol`、`ob_mode`（mysql/oracle）、`es_api_key`/`es_api_id`。新增 `extra_params_schema(kind)` 返回字段元数据供 UI 动态渲染。

### 2. DriverRegistry（app-core）

新增 `crates/app-core/src/registry.rs`：
```rust
pub struct DriverRegistry {
    testers: BTreeMap<DataSourceKind, Arc<dyn ConnectionTester>>,
    relational_metadata: BTreeMap<DataSourceKind, Arc<dyn RelationalMetadataReader>>,
    relational_query: BTreeMap<DataSourceKind, Arc<dyn RelationalQueryExecutor>>,
    redis_metadata: BTreeMap<DataSourceKind, Arc<dyn RedisMetadataReader>>,
    document: BTreeMap<DataSourceKind, Arc<dyn DocumentConnector>>,    // Phase 4
    search: BTreeMap<DataSourceKind, Arc<dyn SearchConnector>>,        // Phase 5
    http_sql: BTreeMap<DataSourceKind, Arc<dyn HttpSqlConnector>>,     // Phase 5
}
impl DriverRegistry {
    pub fn default() -> Self;          // 注册全部内置驱动，品牌 kind 直接指向共享驱动 Arc
    pub fn tester(&self, kind) -> Result<&Arc<dyn ConnectionTester>>;
    pub fn relational_metadata(&self, kind) -> Result<&Arc<dyn RelationalMetadataReader>>;
    pub fn relational_query(&self, kind) -> Result<&Arc<dyn RelationalQueryExecutor>>;
    pub fn redis_metadata(&self, kind) -> Result<&Arc<dyn RedisMetadataReader>>;
    // document/search/http_sql getter 后续阶段补
}
```
注册策略：每个品牌 kind 显式注册到共享协议驱动实例（`MariaDb`/`TiDb`/`OceanBase`/`Doris`/`SelectDb`/`StarRocks`/`GoldenDb` → 同一个 `db_mysql` 实例；PG 族同理），**消除** `metadata.rs:relational_reader` 与 `query.rs:executor` 里的 `match kind → MySql/PostgreSql` 散落路由。受限类型（Access/Oracle/Dm/XuguDb）注册一个 `RestrictedConnectionTester`（返回 `ErrorKind::Unsupported` + 结构化原因）。

重构 `SessionManager`/`MetadataService`/`QueryService`：把各自内部的 `BTreeMap` 替换为 `Arc<DriverRegistry>`，`with_default_*` 改为 `with_registry(connections, Arc::new(DriverRegistry::default()))`。`apps/desktop` 的 `state.rs`/`app.rs` 组装点随之改为构造一次 `DriverRegistry` 传入三个服务（共享同一份，避免重复注册）。

### 3. 品牌共享（db-mysql / db-postgres）

`db-mysql/src/lib.rs` 引入 `MysqlBrand` 枚举（`MySql`/`MariaDb`/`TiDb`/`OceanBase{mode}`/`Doris`/`SelectDb`/`StarRocks`/`GoldenDb`），由 `profile.kind` 派生。现有 `profile.kind == DataSourceKind::Doris` 分支泛化为 `match brand`：
- 默认端口按 brand（`MariaDb`=3306、`StarRocks`=9030、`SelectDb`=9030、`GoldenDb`=2883 等）。
- 连接选项：Doris/SelectDb/StarRocks 关闭 sql_mode 表达式（现有 `doris::configure_connect_options` 推广到分析族）；OceanBase 注入 `SET ob_query_timeout`；GoldenDB/TDSQL 兼容标记。
- 元数据 SQL：分析族（Doris/SelectDb/StarRocks）沿用 `SHOW TABLES/COLUMNS/INDEX` 文本协议路径；OceanBase Oracle 模式标记受限。
- 能力门控：Doris/SelectDb/StarRocks 无事务/FK/触发器（已部分实现，补 SelectDb/StarRocks）。

`db-postgres/src/lib.rs` 同理引入 `PostgresBrand`（含 `Redshift` 只读分析、`CockroachDb` 事务重试提示、`OpenGauss`/`GaussDb`/`KingbaseEs`/`Vastbase`/`HighGo`/`UxDb`/`KwDb` 系统表差异），按 brand 覆盖默认端口与 schema/catalog 查询，对语义不同的 DDL/事务做 capability gating。`TimescaleDb` 已有路由，迁入品牌枚举。

### 4. 新 crate

- `crates/db-http`（Phase 4/5）：基于 `reqwest`（rustls-tls，加入 workspace deps）的共享 HTTP 客户端 + 超时/TLS/认证脱敏。实现 `Elasticsearch`/`Easysearch`/`Meilisearch`/`ManticoreSearch`/`CloudflareD1` 的 tester + `SearchConnector`/`HttpSqlConnector`；`TDengine` 受限 HTTP tester（仅连接探测 + Unsupported 完整能力）。
- `DocumentConnector`/`SearchConnector`/`HttpSqlConnector` trait 定义在 `db-common/src/traits.rs`（Phase 1 定义空 trait 与 `Unsupported` 默认实现，后续阶段填充）。

受限类型不新建 crate，由 `app-core` 的 `RestrictedConnectionTester` 统一处理。

### 5. UI（apps/desktop）

`ui/connection_manager.rs` 类型选择器：硬编码列表替换为按 `ProtocolFamily` 分组的下拉（egui 用 `menu_button(family_label, |ui| for kind in family.kinds() { selectable_value + 支持级别小标签 })`），每项显示 `brand (support)`。`apply_kind_defaults()` 改用 `kind.capabilities()` + `extra_params_schema(kind)` 驱动字段可见性（host/port/database、file_path、token/account、auth source、http_path、SSL、read-only）。连接详情页显示 `SupportLevel` 与能力位。查询编辑器仅对 `capabilities().sql_query` 为 true 的类型启用；文档/搜索类型走专用面板（后续阶段）。

`ui/state.rs`/`ui/app.rs`：服务组装改用共享 `DriverRegistry`。新增 `DataSourceKind::` 调用点（grep 已确认主要在 `connection_manager.rs`、`metadata.rs`、`query.rs`、`session.rs`）随路由重构一并收敛。

## Phased Implementation（逐阶段交付，每阶段完成即跑验证并汇报）

### Phase 1 — 基础架构
1. `db-common/src/connection.rs`：扩 `DataSourceKind` 到 35 变体；手写 `Deserialize`（兼容变体名+as_str，未知→清晰错误）；补 `default_port`/`as_str`/`is_*`；新增 `protocol_family()`。
2. 新增 `db-common/src/protocol_family.rs`、`db-common/src/capabilities.rs`（`SupportLevel`/`ConnectorCapabilities`/`kind.capabilities()`，35 种全覆盖）；`lib.rs` 导出。
3. `db-common/src/traits.rs`：加 `DocumentConnector`/`SearchConnector`/`HttpSqlConnector` trait（含默认 `Unsupported` 实现）+ `extra_params_schema`。
4. 新增 `app-core/src/registry.rs` `DriverRegistry`；重构 `session.rs`/`metadata.rs`/`query.rs` 去掉散落 `match` 路由，改持 `Arc<DriverRegistry>`；`with_default_*` → `with_registry`。
5. `apps/desktop/src/ui/state.rs`+`app.rs`：组装共享 `DriverRegistry` 注入三服务。
6. `ui/connection_manager.rs`：类型选择器分组 + 支持级别；表单字段能力驱动；详情页显示能力。
7. 单测：serde 往返、未知 kind 错误、能力矩阵、协议族映射、旧 `connections.json` 迁移、registry 路由（品牌→共享驱动）。
8. 验证：`cargo fmt --all -- --check`、`cargo check --workspace`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`。

### Phase 2 — MySQL 族
- `db-mysql/src/lib.rs`：`MysqlBrand` 枚举 + 品牌派生；端口/连接选项/元数据 SQL/能力门控按 brand 分支；新增 `MariaDb`/`SelectDb`/`StarRocks`/`GoldenDb`（参考 dbx `mysql_compatible.rs`/`starrocks.rs`/`doris.rs`）。OceanBase Oracle 模式标记受限。
- `DriverRegistry::default` 注册全部 MySQL 族 kind → 共享 `db_mysql` 实例。
- 单测：品牌路由、端口、分析族无事务/FK、SelectDb/StarRocks 元数据路径。
- 验证命令同上。

### Phase 3 — PostgreSQL 族
- `db-postgres/src/lib.rs`：`PostgresBrand`；`Redshift`/`CockroachDb`/`OpenGauss`/`GaussDb`/`KwDb`/`KingbaseEs`/`Vastbase`/`HighGo`/`UxDb`（参考 dbx `opentenbase.rs`/`cloudberry.rs`）；品牌端口、系统表/schema/catalog 差异、能力 gating（Redshift 只读、Cockroach 事务重试）。
- `DriverRegistry` 注册全部 PG 族。
- 单测 + 验证命令同上。

### Phase 4 — 独立驱动
- `db-sqlserver`：现有 TCP tester 升级为 `tiberius` 真实连接 + 元数据 + 查询（参考 dbx `db/sqlserver.rs`）。
- `db-clickhouse`：补齐 HTTP/native 查询 + 库/表/列元数据。
- `db-mongodb`：`DocumentConnector` 实现（ping、库/集合、文档查询、基础 CRUD）。
- `db-redis`：迁入 registry（现有能力保留）。
- 新增 `db-tdengine` 或并入 `db-http`：受限 HTTP tester。
- 验证命令同上（集成测试默认关闭，不依赖本机服务）。

### Phase 5 — HTTP/搜索
- 新增 `crates/db-http`（reqwest+rustls）。
- `Elasticsearch`/`Easysearch`：`SearchConnector`（索引/映射/文档/DSL 查询）；`Meilisearch`（索引/文档/任务/key）；`ManticoreSearch`（优先 MySQL 协议查询 + 搜索能力标记）；`CloudflareD1`（`HttpSqlConnector`，account/database/token，参考 dbx `db/cloudflare_d1`）。
- 统一超时/TLS/认证脱敏/分页/JSON 错误映射。
- `DriverRegistry` 注册文档/搜索/http_sql map。
- UI：搜索/文档类型专用 JSON/DSL 面板（简单实现；关系图谱等难实现 UI 不做）。
- 验证命令同上 + mock HTTP 单测。

### Phase 6 — 受限类型
- `Access`/`Oracle`/`Dm`/`XuguDb`：仅 `DataSourceKind` 变体 + `capabilities=Restricted/Unsupported` + `RestrictedConnectionTester`（返回结构化 Unsupported 含原因与当前支持级别）；UI 可创建/编辑/保存但测试返回受限提示。
- 验证命令同上。

## Verification

每阶段结束统一执行（Plan.md 第 162-166 行）：
```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
UI 冒烟：`cargo run -p orvyn-desktop` 后手动：新建/测试/保存各新类型；受限类型测试返回受限提示；查询编辑器对非 SQL 类型不启用；旧 `connections.json` 可加载。

集成测试默认不依赖本机数据库服务（按 Plan.md 第 154 行）。

## 复用清单（避免重复造轮子）
- `db-common`：`ConnectionProfile`/`ConnectionProfileBuilder`、`ConnectionTestReport`、`AppError`/`ErrorKind::Unsupported`、`unsupported_kind()`、`redact_connection_url`/`mask_secret`/`SecretString`、`ConnectionPoolConfig`、`MetadataNode`/`MetadataPath`。
- `db-mysql`：现有 `mysql_pool`/`mysql_url`/`MySqlMetadataReader`/`MySqlQueryExecutor`/`doris::configure_connect_options`/`doris::execute`/`doris::group_indexes`。
- `app-core`：`ConnectionManager`、`FileConnectionProfileStore`、`TaskManager`。
- `secure-store`：`SecureStore` trait + 现有加密实现（凭据流复用）。
