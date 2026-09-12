[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$runnerPath = Join-Path $PSScriptRoot 'ui-smoke.ps1'

function Assert-Equal {
    param(
        [object] $Actual,
        [object] $Expected,
        [string] $Label
    )
    if (-not [System.StringComparer]::Ordinal.Equals([string]$Actual, [string]$Expected)) {
        throw "${Label}: '$Actual', expected '$Expected'"
    }
}

function Import-FunctionFromScript {
    param(
        [string] $Path,
        [string] $Name
    )

    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile(
        $Path,
        [ref] $tokens,
        [ref] $errors)
    if ($errors.Count -ne 0) {
        throw "failed to parse ${Path}: $($errors[0].Message)"
    }
    $definitions = $ast.FindAll({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -ceq $Name
        }, $true)
    if ($definitions.Count -ne 1) {
        throw "expected exactly one function ${Name} in ${Path}, found $($definitions.Count)"
    }
    Set-Item -Path "Function:script:$Name" -Value $definitions[0].Body.GetScriptBlock()
}

foreach ($functionName in @(
        'Get-NormalizedPath',
        'Test-UiSmokeDeadlineReached',
        'Get-Idle198FocusWaitDisposition',
        'Get-Idle198ObservedMarkers',
        'Write-Idle198LifetimeSample',
        'Register-UiSmokeEvidenceDirectory',
        'Try-RegisterUiSmokeEvidenceDirectory'
    )) {
    Import-FunctionFromScript $runnerPath $functionName
}

# Keep the writer open with the same sharing contract as src/perf.rs. The live
# reader must observe flushed markers without waiting for the application to
# close its perf log.
$markerProbeRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('miv-idle198-live-reader-' + [Guid]::NewGuid().ToString('N'))
$markerProbeLogs = Join-Path $markerProbeRoot 'logs'
$markerProbePath = Join-Path $markerProbeLogs 'perf_events.jsonl'
$markerWriter = $null
try {
    New-Item -ItemType Directory -Path $markerProbeLogs -Force | Out-Null
    $markerWriter = [System.IO.FileStream]::new(
        $markerProbePath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete))
    $markerBytes = [System.Text.Encoding]::UTF8.GetBytes(
        "{`"cat`":`"test_script`",`"message`":`"idle198:small:begin`"}`n")
    $markerWriter.Write($markerBytes, 0, $markerBytes.Length)
    $markerWriter.Flush()
    $dataDir = $markerProbeRoot
    $liveMarkers = @(Get-Idle198ObservedMarkers)
    Assert-Equal $liveMarkers.Count 1 'live shared marker count'
    Assert-Equal $liveMarkers[0] 'idle198:small:begin' 'live shared marker value'
}
finally {
    if ($null -ne $markerWriter) { $markerWriter.Dispose() }
    Remove-Item -LiteralPath $markerProbeRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$scenarioSource = [System.IO.File]::ReadAllText(
    (Join-Path $PSScriptRoot 'ui-smoke\idle198-convergence.rhai'),
    [System.Text.Encoding]::UTF8)
$firstActionIndex = $scenarioSource.IndexOf('run_action("GridColumnCount10")', [System.StringComparison]::Ordinal)
$smallMarkerIndex = $scenarioSource.IndexOf('log("idle198:small:begin")', [System.StringComparison]::Ordinal)
$focusedIndex = $scenarioSource.IndexOf('&& s.focused', [System.StringComparison]::Ordinal)
$registeredIndex = $scenarioSource.IndexOf('&& s.target_registered', [System.StringComparison]::Ordinal)
if ($focusedIndex -lt 0 -or $registeredIndex -lt 0 -or
    $firstActionIndex -gt $focusedIndex -or $firstActionIndex -gt $registeredIndex -or
    $focusedIndex -gt $smallMarkerIndex -or $registeredIndex -gt $smallMarkerIndex) {
    throw 'idle198 focused registered root barrier is not between target action and measurement start'
}
if (-not $scenarioSource.Contains('&& s.target_viewport == "ROOT"')) {
    throw 'idle198 startup focus barrier does not require the ROOT viewport'
}

function Get-UiSmokeForegroundIdentity {
    $script:foregroundCall++
    $foregroundPid = if ($script:lifetimeCase -eq 'post-foreign' -and
        $script:foregroundCall -eq 2) {
        $script:startedPid + 1
    }
    else {
        $script:startedPid
    }
    [void]$script:observationOrder.Add("foreground-$($script:foregroundCall)")
    return [ordered]@{ hwnd = 100 + $script:foregroundCall; pid = $foregroundPid }
}

function Get-Idle198ObservedMarkers {
    [void]$script:observationOrder.Add('markers')
    Start-Sleep -Milliseconds 15
    if ($script:lifetimeCase -eq 'post-dead') {
        $script:process.HasExited = $true
    }
    return @('idle198:small:begin', 'idle198:enlarged:end')
}

function Invoke-LifetimeCase {
    param([string] $Case)

    $samplePath = [System.IO.Path]::GetTempFileName()
    try {
        $script:lifetimeCase = $Case
        $script:foregroundCall = 0
        $script:observationOrder = New-Object System.Collections.ArrayList
        $script:startedPid = 4242
        $script:processStartUtc = '2026-09-08T00:00:00.0000000Z'
        $script:idle198SampleIndex = 0
        $script:lifetimeSamplesPath = $samplePath
        $script:process = [pscustomobject]@{
            HasExited = $false
            RefreshCount = 0
        }
        $script:process | Add-Member -MemberType ScriptMethod -Name Refresh -Value {
            $this.RefreshCount++
            [void]$script:observationOrder.Add("process-$($this.RefreshCount)")
        }
        $clock = [System.Diagnostics.Stopwatch]::StartNew()
        Write-Idle198LifetimeSample $clock 'running'
        $clock.Stop()
        $lines = @(Get-Content -LiteralPath $samplePath -Encoding UTF8)
        Assert-Equal $lines.Count 1 "${Case} sample count"
        $sample = $lines[0] | ConvertFrom-Json
        Assert-Equal ([string]::Join(',', @($script:observationOrder))) `
            'process-1,foreground-1,markers,process-2,foreground-2' `
            "${Case} observation order"
        if ([long]$sample.elapsed_ms -lt 10L) {
            throw "${Case}: elapsed_ms was captured before marker observation completed"
        }
        return $sample
    }
    finally {
        Remove-Item -LiteralPath $samplePath -Force -ErrorAction SilentlyContinue
    }
}

$Scenario = 'Idle198Convergence'
$valid = Invoke-LifetimeCase 'valid'
Assert-Equal $valid.schema_version 2 'valid schema'
Assert-Equal $valid.matches_expected_process $true 'valid match'
Assert-Equal $valid.pre_alive $true 'valid pre alive'
Assert-Equal $valid.post_alive $true 'valid post alive'
Assert-Equal $valid.pre_foreground_pid 4242 'valid pre foreground'
Assert-Equal $valid.post_foreground_pid 4242 'valid post foreground'

$postDead = Invoke-LifetimeCase 'post-dead'
Assert-Equal $postDead.pre_alive $true 'post-dead pre alive'
Assert-Equal $postDead.post_alive $false 'post-dead post alive'
Assert-Equal $postDead.matches_expected_process $false 'post-dead match'

$postForeign = Invoke-LifetimeCase 'post-foreign'
Assert-Equal $postForeign.pre_foreground_pid 4242 'post-foreign pre foreground'
Assert-Equal $postForeign.post_foreground_pid 4243 'post-foreign post foreground'
Assert-Equal $postForeign.matches_expected_process $false 'post-foreign match'

Assert-Equal (Get-Idle198FocusWaitDisposition 5000 5000 5000) `
    'ScenarioTimedOut' `
    'short scenario deadline takes precedence over focus failure'
Assert-Equal (Get-Idle198FocusWaitDisposition 20000 120000 20000) `
    'FocusUnavailable' `
    'focus budget exhaustion remains an environment failure'
Assert-Equal (Get-Idle198FocusWaitDisposition 1000 120000 1000) `
    'Continue' `
    'focus wait continues within both budgets'

$archiveProbeRoot = Join-Path $repoRoot ('target\v370-work\idle198-archive-probe-' + [Guid]::NewGuid().ToString('N'))
$archiveProbeAnalysis = Join-Path $archiveProbeRoot 'analysis'
New-Item -ItemType Directory -Path $archiveProbeAnalysis -Force | Out-Null
try {
    $script:runDir = $archiveProbeRoot
    $script:evidenceEntries = New-Object System.Collections.ArrayList
    $script:archiveErrors = New-Object System.Collections.ArrayList
    $script:reparseTreeCalled = $false
    function Assert-NoReparsePath { }
    function Assert-NoReparseTree {
        $script:reparseTreeCalled = $true
        throw 'synthetic reparse rejection'
    }
    Try-RegisterUiSmokeEvidenceDirectory $archiveProbeAnalysis 'post-analysis'
    Assert-Equal $script:reparseTreeCalled $true 'archive tree validation called'
    Assert-Equal $script:archiveErrors.Count 1 'archive rejection count'
    if (-not ([string]$script:archiveErrors[0]).StartsWith('post-analysis:')) {
        throw "archive rejection was not preserved in archive_errors: $($script:archiveErrors[0])"
    }
}
finally {
    Remove-Item -LiteralPath $archiveProbeAnalysis -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $archiveProbeRoot -Force -ErrorAction SilentlyContinue
}

Write-Host '[idle198-ui-smoke-test] PASS live-reader=1 startup-focus=1 lifetime=3 focus=3 archive=1'
