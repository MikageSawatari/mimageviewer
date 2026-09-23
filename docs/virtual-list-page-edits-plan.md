# 仮想一覧のページ編集状態 (§1.268) 設計

2026-09-23。設計・検収 = ClaudeCode Opus 5.5 / 設計調査 = GPT-6 Sol xhigh (読み取り専用) /
実装 = 別の GPT-6 Sol / 独立レビュー = 実装者と別の Sol。
不具合の記録は [next-release-backlog.md](next-release-backlog.md) §1.268。

## 決定 (設計担当が採用)

1. **ページ編集の有無と値の正本を、page key で引く 1 つの snapshot にする。** index 集合
   (`mask_pages` 等) は、その snapshot を現在の items generation / 並びへ写した**投影**でしかない。
   投影の欠落を「編集なし」の証拠に使わない。
2. 各仮想一覧は、既存の prepare lifecycle (worker 側) でスマートフォルダと同じ exact-key 検索と
   index 投影を行い、UI は view identity・items generation・(コレクションは revision) が一致する結果だけを install する。
   UI スレッドで cold DB 検索や数千件の投影をしない ([ui-responsiveness.md](ui-responsiveness.md) §4)。
3. **編集ツールを閉じたときの `DeleteNoEdits` (見かけの「編集なし」による共有 preview 削除) を撤去する。**
   明示の保存・削除は従来どおり page key で失効させる。将来「編集なし」の掃除を戻すなら、preview worker 側で
   現在の DB 行と未反映の書き込みを確かめ、観測した preview 版を条件に消す。
4. 投影の同期: 保存・消去は keyed owner と現在 bundle の投影を同時に更新する。並べ替え・除去・generation
   変更は投影を写し直すか置き換える。表示だけの絞り込みは DB を読み直さない。サブ展開の更新・検索の
   streaming 置換・コレクション revision の受理は、古い metadata 結果を取消・拒否する。検索は結果を keyed で
   再利用し、毎秒全件を引き直さない。投影は `ViewerContextBundle` に置き、兄弟窓へ渡さない。

## 対象と接続点

| View | Install route and point for edit metadata |
|---|---|
| Sub-folder expansion | Its worker returns `aggregate: None`; `install_prepared_subfolder_expansion` passes that result to `start_loading_subfolder_items`. Prepare exact-key edits alongside its existing exact-key local-adjust lookup. [subfolder_expansion.rs:1295](C:/home/mimageviewer/src/app/subfolder_expansion.rs:1295), [subfolder_expansion.rs:1354](C:/home/mimageviewer/src/app/subfolder_expansion.rs:1354), [subfolder_expansion.rs:2125](C:/home/mimageviewer/src/app/subfolder_expansion.rs:2125) |
| Ctrl+G | `replace_search_view_items` installs each streaming rebuild and then clears page-edit state. Attach metadata to each accepted result at that replacement boundary. [global_search_ui.rs:1252](C:/home/mimageviewer/src/global_search_ui.rs:1252), [global_search_ui.rs:1309](C:/home/mimageviewer/src/global_search_ui.rs:1309) |
| Collections | The collection prepare worker is the natural lookup point; its separate install path currently clears page-edit state. Bind the result to the accepted collection revision. [collection_grid.rs:176](C:/home/mimageviewer/src/app/collection_grid.rs:176), [collection_grid.rs:1461](C:/home/mimageviewer/src/app/collection_grid.rs:1461), [collection_grid.rs:2262](C:/home/mimageviewer/src/app/collection_grid.rs:2262) |
| Reading history; rating list | Both pass real items and a synthetic path to `start_loading_items`. Supply prepared metadata at that boundary, including on rating reload. [src/app.rs:24625](C:/home/mimageviewer/src/app.rs:24625), [src/app.rs:25017](C:/home/mimageviewer/src/app.rs:25017) |


対象の編集種別: 消しゴム mask、隠蔽、注釈 (comic)、ページ単位の補正、書き出しトリミング、表示トリム、
ローカル調整の有無 (スマートフォルダと同じ集合)。回転は別の keyed DB / cache 経路なので、この snapshot には
入れず、キー規則と lifecycle の監査を別に行う。

## 当初の一括 slice 案

- 共通の exact-key snapshot と投影を作り、上表の 5 経路 (サブ展開・Ctrl+G・コレクション・閲覧履歴・
  レーティング一覧) へ接続する。ZIP/PDF のページは既存の `page_key_for_grid_item` のキーを使う。
  ファイル名スタックの `Stack` セルは container なので、実在する平坦なメンバー画像へ投影する。
- `DeleteNoEdits` の close 動作を撤去する。
- 回帰テスト (第 1 層): 各経路で、消しゴム済みの画像を開くと編集結果が描画に反映される、ツールを開いて
  何もせず閉じても共有 preview を消さない。
- ファイル名スタックの切り替えがフォルダ prefix で再 hydrate している経路 ([filename_stack_ui.rs](../src/filename_stack_ui.rs))
  がサブ展開の失敗を引き継ぐかを監査する。
- 後回し: preview cache の版管理全般、回転の refactor。

## 段階

### Phase A (2026-09-23、完了範囲: 初回 install と非破壊 close)

- `src/app/page_edit_snapshot.rs` に実ページ key を正本とする共通 snapshot と index 投影を追加。
  スマートフォルダと同じ DB の exact-key 複数検索を prepare worker で実行し、
  ZIP/PDF ページも `page_key_for_grid_item` の key に従う。
- サブ展開の prepare (`src/app/subfolder_expansion.rs:1337`) と受理
  (`src/app/subfolder_expansion.rs:2124`) に編集 revision を結び、古い結果は再 prepare する。
  `src/app.rs:27974` の sidecar 継続で投影を install する。
- コレクション prepare (`src/app/collection_grid.rs:245`) と grid 受理
  (`src/app/collection_grid.rs:1865`)、別の navigation prepare/landing
  (`src/app/collection_navigation.rs:1930`, `src/app/collection_navigation.rs:2850`) に
  同じ revision gate を設ける。snapshot と投影は表示 bundle に属する
  (`src/app/viewer_context_registry.rs`)。
- close 時の `DeleteNoEdits` を全 view で撤去。編集結果不在と注釈フォント不在による
  shared preview 削除も撤去し、これら三つは共有キャッシュを変更しない。
  投影の欠落・古さと、別 context の保存結果を区別できないため。
  明示保存・削除による key 失効と、完成済み編集結果の close 保存は継続
  (`src/app.rs:60726`, `src/app.rs:60828`)。
- サブ展開とコレクションで保存済み mask が表示用 index に入る回帰テストを追加。
  サブ展開は実際の prepare 受理と stale revision 拒否を通し、コレクションは conceal と
  ZIP member の exact page key も確認する。close は六つの view と三つの skip outcome で
  実際の共有 preview 行の存続を確認する。Phase B の失敗を記録する ignored test も追加。

Phase A の表示保証は上記 2 view の受理された初回 install に限定する。
後続の状態変更・一覧切替で既存の synthetic-path hydration が編集投影を落とす問題は
Phase A2 として扱う。Phase A 以前は両 view の初回 install でも同じ編集が表示されなかった。

### Phase A2 (未実装: install 後の整合)

- Snapshot Lock: サブ展開・コレクションから固定すると、synthetic origin を使った
  prefix rehydrate が初回 install で正しく表示した編集を落とす。固定一覧への復帰でも同じ
  (`src/app/snapshot_ops.rs:826`, `src/app/snapshot_ops.rs:1048`,
  `src/app/snapshot_ops.rs:1144`)。subset の key と世代に結び付く worker 投影が必要。
- rename 後の mounted context 再 hydrate は synthetic `current_folder` を prefix として
  使う (`src/app.rs:33447`, `src/app.rs:33451`)。移行後の key と現在の items を
  worker で照合する。
- metadata import は index 投影を入れ替える一方、keyed snapshot を更新しない
  (`src/app.rs:32098`, `src/app.rs:32131`)。import worker の結果に owner の更新を含める。
- content-identity restore 完了は synthetic view の page-edit state を clear する
  (`src/app/content_identity_restore.rs:438`, `src/app/content_identity_restore.rs:448`)。
  完了時点の view identity / items generation を確認して worker で再投影する。
- 非同期 local-adjust 書込は初回 install 後に確定し得る
  (`src/app.rs:61693`, `src/app.rs:61727`)。revision は増えるが
  (`src/app/smart_folder.rs:5852`)、既に install 済みの投影は自動更新されない。
  兄弟 viewer context の編集にも同じ再同期が必要。context の状態交換は
  `src/app/viewer_context_registry.rs:2194`、fork 時の複製は同ファイル `:2837`。
- ファイル名スタックは script 結果の group/aggregate materialize が UI 側で行われ
  (`src/filename_stack_ui.rs:338`)、`start_loading_items` が snapshot を破棄する。
  flat 切替も synthetic prefix で再 hydrate する (`src/filename_stack_ui.rs:752`)。
  group/materialize と flat member の投影を同じ worker 世代で準備する。

### Phase B (未実装)

- Ctrl+G は結果を streaming 差し替えする度に `replace_search_view_items` が
  編集 map を消す (`src/global_search_ui.rs:1252`, `src/global_search_ui.rs:1309`)。
  増分の keyed snapshot と worker prepare、各置換の identity/generation/epoch gate を設ける。
- 閲覧履歴は `install_reading_history_entries` が synthetic path で直接ロードする
  (`src/app.rs:24592`, `src/app.rs:24633`)。現行の履歴行は Folder/Zip/Pdf 等の
  container であり、画像ページそのものの行はない
  (`src/reading_history_db.rs:25`, `src/app.rs:77400`)。
  ページを開く境界を先に確定した上で worker prepare/acceptance を設ける。
- レーティング一覧は `install_rating_view_rows` が直接ロードし、ローカル再ソートも
  同じ入口を使う (`src/app.rs:24960`, `src/app.rs:25025`)。
  worker prepare と再ソートを含む結果受理の gate を追加する。
- 三経路とも UI スレッドの cold DB 検索や大量の index 投影を避ける。
  preview cache の一般的な版管理と回転の owner 整理は、従来どおり別途監査する。

## 設計調査の原文

## Findings

The failure is at the boundary between real page keys and list indices. `page_path_key` derives a stable key from each image, ZIP entry, or PDF page, but `resume_loading_items_after_sidecar` loads several index keyed edit maps with a prefix query against the list’s synthetic path. That cannot find edits stored under the real page key. The display pipeline then treats a missing `mask_pages` entry as an absent erase layer. [src/app.rs:60616](C:/home/mimageviewer/src/app.rs:60616), [src/app.rs:27978](C:/home/mimageviewer/src/app.rs:27978), [src/app.rs:28086](C:/home/mimageviewer/src/app.rs:28086), [src/app.rs:63640](C:/home/mimageviewer/src/app.rs:63640).

The install routes differ, so changing that prefix query alone would miss cases:

| View | Install route and point for edit metadata |
|---|---|
| Sub-folder expansion | Its worker returns `aggregate: None`; `install_prepared_subfolder_expansion` passes that result to `start_loading_subfolder_items`. Prepare exact-key edits alongside its existing exact-key local-adjust lookup. [subfolder_expansion.rs:1295](C:/home/mimageviewer/src/app/subfolder_expansion.rs:1295), [subfolder_expansion.rs:1354](C:/home/mimageviewer/src/app/subfolder_expansion.rs:1354), [subfolder_expansion.rs:2125](C:/home/mimageviewer/src/app/subfolder_expansion.rs:2125) |
| Ctrl+G | `replace_search_view_items` installs each streaming rebuild and then clears page-edit state. Attach metadata to each accepted result at that replacement boundary. [global_search_ui.rs:1252](C:/home/mimageviewer/src/global_search_ui.rs:1252), [global_search_ui.rs:1309](C:/home/mimageviewer/src/global_search_ui.rs:1309) |
| Collections | The collection prepare worker is the natural lookup point; its separate install path currently clears page-edit state. Bind the result to the accepted collection revision. [collection_grid.rs:176](C:/home/mimageviewer/src/app/collection_grid.rs:176), [collection_grid.rs:1461](C:/home/mimageviewer/src/app/collection_grid.rs:1461), [collection_grid.rs:2262](C:/home/mimageviewer/src/app/collection_grid.rs:2262) |
| Reading history; rating list | Both pass real items and a synthetic path to `start_loading_items`. Supply prepared metadata at that boundary, including on rating reload. [src/app.rs:24625](C:/home/mimageviewer/src/app.rs:24625), [src/app.rs:25017](C:/home/mimageviewer/src/app.rs:25017) |

Smart folders establish the usable pattern: their worker queries only included exact keys and maps results into display indices before install. It covers per-page adjustment, export crop, view trim, mask, conceal, comic annotations, and local-adjust presence. Sub-folder expansion already prepares local-adjust presence exactly; ordinary hydration also looks it up by exact keys. Rotation is a separate keyed DB/cache path, so it needs a key-rule and lifecycle audit, not another `PreparedAggregateMetadata` field merely for symmetry. [smart_folder.rs:4940](C:/home/mimageviewer/src/app/smart_folder.rs:4940), [smart_folder.rs:5057](C:/home/mimageviewer/src/app/smart_folder.rs:5057), [src/app.rs:30555](C:/home/mimageviewer/src/app.rs:30555), [src/app.rs:54620](C:/home/mimageviewer/src/app.rs:54620).

Close currently infers `has_source_edits` from three index sets and queues `DeleteNoEdits` when those and crop/loaded annotations appear empty. The queued deletion addresses the shared preview by page key. Thus one incorrectly hydrated view can remove a preview another view uses. [src/app.rs:60682](C:/home/mimageviewer/src/app.rs:60682), [src/app.rs:60713](C:/home/mimageviewer/src/app.rs:60713), [src/app.rs:60811](C:/home/mimageviewer/src/app.rs:60811), [edit_preview_cache.rs:1376](C:/home/mimageviewer/src/edit_preview_cache.rs:1376).

## Design

Make **one page-keyed edit snapshot** the owner of edit presence and values for the installed item generation. Share the smart folder’s exact-key lookup methods and index projection, while letting each view’s existing prepare lifecycle carry the result. The UI installs only a matching view identity, item generation/order, and, for collections, revision. No cold DB lookup or thousands-of-rows projection belongs on the UI thread; [the responsiveness checklist](C:/home/mimageviewer/docs/ui-responsiveness.md:335) requires worker handling for large SELECTs and bounded large CPU work.

Treat index sets as projections, never proof of durable absence. For preview close, use the exact page key and authoritative edit state, including pending in-memory annotation edits. A missing or stale projection must yield **unknown**, not “no edits.” The safest first rule is that closing an unchanged tool does not delete a shared preview for apparent absence; explicit save/delete mutations already invalidate by page key. Retain other stale-preview deletion outcomes only after reviewing their race with another context’s save. Any future “no edits” cleanup should validate current DB rows and queued writes in the preview worker and condition deletion on the preview version it observed. [src/app.rs:60637](C:/home/mimageviewer/src/app.rs:60637), [src/app.rs:60725](C:/home/mimageviewer/src/app.rs:60725).

After install, edit save/erase must update the keyed owner and current bundle’s projection together; item reorder, removal, or generation change must remap or replace the projection. A visibility-only filter needs no DB reload. Sub-expansion refresh, search streaming replacement, and collection revision acceptance must cancel or reject older metadata results. Search should batch and reuse keyed results across rebuilds rather than query the whole result set every second. Keep index state with its `ViewerContextBundle`; a sibling window must neither receive its indices nor have its preview invalidated by a read-only view transition. [src/app.rs:30829](C:/home/mimageviewer/src/app.rs:30829), [viewer_context_registry.rs:2190](C:/home/mimageviewer/src/app/viewer_context_registry.rs:2190).

The **first slice** is the common exact-key snapshot and projection, wired to all five routes above, plus removal of the unsafe `DeleteNoEdits` close action and regression coverage for viewing and closing an already masked image in each route. Include ZIP/PDF keys where those routes contain pages, using the existing `page_key_for_grid_item`; `Stack` cells are containers, so project edits onto their real flat image members. Audit the separate filename-stack swap, which currently rehydrates from a folder prefix and can inherit the sub-expansion failure. Defer broader preview cache versioning and rotation refactoring until their ownership audits. [edit_source.rs:362](C:/home/mimageviewer/src/edit_source.rs:362), [filename_stack_ui.rs:752](C:/home/mimageviewer/src/filename_stack_ui.rs:752).

Read-only investigation only; no files, builds, or application state were changed.
