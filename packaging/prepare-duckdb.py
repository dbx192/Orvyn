"""Download the pinned native DuckDB library for the current build host."""
import io
import os
import platform
import urllib.request
import zipfile
from pathlib import Path

VERSION = "1.5.4"
system = platform.system()
machine = platform.machine().lower()
if system == "Windows":
    asset = "libduckdb-windows-amd64.zip"
elif system == "Linux":
    asset = "libduckdb-linux-" + ("arm64" if machine in ("arm64", "aarch64") else "amd64") + ".zip"
elif system == "Darwin":
    asset = "libduckdb-osx-universal.zip"
else:
    raise SystemExit("Unsupported build host: " + system)
url = f"https://github.com/duckdb/duckdb/releases/download/v{VERSION}/{asset}"
# Set ORVYN_GITHUB_PROXY to an empty string to use GitHub directly.
url = os.environ.get("ORVYN_GITHUB_PROXY", "https://gh-proxy.org/") + url
with urllib.request.urlopen(url, timeout=120) as response:
    data = response.read()
destination = Path(__file__).resolve().parent / "duckdb-download/local" / VERSION
destination.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(io.BytesIO(data)) as archive:
    for member in archive.infolist():
        name = Path(member.filename).name
        if name in ("duckdb.h", "duckdb.hpp", "duckdb.dll", "duckdb.lib", "libduckdb.so", "libduckdb.dylib"):
            (destination / name).write_bytes(archive.read(member))
print("Prepared DuckDB " + VERSION + " for " + system)
