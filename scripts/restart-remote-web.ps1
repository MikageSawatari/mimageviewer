# Dev helper: stop -> release build -> start the remote-web PoC server.
#
# A running exe keeps a file handle that makes cargo fail with os error 5, so the
# running process is stopped first and we wait for the handle to be released.
# The server is started detached and its stdout (connection URL / QR / bearer token)
# is redirected to a log file.
#
# ASCII only on purpose: Windows PowerShell 5.1 reads BOM-less scripts as ANSI,
# and non-ASCII comments can break parsing (see CLAUDE.md "Markdown / text encoding").
#
# Usage:
#   .\scripts\restart-remote-web.ps1
#   .\scripts\restart-remote-web.ps1 -Bind 0.0.0.0 -Port 8788
#   .\scripts\restart-remote-web.ps1 -DataDir C:\path\to\data
param(
    [string]$Bind = '127.0.0.1',
    [int]$Port = 8787,
    # Isolated data directory used for verification so the live %APPDATA%\mimageviewer
    # is never touched. Launch mimageviewer-core.exe with the same --data-dir.
    [string]$DataDir = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if ([string]::IsNullOrWhiteSpace($DataDir)) {
    $DataDir = Join-Path $root 'target\dev-runtime\data'
}

$exe = Join-Path $root 'target\release\mimageviewer-remote.exe'
$outLog = Join-Path $root 'remote-web-console.log'
$errLog = Join-Path $root 'remote-web-console.err.log'

# 1. Stop the running server and wait for its file handles to be released.
$running = Get-Process -Name 'mimageviewer-remote' -ErrorAction SilentlyContinue
foreach ($proc in $running) {
    Write-Host "stopping mimageviewer-remote (PID $($proc.Id))"
    Stop-Process -Id $proc.Id -Force
    try { Wait-Process -Id $proc.Id -Timeout 10 } catch {}
}

# 2. Release build.
Write-Host 'building (release)...'
cargo build -p mimageviewer-remote --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

# 3. Start detached. stdout carries the connection URL and QR code.
if (Test-Path $outLog) { Remove-Item $outLog -Force }
if (Test-Path $errLog) { Remove-Item $errLog -Force }
$started = Start-Process -FilePath $exe `
    -ArgumentList '--bind', $Bind, '--port', $Port, '--data-dir', $DataDir `
    -RedirectStandardOutput $outLog `
    -RedirectStandardError $errLog `
    -PassThru

Write-Host "started mimageviewer-remote (PID $($started.Id)) on ${Bind}:${Port}"
Write-Host "data dir   : $DataDir"
Write-Host "console log: $outLog"
