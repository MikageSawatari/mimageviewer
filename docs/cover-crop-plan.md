# §1.764 書庫の代表サムネイルに使う表紙範囲 (設計)

- 状態: 設計中 (2026-11-11)。独立設計レビュー前。ブランチ `feat-764-cover-crop`。
- 要件の正本: [next-release-backlog.md §1.764](next-release-backlog.md)。
- 段階: **第 1 段 = 手動の範囲指定** (この文書の §2〜§11)、**第 2 段 = 自動切り出し** (§12、第 1 段の後で詳細化して別にレビュー)。
  手動だけでも ZipPla からの移行の用は足り、自動は誤判定の調整に時間がかかり得るため。
- 既定の割り切り: まれな失敗 (DB の読み書き失敗・処理途中の強制終了など) はログ + 通知で割り切る (CLAUDE.md)。
- 関連: [表示パイプライン §1.3〜1.5](display-pipeline.md#13-ワーカー側の流れ)、
  [仮想フォルダ §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)、
  [表紙の回転 §1.718](cover-rotation-1718-plan.md)、[検索のサムネ §1.741](search-thumb-parent-catalog-plan.md)。

## 1. 要件と前提

- 表表紙・背・裏表紙・折り返しが 1 枚につながった画像から、表表紙の部分だけを親コンテナ
  (フォルダ / ZIP / PDF (EPUB 含む) / 変換アーカイブ / ZIP 内の ZipDir) の**代表サムネイル**にする。
- 範囲は代表サムネイル専用。元画像、閲覧時の表示、本の中の各ページのサムネイル、編集・書き出しには適用しない。
- 手動の範囲指定 (指定・変更・解除) と、設定で有効にする自動切り出し (既定 OFF)。手動が自動より優先。
- 自動が OFF で手動の範囲が無ければ、今の表示と完全に同じ (cache key も変えない)。

コードで確かめた前提 (2026-11-11、master c4e92e5df):

- 手動ピンは `src/folder_thumb_pins.rs` (`folder_thumb_pins.db`、**リリース済み**)。`App::apply_folder_thumb_pin`
  (`src/app.rs`) が `LoadRequest` を leaf 用に書き換え、cache key は `{base_key}#pin:{source_id}`。
  `source_id` は直接 leaf なら `kind|rel|entry|page|mtime|size` の compact 表現、cascade なら
  `cascade:{route_hash}:{leaf_source_id}` (`finalize_cascaded_source_id`)。
- ピンの記録の書き手は `FolderThumbPinDb::{set, remove}` のほか、SQL を直接書く
  `metadata_transfer.rs` (取り込み)、`rename_key_migration.rs` (`source_rel` の付け替え)、
  `zip_key_migration.rs` (`source_entry` の付け替え・行の移し替え)。行の変更は trigger で
  `folder_thumb_pin_revision` を進め、フォルダ代表の選定証明 (`lookup_for_selection`) を失効させる。
- 一覧の回転は描画時の GPU 行列。§1.718 以降、本のタイルは `Loaded` の `CoverIdentity::Page(key)` (表紙ページの回転キー)
  で角度を引く。画素に回転は焼き込まない。
- 固定 leaf が Image / ZipEntry / PdfPage なら、その編集プレビュー (`edit_preview_cache`) と個別色調補正が
  親タイルへ引き継がれる (display-pipeline §1.3.1 末尾)。編集プレビューは収録範囲
  `SourceCoverage::Known(EditPreviewGeometry)` を持つ (§1.165 B1、旧行は `Unknown`)。
- mIV Remote (`src/remote_ipc/thumbnail.rs` の `apply_supported_folder_pin`) は自前で固定 leaf の要求を組み、
  `resolved.source_id` から同じ形の key を作って**同じ catalog に保存する**。
- フォルダの自動代表の選定は、子フォルダの Image ピンをその画像として直接読む
  (`thumb_loader.rs` の候補組み立て、非 Image の子ピンだけ子の WebP を読み取り専用で再利用する)。

## 2. 方針の要点

1. **範囲はピンの記録の一部にする** (`folder_thumb_pins` の行に列を足す)。所有者は 1 つで、
   既存の cascade・選定証明の revision・名前変更の付け替え・metadata の取り込み/書き出しに自然に乗る。
2. **範囲の指定は、その画像を代表に固定することを兼ねる**。範囲を持てるのは leaf (画像 1 枚) を指すピンだけ
   (Image / ZipEntry / PdfPage。変換アーカイブ・ネスト ZIP の entry、EPUB のページを含む)。
   自動選定の代表は中身の追加で変わり得るので、範囲だけを残すと別の画像に当たる。
3. **範囲は `ResolvedPinTarget` が leaf と一緒に運び、`source_id` に範囲を含める**。cache key が範囲ごとに変わるので、
   範囲の変更・解除で古い WebP を自然に外せる。cascade (A → B → leaf) では B の範囲が A のタイルにも効く
   (「A は B と同じ代表を出す」を保つ)。
4. **範囲を含む key の行には、その範囲で切り出した画素だけを書く** (§5 の不変条件)。
   key を作る処理と切り出しの指示を、同じ値 (`ResolvedPinTarget`) から同時に作る。
5. **切り出しは thumbnail worker で、デコード後・縮小前に 1 か所で行う**。縮小済みのサムネを切らない。
   デコード解像度は「切り出した範囲が表示寸法に足りる」ように選ぶ。
6. **範囲の座標は、leaf の元画像の EXIF 適用後・編集前・保存回転前の正規化座標**
   (既存の切り取り `export_crop` と編集プレビューの収録範囲と同じ空間)。描画時の回転 (§1.718) は切り出した画素に
   そのまま掛かるので、表示と一致する。
7. **範囲指定の画面はモーダル**。開いている間は他の操作を受け付けない。

## 3. 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- **範囲指定の画面をモーダルにする** (採用)。開いている間にフォルダ移動・ピンの変更・回転・別ウィンドウ操作が
  入らないので、「指定中に代表や回転が変わった」組み合わせを扱わない。画像の読み込みは worker だが、
  画面が開いている間だけの 1 件で、閉じたら結果を捨てる。
- **範囲をピンとは別の表・別の DB に持つ案は採らない**。名前変更・ZIP キー移行・取り込みの全書き手に
  もう 1 つの表の同期を足すことになり、ずれると別の画像に範囲が当たる。同じ行なら行の移動に範囲が付いてくる。
- **範囲付きの代表だけ別の cache 方式にする案は採らない**。key に範囲を含めれば、既存の cache・失効・`delete_missing`
  の存続集合の仕組みをそのまま使える。範囲の変更で表示中のタイルを作り直す専用処理は持たず、既存のピン変更の経路
  (`folder_thumb_pin_dirty` → 一覧の読み直し) に乗せる。
- **画素に回転を焼き込まない** (§1.718 と同じ)。範囲は回転前の座標で持ち、回転は描画時に掛ける。
  回転を変えても範囲・画素は作り直さない。
- **範囲の解除はピンを残す**。範囲だけを外す操作と、既存の「固定の解除」(行ごと削除 = 範囲も消える) の 2 つにする。
  「範囲を付けたときに自動で固定したか」の記録は持たない (状態を増やすため)。
- **自動切り出し (第 2 段) は範囲の記録を作らない**。設定と leaf から毎回決まる派生値として key に含めるだけにし、
  手動の範囲と同じ列に書かない。

## 4. データ: 範囲の型と永続化

### 4.1 型

```rust
/// 代表サムネ専用の切り出し範囲。leaf 元画像の EXIF 適用後・編集前・保存回転前の正規化座標。
/// 単位は 1/1_000_000 (ppm)。0 <= x0 < x1 <= 1_000_000、y も同じ。
pub(crate) struct CoverCrop { x0: u32, y0: u32, x1: u32, y1: u32 }
```

- 整数で持つ (cache key の綴りと DB 値を安定させるため。浮動小数の丸め差で key が揺れない)。
- 検査 (`CoverCrop::new`): 範囲内・順序・各辺の長さが 1% (10_000) 以上。全体 (0,0)-(1e6,1e6) は「範囲なし」に正規化して `None`。
- 画素への写像は 1 つの関数 (`pixel_bounds(width, height)`、min=floor・max=ceil、`CropRect::pixel_bounds` と同じ丸め)。
  PDF の再レンダや RAW の preview / half 現像で raster 寸法が変わっても、正規化座標なので意味は変わらない。

### 4.2 `folder_thumb_pins` の列追加 (リリース済み DB → 加法 migration)

- 列 `cover_crop_x0 / cover_crop_y0 / cover_crop_x1 / cover_crop_y1 INTEGER` (NULL 可) を足す。
  `init_schema` の既存の IMMEDIATE transaction の中で `pragma table_info` を見て、無い列だけ `ALTER TABLE ... ADD COLUMN`。
  既存行は全部 NULL = 範囲なし。**行を消さない・作り直さない**。表の版番号は持っていないので足さない
  (既存の revision 表・trigger の追加と同じ加法方式)。
- 4 列すべて NULL = 範囲なし。一部だけ NULL・値の検査に落ちる行は、ログを出して**範囲なしとして扱い、ピンは有効のまま**
  (手書きの汚染など、まれな異常。行は書き換えない)。leaf でないピン (Folder 等) に範囲がある行も同じ。
- 読み取り専用の open (`open_readonly`、prepare worker) は schema を作らない。列が無い DB (旧版が作った DB を
  新版の migration 前に読む経路があれば) では範囲なしとして読む。列の有無の判定は catalog の layout 列と同じ方式。
- **旧版の動き**: 旧版は列名を明示して SELECT / INSERT するので、追加列を無視して従来どおり動く。
  旧版の `set` の UPSERT は範囲の列を書き換えないため、「新版で範囲を指定 → 旧版に戻して別の画像を固定 → 新版に戻す」と、
  古い範囲が新しい画像に当たる。版を戻して固定を付け替える、まれな操作なので割り切る (範囲を指定し直すか解除すれば直る)。
  新版の書き手は必ず範囲も一緒に書く (§4.3)。

### 4.3 書き手

| 書き手 | 範囲の扱い |
| --- | --- |
| `FolderThumbPinDb::set(container, source)` (既存の「固定」) | **範囲を NULL にする** (別の画像に固定し直したら古い範囲は無効)。同じ画像を固定し直した場合も範囲を消す (今の「固定」操作の意味を「範囲なしの固定」に揃える) |
| 新 `set_with_cover_crop(container, source, Option<CoverCrop>)` | ピンと範囲を 1 回の UPSERT で書く。source が leaf でなければ範囲付きの書き込みを拒否する |
| 新 `clear_cover_crop(container)` | 範囲の列だけ NULL にする。ピンは残す |
| `remove(container)` | 行ごと削除 (範囲も消える) |
| `rename_key_migration` の `source_rel` 付け替え / `zip_key_migration` の `source_entry` 付け替え | 同じ leaf の名前変更なので**範囲を保つ** (UPDATE なので列はそのまま残る)。`zip_key_migration` の行の移し替え (INSERT) は範囲の列も写す |
| `metadata_transfer` の取り込み (`insert_folder_pin`) / 書き出し | 範囲も運ぶ (§4.4) |

- 列が増えるとこれらの UPDATE / DELETE / INSERT でも trigger が revision を進めるので、範囲の変更でフォルダ代表の
  選定証明が失効する (子の Image ピンの範囲を変えると、親の自動代表も選び直される)。

### 4.4 metadata の書き出し・取り込み

- `PortableFolderThumbPin` に `cover_crop: Option<[u32; 4]>` を足す (`serde(default, skip_serializing_if = "Option::is_none")`)。
  範囲は利用者が作ったデータなので、別環境への移行で失わないようにする。
- 取り込み時も §4.1 の検査を通し、不正なら範囲だけを捨ててピンは取り込む (その旨を skip 理由と同じ形で報告)。
- bundle の `FORMAT_VERSION` は module の既存の規則に従って上げる (今は 12、AI 説明文で上げた版はリリース済み)。
  同じ開発周期の別ブランチが既に 13 を使っていれば、それに相乗りする (merge 時に確認)。

## 5. 解決と cache key (不変条件)

**不変条件 K**: cache key に範囲の identity を含む行には、その範囲で切り出した画素だけが入る。
範囲の無い key の行に、切り出した画素を入れない。

- `FolderThumbPinDb::lookup / lookup_many / lookup_for_selection` は `FolderPinSource` ではなく
  `FolderPin { source, cover_crop: Option<CoverCrop> }` を返す (型を変えて全呼び出し元をコンパイラで列挙する)。
  `App::folder_pin_map` も同じ型。UI の「今の固定と同じか」の比較 (固定 / 解除のトグル表示) は `source` だけで比べる。
- `resolve_pin_target_*` の各段は pin の記録 (`FolderPin`) を受け、leaf に解決した段の範囲を
  `ResolvedPinTarget.cover_crop` に入れ、その段の `source_id` の末尾に `|crop:{x0},{y0},{x1},{y1}` を足す。
  cascade の route hash は各段の `source_id` から作るので、範囲の変更で親の key も変わる。
  非 leaf に解決した段の範囲は §4.2 の異常として無視する。
- **key と切り出しの指示は同じ値から同時に作る**: `ResolvedPinTarget` の `source_id` と `cover_crop` を外から別々に
  読み書きさせず、「pinned key を作る」helper と「`LoadRequest` に leaf を設定する」helper の 2 つだけが参照する
  (field を非公開にし、`LoadRequest` への設定は 1 つの関数で key と `cover_crop` を同時に入れる)。
  今は pinned key の綴りが次の各所に分かれているので、この helper に寄せる:
  `apply_folder_thumb_pin` (app.rs)、`apply_supported_folder_pin` (remote)、フォルダ自動代表の候補
  (thumb_loader.rs の `#pin:` を組む 2 か所)、`folder_thumb_existing_keys_for*` (app.rs)、ZipDir の存続 key
  (app.rs の zip_nav 用、`#pin:{entry}` の別綴り)、smart folder・collection・pin_materialization の key 照合。
  一覧は §7 の表で網羅する。
- `epub_pin_fallback` (ピンが除外・EPUB が読めないときの代替要求) は `base_req` の複製なので範囲を持たない。
- 範囲の無いピンの `source_id` は今と同じ (既存の WebP をそのまま使う)。範囲を付けた leaf だけ key が変わる。

## 6. worker での切り出し

- `LoadRequest` に `cover_crop: Option<CoverCrop>` を足す (§5 の helper だけが設定する)。後段の `ResolvedRead` まで運ぶ。
  フォルダ自動代表の候補が子の Image ピン (範囲付き) を直接読む場合は、その候補の `ResolvedPinTarget.cover_crop` を
  `ResolvedRead` に入れる (要求の `cover_crop` ではなく、選んだ候補の値)。
- **切り出しは 1 か所**: 元画像のデコード (EXIF 適用後) の直後、表示寸法への縮小の前。`CoverCrop::pixel_bounds` で
  画素範囲を求めて切り、その後は今の縮小・WebP 保存へ進む。
- **デコード解像度**: 縮小目標 `display_px` は切り出した範囲の長辺に掛かるので、全体のデコード目標を
  `display_px × (全体の長辺 / 範囲の長辺)` に広げる (元画像の寸法を超えない、既存の上限に従う)。
  JPEG の DCT 縮小 (`pick_dct_scale_num`)、PDF の render 寸法、RAW の preview / half 現像の判定、WIC の縮小デコードが
  この目標を受け取る。範囲なしでは目標は今と同じ。
- **編集プレビュー** (固定 leaf の `edit_preview_key`): 収録範囲が `Known(geometry)` のときは、範囲を
  `geometry.source_uv_rect()` との共通部分として preview の raster 座標へ写し、下地と全注釈レイヤーを同じ境界で切ってから
  縮小・合成する。共通部分が空、または `Unknown` (旧行) のときは編集プレビューを使わず元画像から切り出す
  (旧 preview がどこを収録しているか分からないため)。今の `load_for_container(..., display_px)` は保存サイズ (最大 2048) から
  `display_px` へ縮めて返すので、範囲付きの場合は保存サイズのまま切ってから縮める経路を足す (縮小済みを切らない)。
- **個別色調補正** (固定 leaf の補正 key): 今と同じく切り出し・縮小後の下地へ適用する。
- **寸法**: 親タイルへ返す `source_dims` は `CoverCrop::pixel_bounds(元画像の寸法)` の幅・高さ (JPEG の DCT 縮小でも
  元寸法から計算)。`layout_dims` (PDF の page box) は `None` にする (縦横比は切り出した raster が持ち、page box の比で
  縮小させると範囲の比が崩れるため)。catalog の行にも同じ値を保存する。タイルの縦横比は切り出した範囲になる。
- **表紙 identity** (§1.718): `CoverIdentity::Page(leaf のページキー)` のまま。描画時に leaf の角度で回るので、
  回転前の座標で切った画素が表示と一致する。
- RAW half の再投入・idle quality-upgrade (`SourceOnly`) は要求を作り直す/引き継ぐので範囲も運ぶ。
  `CacheOnly` は範囲付きの key で引くだけ (miss なら今と同じく元ソースを開かない)。
- 動画のピン (`video_pins.db` の WebP を seed する経路) は範囲の対象外 (§4.3 の書き込みで拒否)。

## 7. 一覧・各経路への波及

### 7.0 経路の網羅 (2026-11-11 のコード調査。行番号は c4e92e5df)

| 経路 | 今の作り | 範囲の扱い |
| --- | --- | --- |
| 通常一覧の要求 `make_load_request_with_visibility` → `apply_folder_thumb_pin` (app.rs:102487 / :103196)。呼び出し: 自動比率の seed、優先/hover、keep range、idle upgrade | `resolved.source_id` から pinned key | §5 の helper で key と `cover_crop` を同時に設定 |
| ZipDir 分岐の ZipEntry ピン (app.rs:102641) と ZIP 一覧の存続 key (:39963) | `zipdir:{prefix}#pin:{entry}` (source_id を使わない別綴り) | 範囲なしは今の綴りのまま。範囲付きだけ同じ helper で `|crop:` を付ける。存続 key も同じ helper |
| 存続 key `folder_thumb_existing_keys_for*` (app.rs:102909 / :102933) と各呼び出し元 (フォルダ・タグ・favsearch・履歴・★一覧・rating_view・pin_materialization) | cascade を解決して key を作る | helper 経由で範囲付き key を返す (`delete_missing` で範囲付きの行を毎回消さない) |
| フォルダ自動代表の候補 (thumb_loader.rs:5279〜) | 子の Image ピンは `FolderThumbResolution::Image(path)` として親の行に直接デコード。非 Image の子ピンは子の pinned WebP を読み取り専用で流用 (:5059、key は `source_id` から) | `FolderThumbResolution::Image` に範囲を足し、親の自動代表の行にも切り出した画素を書く。流用の key は helper。親の行の選定証明は pin store の revision を持つので、子の範囲の変更で失効する |
| 子ピンの drive 一覧向け送出 `send_pinned_child_folder_cached` (thumb_loader.rs:2687) | lookup + cascade | helper |
| ドライブ一覧の seed `drive_list_pin_seed_entry` (app.rs:46184) | Image ピンは**ページの行**の画素を pinned key へ写す。key は解決せず `kind|rel|entry|page|` の前方一致 (`drive_list_pinned_cache_key_prefix`、:102116〜) | **範囲付きのピンではページ行からの seed をしない** (不変条件 K)。前方一致は範囲の部分まで一致する行だけを採る (範囲なしのピンが範囲付きの行を拾わない、逆も同じ)。worker の `send_pinned_only_cached` も同じ |
| 仮想フォルダの seed / writeback (app.rs:42961) | 本の**基本 key** (`zipthumb:` 等) ⇔ 先頭ページ行。pinned key は使わない | 影響なし (範囲は pinned key だけ) |
| 動画の seed (app.rs:45976、smart_folder・pin_materialization・everything metadata) | 動画ピンの WebP を pinned key へ | 動画は範囲の対象外 (書き込みで拒否) |
| 本棚・AI 説明文・MCP `item_info` の表紙解決 `resolve_book_cover` (book_cover.rs:133) | 一覧と同じ計画で leaf の位置を返す | 位置だけを使うので範囲は付けない (§7.1)。範囲指定の画面 (§8.2) はこれで leaf を得る |
| コレクション (collection_grid.rs:485 / 再利用 key の digest :286) | `lookup_many`、digest は `FolderPinSource` の各 field を hash | digest に範囲も含める (範囲の変更で再利用 key が変わる) |
| 色フィルタ (color_filter.rs:692〜 / :844) | pin を Debug 表記で hash、ZIP/PDF は `apply_folder_thumb_pin` を直接呼ぶ | 型が `FolderPin` になれば hash に範囲が入る。要求は helper 経由なので切り出した表紙の色になる |
| スマートフォルダ・サブフォルダ展開・Everything 検索の pin map (smart_folder.rs:6951、subfolder_expansion.rs:1569、global_search_ui) | `lookup_many` → `folder_pin_map` → 通常の要求 | 型変更に追従するだけ |
| mIV Remote `apply_supported_folder_pin` (remote_ipc/thumbnail.rs:1008) | Image / Folder の leaf だけ自前で要求を組み、同じ catalog に保存 | **helper 経由で範囲も設定する** (しないと範囲付き key に切り出していない画素を書く)。回転は Remote 側が CPU で掛けているので、回転前に切った画素で一致する |
| metadata の書き出し・取り込み (metadata_transfer.rs:3761 / :6190) と取り込み後の再読込 (metadata_import_refresh.rs:488) | 4 列を明示して読み書き | §4.4 |
| 名前変更・ZIP キー移行・画像フォルダ→ZIP 変換の行コピー (rename_key_migration.rs:2600、zip_key_migration.rs:148 / :288、image_folder_zip_metadata.rs:201) | source の列を付け替え・行を写す | 範囲を保つ。行のコピーが列を列挙していれば範囲の列も写す (実装時に確認) |
| UI の固定状態 (固定ボタン・右クリックの固定/解除・「ピ」バッジ・`GridPin` / `FsPin` キー・ゲームパッドのリング) | `folder_pin_map` の source と比較して toggle | `source` だけで比べる。toggle の解除は行ごと削除 (範囲も消える)、別の画像への固定は範囲を消す (§4.3) |
| 一覧の採用 (app.rs:56713〜) | `source_dims` / `layout_dims` で自動比率と見開きの寸法記録 | 自動比率は切り出した寸法になる (意図どおり)。**見開きの寸法記録 (`record_*_dims_for_spread`) には切り出した寸法を渡さない** (ページの寸法ではないため。コンテナの idx が見開きの判定に入らないことも実装時に確認) |
| 詳細表示の hover (ui_main.rs:18918) | 一覧の `ThumbnailState` と回転を使う | 追加なし |

### 7.1 方針

- **代表サムネを出す全経路が同じ結果になる**: 通常の一覧、本棚、コレクション、スマートフォルダ、検索 (§1.741)、
  ★固定リスト等の合成一覧、詳細表示の hover、ドライブ一覧、mIV Remote、MCP の縮小画像。
  これらは pinned key か、それを作った `Loaded` の画素を使うので、§5 の helper を通れば揃う。
- **範囲を適用しないもの**: フルスクリーン・ページ送り・本の中のページ一覧 (ページ key は pinned key と別)、
  編集・書き出し・コピー、AI 説明文の表紙 (`resolve_book_cover`。表紙を高解像度で読み直す別機能であり、
  サムネ専用という要件に従う)、3D 写真。
- **画素をコピーする経路** (仮想フォルダの seed / writeback、ドライブ一覧の seed、子の WebP の流用) は、
  コピー元とコピー先の key の範囲 identity が一致する場合だけコピーする (不変条件 K)。範囲付きの pinned key の行を
  ページの行へ、ページの行を範囲付きの pinned key の行へ写さない。

## 8. 範囲指定の画面 (第 1 段)

### 8.1 入口 (利用者に確認する: §13)

- 案 A (主): 一覧の**本のタイル** (Folder / ZipFile / PdfFile / 変換済み ConvertibleArchive / ZipDir) の右クリック
  「代表サムネの範囲を指定…」。対象は**そのタイル自身の代表**。今の代表 leaf を worker で解決して表示する。
  - 既存の「代表サムネに固定」は「右クリックした項目を、今開いているフォルダの代表にする」操作で、対象のコンテナが違う。
    案 A は対象のコンテナを右クリックしたタイルにする新しい操作。
- 案 B (副): 本の中 (フォルダ / ZIP / PDF を開いた一覧) の**ページ**の右クリック「このページを代表にして範囲を指定…」。
  対象は今開いているコンテナ (既存の「代表サムネに固定」と同じ)。表紙が最後のページにある本などに使う。
- egui の右クリックメニューとネイティブの右クリックメニューの両方に出す。キー割り当ては足さない
  (矩形をマウスで指定する操作で、キーだけでは完結しないため。§11 の keymap 記録)。
- 出さない場所: 合成一覧で実パスの無いタイル、未変換のアーカイブ (既存の固定と同じ disabled 表示)、Ctrl+G の集約表示、
  detached viewer (リワーク中の凍結規則に触れないため。範囲の結果は通常のピン変更として反映される)。

### 8.2 代表 leaf の解決 (案 A)

- worker で、一覧と同じ計画・`apply_folder_thumb_pin`・変換アーカイブ判定を使う既存の `resolve_book_cover` (AI 説明文の表紙解決)
  を使い、今の代表の `ReadLocation` を得る。UI スレッドで pin DB・stat・ZIP 列挙をしない。
- 得た leaf を、対象コンテナからの `FolderPinSource` に直す (画像 → `File{Image}`、ZIP/変換アーカイブの entry → `ZipEntry`、
  PDF/EPUB のページ → `PdfPage`、サブフォルダの中なら相対パス)。直せない・leaf が画像でない (動画・音声・アイコン・未変換) 場合は
  画面に「この代表には範囲を指定できません。本の中を開いて表紙のページで指定してください」と出す。
- 代表がキャッシュの流用 (子の WebP) で決まっている場合、解決し直した leaf が今の画素と違うことがまれにある (§1.718 の Astra 指摘と同じ現象)。
  画面に出るのは固定される画像そのものなので、利用者は指定する前に分かる。

### 8.3 画面

- モーダルの `egui::Window` (`ui_dialogs/` に 1 ファイル、`common_modal_dialog_open` に「今表示されるか」の helper で登録)。
  `default_pos()`、1093×614 logical points でも OK / キャンセルが見えること (見出しと操作行は本文の外)。
- 画像は専用 worker で読む (EXIF 適用、長辺は画面の表示寸法 × ppp 程度、上限 2048)。読み込み中は表示、失敗は文言 + 閉じる。
  画面を閉じたら結果を捨てる (1 件だけ、世代番号で古い結果を採用しない)。
- 表示は leaf の保存回転を掛けた向き (在メモリの「キー→角度」owner を引く。DB は読まない)。矩形の入出力で回転前の座標と変換する。
- 操作: ドラッグで新しい矩形、四隅・四辺の handle と内部ドラッグで変更 (`export_crop::CropRect::dragged` を流用)。
  既存の範囲があれば初期表示する。
- ボタン: 「OK」(範囲を書いて固定) / 「範囲を解除」(範囲がある場合だけ。ピンは残す) / 「キャンセル」。
  Enter / Escape は `dialog_enter_pressed` / `dialog_escape_pressed`。
- 補足文: 「範囲は一覧の代表サムネだけに使います。画像そのものや閲覧時の表示は変わりません。」
  「範囲を指定すると、この画像が代表に固定されます。」
- 確定は既存のピン書き込みと同じ経路 (UI スレッドの単一 UPSERT、`folder_pin_map` 更新、`folder_thumb_pin_dirty`)。
  書き込み失敗は既存の固定と同じくトーストで通知 (まれな失敗の割り切り)。

## 9. 適用しないもの (まとめ)

元画像のファイル、閲覧・ページ送り・本の中のページ一覧、編集・切り取り・書き出し・コピー、AI 説明文、3D 写真、動画のピン。

## 10. テスト

- DB: 旧 schema の DB (列なし) を開いて列が追加され、既存行が範囲なしで読めること。旧版の SQL (列名を明示した SELECT / INSERT /
  UPSERT) が新 schema で通ること。`set` で範囲が消え、`set_with_cover_crop` / `clear_cover_crop` / `remove` の結果。
  不正値・一部 NULL・非 leaf の範囲が「範囲なし・ピン有効」で読まれること。rename / zip key 移行で範囲が保たれること。
  metadata の書き出し → 取り込みの往復と、不正範囲の取り込み。
- key: 範囲なしの key が今と一致すること (既存 WebP の再利用)。範囲の有無・値で key が変わること。cascade で子の範囲が親の key に効くこと。
  全 key 綴りの経路 (§5) が同じ helper の値になること (存続集合・ZipDir・remote・自動代表の候補)。
- worker: 範囲付き要求の出力寸法・内容 (既知の画素パターンの画像で、切り出し位置を検査)、EXIF 回転済み画像の座標、
  JPEG の DCT 縮小と PDF render の目標寸法、編集プレビュー Known (共通部分の写像・注釈も同じ境界) / Unknown (元画像)、
  `epub_pin_fallback` に範囲が付かないこと、remote の要求にも範囲が付くこと。
- 不変条件 K: 画素をコピーする経路 (seed / writeback / ドライブ一覧 / 子の WebP の流用) が範囲の違う key の間でコピーしないこと。
- 画面: 状態遷移 (読み込み中 → 表示 → OK / 解除 / キャンセル、失敗)、モーダル中に他の入力が通らないこと、
  回転 90/180/270° の表示と保存座標の往復、小画面・UI 倍率のスナップショット。
- 閲覧への非影響: 範囲付きの本を開いたとき、ページ一覧・フルスクリーンが元画像のままであること。

## 11. 文書

- マニュアル: 代表サムネの節 (`htdocs/mimageviewer/manual/` の該当ページ) に範囲指定を追記。製品ページは必要なら 1 行。
- 設計文書: display-pipeline §1.3.2、virtual-folders §3.1.1 (schema・key の例外規則)、ui-responsiveness §4 (新 worker)、
  keymap-spec (キー割り当てを足さない理由)。

## 12. 第 2 段: 自動切り出し (概要、第 1 段の後で詳細化)

- 設定 `thumb_cover_auto_crop: bool` (既定 OFF)。ON で、手動の範囲が無い代表 (自動選定・範囲なしのピン) の leaf にだけ掛ける。
- 判定は worker で、デコード済みの leaf 画像から行う。横長 (縦横比のしきい値以上) でない画像・判定できない画像は元のまま
  (誤爆しない側に倒す)。ZipPla の方式 (四隅の色の帯で外周の余白を対称に除く、表紙・背・折り返しの幅の比から位置を推定、
  綴じ方向で左右を反転) を参考にするが、しきい値は検証画像で決める。
- 綴じ方向は本ごとの見開き設定 (`SpreadMode::advances_right_to_left`) と既定値から決める。入力の取り方は第 2 段の設計で詰める。
- cache key には自動の版・ON/OFF・綴じ方向を含める (`folderthumb:auto-v3` の版と同じ考え方)。OFF のときは key を変えない。
  手動の範囲がある leaf では自動の部分を key に含めない。
- 自動の結果は記録しない (§3)。

## 13. 利用者に確認する点

1. 範囲指定の入口: 案 A (本のタイルの右クリック) + 案 B (本の中のページの右クリック) の両方でよいか。
   ほかに欲しい場所 (アドレスバーの固定ボタン、フルスクリーンなど) があるか。
2. 「範囲を指定すると代表が固定される」「範囲の解除ではピンは残り、固定の解除で両方消える」でよいか。
3. 段階: 第 1 段 (手動) を先に仕上げ、第 2 段 (自動) はその後に設計・実装する進め方でよいか。
