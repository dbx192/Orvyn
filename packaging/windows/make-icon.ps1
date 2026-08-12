$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Input = Join-Path $Root 'packaging\icons\icon.png'
$Output = Join-Path $Root 'packaging\icons\icon.ico'
python -c "from PIL import Image; Image.open(r'$Input').convert('RGBA').save(r'$Output', sizes=[(256,256),(128,128),(64,64),(48,48),(32,32),(16,16)])"
Write-Host "Created $Output"
