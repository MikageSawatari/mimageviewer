# Build only the offline native worker; no model or product process is launched.
param([string] $RepoRoot = (Join-Path $PSScriptRoot '..'), [switch] $SkipBuild)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'vlm-worker-identity.ps1')
$RepoRoot = [System.IO.Path]::GetFullPath($RepoRoot)
if (-not $SkipBuild) {
    $source = Join-Path $RepoRoot 'crates\vlm-worker'
    $build = Join-Path $source 'build'
    $oldNodeReuse = $env:MSBUILDDISABLENODEREUSE
    try {
        $env:MSBUILDDISABLENODEREUSE = '1'
        & cmake -S $source -B $build -G 'Visual Studio 18 2026' -A x64 -DVLM_REFERENCE_TOOLS=OFF
        if ($LASTEXITCODE -ne 0) { throw "[vlm-build] CMake configure failed: $LASTEXITCODE" }
        & cmake --build $build --config Release --target mimageviewer-vlm -- /m:1 /nr:false
        if ($LASTEXITCODE -ne 0) { throw "[vlm-build] CMake build failed: $LASTEXITCODE" }
    } finally {
        if ($null -eq $oldNodeReuse) { Remove-Item Env:MSBUILDDISABLENODEREUSE -ErrorAction SilentlyContinue }
        else { $env:MSBUILDDISABLENODEREUSE = $oldNodeReuse }
    }
}
# Reassembly/reuse has exactly the same gate as a fresh build.
Assert-MivVlmWorkerIdentity -RepoRoot $RepoRoot
