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
$temp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\')
$testRoot = Join-Path $temp ('miv-effetune-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    $source = Join-Path $testRoot 'vendor\effetune-mixwright'
    $bundle = Join-Path $source 'EffeTune Mixwright.vst3'
    $pePath = Join-Path $bundle 'Contents\x86_64-win\EffeTune Mixwright.vst3'
    New-Item -ItemType Directory -Path (Split-Path -Parent $pePath) -Force | Out-Null
    $bytes = New-Object byte[] 128
    $bytes[0] = 0x4d; $bytes[1] = 0x5a; $bytes[0x3c] = 0x40
    $bytes[64] = 0x50; $bytes[65] = 0x45
    [System.IO.File]::WriteAllBytes($pePath, $bytes)
    $noticesRoot = Join-Path $testRoot 'third_party\effetune-mixwright'
    foreach ($relative in @(
        'Contents\Resources\THIRD-PARTY-NOTICES.txt',
        'Contents\Resources\webview\THIRD-PARTY-NOTICES.txt',
        'Contents\Resources\webview\plugins\dsp\NOTICE.txt'
    )) {
        foreach ($path in @((Join-Path $bundle $relative), (Join-Path (Join-Path $noticesRoot 'v0.11.1') $relative))) {
            New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
            [System.IO.File]::WriteAllText($path, "notice $relative")
        }
    }
    $hidden = Join-Path $bundle '.gitignore'
    [System.IO.File]::WriteAllText($hidden, 'resource')
    (Get-Item -LiteralPath $hidden).Attributes = [System.IO.FileAttributes]::Hidden
    [System.IO.File]::WriteAllText((Join-Path $source 'VERSION'), 'v0.11.1')
    $fakeDll = Join-Path $bundle 'not-a-pe.dll'
    [System.IO.File]::WriteAllText($fakeDll, 'MZ')
    Assert-True (Test-MivPeFile $pePath) 'missed .vst3 PE'
    Assert-True (-not (Test-MivPeFile $fakeDll)) 'accepted short non-PE .dll'
    Assert-True (@(Get-MivPeFiles -Paths $bundle).Count -eq 1) 'PE enumeration used extensions'
    $badOffset = Join-Path $bundle 'bad-offset.exe'
    $bytes[0x3c] = 0xff
    [System.IO.File]::WriteAllBytes($badOffset, $bytes)
    Assert-True (-not (Test-MivPeFile $badOffset)) 'accepted truncated PE header'
    $stage = New-MivEffetuneStage -RepoRoot $testRoot
    Assert-True (Test-Path -LiteralPath (Join-Path $stage 'EffeTune Mixwright.vst3\.gitignore')) 'lost hidden resource'
    Assert-True ((Get-FileHash -LiteralPath $pePath).Hash -eq
        (Get-FileHash -LiteralPath (Join-Path $stage 'EffeTune Mixwright.vst3\Contents\x86_64-win\EffeTune Mixwright.vst3')).Hash) 'staged PE differs'
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
    $notice = Join-Path $noticesRoot 'v0.11.1\Contents\Resources\THIRD-PARTY-NOTICES.txt'
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
