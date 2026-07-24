# TensorRT runtime DLL setup script (PoC, PowerShell)
#
# Usage (PowerShell):
#   cd C:\home\mimageviewer\.claude\worktrees\dazzling-mcclintock-d0be64
#   .\scripts\setup-tensorrt-pack.ps1
#   or
#   powershell -ExecutionPolicy Bypass -File scripts\setup-tensorrt-pack.ps1
#
# Action:
#   1. Download from public NuGet / NVIDIA redist URLs into vendor\tensorrt-cache\
#      (skipped if size matches on re-run)
#      - Microsoft.ML.OnnxRuntime.Gpu (~150 MB)
#      - CUDA cudart + cublas (~400 MB)
#      - cuDNN (~646 MB)
#      - TensorRT (~2.24 GB)
#   2. Extract required DLLs into %APPDATA%\mimageviewer\tensorrt\
#   3. Write INSTALL_OK sentinel JSON last (atomic install marker)
#
# Requirements:
#   - Windows 10/11, PowerShell 5.1+
#   - ~3 GB download + ~1.5 GB extracted
#   - 100 Mbps: ~5 min, 20 Mbps: ~25 min

$ErrorActionPreference = 'Stop'

# Pinned versions (ort 2.0.0-rc.12 -> ORT 1.24.2 / CUDA 12.x / cuDNN 9.x / TRT 10.x)
$ORT_GPU_VERSION = '1.24.2'
$CUDA_CUDART_VERSION = '12.9.79'
$CUDA_CUBLAS_VERSION = '12.9.1.4'
$CUDA_NVRTC_VERSION = '12.9.86'
$CUDA_CUFFT_VERSION = '11.4.1.4'
$CUDA_CURAND_VERSION = '10.3.10.19'
$CUDA_CUSOLVER_VERSION = '11.7.5.82'
$CUDA_CUSPARSE_VERSION = '12.5.10.65'
$CUDA_NVJITLINK_VERSION = '12.9.86'
$CUDNN_VERSION = '9.21.1.3'
$CUDNN_CUDA_TAG = 'cuda12'
$TRT_VERSION = '10.16.1.11'
$TRT_MAJOR_MINOR = '10.16.1'
$TRT_CUDA_TAG = 'cuda-12.9'

# Directories
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent $ScriptDir
$CacheDir = Join-Path $RepoRoot 'vendor\tensorrt-cache'
$TargetDir = Join-Path $env:APPDATA 'mimageviewer\tensorrt'

New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null
New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null

# Speed up Invoke-WebRequest by suppressing the progress bar (huge perf hit on PS 5.1)
$ProgressPreference = 'SilentlyContinue'

# Helper: download to Dst if missing or smaller than MinBytes
function Download-IfMissing {
    param(
        [string]$Url,
        [string]$Dst,
        [int64]$MinBytes
    )
    if (Test-Path $Dst) {
        $size = (Get-Item $Dst).Length
        if ($size -ge $MinBytes) {
            $name = Split-Path -Leaf $Dst
            Write-Host "  cached $name $size bytes"
            return
        }
        Write-Host "  partial cache, redownloading"
        Remove-Item $Dst -Force
    }

    Write-Host "  downloading $Url"
    $startTime = Get-Date
    Invoke-WebRequest -Uri $Url -OutFile $Dst -UseBasicParsing
    $elapsed = ((Get-Date) - $startTime).TotalSeconds
    $size = (Get-Item $Dst).Length
    $mb = [Math]::Round($size / 1MB, 1)
    if ($elapsed -gt 0) {
        $speed = [Math]::Round($mb / $elapsed, 1)
    } else {
        $speed = 0
    }
    Write-Host "    -> $mb MB in $elapsed s at $speed MB/s"
}

# Helper: extract files matching basename patterns from a zip into DstDir (flat)
function Extract-Files {
    param(
        [string]$Zip,
        [string[]]$Patterns,
        [string]$DstDir
    )
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($Zip)
    try {
        $extracted = 0
        foreach ($entry in $archive.Entries) {
            if ($entry.Name -eq '') { continue }
            foreach ($pattern in $Patterns) {
                if ($entry.Name -like $pattern) {
                    $dstPath = Join-Path $DstDir $entry.Name
                    [System.IO.Compression.ZipFileExtensions]::ExtractToFile(
                        $entry, $dstPath, $true
                    )
                    $extracted++
                    break
                }
            }
        }
        return $extracted
    }
    finally {
        $archive.Dispose()
    }
}

# Step 1: downloads
Write-Host '============================================='
Write-Host ' Step 1/3: Downloads'
Write-Host "   cache: $CacheDir"
Write-Host '============================================='

# Note: "Microsoft.ML.OnnxRuntime.Gpu" itself became a meta-package since 1.20+
# (only ~300 KB, no DLLs). The Windows binaries live in the .Windows variant.
$OrtGpuNupkg = Join-Path $CacheDir "onnxruntime-gpu-windows-$ORT_GPU_VERSION.nupkg"
$OrtGpuUrl = "https://globalcdn.nuget.org/packages/microsoft.ml.onnxruntime.gpu.windows.$ORT_GPU_VERSION.nupkg"
Download-IfMissing -Url $OrtGpuUrl -Dst $OrtGpuNupkg -MinBytes 100000000

# Clean up previous (wrong) meta-package cache if present, so disk doesn't accumulate
$WrongOldCache = Join-Path $CacheDir "onnxruntime-gpu-$ORT_GPU_VERSION.nupkg"
if (Test-Path $WrongOldCache) {
    Remove-Item $WrongOldCache -Force
}

$CudartZip = Join-Path $CacheDir "cuda_cudart-$CUDA_CUDART_VERSION.zip"
$CudartUrl = "https://developer.download.nvidia.com/compute/cuda/redist/cuda_cudart/windows-x86_64/cuda_cudart-windows-x86_64-$CUDA_CUDART_VERSION-archive.zip"
Download-IfMissing -Url $CudartUrl -Dst $CudartZip -MinBytes 5000000

$CublasZip = Join-Path $CacheDir "libcublas-$CUDA_CUBLAS_VERSION.zip"
$CublasUrl = "https://developer.download.nvidia.com/compute/cuda/redist/libcublas/windows-x86_64/libcublas-windows-x86_64-$CUDA_CUBLAS_VERSION-archive.zip"
Download-IfMissing -Url $CublasUrl -Dst $CublasZip -MinBytes 200000000

# Additional CUDA libraries that onnxruntime_providers_cuda.dll depends on
$NvrtcZip = Join-Path $CacheDir "cuda_nvrtc-$CUDA_NVRTC_VERSION.zip"
$NvrtcUrl = "https://developer.download.nvidia.com/compute/cuda/redist/cuda_nvrtc/windows-x86_64/cuda_nvrtc-windows-x86_64-$CUDA_NVRTC_VERSION-archive.zip"
Download-IfMissing -Url $NvrtcUrl -Dst $NvrtcZip -MinBytes 5000000

$CufftZip = Join-Path $CacheDir "libcufft-$CUDA_CUFFT_VERSION.zip"
$CufftUrl = "https://developer.download.nvidia.com/compute/cuda/redist/libcufft/windows-x86_64/libcufft-windows-x86_64-$CUDA_CUFFT_VERSION-archive.zip"
Download-IfMissing -Url $CufftUrl -Dst $CufftZip -MinBytes 5000000

$CurandZip = Join-Path $CacheDir "libcurand-$CUDA_CURAND_VERSION.zip"
$CurandUrl = "https://developer.download.nvidia.com/compute/cuda/redist/libcurand/windows-x86_64/libcurand-windows-x86_64-$CUDA_CURAND_VERSION-archive.zip"
Download-IfMissing -Url $CurandUrl -Dst $CurandZip -MinBytes 5000000

$CusolverZip = Join-Path $CacheDir "libcusolver-$CUDA_CUSOLVER_VERSION.zip"
$CusolverUrl = "https://developer.download.nvidia.com/compute/cuda/redist/libcusolver/windows-x86_64/libcusolver-windows-x86_64-$CUDA_CUSOLVER_VERSION-archive.zip"
Download-IfMissing -Url $CusolverUrl -Dst $CusolverZip -MinBytes 5000000

$CusparseZip = Join-Path $CacheDir "libcusparse-$CUDA_CUSPARSE_VERSION.zip"
$CusparseUrl = "https://developer.download.nvidia.com/compute/cuda/redist/libcusparse/windows-x86_64/libcusparse-windows-x86_64-$CUDA_CUSPARSE_VERSION-archive.zip"
Download-IfMissing -Url $CusparseUrl -Dst $CusparseZip -MinBytes 5000000

$NvjitlinkZip = Join-Path $CacheDir "libnvjitlink-$CUDA_NVJITLINK_VERSION.zip"
$NvjitlinkUrl = "https://developer.download.nvidia.com/compute/cuda/redist/libnvjitlink/windows-x86_64/libnvjitlink-windows-x86_64-$CUDA_NVJITLINK_VERSION-archive.zip"
Download-IfMissing -Url $NvjitlinkUrl -Dst $NvjitlinkZip -MinBytes 5000000

$CudnnZip = Join-Path $CacheDir "cudnn-$CUDNN_VERSION.zip"
$CudnnUrl = "https://developer.download.nvidia.com/compute/cudnn/redist/cudnn/windows-x86_64/cudnn-windows-x86_64-${CUDNN_VERSION}_${CUDNN_CUDA_TAG}-archive.zip"
Download-IfMissing -Url $CudnnUrl -Dst $CudnnZip -MinBytes 500000000

$TrtZip = Join-Path $CacheDir "TensorRT-$TRT_VERSION.zip"
$TrtUrl = "https://developer.download.nvidia.com/compute/machine-learning/tensorrt/$TRT_MAJOR_MINOR/zip/TensorRT-$TRT_VERSION.Windows.amd64.$TRT_CUDA_TAG.zip"
Download-IfMissing -Url $TrtUrl -Dst $TrtZip -MinBytes 2000000000

# Step 2: extract
Write-Host ''
Write-Host '============================================='
Write-Host ' Step 2/3: Extract DLLs'
Write-Host "   target: $TargetDir"
Write-Host '============================================='

# Wipe any old INSTALL_OK so partial extracts can be detected
$sentinelPath = Join-Path $TargetDir 'INSTALL_OK'
if (Test-Path $sentinelPath) {
    Remove-Item $sentinelPath -Force
}

Write-Host '  extracting ORT GPU NuGet'
$n = Extract-Files -Zip $OrtGpuNupkg -DstDir $TargetDir -Patterns @(
    'onnxruntime.dll',
    'onnxruntime_providers_cuda.dll',
    'onnxruntime_providers_tensorrt.dll',
    'onnxruntime_providers_shared.dll'
)
Write-Host "    -> $n DLLs"

Write-Host '  extracting CUDA cudart'
$n = Extract-Files -Zip $CudartZip -DstDir $TargetDir -Patterns @('cudart64_*.dll')
Write-Host "    -> $n DLLs"

Write-Host '  extracting CUDA cublas'
$n = Extract-Files -Zip $CublasZip -DstDir $TargetDir -Patterns @(
    'cublas64_*.dll',
    'cublasLt64_*.dll'
)
Write-Host "    -> $n DLLs"

Write-Host '  extracting CUDA nvrtc'
$n = Extract-Files -Zip $NvrtcZip -DstDir $TargetDir -Patterns @(
    'nvrtc64_*.dll',
    'nvrtc-builtins64_*.dll'
)
Write-Host "    -> $n DLLs"

Write-Host '  extracting CUDA cufft'
$n = Extract-Files -Zip $CufftZip -DstDir $TargetDir -Patterns @('cufft*64_*.dll')
Write-Host "    -> $n DLLs"

Write-Host '  extracting CUDA curand'
$n = Extract-Files -Zip $CurandZip -DstDir $TargetDir -Patterns @('curand64_*.dll')
Write-Host "    -> $n DLLs"

Write-Host '  extracting CUDA cusolver'
$n = Extract-Files -Zip $CusolverZip -DstDir $TargetDir -Patterns @('cusolver*64_*.dll')
Write-Host "    -> $n DLLs"

Write-Host '  extracting CUDA cusparse'
$n = Extract-Files -Zip $CusparseZip -DstDir $TargetDir -Patterns @('cusparse64_*.dll')
Write-Host "    -> $n DLLs"

Write-Host '  extracting CUDA nvJitLink'
$n = Extract-Files -Zip $NvjitlinkZip -DstDir $TargetDir -Patterns @('nvJitLink_*.dll')
Write-Host "    -> $n DLLs"

Write-Host '  extracting cuDNN'
$n = Extract-Files -Zip $CudnnZip -DstDir $TargetDir -Patterns @('cudnn*.dll')
Write-Host "    -> $n DLLs"

Write-Host '  extracting TensorRT'
$n = Extract-Files -Zip $TrtZip -DstDir $TargetDir -Patterns @(
    'nvinfer_*.dll',
    'nvinfer*.dll',
    'nvonnxparser_*.dll'
)
Write-Host "    -> $n DLLs"

# Step 3: verify and write INSTALL_OK
Write-Host ''
Write-Host '============================================='
Write-Host ' Step 3/3: Verify and write INSTALL_OK'
Write-Host '============================================='

$requiredDlls = @(
    'onnxruntime.dll',
    'onnxruntime_providers_tensorrt.dll',
    'onnxruntime_providers_cuda.dll',
    'onnxruntime_providers_shared.dll'
)
$missing = @()
foreach ($dll in $requiredDlls) {
    $path = Join-Path $TargetDir $dll
    if (-not (Test-Path $path)) {
        $missing += $dll
    }
}

$nvinferCount = (Get-ChildItem -Path $TargetDir -Filter 'nvinfer_*.dll' -ErrorAction SilentlyContinue).Count
$cudartCount = (Get-ChildItem -Path $TargetDir -Filter 'cudart64_*.dll' -ErrorAction SilentlyContinue).Count
$cudnnCount = (Get-ChildItem -Path $TargetDir -Filter 'cudnn*.dll' -ErrorAction SilentlyContinue).Count

if ($missing.Count -gt 0) {
    Write-Error ("Missing required DLLs: " + ($missing -join ', '))
    exit 1
}
if ($nvinferCount -eq 0) {
    Write-Error 'No nvinfer_*.dll extracted (check TRT zip layout)'
    exit 1
}
if ($cudartCount -eq 0) {
    Write-Error 'No cudart64_*.dll extracted'
    exit 1
}
if ($cudnnCount -eq 0) {
    Write-Error 'No cudnn*.dll extracted'
    exit 1
}

$installedAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$json = @"
{
  "version": 1,
  "ort_gpu_version": "$ORT_GPU_VERSION",
  "cuda_cudart_version": "$CUDA_CUDART_VERSION",
  "cuda_cublas_version": "$CUDA_CUBLAS_VERSION",
  "cudnn_version": "$CUDNN_VERSION",
  "trt_version": "$TRT_VERSION",
  "installed_at": "$installedAt"
}
"@
Set-Content -Path $sentinelPath -Value $json -Encoding utf8

$dllCount = (Get-ChildItem -Path $TargetDir -Filter '*.dll').Count
$totalBytes = (Get-ChildItem -Path $TargetDir -Filter '*.dll' | Measure-Object -Property Length -Sum).Sum
$totalMb = [Math]::Round($totalBytes / 1MB, 1)

Write-Host ''
Write-Host 'Setup complete'
Write-Host "  Target: $TargetDir"
Write-Host "  DLLs:   $dllCount files, total $totalMb MB"
Write-Host '  Sentinel: INSTALL_OK'
Write-Host ''
Write-Host 'Next step: cargo run --release --features dev-tools --bin bench_ai -- --backend tensorrt'
