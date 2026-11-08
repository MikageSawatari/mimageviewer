# Developer helper: install the locally built 3D photo depth model add-on
# (tools/depth_model/build_depth_addon.py output) without the release download.
# Produces the same layout as the in-app installer:
#   <DataDir>\addons\depth\active.json
#   <DataDir>\addons\depth\packs\<model_id>\{onnx, LICENSE, NOTICE, depth-model-index.json, INSTALL_OK}
# Default DataDir is %APPDATA%\mimageviewer (the normal profile).
param(
    [string]$Source = (Join-Path $PSScriptRoot '..\target\depth-addon'),
    [string]$DataDir = (Join-Path $env:APPDATA 'mimageviewer')
)
$ErrorActionPreference = 'Stop'

$indexPath = Join-Path $Source 'depth-model-index.json'
if (-not (Test-Path -LiteralPath $indexPath)) {
    throw "Index not found: $indexPath (run tools/depth_model/build_depth_addon.py first)"
}
$index = Get-Content -LiteralPath $indexPath -Raw | ConvertFrom-Json
$modelId = [string]$index.model_id
if ([string]::IsNullOrWhiteSpace($modelId)) { throw 'model_id is empty in the index' }

$root = Join-Path $DataDir 'addons\depth'
$pack = Join-Path $root ("packs\" + $modelId)
New-Item -ItemType Directory -Force -Path $pack | Out-Null

foreach ($file in $index.files) {
    $src = Join-Path $Source $file.name
    if (-not (Test-Path -LiteralPath $src)) { throw "Missing source file: $src" }
    $dst = Join-Path $pack $file.name
    Write-Host "Copying $($file.name) ..."
    Copy-Item -LiteralPath $src -Destination $dst -Force
    $len = (Get-Item -LiteralPath $dst).Length
    if ($len -ne [int64]$file.bytes) { throw "Size mismatch for $($file.name): $len" }
    $hash = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash
    if ($hash -ne ([string]$file.sha256).ToUpperInvariant()) { throw "SHA-256 mismatch for $($file.name)" }
}
Copy-Item -LiteralPath $indexPath -Destination (Join-Path $pack 'depth-model-index.json') -Force

$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText((Join-Path $pack 'INSTALL_OK'), "1`n", $utf8)
$pointer = '{"schema":1,"model_id":"' + $modelId + '"}'
[System.IO.File]::WriteAllText((Join-Path $root 'active.json'), $pointer, $utf8)

Write-Host "Installed depth add-on $modelId to $root"
