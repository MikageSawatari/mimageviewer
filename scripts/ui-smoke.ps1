# Run an isolated, diagnostic portable UI smoke scenario.
#
# This runner has no executable or data-directory override. It prepares and
# launches only target\portable-smoke\mimageviewer.exe with its sibling data
# directory. The executable must carry the portable,test-script build manifest.
#
# NOTE: this file is ASCII-only for Windows PowerShell 5.1.

[CmdletBinding()]
param(
    [ValidateSet('MultiWindowPdf', 'NativeMouseMove')]
    [string] $Scenario = 'MultiWindowPdf',
    [switch] $SkipBuild,
    [int] $TimeoutSeconds = 120
)

$ErrorActionPreference = 'Stop'
trap {
    Write-Host "[ui-smoke] environment failure: $($_.Exception.Message)"
    $host.SetShouldExit(2)
    exit 2
}
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$targetRoot = Join-Path $repoRoot 'target'
$smokeRoot = Join-Path $targetRoot 'portable-smoke'
$exe = Join-Path $smokeRoot 'mimageviewer.exe'
$dataDir = Join-Path $smokeRoot 'data'
$marker = Join-Path $dataDir '.disposable-smoke-data'
$buildManifest = Join-Path $smokeRoot '.test-script-build.json'

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

function Quote-NativeArgument {
    param([string] $Value)
    if ($Value.Contains('"')) {
        throw "[ui-smoke] native argument contains a quote: $Value"
    }
    return '"' + $Value + '"'
}

function Join-NativeArguments {
    param([string[]] $Values)
    return (($Values | ForEach-Object { Quote-NativeArgument $_ }) -join ' ')
}

if ($TimeoutSeconds -le 0) {
    throw '[ui-smoke] TimeoutSeconds must be greater than zero'
}

$implementedScenarios = @('MultiWindowPdf', 'NativeMouseMove')
if ($implementedScenarios -notcontains $Scenario) {
    throw "[ui-smoke] scenario $Scenario is not implemented"
}

$prepareArgs = @{ TestScript = $true }
if ($SkipBuild) { $prepareArgs.SkipBuild = $true }
Push-Location $repoRoot
try {
    & (Join-Path $PSScriptRoot 'prepare-portable-smoke.ps1') @prepareArgs
    if ($LASTEXITCODE -ne 0) {
        throw "[ui-smoke] portable preparation failed with exit $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$exe = Assert-ExactPath $exe (Join-Path $repoRoot 'target\portable-smoke\mimageviewer.exe') 'ui-smoke-exe'
$dataDir = Assert-ExactPath $dataDir (Join-Path $repoRoot 'target\portable-smoke\data') 'ui-smoke-data'
Assert-NoReparsePath $exe $repoRoot 'ui-smoke-exe'
Assert-NoReparsePath $dataDir $repoRoot 'ui-smoke-data'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    throw "[ui-smoke] executable not found: $exe"
}
if (-not (Test-Path -LiteralPath $dataDir -PathType Container)) {
    throw "[ui-smoke] data directory not found: $dataDir"
}
Assert-NoReparseTree $smokeRoot 'ui-smoke'

$expectedMarker = 'mimageviewer-disposable-smoke-v1;test-script=true'
$actualMarker = (Get-Content -LiteralPath $marker -Raw -Encoding ASCII).Trim()
if ($actualMarker -ne $expectedMarker) {
    throw '[ui-smoke] disposable test-script data marker is missing or invalid'
}
if (-not (Test-Path -LiteralPath $buildManifest -PathType Leaf)) {
    throw '[ui-smoke] test-script build manifest is missing'
}
try {
    $manifest = Get-Content -LiteralPath $buildManifest -Raw -Encoding UTF8 | ConvertFrom-Json
}
catch {
    throw '[ui-smoke] test-script build manifest is invalid'
}
$features = @($manifest.features)
if ($manifest.schema_version -ne 1 -or
    $manifest.artifact_flavor -ne 'portable-test-script' -or
    $manifest.cargo_profile -ne 'release' -or
    $features.Count -ne 2 -or
    $features[0] -ne 'portable' -or
    $features[1] -ne 'test-script') {
    throw '[ui-smoke] build manifest is not for portable,test-script release'
}
$exeHash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
if ($manifest.core_sha256 -ne $exeHash) {
    throw '[ui-smoke] executable hash does not match the diagnostic build manifest'
}

switch ($Scenario) {
    'MultiWindowPdf' {
        $scenarioRoot = Join-Path $targetRoot 'ui-smoke\multi-window-pdf'
        $scriptPath = Join-Path $PSScriptRoot 'ui-smoke\multi-window-pdf.rhai'
        $fixtureDir = Join-Path $scenarioRoot 'fixture'
        $settingsPath = Join-Path $dataDir 'settings-override.json'

        $scenarioRoot = Assert-ExactPath $scenarioRoot (Join-Path $repoRoot 'target\ui-smoke\multi-window-pdf') 'ui-smoke-scenario'
        Assert-NoReparsePath $scenarioRoot $repoRoot 'ui-smoke-scenario'
        if (Test-Path -LiteralPath $scenarioRoot) {
            Assert-NoReparseTree $scenarioRoot 'ui-smoke-scenario'
            Remove-Item -LiteralPath $scenarioRoot -Recurse -Force
        }
        New-Item -ItemType Directory -Path $fixtureDir -Force | Out-Null
        & python (Join-Path $PSScriptRoot 'page-turn\generate_pdf_fixture.py') $fixtureDir --docs 2 --pages 2
        if ($LASTEXITCODE -ne 0) {
            throw "[ui-smoke] PDF fixture generator failed with exit $LASTEXITCODE"
        }
        if (@(Get-ChildItem -LiteralPath $fixtureDir -Filter '*.pdf' -File).Count -ne 2) {
            throw '[ui-smoke] PDF fixture must contain exactly two documents'
        }
        if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
            throw "[ui-smoke] scenario script not found: $scriptPath"
        }
        $settingsJson = '{"detached_viewer_open_images_in_window":true,"default_spread_mode":"Single","default_reading_flow":"Paged"}'
        [System.IO.File]::WriteAllText($settingsPath, $settingsJson, (New-Object System.Text.UTF8Encoding($false)))
    }
    'NativeMouseMove' {
        $scenarioRoot = Join-Path $targetRoot 'ui-smoke\native-mouse-move'
        $scriptPath = Join-Path $PSScriptRoot 'ui-smoke\native-mouse-move.rhai'
        $fixtureDir = Join-Path $scenarioRoot 'fixture'
        $settingsPath = Join-Path $dataDir 'settings-override.json'

        $scenarioRoot = Assert-ExactPath $scenarioRoot (Join-Path $repoRoot 'target\ui-smoke\native-mouse-move') 'ui-smoke-scenario'
        Assert-NoReparsePath $scenarioRoot $repoRoot 'ui-smoke-scenario'
        if (Test-Path -LiteralPath $scenarioRoot) {
            Assert-NoReparseTree $scenarioRoot 'ui-smoke-scenario'
            Remove-Item -LiteralPath $scenarioRoot -Recurse -Force
        }
        New-Item -ItemType Directory -Path $fixtureDir -Force | Out-Null
        $ffmpegCommand = Get-Command -Name 'ffmpeg.exe' -CommandType Application -ErrorAction Stop | Select-Object -First 1
        if ($null -eq $ffmpegCommand -or -not (Test-Path -LiteralPath $ffmpegCommand.Source -PathType Leaf)) {
            throw '[ui-smoke] ffmpeg.exe was not found on PATH'
        }
        $videoPath = Join-Path $fixtureDir 'native-mouse-move.mp4'
        $ffmpegArgs = @(
            '-hide_banner', '-loglevel', 'error', '-y',
            '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=10',
            '-t', '120', '-an', '-c:v', 'libx264', '-pix_fmt', 'yuv420p',
            '-movflags', '+faststart', $videoPath
        )
        & $ffmpegCommand.Source $ffmpegArgs
        if ($LASTEXITCODE -ne 0) {
            throw "[ui-smoke] video fixture generator failed with exit $LASTEXITCODE"
        }
        if (@(Get-ChildItem -LiteralPath $fixtureDir -Filter '*.mp4' -File).Count -ne 1 -or
            -not (Test-Path -LiteralPath $videoPath -PathType Leaf) -or
            (Get-Item -LiteralPath $videoPath).Length -le 0) {
            throw '[ui-smoke] video fixture must contain exactly one non-empty MP4'
        }
        if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
            throw "[ui-smoke] scenario script not found: $scriptPath"
        }
        $settingsJson = '{"detached_viewer_open_images_in_window":true}'
        [System.IO.File]::WriteAllText($settingsPath, $settingsJson, (New-Object System.Text.UTF8Encoding($false)))
    }
}

if (-not ('MivUiSmokeWindow' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class MivUiSmokeWindow
{
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
}
'@
}

$arguments = @(
    '--perf-log',
    '--data-dir', $dataDir,
    '--test-script', $scriptPath,
    '--settings-override', $settingsPath,
    $fixtureDir
)
Write-Host "[ui-smoke] scenario: $Scenario"
Write-Host "[ui-smoke] executable: $exe"
Write-Host "[ui-smoke] data: $dataDir"
$process = Start-Process -FilePath $exe -ArgumentList (Join-NativeArguments $arguments) -PassThru
$focusDeadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $focusDeadline) {
    $process.Refresh()
    if ($process.HasExited) { break }
    if ($process.MainWindowHandle -ne 0) {
        $null = [MivUiSmokeWindow]::SetForegroundWindow($process.MainWindowHandle)
        break
    }
    Start-Sleep -Milliseconds 100
}

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
while (-not $process.HasExited -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 100
    $process.Refresh()
}
if (-not $process.HasExited) {
    Write-Host '[ui-smoke] scenario timed out; stopping the exact process started by this runner'
    $process.Kill()
    $process.WaitForExit()
    exit 124
}
$process.WaitForExit()
$process.Refresh()
$processExitCode = $process.ExitCode
if ($null -eq $processExitCode -or -not ($processExitCode -is [int])) {
    throw '[ui-smoke] process exited without an integer exit code'
}

$actualMarker = (Get-Content -LiteralPath $marker -Raw -Encoding ASCII).Trim()
if ($actualMarker -ne $expectedMarker) {
    throw '[ui-smoke] disposable marker changed during the run'
}
Write-Host "[ui-smoke] exit: $processExitCode"
exit $processExitCode
