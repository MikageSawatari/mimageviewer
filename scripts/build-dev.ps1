# Build and stage a fast development runtime with the normal application
# feature set and data profile.
#
# Output:
#   target\dev-runtime\mimageviewer-core.exe
#   target\dev-runtime\mimageviewer-remote.exe
#   FFmpeg DLLs are staged beside it so the core can run without the release
#   launcher. Other native assets, workers, and models use the same embedded
#   extraction path as the regular application.
#
# Both executables are built together on purpose: the core spawns the remote
# service from its own directory, and the two share PROTOCOL_VERSION.
#
# The dev-runtime Cargo profile only changes optimization/build-time settings.
# The portable feature is intentionally NOT enabled, so an ordinary launch uses
# %APPDATA%\mimageviewer just like the installed/release application.
#
# This script builds only the application core. It does not run the result or
# touch the normal application data.
#
# When another worktree is building native code, this waits for it rather than
# racing it into an MSB3191 failure. Pass -WaitForOtherBuildsMinutes 0 to skip.
#
# Usage:
#   .\scripts\build-dev.ps1
#   .\scripts\build-dev.ps1 -TestScript
#   .\scripts\build-dev.ps1 -WaitForOtherBuildsMinutes 0

[CmdletBinding()]
param(
    [switch] $TestScript,
    # Wait this long for another worktree's native build to finish before
    # starting. 0 disables the wait. See Wait-ForOtherNativeBuilds.
    [int] $WaitForOtherBuildsMinutes = 30
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$outputDir = Join-Path $repoRoot 'target\dev-runtime'
$coreExe = Join-Path $outputDir 'mimageviewer-core.exe'
$remoteExe = Join-Path $outputDir 'mimageviewer-remote.exe'
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

function Stop-StagedProcess {
    param(
        [Parameter(Mandatory = $true)]
        [string] $ExeName,
        [Parameter(Mandatory = $true)]
        [string] $ExePath,
        [Parameter(Mandatory = $true)]
        [string] $Label
    )

    if (-not (Test-Path $ExePath -PathType Leaf)) { return }
    $fullPath = [System.IO.Path]::GetFullPath($ExePath)
    Get-Process -Name $ExeName -ErrorAction SilentlyContinue |
        ForEach-Object {
            $processPath = $null
            try { $processPath = $_.Path } catch { $processPath = $null }
            if ($processPath -and
                [System.IO.Path]::GetFullPath($processPath) -eq $fullPath) {
                Write-Host ("[build-dev] stopping development {0} (PID={1})" -f $Label, $_.Id)
                Stop-Process -Id $_.Id -Force -ErrorAction Stop
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

function Wait-ForOtherNativeBuilds {
    # turbojpeg-sys drives cmake/MSBuild from the shared cargo registry copy of
    # libjpeg-turbo. Two worktrees building it at once fail with MSB3191
    # ("cannot create directory ... access is denied") on the .tlog directories,
    # even though each worktree has its own target dir. Waiting is cheaper than
    # the failed build: the C library is only rebuilt once per worktree.
    #
    # CMAKE_BUILD_PARALLEL_LEVEL does not help -- the cmake crate passes its own
    # --parallel, so lowering it is ignored.
    $waited = $false
    $deadline = (Get-Date).AddMinutes($WaitForOtherBuildsMinutes)
    while (@(Get-Process MSBuild -ErrorAction SilentlyContinue).Count -gt 0) {
        if ((Get-Date) -gt $deadline) {
            Write-Host ("[build-dev] still waiting after {0} min; building anyway" -f
                $WaitForOtherBuildsMinutes)
            return
        }
        if (-not $waited) {
            Write-Host '[build-dev] another worktree is building native code; waiting for it'
            $waited = $true
        }
        Start-Sleep -Seconds 20
    }
    if ($waited) {
        Write-Host '[build-dev] other build finished; starting'
    }
}

Push-Location $repoRoot
try {
    if ($WaitForOtherBuildsMinutes -gt 0) {
        Wait-ForOtherNativeBuilds
    }

    # Stop only the development-profile executables when they lock the output.
    Stop-StagedProcess -ExeName 'mimageviewer-core' -ExePath $coreExe -Label 'core'
    Stop-StagedProcess -ExeName 'mimageviewer-remote' -ExePath $remoteExe -Label 'remote service'

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

    # The core spawns the remote service from its own directory, and both sides
    # carry the same PROTOCOL_VERSION. Building only the core leaves a stale
    # service beside it, which fails the handshake instead of running.
    Write-Host '[build-dev] building remote service with Cargo profile dev-runtime'
    & cargo build --profile dev-runtime -p mimageviewer-remote --bin mimageviewer-remote
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    if (-not (Test-Path $remoteExe -PathType Leaf)) {
        throw "[build-dev] remote service was not produced: $remoteExe"
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
    Write-Host ("  remote service: {0}" -f $remoteExe)
    Write-Host ("  data (default): {0}" -f $normalDataDir)
    Write-Host ("  isolated override: --data-dir `"{0}`"" -f
        (Join-Path $outputDir 'data'))
    Write-Host '  default launch: close the installed/resident mImageViewer first (same data-dir namespace)'
    Write-Host '  isolated launch: may run beside the installed/resident mImageViewer'
} finally {
    Pop-Location
}
