# Build and stage a fast, portable development runtime.
#
# Output:
#   target\dev-runtime\mimageviewer-core.exe
#   Native DLLs, workers, the optional VST3 bridge, and AI models are staged
#   beside it so the core can run without the release launcher.
#
# This script builds only the application core. It does not run the result.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$outputDir = Join-Path $repoRoot 'target\dev-runtime'
$coreExe = Join-Path $outputDir 'mimageviewer-core.exe'

function Ensure-LibclangPath {
    if ($env:LIBCLANG_PATH -and
        (Test-Path (Join-Path $env:LIBCLANG_PATH 'libclang.dll'))) {
        return
    }

    $candidates = @(
        'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin',
        'C:\Program Files (x86)\Microsoft Visual Studio\17\BuildTools\VC\Tools\Llvm\x64\bin',
        'C:\Program Files\LLVM\bin'
    )
    foreach ($dir in $candidates) {
        if (Test-Path (Join-Path $dir 'libclang.dll')) {
            $env:LIBCLANG_PATH = $dir
            Write-Host ("[build-dev] using LIBCLANG_PATH={0}" -f $dir)
            return
        }
    }
}

function Copy-IfChanged {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Source,
        [Parameter(Mandatory = $true)]
        [string] $Destination
    )

    if (-not (Test-Path $Source -PathType Leaf)) {
        throw "[build-dev] required file is missing: $Source"
    }

    $destinationDir = Split-Path -Parent $Destination
    if (-not (Test-Path $destinationDir -PathType Container)) {
        New-Item -ItemType Directory -Path $destinationDir | Out-Null
    }

    $sourceInfo = Get-Item -LiteralPath $Source
    $needsCopy = -not (Test-Path $Destination -PathType Leaf)
    if (-not $needsCopy) {
        $destinationInfo = Get-Item -LiteralPath $Destination
        $needsCopy =
            $sourceInfo.Length -ne $destinationInfo.Length -or
            $sourceInfo.LastWriteTimeUtc -ne $destinationInfo.LastWriteTimeUtc
    }

    if ($needsCopy) {
        Copy-Item -LiteralPath $Source -Destination $Destination -Force
        Write-Host ("[build-dev] staged {0}" -f
            (Resolve-Path -Relative $Destination))
    }
}

Push-Location $repoRoot
try {
    # Stop only the development-profile core when it locks the output file.
    if (Test-Path $coreExe -PathType Leaf) {
        $corePath = [System.IO.Path]::GetFullPath($coreExe)
        Get-Process -Name 'mimageviewer-core' -ErrorAction SilentlyContinue |
            ForEach-Object {
                $processPath = $null
                try { $processPath = $_.Path } catch { $processPath = $null }
                if ($processPath -and
                    [System.IO.Path]::GetFullPath($processPath) -eq $corePath) {
                    Write-Host ("[build-dev] stopping development core (PID={0})" -f $_.Id)
                    Stop-Process -Id $_.Id -Force -ErrorAction Stop
                }
            }
    }

    Ensure-LibclangPath
    Write-Host '[build-dev] building portable core with profile dev-runtime'
    & cargo build --profile dev-runtime --bin mimageviewer-core --features portable
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    if (-not (Test-Path $coreExe -PathType Leaf)) {
        throw "[build-dev] core executable was not produced: $coreExe"
    }

    $copies = @(
        @{ src = 'vendor\ffmpeg\bin\avcodec-61.dll'; dst = 'avcodec-61.dll' }
        @{ src = 'vendor\ffmpeg\bin\avformat-61.dll'; dst = 'avformat-61.dll' }
        @{ src = 'vendor\ffmpeg\bin\avutil-59.dll'; dst = 'avutil-59.dll' }
        @{ src = 'vendor\ffmpeg\bin\avfilter-10.dll'; dst = 'avfilter-10.dll' }
        @{ src = 'vendor\ffmpeg\bin\swscale-8.dll'; dst = 'swscale-8.dll' }
        @{ src = 'vendor\ffmpeg\bin\swresample-5.dll'; dst = 'swresample-5.dll' }
        @{ src = 'vendor\pdfium\bin\pdfium.dll'; dst = 'pdfium.dll' }
        @{ src = 'vendor\ort\onnxruntime.dll'; dst = 'onnxruntime.dll' }
        @{
            src = 'vendor\ort\onnxruntime_providers_shared.dll'
            dst = 'onnxruntime_providers_shared.dll'
        }
        @{
            src = 'vendor\susie-worker\mimageviewer-susie32.exe'
            dst = 'mimageviewer-susie32.exe'
        }
    )

    $models = @(
        'realesrgan_x4plus.onnx',
        'realesrgan_x4plus_anime_6b.onnx',
        'realesr_general_x4v3.onnx',
        'realcugan_4x_conservative.onnx',
        '4x_NMKD-Siax_200k.onnx',
        'dejpg_realplksr_otf.onnx',
        'migan.onnx'
    )
    foreach ($model in $models) {
        $copies += @{
            src = "vendor\models\$model"
            dst = "models\$model"
        }
    }

    foreach ($copy in $copies) {
        Copy-IfChanged `
            -Source (Join-Path $repoRoot $copy.src) `
            -Destination (Join-Path $outputDir $copy.dst)
    }

    # The portable distribution omits this unsigned helper, but a local
    # development runtime may use the already-built bridge when available.
    $vstBridge = Join-Path $repoRoot 'vendor\vst3-host\mimageviewer-vst3-host.exe'
    if (Test-Path $vstBridge -PathType Leaf) {
        Copy-IfChanged `
            -Source $vstBridge `
            -Destination (Join-Path $outputDir 'mimageviewer-vst3-host.exe')
    } else {
        Write-Warning '[build-dev] VST3 bridge is absent; VST3 will be unavailable'
    }

    Write-Host ''
    Write-Host '[build-dev] DONE'
    Write-Host ("  core: {0}" -f $coreExe)
    Write-Host ("  data: {0}" -f (Join-Path $outputDir 'data'))
} finally {
    Pop-Location
}
