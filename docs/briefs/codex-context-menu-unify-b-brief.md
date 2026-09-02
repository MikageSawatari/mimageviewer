# ブリーフ: 右クリックメニュー一本化 B — 仮想項目の native 化と Shell 項目のサブメニュー化

正本: [docs/context-menu-unification-plan.md](../context-menu-unification-plan.md)。
**着手前に §1 (特に 8) と §1.1 / §1.2 を読むこと。** phase A は実装済み
([codex-context-menu-unify-a-brief.md](codex-context-menu-unify-a-brief.md))。

作業場所: この worktree (`C:\home\mimageviewer-extlaunch`, ブランチ `external-tool-launch`)。

## 0. この phase の範囲

1. **混在選択の拒否** (正本 §1 の 8) — 先にこれをやる。他より小さく、独立している
2. **仮想項目でも mIV 項目だけのネイティブメニューを出す**
3. **Shell 由来の項目をサブメニューへ畳む。既定サブメニュー、設定で併記に戻せる**

## 1. 混在選択の拒否 + 件数の食い違いの原因特定

### 1.1 まず原因を特定すること

利用者の実機報告 (2026-08-30):

- 「選択項目のパスをコピー」に **件数が出ない**
- 「ゴミ箱へ移動 (タグ・評価も整理)」に **ページを含めた件数が出ている**
- PDF ページは削除できないので、その件数は正しくない

**私 (ClaudeCode) がコードを読んだ限りでは、両方とも仮想項目を除外しているはずで、この
見え方を説明できなかった。** 関係する経路:

- `ContextMenuInput.real_checked_count` は `target.real_paths.len()`
  ([context_menu.rs:1188](../../src/ui_dialogs/context_menu.rs:1188))
- `real_paths` は `collect_checked_paths()` で、`file_operation_path()` を filter している
  ([context_menu.rs:1680](../../src/ui_dialogs/context_menu.rs:1680))
- `file_operation_path()` / `drag_source_path()` はどちらも `ZipImage` / `PdfPage` を返さない
  ([grid_item.rs:276](../../src/grid_item.rs:276), [grid_item.rs:296](../../src/grid_item.rs:296))
- 削除確認ダイアログの文言は別に `count` を受け取っている
  ([context_menu.rs:398](../../src/ui_dialogs/context_menu.rs:398))

**推測で直さないこと。** メニューのラベル・削除確認ダイアログ・実際の削除対象の 3 つについて、
それぞれどの数を使っているかを追い、**どこで仮想項目が混ざるのかを特定してから**直す。
特定できた原因を報告に書くこと。特定できなければ、**直さずに「特定できなかった」と報告する**。

### 1.2 仕様

**実項目と仮想項目が混在した選択では、仮想項目を扱えない操作を実行しない。**

- 対象: 削除、パスのコピー、Shell へ渡す操作など、仮想項目に適用できないもの。
- **メニュー項目は出す。押したらトーストで理由を出して何もしない。**
  隠す / 無効化ではなく実行時に断るのは、利用者が「なぜ無いのか」を知る必要があるため。
  文面は理由と対処を書く。例:
  「圧縮ファイル / PDF 内のページが選択に含まれています。ページは削除できません。
  ページの選択を外してから実行してください」
- **件数表示は素直な選択数にする。** 混在時に「実対象だけの数」を出す必要がなくなるので、
  `real_checked_count` を使った文言分岐は消してよい。
- 混在の判定は純関数にしてテストする。

## 2. 仮想項目のネイティブメニュー

`ZipImage` / `PdfPage` / `Stack` などパスを持たない対象でも、**mIV 項目だけの HMENU を出す**。

- `native_grid_context_menu_target` が実パスを作れないときに諦めるのをやめ、
  **Shell 項目のマージを飛ばす**形にする。仮想対象には渡せる実ファイルが無いので、
  シェル項目はそもそも意味が無い。
- phase A で項目定義は共通化済みなので、**描画先を変えるだけ**になるはず。
  定義側に分岐を増やさないこと。
- 閲覧履歴 grid が native を bypass している現在の挙動は、この phase でも変えない。

## 3. Shell 項目のサブメニュー化

- 既定: mIV 項目の後に区切りを 1 本置き、**最後に「Windows のメニュー」1 項目**を出す。
  その中に `IContextMenu` の項目を入れる。
- 設定: 「Windows のメニューを併記する」を追加し、ON で従来どおり同じ階層へ並べる。
  **既定は OFF (= サブメニュー)。**
- **可能なら、サブメニューが開かれるまで `QueryContextMenu` を遅らせる** (`WM_INITMENUPOPUP`)。
  利用者環境で構築に 1.2〜1.4 秒かかっており、これが右クリックのたびに乗っている
  (正本 §2.2)。**遅延できない場合は無理に作り込まず、その旨を報告すること。**
  遅延なしでもサブメニュー化自体の価値 (長さの解消) はある。
- Shell コマンドの ID オフセットと `InvokeCommand` の対応が、サブメニューでも壊れないこと。
  **phase A で入れた挿入位置の考え方が変わる**ので、テストを更新すること。

## 4. 守ること

- コミット前に `cargo fmt` (引数なし)。テストは `cargo test -p mimageviewer --lib` と
  `cargo test --test ui_snapshot`。
- UI 文言を足したら `python scripts/check_ui_glyphs.py`。
- 設定を追加するので、環境設定の検索索引 entry も足すこと (テストが検査する)。
- UI スレッドで同期 I/O を足さない。
- 範囲を広げない。正本と食い違ったら実装を止めて報告すること。

## 5. 完了報告に含めること

- 件数の食い違いの**特定できた原因** (または特定できなかったこと)
- 変更ファイル一覧、追加した純関数とテスト、テスト結果の件数
- `QueryContextMenu` の遅延ができたかどうか
- 実機で確認すべき操作
