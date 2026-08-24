# Build the development-only Susie crash plugin and stage it with test files.
#
# The plugin exists to check that a plugin crash is survivable: the worker dies,
# and the question is whether later requests still succeed. Without a plugin
# that crashes on demand there was no way to exercise that path, so it had gone
# unverified. See docs/susie-crash-plugin.md.
#
# Usage:
#   .\scripts\setup-susie-crash-plugin.ps1                 # stage into the normal data dir
#   .\scripts\setup-susie-crash-plugin.ps1 -DataDir <path> # stage into an isolated one
#   .\scripts\setup-susie-crash-plugin.ps1 -Remove         # take it back out

[CmdletBinding()]
param(
    [string] $DataDir,
    [switch] $Remove
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if (-not $DataDir) {
    $DataDir = if ($env:APPDATA) {
        Join-Path $env:APPDATA 'mimageviewer'
    } else {
        throw 'APPDATA is not set; pass -DataDir explicitly.'
    }
}
$pluginDir = Join-Path $DataDir 'susie_plugins'
$spi = Join-Path $pluginDir 'miv-crash-test.spi'

if ($Remove) {
    if (Test-Path $spi) {
        Remove-Item -LiteralPath $spi -Force
        Write-Host "[susie-crash] removed $spi"
    }
    Get-ChildItem -LiteralPath $pluginDir -Filter '*.miv-crashtest' -ErrorAction SilentlyContinue |
        ForEach-Object {
            Remove-Item -LiteralPath $_.FullName -Force
            Write-Host ("[susie-crash] removed {0}" -f $_.FullName)
        }
    Write-Host '[susie-crash] done. Restart mImageViewer so the workers reload.'
    return
}

Push-Location $repoRoot
try {
    Write-Host '[susie-crash] building the 32-bit plugin'
    & cargo build --release --target i686-pc-windows-msvc -p mimageviewer-susie-crash-plugin
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}

$built = Join-Path $repoRoot 'target\i686-pc-windows-msvc\release\mimageviewer_susie_crash_plugin.dll'
if (-not (Test-Path $built)) { throw "[susie-crash] build produced no DLL: $built" }

New-Item -ItemType Directory -Force $pluginDir | Out-Null
Copy-Item -LiteralPath $built -Destination $spi -Force
Write-Host "[susie-crash] staged $spi"

# The behaviour comes from the first line of the file, not its name, because
# GetPicture receives the bytes rather than the path. The names match the
# contents only so the folder is readable.
$samples = @{
    'ok.miv-crashtest'              = 'MIVOK'
    'crash-always.miv-crashtest'    = 'MIVCRASH'
    'crash-half.miv-crashtest'      = 'MIVHALF'
    'crash-support.miv-crashtest'   = 'MIVSUPPORTCRASH'
    'ok-second.miv-crashtest'       = 'MIVOK'
    'ok-third.miv-crashtest'        = 'MIVOK'
}
foreach ($name in $samples.Keys) {
    $path = Join-Path $pluginDir $name
    Set-Content -LiteralPath $path -Value $samples[$name] -Encoding ascii -NoNewline
    Write-Host ("[susie-crash] wrote {0}" -f $name)
}

Write-Host ''
Write-Host '[susie-crash] staged. Next:'
Write-Host "  1. copy the .miv-crashtest files somewhere mImageViewer will browse"
Write-Host "  2. restart mImageViewer (workers load plugins once, at startup)"
Write-Host "  3. open the folder and watch the thumbnails"
Write-Host ''
Write-Host '  ok*.miv-crashtest should show a green square; crash-* should kill a worker.'
Write-Host '  The question is whether the ok files still load after that.'
