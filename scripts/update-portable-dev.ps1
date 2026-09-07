# Keep a persistent isolated portable build for development, at
# target\portable-dev, and refresh only its executable.
#
# This is deliberately NOT prepare-portable-smoke.ps1. That script builds a
# disposable tree and wipes its data directory on every run, which is correct
# for automated UI smoke tests -- they must never run against real settings.
# Using it to refresh a build cost a completed index once. The rule here is that
# the script you run to get a newer build cannot delete your data.
#
# Usage:
#   .\scripts\update-portable-dev.ps1            # build, refresh exe, keep data
#   .\scripts\update-portable-dev.ps1 -SkipBuild # refresh exe from the last build
#   .\scripts\update-portable-dev.ps1 -Seed      # also seed data from %APPDATA%
#                                                # (refuses if data already exists)

[CmdletBinding()]
param(
    [switch] $SkipBuild,
    [switch] $Seed
)
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$sandbox = Join-Path $repoRoot 'target\portable-dev'
$dataDir = Join-Path $sandbox 'data'

Push-Location $repoRoot
try {
    if (-not $SkipBuild) {
        & (Join-Path $PSScriptRoot 'build-portable.ps1')
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

    $version = $null
    foreach ($line in (Get-Content (Join-Path $repoRoot 'Cargo.toml'))) {
        if ($line -match '^version\s*=\s*"([^"]+)"') { $version = $Matches[1]; break }
    }
    if (-not $version) { throw '[portable-dev] could not parse package version' }

    $source = Join-Path $repoRoot "dist\mImageViewer_portable_v$version"
    if (-not (Test-Path -LiteralPath $source -PathType Container)) {
        throw "[portable-dev] portable package not found: $source"
    }

    New-Item -ItemType Directory -Force -Path $sandbox | Out-Null

    # Copy everything the package holds EXCEPT data. Overwriting the executable
    # and its native dependencies is the point; the data directory is the one
    # thing this script must never touch.
    foreach ($entry in Get-ChildItem -LiteralPath $source -Force) {
        if ($entry.PSIsContainer -and $entry.Name -eq 'data') { continue }
        Copy-Item -LiteralPath $entry.FullName -Destination $sandbox -Recurse -Force
    }

    $hadData = Test-Path -LiteralPath $dataDir
    if ($Seed) {
        if ($hadData) {
            throw "[portable-dev] data already exists, refusing to overwrite: $dataDir`n" +
                  "  seed a fresh one with: .\scripts\seed-portable-data.ps1 -Destination '$dataDir' -Force"
        }
        & (Join-Path $PSScriptRoot 'seed-portable-data.ps1') -Destination $dataDir
    } elseif (-not $hadData) {
        New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
        Write-Host '[portable-dev] created an empty data directory.'
        Write-Host '[portable-dev] to start from your real settings, run with -Seed.'
    }

    $exe = Join-Path $sandbox 'mimageviewer.exe'
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw "[portable-dev] executable missing after copy: $exe"
    }

    Write-Host ''
    Write-Host "[portable-dev] ready: $exe"
    Write-Host "[portable-dev] data kept at: $dataDir"
    Write-Host '[portable-dev] this build has its own single-instance mutex, so it can'
    Write-Host '[portable-dev] run beside the installed mImageViewer.'
}
finally {
    Pop-Location
}
