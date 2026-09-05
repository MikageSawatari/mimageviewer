# Seed an isolated portable data directory from the real %APPDATA%\mimageviewer.
#
# Why this exists: a normal-profile build shares %APPDATA%\mimageviewer with the
# installed mImageViewer, and its single-instance mutex is a compile-time name,
# so --data-dir alone does not let a work-in-progress build run beside the
# installed one. A portable build has its own mutex and its own data directory.
# Seeding that directory from the real one keeps favorites, ratings, tags and a
# partly built index instead of starting over.
#
# The real directory is around 100 GB, almost all of it caches that regenerate,
# so this copies by exclusion rather than wholesale.
#
# Usage:
#   .\scripts\seed-portable-data.ps1
#   .\scripts\seed-portable-data.ps1 -Destination D:\somewhere\data -IncludeFullText
#   .\scripts\seed-portable-data.ps1 -Force        # overwrite a seeded directory

[CmdletBinding()]
param(
    [string] $Destination = (Join-Path (Split-Path -Parent $PSScriptRoot) 'target\portable-smoke\data'),
    [switch] $IncludeFullText,
    [switch] $Force
)
$ErrorActionPreference = 'Stop'

$source = Join-Path $env:APPDATA 'mimageviewer'
if (-not (Test-Path $source)) { throw "source not found: $source" }

# Excluded because they are large and rebuild themselves, or belong to the
# installed build rather than to a data set.
$excludeDirs = @(
    'tensorrt',          # and every tensorrt.* variant, matched by prefix below
    'runtime',           # launcher extraction for a specific version
    'logs'
)
$excludeFiles = @(
    'audio_analysis.db'  # 1.4 GB of waveform analysis, regenerated on demand
)
if (-not $IncludeFullText) {
    # 58 GB of Tantivy segments plus its bookkeeping. Only Ctrl+G needs them,
    # and they rebuild from the favorites that are being copied.
    $excludeDirs += 'fts_index'
    $excludeFiles += 'fts_meta.db'
}

if (Test-Path $Destination) {
    if (-not $Force) {
        throw "destination already exists: $Destination (pass -Force to overwrite)"
    }
    Remove-Item -Recurse -Force $Destination
}
New-Item -ItemType Directory -Force -Path $Destination | Out-Null

$copiedBytes = 0L
$skipped = New-Object System.Collections.ArrayList

foreach ($entry in Get-ChildItem -LiteralPath $source -Force) {
    $isExcludedDir = $entry.PSIsContainer -and (
        ($excludeDirs -contains $entry.Name) -or ($entry.Name -like 'tensorrt*')
    )
    $isExcludedFile = (-not $entry.PSIsContainer) -and ($excludeFiles -contains $entry.Name)
    if ($isExcludedDir -or $isExcludedFile) {
        [void] $skipped.Add($entry.Name)
        continue
    }
    Copy-Item -LiteralPath $entry.FullName -Destination $Destination -Recurse -Force
    if ($entry.PSIsContainer) {
        $copiedBytes += (Get-ChildItem -LiteralPath $entry.FullName -Recurse -File -Force |
            Measure-Object -Property Length -Sum).Sum
    } else {
        $copiedBytes += $entry.Length
    }
}

Write-Host ("seeded {0}" -f $Destination)
Write-Host ("  copied  : {0:N1} GB" -f ($copiedBytes / 1GB))
Write-Host ("  skipped : {0}" -f ($skipped -join ', '))
Write-Host ''
Write-Host 'This copy is independent from now on. Changes here do not reach'
Write-Host '%APPDATA%\mimageviewer, and changes there do not reach here.'
