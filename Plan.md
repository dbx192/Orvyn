参考 D:\ProjectsRust\dbx 的相关逻辑，在我这个项目目前的基础上进行修改，这个项目有些功能已经完成了。注意代码可维护性，极致体验和性能，只使用rust，对于难实现但是不太重要的的UI可以不实现，例如关系图谱。

  # 纯 Rust 多数据库连接架构与分阶段实现方案

  ## Summary

  将 Orvyn 的数据库支持扩展为“品牌类型 + 协议族驱动 + 能力矩阵”架构，保留每个品牌独立的连接类型、默认端口和 UI 展示，同时尽量复用纯 Rust 协议实现：

  - MySQL 协议族：MySQL、MariaDB、TiDB、OceanBase、Doris、SelectDB、StarRocks、GoldenDB。
  - PostgreSQL 协议族：PostgreSQL、Redshift、CockroachDB、openGauss、GaussDB、KWDB、KingbaseES、Vastbase、HighGo、UXDB。
  - 独立 SQL/分析驱动：SQLite、DuckDB、ClickHouse、SQL Server、TDengine。
  - 独立文档/键值/搜索驱动：MongoDB、Redis、Elasticsearch、Easysearch、Meilisearch、Manticore Search、Cloudflare D1。
  - Access、Oracle、DM、虚谷 XuguDB 等暂以独立类型和显式受限能力纳入，不引入 ODBC、JDBC、厂商 native bridge；待有可靠纯 Rust 驱动或公开协议后再实现完整能力。

  “完整能力”按协议族分阶段交付，每个阶段都必须包含连接测试、查询、元数据浏览和对应能力测试；不把 TCP 端口探测伪装成完整驱动支持。

  ## Key Changes

  ### 1. 重构公共连接模型

  扩展 db-common：

  - 扩充 DataSourceKind，覆盖用户列出的全部类型，并保留现有序列化兼容。
  - 为每种类型增加：
      - 默认端口；
      - 协议族；
      - 是否文件型、关系型、文档型、键值型或搜索型；
      - 默认 URL/连接参数；
      - 能力集合。

  - 新增 ProtocolFamily、ConnectorCapabilities 或等价模型，描述：
      - connection test；
      - SQL query；
      - metadata；
      - CRUD；
      - transactions；
      - HTTP/JSON query；
      - document/key-value operations；
      - DDL 支持；
      - read-only 限制。

  - 保留 ConnectionProfile 的通用字段，并通过 extra_params 支持：
      - TLS；
      - HTTP path；
      - Cloudflare account/database/token；
      - MongoDB auth source；
      - ClickHouse protocol；
      - PostgreSQL/MySQL 兼容模式；
      - 搜索服务索引/租户等参数。

  - 对旧版 connections.json 做反序列化兼容和未知类型错误提示。

  ### 2. 将服务层改为能力注册

  改造 SessionManager、MetadataService、QueryService 和桌面启动注册逻辑：

  - 用统一的 DriverRegistry 管理 tester、metadata reader、query executor、Redis/document/search service。
  - 注册按 DataSourceKind 路由，不再在多个 match 中散落维护驱动。
  - 允许多个品牌类型指向同一个底层驱动实例，同时保留品牌专用适配策略。
  - 统一返回“支持但受限”的能力状态，UI 根据能力矩阵隐藏或禁用不适用操作。
  - 将关系型 SQL trait 与非关系型接口分开：
      - RelationalQueryExecutor / RelationalMetadataReader
      - DocumentConnector
      - SearchConnector
      - HttpSqlConnector
      - RedisConnector

  - 对不具备纯 Rust 实现的类型返回结构化 Unsupported，包含原因和当前支持级别。

  ### 3. 按协议族新增/扩展 crate

  #### MySQL 族

  扩展现有 db-mysql：

  - MariaDB、TiDB、OceanBase、Doris、SelectDB、StarRocks、GoldenDB 增加独立 ConnectionTester 和 profile kind。
  - 共用 MySQL wire protocol、查询执行、连接池和基础元数据实现。
  - 为 Doris/SelectDB/StarRocks 增加 HTTP 与 MySQL 端口差异处理。
  - 为 OceanBase 增加 MySQL/Oracle 模式字段；Oracle 模式在无纯 Rust Oracle 协议实现时标记受限。
  - 为 MariaDB、TiDB、GoldenDB 增加品牌版本识别和兼容能力标记。

  #### PostgreSQL 族

  扩展现有 db-postgres：

  - Redshift、CockroachDB、openGauss、GaussDB、KWDB、KingbaseES、Vastbase、HighGo、UXDB、TimescaleDB 使用 PostgreSQL wire protocol。
  - 按品牌覆盖默认端口、系统表查询、schema/catalog 差异和分页语法。
  - 对 PostgreSQL 兼容但语义不同的 DDL、类型、事务能力进行 capability gating。
  - 保留 Redshift 的只读/分析型限制和 CockroachDB 的事务重试提示。

  #### 独立关系型/分析型

  - ClickHouse：补齐现有 tester，接入 HTTP/native 查询、数据库/表/列元数据。
  - SQL Server：从 TCP 测试升级为基于 tiberius 的真实连接、查询和元数据读取。
  - TDengine：评估纯 Rust HTTP/WS 协议；若无法稳定实现，先提供受限 HTTP 连接。
  - SQLite、DuckDB：沿用现有实现，统一注册到新能力模型。
  - Access、Oracle、DM、XuguDB：保留类型和配置 UI，首期返回明确的受限/未实现状态，不使用 ODBC/JDBC。

  #### HTTP/搜索/文档类

  新增独立 crate 或按功能合并为专用适配层：

  - MongoDB：使用现有 MongoDB Rust driver，补齐 ping、数据库/集合、文档查询和基础 CRUD。
  - Redis：沿用现有 Redis connector，接入统一 registry。
  - Elasticsearch/Easysearch：Rust HTTP client + JSON/DSL 查询、索引/映射/文档操作。
  - Meilisearch：HTTP API、索引、文档、任务和 key 管理。
  - Manticore Search：优先使用 MySQL 协议查询，同时保留搜索专用能力标记。
  - Cloudflare D1：通过 Cloudflare HTTP API，使用 account/database/token 参数；不假设本地 SQLite 文件可替代远端 D1。
  - 对所有 HTTP 服务统一超时、TLS、认证脱敏、分页和错误映射。

  ### 4. 桌面 UI 与配置

  - 连接类型选择器按类别和协议族分组，显示品牌、默认端口和支持级别。
  - 表单根据类型动态显示：
      - host/port/database；
      - file path；
      - token/account/database ID；
      - auth source；
      - HTTP path；
      - SSL/TLS；
      - read-only。

  - 连接详情页显示“完整 / 兼容 / 受限 / 未实现”能力状态。
  - 查询编辑器只对支持 SQL 的类型启用；搜索和文档类使用专用查询面板或 JSON/DSL 编辑器。
  - 对兼容协议类型显示实际底层协议和品牌类型，避免误导用户。
  - 所有密码、token、Cloudflare 凭据继续只进入 secure-store，日志和错误信息必须脱敏。

  ## Test Plan

  - db-common：
      - 所有 DataSourceKind 的序列化、默认端口、协议族和能力矩阵；
      - 旧连接配置迁移；
      - URL 生成、参数校验和敏感信息脱敏。

  - 每个协议族：
      - mock/test server 连接测试；
      - 查询结果到 CellValue 的类型映射；
      - 元数据列表、分页、错误转换；
      - 超时、认证失败、TLS 失败和服务端断连。

  - MySQL/PostgreSQL 兼容品牌：
      - 同一驱动路由多个品牌；
      - 品牌专属端口和 metadata SQL；
      - 不支持能力必须返回结构化 Unsupported。

  - HTTP/搜索：
      - mock HTTP API；
      - token 脱敏；
      - 分页、JSON 错误、限流和超时；
      - D1 account/database/token 参数校验。

  - SQL Server、ClickHouse、MongoDB、Redis、SQLite、DuckDB：
      - 保留现有测试并迁移到 registry；
      - 增加真实集成测试开关，默认不依赖本机服务。

  - UI：
      - 所有类型可创建、编辑、保存、加载；
      - 受限能力正确禁用；
      - 查询编辑器不会为非 SQL 类型错误打开；
      - 配置导入导出兼容。

  - 验证命令：
      - cargo fmt --all -- --check
      - cargo check --workspace
      - cargo test --workspace
      - cargo clippy --workspace --all-targets -- -D warnings

  ## Delivery Phases

  1. 基础架构阶段：扩展类型模型、能力矩阵、DriverRegistry、配置兼容和 UI 类型分组。
  2. MySQL 族阶段：MariaDB、TiDB、OceanBase、Doris、SelectDB、StarRocks、GoldenDB。
  3. PostgreSQL 族阶段：Redshift、CockroachDB、openGauss、GaussDB、KWDB、KingbaseES、Vastbase、HighGo、UXDB。
  4. 独立驱动阶段：SQL Server、ClickHouse、TDengine、MongoDB、Redis。
  5. HTTP/搜索阶段：Cloudflare D1、Elasticsearch、Easysearch、Meilisearch、Manticore Search。
  6. 受限类型阶段：Access、Oracle、DM、XuguDB；仅在出现可靠纯 Rust 驱动或协议实现后升级为完整能力。

  ## Assumptions

  - “纯 Rust”意味着运行时不依赖 ODBC、JDBC、Python、Node、厂商动态库或外部 sidecar。
  - 协议兼容接入允许复用 MySQL/PostgreSQL 驱动，但 UI 和能力矩阵必须明确标注兼容模式。
  - “直接连接”对 HTTP 服务表示直接调用其官方 HTTP API，不强行转化为关系型 SQL。
  - 完整能力按数据库实际语义实现；不承诺所有品牌共享完全一致的 DDL、事务或类型行为。
  - 在没有可靠纯 Rust 驱动的数据库上，系统可以保存配置并显示受限状态，但不能宣称已经支持完整连接能力。
