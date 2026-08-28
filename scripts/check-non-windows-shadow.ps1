<#
.SYNOPSIS
Checks mImageViewer's non-Windows cfg surface from a Windows host.

.DESCRIPTION
Copies the repository to a disposable shadow tree, rewrites application
cfg(windows) predicates to permanently false predicates (so cfg(not(windows))
becomes true), rewrites matching Cargo target dependency tables, and runs the
same portable cargo check used by non-Windows CI.

This catches cfg leaks in this repository. It cannot make dependency crates
compile as if rustc itself targeted Linux, so platform leaks inside dependencies
(for example wgpu_hal::dx12) remain CI's responsibility. CI is the final word.

CARGO_TARGET_DIR deliberately uses a short path because deep paths can make
MSBuild FileTracker fail with FTK1011 on Windows.
#>
[CmdletBinding()]
param(
    [string]$CargoTargetDir,
    [switch]$KeepShadow
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$shadowRoot = Join-Path $repoRoot "target\non-windows-shadow"
$targetRoot = Join-Path $repoRoot "target"
if ([string]::IsNullOrWhiteSpace($CargoTargetDir)) {
    $CargoTargetDir = $targetRoot
}
$CargoTargetDir = [System.IO.Path]::GetFullPath($CargoTargetDir)

function Assert-UnderDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Parent
    )

    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $fullParent = [System.IO.Path]::GetFullPath($Parent)
    if (-not $fullParent.EndsWith([System.IO.Path]::DirectorySeparatorChar)) {
        $fullParent += [System.IO.Path]::DirectorySeparatorChar
    }
    if (-not $fullPath.StartsWith($fullParent, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove path outside expected parent: $fullPath"
    }
}

function Rewrite-CfgLine {
    param([Parameter(Mandatory = $true)][string]$Line)

    $rewritten = [System.Text.RegularExpressions.Regex]::Replace(
        $Line,
        'target_os\s*=\s*"windows"',
        'any()'
    )
    # Replace only the bare cfg predicate. Feature names such as
    # "windows-dpapi" and dependency names remain untouched.
    [System.Text.RegularExpressions.Regex]::Replace(
        $rewritten,
        '(?<!["\w-])windows(?!["\w-])',
        'any()'
    )
}

function Rewrite-RustCfgFile {
    param([Parameter(Mandatory = $true)][string]$Path)

    $lines = [System.IO.File]::ReadAllLines($Path)
    $changed = $false
    $insideCfgAttribute = $false
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $line = $lines[$i]
        if ($line -match '#\s*\[\s*cfg(?:_attr)?\s*\(') {
            $insideCfgAttribute = $true
        }
        if ($insideCfgAttribute -or $line -match '\bcfg!\s*\(') {
            $rewritten = Rewrite-CfgLine $line
            if ($rewritten -cne $line) {
                $lines[$i] = $rewritten
                $changed = $true
            }
        }
        if ($insideCfgAttribute -and $line.Contains(']')) {
            $insideCfgAttribute = $false
        }
    }
    if ($changed) {
        [System.IO.File]::WriteAllLines(
            $Path,
            $lines,
            [System.Text.UTF8Encoding]::new($false)
        )
    }
    $changed
}

function Rewrite-CargoCfgFile {
    param([Parameter(Mandatory = $true)][string]$Path)

    $lines = [System.IO.File]::ReadAllLines($Path)
    $changed = $false
    for ($i = 0; $i -lt $lines.Length; $i++) {
        if ($lines[$i] -notmatch 'cfg\s*\(') {
            continue
        }
        $rewritten = Rewrite-CfgLine $lines[$i]
        if ($rewritten -cne $lines[$i]) {
            $lines[$i] = $rewritten
            $changed = $true
        }
    }
    if ($changed) {
        [System.IO.File]::WriteAllLines(
            $Path,
            $lines,
            [System.Text.UTF8Encoding]::new($false)
        )
    }
    $changed
}

Assert-UnderDirectory -Path $shadowRoot -Parent $targetRoot
if ([System.IO.Directory]::Exists($shadowRoot)) {
    [System.IO.Directory]::Delete($shadowRoot, $true)
}
[System.IO.Directory]::CreateDirectory($targetRoot) | Out-Null

Write-Host "[non-windows-shadow] copying repository to $shadowRoot"
$robocopyArgs = @(
    $repoRoot,
    $shadowRoot,
    "/E",
    "/NFL",
    "/NDL",
    "/NJH",
    "/NJS",
    "/NP",
    "/XD",
    (Join-Path $repoRoot ".git"),
    (Join-Path $repoRoot "target")
)
$repoPrefix = $repoRoot
if (-not $repoPrefix.EndsWith([System.IO.Path]::DirectorySeparatorChar)) {
    $repoPrefix += [System.IO.Path]::DirectorySeparatorChar
}
if ($CargoTargetDir.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase) -and
    -not $CargoTargetDir.StartsWith(
        $targetRoot + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    ) -and
    $CargoTargetDir -cne $targetRoot) {
    $robocopyArgs += $CargoTargetDir
}
$robocopyArgs += @("/XF", (Join-Path $repoRoot ".git"))
& robocopy @robocopyArgs
$robocopyExit = $LASTEXITCODE
if ($robocopyExit -ge 8) {
    throw "robocopy failed with exit code $robocopyExit"
}

$rustRewriteCount = 0
Get-ChildItem -LiteralPath $shadowRoot -Recurse -File -Filter *.rs | ForEach-Object {
    if (Rewrite-RustCfgFile $_.FullName) {
        $rustRewriteCount++
    }
}
$cargoRewriteCount = 0
Get-ChildItem -LiteralPath $shadowRoot -Recurse -File -Filter Cargo.toml | ForEach-Object {
    if (Rewrite-CargoCfgFile $_.FullName) {
        $cargoRewriteCount++
    }
}

Write-Host (
    "[non-windows-shadow] rewritten rust_files={0} cargo_files={1} target_dir={2}" -f
    $rustRewriteCount,
    $cargoRewriteCount,
    $CargoTargetDir
)

$previousCargoTargetDir = $env:CARGO_TARGET_DIR
$previousCargoBuildJobs = $env:CARGO_BUILD_JOBS
$checkPassed = $false
Push-Location $shadowRoot
try {
    $env:CARGO_TARGET_DIR = $CargoTargetDir
    if ([string]::IsNullOrWhiteSpace($env:CARGO_BUILD_JOBS)) {
        # A single job also keeps native dependency builds from fanning out
        # deep MSBuild/FileTracker paths during a cold shadow check.
        $env:CARGO_BUILD_JOBS = "1"
    }
    & cargo check --locked --bin mimageviewer-core --features portable
    if ($LASTEXITCODE -ne 0) {
        throw "non-Windows shadow cargo check failed with exit code $LASTEXITCODE"
    }
    $checkPassed = $true
}
finally {
    Pop-Location
    $env:CARGO_TARGET_DIR = $previousCargoTargetDir
    $env:CARGO_BUILD_JOBS = $previousCargoBuildJobs
    if ($checkPassed -and -not $KeepShadow) {
        [System.IO.Directory]::Delete($shadowRoot, $true)
    }
}

Write-Host "[non-windows-shadow] PASS"
if ($KeepShadow) {
    Write-Host "[non-windows-shadow] kept shadow=$shadowRoot"
}
