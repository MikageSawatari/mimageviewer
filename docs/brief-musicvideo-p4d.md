# 実装 brief: P4d mIV からの入口「音楽ビデオを作成」（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high（実装・テスト）。設計・検収: ClaudeCode Opus 5.5。独立レビューは別担当。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。報告は日本語。

## 1. 正本と設計判断

正本は `docs/music-video-export-plan.md` の **D10**（「起動と引数」: mIV → エディタは起動引数だけ、絶対パス、`--data-dir` は mIV が
使っている data dir、spawn は worker、失敗は理由付きで通知）と §7 の **P4d** 行。以下は設計担当が決めた具体化（2026-10-14）。

- **項目**: 右クリックメニューに静的な項目「音楽ビデオを作成」を追加する（`MenuCommand` と `ContextMenuItemId` を足す。
  `stable_name`・`label`・`ALL`・`from_command`・既定の並び順）。保存済みの並び順に無い新しい項目は既存の
  `resolved_order` が既定の位置へ入れる（`missing_catalog_items_keep_their_canonical_slots_without_reordering_known_ids`）ので、
  移行処理は要らない。カスタマイズで非表示にできる。
- **対象**: `GridItem::Audio`（実ファイル）だけ。サーフェスは一覧（Grid）と全画面（Fullscreen。音楽ビューの右クリックは
  この共有メニュー）。複数選択時は表示しない（または無効）。動画ファイル・動画→音声モードは対象外（今回は出さない）。
  アーカイブ内の音声は存在しない（`GridItem::Audio` は実ファイルのみ）。
- **起動**: 既存の外部ツール起動と同じく worker スレッドで行う（`external_tool::start_launch_worker` のパターン）。
  UI スレッドは exe の存在確認も spawn もしない。exe は core と同じディレクトリの `mimageviewer-musicvideo.exe`
  （`current_exe()` の兄弟。EPUB worker・remote service と同じ解決）。引数は `--audio <絶対パス>` と
  `--data-dir <絶対パス>`。どちらも `std::path::absolute`（I/O なし）で絶対化する（`data_dir::init` は `--data-dir` を
  絶対化しないため）。子プロセスは待たない（切り離す）。GUI subsystem なのでコンソールは出ない。
- **失敗**: exe が無い・起動に失敗した場合は `App::show_feedback_toast` 系で理由を出す（例:「音楽ビデオエディタが
  見つかりません: <パス>」）。成功時の通知は不要。連打で複数起動しても止めない（D10: 複数のエディタは許可）。
- **keymap**: 右クリックメニューだけの操作で、キー割り当ては持たない（`docs/keymap-spec.md`:74 の方針どおり。
  文書の更新は P4f）。
- **開発ビルド**: 利用者が実機で確かめられるよう、`scripts/build-dev.ps1` がエディタも同じ `target/dev-runtime` へビルドする
  ようにする（ASCII のみ。既存の core / remote / EPUB worker と同じ扱い）。release・launcher・portable・署名は P4e。

## 2. 作業範囲

1. `context_menu_model.rs` に項目を追加し、`ContextMenuItemKind::Audio` かつ Grid / Fullscreen のときだけ出す。
2. `ui_dialogs/context_menu.rs` の dispatch に処理を追加する（入力ゲートの許可リストの扱いも既存項目に合わせる）。
3. 起動 worker と結果の受け取り（失敗の通知）。既存の外部ツールの worker / poll の形に合わせる。新しい種類の pending を
   増やす場合は、取消・終了時の扱いを既存と同じにする。
4. `build-dev.ps1` の変更。
5. 設計書に「P4d 実装記録」を追記し、§7 の P4d 行を更新する。

範囲外: 配布（P4e）、マニュアル・製品ページ・keymap-spec（P4f）。

## 3. テスト

- メニューのモデル: Audio の実ファイルでは Grid / Fullscreen に出る。画像・動画・フォルダ・アーカイブ・PDF・仮想項目・複数選択・
  フォルダ文脈では出ない。保存済みの並び順（新しい項目を含まない）に入れても既存の項目の順番が変わらない。非表示にできる。
- dispatch: コマンドが起動要求になり、UI スレッドで I/O をしない（起動関数は差し替え口でテストする）。
- 引数: `--audio` と `--data-dir` が絶対パスで渡る（相対の data dir を渡した場合も絶対化される）。
- 失敗: exe が無い・spawn 失敗で理由付きの通知が出る。
- 既存のメニュー・外部ツール・context menu のテストと UI snapshot がすべて通る（見た目が変わる snapshot があれば
  `docs/ui-snapshot-policy.md` に従って更新し、差分を報告する）。
- 各新テストが、意図的に壊した実装で落ちることを確かめて記録する。
- 確認コマンド（`MSBUILDDISABLENODEREUSE=1`）:
  ```
  cargo test -p mimageviewer --lib context_menu
  cargo test -p mimageviewer --lib external_tool
  cargo test -p mimageviewer --lib
  cargo test -p musicvideo-editor
  cargo fmt --check
  python scripts/check_ui_glyphs.py
  .\scripts\build-dev.ps1
  ```
  `build-dev.ps1` はエージェントのツールから `*>&1` を付けずに呼ぶ。生成物を起動しない。

## 4. 注意

- 本体アプリもエディタ GUI も起動しない。`%APPDATA%\mimageviewer` に触れない。コミットしない。`cargo fmt` はかける。
- 本体はリリース済みの製品。既存のメニュー項目・並び順・外部ツールの挙動を変えない。
- 緑の区切りで止めて報告しなくてよい。止めるのは、設計判断が要るときと前提の矛盾を見つけたときだけ。
- 自分の確認を独立レビューと記録に書かない。

## 5. 報告

変更ファイル、追加テストと失敗確認、全テスト件数、snapshot の変化、正本との相違（あれば）を日本語で。
