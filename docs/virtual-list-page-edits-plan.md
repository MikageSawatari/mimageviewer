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

## 段階 B 設計 (2026-09-23、Ctrl+G とレーティング一覧)

### 共通の install 契約

- 対象は Ctrl+G とレーティング一覧の**フルスクリーン表示**。サムネイルは従来の非同期表示のまま。Phase A の `PageEditSnapshot` を唯一の
  page-key 正本とし、`page_key_for_grid_item` で実ページを識別する。worker は同じ exact-key 一括 lookup と
  `PageEditSnapshot::project` を使う。新しい index map owner は作らない
  (`src/app/page_edit_snapshot.rs:38-68, 74-146, 146-175`)。
- 各要求は
  `{ViewerContextId, surface + surface generation, source items_generation, request sequence, page-edit write stamp}`
  と不変の候補 items を持つ。候補は sequence に一意に束縛し、独立の order token はその外から候補順序を変更できる経路にだけ付ける。worker は**確定順序**の
  `(snapshot, projection)` を同じ結果に載せる。UI は stamp と最新 sequence を照合し、items の generation 更新と投影 install
  を一度の受理で行う。UI で全件 key を再計算しない。`source items_generation` は旧一覧を拒否するが、同じ surface 内で別要求の受理により進んだ場合は最新意図を新
  generation に rebase して再 prepare する。write stamp の進行も再 prepare する。失敗・取消・surface 退出では正しい受理済み一覧を保持し、古い結果を部分
  install しない。Phase A の受理例は `src/app/subfolder_expansion.rs:2119-2133` と
  `src/app/collection_grid.rs:1887-1930, 2320-2333`。
- DB 読みの鮮度は bundle-local `page_edit_revision` と別の、全 page-edit writer が共有する
  process-wide `active_writers: usize` / `completed_writes: u64` (両方 atomic、context swap 対象外) で判定する。
  各 writer は DB write 直前に active を増やし、成功・失敗・panic のいずれでも drop guard が
  completed を増やしてから active を減らす。prepare は read 前後に二値を読み、両時点の active=0
  かつ completed 不変の場合だけ有効。UI 受理も recorded completed との一致と active=0 を要求する。
  不適格結果は最新意図へ rebase し、待機で UI を止めない。snapshot の有無に依存させず、UI 直書きと
  bundle paste / bulk worker の全 connection を覆う (`src/app.rs:30644-30656, 61734-61745`,
  `src/edit_bundle_app.rs:212-223`, `src/edit_bundle_bulk.rs:535`, `src/edit_bundle.rs:497-503`,
  `src/sidecar_import.rs:1563-1605`, `src/metadata_transfer.rs:815-927, 976`)。
  Store の単件/一括 set・delete・copy は `adjustment_db.rs`, `mask_db.rs`, `conceal_db.rs`,
  `comic_db.rs`, `local_adjust_db.rs`, `view_trim_db.rs`, `export_crop.rs` で guard を取得し、
  legacy mask repack と rename migration の直接 SQL も同じ counter に参加する。
  受理後レビューで見つかった `zip_key_migration.rs` の CP932 ZIP key 移行全体 (DB 間の隙間も含む) と
  `metadata_cleanup.rs` の store descriptor 別 orphan delete にも同じ guard を付ける。
  既存 `page_edit_revision` は受理済み bundle の投影整合に残す。

### Ctrl+G: streaming 置換の prepare と受理

- 現行は `poll_global_search_events` が最大 8 batch/frame を受け、変更時に約 1 秒ごとに `rebuild_items_from_global_search` →
  `replace_search_view_items` を呼ぶ。同 helper は `install_new_items` 後に編集 state を clear する
  (`src/global_search_ui.rs:136-143, 1753-1765, 1853-1868, 1889-1935, 1248-1255, 1301-1309`)。**初回、各 streaming tick、Done、sort/filter・集約/ドリル変更、mtime 完了**を同じ
  prepare 入口にする (`src/global_search_ui.rs:1697-1726, 1896-1935`)。
- 検索 session の単一 worker に、**全 accepted hit batch を順序を保つ FIFO**で渡す。batch は最大 500 件で連番を付ける。view/sort/filter
  と Done は最終 batch 連番を含む小さい「最新希望 view command」に畳むが、batch を latest slot で失わない。worker は指定連番まで追記してから command
  と候補順、Phase A snapshot の差分 lookup / 投影を作る。UI の `all_hits` 全件を要求ごとに複製しない。既存 UI の hit clone に全件コピーを重ねない
  (`src/global_search_ui.rs:439-459, 1759-1817`、`src/global_search.rs:35-37`、`src/fts_index.rs:69`)。batch
  rating `get_many` も worker へ移す。新 query / close は session を取消す
  (`src/global_search_ui.rs:414-436, 1562-1597`)。
- worker は同じ `PageEditSnapshot` と「照会済み key」集合 (空ヒットも記録) を session 内で再利用し、その tick に**新しく現れた実ページ key のみ**を
  smart-folder と同じ DB API で batch lookup する。並べ替え・ドリルは DB を引き直さず worker で再投影する。edit epoch 変更時だけ当該 session
  の照会済み集合を失効させる。`load_and_project` を tick ごとに全件実行しない。view command は単一 in-flight prepare と最新希望 sequence
  に分ける。受理/拒否/失敗のたび UI は現在の generation を acknowledgement として返し、同一 session/surface にさらに新しい希望があれば直ちにそれを
  prepare する。generation/edit epoch だけで拒否された最新希望も新 stamp で再実行する。Done は希望状態に残すので、A が G→G+1 を install して B
  が旧 G で拒否されても、後続 tick なしで B/Done の最終候補を G+1 から作り直す。旧 run/退出だけは再試行しない。検索は最大 10,000 hit。key/値/投影は O(採用 hit
  数)、DB は新規 key の上限付き batch のみ。sort/投影/stale payload の破棄は worker。受理時の thumbnail/selection 再利用は既に全件を走査するため
  (`src/global_search_ui.rs:1190-1309`)、10,000 hit の UI pass 時間を gate にする。UI に追加の全件 clone や cold SELECT
  を置かない (`docs/ui-responsiveness.md:335-356, 386-408`)。
- 受理は共通 stamp に加え search run ID、query/filter、Flat/Aggregated/Drilled path、sort、最新 rebuild sequence、初回なら
  Search surface の入場意図、以後は `items_are_global_search_view` を確認する。旧 run、後続 tick、物理コンテナへ移動後、別 context
  の結果は捨てる。受理時だけ既存 `replace_search_view_items` の selection/thumbnail 再利用を実行し、その `clear_page_edit_state`
  **後**に同じ候補の snapshot と投影を install する (`src/global_search_ui.rs:1190-1243, 1261-1309`)。
  query 入力変更時点で旧 SearchHandle と prepare worker の両方を取消し、debounce 中も新しい空の prepare worker を所有する。
  wish に run sequence / query / filter を記録し、受理時の現行値と照合する。filter の明示再実行も新 run にする。
- Phase B 回帰修正: Ctrl+G の address 進捗は rated batch / edit prepare の受理を待たず、stream batch 受信時の
  `valid_hits` / `scanned_candidates` で更新する。Flat の件数は `all_hits` への遅延追記中も受信済み件数を示し、
  Aggregated はコンテナ件数と受信済み hit 件数を別に示す (`src/global_search_ui.rs:2076-2094, 2780-2817`)。

### 閲覧履歴は Phase B の対象外

- 履歴行は本の container を指す。実ページの open は物理 page loader を通り、page key の編集を復元する
  (`src/app.rs:28167-28215`, `src/mask_db.rs:1625-1646`)。Folder/ZIP の実ページ mask 回帰テストは
  Phase B の製品変更前に 2/2 通過した (`src/app/tests.rs:1146-1219`)。履歴 container 行の index をページとして扱う期待は誤り。
- PDF placeholder の page 数・key 順序の一致/不一致、password retry、archive conversion、
  履歴からの detached open は未検証の follow-up check とする。欠陥とは断定しない。
  B では履歴専用 prepare/acceptance lifecycle を追加しない。

### レーティング一覧: 初回・再ソート・reload

- 現行 build worker は DB 行復元まで。受理後の `install_rating_view_rows` が UI で sort/materialize、pin lookup、synthetic
  path の `start_loading_items` を行う。ローカルの sort、★変更による行削除/再配置、reload、Back 復帰もこの入口へ来る
  (`src/rating_view.rs:60-120, 123-154`,
  `src/app.rs:24833-24855, 24879-24922, 24933-24997, 25009-25081`)。build worker を final order + pin
  metadata + Phase A exact-key snapshot/projection まで拡張し、初回/DB reload は完成結果だけ受理する。
- ローカル sort も**worker-side rebuild**にする。worker が rating DB の現在行と pin を読み、希望 sort/★数で最終 rows/items と
  exact-key snapshot/投影を作る。既存 mutable rows/snapshot を貸さず、UI で全行 clone・cold query・lock 待ちをしない。現行の UI 内
  sort と★変更の in-place 行更新は受理後の一括 install に移す (`src/app.rs:13875, 14720, 24918-24977, 30644-30656`)。表示中の
  sort/rows/items/投影は受理まで旧版を保ち、希望 sort は要求内に置く。★の希望 sequence は一覧 UI でなく、単件/一括の共通 rating DB
  **成功 commit 境界**で進める。既存 App-global `rating_session_write_generation` は commit 後に進むので、この値を
  prepare/accept stamp とし、現在または parked の rating view の最新希望も dirty にする。失敗では進めない
  (`src/app.rs:55362-55377, 55384-55427, 55547-55560, 55570-55589`)。これで metadata panel と fullscreen キーの
  `set_rating`、selection/undo 等の共有書込を一括で覆う (`src/ui_metadata_panel.rs:2689-2692`,
  `src/ui_fullscreen.rs:28626-28631`)。prepare 中の sort/★/reload は最新希望へ収束し、旧結果を live rows に混ぜない。
- 履歴の `Rating { stars }` は `Restored` の時点で typed surface と loading address を採用する。items flag と
  synthetic current_folder は worker result の受理時に切り替わる。Back/Forward の current target は受理前から
  ★N である (`src/app.rs:20094-20120, 20474-20507, 24900-24945, 25173-25191`)。
- `RatingDb` の全書込接続と direct SQL writer は process-wide の rating write stamp も共有する。
  `RatingDb::{set_user_rating,set_user_ratings,set_imported_rating,copy_entry_key,move_entry_key,clear_all}`、
  book page copy/move、ZIP / rename key migration、metadata import、metadata cleanup を含む。
  複数 store を巡る migration / cleanup / book-page mapping は全体を guard し、store 間の隙間も read へ公開しない。worker は DB read 前後の
  `active_writers=0` と同じ `completed_writes` を要求し、UI 受理でも同じ stamp を照合する。
  `rating_session_write_generation` は App のユーザー操作意図、shared stamp は他 connection の migration / copy / move を含む
  read freshness に使う。受理済み rating view は shared completed count の進行時に rebuild する。
- rating install の UI `prewarm_rating_cache` / `prewarm_grid_tags` を通さず、worker が確定行の★を index cache にし、
  tags DB の表示タグを一括で読んで同じ prepared result に載せる。XMP rating hydration は従来どおり可視項目の後続 worker。
- 受理後 P2 修正: tag prewarm も process-wide `TAG_WRITES` の read 前後の安定 stamp を持ち、UI 受理時に再照合する。
  stale な tag map は install せず再 prepare する。`TagsDb` の単件/一括更新、sidecar import、legacy import、
  tag 管理改名、book page copy/move は同じ `TagsDb` 境界を通る。直接 SQL の metadata import、
  orphan cleanup、rename/content-identity migration と hard purge は各 transaction 全体を guard する
  (`src/tags_db.rs:87, 262-956`, `src/metadata_transfer.rs:775-989`, `src/metadata_cleanup.rs:750-849`,
  `src/rename_key_migration.rs:1135-1752`, `src/rating_view.rs:252-273`, `src/app.rs:24930-24957`)。
- 受理時は context/surface generation、★数、最新 build/reload/sort sequence と `rating_session_write_generation`、希望
  sort、source items_generation、edit epoch を照合し、rows/items/投影を原子的に install する。order は不変候補 sequence
  が保証する。generation/★ write generation/edit epoch の拒否は最新希望から rebase。`RatingViewPending` に sequence が必要
  (`src/rating_view.rs:60-90`, `src/app.rs:24886-24909, 25017-25022, 25074-25117`)。

### 表示、context、検証と境界

- **2026-09-23 利用者決定: Phase B は通常フォルダと同じ fullscreen 表示規則。** snapshot/投影の受理後は通常の `open_fullscreen` と frame
  preparation を通し、別の readiness gate、offscreen producer、完成 paint-plan 証明、編集描画失敗時の grid 戻しを作らない
  (`src/app.rs:51002-51005`, `src/ui_fullscreen.rs:25652-25723`)。通常経路は comic → final →
  edit/conceal/local-adjust/erase → raw fs cache → thumbnail を利用可能な順に選ぶ。編集 result 未完成なら mask 等では
  raw/thumbnail や既存 holdover を経て後で編集結果へ切り替わり得る
  (`src/ui_fullscreen.rs:9127-9134, 9160-9204, 10299-10424, 25708-25723`)。カラー化/LUT は既存の raw fallback
  抑止と色を合わせた通過表示を**そのまま共有**する (`src/ui_fullscreen.rs:9167-9173, 10414-10423, 25710-25723`,
  `docs/display-pipeline.md:1922-1925, 1936-1939, 2079-2085`)。B が直すのは page edit を選択できないことだけ。描画結果
  pending/失敗時の振舞いを route 別に変えない。
- snapshot と投影、pending/sequence は `ViewerContextBundle` に属し、mount/swap と cancel/drop を同じ context で扱う
  (`src/app/viewer_context_registry.rs:1021, 2194, 2836-2851`)。兄弟 context の index は渡さない。§1.267 の
  collection root 直接 Image は独立 context に immutable prepared 順と session を持ち、search/rating 由来 Image
  はその root session を推定継承しない
  (`src/app.rs:46501-46516, 46553-46613, 46633-46641`、`docs/collection-playback-plan.md:460-461, 473-477`)。collection
  root への復帰は Phase A の collection revision gate に任せる (`src/app/collection_grid.rs:1887-1930`)。
- テストでは search と rating の ignored 二件を un-ignore して production prepare/acceptance を通す。履歴 Folder/ZIP 二件は非 ignore の baseline 回帰 guard として保持する。search の連続 rebuild (新 key
  の lookup と旧 key の再利用)、sort/drill 変更、G→G+1 中の B 拒否と `Done` 後の再 prepare、FIFO batch 無欠落、rating の local
  re-sort/reload と prepare 中の sort/★変更を追加する。metadata panel/fullscreen の★ DB write が in-flight rating
  結果を拒否する race と、snapshot のない通常フォルダでの edit write が in-flight search/rating 結果を拒否する race を検証する。
  edit writer を commit 直前と直後で停止する試験、二 writer が重なる試験で active 中/completed 変更後の stale 受理を拒否する。各
  route の古い sequence/epoch/context 拒否、通常フォルダと同じ未完成 edit fallback → 編集結果 paint、通常フォルダ対照を追加する。10,000 hit で
  worker DB batch 数・UI pass 時間/全件 clone 無し・stale drop を計測する。
- **Phase A2 との境界**: B は search/rating の初回/置換/再ソート受理まで。受理後の Snapshot Lock、rename、metadata
  import、content-identity restore、late write、兄弟 context 間の編集再同期、filename-stack grouping/flat 切替は A2 のまま
  (本書「Phase A2」、`src/app/snapshot_ops.rs:826-1144`, `src/filename_stack_ui.rs:338,752`)。B の検索 tick や
  rating local sort を A2 の汎用事後整合機構として流用しない。
- 実装照合: search worker / 受理 / rebuild は `src/global_search_ui.rs:1256,2114,2336`、rating worker / 受理 / request は
  `src/rating_view.rs:166`, `src/app.rs:24883,24953`、共通書込境界は `src/app.rs:55505,55530`。
  exact-key stamp と bundle swap は `src/app/page_edit_snapshot.rs:194`, `src/page_edit_write_epoch.rs:17`,
  `src/app/viewer_context_registry.rs:2151-2162`。上の旧行番号は実装前の調査位置である。
  受理後修正位置: `src/global_search_ui.rs:2221,3189` (accept / query intent)、`src/zip_key_migration.rs:73`
  (page/rating migration 全体)、`src/metadata_cleanup.rs:738,838` (orphan delete)、`src/metadata_transfer.rs:773,822`
  (import batch)、`src/rating_db.rs:118,285,311,375,406,463` (全 connection)、`src/rating_view.rs:176,252`
  (worker read stamp / prewarm)、`src/app.rs:24897,24930,27506,27950,36275-36350` (accept / install / book page copy/move)。

## 段階 A2 設計 (2026-09-24、install 後の整合)

### 正本、受理、表示

- 対象は Phase A/B で初回受理済みのサブ展開・collection・Ctrl+G・rating の実ページ。
  編集正本は各 `ViewerContextBundle` の `PageEditSnapshot` 一つ、idx map はその投影だけ
  (`src/app/page_edit_snapshot.rs:3-5,44-65,67-77,153-184`,
  `src/app/viewer_context_registry.rs:1034,2243`)。新しい編集用 index map は作らない。
  ZIP/PDF を含む page key は既存 `page_key_for_grid_item` に従う。サムネイル挙動は変えない。
- 同じ並びの編集変更は、worker が変更 key だけ DB から読み、受理済み候補の **worker 作成の不変 page-key 順**で
  該当 idx の sparse delta を作る。UI は小さな delta を keyed snapshot と投影へ同時適用する。
  多数 key・順序変更・通知欠落時は worker が同じ `load_and_project_stable` で全体を再構築し、完成品だけ移す。
  page-key 順は items の版を示す入力であり、第二の編集 index map ではない。初回 prepare が候補と共に作り、
  items を直接変更する経路は UI で compact key 順を 1 frame 最大 2,048 件・2 ms ずつ構築し、
  `finish` と全体 DB prepare は worker に渡す。100,000 件で 49 frame、prepare 開始 0.011 ms、
  frame 最大 1.291 ms を測定。UI で巨大な `items` / snapshot を
  clone して worker に渡さない (`src/app/page_edit_snapshot.rs:94-151`,
  `docs/ui-responsiveness.md:335-348`)。
- 共通受理条件は context ID、top-level surface/対象 identity、source と target の items generation・順序版、
  要求 sequence、現行 App-global `page_edit_revision` (`src/app.rs:14373,30850`)、
  `PAGE_EDIT_WRITES` の active=0 かつ同じ completed count。
  worker の DB 読込前後も書込 stamp を検査する
  (`src/page_edit_write_epoch.rs:17-93`, `src/app/page_edit_snapshot.rs:196-223`)。不一致・置換 cancel は旧候補の
  index を新しい items に貼らず、残る最新希望を owner が再 prepare (次 tick 頼みにしない)。DB/error は
  旧受理表示を保って初回に通知し、100 ms から最大 4 s の有界 backoff を `request_repaint_after` で予約して
  成功まで worker 再読込を続ける。書込変化は待機を短絡し、view 変更は retry を破棄する。
  UI に sleep/busy loop は置かない。context 破棄は pending を cancel する。
  Phase A の subfolder/collection 初回 prepare は非 stamp の `load_and_project` と local revision gate だったため、
  A2 で同じ stable stamp と通知 cursor を受理結果に結んだ (`src/app/subfolder_expansion.rs:1333-1387,2114-2122`,
  `src/app/collection_grid.rs:245-252,1887-1936`)。Phase B の search/rating は既に stamp を照合する。
  Collection navigation の retained snapshot にも同じ stamp を保持する。旧 reuse は local revision だけで
  (`src/app/collection_navigation.rs:1590-1619`)、preflight 完了後の landing にも外部書込 stamp がない
  (`src/app/collection_navigation.rs:2411-2454,2456-2505`)。reuse 前と root preflight 最終受理で active=0・
  completed 一致を検査し、不一致なら retained snapshot を捨て full prepare へ戻す。遅延する物理 source open は
  編集書込だけで取り消さず、最終 landing で同じ stamp を再検査して stale な retained snapshot を捨てる。
  preflight の物理 target 有効性には編集 stamp を混ぜず、着地側が stale owner を再準備する。
  物理 child は通常の物理 loader で編集を読む。root に戻る際は installed generation が無効なので既存の
  collection prepare が全体を再準備する (`src/app/collection_grid.rs:1145-1213,1384-1400`)。worker 新規 prepare も
  stamp を root source owner に運ぶ。revision と stamp は別々に照合する。
  未 commit の同一 bundle 編集は DB 結果で上書きしない。既存 keyed snapshot の変更 key を小さな overlay として
  新候補の worker 算出 idx に移し、local-adjust の先行 memory 更新と後着 commit を区別する
  (`src/app.rs:62105-62122,62240-62284,62307-62327`)。編集中に revision が動けば結果を破棄して再 prepare。
  投影の受理は items generation・key 順に結び、同じ idx の別ページへ編集を漏らさない。worker 待機中の表示と受理後の fullscreen は通常フォルダと
  同じ `open_fullscreen` / raw・holdover・編集結果の既存選択規則を使う (`src/ui_fullscreen.rs:9160-9204`、本書「段階 B 設計」)。

### 遷移ごとの変更点

- **Snapshot Lock**: `snapshot_ops.rs:766-796` が subset と世代を置換した後、検索由来なら
  `clear_page_edit_state`、その他は origin prefix で同期 rehydrate する (`src/app/snapshot_ops.rs:818-827`)。
  正しい初回 install の後でも synthetic origin の編集が消える。固定解除時の saved-items 復帰も prefix、
  固定一覧への復帰も clear/prefix (`src/app/snapshot_ops.rs:1022-1048,1133-1145`)。
  lock/subset・元一覧・list 復帰の各候補を同じ page-key prepare に通す。選択/フィルタ版、
  snapshot generation ID、source items generation と新 items generation を gate に使う。
  フォルダ内 Ctrl+F subset は既存どおり残す。既存 subset 作成は UI 側、DB 読込/投影は worker 側。
- **rename**: 移行完了 worker の受信後、影響 context を選別する処理は既にあるが、synthetic view でも
  `current_folder` prefix を UI から読み直すため keyed owner が消える
  (`src/app.rs:33434-33523,33609-33644,33689-33699`)。現行の無関係 context 不変更・ドラッグ中 main の
  繰延べは維持 (`src/app.rs:33602-33608,33637-33644`)。移行 report の旧/新 path 範囲を編集通知へ渡し、
  現行の synthetic items 全走査 (`src/app.rs:33627-33635`) は今回維持し、
  移行完了後の実 key に該当する bundle だけ worker 再 prepare。旧/新 path、migration job/完了順、
  context/items/編集 stamp を gate にし、移行失敗・journal 再試行は実 DB 状態から再収束する。
- **metadata import**: UI の候補 index は `current_folder`/archive path で affected 判定するため synthetic
  view を落とし得る (`src/app.rs:31800-31820`)。worker は既に各 item の exact page key をまとめて読むが
  idx-only `PageStateResult` を返し、受理は idx maps だけ置換して snapshot を更新しない
  (`src/app/metadata_import_refresh.rs:363-435`, `src/app.rs:32343-32349,32372-32402`)。
  virtual view では既存 worker の idx-only `PageStateResult` で keyed owner を上書きせず、
  受理済み compact key 順を共有して別 worker が exact-key DB を再読込し投影を返す。
  import transaction/refresh request、context/items generation、書込 stamp を受理条件に加え、失敗なら旧正本を
  保ち再取得する。既存の rotation・rating・tag・thumbnail refresh はそのまま扱う。
- **content-identity restore**: 完了時に物理一覧は prefix rehydrate、synthetic view は無条件 clear
  (`src/app/content_identity_restore.rs:421-450`)。復元の page-edit DB 書込 guard が出す通知を
  各該当 bundle が消費する。物理一覧の現行経路は維持し、virtual は現在の実 key と照合して worker で再投影。
  restore request/完了 report と context/items generation・書込 stamp で古い結果を拒否する。部分成功・失敗時も
  report の成功 key と authoritative DB 状態で収束し、無関係 virtual view を clear しない。
- **late writes**: UI 同一 context の edit は `sync_prepared_page_edit_key_for_idx` が snapshot と投影を更新する
  (`src/app.rs:30836-30850,61953-62078`)。その直接 sync を通らない worker 書込と別 context の commit は
  既設投影へ届かない。local-adjust は memory を先行更新して worker 完了が後から来る
  (`src/app.rs:62105-62122,62240-62284`) ので、origin bundle の未保存値を通知の DB 読みで戻さない。
  `PAGE_EDIT_WRITES` は全 writer の active/completed を数えるが key 通知を持たない
  (`src/page_edit_write_epoch.rs:17-93`)。成功 commit の guard を閉じる前に実 page key の変更通知を発行し、
  bulk/import/migration は範囲または全再照会通知にする。失敗・部分成功で key が不明なら保守的な再照会。
  各 guard は成功/失敗とも通知 (失敗時は unknown) を軽量 log へ置いてから、既定どおり completed を加算し
  active を減らす。bundle は通知連番 cursor と最後に処理した completed count を持つ。active=0 の時点で
  通知数と completed 差分が合わない/overflow なら全再 prepare とし、通知欠落でも編集を取り逃がさない。
  nested/multi-store guard も各 guard ごとに一通知、または差分で検出できる欠番を残す。panic で通知を
  確定できない場合も guard の completed 加算は止めず、欠番から全再 prepare する。通知 owner は commit 後に
  `egui::Context::request_repaint` 相当の wakeup を発行し、入力のない active view も drain する。全 viewer
  bundle は通常 frame の drain と remount の双方で cursor を照合し、park 中の log overflow は全再 prepare。
  worker の cold DB と大きな投影だけを worker へ置き、UI は通知を束ねて該当 bundle を dirty にするだけ。
  実装では 128 key 以下の通知を受理済み compact 順で worker 側に絞って読み、超過・unknown・欠番は
  full prepare にする。対象 key batch の completed count を worker の読込 stamp と照合し、通知集約後に
  別 writer が完了しても欠けた key を受理しない。
  read-only open/switch/close は通知しない。
- **sibling contexts**: snapshot は bundle swap と共に動き、physical detached fork は仮想一覧 owner を
  UI で複製しない (`src/app/viewer_context_registry.rs:2243,2848,2912`)。書込通知は process 共通の事実だが、
  dirty key/要求/結果は各 bundle が所有する。main・active detached は各自の不変 key 順で再投影し、
  parked bundle は通知 cursor/dirty 範囲だけ保持して remount 時に worker refresh を要求する。
  通知 log が切れた parked bundle は全再 prepare。兄弟の idx map、選択、texture、shared preview を
  read-only 遷移で変更しない。rename の既存 parked 選別は再利用する (`src/app.rs:33647-33685`)。
  不変 page-key 順は既存 prepared order と共有できる Arc backing を優先し、重複文字列を保持する場合は
  compact arena/offset へ変える。10,000 key で追加保持量 1 MiB 以下を測定上限とし、巨大サブ展開と
  parked 複数 bundle の合計 peak も計測する。キーは省略しない。
  10,000 個の通常 key では compact 順 350,004 bytes、3 bundle 同時保持 1,050,012 bytes を測定。
  10,000 key の単体構築 3.329 ms、全体受理 0.004 ms、1 key 差分受理 0.142 ms（テスト環境）だった。
  100,000 key の実際の UI 分割構築は上記の 49 frame・最大 1.291 ms。
  上限を設けない巨大 subfolder の総メモリと既存 rename UI 全走査は別途性能確認が必要。
- **filename-stack switching**: script worker が返すのは key 列だけ。UI が group/fallback・`StackView`・
  aggregate items を作る (`src/filename_stack_ui.rs:217-277,338-413`)。flat open と aggregate 復帰も UI で
  materialize して `swap_stack_view_items` が synthetic prefix rehydrate するため、サブ展開の flat 実ページ編集が消える
  (`src/filename_stack_ui.rs:690-701,726-775`)。script/fallback grouping、aggregate と flat の候補順、
  flat 実ページの keyed snapshot/投影を同じ worker prepare に移し、UI は候補を move/install するだけにする。
  戻り側も現行 flat での保存を取り込んだ worker 投影を受理してから aggregate へ替える。
  context、source items generation、subfolder snapshot/revision、stack request sequence、script/rule・separator・sort・
  display-order 版、候補順、書込 stamp で受理し、OFF/別フォルダ/別スクリプト結果は cancel/rebase。
  現行の fullscreen 中の script-result 保留と detached park 境界は維持 (`src/filename_stack_ui.rs:282-334`)。

### 既存経路、検証、出荷判断

- **既存 generic path の対照**: 同じ bundle 内の成功した保存は keyed sync が既にある。物理フォルダの restore は
  prefix rehydrate 済みで、`restore_completion_rehydrates_idx_page_edits_without_folder_reload`
  (`src/app/content_identity_restore.rs:674-703`) が対照。rename の無関係 context 保持と parked 選別は
  `src/app/tests.rs:55529-55568,55610-55680` が既存保証。A2 では virtual view にも同等の keyed 結果を
  要求する追加テストを置き、同一 bundle の mask 保存→snapshot/投影即時更新は旧コードでも通る対照テストを
  追加する。既存対照を弱めない。
- **fail-before/pass-after**: (1) mask を初回表示したサブ展開で Snapshot Lock→解除→list 復帰、
  (2) cross-folder virtual item の rename 後も新 key の mask、(3) synthetic view 表示中の metadata import で
  conceal/補正が snapshot と idx に一致、(4) virtual へ移動した後に restore 完了して対象編集が現れる、
  (5) install 後の local-adjust worker commit と削除、(6) window A の保存で parked/active の window B だけが
  対象 key を更新し B の read-only close では A が不変、(7) サブ展開 stack の aggregate→flat→close→再 open で
  mask が残る。各試験で旧/新要求の逆着、write 中と commit 後、items/order 変更、cancel を挟み、
  stale 結果が idx を汚さず最新希望へ再 prepare されることも確認。通常フォルダと ZIP/PDF key を対照にする。
  さらに collection と Ctrl+G からの Snapshot Lock 復帰、外部書込後の collection navigation retained
  snapshot reuse/最終 landing、通知 drop/overflow・panic、nested/multi-store writer、idle active view の
  wakeup と parked remount、失敗した full reread の後に新規書込なしで成功する retry を個別に検証する。
  10,000 件級で worker lookup/projection、追加 key memory、
  UI 受理時間を計測し、UI cold query・全件 clone が無いことを確認。
- **v4.1.0 判断**: 上の七遷移に原理的な実装不能は見つからない。ただし filename-stack は script worker が
  key のみを返す現構造から grouping・両順序・投影・flat/aggregate の受理 lifecycle を移す独立した大きな chunk。
  v4.1.0 に含めるなら単独の構造レビューと性能検証が必要。これを後送する判断なら filename-stack 切替の
  編集表示保証も同時に後送し、A2 全件完了とは扱わない。

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
