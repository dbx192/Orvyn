#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
app="dist/macos/Orvyn.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks"
cargo build -p orvyn --release --locked
cp target/release/orvyn "$app/Contents/MacOS/Orvyn"
cp packaging/duckdb-download/local/1.5.4/libduckdb.dylib "$app/Contents/Frameworks/"
# Replace the actual load command, including absolute build-machine references.
otool -L "$app/Contents/MacOS/Orvyn" | awk '/duckdb.*dylib/ {print $1}' | while IFS= read -r library; do
    install_name_tool -change "$library" '@executable_path/../Frameworks/libduckdb.dylib' "$app/Contents/MacOS/Orvyn"
done
install_name_tool -id '@rpath/libduckdb.dylib' "$app/Contents/Frameworks/libduckdb.dylib"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Orvyn</string>
<key>CFBundleIdentifier</key><string>io.orvyn.desktop</string>
<key>CFBundleName</key><string>Orvyn</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
</dict></plist>
EOF
codesign --force --deep --sign - "$app"
otool -L "$app/Contents/MacOS/Orvyn"
hdiutil create -volname Orvyn -srcfolder "$app" -ov -format UDZO "dist/macos/Orvyn-${version}-$(uname -m).dmg"
