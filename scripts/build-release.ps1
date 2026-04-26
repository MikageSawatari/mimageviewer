# mimageviewer release ビルドラッパー (PowerShell)
#
# トレイ常駐などで `mimageviewer.exe` が動いていると、cargo がリンク段階で
# `target\release\mimageviewer.exe` を上書きできず LNK1104 (アクセスが拒否
# されました) で失敗する。本スクリプトは:
#   1. 実行中の mimageviewer.exe / mimageviewer-susie32.exe を探して停止
#   2. ファイルハンドル解放を待つ
#   3. `cargo build --release --bin mimageviewer` (引数は透過)
# を順に実行する。
#
# 使い方:
#   PS> scripts\build-release.ps1
#   PS> scripts\build-release.ps1 --features foo  # 追加引数は cargo にパス
#
# ビルド完了後にユーザーが mIV を再起動するのは手作業 (タスクトレイ常駐を
# 維持するかどうかはユーザーの意図に依存するため自動で再起動はしない)。

[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $CargoArgs
)

$ErrorActionPreference = 'Stop'

# 停止対象の image name (cargo が触るのは release exe だが、susie32 も
# 親プロセスとファイル系を共有することがあるので一緒に止める)。
$Targets = @('mimageviewer', 'mimageviewer-susie32')

$KilledAny = $false
foreach ($name in $Targets) {
    $procs = Get-Process -Name $name -ErrorAction SilentlyContinue
    foreach ($p in $procs) {
        Write-Host ("[build-release] stopping {0} (PID={1})" -f $p.ProcessName, $p.Id)
        try {
            Stop-Process -Id $p.Id -Force -ErrorAction Stop
            $KilledAny = $true
        } catch {
            Write-Warning ("[build-release] Stop-Process failed for PID={0}: {1}" -f $p.Id, $_)
        }
    }
}

if ($KilledAny) {
    # OS のファイルハンドル解放は数百 ms 遅れることがあるのでポーリングする。
    # Test-Path で書き込みロックは検知できないので、Open-FileWrite を試して捕まえる。
    $exePath = Join-Path -Path (Get-Location) -ChildPath 'target\release\mimageviewer.exe'
    if (Test-Path $exePath) {
        $deadline = (Get-Date).AddSeconds(5)
        while ((Get-Date) -lt $deadline) {
            try {
                # 書き込み排他で開いてみる: 成功したらロックは抜けている。
                $fs = [System.IO.File]::Open($exePath, 'Open', 'ReadWrite', 'None')
                $fs.Close()
                break
            } catch {
                Start-Sleep -Milliseconds 100
            }
        }
    }
}

$cargoCmd = @('build', '--release', '--bin', 'mimageviewer')
if ($CargoArgs) {
    $cargoCmd += $CargoArgs
}
Write-Host ("[build-release] cargo {0}" -f ($cargoCmd -join ' '))
& cargo @cargoCmd
exit $LASTEXITCODE
