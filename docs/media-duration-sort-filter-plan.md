# 動画・音声の長さで並べ替え・絞り込み (§1.762)

最終更新: 2026-11-20

状態: 実装済み (未レビュー)。正本は設計第4.2版 `574f1ccd5`（利用者からの引き継ぎ: 独立レビュー5回目で ACCEPT）。
再起動後の利用者指示により、ビルド・テスト・PNG更新は行っていない。実装・静的確認・未実施事項は §11。

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
  着手前に確認した AI 待機中の早期 `return true` は、§1.822 (`b71e60f75`) で修正済み。
  長さ facet も、待機中に他の facet を素通しにしない。
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
- 動いている session の対象が古くなったとき: 長さのフィールド要件を含む session が動いている間に、その計画開始時の
  (`items_generation`, `items.len()`) と今の値が違えば、details-meta の poll がその session を取り消して破棄し、
  状態を `NotRequested` へ戻す (レビュー 4 回目 P2)。その後は上の取り直しの規則で新しい session を始める。
  この取り消しは通常操作 (削除・追加) による世代交代の処理で、旧 session の channel 切断を `Cancelled` として
  受けない。利用者の「停止」による `Cancelled` は従来どおり維持する。

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
- **別件で修正済み**: AI 待機中に他の facet まで素通しにする問題は、先行作業 §1.822 (`b71e60f75`) で修正。
  本実装はその修正と回帰テストを保持する。

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
  取得完了後の削除で失効して即時に取り直す、取得中の削除 (詳細列・バッジ・AI 要求すべて OFF) で旧 session を
  取り消して `NotRequested` から取り直し `Cancelled` に落ちない、利用者の停止は `Cancelled` のまま取り直さない、
  fork 先は None、park / mount / drop で兄弟 context 不変。
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

## 11. 実装記録と設計担当への引き渡し

### 11.1 実装範囲

- `src/settings.rs` / `settings_db.rs` / `settings_transfer.rs`: ListingDuration、12種類の一覧順、明示的なCollection・代表画像候補、長さfacetのプリセット・分単位範囲、設定互換と候補補完marker。
- `src/catalog.rs` / `app/folder_scan.rs` / `app/book_materialization.rs`: read-only一括取得、ScannedDir.duration_facts、source stamp一致時だけの「メモリ → catalog → Missing」。UI側はメモリclosure、外部再走査側はArc snapshot。既知sizeが無ければどちらも照合しない。
- `src/app.rs` / `app/duration_metadata.rs` / `app/metadata_ops.rs`: 長さの要件とAIのfield要件を分離。全itemsのVideo/Audioを有界WholeListing計画で走査し、隠れた行では長さ以外の取得範囲を広げない。sessionの世代・件数snapshotをpendingが所有し、一致した完了だけでproofを作る。
- `src/app/viewer_context_registry.rs`: 既存details状態と同じ構造体・初期化・swap・fork列挙へproofを機械的に登録。fork先は空、元contextはcacheとproofを保持。detached述語・viewportの判断は変更していない。
- `src/filename_stack.rs` / `filename_stack_ui.rs`: 完全なsort metadataをgroup/集約/flatへ運ぶ。スタックcell自体はMissing。関連するstartup・script・外部ツール等のfixture初期化を機械更新。
- `src/app/listing_sort.rs` / `ui_main.rs` / `app/gamepad_input.rs`: 共通capabilities、disabled理由、読み込み表示、長さfacet、変更／同値再選択時のworker再読込。ゲームパッドの巡回は非対応候補を飛ばす。
- `src/global_search_ui.rs` / `global_search_ui/everything.rs` / `app/smart_folder.rs` / `app/subfolder_expansion.rs` / `rating_view.rs` / `bookmark_browser.rs` / `remote_ipc/{mod,container}.rs`: 非対応producerの長さ順だけを名前順へ正規化。Remoteの候補・selected・実際の順を揃える。
- `src/collection_store/model.rs` / `thumb_loader.rs`: 新variantの網羅と既存allowlist外の扱い。`src/ui_dialogs/smart_folder_editor.rs`: 保存できない長さ条件を既存の保存対象外条件の確認に明示。
- fixtureの機械更新: `src/app/tests.rs` / `app/tests/startup_restore.rs` / `app/similar_navigation_tests.rs` / `filename_stack_script.rs` / `external_tool.rs` / `ui_dialogs/caption_job.rs`。新しい状態回帰は `src/app/tests/duration_762.rs`。
- `tests/ui_snapshot.rs` / `src/lib.rs`: 実ラベル・既存レイアウトhelperを共用するstateless描画fixtureを追加。PNGは変更していない。
- 文書: `docs/spec.md` / `details-view-and-filter-plan.md` / `catalog-design.md` / `async-architecture.md` / `ui-responsiveness.md` / `filename-stack-plan.md` / `list-size-sort-plan.md` / 本書。公開文書は `htdocs/mimageviewer/manual/{grid,settings,tut-grid-power}.html` と `htdocs/mimageviewer/index.html`。

### 11.2 物理フォルダへの入口台帳

| 入口 | 走査・採用の経路 | 長さの取り扱い |
| --- | --- | --- |
| パスを開く・分類を必要とする直接移動 | App::start_open_path_classification_owned_with_navigation (`app.rs`, open-path-classify) | 要求時の保守的boolを捕捉し、directory scan後にworkerでfactsを読む。UI materializeはメモリ優先 |
| Ctrl+↑↓・兄弟移動・一覧順移動 | App::spawn_folder_nav (`app.rs`, DFS/兄弟移動worker) | 行き先を決めた後のscanにfactsを付ける。ページ固定・非対応surfaceは要求を正規化 |
| folder pane・長さ順への変更／同値再選択・更新のworker入口 | App::start_folder_open_scan_with_restore_and_navigation (`app.rs`) | scan後のworker内だけでfactsを読む。メニュー・ツールバー・ゲームパッドの長さ操作はWorkerScanへ合流 |
| BS・履歴・物理コンテナのpreflight | App::start_physical_history_preflight_with_password (`app/collection_navigation.rs`, physical-history-preflight) | directory分岐のworker scan後にfactsを付ける。準備済みscanはそのfactsを保持 |
| 外部変更・通常フォルダreconcile | App::start_external_rescan_with_cause (`app.rs`, external-folder-rescan) | 要求時のメモリArc snapshotとscanのfactsでworker内materialize。採用時にUIでcatalogを開かない |
| 同期load・起動／設定適用等でpre-scanを持たない従来入口 | load_folder_with_scan_claimed_with_search_origin → scan_directory_with_settings | factsはNone。追加catalog I/Oなし。メモリだけで並び、利用者の更新・再選択でcatalog分が入る |
| Smart/Collectionの物理の子、集約一覧、Remote | 各source ownerの既存scan/materialize | 非対応の長さ順は名前順を要求。長さfacetはPCのitemsに対し利用可能。Remoteの候補には出さない |
| 製本・画像フォルダ本・ZIP/PDFページ、代表画像探索 | 既存book/page/thumbnail経路 | 固定順と代表画像allowlistを維持。画像専用scanへ長さ取得を追加しない |

`default_cache_dir()`は保持済みdata-dirからのパス組立てだけ。新しいcatalog open/SELECTは上記workerか、既存details-meta workerだけで行う。同期materializeへI/Oを追加していない。

### 11.3 完了・取消の台帳

| 状況 | 処理 |
| --- | --- |
| workerのFinished | pendingのsnapshotを取り出し、revision・世代・件数が一致する完了だけでproofを作る |
| 全targetがメモリで充足 | launch_details_meta_workerのtarget_count==0で共通完了関数へ進む |
| 計画対象0件 | plan.total==0で同じ完了関数へ進む。空一覧も長さ要件がある場合は完了できる |
| 取得中の追加・削除・世代交代 | poll先頭で古いsnapshotのsessionをcancel・破棄してNotRequestedへ。旧channel切断を利用者の停止に読み替えない |
| 完了後の追加・削除 | 世代・件数の不一致からproofが無効となり、pendingなしなら新しいAllRequirementsを計画 |
| 利用者の停止 | Cancelled中は自動再計画しない。他facetの再構築でも停止を維持し、既存resume操作で再開 |
| 選択情報等のVisibleStage | 新proofを立てず、既存の有効proofも消さない |
| facet変更・取得完了 | 長さ条件だけをproofが有効になるまで外す。proof完成時に表示集合を再構築。items順を自動変更しない |

### 11.4 コード照合で補足した点

1. 保存済みお気に入り表示状態はSettingsではなくApp.favorite_view_states (`src/app.rs:18362`) が所有。Settings::listing_may_sort_by_durationへvaluesを渡し、共通overlay・live・保存済みのORを保つ。
2. 絞り込みバーには一般の新候補補完が無い（`src/settings.rs:9933`）。長さメニューを旧既定バーにも届けるためsort用とは別のone-time markerを追加。旧既定完全一致だけ補完し、空・順序変更・非表示は保持。NameFilterの既存stashも保持。DB表・schemaは変更なし。
3. ゲームパッドsort行は左右巡回で（`src/app/gamepad_input.rs:11160` のpicker_row_supports_value_list）、値一覧から同値を選び直す操作は存在しない。巡回の候補制限と変更時のworker経路を接続し、共通handlerへ同値duration要求が届いた場合もworker再読込にする。新キー操作・値一覧は追加しない。
4. §1.822は着手前にコミット済みで、本書の「既存の問題」を修正済みの記録へ直した。
5. 物理一覧prepareはsurface採用より前にも走る。直接loadは既存destination ownerから並べ替え用surfaceを投影し、元のbook recipeや退去するrating等のsurfaceだけで新しい物理一覧を制限しない。Smart/Search/Collectionの帰路を持つ子は対象外のまま。
6. 既存facet-sort snapshotはtests/ui_snapshot.rsではなくui_main.rsのlib testのsection292_facet_sort_snapshotにある。既存2fixtureを追従させ、指定のintegration側にも共通描画fixtureで通常幅／狭幅を追加した。

7. 走査採用のprojection keyは従来どおり正規化前の要求を識別し、実際の並べ替え要求だけを名前順にする。worker・UI prepare・既存の現在値照合で同じkey契約を保持する。

### 11.5 追加した主な回帰ソースとrunner向けfilter

**全て未実行。成功件数の記録ではない。**

- app::tests::duration_762 の11件: field要件分離、画面外・絞り込み外、cached/RetryLater/0件完了、Finishedと他facet、追加・削除時の退役、停止→別facet変更→再開、VisibleStage、stamp、空Range、worker再選択、正規化前のprojection identity保持。
- duration_762_proof_and_cache_follow_mount_park_and_drop_without_sibling_changes / duration_762_fork_keeps_source_proof_and_starts_without_metadata_proof: context境界。
- duration_762_picker_selected_duration_uses_worker_reload: gamepad handler。
- duration_762_projection_identity_preserves_requested_sort_on_search_physical_child: 非対応sortでも走査採用の識別値が一致すること。
- app::folder_scan::duration_tests: stamp・size不明、出どころ優先、worker snapshot同順、非長さ順でreader不呼出、不要時と動画なしのcatalog非作成。
- video_duration_bulk_read_keeps_source_identity_and_uses_read_only_connection / video_duration_read_does_not_initialize_missing_catalog_or_migrate_old_schema。
- duration_sort_survives_group_aggregate_flat_and_audio_passthrough / extract_stack_parts_preserves_video_and_audio_duration_snapshots。
- duration_sort_rounds_milliseconds_and_keeps_all_unknowns_last / duration_presets_boundaries_ranges_and_legacy_json / duration_scan_may_be_needed_by_common_live_or_saved_favorite_sort / duration_sort_wire_and_candidate_allowlists。
- duration_toolbar_migration_only_extends_untouched_defaults_once / duration_facet_toolbar_migration_respects_name_filter_stash_and_customization / toolbar_duration_markers_survive_settings_db_reload_without_restoring_hidden_options / duration_settings_roundtrip_and_older_reader_preserves_incompatible_db。
- capabilities・menu・toolbar・gamepad・Remote・rating・bookmarkのテスト名にはduration、book_and_extracted_rows、existing_sort_locksを含めた。§1.822のai_facet_も保持。

設計担当の共有runnerでの提案（この実装セッションでは実行しない）:

```powershell
$env:CARGO_BUILD_JOBS='6'
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib -E 'test(/duration|book_and_extracted_rows|existing_sort_locks|ai_facet_/)'
cargo test -p mimageviewer --test ui_snapshot duration_facet_bar
cargo check -p mimageviewer --bin mimageviewer-core
```

以降の関連全体gate・確認用buildは設計担当が既存方針で判断する。

PNG更新候補（全て未更新）:

- 既存 tests/snapshots/facet_sort_dark.png / facet_sort_narrow_dark.png
- 新規 duration_facet_bar_dark.png / duration_facet_bar_narrow_dark.png

後者はtests/ui_snapshot.rs、前者はsection292_facet_sort_snapshotのlib testで更新する。更新と見た目確認も共有runner／設計担当へ引き渡す。

### 11.6 静的確認と未確認

- python scripts/check_ui_glyphs.py: exit 0、危険な字形0件。
- python scripts/check_site_cards.py: exit 0、53カード・上限超過0件。
- 新variantのmatch、Settings移送の網羅分解、ScannedDir/StackMember/DetailsMetaPending初期化、proofのcontext登録をgrepとコード読込で照合。独立静的点検で見つかった停止状態と空Rangeの問題も修正し回帰を追加。
- cargoのビルド・テスト・check・clippy、test/build script、製品起動、PNG生成、git add、commitは未実施。
- コンパイル、テスト実行結果、snapshot実画像、実機表示・性能は未確認。
- まれなcatalog読取失敗はログ＋空factsとして継続。既存データを削除・再作成せず、メモリ分と未取得末尾で一覧を出す。

- 最終整形: `cargo fmt` / `cargo fmt --check` とも exit 0。`git diff --check` も exit 0。PNG差分・ステージ済み差分は0件。
