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

    Write-Host '[test-full] PASS'
} finally {
    Pop-Location
}
