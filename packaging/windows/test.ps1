$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$DuckDbDir = Join-Path $Root 'packaging\duckdb-download\local\1.5.4'
if (-not (Test-Path (Join-Path $DuckDbDir 'duckdb.dll'))) {
  throw "duckdb.dll not found: $DuckDbDir"
}
$env:PATH = "$DuckDbDir;$env:PATH"
Set-Location $Root
cargo test --workspace --locked --target x86_64-pc-windows-msvc
