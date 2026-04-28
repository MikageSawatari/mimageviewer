# mimageviewer release build wrapper (PowerShell)
#
# When mimageviewer.exe is running (e.g. tray-resident), cargo cannot overwrite
# target\release\mimageviewer.exe at link time, failing with LNK1104.
# This script:
#   1. Stops mimageviewer-* processes started from this repo
#   2. Polls for file-handle release (up to 10 seconds)
#   3. Runs `cargo build --release --bin mimageviewer` (extra args are passed through)
#
# Usage:
#   PS> scripts\build-release.ps1
#   PS> scripts\build-release.ps1 --features foo

[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $CargoArgs
)

$ErrorActionPreference = 'Stop'

$repoRoot = (Get-Location).Path
# Append a trailing separator for path-boundary scoping. Without this, sibling
# directories like `C:\home\mimageviewer-old` would also match (StartsWith on
# `C:\home\mimageviewer` is too permissive).
$repoRootPrefix = $repoRoot.TrimEnd('\') + '\'
$repoRootPrefixLower = $repoRootPrefix.ToLower()
$releaseExe = Join-Path -Path $repoRoot -ChildPath 'target\release\mimageviewer.exe'

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
        } elseif ($p.Name -eq 'mimageviewer-susie32') {
            # susie32 worker is extracted to APPDATA but spawned as a child of the
            # repo-built mimageviewer.exe, so stop it together.
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
# cargo は同一ワークスペース内 bin の依存順序を表現できないため、明示的に 2 回呼ぶ。

$coreCmd = @('build', '--release', '--bin', 'mimageviewer-core')
if ($CargoArgs) { $coreCmd += $CargoArgs }
Write-Host ("[build-release] (1/2) cargo {0}" -f ($coreCmd -join ' '))
& cargo @coreCmd
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$launcherCmd = @('build', '--release', '-p', 'mimageviewer-launcher', '--bin', 'mimageviewer')
if ($CargoArgs) { $launcherCmd += $CargoArgs }
Write-Host ("[build-release] (2/2) cargo {0}" -f ($launcherCmd -join ' '))
& cargo @launcherCmd
exit $LASTEXITCODE
