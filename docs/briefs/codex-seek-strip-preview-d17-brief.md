# D17: ストリップ上のプレビュー 1 枚と、プレビュー置き場の集約

正本: `docs/video-seek-strip-plan.md` の **D17** 節と、その下の
**「下部の並び順」「プレビューの置き場所は 1 つに集約する」** 節。ブランチ `video-strip`
(worktree `C:\home\mimageviewer-video-strip`)。着手前にこの 3 節を読むこと。

## 目的

「音声波形を見ながら絵も見てシークしたい」「サムネイル間隔が粗いとセルだけでは狙えない」
という利用者要望。プレビューの出口を 1 か所にまとめ、**シークバーからでもストリップからでも
同じ場所に 1 枚出る**ようにする。

## 現状 (実測済み。ここを直す)

- `render_core.rs` の下部 HUD 描画に
  `let suppress_hover_preview = video_speed_popup_open || seek_strip_visible;` があり、
  **ストリップが出ている間はシークバーの hover プレビューが完全に抑止されている**。
- 位置は `let preview_y = (hud_rect.min.y - preview_size.y - 14.0).max(8.0);` で、
  **下部 HUD の上端基準**。ストリップは HUD と映像の間に後から入ったので、抑止を外すだけだと
  ストリップ 104pt のうち 90pt を覆う (実測値。`native_seek_strip_rect` は `[H-168, H-64]`、
  プレビュー高は約 236pt)。
- ストリップ側には hover プレビューが無い。

## やること

### 1. プレビューの基準をストリップに合わせる

- ストリップが出ているときは **`strip_rect.min.y` 基準**、出ていなければ従来どおり
  `hud_rect.min.y` 基準。どちらも 14pt 空ける。
- **`seek_strip_visible` による抑止を外す。** `video_speed_popup_open` による抑止は残す
  (カーソル動線が seek 行を横切る問題で、理由が別)。
- 「ストリップが出ている」の正本は、既存の `seek_strip_visible` (= presenter に渡った
  `Option<NativeOverlaySeekStrip>` が `Some`) をそのまま使う。設定値を再判定しない。
- **`compute_hud_regions` の `last_drawn_preview_rect` 経路を必ず追随させる。** プレビューの
  実描画 rect は HWND の `SetWindowRgn` region に入っている。基準を変えて rect が動くのに
  region が追随しないと、プレビュー上のクリックが presenter 側へ抜ける。

### 2. ストリップ上の hover / スクラブでも同じプレビューを出す (D17 本体)

- **波形モードとサムネイル列モードの両方**で出す (2026-08-25 に範囲拡大。当初の
  「サムネイル列では出さない」は取り消し済み)。
- 見た目と位置はシークバーのプレビューと同一。**同じ 1 か所**に出す。
- 時刻はポインタ位置から求める。軸の変換は既存の
  `cell_index_at_pointer` / `waveform_time_at_pointer` と `time_for_center_index` を使い、
  新しい変換式を書かない。
- ワーカーは既存の `ThumbnailWorker` (最新勝ちの単発) を共有する。ストリップ用の窓ワーカーは
  N 枚保持が仕事なので用途が違う。**ポインタは 1 つなので 2 面から同時に要求は来ない。
  owner を分けない。**
- **ラベル時刻 = ポインタ位置なので、この経路は「シーク時のズレ許容」設定
  (`video_seek_thumbnail_tolerance_secs`) を使う** (§3 の表でストリップ本体が使わないと
  決めたのとは性質が違う)。
- 鍵ボタン (`native_seek_strip_lock_button_rect`) の上ではプレビューを出さない。
  ストリップ本体の seek / drag 開始判定から鍵矩形を除外しているのと同じ扱い。

### 3. 排他と lifecycle

- タイル一覧表示中、音声モード、長さ不明の動画では出さない (ストリップ本体と同じ条件)。
- 動画切替・フルスクリーン終了・ストリップを「なし」にしたときにプレビューを残さない。
- ドラッグ中も出し続ける (スクラブしながら絵を見るのが要望の主眼)。

## やってはいけないこと

- **egui の `Area` を画面全体で確保しない。** `interactable(false)` でも `Area` は自分の矩形に
  ウィジェットを 1 つ登録し、egui の hit test は「別レイヤーの手前のウィジェットに完全に
  覆われたもの」を捨てる。全画面の受動 Area はその下の HUD ボタンを丸ごと押せなくする
  (2026-08-25 に実害。`docs/video-architecture.md` の該当節を読むこと)。
  プレビューの Area は**実際に描く矩形ぶんだけ**にする。
- ストリップ本体のホイール (範囲変更) やクリック (シーク) をプレビューが奪わない。
- 下部の並び順を変えない (現状維持で確定済み)。
- 症状パッチ (出ないときの追加 repaint / 一括 reset / silent fallback)。構造で解けないなら
  実装せずに報告する。

## 完了条件

- `cargo fmt` 済み。
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る。
- `cargo test -p mimageviewer --lib` が通る。
- `cargo test --test ui_snapshot` が通る (見た目を変えたなら `UPDATE_SNAPSHOTS=1` で更新し、
  更新後の PNG を目視確認した旨を報告する)。
- `python scripts/check_ui_glyphs.py` が 0 件。
- **テストを入れること**: プレビュー rect の基準がストリップの有無で切り替わること
  (ストリップありのとき `strip_rect.min.y` より上にあり、ストリップと重ならないこと) を、
  純関数または rect helper のテストで固定する。
- 実機確認は**こちら (ClaudeCode) が利用者へ依頼する**ので、Codex 側では起動しないこと。

## 報告してほしいこと

- プレビュー rect の基準をどこで切り替えたか、`compute_hud_regions` をどう追随させたか。
- ストリップ側の時刻算出にどの既存関数を使ったか。
- 鍵ボタン上・ドラッグ中・モード切替時の扱い。
- 判断に迷った点と、正本のどの記述を根拠にしたか。
