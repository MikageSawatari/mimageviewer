# タッチ入力診断プローブ — 実機確認手順

対象: v2.13.0 Phase 1 / Step 0。目的は、現行構成のままタッチ入力がどの経路へ配送されるかを
ログで確認することです。ジェスチャや操作を追加するテストではありません。

## 1. セッション前の準備

インストール済み、またはトレイ常駐中の mImageViewer を先に終了してください。検証用 core と
single-instance mutex を共有するため、残っていると検証用 core を起動できません。

PowerShell でリポジトリのルートへ移動し、既存ログを日時付きで退避します。

```powershell
$log = Join-Path $env:APPDATA 'mimageviewer\logs\mimageviewer.log'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if (Test-Path -LiteralPath $log) {
    Copy-Item -LiteralPath $log -Destination "$log.pre-touch-probe-$stamp"
}
```

診断ログの出力先は次のファイルです。

```text
%APPDATA%\mimageviewer\logs\mimageviewer.log
```

検証用 core は通常の `%APPDATA%\mimageviewer` プロファイルを使うため、実際の設定やデータを
更新する可能性があります。

## 2. 診断プローブを有効にして起動

同じ PowerShell で環境変数を設定してから起動します。

```powershell
$env:MIV_TOUCH_DEBUG = '1'
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
Remove-Item Env:MIV_TOUCH_DEBUG
```

`MIV_TOUCH_DEBUG` を設定しない通常起動では、この手順で追加した診断ログは出ません。

## 3. 操作シナリオ

次を上から順に実行してください。このセッションでは UI 倍率や DPI の組み合わせは網羅せず、
入力が各 surface へ配送されるかだけを確認します。

### a. 一覧

一覧のサムネイルを 1 本指でタップし、続けて 1 本指でドラッグします。

確認目的: メイン viewport で `Touch` と合成 pointer がどの順序で届くかを確認します。

### b. 静止画フルスクリーン

静止画をフルスクリーン表示し、1 本指タップ、2 本指ピンチを順に行います。続いて 1 本指で
ドラッグを開始し、押したまま指を画面外へ滑らせて Cancel の発生を試します。

確認目的: 独立したフルスクリーン viewport のイベント列と、Cancel 時に primary release が
届くかを確認します。

### c. 動画 presenter（HUD 非表示）

動画を全画面再生し、HUD が消えている状態で映像の中央をタップします。

確認目的: `WM_POINTER*` が presenter HWND へ配送されるかを確認します。

### d. 動画 HUD — 最重要

**この 3 つは続けて行い、それぞれ別の操作として実施してください。** HUD の表示 region の
内側と外側で配送先が期待どおり割れるかが、Phase 1 の設計判断で最重要の確認項目です。

1. **HUD を表示した状態で、HUD のボタンの上**をタップする
2. **HUD を表示したまま、HUD から離れた映像部分**（画面中央あたり）をタップする
3. **HUD の端すれすれ**（ボタンのすぐ外側）をタップする

確認目的: `WS_EX_NOACTIVATE` と表示 region を持つ HUD HWND に `WM_POINTER*` が届くか、
かつ region の外側は presenter HWND へ抜けるかを確認します。ログの `window=hud` /
`window=presenter` がこの 3 つで切り替わることを見ます。

### e. 動画上の長押し

HUD が消えている状態で、映像の上を長押しします。長押しによってフルスクリーンが閉じた場合は、
終了後の報告に「長押しでフルスクリーンが閉じた」と明記してください。

確認目的: タッチ由来の右クリック合成と、既存の右クリック長押し判定の実機挙動を確認します。

### f. ClickToShow

フルスクリーンの左右パネル設定を ClickToShow にし、画面端をタッチして呼び出しバーを押します。

確認目的: Touch End と `PointerGone` が同じ batch に入っても callout の click が完了するかを
目視確認します。

### g. ペン（利用できる場合のみ）

一覧でペンを画面に触れずにかざし、サムネイルのツールチップまたはハイライトが出るか確認します。

確認目的: ペン hover が現行の pointer 経路で動作するかと、Win32 ログの `PT_PEN` / `INCONTACT`
状態を確認します。

## 4. 終了とログの受け渡し

mImageViewer を終了してログの書き込みを完了させます。PowerShell でログを日時付きファイルとして
デスクトップへコピーします。

```powershell
$log = Join-Path $env:APPDATA 'mimageviewer\logs\mimageviewer.log'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$out = Join-Path ([Environment]::GetFolderPath('Desktop')) "mimageviewer-touch-probe-$stamp.log"
Copy-Item -LiteralPath $log -Destination $out
$out
```

表示された `.log` ファイルを担当者へ渡してください。あわせて、次の目視結果を短く添えてください。

- b で画面外ドラッグ後に操作が押下状態のまま残ったか
- **d の 3 つで、タップした場所と実際に反応した内容**（HUD のボタンが効いたか、映像側の
  再生 / 一時停止が動いたか、何も起きなかったか）
- e の長押しでフルスクリーンが閉じたか
- f の呼び出しバーを押せたか
- g を実施した場合、ペン hover でツールチップまたはハイライトが出たか

機種によってデジタイザの挙動が変わるため、**タブレットの機種名と Windows のバージョン**も
1 行添えてください。
