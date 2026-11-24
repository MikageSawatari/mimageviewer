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
| 書き出し | `export_batch::run_batch_export` → `books::write_composited_page` (呼び出し元はこの 1 か所だけ) → `write_composited_color_image` (`include_metadata: false` 固定、`edits.format` で JPEG/PNG を選ぶ)。 |
| 名前 | `<filename>` `<dirname>` `<num>` (4 桁)。既存・同一実行内の衝突は `_1`〜`_9999`。上書きなし (`save_image_with_metadata` も `create_new`)。 |
| 取消 | 項目の間だけ。`decode_composite_source` / `compose_book_page` は呼ぶたびに新しい未使用の `AtomicBool` を作るので、1 件の途中 (AI・消しゴム・補正レイヤー) は止まらない。 |
| 記憶 | `export_batch_directory` / `export_batch_template` / `export_batch_format` / `export_batch_scale` (いずれもリリース済み)。 |
| ZIP/PDF の `<filename>` | `capture_basename_for_idx` が `<zip名>_<entry名>` / `<pdf名>_pNNNN` を作る (書庫名を含む)。 |

報告に無かった事実:

- `save_with_metadata` の JPEG 転記は元の APP1 を**そのまま**挿入する (`extract_jpeg_app1_segments`)。
  EXIF の IFD1 (元画像の縮小サムネイル) も残る。JPEG の ICC (APP2) と PNG の `iCCP` は転記しない
  (WebP は `ICCP` を転記する)。§6.3 で扱う。
- mIV のデコードは ICC による色変換をしない (コードに ICC 変換が無い)。画素は元の色空間のまま。

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
  元の EXIF・XMP・AI プロンプト (PNG のテキスト) と ICC を残す。形式が変わる項目・PDF ページは残さない
  (単ページの規則と同じ)。ダイアログの説明文に書く。
  - 転記は既存の `save_with_metadata` の仕組みを使い、`SaveOptions` に ICC の欄を足して
    JPEG の APP2 (`ICC_PROFILE`、複数セグメント) と PNG の `iCCP` も転記する (WebP は既に転記する)。
    デコードで色変換をしていない (§1) ので、元の ICC を付けるのが正しい。
  - **EXIF の埋め込みサムネイル (IFD1) は消す**。元の縮小画像が残ると、消しゴム・隠蔽・切り取りで
    消した部分が出力ファイルの中に残る。IFD0 の「次の IFD」を 0 にし、IFD1 が指す
    サムネイルのバイト範囲を 0 で埋める (APP1 の長さとオフセットは変えない)。
    MakerNote など、メーカー独自の領域の中身は解釈しない (§13 Q3 で利用者確認)。
  - Orientation は今の単ページと同じく 1 にする (`caller_applied_orientation: true`)。
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
10. **エンコード** (§3.1、`keep_metadata` なら転記元を渡す)。
11. 書き込み: `Rename` / `Skip` は `create_new`、`Replace` は一時ファイル→置き換え。
12. **`keep_file_time` なら更新日時を設定**。失敗しても出力は残し、その項目を失敗として報告する
    (まれな失敗の割り切り、§9)。
13. `Completed`。

- 一括の書き出し関数は `export_batch` に新しく置き (`write_batch_item` など)、
  `books::write_composited_page` は呼び出し元が無くなるので削除する。製本の合成
  (`compose_book_page*`) は共有のまま変えない。
- 縮小は `scale_export_pixels` から「目標寸法へリサイズする」部分を切り出して共有し、
  単ページは今の `ExportScale` のまま同じ部分を呼ぶ (単ページの挙動は変えない)。

### 4.3 取消

- 一括の取消トークン (`ExportPending.cancel`) を 6・7 のデコードと合成へ渡す。消しゴム・AI・
  補正レイヤー・隠蔽・注釈は既に取消トークンを見るので、1 件の途中でも止まる。
- 取消で止まった項目は書き込まない (書き込みは 11 で、そこまでに取消を確認する)。`Replace` の
  一時ファイルは書く前に作らないので残らない。止まった後の扱いは今と同じ (`Cancelled` を送って終える)。
- 1 件の書き込み・置き換え・日時設定の途中では止めない (短いため)。

### 4.4 進捗と結果

- `ExportEvent` に `Skipped { label, path }` を足し、進捗ダイアログに「スキップ N 件」を出す。
  単ページの worker は送らない。
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
- 書き出しを始めると `export_batch_options = Some(...)` を保存する。**旧 4 欄にはもう書かない**
  (前回の設定の持ち主は新しい欄 1 つ)。旧 4 欄は読み替えの元として `Settings` に残す
  (消すと保存済み DB の読み込みで未知の欄になり得るため、型から外さない)。
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
- 旧版へ戻した場合: v2.6.0 以降は新しい版が保存した設定を検出して保存を止め、復元を案内する
  ([settings-sqlite-migration.md §6.4](settings-sqlite-migration.md))。新しい欄は旧版にとって
  未知の欄で、型の値の追加ではないので、v2.5.0 以前の互換の工夫 (§10.5) も要らない。
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
  `crate::ime_focus` の helper を通す。Enter は今と同じく `dialog_enter_pressed` で「書き出す」。
  ただし**プリセット名の欄にフォーカスがあるときの Enter はプリセットの保存**にする (§8)。
  Esc は今と同じく閉じる。
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
- 保存先は `Settings::save()` (今の保存経路)。保存の失敗は今の設定保存と同じ扱い。
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
  - 同一実行内の衝突は `Replace` / `Skip` でも連番。
  - 取消が合成の途中で効く (遅い消しゴム runner を注入し、取消後に書き込みが無いこと)。
  - `keep_metadata`: 同形式で EXIF・XMP・PNG テキスト・ICC (JPEG APP2 / PNG iCCP / WebP ICCP) が残り、
    IFD1 のサムネイルが消える (IFD0 の次 IFD が 0、元のサムネイルのバイトが残らない)。形式違いでは残らない。
  - `keep_file_time`: 出力の更新日時が元と一致する (ZIP/PDF は書庫の日時)。
  - 透かし (段 B): 9 か所の位置・余白・大きさの割合、不透明度 0 で出力が透かし無しと一致、
    透かし画像を読めないと全件失敗で何も書かない。
- 設定: `export_batch_options` が無い設定から旧 4 欄を読み替える、ある設定では旧 4 欄を無視する、
  書き出し開始で旧 4 欄を書かない、`settings_transfer` の分類漏れ検査が通る。
- UI: §6.2 の配置検査とスナップショット、Enter の振り分け (プリセット名の欄では保存)、
  プリセットの保存・置き換え・呼び出し・削除 (段 C)。

## 11. 文書の更新 (実装と同時)

- `htdocs/mimageviewer/manual/export.html` の一括エクスポートの節 (形式・サイズ・出力先・上書き・
  元の情報・透かし・プリセット)。版固有の説明・実装用語を書かない。
- `htdocs/mimageviewer/index.html` に一括エクスポートの記述があれば合わせる (カードは 150 字まで)。
- 通信は無い。`privacy.html` の「端末内に保存されるデータ」が設定の中身を列挙していれば、
  書き出しのプリセットを足す (実装時に確かめる)。
- 本書の状態欄、[docs/README.md](README.md) の索引、`docs/ui-responsiveness.md` (§4 の記録)。

## 12. 独立設計レビューの記録

(未記録。レビューを受けたら、担当・回・結論と主な指摘への対応をここに書く。)

## 13. 利用者の判断が要る点

- **Q1 リリースの単位**: A (出力の基本)・B (透かし)・C (プリセット) を 1 つの版でまとめて出すか、
  段ごとに出すか。推奨: まとめて (プリセットが全部の選択肢を含むため)。
- **Q2 上書き**: 「上書き」を選択肢に入れるか (今回の元ファイルは上書きしない保護付き、置き換えたファイルは
  ごみ箱へ送らない)。推奨: 入れる。
- **Q3 元の情報を残すときの EXIF**: 形式が同じときだけ残し、EXIF の埋め込みサムネイルは消す。
  カメラのメーカー独自領域 (MakerNote) に含まれ得るプレビュー画像は解釈せず残る。
  消しゴム・隠蔽を使った画像で元の絵を確実に残したくない場合は「元の情報を残す」を OFF にしてもらう
  運用でよいか。推奨: それでよい (既定は OFF)。
- **Q4 透かしの文字のフォント**: 注釈の既定フォント 1 本だけ (選べない) でよいか。選べるようにするなら、
  フォントの一覧作りを worker へ移す作業が増える。推奨: 1 本だけ。
- **Q5 §1.177 (準備中に重い 1 件で UI が止まる)**: 今回は直さず別件のままにするか。この設計は準備に
  I/O を足さないので悪化はしない。推奨: 別件のまま。
