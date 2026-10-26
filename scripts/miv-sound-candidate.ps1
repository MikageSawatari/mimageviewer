# Candidate integrity helpers. No build, approval or signing on import.
param([string] $ValidateCandidate, [string] $WorkspaceRoot)
$candidateValidationRoot = $WorkspaceRoot
. (Join-Path $PSScriptRoot 'effetune-distribution.ps1')
. (Join-Path $PSScriptRoot 'miv-sound-build-contract.ps1')

$MivSoundNotices = @(
    'Contents/Resources/THIRD-PARTY-NOTICES.txt',
    'Contents/Resources/webview/THIRD-PARTY-NOTICES.txt',
    'Contents/Resources/webview/plugins/dsp/NOTICE.txt'
)

function Get-MivSoundTextHash {
    param([string] $Text)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Text)))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Assert-MivSoundSafeDirectory {
    param([string] $Path)
    $ancestor = New-Object IO.DirectoryInfo([IO.Path]::GetFullPath($Path))
    while ($null -ne $ancestor) {
        $item = Get-Item -LiteralPath $ancestor.FullName -Force -ErrorAction SilentlyContinue
        if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "[miv-sound] Reparse point: $($ancestor.FullName)"
        }
        $ancestor = $ancestor.Parent
    }
    if (Test-Path -LiteralPath $Path) { $null = @(Get-MivTreeFiles -Path $Path) }
}

function Get-MivSoundInventory {
    param([string] $Artifact)
    Assert-MivSoundSafeDirectory $Artifact
    $root = [IO.Path]::GetFullPath($Artifact).TrimEnd('\')
    $entries = New-Object 'System.Collections.Generic.SortedDictionary[string,string]' ([StringComparer]::Ordinal)
    $caseNames = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($file in @(Get-MivTreeFiles -Path $root)) {
        $name = $file.FullName.Substring($root.Length + 1).Replace('\', '/')
        if (-not $caseNames.Add($name)) { throw "[miv-sound] Case-colliding artifact path: $name" }
        $entries.Add($name, (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant())
    }
    return ,$entries
}

function Get-MivSoundManifestText {
    param([object] $Entries)
    return (($Entries.Keys | ForEach-Object { "$($Entries[$_])  $_`n" }) -join '')
}

function Read-MivSoundCandidate {
    param([string] $RepoRoot, [string] $Candidate)
    if ($Candidate -cnotmatch '^[a-f0-9]{64}$') { throw '[miv-sound] Candidate must be a SHA-256 ID' }
    $root = Join-Path $RepoRoot "target/miv-sound-candidates/$Candidate"
    Assert-MivSoundSafeDirectory $root
    $record = Get-Content -LiteralPath (Join-Path $root 'candidate.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($record.schema -ne 1 -or $record.id -cne $Candidate -or $record.version -cne $MivSound.VERSION) {
        throw '[miv-sound] Candidate identity/version mismatch'
    }
    $artifact = Join-Path $root 'artifact'
    $entries = Get-MivSoundInventory $artifact
    $manifest = Get-MivSoundManifestText $entries
    if ((Get-MivSoundTextHash $manifest) -cne $Candidate -or $record.manifest -cne $manifest) {
        throw '[miv-sound] Candidate file set/hash mismatch'
    }
    if ((Get-Content -LiteralPath (Join-Path $artifact 'VERSION') -Raw -Encoding UTF8).Trim() -cne $MivSound.VERSION) {
        throw '[miv-sound] Candidate VERSION mismatch'
    }
    foreach ($name in $entries.Keys) {
        if ($name -ne 'VERSION' -and -not $name.StartsWith($MivSound.BUNDLE_NAME + '/', [StringComparison]::Ordinal)) {
            throw "[miv-sound] Unexpected artifact path: $name"
        }
    }
    if (-not $entries.ContainsKey($MivSound.Plugin) -or -not (Test-MivPeFile (Join-Path $artifact $MivSound.Plugin))) {
        throw '[miv-sound] Candidate plugin PE missing/invalid'
    }
    foreach ($notice in $MivSoundNotices) {
        if (-not $entries.ContainsKey($MivSound.BUNDLE_NAME + '/' + $notice)) { throw "[miv-sound] Missing notice: $notice" }
    }
    return @{ Root = $root; Artifact = $artifact; Record = $record; Entries = $entries; Manifest = $manifest }
}

function Copy-MivSoundCandidateRuntime {
    param([string] $RepoRoot, [string] $Candidate, [string] $OutputDir)
    $item = Read-MivSoundCandidate -RepoRoot $RepoRoot -Candidate $Candidate
    $destination = Join-Path $OutputDir 'effetune'
    $expected = [IO.Path]::GetFullPath((Join-Path $RepoRoot 'target/dev-runtime/effetune'))
    if ([IO.Path]::GetFullPath($destination) -ne $expected) { throw '[miv-sound] Unexpected verification destination' }
    Assert-MivSoundSafeDirectory $destination
    if (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Recurse -Force }
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Get-ChildItem -LiteralPath $item.Artifact -Force | Copy-Item -Destination $destination -Recurse -Force
    $actual = Get-MivSoundManifestText (Get-MivSoundInventory $destination)
    if ($actual -cne $item.Manifest) { throw '[miv-sound] Verification copy differs from candidate' }
    Write-Host "[miv-sound] verification runtime candidate: $Candidate"
}

# Only the explicit -Approve command calls this for production paths. Tests pass
# a disposable repository and backup root and never touch production approvals.
function Approve-MivSoundCandidate {
    param([string] $RepoRoot, [string] $Candidate, [string] $BackupRoot)
    $repo = [IO.Path]::GetFullPath($RepoRoot)
    $utf8 = New-Object Text.UTF8Encoding($false)
    $item = Read-MivSoundCandidate -RepoRoot $repo -Candidate $Candidate
    $inspection = $item.Record.provenance.PSObject.Properties['inspection_only']
    if ($inspection -and $inspection.Value) { throw '[miv-sound] Inspection snapshots cannot be approved' }
    $evidence = Get-Content -LiteralPath (Join-Path $item.Root 'ctest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $testLog = Get-Content -LiteralPath (Join-Path $item.Root 'ctest.log') -Raw -Encoding UTF8
    if ($evidence.candidate -cne $Candidate -or $evidence.passed -ne $true -or
        $testLog -notmatch '100% tests passed, 0 tests failed out of [1-9][0-9]*' -or
        $evidence.log_sha256 -cne (Get-FileHash -LiteralPath (Join-Path $item.Root 'ctest.log') -Algorithm SHA256).Hash.ToLowerInvariant()) {
        throw '[miv-sound] Approval requires successful ctest evidence for this candidate; run -TestCandidate -Candidate <ID>'
    }
    $contractHash = Assert-SoundBuildContract $item
    $evidenceContract = $evidence.PSObject.Properties['build_contract_sha256']
    if (-not $evidenceContract -or $evidenceContract.Value -cne $contractHash) {
        throw '[miv-sound] Approval requires ctest evidence bound to the candidate build/test contract'
    }
    $testCount = @($item.Record.provenance.build_contract.selected_tests).Count
    if ($testLog -notmatch ("100% tests passed, 0 tests failed out of " + $testCount + '(?![0-9])')) {
        throw '[miv-sound] Approval requires ctest success for the complete candidate test inventory'
    }
    $tracked = Join-Path $repo "$($MivSound.NOTICES_ROOT)/$($MivSound.VERSION)"
    $vendor = Join-Path $repo $MivSound.SOURCE_ROOT
    $backup = Join-Path $BackupRoot "miv-sound-$($MivSound.VERSION)"
    # Approval never replaces an existing version/backup. Use a new version for
    # a new artifact. Fail before writing any approval output.
    Assert-MivSoundSafeDirectory $vendor
    foreach ($path in @($tracked, $backup)) {
        Assert-MivSoundSafeDirectory $path
        if (Test-Path -LiteralPath $path) { throw "[miv-sound] Approval destination already exists: $path" }
    }
    $mainNotice = [IO.File]::ReadAllText((Join-Path $item.Artifact ($MivSound.BUNDLE_NAME + '/' + $MivSoundNotices[0])))
    $licenses = @($item.Record.provenance.upstream_licenses)
    if ($licenses.Count -ne 2) { throw '[miv-sound] Both upstream licenses must be recorded' }
    foreach ($license in $licenses) {
        if (-not $mainNotice.Replace("`r`n", "`n").Contains($license.text.Replace("`r`n", "`n").Trim())) {
            throw "[miv-sound] Full upstream license absent from notice: $($license.path)"
        }
    }
    foreach ($required in @('Copyright (c) 2026, Yoshiyuki Kobayashi', 'Copyright (c) 2025-2026', 'Permission is hereby granted', 'THE SOFTWARE IS PROVIDED "AS IS"')) {
        if (-not $mainNotice.Contains($required)) { throw "[miv-sound] Upstream notice missing: $required" }
    }
    New-Item -ItemType Directory -Path $backup -Force | Out-Null
    Copy-Item -LiteralPath $item.Artifact -Destination (Join-Path $backup 'artifact') -Recurse
    foreach ($file in @('candidate.json', 'ctest.json', 'ctest.log')) { Copy-Item -LiteralPath (Join-Path $item.Root $file) -Destination $backup }
    if ((Get-MivSoundManifestText (Get-MivSoundInventory (Join-Path $backup 'artifact'))) -cne $item.Manifest) { throw '[miv-sound] Backup differs' }
    New-Item -ItemType Directory -Path $tracked -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $item.Artifact 'VERSION') -Destination $tracked
    [IO.File]::WriteAllText((Join-Path $tracked 'manifest.sha256'), $item.Manifest, $utf8)
    $provenance = "# mIV Sound provenance`n`nCandidate: $Candidate`nUpstream tag: $($item.Record.provenance.identity.upstream_tag)`nUpstream commit: $($item.Record.provenance.identity.upstream_commit)`nFork commit: $($item.Record.provenance.identity.commit)`n`nBuild provenance (from candidate.json):`n`n" + ($item.Record.provenance | ConvertTo-Json -Depth 15) + "`n"
    [IO.File]::WriteAllText((Join-Path $tracked 'PROVENANCE.md'), $provenance, $utf8)
    foreach ($notice in $MivSoundNotices) {
        $dest = Join-Path $tracked $notice
        New-Item -ItemType Directory -Path (Split-Path $dest) -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $item.Artifact ($MivSound.BUNDLE_NAME + '/' + $notice)) -Destination $dest
    }
    Copy-Item -LiteralPath $tracked -Destination (Join-Path $backup 'approval') -Recurse
    if (Test-Path -LiteralPath $vendor) { Remove-Item -LiteralPath $vendor -Recurse -Force }
    Copy-Item -LiteralPath $item.Artifact -Destination $vendor -Recurse
    $null = Assert-MivEffetuneSource -SourceRoot $vendor -NoticesRoot (Join-Path $repo $MivSound.NOTICES_ROOT)
    Write-Host "[miv-sound] APPROVED $Candidate; backup: $backup"
}

if ($ValidateCandidate) {
    $ErrorActionPreference = 'Stop'
    if (-not $candidateValidationRoot) { throw '[miv-sound] WorkspaceRoot required' }
    $null = Read-MivSoundCandidate -RepoRoot $candidateValidationRoot -Candidate $ValidateCandidate
}
