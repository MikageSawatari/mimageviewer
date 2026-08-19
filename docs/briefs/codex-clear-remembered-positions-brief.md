# backlog §1.98 — A/B の場所と、記憶した一覧位置を 1 操作でまとめて消す

対象: [next-release-backlog.md](../next-release-backlog.md) §1.98 (専用スレ >>262)。
**v3.1.2 に入れる (利用者判断 2026-08-19)。直近で追加した機能なので、挙動変更は早いうちに。**

## 0. 現状 (確認済み。調べ直さなくてよい)

- 消す入口は**既に 1 本に集約されている**。
  `execute_clear_quick_folder_slots` ([app.rs:16627](../../src/app.rs:16627)) を、
  フォルダバーのメニュー ([ui_main.rs:9225](../../src/ui_main.rs:9225)) と ring dispatch
  ([gamepad_input.rs:4810](../../src/app/gamepad_input.rs:4810)) が呼ぶ。キー操作
  (`GridClearQuickFolderSlots`) は `apply_history_clear_key_action` 経由で ring dispatch に
  合流するので、**新しい共通関数を作る必要はない。既存の funnel を広げるだけ。**
- 今クリアしているのは `quick_folder_workspaces[2]` だけ
  ([app.rs:16618](../../src/app.rs:16618))。**報告された「フォルダへ入り直すとカーソル位置が
  戻る」は現行仕様どおり**で、位置は別の場所に入っている。
- 一覧のスクロール位置とカーソル位置は A/B 別ではなく、セッション共通の
  `folder_history: HashMap<PathBuf, (f32, Option<usize>)>`
  ([app.rs:10189](../../src/app.rs:10189))。**設定には保存されず、終了時にだけ消える。**

## 1. やること

### 1.1 消す範囲を広げる

同じ 1 操作で、`quick_folder_workspaces` に加えて **`folder_history` 全体**を消す。

### 1.2 現在の一覧を先頭へ戻す

消しただけでは、いま開いているフォルダの位置は画面に残る。操作の意味を
「位置を忘れる」に揃えるため、現在の一覧も先頭へ戻す:

- スクロール位置を先頭へ。
- カーソルを**先頭の可視項目**へ移す。**可視項目が無ければ未選択**にする。
- **保留中の scroll 指示と、クリック範囲選択の anchor も破棄する。** これを残すと、
  操作直後のフレームで古い位置へ戻ったり、次の Shift+クリックが消えたはずの起点を
  使ったりする。

### 1.3 触らないもの

チェック済み項目、レーティング / タグ、本の読書位置、動画・音声の再生位置、ブックマーク。
**この操作は「場所の記憶」だけを対象にする。**

### 1.4 名前を意味に合わせる

表示だけを変え、**永続名は変えない**。

| 種別 | 変える | 変えない |
| --- | --- | --- |
| `KeyAction` | 表示名 ([keymap.rs:3991](../../src/keymap.rs:3991) 「A/B の記憶した場所をクリアする」) | `ini_name()` = `GridClearQuickFolderSlots` |
| `RingActionId` | 表示名 ([ring_shortcut.rs:1189](../../src/ring_shortcut.rs:1189)) | 永続名 `clear_quick_folder_slots` |
| toast | [app.rs:16630](../../src/app.rs:16630) | — |
| フォルダバーのメニュー文言 | [ui_main.rs](../../src/ui_main.rs) の該当箇所 | — |

**`ini_name` / ring の永続名を変えると利用者の `keymap.ini` と設定が壊れる。**
既定キーは引き続き未割り当てのまま。

## 2. やらないこと

- 複数コマンド割り当て機能を追加しない。
- A/B ごとに位置を分ける設計変更をしない (現状はセッション共通で、それを維持する)。
- `folder_history` を設定へ永続化しない。
- 時間窓・遅延を使わない。

## 3. テスト

1. A/B 両方の target / drive-current / nav history が消える (既存の回帰)。
2. **`folder_history` が空になる。**
3. 現在の一覧が先頭へ戻り、カーソルが先頭の可視項目になる。
4. **空の一覧**でカーソルが未選択になり、panic しない。
5. 同じフォルダを A/B の両方で開いていた場合も、位置が 1 回で消える。
6. チェック済み・読書位置・再生位置が**維持される**。
7. メニュー / キー / リングの 3 入口が同じ範囲を消す (parity)。
8. `ini_name` と ring の永続名が変わっていないこと。**保存済みの割り当てを読み戻せること。**
9. 保留中の scroll 指示と選択 anchor が破棄され、操作直後に古い位置へ戻らないこと。
10. mutation: 1.2 の各リセットを 1 つずつ外して、対応するテストが落ちることを報告する。

## 4. ドキュメント

- ユーザー向けマニュアル: フォルダバー / 操作カスタマイズの該当箇所で、この操作が
  **A/B の場所と一覧位置の両方**を消すことを書く。バージョン番号・内部語は書かない。
- [docs/spec.md](../spec.md) と [docs/keymap-spec.md](../keymap-spec.md) の記述を更新する。
- `docs/keymap.ini.default` の説明文が古くなるなら合わせる。
- [next-release-backlog.md](../next-release-backlog.md) §1.98 に結果を追記して閉じる。

## 5. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
python scripts/check_ui_glyphs.py
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
