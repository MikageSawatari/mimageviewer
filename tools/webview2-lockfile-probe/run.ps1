param(
    [switch]$SkipBuild,
    [ValidateRange(5, 60)][int]$StartupSeconds = 20,
    [ValidateRange(5, 120)][int]$ObserveSeconds = 30,
    [ValidateRange(1, 20)][int]$GracefulAttempts = 5,
    [ValidateRange(1, 20)][int]$BlockedAttempts = 3
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
if (-not $SkipBuild) { & (Join-Path $PSScriptRoot 'build.ps1') }
$exe = Join-Path $script:ProbeRoot 'bin\probe.exe'
if (-not [IO.File]::Exists($exe)) { throw 'Build the probe first.' }
$run = Join-Path $script:ProbeRoot ('run-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff') + '-' + $PID)
Assert-ProbePath $run
[IO.Directory]::CreateDirectory($run) | Out-Null
$script:RunClock = [Diagnostics.Stopwatch]::StartNew()
$script:Children = [Collections.Generic.List[object]]::new()
$script:Udfs = [Collections.Generic.List[string]]::new()
$script:Scenarios = [Collections.Generic.List[object]]::new()
$script:CleanupActions = [Collections.Generic.List[object]]::new()
$token = [Guid]::NewGuid().ToString('D')
$build = Get-Content -LiteralPath (Join-Path $script:ProbeRoot 'bin\build.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if ((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash -ne $build.exe_sha256) { throw 'Executable differs from build.json.' }
if ((Get-FileHash -LiteralPath (Join-Path $script:ProbeRoot 'bin\page.html') -Algorithm SHA256).Hash -ne $build.page_sha256) { throw 'Page differs from build.json.' }
if ((Get-FileHash -LiteralPath (Join-Path $script:ProbeRoot 'bin\WebView2Loader.dll') -Algorithm SHA256).Hash -ne $build.loader_sha256) { throw 'Loader differs from build.json.' }
if ((Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'snapshot.h') -Algorithm SHA256).Hash -ne $build.snapshot_sha256 -or
    (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'probe.cpp') -Algorithm SHA256).Hash -ne $build.source_sha256 -or
    (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'page.html') -Algorithm SHA256).Hash -ne $build.page_sha256) {
    throw 'Probe sources differ from build.json; rerun without -SkipBuild.'
}

function New-Udf([string]$Name) {
    $path = Join-Path $run $Name
    Assert-ProbePath $path
    if ([IO.Directory]::Exists($path)) { throw "UDF already exists: $path" }
    $script:Udfs.Add($path)
    return $path
}
function Start-ProbeChild([string[]]$Arguments, [string]$LogPath) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $exe; $start.UseShellExecute = $false
    $start.CreateNoWindow = $true; $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
    $start.Arguments = (@(foreach ($arg in $Arguments) {
        if ($arg.Contains('"') -or $arg.EndsWith('\')) { throw 'Unsupported native argument quoting.' }
        '"' + $arg + '"'
    }) -join ' ')
    $process = [Diagnostics.Process]::new(); $process.StartInfo = $start
    $started = $script:RunClock.ElapsedMilliseconds
    if (-not $process.Start()) { throw 'Cannot start probe child.' }
    $child = [pscustomobject]@{ process = $process; log = $LogPath; started_ms = $started;
        stdout = $process.StandardOutput.ReadToEndAsync(); stderr = $process.StandardError.ReadToEndAsync();
        diagnostics_saved = $false; diagnostic_samples = [Collections.Generic.List[object]]::new();
        launch = [ordered]@{ use_shell_execute = $false; redirect_stdout = $true; redirect_stderr = $true;
            dotnet_version = [Environment]::Version.ToString(); webview2_bInheritHandles = 'NOT_INSTRUMENTED';
            host_standard_handle_flags_changed = $false; log_security_attributes = 'nullptr: noninheritable' } }
    $script:Children.Add($child)
    return $child
}
function Save-ChildDiagnostics($Child) {
    if ($Child.process.HasExited -and -not $Child.diagnostics_saved) {
        $stdout = Get-PipeObservation $Child.stdout
        $stderr = Get-PipeObservation $Child.stderr
        $Child.diagnostic_samples.Add([pscustomobject]@{ ms = $script:RunClock.ElapsedMilliseconds;
            stdout_status = $stdout.status; stderr_status = $stderr.status; stdout_eof = $stdout.eof; stderr_eof = $stderr.eof })
        Save-Json ($Child.log + '.process.json') ([ordered]@{ pid = $Child.process.Id;
            exit_code = $Child.process.ExitCode; started_ms = $Child.started_ms;
            exit_observed_ms = $script:RunClock.ElapsedMilliseconds;
            stdout = $stdout; stderr = $stderr; pipe_observations = @($Child.diagnostic_samples); launch = $Child.launch;
            inheritance_note = 'EOF pending after host exit is a clue, not proof of which descendant inherited a writer. No standard-handle flags were changed.' })
        $Child.diagnostics_saved = $stdout.eof -and $stderr.eof
    }
}
function Get-PipeObservation($Task) {
    $status = [string]$Task.Status
    return [pscustomobject]@{ status = $status; eof = ($status -eq 'RanToCompletion');
        text = $(if ($status -eq 'RanToCompletion') { $Task.Result } else { $null });
        error = $(if ($Task.IsFaulted) { $Task.Exception.ToString() } elseif ($Task.IsCanceled) { 'canceled' } else { $null }) }
}
function Stop-ProbeChild($Child) {
    if (-not $Child.process.HasExited) {
        if ($Child.stop) { [IO.File]::WriteAllText($Child.stop, 'cooperative cleanup close') }
        if ($Child.abort) { [IO.File]::WriteAllText($Child.abort, 'abort incomplete observation during cooperative cleanup') }
        if ($Child.release) { [IO.File]::WriteAllText($Child.release, 'cooperative cleanup release') }
        $waitMs = ($ObserveSeconds + 5) * 1000
        if ($Child.process.WaitForExit($waitMs)) { return }
        Add-CleanupFailure 'probe_wait_timeout' $Child.process.Id "Probe did not exit after cooperative request and $waitMs ms wait." $Child.udf
        Add-CleanupFailure 'last_resort_kill' $Child.process.Id 'Probe cooperative shutdown deadline exceeded.' $Child.udf
        $Child.process.Kill()
        if (-not $Child.process.WaitForExit(5000)) { throw "Probe child did not exit: $($Child.process.Id)" }
    }
}
function Assert-HostShutdown($Child) {
    $events = @(Read-Events $Child.log)
    $end = @($events | Where-Object kind -eq 'shutdown_complete')
    $closeErrors = @($events | Where-Object { $_.kind -eq 'hresult' -and $_.stage -like '*controller_close' -and $_.failed })
    $hostErrors = @($events | Where-Object kind -eq 'host_error')
    if ($end.Count -ne 1 -or -not $end[0].complete -or $Child.process.ExitCode -notin @(0,2,3) -or $closeErrors.Count -or $hostErrors.Count) {
        Add-CleanupFailure 'host_shutdown_incomplete' $Child.process.Id 'Host did not prove bounded Close/natural-exit completion.' $Child.udf
        throw 'Host shutdown was incomplete; see shutdown_complete and cleanup-actions.partial.json.'
    }
}
function Wait-HostShutdown($Child) {
    if (-not $Child.process.WaitForExit(($ObserveSeconds + 5) * 1000)) {
        Add-CleanupFailure 'host_wait_timeout' $Child.process.Id 'Host exceeded shutdown wait; cooperative cleanup follows.' $Child.udf
        Stop-ProbeChild $Child
        throw 'Host shutdown exceeded its deadline.'
    }
    Save-ChildDiagnostics $Child
    Assert-HostShutdown $Child
}
function Start-Host([string]$Udf, [string]$Mode, [string]$Label) {
    $log = Join-Path $run ($Label + '.jsonl')
    $stop = Join-Path $run ($Label + '.stop')
    $child = Start-ProbeChild @('host', '--udf', $Udf, '--mode', $Mode, '--token', $token,
        '--log', $log, '--stop', $stop, '--timeout-ms', [string]($StartupSeconds * 1000),
        '--shutdown-timeout-ms', [string]($ObserveSeconds * 1000)) $log
    $child | Add-Member -NotePropertyName stop -NotePropertyValue $stop
    $child | Add-Member -NotePropertyName udf -NotePropertyValue $Udf
    return $child
}
function Wait-Host($Child) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt ($StartupSeconds * 1000 + 5000)) {
        $events = @(Read-Events $Child.log)
        $storage = @($events | Where-Object { $_.kind -eq 'page_message' -and $_.data.kind -eq 'storage_result' })
        if ($storage.Count -gt 0) { return [pscustomobject]@{ outcome = 'storage_result'; storage = $storage[-1].data; events = $events; elapsed_ms = $watch.ElapsedMilliseconds } }
        if ($Child.process.HasExited) {
            Save-ChildDiagnostics $Child
            Assert-HostShutdown $Child
            return [pscustomobject]@{ outcome = 'exited'; storage = $null; events = @(Read-Events $Child.log); elapsed_ms = $watch.ElapsedMilliseconds }
        }
        if (@($events | Where-Object kind -eq 'host_closing').Count) {
            Wait-HostShutdown $Child
            return [pscustomobject]@{ outcome='exited';storage=$null;events=@(Read-Events $Child.log);elapsed_ms=$watch.ElapsedMilliseconds }
        }
        Start-Sleep -Milliseconds 50
    }
    Add-CleanupFailure 'startup_external_timeout' $Child.process.Id 'Runner startup deadline exceeded.' $Child.udf
    Stop-ProbeChild $Child
    Save-ChildDiagnostics $Child
    return [pscustomobject]@{ outcome = 'external_timeout'; storage = $null; events = @(Read-Events $Child.log); elapsed_ms = $watch.ElapsedMilliseconds }
}
function Require-Ready($Child) {
    $ready = Wait-Host $Child
    Save-Json ($Child.log + '.startup.json') $ready
    if ($ready.outcome -ne 'storage_result' -or -not $ready.storage.pass) { throw "Unlocked host did not verify storage: $($Child.log)" }
    $reported = @($ready.events | Where-Object kind -eq 'browser_pid' | ForEach-Object { $_.pid })
    $matched = @(Get-UdfProcesses $Child.udf | Where-Object is_browser)
    if ($matched.Count -eq 0 -or @($matched | Where-Object { $_.pid -in $reported }).Count -eq 0) {
        throw 'Browser PID did not match the exact disposable --user-data-dir.'
    }
    return $ready
}
function Require-Churn($Child, [long]$AfterElapsed = -1) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt 5000) {
        $events = @(Read-Events $Child.log)
        $failures = @($events | Where-Object { $_.kind -eq 'page_message' -and $_.data.kind -eq 'storage_result' -and
            $_.data.mode -eq 'churn' -and -not $_.data.pass })
        if ($failures.Count) {
            Save-Json ($Child.log + '.churn-failure.json') $failures
            throw 'Churn failure was reported before shutdown; see host log and churn-failure.json.'
        }
        $pulses = @($events | Where-Object { $_.kind -eq 'page_message' -and
            $_.data.kind -eq 'churn_progress' -and $_.data.completed_sequence -ge 10 -and $_.elapsed_ms -gt $AfterElapsed })
        if ($pulses.Count) { return $pulses[-1] }
        if ($Child.process.HasExited) { throw 'Churn host exited before a recent completed-write pulse.' }
        Start-Sleep -Milliseconds 10
    }
    throw 'Churn progress timed out; no shutdown measurement was started.'
}
function Close-Host($Child) {
    Assert-ProbePath $Child.stop
    [IO.File]::WriteAllText($Child.stop, 'close')
    Wait-HostShutdown $Child
    if ($Child.process.ExitCode -ne 0) { throw "Graceful host exit code: $($Child.process.ExitCode)" }
}
function Wait-Quiet([string]$Udf) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $ObserveSeconds) {
        if (@(Get-UdfProcesses $Udf).Count -eq 0) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "Disposable WebView2 processes did not exit naturally: $Udf"
}
function Try-Lock([string]$Udf, [int[]]$ProcessIds = @(), [bool]$CreateNew = $false) {
    $arguments = @('lock', '--udf', $Udf, '--create-new', $(if ($CreateNew) { '1' } else { '0' }))
    if ($ProcessIds.Count -gt 0) { $arguments += @('--pids', ($ProcessIds -join ',')) }
    $output = @(& $exe @arguments)
    $exitCode = $LASTEXITCODE
    if ($exitCode -notin @(0, 2)) { throw "Lock helper failed: $output" }
    $events = @($output | ForEach-Object { $_ | ConvertFrom-Json })
    $attempt = @($events | Where-Object kind -eq 'lock_attempt')
    if ($attempt.Count -ne 1) { throw 'Missing lock helper observation.' }
    return $attempt[0]
}
function Start-Holder([string]$Udf, [bool]$CreateNew, [string]$Label) {
    $log = Join-Path $run ($Label + '.jsonl'); $release = Join-Path $run ($Label + '.release')
    $child = Start-ProbeChild @('lock', '--udf', $Udf, '--log', $log, '--release', $release,
        '--create-new', $(if ($CreateNew) { '1' } else { '0' }), '--timeout-ms', '180000') $log
    $child | Add-Member -NotePropertyName release -NotePropertyValue $release
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt 5000) {
        $attempts = @(Read-Events $log | Where-Object kind -eq 'lock_attempt')
        if ($attempts.Count -gt 0) {
            if (-not $attempts[0].opened) { throw "Cannot acquire lock holder: $log" }
            return $child
        }
        if ($child.process.HasExited) { throw "Lock holder exited: $log" }
        Start-Sleep -Milliseconds 25
    }
    throw 'Lock holder readiness timed out.'
}
function Release-Holder($Child) {
    [IO.File]::WriteAllText($Child.release, 'release')
    if (-not $Child.process.WaitForExit(5000)) { throw 'Lock holder failed to release.' }
    Save-ChildDiagnostics $Child
    if ($Child.process.ExitCode -ne 0) { throw 'Lock holder timed out or failed.' }
}
function Add-OwnerMetadata($Observation, [int]$BudgetMs = 1500) {
    $watch = [Diagnostics.Stopwatch]::StartNew(); $queried = 0; $skipped = 0
    foreach ($owner in $Observation.rm_users) {
        $metadata = $null; $errorText = $null
        if ($queried -ge 4 -or $watch.ElapsedMilliseconds -ge $BudgetMs) { $skipped++; $errorText = 'not queried: diagnostic metadata budget' }
        else {
            $queried++
            try {
                $p = Get-CimInstance -ClassName Win32_Process -Filter ("ProcessId=" + $owner.pid) -OperationTimeoutSec 1 -ErrorAction Stop
                if ($null -ne $p) { $metadata = [pscustomobject]@{ pid = [int]$p.ProcessId; name = $p.Name;
                    creation_utc = $p.CreationDate.ToUniversalTime().ToString('o'); parent_pid = [int]$p.ParentProcessId; command_line = $p.CommandLine } }
            } catch { $errorText = $_.Exception.Message }
        }
        $owner | Add-Member -NotePropertyName process_snapshot -NotePropertyValue $metadata
        $owner | Add-Member -NotePropertyName metadata_error -NotePropertyValue $errorText
    }
    $Observation | Add-Member -NotePropertyName owner_metadata_queried -NotePropertyValue $queried
    $Observation | Add-Member -NotePropertyName owner_metadata_skipped -NotePropertyValue $skipped
    return $Observation
}
function Get-FileDiagnostics([string]$Udf, [string[]]$RelativePaths, [string]$Stage, [string]$Label) {
    $started = $script:RunClock.ElapsedMilliseconds
    $processes = @(); $inventoryError = $null
    try { $processes = @(Get-UdfProcesses $Udf -IncludeRelated) } catch { $inventoryError = $_.Exception.Message }
    $copyDirectory = Join-Path $run ($Label + '-' + $Stage + '-read-copies')
    [IO.Directory]::CreateDirectory($copyDirectory) | Out-Null
    $budget = [Diagnostics.Stopwatch]::StartNew(); $skipped = [Collections.Generic.List[string]]::new(); $processed = 0
    $files = @(foreach ($relative in @($RelativePaths | Sort-Object -Unique)) {
        if ($processed -ge 32 -or $budget.ElapsedMilliseconds -ge 30000) { $skipped.Add($relative); continue }
        $processed++
        $source = Join-Path $Udf $relative.Replace('/', '\')
        Assert-ProbePath $source
        $copy = Join-Path $copyDirectory ([Guid]::NewGuid().ToString('N') + '.bin')
        $observation = Add-OwnerMetadata (Invoke-FileProbe $exe $source $copy)
        if ($observation.cleanup_error) { $cleanupErrors.Add("Diagnostic PID $($observation.process_pid): $($observation.cleanup_error)") }
        [pscustomobject]@{ relative_path = $relative; category = (Get-ChangeCategory $relative); copy_destination = $copy; observation = $observation }
    })
    $result = [pscustomobject]@{ stage = $Stage; started_ms = $started; finished_ms = $script:RunClock.ElapsedMilliseconds;
        processes = $processes; inventory_error = $inventoryError; files = $files;
        skipped_paths = @($skipped); max_files = 32; start_budget_ms = 30000;
        budget_note = 'No new file after 30s; one already-started bounded file/metadata call can finish later. Strict manifests remain complete and uncapped.';
        ownership_limit = 'Restart Manager application users are clues, not an exhaustive handle/mapping inventory. Zero users does not prove no owner. PID creation times must be checked against later metadata.' }
    Save-Json (Join-Path $run ($Label + '-' + $Stage + '-file-diagnostics.json')) $result
    return $result
}
function Get-DiagnosticPaths($Before, $After, $Comparison) {
    return ,@(@($Comparison.differences.path) + @(@($Before) + @($After) | Where-Object { (Get-ChangeCategory $_.path) -eq 'CRASHPAD_METRICS' } | ForEach-Object path) |
        Where-Object { $_ } | Sort-Object -Unique)
}
function Start-LockWatcher([string]$Udf, $HostChild, [int[]]$Ids, [string]$Label, [switch]$Capture) {
    $log = Join-Path $run ($Label + '-watch.jsonl')
    $arguments = @('lock-watch', '--udf', $Udf, '--log', $log,
        '--parent-pid', [string]$HostChild.process.Id, '--pids', ($Ids -join ','), '--timeout-ms', [string](($ObserveSeconds + 30) * 1000))
    if ($Capture) {
        $prefix = Join-Path $run ($Label + '-capture'); $quiet = Join-Path $run ($Label + '-quiet.signal')
        $release = Join-Path $run ($Label + '-release.signal'); $abort = Join-Path $run ($Label + '-abort.signal'); $copy = New-Udf ($Label + '-reference-copy')
        $arguments += @('--capture-prefix', $prefix, '--quiet', $quiet, '--release', $release, '--abort', $abort, '--copy-to-udf', $copy)
    }
    $child = Start-ProbeChild $arguments $log
    if ($Capture) {
        $child | Add-Member -NotePropertyName capture_prefix -NotePropertyValue $prefix
        $child | Add-Member -NotePropertyName quiet -NotePropertyValue $quiet
        $child | Add-Member -NotePropertyName release -NotePropertyValue $release
        $child | Add-Member -NotePropertyName abort -NotePropertyValue $abort
        $child | Add-Member -NotePropertyName copy_udf -NotePropertyValue $copy
    }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt 5000) {
        if (@(Read-Events $log | Where-Object kind -eq 'watch_ready').Count) { return $child }
        if ($child.process.HasExited) { throw 'Native lock watcher exited before ready.' }
        Start-Sleep -Milliseconds 10
    }
    throw 'Native lock watcher did not become ready.'
}
function Invoke-Scenario([string]$Name, [scriptblock]$Body) {
    Write-Host "Running $Name..."
    $begin = $script:RunClock.ElapsedMilliseconds
    $firstCleanup = $script:CleanupActions.Count
    $row = [ordered]@{ name = $Name; status = 'INCONCLUSIVE'; started_ms = $begin; detail = $null }
    try { $detail = & $Body; $row.detail = $detail; $row.status = $detail.status }
    catch { $row.detail = [ordered]@{ error = $_.Exception.Message; stack = $_.ScriptStackTrace }; $row.status = 'ERROR' }
    finally {
        # Each scenario owns fresh UDFs. A failed scenario cannot leak a writer into the next.
        foreach ($child in $script:Children) {
            try { Stop-ProbeChild $child; Save-ChildDiagnostics $child }
            catch { $cleanupErrors.Add($_.Exception.Message); $row.status = 'ERROR' }
        }
        foreach ($udf in $script:Udfs) {
            try { Stop-UdfProcesses $udf }
            catch { $cleanupErrors.Add($_.Exception.Message); $row.status = 'ERROR' }
        }
        if ($script:CleanupActions.Count -gt $firstCleanup) { $row.status = 'ERROR'; $row.cleanup_failures = @($script:CleanupActions | Select-Object -Skip $firstCleanup) }
        $row.elapsed_ms = $script:RunClock.ElapsedMilliseconds - $begin
        $script:Scenarios.Add([pscustomobject]$row)
        Save-Json (Join-Path $run 'scenarios.partial.json') @($script:Scenarios)
    }
    Write-Host "$Name : $($row.status)"
}

function Register-MatchingProcesses($Rows, $Tracked, $Missing, $Clock) {
    foreach ($row in $Rows) {
        $key = [string]$row.pid + '/' + $row.creation_utc
        if ($Tracked.ContainsKey($key) -or $Missing.ContainsKey($key)) { continue }
        $process = $null
        try {
            $process = [Diagnostics.Process]::GetProcessById($row.pid); $null = $process.Handle
            $actualCreated = $process.StartTime.ToUniversalTime()
            $reportedCreated = [DateTime]::Parse($row.creation_utc).ToUniversalTime()
            # CIM carries microseconds; GetProcessTimes carries 100ns. Compare
            # the same microsecond instead of attributing a recycled PID's role.
            if ([Math]::Floor($actualCreated.Ticks / [decimal]10) -ne [Math]::Floor($reportedCreated.Ticks / [decimal]10)) {
                throw 'PID creation time mismatch between CIM row and retained OS handle'
            }
            $Tracked[$key] = [ordered]@{ row = $row; handle = $process; actual_created = $actualCreated;
                first_seen_ms = $Clock.ElapsedMilliseconds; first_seen_utc = [DateTime]::UtcNow.ToString('o') }
            $process = $null
        } catch { $Missing[$key] = [pscustomobject]@{ row = $row; error = $_.Exception.Message } }
        finally { if ($null -ne $process) { $process.Dispose() } }
    }
}
function Read-Capture([string]$Prefix, [string]$Phase) {
    $path = $Prefix + '-' + $Phase + '.json'
    Assert-ProbePath $path
    if ([IO.File]::Exists($path)) { return (Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json) }
    return $null
}
function Invoke-Lifetime([string]$ExitMode, [int]$Round = 1) {
    if ($ExitMode -ne 'graceful') { throw 'Only graceful shutdown is supported.' }
    $label = 'a-' + $ExitMode + '-r' + $Round
    $udf = New-Udf $label
    $hostChild = Start-Host $udf 'churn' ($label + '-host')
    $null = Require-Ready $hostChild
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $tracked = @{}; $missing = @{}
    $timeline = [Collections.Generic.List[object]]::new()
    $inventoryErrors = [Collections.Generic.List[string]]::new()
    $watcher = $null; $quietSince = $null; $quietSent = $null; $released = $false; $postCopyEmpty = 0
    $acquired = $null; $initialKeys = @(); $newAfterQuiet = [Collections.Generic.List[object]]::new()
    try {
        $initial = @(Get-UdfProcesses $udf)
        Register-MatchingProcesses $initial $tracked $missing $clock
        $initialKeys = @($tracked.Keys)
        $browserIds = @($initial | Where-Object is_browser | ForEach-Object { [int]$_.pid })
        if ($browserIds.Count -eq 0) { throw 'No exact-UDF browser before shutdown.' }
        $watcher = Start-LockWatcher $udf $hostChild $browserIds $label -Capture
        # A pulse newer than watcher readiness proves the page kept cycling until
        # the shutdown request. Never ask JS to flush/drain before Close.
        $hostEvents = @(Read-Events $hostChild.log)
        $after = if ($hostEvents.Count) { [long]$hostEvents[-1].elapsed_ms } else { -1 }
        $lastPulse = Require-Churn $hostChild $after
        $shutdown = $clock.ElapsedMilliseconds
        $shutdownUtc = [DateTime]::UtcNow.ToString('o')
        $shutdownQpc = [Diagnostics.Stopwatch]::GetTimestamp()
        # Native watcher starts before the fresh churn pulse. Its extra 30s
        # covers that <=5s handshake, host shutdown, then finite capture/copy.
        # Parent observation is anchored to the actual shutdown request, with
        # 15s reserved after the host's full natural-exit bound for quiet/copy.
        $observationDeadline = $shutdown + ($ObserveSeconds + 15) * 1000
        [IO.File]::WriteAllText($hostChild.stop, 'close')
        while ($clock.ElapsedMilliseconds -lt $observationDeadline) {
            $pollStarted = [DateTime]::UtcNow.ToString('o')
            try { $rows = @(Get-UdfProcesses $udf) }
            catch { $inventoryErrors.Add($_.Exception.Message); break }
            $keysBefore = @($tracked.Keys)
            Register-MatchingProcesses $rows $tracked $missing $clock
            $events = @(Read-Events $watcher.log)
            $acquireRows = @($events | Where-Object kind -eq 'lock_acquired')
            if ($acquireRows.Count) { $acquired = $acquireRows[0] }
            if ($null -ne $quietSent) {
                foreach ($key in @($tracked.Keys | Where-Object { $_ -notin $keysBefore })) { $newAfterQuiet.Add($tracked[$key].row) }
                # Even a row we could not retain invalidates a post-quiet claim.
                if ($rows.Count) { $newAfterQuiet.Add([pscustomobject]@{poll_rows=$rows;observed_ms=$clock.ElapsedMilliseconds}) }
            }
            $alive = @($tracked.Values | Where-Object { -not $_.handle.HasExited })
            $timeline.Add([pscustomobject]@{ ms=$clock.ElapsedMilliseconds; poll_started_utc=$pollStarted;
                poll_finished_utc=[DateTime]::UtcNow.ToString('o'); processes=$rows; tracked_alive=$alive.Count;
                lock_acquired_published=($null -ne $acquired); quiet_signal_sent=($null -ne $quietSent) })
            if ($null -ne $acquired -and $rows.Count -eq 0 -and $alive.Count -eq 0) {
                if ($null -eq $quietSince) { $quietSince = $clock.ElapsedMilliseconds }
                if ($null -eq $quietSent -and $clock.ElapsedMilliseconds - $quietSince -ge 500) {
                    $quietSent = $clock.ElapsedMilliseconds
                    [IO.File]::WriteAllText($watcher.quiet, 'observed exact-UDF quiet in consecutive polls')
                }
            } elseif ($null -eq $quietSent) { $quietSince = $null }
            $copyDone = @($events | Where-Object { $_.kind -in @('reference_copy_done','reference_copy_error') }).Count -gt 0
            if ($copyDone -and $null -ne $quietSent) {
                if ($rows.Count -eq 0 -and $alive.Count -eq 0) { $postCopyEmpty++ } else { $postCopyEmpty = 0 }
                if (-not $released -and $hostChild.process.HasExited -and ($postCopyEmpty -ge 2 -or $newAfterQuiet.Count -gt 0)) {
                    [IO.File]::WriteAllText($watcher.release, 'release after post-copy observations')
                    $released = $true
                }
            }
            if ($watcher.process.HasExited) { break }
            Start-Sleep -Milliseconds 50
        }
        if (-not $watcher.process.HasExited) {
            # Bounded end of observation; a release signal never claims quiet.
            [IO.File]::WriteAllText($watcher.release, 'deadline; observation incomplete unless normal protocol completed')
            if (-not $watcher.process.WaitForExit(3000)) { Stop-ProbeChild $watcher }
        }
        Wait-HostShutdown $hostChild
        Save-ChildDiagnostics $hostChild; Save-ChildDiagnostics $watcher
        $events = @(Read-Events $watcher.log)
        $fastSamples = @($events | Where-Object kind -eq 'watch_sample')
        $violations = @($fastSamples | Where-Object { $_.opened -and $_.alive_before -gt 0 -and $_.alive_after -gt 0 })
        $activeExclusions = @($fastSamples | Where-Object { -not $_.opened -and $_.win32_error -eq 32 -and $_.alive_before -gt 0 -and $_.alive_after -gt 0 })
        $orphans = @($fastSamples | Where-Object { -not $_.parent_alive_before -and -not $_.parent_alive_after -and
            $_.alive_after -gt 0 -and -not $_.opened -and $_.win32_error -eq 32 })
        $acquireRows = @($events | Where-Object kind -eq 'lock_acquired')
        if ($acquireRows.Count) { $acquired = $acquireRows[0] }
        $fastComplete = @($events | Where-Object { $_.kind -eq 'watch_end' -and $_.complete }).Count -gt 0
        $observedComplete = @($events | Where-Object { $_.kind -eq 'watch_end' -and $_.observation_complete }).Count -gt 0
        $referenceObservedComplete = @($events | Where-Object { $_.kind -eq 'watch_end' -and $_.reference_observation_complete }).Count -gt 0
        $protocolComplete = @($events | Where-Object { $_.kind -eq 'watch_end' -and $_.protocol_complete }).Count -gt 0
        $acquisitionComplete = @($events | Where-Object { $_.kind -eq 'watch_end' -and $_.acquisition_capture_complete }).Count -gt 0
        $referenceCapturesComplete = @($events | Where-Object { $_.kind -eq 'watch_end' -and $_.reference_captures_complete }).Count -gt 0
        $releaseEvents = @($events | Where-Object kind -eq 'held_lock_released')
        $hostEvents = @(Read-Events $hostChild.log)
        $churnFailures = @($hostEvents | Where-Object { $_.kind -eq 'page_message' -and $_.data.kind -eq 'storage_result' -and
            $_.data.mode -eq 'churn' -and -not $_.data.pass -and $_.message_received.qpc -and [long]$_.message_received.qpc -lt $shutdownQpc })
        $normalExit = $hostChild.process.HasExited -and $hostChild.process.ExitCode -eq 0 -and
            @($hostEvents | Where-Object kind -eq 'close_requested').Count -gt 0 -and @($hostEvents | Where-Object kind -eq 'host_exit').Count -gt 0
        $expectedExit = $normalExit
        $life = @(foreach ($key in $tracked.Keys) {
            $entry = $tracked[$key]; $process = $entry.handle
            $exit = if ($process.HasExited) { $process.ExitTime.ToUniversalTime() } else { $null }
            $atAcquire = $null; $delta = $null
            if ($null -ne $acquired) {
                $time = [DateTime]::FromFileTimeUtc([long]$acquired.filetime)
                $atAcquire = $entry.actual_created -le $time -and ($null -eq $exit -or $exit -gt $time)
                if ($null -ne $exit) { $delta = ($exit - $time).TotalMilliseconds }
            }
            [pscustomobject]@{ pid=$entry.row.pid; parent_pid=$entry.row.parent_pid; role=$entry.row.role; process_name=$entry.row.process_name;
                process_type=$entry.row.process_type; utility_subtype=$entry.row.utility_subtype; command_line=$entry.row.command_line;
                creation_utc=$entry.actual_created.ToString('o'); cim_creation_utc=$entry.row.creation_utc;
                exit_utc=$(if($null -ne $exit){$exit.ToString('o')}else{$null}); still_alive=($null -eq $exit);
                alive_at_acquisition=$atAcquire; exit_minus_acquisition_ms=$delta;
                lifetime_ms=$(if($null -ne $exit){($exit-$entry.actual_created).TotalMilliseconds}else{$null});
                after_host_exit_ms=$(if($null -ne $exit -and $hostChild.process.HasExited){($exit-$hostChild.process.ExitTime.ToUniversalTime()).TotalMilliseconds}else{$null});
                first_seen_ms=$entry.first_seen_ms; first_seen_utc=$entry.first_seen_utc; discovered_after_initial=($key -notin $initialKeys);
                created_after_acquisition=$(if($null -ne $acquired){$entry.actual_created.ToFileTimeUtc() -gt [long]$acquired.filetime}else{$null});
                first_observed_after_acquisition=$(if($null -ne $acquired){[DateTime]::Parse($entry.first_seen_utc).ToUniversalTime().ToFileTimeUtc() -gt [long]$acquired.filetime}else{$null}) }
        })
        $acquireCapture = Read-Capture $watcher.capture_prefix 'acquired'
        $quietCapture = Read-Capture $watcher.capture_prefix 'quiet'
        $afterCopyCapture = Read-Capture $watcher.capture_prefix 'after-copy'
        $copyCapture = Read-Capture $watcher.capture_prefix 'copy'
        $acquireToQuiet = Compare-Snapshots $acquireCapture $quietCapture
        $quietToAfterCopy = Compare-Snapshots $quietCapture $afterCopyCapture
        $copyEqual = Compare-Snapshots $quietCapture $copyCapture
        $reference = if ($newAfterQuiet.Count) { 'INVALIDATED_NEW_MATCHING_PROCESS_AFTER_QUIET' }
            elseif (-not $referenceObservedComplete -or $null -eq $quietSent -or -not $released -or $missing.Count -or $inventoryErrors.Count) { 'INCOMPLETE' }
            elseif ($quietToAfterCopy.equal -and $copyEqual.equal) { 'UNCHANGED_DURING_QUIET_COPY' } else { 'CHANGED_OR_UNREADABLE' }
        # Slow handle-user diagnostics come after the timed process/capture loop.
        $logUsers = Add-OwnerMetadata (Invoke-FileProbe $exe $hostChild.log)
        if ($logUsers.cleanup_error) { $cleanupErrors.Add("Diagnostic PID $($logUsers.process_pid): $($logUsers.cleanup_error)") }
        Save-Json (Join-Path $run ($label + '-log-users.json')) ([ordered]@{phase='after capture/process observation';observation=$logUsers})
        $measurementComplete = $fastComplete -and $observedComplete -and $expectedExit -and $missing.Count -eq 0 -and
            $inventoryErrors.Count -eq 0 -and $churnFailures.Count -eq 0 -and $reference -eq 'UNCHANGED_DURING_QUIET_COPY'
        $status = Get-LifetimeStatus $violations.Count $activeExclusions.Count $measurementComplete
        $result = [ordered]@{ status=$status; udf=$udf; shutdown_requested_ms=$shutdown; shutdown_requested_utc=$shutdownUtc;
            observation_deadline_ms=$observationDeadline; post_shutdown_capture_budget_ms=15000;
            last_churn_progress=$lastPulse; shutdown_request_qpc=[string]$shutdownQpc; churn_failures_received_before_shutdown=$churnFailures; expected_host_exit=$expectedExit;
            host_exit_utc=$(if($hostChild.process.HasExited){$hostChild.process.ExitTime.ToUniversalTime().ToString('o')}else{$null});
            host_exit_code=$(if($hostChild.process.HasExited){$hostChild.process.ExitCode}else{$null});
            lock_acquisition=$acquired; acquisition_host_state=$(if($null -eq $acquired){'NOT_ACQUIRED'}elseif($acquired.parent_alive_before -or $acquired.parent_alive_after){'HOST_STILL_ALIVE'}else{'HOST_EXITED'});
            lockfile_exists_after_watcher_release=$(if($releaseEvents.Count){$releaseEvents[-1].lockfile_exists_after_watcher_release}else{$null});
            fast_violations=$violations.Count; violations=$violations.Count;
            active_exclusion_samples=$activeExclusions.Count;
            exclusion_after_browser_exit_samples=@($fastSamples | Where-Object { $_.win32_error -eq 32 -and $_.alive_after -eq 0 }).Count;
            orphan_coverage=$(if($orphans.Count -and $fastComplete){'OBSERVED'}else{'NOT_OBSERVED'});
            fast_orphan_coverage=$(if($orphans.Count -and $fastComplete){'OBSERVED'}else{'NOT_OBSERVED'});
            fast_orphan_exclusion_samples=$orphans.Count; orphan_exclusion_samples=$orphans.Count; fast_completed=$fastComplete;
            capture_protocol_completed=$protocolComplete; acquisition_capture_completed=$acquisitionComplete;
            capture_observation_completed=$observedComplete; all_matching_process_lifetimes=$life;
            browser_lifetimes=@($life | Where-Object role -eq 'browser'); alive_at_acquisition=@($life | Where-Object alive_at_acquisition);
            tracking_missing=@($missing.Values); inventory_errors=@($inventoryErrors); process_timeline=@($timeline); fast_samples=$fastSamples;
            log_file_users=$logUsers;
            acquired_to_observed_quiet=$acquireToQuiet; acquisition_change_classification=(Get-ChangeSummary $acquireToQuiet);
            reference_quiet_copy=[ordered]@{ status=$reference; quiet_signal_ms=$quietSent; copy_udf=$watcher.copy_udf;
                capture_protocol_complete=$protocolComplete; captures_complete=$referenceCapturesComplete; observation_complete=$referenceObservedComplete;
                source_comparison=$quietToAfterCopy; source_classification=(Get-ChangeSummary $quietToAfterCopy);
                copy_comparison=$copyEqual; new_after_quiet=@($newAfterQuiet) };
            arrivals_after_acquisition=@($life | Where-Object created_after_acquisition);
            newly_observed_after_acquisition=@($life | Where-Object first_observed_after_acquisition);
            acquisition_to_capture_start_ms=$(if($null -ne $acquired){([decimal]$acquired.snapshot_started.qpc-[decimal]$acquired.qpc)*1000/[decimal]$acquired.qpc_frequency}else{$null});
            observation_limits='Hashes are read intervals, not atomic snapshots. Unchanged metadata cannot rule out mapped writes. CIM polling can miss short-lived processes between polls; no arrivals observed is not proof no process could start.' }
        Save-Json (Join-Path $run ($label + '.json')) $result
        return $result
    } finally {
        if (-not $hostChild.process.HasExited) { [IO.File]::WriteAllText($hostChild.stop, 'cooperative close before observation cleanup') }
        if ($null -ne $watcher -and -not $watcher.process.HasExited) { Stop-ProbeChild $watcher }
        foreach ($entry in $tracked.Values) { $entry.handle.Dispose() }
    }
}

function Invoke-Blocked([bool]$MissingLock, [int]$Round = 1) {
    $label = $(if ($MissingLock) { 'b-create-new' } else { 'b-existing' }) + '-r' + $Round
    $udf = New-Udf $label
    $seed = Start-Host $udf 'write' ($label + '-seed')
    $null = Require-Ready $seed; Close-Host $seed; Wait-Quiet $udf
    $lock = Join-Path $udf 'EBWebView\lockfile'
    $normalLeftLock = [IO.File]::Exists($lock)
    if ($MissingLock) {
        # The only deletion is this known disposable fixture's lockfile after all writers exit.
        Assert-ProbePath $lock
        if ($normalLeftLock) { [IO.File]::Delete($lock) }
    } elseif (-not $normalLeftLock) {
        # Cover OPEN_EXISTING even on runtimes which remove the file at normal exit.
        [IO.File]::WriteAllBytes($lock, [byte[]]@())
    }
    $before = Get-Manifest $udf
    Save-Json (Join-Path $run ($label + '-before.json')) $before
    $holder = Start-Holder $udf $MissingLock ($label + '-holder')
    try {
        $contention = Try-Lock $udf
        if ($contention.opened -or $contention.win32_error -ne 32) { throw 'Holder did not establish exclusive access.' }
        $attempt = Start-Host $udf 'read' ($label + '-attempt')
        $outcome = Wait-Host $attempt
        Save-Json (Join-Path $run ($label + '-attempt.json')) $outcome
        if (-not $attempt.process.HasExited) { Close-Host $attempt }
        Assert-HostShutdown $attempt
        if ($holder.process.HasExited) { throw 'Holder exited before snapshot comparison.' }
        $after = Get-Manifest $udf
        Save-Json (Join-Path $run ($label + '-after.json')) $after
        $comparison = Compare-Manifests $before $after
        $classificationHeld = Get-ChangeSummary $comparison
        $paths = Get-DiagnosticPaths $before $after $comparison
        $heldDiagnostics = Get-FileDiagnostics $udf $paths 'while-held' $label
        $observation = Get-BlockedObservation $outcome
    } finally { Release-Holder $holder }
    $lockAfter = [IO.File]::Exists($lock)
    $postRelease = Get-Manifest $udf
    Save-Json (Join-Path $run ($label + '-released.json')) $postRelease
    $releasedComparison = Compare-Manifests $before $postRelease
    $classificationReleased = Get-ChangeSummary $releasedComparison
    $releasedPaths = @($paths + (Get-DiagnosticPaths $before $postRelease $releasedComparison) | Sort-Object -Unique)
    $releasedDiagnostics = Get-FileDiagnostics $udf $releasedPaths 'after-release' $label
    Wait-Quiet $udf
    Start-Sleep -Milliseconds 500
    $settled = Get-Manifest $udf
    Save-Json (Join-Path $run ($label + '-settled.json')) $settled
    $settledComparison = Compare-Manifests $before $settled
    $settledPaths = @($releasedPaths + (Get-DiagnosticPaths $before $settled $settledComparison) | Sort-Object -Unique)
    $settledDiagnostics = Get-FileDiagnostics $udf $settledPaths 'after-release-settled' $label
    $noTrace = -not $MissingLock -or -not $lockAfter
    # Positive control on this very UDF proves the denial was not a broken fixture.
    $reopen = Start-Host $udf 'read' ($label + '-reopen')
    $null = Require-Ready $reopen; Close-Host $reopen; Wait-Quiet $udf
    return [ordered]@{ status = (Get-StrictBlockedStatus $observation @($comparison, $releasedComparison, $settledComparison) $noTrace);
        udf = $udf; normal_exit_left_lockfile = $normalLeftLock; create_new_delete_on_close = $MissingLock;
        startup = $outcome; blocked = $observation.blocked; responsive = $observation.responsive;
        startup_terminal_elapsed_ms = $observation.startup_terminal_elapsed_ms;
        heartbeat_max_gap_ms = $observation.heartbeat_max_gap_ms; denial_kind = $observation.denial_kind;
        hash_comparison_while_held = $comparison; hash_comparison_after_release = $releasedComparison;
        classification_while_held = $classificationHeld; classification_after_release = $classificationReleased;
        hash_comparison_after_settled = $settledComparison; classification_after_settled = (Get-ChangeSummary $settledComparison);
        file_diagnostics = @($heldDiagnostics, $releasedDiagnostics, $settledDiagnostics);
        lockfile_exists_after_release = $lockAfter; no_trace = $noTrace; positive_control_after_release = $true }
}
function Invoke-ProbeAttempt([string]$Name, [int]$Round, [scriptblock]$Body) {
    $firstChild = $script:Children.Count; $firstUdf = $script:Udfs.Count
    $firstCleanup = $script:CleanupActions.Count
    $start = $script:RunClock.ElapsedMilliseconds
    $row = [ordered]@{ attempt = $Round; status = 'ERROR'; detail = $null }
    try { $row.detail = & $Body; $row.status = $row.detail.status }
    catch { $row.detail = [ordered]@{ error = $_.Exception.Message; stack = $_.ScriptStackTrace } }
    finally {
        try {
            foreach ($child in @($script:Children | Select-Object -Skip $firstChild)) { Stop-ProbeChild $child; Save-ChildDiagnostics $child }
            foreach ($udf in @($script:Udfs | Select-Object -Skip $firstUdf)) { Stop-UdfProcesses $udf }
        } catch {
            $row.status = 'ERROR'
            $row.cleanup_error = $_.Exception.Message
        }
        if ($script:CleanupActions.Count -gt $firstCleanup) { $row.status = 'ERROR'; $row.cleanup_failures = @($script:CleanupActions | Select-Object -Skip $firstCleanup) }
        $row.elapsed_ms = $script:RunClock.ElapsedMilliseconds - $start
        Save-Json (Join-Path $run ($Name + '-attempt-' + $Round + '.json')) $row
    }
    return [pscustomobject]$row
}
function Invoke-LifetimeRepeats([string]$ExitMode, [int]$Count) {
    $attempts = [Collections.Generic.List[object]]::new()
    for ($round = 1; $round -le $Count; ++$round) {
        $row = Invoke-ProbeAttempt ('a-' + $ExitMode) $round { Invoke-Lifetime $ExitMode $round }
        $attempts.Add($row)
    }
    $status = Get-AttemptStatus @($attempts)
    $observed = @($attempts | Where-Object { $_.detail.orphan_coverage -eq 'OBSERVED' }).Count -gt 0
    return [ordered]@{ status = $status; requested_attempts = $Count; attempts_run = $attempts.Count;
        orphan_coverage = $(if ($observed) { 'OBSERVED' } else { 'NOT_OBSERVED' }); attempts = @($attempts) }
}
function Invoke-BlockedRepeats([bool]$MissingLock) {
    $name = if ($MissingLock) { 'b-create-new' } else { 'b-existing' }
    $attempts = [Collections.Generic.List[object]]::new()
    for ($round = 1; $round -le $BlockedAttempts; ++$round) {
        $attempts.Add((Invoke-ProbeAttempt $name $round { Invoke-Blocked $MissingLock $round }))
    }
    return [ordered]@{ status = (Get-AttemptStatus @($attempts)); attempts_run = $attempts.Count; attempts = @($attempts);
        change_sets_while_held = (Get-ChangeSetComparison @($attempts));
        change_sets_after_release = (Get-ChangeSetComparison @($attempts) 'hash_comparison_after_release');
        change_sets_after_settled = (Get-ChangeSetComparison @($attempts) 'hash_comparison_after_settled') }
}

function Invoke-Copy {
    $x = New-Udf 'c-source-X'; $y = New-Udf 'c-copy-Y'
    $seed = Start-Host $x 'write' 'c-seed-X'
    $written = Require-Ready $seed; Close-Host $seed; Wait-Quiet $x
    $lock = Join-Path $x 'EBWebView\lockfile'
    $missing = -not [IO.File]::Exists($lock)
    $before = Get-Manifest $x; Save-Json (Join-Path $run 'c-X-before.json') $before
    $holder = Start-Holder $x $missing 'c-holder-X'
    try {
        Copy-Udf $x $y
        $copiedLock = [IO.File]::Exists((Join-Path $y 'EBWebView\lockfile'))
        $after = Get-Manifest $x; $copy = Get-Manifest $y
        Save-Json (Join-Path $run 'c-X-after.json') $after; Save-Json (Join-Path $run 'c-Y-copy.json') $copy
        $sourceComparison = Compare-Manifests $before $after; $copyComparison = Compare-Manifests $before $copy
        if ($holder.process.HasExited) { throw 'Copy lock holder unexpectedly exited.' }
    } finally { Release-Holder $holder }
    $readY = Start-Host $y 'read' 'c-read-Y'
    $yResult = Require-Ready $readY; Close-Host $readY; Wait-Quiet $y
    $readX = Start-Host $x 'read' 'c-read-X'
    $xResult = Require-Ready $readX; Close-Host $readX; Wait-Quiet $x
    return [ordered]@{ status = $(if ($sourceComparison.equal -and $copyComparison.equal -and -not $copiedLock) { 'PASS' } else { 'FAIL' });
        source = $x; destination = $y; copied_lockfile = $copiedLock; source_unchanged = $sourceComparison;
        copy_equal = $copyComparison; written = $written.storage; read_Y = $yResult.storage; read_X = $xResult.storage }
}

$cleanupErrors = [Collections.Generic.List[string]]::new()
$crashStart = [DateTime]::UtcNow
$crashBefore = Get-CrashObservation $crashStart.AddHours(-24)
Save-Json (Join-Path $run 'crash-before.json') $crashBefore
try {
    Invoke-Scenario 'a-graceful' { Invoke-LifetimeRepeats 'graceful' $GracefulAttempts }
    Invoke-Scenario 'b-existing' { Invoke-BlockedRepeats $false }
    Invoke-Scenario 'b-create-new-delete-on-close' { Invoke-BlockedRepeats $true }
    Invoke-Scenario 'c-copy-and-reopen' { Invoke-Copy }
} finally {
    foreach ($child in $script:Children) {
        try {
            Stop-ProbeChild $child
            Save-ChildDiagnostics $child
        } catch { $cleanupErrors.Add($_.Exception.Message) }
        $child.process.Dispose()
    }
    foreach ($udf in $script:Udfs) {
        try { Stop-UdfProcesses $udf } catch { $cleanupErrors.Add($_.Exception.Message) }
    }
    # A short bounded observation tail catches delayed WER startup; never close
    # or kill WerFault, and never suppress error dialogs via process error mode.
    Start-Sleep -Milliseconds 1000
    $crashAfter = Get-CrashObservation $crashStart
    Save-Json (Join-Path $run 'crash-after.json') $crashAfter
    $crashCheck = [ordered]@{ before=$crashBefore;after=$crashAfter;
        status=$(if(-not $crashBefore.complete -or -not $crashAfter.complete){'UNKNOWN'}
            elseif($crashBefore.werfault.Count -or $crashAfter.werfault.Count -or $crashAfter.application_events.Count){'OBSERVED'}else{'NONE_OBSERVED'});
        note='Finite before/after sampling cannot prove that no transient or delayed dialog occurred; baseline events are historical, not attributed to this run.' }
    if ($script:CleanupActions.Count) { $cleanupErrors.Add('Shutdown timeout or last-resort cleanup occurred; see cleanup_actions.') }
    if ($crashCheck.status -ne 'NONE_OBSERVED') { $cleanupErrors.Add('WER/error-dialog observation is ' + $crashCheck.status) }
    $gate = Get-MigrationPrerequisite @($script:Scenarios) @($cleanupErrors)
    $eventLogs = @(foreach ($file in Get-ChildItem -LiteralPath $run -Filter '*.jsonl') {
        [ordered]@{ file = $file.Name; events = @(Read-Events $file.FullName) }
    })
    $scriptHashes = @(foreach ($file in Get-ChildItem -LiteralPath $PSScriptRoot -File) {
        [ordered]@{ file = $file.Name; sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash }
    })
    $result = [ordered]@{ schema_version = 4; run_directory = $run; utc = [DateTime]::UtcNow.ToString('o');
        token = $token; origin = 'https://effetune-mixwright.localhost/'; build = $build;
        startup_timeout_seconds = $StartupSeconds; observation_timeout_seconds = $ObserveSeconds;
        graceful_attempts = $GracefulAttempts; blocked_attempts_per_mode = $BlockedAttempts;
        historical_forced_evidence = 'run-20261024-140853-682-71368: one forced attempt; live browser with obtainable lock => FAIL. Not rerun.';
        crash_observation=$crashCheck;cleanup_actions=@($script:CleanupActions);
        observation_note = 'Finite samples cannot prove all Chromium versions or unsampled interleavings. Forced termination is removed; the historical literal-invariant failure remains STOP.';
        scope = 'DISPOSABLE WEBVIEW2 EXPERIMENT'; script_sha256 = $scriptHashes; event_logs = $eventLogs;
        migration_prerequisite = $gate; scenarios = @($script:Scenarios); cleanup_errors = @($cleanupErrors) }
    Write-ProbeReport $result
    Write-Host "Results: $run\results.md"
    Write-Host "Migration prerequisite: $gate"
}
if ($gate -eq 'SATISFIED_ON_THIS_RUNTIME') { exit 0 }
exit 2
