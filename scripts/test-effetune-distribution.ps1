# Noninteractive staging/PE regression tests; no signing or product launch.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'effetune-distribution.ps1')
function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "[effetune-test] $Message" }
}
function Assert-Throws([scriptblock] $Action, [string] $Pattern) {
    try { & $Action } catch {
        Assert-True ($_.Exception.Message -match $Pattern) "unexpected failure: $_"
        return
    }
    throw "[effetune-test] expected failure: $Pattern"
}
function Write-FixtureBytes([string] $Path, [byte[]] $Bytes) {
    # FileMode.Create cannot overwrite an existing hidden file on Windows.
    # Temporarily clear only fixture attributes, then restore Hidden so the
    # inventory/copy checks still exercise hidden upstream resources.
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    $attributes = if ($item) { $item.Attributes } else { $null }
    if ($item) { $item.Attributes = [System.IO.FileAttributes]::Normal }
    try { [System.IO.File]::WriteAllBytes($Path, $Bytes) }
    finally {
        if ($null -ne $attributes) { (Get-Item -LiteralPath $Path -Force).Attributes = $attributes }
    }
}
$temp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = Join-Path $temp ('miv-effetune-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    $source = Join-Path $testRoot 'vendor\effetune-mixwright'
    $bundle = Join-Path $source 'EffeTune Mixwright.vst3'
    $pePath = Join-Path $bundle 'Contents\x86_64-win\EffeTune Mixwright.vst3'
    New-Item -ItemType Directory -Path (Split-Path -Parent $pePath) -Force | Out-Null
    $bytes = New-Object byte[] 512
    $bytes[0] = 0x4d; $bytes[1] = 0x5a; $bytes[0x3c] = 0x40
    $bytes[64] = 0x50; $bytes[65] = 0x45
    $bytes[88] = 0x0b; $bytes[89] = 0x02
    [System.IO.File]::WriteAllBytes($pePath, $bytes)
    $noticesRoot = Join-Path $testRoot 'third_party\effetune-mixwright'
    foreach ($relative in @(
        'Contents\Resources\THIRD-PARTY-NOTICES.txt',
        'Contents\Resources\webview\THIRD-PARTY-NOTICES.txt',
        'Contents\Resources\webview\plugins\dsp\NOTICE.txt'
    )) {
        foreach ($path in @((Join-Path $bundle $relative), (Join-Path (Join-Path $noticesRoot 'v0.12.0') $relative))) {
            New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
            [System.IO.File]::WriteAllText($path, "notice $relative")
        }
    }
    $hidden = Join-Path $bundle '.gitignore'
    [System.IO.File]::WriteAllText($hidden, 'resource')
    (Get-Item -LiteralPath $hidden).Attributes = [System.IO.FileAttributes]::Hidden
    [System.IO.File]::WriteAllText((Join-Path $source 'VERSION'), 'v0.12.0')
    $fakeDll = Join-Path $bundle 'not-a-pe.dll'
    [System.IO.File]::WriteAllText($fakeDll, 'MZ')
    Assert-True (Test-MivPeFile $pePath) 'missed .vst3 PE'
    Assert-True (-not (Test-MivPeFile $fakeDll)) 'accepted short non-PE .dll'
    Assert-True (@(Get-MivPeFiles -Paths $bundle).Count -eq 1) 'PE enumeration used extensions'
    $badOffset = Join-Path $bundle 'bad-offset.exe'
    $bytes[0x3c] = 0xff; $bytes[0x3d] = 0xff
    [System.IO.File]::WriteAllBytes($badOffset, $bytes)
    Assert-True (-not (Test-MivPeFile $badOffset)) 'accepted truncated PE header'
    Remove-Item -LiteralPath $fakeDll, $badOffset
    # Fixture approval is created once, before testing any local modification.
    $sourceFull = [System.IO.Path]::GetFullPath($source)
    $manifestPath = Join-Path $noticesRoot 'v0.12.0\manifest.sha256'
    $manifestLines = @(Get-MivTreeFiles $source | ForEach-Object {
        '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.FullName.Substring($sourceFull.Length + 1).Replace('\', '/')
    })
    [System.IO.File]::WriteAllText($manifestPath, ($manifestLines -join "`n") + "`n")
    $resource = Join-Path $bundle '.gitignore'
    $resourceBytes = [System.IO.File]::ReadAllBytes($resource)
    Write-FixtureBytes $resource ([System.Text.Encoding]::UTF8.GetBytes('modified'))
    Assert-Throws { Assert-MivEffetuneSource -SourceRoot $source -NoticesRoot $noticesRoot } 'Approved source hash mismatch'
    Write-FixtureBytes $resource $resourceBytes
    (Get-Item -LiteralPath $resource -Force).Attributes = [System.IO.FileAttributes]::Hidden
    $extra = Join-Path $bundle 'extra.js'
    [System.IO.File]::WriteAllText($extra, 'unapproved')
    Assert-Throws { Assert-MivEffetuneSource -SourceRoot $source -NoticesRoot $noticesRoot } 'Extra unapproved file'
    Remove-Item -LiteralPath $extra
    Remove-Item -LiteralPath $resource -Force
    Assert-Throws { Assert-MivEffetuneSource -SourceRoot $source -NoticesRoot $noticesRoot } 'Missing approved file'
    Write-FixtureBytes $resource $resourceBytes
    (Get-Item -LiteralPath $resource -Force).Attributes = [System.IO.FileAttributes]::Hidden
    $stage = New-MivEffetuneStage -RepoRoot $testRoot
    Assert-True (Test-Path -LiteralPath (Join-Path $stage 'EffeTune Mixwright.vst3\.gitignore')) 'lost hidden resource'
    Assert-True ((Get-FileHash -LiteralPath $pePath).Hash -eq
        (Get-FileHash -LiteralPath (Join-Path $stage 'EffeTune Mixwright.vst3\Contents\x86_64-win\EffeTune Mixwright.vst3')).Hash) 'staged PE differs'
    Assert-MivEffetuneStage -RepoRoot $testRoot -SourceRoot $stage
    $extraStage = Join-Path $stage 'unexpected.js'
    [System.IO.File]::WriteAllText($extraStage, 'extra')
    Assert-Throws { Assert-MivEffetuneStage -RepoRoot $testRoot -SourceRoot $stage } 'Extra unapproved file'
    Remove-Item -LiteralPath $extraStage
    $stagedResource = Join-Path $stage 'EffeTune Mixwright.vst3\.gitignore'
    Write-FixtureBytes $stagedResource ([System.Text.Encoding]::UTF8.GetBytes('modified'))
    Assert-Throws { Assert-MivEffetuneStage -RepoRoot $testRoot -SourceRoot $stage } 'not a PE'
    Write-FixtureBytes $stagedResource $resourceBytes
    $stagePe = Join-Path $stage 'EffeTune Mixwright.vst3\Contents\x86_64-win\EffeTune Mixwright.vst3'
    $signed = New-Object byte[] 520
    $original = [System.IO.File]::ReadAllBytes($pePath)
    $unsignedChanged = [byte[]]$original.Clone()
    $unsignedChanged[300] = 1
    [System.IO.File]::WriteAllBytes($stagePe, $unsignedChanged)
    Assert-Throws { Assert-MivEffetuneStage -RepoRoot $testRoot -SourceRoot $stage } 'not an appended Authenticode signature'
    [Array]::Copy($original, $signed, $original.Length)
    [Array]::Copy([BitConverter]::GetBytes([uint32]512), 0, $signed, 232, 4)
    [Array]::Copy([BitConverter]::GetBytes([uint32]8), 0, $signed, 236, 4)
    [System.IO.File]::WriteAllBytes($stagePe, $signed)
    Assert-MivEffetuneSigningOnlyChange -OriginalPath $pePath -SignedPath $stagePe
    Assert-Throws { Assert-MivEffetuneStage -RepoRoot $testRoot -SourceRoot $stage } 'valid authorized Authenticode signature'
    $signed[300] = 1
    [System.IO.File]::WriteAllBytes($stagePe, $signed)
    Assert-Throws { Assert-MivEffetuneSigningOnlyChange -OriginalPath $pePath -SignedPath $stagePe } 'changed approved PE executable bytes'
    [System.IO.File]::WriteAllBytes($stagePe, $original)
    $arbitrary = Join-Path $testRoot 'arbitrary-source'
    Copy-Item -LiteralPath $stage -Destination $arbitrary -Recurse -Force
    [System.IO.File]::WriteAllBytes((Join-Path $arbitrary 'EffeTune Mixwright.vst3\Contents\x86_64-win\EffeTune Mixwright.vst3'), $signed)
    Assert-Throws { Assert-MivEffetuneStage -RepoRoot $testRoot -SourceRoot $arbitrary } 'only in the fixed signed distribution stage'
    $stale = Join-Path $stage 'stale.txt'
    [System.IO.File]::WriteAllText($stale, 'old version')
    $null = New-MivEffetuneStage -RepoRoot $testRoot
    Assert-True (-not (Test-Path -LiteralPath $stale)) 'mixed stale tree into stage'
    # A dangling junction can make Test-Path false even though the directory
    # entry exists. Staging must reject it before deleting or writing anywhere.
    $null = @(Get-MivTreeFiles -Path $stage)
    Remove-Item -LiteralPath $stage -Recurse -Force
    $junctionDestination = Join-Path $testRoot 'junction-destination'
    New-Item -ItemType Directory -Path $junctionDestination | Out-Null
    New-Item -ItemType Junction -Path $stage -Value $junctionDestination | Out-Null
    try {
        [System.IO.Directory]::Delete($junctionDestination)
        Assert-Throws { New-MivEffetuneStage -RepoRoot $testRoot } 'reparse-point staging path'
        Assert-True (-not (Test-Path -LiteralPath $junctionDestination)) 'staging wrote through dangling junction'
    } finally {
        # Nonrecursive deletion removes only the junction entry.
        [System.IO.Directory]::Delete($stage)
    }
    $notice = Join-Path $noticesRoot 'v0.12.0\Contents\Resources\THIRD-PARTY-NOTICES.txt'
    [System.IO.File]::WriteAllText($notice, 'changed')
    Assert-Throws { Assert-MivEffetuneSource -SourceRoot $source -NoticesRoot $noticesRoot } 'differs from vendor'
    Remove-Item -LiteralPath (Join-Path $source 'VERSION')
    Assert-Throws { Assert-MivEffetuneSource -SourceRoot $source -NoticesRoot $noticesRoot } 'Restore vendor'
    foreach ($name in @('build-dist.ps1', 'build-release.ps1', 'check-vcrt-pe-dependencies.ps1', 'effetune-distribution.ps1', 'sign-files.ps1')) {
        $errors = $null; $tokens = $null
        $null = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $name), [ref] $tokens, [ref] $errors)
        Assert-True ($errors.Count -eq 0) "parse failed: $name"
    }
    $release = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'build-release.ps1') -Raw -Encoding UTF8
    Assert-True ($release.IndexOf('Invoke-MivSign -Files @($effetunePe') -lt
        $release.IndexOf('$launcherExit = Invoke-ReleaseCargo')) 'plugin signing happens after embedding'
    Assert-True ($release.Contains('$env:MIMV_EFFETUNE_DIR = $effetuneStage')) 'launcher ignores signed stage'
    $portable = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'build-portable.ps1') -Raw -Encoding UTF8
    Assert-True (-not $portable.Contains('effetune-distribution.ps1')) 'portable requires EffeTune stage'
    Write-Host '[effetune-test] PASS'
} finally {
    $resolved = [System.IO.Path]::GetFullPath($testRoot).TrimEnd('\')
    Assert-True ($resolved.StartsWith($temp + '\', [System.StringComparison]::OrdinalIgnoreCase)) 'cleanup escaped temp'
    $null = @(Get-MivTreeFiles -Path $resolved)
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
