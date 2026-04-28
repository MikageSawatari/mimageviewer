# TensorRT 高速化パック (trt-pack-v1) を GitHub Releases に draft でアップロードする
# スクリプト。docs/tensorrt-pack-distribution.md §6 と等価の操作を PowerShell に
# 落としたもの。
#
# 前提:
#   - 事前に `cargo run --release --bin build_trt_pack` で
#     dist\trt-pack-v1\ に 16 ファイルが生成されていること
#   - gh CLI が PATH にあり、`gh auth status` で MikageSawatari にログイン済み
#
# 使い方 (リポジトリルートまたは worktree ルートで):
#   powershell -ExecutionPolicy Bypass -File scripts\upload-trt-pack.ps1
#
# Draft で作るので公開はされない。アップロード完了後、内容確認の上で
# Web UI から [Publish release] するか、以下を実行:
#   gh release edit trt-pack-v1 --repo MikageSawatari/mimageviewer --draft=false --prerelease
#
# やり直したい場合 (draft なので安全に削除可):
#   gh release delete trt-pack-v1 --repo MikageSawatari/mimageviewer --yes --cleanup-tag

$ErrorActionPreference = 'Stop'

# スクリプト位置からリポジトリルートを推定
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent $ScriptDir
Set-Location $RepoRoot

$Tag        = 'trt-pack-v1'
$Repo       = 'MikageSawatari/mimageviewer'
$Title      = 'TensorRT acceleration pack v1'
$NotesFile  = 'docs\tensorrt-pack-release-notes.md'
$DistDir    = 'dist\trt-pack-v1'

# 16 個のアセット (順序: manifest → notices → DLL × 13 → engine zip)。
# build_trt_pack.rs が出力する全ファイルを列挙。一覧は明示的に書いて
# 「うっかり別ファイルが混ざる」事故を防止。
$Assets = @(
    'manifest.json',
    'NOTICE-NVIDIA.txt',
    'LICENSE-onnxruntime.txt',
    'cudart64_12.dll',
    'cublasLt64_12.dll',
    'cufft64_11.dll',
    'cudnn_ops64_9.dll',
    'nvJitLink_120_0.dll',
    'nvrtc64_120_0.dll',
    'nvrtc-builtins64_129.dll',
    'nvinfer_10.dll',
    'nvinfer_plugin_10.dll',
    'onnxruntime.dll',
    'onnxruntime_providers_shared.dll',
    'onnxruntime_providers_cuda.dll',
    'onnxruntime_providers_tensorrt.dll',
    'engines-ampere_plus.zip'
)

# 事前チェック: 全ファイルが存在しサイズ > 0 か
Write-Host '============================================='
Write-Host " Pre-flight: dist\trt-pack-v1\ 検証"
Write-Host '============================================='
$totalBytes = 0
foreach ($name in $Assets) {
    $path = Join-Path $DistDir $name
    if (-not (Test-Path $path)) {
        Write-Error "missing: $path"
        exit 1
    }
    $size = (Get-Item $path).Length
    if ($size -le 0) {
        Write-Error "zero-byte: $path"
        exit 1
    }
    $totalBytes += $size
    Write-Host ("  ok: {0,-40} {1,12:N0} bytes" -f $name, $size)
}
$totalGb = [Math]::Round($totalBytes / 1GB, 2)
Write-Host ("`n total: {0:N0} bytes ({1} GB)" -f $totalBytes, $totalGb)

# notes ファイルの存在チェック
if (-not (Test-Path $NotesFile)) {
    Write-Error "release notes file not found: $NotesFile"
    exit 1
}

# gh 認証チェック
Write-Host ''
Write-Host '============================================='
Write-Host ' Pre-flight: gh CLI 認証'
Write-Host '============================================='
gh auth status
if ($LASTEXITCODE -ne 0) {
    Write-Error 'gh CLI が認証されていません。`gh auth login` を実行してください。'
    exit 1
}

# 既存タグ衝突チェック
$existing = gh release view $Tag --repo $Repo --json tagName 2>$null
if ($LASTEXITCODE -eq 0 -and $existing) {
    Write-Host ''
    Write-Warning "$Tag は既に存在します。続行すると失敗します。"
    Write-Warning "削除する場合: gh release delete $Tag --repo $Repo --yes --cleanup-tag"
    exit 1
}

# アップロード実行
Write-Host ''
Write-Host '============================================='
Write-Host " gh release create $Tag (draft)"
Write-Host '============================================='
Write-Host "アップロード中... ($totalGb GB、5〜15 分目安)"

# アセットパスのリスト (full path で渡す方が gh の取り違えがない)
$AssetPaths = $Assets | ForEach-Object { Join-Path $DistDir $_ }

& gh release create $Tag `
    --repo $Repo `
    --target main `
    --title $Title `
    --notes-file $NotesFile `
    --draft `
    @AssetPaths

if ($LASTEXITCODE -ne 0) {
    Write-Error "gh release create failed (exit $LASTEXITCODE)"
    exit $LASTEXITCODE
}

# 確認表示
Write-Host ''
Write-Host '============================================='
Write-Host ' 完了 (draft、まだ公開されていません)'
Write-Host '============================================='
gh release view $Tag --repo $Repo

Write-Host ''
Write-Host 'Web UI で内容を確認してから公開してください:'
Write-Host "  https://github.com/$Repo/releases"
Write-Host ''
Write-Host '公開する場合 (CLI で):'
Write-Host "  gh release edit $Tag --repo $Repo --draft=false --prerelease"
Write-Host ''
Write-Host 'やり直す場合:'
Write-Host "  gh release delete $Tag --repo $Repo --yes --cleanup-tag"
