# Overnight benchmark: DirectML vs TensorRT, all 6 models, 4 images.
#
# Usage (in PowerShell, just before going to sleep):
#   cd C:\home\mimageviewer\.claude\worktrees\dazzling-mcclintock-d0be64
#   .\scripts\bench-overnight.ps1
#
# What it does:
#   1. Verifies prerequisites (test images, TRT pack)
#   2. Builds bench_ai (release, ~1-2 min)
#   3. Runs DirectML benchmark on 4 images x 6 models, warmup 2 / runs 5 (~8-15 min)
#   4. Runs TensorRT benchmark on same set, warmup 2 / runs 5
#      (engine cache cold = ~10-15 min, warm = ~5 min)
#   5. Writes JSON summaries + console logs to bench_results/<timestamp>/
#
# Total estimated runtime: 20-40 min. Safe to leave overnight.
# Results land in: bench_results/<timestamp>/
#
# Notes for clean measurement:
#   - Close other apps (browsers, editors, games) before starting
#   - Plug laptop in (avoid power throttling)
#   - Disable Windows sleep timer for the duration:
#       powercfg -change -standby-timeout-ac 0
#       (revert with: powercfg -change -standby-timeout-ac 30)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# ---------------------------------------------------------------
# Setup
# ---------------------------------------------------------------
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent $ScriptDir
Set-Location $RepoRoot

$Stamp = Get-Date -Format 'yyyy-MM-dd_HHmmss'
$OutDir = Join-Path $RepoRoot "bench_results\$Stamp"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# Master log so the user can scrub through the whole run in one file
$MasterLog = Join-Path $OutDir 'master.log'

function Log {
    param([string]$Msg)
    $line = "[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss'), $Msg
    Write-Host $line
    Add-Content -Path $MasterLog -Value $line -Encoding utf8
}

Log "Starting overnight benchmark"
Log "  worktree: $RepoRoot"
Log "  output:   $OutDir"

# ---------------------------------------------------------------
# Prerequisite checks
# ---------------------------------------------------------------
$TestImages = @(
    'testimage/うちのこ/ComfyUI_2_0003.png',
    'testimage/Sonic Melty _ TuneCore Japan_files/itc316342.png',
    'testimage/CG002.jpg',
    'testimage/mistblossom_claude_2_9_2026-04-24-005929_0.png'
)
foreach ($img in $TestImages) {
    if (-not (Test-Path $img)) {
        Log "ERROR: missing test image: $img"
        Log "Aborting (cannot run benchmarks without all test images)."
        exit 1
    }
}
Log "Test images: all 4 present"

$TrtSentinel = Join-Path $env:APPDATA 'mimageviewer\tensorrt\INSTALL_OK'
if (-not (Test-Path $TrtSentinel)) {
    Log "ERROR: TensorRT pack not installed (no INSTALL_OK at $TrtSentinel)"
    Log "Run scripts\setup-tensorrt-pack.ps1 first."
    exit 1
}
Log "TensorRT pack: ready"

# ---------------------------------------------------------------
# Build bench_ai
# ---------------------------------------------------------------
Log "Building bench_ai (release)..."
$BuildLog = Join-Path $OutDir 'build.log'
& cargo build --release --bin bench_ai *> $BuildLog
if ($LASTEXITCODE -ne 0) {
    Log "ERROR: cargo build failed (exit $LASTEXITCODE)"
    Log "  see: $BuildLog"
    exit 1
}
Log "Build OK"

# ---------------------------------------------------------------
# Bench helper
# ---------------------------------------------------------------
function Run-Bench {
    param(
        [string]$Backend,   # 'directml' | 'tensorrt'
        [int]$Warmup = 2,
        [int]$Runs = 5
    )
    $models = 'realesrgan_x4plus,realesrgan_anime6b,realcugan_4x,realesr_general_v3,nmkd_siax_4x,denoise_realplksr'
    $jsonPath = Join-Path $OutDir "bench_$Backend.json"
    $logPath  = Join-Path $OutDir "bench_$Backend.log"

    Log "Running $Backend benchmark (warmup=$Warmup, runs=$Runs)..."
    Log "  json: $jsonPath"
    Log "  log:  $logPath"
    $tStart = Get-Date

    & cargo run --release --bin bench_ai -- `
        --backend $Backend `
        --models $models `
        --warmup $Warmup `
        --runs $Runs `
        --image $TestImages[0] `
        --image $TestImages[1] `
        --image $TestImages[2] `
        --image $TestImages[3] `
        --json $jsonPath `
        *> $logPath

    $rc = $LASTEXITCODE
    $elapsed = ((Get-Date) - $tStart).TotalSeconds
    Log "  $Backend done in $([Math]::Round($elapsed, 0)) s, exit=$rc"

    if ($rc -ne 0) {
        Log "  WARNING: $Backend bench exited with non-zero code"
        return $false
    }
    if (-not (Test-Path $jsonPath)) {
        Log "  WARNING: $Backend bench finished but JSON not produced"
        return $false
    }
    return $true
}

# ---------------------------------------------------------------
# Run benchmarks
# ---------------------------------------------------------------
$dmlOk = Run-Bench -Backend 'directml'
$trtOk = Run-Bench -Backend 'tensorrt'

# ---------------------------------------------------------------
# Final summary
# ---------------------------------------------------------------
Log "============================================="
Log "Overnight benchmark complete"
Log "  DirectML: $(if ($dmlOk) { 'OK' } else { 'FAILED' })"
Log "  TensorRT: $(if ($trtOk) { 'OK' } else { 'FAILED' })"
Log "  Results dir: $OutDir"
Log "============================================="

if (-not $dmlOk -or -not $trtOk) {
    Log "One or more benchmarks failed - check the .log files in $OutDir"
    exit 1
}
exit 0
