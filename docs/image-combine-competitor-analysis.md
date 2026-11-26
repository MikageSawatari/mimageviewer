# 画像の結合 (§1.904) — 他ソフトの調査

調査日: 2026-11-26。Web 上の公式文書・公式ストア説明・公式チュートリアルを優先して開いた。
各記述は下の「出典」の URL に対応する。表の記号:

- (公) = 公式文書・公式ストア説明・公式チュートリアルで確認
- (第三者) = 第三者の解説記事・利用者フォーラムで確認 (公式ではない)
- (推) = 文書の記述から推測 (明記はない)
- 記載なし = 開いた文書に書かれていない (機能が無いとは限らない)

注意: XnView MP・IrfanView・PhotoScape X は公式マニュアルの該当ページを開けなかった
(XnView wiki は廃止、PhotoScape X 公式サイトは接続拒否、IrfanView はヘルプが本体同梱のみ)。
そのため変更履歴・公式ストア説明・フォーラム・第三者ガイドで補っている。Adobe (Lightroom / Bridge)
と X / Instagram のヘルプは取得時に 403 等で開けなかったので、比較表から外し、末尾の参考に回した。

---

## 1. 項目ごとの比較表

### 1-A. 並べ方・隙間・背景・枠

| 製品 | 1. 並べ方 | 2. 隙間 / 背景色 | 3. 枠 / 角丸 / 影 |
| --- | --- | --- | --- |
| XnView MP (Join images) | 横 / 縦 / Mosaic / Collage (第三者)。1 行あたり枚数の設定あり (第三者・フォーラム) | 隙間あり (第三者・フォーラム、過去版で不具合報告多数)。背景色は既定で白い帯になるとの報告 (第三者)。透過背景は要望止まり (第三者) | 記載なし |
| IrfanView (Create Panorama image) | 横 / 縦の 1 列のみ (第三者)。コマンドラインの方向は 1/2 (第三者) | 「Add space between images」(公・v4.23 変更履歴)。隙間の色を指定可 (第三者) | 記載なし |
| IrfanView (Contact sheet) | 行数 × 列数のグリッド、紙サイズ / ピクセル指定 (第三者) | 行列の間隔 (px)、シート背景色 (第三者) | 記載なし |
| FastStone (Image Strip Builder) | 横 / 縦 (公) | 記載なし | ドロップシャドウ (公・チュートリアル v4.5) |
| FastStone (Contact Sheet Builder) | 列 × 行のグリッド (公) | 間隔 横 / 縦を別指定、シート背景色、サムネイル背景色 (公) | 枠 (Border) 設定あり。例では角丸の黒枠 (公) |
| PhotoScape X (Combine / Collage) | Combine: 縦 / 横 (公・App Store)。Collage: コラージュ枠に配置 (公) | 記載なし | 記載なし (別機能のフィルタに「Margin/Round Corners」「Border Line」あり (公)) |
| PhotoScape 3.x (連結 / ページ) | 連結: 縦 / 横 / 碁盤目 (第三者)。ページ: テンプレートに D&D (第三者・窓の杜) | 記載なし | ページ: インスタントフィルム風フレーム等の装飾 (第三者・窓の杜) |
| Eagle (Combine Images プラグイン、Eagle App 製) | 縦積み / 横並び (公)。グリッドは記載なし | margin と padding を個別指定 (公)。背景は RGBA で不透明度・透過可 (公) | 記載なし |
| ImageMagick montage | `-tile` で列 × 行のグリッド。片方だけ指定すると自動 (公) | `-geometry` の +x+y が隙間 (公)。`-background` で背景、`none` で透過 (公) | `-border` / `-frame` (立体枠) / `-shadow` (公)。既定は枠なし (公) |
| ImageMagick -append / +append | `-append` 縦、`+append` 横 (公) | 隙間なしで連結 (公)。隙間は `-smush` で指定、負値で重ね (公)。余白は `-background` 色で埋まる (公) | 記載なし |
| ComfyUI ImageStitch ノード | right / down / left / up の 4 方向に 2 枚を連結 (公) | `spacing_width` 0〜1024 (偶数)、`spacing_color` white/black/red/green/blue (公) | 記載なし |
| VOVSOFT Image Combiner | 縦 / 横 / グリッド (公)。列数指定は記載なし | 隙間 px 指定 (公)。背景 黒 / 白 / 任意 RGB、透過 PNG 出力可 (公) | 記載なし |
| Image Coupler (国産フリー) | 横並び / 縦並び、2〜9 枚 (第三者・窓の杜) | 解像度違いで生じる余白の位置と色を指定 (第三者) | 記載なし |
| Fotor (Photo Stitch、Web) | 縦 / 横、2〜16 枚 (公) | spacing・margin を手動調整、背景色変更 (公) | 枠の太さと色、角丸 (corner radius) (公) |
| Canva (Grids) | テンプレートのグリッド (公) | Spacing スライダー (公)。セルを単色で塗れる (公) | 記載なし |
| Google Photos (コラージュ) | デザイン (テンプレート) から選択、D&D で並べ替え (公) | 記載なし | 記載なし |
| Picsew (iOS) | 縦 (長尺) / 横 (公) | 記載なし | 枠 (borders) を追加可 (公) |
| Tailor (iOS) | 縦の長尺 1 枚 (公、横は記載なし) | 記載なし | 記載なし |
| SmartStitch (Webtoon 用、OSS) | 縦に連結してから指定高さで分割 (公・README) | 記載なし | 記載なし |
| NeeView / BandiView / Honeyview | 結合画像を作る機能は公式文書に見当たらない。BandiView は表示モードとして縦連続 (Webtoon) 表示あり (公) | — | — |

### 1-B. 大きさ・縦横比・出力サイズ・並び順

| 製品 | 4. 大きさのそろえ方 | 5. 縦横比・切り取り | 6. 出力サイズ指定・上限 | 7. 並び順 |
| --- | --- | --- | --- | --- |
| XnView MP | 記載なし (Classic では幅を合わせず背景で埋める挙動との報告 (第三者)) | 記載なし | 記載なし | 記載なし |
| IrfanView Panorama | 1 枚目 (または最大) に合わせて小さい画像を引き伸ばす。縦連結なら幅、横連結なら高さ (第三者) | 引き伸ばし (切り取りなし) (第三者) | 記載なし | リストへの追加は可 (第三者)、並べ替えは記載なし |
| IrfanView Contact sheet | セル内に縮小 (推)。小さい画像を引き伸ばすオプション (公・v3.70 変更履歴) | 記載なし | 紙サイズ・DPI・カスタム px (第三者) | 記載なし |
| FastStone Image Strip | 「Resize」で全画像を同じ高さに (公)。例は高さ 300 px | 切り取りなし (公) | 高さ指定のみ (公) | D&D で並べ替え (公・v4.3) |
| FastStone Contact Sheet | セル内に縮小 (推) | 記載なし | ページ幅 / 高さを px で指定、印刷サイズ換算ダイアログ (公) | ファイルリストで追加・削除 (公) |
| PhotoScape X | 記載なし | 記載なし | 記載なし | 記載なし |
| Eagle Combine Images | 「Auto size calculation」(公、詳細不明)。配置は start / center / end / stretch (公) | stretch で伸縮 (公) | 記載なし | パネルで D&D 並べ替え (公) |
| ImageMagick montage | 既定 `120x120>` = セルに収まるよう縮小のみ (公)。`<` で拡大のみ、サイズ省略で原寸 (セルは最大画像に合わせる) (公) | 縦横比を保ってセルに収める (推、既定挙動)。セル内は `-gravity` で配置、既定は中央 (公) | セル寸法 × タイル数で決まる (推) | 入力順・行優先で埋める (公) |
| ImageMagick append | 揃えない (公)。そろえるなら事前に `-resize` (推) | — | 入力の合計 (推) | 入力順 (推) |
| ComfyUI ImageStitch | `match_image_size` 既定 True で 2 枚目を 1 枚目に合わせる (公) | False 時は余白を埋める (公) | 記載なし | 入力ソケット順 (公) |
| VOVSOFT Image Combiner | 記載なし | 記載なし | 65,535 px 超も可 (PNG / TIFF / BMP) (公) | リスト内で順序変更、同じ画像の複製可 (公) |
| Image Coupler | 記載なし (余白で吸収) (第三者) | 記載なし | 記載なし | 一括モードはファイル名の連番でグループ化 (第三者) |
| Fotor | 記載なし | 記載なし | キャンバスサイズと向きを変更可 (公) | 記載なし |
| Canva | セル内で拡大縮小・位置調整 (公) | セルを埋めて切り取り (推、セル内で位置を動かす操作から) | テンプレートの寸法 (推) | セルへ D&D (公) |
| Google Photos | 記載なし | 記載なし | 記載なし | D&D (公) |
| Picsew | — (スクリーンショットの重なり検出) | 継ぎ目の手動微調整 (公) | 長い画像を一定サイズのスライスに分割して書き出し (公) | 記載なし |
| SmartStitch | 幅そろえ: なし / 最小幅に合わせる / 指定幅 (既定 720) (公) | 記載なし | 目安の分割高さ (既定 5000)。コマや吹き出しを切らない位置へずらす (公) | フォルダ単位、バッチは章ごと (公) |

### 1-C. プレビュー・保存・プリセット・その他

| 製品 | 8. プレビュー | 9. 保存形式・メタデータ | 10. プリセット | 11. その他 |
| --- | --- | --- | --- | --- |
| XnView MP | ライブプレビューは要望止まり (第三者・2025) | 記載なし | 要望止まり (第三者・2015) | ファイル名の描画は要望止まり (第三者)。位置そろえ (中央・右/下) は不具合報告あり (第三者) |
| IrfanView Panorama | 記載なし | 結果をメイン画面に開き、利用者が形式を選んで保存 (第三者) | 記載なし | ファイル名を描画可 (第三者)。コマンドライン `/panorama` (第三者) |
| IrfanView Contact sheet | 記載なし | 印刷または保存 (第三者) | プロファイル (公・v4.53) | ラベル (ファイル名・寸法・メタデータ)、フッター (第三者) |
| FastStone Image Strip | 記載なし | 作成時に保存ダイアログ (公)。クリップボードへコピー (公・v6.0) | 記載なし | 連番を付ける (公)、ファイル名 (公・v6.1)、中央そろえ「Center」(公・v7.6)、タイトル (公・v8.0) |
| FastStone Contact Sheet | Preview タブ (公) | 記載なし | 設定に名前を付けて保存 (公) | ヘッダ / キャプション / フッター (左・中・右)、EXIF をキャプションに (公) |
| Eagle Combine Images | 変更が即座にキャンバスへ反映 (公) | JPG / PNG / WebP / AVIF / BMP、品質調整 (公) | 記載なし | — |
| ImageMagick montage | なし (CLI) | 任意形式 (推) | — | `-label '%f'` でファイル名、`-title` (公)。既定はラベルなし (公) |
| ComfyUI ImageStitch | ワークフロー上 | — | ワークフローごと保存 (推) | — |
| VOVSOFT Image Combiner | 記載なし | 入出力 JPG / PNG / WebP / GIF / TIFF / BMP。原寸・原画質で保存とする。品質設定は記載なし (公) | 記載なし | EXIF の向きを自動検出 (公) |
| Image Coupler | 記載なし | アルファ除去、JPEG 品質、OptiPNG で PNG 最適化 (第三者) | 記載なし | — |
| Fotor | 編集画面 | PNG / JPG / PDF (公) | — | — |
| Picsew | 編集画面 | 高解像度画像、PDF、スライス (公) | 記載なし | 重なり自動検出、最大 300 枚、文字・矢印・透かし、ぼかし / モザイク (公) |
| Tailor | — | PNG (公・v1.2.0 履歴)。保存後に元スクリーンショットを削除できる (公・v2.0.0) | — | 重なりを自動検出して整列 (公) |
| SmartStitch | — | png (既定) / jpg (品質 100) / webp / bmp / psd / tiff / tga (公) | 設定プロファイル (公) | 後処理で外部プログラム (例: waifu2x) を実行 (公) |

メタデータ (EXIF / ICC / AI 生成情報) を残すかどうかを明記している製品は、今回開いた文書の中には無かった。

---

## 2. 製品ごとの説明と出典

### XnView MP
- Browse 画面の Create > Join images (旧名 Create strip of images)。横 / 縦 / モザイク / コラージュで連結。
  - https://itigic.com/xnview-review-free-image-viewer-and-converter/ (第三者レビュー。Create タブの Join images、縦・横・mosaic・collage)
  - https://newsgroup.xnview.com/viewtopic.php?t=21594 (2015 年の改善要望: 名前変更、プリセット保存、32bit 透過背景、進捗表示。MP 0.75 で Browser に Join images 追加)
  - https://newsgroup.xnview.com/viewtopic.php?p=159929 (行数設定・空白の無駄・ファイル名描画・ライブプレビューの要望。開発者の回答なし)
  - https://newsgroup.xnview.com/viewtopic.php?p=197910 (1.7.1 で Mosaic の位置そろえが無視される不具合。1.7.2 で一部修正。spacing 10px・1 行 3 枚の試験)
- 公式マニュアル (旧 wiki https://www.xnview.com/wiki/index.php/Strip_of_Images) は現在フォーラムへ転送され、内容は読めなかった。
- 背景が既定で白い帯になる、Classic では大きさを変えず背景で埋めた、などは検索結果に出たフォーラム投稿 (newsgroup.xnview.com、p=54149 ほか) の要約で、投稿の版は古い。

### IrfanView
- Image メニューの Create Panorama image (v3.70 追加)。サムネイル画面から選択画像で開ける (v4.20)。v4.23 で「Add space between images」。コマンドライン `/panorama=(direction,file1,...)` (v3.99)。Contact Sheet は v3.30 から、プロファイル保存は v4.53。
  - https://www.irfanview.com/history_old.htm (公式の変更履歴)
- 方向・隙間・隙間の色・ファイル名描画、小さい画像の引き伸ばし、結果をメイン画面に開いて別途保存、Contact sheet の行列・間隔・背景・ラベル・フッター:
  - https://softerviews.org/multipage.html (第三者ガイド)

### FastStone Image Viewer
- Create > Image Strip Builder。公式の機能ページの変更履歴: v3.6 で追加 (横 / 縦)、v4.3 で D&D 並べ替え、v6.0 でクリップボードへコピー、v6.1 で File name、v7.6 で Center、v8.0 で Title。Contact Sheet Builder は「Fully configurable」。
  - https://www.faststone.org/FSViewerDetail.htm
- 公式サイト配布のチュートリアル (v4.5 時点、第三者執筆を公式が配布): Image Strip は「全画像を同じ高さにそろえる・切り取らない」「横 / 縦」「ドロップシャドウ」「連番」「Resize で高さ px 指定」。Contact Sheet は既定 6 列 × 8 行、間隔 既定 横 30 px / 縦 10 px、シート背景色・サムネイル背景色・枠、ヘッダ / キャプション / フッター、Preview タブ、設定の名前付き保存。
  - https://www.faststone.org/DN/FSIV_Tutorial.pdf (p.54〜57、p.83〜84)

### PhotoScape X / PhotoScape 3.x
- PhotoScape X の公式ストア説明: Combine「Attach multiple photos vertically or horizontally to create one final photo.」、Collage は「collage frame」に配置。別機能のフィルタに Margin/Round Corners・Border Line (v2.8)。
  - https://apps.apple.com/us/app/photoscape-x-photo-editor/id929507092
- 公式サイト (x.photoscape.org) は接続拒否で開けず、Combine の詳しい設定は確認できなかった。
- PhotoScape 3.x の「ページ」: テンプレートに D&D、フィルム風フレームなど (窓の杜 2008)。連結の碁盤目配置は第三者記事 (CyberLink ブログの検索要約) のみ。
  - https://forest.watch.impress.co.jp/article/2008/10/23/photoscape.html

### Eagle
- Eagle App 名義の公式プラグイン「Combine Images」。縦積み / 横並び、配置は start / center / end / stretch、margin と padding を個別指定、RGBA の背景 (透過可、プレビューは市松模様)、変更が即座にキャンバスへ反映、D&D で並べ替え、JPG / PNG / WebP / AVIF / BMP と品質調整。
  - https://community-en.eagle.cool/plugin/combine-images

### ImageMagick
- montage: 既定 `-geometry` は `120x120>+4+3` (セル 120×120 に縮小のみ、隙間 横 4 / 縦 3)。`-tile` 省略時は枚数から自動 (4 枚→2×2)。セル内は既定で中央。既定でラベルなし、`-label '%f'` でファイル名。`-background none` で透過。`-frame` `-border` `-shadow`。`-mode Concatenate` で隙間なし連結。
  - https://imagemagick.org/montage/ (オプション一覧)
  - https://usage.imagemagick.org/montage/ (既定値と挙動)
- append: `-append` 縦 / `+append` 横、隙間なし。`-gravity` で揃え位置 (v6.4.7-1 以降)、空いた部分は `-background` 色。隙間は `-smush` (負値で重ね)。
  - https://usage.imagemagick.org/layers/

### ComfyUI ImageStitch (AI 画像の利用者が日常的に使う部品として)
- direction (既定 right、right/down/left/up)、match_image_size (既定 True)、spacing_width (既定 0、0〜1024 の偶数)、spacing_color (既定 white)。
  - https://docs.comfy.org/built-in-nodes/ImageStitch

### VOVSOFT Image Combiner
- 縦 / 横 / グリッド、隙間 px、背景 黒 / 白 / 任意 RGB、PNG の透過保持、EXIF の向き検出、65,535 px 超の出力、順序変更と複製。
  - https://vovsoft.com/software/image-combiner/

### 国産フリーソフト
- Image Coupler: 横 / 縦、2〜9 枚、解像度違いの余白の位置と色、JPEG 品質、PNG 最適化、連番ファイル名で一括連結。
  - https://forest.watch.impress.co.jp/docs/serial/okiniiri/616114.html
- 同じ検索で AzJoinImage (タイル連結・隙間の背景色、Windows 98〜XP 時代)、ImageMerge (1 行 / 1 列の枚数 2〜999) も出たが、記事は開いていない。

### Web / スマートフォンのコラージュ
- Fotor Photo Stitch: 縦 / 横、2〜16 枚、spacing・margin・corner radius、枠の太さと色、背景色、PNG / JPG / PDF。
  - https://www.fotor.com/features/photo-stitch.html
- Canva Grids: Spacing スライダー、セル内で拡大縮小・位置調整、セルの単色塗り。
  - https://www.canva.com/help/using-grids/
- Google Photos: Web では最大 9 枚、編集はアプリで。デザイン選択と D&D での並べ替え、写真の補正。
  - https://support.google.com/photos/answer/12637115?hl=en
  - https://blog.google/products/photos/new-look-for-memories-plus-collage-editor/
- Windows フォト: 専用のコラージュ機能を示す公式文書は見つからなかった (第三者記事は「無い」「版による」で食い違う)。

### スクリーンショット・長尺連結
- Picsew: 重なりを自動検出して連結、継ぎ目の手動調整、縦 / 横、最大 300 枚、枠・文字・透かし、ぼかし / モザイク、PDF とスライス分割の書き出し。
  - https://apps.apple.com/us/app/-/id1208145167
- Tailor: 重なったスクリーンショットを自動で整列・連結、PNG、保存後に元画像を削除可。
  - https://apps.apple.com/us/app/tailor-screenshot-stitching/id926653095
- SmartStitch (Webtoon / 漫画の raw 用): 縦に連結し、目安の高さ (既定 5000) でコマや吹き出しを避けて分割。幅そろえ (なし / 最小幅 / 指定幅 720)、検出感度 (既定 90)、出力形式 (既定 png)、設定プロファイル、waifu2x 等の後処理。
  - https://github.com/MechTechnology/SmartStitch

### ビューア系で結合機能が見当たらないもの
- NeeView: コマンド一覧に保存・コピーはあるが結合は無い。「画像をコピー」は 2 ページ表示時にメインのページだけをコピーする。v40.0 で画像出力に「オリジナルサイズ」「ドットキープ」設定。
  - https://neelabo.github.io/NeeView/ja-jp/command-list.html
  - https://neelabo.github.io/NeeView/ja-jp/changelog.html
- BandiView: Webtoon 表示モードで複数画像を縦につないで表示 (ファイルは作らない)。Honeyview は該当機能なし。
  - https://www.bandisoft.com/bandiview/
  - https://www.bandisoft.com/honeyview/
- ACDSee: 公式ヘルプは見つからなかった。フォーラムでは、Contact sheet に横 / 縦の間隔・マット色・キャプション位置がある、コラージュは編集モードでレイヤーを手で並べる、とされる (第三者、未確認)。

---

## 3. まとめ

### 3-1. ほぼ全製品にある基本 (ベースライン)
- **横 1 列 / 縦 1 列の連結**: 調べたほぼすべてにある。「並べ方」の最低限。
- **隙間 (px) と隙間の色 (背景色)**: IrfanView・XnView・FastStone (Contact sheet)・Eagle・ImageMagick・ComfyUI・VOVSOFT・Image Coupler・Fotor・Canva で共通。指定は「px 数 + 色 1 つ」が大半。
- **大きさが違う画像の扱い**: 次の 2 方式に分かれる。
  - 連結方向と直交する辺をそろえて拡大縮小する (FastStone は高さを指定、IrfanView は 1 枚目に合わせて引き伸ばす、ComfyUI は 2 枚目を 1 枚目に合わせる)
  - そろえずに空いた所を背景色で埋める (ImageMagick append、XnView Classic、Image Coupler の余白位置と色)
- **並び順の変更**: リストでの並べ替えまたは D&D (FastStone・Eagle・VOVSOFT・Canva・Google Photos)。
- **結果の保存**: 保存ダイアログで形式を選ぶか、いったん開いて保存する。JPEG 品質を指定できるものもある。

### 3-2. 差が出る点 (差別化要素)
- **グリッド (列数 × 行数)**: ImageMagick montage・VOVSOFT・XnView (Mosaic、1 行の枚数)・Contact sheet 系 (IrfanView / FastStone) にある。単純な連結ツールには無い。
- **揃え位置 (上 / 中央 / 下、start / center / end / stretch)**: Eagle・ImageMagick (`-gravity`)・FastStone (v7.6 の Center)・XnView (不具合報告あり)。後から足された例が多く、需要があると読める。
- **透過背景**: Eagle (RGBA)・ImageMagick・VOVSOFT。XnView では長年の要望止まり。
- **枠・角丸・影**: Fotor (角丸・枠)・FastStone (影・Contact sheet の角丸枠)・ImageMagick (frame・shadow)・Picsew (枠)。SNS 向けのツールほど持っている。
- **ファイル名・連番・タイトルの描画**: FastStone (連番・ファイル名・タイトル)・IrfanView・ImageMagick (`-label '%f'`)。XnView では要望止まり。
- **ライブプレビュー**: Eagle (即時反映)・FastStone Contact sheet (Preview タブ)。XnView では要望止まり。
- **プリセットの保存**: FastStone Contact sheet・IrfanView Contact sheet (プロファイル)・SmartStitch。単純な連結ツールにはほぼ無い。
- **出力サイズの上限・長尺の分割**: Picsew (スライス)・SmartStitch (目安の高さで分割)・VOVSOFT (65,535 px 超も可)。
- **重なり検出 (スクリーンショットの継ぎ合わせ)**: Picsew・Tailor・SmartStitch。写真・イラストの結合とは別系統の機能。
- **メタデータの扱い**: 明記している製品は無かった。何を残すかを決めて説明すれば、それ自体が差になり得る。

### 3-3. 漫画・AI 画像・SNS の観点
- **漫画の見開き (2 ページを横に)**: 右綴じでは右→左の順で並べる必要がある。調べた汎用ツールに右綴じを意識した設定は見当たらない (並び替えで対応する前提) (推)。NeeView も見開きを 1 枚に書き出す機能は持たず、「画像をコピー」は 2 ページ表示のときメインのページだけ。
- **Webtoon・縦長の漫画**: 縦連結 + 幅そろえ (SmartStitch の「最小幅に合わせる / 指定幅」) + 一定高さでの分割が専用ツールの基本形。BandiView は表示だけ縦につなぐ。
- **4 コマ**: 縦 1 列 + 幅そろえ + 隙間で作れる。専用テンプレートを持つ製品は見当たらない (推)。
- **AI 画像の比較 (同じプロンプトの複数生成・前後比較)**: ComfyUI の ImageStitch は「方向・サイズを合わせる・隙間・隙間の色」の 4 項目だけで、既定は右方向・合わせる・隙間 0・白。グリッドと揃え位置を足せば montage 相当になる。
- **SNS**: SNS 向けツール (Fotor・Canva・Google Photos・Picsew) はテンプレート・角丸・枠・セル内の切り取りを重視し、隙間はスライダーで調整する。画像の長辺と枚数の制限は SNS ごとに違う (下の参考。ヘルプ本文は開けなかった)。長い画像を分割して書き出す機能 (Picsew・SmartStitch) は、投稿上限に合わせるための機能でもある (推)。

### 3-4. 文書にあった既定値
| 製品 | 既定値 |
| --- | --- |
| ImageMagick montage | `-geometry 120x120>+4+3` (セル 120×120、縮小のみ、隙間 横 4 px / 縦 3 px)、セル内は中央、ラベルなし、枠・影なし、`-tile` は自動 (4 枚→2×2、9 枚→3×3) |
| ImageMagick append | 隙間なし。サイズ差で空いた部分は `-background` 色 (既定値の記載なし。例では white を明示) |
| ComfyUI ImageStitch | direction=right、match_image_size=True、spacing_width=0、spacing_color=white |
| FastStone Contact Sheet | 6 列 × 8 行、間隔 横 30 px / 縦 10 px |
| FastStone Image Strip | 「同じ高さにそろえる・切り取らない」が基本動作 (高さの既定値は記載なし) |
| IrfanView Panorama | 小さい画像は引き伸ばして 1 枚目 (または最大) の辺に合わせる (第三者) |
| XnView MP | 背景は白い帯になるとの報告 (第三者、版は古い) |
| SmartStitch | 分割高さ 5000、出力 png、幅そろえなし、感度 90、走査ステップ 5 |
| Google Photos | Web では最大 9 枚 |
| Fotor | 2〜16 枚 |
| Picsew | 最大 300 枚 |

まとめると、既定は「隙間 0 か小さい値、背景は白」が多い。大きさの扱いは「そろえる (拡大縮小)」と「そろえず背景で埋める」で製品ごとに分かれる。

---

## 参考 (本文を開けなかったもの。検索結果の要約のみで未検証)
- Adobe Lightroom Classic の Print モジュール (Contact Sheet / Grid): Page Grid (行・列)、Cell Spacing、Cell Size、Keep Square、余白。helpx.adobe.com は 403 で開けず。
  - https://helpx.adobe.com/lightroom-classic/desktop/print-photos/print-module-layouts-templates.html
- Adobe Bridge の Output (PDF Contact Sheet): 403 で開けず。https://helpx.adobe.com/bridge/using/output-module-pdf-contact-sheet.html
- X: 1 投稿に写真 4 枚まで、写真 1 枚 5 MB (検索要約。https://help.x.com/en/using-x/posting-gifs-and-pictures は 403)。
- Instagram: 幅 1080 px、縦横比 1.91:1〜4:5 (別ページでは 3:4)。範囲外は切り取られ、大きい画像は幅 1080 に縮小 (検索要約。https://help.instagram.com/1631821640426723 は本文を取得できず)。
