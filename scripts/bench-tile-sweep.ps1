# TensorRT 用タイルサイズ最適値の追試スイープ。
#
# 目的: 「大画像でタイルを 256 より大きくすると速くなるか?」の確認。
# 過去計測 (opt level 3 環境) では 256 が最速で 384 は +25% 遅かった。
# 今回は opt level 5 + 静音環境で再計測する。
#
# 使い方:
#   .\scripts\bench-tile-sweep.ps1
#
# 計測対象: anime6b (最も代表的) と nmkd_siax_4x (TRT で最も伸びるモデル) を
#   2 枚の大画像 (ComfyUI 1248x1824, mistblossom 896x1152) で計測。
# タイル: 256 / 384 / 512
# 各 (tile, model, image) で warmup 1 / runs 5 (固定タイルなのでウォームアップ短縮)
# 想定実行時間: 約 8-12 分 (うち初回エンジンビルドが 384/512 用に各 1-2 分)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent $ScriptDir
Set-Location $RepoRoot

$Stamp = Get-Date -Format 'yyyy-MM-dd_HHmmss'
$OutDir = Join-Path $RepoRoot "bench_results\tilesweep_$Stamp"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Log {
    param([string]$Msg)
    "[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss'), $Msg | Tee-Object -FilePath (Join-Path $OutDir 'master.log') -Append
}

Log "Tile size sweep (TRT, opt level 5)"
Log "  output: $OutDir"

# Locate test images via Get-ChildItem (avoid Japanese path embedding)
$img1 = (Get-ChildItem -Path 'testimage' -Filter 'ComfyUI_2_0003.png' -Recurse -File).FullName
$img2 = (Resolve-Path 'testimage/mistblossom_claude_2_9_2026-04-24-005929_0.png').Path
if (-not $img1 -or -not $img2) { Log "ERROR: missing test image"; exit 1 }
Log "  image1: $img1"
Log "  image2: $img2"

# Build once
Log "Building bench_ai..."
& cmd /c "cargo build --release --features dev-tools --bin bench_ai > `"$OutDir\build.log`" 2>&1"
if ($LASTEXITCODE -ne 0) { Log "ERROR: build failed"; exit 1 }
Log "Build OK"

# Run sweep on TRT only (DirectML's optimum is known to be 192, no point sweeping)
$jsonPath = Join-Path $OutDir 'sweep.json'
$logPath  = Join-Path $OutDir 'sweep.log'

Log "Running tile sweep..."
$tStart = Get-Date

$argParts = @(
    'cargo run --release --features dev-tools --bin bench_ai --',
    '--backend tensorrt',
    '--models realesrgan_anime6b,nmkd_siax_4x',
    '--warmup 1',
    '--runs 5',
    '--tile-size 256,384,512',
    "--image `"$img1`"",
    "--image `"$img2`"",
    "--json `"$jsonPath`""
)
$cmdLine = ($argParts -join ' ') + " > `"$logPath`" 2>&1"
& cmd /c $cmdLine

$rc = $LASTEXITCODE
$elapsed = ((Get-Date) - $tStart).TotalSeconds
Log "  done in $([Math]::Round($elapsed, 0)) s, exit=$rc"

if ($rc -ne 0) {
    Log "WARNING: sweep exited with non-zero code; check $logPath"
    exit 1
}
Log "Sweep complete: $jsonPath"
