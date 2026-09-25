# 評価順のスナップショット固定 設計計画

最終更新: 2026-09-15

状態: 利用者仕様を反映した実装前設計。製品コードは未編集、Cargo / GUI は未実行。

本書は [レーティング操作・一覧ソート・タグジャンプ計画](rating-sort-and-tag-navigation-plan.md) のうち、
評価変更へ追従して表示順を動かす旧 Phase C 案を置き換える。完了済みのタグジャンプと名前 / 番号降順、
評価増減の操作規則は変更しない。

## 0. 決定と対象

評価昇順 / 降順は、一覧を開くか明示的に更新した時点の評価を一度だけ取得して並べる。
表示中に評価を変更しても行を移動しない。ファイルメニューの「最新の情報に更新」、割り当てた
`GridReload`、sort横の更新アイコン、選択中と同じ評価sortの再選択、一覧の開き直し、または
並べ替え条件の明示変更で評価を再取得し、その時点で再び並べる。

この仕様により、全表示経路を `GridDisplayOrderState` へ移す再構造は行わない。materialize 済みの
`items` 順そのものを評価順 snapshot とし、既存の index-keyed cache、selection、navigation、
thumbnail queue を並べ替え後に追従させる仕組みを追加しない。

対象は次である。

- `SortOrder::RatingAsc / RatingDesc` と未評価位置設定。
- 通常フォルダ、Ctrl+G / Ctrl+S / タグ、Smart Folder、サブフォルダ展開、ブックマーク、
  レーティング一覧の Normal sort。
- Standard order のコレクション。
- Remote の物理フォルダと、既に接続済みの特殊一覧。
- 評価列ヘッダで並べた詳細表示についても、評価変更だけでは既存行の相対順を変えない。

本 / ZIP / PDF のページ順、閲覧履歴、★固定 snapshot、Manual order のコレクションは対象外であり、
それぞれの固定順を維持する。コレクション Phase 4 / 5 の navigation / Remote 接続をこの実装から
先行させない。フォルダ代表サムネイルの候補順も既存4候補のallowlistを維持する。

## 1. 現行経路の確認

### 1.1 更新操作は追加不要

`KeyAction::GridReload` は既に `KeyContext::Grid` の操作として存在し、既定 chord は
`ChordList::EMPTY` である。ファイルメニューの `MenuCommandId::FileReload` は
「最新の情報に更新」を表示し、同じ `KeyAction::GridReload` を参照する。キーボード経路は
`handle_grid_keys`、メニュー経路は `ui_main.rs` から、ともに
`App::reload_top_level_grid(ctx)` を呼ぶ。

`reload_top_level_grid` は `TopLevelGridSurface` の網羅 match を持ち、Folder、各検索、Smart Folder、
サブフォルダ展開、Rating、ReadingHistory、Bookmarks、DriveList、Collection をそれぞれ既存の
再入場経路へ送る。`Snapshot` は意図どおり no-op、Smart Folder / サブ展開の準備中は二重起動しない。
したがって新しい Action、固定 F5、別メニュー項目は追加しない。操作カスタマイズで F5 等を割り当てる
既存仕様も維持する。

sort toolbar / popupの横には一覧更新アイコンを追加する。tooltipとaccessible labelは「最新の情報に更新」
とし、独自reload関数や疑似キー入力を作らず `reload_top_level_grid(ctx)` へ直接合流させる。通常一覧、
特殊一覧、Collectionのいずれも現在の `TopLevelGridSurface` が行先を決める。

選択中と同じ `RatingAsc / RatingDesc` をもう一度clickした場合も同じrouterへ送る。eguiの
`selectable_value` / combo responseは同値選択で `changed()==false` になるため、reload判定を
`changed()` だけに依存させない。control適用前のsortと `response.clicked()` を使い、別sortなら既存の
sort変更経路、同じ評価sortなら `GridReload` のどちらか一方だけを発行する。同一clickからsort変更と
reloadを二重発行しない。

### 1.2 現在の評価取得と解消する同期 I/O

現行の `start_loading_items_inner` は items install 後に `prewarm_rating_cache()` を呼び、
`RatingDb::get_many` を UI thread から実行する。Ctrl+G の batch poll も `hit.stars` を埋めるために
UI thread から `get_many` を呼ぶ。collection Grid は prepare 自体は worker だが、install 後の
`rebuild_visible_indices` より前に rating cache を受け取らないため、条件によっては point read へ
落ちうる。

評価順では DB snapshot が比較 key そのものになる。これらを比較中の lazy read に任せず、全対象 key を
worker で一括取得してから並べる。描画、comparator、`rebuild_visible_indices`、target loop から SQLite を
読まない。

既に worker で rating を取得している Smart Folder とサブフォルダ展開、`stars` を持つ Ctrl+G hit は
その snapshot を再利用する。別の DB read を重ねない。

### 1.3 既存の selection / viewer 復元境界

評価順の実装は次の既存 identity 境界を使い、raw index を再読込後へ持ち越さない。

- 通常フォルダの明示 reload は `preserve_cursor_hint_for_reload` / `select_after_load` と
  `scroll_to_selected` を使う。同名衝突しうる新しい非同期 request では exact path を request 内に持つ。
- Smart Folder resident root は session の active child / saved selection と scroll snapshot を持つ。
- Collection は `CollectionGridViewportAnchor { entry_id, source_key }` で選択と checked を再解決する。
- Bookmark は stable row ID、Ctrl+G streaming rebuild は content key、Rating view は rating key を使う。
- Remote session release は `GridItem::perf_key()` で開いていた項目を再解決して fullscreen を戻す。
- 通常フォルダの外部変更は main viewer が開いている間は既存どおり reload を defer する。

評価変更だけなら `items` を入れ替えないため、選択中 / fullscreen 中の同じ index が同じ項目を指し続ける。
明示 reload または sort 変更で items を入れ替える場合だけ、上記の owner が stable identity を再解決する。

## 2. 比較契約

### 2.1 typed availability

評価 key は `Option<u8>` 相当で扱う。

- `Some(0)`: 評価可能だが未評価。
- `Some(1..=5)`: 評価済み。
- `None`: 評価非対応。Stack / SearchContainer 等の集約セルもここに含む。

0 を評価非対応 sentinel にしない。`None` は昇順 / 降順とも評価 domain の後ろへ置き、相互は名前昇順で
確定する。物理一覧の folder / media category と `GridDisplayOrder` は既存どおり先に適用し、同じ category
内だけを評価で並べる。コレクション Standard order も既存の category rank を維持する。

未評価位置は既決定の `RatingSortUnratedPosition` を使う。

| 設定 | 降順 | 昇順 |
| --- | --- | --- |
| `BetweenThreeAndTwo` | 5, 4, 3, 0, 2, 1 | 1, 2, 0, 3, 4, 5 |
| `BelowAll` | 5, 4, 3, 2, 1, 0 | 0, 1, 2, 3, 4, 5 |

昇順は評価可能 domain の完全な逆順とする。同じ評価の tie は方向にかかわらずファイル名昇順である。
比較は純粋関数にし、2設定 × 2方向 × 0..=5、`None`、同値 filename tie を table test で固定する。

### 2.2 shared metadata の最小拡張

`ListingSortMetadata` へ `rating: Option<u8>` を追加する場合、既存の `new(mtime, size)` は
`rating=None` のまま互換にし、rating snapshot を持つ producer だけが builder で設定する。
Rating sort を rating 未準備の古い comparator へ流して全行を未対応扱いにする fallback は禁止する。
Rating variant は rating-aware comparator を通すか debug test で失敗させる。

`RatingSortSpec { order, unrated_position }` 相当の小さい値を worker request に焼き付ける。設定を worker
完了時に引き直さない。`SortOrder`、未評価位置、必要なら `GridDisplayOrder` のいずれかが変わった結果は
stale として破棄し、新しい request だけを適用する。

## 3. snapshot の所有と寿命

### 3.1 items 順が正本

別の永続 `thumbnail_order`、全画面共通 rank map、App-global pending bool は追加しない。worker が評価を
取得して sort を終えた `items + aligned sidecars` を既存 install 境界へ渡す。accepted install が
`items_generation` を進めた時点から、その items 順が評価順 snapshot である。

snapshot は次のどれかが accepted になるまで生存する。

1. 別フォルダ / 別 top-level surface / 別 collection を開く。
2. 「最新の情報に更新」または割り当て済み `GridReload` を実行する。
3. sort横の更新アイコン、または選択中と同じ評価sortを再選択する。
4. toolbar / menu の sort、未評価位置、collection Standard sort を明示変更する。
5. filesystem membership change、collection revision、Smart Folder 定義変更等により、その surface の
   既存仕様が full requery / rescan / reprepare を accepted する。
6. 一覧を明示的に閉じて後で開き直す。

次は snapshot を失効させない。

- 単一 / 複数の直接評価、評価増減、リング preview / commit。
- Undo / Redo。
- XMP hydration、metadata import 後の rating cache refresh、別 viewer context の session write。
- rating filter / facet / Ctrl+F の変更、thumbnail / details 表示切替。
- fullscreen open / close、Smart Folder resident root から child を開いて同じ resident root へ戻る操作。

評価変更と同時に別理由の full reload が accepted された場合は、その reload が新しい snapshot 境界になる。
rating write 自体を reload 理由へ昇格させない。

### 3.2 worker snapshot の確定点

rating-aware prepare request は surface identity、surface / items generation、destination identity、
`RatingSortSpec`、開始時の `rating_session_write_generation` を持つ。worker は read-only `RatingDb` を開き、
normalized key を 500件単位等の既存 `get_many` chunk で取得し、未登録を0として aligned facts を完成させる。

apply 時に同じ context / surface / destination / sort spec であることを再確認する。開始後、install 前に
同じ process で commit された session write は cache 表示だけでなく初回 sort key にも重ね、accepted install
直前までを一つの snapshot にする。install 後の write は cache / filterだけを更新し、順序へ反映しない。

cancel、別navigation勝利、context retire、surface変更、より新しい request、worker disconnect で古い結果を
捨てる。別 context の一覧へ結果を適用しない。DB read に失敗した場合は無言で全件0として正常完了せず、
既存一覧を保持できる reloadでは保持して通知する。初回 navigation では一覧を開けなくしないため、
評価順を適用できなかったことを通知して deterministic な名前 tie 順で install する。

## 4. surface ごとの接続

| surface | 評価 snapshot の取得 | 再取得 / install | 固定する境界 |
| --- | --- | --- | --- |
| 通常の物理フォルダ | scan 後の ratable path keyをworkerで `get_many` | folder open、sort変更、`GridReload`、filesystem full reload | category block内のitems順 |
| ZIP / PDF / 本のページ | 取得しない | 既存enumerateのみ | `BOOK_READING_PAGE_ORDER` / page order lock |
| Ctrl+G | search hitの`stars`。現行UI-thread batch readはsearch workerへ移す | query / reload / drill materialize | hit / containerのitems順 |
| Ctrl+S / タグ | query worker結果のpathを同workerまたはrating prepareでbatch取得 | query / reload | result items順 |
| Smart Folder | 既存workerの`ratings_by_path`を使う | open / explicit reload / definition rebuild | resident sessionのroot順 |
| サブフォルダ展開 | 既存workerのrating cacheを使う | scan / explicit reload | prepared snapshot順 |
| Bookmark | bookmark build workerへbatch rating factsを追加 | open / refresh | stable bookmark row順 |
| Rating view | 全行が同じ★なのでRating sortはfilename tieのみ | 既存view build / membership refresh | `RatedAtAsc/Desc`は従来どおり別sort |
| Collection Manual | ratingを表示/filter用にだけ取得 | collection prepare | DBのmanual position |
| Collection Standard | collection prepare workerでrating factsも揃える | open / explicit reload / relevant revision | definition-owned Standard sort順 |
| Reading history / ★固定 | 評価sortを適用しない | 既存reload / no-op | MRU / snapshot順 |
| Drive list | 評価sortを適用しない | drive refresh | drive順 |

更新操作のCollectionでの意味はorder modeごとに分ける。

- 通常 / 特殊一覧: 現在surfaceのquery / scanをやり直し、Rating sortなら最新評価で再sortする。
- Collection Standard: `open_collection_grid(current_id, None)`相当の既存snapshot requestから最新revisionを
  取り直し、definition-owned sortがRatingなら最新評価でprepareする。
- Collection Manual: 同じsnapshot requestでmembership、missing解決、rating badge / filter factsを更新するが、
  DBのmanual positionを再sortしない。更新アイコンを無効にせず、Standardへ暗黙変更しない。

どの経路もreload開始前にsurface固有のstable selection identityをcaptureし、accepted install後に再解決する。
同じ評価sortの再選択でもraw indexやscroll先頭へ落とさない。対象が消えた場合だけ各surfaceの既存fallbackを使う。

Folder 用には scan と rating facts を別々の untyped `Option` に残さない。既存の
`FolderOpenScanPurpose` / `FolderPaneOpenReady` と同じ request ownerが、scan、任意の exact selection、
rating facts、sort spec を成功 / cancel / errorまで一緒に持つ `PreparedFolderListing` 相当を返す。
Rating sort時の直接 `load_folder` もこのworker prepareへ合流させ、UI threadの `prewarm_rating_cache` を
比較前提にしない。非Rating sortの既存即時経路まで一度に非同期化する必要はない。

Special view は各既存 pending ownerを拡張する。全surface共用のApp-global
`rating_sort_pending: Option<_>` を追加しない。Smart Folder、subfolder、collectionの既存 generation / cancel
stampをそのまま使う。

## 5. 評価変更後の処理

### 5.1 書込、Undo、外部反映

既存の `write_user_rating_shared` / batch transaction、session generation、XMP writer、folder count、
content identity、Undo recordを維持する。成功した変更だけを cacheへ公開する契約も変えない。

変更後は次だけを行う。

1. 現在contextの `rating_cache` と表示中のrating badge / details revisionを更新する。
2. rating filter、Rated / Unrated facet、Rating view固有membershipを既存規則で更新する。
3. Ctrl+Gの`all_hits.stars`とdrill件数を更新する。
4. selectedが非表示になった場合は既存nearest-visible policy、checkedは既存WYSIWYG policyを適用する。

`items` のsort、Smart Folder / collectionの再prepare、thumbnail queueの全再投影は行わない。Undo / Redo、
XMP hydration、metadata import refresh、viewer-context swapのsession write同期も同じ契約である。

Ctrl+Gの現行 `rebuild_items_from_global_search()` はrating変更後に最新starsで全体を作り直すため、Rating sortを
追加したまま呼ぶと行が動く。rating変更時は、stable item / container identityを使って既存行の相対順を維持し、
消えた行を除去、既存行のcountを更新、新たにfilterへ入った行だけcandidate順で末尾へ追加する
membership-only rebuildへ分ける。query、sort変更、`GridReload`だけが通常のsorted rebuildを呼ぶ。

Rating viewは「★Nの一覧」というmembership自体が機能なので、Nから外れた行の除去とNへ入った行の追加を
続ける。RatingAsc / Descでは全行の評価が同じでfilename tieになるため、このmembership更新が他の評価bucketを
使った再配置になることはない。既存`RatedAtAsc / Desc`の意味も変えない。

### 5.2 visible_indices と Details

thumbnail表示では `visible_indices` が raw items index順を保つため、rating / facet filterを再計算しても
survivorの順序は動かない。Rating sort専用のvisible permutationは不要である。

詳細表示の `DetailsSortKey::Toolbar` は `visible_indices` をそのまま使う。既存の評価列ヘッダ
`DetailsSortKey::Rating` も利用者仕様に合わせ、ヘッダclick、明示reload、items install時だけ評価で
`details_order` を作る。rating変更に伴うmembership再計算では、現在の `details_order` に残るsurvivorの
相対順を維持し、新たにvisibleになったrowだけraw items順で末尾へ追加する。既存の `details_order` を使えるため、
全items用rank mapや新しいcontext fieldは不要である。別列のheader sortはその列の既存再構築規則を維持する。

## 6. Collection と Remote

### 6.1 Collection

`CollectionOrderMode::Manual` は評価sort選択の影響を受けず、保存済みmanual positionを正本にする。
Standardだけがdefinition-owned `standard_sort` を使い、Rating variant時は
`prepare_collection_snapshot` worker内でrating DBをbatch取得する。prepared snapshotへsparse rating cacheも
含め、`install_collection_grid_items` が `rebuild_visible_indices` を呼ぶ前にcacheを移す。

rating writeはcollection DB revisionを進めないため、revision watchから自動prepareしない。表示中セルのbadgeと
filter membershipだけを更新する。「最新の情報に更新」、collection reopen、entry追加 / 削除 / relink等で既存
revisionが進んだprepareは新しいsnapshotを作る。childからrootへ戻る既存latest-snapshot再取得もreload境界であり、
entry ID、source keyの順でselectionを復元する。

### 6.2 Remote

Remoteの物理フォルダ一覧はUI thread外の `ContainerService::recompute_folder_listing` で構築される。Rating sort時は
同じworkerでread-only rating DBをbatch取得してから応答順を決める。book / ZIP / PDF pageのlock判定をratingで
解除しない。

Remoteへ返したcontainer payloadは一つのimmutable snapshotとみなす。Remoteから評価を書いた直後は現在rowの
rating表示だけを更新し、client側で自動reorderしない。明示reload、navigation、sort変更による次のlist requestが
新しいrating snapshotを返す。UI側の `SetSortOrder` は既存wire値とsettings保存を使い、新Actionを追加しない。

既に接続済みのSmart Folder等は各prepared snapshotを使う。native CollectionのRemote閲覧はcollection Phase 5の
未接続範囲なので、この設計を理由に先行実装しない。Phase 5で接続するときはCollection Standard / Manualの同じ
contractをpayloadへ投影する。

## 7. 実装段階と残工数

### Phase R1: pure key と保存設定

- `RatingAsc / RatingDesc`、`RatingSortUnratedPosition`、純粋比較、serde / DB / Remote wire、toolbar 8→10の
  独立one-time markerを追加する。
- canonical 8だけを10へ拡張し、custom / reorder / partial / empty / 後から隠した候補を補完しない。
- fixed-order / representative allowlistを先にtestで固定する。

見込み: 0.5〜1日。

### Phase R2: worker snapshot とproducer

- physical folderのtyped prepared listingを追加し、rating comparison用DB readをUI thread外へ出す。
- Ctrl+G / Ctrl+S / tag / bookmarkへbatch factsを渡す。
- 既存Smart Folder / subfolderのrating snapshotをsort keyへ接続する。
- Collection Standardへrating factsとinstall前cache handoffを追加する。

見込み: 1.5〜2.5日。

### Phase R3: mutation時の固定と選択

- rating write / Undo / hydration / session syncはcacheとmembershipだけを更新する。
- Ctrl+G membership-only rebuild、Details rating headerのsurvivor順維持を追加する。
- 明示reload / sort変更でstable identity selectionを復元する。

見込み: 0.75〜1.25日。

### Phase R4: Remote、文書、gate

- Remote worker batch rating sortとclient row patchを確認する。
- manual / spec / keymap説明、focused regression、full gate、verification buildを完了する。

見込み: 0.75〜1.25日。GUI確認時間は含めない。

合計は約3.5〜6開発日、製品差分はおおむね16〜22ファイルを見込む。中心は `settings.rs`、
folder load / top-level router、既存の検索・Smart Folder・subfolder・bookmark producer、collection prepare、
Remote containerである。33ファイル規模の `GridDisplayOrderState`移行、items / cache permutation、
thumbnail priority再設計、detached predicate変更は不要である。
producerごとのrating facts接続は残るため、単一comparatorだけの小変更にはならない。

## 8. 回帰試験

### 8.1 snapshot契約

- 6評価値 × 2未評価位置 × 2方向、`None`、同値filename tie。
- RatingDesc / Ascで一覧を開き、単一 / 複数評価、増減、Undo / Redo後もsurvivor順が不変。
- `GridReload`、menu reload、sort変更、未評価位置変更後だけ最新評価で再sort。
- 選択中のRating sort再clickとsort横更新アイコンが `GridReload` と同じsurface routeを一度だけ発行する。
- 同値sort clickを `changed()` だけで判定せず、通常 / 特殊一覧とCollection Standardで再取得・再sortする。
- Collection Manualの更新はmanual positionを維持し、membership / metadataだけを更新する。
- XMP hydration、metadata import、別context session write後にbadge / filterは更新されるが順序は不変。
- rating filterでrowが消える / 現れる場合、survivor順、newcomer末尾、selected / checkedの既存policy。
- thumbnail / Details切替、Details Rating header、別header sort。

### 8.2 surface

- physical Folder、Ctrl+G flat / drill / ZIP hit、Ctrl+S、tag、Smart Folder resident root、subfolder、bookmark。
- Rating viewのmembership更新とRatedAt sort維持。
- Collection Manual全variant不変、Standard Rating snapshot、revision refresh、childからroot復帰のanchor。
- Remote physical folder parity、書込後row不動、explicit reload後再sort。
- book folder、ZIP / PDF page、reading history、★固定、Stack / SearchContainer aggregateの非rating扱い。

### 8.3 lifecycle / I/O

- rating prepareのcancel、replacement、surface退出、context retire、stale sort spec、worker disconnect / DB error。
- worker開始後install前のsession write overlayと、install後writeの順序固定。
- UI threadからrating DB batch / point readを行わず、comparison中のDB accessが0であること。
- reload後に同じpath / entry ID / rating key / bookmark IDを選択し、必要時だけensure-visible。
- main viewer中のfilesystem refresh defer、Remote fullscreen key restore。

## 9. 完了条件

- 利用者操作で評価を変えても、Rating sort中の既存行が移動しない。
- 「最新の情報に更新」と割り当て済み`GridReload`が同じrouterから最新評価を再取得し、再sortする。
- sort横更新アイコンと選択中Rating sortの再選択も同じrouterへ合流し、selection identityを保持する。
- rating filter、Undo / Redo、外部rating反映、Rating view membershipが退行しない。
- Manual / fixed page orderを一度もRating comparatorへ渡さない。
- rating sort用DB I/Oがworker batchにあり、UI / draw / comparatorからSQLiteを読まない。
- items順をsnapshot正本として使い、全体display-order ownerや追加のApp-global sentinelを導入しない。
- phaseごとのfocused tests、`cargo fmt --check`、UI glyph check、`scripts/test-full.ps1`、
  `scripts/build-dev.ps1`が成功する。通常profile / GUIはagentが起動しない。
