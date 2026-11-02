# Inspect the embedded source marker without ever launching the worker.
param([string] $ValidateRepo)
. (Join-Path $PSScriptRoot 'sign-files.ps1')

function Assert-MivVlmIdentityAncestors {
    param([string] $Path)
    $current = [System.IO.Path]::GetFullPath($Path)
    while ($current) {
        if (((Get-Item -LiteralPath $current -Force -ErrorAction Stop).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "[vlm-identity] Reparse-point identity path forbidden: $current"
        }
        $current = [System.IO.Path]::GetDirectoryName($current)
    }
}

function Get-MivVlmWorkerSourceHash {
    param([Parameter(Mandatory = $true)] [string] $SourceRoot)
    $hash = & (Join-Path $PSScriptRoot '..\crates\vlm-worker\tools\source-identity.ps1') -SourceRoot $SourceRoot
    if ($hash -notmatch '^[0-9a-f]{64}$') { throw '[vlm-identity] Invalid source hash' }
    return $hash
}

function Assert-MivVlmWorkerIdentity {
    param([Parameter(Mandatory = $true)] [string] $RepoRoot)
    $worker = Join-Path $RepoRoot 'vendor\vlm-worker\mimageviewer-vlm.exe'
    $recover = 'Restore the pinned vendor/llama.cpp and Vulkan SDK, then run scripts/build-vlm-worker.ps1. See crates/vlm-worker/README.md. Only the current-source vendor worker may be reused.'
    if (-not (Test-Path -LiteralPath $worker -PathType Leaf)) {
        throw "[vlm-identity] Vendor worker is missing: $worker. $recover"
    }
    Assert-MivVlmIdentityAncestors $worker
    if (-not (Test-MivPeFile $worker)) { throw "[vlm-identity] Vendor worker must be a regular PE: $worker. $recover" }
    $expected = Get-MivVlmWorkerSourceHash -SourceRoot (Join-Path $RepoRoot 'crates\vlm-worker')
    $image = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($worker))
    $markers = [regex]::Matches($image, 'MIV_VLM_SOURCE_SHA256:([0-9a-f]{64})(?![0-9a-f])')
    if ($markers.Count -ne 1 -or $markers[0].Groups[1].Value -ne $expected) {
        throw "[vlm-identity] Missing/stale/duplicate source identity in $worker (expected $expected). $recover"
    }
    Write-Host "[vlm-identity] current vendor worker verified: $expected"
}

if ($ValidateRepo) {
    $ErrorActionPreference = 'Stop'
    Assert-MivVlmWorkerIdentity -RepoRoot $ValidateRepo
}
