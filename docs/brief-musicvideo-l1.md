# ブリーフ: 音楽ビデオ L1 — エディタの画面構成

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
正本: [music-video-export-plan.md](music-video-export-plan.md) の **D14**（画面構成）と §7 の L1 行。
前段: P4a（設計の独立レビュー）完了、`3d2d567bc`。
担当: 実装・テスト = Codex GPT-6.1 Sol / high。検収 = ClaudeCode Opus。独立レビュー = 別の GPT-6.1 Sol / xhigh。

---

## 0. 何をするか

いまの編集 UI は、表示ゲイン・モジュレータ・レイヤー一覧・並べ替え・選択レイヤーのプロパティが、
幅 330px の右パネル 1 本のスクロールに縦積みされている（`crates/musicvideo-ui/src/lib.rs` の
`draw_editor`）。レイヤーが増えると縦に長くなり操作しにくい（2026-10-09 ユーザー報告）。
これを次の配置に作り直す。**機能は増やさない。置き場所を変えるだけ**。

| 位置 | 中身 |
| --- | --- |
| 上 | 既存のツールバー（プロジェクトを開く・保存、素材、再生操作など。ラボ側）。Undo / Redo ボタンもここへ移してよい |
| **左** | レイヤー一覧（常時表示）。追加メニュー・削除・奥／手前への並べ替えもここ |
| 中央 | プレビュー（既存） |
| **右** | タブで切り替える: 「レイヤー」= 選択中レイヤーのプロパティだけ / 「モジュレータ」= 既存のモジュレータ編集 / 「プロジェクト」= 表示ゲインなどプロジェクト全体の設定 |
| 下 | タイムライン（既存）。**横幅いっぱい**に置く（キーフレームの位置合わせの精度のため） |

左右パネルは幅を変えられるようにする（既定は左 220px 前後、右 360px 前後。値は任せる）。

---

## 1. 絶対に守ること

1. **履歴を確定する単位を分割しない**（D14 の後半。独立レビューの P2 指摘）。
   現在の `draw_editor` は描画だけでなく、次を所有している:
   - `restore_pending_text_edit_states` / `reconcile_preserving_native_reachability`
   - `apply_history_shortcut`（Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z）
   - `begin_external_edit` の checkpoint と、最後の `observe_ui_pass(EditSurface::Properties, ...)`
   - `draw_layer_delete_confirmation`
   これを領域関数ごとに複製しない。左・右・各タブは **1 つの Properties 編集 transaction** として、
   checkpoint 1 回・観測 1 回にまとめる。shortcut と復元処理は **1 フレームに 1 回、どの領域を描くより前に**
   行う（タイムラインを先に描く配置になっても、shortcut がタイムライン入力より後に処理されないように）。
   タイムラインは既存どおり別 surface（`EditSurface::Timeline`）のまま。
2. **非同期完了の採否は全領域の入力の後**。ラボの `update` は、エディタとタイムラインの入力を反映してから
   `poll_projects` / `poll_lrc` / `sync_runtime_resources` を呼んでいる（app.rs のコメント
   "Project/LRC completion can replace the document..."）。左・右・下のすべての入力と `note_edit` の後に
   これらが来る順序を保つ。
3. **`musicvideo-ui` は egui と `musicvideo-core` / `music-core` だけに依存**（eframe / cpal / ffmpeg を入れない）。
   P4 の方式は別 exe になったが、この層分けは維持する。
4. **見た目と操作の変更は配置だけ**。プロパティの各編集部品（`edit_layer_properties` 以下）、
   EditorCommand、選択の扱い、歌詞編集（`LyricsTextBuffer` と native Undo）は中身を変えない。
5. 既存テスト（`musicvideo-core` / `musicvideo-ui` / `musicvideo_lab`）を全件維持する。
6. `src/`（本体）・`crates/comic-core`・`crates/music-core` を触らない。
7. アプリ（ラボ・本体）を起動しない。ビルドとテストまで。
8. `cargo fmt` を通す。**コミットはしない**（検収後にこちらで行う）。

---

## 2. 形の目安（前提をコードで確かめ、合わなければ根拠を添えて返すこと）

`draw_editor` を、ホストがパネルを並べられる「1 フレーム分の編集セッション」に分ける。例:

```rust
// 1 フレームの最初に 1 回（どのパネルより前）
let mut frame = editor_state.begin_frame(ctx, &mut project);   // 復元・reconcile・shortcut・checkpoint
// ホストが好きな順でパネルを置く
egui::TopBottomPanel::bottom(..).show(ctx, |ui| draw_timeline(ui, ...));      // 既存の別 surface
egui::SidePanel::left(..).show(ctx, |ui| frame.layer_list(ui, &mut project));
egui::SidePanel::right(..).show(ctx, |ui| frame.inspector(ui, &mut project, time, values, calibrations));
let response = frame.finish(ctx, &mut project);                // 削除確認・observe 1 回・EditorResponse
// この後に poll_projects / poll_lrc / sync_runtime_resources、最後に CentralPanel（プレビュー）
```

- 借用の都合で別の形が自然ならそれでよい。要件は §1-1 / §1-2 を満たすこと。
- 既存の `draw_editor` は残さない（ホストは 1 つ。使われない旧入口を残すと 2 経路になる）。
  テストが `draw_editor` を直接呼んでいる箇所は新しい入口に移す。
- 右パネルのタブの選択は UI 状態（`EditorState`）に持ち、Undo/Redo の対象にしない。
  プロジェクトの保存内容にも入れない。
- レイヤー一覧で選択を変えたとき右パネルが「レイヤー」タブ以外なら、タブは切り替えない
  （利用者が開いたタブを勝手に変えない）。ただしレイヤー追加の直後は「レイヤー」タブを開く。
- egui 0.33 の `SidePanel` 内で縦に伸ばすスクロールは、CLAUDE.md「パネル (Area + Frame::popup)」と
  同じ注意（親領域の高さ確保、`auto_shrink([false, false])`）に従う。

---

## 3. テスト（D14 の回帰対象。handler / 状態遷移のテストで固定する）

既存の `musicvideo-ui` の egui テスト（`lib.rs` の tests）と同じやり方で、実際の入力を通して確かめる。

1. **領域をまたぐ連続編集**: 左でレイヤーを並べ替え → 右でスライダーをドラッグ → release。
   Undo の段数と各段の戻り先が、旧 `draw_editor` 1 本のときと同じ。
2. **ドラッグ中の領域移動**: 右のスライダーをドラッグしたまま（pointer down のまま）左のパネル上を通過して release
   → 1 段にまとまる。
3. **タブ切替**: プロパティ編集 → タブ切替 → モジュレータ編集。タブ切替自体は履歴に入らない。
   Undo で両方の編集が個別に戻る。
4. **歌詞の native Undo**: 歌詞欄にフォーカスがある間の Ctrl+Z は TextEdit 側が持ち、履歴が動かない
   （既存テストを新しい配置で維持）。
5. **Undo/Redo 後の選択復元**: 左の一覧の選択と右のタブ内容が復元後のレイヤーを指す。
6. **入力と非同期完了が同じフレーム**: 既存の「完了より先に入力を反映する」テストを新しい配置で維持
   （ラボ側のテスト）。
7. **shortcut の順序**: タイムラインのパネルを先に描く配置でも、同じフレームの Ctrl+Z が 1 回だけ処理される。

---

## 4. 完了条件

1. `cargo test -p musicvideo-core` / `-p musicvideo-ui` / `-p musicvideo_lab` が緑（§3 の新規テストを含む）。
2. `cargo tree -p musicvideo-ui` に eframe / cpal / ffmpeg が出ない。
3. `cargo build --release -p musicvideo_lab` が通る（ユーザーが新しい配置を触るため）。
4. `cargo fmt --check` が通る。
5. 計画書 §7 の L1 行に結果（テスト数・追加テスト）を追記する。

## 5. 検証コマンド

```
cargo test -p musicvideo-core
cargo test -p musicvideo-ui
cargo test -p musicvideo_lab
cargo tree -p musicvideo-ui
cargo build --release -p musicvideo_lab
cargo fmt
```

## 6. 報告してほしいこと

- 新しい入口の形（公開 API）と、§1-1 の「1 transaction・shortcut は最初に 1 回」をどう保証したか
- パネルの描画順と、`poll_*` との順序
- 追加したテストの一覧と、各テストを**わざと壊したら落ちる**ことを確かめたか
  （例: observe を領域ごとに呼ぶ実装に変えると 1 / 2 が落ちる）
- 前提がコードと合わなかった点（あれば）
- `git diff --stat`
