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
$repoRootLower = $repoRoot.ToLower()
$releaseExe = Join-Path -Path $repoRoot -ChildPath 'target\release\mimageviewer.exe'

# Match all "mimageviewer*" prefix processes (Get-Process -Name does not accept
# wildcards, hence the Where-Object filter).
$candidates = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'mimageviewer*' })

$toKill = @()
foreach ($p in $candidates) {
    $path = $null
    try { $path = $p.Path } catch { $path = $null }
    $included = $false
    if (-not $path) {
        $included = $true
    } else {
        $pl = $path.ToLower()
        if ($pl.StartsWith($repoRootLower)) {
            $included = $true
        } elseif ($p.Name -eq 'mimageviewer-susie32') {
            # susie32 worker is extracted to APPDATA but spawned as a child of the
            # repo-built mimageviewer.exe, so stop it together.
            $included = $true
        }
    }
    if ($included) {
        $toKill += $p
    }
}

if ($toKill.Count -eq 0) {
    Write-Host "[build-release] no running mimageviewer process found"
} else {
    foreach ($p in $toKill) {
        $pathLabel = '(path unknown)'
        if ($p.Path) { $pathLabel = $p.Path }
        Write-Host ("[build-release] stopping {0} (PID={1}) {2}" -f $p.ProcessName, $p.Id, $pathLabel)
        try {
            Stop-Process -Id $p.Id -Force -ErrorAction Stop
        } catch {
            Write-Warning ("[build-release] Stop-Process failed for PID={0}: {1}" -f $p.Id, $_)
        }
    }
    # Backup with taskkill in case Stop-Process was denied. taskkill exits 128 on
    # "no such image" so it is harmless when nothing matches.
    & taskkill /IM mimageviewer.exe /F 2>$null | Out-Null
    & taskkill /IM mimageviewer-susie32.exe /F 2>$null | Out-Null
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

$cargoCmd = @('build', '--release', '--bin', 'mimageviewer')
if ($CargoArgs) {
    $cargoCmd += $CargoArgs
}
Write-Host ("[build-release] cargo {0}" -f ($cargoCmd -join ' '))
& cargo @cargoCmd
exit $LASTEXITCODE
