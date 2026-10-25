# Run the complete automated Rust test gate used before a release.
#
# The pack-build-tools feature exposes only the two development helper binaries
# that contain unit tests. Other development binaries remain outside the test
# graph, while every selected target shares one mimageviewer library build.
# This is a noninteractive gate: the live eframe run_native test is #[ignore].
# Headless unit tests with message-only/hidden HWNDs or fake input stay enabled.
# Run only the approved live suite via scripts/test-live-windows.ps1 separately;
# record unperformed live checks as NOT RUN, never as an automated-gate PASS.

[CmdletBinding()]
param(
    # Process-local WER dialog suppression for unattended release gates.
    [switch] $SuppressCrashDialogs
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
. (Join-Path $PSScriptRoot 'vst3-host-vcrt.ps1')
# Handler tests start the vendor host directly, before distribution CMake runs.
# Fail with recovery instructions rather than importing/copying runtime caches.
Assert-MivVst3HostVcrt -RepoRoot $repoRoot
$restoreErrorMode = $false
$modeBefore = [uint32]0
$gateExit = 1

try {
    if ($SuppressCrashDialogs) {
        Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;

public static class MivTestErrorMode
{
    [DllImport("kernel32.dll")]
    public static extern uint SetErrorMode(uint mode);

    [DllImport("kernel32.dll")]
    public static extern uint GetErrorMode();
}
'@
        $requiredMode = [uint32]0x0001 -bor [uint32]0x0002
        $modeBefore = [MivTestErrorMode]::GetErrorMode()
        $requestedMode = $modeBefore -bor $requiredMode
        $previousMode = [MivTestErrorMode]::SetErrorMode($requestedMode)
        $restoreErrorMode = $true
        $effectiveMode = [MivTestErrorMode]::GetErrorMode()
        if (($effectiveMode -band $requiredMode) -ne $requiredMode) {
            throw ('[test-full] required process error-mode bits are not active: before=0x{0:X8}, previous=0x{1:X8}, effective=0x{2:X8}' -f $modeBefore, $previousMode, $effectiveMode)
        }
        Write-Host ('[test-full] crash-dialog suppression active: before=0x{0:X8}, effective=0x{1:X8}' -f $modeBefore, $effectiveMode)
    }

    Push-Location $repoRoot
    try {
        # No cargo-test fallback: process isolation is part of this gate.
        & cargo nextest --version
        $gateExit = $LASTEXITCODE
        if ($gateExit -ne 0) {
            Write-Host '[test-full] cargo-nextest is required. Install with: cargo install cargo-nextest --locked'
        }
        # Explicit root config also applies to the standalone vendor graphs.
        # Ignore any default filter; ignored tests stay ignored, retries stay zero.
        $nextestArgs = @('--config-file', (Join-Path $repoRoot '.config/nextest.toml'),
            '--profile', 'default', '--ignore-default-filter', '--run-ignored', 'default',
            '--retries', '0', '--no-fail-fast', '--no-tests', 'fail')

        if ($gateExit -eq 0) {
            Write-Host '[test-full] cargo nextest run --workspace --features pack-build-tools'
            & cargo nextest run --workspace --features pack-build-tools @nextestArgs
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # nextest does not execute doctests. Preserve the original workspace
            # doc scope and features, including crates with zero runnable examples.
            Write-Host '[test-full] cargo test --workspace --features pack-build-tools --doc --no-fail-fast'
            & cargo test --workspace --features pack-build-tools --doc --no-fail-fast
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # vendor/egui is workspace-excluded, so exercise the locally patched input API
            # explicitly and keep this unfiltered as new regressions are added.
            Write-Host '[test-full] cargo nextest run --manifest-path vendor/egui/Cargo.toml --lib'
            & cargo nextest run --manifest-path vendor/egui/Cargo.toml --lib @nextestArgs
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # vendor/epaint is workspace-excluded as well. Its 1.443 system-font color
            # regressions only run here; keep the run unfiltered with color fonts enabled.
            Write-Host '[test-full] cargo nextest run --manifest-path vendor/epaint/Cargo.toml --features default_fonts,color_fonts --lib'
            & cargo nextest run --manifest-path vendor/epaint/Cargo.toml --features default_fonts,color_fonts --lib @nextestArgs
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # vendor/egui-winit is workspace-excluded. Exercise IME key routing and
            # trace regressions without a filter, including future tests.
            # Cargo requires this excluded package's standalone graph for its
            # dev-dependencies; its patches point at the formal vendor crates.
            Write-Host '[test-full] cargo nextest run --manifest-path vendor/egui-winit/Cargo.toml --lib'
            & cargo nextest run --manifest-path vendor/egui-winit/Cargo.toml --lib @nextestArgs
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # vendor/egui-wgpu is workspace-excluded, so the line above never reaches it. Run its
            # unit tests here, unfiltered: naming one test would silently drop every test added
            # later - which is the failure mode this step exists to prevent.
            Write-Host '[test-full] cargo nextest run --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib'
            & cargo nextest run --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib @nextestArgs
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # vendor/eframe is workspace-excluded for the same reason. Keep this unfiltered so every
            # nonignored pure scheduler regression in the vendored event-loop fork runs.
            # The real-window process test remains ignored (separate approved live entry).
            Write-Host '[test-full] cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib'
            & cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib @nextestArgs
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # Keep the second native backend's lifecycle/output contracts in the
            # automated graph; real-window tests remain ignored as above.
            Write-Host '[test-full] cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,glow --lib'
            & cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,glow --lib @nextestArgs
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            Write-Host '[test-full] PASS'
        }
    } finally {
        Pop-Location
    }
} finally {
    if ($restoreErrorMode) {
        [void][MivTestErrorMode]::SetErrorMode($modeBefore)
        $restoredMode = [MivTestErrorMode]::GetErrorMode()
        if ($restoredMode -ne $modeBefore) {
            throw ('[test-full] process error mode was not restored: expected=0x{0:X8}, actual=0x{1:X8}' -f $modeBefore, $restoredMode)
        }
        Write-Host ('[test-full] process error mode restored: 0x{0:X8}' -f $restoredMode)
    }
}

exit $gateExit
