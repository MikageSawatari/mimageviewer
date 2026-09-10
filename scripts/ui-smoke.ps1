<#
.SYNOPSIS
Runs an isolated, diagnostic portable UI smoke scenario after explicit user approval.

.PARAMETER InteractiveApproved
Confirms that the user explicitly approved the scenario and expected duration.
Automation must not pass this switch until that approval has been obtained.
#>
# Run an isolated, diagnostic portable UI smoke scenario.
#
# This runner has no executable or data-directory override. It prepares and
# launches only target\portable-smoke\mimageviewer.exe with its sibling data
# directory. The executable must carry the portable,test-script build manifest.
#
# NOTE: this file is ASCII-only for Windows PowerShell 5.1.
# Running this script opens and controls the disposable portable application.
# Use -InteractiveApproved only after the user has explicitly approved the
# described scenario and its expected duration.

[CmdletBinding()]
param(
    [ValidateSet('MultiWindowPdf', 'NativeMouseMove', 'NativeTopPanoramaHover', 'NativeTopPanoramaClick', 'StillStripDrag')]
    [string] $Scenario = 'MultiWindowPdf',
    [switch] $SkipBuild,
    [int] $TimeoutSeconds = 120,
    [switch] $InteractiveApproved
)

if (-not $InteractiveApproved) {
    [Console]::Error.WriteLine(
        '[ui-smoke] interactive UI run requires explicit user approval; use -InteractiveApproved only after the user agrees to the scenario and expected duration.')
    exit 2
}

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$targetRoot = Join-Path $repoRoot 'target'
$smokeRoot = Join-Path $targetRoot 'portable-smoke'
$exe = Join-Path $smokeRoot 'mimageviewer.exe'
$dataDir = Join-Path $smokeRoot 'data'
$marker = Join-Path $dataDir '.disposable-smoke-data'
$buildManifest = Join-Path $smokeRoot '.test-script-build.json'
$buttonHelperRoot = Join-Path $PSScriptRoot 'ui-smoke\button-helper'
$buttonHelperIntegrationPath = Join-Path $buttonHelperRoot 'RunnerIntegration.ps1'

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

function Write-UiSmokeJson {
    param(
        [string] $Path,
        [object] $Value
    )
    $json = $Value | ConvertTo-Json -Depth 8
    [System.IO.File]::WriteAllText($Path, $json, (New-Object System.Text.UTF8Encoding($false)))
}

function Write-UiSmokeEvent {
    param([string] $Message)
    $line = "{0} {1}" -f [DateTime]::UtcNow.ToString('o'), $Message
    Write-Host "[ui-smoke] $Message"
    if ($script:runnerEventLog) {
        Add-Content -LiteralPath $script:runnerEventLog -Encoding ASCII -Value $line
    }
}

function Add-UiSmokeEvidenceFile {
    param(
        [string] $Source,
        [string] $RelativeDestination,
        [string] $Kind
    )
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        throw "[$Kind] evidence source is not a file: $Source"
    }
    Assert-NoReparsePath $Source $repoRoot $Kind
    $sourceItem = Get-Item -LiteralPath $Source -Force
    if (($sourceItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "[$Kind] refusing reparse point: $($sourceItem.FullName)"
    }

    $destination = Get-NormalizedPath (Join-Path $script:runDir $RelativeDestination)
    $runRoot = Get-NormalizedPath $script:runDir
    if (-not $destination.StartsWith(($runRoot + '\'), [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "[$Kind] evidence destination escaped the run directory: $destination"
    }
    $evidencePath = $RelativeDestination.Replace('\', '/')
    foreach ($entry in $script:evidenceEntries) {
        if ($entry.path -eq $evidencePath) { return }
    }
    if (Test-Path -LiteralPath $destination) {
        throw "[$Kind] refusing to overwrite unregistered evidence: $destination"
    }
    Assert-NoReparsePath $destination $repoRoot $Kind
    $destinationParent = Split-Path -Parent $destination
    if (-not (Test-Path -LiteralPath $destinationParent)) {
        New-Item -ItemType Directory -Path $destinationParent -Force | Out-Null
    }
    Assert-NoReparsePath $destinationParent $repoRoot $Kind
    Copy-Item -LiteralPath $Source -Destination $destination -Force
    $hash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    [void]$script:evidenceEntries.Add([ordered]@{
        kind = $Kind
        path = $evidencePath
        bytes = (Get-Item -LiteralPath $destination).Length
        sha256 = $hash
    })
}

function Register-UiSmokeEvidenceFile {
    param(
        [string] $Path,
        [string] $RelativePath,
        [string] $Kind
    )
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return }
    $expected = Get-NormalizedPath (Join-Path $script:runDir $RelativePath)
    $actual = Get-NormalizedPath $Path
    if (-not $actual.Equals($expected, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "[$Kind] run evidence path mismatch: $actual (expected $expected)"
    }
    Assert-NoReparsePath $actual $repoRoot $Kind
    $evidencePath = $RelativePath.Replace('\', '/')
    foreach ($entry in $script:evidenceEntries) {
        if ($entry.path -eq $evidencePath) { return }
    }
    [void]$script:evidenceEntries.Add([ordered]@{
        kind = $Kind
        path = $evidencePath
        bytes = (Get-Item -LiteralPath $actual).Length
        sha256 = (Get-FileHash -LiteralPath $actual -Algorithm SHA256).Hash.ToLowerInvariant()
    })
}

function Add-UiSmokeEvidenceDirectory {
    param(
        [string] $SourceRoot,
        [string] $RelativeDestination,
        [string] $Kind
    )
    if (-not (Test-Path -LiteralPath $SourceRoot -PathType Container)) { return }
    Assert-NoReparsePath $SourceRoot $repoRoot $Kind
    Assert-NoReparseTree $SourceRoot $Kind
    $sourceFull = Get-NormalizedPath $SourceRoot
    foreach ($file in (Get-ChildItem -LiteralPath $sourceFull -File -Recurse)) {
        $relative = $file.FullName.Substring($sourceFull.Length).TrimStart('\')
        Add-UiSmokeEvidenceFile $file.FullName (Join-Path $RelativeDestination $relative) $Kind
    }
}

function Try-AddUiSmokeEvidenceFile {
    param(
        [string] $Source,
        [string] $RelativeDestination,
        [string] $Kind
    )
    try {
        Add-UiSmokeEvidenceFile $Source $RelativeDestination $Kind
    }
    catch {
        [void]$script:archiveErrors.Add("${Kind}: $($_.Exception.Message)")
    }
}

function Try-AddUiSmokeEvidenceDirectory {
    param(
        [string] $SourceRoot,
        [string] $RelativeDestination,
        [string] $Kind
    )
    try {
        Add-UiSmokeEvidenceDirectory $SourceRoot $RelativeDestination $Kind
    }
    catch {
        [void]$script:archiveErrors.Add("${Kind}: $($_.Exception.Message)")
    }
}

function Stop-ExactUiSmokeProcess {
    if ($null -eq $script:process) { return }
    try {
        $script:process.Refresh()
        if (-not $script:process.HasExited) {
            if (-not $script:mayTerminateExactApp) {
                Write-UiSmokeEvent 'exact App termination blocked: native button release state is not safe'
                return
            }
            $script:process.Kill()
            $script:process.WaitForExit()
        }
        $script:process.Refresh()
        if ($script:process.HasExited -and $script:process.ExitCode -is [int]) {
            $script:appExitCode = $script:process.ExitCode
        }
    }
    catch {
        [void]$script:archiveErrors.Add("process cleanup: $($_.Exception.Message)")
    }
}

function Finalize-UiSmokeButtonHelper {
    if ($Scenario -ne 'NativeTopPanoramaClick') { return }
    if (-not (Get-Command -Name 'Resolve-UiSmokeButtonHelperFinalization' -CommandType Function -ErrorAction SilentlyContinue)) {
        if ($script:buttonHelperStarted) {
            throw 'native button helper started without its runner finalization policy'
        }
        return
    }

    $script:buttonHelperJoinAttempted = $null -ne $script:buttonHelperHandle
    $script:mayTerminateExactApp = -not $script:buttonHelperStarted
    if ($null -ne $script:buttonHelperHandle) {
        # Close the App-kill interlock before requesting owner cleanup. It opens
        # again only from the helper's typed safe-to-terminate result.
        $script:mayTerminateExactApp = $false
        try {
            $script:buttonHelperStatus = $script:buttonHelperHandle.CancelAndJoin(
                'ui-smoke runner finalization',
                5000)
        }
        catch {
            $script:buttonHelperFinalizationError = $_.Exception.Message
            Write-UiSmokeEvent "button helper finalization failed: $($script:buttonHelperFinalizationError)"
        }
    }

    $resolved = Resolve-UiSmokeButtonHelperFinalization `
        $script:runExitCode `
        $script:runPhase `
        $script:failureMessage `
        $true `
        $script:buttonHelperStarted `
        $script:buttonHelperStatus
    $script:runExitCode = $resolved.ExitCode
    $script:runPhase = $resolved.Phase
    $script:failureMessage = $resolved.Failure
    $script:buttonHelperFailure = $resolved.HelperFailure
    $script:buttonHelperSucceeded = $resolved.HelperSucceeded
    $script:mayTerminateExactApp = $resolved.MayTerminateApp
    if ($null -ne $script:buttonHelperStatus) {
        Write-UiSmokeEvent (
            'button helper terminal={0} release={1} joined={2} safe_to_terminate={3}' -f
                $script:buttonHelperStatus.TerminalKind,
                $script:buttonHelperStatus.ReleaseState,
                $script:buttonHelperStatus.Joined,
                $script:buttonHelperStatus.SafeToTerminateApp)
    }
    elseif ($script:buttonHelperStarted) {
        Write-UiSmokeEvent 'button helper terminal unavailable; exact App termination remains blocked'
    }
}

function Open-UiSmokeRunnerLock {
    param([string] $Path)
    try {
        return [System.IO.File]::Open(
            $Path,
            [System.IO.FileMode]::OpenOrCreate,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None)
    }
    catch [System.IO.IOException] {
        throw '[ui-smoke] another UI smoke runner owns the portable workspace'
    }
}

function Test-UiSmokeDeadlineReached {
    param(
        [long] $ElapsedMilliseconds,
        [long] $DeadlineMilliseconds
    )
    return $ElapsedMilliseconds -ge $DeadlineMilliseconds
}

function Invoke-UiSmokeCapturedProcess {
    param(
        [string] $FilePath,
        [string[]] $ArgumentValues,
        [string] $WorkingDirectory,
        [string] $StandardOutputPath,
        [string] $StandardErrorPath
    )
    return Start-Process `
        -FilePath $FilePath `
        -ArgumentList (Join-NativeArguments $ArgumentValues) `
        -WorkingDirectory $WorkingDirectory `
        -RedirectStandardOutput $StandardOutputPath `
        -RedirectStandardError $StandardErrorPath `
        -WindowStyle Hidden `
        -Wait `
        -PassThru
}

function Save-UiSmokeEvidence {
    if (-not $script:runDir -or -not (Test-Path -LiteralPath $script:runDir -PathType Container)) {
        return
    }
    try {
        Register-UiSmokeEvidenceFile (Join-Path $script:runDir 'prepare.stdout.log') 'prepare.stdout.log' 'prepare-stdout'
        Register-UiSmokeEvidenceFile (Join-Path $script:runDir 'prepare.stderr.log') 'prepare.stderr.log' 'prepare-stderr'
        Register-UiSmokeEvidenceFile $script:runnerEventLog 'runner-events.log' 'runner-events'
    }
    catch {
        [void]$script:archiveErrors.Add("runner-log: $($_.Exception.Message)")
    }
    if ($script:portableValidatedForRun) {
        Try-AddUiSmokeEvidenceFile $buildManifest 'artifact/test-script-build.json' 'build-manifest'
        Try-AddUiSmokeEvidenceFile $marker 'artifact/disposable-smoke-data.txt' 'data-marker'
        if ($script:scriptPath) {
            Try-AddUiSmokeEvidenceFile $script:scriptPath 'inputs/scenario.rhai' 'scenario-script'
        }
        if ($script:settingsPath -and (Test-Path -LiteralPath $script:settingsPath -PathType Leaf)) {
            Try-AddUiSmokeEvidenceFile $script:settingsPath 'inputs/settings-override.json' 'settings-override'
        }
        if ($script:fixtureDir) {
            Try-AddUiSmokeEvidenceDirectory $script:fixtureDir 'inputs/fixture' 'fixture'
        }
        if ($script:fixtureGeneratorPath) {
            Try-AddUiSmokeEvidenceFile $script:fixtureGeneratorPath 'inputs/fixture-generator.py' 'fixture-generator'
        }
        if ($script:fixtureGeneratorDependencyPath) {
            Try-AddUiSmokeEvidenceFile $script:fixtureGeneratorDependencyPath 'inputs/fixture-generator-dependency.py' 'fixture-generator-dependency'
        }
        if ($script:fixtureGeneratorPdfDependencyPath) {
            Try-AddUiSmokeEvidenceFile $script:fixtureGeneratorPdfDependencyPath 'inputs/fixture-generator-pdf-dependency.py' 'fixture-generator-pdf-dependency'
        }
        Try-AddUiSmokeEvidenceDirectory (Join-Path $dataDir 'logs') 'logs' 'application-log'
    }

    if ($script:archiveErrors.Count -gt 0) {
        $script:runExitCode = 2
        if (-not $script:failureMessage) {
            $script:failureMessage = 'one or more evidence files could not be collected'
        }
    }
    $evidenceIndexPath = Join-Path $script:runDir 'evidence-index.json'
    try {
        Write-UiSmokeJson $evidenceIndexPath @($script:evidenceEntries)
    }
    catch {
        [void]$script:archiveErrors.Add("evidence-index: $($_.Exception.Message)")
        $script:runExitCode = 2
    }

    $metadata = [ordered]@{
        schema_version = 1
        scenario = $Scenario
        phase = $script:runPhase
        started_utc = $script:runStartedUtc
        finished_utc = [DateTime]::UtcNow.ToString('o')
        runner_pid = $PID
        prepare_pid = $script:preparePid
        prepare_exit_code = $script:prepareExitCode
        app_pid = $script:startedPid
        app_exit_code = $script:appExitCode
        runner_exit_code = $script:runExitCode
        exit_code = $script:runExitCode
        timed_out = $script:timedOut
        skip_build = [bool]$SkipBuild
        interactive_approved = [bool]$InteractiveApproved
        portable_validated_for_run = $script:portableValidatedForRun
        executable = $exe
        data_directory = $dataDir
        executable_sha256 = $script:validatedExeHash
        failure = $script:failureMessage
        archive_errors = @($script:archiveErrors)
        button_helper_required = ($Scenario -eq 'NativeTopPanoramaClick')
        button_helper_pipe = $script:buttonHelperPipeName
        button_helper_session = $script:buttonHelperSession
        button_helper_server_pid = if ($Scenario -eq 'NativeTopPanoramaClick') { $PID } else { $null }
        button_helper_expected_app_pid = if ($Scenario -eq 'NativeTopPanoramaClick') { $script:startedPid } else { $null }
        button_helper_started = $script:buttonHelperStarted
        button_helper_join_attempted = $script:buttonHelperJoinAttempted
        button_helper_joined = if ($null -ne $script:buttonHelperStatus) { [bool]$script:buttonHelperStatus.Joined } else { $null }
        button_helper_safe_to_terminate_app = if ($null -ne $script:buttonHelperStatus) { [bool]$script:buttonHelperStatus.SafeToTerminateApp } else { $null }
        button_helper_succeeded = $script:buttonHelperSucceeded
        button_helper_release_state = if ($null -ne $script:buttonHelperStatus) { [string]$script:buttonHelperStatus.ReleaseState } else { $null }
        button_helper_terminal_kind = if ($null -ne $script:buttonHelperStatus) { [string]$script:buttonHelperStatus.TerminalKind } else { $null }
        button_helper_owner_phase = if ($null -ne $script:buttonHelperStatus) { [string]$script:buttonHelperStatus.OwnerPhase } else { $null }
        button_helper_primary_failure = if ($null -ne $script:buttonHelperStatus) { [string]$script:buttonHelperStatus.PrimaryFailure } else { $null }
        button_helper_detail = if ($null -ne $script:buttonHelperStatus) { [string]$script:buttonHelperStatus.Detail } else { $null }
        button_helper_cleanup_attempts = if ($null -ne $script:buttonHelperStatus) { [int]$script:buttonHelperStatus.CleanupAttempts } else { $null }
        button_helper_failure = $script:buttonHelperFailure
        button_helper_finalization_error = $script:buttonHelperFinalizationError
        exact_app_termination_permitted = $script:mayTerminateExactApp
    }
    try {
        Write-UiSmokeJson (Join-Path $script:runDir 'run-metadata.json') $metadata
    }
    catch {
        Write-Host "[ui-smoke] could not write run metadata: $($_.Exception.Message)"
        $script:runExitCode = 2
    }
}

function Complete-UiSmokeRun {
    try {
        try {
            Finalize-UiSmokeButtonHelper
        }
        catch {
            $script:mayTerminateExactApp = $false
            Write-UiSmokeEvent "button helper aggregation failed: $($_.Exception.Message)"
            if ($script:runExitCode -eq 0) {
                $script:runExitCode = 2
                $script:runPhase = 'button-helper-failed'
                $script:failureMessage = 'native button helper aggregation failed'
            }
        }
        try {
            Stop-ExactUiSmokeProcess
        }
        catch {
            [void]$script:archiveErrors.Add("process finalization: $($_.Exception.Message)")
            $script:runExitCode = 2
        }
        try {
            Save-UiSmokeEvidence
        }
        catch {
            $script:runExitCode = 2
            Write-Host "[ui-smoke] evidence finalization failed: $($_.Exception.Message)"
        }
    }
    finally {
        if ($null -ne $script:runnerLock) {
            $script:runnerLock.Dispose()
            $script:runnerLock = $null
        }
    }
}

function Initialize-UiSmokeEvidence {
    $runsRoot = Assert-ExactPath (Join-Path $targetRoot 'ui-smoke-runs') (Join-Path $repoRoot 'target\ui-smoke-runs') 'ui-smoke-runs'
    Assert-NoReparsePath $runsRoot $repoRoot 'ui-smoke-runs'
    if (-not (Test-Path -LiteralPath $runsRoot)) {
        New-Item -ItemType Directory -Path $runsRoot -Force | Out-Null
    }
    Assert-NoReparsePath $runsRoot $repoRoot 'ui-smoke-runs'

    $runName = '{0}-{1}-{2}-{3}' -f [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ'), $PID, $Scenario, ([Guid]::NewGuid().ToString('N').Substring(0, 8))
    $script:runDir = Join-Path $runsRoot $runName
    Assert-NoReparsePath $script:runDir $repoRoot 'ui-smoke-run'
    New-Item -ItemType Directory -Path $script:runDir | Out-Null
    Assert-NoReparsePath $script:runDir $repoRoot 'ui-smoke-run'
    $script:runnerEventLog = Join-Path $script:runDir 'runner-events.log'
    Add-UiSmokeEvidenceFile $PSCommandPath 'inputs/ui-smoke.ps1' 'runner-script'
    Write-UiSmokeEvent "run evidence: $($script:runDir)"

    $lockPath = Join-Path $runsRoot '.ui-smoke-runner.lock'
    Assert-NoReparsePath $lockPath $repoRoot 'ui-smoke-lock'
    $script:runnerLock = Open-UiSmokeRunnerLock $lockPath
}

$script:runDir = $null
$script:runnerEventLog = $null
$script:runnerLock = $null
$script:evidenceEntries = New-Object System.Collections.ArrayList
$script:archiveErrors = New-Object System.Collections.ArrayList
$script:runStartedUtc = [DateTime]::UtcNow.ToString('o')
$script:runPhase = 'initializing'
$script:runExitCode = 2
$script:failureMessage = $null
$script:portableValidatedForRun = $false
$script:validatedExeHash = $null
$script:process = $null
$script:preparePid = $null
$script:prepareExitCode = $null
$script:startedPid = $null
$script:appExitCode = $null
$script:timedOut = $false
$script:scriptPath = $null
$script:settingsPath = $null
$script:fixtureDir = $null
$script:fixtureGeneratorPath = $null
$script:fixtureGeneratorDependencyPath = $null
$script:fixtureGeneratorPdfDependencyPath = $null
$script:mayTerminateExactApp = $true
$script:buttonHelperHandle = $null
$script:buttonHelperStatus = $null
$script:buttonHelperStarted = $false
$script:buttonHelperJoinAttempted = $false
$script:buttonHelperSucceeded = $false
$script:buttonHelperPipeName = $null
$script:buttonHelperSession = $null
$script:buttonHelperFailure = $null
$script:buttonHelperFinalizationError = $null

try {
    Initialize-UiSmokeEvidence

    if ($TimeoutSeconds -le 0) {
        throw '[ui-smoke] TimeoutSeconds must be greater than zero'
    }

    $implementedScenarios = @('MultiWindowPdf', 'NativeMouseMove', 'NativeTopPanoramaHover', 'NativeTopPanoramaClick', 'StillStripDrag')
    if ($implementedScenarios -notcontains $Scenario) {
        throw "[ui-smoke] scenario $Scenario is not implemented"
    }

    $script:runPhase = 'preparing-portable'
    $prepareScript = Join-Path $PSScriptRoot 'prepare-portable-smoke.ps1'
    $prepareArguments = @(
        '-NoProfile',
        '-NonInteractive',
        '-ExecutionPolicy', 'Bypass',
        '-File', $prepareScript,
        '-TestScript'
    )
    if ($SkipBuild) { $prepareArguments += '-SkipBuild' }
    $hostExecutable = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    $prepareStdout = Join-Path $script:runDir 'prepare.stdout.log'
    $prepareStderr = Join-Path $script:runDir 'prepare.stderr.log'
    $prepareProcess = Invoke-UiSmokeCapturedProcess `
        $hostExecutable `
        $prepareArguments `
        $repoRoot `
        $prepareStdout `
        $prepareStderr
    $script:preparePid = $prepareProcess.Id
    $script:prepareExitCode = $prepareProcess.ExitCode
    Write-UiSmokeEvent "prepare PID $($script:preparePid) exit: $($script:prepareExitCode)"
    if ($script:prepareExitCode -ne 0) {
        throw "[ui-smoke] portable preparation failed with exit $($script:prepareExitCode)"
    }

    $script:runPhase = 'validating-portable'
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
$script:validatedExeHash = $exeHash
$script:portableValidatedForRun = $true
$script:runPhase = 'preserving-artifact'
Try-AddUiSmokeEvidenceFile $buildManifest 'artifact/test-script-build.json' 'build-manifest'
Try-AddUiSmokeEvidenceFile $marker 'artifact/disposable-smoke-data.txt' 'data-marker'
if ($script:archiveErrors.Count -gt 0) {
    throw '[ui-smoke] diagnostic artifact evidence could not be preserved before launch'
}
    $script:runPhase = 'building-fixture'

    $candidateFixtureGeneratorPath = $null
    $candidateFixtureGeneratorDependencyPath = $null
    $candidateFixtureGeneratorPdfDependencyPath = $null

    switch ($Scenario) {
    'MultiWindowPdf' {
        $scenarioRoot = Join-Path $targetRoot 'ui-smoke\multi-window-pdf'
        $candidateScriptPath = Join-Path $PSScriptRoot 'ui-smoke\multi-window-pdf.rhai'
        $candidateFixtureDir = Join-Path $scenarioRoot 'fixture'
        $candidateSettingsPath = Join-Path $dataDir 'settings-override.json'

        $scenarioRoot = Assert-ExactPath $scenarioRoot (Join-Path $repoRoot 'target\ui-smoke\multi-window-pdf') 'ui-smoke-scenario'
        Assert-NoReparsePath $scenarioRoot $repoRoot 'ui-smoke-scenario'
        if (Test-Path -LiteralPath $scenarioRoot) {
            Assert-NoReparseTree $scenarioRoot 'ui-smoke-scenario'
            Remove-Item -LiteralPath $scenarioRoot -Recurse -Force
        }
        New-Item -ItemType Directory -Path $candidateFixtureDir -Force | Out-Null
        & python (Join-Path $PSScriptRoot 'page-turn\generate_pdf_fixture.py') $candidateFixtureDir --docs 2 --pages 2
        if ($LASTEXITCODE -ne 0) {
            throw "[ui-smoke] PDF fixture generator failed with exit $LASTEXITCODE"
        }
        if (@(Get-ChildItem -LiteralPath $candidateFixtureDir -Filter '*.pdf' -File).Count -ne 2) {
            throw '[ui-smoke] PDF fixture must contain exactly two documents'
        }
        if (-not (Test-Path -LiteralPath $candidateScriptPath -PathType Leaf)) {
            throw "[ui-smoke] scenario script not found: $candidateScriptPath"
        }
        $settingsJson = '{"detached_viewer_open_images_in_window":true,"default_spread_mode":"Single","default_reading_flow":"Paged"}'
        [System.IO.File]::WriteAllText($candidateSettingsPath, $settingsJson, (New-Object System.Text.UTF8Encoding($false)))
    }
    { $_ -in @('NativeMouseMove', 'NativeTopPanoramaHover', 'NativeTopPanoramaClick') } {
        $scenarioSlug = switch ($Scenario) {
            'NativeMouseMove' { 'native-mouse-move' }
            'NativeTopPanoramaHover' { 'native-top-panorama-hover' }
            'NativeTopPanoramaClick' { 'native-top-panorama-click' }
        }
        $scenarioRoot = Join-Path $targetRoot (Join-Path 'ui-smoke' $scenarioSlug)
        $candidateScriptPath = Join-Path $PSScriptRoot (Join-Path 'ui-smoke' ($scenarioSlug + '.rhai'))
        $candidateFixtureDir = Join-Path $scenarioRoot 'fixture'
        $candidateSettingsPath = Join-Path $dataDir 'settings-override.json'

        $scenarioRoot = Assert-ExactPath $scenarioRoot (Join-Path $repoRoot (Join-Path 'target\ui-smoke' $scenarioSlug)) 'ui-smoke-scenario'
        Assert-NoReparsePath $scenarioRoot $repoRoot 'ui-smoke-scenario'
        if (Test-Path -LiteralPath $scenarioRoot) {
            Assert-NoReparseTree $scenarioRoot 'ui-smoke-scenario'
            Remove-Item -LiteralPath $scenarioRoot -Recurse -Force
        }
        New-Item -ItemType Directory -Path $candidateFixtureDir -Force | Out-Null
        $ffmpegCommand = Get-Command -Name 'ffmpeg.exe' -CommandType Application -ErrorAction Stop | Select-Object -First 1
        if ($null -eq $ffmpegCommand -or -not (Test-Path -LiteralPath $ffmpegCommand.Source -PathType Leaf)) {
            throw '[ui-smoke] ffmpeg.exe was not found on PATH'
        }
        $videoPath = Join-Path $candidateFixtureDir ($scenarioSlug + '.mp4')
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
        if (@(Get-ChildItem -LiteralPath $candidateFixtureDir -Filter '*.mp4' -File).Count -ne 1 -or
            -not (Test-Path -LiteralPath $videoPath -PathType Leaf) -or
            (Get-Item -LiteralPath $videoPath).Length -le 0) {
            throw '[ui-smoke] video fixture must contain exactly one non-empty MP4'
        }
        if (-not (Test-Path -LiteralPath $candidateScriptPath -PathType Leaf)) {
            throw "[ui-smoke] scenario script not found: $candidateScriptPath"
        }
        $settingsJson = '{"detached_viewer_open_images_in_window":true}'
        [System.IO.File]::WriteAllText($candidateSettingsPath, $settingsJson, (New-Object System.Text.UTF8Encoding($false)))
    }
    'StillStripDrag' {
        $scenarioRoot = Join-Path $targetRoot 'ui-smoke\still-strip-drag'
        $candidateScriptPath = Join-Path $PSScriptRoot 'ui-smoke\still-strip-drag.rhai'
        $candidateFixtureDir = Join-Path $scenarioRoot 'fixture'
        $candidateSettingsPath = Join-Path $dataDir 'settings-override.json'
        $candidateFixtureGeneratorPath = Join-Path $PSScriptRoot 'ui-smoke\generate_still_strip_drag_fixture.py'
        $candidateFixtureGeneratorDependencyPath = Join-Path $PSScriptRoot 'page-turn\generate_fixture.py'
        $candidateFixtureGeneratorPdfDependencyPath = Join-Path $PSScriptRoot 'page-turn\generate_pdf_fixture.py'

        $scenarioRoot = Assert-ExactPath $scenarioRoot (Join-Path $repoRoot 'target\ui-smoke\still-strip-drag') 'ui-smoke-scenario'
        Assert-NoReparsePath $scenarioRoot $repoRoot 'ui-smoke-scenario'
        if (Test-Path -LiteralPath $scenarioRoot) {
            Assert-NoReparseTree $scenarioRoot 'ui-smoke-scenario'
            Remove-Item -LiteralPath $scenarioRoot -Recurse -Force
        }
        foreach ($generatorPath in @(
            $candidateFixtureGeneratorPath,
            $candidateFixtureGeneratorDependencyPath,
            $candidateFixtureGeneratorPdfDependencyPath
        )) {
            Assert-NoReparsePath $generatorPath $repoRoot 'still-strip-generator'
            if (-not (Test-Path -LiteralPath $generatorPath -PathType Leaf)) {
                throw "[ui-smoke] still-strip fixture generator not found: $generatorPath"
            }
        }
        New-Item -ItemType Directory -Path $candidateFixtureDir -Force | Out-Null
        Assert-NoReparsePath $candidateFixtureDir $repoRoot 'still-strip-fixture'
        & python $candidateFixtureGeneratorPath $candidateFixtureDir --count 40
        if ($LASTEXITCODE -ne 0) {
            throw "[ui-smoke] still-strip fixture generator failed with exit $LASTEXITCODE"
        }
        Assert-NoReparseTree $candidateFixtureDir 'still-strip-fixture'
        $imageFixtureDir = Join-Path $candidateFixtureDir 'images'
        Assert-NoReparsePath $imageFixtureDir $repoRoot 'still-strip-image-fixture'
        $fixtureRootEntries = @(Get-ChildItem -LiteralPath $candidateFixtureDir -Force)
        if ($fixtureRootEntries.Count -ne 2 -or
            -not (Test-Path -LiteralPath $imageFixtureDir -PathType Container)) {
            throw '[ui-smoke] still-strip fixture root must contain only images/ and the PDF sibling'
        }
        $pngFiles = @(Get-ChildItem -LiteralPath $imageFixtureDir -Filter '*.png' -File | Sort-Object Name)
        if ($pngFiles.Count -ne 40 -or @($pngFiles | Where-Object { $_.Length -le 0 }).Count -ne 0) {
            throw '[ui-smoke] still-strip fixture must contain exactly forty non-empty PNG files'
        }
        for ($page = 1; $page -le 40; $page++) {
            if ($pngFiles[$page - 1].Name -ne ('{0:D3}.png' -f $page)) {
                throw '[ui-smoke] still-strip fixture page names are not the expected contiguous sequence'
            }
        }
        $pdfFiles = @(Get-ChildItem -LiteralPath $candidateFixtureDir -Filter '*.pdf' -File)
        if ($pdfFiles.Count -ne 1 -or $pdfFiles[0].Name -ne 'zzz-sibling.pdf' -or $pdfFiles[0].Length -le 0) {
            throw '[ui-smoke] still-strip fixture must contain the one non-empty PDF sibling'
        }
        if (-not (Test-Path -LiteralPath $candidateScriptPath -PathType Leaf)) {
            throw "[ui-smoke] scenario script not found: $candidateScriptPath"
        }
        $settingsJson = '{"detached_viewer_open_images_in_window":true,"auto_fullscreen_image_folders":true,"default_spread_mode":"Single","default_reading_flow":"Paged","fullscreen_seek_bar_locked":true,"still_seek_strip_locked":true,"still_seek_strip_visible":true,"still_seek_strip_height":"large"}'
        [System.IO.File]::WriteAllText($candidateSettingsPath, $settingsJson, (New-Object System.Text.UTF8Encoding($false)))
    }
}

$script:scriptPath = $candidateScriptPath
$script:fixtureDir = $candidateFixtureDir
$script:settingsPath = $candidateSettingsPath
$script:fixtureGeneratorPath = $candidateFixtureGeneratorPath
$script:fixtureGeneratorDependencyPath = $candidateFixtureGeneratorDependencyPath
$script:fixtureGeneratorPdfDependencyPath = $candidateFixtureGeneratorPdfDependencyPath

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
    Try-AddUiSmokeEvidenceFile $scriptPath 'inputs/scenario.rhai' 'scenario-script'
    Try-AddUiSmokeEvidenceFile $settingsPath 'inputs/settings-override.json' 'settings-override'
    Try-AddUiSmokeEvidenceDirectory $fixtureDir 'inputs/fixture' 'fixture'
    if ($script:fixtureGeneratorPath) {
        Try-AddUiSmokeEvidenceFile $script:fixtureGeneratorPath 'inputs/fixture-generator.py' 'fixture-generator'
    }
    if ($script:fixtureGeneratorDependencyPath) {
        Try-AddUiSmokeEvidenceFile $script:fixtureGeneratorDependencyPath 'inputs/fixture-generator-dependency.py' 'fixture-generator-dependency'
    }
    if ($script:fixtureGeneratorPdfDependencyPath) {
        Try-AddUiSmokeEvidenceFile $script:fixtureGeneratorPdfDependencyPath 'inputs/fixture-generator-pdf-dependency.py' 'fixture-generator-pdf-dependency'
    }
    if ($Scenario -eq 'NativeTopPanoramaClick') {
        if (-not (Test-Path -LiteralPath $buttonHelperIntegrationPath -PathType Leaf)) {
            throw '[ui-smoke] native button helper runner integration is missing'
        }
        Assert-NoReparsePath $buttonHelperRoot $repoRoot 'ui-smoke-button-helper'
        Assert-NoReparseTree $buttonHelperRoot 'ui-smoke-button-helper'
        . $buttonHelperIntegrationPath
        Import-UiSmokeButtonHelperTypes $buttonHelperRoot
        Try-AddUiSmokeEvidenceDirectory $buttonHelperRoot 'inputs/button-helper' 'button-helper-source'
        $script:buttonHelperPipeName = 'miv-ui-smoke-button-{0}-{1}' -f `
            $PID,
            ([Guid]::NewGuid().ToString('N').Substring(0, 16))
        $script:buttonHelperSession = [Guid]::NewGuid().ToString('D')
    }
    if ($script:archiveErrors.Count -gt 0) {
        throw '[ui-smoke] scenario inputs could not be preserved before launch'
    }

    Write-UiSmokeEvent "scenario: $Scenario"
    Write-UiSmokeEvent "executable: $exe"
    Write-UiSmokeEvent "data: $dataDir"
    $script:runPhase = 'running'
    $scenarioClock = [System.Diagnostics.Stopwatch]::StartNew()
    $timeoutMilliseconds = [long]$TimeoutSeconds * 1000L
    if ($Scenario -eq 'NativeTopPanoramaClick') {
        $script:process = Invoke-WithUiSmokeButtonHelperEnvironment `
            $script:buttonHelperPipeName `
            $script:buttonHelperSession `
            ([uint32]$PID) `
            {
                Start-Process `
                -FilePath $exe `
                -ArgumentList (Join-NativeArguments $arguments) `
                -PassThru
            }
    }
    else {
        $script:process = Start-Process -FilePath $exe -ArgumentList (Join-NativeArguments $arguments) -PassThru
    }
    $script:startedPid = $script:process.Id
    if ($Scenario -eq 'NativeTopPanoramaClick') {
        $remainingAcceptMilliseconds = [long]$timeoutMilliseconds - $scenarioClock.ElapsedMilliseconds
        if ($remainingAcceptMilliseconds -le 0) {
            throw '[ui-smoke] scenario deadline expired before the native button helper could start'
        }
        $acceptTimeoutMilliseconds = [int][Math]::Min(
            [long][int]::MaxValue,
            $remainingAcceptMilliseconds)
        $script:buttonHelperHandle = [Miv.UiSmoke.ButtonHelperDraft.ButtonHelperRunnerApi]::Start(
            $script:buttonHelperPipeName,
            $script:buttonHelperSession,
            [uint32]$script:startedPid,
            $acceptTimeoutMilliseconds)
        $script:buttonHelperStarted = $true
        Write-UiSmokeEvent (
            'button helper started pipe={0} session={1} server_pid={2} expected_app_pid={3}' -f
                $script:buttonHelperPipeName,
                $script:buttonHelperSession,
                $PID,
                $script:startedPid)
    }
    Write-UiSmokeEvent "started PID: $($script:startedPid)"

    $focusStartMilliseconds = $scenarioClock.ElapsedMilliseconds
    while (-not (Test-UiSmokeDeadlineReached $scenarioClock.ElapsedMilliseconds $timeoutMilliseconds) -and
        ($scenarioClock.ElapsedMilliseconds - $focusStartMilliseconds) -lt 20000L) {
        $script:process.Refresh()
        if ($script:process.HasExited) { break }
        if ($script:process.MainWindowHandle -ne 0) {
            $null = [MivUiSmokeWindow]::SetForegroundWindow($script:process.MainWindowHandle)
            break
        }
        Start-Sleep -Milliseconds 100
    }

    while (-not $script:process.HasExited -and
        -not (Test-UiSmokeDeadlineReached $scenarioClock.ElapsedMilliseconds $timeoutMilliseconds)) {
        Start-Sleep -Milliseconds 100
        $script:process.Refresh()
    }
    if (-not $script:process.HasExited) {
        $script:timedOut = $true
        $script:runPhase = 'timed-out'
        $script:runExitCode = 124
        $script:failureMessage = "scenario exceeded the ${TimeoutSeconds}s launch-to-exit deadline"
        Write-UiSmokeEvent 'scenario timed out; stopping the exact process started by this runner'
    }
    else {
        $script:process.WaitForExit()
        $script:process.Refresh()
        $processExitCode = $script:process.ExitCode
        if ($null -eq $processExitCode -or -not ($processExitCode -is [int])) {
            throw '[ui-smoke] process exited without an integer exit code'
        }
        $script:appExitCode = $processExitCode

        $actualMarker = (Get-Content -LiteralPath $marker -Raw -Encoding ASCII).Trim()
        if ($actualMarker -ne $expectedMarker) {
            throw '[ui-smoke] disposable marker changed during the run'
        }
        $script:runExitCode = $processExitCode
        $script:runPhase = if ($processExitCode -eq 0) { 'completed' } else { 'application-failed' }
        if ($processExitCode -ne 0) {
            $script:failureMessage = "application exited with code $processExitCode"
        }
        Write-UiSmokeEvent "exit: $processExitCode"
    }
}
catch {
    $script:runExitCode = 2
    $script:runPhase = 'environment-failed'
    $script:failureMessage = $_.Exception.Message
    Write-UiSmokeEvent "environment failure: $($script:failureMessage)"
}
finally {
    Complete-UiSmokeRun
}

if ($script:runDir) {
    Write-Host "[ui-smoke] evidence: $($script:runDir)"
}
$host.SetShouldExit($script:runExitCode)
exit $script:runExitCode
