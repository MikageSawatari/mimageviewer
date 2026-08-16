# Run the complete automated Rust test gate used before a release.
#
# The pack-build-tools feature exposes only the two development helper binaries
# that contain unit tests. Other development binaries remain outside the test
# graph, while every selected target shares one mimageviewer library build.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

Push-Location $repoRoot
try {
    Write-Host '[test-full] cargo test --workspace --features pack-build-tools --no-fail-fast'
    & cargo test --workspace --features pack-build-tools --no-fail-fast
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    # vendor/egui-wgpu is workspace-excluded, so the line above never reaches it. Run its
    # unit tests here, unfiltered: naming one test would silently drop every test added
    # later - which is the failure mode this step exists to prevent.
    Write-Host '[test-full] cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib'
    & cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    # vendor/eframe is workspace-excluded for the same reason. Keep this unfiltered so every
    # scheduler and Windows process regression added to the vendored event-loop fork runs.
    Write-Host '[test-full] cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib'
    & cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    Write-Host '[test-full] PASS'
} finally {
    Pop-Location
}
