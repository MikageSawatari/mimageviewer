param([string] $RunnerPath)

$ErrorActionPreference = 'Stop'
$integrationPath = Join-Path $PSScriptRoot 'ui-smoke\button-helper\RunnerIntegration.ps1'
. $integrationPath

function Assert-Equal {
    param([object] $Actual, [object] $Expected, [string] $Label)
    if (-not [System.StringComparer]::Ordinal.Equals([string]$Actual, [string]$Expected)) {
        throw "${Label}: '$Actual', expected '$Expected'"
    }
}

function New-Status {
    param(
        [bool] $Joined,
        [bool] $Safe,
        [bool] $Succeeded,
        [bool] $Outstanding,
        [string] $Release,
        [string] $Terminal
    )
    [pscustomobject]@{
        Joined = $Joined
        SafeToTerminateApp = $Safe
        Succeeded = $Succeeded
        HasOutstandingRelease = $Outstanding
        ReleaseState = $Release
        TerminalKind = $Terminal
    }
}

$released = New-Status $true $true $true $false 'ConfirmedReleased' 'Succeeded'
$success = Resolve-UiSmokeButtonHelperFinalization 0 'completed' $null $true $true $released
Assert-Equal $success.ExitCode 0 'released exit'
Assert-Equal $success.ScenarioSucceeded $true 'released success'
Assert-Equal $success.MayTerminateApp $true 'released kill interlock'

foreach ($primary in @(
    [pscustomobject]@{ code = 124; phase = 'timed-out'; failure = 'App deadline expired' },
    [pscustomobject]@{ code = 7; phase = 'application-failed'; failure = 'application exited with code 7' },
    [pscustomobject]@{ code = 2; phase = 'environment-failed'; failure = 'fixture preparation failed' },
    [pscustomobject]@{ code = 1; phase = 'application-failed'; failure = 'script assertion failed' }
)) {
    $resolved = Resolve-UiSmokeButtonHelperFinalization `
        $primary.code $primary.phase $primary.failure $true $true $released
    Assert-Equal $resolved.ExitCode $primary.code 'primary exit preserved'
    Assert-Equal $resolved.Phase $primary.phase 'primary phase preserved'
    Assert-Equal $resolved.Failure $primary.failure 'primary reason preserved'
    Assert-Equal $resolved.ScenarioSucceeded $false 'primary failure remains failure'
}

$stillRunning = New-Status $false $false $false $false 'Unknown' 'StillRunning'
$unjoined = Resolve-UiSmokeButtonHelperFinalization 0 'completed' $null $true $true $stillRunning
Assert-Equal $unjoined.ExitCode 2 'still-running exit'
Assert-Equal $unjoined.MayTerminateApp $false 'still-running kill interlock'
Assert-Equal $unjoined.ScenarioSucceeded $false 'still-running success'

$outstanding = New-Status $true $false $false $true 'ConfirmedOutstanding' 'GestureFailed'
$unsafe = Resolve-UiSmokeButtonHelperFinalization 0 'completed' $null $true $true $outstanding
Assert-Equal $unsafe.ExitCode 2 'outstanding exit'
Assert-Equal $unsafe.MayTerminateApp $false 'outstanding kill interlock'

$upFailed = New-Status $true $true $false $false 'ConfirmedReleased' 'GestureFailed'
$failedGesture = Resolve-UiSmokeButtonHelperFinalization 0 'completed' $null $true $true $upFailed
Assert-Equal $failedGesture.ExitCode 2 'Up failure exit'
Assert-Equal $failedGesture.MayTerminateApp $true 'released Up failure kill interlock'
Assert-Equal $failedGesture.ScenarioSucceeded $false 'Up failure success'

$beforeDown = New-Status $true $true $false $false 'NoOwnedDown' 'ConnectionFailed'
$preGesture = Resolve-UiSmokeButtonHelperFinalization 0 'completed' $null $true $true $beforeDown
Assert-Equal $preGesture.ExitCode 2 'before-Down exit'
Assert-Equal $preGesture.MayTerminateApp $true 'before-Down kill interlock'

$missingTerminal = Resolve-UiSmokeButtonHelperFinalization 124 'timed-out' 'App deadline expired' $true $true $null
Assert-Equal $missingTerminal.ExitCode 124 'missing-terminal primary exit'
Assert-Equal $missingTerminal.Failure 'App deadline expired' 'missing-terminal primary reason'
Assert-Equal $missingTerminal.MayTerminateApp $false 'missing-terminal kill interlock'

$timeoutStillRunning = Resolve-UiSmokeButtonHelperFinalization `
    124 'timed-out' 'App deadline expired' $true $true $stillRunning
Assert-Equal $timeoutStillRunning.ExitCode 124 'timeout plus still-running exit'
Assert-Equal $timeoutStillRunning.Phase 'timed-out' 'timeout plus still-running phase'
Assert-Equal $timeoutStillRunning.Failure 'App deadline expired' 'timeout plus still-running reason'
Assert-Equal $timeoutStillRunning.MayTerminateApp $false 'timeout plus still-running kill interlock'

$nonzeroOutstanding = Resolve-UiSmokeButtonHelperFinalization `
    7 'application-failed' 'application exited with code 7' $true $true $outstanding
Assert-Equal $nonzeroOutstanding.ExitCode 7 'nonzero plus outstanding exit'
Assert-Equal $nonzeroOutstanding.Phase 'application-failed' 'nonzero plus outstanding phase'
Assert-Equal $nonzeroOutstanding.Failure 'application exited with code 7' 'nonzero plus outstanding reason'
Assert-Equal $nonzeroOutstanding.MayTerminateApp $false 'nonzero plus outstanding kill interlock'

$neverStarted = Resolve-UiSmokeButtonHelperFinalization 2 'environment-failed' 'helper load failed' $true $false $null
Assert-Equal $neverStarted.ExitCode 2 'never-started exit'
Assert-Equal $neverStarted.Failure 'helper load failed' 'never-started primary reason'
Assert-Equal $neverStarted.MayTerminateApp $true 'never-started kill interlock'

$ordinary = Resolve-UiSmokeButtonHelperFinalization 0 'completed' $null $false $false $null
Assert-Equal $ordinary.ExitCode 0 'ordinary scenario exit'
Assert-Equal $ordinary.MayTerminateApp $true 'ordinary scenario kill interlock'

$helperRoot = Join-Path $PSScriptRoot 'ui-smoke\button-helper'
Import-UiSmokeButtonHelperTypes $helperRoot
if (-not ('Miv.UiSmoke.ButtonHelperDraft.ButtonHelperRunnerApi' -as [type])) {
    throw 'runner helper types were not loaded'
}

$environmentNames = @(
    'MIV_UI_SMOKE_BUTTON_PIPE',
    'MIV_UI_SMOKE_BUTTON_SESSION',
    'MIV_UI_SMOKE_BUTTON_SERVER_PID'
)
$environmentBefore = @{}
foreach ($name in $environmentNames) {
    $environmentBefore[$name] = [Environment]::GetEnvironmentVariable(
        $name,
        [EnvironmentVariableTarget]::Process)
}
$session = '11223344-5566-7788-99aa-bbccddeeff00'
$observed = Invoke-WithUiSmokeButtonHelperEnvironment 'test-pipe' $session 321 {
    [pscustomobject]@{
        Pipe = $env:MIV_UI_SMOKE_BUTTON_PIPE
        Session = $env:MIV_UI_SMOKE_BUTTON_SESSION
        ServerPid = $env:MIV_UI_SMOKE_BUTTON_SERVER_PID
    }
}
Assert-Equal $observed.Pipe 'test-pipe' 'child pipe environment'
Assert-Equal $observed.Session $session 'child session environment'
Assert-Equal $observed.ServerPid '321' 'child server PID environment'
foreach ($name in $environmentNames) {
    Assert-Equal `
        ([Environment]::GetEnvironmentVariable($name, [EnvironmentVariableTarget]::Process)) `
        $environmentBefore[$name] `
        "$name restored after success"
}
try {
    Invoke-WithUiSmokeButtonHelperEnvironment 'test-pipe-fault' $session 654 {
        throw 'expected child launch failure'
    }
    throw 'faulting child action unexpectedly returned'
}
catch {
    if (-not $_.Exception.Message.Contains('expected child launch failure')) { throw }
}
foreach ($name in $environmentNames) {
    Assert-Equal `
        ([Environment]::GetEnvironmentVariable($name, [EnvironmentVariableTarget]::Process)) `
        $environmentBefore[$name] `
        "$name restored after failure"
}

$runnerPath = if ($RunnerPath) {
    [System.IO.Path]::GetFullPath($RunnerPath)
}
else {
    Join-Path $PSScriptRoot 'ui-smoke.ps1'
}
$runnerSource = [System.IO.File]::ReadAllText($runnerPath, [System.Text.Encoding]::UTF8)
$integrationSource = [System.IO.File]::ReadAllText($integrationPath, [System.Text.Encoding]::UTF8)
$combinedSource = $runnerSource + [Environment]::NewLine + $integrationSource
$tokens = $null
$parseErrors = $null
$runnerAst = [System.Management.Automation.Language.Parser]::ParseFile(
    $runnerPath,
    [ref]$tokens,
    [ref]$parseErrors)
if ($parseErrors.Count -ne 0) {
    throw "runner parse failed: $($parseErrors[0].Message)"
}
$completeFunctions = @($runnerAst.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] `
        -and $node.Name -ceq 'Complete-UiSmokeRun'
}, $true))
if ($completeFunctions.Count -ne 1) {
    throw "expected one Complete-UiSmokeRun function, found $($completeFunctions.Count)"
}
$completeBody = $completeFunctions[0].Body.Extent.Text
$finalizeMatches = [System.Text.RegularExpressions.Regex]::Matches(
    $completeBody,
    '(?m)^\s*Finalize-UiSmokeButtonHelper\s*$')
$stopMatches = [System.Text.RegularExpressions.Regex]::Matches(
    $completeBody,
    '(?m)^\s*Stop-ExactUiSmokeProcess\s*$')
if ($finalizeMatches.Count -ne 1 -or
    $stopMatches.Count -ne 1 -or
    $finalizeMatches[0].Index -gt $stopMatches[0].Index) {
    throw 'runner does not finalize the helper before exact App cleanup'
}
foreach ($requiredText in @(
    'MIV_UI_SMOKE_BUTTON_PIPE',
    'MIV_UI_SMOKE_BUTTON_SESSION',
    'MIV_UI_SMOKE_BUTTON_SERVER_PID',
    'SafeToTerminateApp',
    'button_helper_release_state'
)) {
    if (-not $combinedSource.Contains($requiredText)) {
        throw "runner is missing required button integration text: $requiredText"
    }
}

Write-Host '[ui-smoke-button-runner-test] PASS policy=12 primary-preservation=6 cleanup-order=1 integration-contract=5 helper-load=1 child-environment=2'
