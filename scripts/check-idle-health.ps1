[CmdletBinding()]
param(
    [string]$Scenario = "static-folder",
    [string]$ExePath = "",
    [int]$ProcessId = 0,
    [switch]$NoLaunch,
    [switch]$SkipPrompt,
    [string]$ThresholdPath = "",
    [string]$OutputDirectory = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

function Get-FileLength {
    param([string]$Path)
    if (Test-Path -LiteralPath $Path) {
        return (Get-Item -LiteralPath $Path).Length
    }
    return 0
}

function Format-Invariant {
    param([double]$Value)
    return $Value.ToString("0.000", [System.Globalization.CultureInfo]::InvariantCulture)
}

function Get-LiveProcess {
    param([int]$Id)
    try {
        $process = Get-Process -Id $Id -ErrorAction Stop
        $process.Refresh()
        if ($process.HasExited) {
            throw "process exited"
        }
        return $process
    }
    catch {
        throw "mImageViewer process $Id is not running"
    }
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($ThresholdPath)) {
    $ThresholdPath = Join-Path $PSScriptRoot "idle_health_thresholds.json"
}
if (-not (Test-Path -LiteralPath $ThresholdPath)) {
    throw "threshold file not found: $ThresholdPath"
}
$Thresholds = Get-Content -LiteralPath $ThresholdPath -Encoding UTF8 -Raw | ConvertFrom-Json
$WarmupSeconds = [int]$Thresholds.warmup_seconds
$MeasureSeconds = [int]$Thresholds.measure_seconds

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $RepoRoot "target\idle-health"
}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null

$LogDirectory = Join-Path $env:APPDATA "mimageviewer\logs"
$PerfLog = Join-Path $LogDirectory "perf_events.jsonl"
$AppLog = Join-Path $LogDirectory "mimageviewer.log"
$Launched = $false

if ($ProcessId -gt 0) {
    $Process = Get-LiveProcess -Id $ProcessId
}
elseif ($NoLaunch) {
    $Process = Get-Process -Name "mimageviewer-core" -ErrorAction SilentlyContinue |
        Sort-Object StartTime -Descending |
        Select-Object -First 1
    if ($null -eq $Process) {
        throw "mimageviewer-core is not running; omit -NoLaunch to start it"
    }
    $Process.Refresh()
}
else {
    if ([string]::IsNullOrWhiteSpace($ExePath)) {
        $ExePath = Join-Path $RepoRoot "target\release\mimageviewer-core.exe"
    }
    if (-not (Test-Path -LiteralPath $ExePath)) {
        throw "verification binary not found: $ExePath`nRun .\scripts\build-release.ps1 first."
    }
    Write-Host "Starting mImageViewer with --perf-log..."
    $Process = Start-Process -FilePath $ExePath -ArgumentList "--perf-log" `
        -WorkingDirectory $RepoRoot -PassThru
    $Launched = $true
}

$ReadyDeadline = (Get-Date).AddSeconds(15)
while (-not (Test-Path -LiteralPath $PerfLog)) {
    if ((Get-Date) -ge $ReadyDeadline) {
        throw "perf log was not created: $PerfLog"
    }
    $Process = Get-LiveProcess -Id $Process.Id
    Start-Sleep -Milliseconds 200
}
$PerfInfo = Get-Item -LiteralPath $PerfLog
if ($PerfInfo.LastWriteTime -lt $Process.StartTime.AddSeconds(-2)) {
    throw "perf log predates process start; launch this process with --perf-log"
}

Write-Host ""
Write-Host "=== idle health measurement ==="
Write-Host "Scenario : $Scenario"
Write-Host "PID      : $($Process.Id)"
Write-Host "Perf log : $PerfLog"
Write-Host ""
Write-Host "Prepare the requested static state, then do not touch the mouse or keyboard during measurement."
Write-Host "After Enter, use the $WarmupSeconds-second warmup to return focus to mImageViewer for a foreground scenario."
Write-Host "For a background scenario, leave another window in the foreground during warmup."
Write-Host "Input during warmup is excluded; stop interacting when the measurement countdown begins."
if (-not $SkipPrompt) {
    Read-Host "Press Enter when the scenario is ready" | Out-Null
}

Write-Host "Warmup: $WarmupSeconds seconds"
Start-Sleep -Seconds $WarmupSeconds

$Process = Get-LiveProcess -Id $Process.Id
$StartWall = Get-Date
$StartProcessElapsed = ($StartWall - $Process.StartTime).TotalSeconds
$StartCpu = $Process.TotalProcessorTime.TotalSeconds
$StartAppLogBytes = Get-FileLength -Path $AppLog
$StartPerfLogBytes = Get-FileLength -Path $PerfLog
$Stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

Write-Host "Measuring: $MeasureSeconds seconds (do not interact)"
Start-Sleep -Seconds $MeasureSeconds

$Stopwatch.Stop()
$Process = Get-LiveProcess -Id $Process.Id
$EndWall = Get-Date
$EndProcessElapsed = ($EndWall - $Process.StartTime).TotalSeconds
$EndCpu = $Process.TotalProcessorTime.TotalSeconds
$EndAppLogBytes = Get-FileLength -Path $AppLog
$EndPerfLogBytes = Get-FileLength -Path $PerfLog

$ElapsedSeconds = [Math]::Max($Stopwatch.Elapsed.TotalSeconds, 0.001)
$CpuDeltaSeconds = [Math]::Max($EndCpu - $StartCpu, 0.0)
# 1.0 = one logical core fully occupied. Machine-wide core countで割らない。
$CpuCoreRatio = $CpuDeltaSeconds / $ElapsedSeconds
$AppLogGrowth = [Math]::Max($EndAppLogBytes - $StartAppLogBytes, 0)
$PerfLogGrowth = [Math]::Max($EndPerfLogBytes - $StartPerfLogBytes, 0)

$Timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$SafeScenario = $Scenario -replace "[^A-Za-z0-9_.-]", "_"
$PerfReportPath = Join-Path $OutputDirectory "$Timestamp-$SafeScenario-perf.json"
$ProcessReportPath = Join-Path $OutputDirectory "$Timestamp-$SafeScenario-process.json"

$Python = Get-Command python -ErrorAction Stop
$Analyzer = Join-Path $PSScriptRoot "analyze_perf.py"
$AnalyzerArgs = @(
    $Analyzer,
    $PerfLog,
    "idle-health",
    "--start-t", (Format-Invariant -Value $StartProcessElapsed),
    "--end-t", (Format-Invariant -Value $EndProcessElapsed),
    "--target-update-rate", (Format-Invariant -Value ([double]$Thresholds.target_update_rate_per_sec)),
    "--max-update-rate", (Format-Invariant -Value ([double]$Thresholds.max_update_rate_per_sec)),
    "--max-reason-streak-secs", (Format-Invariant -Value ([double]$Thresholds.max_reason_streak_seconds)),
    "--max-same-work", ([string][int]$Thresholds.max_same_work),
    "--max-input-events", ([string][int]$Thresholds.max_input_events),
    "--json-out", $PerfReportPath
)

Write-Host ""
& $Python.Source @AnalyzerArgs
$AnalyzerExitCode = $LASTEXITCODE

$Failures = New-Object System.Collections.Generic.List[string]
$Warnings = New-Object System.Collections.Generic.List[string]
if ($CpuCoreRatio -gt [double]$Thresholds.max_cpu_core_ratio) {
    $Failures.Add(
        "CPU one-core ratio $([Math]::Round($CpuCoreRatio, 4)) exceeds $($Thresholds.max_cpu_core_ratio)"
    )
}
elseif ($CpuCoreRatio -gt [double]$Thresholds.target_cpu_core_ratio) {
    $Warnings.Add(
        "CPU one-core ratio $([Math]::Round($CpuCoreRatio, 4)) exceeds target $($Thresholds.target_cpu_core_ratio)"
    )
}
if ($AppLogGrowth -gt [long]$Thresholds.max_app_log_growth_bytes) {
    $Failures.Add(
        "mimageviewer.log grew by $AppLogGrowth bytes (limit $($Thresholds.max_app_log_growth_bytes))"
    )
}
if ($PerfLogGrowth -gt [long]$Thresholds.max_perf_log_growth_bytes) {
    $Failures.Add(
        "perf_events.jsonl grew by $PerfLogGrowth bytes (limit $($Thresholds.max_perf_log_growth_bytes))"
    )
}
if ($AnalyzerExitCode -ne 0) {
    $Failures.Add("analyze_perf.py idle-health failed with exit code $AnalyzerExitCode")
}

$Status = "pass"
if ($Failures.Count -gt 0) {
    $Status = "fail"
}
$ProcessReport = [ordered]@{
    status = $Status
    scenario = $Scenario
    timestamp = $Timestamp
    process = [ordered]@{
        id = $Process.Id
        launched_by_script = $Launched
        path = $Process.Path
    }
    window = [ordered]@{
        start_process_elapsed_secs = $StartProcessElapsed
        end_process_elapsed_secs = $EndProcessElapsed
        measured_wall_secs = $ElapsedSeconds
    }
    metrics = [ordered]@{
        cpu_delta_secs = $CpuDeltaSeconds
        cpu_one_core_ratio = $CpuCoreRatio
        app_log_growth_bytes = $AppLogGrowth
        perf_log_growth_bytes = $PerfLogGrowth
    }
    thresholds = $Thresholds
    perf_report = $PerfReportPath
    warnings = @($Warnings)
    failures = @($Failures)
}
$ProcessReport | ConvertTo-Json -Depth 8 |
    Set-Content -LiteralPath $ProcessReportPath -Encoding UTF8

Write-Host ""
Write-Host "CPU one-core ratio : $([Math]::Round($CpuCoreRatio, 4))"
Write-Host "App log growth     : $AppLogGrowth bytes"
Write-Host "Perf log growth    : $PerfLogGrowth bytes"
foreach ($Warning in $Warnings) {
    Write-Warning $Warning
}
foreach ($Failure in $Failures) {
    Write-Host "FAIL: $Failure" -ForegroundColor Red
}
Write-Host "Process report     : $ProcessReportPath"
Write-Host "Result             : $($Status.ToUpperInvariant())"
if ($Launched) {
    Write-Host "mImageViewer remains running (PID $($Process.Id)); exit it normally when checks are complete."
}

if ($Failures.Count -gt 0) {
    exit 1
}
exit 0
