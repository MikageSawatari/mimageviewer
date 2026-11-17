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
8. **自動切り出しは、ファイルの本 (ZIP・変換アーカイブ・PDF・EPUB とその中の ZipDir) と、専用設定がオンの画像フォルダの本の代表**に掛ける、記録を作らない派生値。
   ファイルの本は中身の変更で、画像フォルダは表紙の変更で判定し直す (§26)。判定できないときは切らない (§12)。
9. **ドライブ一覧と mIV Remote の本のタイルには、範囲も自動も反映しない** (どちらも今もピンの解決を近似・省略している画面)。

## 3. 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- **範囲指定の画面をモーダルにする** (採用)。指定中にフォルダ移動・ピンの変更・回転・別ウィンドウ操作が入らない。
- **代表の解決と画像の読み込みを 1 つの worker 仕事にする** (採用、r1 指摘 5)。「解決済み・未読み込み」の中間状態を持たない。
- **範囲付きの代表は編集プレビューを使わない** (利用者決定 §13-4)。収録範囲 Known / Unknown・共通部分・注釈レイヤーの切り出し・
  画面と結果の不一致の組み合わせを消す。代償は、範囲を付けた表紙のタイルに消しゴム・注釈・編集の切り取りが出ないこと (色調補正は出る)。
- **編集プレビューがある固定ページには自動を掛けない** (r2 指摘 1)。worker は今と同じく編集プレビューを先に確かめ、あれば返す。
  自動の判定は catalog の経路にだけある。判定結果を記録して「切った / 切らなかった」で編集プレビューの優先を変える仕組みを持たない。
- **画像フォルダは専用設定の後ろに置き、表紙が変わったときに判定し直す** (§26 の利用者決定で旧 r2 指摘3の方針を置換)。
  本文ページの同名上書きで判定結果を失効させる仕組みは足さない。既存の代表選択の証明をそのまま使う。
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
- §1.764 の設計時は、EPUB の世代付きの行と pending の存続集合の食い違いを §1.742 の別件として扱った。
  現在は §1.742 で修正済み。手動・自動の切り取り token を持つ完成 key でも、共通の存続照合で
  「保存 → 一覧の読み直し (`delete_missing`) → 行が残る」ことを確認する回帰テストへ更新した。

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
6. **決定 (利用者 2026-11-14、§26で置換)**: 画像フォルダの本も専用設定 (既定オフ) で自動の対象にする。直下の画像だけを判定し、表紙が変わったときに判定し直す。
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
  正規化と存続集合を回帰テストで確かめる。解析倍率の安定性は §12.7 のとおり。
  当時は境界の確認対象だった §1.742 も修正済みで、EPUB の完成行が手動・自動 token 付きでも保持されることを確認する (§12.7)。

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
- 第1段の実装時は §1.742 を範囲外とし、EPUB の worker 完成キーの generation と pending の存続集合の食い違いにより、
  `delete_missing` で行が消える境界を範囲なし／手動範囲ありの両方で記録する見張りテストを置いた。
  現在は §1.742 で修正済みのため、`cover_crop_epub_completed_generation_is_retained_by_folder_cleanup` として
  手動の切り取り token 付き完成行の保持を確認する期待値へ更新した。
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
- 第2段の実装時も、§1.742のEPUB完成generationキーとpendingの存続集合の食い違いを、手動に続き自動付きの見張りテストで記録した。
  当時は完成行が残らない点を§1.742の修正対象として変更しなかった。
  現在は§1.742で修正済みで、`cover_auto_epub_completed_generation_is_retained_by_folder_cleanup` として
  自動の切り取り token 付き完成行も保持される期待値へ更新した。手動・自動とも token の存在と generation を検査し、
  完成キー自体を含まない pending の存続集合で `delete_missing` を通す。
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

## 18. 保存回転のある表紙の自動切り出し (2026-11-12、利用者決定 A)

> **状態 (2026-11-12、利用者決定)**: 今回の版は**回転を見ない自動のまま、制限として出荷する**。回転への対応は §18.6 の D (描画時に切る) で、
> 次の版に入れる ([バックログ §1.773](next-release-backlog.md))。§18.2〜18.4 の rev4 までの案は採らなかった案の記録として残す。
> 出荷する挙動: 手動の範囲は回転に正しく追従する。自動は 1 枚目を mIV で回転した本では、90° は切らずに全体、180° は裏表紙側を切ることがある
> (手動の範囲で直せる)。マニュアルの自動切り出しの節に制限として書いた。

### 18.1 問題 (コードから確認)

- 手動の範囲は「EXIF 適用後・保存回転前」の座標で保存し、範囲画面だけ保存回転を掛けて表示して往復する。タイルは回転前の画素を切って
  描画時に `CoverIdentity::Page(key)` の角度で回す (§1.718)。回転を後で変えても範囲は絵に付いたまま回る。ここは変えない。
- 自動の判定 (`thumb_loader.rs` の auto 分岐、`ui_dialogs/cover_crop.rs` の自動の枠) は保存回転を見ない。90°/270° は縦長として
  `RawAspectGate` で切らない。180° は表表紙の位置が綴じ方向の逆になり、試作では合成本 60 冊を 180° 回すと 12 冊で裏表紙側を切った。
- 利用者の期待: 1 枚目を mIV で回転して正しい向きにした本では、その向きの表紙として表表紙を切り出す。

### 18.2 状態を減らす検討 (CLAUDE.md「設計の簡素化」、rev4)

- **経緯**: rev1〜rev3 は、自動の印に「その本の表紙の出どころの範囲 (scope) に入る回転」のハッシュを入れる案だった。独立設計レビューで、
  遅着の結果 (rev1)・親の印の子への流用 (rev1)・退避した一覧の復元 (rev2)・CacheOnly で scope が消える経路 (rev2)・ZipDir の prefix 外への
  固定 (rev3) と、**scope を作る経路ごとに別の P1 が続いた**。scope を要求・子 (K2)・ピン・cascade・存続キー・復元・採用時の確認の各所で
  作り直す限り、経路を 1 つ足すたびに作り方の食い違いが残る。印を足し続けるのをやめ、scope そのものを無くす。
- **採る: 回転は「綴じ方向」と同じ、アプリ全体で 1 つの自動の識別子の一部にする。**
  - 自動の識別子は今も設定由来の共通の値 (`configured_thumbnail_auto_crop` / `thumbnail_auto_crop_identity`) 1 つで、要求の組み立て・子 (K2)・
    存続キー・色フィルタの再ピンはすべてこれを使っている。ここに **本の中のページ (書庫・PDF の中のページのキー) に保存された非 0 の回転の全体の
    安定ハッシュ** を 1 つ足す: `AutoCropIdentity { binding, book_rotation: Option<u64>, snapshot: Arc<...> }`。等価とキーには binding と
    ハッシュだけを使い、Arc は worker が leaf の角度を引くためだけに運ぶ。本の中のページの回転が 1 つも無ければ今と同じ綴り
    `|autocrop:v1:{rtl|ltr}`、あれば `|autocrop:v1:{rtl|ltr}:r{16 桁 hex}`。
  - 対象を本の中のページに絞るので、**普通の画像 (フォルダ内の画像ファイル) の回転では識別子は変わらない**。自動の判定は書庫・PDF の中の
    ページにしか行わない (画像フォルダの本は自動の対象外) ので、これで足りる。
  - scope の計算・保守的 scope・ピン先や cascade の包含規則は持たない。
- 作り直しの起動・遅着・復元は、既存の境界で「1 つの値を比べる」だけにする:
  1. **回転の変更**: 既存のフック (`rotation_page_owner_changed` → `evict_legacy_cover_thumbnails`、全 viewer context を回る §1.718 の経路) で、
     本の中のページの回転が変わって識別子が変わったら、自動 ON のとき、全 context の自動の対象の本のタイル (`Loaded` / 読み込み中とも) を追い出す。
  2. **退避した一覧の復元**: 退避した一覧は、退避したときの識別子を一緒に持ち、復元するときに今の識別子と違えば、その一覧の自動の対象の本の
     タイルを追い出す (既存の Legacy の復元検査と同じ場所)。
  3. **採用時の確認**: 要求の不変の事実 (`requested.cover_facts`) にその要求の識別子を入れ (CacheOnly を含め組み立て時点の値)、採用するとき
     今の識別子と違えば既存の `evict_thumbnail_for_reload` で捨てて要求し直す。
  専用の待機状態・再要求ループ・行やタイルの状態は持たない。
- **代償 (採用の判断)**: 自動 ON のとき、本の中のページを 1 枚回すと、すべての本の自動のキーが変わり、自動の対象の本のタイルは次に表示する
  ときに作り直しになる (表示中のものはすぐ、他は表示したとき。古い行は既存の `delete_missing` で消える)。本の中のページの回転はまれな操作で、
  自動は既定 OFF の新機能なので、リリース済みの挙動の退行には当たらない。普通の画像の回転・自動 OFF では何も変わらない。
- **ハッシュに入れる回転を「自動の判定が読むページ」だけに絞る案 (統括の提案、2026-11-12) は採らない。** 先読みのページの比率は各ページの
  保存回転を見ない (§18.3) ので、効くのは代表のページ (leaf) の回転だけになる。しかし固定していない本の leaf (先頭の画像) は worker が書庫を
  一覧して決めるもので、UI は回転のキーだけから「どの本の leaf か」を判定できない。判定するには本ごとの leaf を UI 側で持つ必要があり、
  rev1〜rev3 で P1 を出し続けた本ごとの状態を別の形で戻すことになる。代償 (本の中のページを回すたびに自動の本のタイルを作り直す) は
  上のとおり受け入れ、費用は実装時に 1 冊あたりの判定時間を計測して記録する (§18.5)。利用者が本の中のページを頻繁に回す使い方では、
  自動 ON の間は一覧へ戻るたびに表示中の本のタイルの作り直しが起きることを、マニュアルの自動切り出しの節に書く。
- **採らない: 回転の変更を手動の範囲の変更と同じ無効化の入口に通し、回転の値をキーに入れない。** キャッシュの行はキーで分かれているので、
  キーを変えずに作り直させるには行を消す必要があり、§1.718 §2.1 の「書き込み途中の worker の結果が書き戻される」問題に当たる。
- **採らない: 行に判定時の角度を残して読むときに比べる / 全回転の revision (普通の画像を含む) / 自動の本のタイルを本ごとに特定する
  (`Loaded` に状態を足す)。** いずれも rev1〜rev3 の検討どおり。
- 判定の入力: worker は解決した leaf の `CoverIdentity::Page(key)` で、識別子が運ぶスナップショットから角度を引く。キーの印と判定材料が同じ
  スナップショット由来なので食い違わない。範囲画面の自動の枠も同じ識別子 (今の値) から角度を引く。

### 18.3 判定の変更

- 判定は 1 か所の helper にまとめ、worker と範囲画面の自動の枠の両方がこれを呼ぶ。順序は
  **デコード済みの画像 (EXIF 適用後) に保存回転を掛ける → `analysis_raster` (高さ 256 へ正規化) → 縦横比の門と判定 → 得た範囲を
  逆回転で回転前の座標へ戻して `CoverCrop`**。先に解析用 raster を作ってから回すと、90°/270° で高さ 256 の正規化と固定の境界許容の
  意味が変わるため、この順にする。呼び出し前の縦横比の門 (worker の `native` の比、範囲画面側の事前判定) も、この helper の
  回転後の寸法で一本化する。
- 範囲は回転前の座標で持ち、タイルは今までどおり回転前の画素を切って描画時に回す。手動の範囲・タイルの描画は変えない。
- 2 枚目以降のページの比率は、今までどおり EXIF 適用後の寸法から取り、各ページの保存回転は見ない (対象は 1 枚目だけ向きが違う本。
  ページを個別に回した本はまれで、比率は複数ページの一致で決める)。
- 範囲画面の補足文「既定の綴じ方向で判定します」は、画面上の設定名に合わせて「閲覧表示の横連結の方向で判定します」の趣旨に直す
  (snapshot 更新を伴う)。マニュアルの同じ言い方は直した。
- まれな失敗は既定の割り切り (ログ + 通知)。

### 18.4 テスト

- 判定 helper: 90°/180°/270° に回して保存した合成の表紙に保存回転を渡すと、正しい向きの同じ表紙と同じ表表紙 (回転前の座標に戻した範囲) を
  得る。回転を渡さない 90° は今までどおり `RawAspectGate`。180° で回転を渡すと裏表紙側を切らない。回転後に解析化する順序を、
  先に解析化した場合と結果が異なる入力で固定する。
- 識別子とキー: 本の中のページの回転が無ければ今の綴りと同一 (master の関数の oracle 比較を維持)。本の中のページの回転が変わると印が変わり、
  普通の画像の回転では変わらない。正規化は `:r{16 hex}` 付きの印もビルダーの位置にあるときだけ扱う。子 (K2)・存続キー・色フィルタの
  再ピンが、要求と同じ識別子を使う (共通の値から作る) ことを既存のテストの形で確かめる。
- 追い出し: 自動 ON で本の中のページの回転を変えると、全 context の自動の対象の本のタイルが追い出される。普通の画像の回転・自動 OFF では
  追い出さない。
- 復元: 一覧を退避 → 本の中のページを回転 → 復元すると、その一覧の自動の対象の本のタイルが追い出される。回転の変更が無い復元では追い出さない。
- 採用: 回転の変更前に出した要求 (CacheOnly を含む) の結果が変更後に届くと捨てられ、要求し直した結果が採用される。変更が無ければそのまま採用。
- worker: ZIP・PDF・ZipDir (prefix 外への固定を含む)・フォルダが子の本の画像を引き継ぐ場合で、leaf の保存回転が判定に使われる。
- 手動の範囲＋回転を描画まで通すテスト (4efd694a5) と合わせて、手動は回転を変えてもキー・範囲が変わらないこと。

### 18.6 rev4 のレビューと Astra の追加確認 (2026-11-12)

- rev4 の独立設計レビュー = REVISE。P1: 全体の識別子が変わると全ての子の行のキーが変わり、フォルダの自動代表 (子の本の作成済みの行を
  流用するだけで、子を生成しない: `thumb_loader.rs` の K2) が、子を表示し直すまで消える。P3: `rating_view.rs` の存続キーと範囲画面の環境が
  共通の識別子を経由していない。
- 設計とレビューの往復が rev1〜rev4 で収束しないため、AGENTS.md の条件で Astra (gpt-6-astra / medium、読むだけ) に、
  A: rev4 のまま / B: 物理の入れ物のパスで絞る / C: 行に判定時の leaf の角度を残して読むときに比べる / D: 描画時に切る、の比較を依頼した。
- Astra の結論は **D**: 自動用の素材に「切っていない全体の画像・leaf の identity・4 方向それぞれの判定結果」を一体で持たせ、描画時に今の角度の
  結果を選ぶ。素材の有効性が保存回転に依存しないので、回転用のハッシュ・追い出し・遅着の棄却・復元時の検査がすべて不要になる
  (§1.718 の「角度は描画時に引く」の延長)。C は子の行を作り直せない K2 の読み手があるため、フォルダの代表の消失を解消しない。
  D で押さえる境界: 判定の生成 (4 方向・判定済みで切らないも結果として持つ)、範囲画面、永続行・メモリ行の形式 (自動の名前空間の版を上げる)、
  全てのキャッシュ読み取り (K2 はコピーのみ)、worker→UI の素材の受け渡し、コピー・再利用の経路、描画 (UV・寸法・hover・当たり判定を
  1 つの helper で)、対象外の利用者 (手動範囲・編集 preview・通常ページ・ドライブ一覧・Remote) に漏らさない。全体画像は全方向の切り出しに
  必要な解像度で持つ。工数の目安は 19〜38 時間で、今日中の完成は見込めない。
- 本の中のページの回転は「まれ」という rev4 の前提は、利用者の使い方 (スキャンの向きを頻繁に直す) と食い違うため撤回する。
- 利用者の決定 (2026-11-12): 今回は制限として出荷し、D は次の版で入れる (§1.773)。

## 19. 範囲画面の操作配置とドラッグ余白（2026-11-13）

配布ビルドで利用者が観測した操作のしづらさを、表示・配置だけの変更で直す。

- 右クリックの範囲指定は、解決済みの共通メニューtreeを再帰探索し、表示されている代表固定の直後・同階層へ挿入する。
  固定項目が非表示のときだけ区切り付きで末尾へ置く。native/代替メニューで同じnodesを使い、設定の保存形式・項目IDは変えない。
  現在の並べ替え設定は同階層内の編集だけなので、入れ子への追従は合成treeを共有helperへ渡す回帰で検査する。
- 操作行を下端のスクロール外へ移し、左に「  OK  」「キャンセル」、右に「手動の範囲を解除」を置く。
  footerを先に下から配置し、ボタン・検証エラー・補足文の実高さと間隔を除いた残りだけを本文へ渡す。
  見出し、Enter/Escape、各操作の有効条件と保存経路は維持し、検証エラーは操作行の直下に固定する。
  最小幅への縮小時は、非表示の計測用UIで得たボタンの実寸・間隔・右端余白から必要幅を判定し、
  収まらないときだけ解除をOK／キャンセルの下の行へ左寄せする。2行の実高さも本文の残り高さへ反映する。
  100%／200%で実際の窓の右辺を最小幅までドラッグし、手動範囲あり・自動ON/OFFの3ボタンの
  非重複・窓内への収容・クリック到達性を検査する。
- 画像の左右は24ptとし、角の判定9pt＋ウィンドウの標準リサイズ判定10ptに5ptの間隔を取る。
  上下は説明・操作領域に接するため、角の判定が収まる12ptにする。説明文は画像の下にまとめる。
  窓の外側には40ptを目安に余白を取り、小画面では従来の16ptまで縮めて画像の高さを優先する。
  全体枠の角・辺の判定がcanvas内に収まり、実ドラッグが窓を移動・リサイズしないことを100%/200%で検査する。

200%の目視確認で編集領域が小さすぎたため、上記の上下12pt・説明文の後置・外側16ptを追加修正した。
外側の余白は高さ360〜480ptで16〜40ptへ補間し、最大窓幅は左右の増分40ptと
説明文用のスクロールバーの実予約幅を補って900pt＋予約幅とする。
旧版1822cd0efの描画関数を一時テストmoduleで実行し、同じ720×280のfixture・フォント・8フレームで
`cover-crop-canvas-geometry`を測定した。1093×614の100%はcanvas高333.875pt／画像高325.875pt、
200%は手動101pt／93pt・自動83.5pt／75.5pt。双方がこれら以上になる回帰を残す。
修正後は100%が349.875pt／325.875pt、200%が手動166pt／142pt・自動148.5pt／124.5ptで成功した。
固定操作のクリック・検証エラー・全面枠の実ドラッグを含む幾何の焦点4件も成功。

マニュアルgrid.htmlの範囲説明にはメニュー内の位置・ボタンの配置指定が無いため、公開文書は変更しない。
snapshotは共通rendererを使う手動・自動の各100%/200%だけを意図した更新対象とし、ほかのPNGが変われば止めて報告する。

検証: 共有runner（owner=`764ui`）で初回UI/メニュー25件、焦点`cover_crop|context_menu`267件が成功。
snapshotは指定4件を更新して目視し、更新指定なしの`ui_snapshot`257件が成功した。
`cover_crop_small_{100_light,200_dark}.png`と`cover_auto_crop_small_{100_light,200_dark}.png`の4枚だけが変更され、
残り548枚のSHA-256は一致。更新時の比較用`.old/.diff`を誤検出した照合は、元画像のSHA-256一致を確認して区別した。
通常比較後は比較用出力も消え、意図しないPNG変更0。`cargo fmt`/整形check・グリフ検査0件・`git diff --check`も成功。
初回のTurboJPEG/MSBuild停止とLibRawのinclude参照は、同じbundled header・/MT・SIMDを照合した静的libを
`target/764ui-native/`へ実ファイルコピーし、検証コマンドの環境だけで明示して解消（依存設定ファイルは無変更）。
正本ログは`target/764ui-focused-snapshot-update.log`と`target/764ui-snapshot-final.log`。製品起動・全件lib・build-dev・コミットは行っていない。

追加修正後も共有runnerで焦点`cover_crop|context_menu`268件、指定4枚の更新、更新指定なしの
`ui_snapshot`257件が成功。4枚を目視し、残り548枚は元のSHA-256と一致した。
追加修正のログは`target/764ui-baseline-measure.log`、`target/764ui-followup-geometry-final.log`、
`target/764ui-followup-snapshot-update.log`、`target/764ui-followup-final.log`。

最小幅での折り返しの追補検証は、実リサイズ・クリックを含む焦点5件と
`cover_crop|context_menu`269件、更新指定なしの`ui_snapshot`257件が成功。
広い画面の画像高は上記の値を維持。ログは`target/764ui-wrap-focused.log`、`target/764ui-wrap-final.log`。

## 20. 開く回ごとの窓の初期配置（§1.777、2026-11-14）

修正前の実描画テストで、固定idのegui状態が残り、再表示の`default_size`を使わないことを確認した。
1100×620から1920×1080へ変えて開き直しても、100%では914×540pt（初回なら914×1000pt）、
200%では518×278pt（初回なら880×460pt）のままだった。

バックログに併記された「開いた初回だけ位置・サイズを指定し直す」方式を採用する。
現在のeguiには窓単位の状態削除APIがなく、新idを増やすとArea・Resize・scroll・animationの状態が残るため、
固定idを維持する。`CoverCropDialog`が`Initial / UserManaged`の配置状態を所有し、最初の描画でだけ
現在の表示領域に基づく位置とサイズを`current_pos`・`fixed_size`で適用する。その後は利用者の操作を保持する。
初期化をworkerのphaseに結び付けず、読み込み完了・検証エラーでは窓を配置し直さない。

窓idの他の参照は幾何テストだけで、モーダル登録・浮遊層guardはidに依存しない。
固定idと`cover-crop-canvas-geometry`は維持し、他の窓を一括resetしない。
100%/200%で再表示、読み込み中の移動・リサイズと完了後の保持、表示中のhost拡大、反復表示時の
egui data数の上限・他の窓の状態の不変を検査する。§1.773の回転対応はこの変更に含めない。

検証は共有runner（owner=`773`）で新規3件・`cover_crop`73件・通常の`ui_snapshot`257件が成功。
修正前の再現ログは`target/773-premise-final-before.log`、修正後は`target/773-final.log`。
初回のTurboJPEG/MSBuild停止は同版・同条件の静的libと一致するheaderを`target/773-native/`へ実体コピーし、
検証コマンドだけで`target/773-native-env.ps1`を使って解消した。製品の依存設定は変更していない。

## 21. 回転した表紙の自動切り出しを描画時に切る (§1.773、2026-11-13)

### 21.1 目的と前提

- 利用者決定 (2026-11-12): §18.6 の D を次の版で入れる。表紙ページを mIV で回転 (R/L、rotation_db) した本でも、回転した向きの表紙として表表紙を切り出す。
- 今 (v5.2.0) の挙動: 自動の判定は worker で回転前の画像に対して行い、切った画素を保存・送信する。90°/270° は縦長として切らず、180° は裏表紙側を切ることがある。手動の範囲は回転前の座標で保存し、描画時に回すので正しい。
- あわせて直す: 判定用の画像がサムネイルの表示サイズの設定で変わる件 (§1.773 の追記、評価器と実アプリの不一致)。判定の改善 (背の局所変化・白い袖) は §22 で別に扱う。

### 21.2 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- §18.2〜18.6 の経緯のとおり、回転をキーや識別子に入れる案は、作り直し・遅着・一覧の復元・子の行の流用のたびに別の状態が要った。
- D はその逆で、**素材の有効性を回転に依存させない**: 素材は「切っていない画像 + 4 方向それぞれの判定結果」で、どの角度でも同じ素材が正しい。角度は §1.718 と同じく描画時に在メモリ owner から引くだけ。回転の変更・遅着・退避一覧の復元・子の行の流用で、作り直しや比較は要らない。
- 追加する状態は「素材に付く 4 方向の判定結果」1 つだけで、作られたら変わらない値 (回転と時系列で一致/不一致にならない)。

### 21.3 素材と型

- `AutoCropResults`: 4 方向 (保存回転 None/Cw90/Cw180/Cw270) それぞれの `Option<CoverCrop>`。範囲は回転前の座標 (EXIF 適用後、ppm) で持つ (`CoverCrop` の既存の意味と同じ)。`None` は「その向きでは判定して切らない」。素材が自動の対象でない (自動 OFF、手動範囲あり、対象外の形式) ことは、`AutoCropResults` を持たないことで表す (`Option<AutoCropResults>` の `None`)。「判定済みで切らない」と「自動ではない」を型で区別し、欠落を「切らない」に化けさせない。
- 運ぶ場所 (今の自動の範囲が通る経路だけ): worker の結果 `ThumbPixels`、キャッシュ行 `CacheEntry` と catalog の行、子の本の行の流用 (`CachedPinnedFolderThumb` → `save_auto_folder_bytes` → 送信)、`ThumbnailState::Loaded`。
- catalog: `thumbnails` に NULL 可の列 `auto_crops TEXT` を加法移行で足す (既存の `add_thumbnail_column_if_missing` と同じ。読み取り専用の handle の列の有無の検出と、全 SELECT での NULL 読み)。値は 4 方向の固定形式の文字列 (変換は 1 か所)。
- **自動の印を `|autocrop:v2:{rtl|ltr}` にする**: v5.2.0 が保存した v1 の行は切った画素なので、新しい形の行と取り違えない。v1 の行はキャッシュなので移行せず、既存の存続キー (`delete_missing`) が v2 だけを残すことで消える。キャッシュの作り直しは自動 ON の本だけで、1 回。

### 21.4 worker (判定と保存)

- 判定用の画像 (§1.773 の追記): **表紙の元の寸法だけで決まる固定の縮小率**でデコードした画像から `analysis_raster` を作る helper を 1 つ置き、worker・範囲画面・評価器が共用する。JPEG は元の寸法から決めた DCT 縮小率、PDF は元のページ寸法から決めた描画寸法、その他は全体デコード。サムネイルの表示サイズの設定には依存しない。表示用のデコードと別に 1 回デコードが増える場合がある (自動 ON の対象の本だけ)。
- 4 方向の判定: 判定用の画像に保存回転を掛ける → `analysis_raster` → 縦横比の門と判定 → 逆回転で回転前の座標へ (§18.3 の順序)。縦長の画像は 0°/180° で、横長の画像は 90°/270° で門に落ちるので、実質 2 回分の費用。
- 保存する画素: 切らない全体の画像。どの向きの範囲でも、切り出した部分の長辺が表示に要る大きさ (`display_px`) に届くよう、全体を大きめにデコードして保存する (切り出す向きが 1 つでもある素材だけ。目安: 最も小さい範囲の長辺が `display_px` に届く倍率、ただし全体の長辺に上限 (例 4096) を置く)。範囲の無い素材は今と同じ大きさ。
- `source_dims` / `layout_dims` は全体の元寸法のまま (切らない行と同じ意味)。今の「切った寸法を入れる」扱い (`apply_cover_crop`) は自動には使わない。手動の範囲は今どおり worker で切る。
- 色フィルタの代表の色: 今は切った画素から取っている。描画時に切ると向きで変わるので、**全体の画像から取る**ことにする (自動の本では裏表紙・袖の色も含む。精度の割り切りとして記録)。代わりに向きごとに色を持つ案は状態が増えるので採らない。

### 21.5 描画と読み手

- 角度は今どおり `grid_tile_rotation*` (本のタイルは `Loaded.cover = Page(key)` から) で引き、`AutoCropResults` からその角度の範囲を選ぶ。
- 1 つの helper `book_tile_view(loaded, angle) -> (uv_rect, displayed_size)` を置き、次がすべてこれを使う: グリッドのタイルの描画 (`draw_thumb_texture` と `draw_rotated_image_ex` の UV の部分範囲) と当たり判定の矩形、描画後の再描画、詳細表示のホバー (`details_thumbnail_preview_size`)、範囲画面の初期の枠 (その角度の範囲)、一覧の縦横比のサンプル、高画質化の判定 (`rendered_at_px` と `source_dims` は表示される範囲の大きさで比べる)、コレクションの並べ替え画面のテクスチャ表示。
- 対象外のまま: Remote (自動の行を読まない)、ドライブ一覧 (自動を使わない、`|autocrop:` を除外)、手動範囲・編集 preview・通常ページ・仮想フォルダの seed/writeback (自動の印の無いキーだけを扱う)。
- VRAM: 自動で切る本は全体の画像を持つので、その本のテクスチャが大きくなる。VRAM の集計は既存どおりテクスチャの実寸で数える。

### 21.6 範囲画面

- 判定は worker と同じ helper (判定用のデコード + 4 方向) を使う。初期の枠は「手動の範囲 → 今の回転の向きの自動の範囲 → 全体」の順。今の「0° の自動の範囲を回して表示する」は直す (90° 回した表紙で枠が合わないため)。
- 補足文「既定の綴じ方向で判定します」を、画面の設定名に合わせて「閲覧表示の横連結の方向で判定します」の趣旨に直す (§1.773 のあわせて直すもの)。

### 21.7 テスト

- 4 方向の判定: 合成の表紙を 90/180/270° 回した画像に、その保存回転を掛けて判定すると、正しい向きの範囲 (回転前の座標に戻したもの) と一致。90° で回転を掛けなければ門に落ちる。
- 判定用のデコード: JPEG の表示サイズを変えても判定結果が同じ。評価器が同じ helper を通る。
- 保存と読み: v2 の行の保存・読み・子の流用・送信で `AutoCropResults` が欠けない (欠けると型で失敗する)。v1 の行は読まれず、存続キーで消える。
- 描画: 角度を変えると、作り直しなしでタイルの UV・寸法・当たり判定が変わる。退避した一覧の復元・遅着の結果でも、今の角度で正しく描く。
- 評価: 合成本・合成ダミーで、回転なしの結果が §17 の基準を保つ (wrong 0、負例の切り出し 0、陽性 good は試作以上)。90/180/270° に回して保存回転を渡した場合も同じ件数。

### 21.8 まれな失敗

- 既定の割り切り: 判定や読み込みの失敗はログを残し、その素材は「自動ではない」(全体を表示) として扱う。

## 22. 判定の改善 (背の局所変化・白い袖) と実データの評価 (§1.773、計画)

- 対象: §1.773 の追記にある、利用者の実データの PDF で、全高にわたるパネルの境目が無いために見送られる表紙 (絵が背をまたいで続く表紙)
  と、白い袖が余白として外されて位置の根拠を失う表紙。
- 方針: ページの比率から決まる表表紙の幅を前提に、背のあたりの局所的な変化 (細い帯の色・模様・文字の向きの変化) を探す。白い袖を余白として
  外さない (余白は外周の一様な帯だけとし、袖として幅と内容を持つ帯は残す)。閾値を緩めるだけの変更はしない (§1.773 の追記: 誤りと負例の切り出しが出る)。
- 評価 (利用者の了承 2026-11-13、扱いは Git 管理外・件数と数値だけ):
  - 正例: 利用者が閲覧を許可した実データ 17 冊 (うち 8 冊は保存回転が必要。回転の記録は無いので、利用者の回答「正しい向きは回転後」を正解にする)。
  - 負例: 同じフォルダのほかの本のうち 1 枚目が縦長のもの (入口の縦横比で見送るはず)。
  - 正解なし: 同じフォルダのほかの本のうち 1 枚目が横長のもの (画像を見ずに正解を決めない。切ったかどうかの件数だけを記録)。
  - 合成本・合成ダミーの基準 (§17) は保つ: wrong 0、負例の切り出し 0、陽性 good は試作以上。
- 画像の扱い: 利用者が閲覧を許可した本以外の画像は、設計担当・調査エージェント・実装担当のいずれも画像として開かない。判定と評価は数値の診断だけで行う。
- 進め方: §21 の実装 (判定用デコード・4 方向) の後で、試作で証拠の候補を比べ、合成と実データの両方の件数で採否を決めてから Rust に入れる。

### 22.1 試作の結果 (2026-11-14、数値だけ)

試作は Git 管理外 (評価フォルダの `proto/`)。実データの本の名前・画像はここに書かない。実データの正例は回転後の向き (§22 の正解) で判定した。

| 案 | 中身 | 採否 |
| --- | --- | --- |
| V1 | 余白として外す幅の上限を各辺 20% → 5% | V3 に含める |
| V2 | V1 + 「表表紙 + その外側の帯 1 つ」の 2 枚つながりの規則 | 正方形に近い 1 枚絵の負例を 18/18 切ったので不採用 |
| V3 | V2 + 外側の帯が紙に近い条件 + 帯の幅の上限 + 端の細い背 | 採用 (レビュー r2 で下の V3' に改めた) |
| V3' | V3 + 新しい候補にも v5 の拒否条件 (弱い表表紙・別の位置のほうが濃い) + 外側の帯幅を最終の境目で `0.8w` まで | **採用** |
| V4 | V3 + 裏表紙の ISBN バーコードから背の位置を決める | 誤りが出たので不採用 (合成 6・実データ 5) |

| 評価対象 | 今の判定 (v5) | V3 | V3' |
| --- | --- | --- | --- |
| 合成本 (realistic、90) | good 29 | good 42、wrong 0、負例の切り出し 0 | good 41、wrong 0、負例の切り出し 0 |
| 合成ダミー (82) | good 35 | good 51、wrong 0、負例の切り出し 0 | 同左 |
| 正方形に近い 1 枚絵の負例 (18) | 切り出し 0 | 切り出し 0 | 切り出し 0 |
| 実データの正例 (17、回転後の向き) | good 2 | good 9、wrong 0 | good 9、wrong 0 |
| 実データの正解なし・横長 (7) | 切り出し 0 | 切り出し 1 (正否は利用者に確認中) | 同左 |

- 試作の件数は Python の試作のもので、Rust に入れたあと §21 の判定用デコード経路で数え直す (§17 と同じく、件数は試作と一致しなくてよいが wrong 0・負例 0 は必須)。
- 実データの正例で V3 でも切れない 8 冊は、袖・表表紙・背・裏表紙が強い境目なしでつながる表紙。ISBN バーコードは 10/10 冊で見つかり、
  負例での誤検出は 0 だったが、バーコードから背を決める V4 は誤りを出した。バーコードは次に背を探すときの手がかりとして残し、今回は入れない。
- 実データの表紙は、中のページより 8〜15% ほど幅が広い (カバーの折り返し分)。表表紙の幅は §17 の推定 (ページの比率 × 高さ) に対し 0.95〜1.15 倍の幅で探す。

### 22.2 Rust に入れる規則 (V3')

判定の入口・縦横比の門・v5 の判定はそのまま。次の 2 点だけを変える。

1. **余白の上限**: 外周の一様な帯を余白として外す幅を、各辺 `0.05 ×` その辺の長さまでにする (今は 0.2)。白い袖や白い地の表表紙を余白として外しすぎて、
   位置の根拠 (ページの比率に合う幅) を失うのを防ぐ。
2. **2 枚つながりの規則**: v5 が `AutoCropDecline` の `NoJacketEvidence`・`NoLocalEvidence`・`NoSpine`・`WeakFront` で見送ったときだけ
   (`RawAspectGate`・`PageAspectMissing`・`NotWide`・`NoStructure`・`RicherElsewhere` では試さない)、余白を外した範囲に次を試す
   (綴じ方向が左綴じなら左右を反転して同じ計算をし、結果を戻す)。`w` = ページの比率 × 高さ (表表紙の推定幅)。
   - 表表紙の端 `end`: 範囲の端から `0.04w〜0.16w` 内側に厳しいパネルの境目 (v5 の `panel_boundaries`) があれば、そのうち最も端に近いもの
     (端の細い背を外す)。無ければ範囲の端。
   - 形の条件: `end / w` が `1.25〜1.8`。外なら見送る (表表紙の外側に帯が 1 つだけある形の大まかな門。帯の幅そのものは下の `bx` で検査する)。
   - 境目 `bx` を `[end − 1.15w, end − 0.95w]` (ただし `0.2w` 以上) で探す。各位置の左右 `max(3, 0.08w)` 列の帯について、
     平均色の差 `dm` + 0.5 × 標準偏差の差 `ds` + 0.3 × 縦の境目の強さ `edge` が最大の位置。最大値が 40 未満なら見送る。
   - 外側の帯の幅: 最終の `bx` (= 外側の帯の幅) が `0.2w` 以上 `0.8w` 以下。外なら見送る。
   - 外側の帯が紙に近い条件: 列ごとの「インク」(v5 の `column_features` の ink) の平均を `outer_ink` = `[0, bx)`、`front_ink` = `[bx, end)` として、
     `outer_ink <= 0.35` かつ `front_ink - outer_ink >= 0.2` (符号つき。外側のほうが濃い逆転は見送る)。満たさなければ見送る (V2 が 1 枚絵を切った原因をこれで除いた)。
   - 共通の拒否条件: 候補 `[bx, end)` に v5 と同じ `WeakFront` (表表紙の濃さ < 0.6) と `RicherElsewhere` (重ならない同じ幅の別の位置のほうが 0.5 以上濃い) を掛ける。
     該当すれば見送る (見送りの理由は 2 枚つながり側の理由として区別して記録する)。
   - 切り出す範囲は `[bx, end)` × 余白を外した高さ。
3. 閾値・係数は試作の値をそのまま定数にし、ほかの判定の閾値は緩めない。

### 22.3 テストと評価

- 合成のテスト: 2 枚つながり (外側の帯が紙・表表紙に絵) は切る。外側の帯にも絵がある 1 枚絵・正方形に近い絵は切らない。端の細い背は外す。
  外側の帯幅が `0.8w` をわずかに超える形は見送る (形の門は通る境界例)。外側のほうが濃いインクの逆転は見送る。v5 が位置の門だけで見送り、
  2 枚つながりの候補が `WeakFront`/`RicherElsewhere` に当たる負例は見送る。
  白い地の表表紙の余白を 5% までしか外さない。右綴じ・左綴じ、§21 の 4 方向でも同じ範囲。
- 評価器: dummies・realistic を §21 の判定用デコードで回し、wrong 0・負例の切り出し 0・good は v5 以上。4 方向の件数は §21.11 と同じく
  再符号化の揺れを許す (wrong 0・負例 0 は全方向で必須)。
- 実データ: 設計担当が Git 管理外で数え、件数だけをここに記録する。実装担当は実データを使わない。

### 21.9 設計レビュー r1 の反映 (2026-11-14)

独立設計レビュー (Sol xhigh) r1 = REVISE (P1 1・P2 2・P3 1)。基本構成 (描画時に切る・全体の元寸法を持つ・印を v2 に分ける・色は全体から) には異論なし。

- **P1 v1→v2 の直後にフォルダの代表が戻らない**: 子の本の行の流用 (K2、`thumb_loader.rs` の `cached_candidate`) は子の完成行を読むだけで、無ければ子を生成しない。
  v2 に切り替えると子の行が見つからず、子の一覧を開くまでフォルダの代表が消える (v5.2.0 と行き来しても再発)。
  → **K2 の 1 か所だけに互換の読み取りを置く**: 子の v2 の行が無いときに限り、同じ子の v1 の行 (v5.2.0 が保存した切った画素) を「自動ではない素材」
  (`AutoCropResults` なし、画素はそのまま) として流用する。表示は v5.2.0 と同じで消えない。子の本を表示すると v2 の行ができ、以後はそちらを使う。
  v1 の行を流用して作った親の行は、v1 由来であることを素材に持たせ (自動ではない素材なので回転の向きでの切り替えは無い)、親が次に作り直されるときに解消する。
  互換の読み取りは K2 以外 (本のタイル自身の要求など、worker が生成できる経路) には置かない。v1 の行は存続キーに含めず、既存の掃除で消える。
- **P2 列を足す前に開いた読み取り専用の handle**: 列の有無は handle を開いた時点で固定され、RO と RW は別に保持される (`catalog.rs` の列検出、
  `catalog_provider.rs` の RO の再利用)。→ (1) **`|autocrop:v2:` のキーなのに `auto_crops` が NULL の行は「壊れた行」として読み損ね扱い** (自動ではない、
  と読み違えない)。(2) RW の移行で、プロセス内の catalog の schema 世代を上げ、RO handle は読むときに世代を比べて列の有無を調べ直す。
  テスト: 旧 RO を保持 → 別の handle で移行 → v2 を保存 → 同じ RO から読むと `AutoCropResults` が読める。
- **P2 一覧の縦横比のサンプル**: helper を `Loaded` に依存させず、**素材の寸法・判定結果・角度だけを取る純粋関数** `auto_crop_view(source_dims, layout_dims,
  auto_crops, angle)` にする (描画の helper もこれを使う)。縦横比のサンプルは、今も回転を見ていない (保存値で、回転を変えても更新しない) ので、
  それに合わせて **0° の判定結果の寸法** を使う。catalog から直接作るサンプル (`app.rs` の seed) も同じ関数に行の値を渡す。回転の変更・退避一覧の復元で
  サンプルを作り直す処理は足さない (今と同じ)。
- **P3 画素の解像度**: 値を決める。
  - 判定用のデコード: JPEG は元の高さから決めた DCT 縮小率で、縮小後の高さが 1024 以上になる最小の倍率 (元が 1024 未満なら等倍)。PDF は高さ 1024px で描画。
    その他は全体デコード。これを `analysis_raster` (高さ 256) に通す。表示サイズの設定には依存しない。
  - 保存・送信する画素: 切り出す向きが 1 つでもある素材は、どの向きの範囲でも範囲の長辺が送信では `display_px`、保存では `thumb_px` に届く倍率で
    全体を縮小する。ただし全体の長辺は 4096px を上限とし、上限に当たった場合はその解像度で表示する (ログに残す)。切り出す向きが無い素材は今と同じ大きさ。
  - テスト: `display_px` と `thumb_px` が違う条件で、送信後・catalog から読み直した後・K2 でコピーした後のそれぞれで、表示される範囲の有効画素数が
    上の値に届くこと、上限が効くこと。

### 21.10 設計レビュー r2 の反映 (2026-11-14)

r2 = REVISE (P2 1 件、ほかは r1 の指摘を閉じた)。v2 キーで `auto_crops = NULL` を拒否すると、正常な「自動ではない素材」(K2 の v1 互換で作った親の行、
通常の画像を代表にする自動フォルダの行: 設定の印は持つが worker は判定しない) まで毎回読み損ねになる。

→ **素材の種類を 1 つの enum として永続化する**: `AutoMaterial { NotAutomatic, Automatic(AutoCropResults), LegacyV1 }`。
`auto_crops` 列には 3 つの形を固定の綴りで保存し (変換は 1 か所)、**NULL は「未設定」だけ**を意味する。読むとき、`|autocrop:v2:` のキーで NULL の行
(移行前に書かれた行・列を検出できなかった handle が書いた行) は壊れた行として読み損ね扱いにする。`NotAutomatic` と `LegacyV1` は正常な行として読む
(どちらも画素をそのまま表示し、向きによる切り替えは無い)。v2 キーの行を保存するすべての経路 (worker の通常保存、K2 の `save_auto_folder_bytes`、
自動フォルダの保存) は、3 つのどれかを必ず書く (省略可能な引数や既定値で NULL にしない)。
テスト: `Automatic`・`NotAutomatic` (通常画像の自動フォルダ)・`LegacyV1` (K2 の v1 互換で作った親) のそれぞれで、保存 → catalog を開き直す → K2 で再流用 →
送信 まで素材の種類と画素が保たれる。NULL の v2 行は読み損ねになる。

### 21.11 実装記録 (2026-11-14)

- `cover_auto_material.rs` に `AutoMaterial`・4方向の結果・単一codec・純粋な `auto_crop_view` を置いた。
  catalogの追加列、既存ROのschema世代、workerの送信、UIのLoaded、子の行のコピー、退避一覧の復元を同じ素材でつないだ。
- 自動の基本キーをv2に変更し、K2だけはv2の子の行が存在しない場合に同じ子のv1行を `LegacyV1` として読む。
  v1は存続キーへ足さず、壊れた／古いv2からv1へ戻らない。自動OFF・手動のキーの綴りは変えない。
- `thumb_loader/cover_auto_analysis.rs` の固定デコードをworker・範囲画面・評価器が共用する。判定前に各方向へ回し、結果を回転前座標へ戻す。
  判定そのものは変えていない。近傍ページの比率は0°のままとした。
- 全体画素の解像度を表示用・保存用に別々に決め、全方向の範囲の有効長辺と4096の上限を確認する。
  既存の縦横比を正確にする整数寸法の選択は要求長辺より小さくなる場合があるため、実際のfit寸法で必要量を補正する。
  この補正は自動素材だけに適用し、通常／手動の縮小処理は変えない。
- グリッドのUV・寸法・当たり判定・再描画、詳細ホバー、コレクション並べ替え、品質判定が現在の角度の範囲を使う。
  一覧の比率サンプルはLoaded／catalogのどちらも0°の結果を使う。回転の変更に追い出し・再判定・新しい同期I/Oを加えていない。
- 範囲画面は準備の遅着後もメモリ上の現在の回転から自動範囲を選び、手動範囲を優先する。
  補足文は実際に置かれている環境設定のサムネイル頁で「閲覧表示の横連結の方向」に合わせた。
- 回帰テスト: `saved_rotation_detection_matches_upright_jacket_in_source_coordinates`、
  `portrait_gate_uses_saved_rotation_and_the_unrotated_results_stay_declined`、
  `saved_quarter_turn_reduces_only_after_restoring_the_upright_analysis_height`、`cover_auto_analysis_*`、
  `auto_material_*`、`auto_cover_crop_pdf_fixed_analysis_keeps_password_and_read_lease`、
  `auto_cover_late_pixels_keep_four_way_material_and_use_live_angle`、
  `auto_cover_paint_and_hit_geometry_follow_current_angle_without_reupload`、
  `cover_crop_late_prepare_selects_current_angle_and_saves_source_coordinates`。
  既存の `cover_rotation_snapshot_return_reconciles_retained_legacy_and_preserves_current_material` も4方向素材の復元を検査する。
- 焦点nextest: `cover_auto / cover_crop / representative_cache_key / folder_thumb / catalog / cover_rotation /
  color_filter / grid_paint / snapshot_ops` と素材のテスト、計592件成功。評価器の対照条件追加後に `cover_auto` 47件も成功。
- 共通の判定用decodeによるdirectory evaluation（画像の回転0/90/180/270°に対し、保存回転0/270/180/90°）:

  | 入力 | 画像の回転 | 陽性good | wrong | 負例の切り出し | 負例の見送り |
  |---|---|---:|---:|---:|---:|
  | dummies | 0/90/180/270°すべて | 36 | 0 | 0 | 22 |
  | realistic | 0/90/270° | 31 | 0 | 0 | 30 |
  | realistic | 180° | 32 | 0 | 0 | 30 |
  | realistic（0°、再符号化だけの対照） | 0° | 32 | 0 | 0 | 30 |

  wrong・負例・陽性goodの下限は全方向で基準を満たしたが、**realisticの4方向の件数完全一致は未達**。
  回転したZIPのJPEG表紙は評価器で品質95へ再符号化する。`MIV_COVER_AUTO_EVAL_REENCODE_INPUT=1` の対照を加え、
  回転なしでも同じ再符号化経路で31→32になることを確認した。元の0°との寸法・DCT倍率の差はなく、成否の変化は1冊。
  再符号化対照と180°は冊別の成否が2冊で異なり、差を再圧縮だけで説明できたとは断定しない。
  同一画素を回す純粋な回帰テストは4方向で一致している。件数をそろえるための判定変更は行わず、この評価差は設計担当の確認事項として残す。
  出力は `target/cover-auto-eval-{dummies,realistic}-773/rot{0,90,180,270}`、対照はrealistic側の `reencode0-control`。
  評価入力・sheetの内容は閲覧せず、§22の利用者データは使っていない。
- UI snapshotは257件成功。補足文の変更だけを含む `tests/snapshots/preferences_favorite_view_state_dark.png` を更新し、
  差分を目視確認した。既存の小画面・最小幅・画像高さ・再オープンのテストも成功。
  `cargo fmt`、`git diff --check`、`python scripts/check_ui_glyphs.py` も成功（危険glyphなし）。

#### 実装レビュー r1 の対応 (2026-11-14)

- 自動キーの識別を `representative_auto_crop_key_parts` に集約した。代表キーのprefixと、最初の `#pin:` 直前（固定なしなら末尾）の
  完全一致トークンを、正規化・catalogのNULL判定・K2のv1互換で共用する。通常ページの名前に同じ区切り文字が含まれていても旧NULL行を読める。
- `AutoCropView::fit_texture` で、元寸法／page-boxによる縦横比と、テクスチャ画素数×UV範囲による実画素数を分けた。
  詳細ホバーと並べ替えのタイルは実画素の1倍、並べ替えのホバーは2.5倍を上限とする（従来の上限）。全方向辞退の素材にも適用する。
- 列情報は初回に検出した世代と一緒に保持し、全列を持つhandleは他catalogの移行後にも再検出しない。
  UIの `get_or_open_catalog` と検索／集約一覧の準備結果は移行済みRWを使う。旧ROだけはworkerの読取りで世代の変化時に列を再検出する。
  列検出SQLをauthorizerで禁止したテストで、初回・他catalog移行後の全行／単一行／prefix読取りを確認し、旧ROの移行後再検出テストも残した。
- 原寸の比率の門を共通関数にして、回転と `analysis_raster` の生成前に辞退する。門の0.85境界・寸法0・ページ比率の欠損／NaNを含め、
  旧順序をoracleとして4方向の辞退理由が同じことを検査する。判定の閾値とそれ以降の規則は変えていない。
- 追加した回帰テスト:
  `auto_material_literal_zip_page_null_row_is_a_hit_with_auto_off`、
  `auto_material_complete_catalog_reads_do_not_redetect_columns`、
  `auto_crop_texture_fit_preserves_source_aspect_with_rounded_raster`、
  `auto_cover_details_hover_limits_actual_pixels_for_cropped_and_declined_material`、
  `auto_cover_reorder_tile_and_hover_limit_actual_cropped_or_declined_pixels`、
  `raw_aspect_gate_before_pixel_work_preserves_decline_reason_for_each_rotation`。
  矩形の上限比較はf32の1 ULP相当の丸め（30.0000019pt）を考慮し、0.0001ptだけ許容する。
- r1の最終検証: 共有runner（`773 D r1`）経由で焦点nextest599件成功、UI snapshot257件成功。
  `cargo fmt`・`git diff --check`も成功。snapshotの更新はなく、既存552枚のPNGは不変。
  §22.1〜22.3を含む既存本文は保持した。製品起動・build-dev・lib全件・コミットは行っていない。

### 22.4 実装記録 (2026-11-14)

- §22.2のV3'をRustへ移した。平滑化しない解析画素による余白の上限を各辺5%とし、v5の候補決定と拒否条件は保持した。
  v5が指定の4理由で辞退した場合だけ、同じ範囲・列特徴・厳しいパネル境界から2枚つながりの候補を探す。
  RGB全成分の標準偏差、符号付きのインク差、最終境界の帯幅0.2w〜0.8wを検査し、弱い表表紙・別位置が濃い候補の拒否条件をv5と共用した。
  整数の探索端・帯幅は試作と同じ切り捨て、最大値が同じ場合は左側を先に選ぶ。最終の帯幅は設計どおり丸め前のwに対して検査する。
- 見送りは元のv5理由と2枚つながり側の理由を両方持ち、既存のreport／debug logへ
  `no-local-evidence/two-weakfront`などの形で出す。入口の門とv5の`RicherElsewhere`は新しい規則へ進めない。
  デコード・永続化・描画・UIの経路は変更していない。
- 自動キーは**v2を維持する**。§21のv2と今回の判定はともに未出荷で、保存する素材の形式（全体画素と4方向の結果）は同じ。
  出荷済みv1との分離は既に§21で済んでいるため、今回さらにキーの版を上げない。
  開発中に旧判定で生成したv2行を確認する場合は、既存のサムネイル再生成で作り直す。自動削除や追加の移行処理は設けない。
- 追加した回帰テスト（すべてRustで生成した画素）:
  - `two_panel_paper_front_both_bindings_and_four_saved_rotations`: 紙の帯＋絵、細い背の除外、両綴じ方向と保存回転4方向の範囲。
  - `two_panel_art_on_outer_band_and_near_square_illustrations_decline`: 外側にも絵がある負例と正方形に近い負例。
  - `two_panel_final_outer_width_rejects_just_over_point_eight`: 形の門を通っても最終の帯幅が0.8wを超える境界例。
  - `two_panel_signed_ink_gap_rejects_a_darker_outer_band`: 外側のほうが濃いインクの逆転。
  - `two_panel_candidate_still_obeys_weak_front_and_richer_elsewhere_vetoes`: v5の位置の門を通らず、新しい候補が共通拒否条件に当たる負例。
  - `two_panel_fallback_whitelist_preserves_entry_and_v5_veto_declines`: 新規規則へ進む4理由と進まない5理由、reportの綴り。
  - `white_front_content_margin_stops_at_five_percent_on_each_edge`: 白い地を各辺5%より多く外さない。
  - `two_panel_band_std_uses_all_rgb_values_not_luminance_variance`: 帯の標準偏差は輝度の空間分散ではなくRGB全成分から計算する。
- 既存の細縞・網点・ノイズ・上下の模様帯のmust-decline、縮小率の境界安定性、保存回転のテストを含む
  `cover_auto`の焦点nextestは57件成功。既存負例は最終的な辞退を必須としたまま、複合理由から元のv5理由を取り出して従来の期待値も検査する。
- 公開文書`grid.html`の説明との食い違いはないため、公開文書・UI文言・snapshotは変更していない。
  利用者の実データは使わず、評価入力・出力画像・sheetを画像として開いていない。
- 共通の判定用デコード（`MIV_COVER_AUTO_EVAL_DECODE=worker`）によるdirectory evaluation。
  画像の回転0/90/180/270°に対し、保存回転を0/270/180/90°として同じ正立方向を評価した。

  | 入力 | 画像の回転 | 陽性good | wrong | 負例の切り出し | 負例の見送り |
  |---|---|---:|---:|---:|---:|
  | dummies | 0/90/180/270°すべて | 51 | 0 | 0 | 22 |
  | realistic | 0° | 41 | 0 | 0 | 30 |
  | realistic | 90° | 40 | 0 | 0 | 30 |
  | realistic | 180° | 43 | 0 | 0 | 30 |
  | realistic | 270° | 42 | 0 | 0 | 30 |

  全方向でwrong 0・負例の切り出し0を保ち、陽性goodは§21.11の各方向の件数以上。
  再符号化による件数の揺れは§22.3の条件に従い、数をそろえるための閾値変更はしていない。
  出力先は`target/cover-auto-eval-{dummies,realistic}-773-22/rot{0,90,180,270}`。
- 最終検証は共有runner（`773 s22`）経由。上記57件に加えて、`auto_cover_crop|representative_cache_key`の焦点nextest8件も成功。
  `cargo fmt`・`git diff --check`も成功。製品起動・build-dev・lib全件・コミット・追加の独立レビューは行っていない。
- 実データの評価 (設計担当、2026-11-14、Git 管理外・件数だけ): 1 枚目の画像 (長辺 2048) を Rust の評価器 (判定用デコード経路) に通した。
  回転が必要な正例は保存回転 90° を渡し、回転後の向きの正解で判定した。

  | 対象 | 件数 | good | wrong | 切り出し | 見送り |
  |---|---:|---:|---:|---:|---:|
  | 正例・回転なし | 9 | 7 | 0 | — | 2 |
  | 正例・保存回転 90° | 8 | 2 | 0 | — | 6 |
  | 負例 (1 枚目が縦長) | 153 | — | — | 0 | 153 |
  | 正解なし・横長 | 6 | — | — | 0 | 6 |
  | 横長・目視で正解を追加 (後述) | 1 | 1 | 0 | — | 0 |

  当初報告の切り出されなかった 3 冊のうち 1 冊が切れるようになった。残り 2 冊と回転の正例 6 冊は、袖・表表紙・背・裏表紙が強い境目なしで
  つながる形 (§22.1 の見送りのまま)。
  正解なしで切り出した 1 冊 (試作と同じ本) は、利用者の許可を得て設計担当が表紙を目視し、正解を付けた (2026-11-14)。
  左に裏表紙、折り目の右に表表紙が並ぶ形で、表表紙の側を選んでいる (IoU 0.945、good)。ただし左端は折り目より表表紙の内側へ
  表表紙の幅の約 5.5% 入り、著者名の 1 文字目の左端を少し欠く。折り目は 1 列の細い線で、帯平均の境目の評価では文字の端のほうが
  強く出るため。現行の IoU 基準では good の範囲なので規則は変えず、気になる場合の改善候補として記録する。

## 23. 自動の切り出し専用の綴じ方向の設定 (§1.773、2026-11-14、利用者決定 B)

### 23.1 経緯

- 実機確認 (利用者の観測、2026-11-14): 確認用ビルドで、回転した本も回転のいらない本も切り出されなかった。
- 原因 (利用者のログ・設定 DB の読み取り専用のコピーとコードから): 自動の判定は綴じ方向を `default_reading_direction` から取る。
  この設定は画面上は閲覧表示の「横連結の方向」で、横につなげて読むときの並び。横連結を使わない利用者では初期値の「左→右」のままで、
  右綴じであることは見開きの既定 (`default_spread_mode` の Rtl 系) にだけ入っている。そのため右綴じの本が左綴じとして判定され、
  キャッシュのキーはすべて `|autocrop:v2:ltr` だった。利用者のログでは、正例の多くが 180° (左右が入れ替わる向き) のときにだけ「切る」になっていた。
- 評価器で利用者の正例 17 冊を左綴じとして流し直すと good 0・見送り 16・誤り 1 で、実機の症状と一致した (右綴じなら good 9・誤り 0)。
  誤りの 1 冊は裏表紙の側を切っていた。
- この取り方は **v5.2.0 で出した v1 から**ある (§12.4 の「既定の綴じ方向の設定値」がこの値だった)。今回の変更による退行ではない。
- 利用者の判断: **B「自動の切り出し専用に綴じ方向の設定を足す」**。修正は当初「次の版で行い、v5.2.1 は出さない」としたが、2026-11-14 の利用者決定で取り消し、§21〜§26 と §1.777 をまとめて **v5.2.1** として先に出す (ブランチは v5.2.0 由来のまま、master は取り込まない)。
  (退けた案: A は見開きの既定から取る案、C は本ごとの綴じ方向を読む案 (§3 で見送り済み、一覧の時点で引く仕組みが要る))

### 23.2 設定

- `Settings::thumb_cover_auto_crop_binding: ReadingDirection` を足す。既定は **右綴じ (`Rtl`)**。
- 根拠:
  - この機能が対象にするのは、表表紙・背・裏表紙がつながった書籍のカバーをスキャンした本 (書庫・PDF) で、手元の正例も実データもすべて右綴じの日本の漫画。
  - 既存の `ReadingDirection` の既定は `Ltr` だが、それは「横連結の並び」の既定であり、綴じ方向の意味では使わない。
  - 左綴じの本 (横書きの本・海外のコミック) を主に読む利用者は、設定を 1 回変えれば済む。誤った綴じ方向では、見送りが増えるか、
    色の拒否を通った場合に裏表紙を切る (§12.6・§23.1 の数字) ので、多数派に合わせる。
- **既存の利用者の初期値も右綴じ**。設定の行が無い場合は既定値になる (`#[serde(default = ...)]` と `settings_kv` の欠落時の既定)。
  横連結の方向からは引き継がない。
  - 引き継がない理由: 横連結の方向は綴じ方向として選ばれた値ではなく、今回の利用者のように初期値の「左→右」のまま残っていることが多い。
    引き継ぐと、v5.2.0 で自動をオンにしている右綴じの利用者の取り違えが直らない。
  - 影響: v5.2.0 で自動をオンにし、横連結を意図して「左→右」にしていた左綴じの利用者は、次の版で右綴じとして判定される。
    次の版の更新履歴で変更として告知し、新しい設定で直せることを書く。v5.2.0 の公開は 2026-11-13 で自動は既定オフなので、該当は少ない見込み。
  - 移行の要否 (CLAUDE.md「永続データ・スキーマ変更時の判断」): `thumb_cover_auto_crop` は出荷済みだが、今回は新しいキーを足すだけで、
    既存のキーの形式・意味は変えない。欠落時の既定値で読めるので、移行処理は要らない。v5.2.0 に戻した場合は新しいキーを無視して従来どおり動く。
- 環境設定: サムネイルの「つながった表紙から表表紙を自動で切り出す (書庫・PDF の本)」のすぐ下に、「綴じ方向: 右綴じ / 左綴じ」の選択を置く。
  自動がオフのときは無効 (選べない) にする。補足文は「綴じ方向で表表紙の側を判定します。…」の趣旨に直し、横連結の方向に触れない。
  設定の検索 (search_index) から同じ位置へ飛べるようにする。
- 設定の書き出し・取り込み (`settings_transfer`) の項目に足す。
- 範囲画面の補足文・マニュアル (`grid.html`) の「横連結の方向」を新しい設定名に直す。UI の snapshot を更新する。

### 23.3 キーと古い行

- キーの形は変えない (`|autocrop:v2:{rtl|ltr}`)。綴じ方向を取る元を、`default_reading_direction` から新しい設定へ替えるだけ。
  ある綴じ方向での判定の中身は変わらないので、キーの版は上げない。
- 綴じ方向を取る箇所は 1 つの helper にまとめ、次の全部から使う: 一覧の自動の識別子 (`configured_thumbnail_auto_crop`)、
  レーティング表示の経路 (`rating_view`)、範囲画面の判定 (`BookCoverEnv`)。`default_reading_direction` を自動の判定に使う箇所を残さない。
- 古い行:
  - v2 の行 (未出荷・開発中のみ): 新しい設定と同じ綴じ方向の行はそのまま使い、違う綴じ方向の行は使わない。既存の存続集合の処理で、
    今の綴じ方向以外の自動のキーは残さない (§12.4)。
  - v1 の行 (v5.2.0 で保存): 子の本のタイル自身は、今の綴じ方向の v2 のキーで作り直す (`v1:ltr` の画素は自分のタイルには使わない)。
  - **K2 (親フォルダが子の本の行を流用する) の移行 (独立レビュー r1 P1、2026-11-14)**: 子の v2 の行が無いとき、今の綴じ方向と同じ
    `v1:{binding}` を先に探し、無ければ**もう一方の綴じ方向の `v1` の行**も LegacyV1 として流用する。横連結が「左→右」のまま自動を
    オンにしていた利用者は、更新直後に新しい既定 (右綴じ) で要求され、同じ綴じ方向の v1 が無いため、子の一覧を開くまで子の書庫しか
    入っていない親フォルダの代表が消える (§21.9 で防いだ表示消失の再発)。v1 の画素は v5.2.0 で表示していたものなので、子の行が
    作り直されるまでの間それを出すのは更新前と同じ見た目で、退行にならない。v2 の行は綴じ方向をまたいで流用しない (未出荷)。
    回帰テスト: 旧設定 (横連結 Ltr・自動 ON・新しい設定の行なし) と `v1:ltr` の子の行だけがある状態から、親の代表が消えないこと。
- 再読み込み: 自動がオンの間にこの設定を変えたら、オン・オフの切り替えと同じ経路 (`cover_auto_crop_changed`) で一覧の本のタイルを読み直す。
  横連結の方向の変更は、自動の切り出しの再読み込みの条件から外す。

### 23.4 テスト

- 設定: 既定値が右綴じ、行が無い DB から右綴じで読める、保存と読み直しの往復、設定の書き出し・取り込み。
- 環境設定: 自動がオフのときは選択が無効、OK で保存・キャンセルで戻る、変更で再読み込みの条件が立つ。横連結の方向の変更では立たない。
- キー: 新しい設定から `rtl`・`ltr` の識別子が作られ、`default_reading_direction` を変えてもキーが変わらない (一覧・レーティング・範囲画面の各経路)。
- 評価 (設計担当、Git 管理外・件数だけ): 利用者の正例 17 冊を右綴じで流して good 9・誤り 0 (§22.4)。§23.1 の誤りの 1 冊は、
  右綴じでは正しい側を切る (IoU 0.99、good)。判定そのものは変えないので、実装後の再評価は要らない。

### 23.5 実装記録 (2026-11-14)

- `Settings::thumb_cover_auto_crop_binding`を追加し、`Settings::default`とserdeの欠落時の値をともに`Rtl`にした。
  保存は既存の`settings_kv`経路、設定転送は既存の項目定義に追加した。横連結の値の引き継ぎ・DBの移行は設けない。
- `configured_thumbnail_auto_crop`を一覧・レーティング・範囲画面で共用する。`BookCoverEnv`の自動専用だったON/OFFと読み方向の2 fieldは、
  同helperが返す`Option<AutoCropIdentity>`を捕捉する形に置き換え、workerはそのbindingで判定する。
  判定の内容とキーの綴りは変えず、v2を維持する。OFF・手動・ドライブ・Remoteの区別も既存のまま。
- レーティングを実際のApp入口まで確認したところ、`thumbnail_source_discovery_snapshot`が自動のON/OFFを捕捉しておらず、
  workerの存続キーがOFFの既定値になることを確認した。指定のレーティング経路を成立させるため、この既存の軽量コピーにもON/OFFと新しいbindingを渡す。
  回帰テストはliveなSettingsを直接workerへ渡さず、このコピーを通して両方向の存続キーを検査する。
- 環境設定の同じanchorに「綴じ方向:」のradio（右綴じ／左綴じ）を追加し、OFFでは無効にした。
  再読み込みの条件は共通helperの結果の変化で比較し、OFF中のbinding変更と横連結の変更は自動の読み直しを起こさない。
  検索keywordは既に同anchorに「綴じ方向」があったため、単独語でも当たる回帰を追加した。
- `default_reading_direction`と「横連結」の参照を確認した。自動用の読み取りは残さず、閲覧・見開き・Remoteの通常用途は維持した。
  範囲画面の補足文には既に横連結への言及が無かった。公開文書は`grid.html`に加え、同じ説明が残っていた`settings.html`と`migrate-zippla.html`を更新した。
- 変更パス（snapshotと検証結果は後記）:
  `src/settings.rs`、`src/settings_transfer.rs`、`src/app.rs`、`src/app/book_cover.rs`、`src/app/cover_auto_tests.rs`、`src/app/color_filter.rs`、
  `src/rating_view.rs`、`src/ui_dialogs/cover_crop.rs`、`src/ui_dialogs/preferences.rs`、`src/ui_dialogs/preferences/pages.rs`、
  `src/ui_dialogs/preferences/search_index.rs`、`htdocs/mimageviewer/manual/grid.html`、`htdocs/mimageviewer/manual/settings.html`、
  `htdocs/mimageviewer/migrate-zippla.html`、本書、`docs/next-release-backlog.md`。
- 追加したテスト:
  - `thumb_cover_auto_crop_binding_default_missing_and_db_roundtrip`
  - `thumb_cover_auto_crop_binding_transfer_roundtrip_missing_invalid`
  - `thumb_cover_auto_crop_binding_reload_ignores_reading_direction_and_disabled_binding`
  - `thumb_cover_auto_crop_binding_preferences_disabled_ok_cancel_save_and_reopen`
  - `cover_auto_binding_is_shared_by_list_and_dialog_independent_of_reading_direction`
  - `cover_auto_rating_prepare_uses_binding_snapshot_and_ignores_reading_direction`
  既存の`cover_auto_color_identity_follows_binding_and_manual_range_without_source_io`、ZipDirの存続集合・collectionの再利用key、
  `thumb_cover_auto_crop_search_finds_its_anchor`も、新しい設定の変更と横連結からの独立性を検査する。
- 焦点nextestは共有runner（`773 s23`）経由で189件成功。
  filterは`cover_auto|cover_crop|settings_transfer|representative_cache_key|thumb_.*preferences|container_icon_kinds_preferences`。
  設定転送の項目数の固定値は、新しい許可項目1つ分だけ増やした（分類496・許可160・wire 158、除外336は不変）。
- snapshotは`UPDATE_SNAPSHOTS=1`で`preferences_favorite_view_state_dark`だけを更新した。
  変更PNGは`tests/snapshots/preferences_favorite_view_state_dark.png`の1枚（専用の綴じ方向のradioと補足文）。
  更新指定を外した`cargo test -p mimageviewer --test ui_snapshot`は257件成功。他のPNGの変更はない。
- `cargo fmt`・`git diff --check`・`python scripts/check_ui_glyphs.py`は成功（危険glyphなし）。
  新規ファイルはない。UIスレッドの新しい同期I/O・detached経路の変更・判定ロジックの変更はなく、
  評価フォルダ／利用者データの使用、製品起動、build-dev、lib全件、コミット、追加の独立レビューは行っていない。

#### §23.3 K2移行の実装追記

- `legacy_auto_child_keys`が同方向→逆方向のv1キーを返す。K2の共通候補読み取りだけで使い、v2は方向をまたがない。
  存在するv2が壊れている・署名が古い場合にv1へ戻らない既存の条件も保った。子のタイル自身の要求は変更していない。
- 変更: `src/thumb_loader.rs`、`src/thumb_loader/auto_material_tests.rs`。
  `auto_material_k2_missing_binding_setting_keeps_parent_from_old_ltr_child`で旧設定DBの新キー欠落から親の代表選択までを通した。
  `auto_material_k2_prefers_v2_and_legacy_fallback_requires_missing_exact_v2`は、逆方向v2の拒否・逆方向v1の流用・同方向v1の優先を検査する。
  子自身の`auto_material_main_tile_cache_only_does_not_read_v1_compatibility_row`と設定既定のテストを合わせ、独立した焦点4件が成功。

## 24. 本文ページの比率の読み方: 表紙の直後の横長を数えない (§1.773、2026-11-14、利用者決定)

### 24.1 経緯と数字 (Git 管理外の数値のみ、画像は開いていない)

- 利用者の提案: 表紙の後にカラーの口絵・帯のスキャン・カバー裏などの横長が続く本で、比率が決まらず見送られるのではないか。
- 利用者の 177 冊 (PDF のページの大きさ) を今の規則で数え直すと、比率が決まらないのは正解のない横長の 7 冊のうち 5 冊。
  理由は 5 冊とも「縦長が 7 割未満」で、「縦長の大きさがそろわない」は 0 冊。2 冊は全ページが横長 (見開きだけの本、見送りが正しい)、
  3 冊は表紙の後の 3 枚だけが横長で、その後は縦長が続く。
- 縦長ページの比率のばらつき (174 冊・8,407 ページ、本ごとの中央値からのずれ): 50% 0.16%・99% 1.18%・99.9% 4.31%。
  今の「±6% に 7 割」で落ちる本は 0 冊なので、許容は変えない。表表紙の幅は中央値から決めるので、比率の誤差は概ね 1% 以内。
- 読み方の比較 (読む枚数はどれも 8。誤り・負例の切り出しはどれも 0):

  | 読み方 | 合成本 good | 合成本 + 前付けの横長 3 枚 good | 実データで比率が決まるようになる本 |
  |---|---:|---:|---:|
  | 今 (表紙の次から 8 枚) | 41 | 0 | — |
  | 表紙の次の 3 枚を飛ばして 8 枚 | 39 | 41 | 3 |
  | 表紙の次の 16 枚に 8 枚を散らす | 41 | 33 | 3 |
  | **表紙の次から 8 枚、表紙の直後に続く横長を最大 3 枚まで数えない** | **41** | **39** | **3** |

  前付けの合成は、合成本の各冊の表紙の直後に横長 (比 1.39) を 3 枚、本文の後に本文の中央値の縦長を 8 枚足したページの並び。
  同じ表紙・同じ比率なら判定は同じなので、比率の決まり方から結果を写して数えた。縦長の負例 153 冊は比率より前の門で見送るので変わらない。
  合成ダミーは比率を正解から与えるので変わらない。

### 24.2 規則 (採用: 最後の案)

- 読むページは今と同じ (`sample_indices`: 表紙の次から最大 8 枚、表紙が最後のページなら前へ 8 枚。表紙に近い順)。
- 読んだ枠 (表紙に近い順、最大 8 枠) を**位置を保ったまま**扱う。寸法が取れなかった枠 (256 KiB で読めない、解析できない) も 1 枠として残す。
- 先頭の枠から順に、横長 (比 0.9 以上) が続く間を**最大 3 枠まで**取り除く。寸法が取れない枠に当たったら、そこで取り除くのを止める
  (それより後ろの横長は取り除かない)。その後、寸法が取れない枠を落とし、残りを今の `aspect_consensus`
  (3 枚以上・縦長 7 割以上・縦長の 7 割が中央値の ±6%) に渡す。4 枚目以降の横長や、途中に混ざる横長 (見開き) は今までどおり数える。
  (独立レビュー r1 P2: 寸法不明の枠を先に落とすと、`[不明, 横×4, 縦×3]` で実際の 4 枚目の横長まで取り除かれる。この 8 枠を回帰例にする)
- 守り: 全ページが見開きの本は、取り除いた後も横長だけが残るので見送る (今と同じ)。ZIP で読むファイルの数・読む量は変えない
  (最大 8 回・各 256 KiB)。
- 比率が決まらないときの理由を型で返す (`TooFew` / `Landscape` / `Inconsistent`)。ログと §25 の範囲画面の表示に使う。
- 判定の中身は変わらないが、比率が決まる本が増えるので自動の結果が変わり得る。§23 と同じく自動のキーは未出荷の v2 のまま据え置く
  (開発中に作った v2 の行は既存のサムネイル再生成で作り直す)。
- テスト: 表紙の直後に横長 1〜3 枚で決まる、4 枚で取り除くのは 3 枚まで (残りで判定)、途中の横長は取り除かない、全ページ横長は見送る、
  表紙が最後のページで前へ読む場合、理由の型。ZIP の読み取り回数が 8 のままであること。
- 評価: 新しく比率が決まる実データの 3 冊を評価器で流す。正解が無いので、切れた場合の正否は統括経由で利用者に確認する (画像は開かない)。

### 24.3 実装記録

- `cover_auto_sample.rs`で枠を`Option`のまま保持し、先頭から続く0.9以上を最大3枠だけ外してから不明枠を落とす。
  PDFもZIPも同じ`aspect_from_slots`→`aspect_consensus`を使い、逆順のサンプルも表紙に近い側を先頭とする。
- `PageAspectDecline::{TooFew,Landscape,Inconsistent}`を追加した。既存の値だけのAPIはreport APIの`.ok()`で保ち、評価器の呼び方は変えていない。
  workerの`auto cover:`ログに`page-aspect=...`を付け、範囲画面は型を表示へ渡す。
- `zip_loader.rs`は読み取り失敗も不明な1枠として返す。最大8回・各256KiBの既存の制限を維持し、追加読み取りや全体decodeはしない。
- 変更: `src/cover_auto_sample.rs`、`src/zip_loader.rs`、`src/thumb_loader.rs`（ログ）。範囲画面の受取りは§25と共通。
  `leading_landscape_slots_pdf_zip_forward_backward_and_typed_reasons`は先頭1〜3枚・4枚・途中の横長・全ページ横長・不明枠・3種類の辞退を
  PDF/ZIPと前後両方向で検査する。`[不明, 横×4, 縦×3]`も含む。既存の8回のprefix量制限・EXIF・ZIP内scopeのテストと合わせ、独立した焦点5件が成功。
- v2を維持する。今回の実装では評価フォルダや利用者データを使っていない。

## 25. 範囲画面に自動の判定結果を出す (§1.773、2026-11-14、利用者決定)

- 利用者の提案: 切り出されないときに理由が分からない。範囲画面で自動の判定結果を確かめ、その判定に戻せるとよい。
- コードの確認: 範囲画面は、開くたびに画面専用のスレッド (`cover-crop-prepare`) で、一覧と同じ解析デコード
  (`decode_cover_auto_analysis_image` / `_pdf`) と同じ比率の取り方で判定している。入力と解像度は一覧と同じで、表示サイズに依存しない。
  違いは、理由を返さない `detect_cover_crops` を使っていることだけ。判定を足す必要は無い。
- 変更:
  - 範囲画面の判定を `detect_cover_crops_with_report` に替え、今の保存回転の角度の結果 (切った範囲、または見送りの理由) を持つ。
    判定の前で止まる場合も区別する: 設定がオフ、手動の範囲を使用中、編集のプレビューがある、対象外 (フォルダの本・直接読む RAR・
    ネスト ZIP など)、比率が決まらない (§24 の理由の型)、解析に失敗。
  - 画面に 1 行足す: 「自動の判定: 表表紙を切り出しました (綴じ方向: 右綴じ)」または「自動の判定: 切り出していません。<理由>」。
  - 理由の言葉 (利用者向けにまとめる。グリフの決まりに従い記号を足さない):
    - 表紙が横長でない (`RawAspectGate`・`NotWide`): 「表紙がつながった形 (横長) ではありません」
    - 比率が決まらない (`PageAspectMissing`、§24 の型で分ける): 「本文ページの形が決まりません (横長のページが多い / 大きさがそろわない / ページが少ない)」
    - 境目が見つからない (`NoJacketEvidence`・`NoStructure`・`NoLocalEvidence`・`NoSpine`、および 2 枚つながりの見送りのうち
      形・範囲・境目・外側の帯の理由 (`TwoPanel.reason` が `WeakFront`・`RicherElsewhere` 以外)): 「表表紙と背・袖の境目が見つかりません」
    - 色の拒否 (`WeakFront`・`RicherElsewhere`、および `TwoPanel.reason` が `WeakFront`・`RicherElsewhere` のもの): 「表表紙の側が淡く、裏表紙と取り違えるおそれがあるため切りません」
      (独立レビュー r1 P2: 2 枚つながりは境目を見つけた後に色で拒否することがあるので、`TwoPanel` は内側の理由で振り分ける。
      `AutoCropDecline` の全ての値と `TwoPanel` の全ての内側の理由について、表示の対応をテストする)
    - 判定の前で止まる場合: 「自動の切り出しはオフです」「手動の範囲を使っています」「編集のプレビューがあるため自動は使いません」
      「この形式の本は自動の対象外です」「表紙を解析できませんでした」
  - 「自動の判定の枠にする」ボタン: 自動が今の角度で切った場合だけ有効。編集中の枠をその範囲に戻す (保存は OK のとき)。
    既存の「手動の範囲を解除 (自動の判定に戻る)」はそのまま。
- 一覧と画面の結果が食い違わないこと: 同じ helper・同じ入力であることをテストで固定する (同じ表紙で、一覧の worker の結果と
  範囲画面の結果が 4 方向とも一致)。
- UI: 1093×614 でも操作が見えること (§19 の配置の決まり)。snapshot を更新する。マニュアル (`grid.html` の「判定できない画像は全体を表示」の節) に、
  範囲画面で判定の結果と理由を確かめられることを書く。
- 規模: Small〜Medium。

### 25.1 実装記録

- `cover_crop/automatic.rs`に`AutoDetection`（判定前に停止／4方向の判定済み）を置き、workerの準備結果と編集中の画面が同じ型を持つ。
  既存の`auto_crops`・`auto_enabled`の組を置き換えた。`detect_cover_crops_with_report`で各方向の範囲または辞退理由を保持し、
  UIは既存のメモリownerから受けた保存回転で選ぶだけ。判定・先読み・解析decodeは既存の`cover-crop-prepare`内に残す。
- ON/OFF・対象形式・手動範囲・編集preview・比率の3理由・解析失敗を区別する。表示は本文の対応表どおりとし、
  `TwoPanel`は内側の`WeakFront`／`RicherElsewhere`を色の拒否へ、それ以外を境目の理由へ分ける。
- 結果の行は画像直下のスクロール本文、戻すボタンは固定の操作行へ置いた。操作行は実測した4ボタン分の必要幅で折り返し、
  下段も実際の高さを本文から引く。1093×614の100%／200%で従来の画像高さを維持し、最小幅の操作も保つ。
  戻す操作は現在角度の範囲がある場合だけ有効で、draftの枠・drag・検証エラーだけを更新する。保存は既存のOK経路だけ。
- 追加テスト:
  - `cover_crop_auto_reason_mapping_covers_all_detector_and_preanalysis_reasons`: 全`AutoCropDecline`、全`TwoPanel`内側理由（4つのv5理由それぞれ）、全判定前理由の文言。
  - `cover_crop_auto_success_uses_current_angle_and_binding`: 現在角度での成否と右／左綴じの表示。
  - `cover_crop_worker_and_dialog_match_all_four_angles_for_png_jpeg_and_binding`: 生成したPNG・JPEG、両綴じ方向、先頭に横長3枚を含む同じZIPから、
    実際の`process_load_request`と画面の`prepare_cover`を通し、4方向すべての範囲が同じこと（少なくとも1方向は切る正例）。
  - `cover_crop_auto_reset_is_visible_in_small_viewport_and_only_ok_saves`: 100%／200%・4方向でボタンの可視性・非重複・クリック・角度別の無効化、
    結果の行へのscroll到達、枠の復帰、クリックだけではDB不変、Cancelで不変、OKで元座標の範囲を保存すること。
  - `cover_crop_auto_reset_keyboard_enter_does_not_accept`: 戻すボタンをEnterで実行しても、画面を閉じずDBへ保存しない。
  - `cover_crop_enter_uses_focused_clear_when_footer_wraps`: 操作行が折り返した場合も、フォーカスした解除ボタンのEnterがOKへ化けない。
- ボタン固有の操作を`Action`で受け、ダイアログ全体のEnter＝OKは他の操作が無かった場合だけにした。
  戻す操作には保存を伴わない専用の`ResetAutomatic`を使う。既存のIME-safe Enter／Escapeの取得は変更していない。
- 変更: `src/ui_dialogs/cover_crop.rs`、新規`src/ui_dialogs/cover_crop/automatic.rs`・`automatic_tests.rs`、
  `src/ui_dialogs/cover_crop_rotation_tests.rs`（fixtureの型変更だけ）、`htdocs/mimageviewer/manual/grid.html`、本書、バックログ。
- 最終コードでK2・比率・画面・設定・素材を合わせた焦点nextest155件成功（`cover_auto|cover_crop|auto_material|thumb_cover_auto_crop`）。
  素材の独立した焦点15件も成功。既存の小画面・最小幅・1822cd0efからの画像高さ・回転／手動範囲の回帰も成功。
- `UPDATE_SNAPSHOTS=1`で`crop_small`4件を更新し、最終コードの通常モードの`ui_snapshot`257件も成功。
  変更PNGは次の4枚だけ（判定結果の行と、自動枠へ戻す固定ボタンを追加）:
  `tests/snapshots/cover_crop_small_100_light.png`、`tests/snapshots/cover_crop_small_200_dark.png`、
  `tests/snapshots/cover_auto_crop_small_100_light.png`、`tests/snapshots/cover_auto_crop_small_200_dark.png`。
- 全cargo検証は共有runner（`773 s24`）経由で逐次実行した。`cargo fmt`・`git diff --check`・`python scripts/check_ui_glyphs.py`も成功。
  UIスレッドへの新しい同期I/O、detached経路の変更、判定器の閾値変更はない。
  利用者データ・評価フォルダ・共有先のデータは使わず、画像・sheetは開いていない。
  lib全件・build-dev・製品起動・コミット・追加の独立レビューは行っていない。

### 25.2 実機確認後の停止理由の具体化 (2026-11-14)

- `ResolvedRead` と `ResolvedBookCover` の自動入力は、入力または `AutoCoverSkip` の `Result` で保持する。
  従来の `None` から画面で推測せず、workerの解決時に手動範囲・フォルダの固定代表・子フォルダ/中の本の代表・
  画像フォルダの設定OFF・直接読むRAR・入れ子ZIP・残りの対象外を区別して渡す。手動範囲は自動設定のOFFより先に表示する。
  手動停止の値には解除後の自動適用資格も持たせ、自動OFFや固定フォルダを「解除すると自動に戻る」と案内しない。
- ZIP/PDF自身の固定ページには、従来どおり自動を掛ける（手動範囲や編集previewがある場合を除く）。
  フォルダの固定代表には掛けない。K2の画面用leaf再構築でも親側の停止理由を保ち、子の判定入力で置き換えない。
  K2素材の保存/伝播・判定の規則・キー・UIスレッドのI/Oは変更しない。
- `auto cover: rotation=...` と `auto cover material: ...` は、同名leafを区別できるよう `book=<要求元タイルのパス>` と
  `leaf=<従来の表示名>` を併記する。RAW継続が要求を消費する前にログ用の本の名前だけを捕捉する。
- 一般の `Unsupported` は、RAW/動画/音声の書庫leaf、通常の本代表では生じない直接ファイル等の解決位置、読めない要求の
  予備値に残す。RAW/動画等は通常、範囲画面の画像準備の段階で既存の専用エラーとなり、この一般文言まで到達しない。
  マニュアルの範囲画面の説明には理由の列挙がなく、従来の説明と矛盾しないため変更しない。
- 回帰: `cover_crop_folder_stops_arrive_from_worker_for_pin_manual_setting_and_descendant`、
  `cover_crop_archive_exclusions_and_fixed_container_pages_use_worker_types`、
  `book_cover_fixed_pdf_keeps_worker_auto_input_for_range_dialog`、
  `book_cover_auto_cached_child_retains_parent_stop_instead_of_child_eligibility`。
  `cover_crop_auto_reason_mapping_covers_all_detector_and_preanalysis_reasons` の対応表も全停止理由へ広げる。
- 検証: 最終差分で `cover_auto|cover_crop|book_cover|cover_rotation|auto_material|exif_reader` の焦点nextest **283件成功・失敗0**。
  `ui_snapshot cover_` **5件成功**（範囲画面4件と既存の環境設定1件）、PNG変更なし。
  `cargo fmt`・`python scripts/check_ui_glyphs.py`・`git diff --check`成功。全cargoは共有runner `773 rc1` 経由で逐次実行した。
  型変更に合わせたAI説明文と回転テストのfixture補正を含むが、AI本番処理・detached・設定保存・判定器は変更していない。
  lib全件・製品起動・検証用build・コミット・追加の独立レビューは行わず、利用者データや外部評価画像も使用していない。

## 26. 画像フォルダの本も自動の対象にする (§1.773、2026-11-14、利用者決定 B・v5.2.1)

### 26.1 経緯と方針

- §3・§13-6 では、フォルダの本は周辺ページの変更を表す stamp が無いことを理由に、自動の対象外とした (r2 指摘 3)。
- **利用者の方針 (2026-11-14)**: サムネイルに使うのは表紙だけなので、判定し直すのは表紙が変わったときだけでよい。中のページの追加・削除・
  差し替えでは判定し直さず、判定に使ったページの比率がその後に変わっても、表紙が変わるまでは前の結果のままにする。
  これで r2 指摘 3 は仕組みを足さずに方針で閉じる (§3・§13-6 の決定をこの方針で置き換える。手動の範囲指定は今のまま)。
- コードの確認: フォルダの行は代表の選び方の証明に表紙の leaf のパス・mtime・size を持ち (`FolderSelectionProof` の winner)、
  表紙の差し替え・削除・前に並ぶ画像の追加・ピンの変更で作り直される。実際はフォルダの mtime (直下の追加・削除) でも作り直されるが、
  方針より作り直す機会が多いだけで害は無い。中のページの同名の上書きでは作り直さない (方針どおり)。
- 範囲: **専用の設定 (既定オフ) の後ろに置く** (利用者決定 B)。mIV にはフォルダを本と見分ける印が無く、直下に画像があるフォルダが全て
  対象になり得るため。写真のフォルダへの守りは §27 (全種類に掛ける)。

### 26.2 設定

- `Settings::thumb_cover_auto_crop_folders: bool`、既定 `false`。設定の行が無ければ `false` (settings_kv の欠落時の既定。移行処理は要らない)。
- 環境設定: §23 の綴じ方向の行のすぐ下に「画像フォルダの本も対象にする」。自動の切り出しがオフのときは無効。補足文に、写真のように
  見える表紙は切らないこと (§27) と、判定し直すのは表紙が変わったときであることを短く書く。設定の検索から同じ位置へ。
- 設定の書き出し・取り込みの項目に足す。
- 自動がオンの間にこの設定を変えたら、§23 と同じ再読み込みの経路 (`cover_auto_crop_changed`) で一覧のタイルを読み直す。

### 26.3 キー

- フォルダのタイルのキー (`folderthumb:`) は、自動がオンなら今も `|autocrop:v2:{binding}` を持つ (K2 で子の自動の結果を含み得るため、§5.2)。
- この設定がオンのとき、フォルダのキーの印を **`|autocrop:v2f:{binding}`** にする (書庫・PDF の本のキーは変えない)。
  オフで作った行 (フォルダの本の表紙は判定せず `NotAutomatic`) をオンで使い回さないため、またその逆のため。
  キーの解釈 (`representative_auto_crop_key_parts` など) は `v2f` を受け付け、v1 の移行 (`legacy_auto_child_key`) の対象には含めない。
  存続キー・再読み込みの経路も同じ helper で印を決める。v2f は未出荷の新しい印なので移行は要らない。
- 設定を切り替えると全てのフォルダのタイルのキーが変わり作り直されるが、自動のオン・オフの切り替えと同じ扱いで、頻繁な操作ではない。

### 26.4 判定の対象と入力

- 判定するのは、フォルダのタイルの代表が**そのフォルダの直下の画像**の場合だけ (worker の解決で `FolderThumbResolution::Image` が
  直下から選ばれたとき)。子フォルダを辿って選んだ画像とピンで固定した代表 (Seeded) は、親が判定しない (`NotAutomatic`)。
  子の本の行の流用 (K2) は、親が判定し直さず、**子の素材の種類と結果をそのまま保持する** (今の保存・送信の経路どおり。
  `NotAutomatic` に置き換えると描画時の切り出しが消え、親だけ全体の画像に戻るため。独立レビュー r2)。
- 本文ページの比率: worker が表紙を決めるときに読んだ直下の一覧 (`read_dir`) を、名前順 (ZIP と同じ `FileNameSortKey`) に並べて返し、
  表紙の位置から §24 の規則で最大 8 枚を選ぶ。各ファイルの先頭 256 KiB を読み、ZIP と同じ `probe_image_dims_from_bytes` (EXIF の向きを含む)
  で寸法を取る。フォルダを読み直さない。費用は最大 8 回のファイルの open と最大 2 MB の読み込みで、worker で行う (UI スレッドに乗らない)。
- 解析用デコードは既存の `decode_cover_auto_analysis_image(Source::Path)`。判定・素材の保存・描画時に切る経路は書庫と同じ (§21)。
- 範囲画面: フォルダの本 (`BookKind::Folder`) でも、設定がオンで表紙が直下の画像なら自動の判定を出す。画面専用のスレッドで同じ helper を使う。
- 対象外のまま: ドライブ一覧、mIV Remote のタイル (§12 と同じ)。

### 26.5 評価 (Git 管理外・件数だけ)

- 画像フォルダの本 (利用者の指定した場所、7,065 冊) の寸法: 表紙が横長 795 冊。本文の比率が決まるのは今の読み方で 709 冊、§24 で 725 冊。
  縦長ページの比率のずれ (6,980 冊・74,597 ページ) は 90% 0.44%・99% 2.9% で、PDF より大きいが、今の許容で落ちる本は 10 冊。許容は変えない。
- 評価器 (cfg(test)) に、フォルダの一覧 (Git 管理外の JSON) を受けて、本番と同じ比率の取り方・判定でフォルダの本を数える入力を足す。
  この入力では**画像の出力 (sheet) を作らない** (利用者の指定した場所は成人向けを含むため)。結果は件数と理由の分布だけ。
  正解が無いので、切った本の正否は実機確認で利用者に頼む。

### 26.6 実装記録 (2026-11-14)

- `thumb_cover_auto_crop_folders` を既定・欠落時ともfalseで追加。保存・設定転送・検索・無効表示・OK/キャンセル・有効中の再読み込みを既存経路へ接続した。
- 印は `AutoCropIdentity::suffix_for_key` で決め、フォルダだけv2fにする。一覧・存続集合・レーティング・範囲画面は同じ設定captureと正規化を使う。v2fをv1移行へ含めない。
- 代表選択結果に直下の一覧と所有ディレクトリを付け、解決後の自動指示と標本の入力を `ResolvedAutoCover` にまとめた。再帰先・Seededは親の判定対象から外し、K2の素材コピーは変更しない。
- 比率は選択時の一覧を `FileNameSortKey` 順で引き継ぎ、最大8枚×256KiBをworkerで読む。フォルダを再走査せず、本文の変更を追跡する仕組みも追加しない。
- `MIV_COVER_AUTO_EVAL_FOLDER_LIST` のJSON配列を受けるcfg(test)評価を追加。専用分岐は画像を出力せず、summary.jsonの行は番号・寸法・理由・数値の範囲だけ。合成フォルダで出力がsummary.jsonだけで名前を含まないことを検査する。利用者データでの評価は実行していない。
- 主な回帰: `cover_auto_folder_direct_selection_reuses_listing_and_excludes_descendants_and_seeds`、`cover_auto_folder_v2f_identity_roundtrips_and_never_uses_v1_migration`、`cover_auto_folder_switch_keeps_list_survival_rating_and_dialog_identity_consistent`、`cover_auto_rating_folder_switch_uses_v2f_from_settings_snapshot`、`folder_evaluation_emits_counts_and_numeric_rows_without_images_or_names`、設定の既定・欠落・DB/転送往復・無効表示・OK/キャンセルのテスト。
- K2のr2修正は `auto_material_k2_prefers_v2_and_legacy_fallback_requires_missing_exact_v2` に、同方向v1だけ古い場合を追加。同方向が現行なら優先し、v2が存在する場合は別方向v2やv1へ戻らない条件を維持した。
- 検証: `cover_auto|cover_crop|auto_material|representative_cache_key|folder_thumb|thumb_cover|settings_transfer|settings::` の焦点nextest **629件成功・失敗0**。設定の分類項目数の期待値も新項目分を更新して再実行した。`cargo fmt`・`git diff --check`・UI glyph検査成功。
- UI snapshotは設定画面1件を更新後、通常比較で **257件成功・失敗0**。更新PNGは `tests/snapshots/preferences_favorite_view_state_dark.png` だけ（設定と補足文の追加）。画像は開いていない。公開説明はgrid/settingsに加え、移行案内に残っていた対象外の1文も合わせた。

## 27. 写真のように見える表紙は自動で切らない (§1.773、2026-11-14、利用者決定 案 1・v5.2.1)

### 27.1 経緯と数字 (Git 管理外・件数だけ)

- 公開の CC0 写真 (1,000 枚) から、横長の写真を表紙にし、同じ比率の縦長の写真を 8 枚続けた ZIP を 390 組作った (誤判定がいちばん
  起きやすい組み方)。今の判定は右綴じ・左綴じとも 24 組 (重複を除いて 34 組) を切った。
- Python の試作で出荷済みの規則 (v5.2.0、余白の上限 20%) にかけると、Rust が切った組の右綴じ 22/24・左綴じ 21/24 を同じく切る。
  §22 の 2 枚つながりの規則だけで切るのは右綴じ 1 組。**写真を切るのは主に v5.2.0 からある規則**で、写真を ZIP にまとめたアルバムや
  写真集の PDF は v5.2.0 でも切られ得た。
- 当初の利用者の判断: 守り G1・G2 を、書庫・PDF・画像フォルダの**全種類に掛ける**。画像フォルダは §26 の専用設定の後ろ。
- **その後の利用者決定 (2026-11-14): G2を外し、G1だけを残す。** スキャンした画像でも寸法をきれいに揃えている可能性がある。
  写真と漫画の両方を持つ人は少ないので、写真の守りで漫画を読む人の邪魔をしないことを優先する。

### 27.2 門

- 比率の門の後に、**G1 撮影情報だけ**を使う。表紙の EXIF の Exif IFD に、露出時間
  (`ExposureTime`、タグ33434 = 0x829A) か F値 (`FNumber`、タグ33437 = 0x829D) があれば切らない。
  見送りの理由は `PhotoLike { ShotExif }`、ログ・評価器では `photo-like/shot-exif`。
- JPEG・TIFFは、従来の向きを読むのと同じrexif解析から撮影情報を取る。JPEGの先頭上限・壊れたmarkerの扱いも維持する。
  解析用デコードが既に持つbytes、または最初のデコーダからEXIFを取れるPNG/WebP等でもG1を使う。
  メタデータだけのためのファイル読み直しは行わない。追加の読み取りが必要なWIC専用経路等ではG1を使わない。
- PDFの描画ページや、撮影情報のない画像にはG1を掛けない。
  PNG/WebPの追加metadataは撮影情報だけに使い、向きはr3で揃えた従来のデコードと同じ規則を保つ。
- **G2（縦横を入れ替えた寸法の一致）は撤去した。** 画像の±1px・PDFの相対0.5%比較、
  そのための本文寸法の返却・単位区分・表紙の精密point値・保存回転ごとの寸法投影も不要になった。
  A判/B判の±1%除外と、寸法が一致する非写真の正例を見送るという以前の受け入れ判断も撤回する。
  寸法が揃ったスキャン画像を写真と判断する規則は残さない。
- 本文比率は§24の規則をそのまま使う。未知の枠の位置・先頭横長の除外・最大8枚×256KiBを維持し、
  r4のフォルダ本文のrexif→WICの向きの規則も比率のために残す。ZIP本文は従来のbytes規則のまま。
- 範囲画面 (§25) のG1の文言は「写真のように見えるため自動では切りません」のまま。
  自動の印は未出荷のv2/v2fを維持し、開発中の行は従来の再生成で作り直す。
- テスト: ExposureTimeだけ・FNumberだけ・両方・無し、JPEG/TIFF/PNG/WebPの既存の撮影情報と向きの解析、
  G1の書庫/フォルダのworker・範囲画面での適用と、後押しの評価器でもG1を越えないことを保つ。
  表紙と本文の寸法の縦横が一致しても、撮影情報がなければ判定を続けることを画像・PDFで検査する。


### 27.3 実装記録 (2026-11-14)

以下のG2を含む記録は撤去前の履歴。現行の規則は§27.2と、末尾のG2撤去記録を正本とする。

- `cover_auto_crop/photo.rs` に `PhotoEvidence` と `PhotoLikeReason` を置き、保存回転ごとの元寸法の門を通った後、画像の回転・解析rasterの作成前にG1/G2を掛ける。worker・範囲画面・評価器は同じreport付きhelperを使う。
- JPEG/TIFFは向きとExif IFDの撮影情報を一度のrexif解析で取得し、JPEGのDCT倍率選択にも解析済みの向きを渡す。PNG/WebPは既に所有するbytes、または最初のdecodeのfile handleとデコーダから取得する。全体画像・向き・撮影情報・バックエンドを同じdecode結果に保持し、メタデータだけのためにファイルを読み直さない。
- WIC専用形式など、既存のdecodeがEXIFのbytesを持たず追加読み取りが必要な経路ではG1を使わずG2だけにする。PDFはG1なし、G2には丸めた描画寸法でなく実際のpage-box point値を渡す。
- 比率標本は取り除く前の全枠の寸法も返し、G2へ再利用する。A判/B判の±1%除外を維持し、見送りは `photo-like/shot-exif` / `photo-like/transposed-page` と範囲画面の対応文言で示す。未出荷の判定版v2は据え置く。
- 主な回帰: `cover_auto_analysis_exif_requires_exif_ifd_and_extracts_orientation_in_same_parse`、JPEG/TIFFのbytes/path一致、PNG/WebPのmetadata、`cover_auto_photo_pixels_accept_one_pixel_and_keep_all_sample_slots`、`cover_auto_photo_pdf_preserves_points_and_relative_tolerance`、A判/B判の画像・PDF除外、`cover_auto_photo_guard_follows_raw_gate_and_does_not_reach_jacket_rules`、`cover_auto_photo_shot_exif_reaches_zip_and_folder_workers_with_upright_material`、`cover_auto_photo_pdf_worker_applies_transposed_page_guard_with_same_lease`、worker/範囲画面4方向一致と理由表示。
- 検証:上記の焦点nextest **629件成功**。G1はJPEG/PNG/WebP×向き2通り×撮影情報の有無×ZIP/フォルダの24入力組合せで、撮影情報が無ければ切れる同じ表紙が見送られることと正立寸法を検査した。worker/範囲画面の比較は16入力組合せ×4方向で一致。PDFは同じpassword/leaseのままG2で見送る。
- 合成フォルダ評価1件は切り出し0・見送り1で、summary.jsonだけを出力し、画像出力や個別名が無いことを検査した。UI snapshot **257件成功**。利用者データ・外部評価画像・sheetは使用/閲覧していない。
- r3対応: PNG/WebPのデコーダmetadataはG1の撮影情報だけに使い、bytesの向きは従来のrexifだけ、ファイルの向きは従来のrexif→WICに合わせた。`read_exif_orientation(path)`が実際にWICへfallbackするため、そのfallbackは維持する。JPEGの既読bytesから解析する場合も、従来のファイル経路と同じfallbackを使う。出荷済みの手動範囲・本文標本の座標系は変えない。
- r3対応: JPEGファイルの16MiBの先頭上限・壊れたmarkerの扱いも、`exif_reader`の同じreader処理を既読bytesへ適用して維持する。追加のファイル読み直しはない。`cover_auto_analysis_owned_file_bytes_keep_legacy_jpeg_prefix_limit`で、上限の後ろのEXIFについてbytes経路とファイル経路の従来の差も固定する。
- r3検証時の確認: このWindows環境ではWebPファイルのOrientation=6は従来のWIC経路で適用され、同じWebPのZIP bytes経路では適用されない。形式名で一律に向きを決めず、各入力の従来経路をoracleにして検査する。PNGのOrientation=6はZIP/フォルダとも従来の座標のままで、自動枠のOK保存・再読込の回帰が成功した。
- r3対応: G2へ渡す画素/point寸法は、その保存回転の判定角度へ投影してから比較する。G1の撮影情報は回転で変えない。
- r3回帰: `cover_crop_png_orientation6_auto_accept_reload_keeps_legacy_manual_pixels_in_zip_and_folder`で、Orientation=6のPNG表紙・本文を実デコードし、自動枠→画面のOK→保存→再読込→手動workerの画素とcache行まで検査する。`cover_crop_worker_and_dialog_g2_match_every_physical_and_saved_rotation`は、G2なしなら切れる合成ジャケットを物理回転4方向×保存回転4方向で比較し、ZIP/フォルダのworker・範囲画面の結果を固定する。画素/point両方のG1/G2の回転行列と、JPEGの従来の向きも焦点テストで確認する。
- r3検証: `cover_auto|cover_crop|cover_rotation|exif_reader` の焦点nextest **220件成功・失敗0**（既存EXIF読取15件を含む）。初回の既存G1テストのWebPファイル寸法の期待値は、従来のWIC経路をoracleにして修正後、全件再実行した。`cargo fmt`・`git diff --check`成功。レイアウト・文言を変更していないためUI snapshotは再実行せず、PNG更新なし。利用者データ・外部評価画像は使用していない。
- r4対応: 画像フォルダの本文標本は、読み終えた最大256KiBのbytesからrexifの向きの有無を調べ、取れなかったときだけ表紙と共通の `file_orientation_or_wic` でWICへfallbackする。既存probeが適用済みのrexifの向きは二重適用せず、RAW/AVIFのnative適用済み寸法も維持する。追加のrexif用ファイル読み直しや再走査はなく、WICも最大8標本だけ。ZIPの本文は従来のbytes規則のまま。
- r4の順序確認: 標本取得はworker・範囲画面・評価器とも判定の門より前。元寸法の門は各保存回転の `w/h >= 0.85` で、4方向一括の素材では正の寸法なら必ずどこかが通る。現在角度が縦長でも90/270度の結果を作るため標本が必要で、現在角度だけを見たskipは§21のDを変えてしまう。後段の `NotWide` の門は本文比率自体が必要なので標本取得をその後へは移せない。したがって順序は維持し、縦長の表紙でもrexif欠落時のWIC openが最大8回増える費用を明記する。
- r4回帰: `folder_webp_orientation_matches_cover_file_decode_and_zip_stays_bytes_only`、`folder_png_and_jpeg_orientation6_keep_released_dimensions`、`folder_wic_fallback_only_for_missing_rexif_and_never_double_rotates`、`folder_wic_budget_keeps_eight_attempts_and_256k_prefixes`を追加。実際のWICでWebP本文と表紙の寸法を比較し、PNG/JPEG・ZIPの従来結果、rexifのあるときのWIC省略、未知寸法での省略、先頭256KiB・最大8枠を固定する。
- r4検証: `cover_auto|cover_crop|cover_rotation|read_exif_orientation|exif_reader` の焦点nextest **225件成功・失敗0**。`cargo fmt`・`git diff --check`成功。UIの文言・レイアウトは変更せず、snapshot更新なし。利用者データ・外部評価画像は使用していない。

#### G2撤去 (利用者決定、2026-11-14)

- G2と `PhotoLikeReason::TransposedPage`、`PhotoEvidence`、`CoverAutoAnalysis.photo_dims` を削除した。
  写真の判定には既存デコードの `shot_exif` だけを渡す。本文標本から寸法一覧・単位を返す型も取り除き、
  比率または既存の見送り理由だけを返す。PDFのpage-box値は従来の比率・描画・layoutに必要な経路を維持し、G2専用の精密値の受け渡しは削除した。
- 保存回転ごとのG2用寸法入れ替え、A/B判の特例、範囲画面のG2の理由の振り分けを撤去。
  `photo-like/transposed-page` はコード・ログ・評価summaryからなくなる。G1の文言は変更しない。
- r3の自動枠→手動保存の向きの規則と、r4のフォルダ本文の向き・8枠×256KiB・再走査なしを維持した。
  G1のExifIFD/形式別/向きのテストも残す。grid/settingsマニュアルには寸法一致で写真を判断する説明がなく、変更は不要。
- 回帰は `cover_auto_no_shot_exif_keeps_jacket_decisions_for_every_physical_and_saved_rotation`、
  `cover_crop_worker_and_dialog_continue_for_transposed_page_dimensions_at_every_rotation`、
  `cover_auto_pdf_transposed_page_dimensions_continue_detection_with_same_lease` へ置き換えた。
  表紙と本文の縦横が一致していても、G1がなければ判定を続け、正例の切り出しを保つ。
  PDFは同じpassword/leaseを確認する。評価器の合成入力もG2の代わりに実際の撮影EXIF付きJPEGでG1を検査する。
- 検証: `cover_auto|cover_crop|cover_rotation|auto_material|exif_reader` の焦点nextest **240件成功・失敗0**。
  G1の形式別メタデータ、r3の自動枠OK保存・再読込と画素、r4のWebP/PNG/JPEG本文と8枠/256KiBを含む。
  `cargo fmt`・`git diff --check`成功。UI文言・レイアウトは変えず、snapshot更新なし。
  全cargoは共有runner `773 rc1` 経由。利用者データ・外部評価画像・製品起動・lib全件・build-dev・コミットは実施していない。

#### 評価器の後押し比較の使い方 (cfg(test) のみ、2026-11-14)

既存のignored評価に `MIV_COVER_AUTO_EVAL_FALLBACK=edge|edge18|density|density18` を指定すると、境目等の理由で見送った角度だけ後押しで切る。
未指定なら従来の判定のまま。`MIV_COVER_AUTO_EVAL_FALLBACK_RICHER=1` を併用すると `RicherElsewhere` も含める。
比率未確定・横長でない・写真の守りには掛けない。入力の指定・`target/` 内の出力先・共有runner経由の実行方法は既存評価と同じ。
`edge` は本文比率から丸めた全高の幅を綴じ方向側の外端へ置き、`density` は最大0.3幅だけ内側を探す。
`18` 付きは表紙がその幅の1.8倍以上のときだけ。summaryには `fallback_cropped/good/wrong`、元の見送り理由、角度別の後押し、IoUと内側移動量を追加する。
`MIV_COVER_AUTO_EVAL_FOLDER_LIST` でも同じ指定を使える。フォルダ入力はTHUMBS未指定時、summaryだけを出し、画像・個別名は出さない。
正解がないフォルダ入力のIoUとgood/wrongは `null`。本番の判定は変えない。

#### 型による後押しの使い方 (cfg(test) のみ、2026-11-14)

`MIV_COVER_AUTO_EVAL_FALLBACK=layout` / `layout-snap` を追加する。対象の見送り理由・G1等の守り・
`MIV_COVER_AUTO_EVAL_FALLBACK_RICHER=1`・保存回転ごとの扱いは既存の後押しと同じ。未指定と旧4方式の範囲は変えない。
各角度の元寸法から丸めずに `W=p×Hc`、`R=Wc/W`、`F=1.02W` を求め、右綴じでは次の位置へ置く（高さは全体）。

| 型 | R | 範囲の左端 |
|---|---|---|
| T1 | `[1.35, 1.85)` | `Wc-F` |
| T2 | `[1.85, 2.4)` | `0` |
| T3 | `[2.4, 3.1)` | `0.63W` |
| T4 | `[3.1, 3.9)` | `(Wc-2F-0.1W)/2` |

右端は左端+F、左綴じは左右反転する。表紙の外へは出さず、型の外のRには後押ししない。
`layout-snap` は保存回転後の高さ256行の解析rasterで、中央90%の行の輝度
`Y=0.2126R+0.7152G+0.0722B`（0〜255）の隣接列差 `D(x)=平均|Y(x,y)-Y(x-1,y)|` を取り、
3列平均 `d(x)=(D(x-1)+D(x)+D(x+1))/3`、`S(x)=d(x)/max(表紙内のdの中央値,1)` とする。
左綴じは画像を左右反転して右綴じの座標で探してから結果を逆変換する。
予測左端±0.15W、幅0.95〜1.08Wの組を解析列の境界から探す。背側（右綴じは右端・左綴じは左端）を2倍とし、
scoreは `(S(袖側)+2S(背側))/3`。選んだ背側のSとscoreが両方3以上で、予測位置のscoreから1e-9を超えて改善したときだけ合わせる。
弱い場合や予測と同点なら型の位置を保ち、候補同士の同点は予測位置に近い方を選ぶ。

行の `fallback.layout` に `kind`、`ratio`、`predicted`、`final_position`、`snap_applied` と境目の強度を記録する。
位置はその保存回転の元寸法の座標で、既存の `prediction` とIoUは保存回転前の座標へ戻して集計する。
summaryの `fallback_by_layout` はT1〜T4別の `cropped/good/wrong`、`fallback_layout_ratio_distribution` は後押しした型のRの0.2刻み分布。
`ratio_distribution` は守りや見送りも含め、本文比率の取れた全入力の選択角度のRを0.2刻みで数える（空のbinは省略）。
フォルダ一覧でも同じ情報を出し、THUMBS未指定時はsummaryだけで画像・個別名は出さず、正解のないgood/wrongとIoUはnullとする。
`truth.json` の `page_aspect: null` は受理する。ZIPは既存どおり本番の本文標本から比率を取り、
本文を持たないPNG入力のnullは比率不足として見送る。行の `page_aspect_decline` で不足・横長過多等を区別する。

#### 本の一覧と確認用画像の書き出し (cfg(test) のみ、2026-11-15)

`MIV_COVER_AUTO_EVAL_BOOK_LIST` にPDF・ZIP・画像フォルダの絶対パスのJSON配列を渡す。
既存の本の表紙解決、従来の向き、解析用デコード、本文比率の標本取得を共用する。
設定や固定は取り込まず、既定の並びから表紙を選ぶ。
`MIV_COVER_AUTO_EVAL_BINDING=rtl|ltr` は全冊共通で既定rtl。既存の後押しの変数も使える。
PDFは既存のテスト用in-processバックエンドで同じPageSizes・描画操作を行い、製品を起動しない。

`MIV_COVER_AUTO_EVAL_THUMBS=1` を併用すると、BOOK_LISTとFOLDER_LISTの両方で
`<OUTPUT>/thumbs/00001.jpg` のように入力順の5桁番号で書き出す。保存回転後の表紙を長辺640px以下へ縮め、
通常の判定の枠は赤、後押しの枠は青、見送りは枠なしとする。文字は描かない。
回転は既存のページキーでrotation.dbを読取専用で引く。既定は通常のデータ置き場、
`MIV_COVER_AUTO_EVAL_ROTATION_DB` で別のrotation.dbを指定でき、存在しなければ0°。
`MIV_COVER_AUTO_EVAL_SAVED_ROTATION=0|90|180|270` を明示した場合は全冊でその角度を優先する。
設定・回転DBを変更せず、表紙解決用キャッシュはテスト用の一時データ置き場に隔離する。

summaryの行には `sequence`（1始まり）、`path`、`kind`、`reason`、`rotation`、`R`、
`current_crop`、`fallback_crop`、`fallback_type`、`image_filename`（出力先からの相対名）を置く。
`current_crop` と `fallback_crop` は保存回転後の表紙の0〜1の辺座標。
FOLDER_LISTの既存の `crop` / `all_crops` は互換用の保存回転前のppm座標のまま保つ。
元の見送り・フォルダ入力の後押しの詳細と既存の集計も保つ。
開けない本・デコード失敗は `reason: error` と `error` を書き、後続を処理する。
重いI/Oのサムネイルと同じくCPU数が4以下なら1本、それ以外は最大2本で処理し、100冊ごとに進捗を出す。
FOLDER_LISTはTHUMBS未指定なら従来どおり画像・個別名を出さず、summaryだけ。
THUMBS指定時は画像とパスの対応を出す。BOOK_LISTは画像なしでもパスを含む。
生成画像を開いて確認するのは利用者のみとし、実装・設計担当は数値だけを扱う。

```powershell
$env:MIV_COVER_AUTO_EVAL_BOOK_LIST='G:\books-to-check.json'
$env:MIV_COVER_AUTO_EVAL_BINDING='rtl'
$env:MIV_COVER_AUTO_EVAL_THUMBS='1'
$env:MIV_COVER_AUTO_EVAL_OUTPUT='target/cover-auto-books'
# 任意: $env:MIV_COVER_AUTO_EVAL_FALLBACK='layout-snap'
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner '773 rc3' -WorkDir G:\home\mimageviewer-773 -Command 'cargo test -p mimageviewer --lib cover_auto_crop_directory_evaluation -- --ignored --nocapture'
```

合成PDF・ZIP・画像フォルダ、保存回転DB、開けない本・壊れた画像の混在を使い、入力順の行とJPEG、
赤青の画素、回転後の寸法、THUMBS未指定時の画像なしをテストする。利用者データや評価画像を閲覧しない。

検証:共有runner経由の `cargo nextest run -p mimageviewer --lib -E 'test(/cover_auto_crop::evaluation/)'` は
**25件成功・失敗0**。新規は `evaluation_book_list_mixed_real_sources_saved_rotations_and_failures_continue`、
`evaluation_book_list_binding_fallback_blue_and_folder_thumbnail_opt_in`、
`evaluation_book_list_binding_parser_keeps_default_and_rejects_invalid_values` と
`evaluation_thumbnails_*` の4件。PDFの実際の寸法取得・描画、同じPDFの重複、保存回転DBの不変、
UTF-8 BOM、開けない本と壊れた画像の後の継続を含む。`cargo fmt`・`git diff --check` 成功。
テストのTEMP/TMPはworktree内のG:に置き、少数の合成入力だけを使用した。本番・UI・設定・snapshotは変更せず、
製品起動・利用者データの評価・画像の閲覧・lib全件・build-dev・コミットは行っていない。

## 28. 全高・寸法の型を本番へ反映 (利用者決定、2026-11-15)

### 28.1 今回実装する変更1・3・4

- 自動の範囲は、保存回転ごとの向きで横の範囲だけを採り、上下は画像全体を残す。
  判定内部の余白・R等の計算は従来どおり。手動範囲は変更しない。
- 証拠不足の見送り（NoJacketEvidence/NoLocalEvidence/NoSpine/WeakFront/NoStructure/TwoPanelと
  RicherElsewhere）に、評価器のlayout-snapを同じ式・重み・閾値で適用する。
  RawAspectGate/PageAspectMissing/PhotoLike/NotWideは越えない。Rは `[1.35,3.9)` に限る。
  本文が写真の比率であることによる除外は行わない（利用者決定、§28.5）。
- 従来の判定が成功したT2 (`R ∈ [1.85,2.4)`) だけ、合わせる前のlayout位置との横方向IoUを比べる。
  0.6未満ならlayout-snapへ置き換える。0.6以上とT1/T3/T4では従来の横位置を保つ。左綴じは左右反転する。
- `cover_auto_crop/layout.rs` が予測・境目・snapの単一実装。評価器側はadapterから共用する。
  256行・中央90%の輝度列差・3列平均・中央値の下限1・背側重み2・強度/scoreとも3以上・
  予測より1e-9を超える改善、という既存の式と基準を変えない。
- 成功は `AutoCropSuccess {crop, method}` にまとめ、methodはCurrent・LayoutFallback(T1〜T4)・
  LayoutReplaced(T1〜T4)のenumにする。方法と型が食い違う状態を作らない。
  workerの `auto cover:` ログと範囲画面で方法と型を区別する。キャッシュの素材は従来の4方向の範囲を保持する。

### 28.2 キーと評価器

- 自動の印を `|autocrop:v3:{rtl|ltr}`、画像フォルダを `v3f` に更新する。
  **v2は未リリースなので移行不要**。v2/v2fとv1は正規化で旧印として識別し、現在の行はv3/v3fで再生成する。
  K2の出荷済みv1の流用は従来どおり残し、未出荷v2からの移行は追加しない。
- 評価器の未指定baselineは、全高化・型の補完・T2置換を含む本番の詳細report関数をそのまま使う。
  `MIV_COVER_AUTO_EVAL_FALLBACK` は比較用として残す。本番で成功済みなら追加の後押しはしない。
  赤枠/current_cropは本番v3、青枠/fallback_cropは追加の比較だけ。
  JSONの `production_method` / `production_layout` で本番の方法と型も記録する。

### 28.3 本文の保存回転を作り直し無しで実装 (利用者判断、2026-11-15)

本文の標本ごとに、既存のEXIF/WICの向きの規則を適用した寸法へ、保存した90°/270°の回転を一度だけ掛ける。
0°/180°と未登録はそのまま。回転後の寸法で先頭の横長の除外・比率の合議を行う。
PDFは0始まりの `page_key_for_pdf`、ZIPは `zip_entry_key`、画像フォルダは `normalize_path` のviewer共通キーを使う。
変換書庫の現行viewerは変換後ZIPのパスでページを保存しているため、そのキーを使い、元RARのキーへ置き換えない。
EPUBの生成PDFと評価用の複製ZIPは読取先とページ識別元を分ける。

`SampleRotations` は判定worker内だけで回転DBを読取専用で開く。DBが無ければ作らず回転なし、
読取エラーはログを出して回転なしとして続ける。再試行やUIスレッドからの同期I/Oは追加しない。
ZIP/フォルダの最大8枚×先頭256KiB・一覧の再走査なし・未知の枠の位置は従来どおり。
通常worker・範囲画面の専用thread・BOOK_LIST/DIR/FOLDER_LIST評価が同じrotation-aware samplerを使う。
評価器は既存のrotation_db指定をそのまま使い、表紙だけの角度overrideを本文へは掛けない。
DIRのPNGダミーは本文を持たないので、従来どおりtruthの比率を使う。

**本文を後から回しても作り済みの切り出しは作り直さない。** キー・依存の記録・退避の述語は変更しない。
キャッシュを消すか、既存の内容変更による再生成条件を満たすまで前の結果が残る
（画像フォルダの本文だけの差し替えは§26の既存の扱い）。範囲画面は開くたびに現在の回転で判定するので、
合わなくなった本は手動の範囲を指定し直す。利用者の判断として、後から回転するのはまれで、手動の範囲指定で直せるため、
作り直しは次の版で依存証明の形で直す。今回の版には依存証明・再生成制御を入れない。

将来の作り直しに向けて確認した既存経路:

- `App::rotation_page_owner_changed` → `evict_legacy_cover_thumbnails` →
  `legacy_book_cover_needs_reload` は、在メモリの回転ownerに合う旧Legacy代表だけを退避する。
  `viewer_context_registry.rs` は保持中の各contextにも同じ述語を適用するが、Automatic素材は対象外。
- workerのcache bypassも `refresh_legacy_cover && entry.cover == Legacy` だけ。
  `CacheEntry` はファイルのmtime/size、代表ページ、4方向の範囲等を持つが、本文標本のキーと保存回転の証明は持たない。
  読取時の一致判定に本文回転は含まれず、述語だけを広げても同じ永続行が再採用される。
- 手動pinの再読込はpin変更によるキー変更を使う。本文回転にはそのキー変更がない。
  行だけを消しても、回転前の判定workerによる遅い書き戻しと、保持中の別context/一覧の復元を解決できない。

次段の設計案: workerが実際に採った最大8標本のページキーと保存角度を、判定素材の依存証明として所有し、
request・catalog・K2・結果採用・保持した一覧まで一組で運ぶ。workerのキャッシュ読取ではその証明を読取専用の
回転DBと照合し、UIの採用/復元では在メモリownerと照合する。表紙のキーは標本から外すので表紙回転では再生成せず、
本文の依存キーが変わった本だけ既存の退避・request取消経路に通す。遅い旧結果も同じ証明で拒否する。
今回のsamplerはキーを照合するだけで依存を返さない。将来の依存証明の所有・永続化・採用境界は別に設計確認する。
全回転のglobal revisionや全contextの一括消去は使わず、detachedの述語・viewport経路は変更しない。

実装検証（2026-11-15）: `cover_auto_sample/saved_rotation_tests.rs` の7件で、PDFの0始まりキー・前後の8標本・
未知の枠・回転後の先頭横長除外、ZIPの識別元と読取先・8枚×256KiB、フォルダの既存EXIF/WICと保存回転の一回適用、
読取専用DBの欠落/破損/表なし時の継続を検査した。
`cover_auto_worker_zip_and_folder_sample_saved_body_rotations_without_cache_invalidation` は実workerで
0°/180°と90°/270°の違いを確認し、本文回転後のCacheOnly読取でキー・行・画素・結果が変わらないことも検査する。
`cover_auto_pdf_worker_samples_body_rotation_from_logical_page_with_same_password_and_lease` は
EPUBの論理キーと同じpassword/ReadTarget leaseを固定する。
`cover_crop_dialog_reads_saved_body_rotations_with_worker_sample_keys` と評価器のBOOK_LIST/FOLDER_LIST/DIRの3回帰も追加した。

共有runner `773 rc5` の焦点nextest `test(/cover_auto|cover_crop|cover_rotation/)` は **251件成功・失敗0**。
`cargo check -p mimageviewer --bin mimageviewer-core` 成功、範囲関連snapshotは **5件成功・PNG更新なし**。
`cargo fmt`・`git diff --check`・グリフ検査も成功。利用者データ・評価画像は使わず開いていない。
lib全件・build-dev・製品起動・独立レビュー・コミットは行っていない。

### 28.4 実装・検証記録

- 変更1・3・4とv3/v3fのキー更新を実装。worker・範囲画面・評価器は詳細reportの共通入口を使う。
  この段階では変更2は未実装だった。後続の利用者判断による実装は現在の§28.3を参照。
- 新規の判定回帰は `production_layout_fallback_accepts_every_structural_decline_and_preserves_guards`、
  `production_t2_replacement_uses_unsnapped_horizontal_iou_and_strict_point_six`、
  `production_full_height_preserves_horizontal_structural_crop_at_every_rotation`、
  `production_layout_ratio_boundaries_and_all_saved_rotations_share_the_prediction`。
- 範囲画面は `cover_crop_auto_method_layout_binding_messages_keep_one_typed_result` と
  `cover_crop_auto_from_crops_preserves_each_angle`、
  `cover_crop_layout_reasons_keep_small_viewport_actions_reachable` で、方法・型・綴じ・角度、
  1093×614の100%/200%での操作とクリックを固定した。
- catalogの `auto_material_legacy_v1_v2_null_rows_are_hits_in_all_readers` と
  `auto_material_v3_null_and_malformed_are_misses_in_all_readers`、K2・存続キー・master互換のgoldenを含む焦点nextest
  `test(/cover_auto|cover_crop|representative_cache_key|auto_material|color_filter|catalog|folder_thumb/)` は
  **551件成功・失敗0**。
- `cargo check -p mimageviewer --bin mimageviewer-core` 成功。
  `cargo test -p mimageviewer --test ui_snapshot cover_` は **5件成功**（範囲画面4件と環境設定1件）、PNG更新なし。
  `cargo fmt`・`git diff --check`・`python scripts/check_ui_glyphs.py`（危険な字形0件）成功。
- cargoは共有runner `773 rc5` から直列実行。既存native生成物の保護ACLによるLNK1114（error5）が出たため、
  既存物を変更せず `target/773-rc5-build` にビルド先を分け、FFmpegのDLL参照を既存vendor/binから補って完走した。
  TEMP/TMPもG:のworktree内、コンパイル並列数は2。利用者データ・外部評価画像・sheetは使わず開いていない。
  lib全件・製品起動・build-dev・独立レビュー・コミットは行っていない。

### 28.5 本文の写真比率による守りを入れない (利用者決定、2026-11-15)

設計担当の評価では、写真の負例390枚で後押しの誤りが14→236枚に増え、うち225枚は本文の比率が
写真の縦横比と一致していた。2/3・3/4・9/16・4/5との差0.002未満を除外する守りを入れると
誤りは11枚程度に減るが、画像フォルダの本18冊ほど（本文がちょうど3:4・2:3の本）が後押しで切られなくなる。
機能は利用者がオンにするまで効かず、切るほうを優先する方針なので、利用者の判断でこの守りは入れない。

本文が写真の比率で、かつ表紙が本文を横に倒した形（差2%以内）のときだけ止める弱い守りも検討した。
後押しの写真の誤りは222→92枚となる一方、画像フォルダの本4冊が切られなくなるため、これも入れない。
写真の比率ちょうどの本は、画素数で作られた本などで実在する（画像フォルダの本430冊中12冊）。

本文比率による除外のコード・専用の見送り理由・守りを検証するテストと公開文書の説明を撤去した。
撮影情報による既存のG1、全高化、型の補完、T2の置換、v3/v3fはそのまま。本文回転の後続判断は§28.3を参照。
`production_layout_fallback_accepts_photographic_page_ratios_without_shooting_exif` で
写真の比率でも型の補完へ進むことと、撮影情報がある場合は従来どおり見送ることを固定する。

撤去後の検証:共有runner `773 rc5` 経由の焦点nextest `test(/cover_auto|cover_crop/)` は
**210件成功・失敗0**。`cargo check -p mimageviewer --bin mimageviewer-core` 成功、
`cargo test -p mimageviewer --test ui_snapshot cover_` は **5件成功・PNG更新なし**。
`cargo fmt`・`git diff --check`・`python scripts/check_ui_glyphs.py`（危険な字形0件）も成功。
利用者データ・評価画像は使わず開いていない。製品起動・lib全件・build-dev・コミットは行っていない。

### 28.6 実機確認と左右の欠けの扱い (利用者確認、2026-11-15)

利用者が切り出しの結果を見て、概ね正しく切れていると判断した。表表紙の左右がわずかに欠ける本がある。裁断してスキャンした本は本文のページの中央側が少し削られるため、本文の比率ちょうどの幅で切ると表紙の左右が少し欠ける。利用者の判断で、これは仕様とする (幅の補正は入れない)。お気に入りごとの入/切は次以降の版で検討する。

## 29. 本文の保存回転による自動切り出しの作り直し (§1.773 続編、v5.3.0)

設計日: 2026-11-15。コード確認対象は `68101a56e`（設計更新を含む `cf50483ee` でも該当コードを再確認）。**利用者決定（2026-11-15、統括経由）により単純な保存側失効案へ改訂。2026-11-16にkey-only v4/v4fへの更新を含めて独立設計レビューと設計担当の合意を得た。実装・検証の集約先は§29.12。**
本節は§28.3の「作り直さない」という制約を解消する次段の設計であり、§28.3の出荷記録は残す。
判定の式・標本の選択・4方向の結果・手動範囲の座標系は変えない。

### 29.0 単純な案との比較（統括提案を先に検討、2026-11-15）

#### 比較する案と推薦

- **S0・単純削除案**: 回転保存に伴い本の自動代表のcatalog行/memoryと祖先フォルダ代表を削除/退避する。
  新しい本文proof、親の要求内での子の生成、子の世代の運搬は作らない。最後の古い書戻しはログと手動復旧で割り切る。
- **S1・削除＋通知の補強**: S0に、削除完了の通知、全保持一覧の対象別退避、子の通常の保存完了時の親再読込を足す。
  K2の既存の「完成した子だけを使う」という規則をそのまま使う。
- **P・決定前の§29案**: 古い行を残し、本文proofの不一致を再生成資格として読む。親が先に必要になっても、
  古い行を持つ選ばれた子だけ同じworkerで作り直す。coldの選択規則は保つ。

**比較時点ではPを推薦したが、利用者が下記の制約を受け入れ、今回は単純な案を採ると決定した。** 以下の比較は判断の経緯として残す。以前の推薦の根拠は、旧workerの最後の書戻しへの完全対策ではなく、
下記の「子がcoldになって生成されない」「保存OFFでは生成後も子の行が無い」という通常順序である。
単純な案がこの2点を残すことを仕様として受け入れるならS1の方が小さいが、これは今回の
「本文を回した後、必要な本/親の代表を作り直す」と既存の代表表示・保存policyを保つ条件を弱める。
特に先のREVISEで指定された「画像の無い親を先に要求しても、古い子の行を失って代表が消えない」条件を維持する限りの推薦である。
その機能制限を、まれな失敗として暗黙に受け入れたことにはしない。
以下はコードからの順序の確認であり、実機での発生頻度測定や実装済みの試験結果ではない。

#### 29.0.a 実際に使える既存部品と、その限界

| 既存の入口/関数 | 単純案で再利用できること | そのままでは行わないこと |
| --- | --- | --- |
| `rotation_page_owner_changed` (`app.rs:77470`) | 変更したページキーを受け、既存のUI失効を呼ぶ起点。reset/importもownerの更新へ集約済み。 | 本/祖先の自動代表行を削除するworkerは起動していない。 |
| `CatalogDb::delete_one` (`catalog.rs:2025`) | 指定した厳密なキーのthumbnail行を削除する。既存のcatalog revision triggerも進む。 | ページキーから本・全祖先・pin/type/binding/generation別の行・別namespaceのaliasを逆引きして一括削除しない。UIから新たにSQLを直呼びしてよいという意味ではない。 |
| `pin_materialization::Request::prepare` → `commit_pin_materializations` | 既存metadata workerで、選んだ行群のDB削除/seedとmemoryの置換を準備する型。UI同期I/Oを増やさない構造の参考になる。 | pin用のscope/identity/context取消の処理。本の本文回転を保存した後、移動先や全保持snapshotまで自動追跡する汎用の削除通知ではない。 |
| `evict_thumbnail_for_reload` (`app.rs:85279`) | 該当idxのqueue/request ID/CPU画像/textureを一体で退避し、通常demandへ戻す。 | catalog行や共有cache_mapは消さない。これだけだと同じstampの古い行を再びhitする。 |
| `evict_legacy_cover_thumbnails` (`viewer_context_registry.rs:1169`) | mountedとAtRest/Retiringの各ownerの該当idxだけを退避する走査・色失効の形。 | 現行はLegacyのLoadedだけ。snapshot内のsaved/list-view配列、Smartの退避grid全部を変更通知で消す処理ではない。 |
| `reconcile_restored_cover_thumbnails` (`cover_rotation.rs:11`) | snapshot/Smart/direct-sourceの復元時の共通入口。 | 「Automaticも対象」とするだけでは新旧を識別できない。Legacyの `scope.has_rotation(owner)` は非ゼロの有無なので、最後の回転を0°へ戻したことは検出せず、非ゼロなら生成後も復元のたび再退避し得る。 |
| `FolderResolveContext::cached_candidate` / `folder_cached_row_usable_in_cache_dir_with_pins` (`thumb_loader.rs:5335、5499付近`) | 既存のfolder selection proofが、調べたcatalogのrevision（候補行無しも含む）を保存・読取時照合する。 | 子が無ければ生成しない。表示済みの親textureをcatalog revisionの変更だけで直ちに退避する通知も無い。 |
| `ThumbMsg` のFinalized → `poll_thumbnails` → `fire_virtual_folder_writeback_if_ready` (`app.rs:57235、43622付近`) | 子の保存/省略が終わった事実を既存要求の完了として受け取る入口。 | 現行writebackは設定済みtarget_idx/items_genに限るミラー。全祖先の行/texture/休眠snapshotを対象にする通知ではない。旧request IDの完了はpollで捨てられ、parked contextの未pollの完了から別の表示中contextへ通知する保証も無い。 |

「消す関数が全く無い」とはしない。削除・owner単位の退避・復元入口はある。
一方、**回転保存→本と全ての該当親の行削除→保持一覧の失効→子の生成完了後の再選択**が
一つの既存関数で完結している、という前提も置けない。
削除する正確なキー/namespaceは既存builderで求め、ZIP内の文字を含む任意のLIKE/prefix削除へ置き換えない。

#### 29.0.b 通常の操作の順序（まれな失敗としては扱わない）

| 順序 | S0の結果・既存コードでの確認 | S1での塞ぎと残る条件 |
| --- | --- | --- |
| 一覧から本を開く→本文R/L→Escで一覧へ戻る | DB行を消してもsnapshotのLoadedは既に画素を持つので、そのまま戻る。`snapshot_ops.rs:1502/1634` の現行復元guardはAutomaticを見ない。 | 回転通知で保存配列を対象別にEvictedへ書き換えるか、復元時に変更済みの本を識別する情報が必要。単に現ownerに非ゼロ角度があるかでは不十分。Smartはsamplesの代入順も直す（§29.6）。 |
| 本Aの行を消す→Aのタイルを要求せず、一つ上のフォルダPの代表を先に読む | 現K2はAをcoldとして飛ばす（`thumb_loader.rs:6194–6210`）。BがあればB、ZIP/PDFのAしか無ければNone。**Aを表示する通常要求が無ければAの生成は一度も始まらない。** | 親も削除するだけ、または「子が出来たら通知」だけでは塞がらない。来ない完了通知では親を復旧できない。親を読むときの子生成を禁じるなら、回転保存側で別途、非表示でもAを生成する要求が必要。 |
| 上記でBを親行へ保存→後からAの通常タイルを作る | Aの保存が同じ子catalogのrevisionを進める。Bの既存folder selection proofはAの行を探したcatalogのrevisionも記録するので、**次の親worker読取ではBがmissになり再選択する**。DBのBが永久に正しいと判定される、とは限らない。 | ここは既存の選択proofを使える（新しい本文proofではない）。ただしLoadedのBは再読取されないため、子の保存完了後に対象の親texture/要求/保持配列を退避する通知が要る。 |
| Aの再生成を表示したがcache policyがOff、またはAutoで保存を選ばない | `load_one_cached` のcache_map追加はDB保存成功の枝にあり（`thumb_loader.rs:7517`付近）、生成した画素だけではK2の完成行は出来ない。子の行は消えたまま。 | 親を再読込してもK2はAを再び飛ばす。子をforce-saveすれば保存policyを変える。policyを保つなら、子の生成素材を親へ渡すか、親の要求で生成する仕組みが別途必要。単純な完了通知では足りない。 |
| 同じ本/親を別ウィンドウにも表示、またはmainをparkしてF12で本文回転 | 一方の `thumbnails` だけの退避ではもう一方のLoadedと共有/別々のmemory行が残る。 | Legacyのregistry走査の形を再利用し、該当ownerのrequest/queue/画素と全ての保持配列を対象にする。全contextの一括clearや、実装上のmountをして回る必要は無いが、既存述語の変更だけでは済まない。 |
| 本/親の削除完了後、子を一度も表示せず正常終了→再起動して親Pから開く | 消した行自体が復活することは無い。しかしAがcoldになった事実もそのまま残るため、Aしか無いPは次回もNone、BがあればB。 | 揮発的な「子生成待ち」/完了通知だけでは復旧しない。削除前のAと元からcoldな候補を区別する永続の印、または新起動の読取側での子生成が必要。**途中で強制終了したという前提は不要**。 |
| 初回の判定中、向きの違う本を開いてR/Lで直す | 利用者はthumbnail jobを意識せず普通に回せる。Loadedだけを見た退避ではpending/CPU待ちを取りこぼす。request IDをremoveしても、そのworkerが後からcacheへ保存することまでは止めない。 | 同じ本/該当親のpendingも取消対象にし、queue/CPU画像を既存ownerから除く必要がある。この操作全体を「初回だからまれ」とは分類しない。頻度の実測なしに、長いPDF/初回decodeとの重なりを無条件に割り切らない。 |
| 削除workerを投げた直後に一覧へ戻り、新しい要求が古いmemory/DBを読む | UIで新しいSQLを出来ないので削除は非同期になる。新request IDのcache hitは、旧request IDの取消では拒否できない。 | 削除完了後にも対象を退避するか、完了まで対象要求を抑止する順序が要る。元contextを離れたことだけで削除/完了通知を捨てる設計にはできない。 |

最初の削除でK2がBを選ぶこと自体は既存のcold規則であり、Bを出すことを一律禁止する必要はない。
問題は、**Aの生成開始が無い/行にならない場合**と、**BのLoadedが次の読取へ進まない場合**である。
この2つを「古い保存が最後の瞬間に割り込む」という1ケースにまとめない。

#### 29.0.c まれな失敗・狭い競合として割り切るもの

DB削除/保存失敗、disk full、処理中の強制終了は既定どおりログ・通知と手動範囲指定/キャッシュ再作成で扱う。
そのためのtransaction横断rollback、retry timer、終了時の全job待合せは追加しない。
また、通常順序の取消/失効が済んだ直後の最後のDB commitとの競合だけが残る設計なら、統括提案のとおり割り切る余地がある。
ただし、上表のK2・保存OFF・正常再起動は並行処理が一切無くても再現するため、この区分には入れない。
初回判定中の回転も通常の入口であり、UIに古い結果を採用することまで丸ごと許す理由にはしない。
Pもwriter停止・保存barrierを増やさない。正常な読取に必要なproof照合が旧行をmissにすることを使い、
まれな障害のためだけに回復状態を追加しない。

#### 29.0.d 最小限の塞ぎを足した場合の比較

S1に必要な追加は次の3群である。まずこの範囲で止められるか検討した。

1. **既存UI失効の補強**: 本/該当祖先の行削除をworkerで行い、完了後にも対象のmemory/request/textureを失効する。
   既存registryとsnapshot/Smartの保存配列を同じscopeで退避し、0°へ戻す操作も扱う。
2. **親の再読込**: 子の保存完了を元contextがparked/移動済みでも対象親へ届ける。
   既存のUI request IDのcurrentnessと、保存が完了してcatalog revisionが進んだ通知を混同しない。
   親workerの次の読取では既存folder selection proofを使い、Bのcacheを再選択する。
3. **なお残る穴**: 子が非表示/保存OFF/再起動済みでもAの素材を用意する必要がある。
   これを閉じるには、削除側に非表示の子の生成要求とその完了を管理する責務を足すか、
   永続のdirty印から新起動でも生成を再開するか、読取側で子を生成する必要がある。
   force-saveや「本の一覧を一度開く」を必須にするなら機能/操作の変更として明示の判断が要る。

3まで閉じる案をS2とする。S2は実装案として採らず、必要な責務を以下に比較する。
これはテストの**責務の種類**の比較で、実装LOCや実行済みテスト数の見積りを装わない。

| 比較項目 | S0 | S1（1・2まで） | S2（通常順序の全穴を閉じる） | P（旧proof案） |
| --- | --- | --- | --- | --- |
| 新しい永続情報 | 無し | 無し | 子が削除済み/元からcoldを識別するdirty情報が要る。不要にするなら親読取時生成等へ制約を緩める。 | 素材の最大8組の本文proof。別のdirty queue/DB無し。 |
| UIの追加分岐 | 対象退避 | 削除完了・子保存完了・保持配列・pendingの退避 | S1に加え子の要求開始/完了/失敗/正常終了後の再開の扱い | 既存退避＋採用/復元のメモリ照合。一時fallbackだけ選択証拠を運ぶ。 |
| K2/source生成 | 変更無し、代表無し/Bのままが残る | 子の通常要求が来て保存された場合だけ復旧 | 非表示の子生成と親への完了連携、Offなら非永続素材の受渡し、または親読取時生成 | 古い行を持つ子だけ、親を処理中のworkerで共通helperを呼ぶ。新しい子job/常駐owner無し。 |
| 普通の復元/複数窓のテスト | 問題を検出するが直らない | 通知の宛先/保存配列/完了前後/parked等を検査 | S1＋起動/終了/途中の移動をまたぐdirtyと生成責務の遷移 | §29.10のowner単位・採用/復元の照合を検査 |
| 再起動/Off/親を先に開くテスト | 不成立 | 不成立のまま | 永続dirty→生成→親更新、非永続素材の寿命を追加検査 | 古い行を残すので、同じ読取→生成の検査へ集約できる |
| 失敗後のBとAへの復帰 | 元からcoldとの区別を失う | 完成通知が来なければ復帰しない | 子の失敗と再開・Bの有効期間を別途決める必要がある | TemporaryFallback/認証不足の最小memoは必要。これはPの明確な追加コスト。 |

S0の小ささは認める。S1もPよりpayload/codecの変更が少なく、限定された場面では十分である。
しかし通常の全条件を守るS2は「既存の削除関数を呼ぶだけ」ではなく、新たな子生成/dirtyの管理へ移る。
Pも大きいが、**保存してある素材そのものを入力の証拠とし、必要になったworkerだけが作り直す**ので、
新しい常駐の生成ownerや永続の作業待ち列を持たず、完了待ちの組合せを減らせる。

#### 29.0.e 利用者の決定と採用する案（2026-11-15）

利用者は、回転した本のtileを表示する前に上のfolderを開くと代表が別の本/無しになること、
再起動をまたいでも子の行が出来るまで続くことを受け入れ、**単純な保存側失効を採る**と決定した（統括経由）。
§29.0の比較で「親の制約を残さない」としていた条件をここで変更するため、比較時のP推薦は現在の結論ではない。

採用案はS0に、削除完了後の対象退避と、保持素材のセッション番号による復元判定だけを足す。
S1にあった「子保存完了で親を特別に退避」も採らず、親は既存catalog revisionに基づく次の読取に任せる。
Off/Auto非保存では本を表示しても行が出来ないことを含め、§29.0の制約を受入れ範囲として明記する。
Pのproof/codec変更・読取照合・K2内生成・子の世代の引継ぎは撤回し、§29.11の退けた案へまとめる。
以後の§29.1〜29.11はこの利用者決定を正本とする。比較表は採用案の保証へ読み替えない。

### 29.1 採用する範囲と採らない範囲

利用者決定（2026-11-15、統括経由）により、**回転保存側で本の自動代表cacheを消し、完了後に通常の要求で読み直す案**を採る。
§29.0の比較で挙げた親代表の制約を受け入れる。P案の「子を必ず親の中で生成する」という保証は要件から外す。

- 自動のキーだけを `|autocrop:v4:{rtl|ltr}`、画像フォルダを `v4f` へ更新する。出荷済みv3/v3fには本文回転後の古い結果が残り得るため、新しいキーの通常要求で作り直す。旧行のコピー・一括移行・削除は行わない。
  `AutoMaterial`、`auto_crops`、4方向の結果、schema/codecは変更せず、本文proofも追加しない。v4はキーの世代であり、新しい素材形式を意味しない。
  K2の出荷済みv1互換は保つが、v3/v3fへのfallbackは追加しない。現行行が無い場合の既存v1互換だけを維持し、現行行の不正・署名不一致時に旧行へ戻らない。
- 新しい本文proof、読取の5入口での照合、採用直前の本文角度照合、K2内の子生成、子の世代/素材の新しい運搬は作らない。
- 対象は回転の変わったページを本文として持つ**本自身のAutomatic**。ZIP・変換書庫は同じ外側書庫の全ZipDirも保守的に対象とする（2026-11-16、設計担当判断）。全方向が見送りのAutomaticも含む。
  手動範囲・edit preview・NotAutomatic・LegacyV1・通常のページ行・RAW/動画/Remoteの対象範囲を変えない。
- 代表自身の回転だけなら削除/再生成しない。既存4方向の結果を在メモリの角度で選び直す。
- 本文標本8枚を再現して対象を絞る処理は足さない。代表以外のページ変更は、その本の自動結果を保守的に失効する。
  本ごとの失効であり、全回転revisionや全contextの一括clearにはしない。
- 親の代表へ専用処理を足さない。子の削除/保存でcatalog revisionが変われば、既存folder selection proofが次の読取で再選択する。
  親Loadedの即時更新、子の生成完了通知での親退避、非表示の子の強制生成は今回の範囲外。

### 29.2 対象の本と「表紙だけ」の求め方

ページキーは既存の `page_key_for_pdf` / `zip_entry_key` / `normalize_path` と同じ規則を使う。
**ページキーの文字列だけでは、ZIP全体/ZipDirという論理単位、変換元の書庫、画像フォルダの代表選択までは一意に決まらない。**
通常の回転handlerでは `GridItem`・現在の論理source・archive overrideを一緒に捕捉し、
reset/import等のkeysだけの通知では同じtyped source helperにworkerで照合させる。拡張子や区切り文字だけの別解釈を作らない。

| ページの種類 | 本の識別と照合 |
| --- | --- |
| PDF / EPUB | `GridItem::PdfPage` と既存の論理PDF/EPUB path、0始まりpage番号を使う。EPUBの生成PDFを別の本としない。 |
| ZIP | 外側の物理ZIPとentryの組を既存helperで扱う。entry中の `::` / `|generation:` / `#pin:` 等をderived keyと混同しない。 |
| ZipDir | 保存キーの論理prefixは維持するが、失効は外側のZIP・変換元書庫の単位とする。変更ページを含むprefixに限定せず、未保持の別prefixやその外へ固定した行も同じ書庫なら対象にする。 |
| 変換書庫 | viewerの保存回転キーは既存どおり変換後ZIP。`archive_source_override` / 既存の変換元対応で論理書庫へ結び、変換元tileのwriterキーを求める。対応のworker lookupも既存cache DBで行い、UIで新たに開かない。 |
| 画像フォルダ | 画像の直上folderを本の候補とする。`FolderImageOrigin::Direct` に相当する代表だけが対象。祖先folderがK2で継承した素材は、その祖先自身の対象行としては消さない。 |

本の識別は既存 `BookCoverContainer` / `ReadLocation` / request builderの入力を共有した純粋な値とする。
表紙の判定はUIでscanせず、workerが格納行の `CoverIdentity::Page` を復元して変更ページと比較する。
**変更ページ==その行の代表ページならその行を残す**。pin/代表順が違う過去の行は、行ごとの代表で判定する。
画像フォルダでは既存selection proofのwinnerとcoverから、直下画像と子由来の代表を区別する。
同じleafを持つことだけで、上のfolderや別の外側書庫の行まで削除しない。

ZIP・変換書庫ではpinの実効prefixで失効範囲を絞る方式を撤回する。本文回転はまれで、同じ書庫内の再生成の費用より、未保持の固定元を取りこぼす分岐を減らすことを優先する。既存derived-key parserで現行印を確認し、外側書庫とcatalog scopeを厳密に比べて全ZipDir行を列挙する。変更keysにその行の代表以外のページが一つも無い場合だけ、その行を残す。同じ書庫の別代表にとって本文変更なら、その別代表は作り直され得る。PDF・画像フォルダの範囲は従来どおり。

初回生成中で行が無い場合は、捕捉した通常要求のpin/代表選択をworkerの既存解決helperで確認して同じ除外を行う。
PDFの既定page、ZIPの先頭選択、folderの既存sortを共有し、**decode/自動判定/子の生成は起こさない**。
不明な対応や読取失敗はログを出し、推測した全体削除や別の本への拡張はしない。新しい回復機構は足さない。

### 29.3 消す行、保存先、workerのmemory

削除は短命のcache更新workerで行う。UIは変更keys・typed source・既存handle/設定値を捕捉するだけ。
既存metadata/pin materialization workerの「準備→DB更新→完了通知」の実行パターンを使うが、pin変更として通知せず、
元contextを離れたという理由だけでこのcache削除を取り消さない。UI同期SQL・Shell・画像decodeを追加しない。

1. 通常のsource parent catalog、`collection_source_parent_identity`、現在/保持中の `RequestCatalog` と
   `ThumbnailPoolCatalog` の保存先を、既存storage-scope規則で求める。
   再起動後に開く別名行も残さないよう、固定synthetic scopeは既存のsynthetic-path helperと設定中のSmart等のIDから
   到達可能な保存先を列挙する。UIはID/handleだけを捕捉し、DBの存在/ヘッダ読取はworkerで行う。
   `search_results_synthetic_path` / `rating_view_synthetic_path` / `reading_history_synthetic_path` /
   `bookmark_view_synthetic_path` / `subfolder_expansion_synthetic_path` / `smart_folder_synthetic_path` の既存識別を使う。
   全cacheディレクトリの再帰scanや、hashからのパス逆算を通常の回転に追加しない。
   既存のCatalogWork/admissionを守り、存在するDBだけを更新する。cache削除によりretiredになった旧受付が
   新しいepochを取得し直してDBやdirectoryを作り直す経路は作らない。
2. **既存builderだけで、過去の全キーを有限の設定値から列挙できるとはしない。**
   `|types:<16hex>` は過去のpolicyのdigestであり、pin・sort/depth・generationの過去の組合せもある。
   対象catalogで既存行のkey/cover/material/selectionのヘッダを読み、保持memoryの実在キーも候補にする。
   WebP blobを全件decodeせず、必要ならヘッダ専用のcatalog readerを既存readerの列互換規則に沿って用意する。
3. `representative_auto_crop_key_parts` の厳密な位置/文法で現行v4/v4fを識別する。
   v1/v2/v2f/v3/v3fは正規化で旧印として認識するが、旧行の移行・再採用やprefix削除には使わない。
   `folder_thumb_*_cache_key_for_path`、`cache_key_for_request`、`pinned_cache_key`、
   `representative_cache_key_for_policy_with_auto` 等の既存builderと照合する小さな単一helperで、その本自身の行かを確認する。
   行から得た検証済みderived部品はそのまま保持し、生成元を復元できないtypes digestを推測しない。
   右綴じ/左綴じ、full-path/basename、pin/type/generation、folderの既存sort/depthを網羅する。
   過去pinの行の実在文字列も扱うが、通常のZIPページ名の一部を代表キーと誤認しない。
4. §29.2の表紙除外とAutomaticの確認を通った**実在する厳密なキーの集合**を作り、
   `CatalogDb::delete_one` または同等の `WHERE filename = ?` のbatch削除に渡す。
   LIKE/SQL prefix削除にしない。OFF/uncroppedのキーの綴り、行、pin設定は変更しない。
5. workerが参照する `cache_map` の同じ行も除去する。SQL中はmap/queueのlockを持たない。
   完了通知は `InvalidatedRow { slot, catalog, key }` の行単位で返し、捕捉後に作られた現在/保持先のmemoryにも同じ行の除去だけを適用する。未解決の要求は同じ行のslot・保存先・キーを一組として照合し、別の行の一致を組み合わせない。保存先はRequestCatalogのParent row.folder／Collection identity、Fixedはowner poolを使い、Parentのキーにはrow.row_keyを使う。
   その処理はmemoryの短い操作に限り、UIでDBへ問い合わせない。

これは本のcacheの失効であり、親folderのDELETE、親のepoch更新、K2への特別な生成要求を発行しない。
catalog revisionは既存triggerに任せ、counterを手動調整しない。

### 29.4 読み直しの順序（削除完了後に退避）

**先にLoadedを退避して削除workerを待たせる方式は採らず、削除とmemory除去の完了通知を受けてから退避する。**
新しい要求が削除前の古い行を再hitする順序を避け、要求抑止のフラグ・待合せ・timerを増やさないためである。
処理中は従来のtextureを保ち、UIをモーダル化したりworker完了を同期で待ったりしない。

```text
回転ownerの実効変更（0°へ戻す場合も含む）
 → keys/source/設定/handleを捕捉して有限のcache削除workerへ渡す
 → DBの厳密キー削除と既知cache_mapの同じ行の除去
 → App所有の完了通知を非blockingで受ける
 → 対象のセッション番号を進め、現在のmemoryも同じキーで除去
 → mounted / AtRest / Retiringの対象idxと要求を退避
 → 次の通常demandが新request IDで読み直す
```

完了通知はview/items generationの結果として破棄せず、Appがその本の失効として受ける。
新しい常駐actor/子jobは作らず、短命workerの報告を受ける小さなApp所有のserviceに、番号と完了channelをまとめる。
個々のcontextへpending boolを足さない。複数回のR/Lは有限の変更batchとして処理し、完了順に対象番号を進めればよい。
古い削除完了が後着してもう一度退避することは許し、順序の巻戻し・再試行protocolを作らない。

- `rotation_page_owner_changed` を共通入口にし、同じ角度への更新は既存どおり何もしない。
  `scope.has_rotation(owner)` の非ゼロ有無を失効の代用にせず、変更keysを扱うので90°→0°も検出する。
- mountedは `evict_thumbnail_for_reload`、AtRest/Retiringは `evict_legacy_cover_thumbnails` の走査と同じowner単位の処理を使う。
  AutomaticのLoadedだけでなく、その本の自動要求で**Pending/Upgrade中、queue内、CPU画像待ち**も対象にする。
  `requested.remove` が保持するCPU画像とupload_orderも除き、遅い同IDの結果は既存照合で捨てる。
  表紙だけの変更に当たる要求はworkerの代表確認で除外する。Loadedが無いことだけで全要求を対象にしない。
  ただし、代表ページがまだ決まっていない (leaf 未解決の) 生成中の要求は、同じ書庫の別の代表が本文の変更として
  失効したとき、書庫単位の一致で取り消すことがある (実装レビュー r3 の P2)。これは結果が正しいままの不要な
  再生成だけで、同じ書庫の別代表を別 context で生成中に表紙のページを回す、という組み合わせに限られる。
  §29.2 の書庫単位の失効 (粗く単純にする判断) と同じ考えで割り切り、要求 ID と writer キーを結び付ける
  照合は足さない (設計担当判断 2026-11-16)。
- epoch全体をbumpしたり、thumbnail poolを再起動したりしない。兄弟contextと同contextの無関係な本・通常ページの要求は保つ。
  色filterも既存のmain-only権限を保ち、対象の寄与だけを除去する。
- 初回判定中の要求や削除完了までに開始された要求も、通知時の対象走査に含める。
  通知を受けて退避した後の新要求は、削除済みのDB/memoryを使う。source jobの最後の保存がその後で割り込む場合だけは§29.7の割切りとする。

### 29.5 保持一覧の判定（素材に付けたセッション番号）

既存の復元入口を使い、**素材がロードされたときの番号**を復元時に比べる。
snapshotへ保持したときの現在番号を後付けする案は採らない。古いLoadedを保存し直しただけで新しい素材になってしまうためである。

最小限のruntime値の案:

```rust
// 永続化しない。既存typed sourceから得る。
struct AutoCoverSlot { logical_book: BookCoverId, representative_page_key: String }
enum CoverCacheStamp {
    Untracked,
    Tracked { slot: Arc<AutoCoverSlot>, serial: u64 },
}
// App内: slotごとの完了済み削除番号。未登録は0。
```

本ごとの番号を代表ページ別のslotに分けるのは、過去pin/sortの異なる行を一緒に消す場合にも、
「その行にとって表紙だけの回転」を誤って失効させないためである。bindingだけが違い代表が同じなら同じslotを使う。
slotの本はPDF・画像フォルダではtile自身、ZIP・変換書庫では**外側書庫**に正規化する。同じ書庫・代表ページならprefixをまたいで同じ番号を使い、未保持のZipDir行からも捕捉後の保持素材へ失効が届く。writerキーとcatalogの行の形は変えない。K2の子の番号を親へコピーしない。
親が子のAutomaticを持っていても、その親のslotは更新しない。

1. 削除完了時、削除/失効対象のslotだけ `serial += 1`。代表だけの変更で除外したslotは進めない。
   実在行の無い初回要求でも、workerで本文変更と確認した通常の代表選択slotは対象にする。
   全回転global revision、変更履歴、永続のdirty印にはしない。
2. AutomaticをUIで採用するとき、その本と `CoverIdentity::Page` からslotを付け、現在の番号をstampに記録する。
   このstampは `ThumbnailState::Loaded` のUI側の小さな付帯値とし、AutoMaterial/worker結果/catalogへは入れない。
   NoRowの初回要求の識別に必要な論理本の事実は、既存のrequest admission/source factsへ含めるが、番号を子へ運搬しない。
   通知前の古い素材は通知で退避され、通知後の旧request IDは採用されないので、採用直前の本文proof照合は不要。
3. contextやsnapshotのcloneは付帯stampもそのまま保持する。番号をclone時に更新しない。
   snapshotのsaved/list-view、Smartのprepared grid、direct-sourceも同じ規則にする。
4. 復元先のitems/thumbnails/generation/auto_aspect等をそろえてから、`reconcile_restored_cover_thumbnails` でstampを比較する。
   現在のslot番号と異なるAutomaticだけEvictedにし、そのidxの古いaspect寄与とCPU/要求を残さない。
   DBを読まず、在メモリの番号だけを比較する。
5. `rebuild_auto_aspect_samples_from_loaded` で最終Loadedに整合する。Smartでは現在の照合→auto_aspect再代入の順を逆にし、
   saved auto_aspectの代入後に照合する。snapshot/direct-source/list-viewには照合後の再代入は無いが、最終境界のsamplesを同様に検査する。
   古いstreak/gateは再利用せず、scroll/selection/無変更の兄弟contextを一括resetしない。

この番号は受動的な保持素材の新旧判定だけに使い、worker cacheの読取照合、依存証明、保存の世代比較には使わない。
再起動で番号はリセットする。削除が完了していればDB行は既に無いため、番号を永続化する必要は無い。

### 29.6 親の代表と、受け入れた制約

親への専用削除・退避・子の生成完了通知は追加しない。
子のcatalog行を削除するとrevisionが進み、既存K2の `cached_candidate` は行無しを飛ばす。
既存の親のselection proofも次のworker読取で古くなるので、その時点で通常の候補選びへ戻る。
表示中の親textureを即時に選び直す保証は付けない。

利用者が受け入れた挙動:

- 回転した本のtileを表示する前に上のfolderを開くと、別の子の代表、または代表なしになる。
- 子のtileを通常要求で生成し**子の行が保存された場合**、次に親を読むと既存revision照合で元の候補へ戻れる。
  「一度表示すれば必ず保存される」とはしない。OffやAutoの非保存では行が出来ず、別の代表/代表なしが続くことも§29.0の受入れ範囲である。
  force-saveや新しい親子間の画素転送でこの制約を隠さない。
- 再起動をまたいでも、子の行が出来るまでこの状態は続く。元からcoldか削除済みかを識別する印を保存しない。

### 29.7 まれな組合せは割り切る

削除完了前の古い行の再採用と、通知時点で識別できるpending/CPU待ちは§29.4で通常の順序として塞ぐ。
一方、最後の旧workerのDB/memory書戻し、削除失敗、削除処理中の終了などはログを残し、
手動の範囲指定またはcache再作成で直す。再試行timer・writer停止・終了barrier・rollback・回復用の永続印は追加しない。
処理の開始/完了と対象をログへ出し、DBの実エラーは既存の通知経路を使う。
初回判定中の回転を丸ごと無視するのではなく、最後の保存との競合だけがこの割切りに残るようにする。

### 29.8 detachedの範囲と合意の扱い

対象は `viewer_context_registry.rs` の既存owner別退避、Loadedのruntime stampの保持/clone、保持一覧の復元に限る。
viewport生成/破棄、HWND、geometry、入力述語、promote/retireの状態遷移、PDF epoch所有を変更しない。
これは共有データの変更通知を該当ownerのcacheに反映し、古い保持素材を識別する構造的変更であり、
windowの表示状態を推測して症状を隠す修正ではない。`detached-rework-plan.md` §2に沿う。

**2026-11-16、単純な保存側失効について設計担当と独立レビュー担当が改めて合意した。**
今回の合意は、key-only v4/v4f、削除完了後のowner別退避、UI側session stampとclone/復元、Smartのsamples代入順を対象とする。
P案の本文proof/codec変更やK2内生成を承認したものではなく、以前のP案への合意の流用でもない。
新しいdetached専用状態、時間窓、viewport推測を作らず、共有cache変更の事実を既存context所有境界へ届ける構造的修正として§2に適合すると判断した。
同計画§11にもこの範囲と合意を記録する。

### 29.9 触るファイルと関数（実装時）

| ファイル | 範囲 |
| --- | --- |
| `src/app.rs` / `src/app/cover_rotation.rs` | `rotation_page_owner_changed`、typed source捕捉、削除workerの完了poll、slot番号、`evict_thumbnail_for_reload` と復元のstamp比較。 |
| `src/app/viewer_context_registry.rs` | Legacyと同じowner別走査で対象のmemory/request/textureを退避。stampの保持とclone。兄弟/通常ページの要求はそのまま。 |
| `src/grid_item.rs` | Loadedのruntime stamp。AutoMaterial/永続codecを変えず、snapshot等で自然にcloneする。 |
| `src/catalog.rs` / 既存metadata worker周辺 | ヘッダ読取と厳密キーの削除batch、既存CatalogWork/admission/column互換を維持。新しいschemaや全cacheのretirementを行わない。 |
| `src/thumb_loader.rs` / `src/app/book_cover.rs` / `src/app.rs` のkey builder周辺 | 本と代表の共通識別、削除する実在キーの厳密な照合helper、自動keyだけのv4/v4f更新。source生成/K2の選択規則/判定式/AutoMaterialは変更せず、v1互換を保つ。 |
| `src/app/thumbnail_requests.rs` | 必要な論理本のsource factsの保持と、対象のPending/Upgrade/CPU待ちの既存remove経路への接続。新しい子requestや世代は作らない。 |
| `src/app/snapshot_ops.rs` / `src/app/smart_folder.rs` | 保存素材のstampを保持し、復元先の全状態を代入してから照合。Smartのauto_aspectの順序を修正。 |
| `docs/display-pipeline.md` / `docs/ui-responsiveness.md` / `docs/virtual-folders.md` | write側失効、worker限定SQL、復元順序、親の制約。新しいread proofがあるという説明は書かない。 |
| `htdocs/mimageviewer/manual/known-issues.html` / `manual/grid.html` | 旧問題の項目を削除し、下記の受入れ制約を説明する。CHANGELOGは公開担当が扱う。 |

### 29.10 テスト方針と公開文書

全て合成入力と隔離DBを使う。以下は実装後の回帰条件であり、設計段階の実行結果ではない。

1. **対象/key**: PDF・EPUB・ZIP/ZipDir・変換書庫・画像folder、binding両方、pin/types/generation/drive/Collection等の別名を検査。
   任意の区切り文字を含む通常ページ、別の外側書庫/PDF/画像フォルダ、手動/NotAutomatic、K2の親自身の行は削除しない。同じ書庫の別prefixは対象にする。
   未表示のaliasから次回起動後に古い同じ本の行を再採用しないことを検査する。
2. **表紙除外**: 代表ページをR/Lしても行・memory・Loadedのstamp/textureを維持し、描画角度だけが変わる。
   別代表を持つ過去pinの行は行単位で判定し、cold/初回生成中でも同じ代表選択規則を通す。
3. **0°/本文**: 本文90°→0°でも厳密な対象行とmemoryが消え、次の通常要求で現在の保存回転を使う。
   同角度のupdateではjob/serial更新無し。非標本の本文ページによる保守的な同じ本の失効は許す。
4. **順序**: barrierで削除workerを止め、削除前はLoadedを保ち、完了後だけ退避する。
   完了前に出た新request、Pending、Upgrade、CPU upload待ちも除き、旧IDの画像/Finalizedが新要求を壊さない。
   最後のDB書戻しとの競合は復旧保証のテストを増やさず、割切りの境界をログで検査する。
5. **context**: mounted/AtRest/Retiringの該当する本（ZIP・変換書庫はその中の全ZipDir）が退避し、兄弟の別書庫/PDF/画像フォルダ/通常ページのrequest ID・queue・pixels・generationは不変。
   元contextを離れてから削除完了しても通知を捨てず、新しい表示先と共有memoryへ反映する。全体poolの再起動はしない。
6. **保持一覧**: 一覧→本→本文回転→Esc、snapshot saved/list-view、Smart、direct-sourceで、古い素材のserialが保持され、
   復元完了後に対象Loadedと旧samplesが無いことを検査。古いLoadedを新しくsnapshot保存してもserialを更新しない。
   Smartはauto_aspect代入後に照合。無変更の復元/同じ一覧の同期再利用は再decode/兄弟失効を起こさない。
7. **親の制約**: 子行を消した後で親を先に読むと別代表/Noneになる既存K2を固定する。
   子の通常生成と保存後、親の次の読取でcatalog revisionにより再選択する。子を生成しない/Off非保存/正常再起動では
   制約が残ることも期待値に書き、隠れた強制生成・force-save・親の即時退避をしないことを検査する。
8. **互換/失敗**: キーだけv4/v4fに更新し、出荷済みv3/v3fの旧行を再採用/移行しない。K2のv1互換と全kindのcodecは不変で、v3 fallbackは無い。
   UIのSQL read counterを増やさず、LIKE/prefix DELETEを使わない。
   削除失敗はログ/既存通知で完了し、retry/印の永続化/保存データの再作成を起こさない。

既知問題「本文のページを回転した後も、表表紙の自動切り出しの結果が前のまま残る」のh3/pを削除する。
受入れた親の挙動は**既知の不具合として新設せず、grid.htmlの自動切り出しの説明へ制約として記載する**。
意図的に採った選択規則だからである。公開文:

> 本文のページを回転した後、本のタイルを表示する前に上のフォルダを開くと、代表が別の表紙になったり、表示されなくなったりすることがあります。
> 本のタイルを表示してサムネイルが保存されると、その後でフォルダの一覧を読み直したときに、再びその本が代表に選ばれるようになります。
> キャッシュポリシーが「Off」の場合や、「Auto」で保存されなかった場合は、本のタイルを表示しても、別の代表または代表なしの状態が続くことがあります。

実装用語・版の説明を公開文へ出さない。CHANGELOGは公開担当が扱う。

### 29.11 退けた案と理由

- **P案**: 素材のcodec変更、読取の5入口での本文proof照合、採用直前の照合、親の要求内での子の作り直し、
  PDFの世代ticketの引継ぎが必要となり、独立設計レビューでREVISEが続いた。
  さらに失敗時のBとAへの復帰のために一時選択証拠と認証memoまで必要になった。
  利用者が親の制約を受け入れたので、より小さい保存側失効を採る（2026-11-15）。旧詳細は `9a8a37f4e` 以前の設計履歴と§29.0に残る。
  BodyRotationProof / ReadUnavailable / FolderSelectionEvidence / K2の子生成helper / 認証失敗LRUは今回作らない。
  P案の素材形式としてのv4は撤回し、採用案では旧結果を避けるキーだけのv4/v4fを使う。
- **消す前に退避して新要求を抑止する案**: 抑止/解除の状態が必要。削除完了後に既存退避を使えば足りる。
- **保持時点で現在番号を付ける案**: 古いLoadedを保存し直すと失効を隠す。実際の採用時のstampをcloneする。
- **全回転revision/全context clear**: 無関係な本や通常ページまで失効するため採らない。
- **親の即時更新・非表示の子生成・強制保存**: 制約をなくす代わりに生成/完了ownerや保存policyの変更が要る。利用者の今回の決定では採らない。
- **旧writerの停止・永続dirty・終了待ち・retry**: まれな組合せのために設けず、ログと手動の範囲指定/cache再作成で扱う。

### 29.12 実装・検証の記録（2026-11-16）

- 設計: §29.1のkey-only v4/v4fと、§29.8の単純な保存側失効に対する独立設計レビュー／設計担当の合意を反映した。
- 文書: display-pipeline／ui-responsiveness／virtual-folders、detached計画§11、公開マニュアルの自動切り出しと既知問題を同じ範囲へ更新した。
- 実装: `cover_rotation_cache.rs` に短命workerとApp所有の完了channel／slot番号、`app/cover_rotation_cache.rs` にsource捕捉と完了後退避を置いた。workerは既存catalogのヘッダだけを読み、既存builderで検証した実在キーをtransactionで厳密削除する。memoryのlock中にSQLを実行しない。新たなschema・素材codec・本文proof・K2生成は追加していない。
- 完了は `app/logic_service.rs` で非blockingに受ける。mountedとregistryのAtRest／Retiringの該当要求・CPU待ち・画素のみを除去し、全体epochを進めない。workerは既存のrepaint contextで完了を通知する。`ThumbnailState::Loaded` のstampをcloneのまま保持し、snapshot saved／list-view、Smart、direct-sourceの復元時に照合する。復元後に設置されたmemoryも厳密なslot／source照合で除去する。
- 回帰テスト: `cover_cache_completion_*`（通知前の画素保持、通知後の対象別退避、mounted／AtRest／Retiring、CPU待ち、旧IDの画像／Finalized拒否）、`cover_cache_snapshot_saved_and_list_view_restore_evict_old_material_and_samples`、`smart_folder_cover_cache_restore_does_not_resurrect_stale_auto_aspect`、`cover_cache_direct_reader_split_restores_stale_saved_basis_without_pixels_or_aspect_sample`。worker側の `cover_rotation_*` は厳密キー、表紙除外、PDF／EPUB／変換書庫／ZipDir／画像folder、Collectionのdrive、未表示source、初回pending、復元memoryを検査する。
- 更新互換と親の制約: `auto_material_v4_source_rebuilds_v3_with_preexisting_saved_body_rotation` と `cover_rotation_k2_keeps_parent_until_normal_reread_and_child_save` を追加した。v3の後に本文回転済みでも追加操作なしでv4を生成し、K2は子の削除後の別代表／再起動相当のcold状態と、通常保存後の次回再選択を検査する。
- WIP後の補修: 受付時に保持するsource factsへ採番済みrequest IDを設定した。snapshotの回帰は「子の本→固定一覧→固定解除」という既存の復元手順、名前の回帰は既存の自然順、K2の回帰は空のpin DBの実revisionを使う。v4の期待値と、将来の未知の印を含むmaster互換corpusも更新した。
- 変換書庫の保持素材: UIで別名未解決のsource factsを先に落とさず、workerで同じ論理ソースのZIP別名を共有してから対象を絞る。複数のpin/request選択は残し、ZipDirのprefix別所有も維持する。`cover_cache_unsaved_retained_archive_uses_mounted_reader_alias_before_source_filtering` はDB・memory行なし、逆引きcache DBなしの実worker→AtRest退避を検査し、`cover_rotation_zipdir_shares_outer_source_alias_without_invalidating_sibling_prefix` は別prefix不変を検査する。新しいowner・永続状態は足していない。
- 最終検証（`8e3dc58bd`＋作業ツリー）: `run-heavy.ps1 -Owner "773b impl"` 経由で、重いコマンドを1本ずつ実行した。nextestの `test(/cover_cache|cover_rotation|representative_cache_key|auto_material|folder_auto|folder_thumb|catalog|cover_auto|cover_crop/)` は **612成功・0失敗**（17,375除外、`--test-threads 1`、run ID `542fd1b1-b594-450f-aae5-2912b2e39188`）。初回に失敗した6件の補修確認と、未保存変換書庫の追加回帰も成功した。
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo run -p viewer_context_audit` は成功。`cargo test -p mimageviewer --test ui_snapshot cover_ -- --test-threads=1` は **5成功**（手動／自動範囲画面の100%／200%と環境設定の本の表紙、PNG更新なし）。`cargo fmt`（引数なし）、`python scripts/check_ui_glyphs.py`（危険な文字0件）、`git diff --check` も成功した。
- 検証環境: 子のTEMP/TMPは作業ツリーの `target/tmp`、FFmpeg DLLは `target/debug/deps` とPATHへ配置した。sandboxでTurboJPEGのMSBuildが失敗したため、同じ `turbojpeg-sys 1.2.0`／静的CRT設定の既存ビルド成果物を `target/native` へ実ファイルでコピーし、検証用プロセスのexplicit指定だけで再利用した。製品の依存設定・Cargo.lockは変更していない。
- 独立実装レビューは設計担当へ引き継ぐ。利用者の指定によりlib全件・確認用build・製品起動・実データ評価・commitは実行しない。
- 独立実装レビュー修正: 削除中に受付された要求も、完了通知の実在行由来の `affected_slots` で照合し、該当request IDとCPU待ちを既存のowner別退避で除く。leafが未解決の要求は削除した厳密キーでも照合し、別の過去pinによる表紙だけの変更を巻き込まない。`cover_cache_real_row_completion_cancels_requests_admitted_during_deletion` は実workerの「affectedあり・pending空」の通知、明示leaf／未解決要求、CPU待ち／通信中、旧画像／Finalizedの遅着を検査する。
- ZipDirの保存先の論理identityと実効の読取scopeを分離した。workerでpinを解決してから変更ページ／代表ページの所属を照合し、`A/`から`B/`への固定でも保存キーとstampは`A/`のまま失効させる。`cover_cache_zipdir_external_pin_invalidates_writer_rows_and_retained_loaded` は実App requestのprefix、保存行あり／なし、受付済み要求あり／なし、保持Loadedの復元、表紙だけ／元のprefix／兄弟の不変を検査する。codec・schema・公開文書・viewport経路は変更しない。
- レビュー修正の検証（`c482014b6`＋作業ツリー）: `run-heavy.ps1 -Owner "773b r1"` 経由で直列実行。追加の主要2件は2成功、その後 `test(/cover_cache|cover_rotation|thumbnail_requests/) --test-threads 1` は **72成功・0失敗**（run ID `26d00105-4e1f-4e60-95d9-31c99d18cb01`）。`cover_cache_historical_pin_completion_preserves_unresolved_cover_only_request` と、affectedだけの通知に強めたAtRest／Retiringの既存テストも含む。`cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt`（引数なし）、`git diff --check` は成功。UIの見た目・文字列・snapshotの変更はない。
- 同じ直列検証で `cargo run -p viewer_context_audit` も成功した（exit 0）。detached記録は§11の既存項目に追記し、owner／viewportの範囲を拡張していない。

- 再レビュー対応（2026-11-16）: P2-aは書庫単位の保守的失効へ簡素化し、前回のread_book・実効prefixによる所属判定と、変更ページから包含prefixだけを作る処理を削除した。writerのキーは不変、runtime slotを外側書庫＋代表ページへそろえる。`cover_rotation_archive_inventory_deletes_unheld_external_zipdir_pins_after_restart` はpin DB・対象tileのhint無しでZIP／変換書庫の外部prefix固定、両binding、区切り文字を含む名前、手動の保持と受動stampを検査する。
- P2-bは通知を `InvalidatedRow { slot, catalog, key }` にまとめた。`cover_cache_correlated_deleted_rows_preserve_same_name_cover_only_request_in_other_catalog` は実workerの複数catalog batchで、表紙だけのAの未解決要求／CPU待ちを保ち、本文のBだけを取消す。既存ZipDirの別prefix不変テストは、今回受け入れた同書庫内の失効へ期待値を変更する。無関係な別書庫/contextは引き続き不変。
- 再レビュー修正の検証（`1bdf4736d`＋作業ツリー）: run-heavy経由、子TEMP/TMPはtarget/tmp、build jobs 2／test threads 1で直列実行した。初回の3失敗は、明示leafのaffected-only通知の取消、slotのパス正規化、新しい書庫単位方針の期待値を補修した。最終の `test(/cover_cache|cover_rotation|thumbnail_requests/)` は **74成功・0失敗**（run ID `65380bb3-3cfb-4555-b908-994b5ec6f154`）。`cargo check -p mimageviewer --bin mimageviewer-core`、`cargo run -p viewer_context_audit`、`cargo fmt`（引数なし）、`git diff --check` も成功。UI文字列・見た目・snapshotは変更していない。マニュアルには同じ書庫内の本も判定し直すことを追記した。

## 30. 直接読む RAR の自動切り出しと手動の範囲指定 (§1.778、2026-11-17 設計)

- 要件の正本: [next-release-backlog.md §1.778](next-release-backlog.md) (5ch >>778・793)。ブランチ `feat-778-rar-cover-crop`。
- 既定の割り切り: まれな失敗 (DB の読み書き失敗・処理途中の強制終了など) はログ + 通知 (CLAUDE.md)。

### 30.1 コードで確かめた前提 (master `96ed160a0`)

- 直接読む RAR / CBR のタイルは `GridItem::ConvertibleArchive { format: Rar }`。読み取り元は `converted_archive_cache_paths` の
  `Direct(解決済みの RAR、分割なら先頭巻)` / `CachedZip` / `Pending` / `Unavailable` で、変換済み ZIP があれば `CachedZip` が優先される。
  ソリッド・入れ子・暗号化の RAR は `Unavailable` (変換して開く)。
- 基本キーは読み取り元によらず `archivethumb:rar:{名前}` (`convertible_archive_cache_base_key`)、stamp は元の RAR の mtime/size。
  `Pending` は同じキーの `CacheOnly` で読む。自動 ON では `make_load_request_with_auto_crop` が ConvertibleArchive にも自動の印を付けるので、
  **v5.3.0 は直接読む RAR を `archivethumb:rar:X.rar|autocrop:v4:{rtl|ltr}` のキーで `AutoMaterial::NotAutomatic` (判定しない素材) として保存している**。
  worker は `automatic_container_source` が `AutoCoverSkip::DirectRar` を返して判定に進まないため。CachedZip の RAR は同じキーで判定済み。
- worker は heavy I/O キュー ("io{n}"、1〜2 本) で、表紙は `rar_loader::read_first_image_bytes` (ヘッダ順の最初の画像を丸ごと) で読む。
  cancel はプール共通の `Arc<AtomicBool>`。`rar_loader` は自前パッチの `unrar` 0.5.8。一覧 (`enumerate_image_entries_detailed_with_cancel`) は
  entry ごとに cancel を確かめ、重複名を `page (2).png` 形式にした名前と展開後の大きさ (`unpacked_size`) を返す。entry の読み取り
  (`read_entry_bytes`) は丸ごとで cancel が無い。部分読みは無い。
- **手動の範囲指定は、直接読む RAR でもすでに通る**: メニュー (`cover_crop_menu_state`) は `Direct` を有効にし、準備 worker
  (`prepare_cover_inner`) は `read_entry_bytes` で表紙を読む。止まるのは自動の判定だけ (`AutoStop::Source(DirectRar)`「変換前の RAR は自動の対象外です」)。
  手動の範囲を解除した後に自動へ戻るかの `auto_eligible_after_clear` も RAR では false。マニュアル (grid.html) の対象の列挙は「変換済みの書庫」だけを書いている。
- **キャンセル・一時的な読み取り失敗が判定済みと混ざる (既存の不具合、全形式)**:
  - 判定用のデコードが cancel で止まると `reason="analysis-decode-failed"` になり、`NotAutomatic` のまま catalog に保存される。
  - ZIP の標本の読み取り (`read_cover_auto_page_prefixes(...).unwrap_or_default()`) は I/O エラーを空の一覧にし、`TooFew` →
    全方向 `PageAspectMissing` の `Automatic([None; 4])` (判定済みの見送り) として保存される。
  - 範囲画面は標本が `Err` なら判定せず `Stopped(PageAspect)` にする (worker と扱いが違う)。

### 30.2 状態を減らす検討 (CLAUDE.md「設計の簡素化」)

- **判定は今の thumbnail worker の 1 つの仕事の中で行う** (採用)。表紙を読んだ同じ heavy I/O の仕事で標本を読み、判定してから 1 回だけ送る。
  「先に切らない画を出し、裏の別の仕事で判定して差し替える」2 段構えにしない。差し替えの見える切り替わり、判定待ちの中間状態、
  その間の cancel・退避・別 context の扱いが要らない。UI スレッドは何もしない (バックログの「バックグラウンドで」はこの worker を指す)。
  代償は、自動 ON で RAR の本を初めて表示するときにタイルが出るまでの時間が延びること (§30.7 で測る)。
- **読み取り元 (Direct / CachedZip) を key に入れない** (採用、r1 指摘 2 を受けて制約を明記)。キーは論理の本 (元の RAR) を表し、
  判定中 (`Pending`) の `CacheOnly` も同じキーで読む (読み取り元をキーに入れると、判定中に保存済みの表紙を出せなくなり、出荷済みの表示が遅くなる)。
  同じ本でも読み取り元によって判定の材料が少し違う: (a) RAR は丸ごと展開するので上限 (§30.4) で読まない枠が出る場合があるが、ZIP は先頭 256 KiB を読む、
  (b) 本文の保存回転のページキーは Direct では元の RAR、CachedZip では変換後の ZIP のパス (§28.3 の既存の扱い)。
  どちらで作った素材も「その本の表紙の全体の画素 + その時点の材料で判定した 4 方向の範囲」として正しく、表示が壊れることは無い。
  **受け入れる制約**: Direct と CachedZip が入れ替わるのは、利用者が明示的に変換したとき・変換キャッシュを消したときだけで、そのとき前の読み取り元で
  作った素材がそのまま使われ、今の読み取り元で範囲画面を開いた判定と食い違うことがある (範囲画面は開くたびに今の材料で判定する。§28.3 と同じ考え方)。
  本文を回転すれば §29 の失効で作り直される。CachedZip の ZIP の標本に RAR と同じ上限を掛けて揃える案は、出荷済みの判定を弱めるので採らない。
- **部分読みを作らない** (バックログの方針)。初回の費用が大きいと分かった場合の後続案とする (unrar の展開を先頭で打ち切る等)。
- **手動の範囲指定に新しい経路を作らない** (採用)。既存の経路が通るので、テストで固定し、文書を直すだけにする。
- **キャンセル・読み取り失敗は「保存しない」で表す** (採用、§30.5)。失敗の記録・再試行の予定・回数の上限は持たない。
  次に同じタイルを読むとき (退避後・次回起動) に自然に判定し直す。

### 30.3 キー: RAR の本だけ自動の印を `v4r` にする

- 次の 2 種類の代表キーだけ `|autocrop:v4r:{rtl|ltr}` を付ける (置き場所 = 最初の `#pin:` の直前、固定なしは末尾、と正規化は v4 と同じ。固定ありも同じ)。
  1. **`archivethumb:rar:` で始まる本のタイル** (CBR も同じ綴り。読み取り元が Direct / CachedZip / Pending のどれでも同じキー)。
  2. **直接読む RAR の中のフォルダ (ZipDir、`zipdir:{prefix}`)**。直接読む RAR を開くと書庫のツリーになり、中のフォルダのタイルは
     `LoadRequest.path` = その RAR、`zipdir:` キーで要求される (app.rs の ZipDir 分岐)。v5.3.0 はこれも v4 の `NotAutomatic` で保存している (r1 指摘 1)。
     ZipDir の印は**要求の生成時に、外側の書庫のパスの拡張子 (`rar_loader::is_rar_path`、I/O なし)** で決める。キー文字列だけでは RAR か分からないため、
     印を選ぶ関数に「外側が RAR か」を渡す (`set_auto_cover_crop` が要求の path を見る)。ZIP 一覧の存続集合 (`tree.all_cache_keys()` に自動の印を付けたもの、
     ピンなし ZipDir を含む) も同じ関数で作る。変換済み ZIP として開いた RAR の中の ZipDir は外側が ZIP なので v4 のまま。
- 7z・LZH の書庫、ZIP、PDF、ZIP の中の ZipDir は v4 のまま (作り直さない)。
- 理由: v5.3.0 が直接読む RAR を v4 のキーで `NotAutomatic` として保存しているので、同じキーのままでは有効な行として当たり続け、判定されない。
  RAR の本だけキーを変えれば、旧行は読まれず、既存の存続キー (`delete_missing`) で消える。素材の codec・catalog の schema は変えない (key-only、§29 の v4 と同じ型の更新)。
- 費用: 自動 ON の利用者の RAR の本 (CachedZip も含む) のタイルを 1 回ずつ作り直す。ZIP・PDF などは作り直さない。
- 正規化: `archivethumb:rar:` 以外のキーに付いた `v4r` は v4f と同じく既知の印として扱わず文字列のままにする。`archivethumb:rar:` の v4 は旧印として外す。
  v4 の印を名前で探している箇所 (`representative_auto_crop_key_parts`、catalog の `decode_auto_material` の「現行キーの NULL・不正値は読み損ね」、
  `cover_rotation_cache` の削除対象の判定、`rating_view`、評価器の `algorithm`) をすべて v4r にも対応させる。実装時に `autocrop:v4` を全数 grep する。
- **K2 には影響しない** (r1 指摘 6 で訂正)。フォルダの自動代表が子の本の行を流用する候補は ZIP / CBZ / PDF / EPUB だけで、RAR / CBR は
  候補の列挙にも `archive_tile_key` にも入らない (thumb_loader.rs の K2 の候補列挙)。今回 RAR を K2 の候補に加えることはしない。
  旧 v4 の RAR の行への互換読み取りも作らない。`legacy_auto_child_keys` (v1 互換) は v4 のキーだけが対象のまま。

### 30.4 RAR の標本の読み方

- **選び方を ZIP と共有する**: `(名前, 展開後の大きさ)` の一覧から、無視するファイル・prefix 外・入れ子・画像でないものを除き、`FileNameSortKey` で並べ、
  leaf の位置から既存の `sample_indices` (最大 8 枚、leaf の次から。leaf が最後なら前) で選ぶ helper を 1 つ置く。ZIP の
  `read_cover_auto_page_prefixes` もこの helper を使う (ZIP の挙動は変えない)。RAR の一覧の名前は重複変換後の名前で、leaf も
  worker が読んだのと同じ名前で照合する。
- **上限** (定数、§30.9 で利用者に確認):
  - 1 枚: 展開後の大きさが 32 MiB を超える entry は読まず、その枠は「読めない」(既存の `None` の枠) にする。
  - 合計: 選んだ順に展開後の大きさを足し、64 MiB を超える entry からは読まない (同じく `None` の枠)。読む集合は**展開する前に一覧の大きさだけで決める**ので、結果は毎回同じ。
  - `None` の枠が多ければ既存の合議 (読めたページが 3 以上 等) で見送りになる。これは判定済みの見送りとして保存する。
- **展開**: RAR を 1 回だけ開き、ヘッダ順に進めて選んだ名前だけを展開し、それ以外は読み飛ばす (非ソリッドなので読み飛ばしは展開しない)。
  entry の前ごとに cancel を確かめる。名前の照合は `read_entry_bytes` と同じ重複変換。表紙 (leaf) は読み直さない。
- 寸法は ZIP と同じ `probe_image_dims_from_bytes` (EXIF の向きを含む) で取り、本文の保存回転は viewer のページキー
  `zip_entry_key(解決済みの RAR, entry 名)` で引く (§28.3 の rotation-aware sampler に RAR の読み方を足す)。以降の比率の合議・判定は変えない。
- 置き場所: `cover_auto_sample` に RAR の sampler を足し、展開は `rar_loader` に「選んだ名前の集合を 1 回の走査で読む (cancel・合計の上限つき)」関数を置く。
  `read_cover_auto_page_prefixes` と `sample_zip_pages_with_rotations` の先頭の「RAR なら空」は外し、worker の分岐 (`AutoCoverInput::Container`) で
  RAR の sampler を呼ぶ。ZipDir の要求では今と同じく `zip_dir_prefix` の中だけで選ぶ。worker・範囲画面・評価器が同じ sampler を使う。
  評価器 BOOK_LIST は今 `BookKind::Archive` を受け付けないので、RAR / CBR の入力を受け付け、共有の sampler に届くようにする (r1 指摘 7)。
- `AutoCoverSkip::DirectRar` と範囲画面の文言「変換前の RAR は自動の対象外です」、評価器の `direct-rar` を削除する
  (直接読む RAR は非ソリッド・入れ子なし・暗号化なししか無いので、ほかの停止理由は不要)。

### 30.5 判定済みと中断を分ける (全形式)

- 標本の読み取りと判定用のデコードの結果を、型で 3 つに分ける:
  1. **判定した** (`Automatic(4 方向)`、全方向見送りを含む)。ページの画像が壊れている・寸法が取れない・大きさの上限で読まない枠は、
     その本の中身の性質なので `None` の枠として判定に含める。
  2. **判定しない** (`NotAutomatic`): 自動の対象でない (動画の leaf 等、今と同じ)。判定用のデコードが画像の中身の理由で失敗した場合も今と同じくここ。
  3. **中断** (`Cancelled` / `ReadFailed`): cancel、書庫・PDF を開けない・読み取りの I/O エラー、PDF ワーカーの失敗。
- 中断の扱い:
  - `Cancelled`: worker の既存の cancel の扱いに乗せ、catalog に保存しない。
  - `ReadFailed`: 切らない画 (`NotAutomatic`) を今回の表示用に送るが、**catalog には保存しない**。ログと perf event に理由を残す。
    次にそのタイルを読むとき (退避後・次回起動) に判定し直す。再試行の予定や失敗の記録は持たない (まれな失敗の割り切り)。
  - 保存しない経路は、既存の「保存しない」完了 (`SourcePixels` → `Finalized`) を使い、新しい完了の形や待ちの状態を作らない。
- **判定した素材の保存判断に、標本の読み取りと判定の時間を含める** (r1 指摘 5)。今の保存判断 (`CacheDecision::should_cache`) は
  `decode_ms + display_ms` だけを見るので、表紙は軽いが本文の展開が重い RAR の本が保存されず、退避・再表示のたびに展開し直す。
  自動の印のある要求で判定した (`Automatic`) 場合は、比べる時間に「標本の読み取り + 判定用デコード + 判定」の時間を足す。閾値・常に保存する形式の規則は変えない。
  軽くて保存しなかった本は、次に表示したときに同じく軽く判定し直す (今の方針どおり)。
- **分類は下位の読み取り・デコードの境界で行い、型で上へ運ぶ** (r1 指摘 4)。今は ZIP の各 entry の読み取り (`read_prefix_by_name(...).ok()`)、
  画像フォルダの open / read (`.ok()`)、外側の `unwrap_or_default()`、RAR の `unrar_io` (全エラーを `InvalidData` へ)、判定用デコード (`String`) で情報が消えている。
  標本 1 枚の結果を `Read(bytes) / Unreadable(データの理由) / ReadFailed / Cancelled` の enum にし、sampler は `ReadFailed` / `Cancelled` が 1 枚でもあれば
  全体を中断として返す (読めない枠と混ぜない)。判定用デコードの戻り値も `Cancelled / ReadFailed / Decode(データ)` の enum にする。
- 分類の規則 (1 か所の関数に置く):
  - `Cancelled`: cancel の検査で止めた。
  - `ReadFailed`: 元のファイル・書庫を開けない、読み取りの OS エラー (std の `io::Error` で `InvalidData` / `UnexpectedEof` / `Unsupported` 以外)、
    unrar の開く・読む・作るのエラー (`EOpen` / `ERead` / `ECreate` 等、データでないもの)、PDF ワーカーの失敗・再起動・時間切れ。
  - データ: 画像の形式・寸法が読めない、CRC・圧縮データの破損 (unrar の `BadData` / `BadArchive` 等、zip の `InvalidArchive`)、ZIP の `InvalidData`。
    → 標本ならその枠は `None`、表紙の判定用デコードなら今と同じ `NotAutomatic`。
  - leaf が一覧に無い・候補が足りないのは今と同じ判定済みの見送り。
- PDF の `PageSizes` の失敗は `ReadFailed`。ただし PDF のデコードの完了済みの結果を cancel で捨てない既存の区別 (`HarvestOnCancel`) は保つ。
- 範囲画面も同じ型を使う: 中断は「本文のページを読めませんでした」で止める (cancel は画面を閉じた場合だけなので表示しない)。判定した結果は worker と同じ。
- 出荷済みの挙動の変化: ZIP・PDF でキャンセル・一時的な失敗のときに「切らない」が固定されなくなるだけで、通常の使い方が不便になる変化は無い。

### 30.6 失効

- RAR の更新: キーの stamp (元の RAR の mtime/size) が外れて作り直す (今と同じ)。キャッシュの消去でも作り直す。
- 本文の保存回転 (§29) — **出荷済みの不一致を直す** (r1 指摘 3): writer のキーは `archivethumb:rar:{名前}` (形式の綴りを含む) だが、
  `cover_rotation_cache` の `row_belongs_to_with_slot` は `archivethumb:{名前}` 相当と比べており、変換済み (CachedZip) の RAR でも本文を回転した後に
  古い行が残る。既存のテストも形式の綴りの無いキーで書かれていて検出しない。直し方: 照合に使うキーを **本番の builder
  (`convertible_archive_cache_base_key` と自動の印の付与) から作る**。名前だけ・フルパス (`use_full_path`)・固定あり・v4r / v4 の各綴りを、
  本番の builder で作った行で検査する。未保存の要求の代表解決で、`Archive` に別名が無い Direct の RAR が早く抜ける経路も、Direct の RAR を
  元の RAR のパスで照合するように直す。直接読む RAR のページキーは `{解決済みの RAR}::entry` で、失効の単位は外側の書庫 (= その RAR) (§29 と同じ)。
  中の ZipDir (v4r) の行も同じ書庫単位で消えることを検査する。新しい仕組みは作らない。

### 30.7 計測 (perf event)

- `thumb/auto_cover_sample`: `format` (rar / zip / pdf / folder)、`selected` (選んだ枚数)、`read` (読めた枚数)、`skipped_limit` (上限で読まなかった枚数)、
  `bytes` (展開した合計)、`list_ms`・`read_ms`、`outcome` (judged / cancelled / read_failed)。
- 既存の `thumb/auto_cover_crop` の `reason` に `cancelled` / `read-failed` を足し、`sample_ms` を足す。
- 初回は `auto_cover_sample` の ms と、同じ要求のタイルの生成時間で費用を見る。2 回目以降 (キャッシュ利用) は `auto_cover_sample` が出ないこと、
  キャッシュ読みの時間が RAR 以外と同じことで見る。一覧の応答性は既存の可視タイルの first / all-ready の計測 (`analyze_perf.py nav`) で、
  自動 ON の RAR のフォルダの初回と 2 回目を比べる。`scripts/analyze_perf.py` に `auto_cover_sample` の集計が要るかは実装時に判断する。

### 30.8 テスト

- テスト用の RAR は合成データで作り、`testdata/archives/cover-auto-rar/` にコミットする (WinRAR の `Rar.exe` が要るので実行時には作らない)。
  生成手順 (合成の画像は既存の Rust の合成表紙・本文の生成器を使う) を README と生成用のコード (`#[ignore]` のテストまたは script) に残す。
  利用者の書庫・評価用の実データは使わない。
  - 切れる本: つながったカバー (折り返し・表表紙・背・裏表紙) + 縦長の本文 8 枚。
  - 見送る本: 表紙だけ / 本文が 2 枚しか無い。
  - 重複名の entry を含む本、上限の境界 (テストでは上限を引数で小さくする)。
- sampler: 選んだ集合が ZIP と同じ規則、1 回の走査、合計・1 枚の上限、cancel で `Cancelled`、開けない書庫で `ReadFailed`、本文の保存回転のキー。
- worker: 自動 ON の直接読む RAR で `v4r` のキーに `Automatic` を保存し、2 回目はキャッシュから判定せずに読む (`auto_cover_sample` が呼ばれない)。
  見送りも保存して 2 回目に読み直さない。cancel・読み取り失敗で保存しない。v5.3.0 の v4 の `NotAutomatic` 行があっても v4r で判定する。
  CachedZip と Direct で同じキーの素材を共有する。ZIP の I/O 失敗・判定用デコードの cancel が保存されない (既存不具合の回帰)。
- key: `archivethumb:rar:` と直接読む RAR の中の ZipDir だけ v4r、7z/ZIP/PDF・ZIP の中の ZipDir は v4 の golden、固定あり・種類設定ありの綴り、
  正規化の往復、存続キー (ピンなし ZipDir を含む)、`v4r` を名前で探す各所。直接読む RAR の中の ZipDir の旧 v4 の `NotAutomatic` 行があっても判定する。
- 保存判断: 表紙は軽く本文の展開だけが重い RAR の本 (テストでは計測時間を注入) が保存され、2 回目に展開しない。
- 中断の分類: ZIP の entry の読み取り・画像フォルダの open の OS エラー、unrar の読み取りエラーが `ReadFailed` になり保存されない。
  壊れた画像・CRC エラーは `None` の枠として判定に含まれ保存される。
- 評価器: BOOK_LIST が RAR / CBR を受け付け、共有の sampler で判定する。
- 範囲画面: 直接読む RAR で自動の判定結果が初期の枠に出る、手動の範囲を OK → 固定キーで切った画が出る、解除後に自動へ戻る旨の表示、中断の文言。
- §29: 本番の builder で作ったキー (名前だけ・フルパス・固定あり、v4r) の行が、直接読む RAR の本文の回転で消える。変換済み RAR (v4r) も消える。
  直接読む RAR の中の ZipDir の行も消える。未保存の要求も取り消される。

### 30.9 利用者に確認すること (推奨案つき)

- 上限の値: **推奨 1 枚 32 MiB・合計 64 MiB**。ふつうの漫画の 1 ページは 0.5〜5 MB 程度なので、ほとんどの本は 8 枚とも読む。大きな PNG の本で合計に当たると
  読む枚数が減り、3 枚未満なら切らない (今の表示のまま)。
- 初回の費用: 自動 ON で RAR の本を初めて表示するとき、本ごとに本文を最大 8 枚 (64 MiB まで) 展開するので、その本のタイルが出るまでの時間が延び、
  同じ heavy I/O の列の後ろのタイルも遅れる。2 回目以降はキャッシュから出す。自動 OFF (既定) では何も変わらない。
- 更新直後: 自動 ON の利用者は RAR の本と、直接読む RAR の中のフォルダのタイルを 1 回ずつ作り直す (ZIP・PDF などは作り直さない)。
- 読み取り元の入れ替わり (§30.2): RAR を ZIP に変換した・変換キャッシュを消したときは、前の読み取り元で作った判定がそのまま使われる。

### 30.10 退けた案

- **全形式の印を v5 に上げる**: ZIP・PDF の本まで全部作り直しになり、K2 の互換読み取りも要る。
- **読むときに「RAR の v4 で NotAutomatic の行」を読み損ねにする**: 動画が leaf の RAR など正しい NotAutomatic の行まで毎回作り直す。
- **素材の codec に新しい値を足して旧行と見分ける**: リリース済みの列の意味を変えるので、旧版との行き来の扱いが要る。
- **表示を先に出して裏で判定し差し替える**: §30.2。
- **失敗を記録して一定時間後に再試行**: まれな失敗の割り切りに反する。

### 30.11 設計レビューの記録

- r1 (Sol xhigh、2026-11-18、`6d9a43dcb` に対して、REVISE、P2 5・P3 2): 指摘はすべてコードで確かめて反映した。
  1 = 直接読む RAR の中の ZipDir も v4 の `NotAutomatic` で保存済み → §30.3 で v4r の対象に加えた。
  2 = Direct / CachedZip で判定の材料が違う → キーは共有のまま、受け入れる制約として §30.2 に明記 (判定中の表示を遅らせないため)。
  3 = §29 の失効のキー照合が `archivethumb:rar:` の綴りと合わない (出荷済みの不一致) → §30.6 で本番 builder のキーで照合するよう直す。
  4 = 下位の API でエラーの種類が消えている → §30.5 で分類を下位の境界に置き、型で運ぶ。
  5 = 保存判断が標本の費用を含まない → §30.5 で判定した素材の保存判断に含める。
  6 = K2 は RAR を候補にしていない → §30.3 の説明を訂正。
  7 = BOOK_LIST が RAR を受け付けない → §30.4 に加えた。
