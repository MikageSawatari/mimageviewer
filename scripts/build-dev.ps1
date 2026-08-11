# Build and stage a fast development runtime with the normal application
# feature set and data profile.
#
# Output:
#   target\dev-runtime\mimageviewer-core.exe
#   FFmpeg DLLs are staged beside it so the core can run without the release
#   launcher. Other native assets, workers, and models use the same embedded
#   extraction path as the regular application.
#
# The dev-runtime Cargo profile only changes optimization/build-time settings.
# The portable feature is intentionally NOT enabled, so an ordinary launch uses
# %APPDATA%\mimageviewer just like the installed/release application.
#
# This script builds only the application core. It does not run the result or
# touch the normal application data.
#
# Usage:
#   .\scripts\build-dev.ps1
#   .\scripts\build-dev.ps1 -TestScript

[CmdletBinding()]
param(
    [switch] $TestScript
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$outputDir = Join-Path $repoRoot 'target\dev-runtime'
$coreExe = Join-Path $outputDir 'mimageviewer-core.exe'
$normalDataDir = if ($env:APPDATA) {
    Join-Path $env:APPDATA 'mimageviewer'
} else {
    Join-Path $repoRoot 'mimageviewer'
}

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
    $featureArgs = @()
    $featureLabel = 'normal feature set'
    if ($TestScript) {
        $featureArgs = @('--features', 'test-script')
        $featureLabel = 'test-script feature'
    }
    Write-Host ('[build-dev] building normal-profile core with Cargo profile dev-runtime ({0})' -f
        $featureLabel)
    & cargo build --profile dev-runtime --bin mimageviewer-core @featureArgs
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
    )

    foreach ($copy in $copies) {
        Copy-IfChanged `
            -Source (Join-Path $repoRoot $copy.src) `
            -Destination (Join-Path $outputDir $copy.dst)
    }

    Write-Host ''
    Write-Host '[build-dev] DONE'
    Write-Host ("  core: {0}" -f $coreExe)
    Write-Host ("  data (default): {0}" -f $normalDataDir)
    Write-Host ("  isolated override: --data-dir `"{0}`"" -f
        (Join-Path $outputDir 'data'))
    Write-Host '  launch note: close the installed/resident mImageViewer first (shared single-instance mutex)'
} finally {
    Pop-Location
}
