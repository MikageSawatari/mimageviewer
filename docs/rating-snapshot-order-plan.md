# 評価順のスナップショット固定 設計計画

最終更新: 2026-09-25

状態: 設計第5版 (独立レビューで ACCEPT)。R1 (`7c6a7d492`)・R2 完了、R3 着手前。

- 初版 (`d05fc1a76`、2026-09-15): 別セッションで作成。
- 第2版 (`23d553814`): 独立レビュー 1 回目 (GPT-6 Sol / xhigh、REVISE) を反映。
- 第3版 (`a42722080`): 独立レビュー 2 回目 (REVISE) の指摘を反映し、利用者決定 (2026-09-25) により
  対象を絞った。決定は次の 2 点。
  1. Collection とブックマークは対象外にする。Collection は手動の並べ替えで代替できる
     (「★順に並べ直す」の一回きり操作は別件、§0.3)。
  2. 通常フォルダは、既存の UI thread 一括読み取り (`prewarm_rating_cache`) を並べ替えの前に移して使う
     (§4.1。新しい読み取りは増やさないが、既存の同期読み取りを worker へ移すこともしない)。
- 第5版 (本版): 独立レビュー 4 回目と R1 の測定を反映。一括読み取りは chunk 単位 (一つの transaction は
  書き込みを 110〜134 ms 待たせた)、読み取り失敗は open / reload とも名前順で続行、R2〜R3 用の
  `ListingOrderRequest` (§7.1)、ブックマークの sort を写す経路の訂正 (§7.3)。
- 第4版 (`96079bda6`): 独立レビュー 3 回目 (REVISE) の指摘を反映。物理フォルダの読み取り位置 (§4.1)、
  prepared 結果が持つ書き込み世代 (§2.2 / §3.2)、ブックマークは評価順だけを外す (§0.2 / §7.3)、
  Rating view を R3 へ移す (§5.4 / §8)、Collection の書き込み境界 (§7.2)、Snapshot surface の更新アイコン (§7.3)。

本書は [レーティング操作・一覧ソート・タグジャンプ計画](rating-sort-and-tag-navigation-plan.md) のうち、
評価変更へ追従して表示順を動かす旧 Phase C 案を置き換える。完了済みのタグジャンプ、名前 / 番号降順、
評価の1段階増減 (§1.237A、`8878aca2b`) は変更しない。

## 0. 決定と対象

評価昇順 / 降順は、一覧を開くか明示的に更新したときに評価を一度だけ読んで並べる (読み取り中の評価変更の
扱いは §2.2 の rolling read)。
表示中に評価を変更しても行を移動しない。ファイルメニューの「最新の情報に更新」、割り当てた
`GridReload`、sort横の更新アイコン、選択中と同じ評価sortの再選択、一覧の開き直し、または
並べ替え条件の明示変更で評価を読み直し、その時点で再び並べる。

materialize 済みの `items` 順そのものを評価順 snapshot とする。共通の display-order owner、
items / cache の permutation、thumbnail queue の再設計は行わない。

### 0.1 評価順を適用する一覧

- 通常の物理フォルダ (main と各 detached viewer context。Ctrl+↑↓ / BS / folder pane 経由を含む)。
- ファイル名 prefix スタック (集約表示の group 順と group 内 member 順、§4.2)。
- Ctrl+G の flat / drill / ZIP hit。
- Smart Folder resident root、サブフォルダ展開。
- レーティング一覧 (全行同じ★なので名前 tie のみ。`RatedAtAsc / Desc` は従来どおり。§5.4)。
- Remote の物理フォルダ一覧と、Remote から見える上記の一覧。
- 詳細表示の評価列ヘッダ (`DetailsSortKey::Rating`) も、評価変更だけでは既存行の相対順を変えない。

### 0.2 評価順を適用しない一覧 (既存の順を維持)

- Collection (Manual / Shuffle / Standard のすべて。PC と Remote)。**Collection の Standard sort の選択肢に
  評価順を出さず、collection.db にも保存しない** (§7.2)。
- ブックマークの評価順。ブックマーク一覧の既存の並べ方 (通常の sort と登録時刻順) はそのまま使え、
  評価順の選択肢だけをこの一覧で使えなくする (§7.3)。
- Ctrl+S (お気に入り検索) とタグ一覧 (PC と Remote)。現行でもどの sort も適用せず検索結果順で install
  している (`apply_favsearch_results`、タグ結果 install)。
- 閲覧履歴 (MRU)、★固定 snapshot、ドライブ一覧。
- 本 / ZIP / PDF のページ順 (`BOOK_READING_PAGE_ORDER` / page order lock)。
- フォルダ代表サムネイルの候補順 (§1.275/§1.276 の allowlist)。

これらの一覧で全体の sort が評価順になっている場合に、選択肢が効いているように見えないよう、
sort control に理由を示す (§7.3)。

### 0.3 別件へ回すもの

- Collection の手動並びへ「★順に並べ直す」一回きりの操作を加える案 (保存済み manual position を
  その時点の評価で書き換える)。collection.db の sort 形式を変えないので旧版互換の問題が無い。
- ブックマーク一覧の評価順。

どちらも `docs/next-release-backlog.md` §1.283 に記録する。

## 1. 現行経路の確認 (2026-09-25 v4.1.0 時点)

### 1.1 更新操作

`KeyAction::GridReload` は `KeyContext::Grid` の操作で、既定 chord は無い。ファイルメニューの
`MenuCommandId::FileReload`「最新の情報に更新」も同じ action を参照し、キーボード・メニューとも
`App::reload_top_level_grid(ctx)` へ合流する。同関数は `TopLevelGridSurface` の網羅 match
(`src/app/top_level_grid_view.rs`) で各 surface の再入場経路へ送る。新しい Action、固定 F5、
別メニュー項目は追加しない。

sort control は `Button::selectable`、`selectable_label`、menu button で描かれている (`src/ui_main.rs`)。
同値を再 click しても値は変わらないため、reload 判定は `changed()` ではなく click 前の sort と
`clicked()` から一つの dispatch 決定として作る (§7.3)。

### 1.2 評価の取得経路

- 通常フォルダ: `start_loading_items_inner` が items install 後に `prewarm_rating_cache()` を呼び、
  UI thread から `RatingDb::get_many` で一覧全体の評価を読む。perf ログ (`sli_prewarm_rating`、
  利用者環境 180 件、件数は未記録) では中央値 0.12 ms、95% 値 0.83 ms、最大 11.4 ms。
- Ctrl+G: 評価の `get_many` は `search_prepare_worker` 内で行われ、hit の `stars` に載る。
- Smart Folder / サブフォルダ展開: 既存 worker が評価を取得し、prepared の `rating_cache` を install で移す。
- 他に UI thread に残る評価読み取りとして、XMP hydration の `get_many`、`current_folder_rating` の
  point read がある。本計画ではこれらを変えない。

`RatingDb::get_many` は 500 件 chunk ごとに別 statement で読み、prepare / query の失敗 chunk と
失敗 row を無言で読み飛ばして plain map を返す。結果に無い key が「未評価」なのか「読めなかった」のか
区別できない。評価順の比較 key には使えない (§2.2)。

### 1.3 viewer context の所有

各 viewer context (main と detached) の bundle は自分の `rating_cache`、session write の seen generation、
folder pending request を持つ (`src/app/viewer_context_registry.rs`)。swap 時に cache 値を同期し
`rebuild_visible_indices` を呼ぶが、parked context の Ctrl+G `all_hits` / Rating view membership は
更新しない。active context 側の評価編集経路はそれらを別途更新している。§5.2 で一本化する。
parked の live work は context を mount してから動くため、mount / swap 時の publication で足りる。
Remote の container snapshot は bundle とは別に持つ (§6)。

### 1.4 既存の selection 復元境界

raw index を reload 後へ持ち越さず、次の stable identity を使う。

- 通常フォルダ: `preserve_cursor_hint_for_reload` / `select_after_load` と exact path。
- Smart Folder resident root: session の active child / saved selection と scroll snapshot。
- Ctrl+G は content key、Rating view は rating key。
- ファイル名スタック: §4.2 の group key。
- Remote session release は `GridItem::perf_key()` で fullscreen を戻す。
- 通常フォルダの外部変更は main viewer が開いている間は reload を defer する (既存)。

## 2. 比較契約

### 2.1 typed key

評価 key は項目ごとに次の型で持つ。

```text
RatingSortKey = Supported(u8 /* 0..=5, 0 = 未評価 */) | Unsupported
```

- `Unsupported` は評価非対応の項目。昇順 / 降順とも評価 domain の後ろに置き、相互は名前昇順。
  0 を非対応 sentinel にしない。
- 物理一覧の folder / media category と `GridDisplayOrder` は既存どおり先に適用し、同じ category 内だけを
  評価で並べる。
- 未評価位置は新設の `RatingSortUnratedPosition` で選ぶ。

| 設定 | 降順 | 昇順 |
| --- | --- | --- |
| `BetweenThreeAndTwo` (既定) | 5, 4, 3, 0, 2, 1 | 1, 2, 0, 3, 4, 5 |
| `BelowAll` | 5, 4, 3, 2, 1, 0 | 0, 1, 2, 3, 4, 5 |

昇順は評価可能 domain の完全な逆順。同じ評価の tie は方向にかかわらずファイル名昇順。比較は純粋関数にし、
2設定 × 2方向 × 0..=5、`Unsupported`、同値 tie を table test で固定する。

`RatingSortSpec { direction, unrated_position }` を prepare request に焼き付け、完了時に設定を引き直さない。

### 2.2 完全な評価 facts

評価順の比較には、対象 key 全件について「読めた」ことが保証された facts だけを使う。

- `RatingDb` に fallible な一括読み取りを追加する。既存 `get_many` と同じく 500 件程度の chunk ごとに短い
  読み取りを行い (chunk をまたぐ transaction は張らない)、どの chunk / row が失敗しても `Err` を返す。
  既存 `get_many` の挙動は既存呼び出し元のために残す。
- chunk をまたぐ一貫性は要求しない (**rolling read**)。読み取り中に書かれた key は、並びの上では書き込み前・後
  どちらの値で並んでもよい。複数項目をまとめて変える書き込み (チェックした項目への一括評価等) が chunk の間に
  入った場合、前の chunk の項目は変更前、後の chunk の項目は変更後の値で並ぶことがあり、並びがどの一時点の評価とも
  一致しないことを**許容する**。読み取り中の書き込みは、利用者から見ると「一覧を開いている途中の評価変更」であり、
  表示後の評価変更と同じく並びへの反映を保証しない。次の reload で揃う。表示する評価は §3.2 の書き込み世代による重ね合わせで
  正しくなる。一つの transaction で全 chunk を読む案は、R1 の測定 (50,000 key、dev test profile、3 回) で
  UI thread の書き込みを 110〜134 ms 待たせたため採らない。
- 結果は `CompleteRatingFacts` (対象 key 集合 + 値 map + 読み取り前の書き込み世代) として型で区別する。
  map に無い key を 0 と解釈してよいのは、この型の中だけである。書き込み世代は、呼び出し側が読み取りの
  **前**に取った `rating_session_write_generation` を不透明な stamp として受け取り、そのまま保持する (§3.2)。
- Rating comparator は `CompleteRatingFacts` から作った `RatingSortKey` だけを受け取る。評価 key は
  `ListingSortMetadata` に `Option` で足さず、各 row に付けたまま並べ替えを通す (row と key がずれない形)。
  facts を持たない producer が評価順を comparator へ渡したら debug test で落ちる経路にする
  (無言で全件未評価扱いにしない)。

読み取り失敗時: open / reload とも一覧を開けなくしない。評価順を適用できなかったことを通知し、決定的な
名前順で install する。reload で既存一覧を保持する案は、folder load が materialize より前に出ていく context の
状態 (open request、pending、Undo、ZIP 状態、destination) を変えるため、prepare / commit 境界の作り直しが
必要になり、DB 読み取り失敗という稀な場合のためには見合わないので採らない。

失敗時の badge と rating filter は、**評価順以外の sort で一覧を開いたときの既存経路とまったく同じ**にする
(物理フォルダなら install 後の `prewarm_rating_cache`、他の producer ならそれぞれの既存の読み取り)。
並べ替えだけが名前順になり、評価の表示と絞り込みは今日の非評価順と同じ挙動になる。その既存経路には
「`get_many` が失敗した chunk を無言で読み飛ばす」「cache に無い項目は `get_rating` が UI thread で point read する」
という既存の負債があるが、本計画はこれを変えず増やしもしない (§9.3)。test: 失敗時の install が非評価順の
install と同じ cache / filter 経路を通り、評価順用の facts を部分的に使わないこと。

**書き込み待ちの受入条件 (R1)**: `RatingDb` は journal mode を明示しておらず (既定の rollback journal)、
利用者の書き込みは UI thread から同期で行い、busy timeout は 750 ms である。worker (Ctrl+G、Smart Folder、
サブフォルダ展開、Remote) が一括読み取りをしている間、UI thread の書き込みは読み取り中の chunk 1 つ分だけ
待たされうる。R1 で、reader が chunk の読み取り中に writer が commit を試みるよう barrier で競合を確実に起こし、
50,000 key の読み取りと並行した書き込みの待ち時間を測る test を置く (`#[ignore]` で手動実行。共有 CI での
壁時計の assert だけを正しさの test にしない)。待ちが 1 フレーム (16 ms) を超える場合は、WAL 化や書き込みの
worker 化を実装者が独断で入れず、測定値を添えて設計担当へ返す。

## 3. snapshot の所有と寿命

### 3.1 items 順が正本

別の永続 order、全画面共通 rank map、App-global pending bool は追加しない。並べ替えを終えた items を
既存の install 境界へ渡す。accepted install が `items_generation` を進めた時点から、その items 順が
その context の評価順 snapshot である。

snapshot を置き換えるのは次だけである。

1. 別フォルダ / 別 top-level surface を開く。
2. 「最新の情報に更新」、割り当て済み `GridReload`、sort横の更新アイコン、選択中評価sortの再選択。
3. sort、未評価位置の明示変更。
4. filesystem membership change、Smart Folder 定義変更等、その surface の既存仕様が full requery /
   rescan / reprepare を accepted したとき。
5. 一覧を閉じて開き直す。

次は snapshot を置き換えない: 直接評価 / 増減 / リング操作、Undo / Redo、XMP hydration、metadata import 後の
cache refresh、別 viewer context や Remote からの書き込み、rating filter / facet / Ctrl+F 変更、
thumbnail / details 切替、fullscreen open / close、Smart Folder resident root との往復、
ファイル名スタックの集約 / フラット切替。評価の書き込み自体を reload 理由に昇格させない。

### 3.2 snapshot の時点 (第2版の reject-and-reprepare を置き換え)

第2版の「読み取り後に書き込みがあれば結果を捨てて作り直す」は、★増減のキーリピート等で一覧がいつまでも
install されない恐れがあった。次の規則に置き換える。

- **snapshot は §2.2 の一括読み取りで読んだ値である。** 読んだ値で並べた順をそのまま install する。
  読み取り中に書かれた key の並びは、書き込み前・後どちらの値でもよい (§2.2)。
- 読み取り後、install 前に commit された書き込みは「表示後の評価変更」と同じ扱いにする。install 直後、
  最初の描画より前に cache / filter / membership へ反映し、順序へは反映しない。これで install は必ず一度で終わる。
  membership のうち、**外れた行の除去と badge / filter は最初の描画より前**に行う。**新たに入る行** (★N 一覧、
  Ctrl+G の評価条件、評価条件を持つ Smart Folder に読み取り後の書き込みで入る行) は、表示用の情報を worker で
  用意する必要があるため、§5.2 の membership-only 更新で後のフレームに末尾へ追加してよい (R3 レビューで決定)。
  install を捨てて作り直さないので、追加は survivor の順を変えず、書き込みが続いても install が終わらない
  ことは起きない。
- この反映は、**その prepared 結果が持つ書き込み世代 (§2.2) より新しい ledger entry** を重ねる。context の
  `seen_generation` を基準にしない。worker の prepare 中に同じ context で書き込みがあると `seen_generation` が
  先に進み、既存の `sync_current_context_rating_session_writes` は新しい書き込みが無いと判断して、古い facts の
  badge を最初の描画に出してしまうため。読み取り前に取った世代は保守的なので、重ねすぎても値は正しい。
  test: active context で読み取り後 install 前に書き込み、最初の描画の badge が新しい値になる。
- 通常フォルダ (§4.1) は読み取りと install が同じ UI thread の処理の中で続くため、この間に書き込みは入らない。
- 別プロセスによる DB 書き込みは、次に要求された一覧から順序へ反映される。

cancel、別 navigation の勝利、context retire、surface 変更、より新しい request、worker disconnect で古い結果を
捨てる。別 context の一覧へ結果を適用しない。worker 側 producer の前後境界 (読み取り前 / 読み取り後
install 前 / install 後の書き込み) は、制御可能な barrier を使った test で確認する。

## 4. producer ごとの接続

### 4.1 一覧表

| producer | 評価 facts の取得 | snapshot を作る契機 |
| --- | --- | --- |
| 物理フォルダ (main / detached) | 既存の UI thread 一括読み取りを並べ替えの前へ移し、§2.2 の fallible 版にする | open、Ctrl+↑↓ / BS、sort変更、reload、filesystem full reload |
| ファイル名 prefix スタック | 元一覧の facts を再利用 (§4.2) | 元一覧と同じ |
| Ctrl+G | `search_prepare_worker` の既存読み取りを §2.2 の完全版にする | query、reload、drill materialize |
| Smart Folder | 既存 worker の読み取りを §2.2 の完全版にする | open、reload、定義 rebuild |
| サブフォルダ展開 | 既存 worker の読み取りを §2.2 の完全版にする | scan、reload |
| Rating view | 全行同じ★ | 既存 view build / membership refresh |
| Remote 物理フォルダ | `ContainerService::recompute_folder_listing` の worker で §2.2 の読み取り | list request |
| §0.2 の一覧 | 取得しない (表示・filter 用の既存読み取りはそのまま) | 既存 |

**物理フォルダの流れ (利用者決定 2)**: `start_loading_items_inner` の時点では遅い。materialize
(`src/app/folder_scan.rs`) が既に folder / media の並べ替え、重複除外、category 配置を終え、その順で動画の index
などが振られている。そこで後から並べ替えると、それらの index と揃えた sort metadata が古くなる。
したがって次のようにする。

1. 評価順のときだけ、物理フォルダの materialize / adoption の境界で、候補の除外が済んだ後、最終の並べ替えと
   index の導出の**前**に、UI thread で一回の完全な一括読み取り (§2.2) を行う。
2. その facts で最終の並べ替えを行い、並んだ items、揃った sort metadata、評価 cache を一組にして
   `start_loading_items_inner` へ渡す。同関数は渡された cache を使い、後段の `prewarm_rating_cache()` を呼ばない
   (二度読みしない)。
3. 読み取りに失敗した場合は、open / reload とも名前順で続行して通知する (§2.2)。
4. main と detached は同じ folder load 経路に合流しているので、detached も同じ処理を通る。sibling の
   items / pending / generation に触れない。

評価順以外の sort の経路は変えない。

- この経路は UI thread の同期 DB 読み取りを残す既存の負債であり、本計画は新しい読み取りを増やさないが
  worker へ移しもしない。読み取りは件数に比例し、数万件のフォルダでの時間は未測定である。
  既存の perf event (`sli_prewarm_rating`) に件数と評価順かどうかを加え、後から判断できるようにする。
- 後で worker 化する場合も、`RatingSortKey` / `CompleteRatingFacts` / 比較はそのまま使える。

Special view は各既存の pending owner を拡張する。全 surface 共用の App-global
`rating_sort_pending: Option<_>` を追加しない。

### 4.2 ファイル名 prefix スタック

`filename_stack::group_media` / `sort_members` は `SortOrder` で member と group を並べ、その後、集約
(`filename_stack.rs` の aggregate materialize) とフラット (flat materialize) の両方が
`arrange_grid_items_with_sort_metadata` でもう一度並べる。評価順のときは次のとおりにする。

- group 内の member は評価 key で並べる (同値は名前 tie)。
- 集約表示の group 順は、代表 member (`members[0]`、stack セルとサムネイルの出どころ) の評価 key で決める。
- **後段の `arrange_grid_items_with_sort_metadata` で汎用の評価比較をしない。** `GridItem::Stack` は汎用
  comparator から見ると評価非対応に見え、group 順を崩す。フラット表示で member を全体比較すると group が
  混ざり、スタック移動 (Shift+↓↑) が壊れる。評価順ではスタック専用の最終配置を使い、group 順と
  group 内で連続した member 順を保つ。
- 評価 key はその一覧を開いたときの facts から作り、集約 / フラット切替をまたいで保持する。切替で読み直さない。
  表示用の badge は live の `rating_cache` から描き、並べ替え key とは分ける。
- 評価変更後に代表 member や group 順を入れ替えない。明示 reload では代表 member が変わりうるため
  (stack の `perf_key` は代表の path)、選択はその一覧内で変わらない group key で復元する。
- test: `[5,1]` の group と `[4,3]` の group を並べ、降順で前者が先、member 順、集約 / フラット切替、
  評価変更後の不動、reload 後の再配置と選択復元、代表サムネイル。

## 5. 評価変更後の処理

### 5.1 書き込み、Undo、外部反映

既存の `write_user_rating_shared` / batch transaction、session generation、XMP writer、folder count、
content identity、Undo record を維持する。成功した変更だけを cache へ公開する契約も変えない。

変更後は §5.2 の publication だけを行う。`items` の sort、Smart Folder の再 prepare、thumbnail queue の
全再投影は行わない。

Ctrl+G の現行 `rebuild_items_from_global_search()` は評価変更後に最新 stars で全体を作り直すため、
評価順のまま呼ぶと行が動く。評価変更時は stable item / container identity で既存行の相対順を維持し、
消えた行を除去、count を更新、新たに filter へ入った行だけ candidate 順で末尾へ追加する membership-only
rebuild へ分ける。query、sort 変更、reload だけが通常の sorted rebuild を呼ぶ。

### 5.2 context ごとの publication (mounted / parked 共通)

評価の書き込み (どの window / Remote から来たものでも) を各 viewer context へ反映する処理を、context が
所有する一つの手続きにまとめる。mounted の context には即時、parked の context には mount / swap 時に
同じ手続きを適用する。

1. path key の書き込みを `rating_cache` へ反映する。
2. Ctrl+G の `all_hits.stars` と drill 件数を更新する (membership-only rebuild)。
3. Rating view の membership (★N から外れた行の除去、入った行の追加) を更新する。
   評価条件を持つ Smart Folder も同じく membership-only に更新する (外れた行を隠し、入った行を末尾へ足す。
   survivor の順は変えない)。評価の書き込みを理由に Smart Folder を全体再 prepare して並べ直さない。
   外れた行は `items` から取り除かず、rating filter で隠れた行と同じく `visible_indices`・Ctrl+↑↓ の移動先・
   件数から外し、次の明示 reload / 開き直し / 定義 rebuild まで隠したままにする (R3 再レビューで決定)。
   取り除くと後続 index がずれ、Ctrl+↑↓ の移動先一覧、Details の評価ヘッダ順、checked、動画サムネイルの
   進行中作業がそれぞれ壊れたため。同じ session 中に再び条件を満たした行は元の位置で再表示される。
4. rating filter / Rated・Unrated facet と Details state を更新する。
5. selected が非表示になった場合は既存の nearest-visible policy、checked は既存の WYSIWYG policy。

どの手順も survivor の相対順を保つ。現行の「active 側の編集経路だけが all_hits / Rating view を直す」
二重実装はこの手続きへ寄せる。

### 5.3 visible_indices と Details

thumbnail 表示の `visible_indices` は raw items index 順を保つため、filter 再計算で survivor 順は動かない。

詳細表示の `DetailsSortKey::Toolbar` は `visible_indices` をそのまま使う。評価列ヘッダ `DetailsSortKey::Rating`
は、ヘッダ click、明示 reload、items install 時だけ評価で `details_order` を作る。評価変更に伴う membership
再計算では現在の `details_order` に残る survivor の相対順を維持し、新たに visible になった row だけ raw items
順で末尾へ追加する。別列のヘッダ sort はその列の既存規則を維持する。

### 5.4 Rating view

- `list_by_stars` が返す行はすべて選んだ★なので、評価順は追加の DB 読み取りなしで名前昇順になる。
  `RatingViewSort::Normal(order)` は現在汎用の `SortOrder` comparator を呼ぶので、評価順のときは明示的に
  ファイル名昇順の比較へ写す (§2.2 の「facts を持たない comparator へ評価順を渡さない」に合わせる)。
  `RatedAtAsc / Desc` の意味は変えない。
- 現在の poll は評価の書き込み後、worker の rating stamp を見て結果を捨てて作り直す。これは membership
  (★N から外れた行の除去、入った行の追加) を保つための動作だが、評価順の snapshot と §3.2 の「install は
  一度で終わる」に合わない。§5.2 の publication と合わせ、書き込み後は survivor の順を保ったまま membership
  だけを更新する形へ変える。
- 同じ一覧について、別件 §1.280 (一度開くと以後のフォルダ履歴が壊れる) と §1.281 (並べ方を次回も保つ) が
  バックログにある。§1.281 の `RatingViewSort` 保存と本節の評価順の写像は同じ型に触れるため、先に着手した側の
  変更を前提に後の側が合わせる。

## 6. Remote

- Remote の物理フォルダ一覧は `ContainerService::recompute_folder_listing` で構築される。評価順のときは
  同じ worker で §2.2 の facts を読んでから応答順を決める。book / ZIP / PDF page の lock を評価で解除しない。
- Remote へ返した container payload は一つの immutable snapshot とする。Remote から評価を書いた直後は現在
  row の評価表示だけを更新し、client 側で自動 reorder しない。明示 reload、navigation、sort 変更による次の
  list request が新しい snapshot を返す。Remote の書き込みは App の共有書き込み経路を通るので、PC 側の
  context には §5.2 で反映される。
- Remote の Collection、タグ一覧は §0.2 のとおり評価順を適用しない。Remote から見える Smart Folder 等の
  一覧は、PC と同じ prepared snapshot を使う。

## 7. 永続化、互換、UI

### 7.1 settings

- R1〜R3 では評価順を内部の `RatingSortSpec` だけで扱い、`SortOrder` の公開 serde variant は足さない。
  Remote の入力 (`remote_ipc/mod.rs` の parse は `SortOrder` の全 variant を受け付ける)、保存済み settings、
  ゲームパッドの picker (`SortOrder::all()` を巡回) から、途中段階で評価順が入り込まないようにするため。
- R2〜R3 で producer に評価順を渡すため、保存しない一時的な並べ方の要求型を置く。
  `ListingOrderRequest = Standard(SortOrder) | Rating(RatingSortSpec)` (serde を付けない)。物理フォルダの
  materialize、ファイル名スタックの group / member 並べ、Ctrl+G / Smart Folder / サブフォルダ展開の prepare は、
  `settings.sort_order` を直接読まずにこの型を受け取る。App 側は一つの関数 (`ListingOrderRequest::from_settings`
  相当) で要求を作り、R4 までは常に `Standard` を返す。R2〜R3 の test はこの関数の cfg(test) 上書き、または
  prepare / materialize 関数へ `Rating` を直接渡して評価順の経路を駆動する。R4 は公開 variant をこの型へ写すだけにする。
- R4 で `SortOrder::RatingAsc / RatingDesc` と `RatingSortUnratedPosition` (既定 `BetweenThreeAndTwo`、
  serde default 付きの新 field) を公開する。Remote は既存の settings snapshot 経由で読む。
- ツールバーの sort 候補は、既定の 8 件構成のままの利用者だけを 10 件へ拡張する独立の one-time marker を
  持つ。custom / 並べ替え / 部分 / 空 / 後から隠した候補の構成には補完しない。
- 旧版への戻し: settings DB は未知の enum variant を `Incompatible` として扱い、保存を抑止して DB family を
  quarantine しない (`settings_db.rs`、既存)。これを test で固定する。

### 7.2 collection.db には評価順を書かない

v4.1.0 では、collection.db の定義に未知の `standard_sort` が 1 件でもあると、起動時の catalog 検証が失敗し、
Collection actor が閉じて**すべての Collection が開けなくなる** (`v4.1.0:src/collection_store/db.rs` の
`read_definition_row`、`runtime.rs` の open 失敗経路。backup からの復元や上書きはしない)。

本計画は Collection を対象外にするため、評価順を collection.db に保存しない。

- Collection の Standard sort の選択肢 (`src/ui_main.rs` の Collection 用の 2 つの選択 UI) は `SortOrder::all()`
  ではなく、評価順を除いた専用の一覧から作る。
- 実際の書き込み経路は PC の選択 UI → `CollectionStoreClient::set_order` → `CollectionStoreDb::set_order`
  だけである。作成は `file_name` 固定、テキスト取り込みは entry を足すだけで定義の sort を取り込まず、Remote には
  Collection の set-order 書き込み request が無い。
- `CollectionStoreDb::set_order` は Manual / Shuffle でも `standard_sort` を書くので、**order mode にかかわらず**、
  no-op 判定と SQL の前で評価順の `standard_sort` を拒否する。client と DB を直接呼ぶ test と、PC の 2 つの選択
  UI に評価順が出ない test で固定する。
- 全体の sort が評価順のまま Collection を開いた場合は、§7.3 の「並べ替え固定」ではなく、既存どおり
  Collection 自身の order mode / Standard sort に従う。

### 7.3 sort control

- click ごとに、click 前の sort と `clicked()` から「sort 変更」「reload」「何もしない」のいずれか一つだけを
  決める。button / dropdown / menu で同じ決定関数を使う。
- 同じ評価 sort の再 click と sort 横の更新アイコンは、どちらも `reload_top_level_grid(ctx)` へ直接合流する。
  独自 reload 関数や疑似キー入力を作らない。更新アイコンの tooltip / accessible label は「最新の情報に更新」。
- 固定順の一覧 (Ctrl+S、タグ、閲覧履歴、★固定、ドライブ) は `grid_sort_lock_reason()` に「並べ替え固定」を
  表す lock 理由を加え、ツールバーとメニューの両方が同じ述語を見る。lock 中も更新アイコンは reload として使える。
- ブックマーク一覧は lock しない。既存の通常 sort と登録時刻順は選べるまま、評価順の選択肢だけを使えなくして
  理由を示す。ブックマーク一覧を開くときは既存どおり `CreatedAtDesc` に戻る。全体の sort を変えたときに
  `apply_sort_change_reload` が `BookmarkViewSort::Normal` の一覧へ全体の `settings.sort_order` を写す経路では、
  評価順から `BookmarkViewSort::Normal` への変換を型で失敗させ、ブックマーク一覧の並べ方を変えない。
- `TopLevelGridSurface::Snapshot` は reload が何もしないので、この surface では更新アイコンを出さない
  (または無効にする)。
- test: 固定一覧とブックマーク一覧での menu、toolbar、更新アイコン、同値 click、`Normal` のブックマーク一覧を
  表示中に全体の sort を評価順へ変える経路 (`apply_sort_change_reload`)。
- 新しい KeyAction は追加しない (既存 `GridReload` を使う)。

## 8. 実装段階と見積もり

実装担当は段階ごとに一つの Codex セッションを使う。利用者が評価順を選べるようにするのは R4 だけである。

| 段階 | 内容 | 受入条件と focused test |
| --- | --- | --- |
| R1 | `RatingSortKey`、`RatingSortSpec`、純粋比較、fallible な chunk 単位の一括読み取りと `CompleteRatingFacts` (書き込み世代の stamp を含む)、書き込み遅延の測定 test | 0–5 / Unsupported の全 table、tie、chunk 失敗、barrier で競合を起こした書き込み遅延が 16 ms 以内 (超えたら設計担当へ返す) |
| R2 | `ListingOrderRequest` (§7.1)、物理フォルダ (main / detached、open / reload / Ctrl+↑↓ / BS) の評価順 install (§4.1 の読み取り位置)、ファイル名スタックの専用配置、§7.3 の lock 理由とブックマークの評価順除外、Snapshot の更新アイコン | main と detached、sibling 不変、exact selection、install 直後の書き込み反映、スタックの §4.2 test、読み取り失敗時の通知と名前順、固定一覧の lock、ブックマークの既存 sort 維持 |
| R3 | Ctrl+G、Smart Folder、サブフォルダ展開、Rating view (§5.4)、Details、§5.2 の context publication、Ctrl+G membership-only rebuild | producer ごとの sort、書き込み / Undo / hydration 後に survivor 順不変、filter / Details ヘッダ、worker の前後境界 (barrier)、main + detached 2窓 (うち1つ parked) で別窓からの書き込み |
| R4 | Remote 物理 / 特殊一覧、公開 (settings / ツールバー 8→10 / menu / Remote / ゲームパッド)、未評価位置の設定 UI、同値 click と更新アイコン、Collection 選択肢からの除外と書き込み境界の拒否、旧版互換 test、マニュアル・spec・keymap 説明 | control ごとに dispatch が 1 回だけ、8→10 と custom の migration、settings の旧版保護、collection.db に評価順が入らない、Remote の明示 reload、書き込み後に client が並べ替えない、`cargo fmt --check`、glyph check、`test-full.ps1`、`build-dev.ps1` |

見積もり: 約 4〜6 開発日 (設計担当の概算)。GUI 確認時間は含めない。

## 9. 回帰試験

### 9.1 snapshot 契約

- 6評価値 × 2未評価位置 × 2方向、`Unsupported`、同値 tie。
- 評価順で一覧を開き、単一 / 複数評価、増減、Undo / Redo 後も survivor 順が不変。
- reload、menu reload、sort 変更、未評価位置変更、同値再 click、更新アイコンの後だけ最新評価で再 sort。
- XMP hydration、metadata import、別 context / Remote からの書き込み後に badge / filter は更新され、順序は不変。
- rating filter で row が消える / 現れる場合の survivor 順、newcomer 末尾、selected / checked の既存 policy。
- thumbnail / Details 切替、Details 評価ヘッダ、別ヘッダ sort。
- facts の読み取り失敗: open / reload とも名前順で install して通知。
- chunk の間に複数項目の一括評価が入る場合: 各項目は変更前・後どちらかの値で並び、最初の描画の badge は全項目が
  変更後の値になる (worker 側 producer、barrier)。

### 9.2 surface

- 物理フォルダ (main / detached)、スタックの集約 / フラット、Ctrl+G flat / drill / ZIP hit、Smart Folder、
  サブフォルダ展開、Rating view。
- Collection (全 order mode)、Ctrl+S、タグ、閲覧履歴、★固定、ドライブ、本のページで順序が変わらず、
  固定一覧では lock 理由が出る。ブックマークは既存の並べ方を保ち、評価順だけが使えない。
- Remote 物理フォルダ、書き込み後に row 不動、明示 reload 後に再 sort。

### 9.3 lifecycle / I/O / 複数ウィンドウ

- worker 側 prepare の cancel、置き換え、surface 退出、context retire、stale sort spec、worker disconnect、DB error。
- 読み取り後 install 前の書き込みが順序に入らず、最初の描画前に badge / filter へ反映される (barrier)。
- **評価順の prepare / install は UI thread の DB 読み取りを増やさない。** 物理フォルダは既存の一括読み取りを
  移して使い、二度読みしない。XMP hydration と `current_folder_rating` の既存読み取りは本計画の対象外。
- `src/app/multiwindow_scenario_tests.rs` に main + detached 2 窓のシナリオを追加し、parked の窓を含めて、
  別窓からの書き込みで各窓の順序が不変・badge / filter が更新されること、1窓の reload が sibling の items /
  generation を変えないことを確認する。

## 10. 完了条件

- 利用者が評価を変えても、評価順の既存行が移動しない (どの窓・Remote から書いても)。
- 「最新の情報に更新」、`GridReload`、更新アイコン、同値再 click が同じ router から最新評価を読み直し、
  selection identity を保って再 sort する。
- rating filter、Undo / Redo、外部評価の反映、Rating view membership が退行しない。
- §0.2 の一覧を評価 comparator へ一度も渡さず、固定一覧では lock 理由を示す。
- 評価順の比較には完全な facts だけを使い、UI thread の DB 読み取りを増やさない。
- 旧版へ戻しても settings を失わず、collection.db には評価順が入らない。
- 段階ごとの focused test、`cargo fmt --check`、UI glyph check、`scripts/test-full.ps1`、
  `scripts/build-dev.ps1` が成功する。GUI はエージェントが起動しない。
