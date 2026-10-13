# ブリーフ: I2 段 2b — 300ms IME grace の撤去と focus 復帰の作り直し

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
担当: 実装・テスト = Codex GPT-6.1 Sol / high。検収 = ClaudeCode Opus。独立レビュー = 別の GPT-6.1 Sol / xhigh。

**設計の正本は `docs/music-video-export-plan.md` の「#### I2（IME が処理したキー）」の段 2b（設計案 第 5 版、
独立レビュー ACCEPT）。** 項目 1・2・2a・2b・3・4・5・7 と、前提条件 6 の記録（Google 日本語入力・MS-IME）を
まず読み、前提をコードで確かめること。前提と食い違うものを見つけたら、黙って直さずに報告で返すこと
（設計判断が必要なときだけ止まってよい）。

背景: 段 2a（`62bc9f47c` / `29367ba4d`）で、IME が処理したキーの press は egui へ届かなくなった。
300ms grace は「確定直後に利用者が押した通常の Enter / Esc / ショートカット」を捨てるだけになっている。
利用者は全面的な修正（B）を選び、確定直後だけ違う動きを揃えることを了承している。

---

## 1. やること（設計の項目どおり）

1. **grace の撤去**（項目 1）: `crates/egui-ime` の `last_event_at` と `IME_EVENT_GRACE` を外し、
   `ime_input_active` / `ime_input_active_in_viewport` は composing だけを返す。`ime_input_active_with_pending_events`
   は廃止して `ime_composing_with_pending_events`（最終状態への射影）へ統合する。`TextInputPhase::ImeGrace` は
   composing の段として名前と意味を変えて残す。
2. **呼び出し側**（項目 2）: 原則変えない。grace 前提のコメント・分岐（`should_close_fullscreen_on_enter`、
   検索バーの `handle_keyboard` 抑止 `app.rs` 53715 付近、specified-scale の数値 popup、削除確認 Y/N など）は
   1 つずつ確かめ、不要になったものはコメントと分岐を消す。
3. **focus 復帰の作り直し**（項目 2a）: キーの所有（`recovering_text_input` / `keyboard_focus_recovery_input`）は
   変えない。復帰（`restore_focus_for_ime_key`）は「`lost_focus()`、pointer 無しの key press、他の widget に focus が無い、
   かつ（この pass の Ime 反映後も composing、または この pass に Enter / Escape の press が無い）」のときだけ。
   retry latch は常に helper 所有にし、呼び出し側の `focus_request` には書かない。次 pass で使うのは同じ条件
   （`lost_focus()` 以外）を満たし、欄がまだ focus を持っていないときだけ。1 pass で消える。
4. **native presenter のタグ選択欄**（項目 2b）: 今 pass の状態（射影）で判定する。
5. **生キー経路の 0xE5**（項目 3）: 本体 Win32 queue（`src/key_input.rs` の `key_edge_from_message`）と
   native presenter の生キー公開（`src/video/native_window.rs` 2223 付近）で、`VK_PROCESSKEY` の press / repeat を
   記録・公開しない（release は残す）。
6. **同じ ViewportId の stale composing**（項目 5 の最後）: 終端 event を失ったまま同じ id の viewport を閉じて
   開き直したとき composing が残るかをテストで確かめる。残るなら、viewport の破棄・focus 喪失で composing を落とす修正の
   範囲を報告する（小さければ実装してよい）。
7. **文書**: CLAUDE.md「IME 対応」節、`src/ime_focus.rs` / `crates/egui-ime` の doc、`App::ime_input_active` の doc、
   関連 docs（keymap-spec 等、段 2a で触れた箇所）を新しい規則に合わせる。計画書 I2 の段 2b に実施記録を短く追記。

## 2. テスト（設計の項目 5・2a の列挙どおり。抜粋）

- grace 前提のテストを「composing 中は抑止、Commit 後の同じ pass／次の pass の通常キーは通る」へ直す。
- 代表ダイアログ（タグ編集・フォルダを開く・パスワード）で「確定→すぐ Enter で決定」「確定→すぐ Esc で閉じる」。
- 検索欄・アドレス欄（`ui_main.rs` 15377 / 15550 付近）の実 handler で、`[Commit, Disabled, 通常 Enter]` の同じ pass と
  次 pass の両方で決定され focus が手放されること。
- focus 復帰: 復帰（Alt+J / `Disabled`）→ 次 pass の通常 Enter / 通常 Esc / 他の widget の focus 取得、を
  `focus_request` を渡す欄と渡さない欄の両方で。呼び出し側の初回 focus 要求が効くこと。**欄が retry を使う前に
  消えた場合に、後で開き直したときへ印が持ち越されないこと**（設計レビューの追加指摘）。
- 既存の `ime_alt_focus_loss_keeps_bookmark_editor_ownership_and_blocks_panel_toggle`、
  `focused_bookmark_title_escape_does_not_close_fullscreen`、`crates/egui-ime/src/tests.rs` 581 付近の
  「変換中の通常 Esc で focus を保つ」は期待を変えずに通ること。
- native presenter のタグ選択欄: 前 pass composing・今 pass `[Commit, Disabled, 通常 Enter]` と、次 pass の通常 Enter。
- Win32 queue と native 生キー: 0xE5 の press / repeat は捨て release は残す。JIS ^ @ ¥ ろ の scan-code 照合に届かない。
- ラボ（musicvideo-ui）の IME テストと履歴ショートカットのテストも新しい規則で通ること。

## 3. 完了条件

1. `cargo fmt --check`、`cargo test -p egui-winit --lib`、`cargo test -p egui-ime`、`cargo test -p musicvideo-ui -p musicvideo_lab`、
   `cargo test -p comic_lab`、`cargo test -p mimageviewer --lib ime`、`… --lib keyboard`、`… --lib focus`、`… --lib text_mode`、
   変更した handler のテスト、`cargo check -p mimageviewer --bin mimageviewer-core`。
2. `.\scripts\test-full.ps1` を走らせ、既知の App サイズ制限テスト失敗（master 既存、I2 と無関係）と既知の
   capture_region の stack overflow 以外に失敗が無いことを確認する（段 2a と同じ補完手順でよい）。
3. `$env:MSBUILDDISABLENODEREUSE='1'` で `.\scripts\build-dev.ps1 -PreserveRuntime` と `cargo build --release -p musicvideo_lab`。
   起動中の dev core があれば止められないことがあるので、その場合は報告する。PowerShell スクリプトを呼ぶときは `*>&1` を付けない。
4. まれな失敗は割り切る（CLAUDE.md）。状態を足さない（設計項目 7）。

## 4. 報告

変更点、前提と食い違った点、テスト結果（コマンドと件数）、`git diff --stat`。短く。
コミット・アプリ起動はしないでください。自分で「独立レビュー」を名乗る確認は不要です（正式なレビューは設計 lead が別に依頼します）。
