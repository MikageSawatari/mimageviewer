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
#   1. idle-health analyzer regression tests
#   2. cargo clean --release -p mimageviewer -p mimageviewer-launcher
#      (+ the portable target dir's mimageviewer package)
#   3. build-release.ps1   -> target\release\mimageviewer.exe (launcher) + core
#   4. ISCC                -> installer\Output\mImageViewer_setup.exe
#   5. build-portable.ps1  -> dist\mImageViewer_portable_v<ver>.zip (target-portable)
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
    [switch] $SkipVst3Bridge,
    [switch] $NoSign
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Get-Location).Path
$scripts = Join-Path $repoRoot 'scripts'
$portableTargetDir = Join-Path $repoRoot 'target-portable'

# Code signing is ON by default for distribution builds; pass -NoSign to skip it.
# Assert the signing certificate up front (SimplySign Desktop must be running and
# logged in) so a missing cert fails before the multi-minute clean+build, not
# after. build-release.ps1 / build-portable.ps1 do the actual interleaved signing.
$sign = -not $NoSign
if ($sign) {
    . (Join-Path $scripts 'sign-files.ps1')
    Assert-MivSignReady
}

# The idle-health smoke is a release gate, so its analyzer tests must be part of the
# distribution path rather than relying on a developer remembering a separate command.
Write-Host "[build-dist] (1/5) python scripts\test_analyze_perf.py"
& python (Join-Path $scripts 'test_analyze_perf.py')
if ($LASTEXITCODE -ne 0) { throw ("[build-dist] idle-health analyzer tests failed (exit {0})" -f $LASTEXITCODE) }

# --- 1. Clean the workspace package so the app is rebuilt from current source ---
# NOTE: $ErrorActionPreference='Stop' does NOT stop on a native command's non-zero
# exit in PowerShell 5.1, so check $LASTEXITCODE explicitly. A silently-failed
# clean would let the build reuse a stale fingerprint -- the exact bug this script
# exists to prevent.
Write-Host "[build-dist] (2/5) cargo clean --release -p mimageviewer -p mimageviewer-launcher"
& cargo clean --release -p mimageviewer -p mimageviewer-launcher
if ($LASTEXITCODE -ne 0) { throw ("[build-dist] cargo clean (workspace) failed (exit {0})" -f $LASTEXITCODE) }
Write-Host "[build-dist]       cargo clean --release --target-dir target-portable -p mimageviewer"
& cargo clean --release --target-dir $portableTargetDir -p mimageviewer
if ($LASTEXITCODE -ne 0) { throw ("[build-dist] cargo clean (portable) failed (exit {0})" -f $LASTEXITCODE) }

# --- 2. Core + launcher (fresh, since cleaned above) ---
$releaseArgs = @()
if ($SkipVst3Bridge) { $releaseArgs += '-SkipVst3Bridge' }
if ($sign) { $releaseArgs += '-Sign' }
Write-Host ("[build-dist] (3/5) build-release.ps1 {0}" -f ($releaseArgs -join ' '))
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
Write-Host ("[build-dist] (4/5) {0} installer\mimageviewer.iss" -f $isccPath)
& $isccPath (Join-Path $repoRoot 'installer\mimageviewer.iss')
if ($LASTEXITCODE -ne 0) { throw ("[build-dist] ISCC failed (exit {0})" -f $LASTEXITCODE) }

if ($sign) {
    # Sign the installer itself. The launcher inside it was already signed in
    # step 2 (build-release), before Inno embedded it.
    $setupExe = Join-Path $repoRoot 'installer\Output\mImageViewer_setup.exe'
    Write-Host ("[build-dist]       signing {0}" -f $setupExe)
    Invoke-MivSign -Files @($setupExe) -Verify
}

# --- 4. Portable (into target-portable; its app package was cleaned above) ---
$portableArgs = @()
if ($sign) { $portableArgs += '-Sign' }
Write-Host "[build-dist] (5/5) build-portable.ps1"
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $scripts 'build-portable.ps1') @portableArgs
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
