<p align="center">
  <img src="packaging/icons/icon.png" width="96" alt="Orvyn">
</p>
<h1 align="center">Orvyn</h1>

<p align="center">
  <b>专业数据库工作台，释放原生 Rust 性能。</b><br>
  35 种数据源，贯通 SQL、NoSQL、搜索与分析，一个桌面工作区掌控全局。
</p>

<p align="center">
  <a href="https://github.com/dbx192/Orvyn/releases/latest"><img src="https://img.shields.io/github/v/tag/dbx192/Orvyn?label=release&amp;color=teal" alt="最新版本"></a>
  <img src="https://img.shields.io/badge/built_with-Rust-orange.svg" alt="使用 Rust 构建">
  <img src="https://img.shields.io/badge/desktop-Windows%20%7C%20Linux%20%7C%20macOS-0078D4.svg" alt="Windows、Linux、macOS">
  <img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT 许可证">
</p>

<p align="center">
  <a href="README.md">English</a> · <b>简体中文</b> · <a href="README.hi.md">हिन्दी</a> · <a href="README.es.md">Español</a> · <a href="README.fr.md">Français</a><br>
  <a href="https://github.com/dbx192/Orvyn/releases/latest">下载</a> · <a href="#为性能而生">性能架构</a> · <a href="#数据源全景">数据源</a> · <a href="#从源码构建">源码构建</a>
</p>

---

**Orvyn 是基于 Rust + egui 打造的高性能数据库工作台。** SQL 编写、数据编辑、结构设计、Redis 键管理、MongoDB 文档查询、搜索索引探索与大表导出，全部汇聚到一个原生桌面应用。

从生产数据库到本地 DuckDB 分析，Orvyn 将广泛的数据生态收进专注的工作空间。通过原生协议驱动、HTTP 接口与厂商 ODBC 桥接，连接 35 种数据源。SQLite 随构建内置。DuckDB 1.5.4 为可选功能：默认包不包含引擎，需要时选择文件名带 `-duckdb` 的安装包。

**原生执行，虚拟化表格，流式导出，五种界面语言。让数据工作全速向前。**

## 为性能而生

从桌面界面到导出管线，性能贯穿架构设计。

| 技术底座 | 带来的能力 |
| --- | --- |
| **原生 Rust + egui** | 编译为桌面应用，无需 Electron 或浏览器运行时。 |
| **后台 Tokio 运行时** | 数据库任务在 UI 线程外执行；阻塞任务可交给限制并发的工作池处理。 |
| **虚拟化数据表格** | 查询结果和表数据视图仅绘制可见行，避免每帧渲染所有已加载数据。 |
| **服务端分页、过滤与排序** | 按有限页大小浏览表数据，将表过滤条件下推至数据库。 |
| **MySQL / PostgreSQL 连接池** | 跨操作复用连接，减少逐次查询重新建立连接的开销。 |
| **每批 5,000 行的流式导出** | MySQL/PostgreSQL 协议驱动计数一次，随后单次扫描数据、逐批交给写入器，绕过编辑器分页与查询历史。 |
| **增量文件写入器** | CSV、JSON、TSV、XLSX 持续写盘；XLSX 使用恒定内存工作表，达到单表行数上限后自动分表。 |
| **SQL 高亮防抖** | 合并连续编辑触发的高亮任务，超大文档超过阈值后跳过高亮，减少编辑器计算开销。 |

表导出支持进度显示和取消，拦截重复启动的表导出任务，并通过临时文件写入，在取消时保留已有目标文件。其他关系型驱动采用分页导出回退路径。实际吞吐量取决于数据库、执行计划、网络、行宽和输出格式。

## 五种语言，同一个强大工作区

**English · 简体中文 · हिन्दी · Español · Français**

用熟悉的语言，驾驭复杂的数据。Orvyn 在同一个原生应用中提供五种界面语言：首次启动自动匹配受支持的系统语言，随时从界面切换，记住你的选择供下次启动使用。跨地区团队可以使用同一套数据库工具，让每个人都用自己顺手的语言工作。

项目文档也覆盖以上五种语言，可通过页面顶部导航切换。从产品界面到使用文档，让更多开发者轻松上手。

## 数据源全景

**35 种数据源，围绕底层协议构建统一连接能力。**

| 类别 | 支持的数据源 |
| --- | --- |
| **MySQL 协议 · 8** | MySQL · MariaDB · TiDB · OceanBase · Apache Doris · SelectDB · StarRocks · GoldenDB |
| **PostgreSQL 协议 · 11** | PostgreSQL · TimescaleDB · Redshift · CockroachDB · openGauss · GaussDB · KWDB · KingbaseES · Vastbase · HighGo · UXDB |
| **关系型与分析引擎 · 5** | SQLite · DuckDB · SQL Server · ClickHouse · TDengine（REST） |
| **文档、键值与 HTTP SQL · 3** | MongoDB · Redis · Cloudflare D1 |
| **搜索引擎 · 4** | Elasticsearch · Easysearch · Meilisearch · Manticore Search |
| **厂商 ODBC · 4** | Oracle · 达梦 · 虚谷 · Microsoft Access |

同协议族共享驱动，同时保留品牌标识和引擎差异处理。ODBC 数据源需要在本机安装兼容的厂商驱动。具体功能取决于引擎、服务器版本和账号权限。

## 覆盖日常工作的完整能力

### SQL 工作室

- **多标签编辑**：SQL 语法高亮、格式化、关键字与表字段补全，支持识别别名的列建议。
- **灵活执行**：运行光标所在语句、选中 SQL 或完整脚本，分别呈现各语句结果、耗时、影响行数和通知。
- **历史与收藏**：回溯执行记录，将常用 SQL 保存在手边。
- **事务控制**：在支持客户端会话的引擎上执行 `BEGIN`、`COMMIT` 和 `ROLLBACK`。
- **查询取消**：MySQL 和 PostgreSQL 驱动支持服务端取消，具体取决于服务器能力与权限。

### AI 助手

- 从工作区顶部打开 **AI**，在设置中选择服务商、填写 API 地址并测试连接。支持手动填写模型，也可以从服务商获取模型列表。
- 支持流式对话、停止生成和恢复本地对话。发送时可勾选当前 SQL 或已加载的表结构，直接使用当前查询的数据库方言。
- 生成的 SQL 可以复制，或在当前连接的新查询标签页打开，检查后由你执行。

### 数据浏览与编辑

- 通过对象树浏览数据库、模式、表、视图、列、索引、外键、触发器与例程。
- 表数据支持分页、列过滤、排序和可调整列宽的网格。
- 基于主键新增、修改和删除行，先暂存变更，再通过摘要确认后提交。
- 查看对象定义，在同一工作区中切换表数据、表结构与查询标签。

### 结构设计与 DDL 管理

- 在表结构设计器中暂存列新增、删除、重命名和类型修改，再统一应用。
- 通过可视化控件管理主键、索引与外键，生成对应数据库方言的 SQL。
- 直接编辑触发器和例程定义，保留 PostgreSQL 例程的重载签名。
- 从对象树建表，并执行引擎支持的删除、清表和数据库操作。

### 大规模数据进出

- **四种导出格式**：CSV、JSON、Excel 兼容 TSV、原生 XLSX。
- **完整表导出**：增量写入、实时行数进度与取消；MySQL/PostgreSQL 驱动使用上述流式路径。
- **XLSX 精度保护**：大整数与 Decimal 以文本写入，保留精确值；自动去除 XML 非法字符。
- **整库 SQL 导出**：支持仅结构或结构加数据，适用于已支持的引擎。
- **CSV 导入向导**：编码识别、数据预览、分批插入、进度、取消与失败行报告。

### 超越 SQL 的专用工作区

| 工作区 | 功能 |
| --- | --- |
| **Redis** | 游标扫描、平铺与层级键视图，String/Hash/List/Set/Sorted Set 编辑，创建键、重命名、批量删除、TTL 管理、JSON/原始值查看与命令控制台。 |
| **MongoDB** | 文档过滤、投影、排序、分页、聚合管道，以及单文档新增、修改和删除。 |
| **Elasticsearch / Easysearch** | 基于 mapping 的字段补全、原始响应、`PIT` / `search_after` 深分页与当前页 JSON 导出。 |
| **Meilisearch / Manticore Search** | 通过统一搜索工作区进行专用搜索浏览。 |
| **Cloudflare D1** | HTTP SQL 执行，每条语句的结果独立保留。 |

### 保护连接与变更

- 已保存的连接凭据采用 **ChaCha20-Poly1305 加密**。
- **连接导出默认脱敏**，需要明确选择才包含密码。
- **五档 SSL 模式**：`Disable`、`Prefer`、`Require`、`VerifyCa`、`VerifyFull`，取决于连接器支持情况。
- **连接只读模式**：保守拒绝未被识别为只读的 SQL。
- **配置文件锁与原子保存**；主密钥原子创建，已有密钥为空或不可读取时不会静默替换。
- **数据和结构变更暂存**，在提交支持的操作前确认。

### 打造顺手的工作空间

深色与浅色主题，搭配内置的 **Inter / JetBrains Mono / Noto Sans CJK** 字体，与多语言界面共同构成顺手的工作环境。清晰阅读 SQL、快捷切换主题，在同一工作区中切换查询、数据与结构标签。

| 操作 | Windows / Linux | macOS |
| --- | --- | --- |
| 新建查询 | `Ctrl+N` | `Cmd+N` |
| 执行当前语句 | `Ctrl+Enter` | `Cmd+Enter` |
| 执行选中 SQL | `Ctrl+Shift+Enter` | `Cmd+Shift+Enter` |
| 格式化 SQL | `Ctrl+Shift+F` | `Cmd+Shift+F` |
| 聚焦对象搜索 | `Ctrl+L` | `Cmd+L` |
| 切换主题 | `Ctrl+T` | `Cmd+T` |

## 下载与快速开始

前往 **[GitHub Releases](https://github.com/dbx192/Orvyn/releases/latest)** 下载最新安装包。

| 平台 | 发布工作流产物 |
| --- | --- |
| **Windows · x86_64** | Inno Setup 安装包，按用户安装，无需管理员提权。 |
| **Linux · x86_64 / amd64** | `.deb` 包，仅 `-duckdb` 版本内含 DuckDB 动态库。 |
| **macOS · Intel x86_64** | `.dmg` 中的 `.app` 应用，仅 `-duckdb` 版本内含 DuckDB。 |
| **macOS · Apple Silicon aarch64** | `.dmg` 中的 `.app` 应用，仅 `-duckdb` 版本内含 DuckDB。 |

Linux 打包与 ### DuckDB 可选版本

打包脚本默认不包含 DuckDB。Windows 使用 `./packaging/windows/build-installer.ps1 -WithDuckDB`；Linux/macOS 向打包脚本传入 `--with-duckdb`。CI 为四个平台/架构组合同时生成默认版和 `-duckdb` 版本。默认版隐藏新建 DuckDB 连接入口，保留已有连接配置；需要使用时安装 DuckDB 版本，仅复制动态库不会启用该功能。全工作区检查需要先准备 DuckDB。

DuckDB 准备脚本也可处理 ARM64 主机；当前发布工作流构建 Linux x86_64 安装包。

1. 安装对应平台的软件包，启动 Orvyn。
2. 创建连接，选择数据源类型，填写连接信息或本地数据库路径。
3. 测试并保存连接，浏览对象或打开查询标签。
4. 执行 SQL，暂存行或结构变更，或从对象树导出表。Redis 与搜索数据源提供专用浏览器。

## 从源码构建

需要 **Rust stable**（工作区声明最低 1.88，当前 CI 使用 1.96.1）、**Python 3** 和下列本机构建依赖。复现发布构建时建议使用 CI 工具链。

| 平台 | 本机依赖 |
| --- | --- |
| Windows | Visual Studio C++ Build Tools 与 Rust MSVC 工具链。 |
| Linux | `build-essential pkg-config unixodbc-dev libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev` |
| macOS | Xcode 命令行工具与 `brew install unixodbc`。 |

在仓库根目录启动默认版应用：

```sh
cargo run -p orvyn --release --locked
```

默认不启用 DuckDB，无需下载其动态库。需要 DuckDB 时：

```sh
python packaging/prepare-duckdb.py
cargo run -p orvyn --release --locked --features duckdb
```

运行工作区检查：

```sh
cargo test -p orvyn -p app-core --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

DuckDB 准备脚本默认通过 `https://gh-proxy.org/` 下载 **1.5.4**，将动态库放入 `packaging/duckdb-download/local/1.5.4/`。将 `ORVYN_GITHUB_PROXY` 设为空字符串可直连 GitHub，也可设为其他代理前缀。

打包入口：

- [Windows 安装包](packaging/windows/build-installer.ps1)：需要 Inno Setup 6。
- [Linux 软件包](packaging/linux/package.sh)。
- [macOS 应用与 DMG](packaging/macos/package.sh)。

[发布工作流](.github/workflows/ci.yml) 在 `v*` tag 推送时运行：Windows、Linux、macOS 原生测试与严格 Clippy，PostgreSQL、MySQL、Redis、Elasticsearch 集成测试，随后完成四个平台与架构组合的构建及 GitHub 发布。

## 配置

| 平台 | 配置目录 |
| --- | --- |
| Windows | `%APPDATA%\Orvyn` |
| Linux | `$XDG_CONFIG_HOME/orvyn` 或 `~/.config/orvyn` |
| macOS | `~/Library/Application Support/Orvyn` |

- `ORVYN_CONFIG_DIR`：覆盖配置目录。
- `ORVYN_MASTER_KEY`：指定凭据加密密钥；未设置时，在配置目录原子创建私有 `master.key`。
- `RUST_LOG`：控制日志级别；应用日志写入配置目录中的 `orvyn.log`。

**请将主密钥与加密凭据一起备份。** 恢复凭据时必须使用相同密钥。

## 兼容性说明

- 能力随引擎而异。ClickHouse、TDengine、Cloudflare D1、MongoDB 不提供客户端事务控制。
- 取消不会撤销已提交的工作。部分连接器只能停止客户端等待，诊断信息会区分此情况与已确认的服务端取消。
- 只读模式属于客户端防护；服务端函数和其他副作用应通过数据库权限约束。
- SSH 隧道、结构与数据对比、ER 图、用户权限管理、自动更新当前尚未实现。

## 许可证

本项目采用 [MIT 许可证](LICENSE) 开源。
