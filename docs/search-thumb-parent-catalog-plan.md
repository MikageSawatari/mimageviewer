# 検索結果などで作ったサムネイルを親フォルダのキャッシュにも置く (§1.741)

作成: 2026-11-08。状態: **設計確定 (独立設計レビュー r1〜r4 REVISE → r5 ACCEPT、2026-11-09、Codex GPT-6.1 Sol / xhigh)・
実装は未着手 (利用者判断待ち)**。対象版: 利用者判断 (§12)。
バックログ: [next-release-backlog.md §1.741](next-release-backlog.md)。報告: 5ch >>746。

## 1. 問題

- Ctrl+R (Everything 検索) で複数フォルダの ZIP のサムネイルを作っても、親フォルダの代表サムネイルに
  使われない。検索を閉じて元のフォルダへ戻ると、作った行は消える (サブ PC 担当の実機確認・キャッシュの写し、
  backlog §1.741)。
- 原因 (コード、r1 で独立レビューも一致を確認):
  1. Ctrl+G / Ctrl+R は、**開いていた一覧のサムネイル worker pool を借りる**
     (`SearchMetadataResources::for_app` は `reload_queue` があると `thumbnail_init` を作らず、
     `install_missing_search_thumbnail_workers` も pool があれば何もしない)。通常フォルダの pool なら保存先
     (`ThumbnailCatalogProvider::FixedCatalog`) とメモリ (`cache_map`) はそのフォルダのもので、検索結果の行は
     **そのフォルダの DB** にフルパスのキー (`zipthumb:C:\…\F01\vol01.zip`) で入る。Everything の準備 worker も、
     フォルダの動画ピンの seed を「捕捉した owner」= 同じ catalog へ commit する (`everything/metadata.rs`)。
  2. そのフォルダへ戻ると (検索を閉じると `restore_view_return_origin` → 通常の読込)、`delete_missing` が
     一覧に無いキーとして消す。
  3. フォルダの代表選定 (`thumb_loader.rs` の `resolve_folder_thumb_image_inner` → `cached_candidate`) は
     ZIP・PDF を開かず、**子フォルダ自身の catalog** を通常キー (`archive_tile_key`: basename、
     ドライブ/共有ルート直下だけフルパス) で引く。検索側の行は DB もキーも違うので、消えなくても届かない。
     代表選定がキャッシュ行を使うのは ZIP/PDF 系の子 (とピン) で、画像の子は直接デコードする。

## 2. 目標と範囲

- **単体ファイル項目** (画像・ZIP・PDF/EPUB) の**ピンなしの表紙**を、複数の親を並べる一覧で**新しく作った・
  作り直した**とき (元からの生成、またはアイドル時の高画質化の `SourceOnly` による再生成)、**その親フォルダを通常に
  開いたときと同じ DB・同じキー・同じ署名の行**として置く。親フォルダの代表選定と親フォルダの一覧が、その行を
  そのまま使う。逆向き (先にフォルダで作った行を検索結果などが使う) も効く。
  共有を保証するのはこの「新しく保存した行」だけ。合成 DB の旧行や、画面に残っている表示を親フォルダへ移す保証はしない (r3 で合意)。
- 検索を始めた元の一覧の DB とメモリに、検索の行を書かない (動画ピンの seed を含む)。
- 今その一覧で表示できているサムネイルを、表示できなくしない・遅くしない (取り外したドライブの閲覧履歴など)。
- 対象外 (理由は §8): フォルダ行・ピン付きの行・RAR 等の変換対象書庫・ファイル名スタック・ZIP/PDF の中のページ・
  検索の集約行・ドライブ一覧、コレクションの root (§1.668)、音声 (既に実親 scope の別経路)、動画 (catalog を使わない別経路)、Remote。

## 3. 構造の要点

2 つの変更だけで目標を満たす。

1. **検索も自分の pool を持つ (§4)**。複数の親を並べる一覧は、どれも「その一覧の合成 DB を Fixed に持つ自分の
   pool」で動く。今これを満たさないのは Ctrl+G / Ctrl+R が他の一覧の pool を借りるときだけ。
2. **単体ファイルの表紙要求には「親フォルダの行」を付ける (§5)**。worker はその要求の DB の読み書きを、
   pool の Fixed ではなく親フォルダの DB で行う。メモリ (`cache_map`) は今と同じく一覧自身のもので、要求キーで扱う。

一覧の分類 (コードで確認。r1 指摘 8・r2 指摘 7 で補正):

| 一覧 | pool の Fixed (変更後) | 単体ファイルの表紙の DB |
| --- | --- | --- |
| 通常フォルダ・ZIP/PDF の中・ドライブ/共有ルート・コレクションの PhysicalSource | そのフォルダ (今と同じ) | Fixed (今と同じ) |
| Ctrl+G / Ctrl+R (固定検索の Metadata / Everything を含む) | **`__search_results__`** (今は借りた pool のもの) | 親フォルダ |
| Ctrl+S / お気に入り横断検索 (固定検索の Container) | `__search_results__` (今と同じ) | 親フォルダ |
| 固定検索の Local | 現在のフォルダの絞り込み (`SingleFolder`、今と同じ) | Fixed (今と同じ) |
| タグ一覧 | `__search_results__` (今と同じ) | 親フォルダ |
| ★一覧・閲覧履歴・ブックマーク・スマートフォルダ・サブフォルダ展開 | 各合成 DB (今と同じ) | 親フォルダ |
| ドライブ一覧 | `__mimageviewer_drive_list__` (今と同じ。pin-only の読取・seed・clear も今と同じ) | — (項目はドライブで、単体ファイルが無い) |
| コレクション root | なし + §1.668 の元フォルダ別 DB (今と同じ) | §1.668 (今と同じ) |

「親フォルダ」にするかどうかは、一覧の種類ではなく §5.1 の要求の条件と、一覧が「複数の親を並べる」か
(`ThumbStorageScope`、§5.2) で決める。

## 4. 検索も自分の pool を持つ

- Ctrl+G / Ctrl+R の結果を採用するとき、今の pool の Fixed が `__search_results__` でなければ、pool を
  `__search_results__` の catalog と、その `load_all` のメモリで作り直す。作り直しは既存の
  `restart_thumbnail_workers_after_metadata_pin_refresh` と同じ手順 (cancel token・channel・queue を新しくし、
  旧 pool の要求の結果は世代で捨てる)。
- **検索の thumbnail owner の受け渡しを 1 つの契約にする** (r3 指摘 1):
  - `SearchMetadataResources::for_app` は、今の pool が検索のもの (`ThumbnailPoolCatalog::View(search)`) なら
    `SearchThumbnailOwner::Current(その owner の identity と resources)`、それ以外 (pool なし・他の一覧の pool) なら
    `SearchThumbnailOwner::Prepare { request, replaced: 今の pool の identity または なし }` を作る。`Prepare` のとき、
    他の一覧の `folder_thumbnails` は捕捉しない。
  - 準備 worker は `Prepare` なら `__search_results__` を open・`load_all` し (今の pool なしのときの
    `SearchThumbnailInitRequest::prepare` と同じ)、**その新しい owner** に Everything の動画ピン seed を commit する。
    既存 owner を優先して返す今の `prepare(existing)` は `Current` のときだけ使う。
  - Ready の検証 (`is_current`): `Current` は今と同じく現在の owner と identity を比べる。`Prepare` は「現在の pool が
    まだ `replaced` と同じ (Ready 待ちの間に別の owner へ変わっていない)」ことを確かめる。変わっていれば今と同じく
    棄却して準備を取り直す。identity の検証自体は外さない。
  - 採用 (`install_missing_search_thumbnail_workers` を一般化) で、`Prepare` の owner の catalog・メモリで pool を
    作り直す (pool なしなら作る)。以後の rebuild は `Current` として現在の検索 owner を捕捉する今の経路に戻る。
  - `ThumbnailPoolCatalog` は pool の channel・queue・cancel token と同じ context の所有単位に置き、pool を作る・作り直す・
    退役させる箇所で一緒に置き換える (別 context へ漏らさない)。
- 同じ検索の中のストリーミング・再検索・並べ替えの rebuild (`replace_search_view_items_with_ratings`) では
  作り直さない (pool が既に検索のもの)。今の「Ctrl+G の rebuild で worker を作り直さない」作りは保つ。
- pool がどの DB のものかは、pool を作るときに型で記録する (`ThumbnailPoolCatalog`: 実フォルダ / 合成 DB /
  コレクション / なし)。`current_color_catalog` の Arc の比較や合成パスの推測で判定しない。
- 上の契約で、Everything の準備 worker のフォルダ動画ピン seed の commit 先は常に検索自身の `__search_results__` になり、
  借りた pool の DB に seed 行が入る経路が無くなる。
- 検索を閉じると、今と同じく元の一覧を通常に読み込む (新しい pool)。戻りの経路は変えない。
- 費用: 他の一覧から検索に入る 1 回目に pool を作り直す。結果の採用で items は総入れ替えになり、元の一覧の
  要求は今も世代で捨てているので、表示の挙動は変わらない (元の一覧と同じパスの項目の `Loaded` の引き継ぎ
  `preserved` も今と同じ)。

これで次が今の作りのまま成り立つ (r2 指摘 4・5・7 の解消):
- 単体ファイル以外の項目 (フォルダ行・ピン付き・ZIP/PDF の中・集約行) は、その一覧の合成 DB とメモリ (Fixed) で
  今と同じ処理を受ける。再帰 `CachedPinned` の mirror、PDF メタ補完・catch-up も今の Fixed の規則のまま
  (Parent の PDF メタ補完は §5.4)。
- 借りた pool のメモリを別の一覧のキーで引くことが無くなる。

## 5. 単体ファイルの表紙要求の「親フォルダの行」

### 5.1 要求を作るとき (UI スレッド、新しい I/O なし)

`make_load_request_with_visibility` (今と同じく 1 回だけ呼ぶ。r1 指摘 4) の中で、**ピンを使わない通常の表紙の
分岐**だけが `ParentRow` を返す (r2 指摘 1: 条件は要求のフィールドの有無ではなく、どの分岐で作ったかで決める)。

| 元の項目 | ParentRow を返す分岐 | 返さない分岐 |
| --- | --- | --- |
| `Image` | 通常 (ピンの概念なし) | — |
| `ZipFile` | ピンなし (要求は ZIP の先頭画像) | ピンあり (`#pin:`) |
| `PdfFile` (EPUB を含む) | ピンなし (`pdf_page = Some(0)` の通常の表紙) | ピンあり |
| それ以外 (`ConvertibleArchive`・`Folder`・`Stack`・`ZipImage`・`PdfPage`・`ZipDir`・`SearchContainer`・`Audio`・`Video`) | — | すべて |

`ConvertibleArchive` (RAR 等) を外す理由 (r3 指摘 4): 読むのは以前に変換した ZIP で、その ZIP がどの元書庫の署名に
対応するかを今の `CachedZip` は持たない。元書庫が変換後に更新されていると、古い内容の表紙を新しい署名で親 DB へ
保存し得る。また、フォルダの自動代表の候補は ZIP/PDF 系だけで、変換対象書庫の行は代表選定に使われない
(r1 の確認)。目標に要らない対象のために変換の署名を持ち回る契約を足さない。

```text
struct ParentRow {
    source: PathBuf,       // 元の項目のパス
    folder: PathBuf,       // source.parent()。実在の通常パスのときだけ ParentRow を作る
    row_key: String,       // その親フォルダを通常に開いたときの要求キー
}
```

- `row_key` は今のキー helper (`container_cache_base_key` / 画像キー) を
  `use_full_path = use_full_path_cache_keys_for_folder(folder)` で呼んだもの。新しい綴りは作らない。種類フィルタの
  suffix (`representative_cache_key_for_policy`) は、worker が要求キーに付けるのと同じ箇所で `row_key` にも付ける。
- `folder` の DB は、通常にそのフォルダを開くときと同じ (`CatalogDb::open(cache_dir, folder)` /
  `open_for_folder_selection` の `db_path_for`)。
- 要求キー (メモリのキー) は今と同じ (複数の親を並べる一覧ではフルパス)。

### 5.2 使うかどうか

`ThumbStorageScope` (`use_full_path_cache_keys()` が真になる理由を 1 つの型にしたもの) が `MultiParent` の一覧の
要求だけが、受付時 (`attach_viewer_thumbnail_context`) に `ParentRow` を `RequestCatalog::Parent { row: ParentRow, work: CatalogWork }`
として付ける。

```text
enum ThumbStorageScope {
    SingleFolder { full_path_keys: bool },  // 1 つの実フォルダ / ZIP・PDF の中。full_path_keys = ドライブ/共有ルート
    MultiParent,                            // 検索・タグ・★・履歴・ブックマーク・スマート・サブフォルダ展開
    CollectionRoot,                         // §1.668
    DriveList,                              // ドライブ一覧 (Fixed のまま)
}
```

- `use_full_path_cache_keys()` はこの値から導く。判定の材料は今と同じ (合成パスの `current_folder`、検索・タグ・
  履歴・★の flag、コレクションの `Root`、ドライブ/共有ルート)。検索中に `current_folder` が元のフォルダのまま残って
  いても、検索・タグ等の flag があれば `MultiParent`。
- `SingleFolder` の要求は今と同じ `Fixed`。`ParentRow` は付けない (そのフォルダの Fixed が親 DB そのもの)。
- 受付時の `CatalogWork` (cache dir と admission) を持ち、派生要求は継承する (r1 指摘 7)。

### 5.3 provider

`ThumbnailCatalogProvider` を「pool の固定 catalog + 要求ごとの遅延 catalog の LRU」の組にする。

```text
struct ThumbnailCatalogProvider {
    fixed: Option<Arc<CatalogDb>>,   // Fixed の要求が使う
    lazy: Arc<LazyCatalogs>,         // 今の CollectionParentCatalogs を一般化 (pool ごと 16 slot)
}
```

- 今の `CollectionParentCatalogs` の規則 (RO/RW を分けた LRU、OnceLock、lock の外で open/SQL、lookup は存在する DB
  だけ RO・必須列を 1 snapshot で確認、保存を決めた後だけ RW、Auto で保存しないなら DB を作らない、`CatalogWork` の
  admission・clear 境界、CacheOnly の一回 RO 継続) をそのまま使い、slot の識別子を「種類 (§1.668 の元フォルダ別 /
  親フォルダ) + path + cache dir + admission + RO/RW」にする。
- 「Fixed が常に優先」(§1.668 の PhysicalSource 対策) は、受付時に `SingleFolder` なら `Fixed` を付ける形へ置き換わる。

### 5.4 worker の読み・書き (Parent の要求)

手順 0 として、**今の worker の要求の準備 (`prepare_load_request_stamp`、EPUB の read target と世代の解決) を今と同じく
最初に行う** (r3 指摘 3)。以下の「要求の署名」は、この準備の後の署名 (EPUB なら世代の署名) を指す。

| `LoadSourcePolicy` | 1. メモリ | 2. 署名の確定 | 3. 親 DB の lookup | 4. 元から生成 | 5. 保存 |
| --- | --- | --- | --- | --- | --- |
| `CacheOrSource` | 要求キー・要求の署名で引く (今と同じ)。当たれば表示して終わり | 1 が外れたら | observed で。observed が無ければ要求の署名で (読むだけ)。当たればメモリへ要求キー・要求の署名で | 3 が外れたら (今と同じ) | ポリシーが選べば親 DB へ `row_key`・observed。メモリへ要求キー・要求の署名で |
| `SourceOnly` (アイドル時の高画質化) | 飛ばす (今と同じ) | する | 飛ばす | する | 同上 |
| `CacheOnly` | 引く (今と同じ) | しない | 要求の署名で RO lookup だけ | しない | しない |

- **メモリを最初に引き、当たれば filesystem の stat を足さない** (r2 指摘 3)。EPUB の世代解決 (手順 0) は今もメモリより前に
  行っている処理で、省かない。★一覧などが開くときに読んだ合成 DB の旧行 (フルパス) は
  今と同じ速さで表示され、取り外したドライブでも今と同じく出る。stat は、今も元ファイルを読みに行く場面
  (メモリにも DB にも無い) の直前にだけ行うので、オフラインのドライブで新たに待つ場面は増えない。
- **署名 (observed) は、通常一覧の要求が最終的に使う署名と同じ所有者で決める** (r1 指摘 1・r2 指摘 2):
  - 画像・ZIP・通常の PDF: `ParentRow.source` の `std::fs::metadata` を、通常のフォルダ読込
    (`folder_scan.rs` の `mtime_secs` と `len`) と同じ helper で変換する。
  - EPUB: 手順 0 で解決した世代の署名 (`stamp_resolved_pdf_request` の `generation_catalog_pair`) をそのまま使う。
    通常一覧の EPUB 行も同じ署名なので一致する。
  - RAW の follow-up (現像待ちの再開): 最初の処理で確定した observed を継承し、再開時に stat し直さない。
  - 一覧が渡す要求の署名 (お気に入り横断検索の ZIP/PDF の size 0、閲覧履歴の保存時の値など) は、親 DB へ**書く**
    行には使わない。
  - stat できない (取り外したドライブ等): 親 DB は要求の署名で RO lookup だけ行い (保存しない)、外れれば今と同じく
    生成を試みて失敗する経路へ進む。
- clear を跨いだ `CacheOnly` は、§1.668 と同じく旧メモリを使わず、provider の一回 RO 継続で successor を読む。
  clear を跨いだ要求は保存しない (受付時の admission)。
- **合成 DB の旧行は親 DB へ写さない**。旧行で表示した項目は、既定で有効な「アイドル時にキャッシュ由来の
  サムネイルを高画質化する」の `SourceOnly` 要求が元から作り直したときに、observed 付きで親 DB に入る。この設定を
  切っている場合、旧行で表示できている項目は親 DB に入らない (新しく作る項目は入る)。写しの段を設けないのは、
  表示後に stat を足す経路 (r2 指摘 3) と移行の段を増やさないため (§10)。
- 生成した行の DB は親 DB だけ (合成 DB には書かない)。あわせて、**生成・保存したときと、親 DB の lookup で当たった
  ときの両方で**、その一覧自身の `cache_map` に**要求キー・手順 0 の後の要求の署名**で入れる (r3 指摘 2・r4 指摘 1)。
  DB 用の observed をメモリの署名にはしない。親 DB への再保存・旧行の写しはしない。
  Fixed の要求が保存成功時に `cache_map` を更新する今の作りと同じで、表示範囲から外れて戻った
  ときや、その後にドライブを外したときも、メモリから再表示できる (要求の署名が仮の値でも、メモリの照合は今と同じく
  要求の署名どうしなので当たる)。`cache_map` に `row_key` (basename) を入れることはしない。
  複数の親を並べる一覧は §4 で必ず自分の pool とメモリを持つので、他の一覧のメモリを書くことは無い。
- 派生要求 (EPUB fallback、RAW follow-up、PDF catch-up) は外側の `RequestCatalog` と `CatalogWork` を継承する。
  `ParentRow` はピンなしの表紙だけに付くので、キーが外側と変わる派生 (ピンの再帰、EPUB のピンの世代付け足し) は
  起きない。worker で Parent の要求のキーが外側の要求キーと違えば、debug_assert + ログで止め、親 DB に書かない。
- **PDF メタ補完は Parent の保存先でも行う** (r3 指摘 5)。今 Fixed の書き込み接続で行っている PDF メタの保存
  (表紙のレンダリング結果からのページ数等の保存、cache hit 時の catch-up) を、保存先が Parent のときは親 DB の
  書き込み接続 (受付時の `CatalogWork` の admission の下、表紙の保存と同じ `ThumbnailSaveTarget` から開くもの) で、
  今と同じ password の安全条件で行う。今は検索が PDF の親フォルダの pool を借りたときだけ親 DB に入っていた
  pdf_meta が、どこから検索しても親 DB に入る (Enter の即時 placeholder が使える場面が増える)。
  RO の lookup 接続を書き込みに使わない (§1.668 と同じ)。
- perf: 今の `collection/thumb_catalog_open`・`thumb_catalog_lookup` を一般化して種類を field で分け、Parent の stat にも区間を付ける。

## 6. 同名項目の衝突

- 別のフォルダの同名ファイル (`F01\vol01.zip` と `F02\vol01.zip`) は、親フォルダごとの別 DB の同じキーに入り、衝突しない。
  メモリ上は今と同じフルパスのキーで区別する。
- **既存の制約 (r1 指摘 3、r2 指摘 8 で書き方を訂正、そのまま受け入れる)**: 通常の catalog はサブフォルダの
  ドライブ文字を落とす (`db_path_for`、リムーバブルドライブの文字が変わってもキャッシュを引き継ぐため)。
  `C:\photos\vol01.zip` と `D:\photos\vol01.zip` は同じ DB の同じ行になる。署名が違えば相手の行を使わず作り直す
  (1 つの一覧に両方を並べると交互に作り直し得る)。**署名 (mtime 秒 + size) が同じで中身が違うものは区別できず、
  誤表示し得る**。これは今も両方のフォルダを順に開けば起きる既存の制約で、今回その範囲が検索などにも広がる。
  ドライブ文字を保つ identity への変更は通常のフォルダ閲覧のキャッシュ互換を変える別の判断なので行わない。
- 既存の別問題 (今回は直さない、backlog §1.743): 検索の合成 DB では、`ZipImage` (entry 名)・`PdfPage`・`ZipDir` の要求キーが
  書庫を含まず、検索の ZIP 内行は署名が `(0,0)` のことがある (r2 指摘 5)。今回の変更で、借りた一覧のメモリを引く
  経路は無くなるが、Ctrl+G / Ctrl+R の行がいつも `__search_results__` に入るようになるので、従来は開始元ごとの別 DB に
  分かれていた `cover.png`・`(0,0)` の行が 1 つの DB で再利用され得る (r3 指摘 6)。今の通常の索引には ZIP 内のヒットが
  無い (`global_search_ui.rs` 1292 付近) ので、影響は旧索引・古い索引の経路に限られる。この残りと共有化の影響を
  backlog §1.743 に起票した。

## 7. 永続データの判断 (リリース済み)

- サムネイルキャッシュ (catalog DB) はリリース済み。schema・`CATALOG_VERSION`・DB の配置 (hash)・既存の行は変えない。
  変えるのは「複数の親を並べる一覧で作る、単体ファイルのピンなしの表紙の**新しい行**を、どの既存 DB に置くか」と、
  「Ctrl+G / Ctrl+R の行を借りた pool の DB ではなく `__search_results__` に置く」こと。
- 互換: 旧形式 (合成 DB のフルパス行) は今と同じくメモリから読んで表示する。読まなくなる旧行は無いので、
  表示できていたものが表示できなくなる退行・作り直しの待ちは無い (r1 指摘 6)。スキーマ版の gate・一括移行は要らない。
- 元のフォルダの DB に検索が書いた旧行: 今と同じく、そのフォルダを開いたときの `delete_missing` が消す。
- 利用者が作ったデータ (設定・タグ・★・コレクション・ピン) は触らない。

## 8. 対象外とした理由

- **フォルダ行・ピン付きの行**: フォルダの代表の行は、選定の proof・再帰の `CachedPinned` mirror・動画ピンの seed と組に
  なっていて、親 DB で通常一覧と同じ行を作るにはこれらの保存先も揃える必要がある (r1 指摘 5)。目標 (子の ZIP・PDF の
  表紙を親の代表選定が使う) はフォルダ行なしで満たせる: 子の行が親 DB にあれば、フォルダの代表選定は書庫を開かずに
  当たる。フォルダ行・ピン付きの行は今と同じくその一覧の合成 DB (Fixed) に置く。
- **EPUB のピン付き行**: worker が `|generation:` を付け足すので受付時に行キーが決まらない (r1 指摘 2)。なお、通常
  フォルダの読込の存続キーが世代の付かないピンキーを返し、`delete_missing` が EPUB のピン付き行を消している可能性が
  ある (r1 指摘 2 の後半)。§1.741 とは独立した既存の問題として backlog §1.742 に起票した。
- **ファイル名スタック**: 一覧の中の集約行で、代表は通常フォルダでは画像の行をそのまま使う。サブフォルダ展開の
  スタックは今のまま (合成 DB)。
- **ZIP/PDF の中のページ・検索の集約行 (`SearchContainer`)・ドライブ一覧の行**: 実在の親フォルダの通常一覧の行ではない。
- **コレクション root**: §1.668 (v4.11.0 で出荷) は元フォルダごとの別 DB にフルパスのキーで置き、メモリに旧行を読まない。
  親 DB へ寄せると、2000 件規模のコレクション (§1.668 の報告) で旧行を読む別の lookup が要る。provider は同じ
  `LazyCatalogs` を使う形にしておき、寄せるかは別の判断にする。
- **RAR 等の変換対象書庫**: §5.1 (変換済み ZIP と元書庫の署名の対応を持たない。代表選定にも使われない)。
- **音声**: generic provider より前に専用経路へ分かれ、既に実親 scope の `audioart:` を使う (`thumb_loader.rs` 2803 付近)。
- **動画**: catalog を使わない別 thread (`spawn_video_thread`、Shell API・video pin DB)。
- **Remote**: Remote の一覧は本体の catalog を読み取り専用で引く別経路で、書かない。

## 9. 状態の組み合わせ・失敗の扱い

- 保存先は受付時の型 (`Fixed` / `Parent` / `CollectionSourceParent`) で 1 つに決まる。複数の親を並べる一覧は必ず
  自分の pool を持つので、「借りた pool × 要求の scope」の組み合わせが無くなる。
- clear (キャッシュ削除) を跨いだ要求は、受付時の `CatalogWork` の admission で今の Collection と同じく保存しない。
- 兄弟 context (F12・別ウィンドウ) が同じ親フォルダを通常に開いていても、pool ごとの別接続で同じ DB に書くだけで、
  兄弟の items / cache_map / generation / queue / cancel には触れない (共有 epoch を進めない・兄弟を reload しない)。
  兄弟の `cache_map` は次に開くまで新しい行を知らず、同じ行を作って保存し得るが、内容は同じ。
- 検索の pool の作り直しは、その context (検索を開いた context) の pool だけ。兄弟の pool には触れない。
- §1.697 (種類アイコン表示) とは層が別: §1.697 は要求を出すかどうか、§1.741 は出した要求の保存先。§1.697 の
  `NoArt` から `Pending` へ戻った要求も、通常の受付を通るので保存先が付く。
- まれな失敗 (DB の open / 書き込み失敗、ディスクフル、stat の失敗) は既定の割り切り: ログ + 既存の一回通知、
  その項目はメモリだけで表示。再試行・回復処理は足さない。
- UI スレッドで新しい I/O をしない: `ParentRow` は要求を作るときの path からの純粋計算。pool の作り直しの DB の
  open / load_all は今の検索の準備と同じく準備 worker。stat・親 DB の open / lookup / save はすべて worker。

## 10. 簡素化の検討

- 採用: **複数の親を並べる一覧は必ず自分の pool を持つ**。Ctrl+G / Ctrl+R の借用をやめれば、借りたメモリのキーの
  空間・借りた DB への seed・宛先の別型 (r2 案の `Named`) が要らなくなる。単体ファイル以外は今の Fixed の処理のまま。
- 採用: **対象を単体ファイルのピンなしの表紙に絞り、分岐で決める**。フォルダ行の proof・再帰 mirror・seed、EPUB の
  世代、ピンの再帰を親 DB の側で扱わずに済む。
- 採用: **署名は通常一覧と同じ所有者で worker が確定**。一覧ごとに「署名が本物か」の宣言を作らない。
- 採用: **旧行は今と同じく読み、写さない**。移行の段・表示後の stat を作らない。親 DB に入るのは新しく生成した行と、
  アイドル時の高画質化で作り直した行だけで、共有の保証もこの範囲に限る (§2、r3 で合意)。
- 採用しなかった: 検索に入るたびに pool を作り直すだけ (元のフォルダの DB は汚さないが、代表選定との共有ができない)。
- 採用しなかった: 代表選定が検索の DB も引く (検索専用 DB に結果のパスが溜まり続け、§15 の方針と逆向き。代表選定が
  1 フォルダのために別 DB を開く)。
- 採用しなかった: 合成 DB の旧行を親 DB へ写す段階移送 (r2 指摘 3: 表示後の stat がオフラインで worker を塞ぐ)。
- 採用しなかった: ドライブ文字を保つ catalog identity への変更 (§6)。
- 採用しなかった (別判断): フォルダ行、コレクション root も親 DB へ寄せる (§8)。

## 11. 変わる挙動 (利用者から見て)

- 良くなる: Everything 検索・Ctrl+G・★一覧・履歴などで新しく作った (またはアイドル時の高画質化で作り直した)
  ZIP・PDF・画像のサムネイルが、親フォルダの代表サムネイルと親フォルダの一覧で使われる。フォルダを開いて作った
  サムネイルを、検索結果などが使う (開いたことのあるフォルダの項目は、検索結果の初回表示でも作り直さない)。
  更新前に★一覧などの合成 DB へ入っていた行は、そのまま★一覧などで使われ、親フォルダへは移らない
  (アイドル時の高画質化が有効なら、作り直したときに親フォルダにも入る)。
- 良くなる: 検索で表示した PDF のページ数などの情報が PDF の親フォルダのキャッシュに入り、Enter で開いたときの
  即時の仮表示が使える場面が増える (今は検索が PDF の親フォルダから始めたときだけ)。
- 良くなる: 検索を始めた元のフォルダの DB に行が入らない (戻ったときの削除も起きない)。Ctrl+G / Ctrl+R の
  2 回目以降の検索で、前回の結果のサムネイルが `__search_results__` から当たる (今は借りた pool のときは当たらない)。
- 費用: 他の一覧から Ctrl+G / Ctrl+R に入る 1 回目に、`__search_results__` の catalog を準備 worker で開いて読む
  (今の「pool が無いとき」と同じ処理)。メモリに無い単体ファイルの項目は、生成の前に worker が 1 回 stat し、
  親フォルダごとに catalog を開いて引く (§1.668 のコレクションと同じ作り。§1.668 の計測で 1 回の参照は約 0.07ms)。
  メモリで当たる項目の表示は今と同じ。
- キャッシュフォルダのファイル数: 検索で多くのフォルダにまたがって作ると、そのフォルダ数ぶん catalog DB が
  作られる (それらを順に開いたときと同じ)。既定のポリシー Auto で保存しない項目は DB を作らない。
- 不便になる既存の挙動: 無い (コードで追った範囲。§13 のテストで確かめる)。

## 12. 規模と出荷の見込み

- 本体の変更: 検索の pool の作り直し (`global_search_ui.rs` の `for_app`・採用、`app.rs` の pool の型の記録)、
  `ParentRow` (`make_load_request_with_visibility` の 3 分岐と受付)、`RequestCatalog` / provider の一般化
  (`catalog_provider.rs`)、worker の lookup・保存・PDF メタ補完の分岐 (`thumb_loader.rs`)。数百行と、横断のテスト。中規模。
- r1 の見積もり (対象を絞る前の案): 実装・テスト 3〜5 人日 + 独立レビュー・全件 gate・実機確認。r2・r3 で対象を絞り、
  宛先の別型も無くしたので減るが、provider は出荷済みの §1.668 の機構を、検索の owner の受け渡しは Everything の準備を触る。
- v5.1.0 (数日以内) に入れるかは利用者判断。設計担当の見立ては「v5.1.0 に間に合わせるより、v5.1.x で出す方が安全」。

## 13. テスト (実装時)

- 再現: 通常フォルダ A を開いた状態で Ctrl+R (fake run) / Ctrl+G → 結果の ZIP のサムネイル生成 → A の DB に行が無い・
  親 F01 の DB に `zipthumb:vol01.zip` (observed 署名) がある → F01 の親を開くと代表選定が `CachedPinned` で当たる。
  修正前のコードで落ちること。
- 逆向き: F01 を通常に開いて作った行を、検索結果が親 DB の lookup で使う (生成しない)。
- pool: 通常フォルダ・★一覧・コレクションから Ctrl+G / Ctrl+R に入ると pool が `__search_results__` になり、ストリーミング
  中の rebuild では作り直さない。検索を閉じると元の一覧が通常に読み込まれる。借りた pool の DB とメモリに検索の行・
  seed 行が入らない。兄弟 context の pool は変わらない。
- owner の受け渡し: 他の一覧から入った Everything の準備が、新しい `__search_results__` の owner に seed し、Ready の検証を
  通って採用される。Ready 待ちの間に pool が別の owner へ変わったら棄却して取り直す (今の再試行と同じ)。
- 分岐: `ParentRow` が付くのは表 (§5.1) のピンなしの 3 分岐 (Image / ZipFile / PdfFile) だけ。ピンあり・ConvertibleArchive・
  Folder・Stack・ZipImage・PdfPage・ZipDir・SearchContainer・Audio・Video には付かない (種類ごとの受付テスト)。
- 署名: お気に入り横断検索 (size 0) と閲覧履歴 (保存時の署名) でも、親 DB の行は observed で書かれ、通常一覧の行を
  上書きしない。EPUB は手順 0 の世代の署名で、合成 DB の旧行にもメモリで当たり、通常一覧の行とも一致する。
  RAW の follow-up は再開時に stat しない。stat できない項目は親 DB を要求の署名で読むだけで、書かない。
- オフライン再表示 (アイドル時の高画質化を OFF にして、問題が隠れないようにする): (a) 生成・保存、(b) 既存の親 DB の
  行に observed で当たる、のそれぞれの後に、表示範囲から外す (Evicted) → ドライブを外した状態で戻る → メモリから
  再表示される (要求の署名が size 0 のお気に入り横断検索でも)。
- PDF メタ: Parent で表紙を保存したとき・cache hit の catch-up で、PDF の親 DB に pdf_meta が入る (password の条件は今と同じ)。
- 順序: メモリで当たる項目は stat しない (stat の呼び出し回数を数える)。`SourceOnly` はメモリ・DB を引かず親 DB へ保存、
  `CacheOnly` は生成・保存をしない。clear を跨いだ `CacheOnly` の一回 RO 継続、clear を跨いだ要求の保存禁止。
- 互換: ★一覧の合成 DB の旧フルパス行が今と同じく表示される (EPUB を含む)。`SourceOnly` で作り直すと親 DB に入る。
  旧行で表示しただけでは親 DB に入らない (保証の範囲、§2)。
- 同名: F01・F02 の `vol01.zip` が別 DB に入る。ドライブルート直下の項目はルート DB にフルパスのキーで入る。
- 一覧の種類ごとの判定表 (`ThumbStorageScope`) のテスト。`SingleFolder` (通常フォルダ・ZIP の中・ドライブルート・
  コレクションの PhysicalSource) は今と同じ Fixed とキー。§1.731 の 3 テストが通ること。
- 割り切り: 親 DB の open 失敗でもメモリだけで表示され、ログが出る。
- 自動比率の seed と結果到着後の標本、idle upgrade (`UpgradeableCache`)、release check (実際の保存先を見る形に) の回帰。

## 14. 文書の更新 (実装時)

[catalog-design.md](catalog-design.md) (保存先の節)、[architecture-overview.md](architecture-overview.md)、
[async-architecture.md](async-architecture.md) (§1.668 の遅延 catalog の節を一般化)、
[everything-search-plan.md](everything-search-plan.md) (「捕捉した既存 thumbnail owner の catalog へ seed」の段落と pool の借用)、
[search-architecture.md](search-architecture.md) (動画ピン seed の段落)、[display-pipeline.md](display-pipeline.md) の §1.668 の段落。
マニュアル・製品ページは §15 のとおり変更なし。

## 15. 「検索語と結果は保存しません」との関係

- 製品ページ (`index.html`) と `privacy.html` は「Everything 検索の検索語と結果一覧は保存しない。サムネイルキャッシュ
  などは通常どおり PC 内に保存する」と書いている。変更後に親 DB へ書くのは、**その親フォルダを開いたときに作られる
  のと同じ通常のキャッシュ行** (ルート直下ならフルパスのキー) だけで、検索結果の並びや検索語は残らない。約束に沿う。
- `__search_results__` には今もフルパスのキーで結果のパスが入る (pool が無いとき)。変更後は単体ファイルの表紙は
  そこへ新しく入らず、フォルダ行などは今と同じく入る (サムネイルキャッシュとして、今の公開文言の範囲)。
  既存の旧行は消さない (キャッシュの削除・期限で消える)。
- 公開文書の文言は変えない (偽になる文は無い)。
