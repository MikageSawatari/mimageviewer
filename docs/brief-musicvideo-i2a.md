# ブリーフ: I2 段 2a — IME が処理したキーを egui へ渡さない（根の修正）と対症療法の撤去

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
担当: 実装・テスト = Codex GPT-6.1 Sol / high。検収 = ClaudeCode Opus。独立レビュー = 別の GPT-6.1 Sol / xhigh。

**設計の正本は `docs/music-video-export-plan.md` の「#### I2（IME が処理したキー）」の「段 1 の実機記録」と
「段 2a」。** 設計は独立レビューで ACCEPT 済み。まず両節を読み、前提をコードで確かめること。
前提と食い違うものを見つけたら、黙って直さずに報告で返すこと（設計判断が必要なときだけ止まってよい）。

---

## 0. 要点

- 実機記録（Google 日本語入力）で、IME が処理した Enter / Esc / Tab / Space / 文字キーは winit から
  `logical=NamedKey::Process`, physical=実キー の press で届き、egui-winit が physical へ fallback して
  通常の `Event::Key` press を egui へ渡していることを確認した。release は実キーの logical で届く。
- その結果、本体の注釈本文は 300ms の Enter 除去で「確定→すぐ Enter」の改行を捨て（利用者報告）、
  ラボの歌詞欄は確定の Enter だけで改行が入る。

## 1. やること

1. **vendored egui-winit（`vendor/egui-winit/src/lib.rs` の `on_keyboard_input`）で、`logical == NamedKey::Process`
   の press を egui の `Event::Key` / `Text` / `Cut` / `Copy` / `Paste` のどれにもしない。** repeat も同じ。
   判定は Cut/Copy/Paste 判定より前。release はそのまま渡す（追跡集合などの状態は持たない）。
   `EventResponse` の計算と `Ime` イベントの扱いは変えない。判定は「winit のキー情報 → 渡す／渡さない」の
   純関数に切り出して単体テストする（`KeyEvent` は外部から合成できないため）。
   `PATCHES.md` に、段 1 の診断に加えて**挙動を変える変更**として理由・根拠（実機記録・native presenter の
   `VK_PROCESSKEY` 規則と一致）を記録する。trace は残す（既定無効）。trace の出力で、Process の press に対して
   egui へ何も渡していないことが分かる形のままにする。
2. **対症療法の撤去**（計画書の段 2a 2. の一覧どおり）:
   - `src/ui_fullscreen.rs` の text_mode 中の Enter 除去（`f8ff63911`、24555 行付近）。
   - `tools/comic_lab` の `consume_ime_enter` と呼び出し、専用の `ime_composing` field と初期化。
   - `crates/egui-ime` の `normalize_ime_input` の「変換中の Esc press を除く」処理。**空 Preedit の補いは残す**。
   - `crates/egui-ime` の helper focus lock filter の `tab: ime_active`（矢印の lock は残す）。
   - 残すもの（触らない）: egui 本家の `remove_ime_incompatible_events`、アプリ全体の Tab 方針、focus 復帰、
     helper の `return_key(None)`、300ms grace（`IME_EVENT_GRACE`）と `ime_input_active` / `dialog_*_pressed`。
     grace の見直しは段 2b で別に設計する。
   - 撤去したものを説明しているコメント・`CLAUDE.md` の「IME 対応」節・`src/ime_focus.rs` の doc・関連 docs の記述を、
     新しい規則（IME が処理したキーは egui-winit が渡さない）に合わせて直す。grace の説明は段 2b まで残す。
3. **テスト**（計画書の段 2a 3. どおり）:
   - egui-winit の判定の単体テスト。vendored egui-winit は workspace 外なので、`scripts/test-full.ps1` の追加実行対象に
     その単体テスト（`cargo test --manifest-path vendor/egui-winit/Cargo.toml --lib` 相当、既存の書き方に合わせる）を入れる。
   - IME の Enter / Esc / Tab を通常の `Key` press として流している既存テストは、実機記録どおり
     （press なし・release あり・Ime 列）へ直す。ラボの
     `raw_lyrics_enter_keeps_native_newline_semantics_and_commits_only_once` は確定 Enter で改行が入らない期待へ。
   - 本体の注釈本文（`src/ui_text.rs` の本文 TextEdit を text_mode の fullscreen 経路で）で「確定→すぐ Enter で改行が入る」
     ことを handler-level で固定する。Commit・release・直後の通常 Enter が同じ pass の場合と別 pass の場合の両方。
   - 通常の Enter / Esc / Tab、IME 以外の focus 復帰、clipboard、非ラテン配列の physical fallback のテストは残す。
     IME 中という理由だけで通常 press のテストを書き換えない。
   - 撤去した Esc 除去・Tab lock を固定していたテストは、根の修正後に同じ利用者挙動
     （変換中 Esc で変換だけ取消・欄の focus は残る、候補選択中 Tab で focus が移らない）を、
     新しい入力モデルで固定するテストへ置き換える。

## 2. 完了条件

1. `cargo fmt --check`、egui-winit 単体テスト、`cargo test -p egui-ime`、`cargo test -p musicvideo-ui -p musicvideo_lab`、
   `cargo check -p comic_lab`（テストがあれば test）、`cargo test -p mimageviewer --lib ime_focus egui_focus_policy`、
   本体 lib の IME 関連テスト（`ime` を含む名前、`text_mode`、注釈本文の新テスト）。
2. `cargo check -p mimageviewer --bin mimageviewer-core`。
3. `.\scripts\build-dev.ps1 -PreserveRuntime`（`$env:MSBUILDDISABLENODEREUSE='1'`）と `cargo build --release -p musicvideo_lab`。
   build-dev は同じ worktree の dev core を止めることがある。設計 lead が隔離データで起動中の core（trace 記録用）は
   止めてよい。PowerShell スクリプトをツールから呼ぶときは `*>&1` を付けない。
4. `test-full.ps1` は App のサイズ制限テストが master 既存の理由で失敗する（I2 と無関係、別作業）。走らせる場合も
   それ以外の失敗が無いことの確認に使い、App の構造やテスト上限には触れない。
5. 計画書の I2 段 2a に実施内容（撤去したもの・残したもの・テスト）を短く追記する。

## 3. 報告

- 変更点（根の修正・撤去・テスト）と、前提と食い違った点があればその内容
- テスト結果（コマンドと件数）
- `git diff --stat`

コミット・アプリ起動はしないでください。
