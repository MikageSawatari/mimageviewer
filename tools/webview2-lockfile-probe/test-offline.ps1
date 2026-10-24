param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
if (-not $SkipBuild) { & (Join-Path $PSScriptRoot 'build.ps1') }
$exe = Join-Path $script:ProbeRoot 'bin\probe.exe'
$run = Join-Path $script:ProbeRoot ('run-offline-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff') + '-' + $PID)
Assert-ProbePath $run
[IO.Directory]::CreateDirectory($run) | Out-Null
$results = [Collections.Generic.List[object]]::new()
function Check([string]$Name, [bool]$Condition) {
    $results.Add([pscustomobject]@{ name = $Name; pass = $Condition })
    if (-not $Condition) { throw "Offline check failed: $Name" }
}
function Stop-OfflineProcess($Process, [string]$Release = '') {
    if ($Process.HasExited) { return }
    if ($Release) { [IO.File]::WriteAllText($Release, 'offline cooperative release') }
    if ($Process.WaitForExit(5000)) { return }
    Add-CleanupFailure 'offline_wait_timeout' $Process.Id 'Offline helper did not exit after 5000 ms natural/cooperative wait.'
    Add-CleanupFailure 'last_resort_kill' $Process.Id 'Offline helper deadline exceeded.'
    $Process.Kill()
    if (-not $Process.WaitForExit(5000)) { throw 'Offline helper did not exit after last-resort Kill.' }
    throw 'Offline helper required last-resort Kill; test fails.'
}
function Native([string[]]$Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $exe; $start.UseShellExecute = $false; $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
    $start.Arguments = (@($Arguments | ForEach-Object { '"' + $_ + '"' }) -join ' ')
    $process = [Diagnostics.Process]::Start($start)
    $outputTask = $process.StandardOutput.ReadToEndAsync(); $errorTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(5000)) { Stop-OfflineProcess $process; throw 'Offline native invocation timed out.' }
    $code = $process.ExitCode; $output = $outputTask.Result; $errorText = $errorTask.Result
    $process.Dispose()
    return [pscustomobject]@{ exit_code = $code; lines = @($output -split '[\r\n]+' | Where-Object { $_ }); stderr = $errorText }
}
function Lock-Try([string]$Udf, [bool]$Create = $false, [int[]]$Ids = @()) {
    $arguments = @('lock', '--udf', $Udf, '--create-new', $(if ($Create) { '1' } else { '0' }))
    if ($Ids.Count) { $arguments += @('--pids', ($Ids -join ',')) }
    $result = Native $arguments
    return [pscustomobject]@{ exit_code = $result.exit_code; events = @($result.lines | ForEach-Object { $_ | ConvertFrom-Json }) }
}
function Holder([string]$Udf, [bool]$Create, [string]$Name, [int]$Timeout = 10000) {
    $log = Join-Path $run ($Name + '.jsonl'); $release = Join-Path $run ($Name + '.release')
    $arguments = @('lock', '--udf', $Udf, '--create-new', $(if ($Create) { '1' } else { '0' }),
        '--log', $log, '--release', $release, '--timeout-ms', [string]$Timeout)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $exe; $start.UseShellExecute = $false; $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.Arguments = ($arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $p = [Diagnostics.Process]::Start($start)
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt 5000) {
        $events = @(Read-Events $log | Where-Object kind -eq 'lock_attempt')
        if ($events.Count) {
            Check ($Name + ': holder acquired') $events[0].opened
            return [pscustomobject]@{ process = $p; release = $release; log = $log }
        }
        if ($p.HasExited) { throw 'Holder exited early.' }
        Start-Sleep -Milliseconds 20
    }
    Stop-OfflineProcess $p $release; $p.Dispose(); throw 'Holder readiness timeout.'
}
function Release($Holder) {
    [IO.File]::WriteAllText($Holder.release, 'close')
    Check 'holder exits on release' $Holder.process.WaitForExit(5000)
    Check 'holder clean exit' ($Holder.process.ExitCode -eq 0)
    $Holder.process.Dispose()
}
$activeHolder = $null
$completed = $false
$failure = $null
try {
    foreach ($file in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1') {
        Check ($file.Name + ': ASCII') (@([IO.File]::ReadAllBytes($file.FullName) | Where-Object { $_ -gt 127 }).Count -eq 0)
        $tokens = $null; $parseErrors = $null
        $null = [Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$parseErrors)
        Check ($file.Name + ': parses') ($parseErrors.Count -eq 0)
    }
    $udf = Join-Path $run 'udf with spaces'
    [IO.Directory]::CreateDirectory((Join-Path $udf 'EBWebView')) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $udf 'nested\empty')) | Out-Null
    [IO.File]::WriteAllBytes((Join-Path $udf 'nested\payload.bin'), [byte[]]@(0, 1, 127, 128, 255))
    [IO.File]::WriteAllText((Join-Path $udf '.hidden'), 'hidden fixture')
    [IO.File]::SetAttributes((Join-Path $udf '.hidden'), [IO.FileAttributes]::Hidden)
    $base = Get-Manifest $udf
    Check 'manifest includes nested/hidden files' ($base.Count -eq 2)
    $lock = Join-Path $udf 'EBWebView\lockfile'
    $absent = Lock-Try $udf
    Check 'OPEN_EXISTING absent error 2' ($absent.exit_code -eq 2 -and $absent.events[0].win32_error -eq 2)
    $activeHolder = Holder $udf $true 'create-new'
    $logRecords = @(Read-Events $activeHolder.log)
    Check 'native log handles are noninheritable' (@($logRecords | Where-Object { $_.log_handle_inheritable -or $_.log_handle_info_error -ne 0 }).Count -eq 0)
    Check 'standard handle inheritance flags recorded without changing them' (@($logRecords | Where-Object { $_.kind -eq 'standard_handle' -and $_.name -eq 'stdout' }).Count -eq 1)
    $lockedFile = Invoke-FileProbe $exe $lock (Join-Path $run 'locked-copy.bin')
    Check 'file diagnostics preserve sharing violations' ($lockedFile.read_error -eq 32 -and $lockedFile.copy_error -eq 32 -and -not $lockedFile.copied)
    Check 'file diagnostics include Restart Manager status' ($null -ne $lockedFile.rm_error)
    $conflict = Lock-Try $udf $false @($activeHolder.process.Id)
    Check 'exclusive contention error 32' ($conflict.exit_code -eq 2 -and $conflict.events[0].win32_error -eq 32)
    Check 'native liveness bracket' ($conflict.events[0].alive_before -eq 1 -and $conflict.events[0].alive_after -eq 1)
    $secondCreate = Lock-Try $udf $true
    Check 'CREATE_NEW does not overwrite' ($secondCreate.exit_code -eq 2 -and $secondCreate.events[0].win32_error -eq 80)
    $held = Get-Manifest $udf
    Check 'locked file is excluded from hashing' (Compare-Manifests $base $held).equal
    $copy = Join-Path $run 'copy'
    Copy-Udf $udf $copy
    Check 'copy excludes only exact lockfile' (-not [IO.File]::Exists((Join-Path $copy 'EBWebView\lockfile')) -and (Compare-Manifests $base (Get-Manifest $copy)).equal)
    Check 'copy keeps empty directories' ([IO.Directory]::Exists((Join-Path $copy 'nested\empty')))
    Release $activeHolder; $activeHolder = $null
    Check 'DELETE_ON_CLOSE leaves no file' (-not [IO.File]::Exists($lock))
    [IO.File]::WriteAllBytes($lock, [byte[]]@(17, 18, 19))
    $activeHolder = Holder $udf $false 'existing'
    $conflict = Lock-Try $udf
    Check 'OPEN_EXISTING exclusive contention' ($conflict.events[0].win32_error -eq 32)
    Release $activeHolder; $activeHolder = $null
    Check 'OPEN_EXISTING preserves contents' ([BitConverter]::ToString([IO.File]::ReadAllBytes($lock)) -eq '11-12-13')
    [IO.File]::Delete($lock)
    $activeHolder = Holder $udf $true 'bounded-holder' 500
    Check 'holder has finite timeout' $activeHolder.process.WaitForExit(3000)
    Check 'holder timeout exit code' ($activeHolder.process.ExitCode -eq 3)
    Check 'timeout also deletes created lock' (-not [IO.File]::Exists($lock))
    $activeHolder.process.Dispose(); $activeHolder = $null
    $changed = Join-Path $udf 'nested\payload.bin'
    [IO.File]::WriteAllBytes($changed, [byte[]]@(255, 1, 127, 128, 0))
    $diff = Compare-Manifests $base (Get-Manifest $udf)
    Check 'SHA256 catches same-length overwrite' (-not $diff.equal -and $diff.differences[0].change -eq 'content_or_size')
    [IO.File]::WriteAllText((Join-Path $udf 'added'), 'added')
    [IO.File]::Delete((Join-Path $udf '.hidden'))
    $diff = Compare-Manifests $base (Get-Manifest $udf)
    Check 'manifest detects additions/removals' (@($diff.differences | Where-Object change -eq 'added').Count -eq 1 -and @($diff.differences | Where-Object change -eq 'removed').Count -eq 1)
    Check 'exact command line match, quotes and spaces' (Test-UdfCommandLine ('"C:\fake\msedgewebview2.exe" --user-data-dir="' + $udf + '" --foo') $udf)
    Check 'separate switch argument supported' (Test-UdfCommandLine ('fake.exe --user-data-dir "' + $udf + '"') $udf)
    Check 'case and slash normalization' (Test-UdfCommandLine ('fake.exe --user-data-dir="' + $udf.ToUpper().Replace('\', '/') + '/"') $udf)
    Check 'sibling prefix is not a match' (-not (Test-UdfCommandLine ('fake.exe --user-data-dir="' + $udf + '-other"') $udf))
    Check 'substring is not a switch' (-not (Test-UdfCommandLine ('fake.exe --other="--user-data-dir=' + $udf + '"') $udf))
    Check 'duplicate switches rejected' (-not (Test-UdfCommandLine ('fake.exe --user-data-dir="' + $udf + '" --user-data-dir="' + $udf + '"') $udf))
    $chromiumDirectory = Join-Path $udf 'EBWebView'
    Check 'WebView2 Chromium directory has EBWebView suffix' (Test-UdfCommandLine ('fake.exe --user-data-dir="' + $chromiumDirectory + '"') $chromiumDirectory)
    Check 'API UDF root is distinct from Chromium directory' (-not (Test-UdfCommandLine ('fake.exe --user-data-dir="' + $udf + '"') $chromiumDirectory))
    $script:FakeCimRows = @(
        [pscustomobject]@{ ProcessId = 111; ParentProcessId = 110; CreationDate = [DateTime]::UtcNow;
            CommandLine = ('fake.exe --user-data-dir="' + $chromiumDirectory + '"') },
        [pscustomobject]@{ ProcessId = 112; ParentProcessId = 111; CreationDate = [DateTime]::UtcNow;
            CommandLine = ('fake.exe --type=renderer --user-data-dir="' + $chromiumDirectory + '"') },
        [pscustomobject]@{ ProcessId = 999; ParentProcessId = 998; CreationDate = [DateTime]::UtcNow;
            CommandLine = ('fake.exe --user-data-dir="' + $chromiumDirectory + '-other"') },
        [pscustomobject]@{ ProcessId = 113; ParentProcessId = 111; CreationDate = [DateTime]::UtcNow;
            CommandLine = ('fake.exe --type=crashpad-handler --database="' + $chromiumDirectory + '\Crashpad"') },
        [pscustomobject]@{ ProcessId = 114; ParentProcessId = 111; CreationDate = [DateTime]::UtcNow;
            CommandLine = ('fake.exe --type=utility --utility-sub-type=network.mojom.NetworkService --user-data-dir="' + $chromiumDirectory + '"') },
        [pscustomobject]@{ ProcessId = 115; ParentProcessId = 111; CreationDate = [DateTime]::UtcNow;
            CommandLine = ('fake.exe --type=gpu-process --user-data-dir="' + $chromiumDirectory + '"') },
        [pscustomobject]@{ Name='custom-helper.exe'; ProcessId = 116; ParentProcessId = 111; CreationDate = [DateTime]::UtcNow;
            CommandLine = ('custom-helper.exe --user-data-dir="' + $chromiumDirectory + '"') })
    function Get-CimInstance { param($ClassName, $Filter, $OperationTimeoutSec, $ErrorAction); $script:FakeCimRows }
    foreach ($fakeRow in $script:FakeCimRows) { if(-not $fakeRow.Name){$fakeRow | Add-Member -NotePropertyName Name -NotePropertyValue 'msedgewebview2.exe'} }
    try {
        $matched = @(Get-UdfProcesses $udf)
        Check 'process inventory matches actual EBWebView directory' ($matched.Count -eq 5 -and @($matched | Where-Object is_browser).Count -eq 1)
        Check 'all process roles include network GPU renderer and other executables' (@($matched | Where-Object role -eq 'network-service').Count -eq 1 -and
            @($matched | Where-Object role -eq 'gpu-process').Count -eq 1 -and @($matched | Where-Object role -eq 'renderer').Count -eq 1 -and
            @($matched | Where-Object role -eq 'other-executable:custom-helper.exe').Count -eq 1)
        Check 'process inventory never selects prefix sibling' (@($matched | Where-Object pid -eq 999).Count -eq 0)
        $related = @(Get-UdfProcesses $udf -IncludeRelated)
        Check 'diagnostic inventory includes exact crashpad database' ($related.Count -eq 6)
        Check 'crashpad-only match never becomes cleanup eligible' (@($related | Where-Object { $_.pid -eq 113 -and -not $_.cleanup_eligible -and $_.match_basis -eq 'crashpad-database' }).Count -eq 1)
    } finally { Remove-Item Function:\Get-CimInstance }
    $rejected = $false
    try { Assert-ProbePath (Join-Path $script:ProbeRoot '..\outside') } catch { $rejected = $true }
    Check 'PowerShell rejects escaping root' $rejected
    # Deliberately invalid paths are rejected before I/O; no real profile is supplied.
    $outside = Native @('lock', '--udf', (Join-Path $script:ProbeRoot '..\outside'))
    Check 'native rejects escaping root' ($outside.exit_code -eq 10)
    $outsideHost = Native @('host', '--udf', (Join-Path $script:ProbeRoot '..\outside'))
    Check 'host rejects unsafe UDF before COM/WebView2' ($outsideHost.exit_code -eq 10)
    $default = Native @()
    Check 'no implicit default profile' ($default.exit_code -eq 10)
    $copyResult = Invoke-FileProbe $exe $changed (Join-Path $run 'readable-copy.bin')
    Check 'unlocked file can be read and copied' ($copyResult.read_error -eq 0 -and $copyResult.copied -and $copyResult.copy_error -eq 0)
    Check 'copy probe preserves exact bytes' ((Get-FileHash -LiteralPath $changed).Hash -eq (Get-FileHash -LiteralPath (Join-Path $run 'readable-copy.bin')).Hash)
    $noClobber = Invoke-FileProbe $exe $changed (Join-Path $run 'readable-copy.bin')
    Check 'diagnostic copy never overwrites evidence' (-not $noClobber.copied -and $noClobber.copy_error -eq 80)
    $busyLog = Join-Path $run 'concurrent-append.jsonl'
    $writer = [IO.FileStream]::new($busyLog, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite,
        ([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes("{`"kind`":`"one`"}`n{`"kind`":`"two`",`"data`":{}")
        $writer.Write($bytes, 0, $bytes.Length); $writer.Flush()
        $events = @(Read-Events $busyLog)
        Check 'concurrent writer remains open while log is readable' ($events.Count -eq 1 -and $events[0].kind -eq 'one')
        $oldReaderFailed = $false
        try { $null = [IO.File]::ReadAllLines($busyLog) } catch { $oldReaderFailed = $true }
        Check 'legacy reader reproduces sharing failure before writer exits' $oldReaderFailed
        $bytes = [Text.Encoding]::UTF8.GetBytes("}`n")
        $writer.Write($bytes, 0, $bytes.Length); $writer.Flush()
        Check 'previous partial record appears only after newline' (@(Read-Events $busyLog).Count -eq 2)
        $bytes = [Text.Encoding]::UTF8.GetBytes("invalid-json`n")
        $writer.Write($bytes, 0, $bytes.Length); $writer.Flush()
        $invalidFailed = $false
        try { $null = @(Read-Events $busyLog) } catch { $invalidFailed = $true }
        Check 'complete corrupt JSON is not silently dropped' $invalidFailed
    } finally { $writer.Dispose() }
    $unreadable = @([pscustomobject]@{ path = 'EBWebView/Default/Preferences'; size = 1; sha256 = $null; read_error = 'sharing violation' })
    Check 'identical read errors cannot pass strict comparison' (-not (Compare-Manifests $unreadable $unreadable).equal)
    Check 'read errors classify profile changes as unknown rather than confirmed' ($null -eq (Get-ChangeSummary (Compare-Manifests $unreadable $unreadable)).profile_changed)
    Check 'default profile classified separately' ((Get-ChangeCategory 'EBWebView/Default/Preferences') -eq 'PROFILE')
    Check 'Local State classified as profile' ((Get-ChangeCategory 'EBWebView/Local State') -eq 'PROFILE')
    Check 'Crashpad classified separately' ((Get-ChangeCategory 'EBWebView/CrashpadMetrics-active.pma') -eq 'CRASHPAD_METRICS')
    $metrics1 = [pscustomobject]@{ equal = $false; differences = @([pscustomobject]@{path='EBWebView/DeferredBrowserMetrics/BrowserMetrics-AAA-111.pma'; change='added'}) }
    $metrics2 = [pscustomobject]@{ equal = $false; differences = @([pscustomobject]@{path='EBWebView/DeferredBrowserMetrics/BrowserMetrics-BBB-222.pma'; change='added'}) }
    $metricsAttempts = @(
        [pscustomobject]@{attempt=1;status='FAIL';detail=@{hash_comparison_while_held=$metrics1}},
        [pscustomobject]@{attempt=2;status='FAIL';detail=@{hash_comparison_while_held=$metrics2}})
    $setSummary = Get-ChangeSetComparison $metricsAttempts
    Check 'raw metric filenames remain distinct across attempts' (-not $setSummary.raw_sets_equal -and $setSummary.all_rounds_comparable)
    Check 'diagnostic normalized metric names can match' $setSummary.diagnostic_normalized_sets_equal
    Check 'metrics differences remain FAIL despite classification' ((Get-AttemptStatus $metricsAttempts) -eq 'FAIL')
    $summary = Get-ChangeSummary $metrics1
    Check 'metrics changes do not claim profile changed' ($summary.crashpad_metrics_changed -and -not $summary.profile_changed)
    foreach ($status in @('FAIL', 'ERROR', 'INCONCLUSIVE')) {
        Check ('later PASS cannot erase earlier ' + $status) ((Get-AttemptStatus @(@{status=$status},@{status='PASS'})) -eq $status)
    }
    Check 'reference copy alone cannot replace a missing active exclusion sample' ((Get-LifetimeStatus 0 0 $true) -eq 'INCONCLUSIVE')
    Check 'active exclusion with complete measurement can pass' ((Get-LifetimeStatus 0 1 $true) -eq 'PASS')
    Check 'strict lifetime violation remains FAIL even if new capture is incomplete' ((Get-LifetimeStatus 1 0 $false) -eq 'FAIL')
    $blockedControl = [pscustomobject]@{blocked=$true;responsive=$true}
    $equalControl = [pscustomobject]@{equal=$true}
    Check 'unchanged full snapshots allow blocked PASS' ((Get-StrictBlockedStatus $blockedControl @($equalControl,$equalControl,$equalControl) $true) -eq 'PASS')
    Check 'settled-only change cannot pass strict source invariance' ((Get-StrictBlockedStatus $blockedControl @($equalControl,$equalControl,$metrics1) $true) -eq 'FAIL')
    Check 'missing snapshot cannot pass strict source invariance' ((Get-StrictBlockedStatus $blockedControl @($equalControl,$equalControl) $true) -eq 'FAIL')
    $gateRows = @('a-graceful','b-existing','b-create-new-delete-on-close','c-copy-and-reopen' | ForEach-Object {
        [pscustomobject]@{name=$_;status='PASS';detail=[ordered]@{orphan_coverage='OBSERVED';attempts=@(@{status='PASS'})}} })
    Check 'historical literal invariant failure keeps migration STOP even for healthy current suite' ((Get-MigrationPrerequisite $gateRows @()) -eq 'STOP')
    $gateRows[1].status = 'INCONCLUSIVE'; $gateRows[1].detail.orphan_coverage = 'NOT_OBSERVED'
    Check 'incomplete suite cannot erase historical STOP' ((Get-MigrationPrerequisite $gateRows @()) -eq 'STOP')
    $gateRows[1].detail.attempts = @(@{status='INCONCLUSIVE'})
    Check 'incomplete natural exit remains STOP' ((Get-MigrationPrerequisite $gateRows @()) -eq 'STOP')
    $gateRows[1].status = 'PASS'
    Check 'diagnostic cleanup failure prevents global success' ((Get-MigrationPrerequisite $gateRows @('diagnostic PID did not exit')) -eq 'STOP')
    $resources = Native @('resource-check')
    $resourceCases = @($resources.lines | ForEach-Object { $_ | ConvertFrom-Json })
    Check 'native resource checks complete without WebView2' ($resources.exit_code -eq 0 -and $resourceCases.Count -eq 9)
    foreach ($case in $resourceCases) { Check ('resource status, headers and body: ' + $case.uri) $case.pass }
    Save-Json (Join-Path $run 'resource-checks.json') $resourceCases
    $nativeMatches = Native @('process-match-check','--udf',$udf)
    $matchCases = @($nativeMatches.lines | ForEach-Object { $_ | ConvertFrom-Json })
    Check 'native shutdown exact-UDF matcher completes without WebView2' ($nativeMatches.exit_code -eq 0 -and $matchCases.Count -eq 7)
    foreach ($case in $matchCases) { Check ('native shutdown path match: ' + $case.command_line) $case.pass }
    $invalidLog = Join-Path $run 'invalid-host.jsonl'
    $invalidHost = Native @('host','--udf',$udf,'--log',$invalidLog,'--stop',(Join-Path $run 'invalid-host.stop'),
        '--mode','INVALID_OFFLINE_ONLY','--token','offline')
    $invalidEvents = @(Read-Events $invalidLog)
    Check 'host validation failure exits through common shutdown without WebView2' ($invalidHost.exit_code -eq 10 -and
        @($invalidEvents | Where-Object { $_.kind -eq 'shutdown_complete' -and $_.complete }).Count -eq 1 -and
        @($invalidEvents | Where-Object { $_.kind -eq 'host_exit' -and $_.shutdown_complete }).Count -eq 1 -and
        @($invalidEvents | Where-Object { $_.kind -eq 'stage_begin' }).Count -eq 0)
    $callbackLog = Join-Path $run 'callback-exception.jsonl'
    $callbackCheck = Native @('callback-check','--log',$callbackLog)
    $callbackEvent = $callbackCheck.lines[0] | ConvertFrom-Json
    Check 'native ABI callback catches C++ failure and reaches common teardown' ($callbackCheck.exit_code -eq 10 -and
        $callbackEvent.failed -and $callbackEvent.hresult -eq '0x80004005' -and $callbackEvent.shutdown_returned -and
        @((Read-Events $callbackLog) | Where-Object { $_.kind -eq 'shutdown_complete' -and $_.complete }).Count -eq 1)
    $unwritableLog = Join-Path $run 'directory-not-a-log'
    [IO.Directory]::CreateDirectory($unwritableLog) | Out-Null
    $failedLogCheck = Native @('callback-check','--log',$unwritableLog)
    $failedLogEvent = $failedLogCheck.lines[0] | ConvertFrom-Json
    Check 'native diagnostic write failure cannot bypass teardown and remains unsuccessful' ($failedLogCheck.exit_code -eq 10 -and
        $failedLogEvent.log_io_failed -and $failedLogEvent.shutdown_returned)
    $denied = [pscustomobject]@{ outcome = 'exited'; events = @(
        [pscustomobject]@{ kind = 'hresult'; stage = 'environment_callback'; failed = $true; hresult = '0x80070020'; elapsed_ms = 20 }) }
    $verdict = Get-BlockedObservation $denied
    Check 'explicit startup failure recognized' ($verdict.blocked -and $verdict.responsive -and $verdict.denial_kind -eq 'EXPLICIT_STARTUP_FAILURE')
    $pending = [pscustomobject]@{ outcome = 'exited'; events = @(
        @(0..40 | ForEach-Object { [pscustomobject]@{ kind = 'heartbeat'; elapsed_ms = $_ * 500 } }) +
        @([pscustomobject]@{ kind = 'startup_timeout'; elapsed_ms = 20000 })) }
    $verdict = Get-BlockedObservation $pending
    Check 'bounded no-page remains distinct from HRESULT failure' ($verdict.blocked -and $verdict.responsive -and $verdict.denial_kind -eq 'BOUNDED_NO_PAGE')
    $pending.outcome = 'external_timeout'
    Check 'external watchdog cannot pass responsiveness' (-not (Get-BlockedObservation $pending).responsive)
    $stalled = [pscustomobject]@{ outcome = 'exited'; events = @([pscustomobject]@{ kind = 'startup_timeout'; elapsed_ms = 20000 }) }
    Check 'missing heartbeat cannot pass no-page test' (-not (Get-BlockedObservation $stalled).responsive)
    $otherFailure = [pscustomobject]@{ outcome = 'exited'; events = @(
        [pscustomobject]@{ kind = 'hresult'; stage = 'resource_response_create'; failed = $true; elapsed_ms = 50 }) }
    Check 'resource serving failure is not lock contention evidence' (-not (Get-BlockedObservation $otherFailure).blocked)
    $reached = [pscustomobject]@{ outcome = 'storage_result'; events = @($denied.events + @(
        [pscustomobject]@{ kind = 'page_message'; elapsed_ms = 50; data = [pscustomobject]@{ kind = 'page_started' } })) }
    Check 'any page message defeats blocked verdict' (-not (Get-BlockedObservation $reached).blocked)
    # Load function definitions only, never the runner's executable scenario block.
    $runnerTokens = $null; $runnerErrors = $null
    $runnerAst = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'run.ps1'), [ref]$runnerTokens, [ref]$runnerErrors)
    foreach ($definition in $runnerAst.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
        . ([scriptblock]::Create($definition.Extent.Text))
    }
    $script:RunClock = [Diagnostics.Stopwatch]::StartNew()
    $script:Children = [Collections.Generic.List[object]]::new(); $script:Udfs = [Collections.Generic.List[string]]::new()
    $script:Scenarios = [Collections.Generic.List[object]]::new()
    $script:CleanupActions = [Collections.Generic.List[object]]::new()
    $cleanupErrors = [Collections.Generic.List[string]]::new()
    $ObserveSeconds = 5
    $cleanupUdf = Join-Path $run 'cooperative-cleanup-udf'
    [IO.Directory]::CreateDirectory((Join-Path $cleanupUdf 'EBWebView')) | Out-Null
    $cleanupHolder = Start-Holder $cleanupUdf $true 'cooperative-cleanup'
    Stop-ProbeChild $cleanupHolder
    Save-ChildDiagnostics $cleanupHolder
    Check 'normal cleanup sends release and never kills native helper' ($cleanupHolder.process.HasExited -and
        $cleanupHolder.process.ExitCode -eq 0 -and $script:CleanupActions.Count -eq 0)
    $abortParent = Start-Holder $cleanupUdf $true 'cooperative-abort-parent'
    try {
        $abortWatcher = Start-LockWatcher $cleanupUdf $abortParent @($abortParent.process.Id) 'cooperative-abort' -Capture
        Stop-ProbeChild $abortWatcher
        $abortEvents = @(Read-Events $abortWatcher.log)
        Check 'incomplete watcher aborts before quiet without Kill or false protocol completion' ($abortWatcher.process.ExitCode -eq 3 -and
            $script:CleanupActions.Count -eq 0 -and @($abortEvents | Where-Object { $_.kind -eq 'watch_end' -and $_.aborted -and -not $_.protocol_complete }).Count -eq 1)
    } finally { Release-Holder $abortParent }
    $fakeTimedOut = [pscustomobject]@{Id=987654;HasExited=$false;ExitCode=0;waits=0;killed=$false}
    $fakeTimedOut | Add-Member ScriptMethod WaitForExit { param($Milliseconds); $this.waits++; return $this.killed }
    $fakeTimedOut | Add-Member ScriptMethod Kill { $this.killed=$true; $this.HasExited=$true }
    Stop-ProbeChild ([pscustomobject]@{process=$fakeTimedOut;log=(Join-Path $run 'synthetic-timeout');udf=$cleanupUdf})
    Check 'last-resort Kill follows bounded failed wait and records both failures' ($fakeTimedOut.waits -eq 2 -and $fakeTimedOut.killed -and
        $script:CleanupActions.Count -eq 2 -and $script:CleanupActions[0].kind -eq 'probe_wait_timeout' -and $script:CleanupActions[1].kind -eq 'last_resort_kill')
    $fakeShutdownLog = Join-Path $run 'synthetic-b-shutdown.jsonl'
    [IO.File]::WriteAllText($fakeShutdownLog,"{`"kind`":`"shutdown_complete`",`"complete`":false,`"timed_out`":true}`n")
    $badShutdownRejected = $false
    try { Assert-HostShutdown ([pscustomobject]@{log=$fakeShutdownLog;udf=$cleanupUdf;process=$fakeTimedOut}) } catch { $badShutdownRejected=$true }
    Check 'failed host natural-exit wait is rejected even after host process exited' $badShutdownRejected
    [IO.File]::WriteAllText($fakeShutdownLog,"{`"kind`":`"shutdown_complete`",`"complete`":true}`n{`"kind`":`"hresult`",`"stage`":`"late_controller_close`",`"failed`":true}`n")
    $closeFailureRejected = $false
    try { Assert-HostShutdown ([pscustomobject]@{log=$fakeShutdownLog;udf=$cleanupUdf;process=$fakeTimedOut}) } catch { $closeFailureRejected=$true }
    Check 'late controller Close failure cannot pass blocked startup' $closeFailureRejected
    [IO.File]::WriteAllText($fakeShutdownLog,"{`"kind`":`"shutdown_complete`",`"complete`":true}`n")
    $exitSixRejected = $false
    try { Assert-HostShutdown ([pscustomobject]@{log=$fakeShutdownLog;udf=$cleanupUdf;process=@{Id=987654;ExitCode=6}}) } catch { $exitSixRejected=$true }
    Check 'native shutdown exit6 cannot pass even with superficially complete event' $exitSixRejected
    $exceptionExitRejected = $false
    try { Assert-HostShutdown ([pscustomobject]@{log=$fakeShutdownLog;udf=$cleanupUdf;process=@{Id=987654;ExitCode=10}}) } catch { $exceptionExitRejected=$true }
    Check 'exception exit10 cannot pass after successful cleanup' $exceptionExitRejected
    foreach ($allowed in @(0,2,3)) {
        Assert-HostShutdown ([pscustomobject]@{log=$fakeShutdownLog;udf=$cleanupUdf;process=@{Id=987654;ExitCode=$allowed}})
        Check ('clean normal or expected startup exit is accepted: ' + $allowed) $true
    }
    [IO.File]::AppendAllText($fakeShutdownLog,"{`"kind`":`"host_error`",`"error`":`"SYNTHETIC callback failure`"}`n")
    $hostErrorRejected = $false
    try { Assert-HostShutdown ([pscustomobject]@{log=$fakeShutdownLog;udf=$cleanupUdf;process=@{Id=987654;ExitCode=2}}) } catch { $hostErrorRejected=$true }
    Check 'host error cannot be hidden behind allowed startup failure exit2' $hostErrorRejected
    $script:CleanupActions.Clear()
    $script:Children.Clear(); $script:Udfs.Clear()
    $werMock = [pscustomobject]@{ProcessId=123;CreationDate=[DateTime]::UtcNow;CommandLine='WerFault.exe -p 456'}
    $eventMock = [pscustomobject]@{RecordId=77;Id=1000;ProviderName='Application Error';TimeCreated=[DateTime]::UtcNow;Message='probe.exe synthetic crash'}
    $eventMock | Add-Member ScriptMethod ToXml { '<Event><Data>probe.exe</Data></Event>' }
    try {
        function Get-CimInstance { param($ClassName,$Filter,$ErrorAction); return $werMock }
        function Get-WinEvent { param($FilterHashtable,$ErrorAction); return $eventMock }
        $mockCrash = Get-CrashObservation ([DateTime]::UtcNow.AddMinutes(-1))
        Check 'WER observation preserves process and matching Application event XML' ($mockCrash.complete -and $mockCrash.werfault.Count -eq 1 -and
            $mockCrash.application_events.Count -eq 1 -and $mockCrash.application_events[0].record_id -eq 77)
        function Get-WinEvent { param($FilterHashtable,$ErrorAction); throw 'synthetic Application access denied' }
        $unavailableCrash = Get-CrashObservation ([DateTime]::UtcNow)
        Check 'unavailable error-log observation is incomplete rather than no dialog' (-not $unavailableCrash.complete -and $unavailableCrash.errors.Count -eq 1)
    } finally { Remove-Item Function:\Get-CimInstance,Function:\Get-WinEvent }
    $runText = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run.ps1'))
    Check 'forced scenario and parameter are absent from runner' ($runText -notmatch 'ForcedAttempts|Invoke-ForcedRepeats|Invoke-Scenario ''a-forced''')
    $timedAttempt = Invoke-ProbeAttempt 'synthetic-cleanup-timeout' 1 {
        Add-CleanupFailure 'synthetic_wait_timeout' 987654 'SYNTHETIC no process killed.'
        [ordered]@{status='PASS'}
    }
    Check 'successful observation with cleanup timeout becomes attempt ERROR' ($timedAttempt.status -eq 'ERROR' -and $timedAttempt.cleanup_failures.Count -eq 1)
    $script:CleanupActions.Clear()
    $churnLog = Join-Path $run 'churn-progress-fixture.jsonl'
    $goodPulse = [pscustomobject]@{kind='page_message';elapsed_ms=20;data=@{kind='churn_progress';completed_sequence=20}}
    [IO.File]::WriteAllText($churnLog,((ConvertTo-Json -InputObject $goodPulse -Compress) + "`n"))
    $churnFixture = [pscustomobject]@{log=$churnLog;process=[pscustomobject]@{HasExited=$false}}
    Check 'recent completed churn pulse is accepted before shutdown' ((Require-Churn $churnFixture 10).data.completed_sequence -eq 20)
    $stoppedChurn = [pscustomobject]@{kind='page_message';elapsed_ms=21;data=@{kind='storage_result';mode='churn';pass=$false;error='synthetic OPFS failure'}}
    [IO.File]::AppendAllText($churnLog,((ConvertTo-Json -InputObject $stoppedChurn -Compress) + "`n"))
    $stoppedRejected = $false
    try { $null = Require-Churn $churnFixture 10 } catch { $stoppedRejected=$true }
    Check 'progress followed by known churn failure cannot start shutdown measurement' ($stoppedRejected -and [IO.File]::Exists($churnLog+'.churn-failure.json'))
    $pendingPipe = [Threading.Tasks.TaskCompletionSource[string]]::new()
    Check 'pending redirected writer does not block diagnostics' (-not (Get-PipeObservation $pendingPipe.Task).eof)
    $pendingPipe.SetResult('done')
    Check 'pipe result read only after completion' ((Get-PipeObservation $pendingPipe.Task).text -eq 'done')
    $faultedPipe = [Threading.Tasks.TaskCompletionSource[string]]::new()
    $faultedPipe.SetException([InvalidOperationException]::new('offline pipe failure'))
    Check 'faulted pipe result is recorded without throwing' ((Get-PipeObservation $faultedPipe.Task).status -eq 'Faulted')
    $canceledPipe = [Threading.Tasks.TaskCompletionSource[string]]::new(); $canceledPipe.SetCanceled()
    Check 'canceled pipe result is recorded without throwing' ((Get-PipeObservation $canceledPipe.Task).status -eq 'Canceled')
    $pipeOut = [Threading.Tasks.TaskCompletionSource[string]]::new()
    $pipeErr = [Threading.Tasks.TaskCompletionSource[string]]::new()
    $fakePipeChild = [pscustomobject]@{ process = [pscustomobject]@{HasExited=$true;Id=999;ExitCode=0};
        log=(Join-Path $run 'pending-pipe'); started_ms=0; diagnostics_saved=$false;
        stdout=$pipeOut.Task; stderr=$pipeErr.Task; launch=@{}; diagnostic_samples=[Collections.Generic.List[object]]::new() }
    Save-ChildDiagnostics $fakePipeChild
    Check 'exited host with live pipe writers saves diagnostics immediately' ([IO.File]::Exists($fakePipeChild.log + '.process.json') -and -not $fakePipeChild.diagnostics_saved)
    $pipeOut.SetResult('out'); $pipeErr.SetResult('')
    Save-ChildDiagnostics $fakePipeChild
    Check 'later EOF updates preserve earlier pending observation' ($fakePipeChild.diagnostics_saved -and $fakePipeChild.diagnostic_samples.Count -eq 2)
    $parentUdf = Join-Path $run 'watch-parent-udf'
    [IO.Directory]::CreateDirectory((Join-Path $parentUdf 'EBWebView')) | Out-Null
    $activeHolder = Holder $udf $true 'fake-browser'
    $fakeParent = Holder $parentUdf $true 'fake-parent'
    try {
        $ObserveSeconds = 5
        $nativeWatcher = Start-LockWatcher $udf $fakeParent @($activeHolder.process.Id) 'offline-native'
        Stop-OfflineProcess $fakeParent.process $fakeParent.release
        $watchDeadline = [Diagnostics.Stopwatch]::StartNew(); $orphanSamples = @()
        while ($watchDeadline.ElapsedMilliseconds -lt 3000 -and $orphanSamples.Count -eq 0) {
            $orphanSamples = @(Read-Events $nativeWatcher.log | Where-Object { $_.kind -eq 'watch_sample' -and
                -not $_.parent_alive_before -and -not $_.parent_alive_after -and $_.alive_after -gt 0 -and $_.win32_error -eq 32 })
            Start-Sleep -Milliseconds 10
        }
        Check 'native watcher observes exclusion after simulated parent exit' ($orphanSamples.Count -gt 0)
        Release $activeHolder; $activeHolder = $null
        Check 'native watcher ends after simulated browser exits' $nativeWatcher.process.WaitForExit(3000)
        $watchEvents = @(Read-Events $nativeWatcher.log)
        Check 'watcher observes lock availability after natural helper exit' (@($watchEvents | Where-Object { $_.kind -eq 'watch_end' -and $_.complete }).Count -eq 1)
    } finally {
        if (-not $fakeParent.process.HasExited) { Stop-OfflineProcess $fakeParent.process $fakeParent.release }
        $fakeParent.process.Dispose()
    }
    # Handler-level lifecycle test: real native lock helpers stand in for host
    # and browser. No WebView2 is created, and the real parent orchestration runs.
    $functionBackups = @{}
    foreach ($name in @('New-Udf','Start-Host','Require-Ready','Require-Churn','Get-UdfProcesses','Try-Lock','Start-LockWatcher','Wait-HostShutdown')) {
        if ($name -eq 'Get-UdfProcesses') { $functionBackups[$name] = (Get-Item ('Function:\' + $name)).Definition; continue }
        $functionBackups[$name] = @($runnerAst.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $false))[0].Extent.Text
    }
    . ([scriptblock]::Create($functionBackups['Try-Lock'].Replace('function Try-Lock', 'function Test-OriginalTryLock')))
    . ([scriptblock]::Create($functionBackups['Start-LockWatcher'].Replace('function Start-LockWatcher', 'function Test-OriginalWatcher')))
    $script:SingleBrowserUdf = Join-Path $run 'single-observer-browser'
    $singleParentUdf = Join-Path $run 'single-observer-parent'
    [IO.Directory]::CreateDirectory((Join-Path $script:SingleBrowserUdf 'EBWebView')) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $singleParentUdf 'EBWebView')) | Out-Null
    $activeHolder = Holder $script:SingleBrowserUdf $true 'single-browser' 4000
    $script:SingleBrowser = $activeHolder
    $script:SingleParent = Start-Holder $singleParentUdf $true 'single-parent'
    $script:ObserverStarted = $false
    $script:LateHelper = $null
    try {
        function New-Udf { param($Name); if($Name -like '*reference-copy'){return (Join-Path $run $Name)}; return $script:SingleBrowserUdf }
        function Start-Host { param($Udf,$Mode,$Label); return $script:SingleParent }
        function Wait-HostShutdown {
            param($Child)
            if(-not $Child.process.WaitForExit(3000)){throw 'Synthetic cooperative parent did not exit'}
            [IO.File]::AppendAllText($Child.log,"{`"kind`":`"close_requested`",`"synthetic`":true}`n{`"kind`":`"host_exit`",`"synthetic`":true}`n")
        }
        function Require-Ready { param($Child) }
        function Require-Churn { param($Child,$AfterElapsed); [pscustomobject]@{kind='page_message';elapsed_ms=1;data=@{kind='churn_progress';completed_sequence=10}} }
        function Get-UdfProcesses {
            param($Udf,[switch]$IncludeRelated)
            if (-not $script:SingleBrowser.process.HasExited) {
                [pscustomobject]@{pid=$script:SingleBrowser.process.Id;parent_pid=$script:SingleParent.process.Id;
                    creation_utc=$script:SingleBrowser.process.StartTime.ToUniversalTime().ToString('o');is_browser=$true;role='browser';command_line='SYNTHETIC native lock helper'}
            }
        }
        function Try-Lock {
            param($Udf,[int[]]$ProcessIds=@(),[bool]$CreateNew=$false)
            if ($script:ObserverStarted) { throw 'Competing exclusive observer after watcher started' }
            Test-OriginalTryLock $Udf $ProcessIds $CreateNew
        }
        function Start-LockWatcher {
            param($Udf,$HostChild,[int[]]$Ids,$Label,[switch]$Capture)
            $child = Test-OriginalWatcher $Udf $HostChild $Ids $Label -Capture:$Capture
            $script:ObserverStarted = $true
            return $child
        }
        $ObserveSeconds = 8
        $script:SingleParent | Add-Member -NotePropertyName stop -NotePropertyValue $script:SingleParent.release
        $singleObserverResult = Invoke-Lifetime 'graceful' 99
        Check 'parent orchestration never opens lock after watcher starts' ($script:ObserverStarted -and $singleObserverResult.fast_completed)
        Check 'synthetic parent closes cooperatively while watcher continues' $script:SingleParent.process.HasExited
        # The helper explicitly closes its lock before logging/returning, so an
        # alive-but-unlocked tail can correctly be detected as an invariant failure.
        Check 'observed live-process unlock is never hidden by later exit' (
            ($singleObserverResult.fast_violations -gt 0 -and $singleObserverResult.status -eq 'FAIL') -or
            ($singleObserverResult.fast_violations -eq 0 -and $singleObserverResult.status -eq 'PASS'))
        $activeHolder.process.Dispose(); $activeHolder = $null
        $script:SingleBrowser = $null
        $script:SingleParent = Start-Holder $singleParentUdf $true 'single-parent-late'
        $script:SingleParent | Add-Member -NotePropertyName stop -NotePropertyValue $script:SingleParent.release
        $activeHolder = Holder $script:SingleBrowserUdf $true 'single-browser-late' 1800
        $script:SingleBrowser = $activeHolder
        $script:LateUdf = Join-Path $run 'synthetic-late-helper-udf'
        [IO.Directory]::CreateDirectory((Join-Path $script:LateUdf 'EBWebView')) | Out-Null
        function Get-UdfProcesses {
            param($Udf,[switch]$IncludeRelated)
            if (-not $script:SingleBrowser.process.HasExited) {
                [pscustomobject]@{pid=$script:SingleBrowser.process.Id;parent_pid=$script:SingleParent.process.Id;
                    creation_utc=$script:SingleBrowser.process.StartTime.ToUniversalTime().ToString('o');is_browser=$true;role='browser';command_line='SYNTHETIC browser helper'}
            } elseif ([IO.File]::Exists((Join-Path $run 'a-graceful-r100-quiet.signal'))) {
                if ($null -eq $script:LateHelper) { $script:LateHelper = Start-Holder $script:LateUdf $true 'synthetic-late-helper' }
                [pscustomobject]@{pid=$script:LateHelper.process.Id;parent_pid=$script:SingleParent.process.Id;
                    creation_utc=$script:LateHelper.process.StartTime.ToUniversalTime().ToString('o');is_browser=$false;
                    role='network-service';process_type='utility';utility_subtype='network.mojom.NetworkService';command_line='SYNTHETIC late helper'}
            }
        }
        $script:ObserverStarted = $false
        $lateResult = Invoke-Lifetime 'graceful' 100
        Check 'late matching process invalidates reference quiet claim' ($lateResult.reference_quiet_copy.status -eq 'INVALIDATED_NEW_MATCHING_PROCESS_AFTER_QUIET' -and $lateResult.status -ne 'PASS')
        Check 'new process after acquisition retains role creation and live state' (@($lateResult.arrivals_after_acquisition | Where-Object { $_.role -eq 'network-service' -and $_.still_alive }).Count -eq 1)
    } finally {
        Stop-ProbeChild $script:SingleParent
        if($null -ne $script:LateHelper){Stop-ProbeChild $script:LateHelper}
        if($null -ne $activeHolder){
            if (-not $activeHolder.process.HasExited) { Stop-OfflineProcess $activeHolder.process $activeHolder.release }
            $activeHolder.process.Dispose(); $activeHolder = $null
        }
        foreach ($name in $functionBackups.Keys) {
            if ($name -eq 'Get-UdfProcesses') { Set-Item ('Function:\' + $name) ([scriptblock]::Create($functionBackups[$name])) }
            else { . ([scriptblock]::Create($functionBackups[$name])) }
        }
        Remove-Item Function:\Test-OriginalTryLock,Function:\Test-OriginalWatcher
    }
    Check 'all matching process lifetimes include retained OS exit delta' ($singleObserverResult.all_matching_process_lifetimes.Count -eq 1 -and
        $null -ne $singleObserverResult.all_matching_process_lifetimes[0].exit_minus_acquisition_ms)
    Check 'acquisition capture begins after acquisition timestamp' ($singleObserverResult.acquisition_to_capture_start_ms -ge 0)
    Check 'reference copy remains separate from strict lifecycle verdict' ($singleObserverResult.reference_quiet_copy.status -eq 'UNCHANGED_DURING_QUIET_COPY')
    $snapshotUdf = Join-Path $run 'capture-fixture'
    [IO.Directory]::CreateDirectory((Join-Path $snapshotUdf 'EBWebView\Default')) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $snapshotUdf 'EBWebView\EBWebViewMetrics')) | Out-Null
    $profileFile = Join-Path $snapshotUdf 'EBWebView\Default\Preferences'
    $metricFile = Join-Path $snapshotUdf 'EBWebView\EBWebViewMetrics\EBWebViewMetrics-fixture.pma'
    [IO.File]::WriteAllText($profileFile, 'before-profile')
    [IO.File]::WriteAllText($metricFile, 'before-metrics')
    $expectedCapture = Get-Manifest $snapshotUdf
    $captureParentUdf = Join-Path $run 'capture-parent-udf'
    [IO.Directory]::CreateDirectory((Join-Path $captureParentUdf 'EBWebView')) | Out-Null
    $activeHolder = Holder $snapshotUdf $true 'capture-fake-browser'
    $captureParent = Holder $captureParentUdf $true 'capture-fake-parent'
    $captureWatcher = $null
    try {
        $ObserveSeconds = 10
        $captureWatcher = Start-LockWatcher $snapshotUdf $captureParent @($activeHolder.process.Id) 'capture-fixture' -Capture
        Stop-OfflineProcess $captureParent.process $captureParent.release
        Release $activeHolder; $activeHolder = $null
        $waitCapture = [Diagnostics.Stopwatch]::StartNew()
        while ($waitCapture.ElapsedMilliseconds -lt 5000 -and @((Read-Events $captureWatcher.log) | Where-Object kind -eq 'acquisition_snapshot_done').Count -eq 0) { Start-Sleep -Milliseconds 10 }
        $atAcquisition = Read-Capture $captureWatcher.capture_prefix 'acquired'
        Check 'native acquisition snapshot covers every non-lock file' ($atAcquisition.complete -and (Compare-Manifests $expectedCapture $atAcquisition.files).equal)
        Check 'BCrypt snapshot rows carry per-file read timestamps' ($atAcquisition.files.Count -eq 2 -and
            [long]$atAcquisition.files[0].read_started.qpc -le [long]$atAcquisition.files[0].read_finished.qpc)
        Check 'native snapshot records before and after write timestamps' ($atAcquisition.files[0].write_time_before -and $atAcquisition.files[0].write_time_after)
        $stillHeld = Lock-Try $snapshotUdf
        Check 'first acquired lock is retained across snapshot phases' ($stillHeld.events[0].win32_error -eq 32)
        [IO.File]::WriteAllText($profileFile, 'after-profile')
        [IO.File]::WriteAllText($metricFile, 'after-metrics')
        [IO.File]::WriteAllText($captureWatcher.quiet, 'synthetic observed quiet')
        $waitCapture.Restart()
        while ($waitCapture.ElapsedMilliseconds -lt 5000 -and @((Read-Events $captureWatcher.log) | Where-Object kind -eq 'reference_copy_done').Count -eq 0) { Start-Sleep -Milliseconds 10 }
        $afterQuiet = Read-Capture $captureWatcher.capture_prefix 'quiet'
        $afterCopy = Read-Capture $captureWatcher.capture_prefix 'after-copy'
        $referenceCopy = Read-Capture $captureWatcher.capture_prefix 'copy'
        $delta = Compare-Snapshots $atAcquisition $afterQuiet
        $categories = Get-ChangeSummary $delta
        Check 'changes after first acquisition remain recorded' (-not $delta.equal -and $categories.profile_changed -and $categories.crashpad_metrics_changed)
        Check 'quiet reference copy does not erase earlier source differences' ((Compare-Snapshots $afterQuiet $afterCopy).equal -and (Compare-Snapshots $afterQuiet $referenceCopy).equal -and -not $delta.equal)
        Check 'native reference copy excludes exact lockfile' (-not [IO.File]::Exists((Join-Path $captureWatcher.copy_udf 'EBWebView\lockfile')))
        $stillHeld = Lock-Try $snapshotUdf
        Check 'lock remains held until post-copy release signal' ($stillHeld.events[0].win32_error -eq 32)
        [IO.File]::WriteAllText($captureWatcher.release, 'synthetic release')
        Check 'capture protocol has bounded normal completion' $captureWatcher.process.WaitForExit(3000)
        $done = @(Read-Events $captureWatcher.log | Where-Object { $_.kind -eq 'watch_end' -and $_.observation_complete })
        Check 'reference protocol completion is explicit in evidence' ($done.Count -eq 1)
    } finally {
        if ($null -ne $captureWatcher) { Stop-ProbeChild $captureWatcher; $null = $script:Udfs.Remove($captureWatcher.copy_udf) }
        if (-not $captureParent.process.HasExited) { Stop-OfflineProcess $captureParent.process $captureParent.release }
        $captureParent.process.Dispose()
    }
    $busyProfile = [IO.FileStream]::new($profileFile,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    try {
        $snapshotPath = Join-Path $run 'unreadable-snapshot.json'
        $failedCapture = Native @('snapshot','--udf',$snapshotUdf,'--manifest',$snapshotPath)
        $snapshot = Get-Content -LiteralPath $snapshotPath -Raw -Encoding UTF8 | ConvertFrom-Json
        Check 'unreadable native snapshot never claims completeness' ($failedCapture.exit_code -eq 2 -and -not $snapshot.complete -and @($snapshot.files | Where-Object read_error).Count -eq 1)
        Check 'matching incomplete native snapshots cannot prove invariance' (-not (Compare-Snapshots $snapshot $snapshot).equal)
        Check 'missing snapshot cannot confirm file additions from partial evidence' ($null -eq (Get-ChangeSummary (Compare-Snapshots $null $snapshot)).profile_changed)
    } finally { $busyProfile.Dispose() }
    # An unreadable acquisition capture must not conceal a separately complete
    # and unchanged reference copy after the observed quiet barrier.
    $activeHolder = Holder $snapshotUdf $true 'incomplete-acquisition-browser'
    $incompleteParent = Holder $captureParentUdf $true 'incomplete-acquisition-parent'
    $busyProfile = [IO.FileStream]::new($profileFile,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    $incompleteWatcher = $null
    try {
        $incompleteWatcher = Start-LockWatcher $snapshotUdf $incompleteParent @($activeHolder.process.Id) 'incomplete-acquisition' -Capture
        Stop-OfflineProcess $incompleteParent.process $incompleteParent.release
        Release $activeHolder; $activeHolder = $null
        $waitCapture = [Diagnostics.Stopwatch]::StartNew()
        while ($waitCapture.ElapsedMilliseconds -lt 5000 -and @((Read-Events $incompleteWatcher.log) | Where-Object kind -eq 'acquisition_snapshot_done').Count -eq 0) { Start-Sleep -Milliseconds 10 }
        $badFirst = Read-Capture $incompleteWatcher.capture_prefix 'acquired'
        Check 'incomplete acquisition is retained as failed evidence' (-not $badFirst.complete)
        $busyProfile.Dispose(); $busyProfile = $null
        [IO.File]::WriteAllText($incompleteWatcher.quiet,'synthetic later quiet')
        $waitCapture.Restart()
        while ($waitCapture.ElapsedMilliseconds -lt 5000 -and @((Read-Events $incompleteWatcher.log) | Where-Object kind -eq 'reference_copy_done').Count -eq 0) { Start-Sleep -Milliseconds 10 }
        [IO.File]::WriteAllText($incompleteWatcher.release,'synthetic release')
        Check 'incomplete first capture still reaches bounded reference completion' $incompleteWatcher.process.WaitForExit(3000)
        $end = @(Read-Events $incompleteWatcher.log | Where-Object kind -eq 'watch_end')[-1]
        Check 'reference completion is independent of acquisition readability' ($end.protocol_complete -and $end.reference_observation_complete -and -not $end.acquisition_capture_complete -and -not $end.observation_complete)
        $q = Read-Capture $incompleteWatcher.capture_prefix 'quiet'
        $copy = Read-Capture $incompleteWatcher.capture_prefix 'copy'
        Check 'separate quiet copy comparison can be equal despite failed first capture' ((Compare-Snapshots $q $copy).equal -and -not (Compare-Snapshots $badFirst $q).equal)
    } finally {
        if($null -ne $busyProfile){$busyProfile.Dispose()}
        if($null -ne $incompleteWatcher){Stop-ProbeChild $incompleteWatcher;$null=$script:Udfs.Remove($incompleteWatcher.copy_udf)}
        if(-not $incompleteParent.process.HasExited){Stop-OfflineProcess $incompleteParent.process $incompleteParent.release}
        $incompleteParent.process.Dispose()
    }
    $trackedFixture = @{}; $missingFixture = @{}
    $selfProcess = [Diagnostics.Process]::GetCurrentProcess()
    try {
        $wrongRow = [pscustomobject]@{pid=$selfProcess.Id;creation_utc=$selfProcess.StartTime.ToUniversalTime().AddSeconds(-1).ToString('o');role='renderer'}
        Register-MatchingProcesses @($wrongRow) $trackedFixture $missingFixture $script:RunClock
        Check 'PID creation mismatch is kept as unknown instead of attributed' ($trackedFixture.Count -eq 0 -and $missingFixture.Count -eq 1)
    } finally { $selfProcess.Dispose() }
    $node = Get-Command node -ErrorAction SilentlyContinue
    if ($node) {
        $sourceScript = [regex]::Match([IO.File]::ReadAllText((Join-Path $PSScriptRoot 'page.html')), '(?s)<script>(.*?)</script>').Groups[1].Value
        $jsPath = Join-Path $run 'churn-syntax.js'; [IO.File]::WriteAllText($jsPath,$sourceScript)
        & $node.Source --check $jsPath
        Check 'churn page JavaScript parses without launching a browser' ($LASTEXITCODE -eq 0)
    }
    $runnerHolder = Start-Holder $udf $true 'runner-holder'
    Release-Holder $runnerHolder
    Check 'runner starts/releases native holder and writes diagnostics' ([IO.File]::Exists($runnerHolder.log + '.process.json') -and -not [IO.File]::Exists($lock))
    Invoke-Scenario 'offline-return' { [ordered]@{ status = 'PASS'; explanation = 'synthetic offline case' } }
    Invoke-Scenario 'offline-exception' { throw 'deliberate synthetic failure' }
    Check 'runner exception cannot become successful scenario' ($script:Scenarios.Count -eq 2 -and $script:Scenarios[1].status -eq 'ERROR')
    $originalLifetime = @($runnerAst.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Invoke-Lifetime' }, $false))[0].Extent.Text
    try {
        function Invoke-Lifetime { param($ExitMode, $Round); [ordered]@{status='PASS';orphan_coverage=$(if($Round -ge 2){'OBSERVED'}else{'NOT_OBSERVED'})} }
        $repeated = Invoke-LifetimeRepeats 'graceful' 4
        Check 'graceful repeats run all requested attempts' ($repeated.attempts_run -eq 4 -and $repeated.status -eq 'PASS')
        function Invoke-Lifetime { param($ExitMode, $Round); [ordered]@{status=$(if($Round -eq 1){'FAIL'}else{'PASS'});orphan_coverage=$(if($Round -ge 2){'OBSERVED'}else{'NOT_OBSERVED'})} }
        $repeated = Invoke-LifetimeRepeats 'graceful' 4
        Check 'graceful repeats retain earlier violation and run all attempts' ($repeated.status -eq 'FAIL' -and $repeated.attempts.Count -eq 4)
        function Invoke-Lifetime { param($ExitMode, $Round); if($Round -eq 1){throw 'synthetic attempt failure'}; [ordered]@{status='PASS';orphan_coverage='OBSERVED'} }
        $repeated = Invoke-LifetimeRepeats 'graceful' 4
        Check 'graceful repeats retain earlier error and run all attempts' ($repeated.status -eq 'ERROR' -and $repeated.attempts.Count -eq 4)
        $repeatedGraceful = Invoke-LifetimeRepeats 'graceful' 5
        Check 'graceful repeats also finish all five attempts despite first error' ($repeatedGraceful.status -eq 'ERROR' -and $repeatedGraceful.attempts_run -eq 5)
        function Invoke-Lifetime { param($ExitMode, $Round); [ordered]@{status='PASS';orphan_coverage='NOT_OBSERVED'} }
        $repeated = Invoke-LifetimeRepeats 'graceful' 4
        Check 'healthy graceful attempts do not claim forced orphan coverage' ($repeated.status -eq 'PASS' -and $repeated.orphan_coverage -eq 'NOT_OBSERVED')
    } finally { . ([scriptblock]::Create($originalLifetime)) }
    foreach ($child in $script:Children) { $child.process.Dispose() }
    $reportDirectory = Join-Path $run 'synthetic-report'
    [IO.Directory]::CreateDirectory($reportDirectory) | Out-Null
    $checks = [ordered]@{ localStorage = $true; indexedDB = $true; opfs = $true }
    $compare = [ordered]@{ equal = $true; before_files = 2; after_files = 2; differences = @() }
    $fixtures = @(
        [pscustomobject]@{ name = 'a-fixture'; status = 'PASS'; elapsed_ms = 10; detail = [ordered]@{
            active_exclusion_samples = 2; orphan_exclusion_samples = 1; orphan_coverage = 'OBSERVED'; browser_lifetimes = @(); violations = 0 } },
        [pscustomobject]@{ name = 'b-fixture'; status = 'PASS'; elapsed_ms = 10; detail = [ordered]@{
            blocked = $true; responsive = $true; denial_kind = 'EXPLICIT_STARTUP_FAILURE'; startup = $denied;
            hash_comparison_while_held = $compare; hash_comparison_after_release = $compare } },
        [pscustomobject]@{ name = 'c-fixture'; status = 'PASS'; elapsed_ms = 10; detail = [ordered]@{
            source_unchanged = $compare; copy_equal = $compare; written = @{checks=$checks}; read_Y = @{checks=$checks}; read_X = @{checks=$checks} } },
        [pscustomobject]@{ name = 'error-fixture'; status = 'ERROR'; elapsed_ms = 10; detail = @{error='Synthetic fixture only'} })
    $fixtures[0].detail = $singleObserverResult
    $fakeResult = [ordered]@{ run_directory = $reportDirectory; migration_prerequisite = 'SYNTHETIC_NOT_A_RESULT';
        scope = 'SYNTHETIC REPORT TEST ONLY. NO WEBVIEW2 WAS RUN.'; scenarios = $fixtures;
        crash_observation=@{status='OBSERVED';before=$mockCrash;after=$unavailableCrash;note='SYNTHETIC observation only'};
        cleanup_actions=@(@{kind='last_resort_kill';pid=987654;utc='SYNTHETIC';reason='SYNTHETIC no process killed'});
        event_logs=@(@{file='synthetic-host.jsonl';events=@(@{kind='shutdown_complete';complete=$false;timed_out=$true;timeout_ms=30000})});
        cleanup_errors = @(); observation_note = 'Offline report-generation fixture.' }
    Write-ProbeReport $fakeResult
    $markdown = [IO.File]::ReadAllText((Join-Path $reportDirectory 'results.md'))
    Check 'Markdown report includes timings, HRESULT and three storage checks' ($markdown.Contains('0x80070020') -and $markdown.Contains('IndexedDB') -and $markdown.Contains('OPFS') -and $markdown.Contains('Lock acquisition FILETIME'))
    Check 'Markdown distinguishes acquisition changes from quiet reference result' ($markdown.Contains('Exit minus acquisition') -and $markdown.Contains('Reference results never replace the original strict lockfile verdict'))
    Check 'Markdown reports WER evidence unknown log access and last-resort cleanup failure' ($markdown.Contains('WerFault PID 123') -and
        $markdown.Contains('Application event 1000') -and $markdown.Contains('Observation unavailable:') -and
        $markdown.Contains('Cleanup FAILURE: last_resort_kill') -and $markdown.Contains('timed_out=True'))
    Check 'machine report preserves all scenarios' ((Get-Content -LiteralPath (Join-Path $reportDirectory 'results.json') -Raw -Encoding UTF8 | ConvertFrom-Json).scenarios.Count -eq 4)
    $bDetail = $fixtures[1].detail
    $bDetail.classification_while_held = Get-ChangeSummary $metrics1
    $bDetail.classification_after_release = Get-ChangeSummary $metrics1
    $bDetail.file_diagnostics = @([pscustomobject]@{stage='while-held';started_ms=1;finished_ms=2;processes=@(
        [pscustomobject]@{pid=123;match_basis='crashpad-database';cleanup_eligible=$false});
        files=@([pscustomobject]@{relative_path='EBWebView/CrashpadMetrics.pma';observation=@{read_error=0;copy_error=32;rm_error=0;rm_users=@(@{pid=123})}})})
    $fixtures[1].detail = [ordered]@{status='FAIL';attempts_run=2;attempts=@(
        [pscustomobject]@{attempt=1;status='FAIL';detail=$bDetail;elapsed_ms=10},
        [pscustomobject]@{attempt=2;status='FAIL';detail=$bDetail;elapsed_ms=10});
        change_sets_while_held=$setSummary;change_sets_after_release=$setSummary}
    Write-ProbeReport $fakeResult
    $markdown = [IO.File]::ReadAllText((Join-Path $reportDirectory 'results.md'))
    Check 'repetition reports show raw and normalized sets separately' ($markdown.Contains('raw path sets equal=False') -and $markdown.Contains('diagnostic normalized sets equal=True'))
    Check 'repetition reports show profile/metrics and file owner diagnostics' ($markdown.Contains('profile changed=False') -and $markdown.Contains('Crashpad/metrics changed=True') -and $markdown.Contains('Restart Manager PIDs'))
    $completed = $true
} catch {
    $failure = $_.Exception.Message
    throw
} finally {
    if ($null -ne $activeHolder) {
        if (-not $activeHolder.process.HasExited) { Stop-OfflineProcess $activeHolder.process $activeHolder.release }
        $activeHolder.process.Dispose()
    }
    $result = [ordered]@{ schema_version = 1; scope = 'OFFLINE ONLY: no WebView2 environment/controller/browser launched';
        pass = ($completed -and $results.Count -gt 0 -and @($results | Where-Object { -not $_.pass }).Count -eq 0);
        completed = $completed; failure = $failure; tests = @($results) }
    Save-Json (Join-Path $run 'offline-results.json') $result
    Write-Host "Offline results: $run\offline-results.json"
}
Write-Host "PASS: $($results.Count) offline checks. WebView2 scenarios remain unexecuted."
