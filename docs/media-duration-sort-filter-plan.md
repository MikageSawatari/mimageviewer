# 動画・音声の長さで並べ替え・絞り込み (§1.762)

最終更新: 2026-11-20

状態: 設計第4.1版 (第4版 `44ed23c12` にレビュー 3 回目の P2 2 点を反映)。利用者決定 (2026-11-20、§9) と独立レビュー 2 回 (GPT-6.1 Sol / xhigh、どちらも REVISE) を反映。
実装未着手。

- 第1版 `2724a5c40`: 自動の並べ直し (A) を推奨。レビュー 1 回目で、再読込の繰り返し・複数選択の消失・
  materialize が UI スレッドで走る経路・スタック経路で長さが落ちることなどを指摘された。
- 第2版 `81db51421`: 自動の並べ直しを外した案 (B)。レビュー 2 回目で、走査の時点では実効の並べ替えが分からない
  (お気に入り別の表示状態は走査後に UI で決まる)、AI 用述語の流用で AI 解析まで走る、「全件読み終えた」証拠が
  今の状態では判定できない、長さ条件で絞った集合を取得対象にすると隠れた項目が取得されない、を指摘された。
- 第3版 (コミットせず破棄): 利用者がいったん A を選んだため A の形で書いたが、レビュー前に利用者が B へ戻した。
- 第4版 (本版): B。第2版にレビュー 2 回目の指摘を反映した。

## 1. 目的と範囲

利用者の要望 (2026-11-11、v5.5.0 目標 2026-11-19): 動画・音声を再生時間の長さで並べ替えたい・絞り込みたい。
前提は「一覧の表示を遅くしない」こと。

v5.5.0 で入れるもの:

1. 一覧の並べ替え (`SortOrder`) に「長さ（短い順）」「長さ（長い順）」を足す。PC 本体の物理フォルダの一覧で使える (§3.3)。
2. 絞り込みバーに「長さ」を足す (〜1分 / 1〜10分 / 10〜60分 / 60分以上 / 分単位の範囲指定)。どの一覧でも使える。

入れないもの (§8 で後続候補):

- アイテム検索 (Ctrl+G) の検索構文の長さ条件 (`duration:>10m` など)。Tantivy 索引に長さの field が無い。
- 物理フォルダ以外の一覧と Remote での長さ順。PC 本体の一覧は、既にある詳細表示の「長さ」列の見出しで並べ替えられる。

## 2. 現状 (コードで確認、2026-11-19 master `ca6686d93`)

### 2.1 長さの保存と取得

- フォルダ単位の catalog (`src/catalog.rs`) に `video_meta` 表がある (§1.308、**v4.10.0 で出荷済み**)。
  mtime と file_size が完全一致するときだけ再利用する。`readable=0` は確定した読み取り失敗。音声も同じ表。
  [catalog-design.md](catalog-design.md) の「動画・音声メタデータ」。一括で読む API は無い。
  read-only open は `CatalogWork::open_existing()` → `open_existing_read_only_admitted()` があり、schema 初期化・
  migration を通らない (`src/catalog.rs` 326 / 1397 行付近)。
- 書き込むのは details-meta worker だけ (`load_details_video_meta_with_probe`、`src/app/metadata_ops.rs`)。
  catalog lookup → miss のときだけ FFmpeg probe → 確定結果を保存。取消・10 秒 timeout・I/O エラーは
  `Interrupted` として保存せず、メモリでは同じ一覧世代の終端 (`RetryLater`) にする。
- UI はメモリの `details_lazy_meta` (`DetailsLazyMeta.media` 内の長さ) だけを読む。
  `details_lazy_meta` / `details_meta_pending` / `details_image_dims_state` は viewer context ごとの bundle に
  入り swap される (`src/app/viewer_context_registry.rs`)。`ai_model_facet_requested` は App 全体の bool で、
  一覧の install 時に `facet_ai_filter_active()` へ戻る (`src/app.rs` 45833 行付近)。
- details-meta の要件は、詳細表示の遅延列・サムネイルの長さバッジ/再生位置メーター (可視+近傍の段階取得、
  `VisibleStage`)・AI facet (表示集合全体、`AllRequirements`)・選択情報。`ai_model_facet_should_load()` は
  走査範囲だけでなく、AI メタデータ・作成日時・画像寸法・ページ数のフィールド選択にも使われている
  (`src/app.rs` 74228〜74320 行付近)。thumbnail 表示の target 生成には可視範囲の制限がある (75171 行付近)。
- readiness は context ごとの 1 つの `LazyColumnState`。`Ready` は失敗件数だけを持ち、どの範囲・フィールドで
  完了したかを持たない。Finished の処理は pending (scan scope を持つ) を捨ててから共通の完了関数へ進む
  (`src/app.rs` 74040 / 74862 行付近)。全件取得の後も、選択情報の補助取得で `VisibleStage` が始まる。

### 2.2 並べ替え

- `SortOrder` は 10 値。並べ替えは一覧の materialize 時に一度だけ行い、`items` の順そのものを表示順とする
  ([rating-snapshot-order-plan.md](rating-snapshot-order-plan.md) §0)。
- 比較は `SortOrder::compare_listing_keys` が名前 key と `ListingSortMetadata { mtime, file_size: Option }` で
  行う。サイズ順は既知を値で比べ、不明は昇順・降順とも末尾、同値は名前昇順
  ([list-size-sort-plan.md](list-size-sort-plan.md))。4 行割当の行の中だけで比べる。
- 物理フォルダの materialize は、UI スレッドの `prepare_scanned_folder_listing_with_cache_keys`
  (`src/app.rs` 32311 行付近) と、worker (外部変更の reconcile、Remote) の両方で走る。実効の並べ替えは、
  お気に入り別の表示状態 (未保存の live 状態を含む) を UI で投影した `listing_settings` から
  `ListingOrderRequest` として決まる (32350 / 95077 行付近)。
- 走査は worker (open-path-classify など、29818 行付近) と UI スレッドの同期走査 (`scan_directory_with_settings`、
  32133 行付近) の両方がある。メニュー・ツールバー・ゲームパッドの並べ替え変更は `apply_sort_change_reload()`
  = `PhysicalFolderSortReload::Immediate` (同期走査) を使い、別の入口は worker 走査
  (`apply_sort_change_reload_without_ui_io`) を使う (28886 行付近)。
- 選択中の評価順の再選択は「読み直して並べ直す」(`SortControlDecision::Reload`、`src/ui_main.rs`)。
- 非対応の並べ替えの理由表示は `sort_order_disabled_reason` (コレクション / ブックマークの評価順だけ)。
  ゲームパッドと Remote (`remote_grid_sort_state`) は別経路で `all()` を使う。
- `toolbar_sort_items` には候補追加ごとの一回限りの既定補完 (marker 付き) がある。
  `collection_options()` は `all()[..8]` の位置スライス。
- ファイル名スタックは物理 materialize の後に `StackMember` を通して group / 集約 / flat を作り直し、
  `ListingSortMetadata` を mtime・size から組み立て直す (`src/filename_stack_ui.rs` 398 行付近、
  `src/filename_stack.rs` 171 / 197 / 489 / 522 行付近)。動画は単独 group としてこの経路を通る。

### 2.3 詳細表示と絞り込み

- 詳細表示には既に「長さ」列 (`DetailsSortKey::VideoDuration`、動画・音声) があり、列見出しで並べ替えられる。
  **どの一覧でも使える。** 本計画はこれを変えない。
- 絞り込みバー (`FacetFilter`、`settings.facet_filter` に保存) に長さは無い。サイズ facet はプリセット 4 つ +
  範囲指定で、サイズの無い行は除外する。
- AI facet が遅延データ facet の前例 (メニューを開くと表示集合全体を取得、読み込み中は進捗と停止)。
  既存の `passes_facet_filter` は AI 条件が有効で未 Ready のとき **他の facet の判定まで飛ばして** `true` を返す
  (§8 に既存の問題として記録。長さ facet はこの形を写さない)。
- Remote には絞り込みバーが無い。
- settings の読み込みは、未知の field を無視し、未知の enum 値を `Incompatible` として既定設定 + 保存抑止で
  起動する (DB は書き換えない、`src/settings_db.rs` 3404 / 4075 行付近)。

## 3. 設計の要点

### 3.1 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- **自動の並べ直しはしない** (利用者決定 B)。長さ順は他の順と同じく materialize 時に一度だけ並べる。後から長さが
  分かっても並びは動かさず、利用者の「最新の情報に更新」・長さ順の再選択・開き直しで並べ直す (評価順と同じ)。
  第1版の自動の並べ直しで必要だった終了条件・前面判定・複数選択の保護・detached の前面述語が全部要らない。
- **長さの出どころは既存のものだけ**: 並べ替えは「メモリの `details_lazy_meta` ∪ catalog の `video_meta`」、
  絞り込みとバッジはメモリだけ。どちらも既存の details-meta worker が埋める。新しい worker・DB 表・probe 経路を
  作らない。materialize は I/O をしない純関数のまま。
- **catalog は worker で、要るかもしれないときだけ読む** (§4.1)。走査の時点では実効の並べ替え (お気に入り別) が
  分からないので、「この走査の結果が長さ順で並べられる可能性」を UI が要求時に保守的に判定して worker へ渡す。
  可能性が無ければ catalog を開かない。お気に入りの投影を worker へ複製しない。
- **長さ順へ変える操作は worker 走査の経路に通す** (§4.1)。UI スレッドの同期走査に catalog 読み取りを足さない。
- **取得の対象はその一覧の全動画・音声** (表示集合・絞り込みに依らない、§5.1)。対象集合の変化で完了の証拠が
  失効する組み合わせが消える (レビュー 2 回目 P2-4)。
- **長さ順を使える一覧を PC 本体の物理フォルダに絞り、使える並べ替えは 1 つの helper で決める** (§4.3)。
- **取得中のモーダル化は採らない** (全部読んでから一覧を出す案は前提に反し、利用者も選ばなかった)。
- まれな失敗は既定の割り切り (ログ + 通知 [+ 作り直し])。catalog を開けない・読めないときは、メモリにある分だけで
  並べ、残りは長さ未取得として末尾に置く。

### 3.2 長さの並べ替えキー

`ListingSortMetadata` に長さを足す。`Copy` のまま。

```rust
pub(crate) enum ListingDuration {
    /// 動画・音声で、長さが分かっている。有限・0 以上の秒をミリ秒に丸めた整数 (Eq / Ord を保つ)。
    Known(u64),
    /// 動画・音声で、probe 済みだが長さが無い (読み取り不可、長さ無し・非有限・負)。
    Unavailable,
    /// まだ取得していない、動画・音声以外、または長さ順でない。
    Missing,
}
```

- `ListingSortMetadata::new(mtime, size)` は `Missing` を既定にする。長さを埋めない producer は変更しない。
- `DurationAsc`: Known を短い順、その後 Unavailable と Missing (この 2 つは区別せず名前昇順)。
  `DurationDesc`: Known を長い順、その後同じ。**不明は昇順・降順とも末尾**。同値は名前昇順。
- 4 行割当の行の中だけで比べる。フォルダ・書庫・画像の行は全部 Missing なので名前昇順。画像と動画を同じ行に
  混ぜる表示順の設定では、画像は動画・音声の後ろに名前順で並ぶ。
- ミリ秒への丸めは並べ替えだけに使う。表示の値 (`DetailsLazyMeta`) は変えない。

### 3.3 長さ順を使える一覧

| 一覧 | v5.5.0 |
| --- | --- |
| PC 本体の物理フォルダ (main と各 detached context。開く・Ctrl+↑↓ / BS / folder pane・更新・外部変更の再走査) | 対応 |
| 物理フォルダ内のファイル名スタック (集約 / flat) | 単独の動画セルは長さ順 (§4.4)。スタックセルと画像は Missing |
| 本扱いの画像フォルダ・製本・ZIP/PDF のページ順 | 固定順のまま (既存の page order lock) |
| Ctrl+G・スマートフォルダ (物理の子を含む)・サブフォルダ展開・レーティング一覧・ブックマーク・コレクション | 対象外。長さ順を選べず理由を出す (§4.3)。全体の設定が長さ順のまま開いたときは名前昇順で並ぶ |
| 閲覧履歴・タグ一覧・ドライブ一覧など固定順の一覧 | 既存の順のまま |
| Remote (物理フォルダを含む) | 対象外。Remote の候補に出さない |
| フォルダ代表サムネイルの候補順 | 対象外 (`folder_thumb_options` の allowlist のまま) |

## 4. 並べ替えの流れ

### 4.1 長さの出どころと読む時点

materialize は、各動画・音声の長さを次の順で決める (先に見つかったものを使う)。

1. **メモリ**: `details_lazy_meta` に、走査で得た mtime / size と source stamp が一致する値 (Known / Unavailable)
   があれば使う。stamp の判定は既存の `details_lazy_meta_satisfies_idx` と同じものを使う (実装時に
   `DetailsLazyMeta` の stamp の持ち方を確かめる)。
   - UI スレッドの materialize (`prepare_scanned_folder_listing_with_cache_keys`): `details_lazy_meta` を引く closure
     (評価順の `read_rating` closure と同じ形)。メモリの読み取りだけ。
   - worker の materialize (外部変更の reconcile): 要求を作る時点で、UI が今の一覧の動画・音声の長さを
     (正規化 path, stamp) → `ListingDuration` の不変 snapshot (`Arc`) にして要求に入れる。
2. **catalog**: `ScannedDir.duration_facts: Option<FolderDurationFacts>` に、basename + mtime / file_size が
   完全一致する行があれば使う。
3. どちらにも無ければ Missing。走査失敗で size が不明の項目 (`ListingSortMetadata.file_size == None`) は照合しない。

`duration_facts` を作るのは `ScannedDir` を作る **worker** だけ:

- 要求時に UI が `listing_may_sort_by_duration` を判定して worker へ bool で渡す。判定は保守的で、次の OR:
  (a) 共通の並べ替え (お気に入りを適用中は `favorite_view_overlay.common` に退避された値、適用していなければ
  `settings.sort_order`、`src/settings.rs` 9313 行付近)、(b) 今の live の実効値 (`settings.sort_order`)、
  (c) 保存済みのお気に入り別の表示状態のどれか、が長さ順なら true。(レビュー 3 回目 P2-1: 「共通 = 長さ順、
  お気に入り = 名前順」でお気に入りの外へ移る場合を落とさない。)
  false なら catalog を開かない (通常の一覧の費用は増えない)。
- true のとき、走査の後で動画・音声が 1 件以上あれば、そのフォルダの catalog を read-only で開き
  (`open_existing_read_only_admitted()`、catalog が無ければ空の facts)、新しく足す一括読み取り
  (`filename, mtime, file_size, readable, duration_secs` だけの SELECT) で facts を作る。
  `perf::event` を入れ、件数と時間を記録する。
- UI スレッドの同期走査 (`scan_directory_with_settings`) は facts を作らない (`None`)。
- `duration_facts == None` で長さ順になったときは、メモリだけで並べる (catalog の分は Missing)。fail-fast にしない
  (判定は保守的で、並べ替えの変更・お気に入りの変更は自分の再読込を起こすため、None になるのは同期走査の入口と、
  要求後に並べ替えが変わった一時的な場合に限られる)。

長さ順へ変える操作と再選択の経路:

- メニュー・ツールバー・ゲームパッドで長さ順へ変える、または選択中の長さ順を選び直すときは、物理フォルダの
  再読込を worker 走査 (`apply_sort_change_reload_without_ui_io`) で行う。他の並べ替えの変更は既存の経路のまま。
- ほかに長さ順で物理フォルダを開く入口 (開く・Ctrl+↑↓・BS・folder pane・更新・外部変更) が worker 走査か同期走査かを
  実装時に列挙し、検証台帳に残す。同期走査の入口はメモリだけで並ぶ (catalog の分は利用者の更新で並ぶ)。

### 4.2 読み取りの費用

1 フォルダ = 1 DB の read-only 接続で 1 回の SELECT。WAL なので thumbnail writer の書き込みと並行でき、
writer の接続 mutex を共有しない。長さ順になり得ないときは開かない。

### 4.3 使える並べ替えの決め方と表示

- `SortOrder::all()` に `DurationAsc` / `DurationDesc` を足す (並びはサイズ順の後ろ)。`collection_options()` と
  代表サムネイル候補は位置スライスをやめて明示の一覧にし、長さ順を含めない。
- 受理済みの一覧から「使える並べ替え・実際に効く並べ替え・使えない理由」を返す純粋な helper を 1 つ置く
  (`listing_sort_capabilities`)。入力は `TopLevelGridSurface`、Smart / Collection の位置、受理済みの
  `ProjectionRecipe`、既存のページ順 lock と詳細列ソート。既存の `sort_order_disabled_reason`
  (コレクション / ブックマークの評価順) もここへ移す。Smart / Search の物理の子、本扱いのページ一覧を
  物理フォルダと区別する。producer へ渡す実効の要求にも同じ判定を使う (使えない一覧では長さ順を名前順に落とす)。
  - メニュー・ツールバー: 使えない並べ替えは disabled、tooltip に理由。長さ順:
    「この一覧では長さ順を使えません。詳細表示の長さ列で並べ替えられます」。
  - ゲームパッドの巡回: 使えない並べ替えを飛ばす。
  - Remote: 長さ順を候補に出さない。PC 本体の設定が長さ順のとき、Remote の一覧は名前順で並び、Remote の
    選択中の表示も名前順にする (Web UI の扱いは実装時に確かめる)。
- `toolbar_sort_items` は、現在の既定 10 値と完全一致するときだけ一回限りで 12 値へ補完する (既存の marker 方式、
  利用者決定)。
- 選択中の長さ順を再び選ぶと再読込して並べ直す (評価順の `SortControlDecision::Reload` と同じ)。
- 長さ順の説明文 (`description()`): 「動画・音声の長さが短い順。長さが分からないものは末尾。読み込み後に
  更新すると並べ直します」など。
- 詳細表示の列見出しソート (`DetailsSortKey::VideoDuration`) の挙動は変えない。

### 4.4 ファイル名スタック

`StackMember` (または既存の Stack snapshot) に、物理 materialize で得た完全な `ListingSortMetadata` を持たせ、
group 比較・集約表示・flat 表示の全経路でそれを使う。mtime・size から組み立て直して長さを落とさない。
集約したスタックセル自身は Missing (サイズ順と同じく代表を選び直さない)。

## 5. 未取得の長さ

### 5.1 取得の要件

- 走査範囲の述語と、フィールドの要件を分ける (レビュー 2 回目 P2-2)。
  - `whole_listing_meta_scan_wanted()`: 一覧全体の取得が要るか。AI facet (既存の `ai_model_facet_should_load()`)・
    長さ facet・長さ順のどれか。走査範囲 (`AllRequirements`) と、サムネイル表示で可視範囲の stage に
    置き換えないこと (`refresh_thumbnail_details_stage` の早期 return) だけに使う。
  - AI のフィールド要件は既存の `ai_model_facet_should_load()` のまま。長さだけの要求で AI 解析・作成日時・
    画像寸法・ページ数を取らない。
  - 長さのフィールド要件 `duration_lengths_wanted()`: 長さ facet の要求・有効、または長さ順が使える一覧で
    長さ順が選ばれていること。**継続条件**であり、「Missing がある間だけ」にしない (最後の Item と Finished の間で
    要件が消えて worker が取り消されるのを防ぐ、レビュー 1 回目 P2-5)。
- 長さの対象は、その一覧の**全動画・音声** (`items` の Video / Audio。表示集合・絞り込み・可視範囲に依らない)。
  thumbnail 表示の target 生成の可視範囲制限も、長さの要件があるときは動画・音声に適用しない。
  項目ごとの未充足は「その動画・音声のメモリに、source stamp が一致する長さ (Known / Unavailable) が無い」。
- 可視行は Normal、画面外は Low の既存 priority。`RetryLater` は既存どおり同じ一覧世代の終端として扱い、
  同じ世代で再試行しない。catalog にある分は cache hit で probe しない。
- 並べ替えのために読んだ長さは catalog に残るので、次に開いたとき (worker 走査の入口) と利用者の更新で並びに入る。
- `duration_facet_requested` の寿命は `ai_model_facet_requested` と同じ (App 全体の bool、一覧の install で
  `duration_preset.is_some()` へ戻す)。

### 5.2 「全部読み終えた」の証拠

`LazyColumnState::Ready` は範囲を持たないので、長さについての完了を別に記録する (レビュー 2 回目 P2-3)。
絞り込みの「読み込み完了まで長さ条件を適用しない」(§6) と、読み込み中の表示 (§5.3) の終わりに使う。

- viewer context ごとの details 状態に `duration_scan_proof: Option<DurationScanProof>` を足す
  (`details_image_dims_state` と同じ bundle で swap。初期化・構造体の列挙・fork 時の所有方針にも機械的に登録する。
  fork 先は metadata cache が空なので proof も None、元の context は cache と proof を一緒に残す。既存の details
  状態と同じ配置、`src/app/viewer_context_registry.rs` 3957 / 4858 行付近)。
- 中身は**取得対象の snapshot**: session の計画を始めた時点の `items_generation` と `items.len()`
  (レビュー 3 回目 P2-2)。一覧世代は同じ一覧内の削除でも進み、逆に Smart Folder の評価条件の更新は世代を
  進めずに行を足す (`push_grid_item_pending`、`src/app/smart_folder.rs` 1699 / 1783 行付近) ため、世代だけでは
  対象の変化を表せない。
- 立てる: 長さのフィールド要件を含む `AllRequirements` の session が終わり、採用時の (`items_generation`,
  `items.len()`) が計画開始時と一致するとき。Finished・target が全部メモリで満たされていて即時に終わる場合・
  計画段階で対象 0 件の場合の、すべての完了経路で立てる。完了経路は実装時に列挙し、検証台帳に残す。
  一致しなければ立てない。
- 有効: proof の (`items_generation`, `items.len()`) が今の値と一致すること。対象は全動画・音声なので、
  絞り込みの変更では失効しない。選択情報の補助取得 (`VisibleStage`) や他の要件の追加でも失効しない。
  取消・停止では立てない (停止中は未完了)。
- 取り直し: 長さのフィールド要件が有効で、proof が無効 (対象が変わった) で、details-meta の session が
  動いていないとき、details-meta の poll が新しい `AllRequirements` session を始める。新しい一覧の install に
  限らず、削除・追加・復元で対象が変わった場合も同じ規則で取り直す。全部メモリで満たされていれば即時に完了して
  proof が立つ。印は足さず、この条件だけから導く。停止 (`Cancelled`) の間は取り直さない (利用者の再開を待つ)。

### 5.3 読み込み中の表示

- 長さ順の並べ替え項目 (ツールバーのボタン・メニューの選択中の項目) の tooltip と、ツールバーの短いラベルの
  後ろに「長さを読み込み中 n/m」を出す。値は既存の `LazyColumnState::Loading { done, total }` (長さの要件を
  含む session のとき)。`duration_scan_proof` が有効になったら消す。
- 読み終えたときの通知は出さない。説明文 (§4.3) とマニュアルに、読み込み後に更新で並べ直せることを書く。

## 6. 絞り込み (長さ facet)

- `FacetFilter` に `duration_preset: Option<FacetDurationPreset>` を足す。プリセットは
  〜1分 `[0, 60)` / 1〜10分 `[60, 600)` / 10〜60分 `[600, 3600)` / 60分以上 `[3600, ∞)` 秒、範囲指定は
  分単位の min / max (サイズ facet の `Range` と同型)。`settings.facet_filter` と同じく保存する。
  旧版は未知の field として無視する (§2.3)。
- 判定: 動画・音声でメモリの長さが Known かつ範囲内だけを通す。Unavailable・未取得・動画音声以外
  (フォルダ・書庫・画像・スタック) は落とす (サイズ facet がサイズの無い行を落とすのと同じ)。
- 取得: 長さメニューを開いたとき (`duration_facet_requested`)、または長さ条件が有効なとき、§5.1 の要件で
  全動画・音声を取得する。
- **`duration_scan_proof` が有効になるまで、長さ条件だけを適用しない** (未取得のものも表示する。利用者の決定)。
  他の facet は通常どおり判定する (AI facet の早期 `return true` は写さない)。proof が立ったら
  `rebuild_visible_indices`。
- メニュー: 読み込み中は「長さを読み込み中 n/m」と停止 / 再開 (AI facet と同じ)。proof 有効後は各候補の件数。
- すべての一覧 (Ctrl+G・コレクション等を含む) で使える。集約一覧で動画が多いと取得に時間がかかるが、
  取消でき、その間は長さ条件だけが素通しになる。
- Remote には絞り込みバーが無いので変更しない。

## 7. 変更しないもの・互換

- DB の schema は変えない。catalog は読み取りの API を 1 つ足すだけ。
- 永続データで変わるのは settings の `SortOrder` の新しい値 (`settings.sort_order`・`FavoriteViewState.sort_order`・
  `toolbar_sort_items`)、`FacetFilter` の新 field、toolbar 補完の marker。
  - 新しい `SortOrder` の値を v5.4.0 以前が読むと、その版は `Incompatible` として既定設定 + 保存抑止で起動する
    (DB は書き換えない。v5.5.0 に戻せば元の設定で起動する)。サイズ順・評価順を足したときと同じ境界で、
    toolbar の補完によって長さ順を使っていない利用者もこの境界に入る (利用者決定で許容、§9 の 6)。
  - `FacetFilter.duration_preset` は旧版が無視する。
  - 新旧の wire (serde 文字列) と、旧版相当の enum で読んだとき DB を書き換えないことを試験する。
- detached viewer: viewer context の bundle に `duration_scan_proof` の swap field を 1 つ足す (既存の
  `details_image_dims_state` と同じ扱いの機械的な追加)。detached 述語・viewport 経路には触れない見込み。
  触れた場合は CLAUDE.md の合意手続に従い [detached-rework-plan.md](detached-rework-plan.md) §11 に記録する。
  park / mount / drop で兄弟 context の状態が変わらないことを試験する。
- Remote IPC: Remote の候補から長さ順を外すだけで wire の構造は変えない。
- サムネイルの長さバッジ、詳細表示の長さ列、再生位置メーターは変えない。

## 8. 後続候補・既存の問題

- 集約一覧 (Ctrl+G・サブ展開・スマート・レーティング・ブックマーク) と Remote の長さ順。既存 hit の path / stamp から
  親ごとの catalog を worker で読む共通 helper を作れば足せる (親 DB の数だけ open の費用がかかる)。
- Ctrl+G の検索構文の長さ条件 (索引 schema の変更と再索引)。
- **既存の問題**: `passes_facet_filter` は AI モデル / 生成ツール条件が有効で未 Ready の間、タグ・日付・サイズ・
  状態などの他の facet まで判定せず全件を通す (`src/app.rs` 76380 行付近)。v4 系から既存の見込み。
  本計画では直さず、統括へ報告する。

## 9. 利用者の決定 (2026-11-20、統括経由)

1. 長さ未取得の並べ替え: **B**。すぐ一覧を出し、分かっている長さで並べ、未取得は末尾。自動では並べ直さない。
   「最新の情報に更新」・長さ順の選び直し・開き直しで並ぶ (評価順と同じ)。(いったん A を選んだ後、B へ変更。)
2. 読み込み中を見せる: 並べ替えの項目に「長さを読み込み中 n/m」、絞り込みの長さメニューに進捗と停止。
3. 長さ順を使える一覧: v5.5.0 は普通のフォルダだけ。ほかの一覧では案内を出す。
4. 絞り込みの区切り: 〜1分 / 1〜10分 / 10〜60分 / 60分以上と分単位の範囲指定。どの一覧でも使え、読み込みが
   終わるまでは未取得のものも表示する。
5. Ctrl+G の検索構文: v5.5.0 では入れない。
6. ツールバーの既定に長さ順の 2 つを足す。v5.4.0 へ戻したときの扱いはサイズ順・評価順と同じでよい。

## 10. 検証計画 (実装時)

- 比較: Known の昇順・降順、Unavailable・Missing が両方向で末尾、名前の tie、ミリ秒丸め、4 行割当、混在行。
- 出どころ: メモリ優先 → catalog → Missing、stamp 不一致・size 不明は Missing、UI 側 closure と worker 側 snapshot で
  同じ順、facts `None` はメモリだけで並ぶ、長さ順以外では長さを引かない。
- catalog: 一括読み取り API、read-only open (schema 初期化・migration をしない)、catalog 無しは空の facts、
  `listing_may_sort_by_duration` が false のとき開かない、お気に入り別 (live を含む) の長さ順で true。
- 入口: 長さ順への変更・再選択が worker 走査を使う、ほかの入口の列挙台帳。
- スタック: 集約 / flat / group 比較で動画の長さが保たれる、スタックセルは Missing。
- 取得要件: 長さだけの要求で AI 解析・作成日時・画像寸法・ページ数を取らない、サムネイル表示で画面外の
  動画・音声も対象、絞り込みで隠れた動画も対象、最後の Item と Finished の間で要件が消えない、
  `RetryLater` を同じ世代で再試行しない、列・バッジ全 OFF でも完了する、対応外の一覧・長さ順以外では入らない、
  AI facet の既存の挙動が変わらない。
- proof: 全完了経路 (Finished・即時完了・対象 0 件) で立つ、計画後に対象が変わった完了では立たない、取消・停止で
  立たない、補助 `VisibleStage` で消えない、Smart Folder の評価条件更新で行が増えたら失効して取り直す、
  取得完了後の削除で失効して即時に取り直す、停止中は取り直さない、fork 先は None、park / mount / drop で
  兄弟 context 不変。
- `listing_may_sort_by_duration`: 共通 = 長さ順・お気に入り = 名前順でお気に入りの外へ移ると true、
  共通・live・保存済みお気に入りのどれも長さ順でなければ false。
- 並べ替えの操作: `listing_sort_capabilities` の一覧種別ごとの結果、メニュー / ツールバーの disabled と理由、
  ゲームパッドが飛ばす、Remote の候補と表示、長さ順の再選択で再読込、評価順の既存の理由が変わらない、
  読み込み中の表示と proof 有効後に消えること。
- facet: プリセット境界、範囲指定、proof 有効まで長さ条件だけ素通し・他の facet は判定、有効後の件数と除外、
  保存の往復、旧 JSON 欠落、`duration_facet_requested` のリセット。
- 設定: 12 値の roundtrip、toolbar 補完 marker、コレクション・代表サムネイル候補に出ないこと、旧版相当の enum で
  読んだときの `Incompatible` と DB 非変更。
- UI スナップショット: 絞り込みバーに長さが増える分。
- 文書: `docs/spec.md`、本書、[details-view-and-filter-plan.md](details-view-and-filter-plan.md)、
  [catalog-design.md](catalog-design.md)、[async-architecture.md](async-architecture.md)、
  [ui-responsiveness.md](ui-responsiveness.md)、[filename-stack-plan.md](filename-stack-plan.md)、
  [list-size-sort-plan.md](list-size-sort-plan.md) (比較契約の追記)、
  `htdocs/mimageviewer/manual/` の並べ替え・絞り込みのページ、製品ページの機能一覧。
