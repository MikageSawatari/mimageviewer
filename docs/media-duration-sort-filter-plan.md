# 動画・音声の長さで並べ替え・絞り込み (§1.762)

最終更新: 2026-11-19

状態: 設計第1版 (統括経由で利用者判断を待つ項目あり、§9)。実装未着手。

## 1. 目的と範囲

利用者の要望 (2026-11-11、v5.5.0 目標 2026-11-19): 動画・音声を再生時間の長さで並べ替えたい・絞り込みたい。
前提は「一覧の表示を遅くしない」こと。

v5.5.0 で入れるもの:

1. 一覧の並べ替え (`SortOrder`) に「長さ（短い順）」「長さ（長い順）」を足す。
2. 絞り込みバーに「長さ」を足す (〜1分 / 1〜10分 / 10〜60分 / 60分以上 / 範囲指定)。

入れないもの (§8 で後続候補として記録):

- アイテム検索 (Ctrl+G) の検索構文の長さ条件 (`duration:>10m` など)。Tantivy 索引に長さの field が無く、
  索引 schema の変更と全件の再索引が要る。
- 物理フォルダ以外の一覧 (Ctrl+G、スマートフォルダ、サブフォルダ展開、レーティング一覧、ブックマーク、
  コレクション) での長さ順。これらの一覧では、既にある詳細表示の「長さ」列の見出しで並べ替えられる (§2.3)。

## 2. 現状 (コードで確認、2026-11-19 master `ca6686d93`)

### 2.1 長さの保存と取得

- フォルダ単位の catalog (`src/catalog.rs`) に `video_meta` 表がある (§1.308、2026-09-30、**v4.10.0 で出荷済み**)。
  列は `filename / mtime / file_size / readable / duration_secs / width / height / codec`。
  mtime と file_size が完全一致するときだけ再利用する。`readable=0` は確定した読み取り失敗。
  音声も同じ表に入る (幅・高さは NULL)。[catalog-design.md](catalog-design.md) の「動画・音声メタデータ」。
- 書き込むのは details-meta worker だけ (`load_details_video_meta_with_probe`、`src/app/metadata_ops.rs`)。
  catalog lookup → miss のときだけ FFmpeg probe → 確定結果を保存。取消・10 秒 timeout・I/O エラーは
  `Interrupted` として保存しない。集約一覧でも実ファイルの親 + basename で元の catalog を使う。
- UI はメモリの `details_lazy_meta` (`DetailsLazyMeta.video_duration_secs`) だけを読む。
  viewer context ごとの bundle に入り、swap される (`src/app/viewer_context_registry.rs`)。
- details-meta worker を動かす要件は、詳細表示の遅延列・サムネイルの長さバッジ/再生位置メーター
  (可視+近傍の段階取得、`VisibleStage`)・AI facet (一覧全体、`AllRequirements`)・選択情報。
  readiness は context ごとの 1 つの `LazyColumnState` (`details_image_dims_state`)。

### 2.2 並べ替え

- `SortOrder` は 10 値 (名前・番号・日付・サイズの昇降、評価の昇降)。並べ替えは一覧の materialize 時に
  一度だけ行い、`items` の順そのものを表示順とする。materialize 後の permutation はしない
  ([rating-snapshot-order-plan.md](rating-snapshot-order-plan.md) §0)。
- 比較は `SortOrder::compare_listing_keys` が名前 key と `ListingSortMetadata { mtime, file_size: Option }`
  で行う。サイズ順は既知を値で比べ、不明は昇順・降順とも末尾、同値は名前昇順
  ([list-size-sort-plan.md](list-size-sort-plan.md))。4 行の割当 (フォルダ / 書庫 / 画像 / 動画音声) の
  行の中だけで比べる。
- 物理フォルダの materialize (`materialize_local_folder_listing_with_order`、`src/app/book_materialization.rs`)
  は worker で走り、評価順のときは同じ場所で rating DB を読む。Remote の物理フォルダ一覧も同じ関数を使う。
- 選択中の評価順の再選択は「読み直して並べ直す」(`SortControlDecision::Reload`、`src/ui_main.rs`)。
  最上位一覧の更新は `reload_top_level_grid` に合流する。
- `toolbar_sort_items` には候補追加ごとの一回限りの既定補完 (marker 付き) がある。
  `collection_options()` は `all()[..8]` の位置スライス。

### 2.3 詳細表示と絞り込み

- 詳細表示には既に「長さ」列 (`DetailsSortKey::VideoDuration`、動画・音声) があり、列見出しで並べ替えられる。
  遅延列は列全体が Ready になるまでソート・フィルタ無効 ([details-view-and-filter-plan.md](details-view-and-filter-plan.md) §15.7)。
  **どの一覧でも使える。** 本計画はこれを変えない。
- 絞り込みバー (`FacetFilter`、`settings.facet_filter` に保存) に長さは無い。サイズ facet は
  プリセット 4 つ + 範囲指定で、サイズの無い行 (フォルダ等) は除外する。
- AI モデル / 生成ツール facet が遅延データ facet の前例。メニューを開くと `ai_model_facet_requested` が立ち、
  一覧全体の読み込み (`AllRequirements`) を始める。読み込み中はメニューに進捗と停止、絞り込みは
  **Ready まで素通し** (`passes_facet_filter`)。Ready で `rebuild_visible_indices`。
- Remote には絞り込みバーが無い。Ctrl+G の索引には長さが無い。

## 3. 設計の要点

### 3.1 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- **長さの出どころを 1 つにする**: 並べ替えも絞り込みも、既存の details-meta worker が埋める値
  (catalog の `video_meta` とメモリの `details_lazy_meta`) だけを使う。新しい worker・新しい DB 表・
  新しい probe 経路を作らない。
- **並べ替えの時点を 1 つにする**: 長さ順も他の順と同じく materialize 時に一度だけ並べる。
  表示中の `items` を並べ替える新しい経路 (permutation) は作らない。長さが後から分かったときの
  並べ直しは、既存の「最新の情報に更新」と同じ再読込経路に乗せる (§5)。
- **長さ順を使える一覧を物理フォルダに絞る**: 集約一覧 (Ctrl+G・スマート・サブ展開など) は、
  並べ直しの再読込が再検索・再走査になり重いこと、producer ごとに catalog 読み取りを足すと変更範囲が
  サイズ順 (15 ファイル超) と同程度になることから、v5.5.0 では対象外にする。これらは詳細表示の長さ列で
  並べ替えられるので、機能の穴にはならない。
- **並べ直しの終了条件を状態で持たない**: 「一度だけ自動で並べ直した」印を viewer context に足すと、
  detached を含む context bundle への追加になり、終了経路の組み合わせも増える。代わりに、並べ直すかを
  「今の並びが、分かっている長さで並べた並びと違うか」という純関数で決める (§5.3)。再読込は分かっている
  長さを必ず並びへ反映するので、2 回目の判定は一致して止まる。
- **取得中のモーダル化は採らない**: 長さを全部取ってから一覧を出す案は、初回のフォルダ表示を probe の
  完了まで待たせる (1,000 本の動画で数十秒になり得る)。前提の「一覧の表示を遅くしない」に反する。
- まれな失敗は既定の割り切り (ログ + 通知はせず、長さ不明として末尾)。catalog の読み書き失敗は、
  メモリの値で並べるので並べ直しの無限ループにならない (§5.2)。

### 3.2 長さの並べ替えキー

`ListingSortMetadata` に長さを足す (Copy のまま)。

```rust
pub(crate) enum ListingDuration {
    /// 動画・音声で、長さが分かっている (秒、有限・0 以上)。
    Known(f64),        // 比較は total_cmp。NaN・負値は producer で Unavailable に落とす
    /// 動画・音声で、probe 済みだが長さが無い (readable=0、または duration_secs が NULL)。
    Unavailable,
    /// まだ取得していない、または動画・音声以外。
    Missing,
}
```

- `DurationAsc`: Known を短い順、その後 Unavailable・Missing。`DurationDesc`: Known を長い順、その後
  Unavailable・Missing。**不明は昇順・降順とも末尾** (サイズ順と同じ)。不明同士・同値は名前昇順。
- 4 行割当の行の中だけで比べる。フォルダ・書庫・画像の行は全部 Missing なので名前昇順になる。
  画像と動画を同じ行に混ぜる表示順の設定では、画像は動画・音声の後ろに名前順で並ぶ。
- Unavailable と Missing を分けるのは、Missing だけが裏での取得と並べ直しの対象になるため (§5)。

### 3.3 長さ順を使える一覧

| 一覧 | v5.5.0 |
| --- | --- |
| 物理フォルダ (main と各 detached context。Ctrl+↑↓ / BS / folder pane 経由を含む) | 対応 |
| ファイル名スタックの集約表示 (物理フォルダ内) | 単独の動画・音声セルは長さ順 (動画・音声はスタックされない)。スタックセルは Missing |
| Remote の物理フォルダ一覧 | 同じ materialize を使うので catalog にある長さで並ぶ。Remote からは取得を始めない (§6) |
| 本扱いの画像フォルダ・製本・ZIP/PDF のページ順 | 固定順のまま (既存の page order lock) |
| Ctrl+G・スマートフォルダ・サブフォルダ展開・レーティング一覧・ブックマーク・コレクション・閲覧履歴など | 対象外。長さ順を選んでいても、その一覧は名前順で並ぶ。並べ替えの操作に理由を出す (§4.3) |
| フォルダ代表サムネイルの候補順 | 対象外 (`folder_thumb_options` の allowlist のまま) |

## 4. 並べ替えの流れ

### 4.1 materialize (worker)

物理フォルダの materialize で、`ListingOrderRequest` が長さ順のとき、動画・音声の行について長さを決める。

1. 要求に UI が渡した長さ snapshot (§4.2) があれば、それを優先する。
2. snapshot に無いものは、そのフォルダの catalog の `video_meta` をまとめて読む (1 フォルダ = 1 DB、
   1 回の SELECT)。照合は basename + 走査で得た mtime / file_size の完全一致 (details-meta worker の
   lookup と同じ stamp)。
3. どちらにも無ければ Missing。

catalog を開けない・読めないときは全部を snapshot と Missing だけで決める (割り切り、表示は失敗させない)。
probe はしない。UI スレッドの I/O は増えない。

### 4.2 UI が渡す長さ snapshot

再読込の要求を作るとき、UI は現在の context の `details_lazy_meta` から、今の一覧の動画・音声の長さ
(Known / Unavailable) を path + stamp の key で集めて要求に入れる。メモリの読み取りだけで I/O は無い。
長さ順で、かつ物理フォルダの再読込のときだけ作る。

目的は、catalog への保存が失敗した値でも、メモリにある値で並べ直しが一致するようにすること (§5.3 の
終了条件)。初回の表示 (snapshot が空) は catalog だけで並ぶ。

### 4.3 選べる場所と表示

- `SortOrder::all()` に `DurationAsc` / `DurationDesc` を足す (メニュー上はサイズ順の後ろ)。
  `collection_options()` と代表サムネイル候補は明示の一覧にして、長さ順を含めない。
- 一覧のメニュー・ツールバー・ゲームパッドの巡回・Remote の候補に出る (`all()` を正本にしている所)。
- `toolbar_sort_items` は、現在の既定 10 値と完全一致するときだけ一回限りで 12 値へ補完する
  (既存の marker 方式、`migrate_toolbar_sort_rating_options` と同型)。
- 対象外の一覧 (§3.3) で長さ順が選ばれているときは、既存の `sort_order_disabled_reason` と同じ形で
  「この一覧では長さ順を使えません。詳細表示の長さ列で並べ替えられます」を出す。
- 選択中の長さ順を再び選ぶと再読込して並べ直す (評価順の `SortControlDecision::Reload` と同じ)。
- 詳細表示の列見出しソート (`DetailsSortKey::VideoDuration`) の挙動は変えない。

## 5. 未取得の長さ (並べ替え)

### 5.1 取りに行く時期

長さ順の物理フォルダを表示し、動画・音声に Missing があるとき、details-meta worker の一覧全体の取得
(`AllRequirements`、AI facet と同じ範囲) に「動画・音声の長さ」を要件として足す。

- 条件は設定と一覧から導く述語 (`duration_sort_should_load`: 並べ替えが長さ順・現在の一覧が対応・
  動画・音声に Missing がある)。要求の印は足さない。
- 可視行は Normal、画面外は Low の既存 priority。取消・一覧世代・catalog LRU・I/O semaphore は既存のまま。
- catalog にある分は cache hit で probe しない (`details_meta/media_cache_hit`)。

### 5.2 並べ直し

details-meta の session が Ready になったとき (`finish_details_lazy_session`)、次をすべて満たせば、
既存の物理フォルダの再読込 (`reload_current_physical_list` / `reload_current_folder_preserving_override`、
「最新の情報に更新」と同じ経路) を 1 回呼ぶ。

1. 並べ替えが長さ順で、現在の一覧が対応 (§3.3)。
2. その context でグリッドが前面にある (全画面・動画・別の modal が開いていない)。
3. 今の `items` の動画・音声の並びが、メモリの長さで並べた並びと違う (§5.3)。

再読込は既存どおり選択位置とスクロールを保つ。2 を満たさないときは並べ直さない。利用者が「最新の
情報に更新」または長さ順の再選択をすれば、その時点の長さで並ぶ (評価順と同じ考え方)。

### 5.3 終了条件 (状態を持たない)

判定 3 は純関数 `listing_order_matches_known_durations(items, sort_metas 相当, details_lazy_meta)`:
動画・音声の行ごとに、メモリの長さ (無ければ Missing) で比較器を当てた順と、今の順が一致するかを返す。

- 再読込は §4.2 の snapshot でメモリの値を必ず使うので、再読込後の並びは判定に一致する。
- 再読込で一覧世代が変わり、`Interrupted` だった項目は再取得される。値が増えなければ並びは変わらず、
  判定が一致して止まる。値が増えたときだけもう一度並べ直す (正しい並べ直し)。
- catalog の保存に失敗しても、メモリの値で並ぶので一致する。

計算量は動画・音声の件数 n に対して O(n log n)、Ready の時だけ。I/O なし。

## 6. 絞り込み (長さ facet)

- `FacetFilter` に `duration_preset: Option<FacetDurationPreset>` を足す。プリセットは
  〜1分 `[0, 60)` / 1〜10分 `[60, 600)` / 10〜60分 `[600, 3600)` / 60分以上 `[3600, ∞)`、範囲指定は
  分単位の min / max (サイズ facet の `Range` と同型)。`settings.facet_filter` と同じく保存する。
  新 field は `#[serde(default)]` なので旧版は読み飛ばす (既存の stash 群と同じ前提を実装時に確かめる)。
- 判定: 動画・音声で Known かつ範囲内だけを通す。Unavailable・Missing・動画音声以外 (フォルダ・書庫・
  画像・スタック) は落とす (サイズ facet がサイズの無い行を落とすのと同じ)。
- 取得: 長さメニューを開いたとき、または長さ条件が有効なとき、AI facet と同じ一覧全体の取得を始める
  (`duration_facet_requested`、`ai_model_facet_requested` と同じ扱い)。
- **Ready まで素通し** (AI facet と同じ)。メニューには「長さを読み込み中 n/m」と停止 / 再開。Ready で
  `rebuild_visible_indices`。各候補の件数は Ready 後に出す。
- すべての一覧 (Ctrl+G・コレクション等を含む) で使える。集約一覧で動画が多いと取得に時間がかかるが、
  AI facet と同じく取消でき、絞り込みは素通しのまま。
- Remote には絞り込みバーが無いので変更しない。

## 7. 変更しないもの・互換

- DB の schema は変えない (`video_meta` を読むだけ)。永続データで変わるのは settings の `SortOrder` の新しい値、
  `FacetFilter` の新 field、`toolbar_sort_items` の補完 marker。いずれも既存のサイズ順・評価順と同じ扱い
  (新しい列挙値を旧版が読めない downgrade 境界は既存の backup / version 運用)。
- お気に入り別の表示状態 (`FavoriteViewState.sort_order`) は同じ型なので追加なしで保存される。
- detached viewer: 新しい context field は足さない。details-meta の状態と再読込は既存の context ごとの
  経路を使う。触れる範囲は [detached-rework-plan.md](detached-rework-plan.md) の規則に従い記録する。
- Remote IPC: `SortOrder` の stable string が 2 つ増えるだけ。Remote の版の扱いは実装時に確かめる。
- サムネイルの長さバッジ、詳細表示の長さ列、再生位置メーターは変えない。

## 8. 後続候補

- 集約一覧 (Ctrl+G・スマート・サブ展開・レーティング・ブックマーク) の長さ順。producer ごとに catalog を
  読み、並べ直しの再読込の重さを設計する。
- Ctrl+G の検索構文の長さ条件 (索引 schema の変更と再索引)。
- Remote から長さの取得を始めること。

## 9. 利用者に判断をもらう項目

1. **長さをまだ取っていない動画・音声の並べ替え**:
   - (A・推奨) すぐに一覧を出し、分かっている長さで並べ、まだの分は末尾。裏で長さを読み、読み終えたら
     グリッドを見ている間に自動で一度並べ直す。全画面を開いている間に読み終えたときは並べ直さず、
     「最新の情報に更新」か長さ順の再選択で並ぶ。
   - (B) A と同じだが自動では並べ直さない。並べ直しは利用者の更新だけ (評価順と同じ)。
   - (C) 長さを全部読み終えてから一覧を出す (初回は待つ。「一覧の表示を遅くしない」前提に反する)。
2. **読み込み中を見せるか**: (推奨) 長さ順のときは並べ替えの操作 (ツールバー・メニューの項目) に
   「長さを読み込み中 n/m」を出す。絞り込みの長さメニューにも同じ進捗と停止。
3. **長さ順を使える一覧**: (推奨) v5.5.0 は物理フォルダだけ。Ctrl+G・サブフォルダ展開などは詳細表示の
   長さ列で代わりに並べ替えられる。集約一覧は後続。
4. **絞り込みの区切り**: (推奨) 〜1分 / 1〜10分 / 10〜60分 / 60分以上 + 範囲指定 (分単位)。
5. **Ctrl+G の検索構文の長さ条件**: (推奨) v5.5.0 では入れない (索引の作り直しが要る)。

## 10. 検証計画 (実装時)

- 比較: Known の昇順・降順、Unavailable・Missing が両方向で末尾、名前の tie、NaN・負値の扱い、4 行割当。
- materialize: snapshot 優先 → catalog → Missing、stamp 不一致は Missing、catalog が無い・壊れているときの
  Missing、Remote が同じ順を返す。
- 並べ直し: Ready で一致しなければ再読込 1 回、再読込後は一致して止まる、全画面中は並べ直さない、
  catalog 保存失敗でもループしない、選択位置の維持、長さ順以外では何もしない。
- 取得要件: 長さ順の物理フォルダで Missing があるときだけ `AllRequirements` に入る、対象外の一覧では入らない、
  facet の要求・有効時に入る。
- facet: プリセット境界、範囲指定、Ready まで素通し、Ready 後の件数と除外、保存の往復、旧 JSON 欠落。
- 設定: 12 値の roundtrip、toolbar 補完 marker、コレクション・代表サムネイル候補に出ないこと、
  ゲームパッド巡回、Remote options。
- UI スナップショット: 絞り込みバーに長さが増える分の更新。
