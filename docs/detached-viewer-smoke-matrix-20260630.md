# Detached viewer 実機検証マトリクス (2026-06-30)

目的: detached viewer の組み合わせ確認を、チェック数を増やしすぎずに行う。
詳細な低レベル確認は [detached-viewer-smoke-checklist.md](detached-viewer-smoke-checklist.md) を使い、
通常の再確認はこの表を上から順に実施する。

## 0. 準備

- 起動前に `MIV_DETACHED_WINDOW_DEBUG=1` を設定する。
  - PowerShell 例: `$env:MIV_DETACHED_WINDOW_DEBUG=1`
- ログ: `%APPDATA%\mimageviewer\logs\mimageviewer.log`
- 可能なら素材を 1 つの親フォルダにまとめる。
  - `imgA/`, `imgB/`: 通常画像 10 枚以上。1 枚はパノラマ画像、1 枚は編集確認用。
  - `bookA.zip`, `bookB.zip`: 複数ページ ZIP。
  - `pdfA.pdf`, `pdfB.pdf`: 複数ページ PDF。
  - `videoA.mp4`, `videoB.mp4`: 動画。
  - `anim.webp` または `anim.gif`: アニメ画像。
- 2026-06-30 作成済みの代表素材:
  `H:\home\mimageviewer_detached_smoke_20260630`
  - `01_mixed_navigation_pinned_video`: 静止画と動画を混在。ピン留め窓で上下移動して動画へ到達する確認用。
  - `02_many_images`: always-new 複数窓 / ピン / 編集 / V / Shift+Z 用。
  - `03_panorama_and_animation`: 360 候補 + animated WebP / GIF。
  - `04_zip_pdf_parent`: 生成 ZIP / 見開き ZIP / 生成 PDF。

## 1. 成功/失敗の共通マーカー

| 観点 | OK | NG として記録 |
| --- | --- | --- |
| 小窓 / 再生成 | 既存窓の位置・サイズが維持される | 822x656 前後の小窓、位置リセット、ちらつき連続 |
| close | active は `session_closing` -> `session_finish`、passive は `passive_close` が出る | 閉じたはずの窓が残る、空窓が残る |
| linked 復帰 | `passive_activate_still_committed ... active_context=false ... independent=false` | 復帰後にメイン選択へ追従しない |
| independent 復帰 | `active_context_state ... independent=true` | ピン窓 / always-new 窓がメイン操作で中身を変える |
| crash | `panic.log` に新規 panic が増えない | Y-32 / OOM / validation panic |

NG 報告は「ケース ID + 操作 + 見た目 + 該当ログ時刻」で十分。

## 2. 設定セット

| 設定セット | 画像を毎回別ウィンドウ | ZIP/PDF を開いたらページをフルスクリーン表示 | 用途 |
| --- | --- | --- | --- |
| S1 | OFF | ON | 通常 linked + PDF/ZIP 直開き |
| S2 | OFF | OFF | ページ一覧/BS の退行確認 |
| S3 | ON | ON | always-new 複数窓 |

まず S1 -> S3 -> S2 の順で確認する。時間がなければ S1 と S3 だけでよい。

## 3. 最小検証ケース

### A. S1: 通常 linked + ピン切替 (最重要)

| ID | 操作 | 確認 |
| --- | --- | --- |
| A1 | `imgA` の画像を F12 で別ウィンドウ化。メインで別画像を選択 | 別ウィンドウが追従する |
| A2 | 「画像/動画を別ウィンドウで開く」ON。`imgA`、`imgB` を順に開く | 2 枚の独立窓が残る。クリックで復帰できる |
| A3 | A2 の独立窓で `Ctrl+↓` / `Ctrl+PageDown` | フォルダ移動は開始せず toast。メイン一覧も同時に移動しない |
| A4 | A2 の独立窓で `V`、`Shift+Z`、全体補正を確認 | 表示系操作は動く |
| A5 | A2 の独立窓で消しゴム / 補正レイヤー / 隠蔽加工を起動しようとする | 編集機能は無効で案内が出る |
| A6 | 独立窓がある状態で設定 ON→OFF | 開いていた別ウィンドウが閉じ、通知が出る。以後 F12 linked は 1 枚だけでメインに追従する |

### B. S1: PDF/ZIP direct open と folder-nav

| ID | 操作 | 確認 |
| --- | --- | --- |
| B1 | 親一覧から `pdfA.pdf` を開く | メインは親一覧のまま、PDF は別ウィンドウ表示 |
| B2 | PDF 窓でページ送り、`V`/`Shift+Z` は無効または破綻なし | ページ送り正常。モード系でクラッシュしない |
| B3 | `Ctrl+↓` で `pdfB.pdf` または次の本へ移動 | 小窓なし、サイズ維持、同じ窓で切替 |
| B4 | `bookA.zip` でも B1-B3 を短く実施 | ZIP でも同じ |
| B5 | F11 仮想フルスクリーン中に `Ctrl+↓` | borderless のまま。最大化窓に化けない |

### C. S3: always-new 複数窓

| ID | 操作 | 確認 |
| --- | --- | --- |
| C1 | `imgA` から画像を 3 枚連続で開く | 3 窓が少しずれて開く。古い窓は frozen で残る |
| C2 | 1 窓目 -> 3 窓目 -> 2 窓目の順にクリック | active が切替わる。別窓が消えない / 中身が化けない |
| C3 | active 窓で `V`、`Shift+Z`、消しゴムを確認 | `V`/`Shift+Z` は動く。消しゴムなどの編集機能は無効で案内が出る。切替で状態が漏れない |
| C4 | active 窓で F12 / `Ctrl+↓` / `Ctrl+PageDown` | F12 は無効 toast。Ctrl 系フォルダ移動も無効 toast。メイン一覧は動かない |
| C5 | passive 窓を順に閉じる | `passive_close` が出る。空窓や backstop 残りなし |
| C6 | 画像を 8-10 枚程度まで追加で開く | 重さ、ハング、panic の有無を確認。異常があれば枚数を記録 |

### D. S3: PDF/ZIP 複数 book

| ID | 操作 | 確認 |
| --- | --- | --- |
| D1 | `pdfA.pdf` と `pdfB.pdf` を順に開く | 別々の窓。1 つ目が 2 つ目に差し替わらない |
| D2 | 1 つ目へ戻ってページ送り | その PDF だけ動く |
| D3 | 2 つ目へ戻ってページ送り | その PDF だけ動く。1 つ目は保持 |
| D4 | `bookA.zip` / `bookB.zip` でも D1-D3 を短く確認 | ZIP でも同じ |
| D5 | 片方を閉じる | もう片方が残る。font atlas panic が出ない |

### E. S2: ページ一覧/BS

| ID | 操作 | 確認 |
| --- | --- | --- |
| E1 | 親一覧から PDF/ZIP を開く | ページ一覧へ入る。別ウィンドウを勝手に出さない |
| E2 | ページ一覧で画像を開き、BS で戻る | 別ウィンドウが復活/消失を繰り返さない |
| E3 | ピン窓が残っている状態で E1-E2 | ピン窓 / passive 窓に影響しない |

### F. 動画 / アニメ

| ID | 操作 | 確認 |
| --- | --- | --- |
| F1 | S1 で `videoA.mp4` を F12 別ウィンドウ再生 | サイズ維持、再生/停止/close 正常 |
| F2 | `videoA.mp4` 表示中に `videoB.mp4` を開く | 旧窓のサイズが勝手に変わらない。F12 連打で閉じない |
| F3 | S3 で detached 動画を表示し、メイン一覧で別動画を選択する | 明示 open していない選択変更では既存動画窓が勝手に追従しない |
| F4 | S3 で `anim.webp` / `anim.gif` を複数画像窓と混在 | アニメ再生、他窓切替、close 正常 |
| F5 | ピン留めした静止画窓で ↑↓ し、次項目が動画になる | 現在画像を保持し、動画はこの窓で再生できない旨の toast を出す。固まらない |
| F6 | S1 で `anim.webp` / `anim.gif` を F12 別ウィンドウ化してピン | ピン直後もアニメーションが止まらず、再読込表示に落ちない |

## 4. 時間別の実施範囲

| 時間 | 実施ケース |
| --- | --- |
| 10 分 | A1-A4, B1-B3, C1-C3 |
| 20 分 | 上記 + C4-C5, D1-D3, F1 |
| 40 分 | 全ケース |

## 5. ログ確認の短縮コマンド

PowerShell:

```powershell
$log = "$env:APPDATA\mimageviewer\logs\mimageviewer.log"
Select-String -Path $log -Pattern `
  'passive_activate|passive_close|active_context_state|session_|host_lost|clear host|allocate_window_id|font_resync|panic|keepalive_backstop' |
  Select-Object -Last 250
```

見るポイント:

- linked 復帰: `passive_activate_still_committed ... active_context=false ... independent=false`
- pinned/always-new 復帰: `active_context_state ... independent=true`
- close: active は `session_finish`、passive は `passive_close`
- 小窓/再生成疑い: `host_lost`, `clear host`, folder-nav 中の `allocate_window_id`
