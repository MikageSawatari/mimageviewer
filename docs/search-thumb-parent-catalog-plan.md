# 検索結果などで作ったサムネイルを親フォルダのキャッシュに置く (§1.741)

作成: 2026-11-08。状態: **設計案 (独立設計レビュー前)**。対象版: v5.1.0 候補 (利用者判断)。
バックログ: [next-release-backlog.md §1.741](next-release-backlog.md)。報告: 5ch >>746。

## 1. 問題

- Ctrl+R (Everything 検索) で複数フォルダの ZIP のサムネイルを作っても、親フォルダの代表サムネイルに
  使われない。検索を閉じて元のフォルダへ戻ると、作った行は消える (サブ PC 担当の実機確認・キャッシュの写し、
  backlog §1.741)。
- 原因 (コード):
  1. 検索の一覧は、**開いていたフォルダのサムネイル worker pool をそのまま使う**
     (`SearchMetadataResources::for_app` は `reload_queue` があると `thumbnail_init` を作らず、
     `install_missing_search_thumbnail_workers` も pool があれば何もしない)。pool の保存先
     (`ThumbnailCatalogProvider::FixedCatalog`) は開いていたフォルダの catalog なので、検索結果の行は
     **そのフォルダの DB** にフルパスのキー (`zipthumb:C:\…\F01\vol01.zip`) で入る。
     Everything の準備 worker も、フォルダの動画ピンの seed を同じ catalog へ書く
     (`everything/metadata.rs` の `commit_pin_materializations`)。
  2. そのフォルダへ戻ると、`delete_missing` が一覧に無いキーとして消す。
  3. フォルダの代表選定 (`thumb_loader.rs` の `resolve_folder_thumb_image_inner` → `cached_candidate`) は
     ZIP・PDF を開かず、**子フォルダ自身の catalog** を通常キー (`archive_tile_key`: basename、
     ドライブ/共有ルート直下だけフルパス) で引く。検索側の行は DB もキーも違うので、消えなくても届かない。
- pool が無いとき (起動直後にどのフォルダも開いていない等) は `__search_results__` の合成 DB に
  フルパスのキーで入る。これも代表選定からは届かない。

## 2. 目標と範囲

- 実在の親フォルダを持つ項目 (画像・ZIP・PDF/EPUB・RAR 等・フォルダ・音声) のサムネイルを、
  複数の親を並べる一覧で作ったときも、**その親フォルダを通常に開いたときと同じ DB・同じキー**に置く。
  親フォルダの代表選定・親フォルダを開いたときの一覧が、そのまま再利用する。逆向き
  (先にフォルダで作った行を検索結果が使う) も同じ仕組みで効く。
- 検索を始めた元のフォルダの DB に、無関係な行を書かない (動画ピンの seed を含む)。
- 対象外 (理由は §7): コレクションの root (§1.668 の別 DB)、動画のサムネイル (catalog を使わない別経路)、
  Remote。

## 3. 一覧の分類 (保存先とキーの所有者)

`App::use_full_path_cache_keys()` が真になる理由を、型付きの 1 つの判定にまとめる。

```text
enum ThumbStorageScope {
    // 1 つの実フォルダ (または ZIP/PDF の仮想フォルダ) の中身。pool の FixedCatalog に置く。
    SingleFolder { full_path_keys: bool },   // full_path_keys = ドライブ/共有ルート
    // 親の違う項目を並べる一覧。保存先は要求ごとに決める (§4)。
    MultiParent { view_db: ViewCatalog },
    // コレクションの root。§1.668 の別 DB (変更しない)。
    CollectionRoot,
}
```

- `use_full_path_cache_keys()` は `ThumbStorageScope` から導く
  (`SingleFolder { full_path_keys }` → その値、`MultiParent` / `CollectionRoot` → 真)。
  **メモリ上のキー (要求キー・`cache_map`・seed・release check) の綴りは今と同じ**。
  変わるのは「ディスク上のどの DB に、どのキーの行として置くか」だけ。
- 判定の材料は今と同じ: 合成パスの `current_folder` (`is_synthetic_view_path`)、
  `items_are_global_search_view` / `items_are_tag_view` / `items_are_reading_history_view` /
  `items_are_rating_view`、コレクション面の `CollectionGridPosition::Root`、ドライブ/共有ルート。
  **ドライブ/共有ルートは 1 つの実フォルダなので `SingleFolder`**。検索中に `current_folder` が
  元のフォルダ (ドライブルートを含む) のまま残っていても、検索・タグ等の flag が立っていれば
  `MultiParent` を優先する。

棚卸し (コードで確認した一覧と、今の保存先):

| 一覧 | 今の DB | 今の行キー | 変更後 (実在の親を持つ項目) | 変更後 (それ以外) |
| --- | --- | --- | --- | --- |
| 通常フォルダ・ZIP/PDF の中・ドライブ/共有ルート | そのフォルダ (Fixed) | 通常 (ルートはフル) | 変更なし | 変更なし |
| Ctrl+G / Ctrl+S / Ctrl+R / 固定検索 (`items_are_global_search_view`) | pool があれば**開いていたフォルダ**、無ければ `__search_results__` | フル | 親フォルダの DB・通常キー | `__search_results__`・フル |
| タグ一覧 (`items_are_tag_view`) | `__search_results__` (再検索は同じ pool) | フル | 同上 | `__search_results__`・フル |
| ★一覧 (`__rating_view__`) | 合成 DB | フル | 同上 | その合成 DB・フル |
| 閲覧履歴 (`__reading_history__`) / ブックマーク (`__bookmarks__`) | 合成 DB | フル | 同上 | 同上 |
| スマートフォルダ (合成パス) / サブフォルダ展開 (`__subfolder_expansion__`) | 合成 DB | フル | 同上 | 同上 |
| コレクション root | §1.668 の別 DB (`__collection_source_parent_v1__/<hash>`) | フル | **変更なし** (§7) | 変更なし |

「それ以外」= 実在の親を持たない項目: `ZipImage` / `PdfPage` / `ZipDir` / `SearchContainer`
(Ctrl+G の集約、`CACHE_KEY_SEARCH_REP`) / ドライブ一覧の行など。今の合成 DB とフルパスのキーのまま。

## 4. 要求ごとの保存先

### 4.1 要求を受け付けるとき (UI スレッド、I/O なし)

`attach_viewer_thumbnail_context` (今 Collection の親 scope を付けている所) で、`MultiParent` の一覧の要求に
保存先を付ける。

- 項目が実在の親を持つ (`GridItem` の実ファイル path があり、その `parent()` が合成パスでも書庫の中でもない):
  `RequestCatalog::Parent { identity: 親フォルダ, row_key, request_key, work }`。
  - `identity` は通常にそのフォルダを開くときと同じ (`CatalogDb::open(cache_dir, folder)` /
    `open_for_folder_selection` が使う `db_path_for(cache_dir, folder)`)。
  - `row_key` は、**その親フォルダを通常に開いたときの要求キー**。新しい綴りは作らず、今の
    `make_load_request_with_visibility` を `use_full_path_keys = use_full_path_cache_keys_for_folder(親)` で
    呼んだ結果のキー (ピン・ポリシーの suffix を含む) を使う。親がドライブ/共有ルートならフルパスになり、
    代表選定の `archive_tile_key` / `folder_thumb_cache_key_for_path` と一致する。
  - `request_key` はメモリ上のキー (今と同じフルパス)。worker は `req` のキーが `request_key` と
    一致するときだけ `row_key` へ読み替える (§4.3)。
- それ以外: `RequestCatalog::Named { identity: この一覧の合成パス }` (`ViewCatalog` が持つ。
  検索・タグは `search_results_synthetic_path()`、★は `rating_view_synthetic_path()` …)。行キーは要求キーのまま。
- `SingleFolder` は `RequestCatalog::Fixed` (今と同じ)。

**`MultiParent` の要求は pool の FixedCatalog を一切使わない**。これで「検索が借りた pool の DB」に
書く経路が構造的に無くなる。pool の借用自体 (検索で worker を作り直さない今の作り) は変えない。

### 4.2 provider (worker 側)

`ThumbnailCatalogProvider` を「pool の固定 catalog + 要求ごとの遅延 catalog の LRU」の組にする。

```text
struct ThumbnailCatalogProvider {
    fixed: Option<Arc<CatalogDb>>,          // SingleFolder 用。MultiParent の要求は見ない
    lazy: Arc<LazyCatalogs>,                // 今の CollectionParentCatalogs を一般化 (16 slot LRU)
}
```

- 今の `CollectionParentCatalogs` (RO/RW を分けた 16 slot LRU、OnceLock、lock の外で open/SQL、
  lookup は存在する DB だけ RO、保存を決めた後だけ RW、Auto で保存しないなら DB を作らない、
  `CatalogWork` の admission・clear との境界) をそのまま使い、slot の識別子を「identity の種類 + path」にする。
  Collection の別 DB・親フォルダの通常 DB・一覧の合成 DB を同じ LRU で開く。
- 「Fixed が常に優先」(§1.668 の PhysicalSource 対策) は、受付時に `SingleFolder` なら `Fixed` を付ける形へ
  置き換わる。要求の型で保存先が 1 つに決まり、pool の種類との組み合わせが無くなる。
- 新しい RO の open は今の Collection と同じく必須列を 1 snapshot で確かめる
  (`open_existing_collection_parent` を通常 DB にも使う)。旧 schema の通常 DB は miss として扱い、
  保存を決めたときの RW open (通常の `CatalogDb::open` と同じ migration) で新しくなる。

### 4.3 worker の読み・書き

| 要求の保存先 | メモリ (`cache_map`) | DB の lookup | 保存 |
| --- | --- | --- | --- |
| `Fixed` | 今と同じ | 今と同じ | 今と同じ |
| `Parent` | **見ない**、書かない | 親 DB を `row_key` で RO lookup (stamp・proof・ポリシーの検査は今と同じ) | 既存ポリシーが保存を選んだら親 DB へ `row_key` で |
| `Named` | 今と同じ (合成 DB は一覧を開くときに `load_all` 済み) | miss なら合成 DB を RO lookup | 合成 DB へ要求キーで |

- `Parent` でメモリを見ない理由: ★一覧などは開くときに合成 DB の旧行 (フルパス) を `cache_map` へ読んでいる。
  これに当たると親 DB に行が入らず、目標が満たされない。メモリの旧行は使わず、親 DB に無ければ 1 回作る。
- メモリ seed の要求 (`uses_memory_pin_seed`、動画ピン・フレームピン) は今と同じくメモリ優先・保存なし
  (§1.668 と同じ)。要求キーと `request_key` が一致しない派生要求 (ピンの再帰、EPUB fallback 等) は、
  `Parent` の行キーへ読み替えず、保存しない (メモリだけ)。綴りの違う 2 つのキーを worker で作らないため。
  派生要求のうち今 Fixed へ保存して効いているものが `MultiParent` の一覧にあるかは、実装前に棚卸しする
  (§9 の確認事項)。
- PDF メタ補完・旧ピン bytes の mirror など「Fixed の書き込み可能な接続だけ」の処理は、今と同じく Fixed だけ
  (`MultiParent` では行わない。§1.668 と同じ扱い)。
- perf の `collection/thumb_catalog_open`・`thumb_catalog_lookup` は区分を一般化し、`Parent` / `Named` /
  `CollectionRoot` を field で分ける。

### 4.4 一覧を開くとき・seed

- 合成 DB の一覧 (★・履歴・ブックマーク・スマートフォルダ・サブフォルダ展開・タグ) は、今と同じく開くときに
  合成 DB を開いて `load_all` する (pool の Fixed)。`Named` の要求はこのメモリを使う。
- 検索 (Ctrl+G/S/R・固定検索) で pool が無いときの `__search_results__` の準備
  (`SearchThumbnailInitRequest::prepare`) も今と同じ。
- **Everything の準備 worker のフォルダ動画ピン seed** は、`MultiParent` では DB へ commit せず、
  `cache_map` (メモリ) にだけ入れる。今は「捕捉した owner の catalog」= 借りた pool のフォルダ DB へ書いており、
  元のフォルダの DB に無関係な行が入る経路の 1 つ。Ctrl+G 側の同型の seed (`prepare_video_folder_pin_seeds` を
  使う他の準備) も同じ規則にそろえる。
- `delete_missing` は `SingleFolder` の実フォルダだけ (今と同じ)。存続キーは今と同じ関数で作るので、
  `Parent` で書いた行 (= そのフォルダの通常キー) は親フォルダを開いたときに残る。

## 5. 同名項目の衝突

- 親フォルダごとに DB が別なので、`F01\vol01.zip` と `F02\vol01.zip` は別 DB の同じキー `zipthumb:vol01.zip`。
  衝突しない。同じ DB に入るのは同じフォルダの同名項目だけで、ファイルシステム上 1 つしか無い。
- メモリ上は今と同じフルパスのキーで区別する。`Parent` の要求は `cache_map` を読み書きしないので、
  basename のキーがメモリで混ざることも無い。
- ドライブ/共有ルート直下の項目は、ルートの通常キー (フルパス) で入る。ルート DB はドライブ文字を保つ
  (catalog-design §2.2 の例外)。

## 6. 「検索語と結果は保存しません」との関係

- 製品ページ (`index.html`) と `privacy.html` は「検索語と結果一覧は保存しない。サムネイルキャッシュなどは
  通常どおり PC 内に保存する」と書いている。変更後に書くのは、**その親フォルダを開いたときに作られるのと
  同じ行** (親フォルダの DB の basename キー) だけで、検索結果の並びや検索語は残らない。約束に沿う。
- むしろ今は、pool が無いときの `__search_results__` にフルパスのキーで結果のパスが溜まっていく。
  変更後は実在の親を持つ結果はそこへ入らないので、Everything の結果 (すべて実在のファイル・フォルダ) の
  パスは検索専用の DB に残らなくなる。既存の `__search_results__` の旧行は消さない (キャッシュの削除・期限で消える)。
- 公開文書の文言は変えない (既に「サムネイルキャッシュは通常どおり保存」と書いてあり、今回の変更で偽になる文は無い)。

## 7. 対象外とした理由

- **コレクション root**: §1.668 (v4.11.0 で出荷) は元フォルダごとの別 DB にフルパスのキーで置く。
  親フォルダの通常 DB へ寄せれば代表選定とも共有できるが、出荷済みの別 DB の行が使われなくなり、
  §1.668 の報告者 (2000 件のコレクション) に全件の作り直しを 1 回強いる。旧 DB を読んで写す経路を足すと
  状態が増える。provider は同じ `LazyCatalogs` を使う形にしておき、寄せるかは別の判断にする (§10)。
- **動画**: 動画のサムネイルは catalog を使わない別 thread (`spawn_video_thread`、Shell API・video pin DB)。
- **Remote**: Remote の一覧は本体の catalog を読み取り専用で引く別経路 (`remote_ipc`) で、書かない。

## 8. 永続データの判断 (リリース済み)

- サムネイルキャッシュ (catalog DB) はリリース済み。schema・`CATALOG_VERSION`・DB の配置 (hash) は変えない。
  変えるのは「複数の親を並べる一覧が、実在の親を持つ項目の行をどの既存 DB に置くか」だけ。
- 移行コードは作らない。理由: キャッシュは作り直せるデータで、利用者が作ったデータ (設定・タグ・★・
  コレクション・ピン) は触らない。旧行は次の扱いになる。
  - 合成 DB (★・履歴・ブックマーク・スマートフォルダ・サブフォルダ展開・検索) の旧フルパス行: 実在の親を
    持つ項目では読まなくなり、親 DB に無ければ 1 回作り直す。既定で有効な「アイドル時にキャッシュ由来の
    サムネイルを高画質化する」は今も開くたびにキャッシュ由来の項目を作り直して保存している
    (§1.731 の観測) ので、体感の増分は初回 1 回分の生成待ち。旧行は消さない (キャッシュ管理の削除・期限で消える)。
  - 元のフォルダの DB に検索が書いた旧行: 今と同じく、そのフォルダを開いたときの `delete_missing` が消す。
- §1.731 (`23e1abf40`) と同じ扱い。コミットメッセージに「キャッシュの保存先の変更、移行なし」と書く。

## 9. 状態の組み合わせ・失敗の扱い

- 保存先は受付時の型 (`Fixed` / `Parent` / `Named`) で 1 つに決まり、pool の種類・借用の有無と組み合わさない。
  検索が借りた pool でも、`MultiParent` の要求は Fixed を見ない。
- 一覧の切り替え中・clear 中の扱いは §1.668 の `CatalogWork` の admission と同じ (新しい境界を作らない)。
- 兄弟 context (F12・別ウィンドウ) が同じ親フォルダを通常に開いていても、別接続の同じ DB への書き込みで、
  兄弟の items / cache_map / generation / queue / cancel には触れない。兄弟の `cache_map` は次に開くまで
  新しい行を知らないが、表示は既にあるので問題にならない (同じ行を作り直して保存しても内容は同じ)。
- まれな失敗 (DB の open / 書き込み失敗、ディスクフル) は既定の割り切り: ログ + 既存の一回通知、
  その項目はメモリだけで表示。再試行・回復処理は足さない。
- UI スレッドで新しい I/O をしない: 受付時の identity と row_key は path と既存の pin_map からの純粋計算。
  DB の open / lookup / save はすべて worker (今の Collection と同じ)。
- 確認事項 (実装前に棚卸しする):
  1. 今 Fixed へ保存している派生要求 (ピンの再帰 `CachedPinned` の mirror、EPUB fallback、RAW follow-up、
     PDF catch-up) のうち、`MultiParent` の一覧で保存が効いているもの。`Parent` では保存しない (§4.3) ので、
     毎回の作り直しが増えるものがあれば列挙して判断する。
  2. `cache_map` を前提にした処理 (自動比率の seed `reset_and_seed_auto_aspect_with_seed`、
     release check、選択の復元) が `Parent` の要求のメモリ不使用で変わらないこと。
  3. Ctrl+G の再検索・ストリーミング中の rebuild (`replace_search_view_items_with_ratings`) で、
     受付時の保存先が新しい items で付け直されること。

## 10. 簡素化の検討

- 採用: **受付時の型 1 つで保存先を決める**。pool の種類 (Fixed / Collection) × 要求の scope × 借用の有無の
  組み合わせを、要求の型だけにする。
- 採用: **新しいキーの綴りを作らない**。行キーは「親フォルダを通常に開いたときの要求キー」をその関数で作る。
- 採用しなかった: 検索に入るたびに pool を作り直して `__search_results__` を Fixed にする
  (元のフォルダの DB を汚さない点は満たすが、代表選定との共有ができない。Ctrl+G のストリーミング中の
  rebuild で worker を作り直さない今の作りも崩れる)。
- 採用しなかった: 代表選定が検索の DB も引く (検索専用 DB にフルパスで結果のパスが溜まり続け、§6 と逆向き。
  代表選定が 1 フォルダの選定のために別 DB を開く)。
- 採用しなかった: 合成 DB の旧行を親 DB へ写す移行 (分岐が増える。作り直しは初回 1 回で、
  アイドル時の高画質化が既に毎回作り直している)。
- 採用しなかった (別判断): コレクション root も親フォルダの通常 DB へ寄せる (§7)。

## 11. 変わる挙動 (利用者から見て)

- 良くなる: Everything 検索・Ctrl+G などで作った ZIP・PDF・フォルダのサムネイルが、親フォルダの代表サムネイル
  と親フォルダの一覧で使われる。先にフォルダを開いて作ったサムネイルを、検索結果・★一覧・履歴などが使う
  (開いたことのあるフォルダの項目は検索結果の表示が速くなる)。
- 良くなる: 検索を始めた元のフォルダの DB に行が入らない (戻ったときの削除も起きない)。
- 初回だけ遅くなり得る: 更新後に初めて開く★一覧・履歴・ブックマーク・スマートフォルダ・サブフォルダ展開・
  タグ一覧で、実在の親を持つ項目は親フォルダの DB に行が無ければ 1 回作り直す。
- キャッシュフォルダのファイル数: 検索で多くのフォルダにまたがって作ると、そのフォルダ数ぶん catalog DB が
  作られる (それらを順に開いたときと同じ)。既定のポリシー Auto で保存しない項目は DB を作らない。

## 12. テスト (実装時)

- 再現: 実フォルダ A を開いた状態で検索 (Everything の fake run / Ctrl+G) → 結果の ZIP のサムネイル保存 →
  A の DB に行が無い・親 F01 の DB に `zipthumb:vol01.zip` がある → F01 の親を開くと代表選定が
  `CachedPinned` で当たる (`resolve_folder_thumb_image` の cache hit)。修正前のコードで落ちること。
- 逆向き: F01 を通常に開いて作った行を、検索結果が lookup で使う (生成しない)。
- 同名: F01・F02 の `vol01.zip` が別 DB に入り、互いを表示しない。
- ドライブルート直下の項目はルート DB にフルパスのキーで入る。
- 実在の親を持たない項目 (ZipImage・SearchContainer) は合成 DB のまま。pool を借りた検索でも元フォルダの DB に書かない。
- Everything のフォルダ動画ピン seed が借りた pool の DB に書かれない。
- ★一覧・履歴・スマートフォルダ・サブフォルダ展開・タグ一覧で `Parent` になる (一覧の種類ごとの判定表テスト)。
- `SingleFolder` (通常フォルダ・ZIP の中・ドライブルート・コレクションの PhysicalSource) は今と同じ Fixed と
  キー。§1.731 の 3 テストが通ること。
- 兄弟 context: 別 context が同じ親を開いていても、その items / cache_map / generation に変化が無い。
- 割り切り: 親 DB の open 失敗でもメモリだけで表示され、ログが出る。

## 13. 文書の更新 (実装時)

[catalog-design.md](catalog-design.md) (保存先の節)、[architecture-overview.md](architecture-overview.md)、
[async-architecture.md](async-architecture.md) (§1.668 の遅延 catalog の節を一般化)、
[everything-search-plan.md](everything-search-plan.md) (「捕捉した既存 thumbnail owner の catalog へ seed」の段落)、
[search-architecture.md](search-architecture.md) (動画ピン seed の段落)、[display-pipeline.md](display-pipeline.md) の
§1.668 の段落。マニュアル・製品ページは §6 のとおり変更なし。
