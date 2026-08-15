# 実装ブリーフ: 履歴クリア系を操作カスタマイズから割り当て可能にする (backlog §4.3)

対象: v3.0.1 (明日夕方リリース予定)。背景は `docs/next-release-backlog.md` の §4.3。

**着手前に読むこと**: `docs/keymap-spec.md`、`docs/key-customization-impl-plan.md`、
`docs/ring-keyaction-parity.md`。

## 1. 要望

5ch 専用スレ #208 (2026-08-11)。フォルダバーの設定メニューにある

1. **最近開いたフォルダ履歴をクリア**
2. **A/B の記憶した場所をクリア**

を 1 キーで呼び出したい。#210 で「次のバージョンで追加する」と回答したが **v3.0.0 で対応が漏れ**、
#230 で再指摘された。今回必ず入れる。

## 2. いまの実装 (事実)

- メニュー項目は `src/ui_main.rs:9148` / `9152`。押すと `clear_recent_folders()` /
  `clear_quick_folder_slots()` を呼び、続けて `self.settings.save()` している。
- 実体は `src/app.rs:16366` (`clear_recent_folders`) と `src/app.rs:16283`
  (`clear_quick_folder_slots`)。**どちらも呼び出し元はこのメニューだけ** (他に本番経路は無い)。
- どちらも現在 **トーストは出していない**。
- KeyAction / RingActionId には対応が無く、キーからもリングからも呼べない。

## 3. やること

### 3.1 共有の入口を 1 つにする

メニュー・キー・リング・ジェスチャで挙動がずれないよう、**「クリアして保存してトーストを出す」
までを 1 つのメソッドにまとめ**、メニューもそれを呼ぶ形にする。既存の
`clear_recent_folders` / `clear_quick_folder_slots` は状態を触るだけの下位関数として残してよい。

- 保存 (`settings.save()`) を各呼び出し側に書かない。共有メソッドの中に入れる。
- トーストは `show_feedback_toast` を使う。文言は周辺の既存トーストの書き方に合わせる。
- **確認ダイアログは追加しない。** 既存メニューに確認が無いので、キー実行だけ確認を増やすと
  操作感が変わる (backlog の方針)。ファイル削除のような破壊的操作ではない。

### 3.2 KeyAction を 2 つ追加する

名前 (提案): `GridClearRecentFolders` / `GridClearQuickFolderSlots`。
context は `Grid`、trigger は同カテゴリの `GridOpenLocation*` と同じ扱い (押下 1 回・リピート無し)。

**既定キーは割り当てない** (`default_chords` は空)。利用者が操作カスタマイズで割り当てる。

`GridOpenLocationReadingHistory` を雛形にすると、触るべき場所が全部たどれる:

| ファイル | 何を足すか |
| --- | --- |
| `src/keymap.rs` | enum variant / `ALL_ACTIONS` / `ini_name` / `label` / `context` / `trigger` / `default_chords` |
| `docs/keymap.ini.default` | `Keymap::default_reference_ini()` の出力と一致させる (`bundled_keymap_default_matches_generated_reference` が検証) |

`MenuCommandSpec` は**不要** (これは上部メニューバー用で、対象はフォルダバーの ▼ メニュー)。

### 3.3 RingActionId を 2 つ追加する

要望が「キー / マウスジェスチャ / リングショートカット等から」なので、リング側にも出す。
名前 (提案): `ClearRecentFolders` / `ClearQuickFolderSlots`。

| ファイル | 何を足すか |
| --- | --- |
| `src/ring_shortcut.rs` | enum variant / グループ判定 (883 付近) / カタログ列挙 (897 付近) / `ini_name` (977 付近) / パース (1074 付近) / 表示ラベル (1233 付近) |
| `src/app/gamepad_input.rs` | `KeyAction` → `RingActionId` 対応 (111 付近) / `RingActionId` の実行 dispatch (4464 付近) / 文脈による可否 (4714 付近) |
| `src/ui_dialogs/preferences/pages.rs` | `KeyAction` → `RingActionId` 対応 (1425 付近) |
| `src/keymap.rs` の parity テスト | `ring_actions_are_classified_for_key_action_parity` の `key_handled` に登録 (8633 付近) |

### 3.4 実行できる文脈

一覧 (グリッド) 側の操作なので、フルスクリーンや動画再生中には出さない。既存の
`GridOpenLocation*` と同じ扱いにする。**判断が分かれる場合は、既存の同種アクションに合わせる**。

## 4. テスト

- `src/app/tests.rs` に既にある `clear_recent_folders_updates_session_and_settings` を土台に、
  **共有メソッド経由で保存とトーストまで起きること**を確認する。A/B 側にも同等のものを置く。
- keymap の網羅テスト (ALL_ACTIONS / enum / ini 往復) は既存が自動で拾うはずだが、落ちたら直す。
- parity テストが通ること。
- 実行: `cargo test -p mimageviewer --lib keymap::`、`cargo test -p mimageviewer --lib app::`、
  `cargo test -p mimageviewer --lib ring_shortcut::`。
- `cargo fmt --all` をかけてから終える (pre-commit フックが `--check` で弾く)。
- UI 文言を足したら `python scripts/check_ui_glyphs.py`。

## 5. ドキュメント

- `docs/keymap.ini.default` — 上記のとおり生成物と一致させる。
- `docs/keymap-spec.md` — アクション一覧を持っているなら追記。
- `docs/ring-keyaction-parity.md` — 対応表に 2 件追加する (集計の件数も更新)。
- `htdocs/mimageviewer/manual/` — 操作カスタマイズのページと、フォルダバーを説明している
  ページに「キーやリングにも割り当てられる」ことを書く。**既定キーは無い**ことも明記する。
  実装語を出さない (KeyAction / RingActionId 等)。バージョン番号も書かない
  (CLAUDE.md「マニュアル・製品ページの記述方針」)。

## 6. 対象外

- 既定キーの割り当て。
- 確認ダイアログの追加。
- 「閲覧履歴をすべてクリア」(環境設定側、`src/ui_dialogs/preferences/pages.rs:7589`) は
  **今回の対象ではない**。要望は フォルダバーの 2 件。
- `quick_folder_drive_current_dirs` と `recent_folders` のどこまでを消すかの再定義。
  **既存メニューの挙動をそのまま共有する** (今回は入口を増やすだけで、消す範囲は変えない)。

## 7. 進め方

- 変更は 1 コミットにまとめてよい。ドキュメントとマニュアルも同じコミットに含める。
- 途中で「これは §4.3 の範囲を超える」と判断したら、症状パッチを入れずに報告する。
- `docs/next-release-backlog.md` は**触らないこと** (別セッションが並行で編集している)。
  §4.3 の削除はこちらで行う。
