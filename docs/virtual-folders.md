# 仮想フォルダ (ZIP / PDF) 処理

ZIP アーカイブと PDF ドキュメントは「中身のページをフォルダ内のファイルに見立てて扱う」仮想フォルダとして実装されている。
通常画像ファイルとの処理分岐が多く、修正漏れが起きやすい。**ZIP/PDF 対応のある機能を触るときは必ずこのドキュメントを見る**。

---

## 1. GridItem バリアント

`grid_item.rs` の `GridItem` 列挙型は以下の 8 バリアント:

| バリアント | 発生元 | 中身 |
| --- | --- | --- |
| `Folder(PathBuf)` | 通常フォルダ | 実ファイルシステムのディレクトリ |
| `Image(PathBuf)` | 通常フォルダ内 | 画像ファイル |
| `Video(PathBuf)` | 通常フォルダ内 | 動画ファイル |
| `ZipFile(PathBuf)` | 通常フォルダ内 | ZIP アーカイブ (未展開) |
| `PdfFile(PathBuf)` | 通常フォルダ内 | PDF ドキュメント (未展開) |
| `ZipImage { zip_path, entry_name }` | ZIP を開いた中 | ZIP 内の画像エントリ |
| `ZipSeparator { dir_display }` | ZIP 内にサブディレクトリがある時 | 区切り表示用の疑似アイテム (ロード対象外) |
| `PdfPage { pdf_path, page_num }` | PDF を開いた中 | PDF のページ (0-indexed) |

`Folder/Image/Video/ZipFile/PdfFile` は「外側」= 通常フォルダのリスト。
`ZipImage/ZipSeparator/PdfPage` は「内側」= 仮想フォルダのリスト。
同じリストに両者が混在することはない。

---

## 2. 仮想フォルダの展開

### 2.1 ZIP を開く (`App::load_zip_as_folder`)

```
1. zip_loader::enumerate_image_entries(zip_path)
   - ZIP を 1 度だけ開いて全エントリをスキャン
   - **拡張子フィルタは `folder_tree::is_recognized_image_ext` に委譲**
     (ネイティブ + WIC + ロード済み Susie プラグインの対応拡張子すべて)
   - v0.6 以前はローカル定数 IMAGE_EXTS (jpg/jpeg/png/webp/bmp/gif) を
     持っていて HEIC / AVIF / JXL / RAW / PI / MAG が落ちる不整合があったが、
     v0.7.0 で修正済み。ZIP 内でも本体とフォルダスキャンと同じ画像集合が出る。
   - Susie プール未初期化時は `susie_loader::supports_extension` 内で
     `get_pool()` がブロックして init 完了を待つ (通常数百 ms、一度だけ)。
   - __MACOSX/ やドットファイル (._*) を除外
   - Vec<ZipImageEntry> を返す (path, uncompressed_size, mtime)
   - **v0.7.0 以降: 外側 ZIP 内の .zip エントリは再帰展開され**、
     entry_name は "chapters/ch01.zip/page01.jpg" のように親 ZIP 名を含む
     パスになる。内側 ZIP バイト列は zip_loader 内の LRU キャッシュ (256MB) に
     保持され、後続の read_entry_bytes で再展開せずに参照される。

2. エントリをサブディレクトリで BTreeMap にグループ化
   - `entry_dir` が返すパスにネスト ZIP 境界 (.zip/) を含むため、グループ名が
     "chapters/ch01.zip" のように .zip を含むことがある。表示上はそのまま
     見出し ("chapters/ch01.zip") として出る。

3. items に:
   - グループが 2 つ以上あれば各グループの先頭に ZipSeparator を挿入
   - 各エントリを GridItem::ZipImage として追加
   - image_metas に (mtime, uncompressed_size) を記録
```

同期処理。ZIP は比較的高速に列挙できるため UI スレッドでそのまま実行している。
ネスト ZIP が多いと列挙時に内側 ZIP を展開する分 I/O が発生するが、同一
セッション内の再列挙・サムネイル読み込みでは LRU キャッシュに当たる。

### 2.2 PDF を開く (`App::load_pdf_as_folder`)

PDF は **非同期**で開く:

```
1. 即座に items = [] で画面を更新
2. PDF ワーカープロセスに enumerate 要求を投げる (別スレッド)
3. pdf_enumerate_pending に受信チャネルを保持
4. 毎フレーム poll_pdf_enumerate() で結果チェック
5. 成功: GridItem::PdfPage を pages 分だけ追加
   パスワード必要: ダイアログを出して再試行
```

PDF ワーカーは別プロセス (`mimageviewer.exe --pdf-worker`)。プロセス間通信は
stdin/stdout の長さプレフィクス付きバイナリプロトコル。

---

## 3. 分岐ポイント (修正漏れ要注意)

### 3.1 サムネイル生成の分岐

`thumb_loader.rs::process_load_request` 内で `LoadRequest` のフィールドを見て分岐:

| GridItem | `zip_entry` | `pdf_page` | `cache_key_override` | サムネ取得方法 |
| --- | --- | --- | --- | --- |
| Image | None | None | なし | ファイル直接デコード |
| Folder | None | None | `folderthumb:{dirname}` | 再帰的に代表画像を探してデコード |
| ZipFile | None | None | `zipthumb:{filename}` | `zip_loader::read_first_image_bytes` で先頭画像 |
| PdfFile | None | Some(0) | `pdfthumb:{filename}` | PDF ワーカーでページ 0 をレンダリング |
| ZipImage | Some(entry) | None | なし (entry が自動キー) | ZIP からエントリバイト → decode |
| PdfPage | None | Some(page) | `pdf_page_cache_key(page)` | PDF ワーカーでそのページをレンダリング |

**キャッシュキーの命名規則**を勝手に変えないこと。既存キャッシュが全部無効になる。

### 3.2 フルスクリーンロードの分岐

`App::start_fs_load`:

```rust
match grid_item {
    GridItem::PdfPage { .. } => {
        // PDF ワーカーで 4096px 描画
    }
    GridItem::ZipImage { zip_path, entry_name } => {
        // ZIP から bytes 読み出し → image::load_from_memory → 失敗時 WIC ストリームフォールバック
        // (SHCreateMemStream + IWICImagingFactory::CreateDecoderFromStream)
        // EXIF 不可、アニメーション不可
    }
    GridItem::Image(path) => {
        // image::open → 失敗時 WIC フォールバック
        // EXIF Orientation 適用
        // GIF/APNG ならアニメーションモードで全フレーム展開
    }
    _ => { /* それ以外はフルスクリーン対象外 */ }
}
```

**ZipImage でできないことリスト**:

- EXIF Orientation 自動回転 (rexif がパスを要求)
- GIF / APNG アニメーション (fs_animation がパス API)

WIC デコードは `wic_decoder::decode_to_dynamic_image_from_bytes` でバイト列から
直接デコードできるため、ZIP 内の HEIC/AVIF/JXL/TIFF/RAW も開ける
(対応コーデックがインストールされていれば)。サムネイル・フルスクリーン両方の
ZIP エントリ経路で `image::load_from_memory` 失敗時のフォールバックとして使われる。

### 3.3 回転 / 補正 / 消しゴムマスクのキー

すべての DB は以下の正規化キーで保存:

- **Image**: ファイルパス (小文字 + `\` → `/`)
- **ZipImage**: `{zip_path 正規化}|{entry_name}` (`|` 区切り)
  - ネスト ZIP の entry_name は `"chapters/ch01.zip/page01.jpg"` 形式。
    外側 ZIP パスと合わせれば DB 内で一意になる。
- **PdfPage**: `{pdf_path 正規化}|page={page_num}`

新しい永続ストレージを追加する時は、`rotation_db.rs` と `adjustment_db.rs` の
`normalize_path` / page_key 生成に揃えること。**キー規則がズレると ZIP/PDF の回転や補正が保存されない**。

`rating_db.rs` は `App::page_path_key` が返すキー (`adjustment_db::normalize_path` と同形式、
ZipImage は `::`、PdfPage は `::page_N` 区切り) をそのまま DB パスとして使う。
新規 DB を追加する際は同じ関数を使うと 3 バリアントへの対応が同時に揃う。

### 3.4 「先頭 1 枚」の取得

Folder/ZipFile/PdfFile のサムネイルはそれぞれ別ロジックで「代表画像」を取ってくる:

| 容器 | 実装 | 「先頭」の定義 |
| --- | --- | --- |
| Folder | `folder_tree.rs` で再帰走査 | ソート順設定 (`folder_thumb_sort`) と深さ制限 (`folder_thumb_depth`) に従う |
| ZipFile | `zip_loader::read_first_image_bytes` | エントリ名の昇順で最初の画像拡張子 |
| PdfFile | PDF ワーカーでページ 0 を固定取得 | 常に `page_num = 0` |

ここは歴史的にバラバラに実装されていて、統一できていない。触るなら 3 箇所まとめて。

---

## 4. ZipSeparator の扱い

ZipSeparator は **UI 上の区切り線**なので、以下のように特殊扱い:

- クリック不可 / 選択不可
- サムネイルロード対象外 (LoadRequest を作らない)
- キーボードナビゲーションでスキップされる
- ソート時に境界として機能 (隣のグループに渡らない)

新しいキーボード操作や一括処理を追加する時、ZipSeparator をスキップするのを忘れないこと。

---

## 5. ZIP/PDF 対応を追加する時のチェックリスト

新機能が通常画像で動いたら、ZipImage / PdfPage でも動くか確認する。

- [ ] **GridItem::ZipImage で動くか** (バイト経由で処理できるか)
- [ ] **GridItem::PdfPage で動くか** (PDF ワーカー描画後の ColorImage で処理できるか)
- [ ] **DB のキーは正規化されているか** (path だけだと ZIP 内エントリを区別できない)
- [ ] **パスが存在しない項目** (ZipSeparator) を誤ってリストから引いていないか
- [ ] **パスワード付き PDF** で落ちないか (enumerate 段階で止まる可能性)
- [ ] **キャッシュキー** が他と衝突しないプレフィクスになっているか
- [ ] **サムネイル経路とフルスクリーン経路** の両方で対応しているか ([display-pipeline.md](display-pipeline.md))
- [ ] **フォルダ側サイドカー** のキーと整合するか (下記 §6 参照)

---

## 6. フォルダ側サイドカーの相対キー規則

`adjustment.db` / `mask.db` のバックアップとしてフォルダ直下に置かれる `mimageviewer.dat` は、
中のエントリを**フォルダ相対キー**で持つ (絶対パスだとフォルダ移動で意味が消えるため)。
サムネイル用キャッシュキーや DB キーとは別系統なので混同しないこと。

| GridItem         | サイドカー置き場       | 相対キー                                      |
| ---------------- | ---------------------- | --------------------------------------------- |
| `Image(p)`       | `p.parent()`           | `"{filename_lower}"`                          |
| `ZipImage`       | `zip_path.parent()`    | `"{zip_filename_lower}::{entry_name_lower}"`  |
| `PdfPage`        | `pdf_path.parent()`    | `"{pdf_filename_lower}::page_{n}"`            |

ZIP/PDF 用の相対キーは **ZIP/PDF ファイルの親フォルダ** に置かれたサイドカーに保存される。
つまり同じフォルダ内の複数 ZIP・PDF・bare 画像は 1 つのサイドカーファイルにまとまる。

新しい GridItem バリアントを足すときは `App::sidecar_folder` / `App::sidecar_relative_key` と
`sidecar::reconstruct_*_key` の対応を 3 バリアント (Image / ZipImage / PdfPage) と揃えて追加する。
片側だけ足すとインポートで復元されない。

詳細は [preset-and-adjustment.md §9](preset-and-adjustment.md) を参照。
