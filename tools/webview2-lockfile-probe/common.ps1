# ASCII only. Shared by the runner, offline tests and build script.
$script:ProbeRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\target\webview2-lockfile-probe'))
function Assert-ProbePath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if (-not $full.StartsWith($script:ProbeRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Outside disposable probe root: $full"
    }
    $cursor = $full
    while ($cursor) {
        if ([IO.File]::Exists($cursor) -or [IO.Directory]::Exists($cursor)) {
            if (([IO.File]::GetAttributes($cursor) -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Reparse point forbidden: $cursor"
            }
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}
function Save-Json([string]$Path, $Value) {
    Assert-ProbePath $Path
    [IO.File]::WriteAllText($Path, (ConvertTo-Json -InputObject $Value -Depth 40), [Text.UTF8Encoding]::new($false))
}
function Read-Events([string]$Path) {
    Assert-ProbePath $Path
    if (-not [IO.File]::Exists($Path)) { return }
    $stream = [IO.FileStream]::new($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read,
        ([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    $reader = [IO.StreamReader]::new($stream, [Text.Encoding]::UTF8, $true)
    try { $text = $reader.ReadToEnd() } finally { $reader.Dispose() }
    # Only newline-terminated records are complete. A partial inner object may
    # itself end in '}', so that character cannot identify a finished append.
    $lastNewline = $text.LastIndexOf("`n")
    if ($lastNewline -lt 0) { return }
    foreach ($line in $text.Substring(0, $lastNewline).Split("`n")) {
        if ($line.Trim()) {
            $line | ConvertFrom-Json -ErrorAction Stop
        }
    }
}
function Get-Manifest([string]$Udf) {
    Assert-ProbePath $Udf
    if (-not [IO.Directory]::Exists($Udf)) { throw "Missing UDF: $Udf" }
    $base = [IO.Path]::GetFullPath($Udf).TrimEnd('\') + '\'
    $stack = [Collections.Generic.Stack[string]]::new()
    $stack.Push($Udf)
    $rows = [Collections.Generic.List[object]]::new()
    while ($stack.Count) {
        foreach ($entry in [IO.Directory]::EnumerateFileSystemEntries($stack.Pop())) {
            Assert-ProbePath $entry
            $attributes = [IO.File]::GetAttributes($entry)
            if (($attributes -band [IO.FileAttributes]::Directory) -ne 0) { $stack.Push($entry); continue }
            $relative = $entry.Substring($base.Length).Replace('\', '/')
            if ($relative.Equals('EBWebView/lockfile', [StringComparison]::OrdinalIgnoreCase)) { continue }
            $file = [IO.FileInfo]::new($entry)
            $hash = $null; $readError = $null; $size = $null
            try { $size = $file.Length; $hash = (Get-FileHash -LiteralPath $entry -Algorithm SHA256 -ErrorAction Stop).Hash }
            catch { $readError = $_.Exception.Message }
            $rows.Add([pscustomobject][ordered]@{ path = $relative; size = $size;
                sha256 = $hash; read_error = $readError })
        }
    }
    return ,@($rows | Sort-Object -Property path)
}
function Compare-Manifests($Before, $After) {
    $a = @{}; $b = @{}
    foreach ($row in $Before) { $a[$row.path] = $row }
    foreach ($row in $After) { $b[$row.path] = $row }
    $differences = @(
        foreach ($key in @(@($a.Keys) + @($b.Keys) | Sort-Object -Unique)) {
            if (($a.ContainsKey($key) -and $a[$key].read_error) -or ($b.ContainsKey($key) -and $b[$key].read_error)) {
                [pscustomobject]@{ path = $key; change = 'read_error'; before_error = $a[$key].read_error; after_error = $b[$key].read_error }
            }
            elseif (-not $a.ContainsKey($key)) { [pscustomobject]@{ path = $key; change = 'added' } }
            elseif (-not $b.ContainsKey($key)) { [pscustomobject]@{ path = $key; change = 'removed' } }
            elseif ($a[$key].size -ne $b[$key].size -or $a[$key].sha256 -cne $b[$key].sha256) {
                [pscustomobject]@{ path = $key; change = 'content_or_size' }
            }
        }
    )
    return [pscustomobject]@{ equal = ($differences.Count -eq 0); before_files = @($Before).Count;
        after_files = @($After).Count; differences = $differences }
}
function Copy-Udf([string]$Source, [string]$Destination) {
    Assert-ProbePath $Source; Assert-ProbePath $Destination
    if ([IO.Directory]::Exists($Destination)) { throw 'Copy destination must not exist.' }
    [IO.Directory]::CreateDirectory($Destination) | Out-Null
    $base = [IO.Path]::GetFullPath($Source).TrimEnd('\') + '\'
    $stack = [Collections.Generic.Stack[string]]::new(); $stack.Push($Source)
    while ($stack.Count) {
        foreach ($entry in [IO.Directory]::EnumerateFileSystemEntries($stack.Pop())) {
            Assert-ProbePath $entry
            $relative = $entry.Substring($base.Length)
            if ($relative.Equals('EBWebView\lockfile', [StringComparison]::OrdinalIgnoreCase)) { continue }
            $dest = Join-Path $Destination $relative
            Assert-ProbePath $dest
            if (([IO.File]::GetAttributes($entry) -band [IO.FileAttributes]::Directory) -ne 0) {
                [IO.Directory]::CreateDirectory($dest) | Out-Null; $stack.Push($entry)
            } else { [IO.File]::Copy($entry, $dest, $false) }
        }
    }
}
if (-not ('ProbeCommandLine' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ProbeCommandLine {
    [DllImport("shell32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CommandLineToArgvW(string line, out int count);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
    public static string[] Split(string line) {
        int count;
        var p = CommandLineToArgvW(line, out count);
        if (p == IntPtr.Zero) throw new System.ComponentModel.Win32Exception();
        try {
            var result = new string[count];
            for (int i=0; i<count; ++i) result[i] = Marshal.PtrToStringUni(Marshal.ReadIntPtr(p, i * IntPtr.Size));
            return result;
        } finally { LocalFree(p); }
    }
}
'@
}
function Test-PathSwitch([string]$CommandLine, [string]$Switch, [string]$ExpectedDirectory) {
    if (-not $CommandLine) { return $false }
    $arguments = [ProbeCommandLine]::Split($CommandLine)
    $values = @()
    for ($i = 1; $i -lt $arguments.Count; ++$i) {
        if ($arguments[$i].StartsWith($Switch + '=', [StringComparison]::OrdinalIgnoreCase)) {
            $values += $arguments[$i].Substring($Switch.Length + 1)
        } elseif ($arguments[$i] -ieq $Switch -and $i + 1 -lt $arguments.Count) {
            $values += $arguments[++$i]
        }
    }
    if ($values.Count -ne 1) { return $false }
    try {
        return [IO.Path]::GetFullPath($values[0]).TrimEnd('\', '/').Equals(
            [IO.Path]::GetFullPath($ExpectedDirectory).TrimEnd('\', '/'), [StringComparison]::OrdinalIgnoreCase)
    } catch { return $false }
}
function Test-UdfCommandLine([string]$CommandLine, [string]$ExpectedChromiumDirectory) {
    Test-PathSwitch $CommandLine '--user-data-dir' $ExpectedChromiumDirectory
}
function Get-UdfProcesses([string]$Udf, [switch]$IncludeRelated) {
    Assert-ProbePath $Udf
    # Read only process metadata; never inspect any other profile's files.
    $filter = "CommandLine LIKE '%--user-data-dir%'"
    if ($IncludeRelated) { $filter += " OR CommandLine LIKE '%--database%'" }
    foreach ($process in @(Get-CimInstance -ClassName Win32_Process -Filter $filter -OperationTimeoutSec 3 -ErrorAction Stop)) {
        # WebView2 appends EBWebView to the API's userDataFolder before launching
        # Chromium. Compare the entire resulting path, never a subtree/prefix.
        $exact = Test-UdfCommandLine $process.CommandLine (Join-Path $Udf 'EBWebView')
        $crashpad = $IncludeRelated -and (Test-PathSwitch $process.CommandLine '--database' (Join-Path $Udf 'EBWebView\Crashpad'))
        if ($exact -or $crashpad) {
            $arguments = [ProbeCommandLine]::Split($process.CommandLine)
            $type = @($arguments | Where-Object { $_ -like '--type=*' } | ForEach-Object { $_.Substring(7) }) -join ','
            $subtype = @($arguments | Where-Object { $_ -like '--utility-sub-type=*' } | ForEach-Object { $_.Substring(19) }) -join ','
            $browser = $process.Name -ieq 'msedgewebview2.exe' -and -not $type
            $role = if ($browser) { 'browser' } elseif (-not $type) { 'other-executable:' + $process.Name } elseif ($type -eq 'utility' -and $subtype -like '*NetworkService*') { 'network-service' }
                elseif ($type -eq 'utility') { 'utility:' + $subtype } else { $type }
            [pscustomobject]@{ pid = [int]$process.ProcessId; parent_pid = [int]$process.ParentProcessId;
                creation_utc = $process.CreationDate.ToUniversalTime().ToString('o');
                is_browser = $browser; process_name = $process.Name;
                match_basis = $(if ($exact) { 'user-data-dir' } else { 'crashpad-database' }); cleanup_eligible = $exact;
                process_type = $type; utility_subtype = $subtype; role = $role;
                command_line = $process.CommandLine }
        }
    }
}
function Compare-Snapshots($Before, $After) {
    $comparison = Compare-Manifests $Before.files $After.files
    $complete = $null -ne $Before -and $null -ne $After -and $Before.complete -and $After.complete -and
        -not $Before.enumeration_error -and -not $After.enumeration_error
    return [pscustomobject]@{ equal = ($complete -and $comparison.equal); captures_complete = $complete;
        enumeration_complete = ($null -ne $Before -and $null -ne $After -and -not $Before.enumeration_error -and -not $After.enumeration_error);
        before_files = $comparison.before_files; after_files = $comparison.after_files; differences = $comparison.differences;
        before_error = $Before.enumeration_error; after_error = $After.enumeration_error }
}
function Get-ChangeCategory([string]$Path) {
    $name = $Path.Replace('\', '/')
    if ($name -match '^EBWebView/(Crashpad/|CrashpadMetrics(-active)?\.pma$|DeferredBrowserMetrics/|EBWebViewMetrics/|BrowserMetrics/)') {
        return 'CRASHPAD_METRICS'
    }
    if ($name -match '^EBWebView/Default/' -or $name -match '^EBWebView/(Local State|Preferences|Secure Preferences|First Run|Last Version|Last Browser|Variations)$') { return 'PROFILE' }
    return 'OTHER'
}
function Get-ChangeSummary($Comparison) {
    $enumerationComplete = $null -eq $Comparison.enumeration_complete -or $Comparison.enumeration_complete
    $classified = @(foreach ($difference in $Comparison.differences) {
        $category = Get-ChangeCategory $difference.path
        $normalized = $difference.path
        if ($category -eq 'CRASHPAD_METRICS') {
            $normalized = $normalized -replace '(?i)/(BrowserMetrics|EBWebViewMetrics)-[^/]+\.pma$', '/$1-*.pma'
        }
        $confirmed = $difference.change -ne 'read_error' -and ($enumerationComplete -or $difference.change -notin @('added','removed'))
        [pscustomobject]@{ path = $difference.path; change = $difference.change; category = $category;
            confirmed_difference = $confirmed; diagnostic_normalized_path = $normalized }
    })
    $states = @{}
    foreach ($category in @('PROFILE','CRASHPAD_METRICS','OTHER')) {
        $rows = @($classified | Where-Object category -eq $category)
        $states[$category] = if (@($rows | Where-Object confirmed_difference).Count) { $true }
            elseif (-not $enumerationComplete -or @($rows | Where-Object { -not $_.confirmed_difference }).Count) { $null } else { $false }
    }
    return [pscustomobject]@{ profile_changed=$states.PROFILE; crashpad_metrics_changed=$states.CRASHPAD_METRICS;
        other_changed=$states.OTHER; enumeration_complete=$enumerationComplete; files=$classified }
}
function Format-ChangeState($Value) { if ($null -eq $Value) { return 'UNKNOWN' }; return [string]$Value }
function Get-AttemptStatus($Attempts) {
    # Every attempt counts; a later observation never erases an earlier failure.
    foreach ($status in @('ERROR', 'FAIL', 'INCONCLUSIVE')) {
        if (@($Attempts | Where-Object status -eq $status).Count) { return $status }
    }
    if (@($Attempts).Count -eq 0) { return 'INCONCLUSIVE' }
    return 'PASS'
}
function Get-LifetimeStatus([int]$Violations, [int]$ActiveExclusions, [bool]$Complete) {
    if ($Violations -gt 0) { return 'FAIL' }
    if ($ActiveExclusions -gt 0 -and $Complete) { return 'PASS' }
    return 'INCONCLUSIVE'
}
function Get-MigrationPrerequisite($Scenarios, $CleanupErrors) {
    # Historical forced-exit evidence is retained, not rerun or promoted to PASS.
    # Its observed literal invariant failure remains a migration STOP.
    return 'STOP'
}
function Add-CleanupFailure([string]$Kind, [int]$ProcessId, [string]$Reason, [string]$Udf = '') {
    if ($null -eq $script:CleanupActions) { $script:CleanupActions = [Collections.Generic.List[object]]::new() }
    $script:CleanupActions.Add([pscustomobject]@{ utc=[DateTime]::UtcNow.ToString('o');kind=$Kind;pid=$ProcessId;reason=$Reason;udf=$Udf })
    if ($run) { Save-Json (Join-Path $run 'cleanup-actions.partial.json') @($script:CleanupActions) }
}
function Get-CrashObservation([DateTime]$Since) {
    $errors = [Collections.Generic.List[string]]::new()
    $wer = @(); $events = @()
    try { $wer = @(Get-CimInstance Win32_Process -Filter "Name='WerFault.exe'" -ErrorAction Stop | ForEach-Object {
        [pscustomobject]@{pid=[int]$_.ProcessId;creation_utc=$_.CreationDate.ToUniversalTime().ToString('o');command_line=$_.CommandLine} }) }
    catch { $errors.Add('WerFault inventory: ' + $_.Exception.Message) }
    try {
        $events = @(Get-WinEvent -FilterHashtable @{LogName='Application';Id=@(1000,1001);StartTime=$Since} -ErrorAction Stop |
            Where-Object { $_.ToXml() -match '(?i)(msedgewebview2|probe)\.exe' } | ForEach-Object {
                [pscustomobject]@{record_id=$_.RecordId;event_id=$_.Id;provider=$_.ProviderName;
                    utc=$_.TimeCreated.ToUniversalTime().ToString('o');message=$_.Message;xml=$_.ToXml()} })
    } catch {
        if ($_.FullyQualifiedErrorId -notlike 'NoMatchingEventsFound*') { $errors.Add('Application log: ' + $_.Exception.Message) }
    }
    return [pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');since_utc=$Since.ToUniversalTime().ToString('o');
        werfault=$wer;application_events=$events;errors=@($errors);complete=($errors.Count -eq 0)}
}
function Get-StrictBlockedStatus($Observation, [object[]]$Comparisons, [bool]$NoTrace) {
    if ($Observation.blocked -and $Observation.responsive -and $NoTrace -and $Comparisons.Count -eq 3 -and
        @($Comparisons | Where-Object { -not $_.equal }).Count -eq 0) { return 'PASS' }
    return 'FAIL'
}
function Get-ChangeSetComparison($Attempts, [string]$ComparisonProperty = 'hash_comparison_while_held') {
    $sets = @(foreach ($attempt in $Attempts) {
        if ($null -ne $attempt.detail.$ComparisonProperty) {
            $summary = Get-ChangeSummary $attempt.detail.$ComparisonProperty
            [pscustomobject]@{ attempt = $attempt.attempt;
                raw_paths = @($summary.files.path | Sort-Object -Unique);
                diagnostic_normalized_paths = @($summary.files.diagnostic_normalized_path | Sort-Object -Unique) }
        }
    })
    $rawSame = $true; $normalizedSame = $true
    foreach ($set in @($sets | Select-Object -Skip 1)) {
        if ((ConvertTo-Json -InputObject $set.raw_paths -Compress) -cne (ConvertTo-Json -InputObject $sets[0].raw_paths -Compress)) { $rawSame = $false }
        if ((ConvertTo-Json -InputObject $set.diagnostic_normalized_paths -Compress) -cne (ConvertTo-Json -InputObject $sets[0].diagnostic_normalized_paths -Compress)) { $normalizedSame = $false }
    }
    return [pscustomobject]@{ all_rounds_comparable = ($sets.Count -eq @($Attempts).Count -and $sets.Count -gt 0);
        raw_sets_equal = $(if ($sets.Count -gt 0) { $rawSame } else { $null });
        diagnostic_normalized_sets_equal = $(if ($sets.Count -gt 0) { $normalizedSame } else { $null }); attempts = $sets }
}
function Invoke-FileProbe([string]$Exe, [string]$Path, [string]$CopyTo = '') {
    Assert-ProbePath $Path
    $arguments = @('file-probe', '--path', $Path)
    if ($CopyTo) { Assert-ProbePath $CopyTo; $arguments += @('--copy-to', $CopyTo) }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Exe; $start.UseShellExecute = $false; $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
    $start.Arguments = (@($arguments | ForEach-Object { if ($_.Contains('"')) { throw 'Invalid native path.' }; '"' + $_ + '"' }) -join ' ')
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync(); $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(10000)) {
            $cleanupError = $null
            Add-CleanupFailure 'helper_wait_timeout' $process.Id 'file-probe exceeded 10000 ms; last-resort Kill follows.'
            try {
                Add-CleanupFailure 'last_resort_kill' $process.Id 'file-probe deadline exceeded.'
                $process.Kill()
                if (-not $process.WaitForExit(5000)) { $cleanupError = 'Timed-out file-probe did not exit after Kill.' }
            } catch { $cleanupError = $_.Exception.Message }
            return [pscustomobject]@{ path = $Path; diagnostic_error = 'file-probe timeout'; process_pid = $process.Id; cleanup_error = $cleanupError }
        }
        try { $null = $stdout.Wait(1000) } catch { }
        try { $null = $stderr.Wait(1000) } catch { }
        if ($stdout.Status -ne 'RanToCompletion' -or $stderr.Status -ne 'RanToCompletion') {
            return [pscustomobject]@{ path = $Path; diagnostic_error = 'file-probe streams incomplete'; stdout_status = [string]$stdout.Status; stderr_status = [string]$stderr.Status }
        }
        if ($process.ExitCode -ne 0) { return [pscustomobject]@{ path = $Path; diagnostic_error = $stderr.Result; exit_code = $process.ExitCode } }
        return ($stdout.Result | ConvertFrom-Json)
    } finally { $process.Dispose() }
}
function Stop-UdfProcesses([string]$Udf) {
    # Normal cleanup never kills. First allow bounded natural shutdown after all
    # host stop/release signals; failure and any later Kill are durable evidence.
    $watch = [Diagnostics.Stopwatch]::StartNew()
    do {
        $remaining = @(Get-UdfProcesses $Udf)
        if ($remaining.Count -eq 0) { return }
        Start-Sleep -Milliseconds 100
    } while ($watch.ElapsedMilliseconds -lt 30000)
    Add-CleanupFailure 'udf_wait_timeout' 0 'Exact-UDF processes remained after 30000 ms natural-exit wait.' $Udf
    foreach ($row in @(Get-UdfProcesses $Udf)) {
        # Retain an OS handle before re-querying to prevent PID reuse at termination.
        $process = $null
        try {
            $process = [Diagnostics.Process]::GetProcessById($row.pid)
            $null = $process.Handle
            $current = @(Get-UdfProcesses $Udf | Where-Object { $_.pid -eq $row.pid -and $_.creation_utc -eq $row.creation_utc })
            if ($current.Count -eq 1 -and -not $process.HasExited) {
                Add-CleanupFailure 'last_resort_kill' $row.pid 'Exact-UDF natural-exit deadline exceeded.' $Udf
                $process.Kill()
                if (-not $process.WaitForExit(5000)) { throw "Matching test process failed to exit: $($row.pid)" }
            }
        } catch [ArgumentException] { } finally { if ($null -ne $process) { $process.Dispose() } }
    }
}
function Write-ProbeReport($Result) {
    $directory = $Result.run_directory
    Assert-ProbePath $directory
    Save-Json (Join-Path $directory 'results.json') $Result
    $md = [Collections.Generic.List[string]]::new()
    $md.Add('# WebView2 lockfile probe'); $md.Add('')
    $md.Add("Migration prerequisite: **$($Result.migration_prerequisite)**"); $md.Add('')
    if ($Result.scope) { $md.Add($Result.scope); $md.Add('') }
    $md.Add('No migration/product implementation is included. Review results before proceeding.'); $md.Add('')
    if ($Result.historical_forced_evidence) { $md.Add('Historical evidence: ' + $Result.historical_forced_evidence); $md.Add('') }
    if ($Result.crash_observation) {
        $crash = $Result.crash_observation
        $md.Add('## WER and Application errors'); $md.Add('')
        $md.Add("Observation: $($crash.status). $($crash.note)")
        foreach ($phase in @('before','after')) {
            $entry = $crash.$phase
            $md.Add("${phase}: WerFault=$(@($entry.werfault).Count), matching id 1000/1001=$(@($entry.application_events).Count), complete=$($entry.complete).")
            foreach ($process in $entry.werfault) { $md.Add("WerFault PID $($process.pid), created $($process.creation_utc): $($process.command_line)") }
            foreach ($event in $entry.application_events) { $md.Add("Application event $($event.event_id), record $($event.record_id), UTC $($event.utc): $($event.message)") }
            foreach ($failure in $entry.errors) { $md.Add("Observation unavailable: $failure") }
        }
        $md.Add('')
    }
    $md.Add('## Host shutdown'); $md.Add('')
    foreach ($entry in $Result.event_logs) {
        $ends = @($entry.events | Where-Object kind -eq 'shutdown_complete')
        foreach ($end in $ends) { $md.Add("$($entry.file): complete=$($end.complete), controller Close succeeded=$($end.controller_close_succeeded), processes exited=$($end.processes_observed_exited), timed_out=$($end.timed_out), shutdown wait=$($end.wait_elapsed_ms) ms, bound=$($end.timeout_ms) ms.") }
        foreach ($failure in @($entry.events | Where-Object { $_.kind -eq 'shutdown_inventory_error' -or ($_.kind -eq 'hresult' -and $_.stage -like '*controller_close' -and $_.failed) })) {
            $md.Add("$($entry.file): shutdown diagnostic: $($failure | ConvertTo-Json -Compress)")
        }
    }
    foreach ($action in $Result.cleanup_actions) { $md.Add("Cleanup FAILURE: $($action.kind), PID=$($action.pid), UTC=$($action.utc): $($action.reason)") }
    $md.Add('')
    $md.Add('| Scenario | Result | Time (ms) |'); $md.Add('|---|---|---:|')
    foreach ($row in $Result.scenarios) { $md.Add("| $($row.name) | $($row.status) | $($row.elapsed_ms) |") }
    $reportRows = @(foreach ($row in $Result.scenarios) {
        if ($null -ne $row.detail.attempts) {
            $md.Add(''); $md.Add("## $($row.name) repetitions"); $md.Add('')
            $md.Add("Attempts: $($row.detail.attempts_run). Aggregate: $($row.status). Earlier FAIL/ERROR/INCONCLUSIVE outcomes are retained.")
            if ($row.name -like 'b-*') {
                foreach ($key in @('change_sets_while_held', 'change_sets_after_release', 'change_sets_after_settled')) {
                    $sets = $row.detail.$key
                    $md.Add("${key}: comparable=$($sets.all_rounds_comparable), raw path sets equal=$($sets.raw_sets_equal), diagnostic normalized sets equal=$($sets.diagnostic_normalized_sets_equal).")
                }
            }
            foreach ($attempt in $row.detail.attempts) {
                [pscustomobject]@{ name = ($row.name + ' / attempt ' + $attempt.attempt); status = $attempt.status; detail = $attempt.detail; elapsed_ms = $attempt.elapsed_ms }
            }
        } else { $row }
    })
    foreach ($row in $reportRows) {
        $d = $row.detail
        $md.Add(''); $md.Add("## $($row.name)"); $md.Add('')
        if ($d.error) { $md.Add("Error: $($d.error)"); $md.Add(''); continue }
        if ($row.name -like 'a-*') {
            $md.Add("Sharing violations while browser alive: $($d.active_exclusion_samples); violations: $($d.violations).")
            $md.Add("Orphan coverage: $($d.orphan_coverage) ($($d.orphan_exclusion_samples) exclusion samples).")
            $md.Add("Native 5ms watcher: orphan samples=$($d.fast_orphan_exclusion_samples), violations=$($d.fast_violations), natural completion=$($d.fast_completed).")
            $md.Add("Capture protocol complete=$($d.capture_protocol_completed); acquisition capture complete=$($d.acquisition_capture_completed); overall capture observation complete=$($d.capture_observation_completed).")
            $md.Add("Shutdown request UTC: $($d.shutdown_requested_utc); last completed write cycle: $($d.last_churn_progress.data.completed_sequence).")
            $md.Add("Lock acquisition FILETIME: $($d.lock_acquisition.filetime); host state: $($d.acquisition_host_state); acquisition-to-capture-start: $($d.acquisition_to_capture_start_ms) ms."); $md.Add('')
            $md.Add("Watcher created lockfile=$($d.lock_acquisition.create_new); lockfile exists after watcher release=$($d.lockfile_exists_after_watcher_release). This includes observer intervention, not Runtime-only shutdown.")
            $md.Add('| PID | Role | Created (UTC) | Exited (UTC) | Exit minus acquisition (ms) | Alive at acquisition |')
            $md.Add('|---:|---|---|---|---:|---|')
            foreach ($life in $d.all_matching_process_lifetimes) {
                $md.Add("| $($life.pid) | $($life.role) | $($life.creation_utc) | $($life.exit_utc) | $($life.exit_minus_acquisition_ms) | $($life.alive_at_acquisition) |")
            }
            $classification = $d.acquisition_change_classification
            $md.Add(''); $md.Add("Acquisition capture -> observed process quiet: equal=$($d.acquired_to_observed_quiet.equal), captures complete=$($d.acquired_to_observed_quiet.captures_complete).")
            $md.Add("Profile differences=$(Format-ChangeState $classification.profile_changed); Crashpad/metrics differences=$(Format-ChangeState $classification.crashpad_metrics_changed); other differences=$(Format-ChangeState $classification.other_changed).")
            foreach ($diff in $d.acquired_to_observed_quiet.differences) { $md.Add("Comparison entry: $($diff.path) ($($diff.change), $(Get-ChangeCategory $diff.path))") }
            $reference = $d.reference_quiet_copy
            $md.Add(''); $md.Add("Reference quiet-then-copy: $($reference.status). Source before/after copy equal=$($reference.source_comparison.equal); copy equal=$($reference.copy_comparison.equal).")
            $md.Add("Reference protocol complete=$($reference.capture_protocol_complete); captures complete=$($reference.captures_complete); combined observation complete=$($reference.observation_complete).")
            $md.Add("New matching process observations after quiet: $(@($reference.new_after_quiet).Count); process tracking gaps: $(@($d.tracking_missing).Count).")
            $md.Add("Processes created after acquisition: $(@($d.arrivals_after_acquisition).Count); first observed after acquisition: $(@($d.newly_observed_after_acquisition).Count). Full role/PID records are in JSON.")
            $md.Add('Reference results never replace the original strict lockfile verdict. Waiting does not erase acquisition-to-quiet differences.')
            $md.Add('Snapshots are read intervals. Empty consecutive CIM polls can miss short-lived processes between polls; no metadata change is not proof of no mapped writes.')
        } elseif ($row.name -like 'b-*') {
            $md.Add("Blocked: $($d.blocked); observation: $($d.denial_kind); responsive: $($d.responsive); maximum heartbeat gap: $($d.heartbeat_max_gap_ms) ms.")
            $md.Add("Startup outcome: $($d.startup.outcome); observed startup duration: $($d.startup.elapsed_ms) ms.")
            $md.Add("Startup terminal event: $($d.startup_terminal_elapsed_ms) ms from host start; runner duration above also includes natural shutdown wait.")
            $md.Add("Normal exit left lockfile: $($d.normal_exit_left_lockfile); CREATE_NEW/DELETE_ON_CLOSE: $($d.create_new_delete_on_close); lockfile after release: $($d.lockfile_exists_after_release).")
            $md.Add("Files/size/SHA256 unchanged while held: $($d.hash_comparison_while_held.equal) ($($d.hash_comparison_while_held.before_files) files); after release: $($d.hash_comparison_after_release.equal).")
            $md.Add("Files/size/SHA256 unchanged after natural quiet + 500ms: $($d.hash_comparison_after_settled.equal). This comparison is also strict.")
            $md.Add("Same-UDF storage positive control after release: $($d.positive_control_after_release)."); $md.Add('')
            foreach ($key in @('classification_while_held', 'classification_after_release', 'classification_after_settled')) {
                $classification = $d.$key
                $md.Add("${key}: profile changed=$(Format-ChangeState $classification.profile_changed); Crashpad/metrics changed=$(Format-ChangeState $classification.crashpad_metrics_changed); other changed=$(Format-ChangeState $classification.other_changed).")
            }
            $md.Add('Classification and normalized paths are diagnostic only; every file difference still fails the original strict comparison.'); $md.Add('')
            $md.Add('| Stage | HRESULT | Elapsed (ms) |'); $md.Add('|---|---|---:|')
            foreach ($event in @($d.startup.events | Where-Object kind -eq 'hresult')) {
                $md.Add("| $($event.stage) | $($event.hresult) | $($event.elapsed_ms) |")
            }
            foreach ($diff in @($d.hash_comparison_while_held.differences + $d.hash_comparison_after_release.differences + $d.hash_comparison_after_settled.differences)) {
                $md.Add("Comparison entry: $($diff.path) ($($diff.change), $(Get-ChangeCategory $diff.path))")
            }
            foreach ($stage in $d.file_diagnostics) {
                $md.Add(''); $md.Add("File/process observations: $($stage.stage), $($stage.started_ms)..$($stage.finished_ms) ms.")
                foreach ($process in $stage.processes) { $md.Add("Process PID $($process.pid): $($process.match_basis), cleanup eligible=$($process.cleanup_eligible).") }
                if ($stage.inventory_error) { $md.Add("Process inventory unavailable: $($stage.inventory_error)") }
                if ($stage.skipped_paths) { $md.Add("Diagnostic budget: skipped $(@($stage.skipped_paths).Count) paths; full list remains in JSON. Full hash comparisons are not capped.") }
                $md.Add(''); $md.Add('| File | Read error | Copy error | Restart Manager PIDs | RM error |')
                $md.Add('|---|---:|---:|---|---:|')
                foreach ($file in $stage.files) {
                    $pids = @($file.observation.rm_users | ForEach-Object pid) -join ', '
                    $md.Add("| $($file.relative_path) | $($file.observation.read_error) | $($file.observation.copy_error) | $pids | $($file.observation.rm_error) |")
                    if ($file.observation.diagnostic_error) { $md.Add("Diagnostic unavailable: $($file.observation.diagnostic_error)") }
                }
            }
        } elseif ($row.name -like 'c-*') {
            $md.Add("Source unchanged during copy: $($d.source_unchanged.equal); copied files/size/SHA256 equal: $($d.copy_equal.equal); copied lockfile: $($d.copied_lockfile)."); $md.Add('')
            $md.Add('| Operation | localStorage | IndexedDB | OPFS |'); $md.Add('|---|---|---|---|')
            foreach ($key in @('written', 'read_Y', 'read_X')) {
                $checks = $d.$key.checks
                $md.Add("| $key | $($checks.localStorage) | $($checks.indexedDB) | $($checks.opfs) |")
            }
        }
    }
    $md.Add(''); $md.Add('## Evidence'); $md.Add('')
    $md.Add('Full observations, including Win32 error codes, per-sample process command lines and hash differences: [results.json](results.json).')
    $md.Add('Per-host HRESULT/page/heartbeat events: *.jsonl. Complete pre/post SHA256 manifests: *-before.json, *-after.json, *-released.json and c-Y-copy.json.')
    $md.Add('Process exit codes/stdout/stderr: *.process.json. Startup results, including failed positive controls: *.startup.json.')
    $md.Add('Log inheritance flags: log_handle_* on each native log record; standard_handle records preserve the unmodified inherited standard-handle flags. Pipe EOF/pending/fault/cancel states are in *.process.json.')
    $md.Add('Restart Manager file users and process metadata: *-log-users.json, *-file-diagnostics.json. RM results are clues, not an exhaustive inventory; zero users does not prove no mapped/inherited owner.')
    $md.Add('No hook observes the private bInheritHandles value used inside WebView2. Inheritable standard-handle flags and pending pipe EOF are clues, not proof of inheritance by a particular PID.')
    $md.Add(''); $md.Add('## Limits'); $md.Add(''); $md.Add($Result.observation_note)
    $md.Add('Browser matching compares the parsed --user-data-dir to <API UDF>/EBWebView exactly. Only matching test processes are cleaned up.')
    $md.Add('The page acknowledgement follows two animation frames; physical screen pixels are not independently verified.')
    $md.Add('A bounded startup with no page is distinguished from an explicit failure HRESULT. A missing orphan observation is not successful coverage of that premise.'); $md.Add('')
    foreach ($errorText in $Result.cleanup_errors) { $md.Add("Cleanup error: $errorText") }
    [IO.File]::WriteAllLines((Join-Path $directory 'results.md'), $md, [Text.UTF8Encoding]::new($false))
}
function Get-BlockedObservation($Outcome) {
    $pageReached = @($Outcome.events | Where-Object { $_.kind -eq 'page_message' }).Count -gt 0
    $failures = @($Outcome.events | Where-Object { $_.kind -eq 'hresult' -and $_.failed })
    $startupFailure = @($failures | Where-Object { $_.stage -in @('create_environment', 'environment_callback', 'create_controller', 'controller_callback') }).Count -gt 0
    $internalTimeout = @($Outcome.events | Where-Object kind -eq 'startup_timeout').Count -gt 0
    $heartbeats = @($Outcome.events | Where-Object kind -eq 'heartbeat')
    $maxGap = 0; $previous = 0
    foreach ($beat in $heartbeats) { $maxGap = [Math]::Max($maxGap, $beat.elapsed_ms - $previous); $previous = $beat.elapsed_ms }
    # Shutdown waits are a separate finite phase, not a startup UI stall.
    $lastEvent = @($Outcome.events | Where-Object { $_.kind -eq 'host_closing' } | Select-Object -First 1)
    if (-not $lastEvent.Count) { $lastEvent = @($Outcome.events | Select-Object -Last 1) }
    if ($lastEvent.Count) { $maxGap = [Math]::Max($maxGap, $lastEvent[0].elapsed_ms - $previous) }
    return [pscustomobject]@{
        blocked = (-not $pageReached -and ($startupFailure -or ($internalTimeout -and $failures.Count -eq 0)));
        responsive = ($Outcome.outcome -ne 'external_timeout' -and $maxGap -le 3000 -and ($startupFailure -or $heartbeats.Count -gt 0));
        heartbeat_max_gap_ms = $maxGap;
        startup_terminal_elapsed_ms = $(if ($lastEvent.Count) { $lastEvent[0].elapsed_ms } else { $null });
        denial_kind = $(if ($startupFailure) { 'EXPLICIT_STARTUP_FAILURE' } elseif ($internalTimeout) { 'BOUNDED_NO_PAGE' } else { 'NOT_CONFIRMED' })
    }
}
