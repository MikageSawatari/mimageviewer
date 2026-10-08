param([string] $RunnerPath)

$ErrorActionPreference = 'Stop'
$runnerPath = if ($RunnerPath) {
    [System.IO.Path]::GetFullPath($RunnerPath)
}
else {
    Join-Path $PSScriptRoot 'ui-smoke.ps1'
}
$runnerSource = [System.IO.File]::ReadAllText($runnerPath, [System.Text.Encoding]::UTF8)
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $runnerPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -ne 0) { throw 'runner parse failed' }

# Refusal cases execute the actual runner with fake process discovery and
# side-effect tripwires. Admission cases execute only its initialization prefix.
# The prefix suffix cannot build, create evidence, launch an App, or touch Clipboard.
$initializationIndex = $runnerSource.IndexOf('$ErrorActionPreference =')
$approvalIndex = $runnerSource.IndexOf('if (-not $InteractiveApproved)')
$processIndex = $runnerSource.IndexOf('$otherMivProcesses = @(Get-Process')
if ($approvalIndex -lt 0 -or $processIndex -le $approvalIndex -or
    $initializationIndex -le $processIndex) {
    throw 'process guard must follow approval and precede initialization'
}
$prefix = $runnerSource.Substring(0, $initializationIndex)
$hintStatements = @($ast.EndBlock.Statements | Where-Object {
    $_ -is [System.Management.Automation.Language.IfStatementAst] -and
    $_.Extent.Text.Contains('[ui-smoke] ClipboardCapture failed.')
})
if ($hintStatements.Count -ne 1) { throw 'expected one final failure hint' }
$completion = @($ast.EndBlock.Statements | Where-Object {
    $_ -is [System.Management.Automation.Language.TryStatementAst] -and
    $_.Finally.Extent.Text.Contains('Complete-UiSmokeRun')
})
if ($completion.Count -ne 1 -or
    $hintStatements[0].Extent.StartOffset -le $completion[0].Extent.EndOffset) {
    throw 'failure hint must follow finalization, including cleanup failures'
}
$scenarioParameter = @($ast.ParamBlock.Parameters | Where-Object {
    $_.Name.VariablePath.UserPath -ceq 'Scenario'
})[0]
$validateSet = @($scenarioParameter.Attributes | Where-Object {
    $_.TypeName.FullName -ceq 'ValidateSet'
})[0]
$scenarios = @($validateSet.PositionalArguments | ForEach-Object { $_.Value })
$hostExecutable = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$runsRoot = Join-Path $repoRoot 'target\ui-smoke-runs'
function Get-RunNames {
    if (Test-Path -LiteralPath $runsRoot) {
        @(Get-ChildItem -LiteralPath $runsRoot -Force | ForEach-Object { $_.Name }) -join "`n"
    }
}
$runsBefore = Get-RunNames
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('miv-clipboard-guard-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
$prefixPath = Join-Path $tempRoot 'prefix.ps1'
$wrapperPath = Join-Path $tempRoot 'wrapper.ps1'
$stdoutPath = Join-Path $tempRoot 'stdout.txt'
$stderrPath = Join-Path $tempRoot 'stderr.txt'
$wrapper = @'
param([string] $GuardPath, [string] $Scenario, [string] $FakeName,
    [string] $FakePath, [string] $FailDiscovery, [int] $RunExitCode)
$ErrorActionPreference = 'Stop'
function Get-Process {
    param($ErrorAction)
    if ($FailDiscovery -eq 'yes') { throw 'fake process discovery failure' }
    if ($FakeName) {
        [pscustomobject]@{ ProcessName = $FakeName; Path = $FakePath; Id = 12345 }
    }
}
function Start-Process { throw 'unexpected process start' }
function Stop-Process { throw 'unexpected process stop' }
function New-Item { throw 'unexpected directory creation' }
$script:runExitCode = $RunExitCode
& $GuardPath -Scenario $Scenario -InteractiveApproved -SkipBuild
exit $LASTEXITCODE
'@
[System.IO.File]::WriteAllText($wrapperPath, $wrapper, [System.Text.Encoding]::ASCII)
$script:caseCount = 0
function Invoke-GuardCase {
    param([string] $Scenario, [string] $Name, [string] $Path,
        [string] $Discovery, [int] $ExitCode, [int] $RunExitCode = 0,
        [bool] $ExpectHint = $false)
    $suffix = "`$script:runExitCode = `$RunExitCode`n" + $hintStatements[0].Extent.Text + "`n[Console]::WriteLine('guard-passed'); exit 0`n"
    [System.IO.File]::WriteAllText($prefixPath, $prefix + $suffix, [System.Text.Encoding]::ASCII)
    $guardPath = if ($ExitCode -eq 2) { $runnerPath } else { $prefixPath }
    $arguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}" -GuardPath "{1}" -Scenario "{2}" -FakeName "{3}" -FakePath "{4}" -FailDiscovery "{5}" -RunExitCode {6}' -f
        $wrapperPath, $guardPath, $Scenario, $Name, $Path, $Discovery, $RunExitCode
    $process = Start-Process -FilePath $hostExecutable -ArgumentList $arguments `
        -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    $stdout = [System.IO.File]::ReadAllText($stdoutPath)
    $stderr = [System.IO.File]::ReadAllText($stderrPath)
    if ($process.ExitCode -ne $ExitCode) {
        throw "${Scenario}/${Name}: exit $($process.ExitCode), expected ${ExitCode}: $stderr"
    }
    if ($ExitCode -eq 2) {
        if (-not $stderr.Contains('tray-resident') -or
            -not $stderr.Contains('No application was started') -or
            $stdout.Contains('guard-passed')) { throw 'missing early refusal / close-tray instruction' }
    }
    elseif (-not $stdout.Contains('guard-passed')) { throw 'guard did not admit scenario' }
    $hasHint = $stderr.Contains('Before resuming normal paste, copy harmless text once.')
    if ($hasHint -ne $ExpectHint) { throw "unexpected failure hint for ${Scenario}/${RunExitCode}" }
    $script:caseCount++
}
try {
    foreach ($name in @('mimageviewer', 'MIMAGEVIEWER-CORE')) {
        foreach ($path in @('C:\Program Files\mIV\app.exe', 'D:\portable\app.exe',
            'C:\work\target\dev-runtime\app.exe', '')) {
            Invoke-GuardCase 'ClipboardCapture' $name $path 'no' 2
        }
    }
    Invoke-GuardCase 'ClipboardCapture' '' '' 'no' 0
    Invoke-GuardCase 'ClipboardCapture' 'unrelated-process' '' 'no' 0
    Invoke-GuardCase 'ClipboardCapture' '' '' 'yes' 2
    foreach ($scenario in $scenarios | Where-Object { $_ -ne 'ClipboardCapture' }) {
        # Discovery would throw if called: other scenarios must not inspect mIV.
        Invoke-GuardCase $scenario 'mimageviewer' '' 'yes' 0
    }
    Invoke-GuardCase 'ClipboardCapture' '' '' 'no' 0 0 $false
    Invoke-GuardCase 'ClipboardCapture' '' '' 'no' 0 2 $true
    Invoke-GuardCase 'ClipboardCapture' '' '' 'no' 0 124 $true
    Invoke-GuardCase 'MultiWindowPdf' 'mimageviewer' '' 'yes' 0 2 $false
    if ((Get-RunNames) -cne $runsBefore) { throw 'test changed evidence directories' }
    Write-Host "[ui-smoke-clipboard-guard-test] PASS cases=$($script:caseCount) no-app-launch=1 no-process-stop=1 no-clipboard-access=1"
}
finally {
    # Remove only explicitly created files, without recursive traversal.
    foreach ($path in @($prefixPath, $wrapperPath, $stdoutPath, $stderrPath)) {
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    }
    Remove-Item -LiteralPath $tempRoot -Force
}
