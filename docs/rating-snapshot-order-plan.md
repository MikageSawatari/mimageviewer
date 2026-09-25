# 評価順のスナップショット固定 設計計画

最終更新: 2026-09-25

状態: 実装前設計 第2版。2026-09-15 の初版 (`d05fc1a76`) を、独立レビュー (GPT-6 Sol / xhigh、
2026-09-25、判定 REVISE) の指摘と v4.1.0 時点のコード照合で改訂した。製品コードは未編集。

本書は [レーティング操作・一覧ソート・タグジャンプ計画](rating-sort-and-tag-navigation-plan.md) のうち、
評価変更へ追従して表示順を動かす旧 Phase C 案を置き換える。完了済みのタグジャンプ、名前 / 番号降順、
評価の1段階増減 (§1.237A、`8878aca2b`) は変更しない。

## 0. 決定と対象

評価昇順 / 降順は、一覧を開くか明示的に更新した時点の評価を一度だけ取得して並べる。
表示中に評価を変更しても行を移動しない。ファイルメニューの「最新の情報に更新」、割り当てた
`GridReload`、sort横の更新アイコン、選択中と同じ評価sortの再選択、一覧の開き直し、または
並べ替え条件の明示変更で評価を再取得し、その時点で再び並べる。

materialize 済みの `items` 順そのものを評価順 snapshot とする。全表示経路を共通の display-order owner へ
移す再構造、items / cache の permutation、thumbnail queue の再設計は行わない。

### 0.1 評価順を適用する一覧

- 通常の物理フォルダ (main と各 detached viewer context。Ctrl+↑↓ / BS / folder pane 経由を含む)。
- Ctrl+G の flat / drill / ZIP hit。
- Smart Folder resident root、サブフォルダ展開、ブックマーク。
- ファイル名 prefix スタック (集約表示の group 順と、group 内 member 順。§4.2)。
- レーティング一覧 (全行同じ★なので名前 tie のみ。`RatedAtAsc / Desc` は従来どおり)。
- Collection の Standard order (PC と Remote の両方)。
- Remote の物理フォルダ一覧、Remote から見える上記の特殊一覧。
- 詳細表示の評価列ヘッダ (`DetailsSortKey::Rating`) も、評価変更だけでは既存行の相対順を変えない。

### 0.2 評価順を適用しない一覧 (既存の固定順を維持)

- 本 / ZIP / PDF のページ順 (`BOOK_READING_PAGE_ORDER` / page order lock)。
- Ctrl+S (お気に入り検索) とタグ一覧。**現行でもどの sort も適用せず検索結果順で install している**
  (`apply_favsearch_results`、タグ結果 install)。評価順だけを接続すると他の sort と不整合になるため、
  本計画では現行の結果順を維持する。sort 対応は別件とする。
- 閲覧履歴 (MRU)、★固定 snapshot、ドライブ一覧。
- Collection の Manual order と Shuffle order。
- フォルダ代表サムネイルの候補順 (§1.275/§1.276 の allowlist)。

## 1. 現行経路の確認 (2026-09-25 v4.1.0 時点)

### 1.1 更新操作

`KeyAction::GridReload` は `KeyContext::Grid` の操作で、既定 chord は無い。ファイルメニューの
`MenuCommandId::FileReload`「最新の情報に更新」も同じ action を参照し、キーボード・メニューとも
`App::reload_top_level_grid(ctx)` へ合流する。同関数は `TopLevelGridSurface` の網羅 match
(`src/app/top_level_grid_view.rs`) で各 surface の再入場経路へ送る。新しい Action、固定 F5、
別メニュー項目は追加しない。

**ただし Collection 分岐は `open_collection_grid(id, None)` を呼び、新しい session として空の grid を
install するため、現状では選択 anchor を引き継がない。** §6.1 で直す。

sort control は `Button::selectable`、`selectable_label`、menu button で描かれている
(`src/ui_main.rs`)。同値を再 click しても値は変わらないため、reload 判定は `changed()` ではなく
click 前の sort と `clicked()` から一つの dispatch 決定として作る (§7.3)。

### 1.2 評価の取得経路

- 通常フォルダ: `start_loading_items_inner` が items install 後に `prewarm_rating_cache()` を呼び、
  UI thread から `RatingDb::get_many` を実行する。folder open の worker
  (`FolderOpenScanPurpose` / `FolderPaneOpenReady`) は scan だけを返し、materialize と prewarm は
  UI 側で行う。
- Ctrl+G: 評価の `get_many` は既に `search_prepare_worker` 内で行われ、hit の `stars` に載る
  (初版の「UI thread の batch poll」は誤り)。
- Smart Folder / サブフォルダ展開: 既存 worker が評価を取得済み。
- Collection: prepare は worker だが、install で `rebuild_visible_indices` を呼ぶ時点で rating cache を
  持たず、cache miss の `get_rating` が UI thread の point read へ落ちうる。
- 他に UI thread に残る評価読み取りとして、XMP hydration の `get_many`、`current_folder_rating` の
  point read がある。

`RatingDb::get_many` は 500 件 chunk ごとに別 statement で読み、prepare / query の失敗 chunk と
失敗 row を無言で読み飛ばして plain map を返す。**結果に無い key が「未評価」なのか「読めなかった」のか
区別できず、chunk 間で一つの SQLite snapshot でもない。** 評価順の比較 key には使えない (§2.2)。

### 1.3 viewer context の所有

各 viewer context (main と detached) の bundle は自分の `rating_cache`、session write の seen generation、
folder pending request を持つ (`src/app/viewer_context_registry.rs`)。swap 時に cache 値を同期し
`rebuild_visible_indices` を呼ぶが、**parked context の Ctrl+G `all_hits` / Rating view membership は
更新しない**。active context 側の評価編集経路はそれらを別途更新している。§5.2 で一本化する。

### 1.4 既存の selection 復元境界

raw index を reload 後へ持ち越さず、次の stable identity を使う。

- 通常フォルダ: `preserve_cursor_hint_for_reload` / `select_after_load` と exact path。
- Smart Folder resident root: session の active child / saved selection と scroll snapshot。
- Collection: `CollectionGridViewportAnchor { entry_id, source_key }` (reload でも渡すよう §6.1 で直す)。
- Bookmark は stable row ID、Ctrl+G は content key、Rating view は rating key。
- Remote session release は `GridItem::perf_key()` で fullscreen を戻す。
- 通常フォルダの外部変更は main viewer が開いている間は reload を defer する (既存)。

## 2. 比較契約

### 2.1 typed key

評価 key は項目ごとに次の型で持つ。

```text
RatingSortKey = Supported(u8 /* 0..=5, 0 = 未評価 */) | Unsupported
```

- `Unsupported` は評価非対応の項目 (ドライブ等)。昇順 / 降順とも評価 domain の後ろに置き、相互は
  名前昇順。0 を非対応 sentinel にしない。
- 物理一覧の folder / media category と `GridDisplayOrder` は既存どおり先に適用し、同じ category 内だけを
  評価で並べる。Collection Standard も既存の category rank を維持する。
- 未評価位置は新設の `RatingSortUnratedPosition` (未実装) で選ぶ。

| 設定 | 降順 | 昇順 |
| --- | --- | --- |
| `BetweenThreeAndTwo` | 5, 4, 3, 0, 2, 1 | 1, 2, 0, 3, 4, 5 |
| `BelowAll` | 5, 4, 3, 2, 1, 0 | 0, 1, 2, 3, 4, 5 |

昇順は評価可能 domain の完全な逆順。同じ評価の tie は方向にかかわらずファイル名昇順。比較は純粋関数にし、
2設定 × 2方向 × 0..=5、`Unsupported`、同値 tie を table test で固定する。

`RatingSortSpec { order, unrated_position }` を worker request に焼き付け、worker 完了時に設定を引き直さない。

### 2.2 完全な評価 facts

評価順の比較には、対象 key 全件について「読めた」ことが保証された facts だけを使う。

- `RatingDb` に fallible な一括読み取りを追加する。**一つの read transaction** の中で全 chunk を読み、
  どの chunk / row が失敗しても `Err` を返す。既存 `get_many` の無言スキップ挙動は既存呼び出し元のために残す。
- 結果は `CompleteRatingFacts` (対象 key 集合 + 値 map + 読み取り stamp) として型で区別する。
  map に無い key を 0 と解釈してよいのは、この型の中だけである。
- Rating comparator は `CompleteRatingFacts` から作った key だけを受け取る。facts を持たない producer が
  Rating variant を comparator へ渡したら、debug test で落ちる経路にする (無言で全件未評価扱いにしない)。
- `ListingSortMetadata` は現状 `mtime` と `file_size` だけを持つ。評価は同構造へ `Option` で足さず、
  Rating sort 時だけ完全な key 列を別引数 / builder で渡す形にし、「評価が無い metadata」と
  「未評価」の混同を型で防ぐ。

読み取り失敗時: 既存一覧を保持できる reload では保持して通知する。初回 navigation では一覧を開けなくしない
ため、評価順を適用できなかったことを通知し、決定的な名前順で install する。

## 3. snapshot の所有と寿命

### 3.1 items 順が正本

別の永続 order、全画面共通 rank map、App-global pending bool は追加しない。worker が評価を取得して sort を
終えた items を既存の install 境界へ渡す。accepted install が `items_generation` を進めた時点から、その
items 順がその context の評価順 snapshot である。

snapshot を置き換えるのは次だけである。

1. 別フォルダ / 別 top-level surface / 別 collection を開く。
2. 「最新の情報に更新」、割り当て済み `GridReload`、sort横の更新アイコン、選択中評価sortの再選択。
3. sort、未評価位置、collection Standard sort の明示変更。
4. filesystem membership change、collection revision、Smart Folder 定義変更等、その surface の既存仕様が
   full requery / rescan / reprepare を accepted したとき。
5. 一覧を閉じて開き直す。

次は snapshot を置き換えない: 直接評価 / 増減 / リング操作、Undo / Redo、XMP hydration、metadata import 後の
cache refresh、別 viewer context や Remote からの書き込み、rating filter / facet / Ctrl+F 変更、
thumbnail / details 切替、fullscreen open / close、Smart Folder resident root との往復。
評価の書き込み自体を reload 理由に昇格させない。

### 3.2 install 境界の規則 (初版の overlay 案を置き換え)

初版は「worker 開始後 install 前の session write を初回 sort key に重ねる」としていたが、session write
ledger は path ごとに最新 1 件しか持たず、`sync_current_context_rating_session_writes` は worker が並べた
items ではなく index cache を更新するため、安全な境界にならない。次の単純な規則に置き換える。

1. worker は読み取り開始前に、その時点の `rating_session_write_generation` を stamp として取る。
2. worker は §2.2 の単一 transaction で facts を読み、sort して結果を返す。
3. UI thread は結果を受け取った同じ event-loop step の中で、stamp より後に commit された session write の
   うち、この一覧の対象 key に当たるものがあるかを ledger で確認する。
4. 当たるものがあれば結果を捨て、同じ request identity で prepare をやり直す。無ければそのまま install する。
   確認と install の間に他の処理を挟まない。
5. install 後の書き込みは cache / filter / membership だけを更新し、順序へ反映しない (§5)。

同じ process 以外 (別プロセスの mIV 等) による DB 書き込みは、次に要求された一覧から順序へ反映される。
worker の前後境界は、制御可能な barrier を使ったテストで両側を確認する。

cancel、別 navigation の勝利、context retire、surface 変更、より新しい request、worker disconnect で古い結果を
捨てる。別 context の一覧へ結果を適用しない。

## 4. producer ごとの接続

### 4.1 一覧表

`sort_order` で items を materialize するすべての経路を対象に、適用するか固定かを決める。

| producer | 評価 facts の取得 | snapshot を作る契機 | 備考 |
| --- | --- | --- | --- |
| 物理フォルダ (main / detached) | folder open worker を typed prepared listing に拡張して取得 | open、Ctrl+↑↓ / BS、sort変更、reload、filesystem full reload | Rating sort 時だけ materialize まで worker で行う |
| ファイル名 prefix スタック | 物理フォルダ等の prepared facts を再利用 | 元一覧と同じ | §4.2 |
| Ctrl+G | `search_prepare_worker` の既存 `stars` (完全性を §2.2 に揃える) | query、reload、drill materialize | 評価変更時は membership-only rebuild (§5.1) |
| Smart Folder | 既存 worker の評価 | open、reload、定義 rebuild | resident session の root 順 |
| サブフォルダ展開 | 既存 worker の評価 | scan、reload | prepared snapshot 順 |
| Bookmark | build worker に facts 取得を追加 | open、refresh | stable row 順 |
| Rating view | 全行同じ★ | 既存 view build / membership refresh | 名前 tie のみ |
| Collection Standard (PC / Remote) | `prepare_collection_snapshot` worker で facts 取得 | open、reload、revision 更新 | §6 |
| Collection Manual / Shuffle | 表示 / filter 用のみ | prepare | 保存順 / seed 順を維持 |
| Remote 物理フォルダ | `ContainerService::recompute_folder_listing` の worker で facts 取得 | list request | §6.2 |
| Ctrl+S / タグ / 閲覧履歴 / ★固定 / ドライブ | 取得しない | 既存 | §0.2 |

Special view は各既存の pending owner を拡張する。全 surface 共用の App-global
`rating_sort_pending: Option<_>` を追加しない。

物理フォルダは scan と rating facts を別々の untyped `Option` に残さない。既存の `FolderOpenScanPurpose` /
`FolderPaneOpenReady` と同じ request owner が、scan、任意の exact selection、facts、sort spec を
成功 / cancel / error まで一緒に持つ `PreparedFolderListing` 相当を返す。非 Rating sort の既存即時経路を
非同期化する必要はない。detached context の folder open / Ctrl+↑↓ も同じ worker 経路を使い、pending は
その context の bundle が所有する。一つの context の close / cancel が sibling の pending を失効させない。

### 4.2 ファイル名 prefix スタック

`filename_stack::group_media` / `sort_members` は `SortOrder` で member と group を並べる。
Rating sort 時は次のとおりにする。

- group 内の member は評価 key で並べる (同値は名前 tie)。
- 集約表示の group 順は、その group の代表 member (member 順の先頭) の評価 key で決める。
  これにより集約表示とフラット表示で先頭 member の並びが一致する。
- 評価 key は元一覧と同じ `CompleteRatingFacts` から作る。スタック化の時点で DB を読まない。
- 評価変更後に代表 member や group 順を入れ替えない (snapshot 固定)。

## 5. 評価変更後の処理

### 5.1 書き込み、Undo、外部反映

既存の `write_user_rating_shared` / batch transaction、session generation、XMP writer、folder count、
content identity、Undo record を維持する。成功した変更だけを cache へ公開する契約も変えない。

変更後は §5.2 の publication だけを行う。`items` の sort、Smart Folder / collection の再 prepare、
thumbnail queue の全再投影は行わない。

Ctrl+G の現行 `rebuild_items_from_global_search()` は評価変更後に最新 stars で全体を作り直すため、
Rating sort のまま呼ぶと行が動く。評価変更時は stable item / container identity で既存行の相対順を維持し、
消えた行を除去、count を更新、新たに filter へ入った行だけ candidate 順で末尾へ追加する membership-only
rebuild へ分ける。query、sort 変更、reload だけが通常の sorted rebuild を呼ぶ。

### 5.2 context ごとの publication (mounted / parked 共通)

評価の書き込み (どの window / Remote から来たものでも) を各 viewer context へ反映する処理を、context が
所有する一つの手続きにまとめる。mounted の context には即時、parked の context には mount / swap 時に
同じ手続きを適用する。

1. path key の書き込みを `rating_cache` へ反映する。
2. Ctrl+G の `all_hits.stars` と drill 件数を更新する (membership-only rebuild)。
3. Rating view の membership (★N から外れた行の除去、入った行の追加) を更新する。
4. rating filter / Rated・Unrated facet と Details state を更新する。
5. selected が非表示になった場合は既存の nearest-visible policy、checked は既存の WYSIWYG policy。

どの手順も survivor の相対順を保つ。現行の「active 側の編集経路だけが all_hits / Rating view を直す」
二重実装はこの手続きへ寄せる。

### 5.3 visible_indices と Details

thumbnail 表示の `visible_indices` は raw items index 順を保つため、filter 再計算で survivor 順は動かない。
Rating sort 専用の visible permutation は不要。

詳細表示の `DetailsSortKey::Toolbar` は `visible_indices` をそのまま使う。評価列ヘッダ `DetailsSortKey::Rating`
は、ヘッダ click、明示 reload、items install 時だけ評価で `details_order` を作る。評価変更に伴う membership
再計算では現在の `details_order` に残る survivor の相対順を維持し、新たに visible になった row だけ raw items
順で末尾へ追加する。別列のヘッダ sort はその列の既存規則を維持する。

## 6. Collection と Remote

### 6.1 Collection

- Manual / Shuffle は評価 sort の影響を受けない。
- Standard だけが definition-owned `standard_sort` を使い、Rating variant 時は `prepare_collection_snapshot`
  worker 内で §2.2 の facts を取得する。prepared snapshot に rating cache を含め、install の
  `rebuild_visible_indices` より前に cache を移す (UI thread の point read へ落とさない)。
- 評価の書き込みは collection revision を進めないため、revision watch から自動 prepare しない。
- **明示 reload は prepared order のキャッシュを通さない。** Remote の prepared 再利用 key
  (collection ID、revision、display order) には評価の新しさが入っていないため、明示 reload では再利用を
  迂回し新しい facts で prepare する。PC 側の再利用も同じ規則にする。
- reload の前に `CollectionGridViewportAnchor { entry_id, source_key }` を取り、`reload_top_level_grid` の
  Collection 分岐がそれを `open_collection_grid` へ渡して install 後に選択・checked を再解決する。
  child から root へ戻る既存の latest-snapshot 再取得も同じ anchor を使う。

### 6.2 Remote

- Remote の物理フォルダ一覧は `ContainerService::recompute_folder_listing` で構築される。Rating sort 時は
  同じ worker で §2.2 の facts を取得してから応答順を決める。book / ZIP / PDF page の lock を評価で解除しない。
- Remote へ返した container payload は一つの immutable snapshot とする。Remote から評価を書いた直後は現在
  row の評価表示だけを更新し、client 側で自動 reorder しない。明示 reload、navigation、sort 変更による次の
  list request が新しい snapshot を返す。
- 永続 Collection は既に Remote へ接続済み (`remote_ipc/persistent_collections.rs`) なので、§6.1 の
  Standard / Manual / Shuffle の契約と prepared-cache 迂回をそのまま payload に投影する。
- Remote のタグ一覧は固定の sort を返しており (`remote_ipc/collections.rs`)、PC のタグ一覧と同じく
  評価順を適用しない。

## 7. 永続化と互換

`SortOrder` (settings とツールバー構成) と Collection definition の `standard_sort` はリリース済みデータである。

### 7.1 settings

- `SortOrder::RatingAsc / RatingDesc` と `RatingSortUnratedPosition` (既定 `BetweenThreeAndTwo`) を追加する。
  後者は serde default 付きの新 field とし、Remote は既存の settings snapshot 経由で読む。
- ツールバーの sort 候補は、既定の 8 件構成のままの利用者だけを 10 件へ拡張する独立の one-time marker を
  持つ。custom / 並べ替え / 部分 / 空 / 後から隠した候補の構成には補完しない。
- 旧版への戻し: settings DB は未知の enum variant を `Incompatible` として扱い、保存を抑止して DB family を
  quarantine しない (`settings_db.rs`、既存)。これを test で固定する。

### 7.2 collection.db

v4.1.0 の `read_definition_row` は未知の `standard_sort` 文字列で `FromSqlConversionFailure` を返す。
**Rating variant を collection.db に保存する前に、v4.1.0 がこの失敗をどう扱うか (該当 collection だけ読めない /
store 全体が Unavailable / backup からの復元や上書き) をタグ `v4.1.0` のコードで確定する。**
旧版がデータを失う・上書きする経路がある場合は、既存列に Rating を書かず、旧版が既知の値として読める形
(例: 新しい列に Rating を持ち、既存列には従来の値を残す) に設計を変える。この確定は実装者が行い、
結果を設計担当へ返してから保存形式を決める。

### 7.3 同値 click と更新アイコン

- sort control の click ごとに、click 前の sort と `clicked()` から「sort 変更」「reload」「何もしない」の
  いずれか一つだけを決める。button / dropdown / menu / Collection の order mode 切替で同じ決定関数を使う。
- 同じ評価 sort の再 click と sort 横の更新アイコンは、どちらも `reload_top_level_grid(ctx)` へ直接合流する。
  独自 reload 関数や疑似キー入力を作らない。更新アイコンの tooltip / accessible label は「最新の情報に更新」。
- 新しい KeyAction は追加しない (既存 `GridReload` を使う)。

## 8. 実装段階と見積もり

実装担当は段階ごとに一つの Codex セッションを使う。**利用者から Rating sort を選べるようにするのは最終段階
(R5)** とし、途中の段階では variant を `SortOrder::all()`、ツールバー、menu、Remote wire に出さない。
途中段階で公開済みの選択肢が未完成の producer へ届く状態を作らない。

| 段階 | 内容 | 受入条件と focused test |
| --- | --- | --- |
| R1 | 純粋な評価 rank / `RatingSortSpec`、`RatingSortKey`、fallible な単一 transaction 一括読み取りと `CompleteRatingFacts`。variant は内部のみ | 0–5 / Unsupported の全 table、tie、chunk 失敗、並行書き込み中の一貫 snapshot、固定順 / 代表候補 allowlist 不変 |
| R2 | 物理フォルダの prepared listing (main / detached、open / reload / Ctrl+↑↓ / BS)、context 所有の pending、install 境界の規則 (§3.2)、ファイル名スタック | main と detached の置き換え、cancel / error、sibling 不変、exact selection、Rating 経路で UI thread の DB read が 0、install 前後の書き込み境界 |
| R3 | Ctrl+G、Smart Folder、サブフォルダ展開、Bookmark、Rating view、Details、§5.2 の context publication | producer ごとの sort、書き込み / Undo / hydration 後に survivor 順不変、filter / Details ヘッダ、main + detached 2窓 (うち1つ parked) で別窓からの書き込み |
| R4 | Collection Standard (PC / Remote)、Manual / Shuffle 維持、Remote 物理 / 特殊一覧、prepared cache の迂回、Collection reload の anchor、§7.2 の旧版挙動確定 | Collection reload / child 復帰の選択、Remote の明示 reload、書き込み後に client が並べ替えない、固定一覧の除外 |
| R5 | 公開: settings / ツールバー 8→10 / menu / Remote の選択肢、未評価位置の設定 UI、同値 click と更新アイコン、旧版互換 test、マニュアル・spec・keymap 説明 | control ごとに dispatch が 1 回だけ、8→10 と custom の migration、旧版保護、`cargo fmt --check`、glyph check、`test-full.ps1`、`build-dev.ps1` |

見積もり: 約 9〜14 開発日、コード / テスト 30〜45 ファイル程度 (初版の 3.5〜6 日 / 16〜22 ファイルは、
スタック、接続済みの Remote Collection、facts の完全性と cache、context 横断テストを含んでいなかった)。
GUI 確認時間は含めない。

## 9. 回帰試験

### 9.1 snapshot 契約

- 6評価値 × 2未評価位置 × 2方向、`Unsupported`、同値 tie。
- Rating sort で一覧を開き、単一 / 複数評価、増減、Undo / Redo 後も survivor 順が不変。
- reload、menu reload、sort 変更、未評価位置変更、同値再 click、更新アイコンの後だけ最新評価で再 sort。
- XMP hydration、metadata import、別 context / Remote からの書き込み後に badge / filter は更新され、順序は不変。
- rating filter で row が消える / 現れる場合の survivor 順、newcomer 末尾、selected / checked の既存 policy。
- thumbnail / Details 切替、Details 評価ヘッダ、別ヘッダ sort。
- facts の読み取り失敗: reload では既存一覧を保持して通知、初回 open では名前順で install して通知。

### 9.2 surface

- 物理フォルダ (main / detached)、スタックの集約 / フラット、Ctrl+G flat / drill / ZIP hit、Smart Folder、
  サブフォルダ展開、Bookmark、Rating view。
- Ctrl+S / タグ / 閲覧履歴 / ★固定 / ドライブ / 本のページ / Collection Manual・Shuffle で順序が変わらない。
- Collection Standard の Rating snapshot、revision 更新、reload 時の anchor、child から root への復帰。
- Remote 物理フォルダ、Remote Collection、書き込み後に row 不動、明示 reload 後に再 sort。

### 9.3 lifecycle / I/O / 複数ウィンドウ

- prepare の cancel、置き換え、surface 退出、context retire、stale sort spec、worker disconnect、DB error。
- install 前後の session write 境界 (barrier 付き)。
- Rating sort の経路で UI thread から rating DB を読まない。既存の非 Rating 経路の UI thread 読み取り
  (`prewarm_rating_cache`、XMP hydration、`current_folder_rating`) は本計画では移さず、別の負債として扱う。
  本計画はそれを増やさない。
- `src/app/multiwindow_scenario_tests.rs` に main + detached 2 窓のシナリオを追加し、parked の窓を含めて、
  別窓からの書き込みで各窓の順序が不変・badge / filter が更新されること、1窓の reload が sibling の items /
  generation を変えないことを確認する。

## 10. 完了条件

- 利用者が評価を変えても、Rating sort 中の既存行が移動しない (どの窓・Remote から書いても)。
- 「最新の情報に更新」、`GridReload`、更新アイコン、同値再 click が同じ router から最新評価を再取得し、
  selection identity を保って再 sort する。
- rating filter、Undo / Redo、外部評価の反映、Rating view membership が退行しない。
- 固定順の一覧 (§0.2) を Rating comparator へ一度も渡さない。
- Rating sort 用の DB 読み取りは worker の単一 transaction で行い、完全な facts だけを比較に使う。
- 旧版へ戻しても settings / collection.db を失わない。
- 段階ごとの focused test、`cargo fmt --check`、UI glyph check、`scripts/test-full.ps1`、
  `scripts/build-dev.ps1` が成功する。GUI はエージェントが起動しない。
