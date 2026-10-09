# ブリーフ: 音楽ビデオ I1 — IME 補正を本体とエディタで共有する

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
正本: [music-video-export-plan.md](music-video-export-plan.md) の D10「設定・入力」と §7 の I1 行。
前段: L1〜L3 完了（独立レビュー ACCEPT）。
担当: 実装・テスト = Codex GPT-6.1 Sol / high。検収 = ClaudeCode Opus。独立レビュー = 別の GPT-6.1 Sol / xhigh。

**この increment は 2 段で進める。** 出荷済みの本体の入力処理（`src/ime_focus.rs`、30 以上のファイルが使用）に
触れるので、CLAUDE.md の「重要な所有境界の設計は実装前に独立レビュー」に従う。

- **段 1（設計）**: §2 の調査をして、計画書に「I1 設計」節を書き、**そこで止めて報告する**。コードは変えない。
- **段 2（実装）**: 設計の独立レビューが通った後に、別途指示する。

---

## 0. 背景

本体の `src/ime_focus.rs` は便利機能ではなく、egui の既知の入力破壊を補正している:

- IME 変換中の Esc を egui が `Focus::begin_pass` で拾い、TextEdit のフォーカスを外してしまう（plugin が Esc press を
  RawInput から除去）
- Windows が空の `Preedit("")` を送らず `Disabled` だけを送る経路で、未確定文字が残る（plugin が空 preedit を補う）
- IME のキー処理由来の一時的な focus loss の復帰、IME 候補選択中の Tab traversal 抑止（single-line helper）

エディタ（`musicvideo-ui` と、それを載せるホスト）の入力欄はこの補正を受けていない。対象の入力欄:
レイヤー名（`lib.rs` の `text_edit_singleline`）、テキストレイヤーの本文、モジュレータ名と Hz 範囲（`modulators.rs`）、
歌詞（独自 `LyricsTextBuffer` の multiline、`lib.rs` の `TextEdit::multiline`）。

`ime_focus` は `pub(crate)` で、`crate::keyboard_input`（keyboard owner）と `crate::logger`（診断ログ）に結びついている。
`musicvideo-ui` は本体 lib に依存しない（D10 / D16 の層分け）。

---

## 1. 方針（計画書 D10 で決定済み）

egui だけに依存する入力補正（plugin）と single-line helper を**共有クレートへ挙動不変で切り出し**、本体と
エディタのホストがそれぞれ policy を登録する。本体固有の部分（keyboard owner の記録、診断ログ、App の
各ビューポート入口での呼び出し）は本体に残す。

---

## 2. 段 1 で調べて設計に書くこと

1. **境界の線引き**: `src/ime_focus.rs` の各要素（`ImeInputPlugin` / `normalize_ime_input` / `apply_ime_event`、
   per-viewport 状態と 300ms grace、`ime_input_active*` 系、focus retry / text focus contract、
   `restore_focus_for_ime_key`、`show_singleline` / `add_singleline*`、診断 `begin_pass_diagnostics` /
   `TextInputKeyDiagnostic`、`record_keyboard_owner` / `record_side_panel_close`）を、共有クレートへ移すもの・
   本体に残すものに分け、理由を書く。`src/egui_focus_policy.rs`（Tab の focus policy）との関係も確認する。
2. **本体固有部分の接続方法**: 共有側から本体の keyboard owner・logger をどう呼ぶか（例: ホストが登録する
   コールバック／trait、共有側は診断の発生だけを通知）。共有側が本体の型を知らない形にする。
3. **本体の挙動不変をどう保証するか**: 既存の `ime_focus` のテスト（`#[cfg(test)] mod tests`）をどこへ移し、
   何を本体に残すか。raw TextEdit 禁止検査（`src/ime_focus.rs` 末尾の allowlist 検査）の扱い。
   本体の呼び出し元（30 以上のファイル）を書き換えずに済む再エクスポートが可能か。
4. **エディタ側の適用**: 
   - single-line の入力欄（レイヤー名、テキスト本文、モジュレータ名、Hz 範囲）は共有 helper 経由にする。
   - 歌詞は multiline の独自 buffer。plugin（Esc / 空 preedit の補正）は context 単位で効くので登録だけで効くか、
     focus 復帰が必要か、既存の buffer・widget ID・native Undo（L1 / L2 で維持してきたもの）を保てるかを確認する。
   - ホスト（ラボ）が plugin を登録する場所と、本体の「各ビューポート入口で `update_ime_state` を呼ぶ」に相当する
     ものがエディタに要るか。
   - 共有 UI の Undo shortcut の抑止（`apply_history_shortcut` の `wants_keyboard_input`）と、本体の
     「IME 変換中は App shortcut を抑止」（`ime_input_active`）の関係。エディタの Ctrl+Z を IME 変換中にも
     抑止すべきか。
   - `musicvideo-ui` にも raw TextEdit 禁止検査を入れるか。
5. **新しいクレートの名前・置き場所・依存**（egui のみ。eframe / 本体 lib に依存しない）。workspace と
   `[patch]` の egui を共有することの確認。
6. **リスクと検証計画**: 本体側で挙動が変わり得る箇所、回帰テストの一覧（本体の既存 IME テスト全件維持、
   エディタ側の新規 handler テスト: IME の Esc 取消・候補選択中の Tab・Enter 確定・フォーカス復帰・Undo 所有）、
   本体の UI snapshot（`cargo test --test ui_snapshot`）と `test-full` の要否、実機確認の手順（利用者に依頼する内容）。
7. **規模の見積もり**と、段 2 を分割すべきか（例: 段 2a = 本体の切り出しのみで挙動不変を確定、段 2b = エディタ適用）。

## 3. 段 1 の制約

- **コードを変更しない**（調査と計画書への追記だけ）。計画書の D10「設定・入力」の下に「I1 設計（2026-10-09 案）」
  として書く。
- 前提がコードと合わない点は、根拠（file:line）を添えて書く。
- アプリを起動しない。コミットしない。

## 4. 段 1 の報告

- 設計の要約（移すもの・残すもの・接続方法・エディタ適用）
- 段 2 の分割案と見積もり
- 前提がコードと合わなかった点
- 計画書の追記箇所
