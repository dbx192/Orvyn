# Orvyn 1.0.0

[中文](README.md)

Orvyn is a Rust cross-platform desktop database client for connecting to, querying, browsing, and lightly maintaining relational, document, key-value, and search services. Its native egui/eframe UI has no ODBC, JDBC, or sidecar dependency.

## Features

- Create, edit, test, group, delete, import, and export connections. Password saving is opt-in and encrypted locally with ChaCha20-Poly1305.
- Connection timeouts and one retry for transient network failures.
- PostgreSQL, MySQL, and SQL Server TLS modes: Disable, Prefer, Require, Verify CA, and Verify Full, with an optional CA certificate path. Strict modes do not silently fall back to plaintext.
- SQL tabs, execute all/current/selection, cancellation, syntax highlighting, keyword and schema table/column completion, history, and favorites.
- Server-side result paging, sorting, copy, and CSV/JSON/Excel-compatible TSV export. Only visible grid rows are painted; pages range from 25 to 1,000 rows.
- Browse metadata and maintain table data. Views live in a localized Views folder; views and saved queries have context-menu deletion.
- Redis browser and data operations; MongoDB browsing, document queries, and basic CRUD.
- Elasticsearch, Easysearch, Meilisearch, Manticore Search index/JSON queries, plus Cloudflare D1 HTTP SQL.

## Supported Sources

| Family | Sources |
| --- | --- |
| MySQL protocol | MySQL, MariaDB, TiDB, OceanBase, Doris, SelectDB, StarRocks, GoldenDB |
| PostgreSQL protocol | PostgreSQL, TimescaleDB, Redshift, CockroachDB, OpenGauss, GaussDB, KwDb, KingbaseES, Vastbase, HighGo, UXDB |
| Relational/analytics | SQLite, DuckDB, ClickHouse, SQL Server |
| Document/key-value | MongoDB, Redis |
| Search | Elasticsearch, Easysearch, Meilisearch, Manticore Search |
| HTTP SQL | Cloudflare D1 |
| Restricted | TDengine has an HTTP probe only. Access, Oracle, DM, and XuguDB profiles can be saved but have no usable driver. |

The capability matrix disables metadata, DDL, and editing actions that a particular backend does not support.

## Run From Source

Install Rust 1.78 or newer and the C/C++ toolchain for the host platform.

~~~powershell
cargo run -p orvyn --locked
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
~~~

## TLS Policy

Select SSL mode in the connection form. PostgreSQL, MySQL, and SQL Server expose TLS CA certificate path; it stores a file path only, never certificate contents or private keys.

| Mode | Behavior |
| --- | --- |
| Disable | Explicitly disables TLS. |
| Prefer | Negotiates TLS first and may fall back where the protocol permits it. |
| Require | Requires TLS without an additional CA file. |
| Verify CA | Requires TLS and validates the server certificate chain. |
| Verify Full | Also validates the certificate hostname against the connected host. |

Use the database DNS name and a CA file from the organization or cloud provider in strict modes. Supply the appropriate CA for self-signed deployments instead of using Disable.

## Configuration

- Windows: %APPDATA%\Orvyn
- Linux/macOS: $XDG_CONFIG_HOME/orvyn, or orvyn in the working directory when unset

The directory holds profiles, query history, favorites, settings, logs, and encryption material. Override it with ORVYN_CONFIG_DIR and ORVYN_MASTER_KEY. Without the latter, Orvyn creates master.key in the configuration directory; protect both.

## Shared Packaging Preparation

Package on the target operating system. System SDKs, signing, and the DuckDB dynamic library make cross-system desktop packaging unreliable.

The local DuckDB library is not downloaded during builds. Put matching target files in:

~~~text
packaging/duckdb-download/local/1.5.4
~~~

- Windows x64 MSVC: duckdb.lib, duckdb.dll, duckdb.h
- Linux x64: libduckdb.so, duckdb.h
- macOS: libduckdb.dylib, duckdb.h

Restore the correct files after changing DuckDB, architecture, or cleaning target. Use --locked for release builds.

## Windows Installer

Install the Rust MSVC toolchain, Visual Studio Build Tools with Desktop development with C++, and [Inno Setup 6](https://jrsoftware.org/isinfo.php). Prepare the Windows DuckDB files, then run from the repository root:

~~~powershell
rustup target add x86_64-pc-windows-msvc
.\packaging\windows\build-installer.ps1
~~~

The supplied script builds the x64 binary, includes duckdb.dll, invokes Inno Setup, and writes an installer similar to dist\windows\orvyn-1.0.0-windows-x86_64-<icon-hash>-setup.exe. Authenticode-sign it before public distribution.

## Linux .deb

Build on an older baseline such as Ubuntu 22.04:

~~~bash
sudo apt update
sudo apt install -y build-essential pkg-config libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev
rustup target add x86_64-unknown-linux-gnu
cargo build -p orvyn --release --locked --target x86_64-unknown-linux-gnu
~~~

Assemble a Debian package. Its launcher explicitly locates the bundled DuckDB library:

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

An AppImage can reuse this layout through an AppDir and appimagetool; its AppRun needs the same library path. No automated Linux packaging script is included.

## macOS .app and .dmg

Build separately on matching hardware: aarch64-apple-darwin for Apple Silicon and x86_64-apple-darwin for Intel.

~~~bash
xcode-select --install
rustup target add aarch64-apple-darwin # use x86_64-apple-darwin on Intel
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

Check otool -L "$APP/Contents/MacOS/Orvyn" for the DuckDB reference. Public distribution requires Developer ID signing and notarization:

~~~bash
codesign --force --deep --options runtime --sign "Developer ID Application: Your Name (TEAMID)" "$APP"
xcrun notarytool submit dist/macos/Orvyn-1.0.0.dmg --keychain-profile "notary-profile" --wait
xcrun stapler staple dist/macos/Orvyn-1.0.0.dmg
~~~

There is no automated macOS packaging script or .icns icon in this repository.

## License

Licensed under [MIT OR Apache-2.0](Cargo.toml).
