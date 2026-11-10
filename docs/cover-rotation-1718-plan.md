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

- `rotation_page_keys: BTreeSet<String>` を、角度も持つ owner 型 (例 `RotationKeyIndex`) に置き換える。
  中身は「非 0 回転のキー→角度」。既存の存在判定 (`contains`、prefix 範囲、`item_has_page_edit_key` 系の汎用 helper) は
  owner の `keys()` 相当で今までどおり使う。集合と角度を別々に持つ場合は、owner の method だけが両方を更新し、常に一致させる。
- 起動時の `load_rotated_keys` は角度も読む (`SELECT path, angle ... WHERE angle != 0`)。
- **全ての書き手が角度を渡す**。型を変えるので、書き手はコンパイルエラーで全部列挙される。今のコード上の書き手:
  `apply_rotation` / book_structural_op の移動・削除 / content_identity_restore / sidecar_restore と metadata import の snapshot
  (`ImportPageStateSnapshot.rotated` 等を「キー→角度」に) / リネーム / metadata cleanup / 全解除 / 再読み込み (app.rs の reload) /
  フォルダ削除時の retain / `edit_bundle_bulk`。snapshot を作る worker は DB から角度も読む。
- 画像フォルダ→ZIP 変換の metadata コピー (`image_folder_zip_metadata.rs` → `image_folder_zip_operation.rs` の完了側) は、
  今は回転キーを owner へ反映していない。コピーした回転 (キー→角度) を完了時に owner へ反映し、説明文の失効
  (`caption_display.invalidate_rotation_keys`) も同じ helper で行う。
- 回転の実変更を通知する helper を 1 つにまとめる (owner の更新 + 説明文の失効 + §5.2 の旧行タイルの再要求)。

### 4.2 描画時の角度 (`grid_tile_rotation(idx)`)

- ページ item (Image / ZipImage / PdfPage / Video): 今の `get_rotation(idx)` のまま。意味を変えない。
- Stack: 代表画像のページキー (UI が既に知っている) で owner を引く。Stack の画素は代表画像のページ行そのもの。
- SearchContainer: 代表 (`representative.path` / entry / page) のページキーで owner を引く。
- それ以外の本 (Folder / ZipFile / PdfFile / ConvertibleArchive / ZipDir): `Loaded` の `CoverIdentity` が `Page(k)` なら owner から k の角度、
  それ以外は回転なし。`Loaded` でない間は描くものが無い (placeholder)。
- `get_rotation` / `rotation_key_for_idx` の本 item に対する意味 (None) は変えない。回転操作・説明文・見開き判定などはそのまま。
- 呼び出し元 (全描画入口): 一覧の 2 か所 (`ui_main.rs` の grid 描画)、`repaint_grid_cell_after_input`、詳細表示の hover サムネ
  (`painter.image` を直接呼ぶ箇所。ページ item も今は回転していないので、同じ helper で揃える)。
  hit-area / overlay の計算が回転後の寸法を使う箇所があれば、描画と同じ値を使う。
- 描画は DB を引かない。owner は在メモリ。

### 4.3 `Loaded` と CPU 待ち行列

- `ThumbPixels` に `cover: CoverIdentity` を足し、GPU 採用前の画像回収・`fs_upload_backlog`・`thumb_pixels` を通して
  `ThumbnailState::Loaded { cover, .. }` に入れる。テクスチャと同じ値として clone / fork / Ctrl+G の使い回しで一緒に動く。
- idle upgrade (SourceOnly) の結果も新しい identity で `Loaded` を置き換える。

## 5. 旧版が書いた行 (`Legacy`) の扱い

キャッシュはリリース済み。旧行は全部 `Legacy` として読む (列の追加は §6)。旧行を一斉に作り直すと、更新後の初回の一覧表示が
本の数だけ遅くなるので、作り直すのは「回転済みのページを含む本」だけにする。

### 5.1 要求時

- App は本の要求を作るとき、その本が回転済みのページを含むか (`item_has_page_edit_key(idx, rotation keys, include_descendants=true)`
  と同じ在メモリ判定、変換書庫は `archive_or_converted_cache_contains_page_key`) を `LoadRequest.refresh_legacy_cover: bool` に載せる。
  I/O なし。EPUB / 種類除外の代替要求 (`epub_pin_fallback`) にも同じ値を引き継ぐ。
- worker はキャッシュ命中 (メモリ・ディスクとも) で行が `Legacy` かつ `refresh_legacy_cover` のとき、`source_policy.allows_source()`
  なら miss として作り直す (作った行には identity が入る)。CacheOnly ではそのまま使う (回転なし、今の挙動)。
- フォルダ代表の選定が子の保存済み行を流用する (`ChildCached`) とき、子の行が `Legacy` なら親の identity は `Unknown`
  (子が pin で具体的なページを持つなら、その `Page`)。親の行は `Unknown` で保存され、作り直しの対象にならない。
  子の行が後で作り直されると、子のカタログの revision が上がり、親の行の proof が失効して選び直される (既存の仕組み)。

### 5.2 表示中のタイル

- 回転の実変更 (§4.1 の helper) のたびに、全 viewer context の `Loaded` で identity が `Legacy` の本のタイルのうち、
  変更キーを含む本 (同じ在メモリ判定) だけを `evict_thumbnail_for_reload` し、`request_repaint` する。
  次のフレームの要求作成で `refresh_legacy_cover=true` の要求が出て作り直される。
  既存の `evict_pinned_adjustment_materializations` (全 context を回る) と同じ形にする。
- これは描画中ではなく、回転を変えた操作の処理の中で行う。`Legacy` 以外のタイルは触らない (表紙以外の回転で
  無関係なタイルが消えない)。対象は「旧行のまま表示中」のタイルだけで、1 回作り直せば `Legacy` でなくなる。

### 5.3 残る既知の制限 (旧行に限る)

- CacheOnly の経路 (ドライブ一覧、判定待ちの変換書庫) が旧行を表示する間は回転しない。行が通常の要求で作り直されると直る。
- 旧行の子を流用したフォルダの代表は、その子の行が作り直されるまで回転しない (子の本を含む一覧を開けば作り直される)。

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
| 動画フレームの seed (`seed_folder_video_pin_thumbs`、smart_folder の seed) | `Page(normalize_path(video))`。既存行と bytes / stamp が同じでも identity が違えば書き直す (旧 seed 行は 1 回だけ書き直される) |
| ドライブ一覧の seed (`drive_list_pin_seed_entry` 系) | ページ行→本の行のコピーは、そのページのキー (Image / ZipEntry / PdfPage)。本の行→本の行は列ごとコピー |
| ドライブ一覧の暫定→確定 (bytes が同じなら Finalized だけ送る経路) | bytes と identity が両方同じときだけ Finalized だけにする。違えば画素ごと送る |
| 仮想フォルダの seed (本の行→先頭ページ行) と writeback (ページ行→本の行) | どちらもそのページのキー |
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

## 9. mIV Remote

- Remote のページのタイルは、読み込んだ後に `rotation_for_remote_page` で回している。本のタイル (container の ZIP / PDF の表紙、
  ZipDir、フォルダ) は、同じ catalog の行 (または worker の結果) の `CoverIdentity` が `Page(k)` なら、ページのタイルと同じ方法で k の角度を当てる。
- 回転の変更が Remote の表示へ届く仕組み (ETag や Remote 側のキャッシュの失効) は、ページのタイルの回転と同じ経路に揃える。
  ページのタイルと違う扱いが要るなら、実装時に報告する。

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
- 描画入口: 選択変更後の再描画 (`repaint_grid_cell_after_input`) と詳細表示の hover でも回転すること。
- Remote: 本のタイルが表紙の角度で返ること。
- 既存の UI snapshot は変わらない想定 (変わるなら意図を確かめて更新)。

## 11. 文書

- [display-pipeline.md](display-pipeline.md) の回転の表 (「回転 (DB) | 描画時の GPU 行列」) に、本のタイルは素材の `CoverIdentity` から引くことを足す。
- [catalog-design.md](catalog-design.md) に `cover_key` 列と `Legacy` の扱い。
- Remote の本のタイルの回転を [web-remote-plan.md](web-remote-plan.md) の該当節へ。
- バックログ §1.718 の状態。製品ページ・マニュアルは回転の説明を確かめ、本のタイルに触れていなければ変更しない。
