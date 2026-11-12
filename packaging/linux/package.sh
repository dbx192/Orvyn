#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
arch="$(dpkg --print-architecture)"
pkg="dist/linux/orvyn_${version}_${arch}"
mkdir -p "$pkg/DEBIAN" "$pkg/usr/lib/orvyn" "$pkg/usr/bin" "$pkg/usr/share/applications" "$pkg/usr/share/icons/hicolor/256x256/apps"
cargo build -p orvyn --release --locked
install -m755 target/release/orvyn "$pkg/usr/lib/orvyn/orvyn"
install -m755 packaging/duckdb-download/local/1.5.4/libduckdb.so "$pkg/usr/lib/orvyn/libduckdb.so"
cat > "$pkg/usr/bin/orvyn" <<'EOF'
#!/bin/sh
exec env LD_LIBRARY_PATH="/usr/lib/orvyn${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" /usr/lib/orvyn/orvyn "$@"
EOF
chmod 755 "$pkg/usr/bin/orvyn"
cat > "$pkg/DEBIAN/control" <<EOF
Package: orvyn
Version: $version
Section: database
Priority: optional
Architecture: $arch
Maintainer: Orvyn
Depends: libc6, libstdc++6, libodbc2, libx11-6, libxcb1, libxkbcommon0, libwayland-client0
Description: Desktop database client
EOF
cat > "$pkg/usr/share/applications/orvyn.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Orvyn
Exec=orvyn
Icon=orvyn
Categories=Development;Database;
Terminal=false
EOF
install -m644 packaging/icons/icon.png "$pkg/usr/share/icons/hicolor/256x256/apps/orvyn.png"
ldd "$pkg/usr/lib/orvyn/orvyn"
dpkg-deb --root-owner-group --build "$pkg"
