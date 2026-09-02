# レーティング一覧ビュー 設計計画 (rating-list-view)

**状態: Phase 1 実装済み (2026-06-23 起案 / 同日実装、2026-07-05 戻る導線を追加)**

ファイルメニューと「場所▼」に ★1〜★5 の項目を足し、選んだ★レーティングの付いた
アイテム/コンテナを、場所をまたいでフラットに一覧表示する仮想ビュー。通常ソートに加えて
「レーティングを設定した時刻」でもソートできるようにする。

関連: [architecture-overview.md](architecture-overview.md) (永続化ストア §4 / フラットビュー) /
[reading-history-plan.md](reading-history-plan.md) (一番近い雛形) /
[details-view-and-filter-plan.md](details-view-and-filter-plan.md) (ビュー固有ソート/ファセットの前例) /
[ui-responsiveness.md](ui-responsiveness.md) (worker 化 §4)。

---

## 0. 確定した設計判断 (2026-06-23、レビュー反映)

| 論点 | 決定 |
| --- | --- |
| `rating.db` スキーマ拡張 | **richer**: `rated_at_ms` + `source_path` + `kind` + 仮想アイテム復元用メタデータを追加。`kind/source_path` だけでは ZIP/PDF/ZipDir を十分に復元できない |
| 時刻ソートの UI 露出 | **案A: ビュー表示中だけソートに「★設定時刻 ↓/↑」を追加**。グローバル `SortOrder` には足さない |
| メニュー配置 | **場所▼ と ファイル の両方**に `レーティング ▸ ★1..★5` サブメニュー |
| XMP 由来 rating | XMP hydration は `rated_at_ms = NULL`。ユーザーが明示的に★を設定/変更した時だけ現在時刻を入れる |
| 計画書 | 本ファイル (`docs/rating-list-view-plan.md`) を正本にする |

---

## 0.1 実装メモ (2026-06-23)

Phase 1 として、場所横断の ★1〜★5 レーティング一覧ビューを実装済み。

- `rating.db` は後方互換マイグレーションで `rated_at_ms` / `source_path` /
  `kind` / ZIP・PDF 復元メタ列を追加し、既存行は旧キーから復元する。
- ユーザー操作の書き込みは `set_user_rating`、XMP 取り込みは `set_imported_rating`
  に分けた。XMP 由来は `rated_at_ms = NULL` のままなので、フォルダを開いただけで
  「最近設定した★」扱いにならない。
- ファイルメニューと場所▼に `レーティング一覧` / `レーティング` サブメニューを追加し、
  ★1〜★5 の件数を `rating.db` から表示する。
- 一覧構築は worker で行い、`RatingViewRow` をメモリに保持して通常ソートと
  `★設定時刻 ↓/↑` の切り替えを stat し直さずに行う。
- レーティング一覧内で ★ を変更した場合は、選択中の `RatingViewSort` を保持したまま、
  変更行だけをメモリ上で削除 / `rated_at_ms` 更新して再配置する。`enter_rating_view`
  を再呼び出しして全件 stat し直す経路にはしない。
- ★1〜★5 のメニュー件数は `rating_counts_cache` に保持し、ユーザー操作 / XMP 取り込み /
  Undo / 製本ページ copy/move など rating DB を変える経路で無効化する。描画フレームごとの
  `count_by_stars()` 実行は避ける。
- `RatingViewSort` は view-local のままにし、通常フォルダの `SortOrder` へ
  `rated_at` を混ぜない。時刻ソート項目はレーティング一覧表示中だけソート UI に出す。
- レーティング一覧は ★N が固定条件の仮想ビューとして扱い、ツールバーの★フィルタと
  facet の★条件は無効化する。
- `search_results_synthetic_path` / `reading_history_synthetic_path` と同じ分類で扱う
  汎用ガードは `is_synthetic_view_path` に集約した。検索固有の復元・favsearch 経路には
  レーティングビューを混ぜない。
- レーティング一覧からフォルダ / ZIP / PDF / 変換アーカイブ / ZipDir を開くと、
  `rating_view_nav_stack` に開いたコンテナを記録する。Backspace / フォルダバーの親ボタン /
  fullscreen の return-to-parent 予約は `AddressBarNav::RatingViewBack` を経由し、コンテナ内では
  まず ZIP 階層を 1 段戻り、スタックが空になった時点で保持済み `rating_view_rows` から
  レーティング一覧を再インストールする。再検索・再 stat は行わない。

Phase 2 の詳細表示専用 `★設定時刻` 列は実装済み。レーティング一覧の行メタをそのまま
表示・列ソートに使い、通常フォルダへは列と sort key を露出しない (§5.2)。

存在しない行の明示的な整理 UI は v2.3.0 第16弾で実装済み。サムネイルキャッシュ管理の
「メタデータを整理…」から全 path-keyed ストアをバックグラウンドでスキャンし、ストア別件数を
確認してから削除する。実体が無くても直上の親フォルダへ到達できない行は、外付けドライブ / NAS
オフライン保護として残す。整理後は `rating_counts_cache` を無効化し、表示中のレーティング一覧も
再構築する。

---

## 1. 背景と既存資産

### 1.1 レーティングの現状

- 永続化: `rating_db.rs` の `rating.db`。現状スキーマは `ratings(path TEXT PRIMARY KEY,
  stars INTEGER NOT NULL)` のみで、**設定時刻はどこにも残っていない** (XMP `xmp:Rating`
  にも時刻なし、ログは永続でない)。→ 時刻ソートには列追加が前提。
- キー生成: `App::rating_path_key(idx)` ([app.rs](../src/app.rs))。
  - ページ (Image / ZipImage / PdfPage) → `page_path_key` (ZIP/PDF は `container::entry`
    形式、plain Image は `::` なしの正規化パス)。
  - 単一ファイル leaf の Video と、コンテナ (Folder / ZipFile / PdfFile /
    ConvertibleArchive) → `adjustment_db::normalize_path` (= 小文字化 + `\`→`/`、
    `::` なし)。Video はキー形式だけコンテナ系と同じで、分類は
    `GridItem::is_rating_leaf()` / `is_container_ratable()` で行う。
  - ネスト ZIP の本 (ZipDir) → `book_container_key(root, dir_prefix)` を正規化 (`zip::sub`
    形式、`::` ありだが右が画像拡張子でない)。
- 読み書き: `set_rating` / `get_rating` ([app.rs](../src/app.rs))。`accepts_rating()` /
  `is_rating_leaf()` で対象判定。`rating_cache: HashMap<usize, u8>` にプリウォーム
  (`prewarm_rating_cache` → `RatingDb::get_many`)。
- 既存フィルタ: `settings.rating_filter: [bool; 6]` + `passes_rating_filter`
  ([app/metadata_ops.rs](../src/app/metadata_ops.rs)) は **現在のビュー内**を★で絞るだけ。
  ライブラリ横断の★一覧ビューは存在しない (= 本機能が新規)。

### 1.2 雛形になる仮想フラットビュー

| ビュー | 状態フラグ | 構築 | 流用ポイント |
| --- | --- | --- | --- |
| 閲覧履歴 | `items_are_reading_history_view` | `enter_reading_history` → `install_reading_history_entries` | DB→GridItem→`start_loading_items`→`address` 差し替え、synthetic path、full-path cache key の雛形 |
| Ctrl+G 検索 | `items_are_global_search_view` | `replace_search_view_items` ([global_search_ui.rs](../src/global_search_ui.rs)) | サムネ Loaded 保持つき items 差し替え (再ソート時に流用) |
| タグビュー | `items_are_tag_view` | tag_view 内 | worker/cancel、saved_folder/nav_stack、コンテナ/通常ファイル混在一覧の雛形 |

`GridItem` は ★対象集合をそのまま表現でき、**新バリアントは不要** (Ctrl+G の
`SearchContainer` のような専用型を作らない)。

---

## 2. rating.db スキーマ拡張 (richer)

`rating.db` は**リリース済み**なので後方互換マイグレーション
([CLAUDE.md「永続データ・スキーマ変更時の判断」])。列存在チェック付きで ADD COLUMN:

```sql
-- open() のテーブル作成後に、各列が無ければ追加する (PRAGMA table_info で確認)。
ALTER TABLE ratings ADD COLUMN rated_at_ms          INTEGER; -- epoch ms。既存行 / XMP 由来は NULL
ALTER TABLE ratings ADD COLUMN source_path          TEXT;    -- 非正規化の実パスまたはコンテナ root
ALTER TABLE ratings ADD COLUMN kind                 INTEGER; -- アイテム種別タグ (下表)
ALTER TABLE ratings ADD COLUMN entry_name           TEXT;    -- ZipImage の元 entry 名 (大小保持)
ALTER TABLE ratings ADD COLUMN page_num             INTEGER; -- PdfPage の 0-indexed page
ALTER TABLE ratings ADD COLUMN dir_prefix           TEXT;    -- ZipDir の prefix
ALTER TABLE ratings ADD COLUMN archive_format       TEXT;    -- ConvertibleArchive 用 (rar/7z/lzh...)
ALTER TABLE ratings ADD COLUMN zipdir_is_archive    INTEGER; -- ZipDir バッジ用 bool
ALTER TABLE ratings ADD COLUMN zipdir_representative TEXT;   -- ZipDir 代表 entry (任意)

CREATE INDEX IF NOT EXISTS idx_ratings_stars_rated_at
    ON ratings(stars, rated_at_ms DESC);
```

`source_path` は `rating_source_path(idx)` 相当の復元用 path を、元の大小を保って保存する。
変換アーカイブ閲覧中に元アーカイブ path を使うか、キャッシュ ZIP path を使うかは
既存 `rating_path_key` の key 規約と必ず揃える。ZIP/PDF/ZipDir は `source_path` だけでは
復元できないため、**新規書き込み行では kind 別の補助列を必ず埋める**。
通常ファイル/通常コンテナでは補助列は NULL でよい。

`kind` 値 (例、`u8`):

| kind | GridItem |
| --- | --- |
| 0 | Image |
| 1 | Video |
| 2 | Folder |
| 3 | ZipFile |
| 4 | PdfFile |
| 5 | ConvertibleArchive |
| 6 | ZipImage |
| 7 | PdfPage |
| 8 | ZipDir (ネスト本) |

### 2.1 書き込み側の変更

- `RatingDb::set` は互換 API として残すより、内部実装を共有した上で次のように分ける方が
  意図が明確:
  - `set_user_rating(key, stars, meta)` — ユーザー操作。`stars>0` なら
    `rated_at_ms = SystemTime::now()`。
  - `set_imported_rating(key, stars, meta)` — XMP hydration 等の取り込み。`rated_at_ms = NULL`。
  - `stars==0` は従来どおり行削除。
- `App::set_rating` / `set_current_folder_rating_internal` で書き込み時に `RatingMeta` を渡す:
  - `source_path` = `rating_source_path(idx)` の PathBuf 文字列。
  - `kind` = item から決定。
  - `ZipImage` は `entry_name` を元の大小で保存。
  - `PdfPage` は `page_num` を保存。
  - `ZipDir` は `dir_prefix` / `zipdir_is_archive` / `zipdir_representative` を保存。
  - `ConvertibleArchive` は `archive_format` を保存。
- `hydrate_ratings_from_xmp` は `set_user_rating` を使わない。ここで現在時刻を入れると、
  「フォルダを開いて XMP を読んだだけ」の行が最近設定した★として並んでしまう。
- `copy_entry_key` / `move_entry_key` は新列も一緒に運ぶ (SELECT/INSERT のカラムを拡張)。
  `rated_at_ms` は「コピー/移動でも元の設定時刻を保つ」方針 (= ファイル整理で★の時系列が
  壊れない)。要再確認だが既定は「保持」。

### 2.2 読み出し側の追加 API

```rust
pub struct RatingRow {
    pub key: String,
    pub stars: u8,
    pub rated_at_ms: Option<i64>,     // 旧行 / XMP 由来は None
    pub source_path: Option<String>,  // 旧行は None
    pub kind: Option<u8>,             // 旧行は None
    pub entry_name: Option<String>,
    pub page_num: Option<u32>,
    pub dir_prefix: Option<String>,
    pub archive_format: Option<String>,
    pub zipdir_is_archive: Option<bool>,
    pub zipdir_representative: Option<String>,
}
impl RatingDb {
    pub fn list_by_stars(&self, stars: u8) -> Vec<RatingRow>;     // ビュー構築用
    pub fn count_by_stars(&self) -> [usize; 6];                   // メニューの件数バッジ用
}
```

### 2.3 削除ポリシー (全メタストア hard purge、v2.3.0)

- mIV の削除 worker が Shell 削除成功を返した path だけ、rating を含む path-keyed
  全メタストアから素の `DELETE` を行う。照合は exact + `<key>/` + `<key>::` で、
  フォルダ / ZIP / PDF 等の配下キーも対象にする。
- 対象表は `rename_key_migration::STORES` を rename と purge で共有する。rating だけの
  tombstone は設けず、未出荷だった `deleted_at_ms` 列追加・alive filter・prewarm unflag は撤去した。
- ごみ箱から同じ path へ戻しても★は戻らない。タグ・補正・回転等も同じ hard purge 仕様。
- Explorer 等による外部削除、一覧構築時の stat 失敗、切断ドライブでは DB 行を変更しない。
  missing は表示結果から除外するだけで、到達不能を削除と誤認しない。

---

## 3. フラット一覧ビュー本体

### 3.1 App 状態 (新規フィールド)

```rust
items_are_rating_view: bool,         // 他の items_are_* と同じ排他フラグ
rating_view_stars: u8,               // 1..=5 (このビューが表示中の★)
rating_view_sort: RatingViewSort,    // ビュー内の現在ソート。既定は RatedAtDesc
rating_view_rows: Vec<RatingViewRow>,// 再ソート用の在メモリ行 (再 stat しない)
rating_view_pending: Option<RatingViewPending>, // worker ハンドル {cancel, rx}
rating_view_skipped: usize,          // 実在しない等で除外した件数 (UI 表示用、任意)
rating_view_saved_folder: Option<PathBuf>,
rating_view_nav_stack: Vec<PathBuf>, // タグビューと同型。結果から開いた実コンテナから戻る導線
```

`RatingViewSort` はグローバル設定に保存しない view-local enum:

```rust
enum RatingViewSort {
    Normal(SortOrder),
    RatedAtDesc,
    RatedAtAsc,
}
```

`RatingViewRow` は GridItem に加え `rated_at_ms: Option<i64>` / `mtime`/`size`/`name_key` を
保持し、ソート切替時の再構築を stat なしで行う。

### 3.2 構築フロー (必ず worker 化: ui-responsiveness §4)

```
enter_rating_view(stars):
  - 他ビュー/検索を閉じる (close_other_*)
  - start_rating_view_build(stars) で worker spawn:
      worker:
        1. RatingDb::list_by_stars(stars)
        2. 各 RatingRow を GridItem へ復元 (§4) + 実在チェック stat
             - 実在しないコンテナ/ファイルは除外 (skipped++)
             - stat から (mtime, size) を取得 → image_metas
        3. RatingViewRow を rated_at_ms つきで生成
        4. mpsc で UI へ返す (cancel token で旧 build を中断)
  - poll_rating_view_build:
        rows 受信 → ソート適用 (§5) → items/image_metas に展開
        → start_loading_items(items, image_metas)
        → items_are_rating_view = true; rating_view_stars = stars
        → rating_view_sort = RatingViewSort::RatedAtDesc (既定: ★設定時刻 新しい順)
        → address = format!("{} レーティング一覧", "★".repeat(stars))
```

stat は数千件規模になり得る (rating.db の ★N 行数ぶん)。**UI スレッドで全件 stat 禁止**。
`RatingViewPending { cancel: Arc<AtomicBool>, rx }` + `start_/poll_` の定型を使う。

合成 path は `rating_view_synthetic_path(stars)` か単一の `__rating_view__` を data_dir 配下に
置く。`start_loading_items` 内の `delete_missing` / `last_folder` 保存 / sidecar import /
`use_full_path_cache_keys()` など、既に `search_results_synthetic_path()` と
`reading_history_synthetic_path()` を特別扱いしている箇所へ同じ分類で追加する。

### 3.3 クリック / 戻る導線

コンテナ (Folder/Zip/Pdf/Archive/ZipDir) を開く直前に `record_rating_view_nav_open` を呼び、
タグビューの `nav_stack` と同型の `rating_view_nav_stack` に戻り先を積む。変換アーカイブで
変換ダイアログが開いた場合は、キャンセル / ブロック時に `FolderNavHistorySnapshot` から
このスタックと `pending_rating_view_zipdir_open` も復元する。

- Backspace / フォルダバーの親ボタン / fullscreen の return-to-parent 予約は
  `AddressBarNav::RatingViewBack` に解決する。
- ZIP ツリーナビ内では `zip_nav_back()` を先に消化し、ZIP ルートに戻った後で
  `rating_view_nav_stack` を pop する。
- スタックに親コンテナが残っていれば `load_folder_or_convert_archive(top)` で 1 段戻る。
  空になったら `install_rating_view_rows()` で保持済み行から一覧を再表示する。
- ページ (Image/Video/ZipImage/PdfPage) を開く → フルスクリーン。閉じたら通常のグリッド復帰。
- 抜ける → レーティング一覧最上位で Backspace、または Esc / 実フォルダへナビゲートで通常表示
  (他ビューと同じ復帰機構)。
- フォルダバーの履歴 ←/→ には `rating_view_synthetic_path()` も保持する。この path は
  `start_loading_items` から永続 catalog のキーにも渡るため星数を埋め込まず、back/forward
  stack と位置同期する `Option<u8>` の星メタデータを別 stack に保持する。これにより
  ★3 → ★5 のような同一 synthetic path 間の遷移も別 entry として扱える。
- 履歴から rating synthetic path を pop したときは星メタデータを `rating_view_stars` へ
  復元し、同じ星の `rating_view_rows` が残っていれば `install_rating_view_rows()` で
  再インストールする。別の星へ戻る場合は星別の正しい行を worker で再構築し、実フォルダとして
  `__rating_view__` をロードしない。星メタデータも保持 state も無い場合は no-op にする。
- 場所 / ファイルメニューから開くときは、非同期 build を始める前に直前の実フォルダを
  back stack へ記録する。build 完了後に current_folder が synthetic path へ切り替わっても、
  メニュー起動直後の戻り先を失わない。

### 3.4 サムネイル

`start_loading_items` 後、各 GridItem のパスで既存サムネパイプライン (ZIP/PDF ワーカー
含む) が走る。**新規サムネ配線は不要**。

---

## 4. キー → GridItem 復元

新規行 (`kind`/`source_path` と kind 別メタあり) は **DB から直接復元**
(推定不要・表示名は正しい大小)。
旧行 (NULL) は以下のヒューリスティクスにフォールバック:

| キー形式 | 判定 | 復元先 |
| --- | --- | --- |
| `::` なし・stat=dir | — | Folder |
| `::` なし・file・画像拡張子 | — | Image |
| `::` なし・file・動画拡張子 | — | Video |
| `::` なし・file・`.zip` | — | ZipFile |
| `::` なし・file・`.pdf` | — | PdfFile |
| `::` なし・file・`.rar/.7z/.lzh` 等 | — | ConvertibleArchive { format } |
| `::` あり・左 `.pdf`・右 `page_\d+` | page 解析 | PdfPage |
| `::` あり・右が画像拡張子 | — | ZipImage |

**注意 / 既知の限界:**

- plain Image と Folder は文字列だけでは区別不能 → stat の dir/file で解決
  (同一パスが dir かつ file はあり得ないので衝突しない)。
- 旧行のキーは小文字化済み → 通常パスは `canonicalize` 等で実 casing 復元を試す。
  ZIP entry は元の大小をキーだけからは戻せないため、旧行では ZIP を列挙して
  case-insensitive match する。曖昧/失敗時はスキップし、新規行は `entry_name` で解消する。
- ネスト ZIP の本 (ZipDir、`zip::sub` 形式・右が非画像) の旧行は単独復元が難しい。
  MVP は「親 ZIP として開く or その行はスキップ」とし限界として明記。新規行は kind=8 で
  `dir_prefix` 等から復元する。`representative` は保存値を使い、無ければ worker 側で軽く
  再選定するかアイコン表示へフォールバックする。
- パスワード付き PDF の `PdfPage` は、サムネ/フルスクリーンで既存の password cache へ
  合流できるか実装時に確認する。できない場合は、その行だけ開く時に PDF password flow へ
  送るか、MVP では PDF 本体へ誘導する。
- 消えたファイル / 切断ドライブ → stat 失敗で除外。除外件数を表示 (掃除導線は将来)。
  stat 失敗だけでは rating 行を削除・flag しない。外付けドライブ未接続を「削除済み」と
  誤判定しないため。mIV 内削除成功だけは §2.3 の hard purge を適用する。

---

## 5. ソート (案A: ビュー固有)

### 5.1 方針

- グローバル `SortOrder` (FileName/Numeric/DateAsc/DateDesc, [settings.rs](../src/settings.rs))
  は流用。名前/日付/番号は stat 済みメタ (mtime/size/name_key) で動く。
- レーティングビュー専用に `RatingViewSort` を持つ。
  - `RatedAtDesc` / `RatedAtAsc` → `rated_at_ms` で並べる。
    **NULL (旧行 / XMP 由来) は末尾固定** (details の「None は末尾」と同規約)。
  - `Normal(order)` → `SortOrder` に従う。ただし `settings.sort_order` 自体は必要に応じて
    更新する既存操作を残してよい。
- **既定**: ビューを開いた直後は `RatedAtDesc` (★設定時刻 新しい順 = 最近付けた順)。

### 5.2 UI 露出

- **サムネイル表示**: `items_are_rating_view` のときだけ、ツールバーのソートセクション
  (v2.0.0 でデータ駆動化済み) 末尾に「★設定時刻↓ / ★設定時刻↑」を append。
  これは表示中だけの追加候補で、`settings.toolbar_sort_items` の永続候補には混ぜない。
  通常の `SortOrder` を選ぶと `RatingViewSort::Normal(order)` に戻す。
- 設定メニュー側の「ソート順」サブメニューにも、レーティングビュー表示中だけ同じ
  「★設定時刻↓ / ★設定時刻↑」を出す。ツールバーと設定メニューで選べる候補を揃える。
- **詳細表示 Phase 2 (実装済み)**: レーティング一覧専用の
  `DetailsColumnId::RatedAt` / `DetailsSortKey::RatedAt` (表示名「★設定時刻」) を追加する。
  値は items と 1:1・同順序の `rating_view_rows[idx].rated_at_ms` から読み、遅延列にはしない。
  `NULL` は空欄で表示し、昇順・降順のどちらでも時刻ありの項目より後ろへ置く。
  選択情報にも時刻がある場合だけ「★設定時刻」を表示する。
- ★設定時刻は更新日時とは別の列にする。レーティング一覧でも
  `RatingViewSort::Normal(SortOrder::DateAsc/DateDesc)` はファイル本来の更新日時を使うため、bookmark / history
  view のように `image_metas` の mtime slot を別時刻へ差し替えることはできない。
- レーティングビュー以外では列も sort key も出さず、ビューを抜ける時に選択中なら
  `Toolbar` へ戻す。グローバル詳細設定に「通常フォルダでは空の列」を残さない。
- v3.4.0 との settings.db downgrade 互換を守るため、`RatedAt` の enum variant は永続 JSON に
  書かない。保存時は sort key を `Toolbar` に置き換え、列順と列幅 map から `RatedAt` を除外する。
  列幅と表示 ON/OFF は旧版が無視できる専用フィールドへ保存する。
- グローバル `SortOrder` enum・`settings.sort_order` は変更しない (案B の難点回避:
  全フォルダのツールバーに重複項目が出ない / 永続設定の意味が場所で変わらない /
  通常フォルダで `rated_at_ms` をロードしない)。

### 5.3 再ソートを安く

ソート切替時は `rating_view_rows` から再構築するだけ (再 stat しない)。サムネは Ctrl+G の
`replace_search_view_items` と同様に Loaded を保持して並べ替え、テクスチャ再アップロードを
避ける。

### 5.4 ★フィルタ / facet との関係

`★3 レーティング一覧` の中で通常の `settings.rating_filter` がさらに効くと、
「★3 を選んだのに空になる」など直感に反する。レーティングビューでは閲覧履歴ビューと同じく
通常の★フィルタは無効化または固定扱いにする。スマートフィルタは種類/場所/タグ/日付/サイズなど
意味があるものは残してよい。場所 facet は元ファイル/元コンテナの親フォルダで絞り込むため、
★一覧の中から特定の元フォルダだけを残す用途に使える。ただし場所 facet は現在の表示スコープに
束縛された一時条件で、フォルダや別の仮想ビューへ移動したら解除する。rating facet だけは対象★で固定、
または UI disabled にする。

---

## 6. メニュー

- **場所▼** ([ui_main.rs](../src/ui_main.rs) のドライブ一覧/閲覧履歴/本棚の並び) に
  `レーティング ▸ ★1 / ★2 / ★3 / ★4 / ★5` サブメニュー。各項目に `count_by_stars` の
  件数を併記 (任意)。
- **ファイル** メニュー ([ui_main.rs](../src/ui_main.rs) の「閲覧履歴を開く」隣) にも
  同じサブメニューをミラー。
- 5 項目をトップに並べず 1 サブメニューに畳む。
- キー操作は追加しない (menu 駆動)。将来ショートカット化する場合のみ `KeyAction` 追加。

---

## 7. 段階実装

- **Phase 1 (MVP)**: `rated_at_ms` + `source_path` + kind 別メタ列・マイグレーション →
  `enter_rating_view`
  + `items_are_rating_view` (worker 構築・§4 復元) → 場所▼/ファイル サブメニュー →
  既存 SortOrder で並ぶ + サムネ/詳細の「★設定時刻↓/↑」追加 (既定: 新しい順)。
- **Phase 2**: 詳細表示の `RatedAt` 列は実装済み。残りは件数バッジ、除外件数表示、
  PDF password edge の改善。
- **Phase 3 (任意)**: 「★N 以上」等の複合フィルタ、葉/コンテナ絞り込み、ショートカット、
  stale 行の掃除導線。

---

## 8. 触るファイル

| ファイル | 変更 |
| --- | --- |
| `src/rating_db.rs` | `rated_at_ms`/`source_path`/`kind`/kind 別メタ列・マイグレーション (table_info チェック付き ADD COLUMN)・user/imported 書き込み API・`copy/move` 拡張・`list_by_stars`/`count_by_stars`・`RatingRow` |
| `src/rating_view.rs` (新規候補) | `RatingViewSort`・worker・DB row → `RatingViewRow` 復元・legacy row ヒューリスティクス・ソート純関数 |
| `src/app.rs` | `items_are_rating_view`・`rating_view_*` 状態・`enter_rating_view`/`install`・worker (`start_/poll_`)・戻り導線・`set_rating`/`set_current_folder_rating`/XMP hydration で新 API 使用・synthetic path ガード追加 |
| `src/ui_main.rs` | 場所▼/ファイル メニュー項目・ツールバーソート section の view 限定 append |
| `src/settings.rs` | Phase 1 では原則変更不要。`DetailsSortKey::RatedAt` は Phase 2 で追加する場合のみ |
| ドキュメント | 本ファイル更新・`docs/spec.md`・`htdocs/mimageviewer/manual/`・製品ページ ([CLAUDE.md「ドキュメント同時更新」]) |

---

## 9. テスト方針

- `rating_db.rs`: マイグレーション (旧スキーマ DB を開いて列追加されるか)・`set` で
  `rated_at_ms`/kind/`source_path`/kind 別メタが入るか・XMP imported 書き込みでは
  `rated_at_ms` が NULL になるか・`list_by_stars`/`count_by_stars`・copy/move の列伝搬。
  in-memory SQLite で従来同様。
- 復元ヘルパ: キー文字列 → GridItem の純関数を unit test (各 kind / `::` 有無 / page 解析 /
  旧行ヒューリスティクス)。
- ソート: `rating_view_rows` → 並び順 (★設定時刻 ↑↓、NULL 末尾、SortOrder 準拠) を純関数で。
- 必要なら egui_kittest スナップショット (場所▼ サブメニュー / ビュー中のソート追加項目)。

---

## 10. 要再確認 (実装着手時)

- copy/move 時の `rated_at_ms` は「保持」既定で良いか (★の時系列をファイル整理で壊さない)。
- 既定ソートは「★設定時刻 新しい順」で良いか。
- 葉とコンテナを 1 リストに混在で良いか (将来サブ絞り込みは Phase 3)。
