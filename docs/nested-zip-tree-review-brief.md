# ネスト ZIP ツリーナビ (v1.3.0) — 別セッション・レビュー依頼ブリーフ

> このファイルは「新しい Claude Code セッション（別モデル）に fresh な視点で
> adversarial レビューしてもらう」ための自己完結した指示書です。新セッションを開いて
> 「`docs/nested-zip-tree-review-brief.md` を読んで、この通りにレビューして」と指示するか、
> 本文をそのまま貼り付けて使ってください。**実装はせず、指摘のみ**を出してもらいます。

## 0. あなた（レビュアー）の役割

`feat/nested-zip-tree` ブランチで実装された「ネスト ZIP のツリー表示化」を、
**バグ・退行・エッジケース・UI 応答性**の観点で批判的にレビューしてください。
すでに別モデル（実装は Claude Opus 4.8、レビューは Codex CLI を複数ラウンド）で
P1/P2/P3 を一通り潰してありますが、**別の目で見落としを洗い出す**のが目的です。
修正は行わず、`[P1]`/`[P2]`/`[P3]` + `file:line` + 具体的な修正案で報告してください。
false positive と判断した点も理由を添えてください。

## 1. 背景（何を・なぜ）

- **問題**: ネスト ZIP（ZIP 内に別の ZIP / サブフォルダ）を、従来は全画像を 1 本の線形
  リストに**フラット展開**していた。これが見開き（2 ページ表示）と相性が悪く、複数の本
  （内側 ZIP / サブフォルダ）が 1 冊に連結され、本をまたいで見開きペアがずれ、表紙単独
  も効かなかった。見開きペアリングは平坦な並びの偶奇（`pos % 2`）だけで本境界をリセット
  しないのが根本原因。
- **設計（Strategy A）**: 画像の `entry_name`（例 `"outer/ch01.zip/page01.jpg"` のフル
  文字列）を**一切変えず**、表示／ナビ層だけ足す。メモリ上にツリーを持ち、現在階層だけ
  を materialize して表示する。`entry_name` 不変なので、回転 / 補正 / レーティング /
  消しゴム / 隠蔽 / ローカル調整 / タグ / サイドカー / サムネカタログ / 検索索引の永続
  キー 7 系統が全て生存し、**DB マイグレーション不要**。`self.items` が現在の本のページ
  だけになるので、見開きは本ごとに自動でリセットされる（ペアリングコードは無改修）。
- **正本ドキュメント**:
  - [docs/nested-zip-tree-plan.md](nested-zip-tree-plan.md) — 設計の正本（必読）
  - [docs/nested-zip-test-guide.md](nested-zip-test-guide.md) — テスト ZIP とチェックリスト
  - [docs/virtual-folders.md](virtual-folders.md) — ZIP/PDF の分岐表（GridItem variant 一覧）
  - [docs/ui-responsiveness.md](ui-responsiveness.md) — UI スレッド同期 I/O 禁止チェックリスト

## 2. レビュー範囲

差分の起点はブランチのマージベース:

```bash
git merge-base main HEAD     # main が無ければ master
git diff <merge-base>..HEAD
git log --oneline <merge-base>..HEAD
```

主な変更ファイルと役割:

| ファイル | 役割 |
| --- | --- |
| `src/zip_tree.rs` (新規) | `ZipTree`（build / node_at / collapse_redundant / materialize_level / all_cache_keys） + `ZipNavState`（実効 prefix スタック：new / current / at_root / enter / back / sibling）。**純ロジック + 40 ユニットテスト** |
| `src/grid_item.rs` | `GridItem::ZipDir { zip_path, dir_prefix, is_archive, representative }` 追加 + helper（name/display_path/perf_key/zipdir_cache_key 等） |
| `src/app.rs` | `finalize_zip_enumerate` のツリー配線、`zip_nav` フィールド + ライフサイクル、`zip_nav_show_current_level`（軽量再表示）、enter/back/sibling/fullscreen-sibling、見開き本ごとキー（`spread_container_key` / `apply_spread_for_key`）、Model B 代表サムネピン（`book_container_key` / `make_load_request` の ZipDir 分岐 / `refresh_folder_pin_map` / `toggle_folder_pin_for_idx`）、`apply_sort_change_reload`、`update_zip_nav_address`（パンくず）、`existing_keys` の #pin 拡張 |
| `src/ui_fullscreen.rs` | フルスクリーン Ctrl+↑↓ 本またぎ（`zip_nav_sibling_fullscreen`）、見開き永続（`persist_current_spread_mode` / `persist_current_reading_flow`） |
| `src/ui_main.rs` / `src/app/gamepad_input.rs` | グリッド/ゲームパッドのハンドラ配線（enter/back/sibling）、ピンバッジ表示 |

## 3. 重点的に見てほしい観点

1. **`zip_nav` ライフサイクル**: stale / 未クリアで「別ツリー or 旧ツリー」を操作する経路は
   ないか。確認パス: 列挙 pending 窓、検索（Ctrl+F/G/S）の出入り、★固定 snapshot、
   ドライブ一覧、変換アーカイブ（RAR→ZIP キャッシュ）drill-down、履歴 back/forward。
   クリアのチョークポイントは `start_loading_items` 先頭 + 各 leaving 経路。
2. **軽量再表示 `zip_nav_show_current_level`**: `install_new_items` 後に
   `visible_indices` 再構築・idx-keyed ページ編集状態の clear/rehydrate・キュー排水・
   検索/メタ pending cancel・rating prewarm が `start_loading_items` と整合しているか。
   旧階層の stale idx で `thumbnails[i]` 等を範囲外参照して panic しないか。
3. **見開きキー vs ピンキー**: 見開きは `spread_container_key`（zip_path + 現在の実効
   prefix）を set/read 両方で使う。Model B ピンは set=`spread_container_key`（実効）、
   lookup=`book_container_key(zip_path, dir_prefix)`（cell の prefix）。**非崩しの本では
   一致するが collapse ラッパー本ではずれる**（既知の制限）。他にも set と lookup の
   キーがずれる箇所はないか。
4. **フルスクリーン本またぎ**: `zip_nav_sibling_fullscreen` は移動確定後にのみ
   `capture_fs_nav_holdover`（lock は `items_generation` 前進で解除）。端で lock が
   残らないか、`open_fullscreen` の着地、見開き設定の再適用が正しいか。
5. **キャッシュキー**: ZipDir 代表サムネは `zipdir:{prefix}`、ピン時は
   `zipdir:{prefix}#pin:{entry}`。`all_cache_keys()` + finalize の #pin 拡張が
   `delete_missing` の存続基準と一致し、make_load_request の生成キーと文字列一致するか。
6. **UI 応答性** ([ui-responsiveness.md §4](ui-responsiveness.md)): UI スレッドから同期
   到達する新経路で、`std::fs::read` / `read_dir` / `Path::is_dir` / `load_texture` 多重 /
   catalog cold open 等に達するものはないか。ツリー構築は純ロジックだが、列挙自体が
   UI スレッドに戻っていないか（`d1a6e99f` 教訓: 1100 エントリで 2.3 秒ブロックの実害）。
7. **エッジケース**: 深いネスト（3 段以上）、CBZ、大規模（900+ エントリ）、同名 stem の
   「`.zip` という名前のフォルダ」vs「実アーカイブ」、空サブツリー、`entry_name` の
   `\` / 連続スラッシュ / 末尾スラッシュ。`docs/nested-zip-tree-plan.md` §14 の既知の
   制限が妥当か、見落としがないか。

## 4. やってほしいこと

```bash
# ビルド + テスト (2260 件 green が基準。app::tests は --bin が必要)
cargo build --bin mimageviewer-core
cargo test  --bin mimageviewer-core      # zip_tree / grid_item / 既存テスト含む

# テスト ZIP (構造理解用、GUI 実機確認は不要)
python scripts/make_nested_zip_test.py --big   # dist/ziptest/ に生成
```

- 上記観点で `git diff <merge-base>..HEAD` を精読。
- 指摘は `[P1]`/`[P2]`/`[P3]` + `file:line` + 具体的修正案。false positive は理由付き。
- **実装・修正はしない**（このセッションはレビュー専任）。

## 5. すでに対応済み（再修正済みかの確認は歓迎、再指摘は不要）

Codex CLI が指摘し対応済みの主な点（同じものを再発見したら「対応済みを確認」で OK）:
- 階層切替で `visible_indices` 未再構築 → panic / ページ編集状態の漏れ → clear+rehydrate 追加
- ネスト階層でのソート変更 / 内側ピンが ZIP ルートに飛ぶ → 階層維持の再 materialize に変更
- フルスクリーン本またぎの holdover lock が端で残る → 移動確定後にのみ capture
- 代表サムネピン UI（📌ボタン / 右クリック / バッジ）が外側 ZIP を見ていた → 本キーに統一
- 変換アーカイブのパンくずがキャッシュ ZIP パスを露出 → `archive_source_override` を使用
- pinned ZipDir の #pin キーが `delete_missing` で消える → `existing_keys` に #pin を追加

新規のバグ・退行・設計上の懸念があれば歓迎します。
