# Prepare a disposable portable mImageViewer tree for UI smoke tests.
#
# This script never launches target\release\mimageviewer.exe or
# target\release\mimageviewer-core.exe. Those normal builds use
# %APPDATA%\mimageviewer and can mutate the user's real settings.
#
# Output:
#   target\portable-smoke\mimageviewer.exe
#   target\portable-smoke\data\
#
# Usage:
#   .\scripts\prepare-portable-smoke.ps1
#   .\scripts\prepare-portable-smoke.ps1 -SkipBuild
#   .\scripts\prepare-portable-smoke.ps1 -TestScript

[CmdletBinding()]
param(
    [switch] $SkipBuild,
    [switch] $TestScript
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$targetRoot = (Join-Path $repoRoot 'target')
$sandbox = (Join-Path $targetRoot 'portable-smoke')

function Get-NormalizedPath {
    param([string] $Path)
    return [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
}

function Assert-ExactPath {
    param(
        [string] $Path,
        [string] $Expected,
        [string] $Label
    )
    $actualFull = Get-NormalizedPath $Path
    $expectedFull = Get-NormalizedPath $Expected
    if (-not $actualFull.Equals($expectedFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "[$Label] refusing unexpected path: $actualFull (expected $expectedFull)"
    }
    return $actualFull
}

function Assert-NoReparseTree {
    param(
        [string] $Path,
        [string] $Label
    )
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $pending = New-Object System.Collections.Stack
    $pending.Push((Get-Item -LiteralPath $Path -Force))
    while ($pending.Count -gt 0) {
        $item = $pending.Pop()
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "[$Label] refusing reparse point: $($item.FullName)"
        }
        if ($item.PSIsContainer) {
            foreach ($child in (Get-ChildItem -LiteralPath $item.FullName -Force)) {
                $pending.Push($child)
            }
        }
    }
}

function Assert-NoReparsePath {
    param(
        [string] $Path,
        [string] $StopAt,
        [string] $Label
    )
    $current = Get-NormalizedPath $Path
    $stop = Get-NormalizedPath $StopAt
    if (-not ($current.Equals($stop, [System.StringComparison]::OrdinalIgnoreCase) -or
        $current.StartsWith(($stop + '\'), [System.StringComparison]::OrdinalIgnoreCase))) {
        throw "[$Label] refusing path outside repository: $current"
    }
    while ($true) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "[$Label] refusing reparse point: $($item.FullName)"
            }
        }
        if ($current.Equals($stop, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = Split-Path -Parent $current
        if (-not $parent -or $parent -eq $current) {
            throw "[$Label] could not reach repository root from $Path"
        }
        $current = Get-NormalizedPath $parent
    }
}

function Assert-SmokeNotRunning {
    param([string] $ExpectedRoot)
    $rootPrefix = (Get-NormalizedPath $ExpectedRoot) + '\'
    foreach ($process in (Get-Process -ErrorAction SilentlyContinue)) {
        $path = $null
        try { $path = $process.Path } catch { $path = $null }
        if ($path) {
            $full = Get-NormalizedPath $path
            if ($full.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "[portable-smoke] refusing to replace a running smoke tree: $($process.Name) PID=$($process.Id)"
            }
        }
    }
}

Push-Location $repoRoot
try {
    if ($TestScript) {
        $buildArgs = @{ SmokeTestScript = $true }
        if ($SkipBuild) { $buildArgs.SkipBuild = $true }
        & (Join-Path $PSScriptRoot 'build-portable.ps1') @buildArgs
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    } elseif (-not $SkipBuild) {
        & (Join-Path $PSScriptRoot 'build-portable.ps1')
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

    $version = $null
    foreach ($line in (Get-Content (Join-Path $repoRoot 'Cargo.toml'))) {
        if ($line -match '^version\s*=\s*"([^"]+)"') {
            $version = $Matches[1]
            break
        }
    }
    if (-not $version) { throw '[portable-smoke] could not parse package version' }

    $source = if ($TestScript) {
        Join-Path $repoRoot 'target\portable-smoke-package'
    } else {
        Join-Path $repoRoot "dist\mImageViewer_portable_v$version"
    }
    $expectedSource = if ($TestScript) {
        Join-Path $repoRoot 'target\portable-smoke-package'
    } else {
        Join-Path $repoRoot "dist\mImageViewer_portable_v$version"
    }
    $source = Assert-ExactPath $source $expectedSource 'portable-smoke-source'
    if (-not (Test-Path -LiteralPath $source -PathType Container)) {
        throw "[portable-smoke] portable package not found: $source"
    }
    Assert-NoReparsePath $source $repoRoot 'portable-smoke-source'

    $resolvedTargetRoot = Get-NormalizedPath $targetRoot
    $resolvedSandbox = Assert-ExactPath $sandbox (Join-Path $repoRoot 'target\portable-smoke') 'portable-smoke'
    Assert-NoReparsePath $resolvedSandbox $repoRoot 'portable-smoke'
    if (-not $resolvedSandbox.StartsWith(($resolvedTargetRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "[portable-smoke] refusing to replace path outside target: $resolvedSandbox"
    }

    if (Test-Path -LiteralPath $resolvedSandbox) {
        Assert-SmokeNotRunning $resolvedSandbox
        Assert-NoReparseTree $resolvedSandbox 'portable-smoke'
        Remove-Item -LiteralPath $resolvedSandbox -Recurse -Force
    }
    New-Item -ItemType Directory -Path $resolvedSandbox | Out-Null
    foreach ($sourceItem in (Get-ChildItem -LiteralPath $source -Force)) {
        # A portable package may have been run in place. Its data directory can
        # contain real user state and must never be copied into a smoke tree.
        if ($sourceItem.Name -eq 'data') { continue }
        Assert-NoReparseTree $sourceItem.FullName 'portable-smoke-source'
        Copy-Item -LiteralPath $sourceItem.FullName -Destination $resolvedSandbox -Recurse
    }

    $dataDir = Join-Path $resolvedSandbox 'data'
    if (Test-Path -LiteralPath $dataDir) {
        $dataDir = Assert-ExactPath $dataDir (Join-Path $repoRoot 'target\portable-smoke\data') 'portable-smoke-data'
        Assert-NoReparseTree $dataDir 'portable-smoke-data'
        Remove-Item -LiteralPath $dataDir -Recurse -Force
    }
    New-Item -ItemType Directory -Path $dataDir | Out-Null
    $markerValue = if ($TestScript) {
        'mimageviewer-disposable-smoke-v1;test-script=true'
    } else {
        'mimageviewer-disposable-smoke-v1;test-script=false'
    }
    Set-Content -LiteralPath (Join-Path $dataDir '.disposable-smoke-data') -Value $markerValue -Encoding ASCII

    $exe = Join-Path $resolvedSandbox 'mimageviewer.exe'
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw "[portable-smoke] executable missing after copy: $exe"
    }
    if ($TestScript -and -not (Test-Path -LiteralPath (Join-Path $resolvedSandbox '.test-script-build.json') -PathType Leaf)) {
        throw '[portable-smoke] diagnostic package has no test-script build manifest'
    }
    Assert-NoReparseTree $resolvedSandbox 'portable-smoke'

    Write-Host "[portable-smoke] ready: $exe"
    Write-Host "[portable-smoke] isolated data: $dataDir"
    Write-Host "[portable-smoke] test-script: $([bool]$TestScript)"
    Write-Output $exe
}
finally {
    Pop-Location
}
