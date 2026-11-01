# ブリーフ: I2 段 1 — IME が処理したキーの到着を記録する（挙動は変えない）

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
担当: 実装・テスト = Codex GPT-6.1 Sol / high。検収 = ClaudeCode Opus。独立レビュー = 別の GPT-6.1 Sol / xhigh。

---

## 0. 背景

利用者報告（2026-10-14、開発版と v4.6.0 ポータブル版の両方で利用者が確認）: 画像注釈のテキスト本文欄で
日本語を入力し、Enter をすばやく 2 回押すと、確定だけされて改行が入らない。間を空ければ改行が入る。

直接の原因は `src/ui_fullscreen.rs` の 24555 行付近（`f8ff63911`、2026-06-06）で、テキスト注釈モード中は
`ime_input_active`（変換中、または直近 300ms に IME イベント）の間、`Key::Enter` を**すべて**捨てている。
2 回目の Enter も 300ms 以内なので捨てられる。

その対処が必要になった根の候補（コードと公開資料からの見立て。**実機では未確認**）:

1. Windows は IME が処理したキーの virtual key を `VK_PROCESSKEY` に置き換えて届ける。
2. winit 0.30.13 は `VK_PROCESSKEY` を論理キー `NamedKey::Process` として報告し、物理キーは元のキー
   （Enter 等）のまま（`winit-0.30.13/src/platform_impl/windows/keyboard_layout.rs:946`）。
3. egui-winit 0.33.3 の `on_keyboard_input` は `logical_key.or(physical_key)`（`src/lib.rs:815` 付近）で、
   `Process` に当たる egui キーが無いため物理キーへ fallback し、通常の `Key::Enter` press を egui へ渡す。

同じ経路で、変換中の Esc（`egui-ime` の Esc 除去）、候補選択中の Tab（helper の Tab ロック）、
変換中の Backspace / 矢印（egui の `remove_ime_incompatible_events`）も説明できる可能性がある。

**段 1 の目的は、この見立てを利用者の実機・実 IME のログで確かめること。** 挙動は一切変えない。
修正（段 2）はログの結果を見てから設計し、独立レビューを通す。

---

## 1. やること

1. **egui-winit 0.33.3 を `vendor/egui-winit` に取り込み、root `Cargo.toml` の `[patch.crates-io]` で使う。**
   既存の `vendor/egui` / `vendor/eframe` / `vendor/egui-wgpu` と同じ方式（`vendor/egui/PATCHES.md` を参照）:
   registry の `.crate` の SHA-256 を `Cargo.lock` と照合、原本との差分を記録する `PATCHES.md` を置く、
   `.cargo-ok` と `target/` はコピーしない。`.gitignore` で追跡対象になることを確認する。
   本体・ラボ・その他 workspace メンバーが同じ egui-winit を使うこと（`cargo tree -d` で egui-winit が 1 系統）。

2. **記録（trace）を追加する。既定では完全に無効で、挙動・性能を変えない。**
   - 有効化: 環境変数 `MIV_IME_KEY_TRACE=<出力ファイルの絶対パス>`。起動時に 1 回だけ読む。未設定なら何もしない
     （毎イベントで環境変数を読まない）。
   - 記録する事象（1 行 1 事象、追記、行ごとに flush）:
     - winit の `WindowEvent::KeyboardInput`: 単調時刻（ms、起動からの経過）、`ViewportId` または window id、
       state（pressed / released）、repeat、`is_synthetic`、winit の `logical_key`、`physical_key`、`text`、
       そして **egui-winit が egui へ渡した結果**（渡した `egui::Key` と、`Cut/Copy/Paste/Text` を出したか、
       何も出さなかったか）。
     - winit の `WindowEvent::Ime`: 時刻、種類（Enabled / Preedit(長さと cursor の有無) / Commit(長さ) / Disabled）、
       egui-winit が出した `ImeEvent`。
   - **入力した文字の中身を記録しない**（Preedit / Commit / text は長さ・改行かどうか・空かどうかだけ）。
     パスワード欄など利用者の秘密が混ざるため。キーの種類（Enter / Escape / Tab / Process 等）は記録してよい。
   - 書き込み失敗は黙って止める（以後は記録しない）。まれな失敗は割り切る（CLAUDE.md）。
   - trace のコードは vendored egui-winit の中に閉じ、`PATCHES.md` に「診断用・段 2 で扱いを決める」と記録する。

3. **egui-winit の挙動は変えない**（段 1 は記録だけ）。trace 無効時に原本と同じ events が出ることを、
   既存テストと、可能なら egui-winit の単体テスト（KeyEvent を合成できる範囲で）で確かめる。

---

## 2. 完了条件

1. `cargo tree -d` で egui-winit が 1 系統、`Cargo.lock` の egui-winit が path 依存に置き換わっている。
2. `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo test -p musicvideo-ui -p musicvideo_lab -p egui-ime`、
   `cargo test -p mimageviewer --lib ime_focus egui_focus_policy` が緑。
3. `cargo fmt --check`。
4. `.\scripts\build-dev.ps1` と `cargo build --release -p musicvideo_lab` が成功（利用者が実機で記録を取るため）。
   PowerShell スクリプトをツールから呼ぶときは `*>&1` を付けない。
5. 計画書（docs/music-video-export-plan.md）の D10「設定・入力」の I1 の後に「I2（IME が処理したキー）」として、
   背景・段 1 の内容・段 2 は未設計であることを記録する。

## 3. 報告

- 取り込み方（checksum の照合結果、PATCHES.md の要旨）
- trace の出力形式の例（数行）
- 利用者が記録を取るための PowerShell コマンド例（本体とラボそれぞれ）
- `git diff --stat`

コミット・アプリ起動はしないでください。
