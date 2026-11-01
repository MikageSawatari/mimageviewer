param(
    [Parameter(Mandatory = $true)][string]$MigrationExe,
    [switch]$SkipBuild,
    [switch]$InteractiveApproved
)
# ASCII only. External, explicitly approved suite; never called by offline tests.
$ErrorActionPreference = 'Stop'
if (-not $InteractiveApproved) { throw 'This WebView2 suite requires prior approval of the scenarios and desktop use, then -InteractiveApproved.' }
$env:MSBUILDDISABLENODEREUSE = '1'
. (Join-Path $PSScriptRoot 'migration-common.ps1')
$layout = Get-MigrationRunLayout
$pathBudget = Assert-MigrationPathBudget $layout.source $layout.destination
$originalPathBudget = Assert-MigrationPathBudget $layout.source $layout.original_destination
foreach ($check in @($pathBudget.checks) + @($originalPathBudget.checks)) {
    Write-Host ("WebView2 path preflight: {0} = {1} UTF-16 units, headroom {2} -- {3}" -f
        $check.role, $check.utf16_units, $check.headroom_utf16_units, $check.indexeddb_file)
}
if (-not $SkipBuild) { & (Join-Path $PSScriptRoot 'build.ps1') }
$script:MigrationProbe = Join-Path $script:MigrationBinaryRoot 'bin\probe.exe'
$MigrationExe = [IO.Path]::GetFullPath($MigrationExe)
if (-not [IO.File]::Exists($MigrationExe)) { throw 'Build the test-script core migration helper first.' }
$build = Get-Content -LiteralPath (Join-Path $script:MigrationBinaryRoot 'bin\build.json') -Raw -Encoding UTF8 | ConvertFrom-Json
Assert-ProbePathPolicy $build.path_policy $script:MigrationBinaryRoot
foreach ($pair in @(@($script:MigrationProbe, $build.exe_sha256),
    @((Join-Path $script:MigrationBinaryRoot 'bin\page.html'), $build.page_sha256),
    @((Join-Path $script:MigrationBinaryRoot 'bin\WebView2Loader.dll'), $build.loader_sha256),
    @((Join-Path $PSScriptRoot 'probe.cpp'), $build.source_sha256),
    @((Join-Path $PSScriptRoot 'snapshot.h'), $build.snapshot_sha256),
    @((Join-Path $PSScriptRoot 'page.html'), $build.page_sha256))) {
    if ((Get-FileHash -LiteralPath $pair[0] -Algorithm SHA256).Hash -ne $pair[1]) { throw "Probe build/source mismatch: $($pair[0])" }
}
# The hashed executable must also report the same compiled policy now. Querying
# this mode creates neither a host nor WebView2, including with -SkipBuild.
Assert-ProbePathPolicy (Read-ProbePathPolicy $script:MigrationProbe) $script:MigrationBinaryRoot
$script:MigrationRun = $layout.root
Assert-ProbePath $script:MigrationRun
New-Item -ItemType Directory -Path $script:MigrationRun -ErrorAction Stop | Out-Null
$script:MigrationCheckpoints = $layout.checkpoints
[IO.Directory]::CreateDirectory($script:MigrationCheckpoints) | Out-Null
$script:MigrationChildren = [Collections.Generic.List[object]]::new()
$script:MigrationUdfs = [Collections.Generic.List[string]]::new()
$script:MigrationToken = [Guid]::NewGuid().ToString('D')
$fake = $layout.fake_appdata
$data = $layout.data_dir
$source = $layout.source
$destination = $layout.destination
$originalData = $layout.original_data_dir
$originalDestination = $layout.original_destination
foreach ($path in @($fake, $data, $source, $destination, $originalData, $originalDestination)) { Assert-ProbePath $path }
$script:MigrationUdfs.Add($source); $script:MigrationUdfs.Add($destination); $script:MigrationUdfs.Add($originalDestination)
[IO.Directory]::CreateDirectory((Join-Path $data 'effetune')) | Out-Null
[IO.File]::WriteAllText((Join-Path $data 'effetune\mixwright-state.json'), '{}')
$started = [DateTime]::UtcNow
$beforeCrash = Get-CrashObservation ($started.AddHours(-24))
Save-Json (Join-Path $script:MigrationRun 'crash-before.json') $beforeCrash
Save-Json (Join-Path $script:MigrationRun 'path-budget.json') $pathBudget
Save-Json (Join-Path $script:MigrationRun 'original-wait-path-budget.json') $originalPathBudget
$result = [ordered]@{ suite = 'F2b-1 production migration engine with real WebView2'; status = 'ERROR';
    started_utc = $started.ToString('o'); source = $source; destination = $destination;
    path_budget = $pathBudget;
    original_wait_path_budget = $originalPathBudget;
    helper = $MigrationExe; helper_sha256 = (Get-FileHash -LiteralPath $MigrationExe -Algorithm SHA256).Hash;
    build = $build; phases = [ordered]@{}; competitors = [ordered]@{};
    original_source_close_overlap = $null; error = $null; cleanup_errors = @();
    historical_probe_prerequisite = 'Historical strict STOP is preserved; this is a distinct eighth-edition acceptance suite.';
    coverage_limit = 'Generic storage only; does not prove v0.12-v0.14 plugin data compatibility or product UI acceptance.' }
try {
    $seed = Start-MigrationHost $source 'write' 'source-write'
    $result.seed_storage = Wait-MigrationStorage $seed
    Close-MigrationHost $seed
    # Scenario 1 is independent of the checkpoint-paused competitor scenario.
    # Native helper code requests Close immediately before its production lock
    # call, observes the pre-opened original browser handle, and lets the real
    # process wait finish before any checkpoint/JSON output. PowerShell stays
    # outside the original browser's 8-12ms singleton-release residual window.
    $reader = Start-MigrationHost $source 'read' 'source-close-during-migration'
    $null = Wait-MigrationStorage $reader
    $browserEvents = @(Read-Events $reader.log | Where-Object kind -eq 'browser_pid')
    if ($browserEvents.Count -ne 1 -or [uint32]$browserEvents[0].pid -eq 0) { throw 'No unique original browser PID before close.' }
    $originalPid = [uint32]$browserEvents[0].pid
    $script:MigrationCheckpoints = $layout.original_checkpoints
    [IO.Directory]::CreateDirectory($script:MigrationCheckpoints) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $originalData 'effetune')) | Out-Null
    [IO.File]::WriteAllText((Join-Path $originalData 'effetune\mixwright-state.json'), '{}')
    $originalHelper = Start-MigrationChild $MigrationExe @('--miv-sound-migrate-only',
        '--miv-sound-migration-source', $fake, '--data-dir', $originalData,
        '--miv-sound-migration-checkpoints', $script:MigrationCheckpoints,
        '--miv-sound-migration-original-browser', ([string]$originalPid),
        '--miv-sound-migration-close-file', $reader.stop) 'production-original-wait' -Helper
    $result.original_wait_phase = Wait-MigrationPhase $originalHelper 'process_wait_end'
    $result.original_wait_lock_held = Assert-MigrationHeldLock $source
    Wait-MigrationExit $reader 5000
    Assert-MigrationHostExit $reader
    $originalBaseline = Get-MigrationManifest $source
    Save-Json (Join-Path $script:MigrationRun 'original-wait-source-after-wait.json') $originalBaseline
    Resume-MigrationPhase 'process_wait_end'
    $null = Wait-MigrationPhase $originalHelper 'copy_begin'
    $result.original_copy_begin_lock_held = Assert-MigrationHeldLock $source
    Resume-MigrationPhase 'copy_begin'
    $null = Wait-MigrationPhase $originalHelper 'copy_end'
    $result.original_copy_end_lock_held = Assert-MigrationHeldLock $source
    $originalCopied = Get-MigrationManifest $originalDestination
    Save-Json (Join-Path $script:MigrationRun 'original-wait-destination-before-open.json') $originalCopied
    Assert-MigrationExclusions $originalDestination
    $result.original_production_copy_comparison = Compare-Manifests $originalBaseline.included $originalCopied.included
    Resume-MigrationPhase 'copy_end'
    Wait-MigrationExit $originalHelper 15000
    $null = Wait-MigrationPhase $originalHelper 'result' 1000
    $result.original_helper_events = @(Read-MigrationStdout $originalHelper)
    Assert-MigrationHelperEvents $result.original_helper_events
    $result.original_source_close_overlap = Get-MigrationOriginalWaitObservation $result.original_helper_events $originalPid
    if ($originalHelper.process.ExitCode -ne 0 -or -not [IO.File]::Exists((Join-Path $originalData 'effetune\migration-v1.done'))) {
        throw 'Original-close production migration did not complete with its done marker.'
    }
    $originalAfterRelease = Get-MigrationManifest $source
    Save-Json (Join-Path $script:MigrationRun 'original-wait-source-after-release.json') $originalAfterRelease
    $result.original_source_after_release = Compare-Manifests $originalBaseline.included $originalAfterRelease.included
    if (-not $result.original_production_copy_comparison.equal -or -not $result.original_source_after_release.equal) {
        throw 'Original-close migration changed the source or copied different profile bytes.'
    }
    foreach ($round in @(1, 2)) {
        $read = Start-MigrationHost $originalDestination 'read' ('original-wait-destination-read-' + $round)
        $result[('original_wait_destination_storage_' + $round)] = Wait-MigrationStorage $read
        Close-MigrationHost $read
    }
    # Scenario 2 preserves the original wait/copy competitors and their bounded
    # lock checkpoints. It is a separate destination, never a migration retry.
    $script:MigrationCheckpoints = $layout.checkpoints
    $result.source_lock_existed_before_helper = [IO.File]::Exists((Join-Path $source 'EBWebView\lockfile'))
    $helper = Start-MigrationChild $MigrationExe @('--miv-sound-migrate-only',
        '--miv-sound-migration-source', $fake, '--data-dir', $data,
        '--miv-sound-migration-checkpoints', $script:MigrationCheckpoints) 'production-migration' -Helper
    $result.phases.lock_acquired = Wait-MigrationPhase $helper 'lock_acquired'
    $result.lock_held_at_acquired = Assert-MigrationHeldLock $source
    $baseline = Get-MigrationManifest $source
    Save-Json (Join-Path $script:MigrationRun 'source-before-attempts.json') $baseline
    $waitCompetitor = Start-MigrationHost $source 'read' 'competitor-wait'
    $result.competitors.wait_overlap = Wait-MigrationCompetitor $waitCompetitor $source
    Resume-MigrationPhase 'lock_acquired'
    $result.phases.copy_begin = Wait-MigrationPhase $helper 'copy_begin'
    $result.lock_held_at_copy_begin = Assert-MigrationHeldLock $source
    $beforeCopy = Get-MigrationManifest $source
    Save-Json (Join-Path $script:MigrationRun 'source-before-copy.json') $beforeCopy
    $result.source_after_wait_attempt = Compare-Manifests $baseline.included $beforeCopy.included
    $copyCompetitor = Start-MigrationHost $source 'read' 'competitor-copy'
    $result.competitors.copy_overlap = Wait-MigrationCompetitor $copyCompetitor $source
    Resume-MigrationPhase 'copy_begin'
    $result.phases.copy_end = Wait-MigrationPhase $helper 'copy_end'
    $result.lock_held_at_copy_end = Assert-MigrationHeldLock $source
    # The helper retains the source lock at copy_end. Wait only for startup's
    # terminal decision, not browser teardown, before releasing that checkpoint.
    foreach ($child in @($waitCompetitor, $copyCompetitor)) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $events = @(Read-Events $child.log)
            $terminal = @($events | Where-Object { $_.kind -eq 'host_closing' -or $_.kind -eq 'page_message' -or
                $_.kind -eq 'startup_timeout' -or ($_.kind -eq 'hresult' -and $_.failed -and
                $_.stage -in @('create_environment', 'environment_callback', 'create_controller', 'controller_callback')) })
            if ($terminal.Count -or $child.process.HasExited) { break }
            Start-Sleep -Milliseconds 10
        } while ($clock.ElapsedMilliseconds -lt 3500)
        if (-not $terminal.Count) { throw 'No competing startup terminal event while migration retained the lock.' }
    }
    $afterCopy = Get-MigrationManifest $source
    $copied = Get-MigrationManifest $destination
    Save-Json (Join-Path $script:MigrationRun 'source-after-copy-held.json') $afterCopy
    Save-Json (Join-Path $script:MigrationRun 'destination-before-open.json') $copied
    Assert-MigrationExclusions $destination
    $result.source_after_copy_attempt = Compare-Manifests $beforeCopy.included $afterCopy.included
    $result.production_copy_comparison = Compare-Manifests $beforeCopy.included $copied.included
    Resume-MigrationPhase 'copy_end'
    Wait-MigrationExit $helper 15000
    $null = Wait-MigrationPhase $helper 'result' 1000
    $result.helper_events = @(Read-MigrationStdout $helper)
    Assert-MigrationHelperEvents $result.helper_events
    if ($helper.process.ExitCode -ne 0 -or -not [IO.File]::Exists((Join-Path $data 'effetune\migration-v1.done'))) {
        throw 'Production migration did not complete with its done marker.'
    }
    $result.competitors.wait_denial = Get-MigrationBlockedResult $waitCompetitor
    $result.competitors.copy_denial = Get-MigrationBlockedResult $copyCompetitor
    $result.source_lock_exists_after_helper = [IO.File]::Exists((Join-Path $source 'EBWebView\lockfile'))
    if (-not $result.source_lock_existed_before_helper -and $result.source_lock_exists_after_helper) {
        throw 'Migration left a newly created source lockfile behind.'
    }
    $afterRelease = Get-MigrationManifest $source
    Save-Json (Join-Path $script:MigrationRun 'source-after-release.json') $afterRelease
    $result.source_after_release = Compare-Manifests $baseline.included $afterRelease.included
    foreach ($comparison in @($result.source_after_wait_attempt, $result.source_after_copy_attempt,
        $result.production_copy_comparison, $result.source_after_release)) {
        if (-not $comparison.equal) { throw 'Source preservation or production copy hash comparison failed; inspect manifests.' }
    }
    # Verify the destination twice across full WebView2/controller teardown.
    foreach ($round in @(1, 2)) {
        $read = Start-MigrationHost $destination 'read' ('destination-read-' + $round)
        $result[('destination_storage_' + $round)] = Wait-MigrationStorage $read
        Close-MigrationHost $read
    }
    $readOriginal = Start-MigrationHost $source 'read' 'source-positive-control'
    $result.original_storage = Wait-MigrationStorage $readOriginal
    Close-MigrationHost $readOriginal
    $result.status = Get-MigrationAcceptanceStatus $result.original_source_close_overlap `
        $result.competitors.wait_overlap $result.competitors.copy_overlap
} catch { $result.error = $_.Exception.ToString(); $result.status = 'ERROR' }
finally {
    $result.cleanup_errors = Complete-MigrationCleanup
    if ($result.cleanup_errors.Count) { $result.status = 'ERROR' }
    Start-Sleep -Milliseconds 1000
    $afterCrash = Get-CrashObservation $started
    Save-Json (Join-Path $script:MigrationRun 'crash-after.json') $afterCrash
    $result.crash_before = $beforeCrash; $result.crash_after = $afterCrash
    # Baseline application events are historical, never attributed to this run.
    if (-not $beforeCrash.complete -or -not $afterCrash.complete -or $beforeCrash.werfault.Count -or
        $afterCrash.werfault.Count -or $afterCrash.application_events.Count) { $result.status = 'ERROR' }
    $result.finished_utc = [DateTime]::UtcNow.ToString('o')
    Save-Json (Join-Path $script:MigrationRun 'results.json') $result
    $md = @('# F2b-1 production migration probe', '', ('Status: **' + $result.status + '**'), '',
        'Disposable fake APPDATA and isolated data-dir only. Production Rust engine owns all migration copies.', '',
        ('Original closing source process overlap: ' + $result.original_source_close_overlap.observed),
        ('Wait-phase competing process overlap: ' + $result.competitors.wait_overlap.exact_process_overlap_observed),
        ('Copy-phase competing process overlap: ' + $result.competitors.copy_overlap.exact_process_overlap_observed), '',
        'Read results.json, raw JSONL, process records, and manifests for hashes, phases, startup denial, cleanup, and WER observations.',
        'No host or Runtime process is terminated, even if cleanup fails. Leftover PIDs are errors and require operator attention.', '',
        $result.coverage_limit, '', ('Error: ' + $result.error))
    [IO.File]::WriteAllLines((Join-Path $script:MigrationRun 'results.md'), [string[]]$md, [Text.Encoding]::ASCII)
    Write-Host ('Migration probe: ' + $result.status + ' -- ' + $script:MigrationRun)
}
if ($result.status -ne 'PASS') { exit 2 }
