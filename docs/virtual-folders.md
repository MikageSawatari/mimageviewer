# 仮想フォルダ (ZIP / RAR / PDF) 処理

ZIP アーカイブ、直接閲覧できる RAR/CBR、PDF ドキュメントは「中身のページをフォルダ内のファイルに見立てて扱う」仮想フォルダとして実装されている。
通常画像ファイルとの処理分岐が多く、修正漏れが起きやすい。**ZIP/PDF 対応のある機能を触るときは必ずこのドキュメントを見る**。

---

## 1. GridItem バリアント

`grid_item.rs` の `GridItem` 列挙型は以下の 12 バリアント:

| バリアント | 発生元 | 中身 |
| --- | --- | --- |
| `Folder(PathBuf)` | 通常フォルダ | 実ファイルシステムのディレクトリ |
| `Image(PathBuf)` | 通常フォルダ内 | 画像ファイル |
| `Video(PathBuf)` | 通常フォルダ内 | 動画ファイル |
| `Audio(PathBuf)` | 通常フォルダ内 | 音声ファイル |
| `ZipFile(PathBuf)` | 通常フォルダ内 | ZIP アーカイブ (未展開)。`.zip` と別名 `.cbz` を含む (`folder_tree::is_zip_extension` で判定) |
| `PdfFile(PathBuf)` | 通常フォルダ内 | PDF ドキュメント (未展開) |
| `ConvertibleArchive { path, format }` | 通常フォルダ内 | RAR/7z/LZH 等。RAR は worker 判定で直接閲覧または ZIP 変換キャッシュ、7z/LZH は ZIP 変換キャッシュ経由で開く |
| `ZipImage { zip_path, entry_name }` | ZIP を開いた中 | ZIP 内の画像エントリ。`entry_name` はネストでも `"outer/ch01.zip/p.jpg"` のフルパス |
| `ZipDir { zip_path, dir_prefix, is_archive, representative }` | ネスト ZIP を開いた中 (v1.3.0) | 「入れる」子ディレクトリ / 内側アーカイブ。Enter で降りる。仮想コンテナで実パスなし |
| `PdfPage { pdf_path, page_num, content_type }` | PDF を開いた中 | PDF のページ (0-indexed) |
| `Stack { key, representative, count }` | ファイル名スタック | 複数画像を畳んだ集約セル。クリックでメンバーへドリルインする |
| `SearchContainer { path, kind, hit_count, representative }` | Ctrl+G 検索集約ビュー | ヒットを含む親フォルダ/ZIP を 1 セルで表現 (v0.8.0) |

通常フォルダの詳細表示 / 選択情報に出す `ページ数` は、ZIP / PDF に加えて画像のみフォルダ、
親 ZIP 内の子 ZIP・RAR (`ZipDir`)、直接閲覧 RAR/CBR (`ConvertibleArchive`) を対象にする。
`ZipDir` は外側 ZIP を開いたときの列挙済み `ZipTree` 部分木から画像数を数え、UI スレッドで
書庫を開き直さない。RAR/CBR は details meta worker 上の header scan で取得し、元ファイルの
mtime / size と画像認識 fingerprint を identity に catalog cache へ保存する。画像のみフォルダも
自動表示設定の ON/OFF とは独立して同じ worker / catalog 経路で判定する。

`Folder/Image/Video/ZipFile/PdfFile/ConvertibleArchive` は「外側」= 通常フォルダのリスト。
`ZipImage/ZipDir/PdfPage` は「内側」= 仮想フォルダのリスト。`SearchContainer` は Ctrl+G 専用ビュー。
同じリストに外側と内側が混在することはない。

通常グリッドの表示順は `Settings.grid_display_order` の 4 行割り当てで決まる。カテゴリは
実フォルダ (`Folder`) / アーカイブ類 (`ZipFile` / `PdfFile` / `ConvertibleArchive`、ZIP 内では
`ZipDir`) / 画像 (`Image` / `ZipImage` / `PdfPage` / `Stack`) / 動画・音声 (`Video` / `Audio`)。
同じ行へ割り当てたカテゴリは共通の `sort_order` で混在ソートし、空行は読み飛ばす。既定は
1 行目が実フォルダ + アーカイブ類、2 行目が画像 + 動画・音声。組み立ては
`grid_item::arrange_grid_items` を通常フォルダ、ZIP materialize、ファイル名スタック、
レーティング一覧、サブフォルダ展開から共有する。全文検索の flat result は対象外。

サブフォルダ展開では、再帰走査で見つけた ZIP/CBZ と PDF の**本体だけ**をそれぞれ
`ZipFile` / `PdfFile` 1 項目として同じ外側リストへ入れ、内部の `ZipImage` / `PdfPage` は
列挙しない。「画像のみのフォルダを本として扱う」が実効 ON の場合は、通常一覧のページ数判定と
同じ「子コンテナなし・認識メディアが画像だけ」という述語で該当フォルダを `Folder` 1 項目にする。
設定 OFF ではフォルダ項目へ集約せず、その中の `Image` を 1 枚ずつ表示する。

ただし、本として読むページ列は一覧整理用の `sort_order` から独立する。ZIP / CBZ / 直接閲覧
RAR / CBR / 変換キャッシュ ZIP のページと ZIP 内階層は、常にファイル名順
(`SortOrder::FileName`、Windows に近い自然順) で materialize する。画像のみの通常フォルダも
「画像のみのフォルダを本として扱う」が実効 ON のときは同じ順序を使う。通常フォルダ browse、
製本した本 (`Numeric`)、PDF の enumerate 順は従来どおりである。

### 製本への追加 (v1.7.0)

メニュー「製本 > 追加先の本に追加」、ツールバーの本棚「追加」、追加ショートカットは、
仮想フォルダ内ページも通常ページと同じ「コピー型スナップショット」として扱う。

- `GridItem::ZipImage`: グリッド追加で実効カラー補正が恒等かつ非破壊回転も無ければ `zip_loader::read_entry_bytes`
  で格納 bytes をそのまま抽出し、`0001_元名.ext` 形式で本フォルダへ保存する。非恒等の
  カラー補正または非破壊回転を焼き込む場合だけ worker で decode + encode する。
- `GridItem::PdfPage`: PDF ページは実ファイルとして存在しないため、表示/export と同じ画素取得経路で
  `CapturePixelWork` を作り、設定されたキャプチャ形式でエンコード保存する。
- 本フォルダ内のページ (`Pictures\mimageviewer\books\<本名>\NNNN_...`) は通常の
  `GridItem::Image` として表示されるが、場所ベースで「本ページ」と判定する。同じ本への
  再追加は拒否し、別の本への追加は許可する。タグ/★/補正/消しゴム/補正レイヤー/隠蔽/
  テキスト/切り取りは mIV 内部 DB の後段編集として許可し、並べ替え・本名変更・別本コピー/
  移動時にキーを追従する。回転はページ順/見開きとの意味がぶれやすいため抑制する。

### 本ブックマークの identity (v2.7.0)

本ブックマークは表示 index ではなく、コンテナとページの安定 identity を
`book_bookmarks.db` に保存する。対象と正本は次のとおり。

| 本の種類 | コンテナ | ページ identity |
| --- | --- | --- |
| 製本 / 画像のみ通常フォルダ | 本フォルダのユーザー視点パス | コンテナからの相対ファイルパス |
| ZIP / CBZ / 直接閲覧 RAR / CBR | アーカイブ本体 | 完全な `entry_name` |
| 変換閲覧 RAR / CBR / 7z / CB7 / LZH / LHA | `effective_folder()` が返す元アーカイブ | cache ZIP 内の完全な `entry_name` |
| PDF | PDF 本体 | 0-origin ページ番号 |

画像フォルダは相対ファイル名を正本にするため、途中へ画像を追加・削除して表示 index が変わっても
同じファイルを再解決できる。登録時 index は表示 / 高速化用 hint に限る。ZIP 系 entry は `/` 区切りへ
正規化して比較するが表示用表記は保持する。変換閲覧では cache ZIP を永続コンテナに使わない。
集約ビュー上の単独 `Image` は本の所属が確定しないため、元コンテナを開くまで対象外とする。
各行は安定 identity とは別に nullable な任意名称を持つ。名称の変更や解除はページ identity と
登録日時を変えず、空文字または空白だけの入力は名称なし (`NULL`) として保存する。

### ネスト ZIP のツリーナビ (v1.3.0)

ネスト ZIP (ZIP 内に内側 ZIP/サブフォルダ) は、**フラット展開 + 章区切りセルをやめ**、
`entry_name` の `/` 区切りでメモリ上にツリー (`src/zip_tree.rs` の `ZipTree`) を組み、
現在階層だけを `materialize_level` で表示する。子コンテナは `ZipDir` セルになり、
Enter/ダブルクリック/ゲームパッド/Ctrl+↑↓(本またぎ)/Backspace でツリー内を移動する
(状態は `ZipNavState`、`App.zip_nav`)。**`entry_name` は一切変えない**ので回転/補正/
レーティング/タグ等の永続キーは不変 (DB 移行不要)。見開きペアリングは `self.items` が
現在の本のページだけになるため本ごとに自動リセットされる。詳細は
[nested-zip-tree-plan.md](nested-zip-tree-plan.md)。

### 拡張子 → 扱いの対応 (comic-book 別名を含む)

`scan_directory` (`src/app.rs`) はファイル拡張子で次のように分類する:

| 拡張子 | 扱い | GridItem | 根拠 |
| --- | --- | --- | --- |
| `.zip` / **`.cbz`** | ネイティブ ZIP (変換不要で最速ブラウズ) | `ZipFile` | `folder_tree::is_zip_extension` |
| `.pdf` | ネイティブ PDF | `PdfFile` | `ext == "pdf"` |
| `.rar` / **`.cbr`** | 非ソリッドかつ入れ子なしなら直接閲覧。それ以外は ZIP 変換キャッシュ | 外側は `ConvertibleArchive{Rar}`、内側は `ZipImage` を再利用 | worker の `rar_loader::inspect_for_direct_read` + `zip_loader` 末端 dispatch |
| `.7z` / **`.cb7`** | クリックで ZIP 変換 | `ConvertibleArchive{SevenZ}` | 同上 |
| `.lzh` / `.lha` | クリックで ZIP 変換 | `ConvertibleArchive{Lzh}` | 同上 |

comic-book 別名 (`.cbz`/`.cbr`/`.cb7`) は実体フォーマットと同一扱い。**CBZ は ZIP と同じ
ネイティブ閲覧経路**に乗るので `is_zip_extension` を見ている箇所すべて (`is_virtual_folder` /
`load_folder_with_scan` の分岐 / `build_and_save_one` のサムネ catch-up / ネスト ZIP 再帰) を
通す必要がある。`.cbt` (tar) / `.cba` (ace) は未対応。

### 入れ子アーカイブの再帰展開 (v1.3.0)

アーカイブの**中に別のアーカイブ**が入っているケースの扱い:

| 外側 | 入れ子 | 挙動 |
| --- | --- | --- |
| 非ソリッド RAR/CBR | なし | **直接閲覧**。`GridItem::ZipImage { zip_path: 元 RAR, entry_name }` を使い、永続 ZIP キャッシュは作らない |
| ソリッド RAR/CBR、または入れ子あり RAR/CBR | 任意 | 従来どおり ZIP 変換キャッシュを作成して閲覧 |
| ZIP/CBZ | ZIP/CBZ のみ | 従来どおり**無変換**でネスト ZIP ツリー表示 (`entry_name` 不変、一般ケース最速) |
| ZIP/CBZ | RAR/CBR/7z/CB7/LZH/LHA を含む | 列挙 (`enumerate_image_entries_detailed`) が `has_foreign_archives` フラグを立て、`finalize_zip_enumerate` → `offer_zip_foreign_archive_conversion` が **`ArchiveFormat::Zip` で変換ダイアログを提案**。キャンセルしても非 ZIP の中身が見えないだけでツリー閲覧は継続できる |
| RAR/7z/LZH | 任意のアーカイブ | 変換時に**再帰展開** (従来は skip で中身が消えていた) |

変換器 (`archive_converter`) の再帰展開は `ConvertCtx` + `expand_{zip,rar,7z,lzh}` で行い、
入れ子は一時ファイル化 (`tempfile`) してから対応リーダーで開く。出力 cache ZIP には
**フラットな literal エントリ名** (`"books/inner.rar/p01.jpg"`) で書くので、ツリー表示は
`/` split だけで入れ子アーカイブが「本」ノードになる。要点:

- 深さ上限 `MAX_NESTED_ARCHIVE_DEPTH` (=8)。閲覧用 cache 変換は、超過・壊れた・
  パスワード付き入れ子をログ + skip して従来の寛容な表示用変換を維持する。
- 「変換 > ZIP ファイルに変換」の sibling / batch 経路は
  `ConvertOptions { no_clobber: true, verify: true }` を使う。`verify` は
  `ConvertCtx.strict` も兼ね、入れ子展開失敗、アーカイブエントリ読取失敗、
  LZH の未対応圧縮方式 / CRC 不一致、深さ超過を伝播する。変換完了後は一時 ZIP を
  publish 前に開き直し、画像数一致と全画像の終端 / CRC を検証する。どれか失敗すれば
  `TmpCleanup` が一時 ZIP を削除し、同名 `.zip` は残さない。
- **読み戻し**: cache ZIP の literal な `".zip/"` 入りエントリ名は、`read_entry_bytes` の
  **exact-name fallback** (NESTED_CACHE 未ヒット時にフルネーム直接読みを先に試す) で
  解決される。`".rar/"` 等の境界はそもそも分割対象でないので直接読みになる
- 変換済み ZIP の再オープンは `load_zip_as_folder` 冒頭の `try_archive_cache_lookup` で
  cache ZIP へ振り替え (`archive_source_override` = 元 ZIP)。mtime/size 不一致なら miss
  して通常経路 → 列挙で再検出 → 再変換提案
- ループガード: cache ZIP 自身 (`archive_cache::is_under_cache_root`) と
  `archive_source_override` 表示中は提案しない
- `zip_tree::segment_is_archive` は `.rar/.cbr/.7z/.cb7/.lzh/.lha` セグメントも
  アーカイブバッジにする (展開済み cache のフラットパス由来)
- 実機サンプル生成: `python scripts/make_nested_archive_test.py` (7z 系は 7-Zip、
  rar 系は WinRAR が必要)

**受容済みトレードオフ (設計時合意、Codex P2 で再確認)**: 非 ZIP 入れ子を含む ZIP を
変換すると、ページの永続キーが `元ZIP::entry` から `キャッシュZIP::entry` に変わる
(回転/補正/レーティング/タグの紐付け基準が変換後キーに移る)。変換**前**にその ZIP で
付けた編集は変換後ビューに現れない。RAR 等の入れ子を含む ZIP は元々中身が見えて
いなかった = 編集対象になりにくく、実害は限定的と判断して受容。純 ZIP-in-ZIP は
無変換のままなので一般ケースの entry_name 不変は維持される。

### 変換アーカイブ閲覧中の current_folder と「ユーザー視点パス」の二重化

ソリッド・入れ子あり・暗号化 RAR/CBR と 7z/CB7/LZH/LHA を開くと無圧縮 ZIP に変換し
(`archive_cache\<hash>\book.zip`)、以降はそれを通常 ZIP として開く。このとき **`current_folder` は
キャッシュ ZIP を指す**が、ユーザー視点 (address bar / BS の親 / 次回起動の復元) では
**元アーカイブ** を見せる必要がある。両者は `archive_source_override` で橋渡しする
(`open_archive_via_cache` が set、`effective_folder()` =
`archive_source_override.or(current_folder)`)。

直接閲覧 RAR/CBR は `current_folder` 自体が元アーカイブを指し、`archive_source_override` は
使わない。現在開いているコンテナの判定には `is_open_as_container` を使い、静的な一覧分類用
`is_virtual_folder` は RAR を false のまま保つ。ページ DB / sidecar キーは常に
`ZipImage.zip_path::{entry_name}` で作るため、直接閲覧は `{元 RAR}::entry`、従来の変換
キャッシュ閲覧はリリース済みデータと互換の `{cache ZIP}::entry` になる。両経路のキー parity
は既存データの明示 migration が必要なので、ここでは追求しない。

新しく「現在のフォルダ」を永続化・ナビゲーションに使う箇所を足すときは、`current_folder` を
直接使わず **`effective_folder()` を使う** こと。過去に漏れた実例:

- **`last_folder` (次回起動の復元先)**: `start_loading_items` の保存で `source_path` (= キャッシュ
  ZIP) を入れていたため、再起動で `archive_cache\..\book.zip` を素の ZIP として開き、address に
  キャッシュパスが漏れた。`effective_folder()` を保存するよう修正。
- **起動時復元のルーティング**: `App::update` 初回フレームは `load_folder` ではなく
  `load_folder_or_convert_archive` を通す。元アーカイブパスを渡すとキャッシュ参照 →
  `open_archive_via_cache` で開き直し、キャッシュが無ければ変換ダイアログを出す。
  `archive_file_handling` が `Convert` の場合は Confirm 画面だけを省略して自動変換する
  (変換中の進捗 / パスワード入力 / エラーは引き続き表示)。`Ignore` の場合は
  キャッシュヒットも含めて変換アーカイブとして開かない。
- **`resolve_openable_path` / `is_convertible_archive_path`**: 起動復元・アドレスバー入力で
  変換アーカイブを「開けるパス」として返す (ネイティブ ZIP/PDF を判定する `is_virtual_folder`
  とは別関数)。これが無いと変換アーカイブのパスが親フォルダに丸められて本を開き直せない。
- **Ctrl+↑↓ / 兄弟移動**: `archive_file_handling` が `Ignore` 以外なら、
  `folder_should_stop` と `sorted_subdirs` は RAR/CBR/7z/CB7/LZH/LHA も ZIP/PDF と同じ
  コンテナ候補として扱い、`App::load_folder_nav_target` から
  `load_folder_or_convert_archive` へ渡す。`Ignore` では一覧スキャンと同じく候補から外す。
  通常フォルダ一覧には分割 RAR の後続パートも実ファイルとして表示するが、フォルダ横断では
  同じ本へ何度も停止しないよう後続パートを候補から外す。この判定は DFS worker で指定された
  ファイルの RAR header `volume_info()` を確認する。`unrar` の `is_multipart()` /
  `first_part()` は `Vol.2.rar` のような通常ファイル名にも一致するため、名前判定は header を
  確認する候補の絞り込みにだけ使う。
- **分割 RAR の表示とオープン**: 通常フォルダ一覧の UI 側 scan は RAR header I/O を行わず、
  `.rar` / `.cbr` をすべて表示する。後続パートを選んだ場合は open worker が
  `volume_info() == Subsequent` を確認した後だけ先頭パートへ正規化し、先頭を選んだ場合と
  同じ本を開く。inspection / enumerate / read / convert、サムネイル用の直接読込／変換cache
  参照も同じ正規化境界を使う。`None` の単体 RAR は、外側の名前が `Vol.2.rar` 等でも
  選択されたファイル自身を開く。一覧の遅延フィルタや判定完了後の再読込は行わない。
- **フルスクリーン Ctrl+↑↓ 中の未変換アーカイブ**: 変換キャッシュが無くても
  `archive_file_handling == Convert` により確認なしで変換できる場合は、直前ページの
  holdover と nav lock を維持し、変換完了後にキャッシュ ZIP を開いてフルスクリーンへ
  復帰する。確認ダイアログ、パスワード入力、エラー、キャンセルなどユーザー操作が
  必要な状態へ入った場合は復帰予約を破棄し、従来どおり一覧/ダイアログ操作へ戻す。

---

## 2. 仮想フォルダの展開

### 2.1 ZIP / 直接閲覧 RAR を開く (`App::load_zip_as_folder`)

```
1. `zip_loader::enumerate_image_entries(zip_path)`。RAR/CBR パスでは末端 dispatch により
   `rar_loader` の listing を使う
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
   - Windows ツール由来などで ZIP 内エントリ名に `\` が含まれる場合も、列挙時は
     `/` に正規化する。読み戻し (`read_entry_bytes` / `read_entry_from_archive`) では
     `/` で見つからない場合だけ `\` 名も試し、古い/非標準寄り ZIP との互換性を保つ。
   - 日本語 Windows ツール由来で UTF-8 flag が立たず CP932 の raw filename を持つ ZIP は、
     `zip` crate 既定の CP437 decode では文字化けするため、列挙・先頭画像・読み戻しの
     共通 helper で UTF-8 → CP932 → crate 既定名の順に解釈する。
   - **v0.7.0 以降: 外側 ZIP 内の .zip エントリは再帰展開され**、
     entry_name は "chapters/ch01.zip/page01.jpg" のように親 ZIP 名を含む
     パスになる。内側 ZIP バイト列は zip_loader 内の LRU キャッシュ (256MB) に
     保持され、後続の read_entry_bytes で再展開せずに参照される。

2. **(v1.3.0〜) `entry_name` の `/` 区切りでツリー (`zip_tree::ZipTree`) を構築**
   - `.zip`/`.cbz` 境界も構造上はただのディレクトリ階層 (`entry_name` 中で既に `/` 区切り)。
   - 旧実装の「サブディレクトリで BTreeMap グループ化 + 章区切りセル挿入 +
     全エントリをフラットに ZipImage」は廃止。区切り用の疑似バリアントも v2.5.0 で撤去した。

3. ルート階層だけを `materialize_level` で items 化:
   - 冗長ラッパー (画像 0・子 1) は `collapse_redundant` で自動降下
   - 子コンテナ → `GridItem::ZipDir`、直下画像 → `GridItem::ZipImage` (コンテナ先・画像後)
   - image_metas に (mtime, uncompressed_size) を記録 (ZipDir は代表画像の meta)
   - `existing_keys` は `tree.all_cache_keys()` (全階層の entry_name + zipdir: キー) +
     pinned ZipDir の `#pin` キーを含める (`delete_missing` の存続基準)。ZipDir の
     pinned key は、直接画像 source と `FolderPinSource::ZipDir` の cascade leaf の両方を
     解決して追加する。pin の DB キーは **literal prefix を `collapse_redundant` した
     実効 prefix の合成パス** で、lookup は `lookup_many` 1 回に束ねる (per-dir 逐次 lookup
     は UI スレッドを dirs 数比例でブロックするため)
   - 階層内の移動 (enter/back/dfs_step) は `zip_nav_show_current_level` の軽量経路
     (`install_new_items`、`start_loading_items` を通さない)
   - **本ごとピン (Model B) のキー規則**: set 側は `App::pin_container_key()` =
     ルート表示なら `zip_path` (= 外側 ZIP の代表。親フォルダの ZipFile セルが引くキー /
     v1.2.x フラット UI のピンと互換)、本の中なら `zip_path` + 実効 prefix の合成パス。
     RAR/7z/LZH 変換キャッシュ ZIP を閲覧中は `zip_path` の代わりに
     `archive_source_override` (= 元アーカイブ) を root にする。これにより、変換後ビューで
     付けたピンが親フォルダの `ConvertibleArchive` タイルにも反映される。
     lookup 側 (`make_load_request` の ZipDir 分岐) は cell の literal prefix キーで
     `folder_pin_map` を引くが、`refresh_folder_pin_map` が実効 prefix で DB を引いた
     結果を literal キーへ **alias 登録**するので、単一ラッパー本でも一致する。
     ZipDir セル自体を `P` した場合は `source_kind=zipdir` に実効 prefix を保存し、
     通常フォルダの Folder source と同じ cascade で子の pin を辿る。
   - **★固定 (snapshot) との相互作用**: snapshot は items を `start_loading_items` を
     通さず差し替えるため、`activate_snapshot` が `zip_nav` を **take して
     `SnapshotState::saved_zip_nav` へ退避**し、at_origin の解除で `saved_items` と対で
     復元する。`snapshot_return_to_list_view` も (snapshot 内から開いた子 ZIP の)
     zip_nav を破棄する。退避しないと snapshot 表示中の BS が `zip_nav_back()` に
     落ちて stale な ZIP 階層を snapshot ビューへ上書きする
```

非同期。列挙 (`enumerate_image_entries`) は `d1a6e99f` 以降ワーカースレッドで行い
(約 1100 エントリで UI を 2.3 秒ブロックした実害があったため)、ツリー構築 + materialize は
純ロジックなので受信後に UI スレッドで行う。内側 ZIP バイト列は zip_loader 内の LRU
キャッシュに保持され、後続の read_entry_bytes / 深い階層のサムネ読み込みで再展開せず参照される。

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

### 2.3 自動 1 ページ目フルスクリーン (`auto_fullscreen_zip_pdf`)

環境設定のフル機能ウィンドウで「本をページ表示で開く」が ON、または複数ウィンドウモードが
ON のとき、**grid から ZIP/PDF を Enter / ダブルクリックで開く、または
起動引数 / SendTo / 外部ファイラから ZIP/PDF/対応アーカイブを直接開くと、ページ一覧
(L2) を経由せずページを直接フルスクリーン (L3) で開く**。追加設定
`auto_fullscreen_image_folders` が ON の場合は、表示上の項目が通常画像だけのフォルダも
同じくページ一覧をスキップして開く。MangaMeeya 風の「本棚 → 本を開く → 読む → 閉じて本棚」
フロー。開く位置は `book_open_resume` (続きから / 先頭から) に従う。
複数ウィンドウモードでは `auto_fullscreen_zip_pdf` の保存値は書き換えず、実効値だけを ON として扱う。
このモードではメイン一覧をページ一覧へ切り替える明示コマンド「ページを開く」/「一覧を開く」は
利用しない。キー、右クリックメニュー、リング、マウスジェスチャのいずれから発火しても、共通の
`open_grid_container_with_mode` 入口で理由をトースト表示して no-op とし、RAR 変換開始や読書履歴更新を
含む副作用を起こさない。通常 ZIP、直読み RAR、変換対象 RAR/7z/LZH でこの判定を分岐させない。

- グリッド側の open (`ui_main` ダブルクリック / `handle_keyboard` Enter) で
  `pending_auto_fs_open` を立てる (ZipFile/PdfFile/ConvertibleArchive、追加設定 ON の
  Folder)。Folder は `load_folder_with_scan` 後に表示項目が 1 件以上かつ全て `Image`
  だった場合だけ予約を消費してフルスクリーンを開き、サブフォルダ / 動画 / ZIP/PDF /
  変換アーカイブが混ざる場合は一覧表示に戻す。
- 起動パス / 既存インスタンス転送 (`open_startup_path`) は、解決後の openable が
  ZIP/CBZ/PDF または RAR/CBR/7z/CB7/LZH/LHA、または追加設定 ON の Directory のときだけ
  同じ明示オープン意図を渡す。
  変換アーカイブは `archive_file_handling` に従い、`Ask` / `Convert` の場合だけ
  キャッシュヒット / 変換確認 / 自動変換の後で同じ deferred fullscreen 経路に入る。
  `Ignore` では明示オープンの自動フルスクリーン予約も立てない。
- `load_zip_as_folder` / `load_pdf_as_folder` がこれを `mem::take` し、enumerate 完了で
  先頭画像を開く既存の遅延機構 `fs_nav_after_pdf_enumerate` (`DeferredFsReopen`) に
  載せ替える。Ctrl+↑↓ フォルダナビと同じ consume 経路 (`finalize_zip_enumerate` /
  `poll_pdf_enumerate`) で `find_fullscreen_nav_target_filtered` が先頭ページを開く。
  `DeferredFsReopen` は明示 open 由来かどうかも保持し、detached viewer では enumerate
  完了後の window focus と「毎回新しいウィンドウ」判定へ渡す。Ctrl+↑↓ 由来の reopen は
  focus を奪わない。
- 通常フォルダは enumerate 待ちが無いので、`load_folder_with_scan` が既存の走査結果から
  画像のみ判定を行い、`book_open_resume` に従って保存済みページまたは先頭画像を開く。
- パスワード付き PDF では、grid / 起動パス / SendTo 由来の明示オープン予約だけを
  入力ダイアログ後まで維持する。Ctrl+↑↓ 由来の deferred reopen は従来どおり破棄する。

### 退出ルーティング = 「設定で固定分岐」(一時フラグなし)

「どう入ったか」を覚える一時フラグ (旧 `fs_zip_auto_opened` / `auto_opened_container`) は
**廃止**。階層 (L1 ファイル一覧 > L2 ページ一覧 > フルスクリーン) の概念は変えず、
モードB はあくまで **L2 を飛ばすショートカット**として扱う。判定は設定
`effective_auto_fullscreen_zip_pdf()` と「いまコンテナ (ZIP/PDF/変換アーカイブ由来のキャッシュ ZIP、
または追加設定 ON の画像のみ通常フォルダ) の中か」だけ。

- **分岐するのは 2 箇所だけ:**
  1. **L1 でファイルを開く** (Enter/ダブルクリック): モードA → L2 ページ一覧 / モードB →
     フルスクリーン (L2 スキップ)。
  2. **フルスクリーンで <kbd>Esc</kbd> / <kbd>Enter</kbd> / 右クリック**
     (`handle_fullscreen_close_request`): `auto_open_for_current_container()`
     (= 設定B & `current_folder` が ZIP/PDF/変換アーカイブ由来のキャッシュ ZIP、または
     追加設定 ON の画像のみ通常フォルダ & 非検索) が真なら親一覧 (L1) へ直帰
     (`pending_return_to_parent` を立て、`App::update` の入力ナビ合流点が
     `AddressBarNav::Direct(parent)` を発行。L2 を 1 フレームも見せない)。偽なら 1 段だけ
     `close_fullscreen` (モードA のコンテナ → L2、通常画像 → 親グリッド)。
- **<kbd>Backspace</kbd> は両モード共通で階層を 1 段戻す** (分岐なし):
  - フルスクリーン (ZIP/PDF/変換アーカイブ由来ページ、または追加設定 ON の画像のみ通常フォルダ)
    → L2 ページ一覧 (`FsKeyAction.close_to_page_list` → `close_fullscreen`。`current_folder` が
    コンテナ/通常フォルダのまま閉じるので L2 が出る)。
  - L2 ページ一覧 → L1 (通常の grid BS = 親フォルダ)。
    detached viewer が開いている場合でも、この BS は仮想フォルダ退出なので active viewer を
    passive snapshot として残さず閉じる。
  - 設定Bのまま L2 ページ一覧から再度 Enter/ダブルクリックでページ表示した場合も、
    Esc/Enter/右クリックは設定どおり L1 へ直帰する (「直接オープン由来」フラグは持たない)。
- **Ctrl+↑↓ フォルダナビで ZIP/PDF/変換アーカイブコンテナへ移っても**、退出は同じく設定で決まる
  (連続読書)。`reopen_fullscreen_after_folder_nav_load` はフラグを立てる必要がなく、
  移動先が ZIP/PDF/変換アーカイブなら次の本も Esc→親一覧 / Backspace→ページ一覧 になる。
  画像のみ通常フォルダも追加設定 ON なら同じ出口になり、追加設定 OFF または画像以外が混ざる
  通常フォルダへ移った場合は通常の出口に戻る。検索 (Ctrl+S/G) 中は
  Esc が検索を抜ける想定外を避けるため `auto_open_for_current_container()` が偽になり適用しない。

---

## 3. 分岐ポイント (修正漏れ要注意)

### 3.1 サムネイル生成の分岐

`thumb_loader.rs::process_load_request` 内で `LoadRequest` のフィールドを見て分岐:

| GridItem | `zip_entry` | `pdf_page` | `cache_key_override` | サムネ取得方法 |
| --- | --- | --- | --- | --- |
| Image | None | None | なし | ファイル直接デコード |
| Folder | None | None | `folderthumb:auto-vN:{sort}:d{depth}:{dirname-or-fullpath}` | 再帰的に代表画像を探してデコード |
| ZipFile | None | None | `zipthumb:{filename}` | `zip_loader::read_first_image_bytes` で先頭画像 |
| PdfFile | None | Some(0) | `pdfthumb:{filename}` | PDF ワーカーでページ 0 をレンダリング |
| ConvertibleArchive | None または Some(entry) | None | `archivethumb:{format}:{filename-or-fullpath}` | 有効な変換キャッシュ ZIP があれば、その ZIP から先頭画像またはピン画像を読む。キャッシュ未作成/失効時は LoadRequest なしでアイコン表示 |
| ZipImage | Some(entry) | None | なし (entry が自動キー) | ZIP からエントリバイト → decode → bytes から EXIF Orientation 適用 |
| ZipDir | Some(representative) または None (`ZipDirRepresentative`) | None | `zipdir:{dir_prefix}` | 通常は部分木代表 entry を直接読む。ZipDir source pin は `zip_dir_prefix` を worker に渡し、同じ sort で部分木代表を選び直す |
| PdfPage | None | Some(page) | `pdf_page_cache_key(page)` | PDF ワーカーでそのページをレンダリング |

`ConvertibleArchive` の cache ZIP 対応表 (`App.converted_archive_cache_paths`) は
`install_new_items` で現在 items から path/mtime/size だけを snapshot し、SQLite `peek` と
cache ZIP の `exists()` は `ConvertedArchiveCachePathsPending` worker で解決する。worker 完了前は
`make_load_request` が `None` を返し、サムネは Pending のまま次の repaint で再試行される。
この経路で `make_load_request` から `archive_cache.db` やファイルシステムを直接触らないこと。

**キャッシュキーの命名規則**を勝手に変えないこと。Folder 自動代表は選定
アルゴリズム世代を明示して、意図したロジック変更時だけ既存キャッシュを外す。

#### 3.1.1 親コンテナの代表サムネピン (folder thumb pin、v0.9.x)

ユーザーが「このフォルダ / ZIP / PDF の代表サムネは中の特定アイテム」を手動で指定
できる機能。優先順位は **手動ピン > 自動代表選定 > フォルダ/ZIP/PDF アイコン**。

- **DB**: `%APPDATA%/mimageviewer/folder_thumb_pins.db` (`folder_thumb_pins.rs`)。
  schema は `(container_key, source_kind, source_rel, source_entry, source_page)`。
  container_key は `path_key::normalize_keep_drive` で正規化。`source_kind` は
  `image` / `video` / `folder` / `zipfile` / `pdffile` / `zipentry` / `zipdir` /
  `pdfpage` で、`zipdir` は `source_entry` に ZIP 内 prefix を保存する。
- **解決パス**: `make_load_request` 経由で `apply_folder_thumb_pin` が pin map
  (= `App::folder_pin_map`、load 開始時に `lookup_many` で一括取得) を引き、
  pin があれば `LoadRequest` を target アイテム用の形 (path/zip_entry/pdf_page/
  resolve_override) に書き換える。
- **キャッシュキーの例外規則**: pin 適用後は `{base_key}#pin:{source_id}` の形に
  なる。直接 leaf の `source_id` は kind/rel/entry/page/mtime/size を `|` 連結した
  compact 表現。cascade 時は途中コンテナも含む経路 identity を SHA-256 で固定長にして
  `cascade:{route_hash}:{leaf_source_id}` とする。pin の付け替え、途中コンテナ、target
  ファイルの mtime/size 変化で自動的に変わり、別 ZIP/PDF の同名ページでも古い WebP を
  catch しない。`existing_keys` には base + pinned 両形を入れて `delete_missing` の
  巻き添え削除を防ぐ (`folder_thumb_existing_keys_for`)。
- **Video ピンの特殊経路**: pin source が動画の場合、`thumb_loader` 側で動画
  Shell API を直接使うとピン位置の WebP が出ないため、folder load 時に
  `seed_folder_video_pin_thumbs` が `video_pins` DB から WebP を読んで pinned key
  下に catalog + cache_map をミラー seed する。worker は通常の cache_hit 経路で
  動画フレームを取り出す。WebP が無い / 失敗時は seed を skip / 旧 seed 行を purge して
  worker を folder auto-pick fallback (`resolve_folder_thumb_image`) に落とす。
  video pin は `skip_cache = false` 固定で idle quality-upgrade の対象外 (WebP IS the source)。
  - **「動画内 PIN 必須」仕様** (Codex post-merge P2 → ユーザー合意): video source の
    folder pin は `try_set_folder_thumb_pin_with_video_guard` が **set 時に `video_pins.db`
    の WebP 有無をチェック**し、無ければトーストで案内して set を拒否する。sidecar
    `image::open` / Shell API 抽出を seed で同期実行すると、動画 pin 付きフォルダ複数 +
    Shell 遅延でフォルダ移動が固まるため。これにより seed は軽い DB→DB コピーのみに
    なり UI スレッドのヒッチが消える。動画を folder pin したいユーザーは先にフルスクリーン
    で `P` キー / HUD ピンボタンでフレームを保存する。
- **Folder / ZipFile / PdfFile / ZipDir source の cascade 解決** (v0.9.x+ / ZipDir は
  v1.3.x+): pin source がサブフォルダ、ZIP、PDF、ZIP 内コンテナの場合、
  `resolve_pin_target_cascaded` が `folder_thumb_pins.db` を順に lookup して、子コンテナが
  持つ代表 pin の最終 leaf まで辿る。例: A が B (Folder) を pin、B が C (Image) を pin
  → A の親グリッドでの A のタイルは C を表示する。同様に、親フォルダが book.zip /
  book.pdf を pin し、その ZIP/PDF が page 2 を pin していれば、親も page 2 とその編集
  preview を表示する。子側に pin がなければ従来どおり ZIP の先頭画像 / PDF のページ 0
  へフォールバックする。ZIP 内でも、外側 root + ZipDir prefix を合成した仮想 container
  key で lookup し、子の `ZipEntry` / `ZipDir` pin を同じ規則で辿る。
  cascade の段数上限は `Settings.folder_thumb_depth` (規定 3、範囲 0〜10) に揃える
  (= `resolve_folder_thumb_image` のサブフォルダ探索深度と同じ仕様)。
  - サイクル検出: `visited` HashSet で normalize_keep_drive 済みパスを記録。A↔B 循環は
    2 周目で検知して停止し、その時点の Folder を `FolderRepresentative` で auto-pick する。
  - 0 を指定すると cascade は無効化される (= 旧 Phase B 互換挙動)。
  - `pinned_key` は **cascade 経路 hash + leaf source_id** を埋める。連鎖途中のコンテナや
    pin が書き換わっても cache key が変わるので、stale cache を catch しない。
  - `folder_thumb_existing_keys_for` も同じ cascade を実行して existing_keys に leaf
    pinned_key を含める。これをしないと delete_missing が cascade 由来の cache 行を
    毎ロード掃除してしまう (Phase D 後のバグ修正 / 二重実装で識別)。
- **container/source の compat check**: `pin_source_compatible_with_container`
  で DB 汚染や将来の schema 拡張による不整合 (ZipFile container に PdfPage source 等) を
  弾き、`base_req` にフォールバックする。
- **UI**: アドレスバー 📌 ボタン (左クリック toggle / 右クリック解除) + 右クリック
  メニュー「📌 代表サムネに固定 / 解除」。`Settings.show_address_bar_folder_pin` で
  ボタン表示を切替。Ctrl+G アグリゲートビュー / 空フォルダ (idx=usize::MAX) では UI を
  出さない。RAR/7z/LZH 変換キャッシュの drill-down は `zip_nav` が生きていれば
  `archive_source_override` を root にしてピン可能。`zip_nav` が無い中途半端な override
  状態だけは dead pin 回避のため UI を出さない。親フォルダで ConvertibleArchive
  アイテム自体を選択した場合は、まだ中のエントリを選べないため disabled + tooltip
  「変換後に設定可能」。
- **書き換え反映経路**: `set_folder_thumb_pin` / `remove_folder_thumb_pin` が DB
  書き込み + `folder_pin_map` 更新 + `folder_thumb_pin_dirty = true`。
  `consume_folder_thumb_pin_dirty` が **`update` 内 (fullscreen 中以外)** および
  **`close_fullscreen`** で `folder_history` を消して `load_folder` を再実行することで、
  pin の cache key 変化が次フレームのグリッド描画に反映される。fullscreen 中は
  load_folder が close_fullscreen を呼んでしまうので、抜けるまで dirty を保留する。

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
        // bytes から EXIF Orientation 適用、アニメーション不可
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

- GIF / APNG アニメーション (fs_animation がパス API)
- ZIP 内 RAW/WIC 系の Orientation 読み取り (bytes 版は rexif で読める EXIF のみ。JPEG などは自動回転される)

WIC デコードは `wic_decoder::decode_to_dynamic_image_from_bytes` でバイト列から
直接デコードできるため、ZIP 内の HEIC/AVIF/JXL/TIFF/RAW も開ける
(対応コーデックがインストールされていれば)。サムネイル・フルスクリーン両方の
ZIP エントリ経路で `image::load_from_memory` 失敗時のフォールバックとして使われる。
JPEG など EXIF Orientation を持つ ZIP 内画像は、サムネイル・フルスクリーンとも
エントリ bytes から向きを読んで正立表示する。

### 3.3 回転 / 補正 / 消しゴムマスクのキー

すべての DB は以下の正規化キーで保存:

- **Image**: ファイルパス (小文字 + `\` → `/`)
- **ZipImage**: `{zip_path 正規化}::{entry_name 小文字}` (`::` 区切り)
  - ネスト ZIP の entry_name は `"chapters/ch01.zip/page01.jpg"` 形式。
    外側 ZIP パスと合わせれば DB 内で一意になる。
- **PdfPage**: `{pdf_path 正規化}::page_{page_num}`

新しい永続ストレージを追加する時は、`App::page_path_key` と `adjustment_db.rs` の
`normalize_path` / `zip_entry_key` 生成に揃えること。`rotation_db.rs` も正規化済みページキーを
そのまま保存できる API を持つ。**キー規則がズレると ZIP/PDF の回転や補正が保存されない**。

`rating_db.rs` はページ単位 (画像 / ZIP 内画像 / PDF ページ) とコンテナ (フォルダ / ZIP / PDF
本体 / RAR・7z・LZH 等の変換前アーカイブ / ZipDir) の両方を同じテーブルに格納する。
キーは `App::rating_path_key` 経由:
- ページ単位は `App::page_path_key` と同じ (`adjustment_db::normalize_path` 形式、ZipImage は
  `::entry`、PdfPage は `::page_N` 区切り)
- コンテナは `normalize_path(path)` のみで、`::` セパレータが付かない。
  この構造により「ZIP ファイルへのコンテナ★」と「その ZIP 内エントリへのページ★」が同じ
  DB 内で衝突せずに共存できる。
- RAR/7z/LZH を変換キャッシュ ZIP として開いているときのコンテナ★は、cache ZIP ではなく
  `archive_source_override` (= 元アーカイブ) を root にする。親フォルダの
  `ConvertibleArchive` セルに F1〜F6 で付けた★と、変換後ビュー root の Shift+F1〜F6 は
  同じキーを共有する。ZipDir も `元アーカイブ + literal prefix` の合成キーを使う。
新規ページ単位 DB を追加する際は `page_path_key` を使い、コンテナ単位は
`normalize_path(path)` を直接使う。

### 3.4 「先頭 1 枚」の取得

Folder/ZipFile/PdfFile/ConvertibleArchive のサムネイルはそれぞれ別ロジックで「代表画像」を取ってくる:

| 容器 | 実装 | 「先頭」の定義 |
| --- | --- | --- |
| Folder | `thumb_loader::resolve_folder_thumb_image` で再帰走査 | cache miss 時に、グリッドのブロック順に揃えてサブフォルダを `folder_thumb_sort` で先に辿り、見つからなければ直接画像を同じ sort で選ぶ。深さは `folder_thumb_depth` |
| ZipFile | `zip_loader::read_first_image_bytes` | エントリ名の昇順で最初の画像拡張子 |
| PdfFile | PDF ワーカーでページ 0 を固定取得 | 常に `page_num = 0` |
| ConvertibleArchive | 変換キャッシュ ZIP に対して `zip_loader::read_first_image_bytes` | 有効な `archive_cache.db` 行がある場合のみ ZIP と同じ。未変換/失効時はアイコン |

Folder 自動代表の cache key には自動選定アルゴリズム版・`folder_thumb_sort`・
`folder_thumb_depth` を含める。キャッシュヒット時は表示速度を優先して毎回の
再スキャンはしないが、選定ロジックや設定が変わったときは古い自動代表 WebP を
読まずに再生成される。特定画像を厳密に使いたい場合は folder thumb pin を使う。

ここは歴史的にバラバラに実装されていて、完全には統一できていない。触るなら
3 箇所まとめて確認する。

---

## 4. 旧区切り疑似アイテムの撤去

旧フラット ZIP 表示で使っていた `GridItem::ZipSeparator` は、v1.3.0 以降の本番経路では
生成されなくなり、v2.5.0 で型・描画・ナビ・検索・テストを含めて完全撤去した。
現在の ZIP 一覧は `ZipDir` と `ZipImage` だけで階層とページを表現する。

---

## 5. ZIP/PDF 対応を追加する時のチェックリスト

新機能が通常画像で動いたら、ZipImage / PdfPage でも動くか確認する。

- [ ] **GridItem::ZipImage で動くか** (バイト経由で処理できるか)
- [ ] **GridItem::PdfPage で動くか** (PDF ワーカー描画後の ColorImage で処理できるか)
- [ ] **DB のキーは正規化されているか** (path だけだと ZIP 内エントリを区別できない)
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
