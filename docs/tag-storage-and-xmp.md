# タグ / レーティングの保存先と ExifTool 検証

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## タグ / レーティングの保存先と ExifTool 検証

### 通常のタグ操作はメディアファイルを書き換えない

**mIV タグの正本は `%APPDATA%/mimageviewer/tags.db`** ([src/tags_db.rs](../src/tags_db.rs))。
通常のタグ付与 / 削除 / クリアは **メディア本体 / XMP / IPTC を一切書き換えない**
([src/tag_write_worker.rs](../src/tag_write_worker.rs))。Tantivy 全文索引にも投影しない
([docs/tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md) D13「書き込み投影ゼロ」)。

そのため:

- **タグは他ソフトへ持ち出せない**。Lightroom / Bridge / digiKam / XnView MP でも、
  Windows エクスプローラーの「タグ」欄でも見えない。比較資料・製品ページ・紹介文で
  「mIV のタグはファイル本体に書かれるので他ソフトと共有できる」と書かない。
  これは **v1.0 の旧仕様** で、現行仕様ではない (2026-08 に旧記述を信じたまま誤った
  比較資料を作り、全面訂正した経緯がある)。
- 移動耐性は **フォルダごとのサイドカー `mimageviewer.dat`** へのミラーで確保する
  ([src/sidecar.rs](../src/sidecar.rs))。ミラー先はサイドカーであって**メディアファイルではない**。
  設定 `tag_sidecar_backup_enabled` は **既定 OFF**。
- 外部書き出し (XMP / CSV / JSON) は現行仕様では **行わない**
  ([docs/tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md) D19)。

正本は [docs/tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md)、現行の全体像は
[docs/search-architecture.md](search-architecture.md) §4.9。ユーザー向け記述は
`htdocs/mimageviewer/manual/tags.html` が現行仕様に一致しているので、迷ったらそちらを読む。
`docs/archive/search-metadata/tag-feature.md` は **v1.0 の履歴** なので現行仕様の根拠にしない。

### 「本を優先」するレーティングの保存先 (§1.417)

`RatingBookPreferred1..5/Clear/StepUp/StepDown` は既定キーなしの 8 操作で、
`rating.db` の既存の保存・Undo・公開経路を使う。新しい評価ストアやキー体系は作らない。

- 一覧では既存のアイテム用と同じ選択 / チェック項目を評価する。本のページ一覧でも対象は選択ページ。
- 画像閲覧・native 動画 / 音声で採用済み `ProjectionRecipe::Book(scope)` なら、入力を受けた
  viewer context の物理状態と scope を照合し、開いている本全体の対象を入力時に値で確定する。
  **本の内部から付けた★は、親一覧でその本のセルへ付けたコンテナ★と `rating.db` の同じ行**になる。
  変換書庫はキャッシュ ZIP でなく元書庫、入れ子 ZIP は既存の階層付きキーを使う。
- 本扱いでない閲覧 (`OrdinaryList` / `ExtractedResult`) は表示アイテムへ適用する。
  検索等の抽出結果から画像を開いただけでは元フォルダを評価しない。
- 対象解決は I/O なし。本と物理状態を照合できなければ書き込まず通知し、別対象へ代替適用しない。
  XMP の対象・設定・書き込み経路は既存どおりで、本のコンテナ★はアプリ内 DB に保存する。

詳細: [本を優先するレーティング操作](book-preferred-rating-plan.md)。

### いま実際にファイルへ XMP を書く 2 経路

`src/xmp_writer.rs` はタグ用途では縮退したが、消えてはいない。以下の 2 つは **今もユーザーの
ファイルを書き換える**ので、変更したら ExifTool で検証する。

| 経路 | 呼び出し元 | 書き込む物 | 条件 |
| --- | --- | --- | --- |
| レーティング | `rating_write_worker` → `xmp_writer::apply_rating` | `xmp:Rating` | 設定 `write_rating_to_xmp` (**既定 OFF**) + `GridItem::Image` かつ JPEG / PNG / WebP。製本ページは除外 |

**旧 XMP タグの経路は 2 つとも既に無い。** 手動の「旧XMPタグを取り込む / 取り込んでファイルから削除」は
`e63600147` (2026-08-30、v3.4.0) で、初訪問時の自動 seed は v3.9.1 で撤去した (`39390ec77`、§1.226)。
`src/tag_legacy_xmp_worker.rs` も `src/tag_legacy_seed_worker.rs` も存在しない。**復活させない。**

この経路は `src/xmp_writer.rs` の `XMP_WRITE_LOCK` で直列化されている (同一ファイルへの read-modify-write を
並走させると後勝ちで相手の編集を消すため)。**新しい書き込み経路を足すときもこのロックを通す**。

### ExifTool でのラウンドトリップ検証

上記 2 経路を変更したときの検証手順。ExifTool (exiftool.org) は業界標準の XMP 解釈
リファレンス実装なので、ここで期待どおり出れば Lightroom / Bridge / digiKam / XnView MP でも
同じに見える。

```powershell
# UTF-8 対応のためコンソールを切り替え (CP932 のままだと日本語タグが化ける)
chcp 65001
# または
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

# (1) レーティング書き込み (write_rating_to_xmp = ON) の確認
exiftool -XMP:Rating "path\to\rated.jpg"
#   Rating: 4                    ← mIV で付けた★の数。0 / 未評価なら要素ごと消える

# (2) 「旧XMPタグを取り込んでファイルから削除」の前後比較
exiftool -XMP-dc:Subject -IPTC:Keywords "path\to\legacy.jpg"
#   削除前: Subject: #原神, #風景, landscape
#   削除後: Subject: landscape    ← "#" 始まりだけ消え、他ソフト由来のタグは残る
#   Keywords:                     ← mIV は IPTC を読み書きしないので前後とも不変
```

**旧タグ削除の検証で一番大事なのは「他ソフト由来のタグが残ること」**。`#` 始まりだけが消え、
`#` の付かない既存キーワードと `xtw:*` 等の Extended XMP が保持されているかを、削除の前後で
必ず比較する (`apply_tag_op` は最小差分編集なので、パケット再構築の退行はここでしか出ない)。

### 文字化け対処

PowerShell で日本語タグが `繝励Μ繧ｭ繝･繧｢` のような mojibake になるのはコンソール出力
エンコーディングが CP932 のまま UTF-8 バイト列を解釈しているだけで、**ファイル内容自体は
正しい UTF-8**。`chcp 65001` か `[Console]::OutputEncoding = [System.Text.Encoding]::UTF8`
で解消。確実に機械可読にしたいなら JSON で取り出す:

```powershell
exiftool -j -XMP:Subject "path\to\tagged.jpg" | ConvertFrom-Json
```

JSON は仕様上 UTF-8 固定なのでコンソール設定に依存しない。

### Windows エクスプローラー互換 / IPTC 併記について

**現行設計では検討対象外**。mIV はタグをファイルへ書かない (`tags.db` 正本) ので、
「XMP `dc:subject` に加えて IPTC Keywords へも併記する」という v1.0 前提の dual-write 案は
**前提ごと消えている**。エクスプローラーの「タグ」欄に出したいという要望が来た場合、論点は
IPTC 併記ではなく **「タグを外部形式へ書き出す明示コマンドを設けるか」** という設計判断になる。

これは [docs/tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md) §10 (非対象 / 将来) と
D19 で「v1 では外部書き出しをしない (非破壊・外部書き込み無し方針の維持)。将来の明示
エクスポートは検討余地」と整理済み。要望が来たらこの節ではなく **そちらで判断する**。
