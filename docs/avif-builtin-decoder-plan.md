# AVIF 内蔵デコーダ (libavif + dav1d) 設計 — backlog §1.407

正本はこの文書。バックログ §1.407 には現状と残りだけを書く。

## 1. 経緯と原因

- 5ch >>599 (2026-10-20): AVIF がデコードエラーになり、Susie プラグイン導入後は表示できるが遅い、との報告。
- 開発機 (Windows 11 Pro 10.0.26300、Microsoft.AV1VideoExtension 2.0.35.0) で、WIC (mIV と同じ
  Frame 0 → 32bpp BGRA 変換) に自作 AVIF 約 35 種を通した (観測者: ClaudeCode のスクリプト。mIV 本体は起動していない)。
  - **デコードエラーは再現しなかった**: 8/10/12bit、4:2:0/4:2:2/4:4:4、モノクロ、full range、HDR PQ タグ、
    8192x6144、aom/svt/rav1e、アニメの 1 フレーム目、grid 2x2、透過付き、irot/imir はいずれも WIC で読めた
    (不正ファイル 2 種は失敗)。報告者の失敗の原因は**未特定**。候補 (未確認): 報告者の環境に AV1 拡張が無い、
    今回作れなかった構造を含む。
  - **WIC 経路の欠陥を 2 つ確認した**:
    1. 透過が失われる (アルファが全面 255)。
    2. AVIF の色変換行列・レンジ (nclx) を無視して色がずれる。BT.709 limited を明記した純赤が (233,0,2)、
       BT.601 full range を明記した純赤が (255,26,0)。元画像既知の 800x600 で元画像との PSNR は
       libavif 33.4dB に対し WIC 27.4dB。
- 利用者決定 (2026-10-20): WIC や Susie に頼らず、mIV が専用ライブラリで AVIF を読む。依存は
  **libavif + dav1d + libyuv** (B 案「同梱 FFmpeg の dav1d を流用し容器以降を自前実装」と比較のうえ A 案)。

## 2. 依存とビルド

| 部品 | 版 | ライセンス | 役割 |
| --- | --- | --- | --- |
| libavif | 1.4.2 | BSD-2-Clause (ファイル内に第三者 notice あり、全文を同梱) | 容器解析・grid・透過・irot/imir/clap・色変換 |
| dav1d | 1.5.3 | BSD-2-Clause | AV1 復号 |
| libyuv | 644251f25 | BSD-3-Clause | YUV→RGB の高速変換 |

- 版は libavif 1.4.2 自身が `cmake/Modules/LocalDav1d.cmake` / `LocalLibyuv.cmake` で固定している組み合わせ。
- `scripts/setup-libavif.sh` (エージェント用) がソースを SHA-256 / commit で照合して取得し、
  静的 CRT (`/MT`、`.cargo/config.toml` の `+crt-static` に合わせる) で `vendor/libavif/{include,lib,licenses}` を作る。
  meson 1.12.1・ninja 1.13.2 (venv)・NASM 3.02 (zip) も `vendor/libavif/tools/` に取得し、利用者の環境全体には入れない。
  再取得できるのでツリー外バックアップは不要。
- `crates/libavif-sys` は作成済みの静的ライブラリをリンクするだけで、cargo の時点で C ビルドはしない
  (dav1d は meson と NASM が要り、LibRaw のように cc で組めないため)。
- 新しい DLL / exe は増えない。dav1d は同梱 FFmpeg の中にもあるが、FFmpeg は別 DLL なので衝突しない。

## 3. 組み込み方

- デコード順: AVIF (拡張子 `avif`) は **内蔵 (libavif) → WIC → Susie**。他形式の順序と JPEG 高速経路は変えない。
  内蔵で読めないとき (想定外の構造・libavif の制限) の救済として WIC / Susie の順を残す。
- 通常ファイル・書庫内バイト列・サムネイル・フルサイズ・類似検索・コンテキストメニュー・本 (books)・mIV Remote
  (core の canonical loader と thumb_loader を通る) の全経路で、同じ 1 つの関数を通す。経路ごとに AVIF 分岐を複製しない。
- 出力は RGBA8 (straight alpha)。10/12bit は 8bit へ変換する。HDR の表示対応・アニメーション AVIF の全フレーム再生は
  範囲外 (1 フレーム目を静止画として表示する従来どおり)。ICC は他形式と同じく使わない。
- 向き: AVIF の表示向きは irot / imir (と clap) が正本で、EXIF Orientation は適用しない (HEIF/MIAF の規定、
  libavif・ブラウザと同じ)。内蔵デコーダが clap → irot → imir を適用した画素を返し、後段の EXIF 向き補正を
  その結果に重ねない。WIC で読んだ場合の扱いは変えない。
- 安全: libavif の `imageSizeLimit` / `imageDimensionLimit` を mIV の上限 (WIC と同じ 32768 辺、呼び出し元の
  割り当て予算) に合わせ、RGBA バッファは寸法を見て予算内のときだけ確保する。strict は無効
  (実在するファイルを表示する目的。ブラウザと同じ寛容さ)。
- スレッド: dav1d の内部スレッド数は呼び出しの種類で固定する (実行時の空き状況で変えない)。
  背景の一括処理 (サムネイル・類似検索など、既に並列 worker で動く) は 1、利用者が待っている
  フルサイズ表示は複数。具体値は実装で worker 数を確認して決め、ここに書く。
- UI スレッドへ新しいデコードを足さない。既存の呼び出し元のスレッドのまま差し替える。
- 診断: 実際に使ったデコーダ (libavif / WIC / Susie)・所要時間・失敗段階 (libavif の結果コード) を
  perf ログに残す。Susie 診断の「本体優先」「このプラグインは呼ばれません」の誤説明を直す。
- ライセンス: ルートに `LIBAVIF-LICENSE.txt` / `DAV1D-LICENSE.txt` / `LIBYUV-LICENSE.txt` を置き、
  LibRaw と同じくバージョン情報に全文を埋め込み、installer / portable に同梱する。ソース提供の義務はない。
- まれな失敗は既定の割り切り (ログ + 通知の既存経路)。

## 4. 検証

- `testdata/avif/` のフィクスチャ (`make_fixtures.py` で再生成) で、色 (象限の中心)、透過、grid、irot/imir、
  アニメの 1 フレーム目、モノクロ、10/12bit、4:2:2/4:4:4、不正ファイル (内蔵が失敗し WIC→Susie へ回る) を
  自動テストで確認する。パスとバイト列、ZIP 内、サムネイル経路の一致も確認する。
- 実機確認は利用者 (透過 AVIF・色・ZIP 内・mIV Remote)。

## 5. 未決・記録

### 実装判断（2026-10-20、avif-1407）

- dav1d は需要のフルサイズ表示・clipboard・Remote AI で **4 threads**、サムネイル・
  類似索引・一括変換・製本 editor thumbnail・fullscreen 先読み・ベンチで **1 thread**。
  開発機の `NUMBER_OF_PROCESSORS=24`、fullscreen admission は process 共通で6件
  （`FS_PAGE_LOAD_TOTAL_PERMITS`）、thumbnail/batch は設定 Auto で logical CPU / 2
  （この環境では12 workers）。背景 worker にさらに内部並列を掛けない。
  需要側も6×4=24を目安に内部並列を4に固定し、空き状況による切替や新しい worker は追加しない。
- `libavif-sys` は既成の avif/dav1d/yuv static libraries と `/MT` の C shim のみをリンク。
  `VERSION` の全内容を検証し、同梱版を `BUILD_ID` として about へ出す。
- Rust の `avif_decoder` が割当前の寸法・RGBA予算検査と clap→irot→imir を一か所で所有。
  `image_decode::Decoded` の `Orientation::Applied` を後段へ運び、拡張子で EXIF を抑止しない。
  不正 clap だけは無視し、他の変換は続ける。native失敗は stage/result/diag をログへ記録して委譲。
- libavif 1.4.2 は `imageSizeLimit > 16384*16384` 自体を拒否する（`read.c`）。
  各辺32768と、このnative総画素上限・呼出元RGBA予算を併用する。超過は型付き
  Dimensions/Budget とし、通常経路は既存WIC/Susieへ進む。bounded経路のWICにも同じ予算を渡す。
- now-playing artwork の既存 bounded 経路はWICまでで、Susie呼出しは元からない。
  Susieには割当前RGBA予算を渡すAPIがないため、ここへ非boundedのSusieを新設しない。
  内蔵→bounded WICの既存終端を維持する。この例外は設計担当の確認事項。
- 既存cancelをnative呼出し前後・fallback前後で照合する。実行中の1回のlibavif decodeは中断しない。
- thumbnail catalog はmtime/sizeと全体schema version、PDF専用meta移行を持つが、
  画像形式/decoder versionで無効化する共通機構はない。代表キー `zipthumb:` / `folderthumb:`
  には元画像形式を特定できない旧行もあり、AVIFだけを漏れなく選べない。
  利用者指示に従い新しい無効化/スキーマ変更は行わない。既存WIC製AVIFサムネイルは再生成まで残る。
- 今回は長期処理の新しい状態/並行ownerを作らず、既存worker内のデコード境界だけを差し替える。
  modal化や専用live-rebuildは不要。既存同期UIから今回のデコード呼出しへ至る経路は
  棚卸しの範囲では見つからず、worker配置は変更しない。
- exeサイズ増分・実画像での速度比較は、同じprofileの変更前ビルドがこのworktreeにないため未測定。
  tiny synthetic fixtureの測定を実画像での性能根拠にはしない。

### 接続先

| 呼出元 | 接続と向き |
| --- | --- |
| canonical_image_loader | path / verified bytes / archive bytes、FallbackDecoder seamにnative追加、型付き向き |
| thumb_loader | decode_image_for_thumb、decode_zip_chain（ZIPとverified source）、load_one_cachedの通常path、batch path/ZIP |
| app | panorama source、fullscreen需要/先読みのworkload指定 |
| books | 通常file / ZIP bytesのcomposite source decode |
| similar_image | file/bytesのproxy、既存file WIC→bytes WICとSusie source選択を維持 |
| ui_dialogs/context_menu | file / ZIP clipboard worker |
| effetune/now_playing/artwork | bounded bytes decode（既存終端はWIC） |
| bin/bench_dupe | full decodeとsynthesis、二重EXIFを撤去 |
| bin/bench_ai / bench_zip_seek / bench_raw | file入力 / ZIP decodeと並列ZIP計測 / DNG限定WIC対照（同じhelper経由） |
| remote_ipc/container / thumbnail | canonical / thumb_loaderを共用、独立デコード経路なし |

直接WIC呼出しを残したもの: canonicalのproduction seam、共有helperの実装、artworkの
WIC対照unit tests。`bench_raw` のDNG専用対照もshared helper経由へ接続。

### 自動検証

実行コマンド・件数・変更ファイル・制限は [実装・検証記録](avif-1407-implementation-results.md) を参照。
AVIF＋ライセンスの最終focused 22/22、関連69/69、UI通常比較11/11成功。fmt clean、Clippy最終exit0（既存警告あり）。確認buildも成功（通常profile、core / Remote / EPUB）。製品バイナリは起動していない。
