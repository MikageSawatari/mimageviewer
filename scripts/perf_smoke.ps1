[CmdletBinding()]
param(
    [int]$ThresholdMs = 16,
    [string]$ExePath = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

# Collect a perf log over the startup / navigation / search scenarios and list the
# frame-interval gaps with analyze_perf.py (CLAUDE.md release checklist, Phase 2).
#
#   .{0}scripts{0}perf_smoke.ps1     (run it as: scripts/perf_smoke.ps1)
#
# The scenario itself is manual: open a folder, press Ctrl+Down five times, search
# with Ctrl+G, then exit the app completely. Analysis runs once the process exits.
#
# The distributed mimageviewer.exe is the launcher and exits as soon as it spawns
# core, so it cannot be waited on. This starts core directly, which links FFmpeg
# through its import library, so the DLLs are staged next to the exe first.
#
# ASCII only on purpose: PowerShell 5.1 reads a BOM-less UTF-8 script as the ANSI
# codepage, which turns Japanese output into mojibake (CLAUDE.md encoding policy).

$RepoRoot = Split-Path -Parent $PSScriptRoot
$ReleaseDir = Join-Path $RepoRoot "target/release"
if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $ReleaseDir "mimageviewer-core.exe"
}
if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "verification binary not found: $ExePath`nRun scripts/build-release.ps1 first."
}

$FfmpegBin = Join-Path $RepoRoot "vendor/ffmpeg/bin"
$FfmpegDlls = @(Get-ChildItem -Path (Join-Path $FfmpegBin "*.dll") -ErrorAction SilentlyContinue)
if ($FfmpegDlls.Count -eq 0) {
    throw "no FFmpeg DLL in $FfmpegBin`nRun: bash scripts/setup-ffmpeg.sh"
}
Copy-Item -Path $FfmpegDlls.FullName -Destination $ReleaseDir -Force
Write-Host "(staged $($FfmpegDlls.Count) FFmpeg DLLs into $ReleaseDir)"

$PerfLog = Join-Path $env:APPDATA "mimageviewer/logs/perf_events.jsonl"

Write-Host ""
Write-Host "=== perf smoke ==="
Write-Host "1. Starting mImageViewer (core) with --perf-log."
Write-Host "2. Do this by hand while it runs:"
Write-Host "     a) open any folder"
Write-Host "     b) press Ctrl+Down five times (move between folders)"
Write-Host "     c) press Ctrl+G, type something, press Enter"
Write-Host "     d) exit the app completely (from the tray icon if tray residency is on)"
Write-Host "3. Analysis runs after the process exits."
Write-Host ""
Write-Host "perf-log: $PerfLog"
Write-Host ""

if (Test-Path -LiteralPath $PerfLog) {
    Move-Item -LiteralPath $PerfLog -Destination "$PerfLog.prev" -Force
    Write-Host "(moved the previous log to $PerfLog.prev)"
}

$Process = Start-Process -FilePath $ExePath -ArgumentList "--perf-log" `
    -WorkingDirectory $RepoRoot -PassThru
$Process.WaitForExit()
Write-Host ""
Write-Host "mImageViewer exit code: $($Process.ExitCode)"

if (-not (Test-Path -LiteralPath $PerfLog)) {
    throw "perf-log was not written: $PerfLog"
}

Write-Host ""
Write-Host "=== analyze_perf.py hitches (>= ${ThresholdMs}ms) ==="
& python (Join-Path $RepoRoot "scripts/analyze_perf.py") $PerfLog hitches --ms $ThresholdMs

Write-Host ""
Write-Host "How to read this - the count alone is NOT the verdict:"
Write-Host "  mIV deliberately sleeps when idle, so every pause between your own actions"
Write-Host "  is counted as a hitch. Judge the gaps over 100ms one at a time, by the"
Write-Host "  ui.tail_repaint.action immediately before each:"
Write-Host "    none                               -> no repaint requested, asleep waiting"
Write-Host "                                          for input. Expected."
Write-Host "    request_repaint_after_idle_upgrade -> a scheduled wake. Expected."
Write-Host "    anything else over 100ms           -> suspect a synchronous I/O regression"
Write-Host "                                          on the UI thread"
Write-Host "                                          (docs/ui-responsiveness.md section 4)."
Write-Host "  Overall target: at least 97% of gaps under 16ms."
