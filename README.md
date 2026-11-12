# Orvyn

[English](README.en.md)

使用 Rust 与 egui/eframe 构建的原生数据库客户端，采用原生协议、HTTP、厂商 ODBC 驱动和 DuckDB 动态库。

## 功能

- SQL 高亮、美化、关键字与表字段补全、别名识别、历史和收藏。
- 服务端分页、过滤、排序，以及基于主键的数据编辑。
- CSV、JSON、Excel 兼容 TSV 和原生 XLSX 导出；XLSX 将超大整数及 Decimal 保留为文本。
- CSV 编码识别、分批导入、进度、取消与失败或状态未确认行输出。
- MongoDB 过滤、投影、排序、分页、聚合和单文档增删改。
- Elasticsearch/Easysearch 基于 mapping 的补全、完整响应、可用时采用 PIT/search_after 分页、取消和当前页 JSON 导出。
- Meilisearch、Manticore、Cloudflare D1、Redis 操作；D1 保留各语句结果。
- 真实存储例程定义编辑，保留 PostgreSQL 重载签名。
- 敏感连接参数加密、旧配置迁移、默认脱敏导出，以及文件锁和原子保存。

## 支持范围

MySQL/PostgreSQL 兼容品牌共享协议驱动，行为取决于品牌、版本和权限。SQLite/DuckDB 使用本地引擎；DuckDB 需要匹配动态库。SQL Server/ClickHouse 使用独立连接器，ClickHouse 无客户端事务。TDengine 提供 REST SQL、表及超级表元数据，无事务、视图和索引设计器。Oracle、达梦、虚谷、Access 需要匹配架构的厂商 ODBC 驱动。MongoDB 界面未提供多文档事务，D1 无客户端管理事务。

SQLite/DuckDB 使用引擎中断，普通 PostgreSQL/MySQL 查询发送服务端取消。其余连接器、部分分析型品牌和事务路径可能只能取消客户端等待，诊断会明确提示服务端状态未确认。取消不保证撤销已提交的数据。

只读模式保守检查 SQL 并阻止已暴露的文档写操作；服务端函数副作用仍需数据库权限约束。能力标记表示连接器已实现的操作，服务器版本和权限可能进一步限制功能。

## 编译验证

需要 Rust stable（依赖至少要求 1.88）、Python 3、本机 C/C++ 工具链。

```text
python packaging/prepare-duckdb.py
cargo run -p orvyn --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

下载默认使用 GitHub 代理，将 `ORVYN_GITHUB_PROXY` 设为空字符串可直连。DuckDB 1.5.4 文件位于 `packaging/duckdb-download/local/1.5.4/`，更换版本或架构后重新准备。

Windows 需要 Visual Studio C++ Build Tools 和 Rust MSVC。Linux 安装 `build-essential pkg-config unixodbc-dev libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev`。macOS 安装 Xcode 命令行工具和 `brew install unixodbc`。

## 打包和 CI

- Windows：`packaging/windows/build-installer.ps1`，需要 Inno Setup 6。
- Linux：`bash packaging/linux/package.sh`，生成包含 DuckDB 的 `.deb`。
- macOS：`bash packaging/macos/package.sh`，生成 `.app`/`.dmg`，修正动态库路径并临时签名；正式分发另需 Developer ID 签名和公证。

`.github/workflows/ci.yml` 定义三平台测试、严格 Clippy、打包和临时 PostgreSQL/MySQL/Redis/Elasticsearch 服务测试。对应 runner 实际通过后才能确认平台发布可用性。集成测试默认忽略，只读取显式 `ORVYN_TEST_*` 环境变量，不读取本机凭据文件。

## 配置

- Windows：`%APPDATA%\Orvyn`
- Linux：`$XDG_CONFIG_HOME/orvyn` 或 `$HOME/.config/orvyn`
- macOS：`$HOME/Library/Application Support/Orvyn`

通过 `ORVYN_CONFIG_DIR` 覆盖目录。通过 `ORVYN_MASTER_KEY` 指定密钥，或保留原子创建的私有 `master.key`。请与加密凭据一起备份；损坏的已有密钥不会自动覆盖。能够读取整个配置目录的用户仍可取得密钥。

验证与限制见 [修复清单](docs/REMEDIATION.md)。SSH 隧道、结构/数据对比、ER 图、用户权限管理、自动更新仍属后续功能，当前未实现。

许可证：MIT OR Apache-2.0，见包元数据。
