# E2E スモークテスト手順書

Claude Code の Computer Use を使って手動実行するテスト。
アプリの主要フローが正常に動作することを確認する。

## 前提条件

- `cargo build --release` でビルド済み
- `testimage/` にテスト画像がある

### 環境準備（重要）

Computer Use はセッションの allowlist にないアプリのウィンドウを
**黒い矩形でマスク**してスクリーンショットに表示する。
テスト対象アプリと重なるウィンドウがあるとテストに支障が出るため、
以下を事前に行う。

1. **HoYoPlay を終了する** — 最小化しても自動復元されるため、
   ユーザ自身がアプリケーションを終了する必要がある
2. **プライマリモニター上の不要なウィンドウを最小化する** —
   エクスプローラー、OBS など大きなウィンドウが重なりやすい
3. **Claude Code のウィンドウをセカンドモニターに移動する** —
   Claude Code 自体も allowlist 外のため、プライマリモニターに
   あると mimageviewer と重なってマスクされる

### Computer Use アクセス設定

```
request_access(apps=["mimageviewer.exe"])
```

- アプリ名は `"mimageviewer.exe"` で登録する（スタートメニューに
  登録されていないため、実行ファイル名で指定する必要がある）
- tier は `"full"` で付与される

## 既知の制約

### フルスクリーンビューポートへのキー送信

mimageviewer のフルスクリーン表示は egui の `show_viewport_immediate`
で**別ウィンドウ**（タイトル: `"egui window"`）として描画される。

Computer Use の `key` アクションはメインウィンドウに送信されるため、
フルスクリーンビューポートに ESC が届かない。

**対処法:** Win32 API の `PostMessage` で直接キーを送信する。

```powershell
# フルスクリーンウィンドウのハンドルを取得
# (check_miv_windows.ps1 で Class="Window Class", Title="egui window" を探す)
$h = [IntPtr]::new(<handle>)
[SendKey]::SetForegroundWindow($h)
[SendKey]::PostMessage($h, 0x0100, [IntPtr]::new(0x1B), [IntPtr]::Zero)  # WM_KEYDOWN VK_ESCAPE
[SendKey]::PostMessage($h, 0x0101, [IntPtr]::new(0x1B), [IntPtr]::Zero)  # WM_KEYUP
```

あるいは、フルスクリーン中に**右クリック**でもグリッドに戻れる
（ただし Computer Use の右クリックも届かない場合がある）。

### TourBox Console の割り込み

TourBox Console がフォアグラウンドに出ると Computer Use の
操作がブロックされる。発生した場合は PowerShell で mimageviewer
を前面に戻す:

```powershell
# minimize_explorer.ps1 を実行するか、以下を直接実行
$miv = Get-Process mimageviewer
# SetForegroundWindow で復帰
```

## テストシナリオ

### 1. 起動テスト

**手順:**
1. `target/release/mimageviewer.exe` を起動
   ```bash
   cd H:/home/mimageviewer && start target/release/mimageviewer.exe
   ```
2. `request_access(apps=["mimageviewer.exe"])` でアクセス許可を取得
3. スクリーンショットを取得

**確認項目:**
- ウィンドウが表示される
- ツールバー（列数・比率・お気に入り）が表示される
- グリッド領域が表示される（空または前回のフォルダ）

### 2. フォルダ表示テスト

**手順:**
1. ツールバーの「testimage」お気に入りボタンをクリック
   （お気に入り未登録の場合はアドレスバーにパスを入力）

**確認項目:**
- グリッドにサムネイルが表示される
- サブフォルダがグリッド先頭に表示される
- 画像のサムネイルが正しく描画される
- ファイル名がセル下部に表示される

### 3. フルスクリーン表示テスト

**手順:**
1. 画像のあるサブフォルダ（例: iphone）にダブルクリックで移動
2. 画像サムネイル（動画以外）をダブルクリック
3. スクリーンショットで全画面表示を確認
4. グリッドに戻る（上記「フルスクリーンビューポートへのキー送信」参照）

**確認項目:**
- フルスクリーンモードに遷移する
- 画像が画面全体にアスペクト比を維持して表示される
- 左クリックで次の画像に遷移する
- ESC（PostMessage 経由）でグリッドに戻る

**注意:**
- フルスクリーン中の左クリックは「次の画像」動作になる
- ダブルクリックは「ズームイン/アウト」動作になる
- グリッドに戻るには ESC を PostMessage で送信するのが確実

### 4. キーボードナビゲーション

**手順:**
1. グリッド表示状態でセルをクリックして選択
2. 矢印キー (←→↑↓) を押す
3. 選択枠（青いハイライト）が移動することを確認
4. BS（Backspace）キーを押す

**確認項目:**
- 矢印キーで選択が移動する
- 選択中のファイル名とサイズがセル下部に表示される
- BS で親フォルダに遷移する
- 遷移前のフォルダがハイライトされる

### 5. 設定永続化テスト

**手順:**
1. ツールバーで列数を変更（例: 9 → 4）
2. アプリを閉じる
   ```bash
   powershell -Command "Stop-Process -Name mimageviewer -Force"
   ```
3. アプリを再起動する
   ```bash
   start target/release/mimageviewer.exe
   ```
4. ツールバーの列数表示を確認

**確認項目:**
- 列数が変更後の値（4）で保持されている
- 最後に開いたフォルダが復元されている
- アスペクト比設定が保持されている

**クリーンアップ:** テスト後は列数を元の値に戻す。

## ヘルパースクリプト

テスト中に使用する PowerShell スクリプトが
`C:\Users\mikag\` に以下の名前で保存されている:

| スクリプト | 用途 |
|---|---|
| `find_overlap.ps1` | プライマリモニター上の全ウィンドウ一覧 |
| `check_miv_windows.ps1` | mimageviewer の全ウィンドウ（ハンドル・座標） |
| `minimize_explorer.ps1` | エクスプローラーを最小化し mimageviewer を前面に |
| `send_esc.ps1` | フルスクリーンビューポートに ESC を送信 |

**注意:** `send_esc.ps1` のウィンドウハンドルは毎回変わるため、
`check_miv_windows.ps1` で `"egui window"` のハンドルを確認して
書き換える必要がある。

## 実行方法

Claude Code で以下のように依頼:

```
mimageviewer の E2E スモークテストを実行してください。
docs/e2e-smoke-test.md の手順に従って、Computer Use で
アプリを操作し、各シナリオの確認項目をチェックしてください。
```
