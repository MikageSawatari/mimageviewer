# Build the development-only Susie crash plugin and stage it with test files.
#
# The plugin exists to check that a plugin crash is survivable: the worker dies,
# and the question is whether later requests still succeed. Without a plugin
# that crashes on demand there was no way to exercise that path, so it had gone
# unverified. See docs/susie-crash-plugin.md.
#
# The plugin goes to the data dir; the test files go somewhere browsable. They
# are deliberately separate -- the plugin folder is scanned for .spi at startup
# and is not a place to keep images.
#
# Usage:
#   .\scripts\setup-susie-crash-plugin.ps1                      # default locations
#   .\scripts\setup-susie-crash-plugin.ps1 -DataDir <path>      # isolated data dir
#   .\scripts\setup-susie-crash-plugin.ps1 -SampleDir <path>    # where to put the files
#   .\scripts\setup-susie-crash-plugin.ps1 -Remove              # take it all back out

[CmdletBinding()]
param(
    [string] $DataDir,
    [string] $SampleDir = 'C:\tmp\miv-susie-crash-test',
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
    # Sample files from either location, including the plugin folder where an
    # earlier version of this script put them by mistake.
    foreach ($dir in @($SampleDir, $pluginDir)) {
        Get-ChildItem -LiteralPath $dir -Filter '*.miv-crashtest' -ErrorAction SilentlyContinue |
            ForEach-Object {
                Remove-Item -LiteralPath $_.FullName -Force
                Write-Host ("[susie-crash] removed {0}" -f $_.FullName)
            }
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

# A running mImageViewer holds the .spi open in its worker processes. Skip the
# copy when the staged plugin already matches, and say so rather than failing
# when it does not: the sample files are still worth writing.
$needsCopy = $true
if (Test-Path $spi) {
    if ((Get-FileHash -LiteralPath $built).Hash -eq (Get-FileHash -LiteralPath $spi).Hash) {
        $needsCopy = $false
        Write-Host '[susie-crash] plugin already staged and identical'
    }
}
if ($needsCopy) {
    try {
        Copy-Item -LiteralPath $built -Destination $spi -Force
    } catch [System.IO.IOException] {
        Write-Warning ('[susie-crash] cannot replace {0} while mImageViewer is running.' -f $spi)
        Write-Warning '[susie-crash] close it and re-run to pick up the newer plugin.'
    }
}

# Clean up sample files an earlier version of this script left in the plugin
# folder, so the .spi scan does not have to step over them.
Get-ChildItem -LiteralPath $pluginDir -Filter '*.miv-crashtest' -ErrorAction SilentlyContinue |
    ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force }

# The behaviour comes from the first line of the file, not its name, because
# GetPicture receives the bytes rather than the path. The names match the
# contents only so the folder is readable. Ordering matters when browsing:
# the ok files sit on both sides of the crash so it is visible whether the
# ones after it still load.
$samples = [ordered]@{
    '01-ok-before.miv-crashtest'    = 'MIVOK'
    '02-crash-always.miv-crashtest' = 'MIVCRASH'
    '03-ok-after.miv-crashtest'     = 'MIVOK'
    '04-ok-after.miv-crashtest'     = 'MIVOK'
    '05-crash-half.miv-crashtest'   = 'MIVHALF'
    '06-ok-after.miv-crashtest'     = 'MIVOK'
    '07-crash-support.miv-crashtest' = 'MIVSUPPORTCRASH'
    '08-ok-after.miv-crashtest'     = 'MIVOK'
}
New-Item -ItemType Directory -Force $SampleDir | Out-Null
foreach ($name in $samples.Keys) {
    $path = Join-Path $SampleDir $name
    Set-Content -LiteralPath $path -Value $samples[$name] -Encoding ascii -NoNewline
}

Write-Host ''
Write-Host '[susie-crash] plugin:'
Write-Host ("    {0}" -f $spi)
Write-Host '[susie-crash] test files:'
Write-Host ("    {0}" -f (Resolve-Path $SampleDir).Path)
Write-Host ''
Write-Host 'Next:'
Write-Host '  1. restart mImageViewer (workers load plugins once, at startup)'
Write-Host ("  2. open {0} in the grid" -f (Resolve-Path $SampleDir).Path)
Write-Host ''
Write-Host 'The 01 file should show a green square. 02 kills a worker.'
Write-Host 'The question is whether 03 and 04 still load after that.'
