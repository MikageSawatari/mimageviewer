# ASCII only. Production migration is executed by the Rust helper, never here.
. (Join-Path $PSScriptRoot 'common.ps1')
# Keep the existing probe binaries and historical runs in their original root.
# Shared path guards now apply only to this migration suite's shorter workspace.
$script:MigrationBinaryRoot = $script:ProbeRoot
$script:ProbeRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\target\wm'))

function Get-MigrationRunLayout {
    param([ValidatePattern('^[a-f0-9]{8}$')][string]$RunId = [Guid]::NewGuid().ToString('N').Substring(0, 8))
    $root = Join-Path $script:ProbeRoot $RunId
    Assert-ProbePath $root
    $fake = Join-Path $root 'a'
    $data = Join-Path $root 'd'
    return [pscustomobject]@{ root = $root; fake_appdata = $fake; data_dir = $data;
        source = (Join-Path $fake 'mimageviewer-vst3-host.exe');
        destination = (Join-Path $data 'effetune\webview'); checkpoints = (Join-Path $root 'c');
        original_data_dir = (Join-Path $root 'o'); original_checkpoints = (Join-Path $root 'p');
        original_destination = (Join-Path $root 'o\effetune\webview') }
}
function Assert-MigrationPathBudget([string]$Source, [string]$Destination) {
    # The fixture's fixed origin uses this LevelDB directory. Reserve a full
    # uint64 file number, rather than just the observed six-digit MANIFEST name.
    # Another 32 UTF-16 units remain available below the 259-unit Win32 limit.
    # This is a conservative fixture preflight, not a WebView2-wide path promise.
    $tail = 'EBWebView\Default\IndexedDB\https_effetune-mixwright.localhost_0.indexeddb.leveldb\MANIFEST-18446744073709551615'
    $maxFileUnits = 259
    $reserve = 32
    $checks = @(foreach ($pair in @(@('source', $Source), @('destination', $Destination))) {
        $udf = [IO.Path]::GetFullPath($pair[1])
        $file = Join-Path $udf $tail
        $units = $file.Length # .NET strings count UTF-16 code units, including surrogate pairs.
        if ($units -gt ($maxFileUnits - $reserve)) {
            throw ("WebView2 migration probe path budget exceeded before startup: {0} = {1} UTF-16 units, maximum {2} (259 minus {3} reserved). Use a shorter checkout path. File: {4}" -f
                $pair[0], $units, ($maxFileUnits - $reserve), $reserve, $file)
        }
        [pscustomobject]@{ role = $pair[0]; udf = $udf; indexeddb_file = $file;
            utf16_units = $units; headroom_utf16_units = ($maxFileUnits - $units) }
    })
    return [pscustomobject]@{ max_path_utf16_units = 260; maximum_file_utf16_units = $maxFileUnits;
        reserved_utf16_units = $reserve; allowed_file_utf16_units = ($maxFileUnits - $reserve); checks = $checks }
}

function Test-MigrationExcluded([string]$RelativePath) {
    $p = $RelativePath.Replace('\', '/').TrimEnd('/')
    if ($p -ieq 'EBWebView/lockfile' -or $p -ieq 'EBWebView/CrashpadMetrics.pma' -or
        $p -ieq 'EBWebView/CrashpadMetrics-active.pma') { return $true }
    foreach ($directory in @('Crashpad', 'BrowserMetrics', 'DeferredBrowserMetrics', 'EBWebViewMetrics')) {
        $root = 'EBWebView/' + $directory
        if ($p -ieq $root -or $p.StartsWith($root + '/', [StringComparison]::OrdinalIgnoreCase)) { return $true }
    }
    return $false
}
function Get-MigrationManifest([string]$Udf) {
    # Preserve raw rows, including read errors, for audit. Only the exact accepted
    # exclusions are removed when constructing the acceptance comparison.
    $raw = Get-Manifest $Udf
    return [pscustomobject]@{ raw = $raw; included = @($raw | Where-Object { -not (Test-MigrationExcluded $_.path) });
        excluded = @($raw | Where-Object { Test-MigrationExcluded $_.path }) }
}
function Assert-MigrationExclusions([string]$Udf) {
    Assert-ProbePath $Udf
    foreach ($p in @('EBWebView\lockfile', 'EBWebView\Crashpad', 'EBWebView\CrashpadMetrics.pma',
        'EBWebView\CrashpadMetrics-active.pma', 'EBWebView\BrowserMetrics',
        'EBWebView\DeferredBrowserMetrics', 'EBWebView\EBWebViewMetrics')) {
        if (Test-Path -LiteralPath (Join-Path $Udf $p)) { throw "Excluded destination path exists before reopening: $p" }
    }
}
function Join-MigrationArguments([string[]]$Values) {
    return (@(foreach ($value in $Values) {
        if ($value.Contains('"') -or $value.EndsWith('\') -or $value -match '[\r\n]') { throw 'Unsupported native argument.' }
        '"' + $value + '"'
    }) -join ' ')
}
function Start-MigrationChild([string]$Executable, [string[]]$Arguments, [string]$Label,
    [string]$StopPath = '', [string]$Udf = '', [switch]$Helper) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable; $start.Arguments = Join-MigrationArguments $Arguments
    $start.UseShellExecute = $false; $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
    $start.EnvironmentVariables['MSBUILDDISABLENODEREUSE'] = '1'
    $process = [Diagnostics.Process]::new(); $process.StartInfo = $start
    if (-not $process.Start()) { throw "Cannot start child: $Label" }
    $child = [pscustomobject]@{ process = $process; label = $Label; stop = $StopPath; udf = $Udf;
        helper = [bool]$Helper; log = (Join-Path $script:MigrationRun ($Label + '.jsonl'));
        stdout_lines = [Collections.Generic.List[string]]::new();
        stdout_pending = $process.StandardOutput.ReadLineAsync(); stdout_eof = $false;
        stderr = $process.StandardError.ReadToEndAsync(); started_utc = [DateTime]::UtcNow.ToString('o') }
    $script:MigrationChildren.Add($child)
    return $child
}
function Read-MigrationStdout($Child) {
    # Never synchronously await EOF or an inherited pipe writer.
    while (-not $Child.stdout_eof -and $Child.stdout_pending.IsCompleted) {
        if ($Child.stdout_pending.IsFaulted -or $Child.stdout_pending.IsCanceled) { throw "stdout failed: $($Child.label)" }
        $line = $Child.stdout_pending.Result
        if ($null -eq $line) { $Child.stdout_eof = $true; break }
        $Child.stdout_lines.Add($line)
        [IO.File]::AppendAllText(($Child.log + '.stdout.txt'), $line + "`n", [Text.UTF8Encoding]::new($false))
        $Child.stdout_pending = $Child.process.StandardOutput.ReadLineAsync()
    }
    if ($Child.helper) {
        foreach ($line in $Child.stdout_lines) {
            if ($line.Trim()) { $line | ConvertFrom-Json -ErrorAction Stop }
        }
    }
}
function Save-MigrationChild($Child) {
    $null = @(Read-MigrationStdout $Child)
    Save-Json ($Child.log + '.process.json') ([ordered]@{ pid = $Child.process.Id; label = $Child.label;
        started_utc = $Child.started_utc; observed_utc = [DateTime]::UtcNow.ToString('o');
        exited = $Child.process.HasExited; exit_code = $(if ($Child.process.HasExited) { $Child.process.ExitCode } else { $null });
        stdout_eof = $Child.stdout_eof; stderr_status = [string]$Child.stderr.Status;
        stderr = $(if ([string]$Child.stderr.Status -eq 'RanToCompletion') { $Child.stderr.Result } else { $null });
        forced_termination = $false })
}
function Wait-MigrationExit($Child, [int]$Milliseconds = 35000) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while (-not $Child.process.HasExited -and $clock.ElapsedMilliseconds -lt $Milliseconds) {
        $null = @(Read-MigrationStdout $Child)
        Start-Sleep -Milliseconds 20
    }
    Save-MigrationChild $Child
    if (-not $Child.process.HasExited) { throw "Natural-exit deadline exceeded; process left alive, never terminated: $($Child.process.Id)" }
}
function Start-MigrationHost([string]$Udf, [string]$Mode, [string]$Label) {
    Assert-ProbePath $Udf
    $stop = Join-Path $script:MigrationRun ($Label + '.stop')
    $log = Join-Path $script:MigrationRun ($Label + '.jsonl')
    return Start-MigrationChild $script:MigrationProbe @('host', '--udf', $Udf, '--mode', $Mode,
        '--token', $script:MigrationToken, '--log', $log, '--stop', $stop,
        '--timeout-ms', '20000', '--shutdown-timeout-ms', '30000') $Label $stop $Udf
}
function Request-MigrationClose($Child) {
    if ($Child.stop) { Assert-ProbePath $Child.stop; [IO.File]::WriteAllText($Child.stop, 'close') }
}
function Assert-MigrationHostExit($Child) {
    $events = @(Read-Events $Child.log)
    $end = @($events | Where-Object kind -eq 'shutdown_complete')
    $errors = @($events | Where-Object { $_.kind -eq 'host_error' -or $_.kind -eq 'shutdown_inventory_error' -or
        ($_.kind -eq 'hresult' -and $_.stage -like '*controller_close' -and $_.failed) })
    if ($end.Count -ne 1 -or -not $end[0].complete -or $errors.Count -or $Child.process.ExitCode -notin @(0, 2, 3)) {
        throw "Host did not prove Close and natural-exit completion: $($Child.label)"
    }
}
function Close-MigrationHost($Child) {
    Request-MigrationClose $Child
    Wait-MigrationExit $Child
    Assert-MigrationHostExit $Child
}
function Wait-MigrationStorage($Child) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    do {
        $rows = @(Read-Events $Child.log | Where-Object { $_.kind -eq 'page_message' -and $_.data.kind -eq 'storage_result' })
        if ($rows.Count) {
            $data = $rows[-1].data
            Save-Json ($Child.log + '.storage.json') $data
            if (-not $data.pass -or -not $data.checks.localStorage -or -not $data.checks.indexedDB -or -not $data.checks.opfs) {
                throw "Storage verification failed: $($Child.label)"
            }
            return $data
        }
        if ($Child.process.HasExited) { throw "Host exited before storage result: $($Child.label)" }
        Start-Sleep -Milliseconds 20
    } while ($clock.ElapsedMilliseconds -lt 25000)
    throw "Storage result deadline: $($Child.label)"
}
function Wait-MigrationPhase($Child, [string]$Phase, [int]$Milliseconds = 8000) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    do {
        $events = @(Read-MigrationStdout $Child)
        $found = @($events | Where-Object phase -eq $Phase)
        if ($found.Count) { return $found[-1] }
        if ($Child.process.HasExited -and $Child.stdout_eof) { Save-MigrationChild $Child; throw "Helper exited before $Phase; inspect stdout and result." }
        Start-Sleep -Milliseconds 5
    } while ($clock.ElapsedMilliseconds -lt $Milliseconds)
    throw "Helper phase deadline: $Phase"
}
function Assert-MigrationHelperEvents([object[]]$Events) {
    $required = @('lock_acquired', 'process_wait_begin', 'process_wait_end', 'copy_begin', 'copy_end', 'result')
    $observed = @($Events | Where-Object { $_.phase -in $required })
    if (($observed.phase -join ',') -cne ($required -join ',')) { throw 'Incomplete, duplicate, or out-of-order production migration phases.' }
    $previous = -1
    foreach ($event in $observed) {
        if ($null -eq $event.elapsed_ms -or [long]$event.elapsed_ms -lt $previous) { throw 'Migration event timing is missing or out of order.' }
        $previous = [long]$event.elapsed_ms
    }
    if ($observed[-1].passed -ne $true) { throw 'Production helper did not report a successful result.' }
}
function Get-MigrationOriginalWaitObservation([object[]]$Events, [uint32]$OriginalPid) {
    # Native handle observations are made before JSON output or checkpoint
    # pauses. PowerShell/CIM sampling after lock_acquired cannot prove 8ms overlap.
    $phases = @('lock_acquired', 'process_wait_begin', ('process_wait_pid_begin:' + $OriginalPid),
        ('process_wait_pid_end:' + $OriginalPid), 'process_wait_end', 'copy_begin')
    $rows = @($Events | Where-Object { $_.phase -in $phases })
    $ordered = ($rows.phase -join ',') -ceq ($phases -join ',')
    $identity = $rows.Count -eq $phases.Count -and
        @($rows | Where-Object { $_.original_pid -ne $OriginalPid }).Count -eq 0
    $timed = $true; $previous = -1
    foreach ($row in $rows) {
        if ($null -eq $row.elapsed_ms -or [long]$row.elapsed_ms -lt $previous) { $timed = $false; break }
        $previous = [long]$row.elapsed_ms
    }
    $alive = $ordered -and $identity -and $timed -and $rows[0].original_alive -eq $true -and
        $rows[1].original_alive -eq $true -and
        $rows[2].original_alive -eq $true -and $rows[3].original_alive -eq $false -and
        $rows[4].original_alive -eq $false -and $rows[5].original_alive -eq $false
    return [pscustomobject]@{ original_pid = $OriginalPid; observed = [bool]$alive;
        native_events = $rows; ordered_wait_evidence = [bool]$ordered;
        note = 'Requires the same original browser alive at actual lock acquisition and its production PID wait begin, then exited before wait end and copy. Missing evidence is INCONCLUSIVE.' }
}
function Get-MigrationAcceptanceStatus($OriginalWait, $WaitCompetitor, $CopyCompetitor) {
    if ($OriginalWait.observed -eq $true -and $WaitCompetitor.exact_process_overlap_observed -eq $true -and
        $CopyCompetitor.exact_process_overlap_observed -eq $true) { return 'PASS' }
    return 'INCONCLUSIVE'
}
function Resume-MigrationPhase([string]$Phase) {
    $path = Join-Path $script:MigrationCheckpoints ($Phase + '.continue')
    Assert-ProbePath $path
    [IO.File]::WriteAllText($path, 'continue')
}
function Wait-MigrationLockAvailable([string]$Udf) {
    $path = Join-Path $Udf 'EBWebView\lockfile'; Assert-ProbePath $path
    $clock = [Diagnostics.Stopwatch]::StartNew()
    do {
        if (-not [IO.File]::Exists($path)) { return }
        try {
            $file = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
            $file.Dispose(); return
        } catch [IO.IOException] { }
        Start-Sleep -Milliseconds 2
    } while ($clock.ElapsedMilliseconds -lt 5000)
    throw 'Source did not naturally release the singleton lock; migration was not started.'
}
function Assert-MigrationHeldLock([string]$Udf) {
    $path = Join-Path $Udf 'EBWebView\lockfile'; Assert-ProbePath $path
    try {
        $file = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
    } catch [IO.IOException] {
        $win32 = $_.Exception.HResult -band 65535
        if ($win32 -in @(32, 33)) { return [pscustomobject]@{ held = $true; win32_error = $win32; observed_utc = [DateTime]::UtcNow.ToString('o') } }
        throw
    }
    $file.Dispose()
    throw 'Migration checkpoint did not retain an exclusive source lock.'
}
function Wait-MigrationCompetitor($Child, [string]$Udf) {
    # Poll while the helper is at its bounded checkpoint. Resume only after a
    # real matching Runtime process was observed, or report missing overlap.
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $observed = @(); $events = @()
    do {
        $events = @(Read-Events $Child.log)
        $observed = @(Get-UdfProcesses $Udf)
        if ($observed.Count) { break }
        if ($Child.process.HasExited -or @($events | Where-Object kind -eq 'host_closing').Count) { break }
        Start-Sleep -Milliseconds 10
    } while ($clock.ElapsedMilliseconds -lt 3500)
    return [pscustomobject]@{ observed_utc = [DateTime]::UtcNow.ToString('o'); matching_processes = $observed;
        startup_requested = @($events | Where-Object { $_.kind -eq 'stage_begin' -and $_.stage -eq 'create_environment' }).Count -gt 0;
        exact_process_overlap_observed = $observed.Count -gt 0 }
}
function Get-MigrationBlockedResult($Child) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    do {
        $events = @(Read-Events $Child.log)
        if ($Child.process.HasExited -or @($events | Where-Object kind -eq 'host_closing').Count) { break }
        if (@($events | Where-Object kind -eq 'page_message').Count) { break }
        Start-Sleep -Milliseconds 20
    } while ($clock.ElapsedMilliseconds -lt 25000)
    Request-MigrationClose $Child
    Wait-MigrationExit $Child
    Assert-MigrationHostExit $Child
    $outcome = [pscustomobject]@{ events = @(Read-Events $Child.log); outcome = 'exited' }
    $result = Get-BlockedObservation $outcome
    Save-Json ($Child.log + '.blocked.json') $result
    if (-not $result.blocked -or -not $result.responsive) { throw "Competing startup was not responsively refused: $($Child.label)" }
    return $result
}
function Complete-MigrationCleanup {
    $failures = [Collections.Generic.List[object]]::new()
    # Release helper checkpoints first; all waits remain bounded even on error.
    foreach ($phase in @('lock_acquired', 'process_wait_end', 'copy_begin', 'copy_end')) { Resume-MigrationPhase $phase }
    foreach ($child in $script:MigrationChildren) { if (-not $child.helper) { Request-MigrationClose $child } }
    foreach ($child in $script:MigrationChildren) {
        try {
            Wait-MigrationExit $child
            if (-not $child.helper) { Assert-MigrationHostExit $child }
        } catch { $failures.Add([pscustomobject]@{ pid = $child.process.Id; label = $child.label; reason = $_.Exception.Message }) }
    }
    foreach ($udf in $script:MigrationUdfs) {
        try {
            $remaining = @(Get-UdfProcesses $udf -IncludeRelated)
            if ($remaining.Count) { $failures.Add([pscustomobject]@{ udf = $udf; remaining = $remaining; reason = 'Matching Runtime processes remain; no termination performed.' }) }
        } catch { $failures.Add([pscustomobject]@{ udf = $udf; reason = $_.Exception.Message }) }
    }
    return ,@($failures)
}
