# Build and stage a fast development runtime with the normal application
# feature set and data profile.
#
# Output:
#   target\dev-runtime\mimageviewer-core.exe
#   target\dev-runtime\mimageviewer-remote.exe
#   target\dev-runtime\mimageviewer-epub-pdf.exe
#   target\dev-runtime\mimageviewer-mcp.exe
#   target\dev-runtime\mimageviewer-musicvideo.exe
#   FFmpeg DLLs are staged beside it so the core can run without the release
#   launcher. Other native assets and models use the same embedded
#   extraction path as the regular application.
#
# The core, remote service, EPUB converter, MCP adapter, and music-video editor are built together: the core
# spawns each companion from its own directory. Core and remote share PROTOCOL_VERSION.
#
# The dev-runtime Cargo profile only changes optimization/build-time settings.
# The portable feature is intentionally NOT enabled, so an ordinary launch uses
# %APPDATA%\mimageviewer just like the installed/release application.
#
# This script builds the application core and its companions. It does not run them or
# touch the normal application data.
#
# When another worktree is building native code, this waits for it rather than
# racing it into an MSB3191 failure. Pass -WaitForOtherBuildsMinutes 0 to skip.
#
# Usage:
#   .\scripts\build-dev.ps1
#   .\scripts\build-dev.ps1 -TestScript
#   .\scripts\build-dev.ps1 -PreserveRuntime
#   .\scripts\build-dev.ps1 -WaitForOtherBuildsMinutes 0

[CmdletBinding()]
param(
    # Trial (shisaku): stage a complete mIV Sound bundle only in dev-runtime.
    # Kept under the trial's original argument name; old Mixwright bundles
    # cannot provide mIV Sound's isolated storage contract.
    [string] $EffetuneBundle,
    [switch] $TestScript,
    [string] $MivSoundCandidate,
    # Fail when an exact staged development executable is running instead of
    # stopping it. This keeps a live verification runtime intact.
    [switch] $PreserveRuntime,
    # Wait this long for another worktree's native build to finish before
    # starting. 0 disables the wait. See Wait-ForOtherNativeBuilds.
    [int] $WaitForOtherBuildsMinutes = 30
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'build-output-safety.ps1')
Assert-MivFixedCargoOutput -WorkingDirectory (Join-Path $PSScriptRoot '..')
$env:MSBUILDDISABLENODEREUSE = '1'
. (Join-Path $PSScriptRoot 'miv-sound-candidate.ps1')
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$targetDir = Join-Path $repoRoot 'target'
$outputDir = Join-Path $targetDir 'dev-runtime'
$coreExe = Join-Path $outputDir 'mimageviewer-core.exe'
$remoteExe = Join-Path $outputDir 'mimageviewer-remote.exe'
$epubWorkerExe = Join-Path $outputDir 'mimageviewer-epub-pdf.exe'
$mcpExe = Join-Path $outputDir 'mimageviewer-mcp.exe'
$musicvideoExe = Join-Path $outputDir 'mimageviewer-musicvideo.exe'
. (Join-Path $PSScriptRoot 'musicvideo-build-safety.ps1')
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
        [string] $Label,
        [switch] $PreserveRuntime
    )

    if (-not (Test-Path $ExePath -PathType Leaf)) { return }
    $fullPath = [System.IO.Path]::GetFullPath($ExePath)
    Get-Process -Name $ExeName -ErrorAction SilentlyContinue |
        ForEach-Object {
            $processPath = $null
            try { $processPath = $_.Path } catch { $processPath = $null }
            if ($processPath -and
                [System.IO.Path]::GetFullPath($processPath) -eq $fullPath) {
                if ($PreserveRuntime) {
                    throw ("[build-dev] development {0} is running (PID={1}): {2}. -PreserveRuntime refuses to stop it; close it and retry." -f
                        $Label, $_.Id, $fullPath)
                }
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

function Copy-EffetuneBundleIfChanged {
    if ($EffetuneBundle) {
        if ($MivSoundCandidate) {
            throw '[build-dev] -EffetuneBundle and -MivSoundCandidate are mutually exclusive'
        }
        $source = (Resolve-Path -LiteralPath $EffetuneBundle).Path
        $destination = Join-Path $outputDir 'effetune'
        $source = [IO.Path]::GetFullPath($source).TrimEnd('\', '/')
        $destination = [IO.Path]::GetFullPath($destination).TrimEnd('\', '/')
        $separator = [IO.Path]::DirectorySeparatorChar
        if ($source.Equals($destination, [StringComparison]::OrdinalIgnoreCase) -or
            $source.StartsWith($destination + $separator, [StringComparison]::OrdinalIgnoreCase) -or
            $destination.StartsWith($source + $separator, [StringComparison]::OrdinalIgnoreCase)) {
            throw '[build-dev] EffeTune source and destination overlap'
        }
        Assert-MivSoundSafeDirectory $source
        Assert-MivSoundSafeDirectory $destination
        $plugin = Join-Path $source ('Contents/x86_64-win/' + $MivSound.BUNDLE_NAME)
        if ((Split-Path $source -Leaf) -cne $MivSound.BUNDLE_NAME -or
            -not (Test-MivPeFile $plugin)) {
            throw '[build-dev] Trial override requires a complete mIV Sound.vst3 bundle; old EffeTune Mixwright trials must be ported to mIV Sound first'
        }
        if (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Recurse -Force }
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        Copy-Item -LiteralPath $source -Destination $destination -Recurse -Force
        return
    }
    if ($MivSoundCandidate) {
        Copy-MivSoundCandidateRuntime -RepoRoot $repoRoot -Candidate $MivSoundCandidate -OutputDir $outputDir
        return
    }
    $source = Join-Path $repoRoot $MivSound.SOURCE_ROOT
    $null = Assert-MivEffetuneSource -SourceRoot $source -NoticesRoot (Join-Path $repoRoot $MivSound.NOTICES_ROOT)
    $destination = Join-Path $outputDir 'effetune'
    Assert-MivSoundSafeDirectory $destination
    if (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Recurse -Force }
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Get-ChildItem -LiteralPath $source -Force | Copy-Item -Destination $destination -Recurse -Force
    Assert-MivEffetuneRawSource -SourceRoot $destination -ManifestPath (Join-Path $repoRoot $MivSound.Manifest)
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

$oldCandidate = $env:MIV_SOUND_CANDIDATE
Push-Location $repoRoot
try {
    # Reject before build/process handling, as well as at the staging boundary.
    if ($EffetuneBundle -and $MivSoundCandidate) {
        throw '[build-dev] -EffetuneBundle and -MivSoundCandidate are mutually exclusive'
    }
    if ($MivSoundCandidate) {
        $null = Read-MivSoundCandidate -RepoRoot $repoRoot -Candidate $MivSoundCandidate
        $env:MIV_SOUND_CANDIDATE = $MivSoundCandidate
    } else {
        Remove-Item Env:MIV_SOUND_CANDIDATE -ErrorAction SilentlyContinue
    }
    & (Join-Path $repoRoot 'scripts\check-vcrt-pe-dependencies.ps1')

    if ($WaitForOtherBuildsMinutes -gt 0) {
        Wait-ForOtherNativeBuilds
    }

    Assert-MivMusicvideoBuildOutputsIdle -OutputPaths @($musicvideoExe)

    # Stop only the development-profile viewer companions when they lock the output.
    Stop-StagedProcess -ExeName 'mimageviewer-core' -ExePath $coreExe -Label 'core' `
        -PreserveRuntime:$PreserveRuntime
    Stop-StagedProcess -ExeName 'mimageviewer-remote' -ExePath $remoteExe -Label 'remote service' `
        -PreserveRuntime:$PreserveRuntime
    Stop-StagedProcess -ExeName 'mimageviewer-epub-pdf' -ExePath $epubWorkerExe -Label 'EPUB converter' `
        -PreserveRuntime:$PreserveRuntime
    Stop-StagedProcess -ExeName 'mimageviewer-mcp' -ExePath $mcpExe -Label 'MCP adapter' `
        -PreserveRuntime:$PreserveRuntime

    Ensure-LibclangPath
    $featureArgs = @()
    $featureLabel = 'normal feature set'
    if ($TestScript) {
        $featureArgs = @('--features', 'test-script')
        $featureLabel = 'test-script feature'
    }
    Write-Host ('[build-dev] building normal-profile core with Cargo profile dev-runtime ({0})' -f
        $featureLabel)
    & cargo build --profile dev-runtime --target-dir $targetDir --bin mimageviewer-core @featureArgs
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
    & cargo build --profile dev-runtime --target-dir $targetDir -p mimageviewer-remote --bin mimageviewer-remote
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    if (-not (Test-Path $remoteExe -PathType Leaf)) {
        throw "[build-dev] remote service was not produced: $remoteExe"
    }

    Write-Host '[build-dev] building EPUB PDF worker with Cargo profile dev-runtime'
    & cargo build --profile dev-runtime --target-dir $targetDir -p epub-pdf-worker --bin mimageviewer-epub-pdf
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    if (-not (Test-Path $epubWorkerExe -PathType Leaf)) {
        throw "[build-dev] EPUB PDF worker was not produced: $epubWorkerExe"
    }

    Write-Host '[build-dev] building MCP adapter with Cargo profile dev-runtime'
    & cargo build --profile dev-runtime --target-dir $targetDir -p mimageviewer-mcp --bin mimageviewer-mcp
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if (-not (Test-Path $mcpExe -PathType Leaf)) {
        throw "[build-dev] MCP adapter was not produced: $mcpExe"
    }

    Write-Host '[build-dev] building music-video editor with Cargo profile dev-runtime'
    & cargo build --profile dev-runtime --target-dir $targetDir -p musicvideo-editor --bin mimageviewer-musicvideo
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if (-not (Test-Path $musicvideoExe -PathType Leaf)) {
        throw "[build-dev] music-video editor was not produced: $musicvideoExe"
    }

    $copies = @(
        @{ src = 'vendor\ffmpeg\bin\avcodec-61.dll'; dst = 'avcodec-61.dll' }
        @{ src = 'vendor\ffmpeg\bin\avformat-61.dll'; dst = 'avformat-61.dll' }
        @{ src = 'vendor\ffmpeg\bin\avutil-59.dll'; dst = 'avutil-59.dll' }
        @{ src = 'vendor\ffmpeg\bin\avfilter-10.dll'; dst = 'avfilter-10.dll' }
        @{ src = 'vendor\ffmpeg\bin\swscale-8.dll'; dst = 'swscale-8.dll' }
        @{ src = 'vendor\ffmpeg\bin\swresample-5.dll'; dst = 'swresample-5.dll' }
        @{ src = 'vendor\vcrt\msvcp140.dll'; dst = 'msvcp140.dll' }
        @{ src = 'vendor\vcrt\msvcp140_1.dll'; dst = 'msvcp140_1.dll' }
        @{ src = 'vendor\vcrt\vcruntime140.dll'; dst = 'vcruntime140.dll' }
        @{ src = 'vendor\vcrt\vcruntime140_1.dll'; dst = 'vcruntime140_1.dll' }
    )

    foreach ($copy in $copies) {
        Copy-IfChanged `
            -Source (Join-Path $repoRoot $copy.src) `
            -Destination (Join-Path $outputDir $copy.dst)
    }

    Copy-EffetuneBundleIfChanged

    & (Join-Path $repoRoot 'scripts\check-vcrt-pe-dependencies.ps1') `
        -InputPaths @($coreExe, $remoteExe, $epubWorkerExe, $mcpExe, $musicvideoExe) -RequireCompanionRuntime `
        -ReportPath 'target\vcrt-pe-reports\dev-runtime.json'

    Write-Host ''
    Write-Host '[build-dev] DONE'
    Write-Host ("  core: {0}" -f $coreExe)
    Write-Host ("  remote service: {0}" -f $remoteExe)
    Write-Host ("  EPUB PDF worker: {0}" -f $epubWorkerExe)
    Write-Host ("  MCP adapter: {0}" -f $mcpExe)
    Write-Host ("  music-video editor: {0}" -f $musicvideoExe)
    Write-Host ("  data (default): {0}" -f $normalDataDir)
    Write-Host ("  isolated override: --data-dir `"{0}`"" -f
        (Join-Path $outputDir 'data'))
    Write-Host '  default launch: close the installed/resident mImageViewer first (same data-dir namespace)'
    Write-Host '  isolated launch: may run beside the installed/resident mImageViewer'
} finally {
    $env:MIV_SOUND_CANDIDATE = $oldCandidate
    Pop-Location
}
