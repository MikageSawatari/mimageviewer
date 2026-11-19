# 動画・音声の長さで並べ替え・絞り込み (§1.762)

最終更新: 2026-11-20

状態: 設計第2版。第1版 (`2724a5c40`) への独立レビュー 1 回目 (GPT-6.1 Sol / xhigh、REVISE) を反映し、
自動の並べ直しをやめた (§3.1)。利用者判断は §9 (統括経由で質問中)。実装未着手。

## 1. 目的と範囲

利用者の要望 (2026-11-11、v5.5.0 目標 2026-11-19): 動画・音声を再生時間の長さで並べ替えたい・絞り込みたい。
前提は「一覧の表示を遅くしない」こと。

v5.5.0 で入れるもの:

1. 一覧の並べ替え (`SortOrder`) に「長さ（短い順）」「長さ（長い順）」を足す。物理フォルダの一覧で使える (§3.3)。
2. 絞り込みバーに「長さ」を足す (〜1分 / 1〜10分 / 10〜60分 / 60分以上 / 範囲指定)。どの一覧でも使える。

入れないもの (§8 で後続候補):

- アイテム検索 (Ctrl+G) の検索構文の長さ条件 (`duration:>10m` など)。Tantivy 索引に長さの field が無い。
- 物理フォルダ以外の一覧 (Ctrl+G、スマートフォルダ、サブフォルダ展開、レーティング一覧、ブックマーク、
  コレクション) での長さ順。これらは既にある詳細表示の「長さ」列の見出しで並べ替えられる (§2.3)。

## 2. 現状 (コードで確認、2026-11-19 master `ca6686d93`)

### 2.1 長さの保存と取得

- フォルダ単位の catalog (`src/catalog.rs`) に `video_meta` 表がある (§1.308、**v4.10.0 で出荷済み**)。
  列は `filename / mtime / file_size / readable / duration_secs / width / height / codec`。
  mtime と file_size が完全一致するときだけ再利用する。`readable=0` は確定した読み取り失敗。
  音声も同じ表に入る。[catalog-design.md](catalog-design.md) の「動画・音声メタデータ」。
  一括で読む API は無い (`get_video_meta` は 1 件ずつ)。
- 書き込むのは details-meta worker だけ (`load_details_video_meta_with_probe`、`src/app/metadata_ops.rs`)。
  catalog lookup → miss のときだけ FFmpeg probe → 確定結果を保存。取消・10 秒 timeout・I/O エラーは
  `Interrupted` として保存しない。集約一覧でも実ファイルの親 + basename で元の catalog を使う。
- UI はメモリの `details_lazy_meta` (`DetailsLazyMeta.media` 内の長さ) だけを読む。
  `details_lazy_meta` / `details_meta_pending` / `details_image_dims_state` は viewer context ごとの bundle に
  入り swap される (`src/app/viewer_context_registry.rs`)。`ai_model_facet_requested` は App 全体の bool で、
  一覧の install 時に `facet_ai_filter_active()` へ戻る (`src/app.rs` 45833 行付近)。
- details-meta worker を動かす要件は、詳細表示の遅延列・サムネイルの長さバッジ/再生位置メーター
  (可視+近傍の段階取得、`VisibleStage`)・AI facet (表示集合全体、`AllRequirements`)・選択情報。
  readiness は context ごとの 1 つの `LazyColumnState`。

### 2.2 並べ替え

- `SortOrder` は 10 値。並べ替えは一覧の materialize 時に一度だけ行い、`items` の順そのものを表示順とする
  ([rating-snapshot-order-plan.md](rating-snapshot-order-plan.md) §0)。
- 比較は `SortOrder::compare_listing_keys` が名前 key と `ListingSortMetadata { mtime, file_size: Option }` で
  行う。サイズ順は既知を値で比べ、不明は昇順・降順とも末尾、同値は名前昇順
  ([list-size-sort-plan.md](list-size-sort-plan.md))。4 行割当の行の中だけで比べる。
- **物理フォルダの materialize は UI スレッドで走る経路がある**。走査 worker (`open-path-classify` など) は
  `ScannedDir` を返し、UI 側の `prepare_scanned_folder_listing_with_cache_keys` が
  `materialize_local_folder_listing_with_order` を呼ぶ (`src/app.rs` 32360 行付近。評価順の rating DB 読み取りも
  ここで UI スレッドから行う、評価順計画 §4.1 で合意済み)。外部変更の再走査 (reconcile) と Remote は worker 上で
  materialize する。
- 選択中の評価順の再選択は「読み直して並べ直す」(`SortControlDecision::Reload`、`src/ui_main.rs`)。
  最上位一覧の更新は `reload_top_level_grid` に合流する。
- 非対応の並べ替えの理由表示は `sort_order_disabled_reason` (コレクション / ブックマークの評価順だけ)。
  ゲームパッド (`src/app/gamepad_input.rs`) と Remote (`remote_grid_sort_state`、`src/remote_ipc/mod.rs`) は
  別経路で `all()` / `collection_options()` を使う。
- `toolbar_sort_items` には候補追加ごとの一回限りの既定補完 (marker 付き) がある。
  `collection_options()` は `all()[..8]` の位置スライス。
- ファイル名スタックは物理 materialize の後に `StackMember` を通して group / 集約 / flat を作り直し、
  その際 `ListingSortMetadata` を mtime・size から組み立て直す (`src/filename_stack_ui.rs` 398 行付近、
  `src/filename_stack.rs` 171 / 197 / 489 / 522 行付近)。動画は複数枚のスタックにならないが、単独 group として
  この経路を通る。音声は passthrough。

### 2.3 詳細表示と絞り込み

- 詳細表示には既に「長さ」列 (`DetailsSortKey::VideoDuration`、動画・音声) があり、列見出しで並べ替えられる。
  遅延列は Ready までソート・フィルタ無効 ([details-view-and-filter-plan.md](details-view-and-filter-plan.md) §15.7)。
  **どの一覧でも使える。** 本計画はこれを変えない。
- 絞り込みバー (`FacetFilter`、`settings.facet_filter` に保存) に長さは無い。サイズ facet はプリセット 4 つ +
  範囲指定で、サイズの無い行は除外する。
- AI モデル / 生成ツール facet が遅延データ facet の前例。メニューを開くと `ai_model_facet_requested` が立ち、
  表示集合全体の取得を始める。サムネイル表示でも可視範囲の stage に置き換えられない
  (`refresh_thumbnail_details_stage` の早期 return)。読み込み中はメニューに進捗と停止。
  ただし既存の `passes_facet_filter` は AI 条件が有効で未 Ready のとき **他の facet の判定まで飛ばして**
  `true` を返す (§8 に既存の問題として記録。長さ facet はこの形を写さない)。
- Remote には絞り込みバーが無い。Ctrl+G の索引には長さが無い。
- settings の読み込みは、未知の field を無視し、未知の enum 値を `Incompatible` として既定設定 + 保存抑止で
  起動する (DB は書き換えない、`src/settings_db.rs` 3404 / 4075 行付近)。

## 3. 設計の要点

### 3.1 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- **自動の並べ直しはしない (第2版で変更)**。第1版は「長さを読み終えたら、今の並びと分かっている長さの並びが
  違えば一度だけ再読込する」案だった。独立レビューで次が分かった。
  - 取得の対象 (表示集合) と再読込が使う長さ (catalog の全件) が違い、絞り込みで隠れた行があると判定が
    一致せず再読込を繰り返す。スタック経路で長さが落ちても同じ。画像と動画が同じ行にあると、動画同士の
    比較では並びのずれを検出できない。
  - 一覧の install は複数選択 (`checked`) と範囲選択の起点を消す。名前変更・D&D・別の viewer など、
    自動の再読込を受け付けてよいかの判定は、既存のどの述語とも一致しない (detached を含む)。
  これを全部扱うと、扱う組み合わせが大きく増える。長さの並べ直しは、評価順と同じく**利用者の更新操作**
  (「最新の情報に更新」・`GridReload`・長さ順の再選択) だけにする。並べ替えの時点は materialize の 1 つだけになり、
  終了条件・前面判定・選択の復元を新しく作らずに済む。
- **長さの出どころを 1 つにする**: 並べ替えは catalog の `video_meta`、絞り込みとバッジはメモリの
  `details_lazy_meta`。どちらも既存の details-meta worker が埋める。新しい worker・DB 表・probe 経路を作らない。
- **catalog は worker で読む**: materialize は I/O をしない純関数のままにし、catalog の長さは `ScannedDir` を作る
  走査 worker が読んで不変の facts として渡す (§4.1)。
- **長さ順を使える一覧を物理フォルダに絞る** (§3.3)。どの一覧でどの並べ替えが使えるかは 1 つの helper が決め、
  メニュー・ツールバー・ゲームパッド・Remote がそれを使う (§4.3)。
- **取得中のモーダル化は採らない**: 全部読んでから一覧を出す案は、初回のフォルダ表示を probe の完了まで
  待たせる (1,000 本の動画で数十秒になり得る)。前提の「一覧の表示を遅くしない」に反する。
- まれな失敗は既定の割り切り (ログ + 通知 [+ 作り直し])。catalog を読めないときは長さ未取得として末尾に並べる。

### 3.2 長さの並べ替えキー

`ListingSortMetadata` に長さを足す。`Copy` のまま。

```rust
pub(crate) enum ListingDuration {
    /// 動画・音声で、長さが分かっている。値は有限・0 以上の秒をミリ秒に丸めた整数 (Eq / Ord を保つ)。
    Known(u64),
    /// 動画・音声で、probe 済みだが長さが無い (readable=0、または duration_secs が NULL・非有限・負)。
    Unavailable,
    /// まだ取得していない、動画・音声以外、または facts が無い。
    Missing,
}
```

- `ListingSortMetadata::new(mtime, size)` は `Missing` を既定にする。非対応の producer は変更しない
  (全項目 Missing なので、長さ順が選ばれていても名前昇順と同じ結果になる)。
- `DurationAsc`: Known を短い順、その後 Unavailable と Missing (この 2 つは区別せず名前昇順)。
  `DurationDesc`: Known を長い順、その後同じ。**不明は昇順・降順とも末尾** (サイズ順と同じ)。同値は名前昇順。
- 4 行割当の行の中だけで比べる。フォルダ・書庫・画像の行は全部 Missing なので名前昇順。画像と動画を同じ行に
  混ぜる表示順の設定では、画像は動画・音声の後ろに名前順で並ぶ。
- ミリ秒への丸めは並べ替えだけに使う。表示の値 (`DetailsLazyMeta`) は変えない。

### 3.3 長さ順を使える一覧

| 一覧 | v5.5.0 |
| --- | --- |
| 物理フォルダ (main と各 detached context。開く・Ctrl+↑↓ / BS / folder pane・更新・外部変更の再走査) | 対応 |
| 物理フォルダ内のファイル名スタック (集約 / flat) | 単独の動画セルは長さ順 (§4.4)。スタックセルと画像は Missing |
| Remote の物理フォルダ一覧 | 同じ facts で並ぶ (catalog にある分)。Remote からは取得を始めない |
| 本扱いの画像フォルダ・製本・ZIP/PDF のページ順 | 固定順のまま (既存の page order lock) |
| Ctrl+G・スマートフォルダ (物理の子を含む)・サブフォルダ展開・レーティング一覧・ブックマーク・コレクション | 対象外。長さ順を選べず、理由を出す (§4.3)。全体の設定が長さ順のときは名前昇順で並ぶ |
| 閲覧履歴・タグ一覧・ドライブ一覧など固定順の一覧 | 既存の順のまま |
| フォルダ代表サムネイルの候補順 | 対象外 (`folder_thumb_options` の allowlist のまま) |

## 4. 並べ替えの流れ

### 4.1 catalog の長さを読む (worker)

- catalog に一括読み取りの API を足す: `read_video_durations(&self) -> Result<HashMap<String, (mtime, size,
  VideoDurationFact)>>` (`filename, mtime, file_size, readable, duration_secs` だけを SELECT)。
- 物理フォルダの `ScannedDir` を作る各 worker は、その時点の並べ替え要求が長さ順のときだけ、走査の後で
  そのフォルダの catalog を **read-only で開き** (既存 catalog の read-only open と admission を使う。
  schema 初期化・migration をする通常の `open()` は使わない。`default_cache_dir()`)、一括で読み、
  `ScannedDir.duration_facts: Option<FolderDurationFacts>` に入れる。catalog が無い (一度もサムネイルを作って
  いないフォルダ) ときは空の facts。
- 照合は basename + 走査で得た mtime / file_size の完全一致 (details-meta worker の lookup と同じ stamp)。
  走査失敗で表示用 metadata が `(0, 0)` の項目は、typed な size availability (`ListingSortMetadata.file_size`
  が None) を見て照合しない (Missing)。
- 入口: open-path-classify、フォルダ読み込み worker、Ctrl+↑↓ / BS / folder pane、更新、外部変更の reconcile、
  Remote の物理フォルダ。実装時に `ScannedDir` を作る呼び出し元を全部列挙し、検証台帳に残す。
- materialize は facts を `ListingSortMetadata.duration` に移すだけで I/O をしない。長さ順の要求で facts が
  `None` (入口の漏れ) のときは debug / test build で fail-fast、release は全部 Missing (名前順) で続行する
  (サイズ順の旧 API と同じ扱い)。

### 4.2 読み取りの費用

- 1 フォルダ = 1 DB の 1 回の SELECT。WAL なので thumbnail writer の書き込みと並行できる。read-only 接続は
  短命で、writer の接続 mutex を共有しない。費用は未測定なので `perf::event` を入れ、実装時に件数別に測る。
- 長さ順でないときは開かない。通常の一覧の表示に費用は足さない。

### 4.3 使える並べ替えの決め方と表示

- `SortOrder::all()` に `DurationAsc` / `DurationDesc` を足す (並びはサイズ順の後ろ)。`collection_options()` と
  代表サムネイル候補は位置スライスをやめて明示の一覧にし、長さ順を含めない。
- 受理済みの一覧から「使える並べ替え・実際に効く並べ替え・使えない理由」を返す helper を 1 つ置く
  (`listing_sort_capabilities`)。既存の `sort_order_disabled_reason` (コレクション / ブックマークの評価順) も
  ここへ移す。Smart / Search の物理の子と、本扱いのページ一覧を物理フォルダと区別する
  (`current_grid_reload_is_physical()` だけでは足りない)。
  - メニュー・ツールバー: 使えない並べ替えは disabled にし、理由を tooltip に出す。
    長さ順: 「この一覧では長さ順を使えません。詳細表示の長さ列で並べ替えられます」。
  - ゲームパッドの巡回: 使えない並べ替えを飛ばす。
  - Remote: 候補から外す。選択中が使えないときの表示は実装時に Web UI の扱いを確かめて決める。
- `toolbar_sort_items` は、現在の既定 10 値と完全一致するときだけ一回限りで 12 値へ補完する
  (既存の marker 方式)。
- 選択中の長さ順を再び選ぶと再読込して並べ直す (評価順の `SortControlDecision::Reload` と同じ)。
- 長さ順の説明文 (`description()`) に「長さが分からない動画・音声は末尾。読み込んだ後、更新すると並べ直します」と書く。
- 詳細表示の列見出しソート (`DetailsSortKey::VideoDuration`) の挙動は変えない。

### 4.4 ファイル名スタック

`StackMember` (または既存の Stack snapshot) に、物理 materialize で得た完全な `ListingSortMetadata` を持たせ、
group 比較・集約表示・flat 表示の全経路でそれを使う。mtime・size から組み立て直して長さを落とさない。
集約したスタックセル自身は Missing (サイズ順と同じく代表を選び直さない)。

## 5. 未取得の長さ (並べ替え)

### 5.1 取りに行く時期

長さ順が使える一覧 (§3.3) で長さ順が選ばれている間は、details-meta worker の表示集合全体の取得
(`AllRequirements`) に「動画・音声の長さ」を要件として足す。

- 要件は**継続条件** (`duration_sort_wants_lengths`: 並べ替えが長さ順・現在の一覧が対応) と、項目ごとの
  未充足 (その動画・音声のメモリに長さが無い) に分ける。「Missing がある間だけ」を要件にすると、最後の
  Item を受けてから Finished を受けるまでに要件が消え、worker が取り消されて Ready に届かない
  (レビュー P2-5)。
- AI facet と同じ集合 (現在の表示集合 `current_grid_order()`) を対象にし、サムネイル表示でも可視範囲の
  stage に置き換えない。既存の `ai_model_facet_should_load()` を「一覧全体の取得が要る」述語に広げ、
  AI facet・長さ facet・長さ順の 3 つで共有する。
- 可視行は Normal、画面外は Low の既存 priority。`RetryLater` は既存どおり同じ一覧世代の終端として扱い、
  同じ世代で再試行しない。catalog にある分は cache hit で probe しない。

### 5.2 読み込み中と読み終えた後

- 読み込み中: 長さ順の並べ替え項目 (ツールバーのボタン・メニューの選択中の項目) の tooltip と、ツールバーの
  短いラベルの後ろに「長さを読み込み中 n/m」を出す。値は既存の `LazyColumnState::Loading { done, total }`。
- 読み終えた後: 並びは変えない。利用者が「最新の情報に更新」・長さ順の再選択・一覧の開き直しをすると、
  そのとき catalog にある長さで並ぶ。2 回目以降に開くフォルダは、前回読んだ長さが catalog にあるので
  最初から並ぶ。
- 読み終えたことを知らせる通知は出さない (一回だけ出す印を状態として持たないため)。説明文 (§4.3) と
  マニュアルに更新で並べ直すことを書く。

## 6. 絞り込み (長さ facet)

- `FacetFilter` に `duration_preset: Option<FacetDurationPreset>` を足す。プリセットは
  〜1分 `[0, 60)` / 1〜10分 `[60, 600)` / 10〜60分 `[600, 3600)` / 60分以上 `[3600, ∞)` 秒、範囲指定は
  分単位の min / max (サイズ facet の `Range` と同型)。`settings.facet_filter` と同じく保存する。
  旧版は未知の field として無視する (§2.3)。
- 判定: 動画・音声でメモリの長さが Known かつ範囲内だけを通す。Unavailable・未取得・動画音声以外
  (フォルダ・書庫・画像・スタック) は落とす (サイズ facet がサイズの無い行を落とすのと同じ)。
- 取得: 長さメニューを開いたとき (`duration_facet_requested`)、または長さ条件が有効なとき、§5.1 と同じ
  一覧全体の取得を始める。`duration_facet_requested` の寿命は `ai_model_facet_requested` と同じ
  (App 全体の bool、一覧の install で `duration_preset.is_some()` へ戻す)。
- **読み込みが全部終わるまで長さ条件だけを適用しない**。他の facet は通常どおり判定する (AI facet の早期
  `return true` は写さない)。「全部終わった」は表示集合全体の取得 session が Ready になったことで、
  `Reconciling` や `VisibleStage` の Ready は含めない (AI の `ai_model_facet_ready()` の条件はそのまま使わない)。
- メニュー: 読み込み中は「長さを読み込み中 n/m」と停止 / 再開 (AI facet と同じ)。Ready 後は各候補の件数。
  Ready で `rebuild_visible_indices`。
- すべての一覧 (Ctrl+G・コレクション等を含む) で使える。集約一覧で動画が多いと取得に時間がかかるが、
  取消でき、その間は長さ条件だけが素通しになる。
- Remote には絞り込みバーが無いので変更しない。

## 7. 変更しないもの・互換

- DB の schema は変えない (`video_meta` を読むだけ。読み取り API を 1 つ足す)。
- 永続データで変わるのは settings の `SortOrder` の新しい値 (`settings.sort_order`・`FavoriteViewState.sort_order`・
  `toolbar_sort_items`)、`FacetFilter` の新 field、toolbar 補完の marker。
  - 新しい `SortOrder` の値を v5.4.0 以前が読むと、その版は `Incompatible` として既定設定 + 保存抑止で起動する
    (DB は書き換えない。v5.5.0 に戻せば元の設定で起動する)。サイズ順・評価順を足したときと同じ境界。
    toolbar の補完によって、長さ順を使っていない利用者もこの境界に入る (§9 の 6)。
  - `FacetFilter.duration_preset` は旧版が無視する。
  - 新旧の wire (serde 文字列) と、旧版相当の読み込みで DB を書き換えないことを試験する。
- detached viewer: 新しい context field は足さない。自動の並べ直しをやめたので、detached 述語・viewport 経路には
  触れない見込み。`duration_facet_requested` は AI の bool と同じ App 全体の扱い。実装で detached の経路に
  触れたら CLAUDE.md の合意手続に従い [detached-rework-plan.md](detached-rework-plan.md) §11 に記録する。
- Remote IPC: `SortOrder` の stable string が 2 つ増えるだけで wire の構造は変えない (値は String)。
- サムネイルの長さバッジ、詳細表示の長さ列、再生位置メーターは変えない。

## 8. 後続候補・既存の問題

- 集約一覧 (Ctrl+G・サブ展開・スマート・レーティング・ブックマーク) の長さ順。既存 hit の path / stamp から
  親ごとの catalog を worker で読む共通 helper を作れば、自動の並べ直しが無いので比較的単純に足せる
  (親 DB の数だけ open の費用がかかる)。
- Ctrl+G の検索構文の長さ条件 (索引 schema の変更と再索引)。
- Remote から長さの取得を始めること。
- **既存の問題**: `passes_facet_filter` は AI モデル / 生成ツール条件が有効で未 Ready の間、タグ・日付・サイズ・
  状態などの他の facet まで判定せず全件を通す (`src/app.rs` 76380 行付近)。v4 系から既存の見込み。
  本計画では直さず、統括へ報告する。

## 9. 利用者に判断をもらう項目 (統括経由で質問中、2026-11-19)

1. **長さ未取得の並べ替え**: 第2版の推奨は B (未取得は末尾、裏で読み、並べ直しは更新・再選択のとき)。
   第1版で推奨した A (読み終えたら自動で一度並べ直す) は、独立レビューで再読込の繰り返し・複数選択の消失などが
   見つかったため取り下げる。C (全部読んでから表示) は前提に反する。
2. **読み込み中を見せるか**: (推奨) 長さ順の項目に「長さを読み込み中 n/m」、絞り込みの長さメニューに進捗と停止。
3. **長さ順を使える一覧**: (推奨) v5.5.0 は物理フォルダだけ。他は理由を出し、詳細表示の長さ列で代わりに並べ替える。
4. **絞り込みの区切り**: (推奨) 〜1分 / 1〜10分 / 10〜60分 / 60分以上 + 範囲指定 (分単位)。
5. **Ctrl+G の検索構文の長さ条件**: (推奨) v5.5.0 では入れない。
6. (追加) **ツールバーの既定に長さ順を補完するか**: 補完すると、v5.5.0 へ上げた後に v5.4.0 へ戻したとき、
   長さ順を使っていなくても設定が既定で起動する (DB は残る)。補完しなければ、長さ順はメインメニューと
   ツールバーの設定から足して使う。(推奨) 既存の追加時と同じく補完する。

## 10. 検証計画 (実装時)

- 比較: Known の昇順・降順、Unavailable・Missing が両方向で末尾、名前の tie、ミリ秒丸め、4 行割当、混在行。
- catalog: 一括読み取り API、read-only open、stamp 一致 / 不一致、catalog 無し・壊れているとき Missing、
  scan 失敗項目を照合しない。
- materialize: facts → `ListingSortMetadata.duration`、facts `None` の fail-fast (test)、Remote が本体と同じ順。
- 入口: `ScannedDir` を作る全入口で長さ順のとき facts が付く (handler-level test、列挙台帳)。
- スタック: 集約 / flat / group 比較で動画の長さが保たれる、スタックセルは Missing。
- 取得要件: 長さ順・対応一覧でだけ `AllRequirements` に入る、最後の Item と Finished の間で要件が消えない、
  サムネイル stage で置き換えられない、`RetryLater` を同じ世代で再試行しない、列・バッジ全 OFF でも Ready に届く、
  対応外の一覧・長さ順以外では入らない、park / mount で兄弟 context の状態が変わらない。
- 並べ替えの操作: `listing_sort_capabilities` の一覧種別ごとの結果、メニュー / ツールバーの disabled と理由、
  ゲームパッドが飛ばす、Remote の候補、長さ順の再選択で再読込、評価順の既存の理由が変わらない。
- facet: プリセット境界、範囲指定、全部 Ready まで長さ条件だけ素通し・他の facet は判定、Ready 後の件数と除外、
  保存の往復、旧 JSON 欠落、`duration_facet_requested` のリセット。
- 設定: 12 値の roundtrip、toolbar 補完 marker、コレクション・代表サムネイル候補に出ないこと、
  旧版相当の enum で読んだときの `Incompatible` と DB 非変更。
- UI スナップショット: 絞り込みバーに長さが増える分。
- 文書: `docs/spec.md`、本書、[details-view-and-filter-plan.md](details-view-and-filter-plan.md)、
  [catalog-design.md](catalog-design.md)、[ui-responsiveness.md](ui-responsiveness.md)、
  [filename-stack-plan.md](filename-stack-plan.md)、[list-size-sort-plan.md](list-size-sort-plan.md) (比較契約の追記)、
  `htdocs/mimageviewer/manual/grid.html` ほか並べ替え・絞り込みを説明しているページ、製品ページの機能一覧。
