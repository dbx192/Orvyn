# Rust DB Client 跨平台发布说明

本文按当前仓库的实际结构编写：Rust workspace、`eframe/egui` 桌面程序、二进制 crate 为 `desktop`，并动态链接 DuckDB 1.5.4。

## 1. 推荐发布产物

每个平台应在对应的原生系统上构建。不要把“在 Linux 上给三个系统交叉编译”作为正式发布方案；GUI、系统 SDK、代码签名和 DuckDB 动态库会让这种方案很不稳定。

建议至少发布以下文件：

| 平台 | 架构 | 便携版 | 安装包 |
| --- | --- | --- | --- |
| Linux | x86_64 | `rust-db-client-0.1.0-linux-x86_64.tar.gz` | `.AppImage`，可选 `.deb` |
| Windows 10/11 | x86_64 | `rust-db-client-0.1.0-windows-x86_64.zip` | Inno Setup 生成的 `.exe` |
| macOS Apple Silicon | arm64 | `.app` | `.dmg` |
| macOS Intel（仍需支持时） | x86_64 | `.app` | `.dmg` |

当前程序不是严格意义上的“单文件程序”：

- Linux 必须携带 `libduckdb.so`；
- Windows 必须携带 `duckdb.dll`；
- macOS 必须携带 `libduckdb.dylib`；
- SQLite 已启用 `bundled`，不需要另外携带 SQLite 动态库。

## 2. 正式发布前的必要准备

### 2.1 固定版本并锁定依赖

在根目录 `Cargo.toml` 修改版本：

```toml
[workspace.package]
version = "0.1.0"
```

提交 `Cargo.lock`，所有发布构建使用 `--locked`：

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

### 2.2 准备应用图标

当前仓库没有正式应用图标。建议添加：

```text
packaging/icons/icon.png       # 512x512 或 1024x1024，Linux
packaging/icons/icon.ico       # Windows，多尺寸 ICO
packaging/icons/AppIcon.icns   # macOS
```

没有图标也能生成便携版，但安装包和 macOS 应用不适合正式发布。

### 2.3 修正配置目录和主密钥

当前 `apps/desktop/src/main.rs` 默认把数据写到相对目录 `.rust-db-client`。安装后，工作目录可能是 `Program Files`、`/usr/bin` 或其他不可写/不可预测目录。正式发布前应改为系统用户数据目录，例如：

| 平台 | 建议目录 |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/rust-db-client` 或 `~/.config/rust-db-client` |
| Windows | `%APPDATA%\Rust DB Client` |
| macOS | `~/Library/Application Support/Rust DB Client` |

可使用 Rust 的 `directories` crate 获取这些目录，同时继续保留 `RUST_DB_CLIENT_CONFIG_DIR` 作为覆盖项。

另外，当前未设置 `RUST_DB_CLIENT_MASTER_KEY` 时会使用固定的开发密钥 `development-master-key-change-me`。正式版不可继续使用该默认值。至少要在首次运行时生成随机本机密钥并限制文件权限；更好的做法是使用 Windows Credential Manager、macOS Keychain 和 Linux Secret Service 保存密钥。

### 2.4 Windows 隐藏控制台窗口

Windows GUI 正式版建议在 `apps/desktop/src/main.rs` 顶部加入：

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
```

否则双击程序时可能同时出现控制台窗口。

### 2.5 准备各平台 DuckDB 1.5.4 库

`.cargo/config.toml` 当前要求库位于：

```text
target/duckdb-download/local/1.5.4/
```

各构建机放入与目标架构完全一致的官方库：

```text
Linux x86_64:  libduckdb.so, duckdb.h
Windows x64:   duckdb.lib, duckdb.dll, duckdb.h
macOS arm64:   libduckdb.dylib, duckdb.h
macOS x86_64:  libduckdb.dylib, duckdb.h
```

`target/` 已被 Git 忽略，因此这些文件不会随代码仓库自动到达新的构建机。应在发布流水线中从可信、固定版本的制品库下载，并校验 SHA-256。不要混用架构，也不要把 Linux、Windows、macOS 同名库视为可互换文件。

## 3. Linux x86_64

建议在要支持的最老发行版上构建，例如 Ubuntu 22.04。这样生成的程序通常能在更新的 glibc 系统上运行。

### 3.1 安装构建依赖

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config patchelf file curl \
  libx11-dev libxi-dev libgl1-mesa-dev libasound2-dev \
  libudev-dev libwayland-dev libxkbcommon-dev

rustup toolchain install stable
rustup default stable
```

将 Linux x86_64 的 `libduckdb.so` 和 `duckdb.h` 放入 `target/duckdb-download/local/1.5.4/` 后构建：

```bash
cargo test --workspace --locked
cargo build -p desktop --release --locked
```

原始二进制位于 `target/release/desktop`。检查它确实动态依赖 DuckDB：

```bash
readelf -d target/release/desktop | grep NEEDED
LD_LIBRARY_PATH="$PWD/target/duckdb-download/local/1.5.4" \
  ldd target/release/desktop
```

`ldd` 中不得出现 `not found`。

### 3.2 生成便携 tar.gz

```bash
VERSION=0.1.0
ROOT="$PWD"
OUT="$ROOT/dist/linux/rust-db-client-$VERSION-linux-x86_64"

rm -rf "$ROOT/dist/linux"
mkdir -p "$OUT/lib"
install -m 755 target/release/desktop "$OUT/rust-db-client-bin"
install -m 755 target/duckdb-download/local/1.5.4/libduckdb.so "$OUT/lib/libduckdb.so"
```

在 `$OUT/rust-db-client` 创建启动脚本：

```bash
#!/bin/sh
set -eu
APP_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export LD_LIBRARY_PATH="$APP_DIR/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$APP_DIR/rust-db-client-bin" "$@"
```

然后执行：

```bash
chmod 755 "$OUT/rust-db-client"
tar -C "$ROOT/dist/linux" -czf \
  "$ROOT/dist/linux/rust-db-client-$VERSION-linux-x86_64.tar.gz" \
  "rust-db-client-$VERSION-linux-x86_64"
```

解压到一个临时目录并运行 `./rust-db-client` 做一次真实验收。

### 3.3 生成 AppImage

先下载并授权 `linuxdeploy-x86_64.AppImage`，然后构建 AppDir：

```bash
VERSION=0.1.0
APPDIR="$PWD/dist/linux/AppDir"

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/lib/rust-db-client" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/512x512/apps"
install -m 755 target/release/desktop "$APPDIR/usr/lib/rust-db-client/rust-db-client"
install -m 755 target/duckdb-download/local/1.5.4/libduckdb.so "$APPDIR/usr/lib/rust-db-client/libduckdb.so"
install -m 644 packaging/icons/icon.png "$APPDIR/usr/share/icons/hicolor/512x512/apps/rust-db-client.png"
patchelf --set-rpath '$ORIGIN' "$APPDIR/usr/lib/rust-db-client/rust-db-client"
```

创建 `dist/linux/rust-db-client.desktop`：

```ini
[Desktop Entry]
Type=Application
Name=Rust DB Client
Exec=rust-db-client
Icon=rust-db-client
Categories=Development;Database;
Terminal=false
```

让 `linuxdeploy` 收集需要随包携带的库并生成 AppImage：

```bash
export LD_LIBRARY_PATH="$APPDIR/usr/lib/rust-db-client"
./linuxdeploy-x86_64.AppImage \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/lib/rust-db-client/rust-db-client" \
  --desktop-file dist/linux/rust-db-client.desktop \
  --icon-file packaging/icons/icon.png \
  --output appimage

mv Rust_DB_Client-x86_64.AppImage \
  "dist/linux/rust-db-client-$VERSION-linux-x86_64.AppImage"
```

`linuxdeploy` 实际生成的文件名可能随版本略有不同，先用 `ls *.AppImage` 确认再重命名。在 Ubuntu 22.04 和至少一个较新发行版上测试启动、文件选择框、中文字体、SQLite/DuckDB 打开和远程数据库连接。

### 3.4 可选：生成 Debian/Ubuntu `.deb`

`.deb` 内使用一个启动脚本设置 DuckDB 搜索路径。目录结构建议为：

```text
deb-root/
  DEBIAN/control
  usr/bin/rust-db-client
  usr/lib/rust-db-client/rust-db-client-bin
  usr/lib/rust-db-client/libduckdb.so
  usr/share/applications/rust-db-client.desktop
  usr/share/icons/hicolor/512x512/apps/rust-db-client.png
```

`DEBIAN/control` 示例：

```text
Package: rust-db-client
Version: 0.1.0
Section: devel
Priority: optional
Architecture: amd64
Maintainer: YOUR NAME <you@example.com>
Description: Cross-platform database client written in Rust
```

`usr/bin/rust-db-client` 内容：

```bash
#!/bin/sh
export LD_LIBRARY_PATH="/usr/lib/rust-db-client${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec /usr/lib/rust-db-client/rust-db-client-bin "$@"
```

设置权限并打包：

```bash
chmod 755 deb-root/usr/bin/rust-db-client
chmod 755 deb-root/usr/lib/rust-db-client/rust-db-client-bin
dpkg-deb --build --root-owner-group deb-root \
  dist/linux/rust-db-client_0.1.0_amd64.deb
```

在干净的 Ubuntu 虚拟机中用 `sudo apt install ./rust-db-client_0.1.0_amd64.deb` 安装并卸载测试。

## 4. Windows 10/11 x64

必须在 Windows x64 上使用 MSVC 工具链构建。安装：

- Visual Studio 2022 Build Tools，勾选“使用 C++ 的桌面开发”和 Windows SDK；
- Rust stable 的 `x86_64-pc-windows-msvc` 工具链；
- Inno Setup 6（生成安装程序时需要）。

在 PowerShell 中执行：

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup default stable-x86_64-pc-windows-msvc
rustup target add x86_64-pc-windows-msvc

# 先把 duckdb.lib、duckdb.dll、duckdb.h 放到该目录
Get-ChildItem .\target\duckdb-download\local\1.5.4

cargo test --workspace --locked
cargo build -p desktop --release --locked --target x86_64-pc-windows-msvc
```

产物为：

```text
target\x86_64-pc-windows-msvc\release\desktop.exe
```

### 4.1 生成便携 zip

```powershell
$Version = "0.1.0"
$Out = ".\dist\windows\rust-db-client-$Version-windows-x86_64"
Remove-Item .\dist\windows -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Out -ItemType Directory -Force | Out-Null

Copy-Item .\target\x86_64-pc-windows-msvc\release\desktop.exe "$Out\RustDBClient.exe"
Copy-Item .\target\duckdb-download\local\1.5.4\duckdb.dll "$Out\duckdb.dll"
Compress-Archive -Path "$Out\*" -DestinationPath "$Out.zip" -Force
```

`duckdb.dll` 必须和 `RustDBClient.exe` 位于同一目录。可用下面的命令检查依赖：

```powershell
dumpbin /DEPENDENTS "$Out\RustDBClient.exe"
dumpbin /DEPENDENTS "$Out\duckdb.dll"
```

如果输出包含目标机器可能没有的 MSVC Runtime，应在安装包中加入微软官方 Visual C++ 2015-2022 x64 Redistributable，或确认 DuckDB 和 Rust 程序均采用了兼容的静态 CRT 构建。不要从开发机手工复制不明来源的系统 DLL。

### 4.2 用 Inno Setup 生成安装程序

创建 `packaging/windows/installer.iss`：

```iss
#define MyAppName "Rust DB Client"
#define MyAppVersion "0.1.0"
#define MyAppExeName "RustDBClient.exe"

[Setup]
AppId={{7E8D9E1F-38E0-4D79-908C-F5B5A7A74A31}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
DefaultDirName={localappdata}\Programs\RustDBClient
DefaultGroupName={#MyAppName}
OutputDir=..\..\dist\windows
OutputBaseFilename=rust-db-client-{#MyAppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\icons\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}

[Files]
Source: "..\..\target\x86_64-pc-windows-msvc\release\desktop.exe"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
Source: "..\..\target\duckdb-download\local\1.5.4\duckdb.dll"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
```

编译安装包：

```powershell
& "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" .\packaging\windows\installer.iss
```

在一台没有 Rust、Visual Studio 和 DuckDB 的干净 Windows 虚拟机中测试安装、启动、升级覆盖和卸载。

正式对外分发时，用受信任的代码签名证书签名 EXE 和安装包，否则 SmartScreen 很可能显示未知发布者。示例：

```powershell
signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 /a .\dist\windows\*.exe
signtool verify /pa .\dist\windows\*.exe
```

## 5. macOS

macOS 应在真实 macOS 或 macOS CI runner 上构建。Apple Silicon 和 Intel 要分别使用对应架构的 DuckDB 动态库。

### 5.1 构建原始二进制

安装 Xcode Command Line Tools 和 Rust：

```bash
xcode-select --install
rustup toolchain install stable
rustup default stable
```

Apple Silicon 构建：

```bash
rustup target add aarch64-apple-darwin
# 将 arm64 的 libduckdb.dylib 和 duckdb.h 放入 target/duckdb-download/local/1.5.4
cargo test --workspace --locked
cargo build -p desktop --release --locked --target aarch64-apple-darwin
```

Intel 构建：

```bash
rustup target add x86_64-apple-darwin
# 换成 x86_64 的 libduckdb.dylib 和 duckdb.h；不能继续使用 arm64 版本
cargo build -p desktop --release --locked --target x86_64-apple-darwin
```

分别检查架构与依赖：

```bash
file target/aarch64-apple-darwin/release/desktop
file target/duckdb-download/local/1.5.4/libduckdb.dylib
otool -L target/aarch64-apple-darwin/release/desktop
```

### 5.2 创建 `.app`

以下以 Apple Silicon 为例：

```bash
VERSION=0.1.0
APP="$PWD/dist/macos/Rust DB Client.app"

rm -rf "$PWD/dist/macos"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources"
install -m 755 target/aarch64-apple-darwin/release/desktop "$APP/Contents/MacOS/RustDBClient"
install -m 755 target/duckdb-download/local/1.5.4/libduckdb.dylib "$APP/Contents/Frameworks/libduckdb.dylib"
install -m 644 packaging/icons/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
```

创建 `$APP/Contents/Info.plist`：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Rust DB Client</string>
  <key>CFBundleDisplayName</key><string>Rust DB Client</string>
  <key>CFBundleIdentifier</key><string>com.yourcompany.rustdbclient</string>
  <key>CFBundleExecutable</key><string>RustDBClient</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
```

将二进制中的 DuckDB 引用改成应用包内路径：

```bash
BIN="$APP/Contents/MacOS/RustDBClient"
DYLIB="$APP/Contents/Frameworks/libduckdb.dylib"
OLD_DUCKDB_REF=$(otool -L "$BIN" | awk '/libduckdb\.dylib/{print $1; exit}')
test -n "$OLD_DUCKDB_REF"

install_name_tool -id "@rpath/libduckdb.dylib" "$DYLIB"
install_name_tool -change "$OLD_DUCKDB_REF" "@rpath/libduckdb.dylib" "$BIN"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$BIN"

plutil -lint "$APP/Contents/Info.plist"
otool -L "$BIN"
```

此时 `otool -L` 应显示 `@rpath/libduckdb.dylib`。先用 `open "$APP"` 做本机测试。

### 5.3 签名、公证并生成 DMG

面向其他用户分发时，需要 Apple Developer ID 证书和公证。先签内部动态库，再签应用：

```bash
DEVELOPER_ID="Developer ID Application: YOUR COMPANY (TEAMID)"

codesign --force --timestamp --options runtime --sign "$DEVELOPER_ID" \
  "$APP/Contents/Frameworks/libduckdb.dylib"
codesign --force --timestamp --options runtime --sign "$DEVELOPER_ID" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
spctl --assess --type execute --verbose=4 "$APP"
```

生成 DMG：

```bash
hdiutil create \
  -volname "Rust DB Client" \
  -srcfolder "$APP" \
  -ov -format UDZO \
  "$PWD/dist/macos/rust-db-client-0.1.0-macos-arm64.dmg"

codesign --force --timestamp --sign "$DEVELOPER_ID" \
  "$PWD/dist/macos/rust-db-client-0.1.0-macos-arm64.dmg"
```

先创建一次公证凭据：

```bash
xcrun notarytool store-credentials "rust-db-client-notary" \
  --apple-id "YOUR_APPLE_ID" \
  --team-id "YOUR_TEAM_ID" \
  --password "APP_SPECIFIC_PASSWORD"
```

提交并等待公证，成功后装订票据：

```bash
DMG="$PWD/dist/macos/rust-db-client-0.1.0-macos-arm64.dmg"
xcrun notarytool submit "$DMG" \
  --keychain-profile "rust-db-client-notary" --wait
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"
spctl --assess --type open --context context:primary-signature --verbose=4 "$DMG"
```

Intel 包重复相同步骤，把构建目标和文件名改为 `x86_64-apple-darwin`、`macos-x86_64`。除非确有单一通用包需求，分别发布 arm64 和 x86_64 DMG 更容易验证。若制作 Universal 2，应用二进制和 `libduckdb.dylib` 都必须是 Universal 2，不能只对主程序执行 `lipo`。

## 6. 每次发布的验收清单

1. `Cargo.toml` 版本、安装包版本和文件名一致。
2. `cargo fmt --check`、测试和 Clippy 全部通过。
3. 使用干净机器/虚拟机测试，而不是只在开发机测试。
4. 安装后无需 Rust、Cargo、编译器或手工设置环境变量即可启动。
5. DuckDB、SQLite、MySQL、PostgreSQL、Redis 至少各做一次连接测试。
6. 测试中文显示、文件打开/保存对话框、导入导出和应用重启后的配置读取。
7. 验证配置写入用户目录，不写安装目录，也不再使用开发默认主密钥。
8. Windows 用 `dumpbin` 检查 DLL，Linux 用 `ldd`，macOS 用 `otool -L`，不得有缺失依赖。
9. Windows 验证 Authenticode 签名；macOS 验证 codesign、公证和 stapler；Linux 至少发布 SHA-256。
10. 对最终产物生成校验文件。

Linux/macOS：

```bash
cd dist
find . -type f \( -name '*.zip' -o -name '*.gz' -o -name '*.deb' -o -name '*.AppImage' -o -name '*.dmg' \) \
  -print0 | sort -z | xargs -0 shasum -a 256 > SHA256SUMS
```

Windows PowerShell：

```powershell
Get-ChildItem .\dist -Recurse -File |
  Where-Object { $_.Extension -in '.zip', '.exe' } |
  Get-FileHash -Algorithm SHA256 |
  Format-Table -AutoSize
```

## 7. 推荐的发布顺序

1. 先完成配置目录、主密钥、Windows 子系统和图标四项发布前修改。
2. 在三类原生构建机上运行质量检查并构建 release。
3. 先生成便携包，验证 DuckDB 动态库布局。
4. 再由同一批已验证二进制生成 AppImage/deb、Windows Setup 和 macOS DMG。
5. 完成 Windows 签名和 macOS 签名、公证。
6. 在干净系统做最终冒烟测试。
7. 生成 `SHA256SUMS`，随安装包和本版本变更说明一起发布。
