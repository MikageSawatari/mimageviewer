# Upload the TensorRT acceleration pack (trt-pack-v2) to GitHub Releases as a draft.
# Equivalent to docs/tensorrt-pack-distribution.md section 6, expressed as PowerShell.
#
# Prerequisites:
#   - dist\trt-pack-v2\ must contain the 21 files produced by
#     `cargo run --release --bin build_trt_pack`
#   - gh CLI must be on PATH and authenticated as MikageSawatari
#     (run `gh auth status` to check)
#
# Usage (from worktree or repo root):
#   powershell -ExecutionPolicy Bypass -File scripts\upload-trt-pack.ps1
#
# This script creates a DRAFT release. Nothing is published until you flip the
# draft flag. Inspect the contents in the Web UI first, then publish via:
#   gh release edit trt-pack-v2 --repo MikageSawatari/mimageviewer --draft=false --prerelease
#
# To roll back (safe while still draft):
#   gh release delete trt-pack-v2 --repo MikageSawatari/mimageviewer --yes --cleanup-tag
#
# IMPORTANT: this file is intentionally ASCII-only. Windows PowerShell 5.1 reads
# .ps1 files as ANSI (CP932 in JP locale) unless they carry a UTF-8 BOM, so
# embedding Japanese strings without a BOM produces mojibake and parse errors.

$ErrorActionPreference = 'Stop'

# Resolve repo root from this script's location.
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent $ScriptDir
Set-Location $RepoRoot

$Tag       = 'trt-pack-v2'
$Repo      = 'MikageSawatari/mimageviewer'
$Title     = 'TensorRT acceleration pack v2'
$NotesFile = 'docs\tensorrt-pack-release-notes.md'
$DistDir   = 'dist\trt-pack-v2'

# 21 assets, listed explicitly so an unrelated file in dist\ cannot leak in.
# Order: manifest -> notices -> 17 DLLs -> engine zip.
# v2 differences from v1: added 4 DLLs (cublas64_12, cudnn64_9, cudnn_graph64_9,
# nvonnxparser_10) that were incorrectly removed in v1 trim, causing CPU fallback.
$Assets = @(
    'manifest.json',
    'NOTICE-NVIDIA.txt',
    'LICENSE-onnxruntime.txt',
    'cudart64_12.dll',
    'cublas64_12.dll',
    'cublasLt64_12.dll',
    'cufft64_11.dll',
    'cudnn64_9.dll',
    'cudnn_graph64_9.dll',
    'cudnn_ops64_9.dll',
    'nvJitLink_120_0.dll',
    'nvrtc64_120_0.dll',
    'nvrtc-builtins64_129.dll',
    'nvinfer_10.dll',
    'nvinfer_plugin_10.dll',
    'nvonnxparser_10.dll',
    'onnxruntime.dll',
    'onnxruntime_providers_shared.dll',
    'onnxruntime_providers_cuda.dll',
    'onnxruntime_providers_tensorrt.dll',
    'engines-ampere_plus.zip'
)

# --- Pre-flight: every asset exists and is non-empty -----------------------
Write-Host '============================================='
Write-Host " Pre-flight: dist\trt-pack-v2\ verification"
Write-Host '============================================='
$totalBytes = [int64]0
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
Write-Host ''
Write-Host ("  total: {0:N0} bytes ({1} GB)" -f $totalBytes, $totalGb)

if (-not (Test-Path $NotesFile)) {
    Write-Error "release notes file not found: $NotesFile"
    exit 1
}

# --- Pre-flight: gh CLI auth ------------------------------------------------
Write-Host ''
Write-Host '============================================='
Write-Host ' Pre-flight: gh CLI auth'
Write-Host '============================================='
gh auth status
if ($LASTEXITCODE -ne 0) {
    Write-Error 'gh CLI is not authenticated. Run `gh auth login` first.'
    exit 1
}

# --- Pre-flight: tag must not already exist ---------------------------------
# `gh release view` exits 1 and writes "release not found" to stderr when the
# release does not exist. In Windows PowerShell 5.1, redirecting a native
# command's stderr (`2>$null`) wraps each line as a NativeCommandError, which
# under `$ErrorActionPreference = 'Stop'` becomes a terminating error and
# aborts the script. Wrap the call in try/catch so "not found" is treated as
# the expected happy path for a fresh upload.
$existing = $null
try {
    $existing = gh release view $Tag --repo $Repo --json tagName 2>$null
}
catch {
    $existing = $null
}
if ($LASTEXITCODE -eq 0 -and $existing) {
    Write-Host ''
    Write-Warning "$Tag already exists. Aborting (would conflict)."
    Write-Warning "If this is a stale draft from a previous attempt, remove it:"
    Write-Warning "  gh release delete $Tag --repo $Repo --yes --cleanup-tag"
    exit 1
}

# --- Upload -----------------------------------------------------------------
Write-Host ''
Write-Host '============================================='
Write-Host " gh release create $Tag (draft)"
Write-Host '============================================='
Write-Host "uploading... ($totalGb GB, expect 5-15 minutes)"

# Pass full paths to gh; safer than relying on relative interpretation.
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

# --- Done -------------------------------------------------------------------
Write-Host ''
Write-Host '============================================='
Write-Host ' Done (DRAFT, not yet public)'
Write-Host '============================================='
gh release view $Tag --repo $Repo

Write-Host ''
Write-Host 'Inspect the draft in the Web UI:'
Write-Host "  https://github.com/$Repo/releases"
Write-Host ''
Write-Host 'Publish (CLI):'
Write-Host "  gh release edit $Tag --repo $Repo --draft=false --prerelease"
Write-Host ''
Write-Host 'Roll back / start over:'
Write-Host "  gh release delete $Tag --repo $Repo --yes --cleanup-tag"
