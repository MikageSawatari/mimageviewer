# EffeTune staging helpers. Dot-source; no signing/building on import.
. (Join-Path $PSScriptRoot 'sign-files.ps1')

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
