$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Target = Join-Path $Root 'target\x86_64-pc-windows-msvc\release\orvyn.exe'
$DuckDb = Join-Path $Root 'target\x86_64-pc-windows-msvc\release\duckdb.dll'
$Iss = Join-Path $Root 'packaging\windows\installer.iss'
$IsccCandidates = @(
  'D:\Program Files (x86)\Inno Setup 6\ISCC.exe',
  (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
  (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
)
$Iscc = $IsccCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not (Test-Path $Target) -or -not (Test-Path $DuckDb)) {
  cargo build -p orvyn --release --locked --target x86_64-pc-windows-msvc
}
if (-not $Iscc) {
  throw 'Inno Setup 6 is required. Install it from https://jrsoftware.org/isinfo.php and rerun this script.'
}
& $Iscc $Iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
Write-Host "Installer created in $Root\dist\windows"
