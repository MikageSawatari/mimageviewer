# §1.764 書庫の代表サムネイルに使う表紙範囲 (設計)

- 状態: **設計確定 (2026-11-11)**。独立設計レビュー r1〜r3 = REVISE、r4 = ACCEPT (§14)。実装: 手動 (第 1 段) → 自動 (第 2 段)。ブランチ `feat-764-cover-crop`。
- 要件の正本: [next-release-backlog.md §1.764](next-release-backlog.md)。
- **リリース条件 (利用者決定 2026-11-11)**: 手動と自動の両方がそろってから出す (ZipPla が自動のため)。
  実装の順は手動 (§2〜§11) → 自動 (§12)。次の版に間に合わなければ、その次の版に回す。
- 既定の割り切り: まれな失敗 (DB の読み書き失敗・処理途中の強制終了など) はログ + 通知で割り切る (CLAUDE.md)。
- 関連: [表示パイプライン §1.3〜1.5](display-pipeline.md#13-ワーカー側の流れ)、
  [仮想フォルダ §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)、
  [表紙の回転 §1.718](cover-rotation-1718-plan.md)、[検索のサムネ §1.741](search-thumb-parent-catalog-plan.md)。

## 1. 要件と前提

- 表表紙・背・裏表紙・折り返しが 1 枚につながった画像から、表表紙の部分だけを親コンテナ
  (フォルダ / ZIP / PDF (EPUB 含む) / 変換アーカイブ / ZIP 内の ZipDir) の**代表サムネイル**にする。
- 範囲は代表サムネイル専用。元画像、閲覧時の表示、本の中の各ページのサムネイル、編集・書き出し、類似画像の索引には適用しない。
- 手動の範囲指定 (指定・変更・解除) と、設定で有効にする自動切り出し (既定 OFF)。手動が自動より優先。
- 自動が OFF で手動の範囲が無ければ、今の表示と完全に同じ (cache key も変えない)。

コードで確かめた前提 (2026-11-11、master c4e92e5df):

- 手動ピンは `src/folder_thumb_pins.rs` (`folder_thumb_pins.db`、**リリース済み**)。`App::apply_folder_thumb_pin`
  (`src/app.rs`) が `LoadRequest` を leaf 用に書き換え、cache key は `{base_key}#pin:{source_id}`。
  `source_id` は直接 leaf なら `kind|rel|entry|page|mtime|size`、cascade なら `cascade:{route_hash}:{leaf_source_id}`。
  ZipDir の ZipEntry ピンだけは `zipdir:{prefix}#pin:{entry}` の別綴り (app.rs:102641 / :39963)。
- 表示設定 (種類の表示) は `representative_cache_key_for_policy` が基本 key 側に `|types:…` を付ける (thumb_loader.rs:344、
  pinned key では `{base}|types:…#pin:{source_id}`)。EPUB の世代 `|generation:m:s` は別の関数が末尾に付ける (:853)。
- ピンの記録の書き手は `FolderThumbPinDb::{set, remove}`、SQL を直接書く `metadata_transfer.rs`・`rename_key_migration.rs`・
  `zip_key_migration.rs`・`image_folder_zip_metadata.rs`。行の変更は trigger で `folder_thumb_pin_revision` を進め、
  フォルダ代表の選定証明 (`lookup_for_selection`) を失効させる。
- 一覧の回転は描画時の GPU 行列。§1.718 以降、本のタイルは `Loaded` の `CoverIdentity::Page(key)` で角度を引く。
- 固定 leaf の編集プレビューと個別色調補正は親タイルへ引き継がれる (display-pipeline §1.3.1 末尾)。worker は編集プレビューを
  catalog より先に確かめ、あればそこで返す (thumb_loader.rs:3277)。自動選定の代表には編集プレビューを使わない。
- mIV Remote は Image / Folder のピンだけを自前で解決し (`apply_supported_folder_pin`)、**本体と同じ catalog に保存する**。
  ZIP / PDF / ZipDir の本のタイルは別の経路 (`remote_ipc/container.rs`) で基本 key と先頭画像を使い、ピンを見ない。
- フォルダの自動代表の選定は、子フォルダの Image ピンを親の自動代表の行へ直接デコードし (thumb_loader.rs:5494)、子の本 (ZIP 等) の
  タイルの行は読み取り専用で流用する (:5059 / :5455)。
- 縮小前の raster は類似画像索引へも渡る (`similar_index::offer_thumbnail_raster`、thumb_loader.rs:6332)。
- ドライブ一覧は表示時に I/O をしない cache-only の画面で、ページ行や子の行を写して近似表示する (app.rs:46184〜)。

## 2. 方針の要点

1. **範囲はピンの記録の一部にする** (`folder_thumb_pins` の行に列を足す)。範囲を持てるのは leaf (画像 1 枚) を指すピンだけ
   (Image / ZipEntry / PdfPage。変換アーカイブ・ネスト ZIP の entry、EPUB のページを含む。RAW・動画は除く)。
2. **範囲の指定は、その画像を代表に固定することを兼ねる** (利用者決定 §13-2)。
3. **範囲は `ResolvedPinTarget` が leaf と一緒に運び、pinned key の `source_id` に範囲を含める**。
   key と切り出しの指示は同じ値から 1 つの helper で同時に作る (§5)。
4. **切り出しは thumbnail worker で 1 か所**: 元寸法の確定と類似画像索引への受け渡しの後、縮小の前 (§6)。
5. **範囲で切る代表は元画像から作る** (編集プレビューは使わない。利用者決定 §13-4)。
6. **座標は leaf 元画像の EXIF 適用後・保存回転前の正規化座標**。描画時の回転 (§1.718) はそのまま掛かる。
7. **範囲指定の画面はモーダル**。代表の解決と画像の読み込みは 1 つの worker 仕事 (§8)。
8. **自動切り出しは、ファイルの本 (ZIP・変換アーカイブ・PDF・EPUB とその中の ZipDir) の代表だけ**に掛ける、記録を作らない派生値。
   本のファイルの stamp が中身の変更を表すので、周辺ページの変更で自然に作り直される。判定できないときは切らない (§12)。
9. **ドライブ一覧と mIV Remote の本のタイルには、範囲も自動も反映しない** (どちらも今もピンの解決を近似・省略している画面)。

## 3. 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- **範囲指定の画面をモーダルにする** (採用)。指定中にフォルダ移動・ピンの変更・回転・別ウィンドウ操作が入らない。
- **代表の解決と画像の読み込みを 1 つの worker 仕事にする** (採用、r1 指摘 5)。「解決済み・未読み込み」の中間状態を持たない。
- **範囲付きの代表は編集プレビューを使わない** (利用者決定 §13-4)。収録範囲 Known / Unknown・共通部分・注釈レイヤーの切り出し・
  画面と結果の不一致の組み合わせを消す。代償は、範囲を付けた表紙のタイルに消しゴム・注釈・編集の切り取りが出ないこと (色調補正は出る)。
- **編集プレビューがある固定ページには自動を掛けない** (r2 指摘 1)。worker は今と同じく編集プレビューを先に確かめ、あれば返す。
  自動の判定は catalog の経路にだけある。判定結果を記録して「切った / 切らなかった」で編集プレビューの優先を変える仕組みを持たない。
- **自動の対象をファイルの本に限る** (r2 指摘 3)。フォルダの本では周辺ページの変更を表す stamp が無く、入力ページの stamp を証明に
  持たせる仕組みが要る。ZipPla からの移行で問題になるのは書庫の本で、フォルダは手動で指定できる。
- **ドライブ一覧に範囲も自動も反映しない** (r2 指摘 2)。ドライブ一覧の行を正しく作る作り手 (代表専用の要求) を新設しない。
  ドライブの根に範囲は付けられない (入口を出さない)。
- **RAW の leaf には範囲も自動も付けない** (r1 指摘 10)。
- **範囲をピンとは別の表に持たない**。行の移動に範囲が付いてくる。旧版との取り違えは束縛列で防ぐ (§4.2)。
- **範囲の解除はピンを残す**。「範囲を付けたときに自動で固定したか」の記録は持たない。
- **範囲の変更で表示中のタイルを作り直す専用処理を持たない**。既存のピン変更の経路 (`folder_thumb_pin_dirty` → 読み直し) に乗せる。
- **自動を ON / OFF にしたときの作り直しは、種類の表示設定を変えたときと同じ** (key が変わり、表示した本のタイルから作り直す)。
  作り直しを減らす近道 (縦長の行の写し) は、正しさの条件が増えるので持たない (r2 指摘 7)。
- **自動は本ごとの綴じ方向を読まない** (§12.4)。読むには一覧の時点で本ごとの設定を引く仕組みと、その変更での失効が要る。
  既定の綴じ方向と違う本では、自動は切らないか、裏表紙が色豊かな場合に裏表紙を選ぶことがある (§12.6 の数字)。手動で直せる。

## 4. データ: 範囲の型と永続化

### 4.1 型

```rust
/// 代表サムネ専用の切り出し範囲。leaf 元画像の EXIF 適用後・保存回転前の正規化座標 (1/1_000_000 単位)。
pub(crate) struct CoverCrop { x0: u32, y0: u32, x1: u32, y1: u32 }
```

- 整数で持つ (cache key と DB 値を安定させる)。検査: 範囲内・順序・各辺 1% (10_000) 以上。全体は「範囲なし」(`None`)。
- 画素への写像は 1 つの関数 `pixel_bounds(width, height)` (min=floor・max=ceil、`CropRect::pixel_bounds` と同じ丸め)。

### 4.2 `folder_thumb_pins` の列追加 (リリース済み DB → 加法 migration)

- 列 `cover_crop_x0 / _y0 / _x1 / _y1 INTEGER` と、**束縛列 `cover_crop_source TEXT`** (範囲を書いたときの source の
  正規化表現 `kind|rel|entry|page`) を足す。`init_schema` の既存の IMMEDIATE transaction 内で `pragma table_info` を見て
  無い列だけ `ALTER TABLE ... ADD COLUMN`。既存行は NULL = 範囲なし。行を消さない・作り直さない。
- **範囲の有効性は 1 つの decoder (`decode_cover_crop(row)`) だけが決める**: 5 列がそろい、値が検査を通り、`cover_crop_source` が
  今の行の source と一致し、source が leaf (RAW・動画でない) のときだけ有効。それ以外はログを出して範囲なしとして扱い、
  ピンは有効のまま (行は書き換えない)。
  - 旧版が `set` で別の画像へ固定し直しても、束縛が一致しないので古い範囲は当たらない (r1 指摘 6)。
  - 旧版の名前変更 (`source_rel` だけを書き換える) の後も束縛が外れて範囲なしになる (範囲は失われる側に倒れる)。
- 同じ IMMEDIATE transaction 内に source 4 列の `AFTER UPDATE` trigger も冪等に追加する。
  source が実際に変わったら、束縛列の値に関係なく範囲の5列をNULLにする。旧版のa→b→aで範囲を復活させず、
  開発版の古い不一致行が新しい保存を誤って消すことも防ぐ。未リリースの旧trigger定義はDROP＋CREATEで置き換える。
  新版のset・改名・ZIPキー移行・画像フォルダZIP化・metadata取り込みは、同一transaction内でsourceを先に書き、
  decoderで検証済みの範囲と新しい束縛を別UPDATEで後から書く。後者はsource列を触らずtriggerを再発火しない。
  container keyだけの名前変更では範囲を保つ。revision triggerの追加増分は変更検出用のtokenとして許容する。
- 読み取り専用の open (`open_readonly`) は schema を作らない。列の無い DB では範囲なしとして読む (catalog の layout 列と同じ方式)。
- 別プロセス (mIV Remote) の `open` も同じ `init_schema` を通る (同じ版の exe。列の追加が重なっても IMMEDIATE で直列化される)。

### 4.3 書き手

| 書き手 | 範囲の扱い |
| --- | --- |
| `FolderThumbPinDb::set(container, source)` (既存の「固定」) | 範囲と束縛を NULL にする (同じ画像の固定し直しでも消す) |
| 新 `set_with_cover_crop(container, source, Option<CoverCrop>)` | 同一transaction内でsourceをUPSERTし、範囲・束縛を別UPDATEで書く。source が leaf でない・RAW・動画なら拒否 |
| 新 `clear_cover_crop(container)` | 範囲と束縛だけ NULL。ピンは残す |
| `remove(container)` | 行ごと削除 |
| `rename_key_migration` の `source_rel` 付け替え、`zip_key_migration` の `source_entry` 付け替え・行の移し替え、`image_folder_zip_metadata` の行の写し替え | 同じ leaf の名前変更。**付け替え前に `decode_cover_crop` で有効だった範囲だけ**、束縛列を新しい source に付け替えて保つ。無効だった範囲は NULL にする (r2 指摘 4) |
| `metadata_transfer` の書き出し・取り込み | §4.4 |

### 4.4 metadata の書き出し・取り込み

- `PortableFolderThumbPin` に `cover_crop: Option<[u32; 4]>` を足す (`serde(default, skip_serializing_if = "Option::is_none")`)。
- **書き出しは直接 SQL で読んだ行を `decode_cover_crop` に通し、有効な範囲だけを書き出す** (r2 指摘 4。旧版で別の画像へ固定し直した
  行の無効な範囲を、取り込みで復活させない)。取り込みでは束縛列を取り込む行の source から作る。不正な範囲は範囲だけ捨てて
  ピンは取り込み、skip 理由として報告する。
- `FORMAT_VERSION` を 13 に上げ、**受け入れる版の列挙 (metadata_transfer.rs:4204 / :4291) には 12 以前を残す** (r1 指摘 9)。
  12 以前の bundle は `cover_crop = None` として読む。同じ開発周期の別ブランチが 13 を使っていれば相乗りする。

## 5. 解決と cache key

### 5.1 不変条件 K

- **K1 (leaf の pinned 行)**: pinned key (直接 leaf・cascade・ZipDir の ZipEntry ピン) の行には、その key が表す「実効の範囲」
  (leaf を指す段の範囲) で切り出した画素だけを入れる。範囲が違えば key が違う。
- **K2 (フォルダの自動代表の行)**: `folderthumb:auto-v3…` (`AutoSelected`) の行は、key ではなく**検証済みの選定証明**が由来を保証する
  (今の契約のまま)。子の Image ピンの範囲で切った画素や、子の本のタイルの (自動で切った) 行の写しを含んでよい。範囲の変更は
  pin store の revision を、子の本のタイルの作り直しは子の catalog の revision を進め、証明を失効させる。
- **K3 (自動の identity を含む行)**: 自動の identity (§5.2) を含む key の行には、その版・設定で作った結果だけを入れる。
  自動の identity の有無が違う key の間で行を写さない。**例外は K2 の流用だけ** (r3 指摘 1): フォルダの自動代表は、子の完成済みの
  代表の行を、**その子の writer と同じ key** (子の手動の範囲・自動の対象かどうかを反映した key。今の設定を一律に付けた key ではない) で
  読み、画素をそのまま継承する。親では切り直さない。証明は子の行の由来を持つ今の仕組みのまま。
- **ドライブ一覧の行は K1・K3 の対象外の近似表示**とし、範囲・自動の identity を含む行を写さず、範囲・自動を含まない key だけを使う (§7.1)。
  ドライブ向けに切り出しを生成しない。フォルダの自動代表の行 (K2) のように、key に印の無い行に子の範囲や自動の結果が含まれている場合は、
  既存の近似としてそのまま流用し得る (r3 指摘 4)。

### 5.2 key の綴り (OFF・範囲なしは今と完全に同じ)

- 直接 leaf の `source_id`: 範囲なし `kind|rel|entry|page|mtime|size`、範囲あり `kind|rel|entry|page|crop:x0,y0,x1,y1|mtime|size`。
- cascade: 各段の `source_id` から route hash を作るので、leaf 段の範囲が親の key に効く。
- ZipDir の ZipEntry ピン: 範囲なし `zipdir:{prefix}#pin:{entry}`、範囲あり `zipdir:{prefix}#pin:{entry}|crop:x0,y0,x1,y1`。
- **自動の identity `|autocrop:v1:{rtl|ltr}`** は、表示設定と同じく**基本 key 側の末尾** (`|types:` の後) に置く。例:
  - 本のタイル (基本): `zipthumb:a.zip` → `zipthumb:a.zip|autocrop:v1:rtl`、種類設定ありで `zipthumb:a.zip|types:…|autocrop:v1:rtl`
  - 固定した本: `zipthumb:a.zip|types:…|autocrop:v1:rtl#pin:{source_id}` (EPUB は末尾に `|generation:m:s`)
  - フォルダの自動代表: `folderthumb:auto-v3:…|autocrop:v1:rtl` (K2 の流用で子の自動の結果を含み得るので、フォルダの key にも付ける)
  - 範囲付きのピン・ドライブ一覧・Remote の key には付けない。
- 正規化はmaster (`c4e92e5df`) の処理をそのまま基礎にする。区切りは最初の `#pin:` だけで、
  その前（固定なしならキー全体）末尾の16桁hexの `|types:` を1つだけ外して、現在の種類設定を付け直す。
  `|generation:` は区切りにしない。固定なしの `pdfthumb:a.epub|generation:m:s` の種類設定ありの綴りは、
  masterと同じ `pdfthumb:a.epub|generation:m:s|types:…`。固定ありでは従来どおりgenerationはpin側の末尾にある。
  自動はこの処理と分け、最初の `#pin:` の直前、固定なしではキー末尾に `|autocrop:v1:{rtl|ltr}` を1つだけ置く。
  その正確な位置にある完全一致トークンだけを先に外してからmasterの処理を通すため、通常のbuilder出力では冪等で、
  自動の追加・解除が往復する。ZIP内名の途中の `autocrop` / `generation`、未知の版、複数の `types:` はmasterの解釈を保つ。
  例: 固定なしgeneration付きの自動キーは `pdfthumb:a.epub|generation:m:s|types:…|autocrop:v1:rtl`。
  ページのkeyと仮想フォルダのseed/writebackには自動を付けない。
- 存続集合 (`delete_missing`): 今の key に加えて、表示中の設定の自動 identity 付きの key を入れる。ZIP の一覧の存続集合
  (`tree.all_cache_keys()` 由来、app.rs:39914) には、**ピンなしの ZipDir の自動 identity 付きの key** も明示して足す。
- 前方一致で行を探す経路 (ドライブ一覧の `drive_list_pinned_cache_key_prefix`、Folder 種の最新行) は、残りが `crop:` で始まる行と
  `|autocrop:` を含む行を除く。

### 5.3 所有者

- `lookup / lookup_many / lookup_for_selection` は `FolderPin { source, cover_crop: Option<CoverCrop> }` を返す
  (`decode_cover_crop` はここで呼ぶ)。`folder_pin_map` も同じ型。UI の固定 / 解除のトグル判定は `source` だけで比べる。
- `resolve_pin_target_*` は leaf 段の範囲を `ResolvedPinTarget` に入れ、その段の `source_id` を §5.2 の形にする。
- **key と切り出しの指示を外から別々に書かせない**: `ResolvedPinTarget` の `source_id` と `cover_crop` は非公開にし、
  (a) pinned key を作る helper (範囲を入れるか、入れない近似用か (ドライブ一覧) を引数で受ける)、(b) `LoadRequest` に leaf を設定する
  helper (key と `cover_crop` を同時に入れる) の 2 つだけが読む。§7.1 の pinned key の綴りは全部この helper に寄せる。
- 自動の identity は §5.2 の 1 つの関数だけが付ける。`LoadRequest` には `auto_cover_crop: Option<AutoCropIdentity>` を持たせ、
  key の付与と同時に設定する (付与と判定の指示が食い違わない)。

## 6. worker での切り出し

### 6.1 入力と代替要求

- `LoadRequest.cover_crop` (手動、§5.3 の helper だけが設定) と `LoadRequest.auto_cover_crop` (自動) を `ResolvedRead` まで運ぶ。
  手動の範囲がある要求には自動を付けず、編集プレビューの key (`edit_preview_key`) も設定しない (§6.3)。
- フォルダ自動代表が子の Image ピン (範囲付き) を直接読む場合は、`FolderThumbResolution::Image` に子の範囲を足し、
  `ResolvedRead` に入れる (K2)。フォルダの本は自動の対象外なので、子の Image への自動は掛けない。
- **種類除外・EPUB 読めずの代替要求** (`epub_pin_fallback`、thumb_loader.rs:3188): 今は代替要求に元の pinned key を付け直して保存する
  (:4963)。**元の要求が範囲付きの場合は付け直さず、代替要求自身の key で保存・参照する** (r1 指摘 1)。
  - 費用: 範囲付きの pinned key は毎回 miss し、worker が除外の判定 (ZIP の entry 列挙を含む) をやり直してから代替要求の key で
    cache hit になる (r2 指摘 5)。範囲付きのピンの先が種類の表示設定で除外されている間だけの費用として受け入れる。
  - cache-only の読み手 (ドライブ向けの確定読取 thumb_loader.rs:2756 / :2778、子の代表の流用 :5082) は範囲付きの key を完全一致で読み、
    代替の行を探さない。その間、親の自動代表は子の流用をやめて通常の探索に戻り、ドライブ一覧は範囲を見ない (§5.1)。
    誤った画素は出さず、親の表示が子の代替表示と揃わないことだけを受け入れる (除外設定 + 範囲付きピンの組み合わせに限る)。
  - **`auto_cover_crop`がSomeで手動の範囲が無い要求の代替要求は、自動の指示を保ったまま元のpinned keyで保存する**
    (r3 指摘 2)。同じ設定で代替の表紙を判定するので K3 を満たし、子の本の行を pinned key で探す既存の読み手もそのまま見つけられる。
    手動の範囲のkeyは代替要求に引き継がない。自動OFFのEPUB読めず代替は第1段どおり代替自身の基本keyへ保存し、
    自動OFFの種類除外代替はリリース済みの元pinned keyへの保存を保つ。

### 6.2 切り出しの位置と解像度

- 順序: デコード → EXIF 適用 → 元寸法の確定 → 類似画像索引への受け渡し (`offer_thumbnail_raster`、元画像のまま) →
  (自動なら判定) → 切り出し → 縮小 → WebP 保存 (r1 指摘 4)。同じ raster を使う他の consumer も切り出し前に済ませる。
- **手動の範囲**: 範囲は先に分かるので、全体のデコード目標を `display_px × (全体の長辺 / 範囲の長辺)` に広げる (元寸法・既存の上限を
  超えない)。JPEG の DCT 縮小 (`thumbnail_jpeg_decode_target`) と PDF の render 寸法 (`thumbnail_pdf_render_target`) が受け取る。
  WIC・image crate・Susie の経路は今も全体の解像度でデコードしている (thumb_loader.rs:6151、wic_decoder.rs:80) ので、そのまま全体を
  デコードしてから切る (目標寸法を足さない。実装担当の確認 2026-11-11)。
- **自動** (r2 指摘 10): (1) 今と同じ目標でデコードする。(2) そこから**高さ 256 の解析用 raster** を作って判定する (デコード倍率に
  よらず判定の入力をそろえる)。(3) 切ると判定し、範囲の長辺が `display_px` に足りない場合だけ、**同じ bytes / 同じ PDF の lease から
  手動と同じ目標で 1 回だけデコードし直す**。デコードは最大 2 回。
  判定の印刷密度・輝度分散・色の豊かさ・境界強度へ追加の平滑化を掛けない。高さ256のnative入力の細かなtextureを保ち、
  縮小は出力1画素分の面積平均を使う。余白も未平滑の解析画素で判定し、元画素では余白でない模様帯を除去しない。
  埃が縮小率によって残ったり消えたりする場合の結果の一致は保証せず、そのための平滑化を追加しない。
- 返す寸法: `source_dims` = `CoverCrop::pixel_bounds(元寸法)` の幅・高さ、`layout_dims` = `None`。catalog の行にも同じ値を保存する。
- 表紙 identity (§1.718): `CoverIdentity::Page(leaf のページキー)` のまま。回転前に切った画素が描画時に回るので表示と一致する。
- 見開きの寸法記録 (`record_*_dims_for_spread`) には、切った結果の寸法を渡さない (ページの寸法ではない)。
- idle quality-upgrade は同じ要求を作り直すので範囲・自動を運ぶ。`CacheOnly` は key で引くだけで、判定・先読みに進まない。

### 6.3 編集プレビュー (利用者決定 §13-4)

- **手動の範囲付きの要求は編集プレビューを使わず元画像から作る** (`edit_preview_key` を設定しない)。個別色調補正
  (`pinned_page_adjustment_key`) は今と同じく縮小後の下地に掛ける。範囲指定の画面も元画像を表示するので、画面と結果が一致する。
- **編集プレビューがある固定ページには自動を掛けない**: worker は今と同じ順で編集プレビューを先に確かめ、あれば返す。
  無ければ catalog の経路へ進み、そこで初めて自動を判定する。

## 7. 一覧・各経路への波及

### 7.1 経路の網羅 (2026-11-11 のコード調査。行番号は c4e92e5df)

| 経路 | 今の作り | 範囲・自動の扱い |
| --- | --- | --- |
| 通常一覧の要求 `make_load_request_with_visibility` → `apply_folder_thumb_pin` (app.rs:102487 / :103196) | `resolved.source_id` から pinned key | §5.3 の helper。自動の identity は §5.2 の関数 |
| ZipDir 分岐の ZipEntry ピン (app.rs:102641) と ZIP 一覧の存続 key (:39914 / :39963) | `#pin:{entry}` の別綴り、`tree.all_cache_keys()` | helper (§5.2)。ピンなし ZipDir の自動の key も存続集合へ |
| 存続 key `folder_thumb_existing_keys_for*` (app.rs:102909 / :102933) と呼び出し元 | cascade を解決して key を作る | helper。範囲付き・自動の identity 付きの key も存続集合に入れる |
| 種類除外・EPUB の代替要求 (thumb_loader.rs:3188 / :4963) | 代替画素を元の pinned key で保存 | §6.1 |
| フォルダ自動代表 (thumb_loader.rs:5279〜) | 子の Image ピンを直接デコード、子の本の行を流用 | 子の範囲を運ぶ (K2)。流用する子の本の行の key は §5.2 の関数 (今の設定の自動 identity 付き) |
| 子ピンのドライブ向け送出 `send_pinned_child_folder_cached` (thumb_loader.rs:2687) | lookup + cascade | 近似用の helper (範囲・自動なしの key) |
| ドライブ一覧の seed `drive_list_pin_seed_entry` (app.rs:46184〜46471)、`send_pinned_only_cached` (thumb_loader.rs:2499) | ページ行・子の最新の pinned 行をドライブの pinned key へ写す。前方一致 | **範囲・自動を反映しない**: key は範囲・自動なし、前方一致は `crop:`・`|autocrop:` を含む行を除く。ドライブの根には範囲の入口を出さない |
| 仮想フォルダの seed / writeback (app.rs:42961) | 本の基本 key ⇔ 先頭ページ行 | 自動 identity の無い基本 key とページ行の間だけで行う (今と同じ key)。自動 ON の本のタイルは別の key なので seed の対象にならない |
| 動画の seed | 動画ピンの WebP | 対象外 |
| 本棚・AI 説明文・MCP `item_info` の `resolve_book_cover` | leaf の位置 | 範囲・自動を付けない (§7.2) |
| 範囲指定の画面の解決 (§8.2) | — | 新しい worker 仕事 |
| コレクション (collection_grid.rs:485 / digest :286) | `FolderPinSource` の各 field を hash | digest に実効の範囲と自動 identity を含める |
| 色フィルタ (color_filter.rs:692〜 / :844) | pin を Debug で hash、ZIP/PDF は `apply_folder_thumb_pin` | 型変更で範囲が入る。自動 identity も hash に入れる |
| スマートフォルダ・サブフォルダ展開・Everything 検索・一覧の各 pin map | `lookup_many` → 通常の要求 | 型変更に追従 |
| mIV Remote `apply_supported_folder_pin` (remote_ipc/thumbnail.rs:1008) | Image / Folder のピンを自前で解決し同じ catalog に保存 | **helper 経由で手動の範囲を付ける** (K1)。自動は付けない |
| mIV Remote の本のタイル (`remote_ipc/container.rs:8040`) | 基本 key・先頭画像。ピンを見ない | 今と同じ (範囲も自動も反映しない)。閲覧用 raster (`RemoteSourceDecodeIdentity`) は変えない |
| 類似画像索引 (`offer_thumbnail_raster`) | 縮小前 raster を元画像の identity で登録 | 切り出し前に渡す (§6.2) |
| metadata の書き出し・取り込み・取り込み後の再読込 | 4 列 | §4.4 |
| 名前変更・ZIP キー移行・画像フォルダ→ZIP 変換 | source の列を付け替え・写す | §4.3 |
| UI の固定状態 (固定ボタン・右クリック・「ピ」バッジ・`GridPin` / `FsPin`・ゲームパッドのリング) | source と比較して toggle | `source` だけで比べる。toggle の解除は行ごと削除、別画像への固定は範囲を消す |
| 一覧の採用 (app.rs:56713〜) | 寸法で自動比率と見開きの寸法記録 | 自動比率は切り出した寸法 (意図どおり)。見開きの記録には渡さない |
| 詳細表示の hover | 一覧の `ThumbnailState` | 追加なし |

### 7.2 範囲・自動を適用しないもの

フルスクリーン・ページ送り・本の中のページ一覧、編集・書き出し・コピー、AI 説明文 (表紙を高解像度で読み直す別機能)、
3D 写真、類似画像索引、動画のピン、RAW の leaf、ドライブ一覧、mIV Remote の本のタイル (Remote は手動の範囲だけ Image ピンで反映)。
自動はさらにフォルダの本と、編集プレビューがある固定ページにも掛けない。

## 8. 範囲指定の画面

### 8.1 入口 (利用者決定 §13-1: 案 A と案 B の両方)

- 案 A: 一覧の**本のタイル** (Folder / ZipFile / PdfFile / 変換済み ConvertibleArchive / ZipDir) の右クリック
  「代表サムネの範囲を指定…」。対象は**そのタイル自身の代表**。
- 案 B: 本の中の**ページ**の右クリック「このページを代表にして範囲を指定…」。対象は今開いているコンテナ
  (既存の「代表サムネに固定」と同じ。source は既存の `folder_pin_selected_source` で作る)。
- egui とネイティブの右クリックメニューの両方 (同じ `MenuCommand` モデル)。キー割り当ては足さない。
- 出さない場所: 実パスの無い合成一覧のタイル、未変換のアーカイブ (既存の固定と同じ disabled)、Ctrl+G の集約、detached viewer、
  ドライブ・共有の根 (ドライブ一覧に反映しないため)。

### 8.2 代表の解決と画像の読み込み (1 つの worker 仕事、r1 指摘 5・12)

- UI は「論理コンテナ (実パス + 種類、ZipDir なら外側の ZIP と実効 prefix)」と設定値 (本棚の明示パスの `Option`・sort・depth・
  種類の表示設定・`archive_cache` の handle・自動の設定等) だけを捕捉して worker へ渡す。既定の本棚パスの解決も worker 側で行う。
  UI で pin DB・stat・ZIP 列挙・`book_root_path()` を呼ばない。
- worker は、一覧と同じ計画で今の代表 leaf を解決し、次をまとめて返す:
  **永続化できる `FolderPinSource`** (対象コンテナからの相対。変換アーカイブは元アーカイブの相対パス + 変換 ZIP 内の entry 名、
  という既存のピンと同じ表現)、**物理の読み先**、**回転キー**、**今の実効の範囲** (cascade の先の段の範囲を含む)、
  **自動の判定結果** (自動 ON・対象の本で、手動の範囲が無い場合)、**元画像のデコード結果** (EXIF 適用済み、長辺は画面の表示寸法 × ppp
  程度で上限 2048)。EPUB の `ReadTarget` lease と取得済みの ZIP bytes はこの仕事の中で使い切る。
- 既存の `resolve_book_cover` (`BookKind` に ZipDir が無く、物理の読み先しか返さない) を、論理コンテナと上の返り値に対応させて共有する
  (一覧・AI 説明文と同じ解決を保つため。`cover_ref` の文字列解析はしない)。
- leaf が画像でない (動画・音声・アイコン・未変換)・RAW・解決できない場合は、画面に理由と「本の中を開いて表紙のページで指定してください」を出す。
- 代表が子の WebP の流用で決まっている場合、解決し直した leaf が今の画素と違うことがまれにある (§1.718 の Astra 指摘と同じ現象)。
  画面に出るのは固定される画像そのものなので、利用者は指定する前に分かる。

### 8.3 画面

- モーダルの `egui::Window` (`ui_dialogs/` に 1 ファイル、`common_modal_dialog_open` に「今表示されるか」の helper で登録)。
  `default_pos()`、1093×614 logical points でも OK / キャンセルが見える (見出しと操作行は本文の外)。
- 状態は「読み込み中 → 表示 → 閉じる」と「失敗 → 閉じる」だけ。worker の結果は世代番号で照合し、閉じた後の結果は捨てる。
- 表示は leaf の保存回転を掛けた向き (在メモリの「キー→角度」owner を引く。DB は読まない)。矩形は回転前の座標と相互に変換する。
- 操作: ドラッグで新しい矩形、四隅・四辺の handle と内部ドラッグで変更 (`export_crop::CropRect::dragged` を流用)。
  初期表示は、手動の実効の範囲 (cascade の先の段のものを含む)、無ければ自動の判定結果、どちらも無ければ全体。
- ボタン: 「OK」/「手動の範囲を解除」/「キャンセル」。Enter / Escape は `dialog_enter_pressed` / `dialog_escape_pressed`。
  - 「OK」: **対象コンテナ自身の行**に、解決した leaf への固定と範囲を書く (`set_with_cover_crop`)。自動の結果を手直しした場合も手動の範囲になる。
  - 「手動の範囲を解除」: 対象コンテナ自身の行を、**同じ leaf への範囲なしの固定**にする (r1 指摘 11)。cascade の先の段の範囲は変えない。
    手動の実効の範囲が無い場合は押せない。自動切り出しが ON の本では、解除後は自動の判定に戻る旨を横に表示する (r2 指摘 11)。
- 補足文: 「範囲は一覧の代表サムネだけに使います。画像そのものや閲覧時の表示は変わりません。」
  「範囲を指定すると、この画像が代表に固定されます。」
  「このページの消しゴム・注釈・切り取りは代表サムネには反映されません (色調補正は反映されます)。」
- 書き込みは既存のピン書き込みと同じ経路 (UI スレッドの単一 UPSERT、`folder_pin_map` 更新、`folder_thumb_pin_dirty`)。
  失敗はトースト (まれな失敗の割り切り)。

## 9. 適用しないもの (まとめ)

§7.2 を参照。

## 10. テスト

- DB: 列の無い旧 DB を開いて列が足され、既存行が範囲なしで読める。旧版の SQL (列名を明示した SELECT / INSERT / UPSERT) が通る。
  旧版の UPSERT で source が変わった行・旧 SQL の名前変更の行で範囲が当たらない。`set` / `set_with_cover_crop` / `clear_cover_crop` /
  `remove`。不正値・一部 NULL・非 leaf の範囲が「範囲なし・ピン有効」。新版の名前変更・ZIP キー移行・画像フォルダ→ZIP 変換で
  有効な範囲が保たれ、無効だった範囲は復活しない。bundle の往復、v12 の bundle の取り込み、不正範囲の取り込み、
  **旧版で固定し直した行 (束縛が外れた範囲) を書き出して取り込んでも範囲が復活しない**。
- key: OFF・範囲なしの key が今と一致 (今の key の文字列を固定した golden test、§5.2 の例を含む)。範囲の有無・値・自動の ON/OFF で
  key が変わる。正規化の関数が冪等 (`|types:` と `|autocrop:` を重ねない)。cascade で子の範囲が親の key に効く。§7.1 の全綴りが helper の
  値になる。ドライブ一覧の前方一致が範囲・自動の行を拾わない。ピンなし ZipDir の自動の key が存続集合に入る。
- worker: 範囲付きの出力の寸法・内容 (既知の画素パターンで切り出し位置を検査)、EXIF 回転済み画像、JPEG の DCT 縮小と PDF の
  render の目標寸法、類似画像索引に元画像の raster と寸法が渡る、範囲付きで編集プレビューを使わない、編集プレビューがある固定ページで
  自動を掛けない、代替要求が範囲付きの key に保存されない、Remote の要求に手動の範囲が付き自動が付かない、自動代表が子の範囲で切った
  画素を作り子の範囲の変更で証明が失効する、自動のデコードし直しが最大 1 回、`CacheOnly` が判定・先読みに進まない。
- 不変条件 K: ドライブ一覧の seed が範囲・自動の行を写さない。seed / writeback が自動 identity の付いた key に触れない。
- 画面: 状態遷移 (読み込み中 → 表示 → OK / 解除 / キャンセル、失敗、閉じた後の結果の破棄)、モーダル中に他の入力が通らない、
  回転 90/180/270° の座標の往復、cascade の範囲の初期表示と解除が対象コンテナだけを変える、小画面・UI 倍率のスナップショット。
- 閲覧への非影響: 範囲付きの本を開いたとき、ページ一覧・フルスクリーンが元画像のまま。
- 自動の判定器: §12.6 の受け入れ。

## 11. 文書

- マニュアル: 代表サムネの節 (`htdocs/mimageviewer/manual/grid.html` ほか固定を説明しているページ) に範囲指定と自動を追記。
  設定ページに自動の項目。製品ページは必要なら 1 行。
- 設計文書: display-pipeline §1.3.2、virtual-folders §3.1.1 (schema・key の例外規則)、ui-responsiveness §4 (新 worker 仕事)、
  keymap-spec (キー割り当てを足さない理由)。

## 12. 自動切り出し

### 12.1 ZipPla の仕組み (調査、2026-11-11)

ZipPlaFork (commit 07955f52、**AGPL v3**) の公開コードを読んだ結果を、仕組みとして自分の言葉でまとめる。
**mIV の実装はこれと独立に作り、コード・構造・定数表を写さない**。比較用の再現は評価用で Git 管理外に置く (§12.6)。

- **外周の余白**: 縮小した画像の四隅の色を見て、両端の隅が似ている辺だけを調べる。辺の色から一定以上離れた画素が 1 つでも
  現れた位置で止める。左右・上下はそれぞれ短い側に揃えて対称に除く。除く量の上限は無い。
  - 外れやすい条件: 片側だけの余白 (スキャンの傾き・のど・シール) では対称化でその軸が除かれない。ほこり 1 点で止まる。
    絵の端が余白の色に近いと削り込む。
- **表紙の位置**: 枠の比率は常に 1:√2。高さ 210 に対し表紙 148・背 15・折り返し 75 の固定の比を仮定し、余白を除いた画像の
  縦横比がどの区間に入るかで「表紙だけ」「折り返し + 表紙」「表紙 + 背 + 裏」… の並びを決め打ちし、区間ごとに決めた位置で切る。
  左→右読みでは位置を左右反転する。区間の境目では位置が表紙幅の 3 割ほど跳ぶ (作者自身も「非常にシビア」とコメント)。
  - 外れやすい条件: 実物の比が仮定と違う (折り返しの幅・背の厚さは本ごとに違う)、余白の取り残しで縦横比がずれる、綴じ方向の取り違え、
    見開きのイラスト (2 等分の片方になる)、横長のイラスト (中央を切る)。画像の中身で表と裏を見分ける仕組みは無い。
- **描画時の注目点**: サムネの枠が 1:√2 でないとき、推定範囲を縮小した画像の 2×2 の勾配から直線上に投票し、最大の点を中心に
  枠を置く。長い直線 (枠線・バーコード) に引かれやすく、平坦な表紙では左上に寄る。
- **候補ページ**: 先頭から順に試し、余白が無い・縦横比が極端なページだけを減点して次のページへ進む (最大 7 ページ)。
  見開きは減点されないので先頭が見開きならその片方を使う。
- 報告者の画像 (5ch >>760、Git 管理外の評価用) では、ZipPla は構成 2 (白い折り返しに作者のコメント) で折り返しを含む範囲を選び、
  32 巻 (構成 3) で裏表紙 (バーコード側) を選んでいる。固定の比と区間の決め打ちが、実物の比のずれで外れる例と読める。

### 12.2 mIV の方式 (利用者案 A・B を採り入れた v5)

判定は worker の純粋関数 `cover_auto_crop(image, page_aspect, binding) -> Result<CoverCrop, AutoCropDecline>` とし、
理由付きで「切らない」を返す。入力は §6.2 の高さ 256 の解析用 raster と、元画像の寸法。

1. **元画像の比の入口**: 元画像 (余白を除く前) の縦横比が 0.85 未満なら切らない (ふつうの縦長の表紙。何も読まない)。
2. **大きさの先読み (案 A、§12.3)**: 本の他のページの縦横比から表紙 1 面の縦横比 `r` を決める。決まらなければ切らない。
3. **外周の余白**: 辺ごとに独立に、その行 (列) の 97% 以上の画素が端の色 (中央値) から距離 40 以内なら余白として内へ進む。各辺 20% まで。
   対称化しない (片側だけの余白に対応)。余白を除いた比 `s` が `1.25 r` 未満なら切らない (背が付いただけの画像など)。
4. **面の境目**: 隣の列との色の差が大きい行が高さの 90% 以上を占める列。スキャンの傾きは左右 3 列 (解析用 raster で約 0.7°) の
   近傍の最大で吸収する。近すぎる境目は強いほうを残す。境目で区切った帯を「面」とする。
5. **カバーである証拠**: 背 (幅 `r·H` の 4〜16%、印刷の密度 60% 以上の細い面) か、平らな面 (幅 `0.15 r·H` 以上で、印刷の密度 35% 未満、
   または明るさの標準偏差 14 未満) が 1 つも無ければ切らない。横長・見開きのイラストは絵の中の縦の線が境目に見えることがあるが、
   背や平らな面を持たないのでここで止まる。
6. **位置は綴じ方向の並びで決める (案 B の前段)**: 表表紙の側 (右綴じなら左端、左綴じなら右端) から見て、
   幅 `0.04 r·H` 未満の帯 (スキャンの端の細片) を飛ばし、次の面の幅が `0.2〜0.8 r·H` で**何か印刷されている** (印刷の密度 3% 以上) なら
   折り返しとして飛ばす (真っ白な帯は折り返しとみなさない)。そこを表表紙の始まりとし、終わりは始まり + `r·H` の ±10% にある境目
   (無ければ始まり + `r·H`)。始まりが折り返しの境目でも、終わりが境目でもなければ切らない (位置の根拠が無い)。
   **折り返しを飛ばさずに画像の端から始まる場合は、終わりが背の境目でなければ切らない** (r3 指摘 3。実物のカバーでは表表紙と裏表紙の
   間に必ず背がある。「絵 + 白地」の横長のイラストはここで止まる)。
7. **色の豊かさは拒否にだけ使う (案 B)**: 候補の点 = 色の豊かさ (反対色成分 r−g・(r+g)/2−b の標準偏差と平均の大きさ、
   Hasler–Süsstrunk の colorfulness) + 印刷の密度 (紙の色 = 画像の明るいほうから 5% の画素の中央値、から色の距離 45 を超える画素の割合。
   黄ばんだスキャンにも合わせる) − 白地の帯 (印刷の密度 15% 未満の列の割合)。表表紙の候補が 0.6 未満 (ほぼ白) なら切らない。
   表表紙の候補と半分以上重ならない同じ幅の窓のうち最も高い点が、表表紙の候補より 0.5 以上高ければ切らない
   (淡い色・白黒の表紙と色豊かな裏表紙、または綴じ方向の取り違えで裏表紙を選びかけている場合)。
8. **出力**: 表表紙の左右と、余白を除いた上下を元画像の正規化座標にした `CoverCrop`。

色の豊かさで位置を決める案 (v3、合成ダミーで最良) は、色豊かな裏表紙・白黒の表表紙で裏表紙を高い確信で選んだ (r2 指摘 9) ので採らない。
位置は「並び + 検出した境目」、色は「拒否」に分けた。綴じ方向は位置の決定に使うが、色の拒否が独立に掛かる。
しきい値は §12.6 の試作の値を初期値とし、実装後に評価用の道具で調整する。調整したら版 (`autocrop:vN`) を上げる。

### 12.3 大きさの先読みの費用 (案 A、r2 指摘 6)

- 入口の判定 (§12.2-1) を通った本だけが読む。対象は代表 leaf と同じ本の、読む順で leaf の次から最大 8 ページ (leaf が最後なら前)。
  画像以外は飛ばす。判定: 読めたページが 3 以上、縦長 (`w/h < 0.9`) の割合が 70% 以上、縦長のうち中央値から ±6% に入る割合が
  70% 以上のとき `r` = 縦長の中央値。横長 (見開き) のページは `r` の計算に使わない。条件を満たさなければ切らない。
- 形式ごとの読み方と上限 (どれも worker 内、デコードしない):
  - **ZIP・変換アーカイブの変換 ZIP (ネストでない entry)**: 既存の entry の先頭部分読み (zip_loader.rs:3087) で最大 256 KB を読み、
    `probe_image_dims_from_bytes` で寸法を得る (EXIF の向き 5〜8 はこの関数が既に入れ替える)。上限は 8 × 256 KB。
  - **ネスト ZIP の中のページ・直接読む RAR**: 部分読みが無い (途中の ZIP を展開する / entry 全体を返す) ので**先読みせず切らない**。
  - **ZipDir**: 代表entryを直接読む要求も含め、共通App builderが`LoadRequest.zip_dir_prefix`を設定し、このscope内だけで前後を選ぶ。
    範囲画面は解決後の同じ要求のprefixを受け取り、外側ZIP全体や独自のprefixへ置き換えない。
  - **PDF・EPUB (変換済み PDF)**: PDF ワーカーの `PageSizes` 要求 (ページを読み込まずに全ページの寸法を返す既存の要求、pdf_loader.rs:2718)。
    ページ数に比例するが render 1 枚より軽いと記録されている。代表の render と同じパスワード (`None` は通常の PDF の指定)・同じ EPUB の `ReadTarget` lease を渡し、
    変換・パスワードの問い合わせは起こさない。パスワードが必要なのに得られていない場合は切らない (r3 指摘 5)。
  - **フォルダの本**: 対象外 (§3)。
- 結果は catalog の行に入るので、同じ本の 2 回目からは読まない。本のファイルの stamp が変われば key の stamp が外れて作り直される。

### 12.4 設定・key・生成

- 設定 `thumb_cover_auto_crop: bool` (既定 OFF)。環境設定のサムネイルの項目に「つながった表紙から表表紙を自動で切り出す
  (書庫・PDF の本)」。ON / OFF の切り替えでは、種類の表示設定と同じく表示した本のタイルから作り直す。
- 対象: ZipFile・変換済み ConvertibleArchive・PdfFile (EPUB 含む)・ZipDir のタイルの代表のうち、手動の範囲が無いもの
  (自動選定・範囲なしのピン)。leaf が RAW・動画・音声、編集プレビューがある固定ページ (§6.3)、ネスト ZIP・直接読む RAR の中 (§12.3) は
  判定に進まず切らない。フォルダのタイルは対象外だが、子の本のタイルの (自動で切った) 行を流用することはある (K2)。
- 綴じ方向は**既定の綴じ方向の設定値**から決め、key に含める。本ごとの見開き設定 (`spread_db`) は読まない (§3)。
- key: §5.2。判定の理由 (切った / 先読みで比が決まらない / 広くない / カバーの証拠なし / 位置の根拠なし / 色の拒否) を perf event と
  debug ログに残す。
- 自動で切ったときの扱いは手動の範囲と同じ (§6.2)。切らないと判定したときは今と同じ画素。

### 12.5 自動と手動の関係

- 手動の範囲がある leaf では自動を使わない (key にも自動 identity を付けない)。
- 範囲指定の画面の初期表示は、手動の範囲、無ければ自動の判定結果、どちらも無ければ全体 (§8.3)。自動の結果を手直しして OK すると手動の範囲になる。

### 12.6 評価用の道具と試作の結果 (Git 管理外)

- 置き場所: `G:\home\mimageviewer-764-eval\` (リポジトリの外。報告者の画像・その切り出し・手元の画像から合成した本はコミットしない)。
  - `proto/cover_auto.py`: 試作 (ZipPla 相当の再現 `zp_crop` と mIV 方式 v3〜v5)。
  - `proto/gen_dummies.py`: 第三者の絵を使わない合成ダミー 78 件 (構成 1〜6 × 通常 / 淡い色 / 白黒 × 右綴じ / 左綴じ × スキャンの余白と
    ノイズの有無、表紙だけ・横長・見開きのイラスト) と正解。
  - `proto/gen_realistic.py`: 手元の画像を素材にした本物らしいカバーの合成 90 冊 (利用者の指示。素材は読み取りだけ。素材の場所・中身は
    記録しない)。構成 1〜6 × 裏表紙 4 種 (小さな絵・文字だけ・全面の絵・色付き地) × 3 冊、表表紙は 7 割が通常・淡い色と白黒が 15% ずつ、
    3 割は左綴じ。紙の黄ばみ・片側の余白・0.7° までの傾き・JPEG の劣化を加え、ZIP に縦長の本文 8 ページ (時々見開き) を入れて
    §12.3 の先読みも通す。表紙だけ・横長・見開きのイラスト各 6 冊。正解は表表紙の 2 次元の矩形。
  - `proto/eval_*.py`: 両方式の結果を並べた一覧画像 (`sheet-*`) と集計。判定は 2 次元の IoU 0.85 以上を正解とする (r2 指摘 9)。
- 試作の結果 (2026-11-11、v5):

  | 評価 | mIV 方式 v5 | ZipPla 相当 |
  | --- | --- | --- |
  | 合成ダミー 82 件 (r3 の反例「絵 + 白地」4 件を追加。綴じ方向は正しく与える) | 正解 57・誤り 0・切らない 25 | 正解 41・誤り 41 |
  | 合成ダミー 82 件 (綴じ方向を全部逆に与える) | 正解 22・誤り 8・切らない 52 | — |
  | 手元画像の合成 90 冊 (綴じ方向は正しく与える) | 正解 59・誤り 0・切らない 31 | 正解 35・誤り 55 (右綴じ固定) |
  | 手元画像の合成 90 冊 (全部右綴じとして扱う。3 割は取り違え) | 正解 49・誤り 5・切らない 36 | 同上 |
  | 負例 (表紙だけ・横長・見開き・絵 + 白地) | 合成ダミー 10 件・手元の合成 18 冊ともすべて切らない | 横長・見開き・絵 + 白地をすべて切る |
  | 報告者の構成図 1〜6 (右綴じ・左綴じ、低解像度) | 構成 1・2・4・5 で表表紙との横の重なり 0.93〜0.98。構成 3 は低解像度で背を検出できず切らない。構成 6 は切らない | 0.89〜0.97 |

  - v5 は「誤って切らない」側に寄せたため、手元画像の合成で約 3 割を切らない (今の表示のまま。手動で指定できる)。
    切らない理由は、カバーの証拠が見つからない (折り返しが写真の一覧などで平らでない)・位置の根拠が無い・色の拒否が多い。
    実装後に評価用の道具で、誤りを増やさない範囲で拾える数を増やす。
  - 報告者の構成図は余白の整った理想的な画像なので、実際のスキャンで ZipPla が外す例はここでは再現しない。
- 実装の受け入れ (Rust): 同じ合成ダミーと手元画像の合成本で、(1) 負例 (表紙だけ・横長・見開き・絵 + 白地) で 1 件も切らない、
  (2) 綴じ方向が正しい場合の誤りが試作 v5 以下 (今は 0)、(3) 正解が試作 v5 以上。テスト用のダミーはテストコードで生成する (PNG をコミットしない)。評価用の道具は
  `#[ignore]` のテストか開発用の example とし、ディレクトリを環境変数で受けて一覧画像と集計を出す (利用者に見せる比較はこれで作る)。

### 12.7 実装時に確かめること (r3 の注記)

- 解析用 raster を高さ 256 にそろえても、JPEG の DCT 倍率・PDF の render 寸法が違えば画素は同じにならない。倍率を変えて判定の矩形が
  安定する (境目の位置が解析用 raster で ±1 列程度) ことをテストで確かめる。揺れるなら解析用 raster の作り方 (面積平均の縮小など) を決め直す。
- EPUB の世代付きの行と存続集合の食い違いは既存の §1.742 で、今回の接尾辞の正規化とは別件。手動・自動それぞれの完成 key で
  「保存 → 一覧の読み直し (`delete_missing`) → 行が残る」の回帰テストを置き、§1.742 に当たる場合はその旨を記録する (直すのは §1.742)。

## 13. 利用者の決定

1. **決定 (利用者 2026-11-11)**: 入口は案 A (本のタイルの右クリック) と案 B (本の中のページの右クリック) の両方。
   キー割り当ては足さない。ほかの入口は追加しない。
2. **決定 (利用者 2026-11-11)**: 範囲を指定するとその画像を代表に固定する。範囲の解除では固定は残る。既存の固定解除で範囲も消える。
   別の画像に固定し直すと範囲は消える。
3. **決定 (利用者 2026-11-11)**: リリースは手動と自動がそろってから (次の版に間に合わなければその次)。実装は手動 → 自動の順、
   自動の設計は今から並行して進める。
4. **決定 (利用者 2026-11-11)**: 範囲で切る表紙 (手動・自動) は元画像から作り、その表紙ページの消しゴム・注釈・編集の切り取りは
   タイルに反映しない。色調補正は反映する。
5. **決定 (利用者 2026-11-11)**: 自動の評価に手元の実際の本を使ってよい。手元の本は表紙が処理済みのものが多いので、代わりに
   報告者の構成図を参考に手元の画像を素材にした合成本で評価する (§12.6)。素材は読み取りだけ、結果は Git 管理外。
6. **決定 (利用者 2026-11-11)**: 自動はファイルの本だけ。フォルダの本は自動の対象外 (手動の範囲指定は使える)。
   ドライブ一覧・Remote の本のタイルには範囲も自動も反映しない。本ごとの綴じ方向は読まず、既定の綴じ方向を使う。
   自動の ON / OFF では種類の表示設定と同じく本のタイルを作り直す。(設計担当の提案に利用者が同意)

## 14. 独立設計レビューの記録

### 14.1 r1 (Sol xhigh、2026-11-11、4a8a46df9 に対して、REVISE)

| 指摘 | 対応 |
| --- | --- |
| P1-1 種類除外の代替要求が範囲付きの key に未切り出しの画素を書く | §6.1: 範囲付きでは元の key を付け直さない |
| P1-2 ドライブ一覧の seed | r2 で方針を変更 (§14.2 P1-2) |
| P1-3 強い K と自動代表への伝播が矛盾 | §5.1 で K1 / K2 / K3 に分けた |
| P1-4 類似画像索引に切った raster が漏れる | §6.2 の順序を変更 |
| P1-5 `resolve_book_cover` が ZipDir と論理ピン元を扱えない | §8.2 の新しい返り値と論理コンテナの入力、解決と読み込みを 1 つの仕事に |
| P1-6 旧版の UPSERT で範囲が別の画像に当たる | §4.2 の束縛列と単一の decoder |
| P2-7 編集プレビューの共通部分と寸法・画面の不一致 | 範囲付きは元画像から作る (利用者決定 §13-4) |
| P2-8 Remote の本のタイルの経路が表に無い | §7.1 に追加。今もピンを見ないので範囲・自動も反映しない |
| P2-9 bundle の版の受け入れ | §4.4 |
| P2-10 RAW の preview と half の画角 | RAW の leaf は範囲・自動の対象外 |
| P2-11 cascade の範囲と親の「範囲を解除」 | §8.3: 対象コンテナ自身を同じ leaf の範囲なし固定にする |
| P3-12 `book_cover_env` の Shell 呼び出し | §8.2: 設定値だけを捕捉し、既定パスは worker で解決 |

### 14.2 r2 (同じレビュー担当、e6e2c5372 に対して、REVISE)

| 指摘 | 対応 |
| --- | --- |
| P1-1 自動の結果を持たないと編集プレビューの優先と cache hit が両立しない | 編集プレビューがある固定ページには自動を掛けない (§3・§6.3)。判定は catalog の経路だけにあり、判定結果で優先順位を変えない |
| P1-2 ドライブ一覧で seed を禁じると範囲付きの行の作り手がない | ドライブ一覧には範囲・自動を反映しない (§3・§5.1・§7.1)。ドライブの根には範囲の入口を出さない |
| P1-3 周辺ページの変更で自動の結果が失効しない | 自動をファイルの本に限る (本のファイルの stamp が中身の変更を表す)。フォルダの本は対象外 (§3・§12.4) |
| P1-4 束縛が外れた範囲を書き出し・移行が復活させる | §4.3・§4.4: 単一の decoder で有効だった範囲だけを書き出し・付け替える |
| P1-5 代替の行を cache-only の読み手が見つけない | §6.1: 誤った画素を出さないことを優先し、除外設定 + 範囲付きピンの間は親・ドライブが子の代替表示を流用しないことを受け入れる。費用の記述を修正 |
| P2-6 先読みの上限が形式全体で成り立たない | §12.3: 形式ごとに定義。ネスト ZIP・直接読む RAR は先読みせず切らない。PDF は既存の `PageSizes`、同じパスワード・lease |
| P2-7 縦長の行の写しの入口条件が判定と一致しない | 写しの近道を削除 (§3)。判定関数には元画像の比の入口を明示 (§12.2-1) |
| P2-8 自動の接尾辞の正規形と存続集合 | §5.2 に綴りの例・冪等性・ZipDir の存続集合・golden test |
| P2-9 色の差は表表紙の確信を示さない | 判定器を v5 に変更 (§12.2): 位置は綴じの並び + 境目、色は拒否だけ。評価を 2 次元・淡色・白黒・全面の絵の裏表紙・傾き・横長の画集に拡張 (§12.6) |
| P2-10 自動のデコード解像度 | §6.2: 解析用 raster を固定寸法で作り、切るときだけ同じ bytes / lease から 1 回デコードし直す。EXIF の向きの記述を修正 |
| P3-11 自動 ON での「範囲を解除」の意味 | §8.3: 「手動の範囲を解除」とし、自動に戻る旨を表示 |

### 14.3 r3 (同じレビュー担当、4b7f2c56d に対して、REVISE、P1 なし)

| 指摘 | 対応 |
| --- | --- |
| P2-1 K2 の流用と K3 の禁止が衝突 | §5.1: K2 の流用を K3 の明示の例外にし、子の writer と同じ key で読む |
| P2-2 自動の代替要求と cache-only の読み手の key 不一致 | §6.1: 手動の範囲が無い要求の代替は自動の指示を保ち元の pinned key に保存 |
| P2-3 「絵 + 白地」の横長イラストを切る | §12.2-6: 端から始まるなら背が必要、真っ白な帯は折り返しでない。反例を負例に追加し、すべて切らないことを確認 (§12.6) |
| P3-4 ドライブ一覧の間接的な流用 | §5.1: 生成しない・印のある行を写さない・印の無い近似行の流用は許容、と範囲を限定 |
| P3-5 PDF のパスワード条件 | §12.3: 「必要なのに得られていない場合」に限定 |
| P3-6 解析倍率の安定性・EPUB の存続集合 | §12.7 の実装時の確認事項 |

### 14.4 r4 (同じレビュー担当、6540da52a に対して、ACCEPT)

- r3 の 6 項目はすべて解消。新しい設計上の問題なし。
- 実装担当への注記: 子の手動 / 自動 / 対象外ごとの key 一致、代替要求の指示の保持、`|types:`・`|autocrop:`・`|generation:` の
  正規化と存続集合を回帰テストで確かめる。解析倍率の安定性と §1.742 は §12.7 のとおり。

## 15. 第1段（手動）の実装と検証（2026-11-11）

- 対象は §4〜§11 の手動範囲。§12 の自動判定・設定・自動 identity の出力は未実装。
- ピンの加法 schema 移行、束縛の decoder、同じ leaf の改名／ZIP 移行／画像フォルダ ZIP 化、
  bundle v13 と v7〜v12 の受け入れ、leaf／cascade／ZipDir の helper、worker 切り出し、両方のメニュー入口と modal を接続した。
- 元画像、ページ一覧、閲覧・編集・書き出しの要求には範囲を付けない。ドライブは範囲のない近似キーを使い、
  最新行を選ぶ SQL の LIMIT より前に範囲・自動 identity のある行を除く。
- 本体の source map、コレクション digest、色フィルタ、インポート後の pin refresh も `FolderPin` に追従。
  detached 関連は bundle の map 型変更だけで、述語・viewport routing を変更しない。

### 15.1 前提の食い違いと判断

- §6.2 の WIC 縮小デコードという旧記述は、利用者承認の `1710930a0` で訂正した。
  WIC・image crate・Susie は従来の全体デコードの後に切り、JPEG DCT と PDF render の目標だけを広げる。
- source-only の既存 resolver 呼び出しとの互換性は `PinSpec` で保つ。実際の DB 読み取りと pin map は範囲を持つ `FolderPin` に統一する。
  有効な Image／ZipEntry かどうかの decoder は、起動時点の Susie 登録状況を参照せず、RAW・既知の動画／音声を除外する。
- ZipDir の直接 ZipEntry 要求は既存の取得済み stamp から解決し、新しい UI stat を足さない。
  直接ピンの特殊綴りと、子 ZipDir へ cascade した場合の経路 identity の綴りを維持する。
- 在メモリ回転と同じ本棚判定を保つため、既定本棚パスの解決は worker に移す。
  独立レビューで見つかった AI 説明文生成・表示側の兄弟 consumer も追従した。
- Remote の Image／Folder ピンには手動範囲を渡す。Remote の個別色調補正キー未設定は既存の動作として維持する。
- §1.742 は今回の範囲外。EPUB の worker 完成キーには generation が付く一方、現行の存続集合には pending キーだけが入り、
  `delete_missing` で行が消える既存の境界を、範囲なし／手動範囲ありの両方で記録するテストを置いた。
  Folder／ZIP／PDF の手動範囲の完成行は別の存続テストで検査する。

### 15.2 検証の担当

焦点テスト、UI snapshot、fmt・グリフ検査、通常 profile の確認用 build は実装担当が行う。
lib 全件・test-full と実機確認は設計担当へ引き継ぐ。独立実装レビューは Sol / xhigh の別コンテキストで行い、
既定本棚回転、200% の画像表示、入力エラーの可視性の指摘を修正後、静的レビュー・PNG の目視とも ACCEPT。

- `cargo nextest run -p mimageviewer --lib -E 'test(/cover_crop|folder_thumb_pin|book_cover|representative_cache_key|remote_image_folder_pin|caption_rotation|cover_rotation_tests|cover_scope_tests|drive_list_pin|color_representative|collection_container_pin|metadata_transfer::tests::.*(version|pin)|zip_key_migration/)' --no-fail-fast`
  — **193 成功 / 失敗 0**。`target/cover-crop-verified-focused.log`。
- `cargo test -p mimageviewer --test ui_snapshot cover_crop`（新規2枚だけ更新）— **2 成功**。
  `target/cover-crop-snapshots-revised.log`。100%/200%とも画像全体・handle・固定ボタンを目視。
- 更新指定を外した `cargo test -p mimageviewer --test ui_snapshot` — **251 成功 / 失敗 0**。
  `target/cover-crop-snapshots-compare.log`。既存 PNG の差分 0。
- `cargo fmt`（引数なし）・`cargo fmt --check` 成功。`python scripts/check_ui_glyphs.py` は 0 件。
  `python scripts/check_site_cards.py` は 53 枚検査・超過 0。
- 初回の native 依存ビルドは MSBuild の並列実行で失敗し、`CARGO_BUILD_JOBS=1` で再実行して解消した。
  反復中のテスト fixture の DirectImage 指定漏れと、v13 追加後のメッセージ期待値更新漏れも修正済み。
  これらの失敗・compile-only 実行は成功件数に含めない。
- `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は同じ並列数で **成功（exit 0）**。
  core・Remote・EPUB PDF・MCP・音楽ビデオ編集の5実行ファイルと依存 DLL を `target/dev-runtime/` に配置し、
  PE 検査も成功（runtime=4 / pe=5）。`target/cover-crop-build-dev.log`。
  `portable` は有効にせず、実行ファイルは起動していない。

### 15.3 コミットの引き継ぎ

実装担当が新規4ファイルを明示して `git add -- <paths>` を試したが、共通 Git 管理ディレクトリ
`C:/home/mimageviewer/.git/worktrees/mimageviewer-764/index.lock` の作成をセッションの書き込み権限で拒否された。
コミットは未作成で、ブランチは `feat-764-cover-crop`、HEAD は設計訂正 `1710930a0` のまま。
変更は作業ツリーに保持し、今回の55パスだけを対象にする `target/cover-crop-commit.ps1` を用意した。
PowerShell の構文検査は成功。権限のある端末で同スクリプトを実行すると、引数なしの `cargo fmt`、
新規4ファイルの明示 add、pathspec commit を行う。merge・push・branch 切り替えは行わない。

### 15.4 第1段の再レビュー修正（基点 `bd3ba8b30`）

初回差分は設計担当が `bd3ba8b30` としてコミットした。以下は独立レビュー指摘を設計担当が確認した後の修正で、
実装担当は追加のコミットを行わず、作業ツリーで引き継ぐ。

- key正規化は既知の派生接尾辞の完全一致を末尾から除く。ZIP内名の `|autocrop:one/` / `two/`、
  内名の途中の `autocrop:v1:rtl` / `ltr`、未知版を保持するgoldenを追加した。
- §4のsource変更トリガーで、旧版のsource-only UPSERTによる a→b→a 後も範囲を復活させない。
  source＋束縛の同時更新、container_keyだけの改名、既存schemaへの一度だけの追加、recursive_triggers OFF/ONを検査する。
  revisionの追加増分を使う箇所は失効用で、正確な増分を検査していた2件の期待値を更新した。
- Ctrl+Gとmain以外のownerを共有の範囲指定受付で除外する。drive/share root、合成タイル、変換前／変換済み書庫、
  別窓mountとmainへの復帰のテストを追加した。別窓側の記録は `detached-rework-plan.md` §11。
- AI説明文の転送テストは現在の `FORMAT_VERSION` を期待する。テスト内のほかの12固定値は旧形式の受け入れを検査するfixtureで維持する。
- 色フィルタの範囲なしidentityは旧 `Option<FolderPinSource>` のDebug綴りを保ち、範囲がある場合だけ追加する。
  固定あり／なしのliteral goldenと、同じページの範囲変更のidentityを検査する。
- PDFのfixtureを描画目標の計算後の境界へ移し、実worker関数から64→128pxの拡張と、PageSizes／Renderで同じ
  password・ReadTarget借用（lease）を検査する。Folder／ZIP／PDFと実ZipDir一覧の準備で、変更前の範囲キーは
  存続集合から消え、変更後のキーと保存行だけが残ることを検査する。
- 初回焦点実行は212件中211成功、失敗1件は新トリガーで旧束縛が消える変換fixtureだった。
  decoder検証を維持するため、変換／改名／ZIP内名移行／exportのstale fixtureはトリガー導入前のDB状態を明示する。
  独立追レビューで見つかったZIP内名の途中の派生tokenと、このfixtureの2件を修正し、静的追レビューはACCEPT。

最終差分の検証（2026-11-12）:

- `cargo test -p mimageviewer --lib metadata_transfer` — **122成功 / 失敗0**。
  `target/cover-crop-revise-metadata.log`。
- `cargo nextest run -p mimageviewer --lib -E 'test(/cover_crop|folder_thumb_pin|representative_cache_key|app::color_filter|rename_key_migration|zip_key_migration|image_folder_zip_metadata|pdf_worker_save_skip_and_harvested_reply_keep_request_correlation|pdf_interrupted_worker_sends_one_correlated_cancellation|book_cover_only_pdf_cancel_harvest_saves_dimensions_and_layout/)' --no-fail-fast`
  — **212件中211成功 / 失敗1**。`target/cover-crop-revise-focused.log`。
  失敗は変更していない `pdf_and_image_store_copy_does_not_wait_for_nonoverlapping_epub_range` の500ms受信待ち。
  `cargo nextest run -p mimageviewer --lib -E 'test(/pdf_and_image_store_copy_does_not_wait_for_nonoverlapping_epub_range/)' --test-threads 1`
  の単独再検証は **1成功 / 失敗0**。テストの待ち時間・内容は変更していない。
  `target/cover-crop-revise-epub-lock.log`。対象212件はこの2実行を合わせて成功を確認し、一括全成功とは区別する。
- `cargo fmt`（引数なし）・`cargo fmt --check`・`git diff --check` 成功。
  `python scripts/check_ui_glyphs.py` は0件。lib全件・test-full・製品起動は実施していない。
- 確認用 `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は **成功（exit 0）**。
  `CARGO_BUILD_JOBS=1` / `CMAKE_BUILD_PARALLEL_LEVEL=1`、通常profile、portableなし。
  5実行ファイルと依存DLLの配置、PE検査（runtime=4 / pe=5）も成功。実行ファイルは起動していない。
  `target/cover-crop-revise-build-dev.log`。

## 16. 第2段（自動）の実装・引き継ぎ（2026-11-12）

基点は第1段と再レビュー修正 `bd3ba8b30` / `6407c82a1`。実装担当はコミットせず、作業ツリーで引き継ぐ。
評価資料・画像はリポジトリへコピーしない。検出は指定の `miv5_crop` とそのhelpersの挙動を独立したRustの純粋関数へ移し、
ZipPlaのコードは参照しない。ディレクトリ評価・独立レビュー・確認用buildは設計担当が行う。

### 16.1 接続と実装上の判断

- `cover_auto_crop` は元寸法の入口、余白、境界、背／平らな面、綴じの並び、色の拒否を理由付きで判定する。
  解析用rasterは高さ256・面積平均（横1解析画素幅、縦2解析画素幅）で統一する。試作の`downscale`は小画像を拡大しないが、workerの縮小済み入力を扱うため、
  §6.2に従って小画像も256へ拡大する。cubicでは低解像度で約3列揺れたため、§12.7に従って正規化を修正した。
  横の背境界を太らせずに縦方向の単発ノイズを平均し、判定しきい値を変えずに、倍率間の境界の安定性を合成・実JPEG DCTテストで確認する。
  100:1を超える細片は、縦長1面が最小1%の幅を持てないため解析を辞退し、拡大による過大な作業領域を作らない。
  **追記**: 上記の縦2画素幅は初回実装の判断。細textureを平らな面へ変える独立レビュー指摘で撤回し、
  判定統計を保持する方式へ修正した（§17）。
- ZIPの通常列挙は入れ子を展開し得るので、先読みは中央目次だけを調べる限定helperを使う。
  既存と同じ名前解決とprefix読取で、同じハンドルから最大8回×256KiBを読む。全entry読取へのfallbackは持たない。
  読む順の次、末尾だけ前を選び、portrait比率と中央値の合意を調べる。PDF/EPUBは同じpasswordとReadTargetでPageSizesを使う。
- 自動identityを追加するのは正規化関数だけ。要求のsetterでidentityとkeyを同時に設定する。
  手動範囲と既知のRAW／動画／音声を除外し、Drive・Remote・ページseed/writebackは範囲なしの名前空間を保つ。
  フォルダのAutoSelectedにはK2のためのidentityだけを付け、フォルダ画像を自動判定しない。
- K2は子のwriterと同じkeyで読む。子の手動範囲・対象外のleaf・FolderのSeeded pinも区別する。
  leafの種類の判定には解決済みの実パスとentryを渡し、空のprobeでRAW／動画／音声を見落とさない。
  手動範囲のない代替要求は自動の指示と元のpinned keyを保つ。
  存続集合には従来の基本・固定keyと現在の自動keyを併せて入れ、前の綴じ方向の自動keyは残さない。
  フォルダ名が`.cr3`や`.mp4`でもFolderの役割を保ち、K2用identityを落とさない。
- 初回デコードの全体を類似索引へ渡してから判定する。切った長辺が表示寸法に足りないときだけ、
  同じJPEG bytes／同じPDF leaseから最大1回再デコードする。CacheOnlyは判定・先読みをしない。
- 設定は既定OFFの加法field。通常Settings DBは汎用serde保存で、RemoteListingSettingsへは追加しない。
  設定転送の明示的な項目一覧へ登録する。ON/OFF、またはON中の既定綴じ方向の変更は、種類の表示設定と同じreloadを使う。
  Collectionのsource digestと再利用判定もON時のidentityを含め、OFF時の旧digestを保つ。
- 範囲画面は既存の1つの準備workerで先読み・判定する。手動範囲を優先し、無ければ自動、無ければ全体。
  固定ページの有効な編集previewがある場合は自動判定しない。自動の初期枠だけでは手動解除を有効にせず、OKで手動の範囲にする。

### 16.2 ディレクトリ評価の実行方法（設計担当）

`cover_auto_crop_directory_evaluation` はignored test。ZIPでは実際のbounded先読み、PNGダミーではtruthのpage_aspectを使う。
比較画像 `sheet.png` と2次元IoUによる集計 `summary.json` を出す。素材を変更せず、出力先は明示できる。

```powershell
$env:MIV_COVER_AUTO_EVAL_DIR = 'G:\home\mimageviewer-764-eval\dummies' # realisticも別実行
$env:MIV_COVER_AUTO_EVAL_OUTPUT = '.\target\cover-auto-eval-dummies'
cargo test -p mimageviewer --lib cover_auto_crop_directory_evaluation -- --ignored --nocapture
```

合成ダミー57/82・本物らしい合成59/90以上、誤り0、負例を一度も切らないという受入判定は、この設計担当の実行結果で行う。
実装担当はこの外部評価を実行しない。

### 16.3 実装担当の自動検証

対象は上記基点に対する未コミットの第2段差分。通常のtest profile・既定features、
`CARGO_BUILD_JOBS=1` / `CMAKE_BUILD_PARALLEL_LEVEL=1`で実行した。

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/cover_auto|auto_cover_crop|auto_crop_scale_tests|thumb_cover_auto_crop|cover_crop|representative_cache_key|folder_thumb_pin|book_cover|cover_rotation_tests|cover_scope_tests|color_representative|collection_container_pin|settings_transfer/)' --no-fail-fast
```

- 最終版は **243成功・失敗0（exit 0）、17127 skipped**。
  `target/cover-auto-stage2-focused.log`。lib全件・test-fullは実行していない。
- 自前生成のカバー16条件（左右綴じ・余白と埃・傾き・折り返しの有無）はすべて2次元IoU 0.85以上。
  負例10条件（表紙だけ・横長・見開き・絵と白地の左右、左右綴じ）はすべて辞退した。
  128/256/512/1024行の入力と、実TurboJPEGの3種類以上のDCT倍率で、左右端の揺れが解析用rasterの1.1列以内。
- ZIPの実先読みとworker保存、CacheOnlyの再利用、種類除外の代替、編集preview優先、
  PDF/EPUBのPageSizesと最大1回の拡大再描画で同じpassword・ReadTarget借用を確認した。
  K2の手動／自動／RAW／動画／音声／Folder Seeded pinの子writer key一致を確認した。
  Folder/ZIP/PDF/ZipDirの現設定キーと従来キーの存続、旧綴じ方向の自動キーの除去、
  OFFのgolden・Collection digest・色フィルタ・設定の保存／転送も含む。
- §1.742のEPUB完成generationキーと存続集合の既存境界は、手動に続き自動付きでも記録した。
  generation付きの完成行が現行のpending集合では残らない点は§1.742の修正対象で、今回変更しない。
- 反復中の3件の失敗は、設定転送の件数期待値、CacheOnly用fixtureの在メモリmap未水和、cubic rasterの倍率間の揺れだった。
  件数を追加項目分更新し、fixtureは既存Fixed providerの契約どおりmapを水和し、rasterは§16.1の面積平均へ修正した。
  続く1件は転送wireの件数期待値の更新漏れで修正済み。最終成功件数にはこれらの失敗を含めない。
- `cargo test -p mimageviewer --test ui_snapshot`（更新指定なし）は **253成功・失敗0（exit 0）**。
  `target/cover-auto-stage2-snapshots-compare.log`。自動ONの範囲画面2枚を追加し、設定欄の
  `preferences_favorite_view_state_dark` 1枚だけを意図的に更新した。新規2枚と設定の更新は限定filterで実行し、
  1093×614の100%/200%で説明・操作行・画像とhandle、設定欄の追加文と後続項目を目視した。
  既存の手動用2枚を含む他のPNGは変更していない。
  最初の全件比較は252成功・設定欄1件の意図した差分で、更新後の全件比較を上記の最終結果とする。
- 引数なしの`cargo fmt`、`cargo fmt --check`、`git diff --check`は成功。
  `python scripts/check_ui_glyphs.py`は0件、`python scripts/check_site_cards.py`は53枚・超過0。
- 指示どおりコミット・確認用build・製品起動・外部ディレクトリ評価・追加の独立レビューは行っていない。
  設計担当は§16.2の評価、別コンテキストの独立レビューと確認用buildを実行する。

## 17. 第1段確認・第2段独立レビューの5件修正（基点 `995f75f63`）

- keyの基礎処理はmasterの関数を保持し、自動トークンの除去・追加だけを正確なbuilder位置で分離した（§5.2）。
  test moduleへmaster関数をoracleとしてコピーし、ZIP内名・複数types・generation・pinの有無を両policyで比較する。
  実builderの固定／ZipDir／EPUB generation／フォルダ出力では、冪等性・自動の往復・policy切替も検査する。
- crop消去triggerはsource値の変更だけを条件にする。新しい保存・コピー・改名・ZIP移行・取り込みは共通helperで
  同一transactionのsource→crop＋束縛の2段に統一した。古い不一致行からの新規保存、旧版a→b→a、
  改名・移行・container keyだけの変更、旧開発triggerの置換と再openの冪等性を検査する。
- EPUB読めずの代替で元のpinned keyを継承するのは自動Some・手動なしだけ。OFFは第1段の代替基本行へ保存する。
  workerから実保存し、OFF/ONそれぞれの行と、誤った別キーの行がないことを検査する。
- 判定rasterの縦2画素幅平滑化を撤回した。300×256、幅70/179/51の3色帯、行ごとの輝度±30と16行ごとの色相変化では、
  旧平滑化で広い面の輝度stdが約30.001から約1.361へ落ち、存在しない平らな面を作っていた。
  反例のRGB値は未提示だったため、同じ寸法・textureと旧処理の解析範囲67..246を再現する生成fixtureを作った。
  旧kernelの入力では切り出しを受理し、nativeの未平滑入力では`NoJacketEvidence`になることを同じ純粋関数で確認する。
  native256の画素・印刷密度・分散・色の点数・境界強度を保持する回帰と、細縞／網点／ノイズの両綴じ方向の拒否を追加した。
  面積平均だけでは低解像度間で約5列の揺れが残った。原因は256行の入力だけで埃が余白の除去を早く止め、
  高さと境界のsupport順位を変えることだった。余白座標の参照だけに3タップを使い、元画素の特徴量を保った。
  小型Rust検証13件は成功、128/256/512/1024行の最大差は約1.00037列。実DCT倍率と元の余白座標のテストも通し、
  許容値1.1列は広げていない。外部評価は生成sheetを開かず、集計だけを確認する。
  **fix4追記**: 3タップの余白参照も模様帯を切除して分散を失わせるため撤去し、余白は`source.get`で判定する。
  埃入りfixtureの倍率間一致は試作の保証ではないため、安定性検査は余白・傾きを残した埃なし入力に変更し、1.1列は維持する。
- 通常ZipDirのdirect entry・直接pin・cascadeに共通の要求baseからsampling prefixを渡す。
  範囲画面も解決済み要求の同じprefixを使う。Aの最後のページの後に比率の違うBがあるZIPで、
  実App要求→samplerと画面用resolverの両方がAの比率を得ることを検査する。

検証は指定の共有`run-heavy.ps1`、owner=`764-codex`を通し、Cargo build/test/checkを同時実行しない。
追加のコミット・確認用build・製品起動・独立レビューは行わず、設計担当へ作業ツリーを渡す。

### 17.1 検証記録

対象は`995f75f63`からの未コミット差分。既定features・通常test profile、共有runnerのsandbox fallbackでは
BelowNormal・`CARGO_BUILD_JOBS=6`、`CMAKE_BUILD_PARALLEL_LEVEL=1`。他セッションの全件ロック待ちも同じrunnerに従った。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner '764-codex' -WorkDir G:\home\mimageviewer-764 -Command "cargo nextest run -p mimageviewer --lib -E 'test(/cover_auto|auto_cover_crop|auto_crop_scale_tests|thumb_cover_auto_crop|cover_crop|representative_cache_key|folder_thumb_pin|book_cover|caption_rotation|cover_rotation_tests|cover_scope_tests|drive_list_pin|color_representative|collection_container_pin|metadata_transfer|zip_key_migration|rename_key_migration|image_folder_zip_metadata|zipdir_thumb_request/)' --no-fail-fast"
```

- 最終焦点実行は **436成功・失敗0（exit0）、16945 skipped**。`target/cover-crop-five-fixes-focused.log`。
  設定済みsource・旧版UPSERT・stale crop保存・migrationの再open・改名／ZIP化／取り込み・master oracle・scopeを含む。
- 最初のcompileは新しいoracle testの`SortOrder` import漏れでテスト未実行、修正済み。
  面積平均だけの反復は435件中433成功で、生成fixtureの境界期待値と約5列の揺れの2件が失敗した。
  fixtureを旧処理の実際の受理まで再現するものへ直し、余白位置参照を分離してから上記最終版を検証した。
  これらの失敗は最終成功件数に含めない。lib全件・test-fullは実行していない。
- 外部評価の今回の基準は、設計担当が提示した現在の入力に対して、正例goodがdummies **35以上**・realistic **29以上**、
  wrong **0**、負例のcrop **0**。§16.2の初回基準は、今回の利用者指示によるこの基準へ更新する。
  出力は`target/cover-auto-eval-{dummies,realistic}-five-fixes/`。sheetを開かずsummaryの集計欄だけを読む。

外部評価の結果（正しい綴じ方向、集計値のみ確認）:

| 入力 | good正例 | wrong | declined合計 | 負例crop | 負例declined |
| --- | ---: | ---: | ---: | ---: | ---: |
| dummies | 36 | 0 | 46 | 0 | 22 |
| realistic | 31 | 0 | 59 | 0 | 30 |

両方とも上記受入基準を満たした。ignored testは各1成功・失敗0（exit0）。
実際のZIP先読みを使い、評価入力をコピーせず、生成sheetは開いていない。
`target/cover-crop-five-fixes-final-checks.log` に集計と実行結果を残した。
同じrunner内の `target/cover-crop-five-fixes-final-checks.ps1` が2評価→整形check→UI snapshotを直列に実行する。

- 引数なしの`cargo fmt`を実施後、共有runner内の`cargo fmt --check`は成功。
- 範囲画面のscope連携を変更したため、更新指定なしの`cargo test -p mimageviewer --test ui_snapshot`を実施し、
  **253成功・失敗0（exit0）**。PNG更新なし。上記final-checks logを正本とする。
- `python scripts/check_ui_glyphs.py`は0件、`git diff --check`は成功。
- 補助の隔離probeの初回native準備はMSBuildで失敗したが、既存の検証用TurboJPEGライブラリを明示して解消した。
  その準備失敗とprobe13件は、本体436件・外部評価2件・UI253件の成功件数へ合算しない。
- 実装担当は追加のコミット、製品起動、確認用build、lib全件、test-full、独立レビューを行っていない。
  設計担当が差分をレビューし、外部評価を再実行してコミットする。

### 17.2 fix4: 模様を余白にしない（基点 `6d4658154`）

`margin_pixel`を撤去し、余白判定へ未平滑の`source.get`を渡す。提示された300×256の生成式をそのままテストへ移し、
余白が`[0,1,300,255]`で、余白処理を通した判定が`NoJacketEvidence`になることを検査する。
中央が平らで上下だけに模様がある帯幅40/50/64、最大振幅24/32/40の3変種（試作も辞退）も、左右綴じで必ず辞退させる。
既存の細縞・網点・ノイズの辞退と、埃なしの合成入力・実JPEG DCTの1.1列の境界検査を維持する。

検証は共有runnerのowner=`764 fix4`で直列実行し、外部評価は集計値だけを確認する。
出力先は`target/cover-auto-eval-{dummies,realistic}-fix4/`。入力画像をコピーせず、sheetを開かない。

- 焦点nextestは`cover_auto|auto_cover_crop|auto_crop_scale_tests|key_normalization_tests|representative_cache_key|cover_crop_key_normalization|thumb_loader::tests::.*cache_key`のfilterで、
  **47成功・失敗0（exit0）、17336 skipped**。`target/cover-crop-fix4-focused.log`。
  初回は追加変種の帯幅50・最大振幅40も辞退すると誤って期待して1失敗だったが、試作も同じ入力をcropすることを確認した。
  上記の試作も辞退する3条件へ直して再実行した。本体の閾値は変更していない。

外部評価は各1成功・失敗0（exit0）で、受入基準を満たした。集計値のみ確認:

| 入力 | good正例 | wrong | declined合計 | 負例crop | 負例declined |
| --- | ---: | ---: | ---: | ---: | ---: |
| dummies | 36 | 0 | 46 | 0 | 22 |
| realistic | 32 | 0 | 58 | 0 | 30 |

実行ログは`target/cover-crop-fix4-final-checks.log`。評価入力をコピーせず、生成sheetは開いていない。

- 引数なしの`cargo fmt`と、同じrunner内の`cargo fmt --check`は成功。
- 更新指定なしの`cargo test -p mimageviewer --test ui_snapshot`は**253成功・失敗0（exit0）**。snapshot更新なし。
- `git diff --check`は成功。製品起動・build-dev・lib全件・コミットは行っていない。
