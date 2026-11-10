# §1.718 本のタイルの表紙を、表紙ページの回転に合わせる (設計)

- 状態: 設計 (2026-11-10)。前回 (2026-11-06) は設計段階で止めた。経緯・退けた案・前回の指摘は
  [next-release-backlog.md §1.718](next-release-backlog.md) と下の §8。
- 範囲: 一覧の本のタイル (Folder / ZipFile / PdfFile (EPUB 含む) / ConvertibleArchive / ZipDir /
  SearchContainer / Stack)、詳細表示の hover サムネ、mIV Remote の本のタイル。
- 既定の割り切り: まれな失敗 (DB の読み書き失敗など) はログを残して回転なしで描く。

## 1. 症状と原因

- 観測 (サブ PC 担当、2026-11-05): `sideways-train.zip` の表紙 `001.png` を R で正立させた後も、本の一覧のタイルが横倒しのまま。
  本の中のページ一覧では正立していた。
- 原因 (コード): 一覧の回転は描画時に `App::get_rotation(idx)` → `rotation_key_for_idx` から当てる
  (`grid_paint.rs` の `draw_thumb_texture`、`draw_rotated_image`)。キーを返すのは Image / ZipImage / PdfPage / Video だけで、
  本の item は常に回転なし。本のタイルの画素が「どのページから作られたか」は、作った worker しか知らず、
  キャッシュ行にも `Loaded` にも残っていない。

## 2. 方針 (前回 Astra 推奨の作りを採る)

1. **サムネを作る経路が、画素と一緒に「その画素の表紙ページ」(`CoverIdentity`) を返す。**
   待ち行列・メモリキャッシュ・永続キャッシュ行・`ThumbnailState::Loaded` まで、画素と同じ素材として運ぶ。
2. **角度はサムネ要求に載せない。描画時に、在メモリの「ページキー→角度」から引く。**
   起動時に読んでいる全非 0 回転キーの集合 (`rotation_page_keys`) を、角度も持つ単一の owner に広げる。
3. 回転を変えても画素は作り直さない。描画が次のフレームで新しい角度を引くだけ。
   要求の revision、遅着結果の照合、描画中の evict は要らない (前回指摘 P2-1・P2-4・P2-5 の前提そのものを消す)。

### 2.1 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- **画素に回転を焼き込む案は採らない** (2026-11-10 の調査で確定)。仮想フォルダの seed / writeback (本の行⇔先頭ページの行のコピー)、
  ドライブ一覧の seed (ページ行→本の行のコピー)、Stack (ページ行そのものを使う)、編集プレビュー (ページ単位の cache) が
  本の行とページの行の間で画素を相互にコピーしており、焼き込むと二重回転か回転の消失が起きる。
  また単一キーの削除には書き込み途中の worker を止める epoch が無く、古い角度の行が書き戻される。
- **回転済みの本だけキャッシュを避ける案は採らない** (前回 Astra)。子キャッシュ再利用と CacheOnly の契約を変える。
- **回転変更時にタイルを作り直す処理を持たない**。描画時に引くので、identity が分かっている限り「変更に追従させる処理」が要らない。
  作り直しが要るのは、旧版が書いた identity の無い行 (§5) だけで、これは 1 行につき 1 回で収束する。
- 角度は「キー→角度」の 1 つの owner に置き、既存の存在集合と別の綴りを持たない (§4.1)。

## 3. `CoverIdentity`

```rust
pub(crate) enum CoverIdentity {
    /// 旧版が書いた行 (cover_key 列が NULL / 列が無い)。由来が分からない。
    Legacy,
    /// 今の版が作ったが、由来のページを特定できない (旧行の子キャッシュを流用した等)。
    Unknown,
    /// 回転できない素材 (本棚の本のページ、音声のアート、アイコン等)。
    Unrotatable,
    /// 回転 DB のページキー (`page_key_for_grid_item` と同じ体系、動画は normalize_path)。
    Page(String),
}
```

- 描画: `Page(k)` だけが回転を持つ。それ以外は回転なし (今の挙動)。
- `Loaded` に入れるときだけ、`Legacy` に `scope: Option<CoverSourceScope>` (その結果を出した要求の表紙の出どころの範囲) を付ける (§5.0・§5.2)。
  永続表現と worker の結果では付けない。
- `Legacy` と `Unknown` を分けるのは、`Legacy` だけを作り直しの対象にするため (§5)。`Unknown` を作り直し対象にすると、
  作り直しても `Unknown` に戻る経路 (旧行の子の流用) で毎回作り直しが起きる。
- 永続表現 (`thumbnails.cover_key TEXT`): NULL = `Legacy`、`"u"` = `Unknown`、`"n"` = `Unrotatable`、`"p:<key>"` = `Page`。
  変換は 1 か所 (`CoverIdentity::{to_sql, from_sql}`) に置く。

### 3.1 ページキーへの写し方 (worker)

- 既存の `edit_source::caption_rotation_key(&ReadLocation, books_root)` を、§1.717 の説明文と共通の
  `cover_rotation_key` として使う (名前を一般化してよい)。File / ArchiveEntry / PdfPage はページキー、
  本棚の本のページ (`is_direct_book_folder`) は `Unrotatable`。
- books_root は App が設定値 (明示パスか既定か) だけを要求に載せ、既定パスの解決 (`SHGetKnownFolderPath`) は worker 側で行う。
  UI スレッドの要求作成で `book_root_path()` を呼ばない。説明文 worker (`book_cover_env.books_root`) の解決と同じ扱いにする。
- 変換書庫 (RAR 等): 読み先は cache ZIP の entry で、本の中のページ一覧 (`tree.zip_path`) と同じキーになる。
  Direct (cache ZIP を作らない) 読みのキーが本の中の表示と一致するかは実装時にコードで確かめ、一致しなければ報告する。

## 4. App 側

### 4.1 角度の owner (`rotation_page_keys` の拡張)

- `rotation_page_keys: BTreeSet<String>` を、角度も持つ **1 つの map** (「非 0 回転のキー→角度」、`BTreeMap<String, Rotation>`)
  の owner 型に置き換える。集合と map を並べて持たない。既存の存在判定 (`contains`、prefix 範囲、`item_has_page_edit_key` 系の
  汎用 helper) は map のキーで今までどおり使う (汎用 helper が `&BTreeSet<String>` を取るなら、キーの範囲を引ける形へ広げる)。
- 起動時の `load_rotated_keys` は角度も読む (`SELECT path, angle ... WHERE angle != 0`)。
- **全ての書き手が角度を渡す**。型を変えると in-process の書き手はコンパイルエラーで列挙されるが、**DB だけを書いて owner を
  触らない書き手はコンパイラでは見つからない**ので、DB の書き手 (`RotationDb::set*` / `clear_all` / restore store のコピー) から
  逆にたどって確かめる。今のコード上の書き手:
  `apply_rotation` / book_structural_op の移動・削除 / content_identity_restore / sidecar_restore と metadata import の snapshot
  (`ImportPageStateSnapshot.rotated` 等を「キー→角度」に) / リネーム / metadata cleanup / 全解除 / 再読み込み (app.rs の reload) /
  フォルダ削除時の retain / `edit_bundle_bulk`。snapshot を作る worker は DB から角度も読む。
- 画像フォルダ→ZIP 変換は、完了時に既に `page_state=true` の metadata refresh で全体 snapshot を読み直して owner を置き換えている
  (`image_folder_zip_operation.rs` → `metadata_import_refresh.rs`)。この snapshot が角度を持つようにするだけで足りる。
- **EPUB→PDF 変換は今回の範囲から外す** (既知の制限、[バックログ §1.757](next-release-backlog.md))。`epub_convert.rs` の
  `copy_restore_stores_without_identity_at` (単発・一括) は回転を PDF のキーへ DB だけでコピーし、今の `rotation_page_keys` も更新しない。
  これは変換機能に元からある不足で、今回の修正で悪化しない (変換後の PDF の本のタイルが回転しないのは今と同じ。次回起動で直る)。
  - 外す理由: 正しく直すには、変換 worker の DB コピーと、利用者の回転操作・snapshot の置き換えなど owner の全ての更新の間に
    順序を付ける仕組みが要る (ダイアログが置き換わっても worker はコピーを続けるため、ダイアログの寿命では順序を守れない。
    2026-11-10 の設計レビュー 3 回目)。§1.718 の本体 (表紙の identity と描画時の角度) とは独立した、変換機能側の構造の問題。
  - 他の書き手は、画面の操作か全体 snapshot の置き換えで owner を更新しており、この問題は無い。
- 回転の実変更を通知する helper を 1 つにまとめる (owner の更新 + 説明文の失効 + §5.2 の旧行タイルの再要求)。
  snapshot の読み直しで値が変わらなかったキーについては失効を出さない。

### 4.2 描画時の角度 (`grid_tile_rotation(idx)`)

- ページ item (Image / ZipImage / PdfPage / Video): 今の `get_rotation(idx)` のまま。意味を変えない。
- Stack: 代表画像のページキー (UI が既に知っている) で owner を引く。Stack の画素は代表画像のページ行そのもの。
- SearchContainer: 代表 (`representative.path` / entry / page) のページキーで owner を引く。
- それ以外の本 (Folder / ZipFile / PdfFile / ConvertibleArchive / ZipDir): `Loaded` の `CoverIdentity` が `Page(k)` なら owner から k の角度、
  それ以外は回転なし。`Loaded` でない間は描くものが無い (placeholder)。
- `get_rotation` / `rotation_key_for_idx` の本 item に対する意味 (None) は変えない。回転操作・説明文・見開き判定などはそのまま。
- 呼び出し元 (全描画入口): 一覧の 2 か所 (`ui_main.rs` の grid 描画)、`repaint_grid_cell_after_input`、詳細表示の hover サムネ
  (`painter.image` を直接呼ぶ箇所)。hit-area / overlay の計算が回転後の寸法を使う箇所があれば、描画と同じ値を使う。
- **詳細表示の hover は、ページ item も含めて owner だけを引く** (メモリのみ)。`get_rotation` は cache miss で同期 SELECT をするので
  hover から呼ばない。owner は、§1.757 (EPUB→PDF 変換でコピーした PDF のキー) を除く全非 0 回転を持つので、キーが無ければ
  0° として描く。§1.757 の PDF のキーは次回起動 (または全体 snapshot の読み直し) まで owner に無く、その間は本のタイル・hover・
  旧行の作り直しの判定が回転なしとして扱う (ページ一覧のタイルは `get_rotation` の DB 読みで回る)。hover の表示枠と配置の寸法も
  90°/270° で縦横を入れ替える (最後の mesh だけでなく)。ページ item の hover が回るようになるのは今回の修正に含める
  (一覧と hover の向きを揃える)。
- 新しく足す描画経路は DB を引かない。

### 4.3 `Loaded` と CPU 待ち行列

- `ThumbPixels` に `cover: CoverIdentity` を足し、GPU 採用前の画像回収・`fs_upload_backlog`・`thumb_pixels` を通して
  `ThumbnailState::Loaded { cover, .. }` に入れる。テクスチャと同じ値として clone / fork / Ctrl+G の使い回しで一緒に動く。
- idle upgrade (SourceOnly) の結果も新しい identity で `Loaded` を置き換える。

## 5. 旧版が書いた行 (`Legacy`) の扱い

キャッシュはリリース済み。旧行は全部 `Legacy` として読む (列の追加は §6)。旧行を一斉に作り直すと、更新後の初回の一覧表示が
本の数だけ遅くなるので、作り直すのは「回転済みのページを含む本」だけにする。

### 5.0 判定は 1 つ: 表紙の出どころの範囲 `CoverSourceScope`

- 表紙の出どころになり得るページキーの範囲 `CoverSourceScope` を、**要求を組み立てる処理の中で、元のパスと種類がまだ手元にある
  時点で**作り、要求と一緒に持たせる (`make_load_request_with_visibility` と `apply_folder_thumb_pin` が読み先を書き換える箇所)。
  組み立て終わった要求のフィールドからは作らない (変換書庫の元のパス、pin の `resolved.abs_path`、ZipDir の dir prefix は
  要求に残らないため)。pin の解決 (pin DB と stat を読む既存処理) をやり直さない。新しい I/O は足さない。
  - 例: 自動選定のフォルダ → そのフォルダ以下の prefix。ZIP / PDF / EPUB → その容器のページキーの prefix。
    変換書庫・変換書庫を指す pin → 読み先 (cache ZIP) と元の書庫の両方の綴り (`archive_or_converted_cache_contains_page_key` と同じ扱い)。
    ZipDir → ZIP のキー prefix + dir prefix (pin なしでも、要求作成時の dir prefix から作る)。具体的なページを指す pin → そのページのキー。
  - **代替要求 (`epub_pin_fallback`) を持つ要求の範囲は、本体と代替要求の範囲の和** (再帰的に)。worker は除外された書庫の pin や
    読めない EPUB で代替要求に切り替えるので、代替要求の表紙 (例: 入れ子の pin から外側のフォルダの `cover.png`) も範囲に入れる。
  - **元ソースを decode する経路が無い要求は範囲を持たない (`None`)**: CacheOnly、動画フレームの seed (`uses_memory_pin_seed`)、
    ドライブ一覧の `pinned_only` など。`allows_source()` だけでは判定しない。
- 判定 `scope.has_rotation(owner)` = 範囲内に owner の非 0 回転キーがあるか (BTreeMap の範囲検索)。§5.1・§5.2・採用時の 3 か所で
  同じ関数を使う (変更されたキーではなく、今の owner 全体で判定する)。
- 範囲は要求 owner (`ThumbnailRequests` の request) に `begin` の時点で不変の事実として持たせ、待ち行列・アップロード待ちの間も保つ。
  採用時にそこから読み、`Loaded` の `Legacy` に移す (§5.2)。viewer context ごとの要求から作るので、別 context の pin や変換の状態を混ぜない。

### 5.1 要求時

- App は本の要求を組み立て終えた後、§5.0 の範囲 (代替要求との和) が `Some` でその範囲に回転があるとき
  `LoadRequest.refresh_legacy_cover = true` にする。代替要求 (`epub_pin_fallback`) にも同じ値を引き継ぐ。
- worker はキャッシュ命中 (**メモリ命中の判定より前**に。ディスクも同じ) で行が `Legacy` かつ `refresh_legacy_cover` のとき、
  その旧行の画素を手元に残したまま、元ソースから作り直しを試みる。§1.741 の親 catalog 行も同じ順序で判定し、親の stamp 観測と
  保存先は通常の miss と同じにする。
  - **元ソースを読めない (オフラインのドライブ、消えたファイル等) ときは、残しておいた旧行の画素を今までどおり返す** (identity は `Legacy`)。
    §1.741 の「元ソースが無くても保存済みの表紙を出す」契約を変えない。行は書き換えない。
- **作り直した結果は、その旧行があった保存先に旧行を置き換えて保存する。保存方針 (`CachePolicy::Off`、Auto の閾値) にはよらない**
  (`force_cache` と同じ扱い。既存の admission / retire の検査はそのまま通す)。既にある行を新しい形式に書き換えるだけなので、
  これで 1 行につき 1 回で収束する。
  - 例外: §1.741 の一覧で、旧行が synthetic の Fixed DB からメモリへ読まれただけで、親 catalog に行が無い場合は、強制保存しない
    (新しい親 DB を作らない)。通常の保存方針に従う。`CachePolicy::Off` ならその一覧を開くたびに作り直す (Off の通常の decode と同じ費用)。
  - 保存に失敗した場合はまれな失敗として割り切る (ログ。次に要求されたときにもう一度作り直す)。
- フォルダ代表の選定が子の保存済み行を流用する (`ChildCached`) とき、子の行が `Legacy` なら親の identity は `Unknown`
  (子が pin で具体的なページを持つなら、その `Page`)。親の行は `Unknown` で保存され、作り直しの対象にならない。
  子の行が後で作り直されると、子のカタログの revision が上がり、親の行の proof が失効して選び直される (既存の仕組み)。

### 5.2 表示中のタイルと、遅れて届く結果

- `Loaded` の identity は `Legacy { scope: Option<CoverSourceScope> }` として持つ。`scope` は、その結果を出した要求の範囲 (§5.0)。
  採用時に要求 owner の事実から移す。CacheOnly・seed など decode 経路の無い要求の結果は `scope = None`。
- 回転の実変更 (§4.1 の helper) のたびに、全 viewer context の `Loaded` で `Legacy { scope: Some(s) }` の本のタイルのうち、
  `s.has_rotation(owner)` が真のものだけを `evict_thumbnail_for_reload` し、`request_repaint` する。
  次のフレームの要求作成で `refresh_legacy_cover=true` の要求が出て作り直される。
  既存の `evict_pinned_adjustment_materializations` (全 context を回る) と同じ形にする。
- **採用時にも同じ判定をする**: 結果が `Legacy` で、その要求の `refresh_legacy_cover` が false (要求 owner の事実)、範囲が `Some(s)` で、
  今は `s.has_rotation(owner)` が真なら、採用せずに既存の要求 owner を通して再要求する。読み込み中・CPU 待ち行列・暫定画像の
  段階で別 context が表紙を回した場合を、これで拾う (回転の revision は要らない)。
  `refresh_legacy_cover` が true だった要求の `Legacy` 結果 (元ソースが読めず旧行の画素を返した場合) は、そのまま採用する。
  これで「読めない → 再要求 → 読めない」の繰り返しは起きない (次の機会は次の回転操作か、一覧の開き直し)。
- これは描画中ではなく、回転を変えた操作の処理と結果の採用の中で行う。`Legacy` 以外のタイルは触らない (表紙以外の回転で
  無関係なタイルが消えない)。作り直した行は `Legacy` でなくなるので、同じタイルが繰り返し再要求されることはない。

### 5.3 残る既知の制限 (旧行に限る)

- CacheOnly の経路 (ドライブ一覧、判定待ちの変換書庫) が旧行を表示する間は回転しない。行が通常の要求で作り直されると直る。
- 旧行の子を流用したフォルダの代表は、その子の行が作り直されるまで回転しない (子の本を含む一覧を開けば作り直される)。
- mIV Remote は旧行を回転なしで返す (§9)。デスクトップで作り直されると直る。

## 6. 永続キャッシュ (catalog) の変更 (リリース済み → 移行あり)

- `thumbnails` に `cover_key TEXT` を `add_thumbnail_column_if_missing` (既存の source_width 等と同じ方式) で足す。
  旧行は NULL。**`CATALOG_VERSION` は上げない** (上げると全サムネ削除)。
- `init_schema` を通らない read-only の読み手 (フォルダ代表の選定、Collection、maintenance 後の cache-only 継続) は、
  既存の layout 列と同じく列の有無を判定し、無ければ NULL (= `Legacy`) として読む。`load_all` / `load_one` / prefix 読みの全部。
  Collection の必須列には足さない (旧 cache を丸ごと拒否しない)。
- `CacheEntry` に `cover: CoverIdentity` を足し、メモリキャッシュ (`cache_map`) でも画素と一緒に持つ。
- 行を書く全経路で identity を書く (§7)。trigger (`folder_selection_revision`) の対象は今と同じ。

## 7. identity を作る経路 (全 producer)

| 経路 | identity |
| --- | --- |
| キャッシュ miss の decode (`resolve_read_after_cache_miss` → `load_one_cached`) | 実際に読んだ `ReadLocation` から §3.1。ページ item の行にも書く (Stack・仮想フォルダの seed と一致させるため) |
| キャッシュ命中 (メモリ・ディスク) | 行の `cover_key`。§5.1 の作り直し判定 |
| `ChildCached` (子の行の流用、`save_auto_folder_bytes`) | 子の行の identity。子が `Legacy` なら `Unknown` (pin の具体ページがあればその `Page`) |
| 動画フレームの seed (`seed_folder_video_pin_thumbs`、smart_folder の seed、metadata refresh の pin 確定 `pin_materialization.rs` → `commit_pin_materializations`) | `Page(normalize_path(video))`。既存行と bytes / stamp が同じでも identity が違えば書き直す (「変わっていない」判定に identity を含める。旧 seed 行は 1 回だけ書き直される) |
| ドライブ一覧の seed (`drive_list_pin_seed_entry` 系) | **コピー元の行の identity をそのまま写す** (ページ行からでも本の行からでも)。コピー先の名前からページを推測しない |
| ドライブ一覧の暫定→確定 (bytes が同じなら Finalized だけ送る経路) | bytes と identity が両方同じときだけ Finalized だけにする。違えば画素ごと送る |
| 仮想フォルダの seed (本の行→先頭ページ行) と writeback (ページ行→本の行) | **コピー元の行の identity をそのまま写す**。seed は「本の表紙」を「一覧で先頭のページ」の行へ写すが、ZIP の格納順と一覧順が違えば別ページの画素になる (`book-thumb-cover-only-plan.md` に既知)。行の名前から identity を付け替えると、別ページの角度が付く |
| 一括キャッシュ作成 (`app/cache_ops.rs` の ZIP のページ行・本の表紙行、PDF / EPUB の表紙行、共通 encoder `thumb_loader.rs` の直接保存) | 実際に encode したファイル / entry / ページのキー。ZIP は取り出した entry 名を保持して渡す |
| 音声のアートの直接 SQL (`catalog.rs` の audio-art 保存) | `Unrotatable` |
| 編集プレビュー (`edit_preview_key` / 手動代表の `edit_preview_validate_container`) | `edit_preview_key` のページ |
| RAW half の再投入 | 再投入する解決済みの読み先 (通常の miss と同じ) |
| EPUB / 種類除外の代替要求 | 再帰した内側の要求が作る identity をそのまま返す |
| 音声のアート、アイコン等の画素 | `Unrotatable` |
| mIV Remote の container (`remote_ipc/container.rs` の `load_one_cached`) | 通常の miss と同じ。同じ catalog に書くので identity を書く |

## 8. 前回のレビュー指摘への対応 (2026-11-06 の Sol/xhigh 2 回目)

1. 要求時の回転 revision: 角度を要求に載せないので該当しない。
2. 旧行の移行 (ChildCached・動画 seed・cache-only): §5・§7 のとおり種類ごとに決めた。作り直せない経路は既知の制限として §5.3 に書く。
3. 通常経路以外の producer と派生要求: §7 の表。`refresh_legacy_cover` は代替要求にも引き継ぐ。暫定→確定は identity も比較。
4. 描画中の evict: 描画では evict しない。§5.2 の再要求は回転操作の処理の中で行い `request_repaint` する。
5. 全体 revision による無関係タイルの evict: revision を持たない。evict は `Legacy` かつ変更キーを含む本だけ。
6. read-only の読み手: §6。
7. Stack: §4.2 で代表のページキーを使う。SearchContainer は代表の path / entry / page を使う。
8. identity を `Loaded` に束ねる: §4.3。`Legacy` / `Unknown` / `Unrotatable` を区別する。

### 8.1 2026-11-10 設計レビュー (Sol/xhigh) の指摘と対応

判定は「指摘を直せば実装可」。P2 × 8、P3 × 1。全件をコードで確かめ、妥当と判断して反映した。

1. EPUB→PDF 変換の回転コピーが owner に届かない → §4.1。画像フォルダ→ZIP と同じ metadata refresh の経路に乗せる。
2. seed / writeback でコピー先のページの identity を付けると、別ページの角度が付く → §7。コピー元の identity を写す。
3. 一括キャッシュ作成・pin 確定・音声のアートの直接 SQL が producer 表に無い → §7 に追加。
4. 回転の変更後に届く `Legacy` 結果 → §5.2。採用時にも同じ判定をする。
5. 保存方針で作り直しが保存されず収束しない、CacheOnly の旧行を evict し続ける → §5.1 (既存行の置き換えは保存方針によらない)、
   §5.2 (`Legacy { refreshable }`)。
6. 変換書庫を pin したフォルダが判定から漏れる → §5.0 (実効の読み先を含む述語を 1 つにまとめる)。
7. Remote の identity の受け渡しと反映の時期が具体的でない → §9。
8. hover から `get_rotation` を呼ぶと同期 SELECT の経路が増える → §4.2 (hover は owner だけを引き、寸法も入れ替える)。
9. (P3) 画像フォルダ→ZIP は既に snapshot で owner を置き換えている → §4.1 の記述を直した。

### 8.2 2026-11-10 設計の再レビュー (同じ Sol/xhigh) の指摘と対応

判定は「指摘を直せば実装可」。P2 × 5、P3 × 1。元の 9 件のうち 2・3・8・9 は解決、1・4・5・6・7 は一部が残っていた。

1. 実効の読み先を在メモリで解決できるという前提が誤り (pin の解決は pin DB と stat を読む) → §5.0。解決をやり直さず、
   組み立て終わった要求から範囲を作る純粋な関数にした。
2. 元ソースを読めない旧行を捨てると、オフラインの表紙が消える → §5.1。旧行の画素を残して作り直しを試み、読めなければ旧行を返す。
3. EPUB 変換の完了は画面の置き換えで捨てられる → §4.1。変換 worker から App 全体の受け口へ直接送る。
4. 要求 owner が要求の事実を持っていない → §5.0・§5.2。`begin` で範囲と `refresh_legacy_cover` を不変の事実として持たせる。
   decode 経路の無い要求 (動画フレームの seed 等) は範囲 `None`。
5. Remote の完了済み受け渡しが旧行を使い回す → §9。`Legacy` の完了済み raster は使い回さない。
6. (P3) synthetic のメモリ行からの作り直しで新しい親 DB を作ってしまう → §5.1 の例外。

### 8.3 2026-11-10 設計レビュー 3 回目 (同じ Sol/xhigh) の指摘と対応

判定は「指摘を直せば実装可」。P2 × 2、P3 × 1。§8.2 の 2・4・5・6 は解決。

1. 範囲に代替要求の出どころが入っていない → §5.0。本体と代替要求の範囲の和にした。
2. EPUB 変換の受け口では、DB のコピーと owner の他の更新の順序が守れない → §4.1。順序を付ける仕組みは owner の全書き手に及ぶので、
   EPUB→PDF 変換の回転の反映は今回の範囲から外し、変換機能側の既存の不足として別項目にした (今回の修正で悪化しない)。
3. (P3) 組み立て終わった要求には変換書庫の元のパスや ZipDir の prefix が残らない → §5.0。要求の組み立て中に範囲を作る。

### 8.4 2026-11-10 確認ラウンド (同じ Sol/xhigh)

判定は「文書の修正だけで実装可」。P1・P2 なし。§8.3 の 1・3 は解決。EPUB→PDF を §1.757 へ外すことは §1.718 として受け入れ可
(identity は角度の取得と独立で、owner に無いキーは誤った回転を作らない)。P3 1 件: owner が全非 0 回転を持つという記述と、
変換直後の受け入れ条件が除外と矛盾 → §4.2 と §10 を直した。

## 9. mIV Remote

- 今の Remote は、サムネの結果から画素だけを取り出して identity を捨てている (`remote_ipc/thumbnail.rs` のフォルダ、
  `remote_ipc/container.rs` の container、`RemoteSourceRaster`)。container の回転は読み込み前に決めている。
- 変更: `RemoteSourceRaster` (と folder の結果) に `CoverIdentity` を通す。本のタイル (container の ZIP / PDF の表紙、ZipDir、フォルダ) は、
  **読み込んだ後に** identity が `Page(k)` なら k の角度を Remote の worker 側で RotationDb (read-only) から引いて回す。
  ページのタイルの回転 (`rotation_for_remote_page`) は今のまま。本のタイルでは容器自身のパスのキーで回さない。
- Remote は旧行の作り直し (`refresh_legacy_cover`) をしない。`Legacy` は回転なしで返し、デスクトップ側で行が作り直されると直る
  (§5.3 に追記)。Remote が作る新しい行には identity を書く。
- `RemoteSourceSingleFlight::acquire` は、完了済みの受け渡し (raster) を source identity が同じなら使い回す。identity が `Legacy` の
  完了済み raster は使い回さない (毎回 catalog を読み直す)。使い回す raster の identity は、その画素と一緒に保存したものだけを使い、
  新しく読んだ identity を古い画素へ付けない。
- 回転の変更が Remote のブラウザに届くのは、ページのタイルと同じく、キャッシュ期限が過ぎた**後の次の取得** (一覧を開き直す等)。
  期限が過ぎただけでは表示中の画像は貼り替わらない。
- ブラウザへの反映: サムネの ETag は無い (`/api/thumb` は `private, max-age=60`、一覧の URL は session epoch 付き)。
  回転の DB 変更で Remote の state generation は進むが、表示中の一覧のサムネは貼り直さない。**ページのタイルの回転と同じ
  タイミング** (一覧を開き直す・キャッシュ期限) で反映される形に揃え、本のタイルだけの再取得の仕組みは足さない。
  これは今のページのタイルの挙動と同じであることをテストではなく文書に書く。

## 10. テスト

- catalog: 列の無い旧 schema の DB に列が足されて旧行が `Legacy` で読めること。read-only の読み手が列の無い DB を読めること。
  identity の書き込みと読み戻し (4 種類)。`CATALOG_VERSION` が変わらないこと。
- producer: ZIP (先頭・pin)、PDF・EPUB (先頭・pin)、変換書庫、フォルダ (直下の画像・depth・ChildCached・旧行の子)、ZipDir (pin あり・なし)、
  動画フレームの seed、ドライブ一覧の seed と暫定→確定、仮想フォルダの seed / writeback、編集プレビューの手動代表で、
  返す identity が本の中のページのキーと一致すること。本棚の本は `Unrotatable`。
- App: ZIP の表紙を 90° → 本のタイルが 90°。2 ページ目の回転ではタイルが変わらず evict も起きない。全解除で 0°。
  pin で表紙を替えると新しい表紙の角度。`Legacy` の行は回転済みのページを含む本だけ作り直され、CacheOnly では作り直さない。
  表示中の `Legacy` タイルは表紙の回転で 1 回だけ再要求される。`Unknown` は再要求されない。画像フォルダ→ZIP 変換の後も owner に角度が入る。
  別 viewer context (別ウィンドウ) の同じ本のタイルも追従する。UI スレッドで RotationDb を読まないこと。
- 追加 (設計レビュー 2026-11-10): 格納順と一覧順が違う ZIP で、仮想フォルダの seed / writeback を経ても表紙の identity が
  実際の画素のページのままであること。変換書庫 (RAR / 7z) の表紙を pin したフォルダで、表紙の回転が判定・再要求に掛かること。
  (EPUB→PDF 変換の直後の確認は §1.757 へ移した。) `CachePolicy::Off` でも旧行の作り直しが
  1 回で収束すること。別 context で読み込み中・CPU 待ち行列・暫定画像の段階の `Legacy` 結果が、表紙の回転後に採用されず再要求されること。
  CacheOnly で届いた `Legacy` は回転操作で evict されないこと。一括キャッシュ作成と pin 確定の行に identity が入ること。
- 追加 (再レビュー): 元ソースが読めない回転済みの本 (旧行あり) で、旧行の表紙が表示され続け、再要求が繰り返されないこと。
  入れ子の pin が代替要求のフォルダの表紙へ切り替わる場合 (既存の `book.cbz::inner.zip/page.png` → `cover.png` の fixture)、
  `cover.png` だけの回転でも範囲に入ること。pin なしの ZipDir と変換書庫を指す pin の範囲。Remote で同じ本の表紙を 2 回取得し、その間に
  デスクトップが旧行を作り直した場合、2 回目は回転した表紙になること。§1.741 の synthetic のメモリ旧行の作り直しで、
  `CachePolicy::Off` のとき親 DB が作られないこと。動画フレームの seed の要求は範囲 `None` になること。
- 描画入口: 選択変更後の再描画 (`repaint_grid_cell_after_input`) と詳細表示の hover でも回転すること。hover が RotationDb を読まないこと。
- Remote: 本のタイルが表紙の角度で返ること。
- 既存の UI snapshot は変わらない想定 (変わるなら意図を確かめて更新)。

## 11. 文書

- [display-pipeline.md](display-pipeline.md) の回転の表 (「回転 (DB) | 描画時の GPU 行列」) に、本のタイルは素材の `CoverIdentity` から引くことを足す。
- [catalog-design.md](catalog-design.md) に `cover_key` 列と `Legacy` の扱い。
- Remote の本のタイルの回転を [web-remote-plan.md](web-remote-plan.md) の該当節へ。
- バックログ §1.718 の状態。製品ページ・マニュアルは回転の説明を確かめ、本のタイルに触れていなければ変更しない。
