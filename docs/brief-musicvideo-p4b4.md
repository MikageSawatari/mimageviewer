# 実装 brief: P4b-4 未保存の確認と終了（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high（実装・テスト）。設計・検収: ClaudeCode Opus 5.5。独立レビューは別担当。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。報告は日本語。

## 1. 正本

`docs/music-video-export-plan.md` の D16 末尾「#### P4b の設計」節の **P4b-4**（実装前レビュー 3 回で ACCEPT。2 回目の
「先行する Project 読込」「歌詞の未確定の編集」の規則を含む）と、D10「未保存の編集と終了」が正本。
前提がコードと食い違ったら、実装で吸収せず止めて報告する。

## 2. 現状（確認済みの前提）

- 文書の dirty 判定は `crates/musicvideo-ui/src/document.rs` の `is_dirty`（保存済み Project との比較）。
  `complete_save` の true は「保存の基準を現在の文書へ採用した」であって「clean になった」ではない。
- 歌詞の行対応の確認待ちは `musicvideo-ui` の `has_pending_lyrics_edit()`（`lyrics_editing.rs` の `PendingResolution`）。
- エディタに「新規」の操作は無い。ウィンドウを閉じる要求の処理（`CancelClose`）も無い（終了は Drop の取消だけ）。
- Project 読込の競合確認（H1 の `ApplyProject` / `DiscardProject`）は既にある。

## 3. 作業範囲

1. 操作の意図の型: `開く(パス)` / `新規` / `閉じる`。保留中の意図は 1 つだけで、文書 session に所属させる
   （別の文書へ切り替わったら捨てる）。P4c の「最近使ったものから開く」は後で `開く(パス)` に合流させるので、
   専用の分岐は作らない。
2. 「新規」の操作を追加する（ツールバー。既存の Project の既定値で新しい文書を作る。音声・素材の扱いは既存の
   Project 読込の切り替えと同じ規則にそろえる。迷ったら止めて報告）。
3. 意図を受理した時点で:
   - 開始済み・待機中の Project 読込を取り消す。**取消フラグだけで済ませず、既に受信可能になった成功結果も採用しない**。
     Ready の結果は既存の遅延解放の経路で捨てる。UI は終了を待たない。保留中は読込の新規開始も受け付けない。
   - 歌詞の確認待ちがあれば先にその確認を出す。全レイヤーの `has_pending_lyrics_edit()` が false になるまで意図を
     保留する（`NeedsResolution` なら確認を続け、`Unchanged` も完了として扱う）。取り消しは既存の `cancel_pending()` と
     返された TextEdit の rollback の復元を使い、反映は既存の編集 transaction・履歴・`note_edit` を通す。
4. 判定（1 つの関数）: 未保存でなければ実行。未保存なら「保存 / 破棄 / キャンセル」のモーダル。保存は通常の保存
   （未保存の新規なら保存先の選択）を経る。保存の失敗・キャンセルなら意図を捨てる。
5. 保存完了後の再判定: `complete_save` は保存の基準の更新だけ。意図の再判定は、そのフレームの入力と完了の採用
   （`poll_projects` の後の `poll_lrc` などを含む）がすべて済んだ後に 1 回行う。保存を待つ間も確認のモーダルを出したままにする。
   保存中に意図が来たら、保存の完了を待ってから判定する（UI は止めない）。
6. 書き出し中の閉じる要求: 「書き出しを中止して閉じますか」。承認なら中止の完了（worker の終端の結果）を待ってから
   判定へ進む。拒否なら閉じない。
7. ウィンドウを閉じる要求: eframe の close 要求を `CancelClose` で取り消してこの判定へ回す。実行が決まったら `Close` を送る。
   承認済みの終端状態を持ち、自分で送った `Close` を再び `CancelClose` しない。
8. 確認中にウィンドウを閉じる要求が来たら、保留中の意図を「閉じる」に置き換える（他の意図はモーダルなので来ない）。
9. 設計書の P4b 節に「P4b-4 実装記録」を追記し、§7 の P4b 行を更新する。D10 の文言と実装が違う点があれば記録する。

範囲外: 起動引数・最近使った一覧（P4c）。

## 4. テスト（handler テスト。順序を制御する）

- 開く・新規・閉じる × 保存成功 / 保存失敗 / 保存キャンセル / 破棄 / キャンセル。
- 保存は成功したが、完了時点で再び未保存（保存中に LRC・素材の完了が採用された）→ 再確認が出る。
- 同じフレームで保存完了と LRC の採用が起きる順序（再判定がその後に 1 回）。
- 開始済み Project 読込の完了と保存完了の両順序、Ready / 待機中の読込がある状態での意図受理、既に受信可能になった
  成功結果が採用されないこと。
- 歌詞の取消と読込完了が同じフレームになる順序。歌詞の `NeedsResolution` の継続、`Unchanged` の完了扱い、取消時の rollback。
- 保存中の閉じる要求、書き出し中の閉じる要求（承認・拒否）、中止の完了が worker の終端で決まること。
- close 要求の `CancelClose` と、承認後の `Close` を再び取り消さないこと。確認中の閉じる要求による置き換え。
- 文書 session が切り替わったら保留中の意図を捨てること。
- 既存の H1 / H2 / H3a / I1 / L1〜L3 / P4b-1〜3 のテストがすべて通る。
- 各新テストが、意図的に壊した実装で落ちることを確かめて記録する。
- 確認コマンド（`MSBUILDDISABLENODEREUSE=1`）:
  ```
  cargo test -p musicvideo-editor
  cargo check -p musicvideo-editor --features dev-tools --bins
  cargo test -p musicvideo-core -p musicvideo-ui
  cargo test -p mimageviewer --lib mvexport
  cargo fmt --check
  python scripts/check_ui_glyphs.py
  cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo
  ```

## 5. 注意

- 本体アプリもエディタも起動しない。コミットしない。`cargo fmt` はかける。
- まれな失敗（保存先の書き込み失敗など）はログと通知だけ。回復の仕組みは足さない。
- 状態を増やす前に、既存の pending と H1 の仕組みで表せないかを先に考える（CLAUDE.md「設計の簡素化」）。
- 緑の区切りで止めて報告しなくてよい。止めるのは、設計判断が要るときと前提の矛盾を見つけたときだけ。

## 6. 報告

変更ファイル、追加テストと失敗確認、全テスト件数、正本との相違（あれば）を日本語で。
