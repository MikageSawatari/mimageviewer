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

[CmdletBinding()]
param(
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$targetRoot = (Join-Path $repoRoot 'target')
$sandbox = (Join-Path $targetRoot 'portable-smoke')

Push-Location $repoRoot
try {
    if (-not $SkipBuild) {
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

    $source = Join-Path $repoRoot "dist\mImageViewer_portable_v$version"
    if (-not (Test-Path -LiteralPath $source -PathType Container)) {
        throw "[portable-smoke] portable package not found: $source"
    }

    $resolvedTargetRoot = [System.IO.Path]::GetFullPath($targetRoot).TrimEnd('\') + '\'
    $resolvedSandbox = [System.IO.Path]::GetFullPath($sandbox)
    if (-not $resolvedSandbox.StartsWith($resolvedTargetRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "[portable-smoke] refusing to replace path outside target: $resolvedSandbox"
    }

    if (Test-Path -LiteralPath $resolvedSandbox) {
        Remove-Item -LiteralPath $resolvedSandbox -Recurse -Force
    }
    Copy-Item -LiteralPath $source -Destination $resolvedSandbox -Recurse

    $dataDir = Join-Path $resolvedSandbox 'data'
    if (Test-Path -LiteralPath $dataDir) {
        Remove-Item -LiteralPath $dataDir -Recurse -Force
    }
    New-Item -ItemType Directory -Path $dataDir | Out-Null
    Set-Content -LiteralPath (Join-Path $dataDir '.disposable-smoke-data') -Value 'safe to delete'

    $exe = Join-Path $resolvedSandbox 'mimageviewer.exe'
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw "[portable-smoke] executable missing after copy: $exe"
    }

    Write-Host "[portable-smoke] ready: $exe"
    Write-Host "[portable-smoke] isolated data: $dataDir"
    Write-Output $exe
}
finally {
    Pop-Location
}
