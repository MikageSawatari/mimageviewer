# 実装 brief: P4b-3 スナップショット入力の MP4 書き出し（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high（実装・テスト）。設計・検収: ClaudeCode Opus 5.5。独立レビューは別担当。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。報告は日本語。

## 1. 正本

`docs/music-video-export-plan.md` の D16 末尾「#### P4b の設計」節の **P4b-3**、D10「未保存の編集と終了」の書き出しの段落、
D11（出力の既定値）、D13（中断可能・UI を止めない）、H3a の契約（D3 の改訂）が正本（実装前レビュー 3 回で ACCEPT）。
前提がコードと食い違ったら、実装で吸収せず止めて報告する。

## 2. 作業範囲

### 2.1 本体 lib（`src/video/mvexport/`）

1. `MusicVideoExportRequest` から `audio_path` をなくし、スナップショットを受ける:
   - Project の複製
   - 素材の bundle（`Assets` を Project の全 `images` を残して fork したもの。フォントは Arc 共有、
     文字・拡縮キャッシュは空から作り直してよい）
   - PCM の `Arc`（stereo f32 samples・sample rate）、長さを揃える前の元音源長、PCM の識別。識別は型引数で受け
     （`AnalysisKey<I>` と同じ考え方）、書き出しの解析タイムラインの key へそのまま渡す。本体 lib は
     `musicvideo-ui` に依存しない。
   - エンコーダ・ビットレート（開始時の値で固定）、`replace_existing`、`compute_frame_hash`。
   書き出しはファイルを読まない。
2. 較正は worker で同じ関数から計算し直す（エディタの較正結果は受け取らない）。
3. AAC: PCM 全体の複製を作らない。H3a の view（出力尺で見た PCM）を論理長まで走査し、既存の入力単位
   （`AUDIO_CHUNK_SAMPLES_PER_CHANNEL` の stereo チャンク）だけを実体化して `ProcessedChunk` に渡す。
   切り詰め・無音延長・PTS は view の規則に従い、現行の `fit_audio_to_duration` + `chunks` と同じ出力になること
   （ビット一致をテストする）。`fit_audio_to_duration` による全体の resize はやめる。
4. 進捗: 段階（反応範囲の計算・音声の符号化・フレームの書き出し・仕上げ）と frame 数を、UI が読める形で出す
   （既存の `ExportPending` の progress を拡張する形でよい）。
5. 公開: 同じディレクトリの一時 MP4 に書き、最後の公開操作で保証する。`replace_existing = false` なら上書きしない
   公開（`tempfile` の `persist_noclobber` 相当）。true なら置換付きの公開。原本を先に削除しない。false で公開時に
   存在していたら失敗（通知だけ）。取消・公開失敗では既存の出力を残す。現行の `exists()` 確認後の `rename()` は
   置き換える。開始時の `output_path.exists()` 拒否は `replace_existing = false` のときだけ残してよい（最終保証は
   公開操作）。
6. `src/bin/mvexport_cli.rs` は自分で FFmpeg デコード（既定トラック）してから同じ入口を呼ぶ。識別は CLI 内で
   固定値でよい。`replace_existing` は false。

### 2.2 エディタ（`crates/musicvideo-editor`）

1. 「MP4 を書き出す」操作を追加する（連番 PNG は開発用として残す）。押すと設定ダイアログ（エンコーダ・
   ビットレート。既定は D11 と `mvexport_cli` の既定値。出力の解像度・fps は Project の値を表示）→ 保存ダイアログ
   （既存の rfd の使い方）→ 開始。ダイアログで既存のファイルを選んだら `replace_existing = true`、無ければ false。
   書き出し設定は保存しない（D10）。
2. **開始できる条件**: 素材がすべて Ready、Project / LRC / 素材の読込・保存・歌詞の行対応確認のどれも進行中でない、
   音声がある。満たさないときはボタンを無効にし、理由をツールチップ等で出す。
3. **モーダル**: 開始時に再生を一時停止（P4b-2 のコマンド）し、新しいプレビュー worker の起動を止める。実行中の
   プレビュー worker には取消を通知し、結果は非同期で受けて捨てる（UI は待たない）。書き出し中は編集・開く・保存・
   新規・再生を受け付けない。進捗（段階と frame 数）と「中止」だけを出す。中止は既存のキャンセル（書きかけの削除）。
   中止の完了は frame 数ではなく worker の終端の結果で判断する。完了・失敗・中止の結果を表示してモーダルを閉じる。
4. スナップショットの作成は UI スレッドで行ってよいが、重い処理（PCM の複製・画像の複製）をしないこと。
   PCM はプレビューが使っている同じ `Arc` と、その `ResourceRevision` を渡す（`Arc` の同一性を識別に使わない）。
5. ウィンドウを閉じる要求が書き出し中に来た場合の確認（D10）は P4b-4 で実装する。P4b-3 では、書き出し中の閉じる
   要求の扱いを変えない（既存の Drop の取消のまま）。終了の契約は P4b-4 で完成させる。
6. 設計書の P4b 節に「P4b-3 実装記録」を追記し、§7 の P4b 行を更新する。

範囲外: 未保存確認と終了（P4b-4）、起動引数（P4c）。

## 3. テスト

- スナップショット: 書き出し開始後に元の音声・画像ファイルを変更・削除しても、出力（RGBA ハッシュと音声）が
  開始時点の内容と一致する。Undo で以前の音声へ戻した後の書き出しが、戻した音声を使う（識別と PCM が一致）。
- AAC: view からのチャンク実体化が、従来の resize + chunks と同じパケット列（または同じ PCM 入力列）になる。
  出力尺がソースより短い・長い・同じの 3 通り。
- 公開: `replace_existing = false` で公開直前に出力先が作られた場合に上書きしない。true で置き換える。取消・失敗で
  既存の出力が残る。原本を先に消さない。
- 開始条件: 読込中・保存中・歌詞確認待ち・素材未 Ready・音声なしのそれぞれで開始できない。
- モーダル: 開始で再生が止まり、プレビュー worker が新しく起動しない。書き出し中の編集・開く・保存・再生が受け付けられない。
  中止の完了が worker の終端の結果で決まる（frame 数が最後まで進んでも終端前は完了扱いにしない）。
- 較正: 書き出しの較正がエディタの較正と同じ値になる（同じ入力）。
- H3a: 書き出しの解析タイムラインの key にエディタと同じ識別が入る。
- 既存の H1 / H2 / H3a / I1 / L1〜L3 / P4b-1 / P4b-2 のテストがすべて通る。
- 各新テストが、意図的に壊した実装で落ちることを確かめて記録する。
- 5 分 1080p60 の実測: 編集済みのプロジェクト（画像・スペクトラム・鍵盤・文字・モジュレータを含む）を CLI で
  書き出し、完走と速度・各区間を記録する。マシンが静かなことを確認してから測る（他の cargo / codex が動いて
  いないこと）。静かにできない場合はその旨を記録し、数値を正式値としない。
- 確認コマンド（`MSBUILDDISABLENODEREUSE=1`）:
  ```
  cargo test -p musicvideo-editor
  cargo check -p musicvideo-editor --features dev-tools --bins
  cargo test -p musicvideo-core -p musicvideo-ui
  cargo test -p mimageviewer --lib mvexport
  cargo fmt --check
  python scripts/check_ui_glyphs.py
  cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo
  cargo build --release --bin mvexport_cli
  ```

## 4. 注意

- 本体アプリもエディタも起動しない（CLI の書き出しは可）。コミットしない。`cargo fmt` はかける。
- まれな失敗（書き込み失敗・公開失敗）はログと通知だけ。再試行は作らない。
- 緑の区切りで止めて報告しなくてよい。止めるのは、設計判断が要るときと前提の矛盾を見つけたときだけ。

## 5. 報告

変更ファイル、追加テストと失敗確認、全テスト件数、5 分の実測値、正本との相違（あれば）を日本語で。
