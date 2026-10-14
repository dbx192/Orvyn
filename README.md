# Orvyn 1.0.0

[English](README.en.md)

**Orvyn 是一个用纯 Rust 打造的高性能跨平台数据库客户端。** 单个原生二进制，GPU 加速界面，不依赖 ODBC、JDBC、Electron 或任何 sidecar——启动快、体积小、内存占用低。一套客户端管住 20+ 数据源：关系型、文档型、键值、搜索引擎，全部原生协议直连。

## 为什么是 Orvyn

- **纯 Rust 原生性能**：egui/eframe GPU 渲染，无虚拟机、无浏览器内核。结果网格虚拟化绘制，只渲染可见行。
- **百万行不卡**：服务端分页（25–1000 行/页）、深分页扁平化、筛选条件下推到数据库执行，大表翻页、排序、筛选依旧顺滑。
- **20+ 数据源，一个界面**：原生驱动直连而非 ODBC/JDBC 桥接；按能力矩阵自动禁用后端不支持的操作。
- **安全默认**：密码可选保存并用 ChaCha20-Poly1305 本地加密；TLS 策略完整支持到 Verify Full 且不静默回退明文；一键只读连接。
- **开箱即用的本地化**：内置思源黑体 CJK 字体链，支持简体中文、English、Español、Français、हिन्दी 五种界面语言。

## 核心功能

**SQL 编辑器**

- 多标签编辑器：执行全部 / 当前 / 选中 SQL，长查询随时取消。
- 语法高亮、关键字与 schema 表/列智能补全、查询历史与收藏。

**数据网格（Navicat 风格）**

- 服务端分页、排序与多列组合筛选。
- 检测到主键即可行内编辑、插入、删除；无主键表默认只读。
- CSV / JSON / Excel 兼容 TSV 导出；可编辑结果集（TableEditor）中的数据同样可导出。

**表结构设计与维护**

- 可视化管理列、索引、外键、触发器、函数，结构变更批量应用；批量 DDL 执行队列。
- 表数据导入向导、数据库结构/数据导出、DDL 本地保存。
- 对象树右键即达：复制名称/完整路径、新建查询、导入 CSV、清空/删除表，删除数据库带二次确认弹窗。

**Redis（Tiny RDM 风格）**

- 多连接标签页、树形键浏览、SCAN 分页与 glob 模式过滤。
- 常见数据结构查看与编辑、TTL 修改、键删除、FLUSHDB 二次确认。

**MongoDB / 搜索引擎 / HTTP SQL**

- MongoDB 集合浏览、文档查询与基础 CRUD。
- Elasticsearch、Easysearch、Meilisearch、Manticore Search 索引与 JSON/DSL 查询；Cloudflare D1 HTTP SQL。

**连接管理与安全**

- 连接新建、编辑、测试、分组、删除、导入和导出；连接与测试均有超时控制，瞬时网络错误自动重试。
- PostgreSQL、MySQL、SQL Server 支持 Disable / Prefer / Require / Verify CA / Verify Full 五档 TLS 与可选 CA 文件路径，严格模式绝不静默退回明文。
- 只读连接开关；亮 / 暗主题一键切换（Ctrl+T）。

## 数据源

| 分类 | 数据源 |
| --- | --- |
| MySQL 协议族 | MySQL、MariaDB、TiDB、OceanBase、Doris、SelectDB、StarRocks、GoldenDB |
| PostgreSQL 协议族 | PostgreSQL、TimescaleDB、Redshift、CockroachDB、OpenGauss、GaussDB、KwDb、KingbaseES、Vastbase、HighGo、UXDB |
| 关系/分析 | SQLite、DuckDB、ClickHouse、SQL Server |
| 文档/键值 | MongoDB、Redis |
| 搜索 | Elasticsearch、Easysearch、Meilisearch、Manticore Search |
| HTTP SQL | Cloudflare D1 |
| 受限 | TDengine 仅 HTTP 连接测试；Access、Oracle、DM、XuguDB 可保存配置，但没有可用驱动 |

注册支持不代表所有后端都有相同的元数据、DDL 或编辑能力；界面按能力矩阵禁用不适用操作。

## 运行源码

前提：Rust 1.78 或更高版本，以及当前系统的 C/C++ 构建工具链。

~~~powershell
cargo run -p orvyn --locked
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
~~~

## TLS 策略

在连接表单选择 SSL 模式。PostgreSQL、MySQL 和 SQL Server 可填写 TLS CA certificate path；它只保存文件路径，不保存证书内容或私钥。

| 模式 | 行为 |
| --- | --- |
| Disable | 明确禁用 TLS。 |
| Prefer | 优先协商 TLS，协议允许时可回退。 |
| Require | 必须 TLS，但不要求额外 CA 文件。 |
| Verify CA | 必须 TLS，并校验服务器证书链。 |
| Verify Full | 在 Verify CA 基础上校验证书主机名。 |

严格模式应使用数据库 DNS 名称，并填写企业或云服务商提供的 CA 文件。自签名服务应提供对应 CA，而不是改用 Disable。

## 配置和凭据

- Windows：%APPDATA%\Orvyn
- Linux/macOS：$XDG_CONFIG_HOME/orvyn；未设置时为当前目录下的 orvyn

该目录保存连接、查询历史、收藏、设置、日志和加密材料。可覆盖：

~~~powershell
$env:ORVYN_CONFIG_DIR = "D:\path\to\orvyn-config"
$env:ORVYN_MASTER_KEY = "replace-with-a-strong-private-key"
~~~

未设置 ORVYN_MASTER_KEY 时应用会创建 master.key。请保护配置目录和该密钥。

## 打包共同准备

请在目标系统本机打包。系统 SDK、签名和 DuckDB 动态库使跨系统打包不可靠。

项目使用本地 DuckDB 库，构建时不会自动下载。默认目录：

~~~text
packaging/duckdb-download/local/1.5.4
~~~

目录必须包含目标平台文件：

- Windows x64 MSVC：duckdb.lib、duckdb.dll、duckdb.h
- Linux x64：libduckdb.so、duckdb.h
- macOS：libduckdb.dylib、duckdb.h

更换 DuckDB 版本、目标架构或清理 target 后，须放回匹配库文件。发布构建一律使用 --locked。

## Windows：生成安装程序

仓库内置脚本会构建 MSVC x64、附带 duckdb.dll，并调用 Inno Setup 6。

1. 安装 Rust MSVC 工具链和 Visual Studio Build Tools 的 Desktop development with C++。
2. 安装 [Inno Setup 6](https://jrsoftware.org/isinfo.php)。
3. 准备 Windows DuckDB 文件。
4. 在仓库根目录执行：

~~~powershell
rustup target add x86_64-pc-windows-msvc
.\packaging\windows\build-installer.ps1
~~~

产物在 dist\windows\，名称类似：

~~~text
orvyn-1.0.0-windows-x86_64-<icon-hash>-setup.exe
~~~

公开发布前应对 .exe 做 Authenticode 签名，否则可能触发 SmartScreen。

## Linux：生成 .deb

建议在 Ubuntu 22.04 一类较旧基线上构建，以提高 libc 兼容性：

~~~bash
sudo apt update
sudo apt install -y build-essential pkg-config libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev
rustup target add x86_64-unknown-linux-gnu
cargo build -p orvyn --release --locked --target x86_64-unknown-linux-gnu
~~~

组装 .deb。启动器设置 LD_LIBRARY_PATH，确保能找到随包的 DuckDB：

~~~bash
PKG=dist/linux/orvyn_1.0.0_amd64
rm -rf "$PKG"
mkdir -p "$PKG/DEBIAN" "$PKG/usr/lib/orvyn" "$PKG/usr/bin" "$PKG/usr/share/applications"
cp target/x86_64-unknown-linux-gnu/release/orvyn "$PKG/usr/lib/orvyn/"
cp packaging/duckdb-download/local/1.5.4/libduckdb.so "$PKG/usr/lib/orvyn/"
printf '#!/bin/sh\nexec env LD_LIBRARY_PATH=/usr/lib/orvyn /usr/lib/orvyn/orvyn "$@"\n' > "$PKG/usr/bin/orvyn"
chmod 755 "$PKG/usr/bin/orvyn"
printf 'Package: orvyn\nVersion: 1.0.0\nSection: database\nPriority: optional\nArchitecture: amd64\nMaintainer: Orvyn\nDescription: Cross-platform desktop database client\n' > "$PKG/DEBIAN/control"
printf '[Desktop Entry]\nType=Application\nName=Orvyn\nExec=orvyn\nCategories=Development;Database;\nTerminal=false\n' > "$PKG/usr/share/applications/orvyn.desktop"
dpkg-deb --build "$PKG"
sudo apt install ./dist/linux/orvyn_1.0.0_amd64.deb
orvyn
~~~

AppImage 可复用该 usr 布局和 appimagetool；其 AppRun 也必须设置同样的库路径。仓库目前没有自动 Linux 打包脚本。

## macOS：生成 .app 和 .dmg

在相应架构的 Mac 上构建：Apple Silicon 用 aarch64-apple-darwin，Intel 用 x86_64-apple-darwin。

~~~bash
xcode-select --install
rustup target add aarch64-apple-darwin # Intel 改为 x86_64-apple-darwin
cargo build -p orvyn --release --locked --target aarch64-apple-darwin
TARGET=aarch64-apple-darwin
APP=dist/macos/Orvyn.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks"
cp "target/$TARGET/release/orvyn" "$APP/Contents/MacOS/Orvyn"
cp packaging/duckdb-download/local/1.5.4/libduckdb.dylib "$APP/Contents/Frameworks/"
cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Orvyn</string>
<key>CFBundleIdentifier</key><string>io.orvyn.desktop</string>
<key>CFBundleName</key><string>Orvyn</string>
<key>CFBundleShortVersionString</key><string>1.0.0</string>
</dict></plist>
EOF
install_name_tool -add_rpath @executable_path/../Frameworks "$APP/Contents/MacOS/Orvyn"
hdiutil create -volname Orvyn -srcfolder "$APP" -ov -format UDZO dist/macos/Orvyn-1.0.0.dmg
~~~

用 otool -L "$APP/Contents/MacOS/Orvyn" 检查 DuckDB 动态库引用。外部发布需要 Developer ID 签名和公证：

~~~bash
codesign --force --deep --options runtime --sign "Developer ID Application: Your Name (TEAMID)" "$APP"
xcrun notarytool submit dist/macos/Orvyn-1.0.0.dmg --keychain-profile "notary-profile" --wait
xcrun stapler staple dist/macos/Orvyn-1.0.0.dmg
~~~

仓库目前没有 macOS 自动打包脚本或 .icns 图标。

## 许可证

采用 [MIT OR Apache-2.0](Cargo.toml) 双许可证。
