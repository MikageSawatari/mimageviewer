# User-run standalone eframe window test, separate from unattended gates.
# Agents must not execute this runner: standalone test windows are user-run only.
# -InteractiveApproved asserts prior explicit approval; it never obtains consent.
[CmdletBinding()]
param(
    [switch] $InteractiveApproved
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

Write-Host '[test-live-windows] Opens an eframe application window and uses the desktop.'
Write-Host '[test-live-windows] One test: 320x200 window, synchronous damage/paint ordering, then close.'
Write-Host '[test-live-windows] Allow about 1 minute of desktop use, plus several minutes for a cold build.'
Write-Host '[test-live-windows] Use an isolated test desktop/session; leave keyboard/mouse idle during the run.'
Write-Host '[test-live-windows] Disposable in-process fixtures and loopback IPC only; no mIV profile or real Everything client.'
if (-not $InteractiveApproved) {
    Write-Error 'Prior explicit approval is required. Review the suite and use -InteractiveApproved only after approval.'
    exit 1
}
if ($env:OS -ne 'Windows_NT') {
    Write-Error 'This runner requires Windows.'
    exit 1
}

# An exact allowlist excludes unrelated ignored hardware/manual probes. Zero matches
# fail. No retry, timeout override, default filter, or parallel desktop activity.
$eframeTests = @(
    'native::run::tests::windows_process_test::synchronous_damage_returns_before_outer_paint_gate'
)
$nextestArgs = @('--config-file', (Join-Path $repoRoot '.config/nextest.toml'),
    '--profile', 'default', '--ignore-default-filter', '--run-ignored', 'only',
    '--retries', '0', '--test-threads', '1', '--no-tests', 'fail')
Push-Location $repoRoot
try {
    $filter = ($eframeTests | ForEach-Object { "test(=$_)" }) -join ' | '
    & cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib @nextestArgs -E $filter
    $gateExit = $LASTEXITCODE
} finally {
    Pop-Location
}
exit $gateExit

