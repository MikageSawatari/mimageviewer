# remote-web PoC サーバを「停止 → release ビルド → 起動」する開発用ヘルパー。
#
# 稼働中の exe がファイルを掴んでいると cargo が上書きできず os error 5 で失敗するため、
# 先に確実に停止してからビルドする。起動はコンソールを占有しないよう detached で行い、
# 標準出力 (接続 URL / QR / Bearer トークン) をログファイルへ落とす。
#
# 使い方:
#   .\scripts\restart-remote-web.ps1
#   .\scripts\restart-remote-web.ps1 -Bind 0.0.0.0 -Port 8788
param(
    [string]$Bind = '127.0.0.1',
    [int]$Port = 8787
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$exe = Join-Path $root 'target\release\mimageviewer-remote.exe'
$outLog = Join-Path $root 'remote-web-console.log'
$errLog = Join-Path $root 'remote-web-console.err.log'

# 1. 稼働中プロセスを停止し、ファイルハンドルが解放されるまで待つ
$running = Get-Process -Name 'mimageviewer-remote' -ErrorAction SilentlyContinue
foreach ($proc in $running) {
    Write-Host "stopping mimageviewer-remote (PID $($proc.Id))"
    Stop-Process -Id $proc.Id -Force
    try { Wait-Process -Id $proc.Id -Timeout 10 } catch {}
}

# 2. release ビルド
Write-Host 'building (release)...'
cargo build -p mimageviewer-remote --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

# 3. 起動 (detached)。stdout に接続 URL と QR が出るのでログへ落とす
if (Test-Path $outLog) { Remove-Item $outLog -Force }
if (Test-Path $errLog) { Remove-Item $errLog -Force }
$started = Start-Process -FilePath $exe `
    -ArgumentList '--bind', $Bind, '--port', $Port `
    -RedirectStandardOutput $outLog `
    -RedirectStandardError $errLog `
    -PassThru

Write-Host "started mimageviewer-remote (PID $($started.Id)) on ${Bind}:${Port}"
Write-Host "console log: $outLog"
