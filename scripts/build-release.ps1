# mimageviewer release build wrapper (PowerShell)
#
# When mimageviewer.exe is running (e.g. tray-resident), cargo cannot overwrite
# target\release\mimageviewer.exe at link time, failing with LNK1104.
# This script:
#   1. Stops mimageviewer-* processes started from this repo or extracted to APPDATA
#   2. Polls for file-handle release (up to 10 seconds)
#   3. Rebuilds the VST3 C++ bridge before core embeds it
#   4. Builds release core + launcher (extra cargo args are passed through)
#   5. Clears the extracted VST3 bridge cache so next launch re-extracts it
#
# Usage:
#   PS> scripts\build-release.ps1
#   PS> scripts\build-release.ps1 --features foo

[CmdletBinding()]
param(
    [switch] $SkipVst3Bridge,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $CargoArgs
)

$ErrorActionPreference = 'Stop'

function Ensure-LibclangPath {
    if ($env:LIBCLANG_PATH) {
        $configured = Join-Path -Path $env:LIBCLANG_PATH -ChildPath 'libclang.dll'
        if (Test-Path $configured) {
            return
        }
    }

    $candidateDirs = @(
        'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin',
        'C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\bin',
        'C:\Program Files (x86)\Microsoft Visual Studio\17\BuildTools\VC\Tools\Llvm\x64\bin',
        'C:\Program Files (x86)\Microsoft Visual Studio\17\BuildTools\VC\Tools\Llvm\bin',
        'C:\Program Files\LLVM\bin'
    )
    foreach ($dir in $candidateDirs) {
        if (Test-Path (Join-Path -Path $dir -ChildPath 'libclang.dll')) {
            $env:LIBCLANG_PATH = $dir
            Write-Host ("[build-release] using LIBCLANG_PATH={0}" -f $dir)
            return
        }
    }

    $vsRoots = @(
        'C:\Program Files (x86)\Microsoft Visual Studio',
        'C:\Program Files\Microsoft Visual Studio'
    )
    foreach ($root in $vsRoots) {
        if (-not (Test-Path $root)) { continue }
        $found = Get-ChildItem -Path $root -Recurse -Filter libclang.dll -ErrorAction SilentlyContinue |
            Sort-Object @{ Expression = { if ($_.FullName -like '*\x64\bin\libclang.dll') { 0 } else { 1 } } }, FullName |
            Select-Object -First 1
        if ($found) {
            $env:LIBCLANG_PATH = $found.DirectoryName
            Write-Host ("[build-release] using LIBCLANG_PATH={0}" -f $found.DirectoryName)
            return
        }
    }
}

$repoRoot = (Get-Location).Path
# Append a trailing separator for path-boundary scoping. Without this, sibling
# directories like `C:\home\mimageviewer-old` would also match (StartsWith on
# `C:\home\mimageviewer` is too permissive).
$repoRootPrefix = $repoRoot.TrimEnd('\') + '\'
$repoRootPrefixLower = $repoRootPrefix.ToLower()
$releaseExe = Join-Path -Path $repoRoot -ChildPath 'target\release\mimageviewer.exe'
$appDataRoot = Join-Path -Path $env:APPDATA -ChildPath 'mimageviewer'
$appDataRootPrefix = $appDataRoot.TrimEnd('\') + '\'
$appDataRootPrefixLower = $appDataRootPrefix.ToLower()
$appDataProcessNames = @(
    'mimageviewer-core',
    'mimageviewer-vst3-host',
    'mimageviewer-susie32'
)
$appDataVst3Bridge = Join-Path -Path $appDataRoot -ChildPath 'vst3\mimageviewer-vst3-host.exe'

function Ensure-VendorVst3BridgeFromCache {
    param(
        [Parameter(Mandatory = $true)]
        [string] $VendorExe,
        [string] $Reason = 'VST3 bridge rebuild skipped'
    )

    if (Test-Path $VendorExe) {
        return $true
    }
    if (-not (Test-Path $appDataVst3Bridge)) {
        return $false
    }

    $vendorDir = Split-Path -Parent $VendorExe
    if (-not (Test-Path $vendorDir)) {
        New-Item -ItemType Directory -Path $vendorDir | Out-Null
    }
    Copy-Item -LiteralPath $appDataVst3Bridge -Destination $VendorExe -Force
    Write-Warning ("[build-release] {0}; copied extracted bridge cache back to vendor: {1}" -f $Reason, $VendorExe)
    return $true
}

# Match all "mimageviewer*" prefix processes (Get-Process -Name does not accept
# wildcards, hence the Where-Object filter).
$candidates = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'mimageviewer*' })

$toKill = @()
foreach ($p in $candidates) {
    $path = $null
    try { $path = $p.Path } catch { $path = $null }
    $included = $false
    $pathLabel = '(path unknown)'
    if (-not $path) {
        $included = $true
    } else {
        $pathLabel = $path
        $pl = $path.ToLower()
        if ($pl.StartsWith($repoRootPrefixLower)) {
            $included = $true
        } elseif ($appDataProcessNames -contains $p.Name -and $pl.StartsWith($appDataRootPrefixLower)) {
            # The launcher extracts core and helper processes to APPDATA. During
            # local release testing they are children of the repo-built launcher,
            # so stop them together; otherwise stale bridge/core processes can
            # keep running while cargo successfully rebuilds the launcher.
            $included = $true
        }
    }
    if ($included) {
        # Bundle the candidate process with the path label captured *now* (inside the
        # try/catch), so later code does not have to re-access $p.Path which can throw
        # for elevated/protected processes (Codex P2).
        $toKill += [pscustomobject]@{ Process = $p; PathLabel = $pathLabel }
    }
}

if ($toKill.Count -eq 0) {
    Write-Host "[build-release] no running mimageviewer process found"
} else {
    $failedPids = @()
    foreach ($entry in $toKill) {
        $p = $entry.Process
        Write-Host ("[build-release] stopping {0} (PID={1}) {2}" -f $p.ProcessName, $p.Id, $entry.PathLabel)
        try {
            Stop-Process -Id $p.Id -Force -ErrorAction Stop
        } catch {
            $failedPids += $p.Id
            Write-Warning ("[build-release] Stop-Process failed for PID={0}: {1}" -f $p.Id, $_)
        }
    }
    # Backup with taskkill /PID ... ONLY for the specific PIDs we already filtered to
    # the repo. A global `taskkill /IM mimageviewer.exe /F` would kill installed/portable
    # mIV instances unrelated to this build (Codex P2). taskkill writes to stderr and
    # exits non-zero on miss, which under $ErrorActionPreference='Stop' becomes a
    # NativeCommandError, so fence the block with a local EAP=Continue.
    if ($failedPids.Count -gt 0) {
        $prevEAP = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            foreach ($id in $failedPids) {
                & taskkill /PID $id /F 2>$null | Out-Null
            }
        } finally {
            $ErrorActionPreference = $prevEAP
        }
    }
}

# Wait for the OS file handle to release. Stop-Process is synchronous but the
# kernel handle table can lag for a few hundred ms. Poll by trying an exclusive
# write open.
if (Test-Path $releaseExe) {
    $deadline = (Get-Date).AddSeconds(10)
    $unlocked = $false
    while ((Get-Date) -lt $deadline) {
        try {
            $fs = [System.IO.File]::Open($releaseExe, 'Open', 'ReadWrite', 'None')
            $fs.Close()
            $unlocked = $true
            break
        } catch {
            Start-Sleep -Milliseconds 200
        }
    }
    if (-not $unlocked) {
        Write-Warning ("[build-release] {0} is still locked after 10s." -f $releaseExe)
        $handleExe = Get-Command handle.exe -ErrorAction SilentlyContinue
        if ($handleExe) {
            Write-Warning "[build-release] handle.exe output:"
            & handle.exe -nobanner $releaseExe 2>$null
        } else {
            Write-Warning "[build-release] install Sysinternals handle.exe to identify the locker."
        }
        Write-Warning "[build-release] proceeding to cargo build anyway; link may still fail."
    }
}

# 2 段階ビルド (ランチャー方式):
#   1. core (本体、FFmpeg DLL に静的依存) を `mimageviewer-core.exe` として生成
#   2. launcher (FFmpeg 非依存、core + 5 DLL を include_bytes! で内包) を
#      `mimageviewer.exe` として生成。配布する単体 exe はこちら。
#
# VST3 bridge is built first because mimageviewer-core embeds it with
# include_bytes!. Cargo also cannot express the core -> launcher ordering, so
# the two Rust binaries are built explicitly after the bridge.

if (-not $SkipVst3Bridge) {
    $cmakeExe = Get-Command cmake -ErrorAction SilentlyContinue
    $vst3SdkLicense = Join-Path -Path $repoRoot -ChildPath 'vendor\vst3sdk\LICENSE.txt'
    $vst3SourceDir = Join-Path -Path $repoRoot -ChildPath 'crates\vst3-host'
    $vst3BuildDir = Join-Path -Path $vst3SourceDir -ChildPath 'build'
    $vst3VendorExe = Join-Path -Path $repoRoot -ChildPath 'vendor\vst3-host\mimageviewer-vst3-host.exe'

    if (-not $cmakeExe) {
        throw "[build-release] cmake was not found. Install CMake or pass -SkipVst3Bridge to reuse the existing vendor bridge."
    }
    if (-not (Test-Path $vst3SdkLicense)) {
        if (Ensure-VendorVst3BridgeFromCache -VendorExe $vst3VendorExe -Reason 'VST3 SDK was not found at vendor\vst3sdk') {
            Write-Warning "[build-release] reusing existing VST3 bridge. Run scripts\setup-vst3-sdk.sh to rebuild bridge changes."
            $SkipVst3Bridge = $true
        } else {
            throw "[build-release] VST3 SDK was not found at vendor\vst3sdk, and no reusable bridge exe was found in vendor\vst3-host or APPDATA. Run scripts\setup-vst3-sdk.sh, or restore an existing vendor bridge and pass -SkipVst3Bridge."
        }
    }
    if (-not $SkipVst3Bridge -and -not (Test-Path (Join-Path -Path $vst3BuildDir -ChildPath 'CMakeCache.txt'))) {
        Write-Host "[build-release] configuring VST3 bridge (cmake)"
        & cmake -S $vst3SourceDir -B $vst3BuildDir -G "Visual Studio 18 2026" -A x64
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

    if (-not $SkipVst3Bridge) {
        Write-Host "[build-release] (1/3) cmake --build crates/vst3-host/build --config Release"
        & cmake --build $vst3BuildDir --config Release
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        if (-not (Test-Path $vst3VendorExe)) {
            throw "[build-release] VST3 bridge build did not produce $vst3VendorExe"
        }
    }
} else {
    $vst3VendorExe = Join-Path -Path $repoRoot -ChildPath 'vendor\vst3-host\mimageviewer-vst3-host.exe'
    if (-not (Ensure-VendorVst3BridgeFromCache -VendorExe $vst3VendorExe -Reason 'VST3 bridge rebuild skipped')) {
        throw "[build-release] -SkipVst3Bridge was specified, but no reusable bridge exe was found in vendor\vst3-host or APPDATA."
    }
    Write-Warning "[build-release] skipping VST3 bridge rebuild; core will embed the existing vendor/vst3-host exe."
}

Ensure-LibclangPath

$coreCmd = @('build', '--release', '--bin', 'mimageviewer-core')
if ($CargoArgs) { $coreCmd += $CargoArgs }
Write-Host ("[build-release] (2/3) cargo {0}" -f ($coreCmd -join ' '))
& cargo @coreCmd
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$launcherCmd = @('build', '--release', '-p', 'mimageviewer-launcher', '--bin', 'mimageviewer')
if ($CargoArgs) { $launcherCmd += $CargoArgs }
Write-Host ("[build-release] (3/3) cargo {0}" -f ($launcherCmd -join ' '))
& cargo @launcherCmd
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$extractedBridge = Join-Path -Path $appDataRoot -ChildPath 'vst3\mimageviewer-vst3-host.exe'
$extractedBridgeHash = Join-Path -Path $appDataRoot -ChildPath 'vst3\mimageviewer-vst3-host.exe.sha256'
foreach ($path in @($extractedBridge, $extractedBridgeHash)) {
    if (Test-Path $path) {
        try {
            Remove-Item -LiteralPath $path -Force -ErrorAction Stop
            Write-Host ("[build-release] removed stale extracted VST3 bridge cache: {0}" -f $path)
        } catch {
            Write-Warning ("[build-release] failed to remove {0}: {1}" -f $path, $_)
        }
    }
}

exit 0
