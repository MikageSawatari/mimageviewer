# Build only the offline native worker; no model or product process is launched.
param([string] $RepoRoot = (Join-Path $PSScriptRoot '..'), [switch] $SkipBuild, [switch] $Rebuild)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'vlm-worker-identity.ps1')
$RepoRoot = [System.IO.Path]::GetFullPath($RepoRoot)
if ($SkipBuild -and $Rebuild) { throw '[vlm-build] -SkipBuild and -Rebuild cannot be combined.' }
if ($SkipBuild) {
    Assert-MivVlmWorkerIdentity -RepoRoot $RepoRoot
    return
}
if (-not $Rebuild) {
    try {
        Assert-MivVlmWorkerIdentity -RepoRoot $RepoRoot
        Write-Host '[vlm-build] Reusing the current-source worker; CMake and SDK are not needed.'
        return
    } catch {
        Write-Host "[vlm-build] Worker requires a rebuild: $($_.Exception.Message)"
    }
}
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    throw '[vlm-build] CMake missing. Restore the verified backup with bootstrap-vendor.sh, or install native build tools. See crates/vlm-worker/README.md.'
}
if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot 'vendor\llama.cpp\CMakeLists.txt') -PathType Leaf)) {
    throw '[vlm-build] Pinned llama.cpp source missing. Restore the verified worker backup first; otherwise use scripts/setup-llama-cpp.ps1 (checksum required) and rebuild. See crates/vlm-worker/README.md.'
}
Assert-MivVlmIdentityAncestors (Join-Path $RepoRoot 'vendor\llama.cpp\CMakeLists.txt')
$outputDirectory = Join-Path $RepoRoot 'vendor\vlm-worker'
$existingAncestor = $outputDirectory
while (-not (Test-Path -LiteralPath $existingAncestor)) { $existingAncestor = Split-Path -Parent $existingAncestor }
Assert-MivVlmIdentityAncestors $existingAncestor
$existingWorker = Get-Item -LiteralPath (Join-Path $outputDirectory 'mimageviewer-vlm.exe') -Force -ErrorAction SilentlyContinue
if ($existingWorker) { Assert-MivVlmIdentityAncestors $existingWorker.FullName }
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
# Reassembly/reuse has exactly the same gate as a fresh build.
Assert-MivVlmWorkerIdentity -RepoRoot $RepoRoot
