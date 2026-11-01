# 実装 brief: P4b-2 音声スレッドが再生を所有（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high（実装・テスト）。設計・検収: ClaudeCode Opus 5.5。独立レビューは別担当。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。報告は日本語。

## 1. 正本

`docs/music-video-export-plan.md` の D16 末尾「#### P4b の設計」節の **P4b-2** と、D10「音声再生の所有」が正本
（実装前レビュー 3 回で ACCEPT）。前提がコードと食い違ったら、実装で吸収せず止めて報告する。

## 2. 現状（確認済みの前提）

- `crates/musicvideo-editor/src/playback.rs` の `AudioPlayer::new` は UI スレッドから呼ばれ、デバイス照会・
  stream 生成をその場で行う（`app.rs` の `audio_player_factory`、音声の採用時）。f32 以外の出力形式は
  エラーで再生不可。
- UI の `snapshot()`・`toggle()`・`seek()`・`replace_audio()` と、音声コールバックの `fill_output_at` が
  同じ `Mutex<PlayerState>` を取り合う。
- 再生位置は再生 anchor（コールバックの playback タイムスタンプ）から求めた**聞こえている位置**。

## 3. 作業範囲

1. 専用の音声スレッドを作り、デバイス照会・`cpal::Stream` の生成・維持・drop をすべてそこで行う。
   UI はコマンドをチャネルで送るだけで、このスレッドを待たない（`join` しない）。エディタの終了時は
   終了コマンドを送って切り離す（stream の drop はこのスレッド内）。
2. コマンドは FIFO で処理する: 音声の差し替え（PCM の `Arc` と `ResourceRevision` と位置）、再生／一時停止の
   切り替え、一時停止、シーク、終了。UI は送るたびに操作番号を振る。
3. UI が読むスナップショット: 適用済みの操作番号、PCM の `ResourceRevision`、聞こえている位置、再生中か、
   直近のエラー（デバイスが開けない等）。UI は**自分が最後に送った操作番号より古い**スナップショットの位置を
   採用しない（その間は自分が持つ位置を使う）。スナップショット用の lock はコールバックが PCM を書く処理の
   lock と分ける（UI の読み取りがコールバックを待たせない、コールバックが UI の読み取りを待たない）。
4. 位置は現行と同じ再生 anchor の方式で求める（書込カーソルで代用しない）。H3a の `index_at` に渡すのは
   この位置。差し替えでは停止状態にし、anchor を消す（現行どおり）。古い PCM の解放はこのスレッドで行う。
5. 出力形式: f32 に加え i16 / u16 など cpal が出す形式を受け、変換して書く。出力 sample rate への対応は
   現行の分数ステップ（`source_frames_per_output_frame`）を維持する。新しい resample 方針は足さない。
6. デバイスが開けない・途中で失われた（まれ）: ログと画面の通知だけ。デバイス初期化に失敗した後も、次の再生
   操作で開き直しを試みる。再試行のループや待機は作らない。
7. `app.rs` の呼び出し側を新しい API に替える。H2 の音声差し替え・Undo の順序、H3a の A1b テスト
   （`audio_player_factory` を使う headless テスト）が成り立つよう、テスト用にデバイスを使わない差し替え口を残す
   （コマンドとスナップショットの規則は本物と同じコードを通すこと）。
8. 設計書の P4b 節に「P4b-2 実装記録」を追記し、§7 の P4b 行を更新する。

範囲外: 書き出し（P4b-3）、未保存確認（P4b-4）。

## 4. テスト

- 操作番号: 差し替え・Undo・同じ PCM への連続シークの直後に、古いスナップショットの位置が採用されない。
- FIFO: コマンドの順序どおりに適用される（差し替え→シーク、シーク→差し替え）。
- 差し替えで停止・anchor 消去・新しい `ResourceRevision` がスナップショットに出る。
- 出力形式の変換: f32 / i16 / u16 のコールバック充填が同じ PCM から期待値を書く（デバイス無しで、充填関数を
  直接テストする）。
- lock の分離: スナップショットの読み取りが、PCM を書く処理の lock を取らないこと（構造で示すか、テストで
  コールバック側の lock を保持したまま UI が読めることを確かめる）。
- UI スレッドがデバイス照会・stream 生成・drop・join をしないこと（コードの経路で示す）。
- 終了: 終了コマンド後にスレッドが stream を drop して終わる（デバイス無しの差し替え口で確認）。
- 既存の H1 / H2 / H3a / I1 / L1〜L3 のテストがすべて通る。
- 各新テストが、意図的に壊した実装（例: 操作番号の比較を外す、lock を共有に戻す）で落ちることを確かめて記録する。
- 確認コマンド（`MSBUILDDISABLENODEREUSE=1`）:
  ```
  cargo test -p musicvideo-editor
  cargo check -p musicvideo-editor --features dev-tools --bins
  cargo test -p musicvideo-core -p musicvideo-ui
  cargo fmt --check
  python scripts/check_ui_glyphs.py
  cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo
  ```

## 5. 注意

- 本体アプリもエディタも起動しない（実機の音は設計担当が後で確認する）。コミットしない。`cargo fmt` はかける。
- 緑の区切りで止めて報告しなくてよい。止めるのは、設計判断が要るときと前提の矛盾を見つけたときだけ。

## 6. 報告

変更ファイル、追加テストと失敗確認、全テスト件数、正本との相違（あれば）を日本語で。
