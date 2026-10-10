[CmdletBinding()]
param([string]$Scenario, [string]$TargetKey, [int]$ProcessId,
    [datetime]$ExpectedStartTimeUtc, [string]$SleepEvidencePath, [string]$OutputDirectory,
    [long]$RootHwnd, [long]$BackgroundHwnd, [int]$BackgroundProcessId, [string]$CaseId, [switch]$InteractiveApproved)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
if (-not $InteractiveApproved -or $ProcessId -le 0) { exit 2 }
. (Join-Path $PSScriptRoot 'Support.ps1')
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$scripts = Join-Path $repo 'scripts'
Import-ReleaseCheckFunctions (Join-Path $scripts 'ui-smoke.ps1') @('Get-NormalizedPath','Assert-ExactPath','Assert-NoReparsePath','Assert-NoReparseTree')
Import-ReleaseCheckFunctions (Join-Path $scripts 'build-portable.ps1') @('Get-MivOrdinalUniquePaths','Get-MivSourceFingerprintRecords','Get-MivSourceFingerprint','Assert-SmokeBuildManifest')
Import-ReleaseCheckFunctions (Join-Path $scripts 'check-idle-health.ps1') @('Get-FileLength','Format-Invariant','Test-InitialIndexScanSettled')
$exe = Join-Path $repo 'target\portable-smoke\mimageviewer.exe'
$data = Join-Path $repo 'target\portable-smoke\data'
$preflight=Invoke-ReleaseCheckPreflight {
Assert-ReleaseCheckPaths $repo $exe $data $OutputDirectory
$null = Assert-ExactPath $SleepEvidencePath (Join-Path $OutputDirectory 'completion.json') 'release-sleep-evidence'
Assert-ReleaseCheckArtifact $repo
Initialize-ReleaseCheckNative
}
if($preflight.exit_code -ne 0){
    [Console]::Error.WriteLine(('ReleaseChecks sampler SKIPPED: '+$preflight.environment_error))
    exit 2
}
$perf = Join-Path $data 'logs\perf_events.jsonl'
$app = Join-Path $data 'logs\mimageviewer.log'
$thresholds = Get-Content -LiteralPath (Join-Path $scripts 'idle_health_thresholds.json') -Encoding UTF8 -Raw | ConvertFrom-Json
$reportPath = Join-Path $OutputDirectory 'sampler.json'
$report = [ordered]@{ schema_version=1; case_id=$CaseId; expected_pid=$ProcessId; verdict='SKIPPED'; exit_code=2; failures=@(); environment_errors=@(); warnings=@(); thresholds=$thresholds }
function Get-ExactMeasuredProcess {
    $p = Get-Process -Id $ProcessId -ErrorAction Stop; $p.Refresh()
    $cim = Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction Stop
    Assert-ReleaseCheckIdentity $p $cim $ProcessId $ExpectedStartTimeUtc $exe $data
    return $p
}
try {
    $p = Get-ExactMeasuredProcess
    if (-not (Test-InitialIndexScanSettled $perf)) { throw 'Initial index scan not settled before idle pause.' }
    $report.index_scan_wait='settled'; $report.identity_evidence='command_line'; $report.portable_identity_verified=$true
    if ($Scenario -eq 'video-pin-background' -and [string]::IsNullOrWhiteSpace($TargetKey)) { throw 'Video pin target key missing.' }
    Start-Sleep -Seconds ([int]$thresholds.warmup_seconds)
    $p = Get-ExactMeasuredProcess
    $startCpu = $p.TotalProcessorTime.TotalSeconds
    $startApp = Get-FileLength $app; $startPerf = Get-FileLength $perf
    $samples = New-Object 'Collections.Generic.List[object]'
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $startUtc = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $samples.Add((Get-ReleaseCheckWindowSample $ProcessId))
    while ($clock.Elapsed.TotalSeconds -lt [double]$thresholds.measure_seconds) {
        $p = Get-ExactMeasuredProcess
        $samples.Add((Get-ReleaseCheckWindowSample $ProcessId))
        if ('System.Windows.Forms.Application' -as [type]) { [Windows.Forms.Application]::DoEvents() }
        Start-Sleep -Milliseconds 250
    }
    $endUtc = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(); $clock.Stop()
    $samples.Add((Get-ReleaseCheckWindowSample $ProcessId))
    $p = Get-ExactMeasuredProcess
    $endCpu = $p.TotalProcessorTime.TotalSeconds
    $endApp = Get-FileLength $app; $endPerf = Get-FileLength $perf
    $report.measure_start_unix_ms=$startUtc; $report.measure_end_unix_ms=$endUtc
    $report.wall_seconds=$clock.Elapsed.TotalSeconds; $report.window_samples=@($samples.ToArray())
    $report.metrics=[ordered]@{cpu_delta_secs=$endCpu-$startCpu; cpu_one_core_ratio=($endCpu-$startCpu)/$clock.Elapsed.TotalSeconds; app_log_growth_bytes=$endApp-$startApp; perf_log_growth_bytes=$endPerf-$startPerf}
    $report.cpu_core_ratio=$report.metrics.cpu_one_core_ratio
    $report.app_log_growth_bytes=$report.metrics.app_log_growth_bytes; $report.perf_log_growth_bytes=$report.metrics.perf_log_growth_bytes
    $report.process_alive_start=$true; $report.process_alive_end=$true
    $report.root_hwnd=$RootHwnd; $report.background_hwnd=$BackgroundHwnd; $report.background_process_id=$BackgroundProcessId
    Assert-ReleaseCheckWindowSamples $samples $Scenario $ProcessId $RootHwnd $BackgroundHwnd $BackgroundProcessId
    # The real worker boundary, not notification arrival, closes the clock window.
    $wait = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $SleepEvidencePath -PathType Leaf)) {
        if ($wait.Elapsed.TotalSeconds -gt 45) { throw 'Worker sleep completion evidence missing.' }
        Start-Sleep -Milliseconds 100
    }
    $sleep = Get-Content -LiteralPath $SleepEvidencePath -Encoding UTF8 -Raw | ConvertFrom-Json
    if ($sleep.pid -ne $ProcessId) { throw 'Worker sleep PID mismatch.' }
    $report.sleep_start_unix_ms=$sleep.sleep_start_unix_ms; $report.sleep_end_unix_ms=$sleep.sleep_end_unix_ms
    Write-ReleaseCheckJson $reportPath $report
    $clockReport = Join-Path $OutputDirectory 'clock.json'
    & python (Join-Path $scripts 'release_check_analysis.py') clock-window $perf --sampler $reportPath --expected-pid $ProcessId --json-out $clockReport
    if ($LASTEXITCODE -ne 0) { throw 'Session clock/sleep containment could not be verified.' }
    $converted = Get-Content -LiteralPath $clockReport -Encoding UTF8 -Raw | ConvertFrom-Json
    $report.evidence=[ordered]@{clock=$converted.evidence.clock; process=[ordered]@{pid=$ProcessId; start_time_utc=$ExpectedStartTimeUtc.ToString('o'); path=$exe; data_dir=$data; identity_evidence='command_line'}}
    $perfReport = Join-Path $OutputDirectory 'idle-perf.json'
    $args = @((Join-Path $scripts 'analyze_perf.py'),$perf,'idle-health','--start-t',(Format-Invariant $converted.evidence.clock.start_t),'--end-t',(Format-Invariant $converted.evidence.clock.end_t),
        '--target-update-rate',(Format-Invariant $thresholds.target_update_rate_per_sec),'--max-update-rate',(Format-Invariant $thresholds.max_update_rate_per_sec),
        '--max-reason-streak-secs',(Format-Invariant $thresholds.max_reason_streak_seconds),'--max-same-work',[string]$thresholds.max_same_work,
        '--max-input-events',[string]$thresholds.max_input_events,'--expected-pid',[string]$ProcessId,'--allow-sleeping-window','--json-out',$perfReport)
    if ($Scenario -eq 'video-pin-background') { $args += @('--require-work-key',$TargetKey) }
    & python @args
    if ($LASTEXITCODE -ne 0) { $report.failures += 'Idle perf verdict failed.' }
    $report.perf_report=$perfReport
    if ($report.metrics.cpu_one_core_ratio -gt $thresholds.max_cpu_core_ratio) { $report.failures += 'CPU one-core limit exceeded.' }
    elseif ($report.metrics.cpu_one_core_ratio -gt $thresholds.target_cpu_core_ratio) { $report.warnings += 'CPU target exceeded.' }
    if ($report.metrics.app_log_growth_bytes -gt $thresholds.max_app_log_growth_bytes -or $report.metrics.perf_log_growth_bytes -gt $thresholds.max_perf_log_growth_bytes) { $report.failures += 'Log growth limit exceeded.' }
    if ($report.metrics.app_log_growth_bytes -lt 0 -or $report.metrics.perf_log_growth_bytes -lt 0) { throw 'Logs rotated during measurement.' }
    $report.verdict=if($report.failures.Count){'FAIL'}elseif($report.warnings.Count){'PASS_WITH_WARNINGS'}else{'PASS'}
    $report.exit_code=if($report.failures.Count){1}else{0}
} catch { $report.environment_errors += $_.Exception.Message }
Write-ReleaseCheckJson $reportPath $report
exit $report.exit_code
