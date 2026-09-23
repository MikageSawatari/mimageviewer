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

## 段階 B 設計 (2026-09-23、設計のみ・独立レビュー待ち)

### 共通の install 契約

- 対象は Ctrl+G、閲覧履歴から開いたページ、レーティング一覧の**フルスクリーン表示**。
  サムネイルは従来の非同期表示のまま。Phase A の `PageEditSnapshot` を唯一の page-key 正本とし、
  `page_key_for_grid_item` で実ページを識別する。worker は同じ exact-key 一括 lookup と
  `PageEditSnapshot::project` を使う。新しい index map owner は作らない
  (`src/app/page_edit_snapshot.rs:38-68, 74-146, 146-175`)。
- 各要求は `{ViewerContextId, TopLevelGridSurface + surface generation, source items_generation,
  request sequence, page_edit_revision, candidate order token}` を持つ。worker は候補 items の
  **確定順序**に対する `(snapshot, projection)` を同じ結果に載せる。UI は全 stamp と最新 sequence、
  候補順序 token を照合し、items の generation 更新と投影 install を一度の受理で行う。
  token は不変候補 payload の identity とし、UI で全件の key を再計算して照合しない。
  `source items_generation` は旧一覧からの要求を拒否し、受理後の新 generation に投影を結び直す。
  edit revision が進んだ結果は拒否して再 prepare する。失敗・取消・surface 退出は現行の正しい
  受理済み一覧を保持するか空の準備表示へ戻し、古い結果を部分 install しない。
  Phase A の受理例は `src/app/subfolder_expansion.rs:2119-2133` と
  `src/app/collection_grid.rs:1887-1930, 2320-2333`。

### Ctrl+G: streaming 置換の prepare と受理

- 現行は `poll_global_search_events` が最大 8 batch/frame を受け、変更時に約 1 秒ごとに
  `rebuild_items_from_global_search` → `replace_search_view_items` を呼ぶ。同 helper は
  `install_new_items` 後に編集 state を clear する
  (`src/global_search_ui.rs:136-143, 1753-1765, 1853-1868, 1889-1935, 1248-1255, 1301-1309`)。
  **初回、各 streaming tick、Done、sort/filter・集約/ドリル変更、mtime 完了**を同じ prepare 入口にする
  (`src/global_search_ui.rs:1697-1726, 1896-1935`)。
- 検索 session の単一 worker に受信済み hit batch (最大 500 件) と view/sort/filter 変更を順に渡し、
  worker が候補順と Phase A snapshot の差分 lookup / 投影を作る。既存 UI の batch ごとの
  rating `get_many` も worker 側へ移す (`src/global_search_ui.rs:1766-1817`、
  `src/global_search.rs:35-37`、`src/fts_index.rs:69`)。新しい query / close では session を取消す
  (`src/global_search_ui.rs:414-436, 1562-1597`)。
- worker は同じ `PageEditSnapshot` と「照会済み key」集合 (空ヒットも記録) を session 内で再利用し、
  その tick に**新しく現れた実ページ key のみ**を smart-folder と同じ DB API で batch lookup する。
  並べ替え・ドリルは DB を引き直さず worker で再投影する。編集 revision 変更時だけ当該
  session の照会済み集合を失効させる。`load_and_project` を tick ごとに全件実行しない。
  request は latest-wins の 1 slot に畳み、完了前の tick は最新候補へ収束させる。
  検索は最大 10,000 hit、batch は 500 件なので、key/値/投影は O(採用 hit 数)、DB は
  新規 key にだけ上限付き batch、UI への完成結果も 1 件に抑える。大量候補の sort / 投影 /
  stale payload の破棄は worker 側に置き、UI に全件 clone や cold SELECT を追加しない
  (`docs/ui-responsiveness.md:335-356, 386-408`)。
- 受理は共通 stamp に加え search run ID、query/filter、Flat/Aggregated/Drilled path、sort、
  最新 rebuild sequence、初回なら Search surface の入場意図、以後は
  `items_are_global_search_view` を確認する。旧 run、後続 tick、
  物理コンテナへ移動後、別 context の結果は捨てる。受理時だけ既存
  `replace_search_view_items` の selection/thumbnail 再利用を実行し、その `clear_page_edit_state`
  **後**に同じ候補の snapshot と投影を install する (`src/global_search_ui.rs:1190-1243, 1261-1309`)。

### 閲覧履歴: 行ではなく、開いた本のページ列で受理

- 履歴行は Folder/Zip/Pdf/Archive/Video/Audio の container/media で画像ページ行を持たない。
  現行 `enter_reading_history` は UI で `list_recent`、pin lookup、synthetic path の
  `start_loading_items` を行う。履歴行へ page edit を投影する設計・既存 ignored test の期待は誤り
  (`src/reading_history_db.rs:15-31`, `src/app.rs:24617-24693, 77702-77753`,
  `src/app/tests.rs:1145-1161`)。履歴一覧の取得・materialize・pin lookup と選択行の
  path status 判定 (`src/app.rs:19304-19340`) も worker へ移し、container 行には page snapshot を作らない。
- **ページ open 境界**は履歴行を選択した瞬間ではなく、選択した container の folder scan または
  ZIP/PDF enumeration が実ページ items と順序を確定し、最初の `open_fullscreen` を許す直前。
  `note_reading_history_open` は戻り先だけを記録し、folder の auto-open は列挙後、
  ZIP/PDF は deferred open を使う (`src/app.rs:19343-19375, 40014-40039,
  22513-22521, 22645-22666, 25249-25277, 26293-26307`)。
  この列挙/scan worker の後段で、実ページ items に `PageEditSnapshot::load_and_project` を実行する。
  PDF meta-cache の先行 placeholder は履歴起点では paint/auto-open を保留する
  (`src/app.rs:26331-26342, 26522-26543`)。
  変換 archive も最終的な実ページ key が確定してから同じ経路へ入れる。
- 受理は要求時の履歴 context/surface generation を保持する open intent、選択 row の
  key・container path/kind、open/scan/enumerate sequence、行き先の items generation/order、
  edit revision を照合する。履歴 surface から物理 view へ移った後は、現在の surface でなく
  この intent と行き先を照合する。別窓 open は source と行き先の `ViewerContextId` も照合する。
  sidecar 後段が用意済み投影を prefix hydration で上書きしないよう同じ continuation を通し、
  page snapshot と items を受理してから deferred fullscreen を解放する
  (`src/app.rs:27385-27399, 28008-28043`)。Video/Audio はページ投影対象外。

### レーティング一覧: 初回・再ソート・reload

- 現行 build worker は DB 行復元まで。受理後の `install_rating_view_rows` が UI で
  sort/materialize、pin lookup、synthetic path の `start_loading_items` を行う。
  ローカルの sort、★変更による行削除/再配置、reload、Back 復帰もこの入口へ来る
  (`src/rating_view.rs:60-120, 123-154`, `src/app.rs:24833-24855, 24879-24922,
  24933-24997, 25009-25081`)。build worker を final order + pin metadata + Phase A
  exact-key snapshot/projection まで拡張し、初回/DB reload は完成結果だけ受理する。
- ローカル sort と既存行だけの★変更は、現在 bundle の同じ keyed snapshot を read-only lease で
  worker に渡し、**DB を再照会せず**新順へ投影する。追加行のある reload は新 key だけを照会し、
  edit revision が違えば再 prepare する。lease は同じ snapshot の参照であり第二の編集正本ではない。
  行・snapshot を UI で大きく clone せず、希望 sort と行変更も受理まで live rows に反映しない。
  受理時は context/surface generation、★数、
  rating build/reload sequence、sort と display order、行 membership/order token、
  source items_generation、edit revision を照合し、rows と items と投影を原子的に install する。
  `RatingViewPending` が現在は stars しか持たないため sequence を追加する
  (`src/rating_view.rs:60-90`, `src/app.rs:24886-24909, 25017-25022, 25074-25117`)。

### pending 表示、context、検証と境界

- **未編集画像を先に描き、後から編集済み画像へ切り替えることは不可**。
  色調補正/カラー化/LUT は初回 paint から完成画像と同じ色であるべき確定要件
  (`docs/display-pipeline.md:1918-1941` の R1–R4)。snapshot pending 中は前の受理済み
  display unit をその identity のまま保つ。初回/別ページで保持画像が無ければ中立の準備表示にし、
  target の raw/fullscreen texture は公開しない。受理済み投影と当該ページの適切な rendition が
  揃ってから target を提示し、次の held-page 移動はそのページの提示後に受理する。
  これにより R1 のページ飛ばしも防ぐ。失敗時は誤った raw へ fall back せず既存表示/準備表示を保つ。
- snapshot と投影、pending/sequence は `ViewerContextBundle` に属し、mount/swap と cancel/drop を
  同じ context で扱う (`src/app/viewer_context_registry.rs:1021, 2194, 2836-2851`)。
  兄弟 context の index は渡さない。§1.267 の collection root 直接 Image は独立 context に
  immutable prepared 順と session を持ち、search/history/rating 由来 Image はその root session を
  推定継承しない (`src/app.rs:46501-46516, 46553-46613, 46633-46641`、
  `docs/collection-playback-plan.md:460-461, 473-477`)。collection root への復帰は Phase A の
  collection revision gate に任せる (`src/app/collection_grid.rs:1887-1930`)。
- テストでは Phase A の ignored 三件を un-ignore して production prepare/acceptance を通す。
  履歴 test は container index の mask 判定から、開いた Folder/ZIP/PDF の実ページ paint 判定へ直す
  (`src/app/tests.rs:1117-1161`)。search の連続 rebuild (新 key の lookup と旧 key の再利用)、
  sort/drill 変更、rating の複数行 local re-sort と reload、各 route の古い sequence/revision/
  context/order 拒否、pending 中の raw 禁止と最終 paint、通常フォルダ対照を追加する。
  10,000 hit 条件で worker DB batch 数・UI pass 時間・stale result drop を計測する。
- **Phase A2 との境界**: B は上記三 route の初回/置換/再ソート/履歴ページ open の受理まで。
  受理後の Snapshot Lock、rename、metadata import、content-identity restore、late write、
  兄弟 context 間の編集再同期、filename-stack grouping/flat 切替は A2 のまま
  (本書「Phase A2」、`src/app/snapshot_ops.rs:826-1144`, `src/filename_stack_ui.rs:338,752`)。
  B の検索 tick や rating local sort を A2 の汎用事後整合機構として流用しない。

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
