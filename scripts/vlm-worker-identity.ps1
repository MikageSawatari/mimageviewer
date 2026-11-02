# Inspect the embedded source marker without ever launching the worker.
param([string] $ValidateRepo, [string] $RestoreBackupRepo)
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
    param([Parameter(Mandatory = $true)] [string] $RepoRoot, [string] $WorkerPath)
    $worker = if ($WorkerPath) { $WorkerPath } else { Join-Path $RepoRoot 'vendor\vlm-worker\mimageviewer-vlm.exe' }
    $recover = 'Restore the current-source worker from C:\home\mimageviewer_vendor_backup\vlm-worker first (bootstrap-vendor.sh validates before publication). Otherwise use scripts/setup-llama-cpp.ps1 for the pinned source, install the Vulkan SDK, and run scripts/build-vlm-worker.ps1. See crates/vlm-worker/README.md.'
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

function Restore-MivVlmWorkerBackup {
    param([Parameter(Mandatory = $true)] [string] $RepoRoot,
        [string] $BackupPath = 'C:\home\mimageviewer_vendor_backup\vlm-worker\mimageviewer-vlm.exe')
    $destination = Join-Path ([System.IO.Path]::GetFullPath($RepoRoot)) 'vendor\vlm-worker\mimageviewer-vlm.exe'
    if (Test-Path -LiteralPath $destination) {
        Assert-MivVlmWorkerIdentity -RepoRoot $RepoRoot
        return
    }
    if (-not (Test-Path -LiteralPath $BackupPath -PathType Leaf)) {
        throw '[vlm-restore] Backup missing. Use scripts/setup-llama-cpp.ps1 (requires a verified archive checksum), Vulkan SDK and scripts/build-vlm-worker.ps1; see crates/vlm-worker/README.md.'
    }
    Assert-MivVlmIdentityAncestors $BackupPath
    $directory = Split-Path -Parent $destination
    # Check every existing ancestor before creating the missing vendor folders.
    $ancestor = $directory
    while (-not (Test-Path -LiteralPath $ancestor)) { $ancestor = Split-Path -Parent $ancestor }
    Assert-MivVlmIdentityAncestors $ancestor
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    Assert-MivVlmIdentityAncestors $directory
    $candidate = Join-Path $directory ('.restore-' + [guid]::NewGuid().ToString('N') + '.exe')
    try {
        [System.IO.File]::Copy($BackupPath, $candidate, $false)
        Assert-MivVlmWorkerIdentity -RepoRoot $RepoRoot -WorkerPath $candidate
        # Atomic publication, without replacing an existing file from another run.
        [System.IO.File]::Move($candidate, $destination)
        Write-Host '[vlm-restore] Published the verified backup worker as a real file.'
    } finally {
        if (Test-Path -LiteralPath $candidate) { Remove-Item -LiteralPath $candidate -Force }
    }
}

if ($ValidateRepo) {
    $ErrorActionPreference = 'Stop'
    Assert-MivVlmWorkerIdentity -RepoRoot $ValidateRepo
} elseif ($RestoreBackupRepo) {
    $ErrorActionPreference = 'Stop'
    Restore-MivVlmWorkerBackup -RepoRoot $RestoreBackupRepo
}
