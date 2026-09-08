[CmdletBinding()]
param([string] $RunnerPath)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $RunnerPath) {
    $RunnerPath = Join-Path $PSScriptRoot 'ui-smoke.ps1'
}
$runnerPath = [System.IO.Path]::GetFullPath($RunnerPath)
$runsRoot = Join-Path $repoRoot 'target\ui-smoke-runs'

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

function Get-DirectoryEntryNames {
    param([string] $Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return @() }
    return @(
        Get-ChildItem -LiteralPath $Path -Force |
            ForEach-Object { $_.Name } |
            Sort-Object
    )
}

$runnerSource = [System.IO.File]::ReadAllText($runnerPath, [System.Text.Encoding]::UTF8)
$tokens = $null
$parseErrors = $null
$runnerAst = [System.Management.Automation.Language.Parser]::ParseFile(
    $runnerPath,
    [ref]$tokens,
    [ref]$parseErrors)
if ($parseErrors.Count -ne 0) {
    throw "runner parse failed: $($parseErrors[0].Message)"
}
$scenarioParameter = @(
    $runnerAst.ParamBlock.Parameters |
        Where-Object { $_.Name.VariablePath.UserPath -ceq 'Scenario' }
)
if ($scenarioParameter.Count -ne 1) {
    throw "expected one Scenario parameter, found $($scenarioParameter.Count)"
}
$validateSet = @(
    $scenarioParameter[0].Attributes |
        Where-Object { $_.TypeName.FullName -ceq 'ValidateSet' }
)
if ($validateSet.Count -ne 1) {
    throw "expected one Scenario ValidateSet, found $($validateSet.Count)"
}
$scenarios = @($validateSet[0].PositionalArguments | ForEach-Object { [string]$_.Value })
if ($scenarios.Count -eq 0) {
    throw 'Scenario ValidateSet was empty'
}
$gateIndex = $runnerSource.IndexOf('if (-not $InteractiveApproved)', [System.StringComparison]::Ordinal)
$initializationIndex = $runnerSource.IndexOf('$ErrorActionPreference =', [System.StringComparison]::Ordinal)
if ($gateIndex -lt 0 -or $initializationIndex -lt 0 -or $gateIndex -gt $initializationIndex) {
    throw 'InteractiveApproved gate is not before runner initialization'
}
if (-not $runnerSource.Contains('.PARAMETER InteractiveApproved')) {
    throw 'runner help does not describe InteractiveApproved'
}
if (-not $runnerSource.Contains('interactive_approved = [bool]$InteractiveApproved')) {
    throw 'run metadata does not record InteractiveApproved'
}

$hostExecutable = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
foreach ($scenario in $scenarios) {
    $runsBefore = @(Get-DirectoryEntryNames $runsRoot)
    $gateStdout = [System.IO.Path]::GetTempFileName()
    $gateStderr = [System.IO.Path]::GetTempFileName()
    try {
        $gateProcess = Start-Process `
            -FilePath $hostExecutable `
            -ArgumentList @(
                '-NoProfile',
                '-NonInteractive',
                '-ExecutionPolicy', 'Bypass',
                '-File', ('"' + $runnerPath + '"'),
                '-Scenario', $scenario,
                '-SkipBuild') `
            -RedirectStandardOutput $gateStdout `
            -RedirectStandardError $gateStderr `
            -WindowStyle Hidden `
            -Wait `
            -PassThru
        $gateExitCode = $gateProcess.ExitCode
        $gateOutput = @(
            [System.IO.File]::ReadAllText($gateStdout, [System.Text.Encoding]::UTF8),
            [System.IO.File]::ReadAllText($gateStderr, [System.Text.Encoding]::UTF8)
        )
    }
    finally {
        Remove-Item -LiteralPath $gateStdout -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $gateStderr -Force -ErrorAction SilentlyContinue
    }
    $runsAfter = @(Get-DirectoryEntryNames $runsRoot)
    Assert-Equal $gateExitCode 2 "$scenario unapproved exit"
    if (-not (($gateOutput -join "`n").Contains('-InteractiveApproved'))) {
        throw "$scenario unapproved run did not explain the InteractiveApproved requirement"
    }
    if ([string]::Join("`n", $runsBefore) -cne [string]::Join("`n", $runsAfter)) {
        throw "$scenario unapproved run created or changed the ui-smoke run directory inventory"
    }
}

Write-Host "[ui-smoke-approval-test] PASS scenarios=$($scenarios.Count) initialization-before-side-effects=1 metadata=1 help=1"
