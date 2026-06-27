# mImageViewer distribution build orchestrator (PowerShell).
#
# Runs the full release build as ONE reliable command. The key guarantee: it
# `cargo clean`s the workspace package (release profile) FIRST, so the app code
# is ALWAYS recompiled from current source. This defeats cargo's occasional
# false "up-to-date" for the release build, which has shipped stale binaries
# (see CLAUDE.md "feedback_release_stale_core_cache"). Dependencies stay cached,
# so the clean only costs the app recompile (~4 min core), not a full rebuild.
#
# Steps:
#   1. cargo clean --release -p mimageviewer -p mimageviewer-launcher
#      (+ the portable target dir's mimageviewer package)
#   2. build-release.ps1   -> target\release\mimageviewer.exe (launcher) + core
#   3. ISCC                -> installer\Output\mImageViewer_setup.exe
#   4. build-portable.ps1  -> dist\mImageViewer_portable_v<ver>.zip (target-portable)
#
# For day-to-day development keep using scripts\build-release.ps1 directly: it is
# the fast incremental build and does NOT clean. This script is only for cutting
# distribution artifacts.
#
# Usage:
#   PS> scripts\build-dist.ps1
#   PS> scripts\build-dist.ps1 -SkipVst3Bridge
#
# Sub-scripts are launched in a child PowerShell (powershell -File) so their
# `exit` ends only the child and the exit code comes back via $LASTEXITCODE;
# this orchestrator then decides whether to continue.

[CmdletBinding()]
param(
    [switch] $SkipVst3Bridge
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Get-Location).Path
$scripts = Join-Path $repoRoot 'scripts'
$portableTargetDir = Join-Path $repoRoot 'target-portable'

# --- 1. Clean the workspace package so the app is rebuilt from current source ---
Write-Host "[build-dist] (1/4) cargo clean --release -p mimageviewer -p mimageviewer-launcher"
& cargo clean --release -p mimageviewer -p mimageviewer-launcher
Write-Host "[build-dist]       cargo clean --release --target-dir target-portable -p mimageviewer"
& cargo clean --release --target-dir $portableTargetDir -p mimageviewer

# --- 2. Core + launcher (fresh, since cleaned above) ---
$releaseArgs = @()
if ($SkipVst3Bridge) { $releaseArgs += '-SkipVst3Bridge' }
Write-Host ("[build-dist] (2/4) build-release.ps1 {0}" -f ($releaseArgs -join ' '))
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $scripts 'build-release.ps1') @releaseArgs
if ($LASTEXITCODE -ne 0) { throw ("[build-dist] build-release.ps1 failed (exit {0})" -f $LASTEXITCODE) }

# --- 3. Installer ---
$isccPath = $null
$isccCmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
if ($isccCmd) { $isccPath = $isccCmd.Source }
if (-not $isccPath) {
    foreach ($c in @(
            'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
            'C:\Program Files\Inno Setup 6\ISCC.exe')) {
        if (Test-Path $c) { $isccPath = $c; break }
    }
}
if (-not $isccPath) { throw "[build-dist] ISCC.exe not found. Install Inno Setup 6." }
Write-Host ("[build-dist] (3/4) {0} installer\mimageviewer.iss" -f $isccPath)
& $isccPath (Join-Path $repoRoot 'installer\mimageviewer.iss')
if ($LASTEXITCODE -ne 0) { throw ("[build-dist] ISCC failed (exit {0})" -f $LASTEXITCODE) }

# --- 4. Portable (into target-portable; its app package was cleaned above) ---
Write-Host "[build-dist] (4/4) build-portable.ps1"
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $scripts 'build-portable.ps1')
if ($LASTEXITCODE -ne 0) { throw ("[build-dist] build-portable.ps1 failed (exit {0})" -f $LASTEXITCODE) }

# --- Summary ---
Write-Host ""
Write-Host "[build-dist] DONE. Distribution artifacts:"
Write-Host ("  single exe : {0}" -f (Join-Path $repoRoot 'target\release\mimageviewer.exe'))
Write-Host ("  installer  : {0}" -f (Join-Path $repoRoot 'installer\Output\mImageViewer_setup.exe'))
$portableZip = Get-ChildItem (Join-Path $repoRoot 'dist') -Filter 'mImageViewer_portable_v*.zip' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($portableZip) { Write-Host ("  portable   : {0}" -f $portableZip.FullName) }
Write-Host ""
Write-Host "Note: the Vector zip (installer + readme) is a normal-release-only step, not built here."
exit 0
