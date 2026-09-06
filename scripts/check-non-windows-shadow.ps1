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

The same limit applies to the standard library, and it produces one known false
positive worth recognising on sight: code under cfg(not(windows)) that names
std::os::unix fails here with "could not find `unix` in `os`", because rustc is
still building for Windows whose std gates that module out. The error text says
so - it points at std's own os/mod.rs with "found an item that was configured
out". On Linux the module exists and the same code compiles. Do not "fix" the
source for this; check the rest of the output for real errors instead, and let
CI judge that line.

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
function Remove-ShadowTree {
    # A plain Directory::Delete throws on a read-only file, and anything copied out of a
    # git object store is read-only. Clear the attribute first so the tree can go.
    param(
        [Parameter(Mandatory = $true)][string] $Path
    )

    if (-not [System.IO.Directory]::Exists($Path)) {
        return $true
    }
    Get-ChildItem -LiteralPath $Path -Recurse -Force -File -ErrorAction SilentlyContinue |
        Where-Object { $_.IsReadOnly } |
        ForEach-Object {
            try {
                $_.IsReadOnly = $false
            } catch {
                # Report it through the delete below rather than here; one unreadable
                # file should not hide the rest.
            }
        }
    try {
        [System.IO.Directory]::Delete($Path, $true)
        return $true
    } catch {
        Write-Warning "[non-windows-shadow] could not remove $Path : $($_.Exception.Message)"
        return $false
    }
}

if (-not (Remove-ShadowTree -Path $shadowRoot)) {
    # A stale tree would be checked instead of the current source, so this one is fatal.
    throw "could not clear the previous shadow at $shadowRoot"
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
    # A bare name excludes at any depth. The full path below only covers this
    # repository's own .git; nested checkouts such as testdata/retro-images/pymag-ref
    # carry their own, whose pack files are read-only and later refuse deletion.
    ".git",
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
    # Capture so the known host-std false positive can be told apart from real
    # errors. Without this the check is permanently red for any cfg(not(windows))
    # code that names std::os::unix, and a gate nobody can pass is a gate nobody runs.
    #
    # cargo writes diagnostics to stderr, so send stderr to a FILE. Do not use
    # `2>&1` here: Windows PowerShell 5.1 wraps each native stderr line in an
    # ErrorRecord and trips -ErrorAction Stop even when cargo exits 0
    # (CLAUDE.md, "Release Build stderr Trap").
    $diagnosticsPath = Join-Path ([System.IO.Path]::GetTempPath()) "miv-non-windows-shadow.log"
    & cargo check --locked --bin mimageviewer-core --features portable 2> $diagnosticsPath
    $checkExit = $LASTEXITCODE
    $checkOutput = if (Test-Path $diagnosticsPath) { Get-Content -LiteralPath $diagnosticsPath } else { @() }
    $checkOutput | ForEach-Object { Write-Output $_ }
    if ($checkExit -ne 0) {
        $errorLines = @($checkOutput | Where-Object { "$_" -match "^error(\[E[0-9]+\])?:" })
        $hostStdOnly = @($errorLines | Where-Object {
                "$_" -notmatch "could not find ``unix`` in ``os``" -and
                "$_" -notmatch "^error: could not compile"
            })
        if ($errorLines.Count -gt 0 -and $hostStdOnly.Count -eq 0) {
            Write-Output ""
            Write-Output "[non-windows-shadow] The only errors are the known host-std limitation:"
            Write-Output "[non-windows-shadow] rustc still targets Windows here, whose std gates"
            Write-Output "[non-windows-shadow] out std::os::unix, so cfg(not(windows)) code that"
            Write-Output "[non-windows-shadow] names it cannot compile on this host. On Linux it"
            Write-Output "[non-windows-shadow] does. Treating this run as a pass; CI is the judge"
            Write-Output "[non-windows-shadow] of those lines."
            $checkPassed = $true
        }
        else {
            throw "non-Windows shadow cargo check failed with exit code $checkExit"
        }
    }
    else {
        $checkPassed = $true
    }
}
finally {
    Pop-Location
    $env:CARGO_TARGET_DIR = $previousCargoTargetDir
    $env:CARGO_BUILD_JOBS = $previousCargoBuildJobs
    if ($checkPassed -and -not $KeepShadow) {
        # The verdict is about the compile. If the temp tree survives, say so and leave
        # the exit code alone -- a leftover directory is not a failed check.
        [void] (Remove-ShadowTree -Path $shadowRoot)
    }
}

Write-Host "[non-windows-shadow] PASS"
if ($KeepShadow) {
    Write-Host "[non-windows-shadow] kept shadow=$shadowRoot"
}
