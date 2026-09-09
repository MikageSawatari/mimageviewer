# Run the complete automated Rust test gate used before a release.
#
# The pack-build-tools feature exposes only the two development helper binaries
# that contain unit tests. Other development binaries remain outside the test
# graph, while every selected target shares one mimageviewer library build.

[CmdletBinding()]
param(
    # Process-local WER dialog suppression for unattended release gates.
    [switch] $SuppressCrashDialogs
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
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
        Write-Host '[test-full] cargo test --workspace --features pack-build-tools --no-fail-fast'
        & cargo test --workspace --features pack-build-tools --no-fail-fast
        $gateExit = $LASTEXITCODE

        if ($gateExit -eq 0) {
            # vendor/egui is workspace-excluded, so exercise the locally patched input API
            # explicitly and keep this unfiltered as new regressions are added.
            Write-Host '[test-full] cargo test --manifest-path vendor/egui/Cargo.toml --lib'
            & cargo test --manifest-path vendor/egui/Cargo.toml --lib
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # vendor/egui-wgpu is workspace-excluded, so the line above never reaches it. Run its
            # unit tests here, unfiltered: naming one test would silently drop every test added
            # later - which is the failure mode this step exists to prevent.
            Write-Host '[test-full] cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib'
            & cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib
            $gateExit = $LASTEXITCODE
        }

        if ($gateExit -eq 0) {
            # vendor/eframe is workspace-excluded for the same reason. Keep this unfiltered so every
            # scheduler and Windows process regression added to the vendored event-loop fork runs.
            Write-Host '[test-full] cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib'
            & cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib
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
