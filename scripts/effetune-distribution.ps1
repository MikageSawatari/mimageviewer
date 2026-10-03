# EffeTune staging helpers. Dot-source; no signing/building on import.
param([string] $ValidateSource, [string] $WorkspaceRoot)
. (Join-Path $PSScriptRoot 'sign-files.ps1')

function Read-MivEffetuneManifest {
    param([string] $ManifestPath)
    $entries = New-Object 'System.Collections.Generic.Dictionary[string,string]' ([System.StringComparer]::Ordinal)
    $caseNames = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($line in Get-Content -LiteralPath $ManifestPath -Encoding UTF8) {
        if ($line.StartsWith('#') -or $line.Length -eq 0) { continue }
        if ($line -notmatch '^([a-f0-9]{64})  (.+)$') { throw "[effetune] Invalid approved manifest line: $line" }
        $hash = $Matches[1]; $name = $Matches[2]
        if ($name.Contains('\') -or $name.StartsWith('/') -or $name.Contains(':') -or
            @($name.Split('/') | Where-Object { $_ -in @('', '.', '..') }).Count -ne 0 -or
            -not $caseNames.Add($name)) { throw "[effetune] Invalid/duplicate approved manifest path: $name" }
        $entries.Add($name, $hash)
    }
    if (-not $entries.ContainsKey('VERSION')) { throw '[effetune] Approved manifest has no VERSION' }
    return ,$entries
}

function Assert-MivEffetuneFileSet {
    param([string] $SourceRoot, [object] $Entries)
    $root = [System.IO.Path]::GetFullPath($SourceRoot).TrimEnd('\')
    $ancestor = New-Object System.IO.DirectoryInfo($root)
    while ($null -ne $ancestor) {
        $item = Get-Item -LiteralPath $ancestor.FullName -Force -ErrorAction Stop
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "[effetune] Reparse-point source ancestor: $($ancestor.FullName)"
        }
        $ancestor = $ancestor.Parent
    }
    $files = @(Get-MivTreeFiles -Path $root)
    $actual = New-Object 'System.Collections.Generic.Dictionary[string,string]' ([System.StringComparer]::Ordinal)
    foreach ($file in $files) {
        $name = $file.FullName.Substring($root.Length + 1).Replace('\', '/')
        if (-not $Entries.ContainsKey($name)) { throw "[effetune] Extra unapproved file: $name. Restore the approved v0.12.0 source; do not regenerate the manifest to accept local changes." }
        $actual.Add($name, $file.FullName)
    }
    foreach ($name in $Entries.Keys) {
        if (-not $actual.ContainsKey($name)) { throw "[effetune] Missing approved file: $name. Restore the complete approved v0.12.0 source." }
    }
    return ,$actual
}

function Assert-MivEffetuneRawSource {
    param([string] $SourceRoot, [string] $ManifestPath)
    $entries = Read-MivEffetuneManifest -ManifestPath $ManifestPath
    $files = Assert-MivEffetuneFileSet -SourceRoot $SourceRoot -Entries $entries
    foreach ($name in $entries.Keys) {
        if ((Get-FileHash -LiteralPath $files[$name] -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entries[$name]) {
            throw "[effetune] Approved source hash mismatch: $name. Restore the approved v0.12.0 source; do not regenerate the manifest to accept local changes."
        }
    }
}

function Assert-MivEffetuneSigningOnlyChange {
    param([string] $OriginalPath, [string] $SignedPath)
    # Authenticode may change only CheckSum, the Certificate Table directory,
    # alignment padding (up to 7 zero bytes), and the appended certificate.
    # A valid signature alone must not authorize swapping in another signed DLL.
    $original = [System.IO.File]::ReadAllBytes($OriginalPath)
    $signed = [System.IO.File]::ReadAllBytes($SignedPath)
    if (-not (Test-MivPeFile $OriginalPath) -or -not (Test-MivPeFile $SignedPath)) {
        throw '[effetune] Modified stage file is not a PE'
    }
    $optional = [BitConverter]::ToUInt32($original, 0x3c) + 24
    $magic = [BitConverter]::ToUInt16($original, $optional)
    $directory = if ($magic -eq 0x20b) { $optional + 144 } elseif ($magic -eq 0x10b) { $optional + 128 } else { throw '[effetune] Unknown PE optional header' }
    if ($directory + 8 -gt $original.Length -or $directory + 8 -gt $signed.Length) { throw '[effetune] Truncated PE optional header' }
    if ([BitConverter]::ToUInt64($original, $directory) -ne 0) { throw '[effetune] Approved plugin unexpectedly already signed' }
    $certificateOffset = [BitConverter]::ToUInt32($signed, $directory)
    $certificateSize = [BitConverter]::ToUInt32($signed, $directory + 4)
    if ($certificateSize -lt 8 -or $certificateOffset -lt $original.Length -or
        $certificateOffset -gt $original.Length + 7 -or
        [uint64]$certificateOffset + $certificateSize -ne $signed.Length) { throw '[effetune] Stage change is not an appended Authenticode signature' }
    for ($i = $original.Length; $i -lt $certificateOffset; $i++) {
        if ($signed[$i] -ne 0) { throw '[effetune] Invalid signing alignment padding' }
    }
    [Array]::Clear($original, $optional + 64, 4)
    [Array]::Clear($signed, $optional + 64, 4)
    [Array]::Clear($original, $directory, 8)
    [Array]::Clear($signed, $directory, 8)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $expected = [BitConverter]::ToString($sha.ComputeHash($original))
        $actual = [BitConverter]::ToString($sha.ComputeHash($signed, 0, $original.Length))
        if ($expected -ne $actual) { throw '[effetune] Signed stage changed approved PE executable bytes' }
    } finally { $sha.Dispose() }
}

function Assert-MivEffetuneStage {
    param([string] $RepoRoot, [string] $SourceRoot)
    $approved = Join-Path $RepoRoot 'vendor\effetune-mixwright'
    $manifest = Join-Path $RepoRoot 'third_party\effetune-mixwright\v0.12.0\manifest.sha256'
    # A signed stage depends on the same verified original, never on an arbitrary
    # environment-provided source or on trusting a new manifest made from it.
    Assert-MivEffetuneRawSource -SourceRoot $approved -ManifestPath $manifest
    $entries = Read-MivEffetuneManifest -ManifestPath $manifest
    $files = Assert-MivEffetuneFileSet -SourceRoot $SourceRoot -Entries $entries
    foreach ($name in $entries.Keys) {
        $path = $files[$name]
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -eq $entries[$name]) { continue }
        $fixedStage = [System.IO.Path]::GetFullPath((Join-Path $RepoRoot 'target\effetune-dist-source')).TrimEnd('\')
        if (-not [System.IO.Path]::GetFullPath($SourceRoot).TrimEnd('\').Equals($fixedStage, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw '[effetune] Modified sources are allowed only in the fixed signed distribution stage'
        }
        Assert-MivEffetuneSigningOnlyChange -OriginalPath (Join-Path $approved $name) -SignedPath $path
        $signature = Get-AuthenticodeSignature -LiteralPath $path
        $selector = Get-MivSignSelector
        $authorized = $signature.SignerCertificate -and $(if ($selector[0] -eq '/sha1') {
            $signature.SignerCertificate.Thumbprint -eq $selector[1]
        } else { $signature.SignerCertificate.Subject.Contains($selector[1]) })
        if ($signature.Status -ne 'Valid' -or -not $authorized) {
            throw "[effetune] Modified stage PE requires a valid authorized Authenticode signature: $path (status=$($signature.Status))"
        }
    }
}

function Assert-MivEffetuneSource {
    param([string] $SourceRoot, [string] $NoticesRoot)
    $versionPath = Join-Path $SourceRoot 'VERSION'
    $bundle = Join-Path $SourceRoot 'EffeTune Mixwright.vst3'
    if (-not (Test-Path -LiteralPath $versionPath -PathType Leaf) -or
        -not (Test-Path -LiteralPath $bundle -PathType Container)) {
        throw "[effetune] Missing VERSION or bundle in $SourceRoot. Restore vendor/effetune-mixwright from the approved Mixwright release (see docs/effetune-integration-plan.md section 7)."
    }
    $version = (Get-Content -LiteralPath $versionPath -Raw -Encoding UTF8).Trim()
    if ($version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?$') {
        throw "[effetune] Invalid VERSION: $version"
    }
    if ($version -ne 'v0.12.0') { throw '[effetune] Unsupported VERSION; restore approved v0.12.0' }
    Assert-MivEffetuneRawSource -SourceRoot $SourceRoot -ManifestPath (Join-Path $NoticesRoot 'v0.12.0\manifest.sha256')
    $null = @(Get-MivTreeFiles -Path $SourceRoot)
    $plugin = Join-Path $bundle 'Contents\x86_64-win\EffeTune Mixwright.vst3'
    if (-not (Test-Path -LiteralPath $plugin -PathType Leaf)) {
        throw "[effetune] Plugin PE missing from $bundle; restore the complete approved release."
    }
    if (-not (Test-MivPeFile -Path $plugin)) {
        throw "[effetune] Plugin has no valid PE header: $plugin; restore the complete approved release."
    }
    $tracked = Join-Path $NoticesRoot $version
    foreach ($relative in @(
        'Contents\Resources\THIRD-PARTY-NOTICES.txt',
        'Contents\Resources\webview\THIRD-PARTY-NOTICES.txt',
        'Contents\Resources\webview\plugins\dsp\NOTICE.txt'
    )) {
        $sourceNotice = Join-Path $bundle $relative
        $trackedNotice = Join-Path $tracked $relative
        if (-not (Test-Path -LiteralPath $sourceNotice -PathType Leaf) -or
            -not (Test-Path -LiteralPath $trackedNotice -PathType Leaf)) {
            throw "[effetune] License notice missing: $relative. Update tracked third_party/effetune-mixwright/$version together with the vendor release."
        }
        if ((Get-FileHash -LiteralPath $sourceNotice -Algorithm SHA256).Hash -ne
            (Get-FileHash -LiteralPath $trackedNotice -Algorithm SHA256).Hash) {
            throw "[effetune] Tracked license notice differs from vendor: $relative"
        }
    }
    return $version
}

function New-MivEffetuneStage {
    param([string] $RepoRoot)
    $repo = [System.IO.Path]::GetFullPath($RepoRoot).TrimEnd('\')
    $target = Join-Path $repo 'target'
    $stage = Join-Path $target 'effetune-dist-source'
    # Fixed workspace-relative destination. Check all ancestors before deletion
    # or copying so a pre-existing junction cannot escape the workspace.
    foreach ($ancestor in @($repo, $target, $stage)) {
        # Test-Path can report false for dangling symlinks/junctions; Get-Item
        # inspects the directory entry itself before any cleanup/copy operation.
        $item = Get-Item -LiteralPath $ancestor -Force -ErrorAction SilentlyContinue
        if ($null -ne $item) {
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "[effetune] Refusing reparse-point staging path: $ancestor"
            }
        }
    }
    $source = Join-Path $repo 'vendor\effetune-mixwright'
    $version = Assert-MivEffetuneSource -SourceRoot $source -NoticesRoot (Join-Path $repo 'third_party\effetune-mixwright')
    if (Test-Path -LiteralPath $stage) {
        $null = @(Get-MivTreeFiles -Path $stage)
        Remove-Item -LiteralPath $stage -Recurse -Force
    }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $source 'VERSION') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $source 'EffeTune Mixwright.vst3') -Destination $stage -Recurse -Force
    Write-Host "[effetune] staged $version at $stage"
    return $stage
}

if ($ValidateSource) {
    $ErrorActionPreference = 'Stop'
    if (-not $WorkspaceRoot) { throw '[effetune] -WorkspaceRoot is required for source validation' }
    Assert-MivEffetuneStage -RepoRoot $WorkspaceRoot -SourceRoot $ValidateSource
}
