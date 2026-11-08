# 検索結果などで作ったサムネイルを親フォルダのキャッシュにも置く (§1.741)

作成: 2026-11-08。状態: **設計案 r2 (独立設計レビュー r1 REVISE を反映)**。対象版: 利用者判断 (§12)。
バックログ: [next-release-backlog.md §1.741](next-release-backlog.md)。報告: 5ch >>746。

## 1. 問題

- Ctrl+R (Everything 検索) で複数フォルダの ZIP のサムネイルを作っても、親フォルダの代表サムネイルに
  使われない。検索を閉じて元のフォルダへ戻ると、作った行は消える (サブ PC 担当の実機確認・キャッシュの写し、
  backlog §1.741)。
- 原因 (コード、r1 で独立レビューも一致を確認):
  1. Ctrl+G / Ctrl+R は、**開いていた一覧のサムネイル worker pool を借りる**
     (`SearchMetadataResources::for_app` は `reload_queue` があると `thumbnail_init` を作らず、
     `install_missing_search_thumbnail_workers` も pool があれば何もしない)。通常フォルダの pool なら保存先
     (`ThumbnailCatalogProvider::FixedCatalog`) はそのフォルダの catalog で、検索結果の行は**そのフォルダの DB** に
     フルパスのキー (`zipthumb:C:\…\F01\vol01.zip`) で入る。Everything の準備 worker も、フォルダの動画ピンの
     seed を同じ catalog へ commit する (`everything/metadata.rs` の `commit_pin_materializations`)。
  2. そのフォルダへ戻ると、`delete_missing` が一覧に無いキーとして消す。
  3. フォルダの代表選定 (`thumb_loader.rs` の `resolve_folder_thumb_image_inner` → `cached_candidate`) は
     ZIP・PDF を開かず、**子フォルダ自身の catalog** を通常キー (`archive_tile_key`: basename、
     ドライブ/共有ルート直下だけフルパス) で引く。検索側の行は DB もキーも違うので、消えなくても届かない。
     なお代表選定がキャッシュ行を使うのは ZIP/PDF 系の子 (とピン) で、画像の子は直接デコードする。

## 2. 目標と範囲

- **単体ファイル項目** (画像・ZIP・PDF/EPUB・RAR 等の変換対象書庫) のサムネイルを、複数の親を並べる一覧で
  作ったときも、**その親フォルダを通常に開いたときと同じ DB・同じキー・同じ署名 (mtime/size) の行**として置く。
  親フォルダの代表選定と、親フォルダを開いたときの一覧が、その行をそのまま使う。逆向き (先にフォルダで
  作った行を検索結果などが使う) も同じ仕組みで効く。
- 検索を始めた元のフォルダの DB に、無関係な行を書かない (動画ピンの seed を含む)。
- 今その一覧で表示できているサムネイルを、表示できなくしない (取り外したドライブの閲覧履歴など)。
- 対象外 (理由は §8): フォルダ行・ピン付きの行・ファイル名スタック・ZIP/PDF の中のページ・検索の集約行、
  コレクションの root (§1.668)、音声 (既に実親 scope の別経路)、動画 (catalog を使わない別経路)、Remote。

## 3. 一覧の分類

`App::use_full_path_cache_keys()` が真になる理由を、型付きの 1 つの判定にまとめる。

```text
enum ThumbStorageScope {
    SingleFolder { full_path_keys: bool },  // 1 つの実フォルダ / ZIP・PDF の仮想フォルダ。full_path_keys = ドライブ/共有ルート
    MultiParent { view_db: ViewCatalog },   // 親の違う項目を並べる一覧
    CollectionRoot,                         // §1.668 の別 DB (変更しない)
}
```

- `use_full_path_cache_keys()` はこの値から導く (`SingleFolder { full_path_keys }` → その値、ほかは真)。
  **メモリ上のキー (要求キー・`cache_map`・seed・release check) の綴りは今と同じ**。
- 判定の材料は今と同じ (合成パスの `current_folder`、検索・タグ・履歴・★の flag、コレクションの
  `CollectionGridPosition::Root`、ドライブ/共有ルート)。ドライブ/共有ルートは 1 つの実フォルダなので `SingleFolder`。
  検索中に `current_folder` が元のフォルダのまま残っていても、検索・タグ等の flag があれば `MultiParent`。

棚卸し (コードで確認。r1 の指摘 8 で補正):

| 一覧 | 今の pool の DB (Fixed) | `ViewCatalog` (§4.3 の Named) |
| --- | --- | --- |
| 通常フォルダ・ZIP/PDF の中・ドライブ/共有ルート・コレクションの PhysicalSource | そのフォルダ | — (`SingleFolder`、変更なし) |
| Ctrl+G / Ctrl+R | **借りた pool のもの** (通常フォルダならそのフォルダ、ほかの一覧ならその一覧の DB)。pool が無ければ `__search_results__` | `__search_results__` |
| Ctrl+S / お気に入り横断検索 | `__search_results__` で pool を作る (`start_loading_extracted_items`) | `__search_results__` |
| 固定検索 | 元の経路 (Local / Container / Metadata / Everything) と同じ | 元の経路と同じ |
| タグ一覧 | `__search_results__` (再検索も同じ DB で worker を作り直す) | `__search_results__` |
| ★一覧・閲覧履歴・ブックマーク・スマートフォルダ・サブフォルダ展開 | 各合成 DB | 各合成 DB |
| ドライブ一覧 | `__mimageviewer_drive_list__` (現在地の `__drive_list__` とは別) | — (項目は単体ファイルでないので Parent にならない) |
| コレクション root | §1.668 の別 DB | — (`CollectionRoot`、変更なし) |

## 4. 要求ごとの保存先

### 4.1 受付 (UI スレッド、新しい I/O なし)

`attach_viewer_thumbnail_context` で、`MultiParent` の一覧の要求に保存先を付ける。要求の作成
(`make_load_request_with_visibility`、ピンの解決を含む) は**今と同じく 1 回だけ**呼ぶ (r1 指摘 4: 再呼出しは
ピンの cascade や metadata の I/O を増やす)。

- **Parent の条件** (すべて満たすとき): 項目が `Image` / `ZipFile` / `PdfFile` / `ConvertibleArchive`。
  要求にピンが付いていない (キーに `#pin:` が無く、`resolve_override` も無い)。`zip_entry` / `pdf_page` が無い。
  元ファイルの `parent()` が実在の通常パス (合成パスでも書庫の中でもない)。
  → `RequestCatalog::Parent { folder: 親, row_key, work }`。
  - `folder` は通常にそのフォルダを開くときと同じ identity (`CatalogDb::open(cache_dir, folder)` /
    `open_for_folder_selection` の `db_path_for`)。ConvertibleArchive は**元の書庫**の親 (変換キャッシュ ZIP の親ではない)。
  - `row_key` は純粋 helper `ordinary_file_row_key(item, parent_full_path_keys)` で作る。今の
    `container_cache_base_key` / `convertible_archive_cache_base_key` / 画像キー (basename、ルートなら
    `image_full_path_cache_key`) を `use_full_path = use_full_path_cache_keys_for_folder(親)` で呼ぶだけで、新しい綴りは
    作らない。種類フィルタの suffix (`representative_cache_key_for_policy`) は worker が要求キーに付けるのと
    同じ箇所で row_key にも付ける。
- それ以外の `MultiParent` の要求: `RequestCatalog::Named { view: ViewCatalog の identity, work }`。行キーは要求キーのまま。
- `SingleFolder` は `RequestCatalog::Fixed` (今と同じ)。`CollectionRoot` は今の `CollectionSourceParent`。
- `Parent` / `Named` は受付時の `CatalogWork` (cache dir と admission) を持ち、派生要求は継承する (r1 指摘 7)。

**`MultiParent` の要求は pool の FixedCatalog を見ない**。検索が借りた pool の DB に書く経路が構造的に無くなる。
pool の借用自体 (検索で worker を作り直さない今の作り) は変えない。

### 4.2 provider

`ThumbnailCatalogProvider` を「pool の固定 catalog + 要求ごとの遅延 catalog の LRU」の組にする。

```text
struct ThumbnailCatalogProvider {
    fixed: Option<Arc<CatalogDb>>,   // Fixed の要求だけが使う
    lazy: Arc<LazyCatalogs>,         // 今の CollectionParentCatalogs を一般化 (pool ごと 16 slot)
}
```

- 今の `CollectionParentCatalogs` の規則 (RO/RW を分けた LRU、OnceLock、lock の外で open/SQL、lookup は
  存在する DB だけ RO・必須列を 1 snapshot で確認、保存を決めた後だけ RW、Auto で保存しないなら DB を作らない、
  `CatalogWork` の admission・clear 境界、CacheOnly の一回 RO 継続) をそのまま使い、slot の識別子を
  「種類 (CollectionSourceParent / Parent / Named) + path + cache dir + admission + RO/RW」にする。
- 「Fixed が常に優先」(§1.668 の PhysicalSource 対策) は、受付時に `SingleFolder` なら `Fixed` を付ける形へ置き換わる。

### 4.3 worker の読み・書き

`Parent` の要求は、次の順で処理する。

1. **署名の確定** (worker、r1 指摘 1): 元ファイルを `std::fs::metadata` し、通常のフォルダ読込と同じ変換
   (mtime 秒・size) で `observed` 署名を作る。一覧によっては要求の署名が仮の値 (お気に入り横断検索の ZIP/PDF の
   size 0、`app.rs` 35177 付近) や保存時点の値 (閲覧履歴、`reading_history_meta_for_entry`) なので、**親 DB の行は
   要求の署名ではなく observed で読み書きする**。stat できないとき (取り外したドライブ等) は observed なし。
2. **メモリ (今と同じ)**: `cache_map` を要求キー・要求の署名で引く。★一覧などが開くときに読んだ合成 DB の旧行に
   当たれば、今と同じく表示する (r1 指摘 6: 取り外したドライブの履歴も今と同じく出る)。
3. **親 DB**: 2 で外れたら、observed があれば親 DB を `row_key`・observed で RO lookup。observed が無ければ要求の
   署名で読むだけ (保存しない)。当たれば表示。
4. **元から生成**: 外れたら今と同じく生成し、既存のポリシーが保存を選んだら**親 DB へ `row_key`・observed で保存**する。
   合成 DB・借りた pool の DB には書かない。`cache_map` にも入れない (メモリの基本キーはフルパスなので、
   入れても害は無いが、Fixed 以外で `cache_map` を書く経路を増やさない)。
5. **旧行の写し**: 2 で当たり、旧行の署名が observed と一致し、既存のポリシーが保存を選ぶときだけ、表示の後に
   親 DB を RO で確かめ、無ければ同じ bytes を親 DB へ保存する (段階移送。一致しない・stat できないなら写さない)。
   ポリシーの判断・書き込みの接続は 4 と同じものを使う。

`Named` の要求は今の Fixed と同じ処理を、宛先だけ `view` の DB にして行う (メモリ → DB の lookup → 生成・保存)。

- 派生要求 (EPUB fallback、RAW follow-up、PDF catch-up) は外側の要求の `RequestCatalog` と `CatalogWork` を継承する。
  `Parent` は単体ファイル・ピンなしに限るので、キーが外側と変わる派生 (ピンの再帰、EPUB のピンの
  `|generation:` 付け足し) は起きない。worker で `Parent` を持つ要求のキーが外側の要求キーと違えば、
  debug_assert + ログで止め、保存しない (想定外の組み合わせを黙って別の行へ書かない)。
- PDF メタ補完・旧ピン bytes の mirror など「Fixed の書き込み可能な接続だけ」の処理は今と同じく Fixed だけ。
- perf: 今の `collection/thumb_catalog_open`・`thumb_catalog_lookup` を一般化し、種類を field で分ける。
  `Parent` の stat と旧行の写しにも区間を付ける。

### 4.4 一覧を開くとき・seed

- 合成 DB の一覧は今と同じく開くときに合成 DB を `load_all` してメモリへ読む (§4.3 の 2 で使う)。
- **フォルダ動画ピンの seed** (Everything の準備 worker の `commit_pin_materializations`、同じ helper
  `prepare_video_folder_pin_seeds` を使う Ctrl+G 側の準備): `MultiParent` では DB へ commit せず、`cache_map`
  (メモリ) にだけ入れる。seed は準備のたびに video pin DB から作り直すので、DB に残さなくても表示は変わらない。
  これで「借りた pool のフォルダ DB に seed 行が入る」経路が無くなる。
- `delete_missing` は `SingleFolder` の実フォルダだけ (今と同じ)。存続キーは今と同じ関数で作るので、
  `Parent` で書いた行 (= そのフォルダの通常キー) は親フォルダを開いたときに残る。

## 5. 同名項目の衝突

- 別のフォルダの同名ファイル (`F01\vol01.zip` と `F02\vol01.zip`) は、親フォルダごとの別 DB の同じキーに入り、衝突しない。
- **既存の制約 (r1 指摘 3、そのまま受け入れる)**: 通常の catalog はサブフォルダのドライブ文字を落とす
  (`db_path_for`、リムーバブルドライブの文字が変わってもキャッシュを引き継ぐため)。`C:\photos\vol01.zip` と
  `D:\photos\vol01.zip` は同じ DB の同じ行になる。これは今も両方のフォルダを順に開けば起きることで、行の署名
  (mtime 秒 + size) が違えば相手の行は使わず作り直す (誤表示しない。両方を 1 つの一覧に並べると交互に作り直し得る)。
  署名まで一致するのは実質的に同じファイルの複製だけ。今回この制約を共有の範囲へ広げるが、ドライブ文字を保つ
  identity への変更は通常のフォルダ閲覧のキャッシュ互換を変える別の判断なので行わない。
- メモリ上は今と同じフルパスのキーで区別する。

## 6. 「検索語と結果は保存しません」との関係

- 製品ページ (`index.html`) と `privacy.html` は「検索語と結果一覧は保存しない。サムネイルキャッシュなどは
  通常どおり PC 内に保存する」と書いている。変更後に親 DB へ書くのは、**その親フォルダを開いたときに作られる
  のと同じ通常のキャッシュ行**だけで、検索結果の並びや検索語は残らない。約束に沿う。
- 今は、pool が無いときの `__search_results__` にフルパスのキーで結果のパスが溜まっていく。変更後は Everything の
  結果 (すべて実在のファイル・フォルダ) のうち単体ファイルはそこへ新しく入らない。フォルダ行などは今と同じく
  入る。既存の旧行は消さない (キャッシュの削除・期限で消える)。
- 公開文書の文言は変えない (既に「サムネイルキャッシュは通常どおり保存」と書いてあり、偽になる文は無い)。

## 7. 永続データの判断 (リリース済み)

- サムネイルキャッシュ (catalog DB) はリリース済み。schema・`CATALOG_VERSION`・DB の配置 (hash)・既存の行は変えない。
  変えるのは「複数の親を並べる一覧で作った単体ファイル項目の新しい行を、どの既存 DB に置くか」だけ。
- 互換: 旧形式 (合成 DB のフルパス行) は今と同じく読んで表示し、署名が一致すれば親 DB へ写す (§4.3 の 5)。
  旧行を読まなくなる一覧は無いので、表示できていたものが表示できなくなる退行も、作り直しの待ちも無い (r1 指摘 6)。
  スキーマ版の gate や一括移行は要らない (行の形式は変えないため)。
- 元のフォルダの DB に検索が書いた旧行: 今と同じく、そのフォルダを開いたときの `delete_missing` が消す。
- 利用者が作ったデータ (設定・タグ・★・コレクション・ピン) は触らない。

## 8. 対象外とした理由

- **フォルダ行・ピン付きの行**: フォルダの代表の行は、選定の proof・再帰の `CachedPinned` mirror・動画ピンの seed と
  組になっていて、親 DB で通常一覧と同じ行を作るには、これらの保存先も揃える必要がある (r1 指摘 5)。目標
  (子の ZIP・PDF の表紙を親の代表選定が使う) はフォルダ行なしで満たせる: 子の行が親 DB にあれば、フォルダの
  代表選定は書庫を開かずに当たる。フォルダ行・ピン付きの行は今の保存先 (Named) のまま。
- **EPUB のピン付き行**: worker が `|generation:` を付け足すので受付時に行キーが決まらない (r1 指摘 2)。ピンなしに
  限るので対象外になる。なお、通常フォルダの読込の存続キーが世代の付かないピンキーを返し、`delete_missing` が
  EPUB のピン付き行を消している可能性がある (r1 指摘 2 の後半)。§1.741 とは独立した既存の問題として別に起票する。
- **ファイル名スタック**: 代表画像の要求だが、一覧の中の集約行で、`drag_source_path()` も返さない。今のまま。
- **ZIP/PDF の中のページ・検索の集約行 (`SearchContainer`)・ドライブ一覧の行**: 実在の親フォルダの通常一覧の行ではない。
- **コレクション root**: §1.668 (v4.11.0 で出荷) は元フォルダごとの別 DB にフルパスのキーで置く。旧行はメモリに
  読まれていないので、§4.3 の段階移送をそのまま使えない (元フォルダごとの旧 DB の lookup が別に要る)。provider は
  同じ `LazyCatalogs` を使う形にしておき、寄せるかは別の判断にする。
- **音声**: generic provider より前に専用経路へ分かれ、既に実親 scope の `audioart:` を使う (`thumb_loader.rs` 2803 付近)。
- **動画**: catalog を使わない別 thread (`spawn_video_thread`、Shell API・video pin DB)。
- **Remote**: Remote の一覧は本体の catalog を読み取り専用で引く別経路で、書かない。

## 9. 状態の組み合わせ・失敗の扱い

- 保存先は受付時の型 (`Fixed` / `Parent` / `Named` / `CollectionSourceParent`) で 1 つに決まり、pool の種類・借用の
  有無と組み合わさない。
- clear (キャッシュ削除) を跨いだ要求は、受付時の `CatalogWork` の admission で今の Collection と同じく保存しない。
- 兄弟 context (F12・別ウィンドウ) が同じ親フォルダを通常に開いていても、pool ごとの別接続で同じ DB に書くだけで、
  兄弟の items / cache_map / generation / queue / cancel には触れない (共有 epoch を進めない・兄弟を reload しない)。
  兄弟の `cache_map` は次に開くまで新しい行を知らず、同じ行を作って保存し得るが、内容は同じ。
- §1.697 (種類アイコン表示) とは層が別: §1.697 は要求を出すかどうか、§1.741 は出した要求の保存先。§1.697 の
  `NoArt` から `Pending` へ戻った要求も、通常の受付を通るので保存先が付く。
- まれな失敗 (DB の open / 書き込み失敗、ディスクフル、stat の失敗) は既定の割り切り: ログ + 既存の一回通知、
  その項目はメモリだけで表示。再試行・回復処理は足さない。
- UI スレッドで新しい I/O をしない: 受付時の identity と row_key は path からの純粋計算。stat・DB の open / lookup /
  save はすべて worker。

## 10. 簡素化の検討

- 採用: **受付時の型 1 つで保存先を決める**。pool の種類 × 要求の scope × 借用の有無の組み合わせを、要求の型だけにする。
- 採用: **対象を単体ファイル・ピンなしに絞る**。フォルダ行の proof・再帰 mirror・seed、EPUB の世代、ピンの再帰を
  親 DB の側で扱わずに済み、目標は満たせる。
- 採用: **署名は worker の stat で確定**。一覧ごとに「署名が本物か」の宣言を作らない。
- 採用: **旧行は今と同じく読み、一致すれば写す**。移行の段や版の gate を作らずに互換を保つ。
- 採用しなかった: 検索に入るたびに pool を作り直して `__search_results__` を Fixed にする (元のフォルダの DB を
  汚さない点は満たすが、代表選定との共有ができない。Ctrl+G のストリーミング中の rebuild で worker を作り直さない今の作りも崩れる)。
- 採用しなかった: 代表選定が検索の DB も引く (検索専用 DB に結果のパスが溜まり続け、§6 と逆向き。代表選定が
  1 フォルダのために別 DB を開く)。
- 採用しなかった: ドライブ文字を保つ catalog identity への変更 (§5)。
- 採用しなかった (別判断): フォルダ行、コレクション root も親 DB へ寄せる (§8)。

## 11. 変わる挙動 (利用者から見て)

- 良くなる: Everything 検索・Ctrl+G・★一覧・履歴などで作った ZIP・PDF・書庫のサムネイルが、親フォルダの代表
  サムネイルと親フォルダの一覧で使われる。フォルダを開いて作ったサムネイルを、検索結果などが使う
  (開いたことのあるフォルダの項目は検索結果の表示が速くなる)。
- 良くなる: 検索を始めた元のフォルダの DB に行が入らない (戻ったときの削除も起きない)。
- 費用: 複数の親を並べる一覧で、単体ファイル項目ごとに worker が 1 回 stat する。メモリに旧行が無い項目は、
  親フォルダごとに catalog を開いて引く (§1.668 のコレクションと同じ作り。計測は §1.668 で 1 回の参照が約 0.07ms)。
  表示の順番 (メモリ → DB → 生成) は今と同じなので、メモリで当たっていた項目の表示は遅くならない。
- キャッシュフォルダのファイル数: 検索で多くのフォルダにまたがって作ると、そのフォルダ数ぶん catalog DB が
  作られる (それらを順に開いたときと同じ)。既定のポリシー Auto で保存しない項目は DB を作らない。
- 不便になる既存の挙動: 無い (コードで追った範囲。§12 のテストで確かめる)。

## 12. 規模と出荷の見込み

- 本体の変更: `RequestCatalog` / provider の一般化 (`catalog_provider.rs`)、worker の lookup・保存の分岐
  (`thumb_loader.rs`)、受付時の分類と row_key (`app.rs`)、動画ピン seed の書き先 (`everything/metadata.rs` 等)。
  数百行と、横断のテスト。中規模。
- r1 の見積もり (対象を絞る前の案): 実装・テスト 3〜5 人日 + 独立レビュー・全件 gate・実機確認。r2 で対象を
  単体ファイル・ピンなしに絞ったので減るが、provider は出荷済みの §1.668 の機構を触る。
- v5.1.0 (数日以内) に入れるかは利用者判断。設計担当の見立ては「v5.1.0 に間に合わせるより、v5.1.x で出す方が
  安全」。

## 13. テスト (実装時)

- 再現: 通常フォルダ A を開いた状態で Ctrl+R (fake run) / Ctrl+G → 結果の ZIP のサムネイル生成 →
  A の DB に行が無い・親 F01 の DB に `zipthumb:vol01.zip` (observed 署名) がある → F01 の親を開くと代表選定が
  `CachedPinned` で当たる。修正前のコードで落ちること。
- 逆向き: F01 を通常に開いて作った行を、検索結果が親 DB の lookup で使う (生成しない)。
- 署名: お気に入り横断検索 (size 0) と閲覧履歴 (保存時の署名) でも、親 DB の行は observed で書かれ、通常一覧の
  行を上書きしない。stat できない項目は親 DB に書かない。
- 互換: ★一覧の合成 DB の旧フルパス行が今と同じく表示され、署名が一致すれば親 DB に写る。stat できない項目は
  旧行で表示され続ける。
- 同名: F01・F02 の `vol01.zip` が別 DB に入る。ドライブルート直下の項目はルート DB にフルパスのキーで入る。
- 対象外: フォルダ行・ピン付き・ZipImage・SearchContainer・Stack は今の保存先 (Named)。借りた pool の DB に書かない。
- seed: Everything のフォルダ動画ピン seed が借りた pool の DB に書かれず、表示は今と同じ。
- 一覧の種類ごとの判定表 (`ThumbStorageScope`) のテスト。`SingleFolder` (通常フォルダ・ZIP の中・ドライブルート・
  コレクションの PhysicalSource) は今と同じ Fixed とキー。§1.731 の 3 テストが通ること。
- clear: `Parent` / `Named` とも、clear を跨いだ旧要求は保存せず、CacheOnly の一回 RO 継続は今と同じ。
- 兄弟 context: 別 context が同じ親を開いていても、その items / cache_map / generation に変化が無い。
- 割り切り: 親 DB の open 失敗でもメモリだけで表示され、ログが出る。
- 自動比率の seed (`reset_and_seed_auto_aspect_with_seed`) と結果到着後の標本、idle upgrade (`UpgradeableCache`)、
  release check (実際の保存先を見る形に) の回帰。

## 14. 文書の更新 (実装時)

[catalog-design.md](catalog-design.md) (保存先の節)、[architecture-overview.md](architecture-overview.md)、
[async-architecture.md](async-architecture.md) (§1.668 の遅延 catalog の節を一般化)、
[everything-search-plan.md](everything-search-plan.md) (「捕捉した既存 thumbnail owner の catalog へ seed」の段落)、
[search-architecture.md](search-architecture.md) (動画ピン seed の段落)、[display-pipeline.md](display-pipeline.md) の
§1.668 の段落。マニュアル・製品ページは §6 のとおり変更なし。
