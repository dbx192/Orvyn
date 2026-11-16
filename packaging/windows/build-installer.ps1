$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Target = Join-Path $Root 'target\x86_64-pc-windows-msvc\release\orvyn.exe'
$DuckDb = Join-Path $Root 'target\x86_64-pc-windows-msvc\release\duckdb.dll'
$DuckDbSource = Join-Path $Root 'packaging\duckdb-download\local\1.5.4\duckdb.dll'
$Iss = Join-Path $Root 'packaging\windows\installer.iss'
$Icon = Join-Path $Root 'packaging\icons\icon.ico'
$IsccCandidates = @(
  'D:\Program Files (x86)\Inno Setup 6\ISCC.exe',
  (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
  (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
)
$Iscc = $IsccCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not (Test-Path $Icon)) {
  throw "Windows icon not found: $Icon. Put the final multi-size icon.ico in packaging\icons."
}
$IconHash = (Get-FileHash -Algorithm SHA256 $Icon).Hash.Substring(0, 12).ToLowerInvariant()
$Version = (Get-Content (Join-Path $Root 'Cargo.toml') | Select-String '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
if ([string]::IsNullOrWhiteSpace($Version)) {
  throw 'Could not read workspace version from Cargo.toml.'
}
cargo build -p orvyn --release --locked --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
if (-not (Test-Path $DuckDbSource)) { throw "DuckDB library not found: $DuckDbSource" }
Copy-Item $DuckDbSource $DuckDb -Force
if (-not $Iscc) {
  throw 'Inno Setup 6 is required. Install it from https://jrsoftware.org/isinfo.php and rerun this script.'
}
& $Iscc "/DMyAppVersion=$Version" "/DMyIconHash=$IconHash" $Iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
Write-Host "Installer created in $Root\dist\windows"
