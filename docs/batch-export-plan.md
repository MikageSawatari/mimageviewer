# 一括エクスポート (Ctrl+E) の拡張 設計 (§1.362)

- 状態: **設計中 (実装前)**。独立設計レビューの結果は §12 に記録する (未記録の間はレビュー未了)。
- 範囲の正本: [backlog-on-hold.md §1.362](backlog-on-hold.md) (利用者決定 2026-10-10、着手は利用者決定 2026-11-24)。
- 調査: `internal/batch-convert-2026-10/report.md` (2026-10-10 時点。§1 で今のコードと照合した)。
- 関連: [preset-and-adjustment.md](preset-and-adjustment.md) (焼き込みの段・合成順)、
  [ui-responsiveness.md §4](ui-responsiveness.md)、[coding-conventions.md「ダイアログ」](coding-conventions.md)、
  [settings-sqlite-migration.md §6.4 / §10.5](settings-sqlite-migration.md) (旧版へ戻したときの扱い)、
  [settings-export-import-plan.md](settings-export-import-plan.md) (設定の持ち運びの分類)。

対象はグリッド選択の一括エクスポートだけ。フルスクリーン 1 枚の Ctrl+E (`export_dialog.rs` の
`ExportDialogState` / `run_export`)、製本、外部ツール受け渡し、クリップボードは変えない。

## 1. 今のコード (2026-11-24 master `361411c72`) と調査報告の照合

調査報告 Part 2.1 の記述は今のコードと一致する。行番号は動いているので、ここでは関数名で示す。

| 観点 | 今のコード |
| --- | --- |
| 入口 | `App::open_export_batch_dialog` (`src/ui_dialogs/export_batch.rs`)。対象は `grid_batch_export_targets` (画像・ZIP 内画像・PDF ページ、スタックはメンバーへ展開)。 |
| 準備 | `advance_export_batch_preparation` が UI スレッドで 1 件ずつ `batch_export_item_for_target` → `book_baked_edit_snapshot` を読む (6ms 予算、1 件は必ず進める)。§1.177 は未解決。 |
| 形式 | `crate::capture::CaptureFormat` (PNG / JPEG 95・85・75)。この型は**キャプチャ設定と共有**している (`CaptureFormat::ALL` のコメント)。 |
| サイズ | `crate::export_dialog::ExportScale` (そのまま / 1/2 / 1/4 / 長辺 N px 以下・縮小のみ)。この型は**単ページの `export_default_scale` とも共有**し、設定へ serde で保存している。縮小は `scale_export_pixels` (Lanczos3 固定)。 |
| 書き出し | `export_batch::run_batch_export` → `books::write_composited_page` → `write_composited_color_image` (`include_metadata: false` 固定、`edits.format` で JPEG/PNG を選ぶ)。`write_composited_page` は**製本の `write_source` (`BookPageSource::Composited`) も使う**。 |
| 完了処理 | `poll_export_pending` は成功が 1 件なら Explorer で示し、複数なら件数を通知する。一覧の更新は「成功は全部同じフォルダ」を前提に、最初の成功パスだけを `note_exported_file_for_folder_refresh` へ渡す。 |
| 名前 | `<filename>` `<dirname>` `<num>` (4 桁)。既存・同一実行内の衝突は `_1`〜`_9999`。上書きなし (`save_image_with_metadata` も `create_new`)。 |
| 取消 | 項目の間だけ。`decode_composite_source` / `compose_book_page` は呼ぶたびに新しい未使用の `AtomicBool` を作るので、1 件の途中 (AI・消しゴム・補正レイヤー) は止まらない。 |
| 記憶 | `export_batch_directory` / `export_batch_template` / `export_batch_format` / `export_batch_scale` (いずれもリリース済み)。 |
| ZIP/PDF の `<filename>` | `capture_basename_for_idx` が `<zip名>_<entry名>` / `<pdf名>_pNNNN` を作る (書庫名を含む)。 |

報告に無かった事実:

- `save_with_metadata` の転記は形式ごとに違う。JPEG は元の APP1 (EXIF・XMP) を**そのまま**挿入し、
  EXIF の Orientation だけ 1 にする (IFD1 = 元画像の縮小サムネイルも残る)。PNG はテキストチャンク
  (tEXt / iTXt / zTXt) だけで、`eXIf` と `iCCP` は転記しない。WebP は `ICCP`・`EXIF`・`XMP ` を
  そのまま転記し、Orientation も直さない。JPEG の ICC (APP2) は転記しない。§3.6 で扱う。
- mIV のデコードは ICC による色変換をしない (コードに ICC 変換が無い)。ただしデコード結果は常に
  RGBA へ変換する (`dynamic_image_to_color_image`) ので、グレースケールや CMYK の元画像の ICC は
  出力の画素形式と合わない。
- WebP の書き出しは `webp 0.3` の `Encoder::encode` / `encode_lossless` で、内部のエラーを
  `unwrap()` する (libwebp の上限 16383px を超えると worker が panic する)。

## 2. 段の分け方

3 段に分け、**A → B → C の順**に実装・レビューする。各段は単独でテストが緑になり、master へ入れられる。

| 段 | 内容 | 主な変更 |
| --- | --- | --- |
| A 出力の基本 | 形式 (WebP・JPEG 任意品質・元と同じ形式)、リサイズ方式、出力先 (元と同じフォルダ・構造の維持)、上書きの扱い、元の情報と更新日時、置換子、1 件の途中での取消、設定の移行、ダイアログの組み替え | `export_batch.rs` に新しい選択肢の型と worker、`ui_dialogs/export_batch.rs`、`settings.rs`、`fast_resize.rs`、`save_with_metadata.rs` |
| B 透かし | 文字・画像の透かし (位置・大きさ・不透明度) | `export_batch` に透かし段、ダイアログに欄 |
| C プリセット | 名前付きで保存・呼び出し・削除 | `settings.rs` に一覧、ダイアログ上部の行 |

プリセットを最後にする理由: プリセットは A・B の選択肢一式をそのまま保存するので、
選択肢の型が固まってから作るほうが一度で済む。選択肢の型は最初から 1 つの構造体
(`BatchExportOptions`) にまとめ、B は欄を足すだけにする (§5)。

リリースの単位 (A〜C をまとめて 1 版で出すか、段ごとに出すか) は利用者の判断 (§13 Q1)。
どちらでも永続形式は同じ (`#[serde(default)]` の欄追加だけ) なので、設計は変わらない。

## 3. 選択肢 (`BatchExportOptions`)

ダイアログの値はすべてこの 1 つの構造体に入る。worker へは開始時に clone を 1 回渡すだけで、
実行中に UI から変えない。プリセット (§8) もこの構造体をそのまま保存する。

### 3.1 形式

```rust
enum BatchFormatKind { Png, Jpeg, Webp, SameAsSource }
struct BatchFormat {
    kind: BatchFormatKind,
    jpeg_quality: u8,        // 1..=100、既定 95
    webp_quality: u8,        // 1..=100、既定 90 (単ページの WebP と同じ)
    webp_lossless: bool,     // 既定 false
    same_fallback: SameFallback, // Jpeg / Png。既定 Jpeg
}
```

- 品質はダイアログの数値欄 (DragValue) で任意に指定する。各形式の品質は独立に覚える
  (形式を切り替えても他方の値を失わない)。
- **元と同じ形式**: 元の拡張子で決める (I/O なし)。`jpg`/`jpeg`/`jfif` → JPEG、`png` → PNG、
  `webp` → WebP。それ以外 (GIF・BMP・TIFF・HEIC・AVIF・JXL・RAW・PSD など) と PDF ページは
  `same_fallback` (JPEG か PNG) にする。ZIP 内画像はエントリ名の拡張子で決める。
  品質は上の `jpeg_quality` / `webp_quality` を使う。
- JPEG のサブサンプリング・progressive、PNG の圧縮レベル、AVIF/JXL/TIFF は範囲外 (§1.362)。
- 透過: JPEG は今と同じく `edits.jpeg_matte` で背景を合成する。PNG・WebP はアルファを保つ。
- **形式ごとの寸法の上限**: エンコードの前に出力寸法を確かめ、WebP は各辺 16383px、JPEG は各辺 65535px を
  超えたらその項目を「この形式で書ける大きさを超えています (W×H)」の失敗にして次へ進む
  (リサイズの指定と保存済みの AI 拡大の組み合わせで、通常の操作でも起こり得るため)。
  WebP のエンコードは `Result` を返す `encode_simple(false, quality)` / `encode_simple(true, 75.0)` へ替える
  (`encode` / `encode_lossless` はこれを `unwrap()` しているだけなので、同じ入力では同じバイト列になり、
  単ページの出力は変わらない)。
- 形式の決定は **worker の書き出しだけが持つ**。`BakedEditSnapshot.format` は製本・外部ツール用に
  残すが、一括の新しい書き出し関数は読まない (同じ意味を 2 か所に持たない)。
- `CaptureFormat` と `ExportFormat` は変えない (キャプチャ設定・単ページと共有のため)。

### 3.2 リサイズ

```rust
enum BatchResizeMode {
    None,                 // そのまま
    Percent(u32),         // 1..=400
    LongEdge(u32),        // 長辺
    ShortEdge(u32),       // 短辺
    Width(u32),           // 幅 (高さは比率から)
    Height(u32),          // 高さ (幅は比率から)
    Fit { width: u32, height: u32 }, // 枠に収める (縦横比を保って枠の内側)
}
struct BatchResize { mode: BatchResizeMode, allow_upscale: bool, filter: ResizeFilter }
enum ResizeFilter { Lanczos3, CatmullRom, Bilinear, Nearest } // 既定 Lanczos3
```

- px 指定の範囲は今の長辺指定と同じ 256〜16384 (`ExportScale::LONG_EDGE_MIN/MAX`)。Fit の各辺も同じ。
- **拡大の可否は 1 つの規則で決める**: 目標寸法を出し、元より大きくなる (幅か高さのどちらかが元を超える)
  かつ `allow_upscale == false` なら**リサイズしない** (元の寸法のまま)。`Percent(200)` も
  `allow_upscale` が OFF なら変わらない。既定は OFF (今の長辺指定と同じ縮小のみ)。
- 1/2・1/4 は `Percent(50)` / `Percent(25)` で表す。今の `ExportScale::scaled_by_factor` と
  **同じ式** (`(w as f32 * factor).round().max(1)`、factor = p / 100) で寸法を出すので、
  1/2・1/4・長辺指定の出力寸法は今と同じになる (§10 のテストで固定する)。
- フィルタは `fast_image_resize` の `FilterType::{Lanczos3, CatmullRom, Bilinear}` と
  `ResizeAlg::Nearest`。`crate::fast_resize::Quality` に `CatmullRom` / `Nearest` を足し、
  既存の `Bilinear` / `Lanczos3` の意味は変えない。UI の表示名は「Lanczos (既定)」「バイキュービック」
  「バイリニア」「最近傍 (ドット絵向け)」。
- リサイズは**保存済み編集の合成の後** (AI 拡大・切り取り・回転の後) の絵に掛ける。今と同じ位置。
- 埋める (Fill = はみ出しを切る)、メガピクセル指定は範囲外。
- PDF ページは今と同じく長辺 4096px で描画してから合成する。拡大を許可して 4096px を超える目標を
  指定すると、描画済みの絵を拡大する (PDF を目標寸法で描き直さない)。ダイアログには書かない
  既知の制限として本書に残す。

### 3.3 出力先

```rust
enum BatchDestination {
    Folder { path: PathBuf, keep_structure: bool },
    SameAsSource { subfolder: String }, // 空なら元のフォルダの直下
}
```

各項目の**元の場所** (`source_dir`) を次のように決める (I/O なし、パス演算だけ):

| 元 | `source_dir` |
| --- | --- |
| 画像ファイル・スタックのメンバー | ファイルの親フォルダ |
| ZIP 内画像 | ZIP ファイルの親フォルダ |
| PDF ページ | PDF ファイルの親フォルダ |

ZIP/PDF は書庫を 1 段のフォルダとして再現しない。`<filename>` が既に書庫名を含む
(`<zip名>_<entry名>`) ので、元と同じフォルダに並べても名前で区別できる。書庫内のフォルダ階層も
再現しない。これは単ページの「元の場所」と同じ定義である。

- **元と同じフォルダ**: `source_dir` (+ `subfolder`)。`subfolder` は 1 段の名前として
  `basename_from_text` で正規化する (区切り文字を含めて別の場所へ出さない)。
- **構造の維持** (`Folder` + `keep_structure`): 全項目の `source_dir` の**共通の祖先**を基点とし、
  `path / (source_dir から基点を除いた相対パス)` へ出す。共通の祖先は大小文字を区別しない
  コンポーネント比較で求める (Windows のパス規則)。ドライブ・UNC 共有が違う項目が混ざり共通の祖先が
  無いときは、相対パスの先頭にドライブ名 (`C`) / `サーバ名_共有名` を 1 段置く。
  基点は開始時に worker が全項目から 1 回だけ決める (項目の順序に依存しない)。
- 通常フォルダの一覧で開いた場合は基点がそのフォルダになるので、構造の維持は「選んだ項目が
  サブフォルダを含む場合だけ」効く。検索結果・コレクション・スマートフォルダでは基点が自動で
  共通の祖先になる。基点の手動指定は作らない。
- フォルダ作成は worker が書く直前に `create_dir_all` する (今と同じ)。

### 3.4 ファイル名と置換子

今の `<filename>` `<dirname>` `<num>` に次を足す。未知の `<...>` は今と同じくそのまま残す。

| 置換子 | 意味 | 決める場所 |
| --- | --- | --- |
| `<ext>` | 元の拡張子 (小文字、ZIP はエントリ名の拡張子、PDF ページは `pdf`) | 準備時 (I/O なし) |
| `<num:N>` | 連番を N 桁 (1〜9) で。`<num>` は今どおり 4 桁 | 準備時 |
| `<date>` | 元の更新日 `YYYYMMDD` (ローカル時刻) | worker |
| `<time>` | 元の更新時刻 `HHMMSS` (ローカル時刻) | worker |

- 元の更新日時は、画像ファイルはそのファイル、ZIP・PDF は**書庫ファイル**の更新日時 (§6.4 と同じ)。
- 出力寸法の置換子 (`<width>` など) は入れない。名前が合成の前に決まらないと、§4.2 の
  「スキップは読み込みより前に判定する」が成り立たないため。
- ダイアログの例 (先頭 1 件) では `<date>` `<time>` を展開せず、そのまま表示して
  「書き出し時に決まります」と添える (UI で stat しない)。
- 正規化は今と同じ `basename_from_text` (区切り文字は `_`)。

### 3.5 上書きの扱い

```rust
enum OverwritePolicy { Rename, Skip, Replace } // 既定 Rename (今の挙動)
```

- `Rename`: 今と同じ。既存ファイルと同一実行内の名前を避けて `_1`〜`_9999`。
- `Skip`: 既存のファイルがあれば、その項目を**読み込む前に**飛ばし、「スキップ」として数える。
- `Replace`: 既存のファイルを置き換える。同じフォルダの一時ファイルへ書いてから
  `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` 相当で置き換える (書きかけの絵を残さない)。
  置き換えたファイルはごみ箱へ送らない (戻せないことをダイアログに書く)。
- **同一実行内の衝突は方針に関係なく常に連番**にする。2 つの項目が同じ名前へ落ちたとき、
  後の項目が前の項目の出力を上書き・スキップしない。
- **予約キー**: 同一実行内の名前の予約 (今の `claimed: HashSet<PathBuf>`) は、パスそのものではなく
  `OutputKey` で比較する。`OutputKey` は出力先の絶対パスを区切り `\` に揃え、`.` / `..` を畳み、
  末尾の `.` と空白を除き、全体を小文字化した文字列 (Windows のファイル名は大小文字を区別しないため)。
  別フォルダの `A.jpg` と `a.jpg`、`C:\out` と `c:\OUT\` のように表記が違っても同じ出力は同じキーになる。
  判定の順は「予約済みのキーなら連番へ進む」→「予約されていなければ、方針に従って既存ファイルを見る」。
  既存ファイルの有無は今と同じく `exists()` (ファイルシステムの比較規則) で見る。
- **今回の入力は上書きしない**: `Replace` でも、今回の対象の元ファイル (画像ファイル・ZIP・PDF) と
  同じ実体へは書かない。worker は開始時に全項目の元を `canonicalize` した集合を作り、
  既存の出力先を置き換える前にそれも `canonicalize` して照合する。一致したら置き換えず、
  その項目を「元ファイルは上書きしません」の失敗にする (8.3 形式名・シンボリックリンク経由の
  同一実体も、正規化で同じになる)。`Rename` / `Skip` は既存ファイルに書かないので照合しない。
- 「確認する」(1 件ごとに尋ねる) は作らない (§9 簡素化)。

### 3.6 元の情報と更新日時

```rust
keep_metadata: bool,   // 既定 false (今の挙動)
keep_file_time: bool,  // 既定 false
```

- `keep_metadata`: **出力の形式が元と同じ (JPEG→JPEG / PNG→PNG / WebP→WebP) の項目だけ**、
  元の情報を残す。形式が変わる項目・PDF ページは残さない (単ページの規則と同じ)。ダイアログの説明文に書く。
  形式ごとの転記対象:

  | 形式 | 残すもの |
  | --- | --- |
  | JPEG | APP1 (EXIF・XMP)、APP2 の ICC (`ICC_PROFILE`、複数セグメントを順に) |
  | PNG | tEXt / iTXt / zTXt (AI プロンプト)、`eXIf`、`iCCP` |
  | WebP | `EXIF`、`XMP `、`ICCP` |

  - **EXIF の共通処理**: 3 形式とも、EXIF は TIFF 本体 (JPEG は APP1 の `Exif\0\0` の後、PNG の `eXIf` は
    本体そのもの、WebP の `EXIF` は先頭に `Exif\0\0` があればその後) に対して同じ関数で処理する。
    処理は 2 つだけ: Orientation を 1 にする、IFD1 (埋め込みの縮小画像) を外す (IFD0 の「次の IFD」を 0 にし、
    IFD1 が指す縮小画像のバイト範囲を 0 で埋める。長さとオフセットは変えない)。TIFF として読めない EXIF は
    転記しない (その項目は EXIF なしで書き、失敗にはしない)。
  - **ICC は出力の画素形式に合うものだけ**: 出力は常に RGB(A) なので、ICC ヘッダの色空間
    (先頭から 16〜19 バイト) が `RGB ` のものだけ転記する。グレースケール (`GRAY`)・CMYK などの ICC は
    付けない (付けると出力に合わないプロファイルになる)。それらの元画像は ICC なしの出力になり、
    他のソフトでは sRGB として表示される。
  - **単ページの出力は変えない**: `SaveOptions` に足す欄 (EXIF の共通処理、JPEG/PNG の ICC 転記、
    PNG の `eXIf` 転記) は既定で OFF にし、一括の書き出しだけが ON にする。単ページの WebP は今と同じく
    EXIF をそのまま転記する (単ページの IFD1 の扱いは §13 の補足、別件)。
  - XMP は解釈せずそのまま残す。XMP の `xmp:Thumbnails`、EXIF の MakerNote (メーカー独自の領域) に
    含まれ得る縮小画像は消さない (§13 Q3 で利用者確認)。
  - EXIF の画素数タグは書き換えない (単ページと同じ)。
  - ZIP 内画像の転記元はデコードに使ったエントリのバイト列 (同じ読み込みを使い、2 回読まない)。
- `keep_file_time`: 書き出した後、出力ファイルの更新日時を元の更新日時 (§3.4 と同じ値) にする。
  `std::fs::File::set_modified` を使い、依存を足さない。作成日時は変えない。

## 4. worker と取消

### 4.1 準備 (UI スレッド)

準備の流れ (`advance_export_batch_preparation`) は変えない。新しい選択肢のために準備で増えるのは
文字列とパスの演算だけ (`source_dir`、`<ext>`、元の拡張子) で、ファイル I/O・DB・デコードを足さない。
`BatchExportItem` に `source_dir: PathBuf` と `source_ext: String` を足す。

§1.177 (重い 1 件の間 UI が止まる) はこの設計では直さず、悪化もさせない (§13 Q5)。

### 4.2 1 件の処理 (worker)

開始時に 1 回:

1. 出力の基点 (§3.3 構造の維持) を決める。
2. `Replace` なら元ファイルの正規化集合 (§3.5) を作る。
3. 透かし画像を読む (段 B、§7.3)。読めなければ全件を同じ理由で失敗にして終える。

項目ごと (今の順序を保ち、増える段を太字にする):

1. 取消の確認。
2. 構成の確認 (`composition_check`、今と同じ)。
3. **元の更新日時を取得** (`<date>` `<time>` か `keep_file_time` を使うときだけ `metadata`)。
4. **出力フォルダと名前を決める** (§3.3、§3.4)。同一実行内の名前は今と同じ `claimed` で予約する。
5. **`Skip` で既存のファイルがあれば `Skipped` を送って次へ** (デコードしない)。
6. デコード (**取消トークン付き** `decode_composite_source_for_materialization`、PDF の長辺 4096 は今と同じ)。
7. 保存済み編集の合成 (**取消トークン付き** `compose_book_page_for_materialization` 相当。
   diffusion fallback の戻り値は今と同じく使わない)。
8. **リサイズ** (§3.2)。
9. **透かし** (段 B)。
10. **形式ごとの寸法の上限を確かめ** (§3.1)、**エンコード** (`keep_metadata` なら転記元を渡す)。
11. 書き込み: `Rename` / `Skip` は `create_new`、`Replace` は一時ファイル→置き換え。
12. **`keep_file_time` なら更新日時を設定**。失敗しても出力は残し、その項目を失敗として報告する
    (まれな失敗の割り切り、§9)。
13. `Completed`。

- 一括の書き出し関数は `export_batch` に新しく置き (`write_batch_item` など)、一括はそちらだけを通る。
  `books::write_composited_page` は**製本 (`write_source`) のために残し、変えない**。製本の合成
  (`compose_book_page*`) も共有のまま変えない。
- 縮小は `scale_export_pixels` から「目標寸法へリサイズする」部分を切り出して共有し、
  単ページは今の `ExportScale` のまま同じ部分を呼ぶ (単ページの挙動は変えない)。

### 4.3 取消

- 一括の取消トークン (`ExportPending.cancel`) を 6・7 のデコードと合成へ渡す。消しゴム・AI・
  補正レイヤー・隠蔽・注釈は既に取消トークンを見るので、1 件の途中でも止まる。
- 取消で止まった項目は書き込まない (書き込みは 11 で、そこまでに取消を確認する)。`Replace` の
  一時ファイルは書く前に作らないので残らない。止まった後の扱いは今と同じ (`Cancelled` を送って終える)。
- 1 件の書き込み・置き換え・日時設定の途中では止めない (短いため)。

### 4.4 進捗と結果

- `ExportEvent` に `Skipped { label, path }` を足し、`ExportPending` に `skipped: Vec<…>` を持つ。
  単ページの worker は送らない (単ページの完了処理は今と同じ結果になる)。
- 進捗: 分母は今と同じ対象件数、完了数は「成功 + スキップ + 失敗」。進捗ダイアログに
  「成功 N / スキップ M / 失敗 K」を出す。
- 完了 (`AllDone`) の通知:

  | 結果 | 通知 |
  | --- | --- |
  | 失敗あり | 今と同じく進捗ダイアログを残し「完了: 成功 N / スキップ M / 失敗 K」 |
  | 失敗なし・スキップなし・成功 1 件 | 今と同じく Explorer で示す |
  | 失敗なし・スキップなし・成功 2 件以上 | 今と同じく「N 件をエクスポートしました」 |
  | 失敗なし・スキップあり・成功 1 件以上 | 「N 件をエクスポートしました (既にある M 件はスキップ)」 |
  | 失敗なし・全件スキップ | 「既にある M 件をスキップしました (書き出した画像はありません)」 |

  取消 (`Cancelled`) の通知は今と同じ文で、スキップ件数は足さない。
- **一覧の更新**: `AllDone` / `Cancelled` のどちらでも、成功パスのうち**親フォルダが今の表示先と一致する
  ものを探し**、1 つでもあればそのパスで `note_exported_file_for_folder_refresh` を 1 回だけ呼ぶ
  (今は最初の成功パスだけを渡しており、「元と同じフォルダ」で出力先が複数になると、
  先頭が別のフォルダのとき今の表示先が更新されない)。判定は今の関数と同じ `path_eq`。
  単ページは出力先が 1 つなので結果は今と同じ。
- 区間の計測: `perf::event("export_batch/item", …)` に decode / compose / resize / watermark /
  encode / write の ms、出力寸法、形式、方針の結果 (written / skipped / replaced / failed) を入れる。
  開始時の準備 (基点・正規化・透かし読み込み) も 1 つの event にする。

### 4.5 ローカル AI の借用

`local_ai_activity` (v3.5.0 F09) は今と同じく worker の寿命で持つ。並列化はしない (範囲外)。

## 5. 永続化と移行

### 5.1 欄

`Settings` に次を足す。どちらも**未リリース**の欄なので、この 2 つに移行は要らない。

```rust
#[serde(default)]
pub export_batch_options: Option<BatchExportOptions>, // 前回の設定
#[serde(default)]
pub export_batch_presets: Vec<BatchExportPreset>,     // 段 C
```

`BatchExportOptions` の各欄は `#[serde(default)]` 付きにし、段 B・C の欄追加や今後の欄追加で
前の段の保存値が読めなくならないようにする。

### 5.2 リリース済みの欄からの移行

`export_batch_directory` / `export_batch_template` / `export_batch_format` / `export_batch_scale`
はリリース済み。次のように読み替える。

- ダイアログを開くとき、`export_batch_options` が `None` なら旧 4 欄から `BatchExportOptions` を作る
  (`BatchExportOptions::from_legacy`)。`Some` ならそれだけを使い、旧 4 欄は読まない。
- 書き出しを始めると `export_batch_options = Some(...)` を保存する。**旧 4 欄の値はもう更新しない**
  (前回の設定の持ち主は新しい欄 1 つ。`write_settings_kv` は全欄を書き直すので、旧 4 欄の行は
  最後の値のまま書かれ続ける)。
- 旧 4 欄を型に残す理由は「`export_batch_options` が無い設定 (今のリリースで保存した設定) から読み替えるため」
  だけ。`build_settings_from_db` は未知の欄を無視するので、欄を消しても読み込みは壊れない。ただし読み替えの
  元が無くなるので残す。
- 読み替え表:

| 旧 | 新 |
| --- | --- |
| `CaptureFormat::Png` | `kind: Png` |
| `Jpeg95` / `Jpeg85` / `Jpeg75` | `kind: Jpeg`, `jpeg_quality: 95 / 85 / 75` |
| `ExportScale::Full` | `None` |
| `Half` / `Quarter` | `Percent(50)` / `Percent(25)` |
| `LongEdge(n)` | `LongEdge(n)`、`allow_upscale: false` |
| `export_batch_directory` (と今の初期値の決め方) | `Folder { path, keep_structure: false }` |
| `export_batch_template` | そのまま |
| — | `filter: Lanczos3`、`overwrite: Rename`、`keep_metadata: false`、`keep_file_time: false`、透かしなし |

- 今の保存先の初期値の決め方 (`export_batch_directory` が存在すればそれ、無ければ単ページの直前、
  無ければ今のフォルダ) は `Folder` の初期値にだけ使う。`export_batch_options` の `Folder` の path が
  空のときも同じ決め方で埋める。
- 旧版へ戻した場合: (1) 新しい 2 欄は旧版にとって未知の欄で、旧版は未知の欄を無視して読める
  (既存の enum への値の追加ではないので、`unknown variant` の非互換にはならない)。(2) それとは別に、
  v2.6.0 以降は新しい版が保存した設定を `app_version` で検出して保存を止め、復元を案内する
  ([settings-sqlite-migration.md §6.4](settings-sqlite-migration.md))。(3) 旧版で設定を保存し直すと
  未知の 2 欄 (前回の設定・プリセット) は保持されない。これは既存の新欄と同じ扱いで、
  新しい版へ戻すと旧 4 欄から読み替え直す (プリセットは失われる)。
- 設定の持ち運び (§1.317): `export_batch_options` は既存の `export_batch_*` と同じく
  「環境設定外で管理する状態」として**除く**。`export_batch_presets` も、保存先・透かし画像の
  PC 固有のパスを含むので**除く** (`settings_transfer.rs` の分類表に足す)。

## 6. ダイアログ

### 6.1 構成

```
┌ 一括エクスポート ─────────────────────────── × ┐
│ 12 件を書き出します (対象外 2 件)                 │  ← 固定 (見出し)
│ プリセット [ (選ぶ) ▼ ] [保存...] [削除]          │  ← 固定 (段 C)
├──────────────────────────────────┤
│ ▼ 出力先とファイル名                            │  ↑
│   ◉ フォルダ [............] [変更...]           │  │
│      ☐ 元のフォルダ構造を保つ                   │  │
│   ○ 元と同じフォルダ  サブフォルダ [....]      │  │
│   ファイル名 [<filename>      ]                 │  │  ScrollArea
│   <filename> <dirname> <num> <num:N> <ext> …   │  │  (縦)
│   例: IMG_0001.jpg                              │  │
│   同じ名前があるとき ◉連番 ○スキップ ○上書き   │  │
│ ▼ 形式                                          │  │
│   [JPEG ▼] 品質 [95]  …                         │  │
│   ☐ 元の情報 (EXIF・XMP・AI プロンプト・色) を残す │  │
│   ☐ 元の更新日時にする                          │  │
│ ▼ サイズ                                        │  │
│   [長辺 ▼] [2048] px  ☐ 拡大する  方式 [Lanczos ▼] │  │
│ ▼ 透かし (段 B)                                 │  ↓
├──────────────────────────────────┤
│ [書き出す] [キャンセル]          エラー文        │  ← 固定 (操作行)
└──────────────────────────────────┘
```

- 見出し 2 行と操作行は ScrollArea の外。本文だけを縦 ScrollArea
  (`auto_shrink([false, true])`) に入れる。
- 本文の最大高は `ctx.content_rect()` から、枠・見出し・操作行の**実測の高さ**と画面余白を引いて
  決める (固定の本文高・一律の大きな控除を使わない)。`resizable(false)` のままにし、幅は約 520pt。
- 節は `CollapsingHeader` にしない (閉じた節の設定が効いていることに気付けないため)。見出しラベルだけ置く。
- `default_pos()` を使い `anchor()` を使わない。ドラッグ移動・× は今と同じ。
- テキスト欄 (保存先・サブフォルダ・ファイル名・透かしの文字・プリセット名) はすべて
  `crate::ime_focus` の helper を通す。Esc は今と同じく閉じる。
- **Enter の持ち主は 1 つ**: Enter は描画の前に `dialog_enter_pressed` で 1 回だけ取り、その Enter で
  実行するのは次のどれか 1 つだけにする。
  1. ComboBox などの popup が開いているか、ボタン・チェック・ラジオ・ComboBox にキーボードの
     フォーカスがある → **部品の操作だけ** (egui がその部品を操作する)。書き出し・保存はしない。
  2. プリセット名の欄にフォーカスがある → **プリセットの保存だけ** (§8)。
  3. それ以外 (フォーカスが無いか、保存先・サブフォルダ・ファイル名・透かしの文字の欄にある) →
     今と同じく「書き出す」(開始できる状態のときだけ)。
  判定は描画の前のフレームのフォーカスと popup の状態で行う (Enter を押したフレームの描画で部品が
  先に反応するため)。今は Enter を無条件で開始に使っており、「変更...」ボタンにフォーカスがあると
  フォルダ選択と書き出しが両方動き得る。これも 1 の規則で直る。
- ComboBox は `dark_popup_style` 等の既存 helper の配色に従い、popup 内に ScrollArea を置く場合は
  wheel を消費する (coding-conventions の規則)。文字色は `ui.visuals()` の値を使う。今の
  エラー表示の固定色 `Color32::from_rgb(255, 140, 140)` は `error_fg_color` に直す (触るついで)。
- 表示されるかどうかの登録は今の `export_batch_dialog.is_some()` のまま (準備中も描画する)。
- 選択に応じて出す欄を変える (品質は JPEG/WebP のときだけ、代わりの形式は「元と同じ形式」のときだけ、
  サブフォルダは「元と同じフォルダ」のときだけ、構造の維持は「フォルダ」のときだけ)。

### 6.2 小さな画面

- 1093×614 logical points と 1366×728 で、見出し・操作行の全ボタンとタイトルの × が
  画面内に収まり押せること、本文が最後の欄までスクロールで届くことを検査する
  (`tests/support/scroll_layout.rs` の helper)。UI 倍率 100% / 200% の両方。
- UI のスナップショット (`tests/ui_snapshot.rs`) に、既定の状態・全欄を出した状態
  (元と同じ形式 + 元と同じフォルダ + 透かし) を 1093×614 で足す。

### 6.3 文言

- 今の注記「元の EXIF / AI プロンプトは引き継ぎません」「同名のファイルは上書きせず、末尾に連番を付けます」は、
  選択に応じた文へ置き換える (例: 元の情報を残す OFF のときは今の文、ON のときは
  「形式が元と同じ画像だけ、元の情報を残します」)。上書きを選んだときは
  「上書きしたファイルは元に戻せません。今回の元ファイルは上書きしません」。
- 固定文言に絵文字・環境依存記号を使わない (`scripts/check_ui_glyphs.py`)。

## 7. 透かし (段 B)

### 7.1 選択肢

```rust
struct BatchWatermark {
    enabled: bool,                // 既定 false
    kind: WatermarkKind,          // Text / Image
    text: String,
    text_color: [u8; 3],          // 既定 白
    text_outline: bool,           // 既定 true (黒の縁取り)
    image_path: Option<PathBuf>,
    anchor: WatermarkAnchor,      // 9 か所 (左上〜右下)。既定 右下
    size_percent: u32,            // 既定: 文字 4 / 画像 20
    margin_percent: u32,          // 0..=20、既定 2
    opacity_percent: u32,         // 0..=100、既定 60
}
```

- 文字と画像は 1 つだけ (両方同時には付けない)。
- 大きさの基準: **文字**は文字の高さ (`size_px`) を出力画像の短辺に対する %、**画像**は透かし画像の幅を
  出力画像の幅に対する % にする (縦横比は保つ。透かし画像が出力より大きくなる場合は出力の内側に収まるまで縮める)。
- 余白は出力画像の短辺に対する %。位置は 9 か所の基準点から余白だけ内側。
- タイル (敷き詰め)・回転・影・フォントの選択は入れない (§13 Q4)。

### 7.2 合成の順番

```
デコード → 保存済み編集 (補正・消しゴム・補正レイヤー・隠蔽・AI・表示用補正・注釈・回転・切り取り)
        → リサイズ → 透かし → エンコード (JPEG はここで背景合成)
```

- 透かしは**全件に同じもの**を、各画像の保存済み編集とリサイズの**後**に重ねる (§1.362 の決定)。
- リサイズの後にする理由: 透かしの大きさを出力寸法に対して決めるので、どのサイズで書き出しても
  文字が同じ割合・同じ鮮明さになる。リサイズの前に焼くと縮小で文字がつぶれる。
- 焼き込みの段 (`bake_stage_export_batch`) に関係なく掛ける (透かしは書き出しの段であり、編集ではない)。
- 合成は straight alpha の src-over (`comic_overlay::composite_overlay_over` と同じ流儀) で、
  不透明度は透かしのアルファに掛ける。透過のある出力 (PNG・WebP) でも同じ式。

### 7.3 描画

- 文字: 注釈の文字描画 (comic-core の `TextBlock`、縁取りは `outline`) を使う。フォントは注釈の既定
  フォント (`comic_overlay::load_comic_fonts`、Yu Gothic 等) を **worker が開始時に 1 回**読む。
  読めなければ全件を「透かしのフォントを読めません」の失敗にして終える。
  文字の外接矩形を測り、基準点へ置く。複数行は入れない (1 行)。
- 画像: worker が開始時に 1 回デコードして RGBA にし、項目ごとに目標寸法へ縮小する
  (同じ寸法の出力が続くときは前の縮小結果を使う)。PNG のアルファをそのまま使う。
  ファイルを読めない・デコードできないときは全件を同じ理由で失敗にして終える。
- UI はファイル選択 (`rfd`、今の保存先選択と同じ) とファイル名の表示だけで、画像を読まない。
  プレビューは作らない (範囲外)。

## 8. プリセット (段 C)

```rust
struct BatchExportPreset { name: String, options: BatchExportOptions }
```

- 保存: 「保存...」でダイアログ上部の行が名前の入力欄に変わる。Enter か「保存」で今のダイアログの値
  (`BatchExportOptions` 全部: 形式・品質・リサイズ・出力先・名前・上書き・元の情報・日時・透かし) を保存する。
  名前は前後の空白を除き 1〜40 文字。同じ名前があれば置き換える (ボタンの表示を「上書き保存」に変える)。
  一覧の上限は 50 件。
- 呼び出し: ComboBox で選ぶと、その値でダイアログの値を**置き換える**。プリセットとダイアログの値の
  結び付き (変更済みの印・自動保存) は持たない。書き出すと、その時点の値が「前回の設定」になる。
- 削除: 選んでいるプリセットを「削除」で消す。確認は出さない (プリセットは設定の組であり、
  画像や編集は失われない)。
- 保存は、利用者が保存・削除を押した時点で `Settings::save_checked()` を呼ぶ (UI スレッドで同期)。
  これは今の一括エクスポートの開始時の保存、環境設定の OK など、利用者の明示操作で設定を保存する
  既存の経路 (`settings.save()` の呼び出しは約 230 か所) と同じ扱いで、新しい種類の I/O ではない。
  プリセット専用の書き込み worker を作ると、同じ `settings.db` を書く経路が 2 本になり、
  順序の食い違いが生まれるので作らない。`save_checked()` が失敗を返したら
  「プリセットを保存できませんでした」を通知し、ログに残す (メモリ上の一覧はそのまま。次に設定が
  保存されたときに一緒に書かれる)。保存は `SETTINGS_WRITE_GATE` で直列化される。
- プリセットの並びは保存した順。並べ替え・名前の変更・書き出し/読み込み・メニューからの直接実行は範囲外。

## 9. 簡素化と、まれな失敗の割り切り

CLAUDE.md「設計の簡素化」に沿って検討した結果:

- **採用: 選択肢は開始時に 1 回だけ worker へ渡す**。実行中にダイアログは無く (今と同じく進捗ダイアログ
  だけ)、選択肢が途中で変わる組み合わせが無い。
- **採用: 上書きの「確認する」を作らない**。worker から UI へ 1 件ごとに尋ねると、待機中の取消・閉じる・
  別の書き出しとの組み合わせが生まれる。方針は開始前に 1 つ選ぶ。
- **採用: 同一実行内の衝突は常に連番**。方針ごとに同一実行内の扱いを変えない。
- **採用: 元の情報は形式が同じときだけ**。形式をまたぐ転記 (EXIF を PNG へ等) を作らない。
- **採用: プリセットは値の置き換えだけ**。変更済みの追跡・プリセットとの同期を持たない。
- **採用: 構造の維持の基点は自動 (共通の祖先) だけ**。手動の基点の欄を作らない。
- **採用: 透かしの素材 (フォント・画像) は開始時に 1 回読む**。項目ごとに読み直さない。読めなければ
  全体を失敗にする (一部だけ透かしの無い出力を作らない)。
- **採用しない: 1 件ごとのモーダル化の追加**。一括エクスポートのダイアログは既に modal 相当
  (`common_modal_dialog_open`)、書き出し中は今の進捗ダイアログのままで足りる。

まれな失敗は割り切り (ログ + 通知): 書き込み・置き換え・更新日時の設定・フォルダ作成の失敗は、
その項目のエラーとして進捗ダイアログの一覧と mimageviewer.log に出し、次の項目へ進む (今と同じ)。
再試行・失敗の種類分けはしない。置き換えの途中で mIV が強制終了した場合に残る一時ファイル
(`<名前>.miv-tmp-*`) は掃除しない (次回の同じ書き出しでは別名の一時ファイルを使う)。

## 10. テスト

- **既定の挙動が変わらないこと**: `from_legacy` で作った選択肢 (旧 4 欄の全組み合わせ: PNG/JPEG95/85/75 ×
  そのまま/1/2/1/4/長辺) で書いた出力が、今の `write_composited_page` の出力と**バイト一致**し、
  名前も一致する (同じ入力画像・同じ編集 snapshot)。今の関数を削除する前にこのテストを書き、
  比較の基準 (期待バイト列のハッシュ) を固定してから削除する。
- 選択肢の単体: 寸法の計算 (各モード × 拡大の可否 × 縦長/横長/正方形、Percent(50/25) が今の
  Half/Quarter と同値)、形式の決定 (拡張子の表、PDF → 代わりの形式)、置換子 (`<num:N>`・`<ext>`・
  未知の置換子が残る)、出力先 (元と同じフォルダ、サブフォルダの正規化、共通の祖先、ドライブ違い)。
- worker:
  - `Skip` は既存ファイルの項目をデコードしない (デコード回数の計測で確かめる)。
  - `Replace` は既存ファイルを置き換え、今回の元ファイル (別名・大小文字違いのパスを含む) は置き換えない。
  - 同一実行内の衝突は `Replace` / `Skip` でも連番。別フォルダの `A.jpg` と `a.jpg`、出力先の表記違い
    (`C:\out` と `c:\OUT\`) でも先の出力を置き換え・スキップしない。
  - WebP で 16383px を超える項目はその項目だけ失敗し、worker は止まらず次へ進む (panic しない)。
  - 完了処理: §4.4 の通知の表の各行、成功パスの先頭が別フォルダでも今の表示先の更新が 1 回だけ要求される。
  - 取消が合成の途中で効く (遅い消しゴム runner を注入し、取消後に書き込みが無いこと)。
  - `keep_metadata`: 3 形式それぞれで §3.6 の表のものが残り、EXIF の Orientation が 1、IFD1 の縮小画像が
    消える (IFD0 の次 IFD が 0、元の縮小画像のバイトが残らない)。`GRAY` / CMYK の ICC は付かない。
    形式違いでは何も残らない。読めない EXIF は落とし、その項目は成功する。
  - 単ページの出力が変わらない: `SaveOptions` の新しい欄が既定 OFF のとき、JPEG / PNG / WebP の
    転記結果が今と同じバイト列 (WebP の `encode_simple` への置き換えを含む)。
  - `keep_file_time`: 出力の更新日時が元と一致する (ZIP/PDF は書庫の日時)。
  - 透かし (段 B): 9 か所の位置・余白・大きさの割合、不透明度 0 で出力が透かし無しと一致、
    透かし画像を読めないと全件失敗で何も書かない。
- 設定: `export_batch_options` が無い設定から旧 4 欄を読み替える、ある設定では旧 4 欄を無視する、
  書き出し開始で旧 4 欄を書かない、`settings_transfer` の分類漏れ検査が通る。
- UI: §6.2 の配置検査とスナップショット、Enter の振り分け (§6.1 の 1〜3 がそれぞれ 1 つだけ実行される。
  ボタン・ComboBox にフォーカスがあるときは書き出さない、プリセット名の欄では保存だけ)、
  プリセットの保存・置き換え・呼び出し・削除と、保存失敗時の通知 (段 C)。

## 11. 文書の更新 (実装と同時)

- `htdocs/mimageviewer/manual/export.html` の一括エクスポートの節 (形式・サイズ・出力先・上書き・
  元の情報・透かし・プリセット)。版固有の説明・実装用語を書かない。
- `htdocs/mimageviewer/index.html` に一括エクスポートの記述があれば合わせる (カードは 150 字まで)。
- 通信は無い。`privacy.html` の「端末内に保存されるデータ」が設定の中身を列挙していれば、
  書き出しのプリセットを足す (実装時に確かめる)。
- 本書の状態欄、[docs/README.md](README.md) の索引、`docs/ui-responsiveness.md` (§4 の記録)。

## 12. 独立設計レビューの記録

担当: Codex `gpt-6.1-sol` / `xhigh`、read-only、ソース照合のみ (製品の起動・テストはしていない)。

### 1 回目 (2026-11-24、対象 1bde71a10): REVISE

| 指摘 | 対応 |
| --- | --- |
| P1 `write_composited_page` は製本の `write_source` も使うので削除できない | 製本用に残し、一括だけ新しい関数へ (§1・§4.2) |
| P1 予約集合が `PathBuf` 比較で、大小文字・表記違いの同じ出力を区別してしまう | `OutputKey` を定義し、判定の順を明記 (§3.5) |
| P2 形式ごとの転記の違い (PNG `eXIf`、WebP の Orientation、XMP の縮小画像) | 形式別の転記表と EXIF の共通処理、単ページは既定 OFF で不変、Q3 を 2 択に (§1・§3.6・§13) |
| P2 WebP 16383px 超で `unwrap()` の panic | 形式ごとの寸法の上限で項目失敗、`encode_simple` へ (§1・§3.1・§4.2) |
| P2 RGBA 出力にグレースケール等の ICC を付けると不適合 | `RGB ` の ICC だけ転記 (§1・§3.6) |
| P2 Enter が部品の操作と書き出しを両方動かし得る | Enter の持ち主を 1 つにする規則 (§6.1) |
| P2 プリセットの保存が UI の同期書き込み | 既存の明示操作の保存経路と同じ扱いにする理由、`save_checked` と失敗通知 (§8) |
| P2 一覧の更新が最初の成功パスだけを見る | 今の表示先に一致する成功パスを探す (§4.4) |
| P2 スキップ件数が完了時に伝わらない | 進捗と完了通知の表 (§4.4) |
| P3 未知の欄・旧欄の説明が不正確 | 書き直し (§5.2) |

## 13. 利用者の判断が要る点

- **Q1 リリースの単位**: A (出力の基本)・B (透かし)・C (プリセット) を 1 つの版でまとめて出すか、
  段ごとに出すか。推奨: まとめて (プリセットが全部の選択肢を含むため)。
- **Q2 上書き**: 「上書き」を選択肢に入れるか (今回の元ファイルは上書きしない保護付き、置き換えたファイルは
  ごみ箱へ送らない)。推奨: 入れる。
- **Q3 「元の情報を残す」に残り得る元の縮小画像**: 形式が同じときだけ残し、EXIF の埋め込みサムネイル
  (IFD1) は消す。次の 2 つから選ぶ。
  - (a) EXIF の縮小画像だけ消す。XMP の `xmp:Thumbnails` と MakerNote (カメラのメーカー独自の領域) に
    含まれ得る縮小画像は残る。消しゴム・隠蔽で隠した部分を確実に残したくないときは「元の情報を残す」を
    OFF にしてもらう (既定は OFF。ON のときダイアログに「隠した部分が元の情報の中の縮小画像に残ることが
    あります」と書く)。費用: 小。
  - (b) (a) に加えて XMP の `xmp:Thumbnails` も消す。XMP (XML) を解釈して書き直す処理が増える。
    MakerNote は形式がメーカーごとに違うので (b) でも消せない。費用: 中。
  推奨: (a)。
  また、グレースケール・CMYK の元画像の ICC は出力 (常に RGB) に合わないので付けない (§3.6)。
  その画像は他のソフトで色の見え方が少し変わり得る。これは既定の規則とし、質問にはしない。
- **Q4 透かしの文字のフォント**: 注釈の既定フォント 1 本だけ (選べない) でよいか。選べるようにするなら、
  フォントの一覧作りを worker へ移す作業が増える。推奨: 1 本だけ。
- **補足 (質問ではない、別件の報告)**: 単ページの Ctrl+E は「メタデータを保持」が既定 ON で、
  JPEG・WebP の EXIF をそのまま転記する。消しゴム・隠蔽を焼いた出力にも、元の絵の縮小画像 (IFD1) が
  残り得る (source inspection、実機の観測ではない)。単ページは今回の範囲外なので、別のバックログ項目として扱う。
- **Q5 §1.177 (準備中に重い 1 件で UI が止まる)**: 今回は直さず別件のままにするか。この設計は準備に
  I/O を足さないので悪化はしない。推奨: 別件のまま。
