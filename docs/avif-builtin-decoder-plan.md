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
  (core の canonical loader / thumb_loader と旧画像 API) の全経路で、同じ 1 つの関数を通す。経路ごとに AVIF 分岐を複製しない。
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

- dav1d は需要のフルサイズ表示・clipboard・Remote AI・旧 Remote 画像 API で **4 threads**、サムネイル・
  類似索引・一括変換・製本 editor thumbnail・fullscreen 先読み・URL画像取得・
  EPUB寸法解析・ベンチで **1 thread**。
  開発機の `NUMBER_OF_PROCESSORS=24`、fullscreen admission は process 共通で6件
  （`FS_PAGE_LOAD_TOTAL_PERMITS`）、thumbnail/batch は設定 Auto で logical CPU / 2
  （この環境では12 workers）。背景 worker にさらに内部並列を掛けない。
  需要側も6×4=24を目安に内部並列を4に固定し、空き状況による切替や新しい worker は追加しない。
- `libavif-sys` は既成の avif/dav1d/yuv static libraries と `/MT` の C shim をリンク。
  `VERSION` の全内容を検証し、同梱版を `BUILD_ID` として about へ出す。
- `libavif-sys` の safe Rust API が割当前の寸法・RGBA予算検査と clap→irot→imir を一か所で所有。
  本体の `avif_decoder` は workload・path I/O・perf/ログの薄い層。Remote と EPUB の寸法解析も共用する。
  `image_decode::Decoded` の `Orientation::Applied` を後段へ運び、拡張子で EXIF を抑止しない。
  不正 clap だけは無視し、他の変換は続ける。native失敗は stage/result/diag をログへ記録して委譲。
- libavif 1.4.2 は `imageSizeLimit > 16384*16384` 自体を拒否する（`read.c`）。
  各辺32768と、このnative総画素上限・呼出元RGBA予算を併用する。超過は型付き
  Dimensions/Budget とし、通常経路は既存WIC/Susieへ進む。bounded経路のWICにも同じ予算を渡す。
- now-playing artwork と clipboard capture の既存 bounded 経路はWICまでで、Susie呼出しは元からない。
  Susieには割当前RGBA予算を渡すAPIがないため、ここへ非boundedのSusieを新設しない。
  内蔵→bounded WICの既存終端を維持する。この既存終端を維持する。
- 既存cancelをnative呼出し前後・fallback前後で照合する。実行中の1回のlibavif decodeは中断しない。
- thumbnail catalog はmtime/sizeと全体schema version、PDF専用meta移行を持つが、
  画像形式/decoder versionで無効化する共通機構はない。代表キー `zipthumb:` / `folderthumb:`
  には元画像形式を特定できない旧行もあり、AVIFだけを漏れなく選べない。
  利用者指示に従い新しい無効化/スキーマ変更は行わない。既存WIC製AVIFサムネイルは再生成まで残る。
- 今回は長期処理の新しい状態/並行ownerを作らず、既存worker内のデコード境界だけを差し替える。
  modal化や専用live-rebuildは不要。既存同期UIから今回のデコード呼出しへ至る経路は
  通常画像の棚卸しでは見つからず、worker配置は変更しない。
  スタンプの既存同期描画経路は下記の固定形式として対象外とし、新しいデコードを足さない。
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
| app/cache_ops | 品質比較 file/ZIP、ZIP全件/代表1枚の共通worker境界。JPEG DCTの元寸法・RAWと非AVIFの向きを維持 |
| audio_thumbnail | sidecar→埋め込みAPIC。Parse/Decode/ConversionはCorrupt、Dimensions/BudgetはTooLarge、ReadはIoへ写し、破損sidecarを致命I/O扱いにしない |
| clipboard_capture/fetch | URL/data URL取得workerのAVIF（avis・compatible brandを含む）。既存1GiB/32768上限とWIC変換規則を維持、Susieは新設しない |
| app/book_composition_editor_prepare | ZIPの横長判定もAVIFは全entryを読み、64KiB prefixで容器が欠ける場合を回避。RAWと他形式prefix規則は維持 |
| fast_resize / app/metadata_ops | 先行寸法・一括編集・詳細情報。safe metadata APIで変換後寸法を取得、EXIFを重ねない |
| remote_ipc/container / thumbnail | 画素はcanonical / thumb_loader。別のfile/ZIP寸法probeもsafe metadata APIへ接続 |
| crates/remote-web/image_support | 旧 /api/image・/api/image-info。safe decode/dimensionsを共用し、認証・SourceReadEligibility・保存回転・AVIF passthrough条件を保持 |
| bin/probe_birefnet / gpu_lanczos_spike / diff_images | 任意ファイルを受ける開発用入力も共有decodeへ接続。既存非AVIFのEXIF扱いは維持 |
| crates/epub-pdf-worker/package | image/* spineのintrinsic寸法にsafe metadata APIを使用（1 thread）。ページ描画自体は従来のWebView2 |

### 第2ラウンドの再棚卸し（2026-10-20）

リポジトリ全体（`src/`・`crates/`・`tests/`、生成物/vendorを除く）を
`image::open` / `load_from_memory` / `image_dimensions` / `ImageReader` / `wic_` で再検索した。
AVIFが届く実行入口は上表。画素変換はsafe API一か所で、寸法だけの入口はdecodeせずmetadata解析する。
Remote ZIP寸法はAVIFだけ全entryを読み、他形式のprefix probeは維持する。

検索に残る直接呼出しの扱い:

| 残す場所 | 理由・入力契約 |
| --- | --- |
| image_decode、canonical production seam、remote-web/image_support、clipboard_capture/fetch | 共有境界内の既存image/WIC fallback実装。AVIFの内蔵失敗を記録してから進む |
| thumb_loaderのimage open / guessed-format retry、similar_imageのimage bytes | 共有primary closure内。AVIF成功時は呼ばない |
| fast_resize、metadata_ops、remote container、EPUB packageのImageReader | AVIF metadata分岐後の他形式ヘッダ取得 |
| audio_thumbnail::decode_picture_with_jpeg_decoder | APICはPNG/JPEGの明示的format制限。AVIF sidecarはthumb_loaderを通る |
| catalog、clipboard_capture/data、materializer・pipeline_debug等のテスト | 保存済みJPEG/WebPサムネイル、PNG clipboard、生成物/テスト出力の再読込 |
| lib::load_icon、tray、colorizeの固定アセット | include_bytesのPNG/JPEG、AVIF入力なし |
| comic_stamp::decode_raster | pickerはPNG/JPEG/WebP/GIF/BMP限定、埋め込みはPNG。既存同期UI呼出しあり（app::build_stamp_images / ui_text）で、新しいAVIF decodeは追加しない |
| bench_aiのimage open | RAW専用分岐。通常入力は共有decode済み |
| compare_alpha_upscale / gpu_lanczos_spike reference | 前者はPNG/WebP/TIFF限定列挙、後者は固定名PNG比較出力。任意source入力は共有decode済み |
| epub-pdf-worker/render::source_images | JPEG/PNGのPDF埋め込み品質照合だけ。spine寸法は別途接続済み |
| ort-patched/test_util、tests/、各cfg(test) fixture検査 | 既知形式の固定fixture/生成結果。製品の画像入力経路なし |

旧Remote APIの呼出しコードは残っている。`web/app.js` の画像要求と `imageInfo` は
`entry.address` がある場合はaddressed API、ない場合は `/api/image` / `/api/image-info` へ進む。
ただし現行の通常一覧は `http::api_list` が常にaddressを付け、集合一覧も
`add_collection_entry_addresses`、永続集合もstate.addressを設定するため、通常は新APIを使う。
旧APIはaddressのないentry用の互換分岐であり、現行の通常画面で常用しているとは確認していない。
server側の認証・最新の種類設定とsource資格の確認は変更しない。保存回転なし・要求幅が元幅以上・
元形式がブラウザ対応、というpassthrough条件も維持する。AVIFのprobe寸法は変換後、
再encodeする画素も内蔵変換後で、保存回転はその後に一度だけ適用する。

### セットアップと失敗契約

- setupのskipは期待VERSION一致に加え、header・static libraries 3本・licenses 3本の全7ファイルが必要。
  bootstrapはAVIF setupを毎回呼び、`--force`も伝える。ネットワークを要するsetupは本作業で実行しない。
- decoder ownerはNULL確認後の`NonNull`だけを保持。C shimのcloseもNULLを無視する。
- no-hint bytesはlibavifの`avifPeekCompatibleFileType`でmajor/compatible brandsのavif/avisを認識する。
- safe decodeはstraight RGBA8と型付きstage/code/diagを返す。破損・上限・読み取り失敗を区別しつつ
  既存fallbackへ進め、取消は既存の前後検査を維持する。
- Susie診断はAVIF/TIFF=内蔵→Windowsの表示機能→Susie、HEIC/HEIF/JPEG XL=Windowsの表示機能→Susie。
  UIのWIC略称は「Windows の表示機能」に統一する（統計表示も同様）。

### 自動検証・引き継ぎ

第1ラウンド: AVIF＋ライセンスfocused 22/22、関連69/69、UI比較11/11成功。
`cargo check`とfmt成功。Clippy exit0だが既存警告あり（本体2000件、lib test2768件、1788件重複）。
新設AVIF/helper/sysと当時の差分行には警告なし。exe増分・実画像速度は未測定。
その後、設計担当が`eea48ebac`で`test-lib-full.ps1`全13,892件成功を報告した。
これは第2ラウンド差分の全件検証ではない。

第2ラウンドは全コマンドに`MSBUILDDISABLENODEREUSE=1`、このsandboxのTurboJPEG用に
`CMAKE_GENERATOR=NMake Makefiles`を設定した。リポジトリのCargo設定は変更しない。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功（最終差分でも再確認） |
| `cargo build -p mimageviewer-remote --bin mimageviewer-remote` | 成功 |
| 同上 `--features embedded-web-assets` | 成功。PE32+ import表にavif/dav1d/yuv DLLなし（static指定はsys build.rs） |
| `cargo nextest run -p mimageviewer-remote` | 142成功、1 skip。AVIF画素/alpha/irot・旧画像API・保存回転・passthroughを含む |
| `cargo nextest run -p libavif-sys -p epub-pdf-worker -E 'test(/avif/)'` | 3成功（shim NULL close、確保前上限、EPUB intrinsic寸法） |
| `cargo nextest run -p mimageviewer --lib -E 'test(/avif/) \| test(~audio_thumbnail::) \| test(~raw_quality_sample_tests::) \| test(~canonical_image_loader::) \| test(~effetune::now_playing::artwork::) \| test(~fast_resize::) \| test(~clipboard_capture::fetch::) \| test(~raw_remote_file_and_zip_probes) \| test(~book_composition_editor_prepare::) \| test(~a_plain_image_file_reports_its_shape) \| test(~batch_file_dimension) \| test(~byte_probe)'` | 135成功。破損sidecar→正常APIC、品質比較file/ZIP、ZIP全件/代表保存worker境界、64KiBを超えるAVIFの製本横長判定を含む |
| UI snapshot: `cargo nextest run -p mimageviewer --test ui_snapshot -E 'test(~susie_diagnostic_) \| test(~avif_license_)'` | UPDATE_SNAPSHOTS=1で11成功、変更3枚を明暗・折返し・復帰履歴まで目視確認 |
| `cargo check -p mimageviewer --features dev-tools --bin diff_images --bin probe_birefnet` | 成功 |
| `python scripts/check_ui_glyphs.py` | dangerous glyph 0 |
| `cargo fmt` | 引数なしで実行、成功 |

最終のURL取得判定調整後は `cargo nextest run -p mimageviewer --lib -E
'test(/avif/) | test(~clipboard_capture::fetch::)'` で47成功（上記135件と重複あり）。
共有/EPUBも再度3成功、UI通常比較は最終11成功、`cargo fmt --check`も成功。
ログは`target/avif-r2-*.log`（gitignore）。

追加検査で分かった既存制限:

- `cargo check -p mimageviewer --features dev-tools --bin gpu_lanczos_spike`相当の合同checkは、
  変更していないwgpuコードの6件で失敗（InstanceDescriptor / RequestAdapterOptions /
  get_mapped_rangeのAPI不整合）。このbinのAVIF接続差分はsource画像decodeの1箇所のみ。
  本体・Remote・他2ツールの成功と分けて記録し、無関係なwgpu更新は本変更に含めない。
- `bash -n scripts/bootstrap-vendor.sh`はGit Bash自体のCreateFileMapping / Win32 error 5で起動不能。
  setupのネットワーク実行はしない。bootstrapの常時setup呼出し・force伝達はコードで照合した。
全件ゲートとbuild-devは設計担当が実行するため、本ラウンドでは行わない。製品は起動しない。

### 第2ラウンド変更ファイル

- 共有API・ビルド: `Cargo.lock`, `crates/epub-pdf-worker/Cargo.toml`, `crates/libavif-sys/Cargo.toml`, `crates/libavif-sys/shim/miv_avif.c`, `crates/libavif-sys/src/lib.rs`, `crates/remote-web/Cargo.toml`, `scripts/bootstrap-vendor.sh`。
- 本体・回帰テスト・UI: `src/app/book_composition_editor_prepare.rs`, `src/app/cache_ops.rs`, `src/app/metadata_ops.rs`, `src/audio_thumbnail/tests.rs`, `src/avif_decoder.rs`, `src/bin/diff_images.rs`, `src/bin/gpu_lanczos_spike.rs`, `src/bin/probe_birefnet.rs`, `src/clipboard_capture/fetch.rs`, `src/clipboard_capture/fetch_tests.rs`, `src/fast_resize.rs`, `src/image_decode.rs`, `src/remote_ipc/container.rs`, `src/thumb_loader.rs`, `src/ui_dialogs/stats_dialog.rs`, `src/ui_susie_diagnostic.rs`。
- Remote・EPUB: `crates/epub-pdf-worker/src/package.rs`, `crates/remote-web/src/image_support.rs`, `crates/remote-web/src/store.rs`。
- スナップショット: `tests/snapshots/susie_diagnostic_ready_after_recovery.png`, `tests/snapshots/susie_diagnostic_ready_with_plugins.png`, `tests/snapshots/susie_diagnostic_ready_with_plugins_dark.png`。
- 文書: `docs/architecture-overview.md`, `docs/avif-1407-implementation-results.md`, `docs/avif-builtin-decoder-plan.md`, `docs/display-pipeline.md`, `docs/native-dependencies.md`, `docs/web-remote-plan.md`。

`docs/avif-1407-implementation-results.md`は本節へ統合して削除。その他は変更。


### 第3ラウンド: ZIP一括生成のEXIF重複解析（2026-10-20）

- 第2ラウンドの全件/代表1枚の呼出元と共通helperが各1回解析し、JPEG DCT成功時も2回になっていた。
  呼出元はDCT画像と`ScaleStats`を対で渡すだけにし、共通helperで向きを1回解析して
  画素回転と`source_dims_after_exif`の両方へ使う。非DCTの既存向き規則も同じ境界で維持する。
- 内蔵AVIFの`Orientation::Applied`はEXIFを解析せず、変換済み画素/寸法を使う。
  RAWは従来のLibRaw分岐を維持。取消済み・decode失敗では新しい解析を始めない。
- `FnOnce`のreader seamで、DCT成功・JPEGフルdecodeへのfallback・PNGについて向き1〜8の各1回を固定する。
  同じ解析値でDCT元寸法が入れ替わること、適用済みAVIFと取消時は0回であることも検査する。

他入口の再確認（基点`a929143ef`、AVIF導入前の`797dbf905`とも比較）:

| 入口 | 結果 |
| --- | --- |
| thumb_loader: 通常path / verified bytes / ZIP / batch path・ZIP | 1回の解析値を画素とDCT寸法で再利用。decode_zip_chainは向き状態を返すだけで再解析しない |
| canonical / books / context_menu / panorama | `Decoded::oriented`を1回呼ぶだけ。共有decode内部に重ねたEXIF解析なし |
| metadata_ops / fast_resize / 製本prepare / core Remote寸法 | メタデータ取得の各段で最大1回。AVIFはsafe metadata APIの変換後寸法、EXIF再解析なし |
| now-playing artwork | 取得済みorientation値で回転。共通decodeからorientedを呼び直さない。bounded WIC自身のorientation処理は従来どおり |
| similar_imageのJPEG DCT / bench_dupeのJPEG DCT | 元寸法用read後にapply_exif_orientation_from_bytesが読む同型の2回あり。ただし両方とも797dbf905から存在し、今回のAVIF接続で追加したものではない。本P3修正には含めない |
| 旧Remote画像API | 非AVIFはprobeとdecodeの別段で各1回（元から）。内蔵AVIFは両段ともEXIFを読まない |

第3ラウンドの変更は`src/app/cache_ops.rs`と本節のみ。
`MSBUILDDISABLENODEREUSE=1`、`CMAKE_GENERATOR=NMake Makefiles`で検証した。

| コマンド | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功（既存警告あり） |
| `cargo nextest run -p mimageviewer --lib -E 'test(~app::cache_ops::) \| test(/avif/)'` | 33成功。新しい回数固定テストは3経路×向き8種、AVIF/取消時0回を含む |
| `cargo fmt` / `cargo fmt --check` | 成功 |

ログは`target/avif-r3-{check,focused,fmt,fmt-check}.log`（gitignore）。
コミット・製品起動・全件ゲート・build-devは行わない。
