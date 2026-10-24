# プロジェクト参照情報 (技術スタック・構成・対応形式)

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## Tech Stack

- **Language**: Rust (edition 2024, stable MSVC toolchain)
- **GUI**: eframe 0.37 + egui 0.37 (wgpu 30 backend)。Rust 1.99 を `rust-toolchain.toml` で固定 (MSRV 1.95)
- **Image decoding**: `image` crate (PNG, GIF, WebP, BMP) + `turbojpeg` (JPEG, libjpeg-turbo SIMD) + libavif + dav1d + libyuv (AVIF、静的リンク) + WIC (HEIC, JXL, TIFF) + LibRaw (RAW、CDDL-1.0、静的リンク)
- **JPEG 高速デコード**: `turbojpeg` クレート (libjpeg-turbo スタティックリンク、SIMD 最適化)。サムネ生成時は **DCT スケール (1/8〜1/1)** で decode して 5-30MB カメラ JPEG を 2.5-6× 高速化 ([docs/dct-scale-plan.md](dct-scale-plan.md))。圧縮入力 128MB 超は image クレート / WIC chain にフォールバック (並列ワーカー × 圧縮 buffer の積算メモリ圧迫を回避)。ビルドに cmake + NASM が必要。
- **Parallel loading**: `rayon` (dedicated thread pool per folder load)
- **Thumbnail cache**: SQLite via `rusqlite` (bundled), WebP encoding via `webp` crate
- **Video thumbnails**: Windows Shell API (IShellItemImageFactory)
- **Video inline playback**: `ffmpeg-the-third` クレート + FFmpeg LGPL shared DLL (BtbN ビルド) + `cpal` (WASAPI Shared 音声出力)。フルスクリーンで動画を MP4 / MKV / MOV / AVI / WMV / MPG / MPEG / HEVC / AV1 として再生する。`avcodec / avformat / avutil / avfilter / swscale / swresample` の 6 DLL を launcher (`crates/launcher/`) が core / remote service / EPUB converter / EffeTune Mixwright bundle とともに `include_bytes!` で内包し、初回起動時に `%APPDATA%/mimageviewer/runtime/<version>/` へ展開して本体 (`mimageviewer-core.exe`) を spawn する。本体側は exe と同じディレクトリの DLL を Windows ローダが解決するだけで個別ロード処理は持たない。ビルドに libclang (LLVM/Clang) が必要。詳細は「FFmpeg LGPL DLL 管理」節を参照
- **ZIP support**: `zip` crate
- **PDF support**: `pdfium-render` crate + PDFium DLL (exe に埋め込み) + マルチプロセスワーカープール (設定 3〜10、既定 5、1 つを Critical 予約)
- **EPUB conversion**: `crates/epub-pdf-worker` が WebView2 で DRM のない EPUB を PDF 化する。配布ビルドでは `mimageviewer-epub-pdf.exe` を署名後に launcher へ内包し、core と同じ versioned runtime へ展開する。portable 版は core exe の隣に loose 同梱する
- **PDF password**: `windows-dpapi` crate (DPAPI 暗号化でパスワード永続保存)
- **AI upscaling**: `ort` crate (ONNX Runtime v2、`load-dynamic` モード、`directml` + `cuda` + `tensorrt` features)。Real-ESRGAN / Real-CUGAN / NMKD-Siax ONNX モデルでタイル分割 4x アップスケール。バックエンドは Settings の `ai_backend` で DirectML / TensorRT を切替 (TRT は NVIDIA 専用)
- **ONNX Runtime DLL**: `onnxruntime.dll` / `onnxruntime_providers_shared.dll` (Microsoft.ML.OnnxRuntime.DirectML NuGet v1.24.2) を exe に `include_bytes!` で埋め込み、AI runtime 初期化 worker が `%APPDATA%/mimageviewer/` に展開して `ort::init_from()` で動的ロードする。Microsoft 公式 app-local VC runtime 4 本も全配布 exe の隣へ同梱するため、利用者による VC++ 再頒布可能パッケージの追加インストールは不要
- **TensorRT 対応 (NVIDIA GPU 高速化、オプション)**: `Microsoft.ML.OnnxRuntime.Gpu.Windows` + NVIDIA CUDA Runtime / cuBLAS / cuFFT / cuRAND / cuSOLVER / cuSPARSE / NVRTC / nvJitLink / cuDNN / TensorRT (合計 ~6.8 GB) を `%APPDATA%/mimageviewer/tensorrt/` に展開して使用。pack DL は `scripts/setup-tensorrt-pack.ps1` (PoC 版、アプリ内 DL UI は将来実装)。実測 1.4-3.4x (アップスケール) / 4.5x (デノイズ) 高速化。エンジンビルダーは `mimageviewer.exe --tensorrt-build <model>` 子プロセス。詳細は [docs/archive/ai/tensorrt-batching-feasibility.md](archive/ai/tensorrt-batching-feasibility.md)
- **AI image classification**: ヒューリスティクスでイラスト/漫画/CG/写真を自動判別
- **AI inpainting**: MI-GAN (ONNX, DirectML) を消しゴムツールから利用してマスク領域を補完
  （見開きページ中央欠落補完は精度不足で削除済み。タグ `v0.6.0-with-spread-inpaint` 参照）
- **AI model management**: exe に `include_bytes!` で埋め込み → 初回起動時に `%APPDATA%/mimageviewer/models/` に展開
- **Build tool**: cargo (MSVC toolchain on Windows) + cmake + NASM (TurboJPEG ビルドに必要)

## Project Structure

```
mimageviewer/
├── CLAUDE.md
├── docs/
│   ├── spec.md                     # 全体仕様書（実装状況チェックリスト付き）
│   ├── catalog-design.md           # サムネイルカタログ設計書
│   ├── thumbnail-memory-redesign.md # サムネイルメモリ管理 再設計メモ
│   └── dpi-multimonitor-issue.md   # マルチモニター DPI 問題調査
├── htdocs/
│   ├── index.html                  # mikage.to トップページ
│   └── mimageviewer/index.html     # mImageViewer 製品ページ
├── src/
│   ├── main.rs              # windows_subsystem + mimageviewer::run() の薄い入口
│   ├── lib.rs               # 単一 crate root + 起動処理 + 全モジュール宣言
│   ├── app.rs               # App 構造体 + eframe::App 実装
│   ├── ai/                  # AI 機能モジュール
│   │   ├── mod.rs           # ModelKind, ImageCategory, AiError 型定義
│   │   ├── runtime.rs       # ONNX Runtime (DirectML EP) セッション管理
│   │   ├── model_manager.rs # モデル埋め込み・展開・パス管理
│   │   ├── classify.rs      # 画像タイプ分類 (ヒューリスティクス)
│   │   ├── denoise.rs       # JPEG ノイズ除去推論
│   │   └── upscale.rs       # タイル分割 4x アップスケール推論
│   ├── ui_main.rs           # メイン画面 UI（グリッド描画）
│   ├── ui_fullscreen.rs     # フルスクリーン表示（ビューポート制御・描画 dispatch）
│   ├── ui_fullscreen/
│   │   └── draw_icons.rs    # 上部ホバーバー / 動画 HUD のアイコン・情報テキスト helper
│   ├── ui_helpers.rs        # UI ヘルパー関数
│   ├── ui_metadata_panel.rs # フルスクリーン メタデータパネル（AI + EXIF）
│   ├── ui_susie_diagnostic.rs # Susie プラグイン診断パネル描画（環境設定から切り出し、kittest でスナップショットテスト）
│   ├── ui_dialogs/          # ダイアログ群
│   │   ├── mod.rs
│   │   ├── preferences.rs        # 環境設定（状態・App 連携・ツリー / ページ dispatch）
│   │   ├── preferences/
│   │   │   └── pages.rs          # 環境設定の page_* 描画関数
│   │   ├── cache_manager.rs      # サムネイルキャッシュ管理
│   │   ├── archive_cache_manager.rs # 変換済みアーカイブキャッシュ管理（v0.7.0）
│   │   ├── archive_convert.rs    # 7z/LZH → ZIP 変換ダイアログ（v0.7.0）
│   │   ├── cache_creator.rs      # 一括キャッシュ作成
│   │   ├── thumb_quality.rs      # サムネイル画質 A/B 比較
│   │   ├── thumb_quality_fullscreen.rs
│   │   ├── toolbar_settings.rs   # ツールバーカスタマイズ
│   │   ├── favorites_editor.rs   # お気に入り編集
│   │   ├── fav_add.rs            # お気に入り追加
│   │   ├── open_folder.rs        # フォルダを開く
│   │   ├── context_menu.rs       # 右クリックコンテキストメニュー
│   │   ├── duplicate_settings.rs # 同名ファイル処理設定
│   │   ├── exif_settings.rs      # EXIF 表示設定
│   │   ├── slideshow_settings.rs # スライドショー設定
│   │   ├── rotation_reset.rs     # 回転情報リセット確認
│   │   └── stats_dialog.rs       # 統計
│   ├── png_metadata.rs      # AI 画像メタデータ読み取り（PNG tEXt/iTXt/zTXt）
│   ├── exif_reader.rs       # EXIF 読み取り（rexif クレート）
│   ├── rotation_db.rs       # 回転情報 DB（SQLite、非破壊回転）
│   ├── settings.rs          # 設定の読み書き API (Phase 3 で SQLite 経路に切替)
│   ├── settings_db.rs       # 設定永続化 SQLite バックエンド (settings.db、bak1..bak10 世代、JSON migration、boot decision tree、quarantine)
│   ├── catalog.rs           # SQLite サムネイルカタログ
│   ├── collection_store/    # 名前付きコレクション（v4.0.0）の永続化。単一 actor + immutable snapshot。
│   │                        # db/model/path/prepare/read_lease/runtime/text/export_all（collection.db、bak1..bak10）
│   ├── app/collection_grid.rs      # コレクション直下の一覧（items 生成・並べ替え・右クリック）
│   ├── app/collection_navigation.rs # コレクションの有効な順番でのページ送り・連続再生・親への復帰
│   ├── ui_dialogs/collections.rs   # コレクション管理ウィンドウ（作成/固定/削除/並べ替え/取り込み/書き出し）
│   ├── remote_ipc/persistent_collections.rs # mIV Remote への読み取り専用公開
│   ├── folder_tree.rs       # フォルダツリー走査ヘルパー
│   ├── folder_thumb_pins.rs # 親コンテナ（Folder/ZipFile/PdfFile）の代表サムネ手動ピン DB（v0.9.x、`#pin:` cache key suffix で identity を表現）
│   ├── grid_item.rs         # GridItem（Folder/Image/Video/Audio/ZipFile/PdfFile/ZipImage/ZipDir/PdfPage/Stack）/ ThumbnailState 定義
│   ├── filename_stack.rs    # ファイル名 prefix スタック（v2.0.0）の純ロジック（StackGroup/StackView/group_media/materialize_*）
│   ├── filename_stack_ui.rs # 上記の App グルー（トグル・集約⇔フラット切替・Shift+↓↑ ジャンプ）
│   ├── thumb_loader.rs      # サムネイル並列ロード
│   ├── wic_decoder.rs       # WIC 画像デコード（HEIC/JXL/TIFF と AVIF fallback。RAW は src/raw/）
│   ├── susie_loader.rs      # Susie プラグイン 32bit ワーカープール + IPC（v0.7.0、PI/MAG/Q0/PIC/MAKI…）
│   ├── os_theme.rs          # UI テーマ（System/Light/Dark）Windows レジストリ連携（v0.7.0）
│   ├── video/               # 動画インライン再生 (FFmpeg LGPL DLL)
│   │   ├── mod.rs           # VideoPlayer 公開 API (open / tick / seek / volume…)
│   │   ├── ffmpeg_loader.rs # DLL が exe と同居しているかを検証してログ出力 (展開・ロード自体は launcher と Windows ローダが担当)
│   │   ├── decoder.rs       # 動画/音声デコード worker (avformat/avcodec/swscale/swresample)
│   │   ├── audio.rs         # cpal (WASAPI) 経由の音声出力 + ring buffer
│   │   ├── clock.rs         # AvClock (薄い互換 facade、内部は engine/ に委譲)
│   │   └── engine/          # 動画再生エンジン (state machine + master clock 分割)
│   │       ├── mod.rs       # EngineEvent enum (Decoder/Audio events)
│   │       ├── actor.rs     # EngineActor (state machine、source of truth)
│   │       ├── state.rs     # EngineState / DecoderEvent / AudioEvent / ReadinessLatch
│   │       ├── clock.rs     # MasterClock + ClockAnchor (純粋値オブジェクト)
│   │       └── audio_bookkeeping.rs # 音声バッファ会計 (atomic、unit test 容易)
│   ├── video_thumb.rs       # 動画サムネイル取得（Windows Shell API）
│   ├── zip_loader.rs        # ZIP アーカイブ内画像列挙・読み込み（ZIP in ZIP 再帰列挙 + 非 ZIP アーカイブ検出フラグ、画像判定は is_recognized_image_ext 経由）
│   ├── archive_converter.rs # RAR/7z/LZH/(入れ子入り)ZIP → 無圧縮 ZIP 変換（unrar / sevenz-rust2 / delharc、入れ子アーカイブは再帰展開、v1.3.0）
│   ├── archive_cache.rs     # 変換済み ZIP のマッピング DB（SQLite）（v0.7.0）
│   ├── pdf_loader.rs        # PDF ページ列挙・レンダリング（PDFium）
│   ├── pdf_passwords.rs     # PDF パスワード DPAPI 暗号化保存
│   ├── fs_animation.rs      # アニメーション GIF / APNG デコード
│   ├── gpu_info.rs          # GPU 情報取得（VRAM サイズ等）
│   ├── monitor.rs           # モニター情報取得（DPI 等）
│   ├── stats.rs             # 読み込み統計
│   ├── undo_stack.rs        # メタ操作 (レーティング/タグ) Undo/Redo の純粋データ構造
│   ├── undo_ops.rs          # 上記スタックを App に impl するファサード（apply_meta_undo/redo 等）
│   ├── logger.rs            # パフォーマンス分析用ファイルロガー
│   └── bin/
│       └── bench_thumbs.rs  # サムネイル生成ベンチマーク
├── scripts/
│   ├── setup-pdfium.sh      # PDFium DLL ダウンロードスクリプト
│   ├── setup-ort.sh         # ONNX Runtime DirectML DLL ダウンロードスクリプト
│   ├── setup-susie-worker.sh # Susie 32bit ワーカーのビルド＆配置スクリプト
│   └── setup-ffmpeg.sh      # FFmpeg LGPL shared build (BtbN) ダウンロードスクリプト
├── vendor/
│   ├── pdfium/              # PDFium DLL（.gitignore、setup-pdfium.sh で取得）
│   │   └── bin/pdfium.dll   # include_bytes! で exe に埋め込まれる
│   ├── models/              # AI ONNX モデル（.gitignore、配布スクリプトなし）
│   │   └── *.onnx           # include_bytes! で exe に埋め込まれる。
│   │                        # 新規開発環境では %APPDATA%\mimageviewer\models\
│   │                        # (インストール済み環境が展開したもの) からコピーする
│   ├── ort/                 # ONNX Runtime DirectML DLL（.gitignore、setup-ort.sh で取得）
│   │   ├── onnxruntime.dll  # include_bytes! で exe に埋め込まれる
│   │   └── onnxruntime_providers_shared.dll
│   ├── susie-worker/        # 32bit Susie ワーカー exe（.gitignore、setup-susie-worker.sh で生成）
│   │   └── mimageviewer-susie32.exe  # include_bytes! で exe に埋め込まれ、
│   │                                 # 初回起動時に APPDATA へ自動展開
│   └── ffmpeg/              # FFmpeg LGPL shared build (.gitignore、setup-ffmpeg.sh で取得)
│       ├── bin/             # avcodec/avformat/avutil/avfilter/swscale/swresample DLL
│       ├── include/         # ffmpeg-the-third のビルド時参照 (FFMPEG_DIR)
│       ├── lib/             # import library (.lib)
│       └── LICENSE.txt      # LGPLv3-or-later 本文 (ソフトウェア情報に転載する)
├── Cargo.toml
└── Cargo.lock
```

## Implementation Phases

1. **Phase 1** ✅ — コアビューワー（グリッド・フルスクリーン・設定永続化）
2. **Phase 1.5** ✅ — サムネイルカタログ（SQLite + WebP）
3. **Phase 2** ✅ — AI アップスケール（ONNX Runtime + DirectML、Real-ESRGAN / Real-CUGAN / NMKD-Siax）+ 画像タイプ自動判別 + JPEG ノイズ除去 + 消しゴムツールでの MI-GAN 補完
4. **Phase 3** ✅ — お気に入り・ツールバー・ZIP・WIC・動画・アニメーション

## Supported Image Formats

- **内蔵**: JPEG, PNG, GIF, WebP, BMP
- **RAW（内蔵 LibRaw）**: DNG, CR2, CR3, NEF, NRW, ARW, SRF, SR2, RAF, ORF, RW2, PEF, PTX, RWL, IIQ, CRW, SRW, 3FR, ERF, KDC, DCR, MRW, MOS
  - Nikon HE/HE* と JPEG XL 圧縮 DNG は現像非対応。使える埋め込みプレビューは表示可能。
- **AVIF 内蔵**: libavif + dav1d + libyuv。RGBA8・clap/irot/imir 適用済み。失敗時は WIC → Susie
- **WIC 経由**: HEIC, HEIF, JXL, TIFF, TIF
- **動画（サムネイルのみ）**: MP4, AVI, MOV, MKV, WMV, MPG, MPEG

## Performance Notes

- **JPEG デコード**: TurboJPEG (SIMD) + **DCT スケール** で カメラ JPEG (5-30MB / 20-60MP) のサムネ生成を 2.5-6× 高速化。target_px (= max(display_px, thumb_px)) に応じて 1/8〜1/1 のスケール係数を選び、decoder 内側で縮小デコード。圧縮入力 128MB 超は image クレート (zune-jpeg) / WIC にフォールバック。詳細: [docs/dct-scale-plan.md](dct-scale-plan.md)
- **PDF レンダリング**: 5 ワーカープロセス並列で Cold 1441ms → 10ms (99% 改善)。各プロセスが独立に PDFium を初期化 (1 つは Enter 操作用に Critical 予約)
- **キャッシュ読み込み**: 2〜3ms/枚（WebP デコード）
- **キャンセル遅延**: 旧タスクが1枚のデコード中の場合、最大1デコード時間待つ
- **ログ**: `cargo run` 時に `mimageviewer.log` へ出力（.gitignore 済み）
- **ベンチマーク**: `docs/archive/performance-refactoring/bench-scroll-report.md` に詳細結果あり

## Screenshot Workflow

製品ページ用スクリーンショットの素材は `htdocs/mimageviewer/sozai/` に配置される。
ユーザーのディスプレイ環境はマルチモニターで、`mss` による全画面キャプチャが素材として提供される。

### モニター座標の特定方法

```python
# Python mss でモニター一覧を取得
import mss
with mss.mss() as sct:
    for i, m in enumerate(sct.monitors):
        print(f'mss monitor {i}: {m}')
```

mss monitor 0 は全モニターの合成（仮想全画面）。monitor 1以降が個別モニター。
左4Kモニター（プライマリ）が対象の場合、通常は `left=0, top=0` のモニターを探す。

### 切り出し座標の計算

mss の仮想座標系で全体画像の原点は `(monitors[0]['left'], monitors[0]['top'])`。
対象モニターが `left=L, top=T, width=W, height=H` のとき、
画像中の切り出し範囲は:

```
x0 = L - monitors[0]['left']
y0 = T - monitors[0]['top']
crop = img.crop((x0, y0, x0 + W, y0 + H))
```

### 実績値（2026-04 時点）

- mss monitor 0: `left=0, top=-1124, width=6001, height=3840`
- 左4Kモニター（monitor 3）: `left=0, top=0, width=3840, height=2160`
- → 切り出し: `img.crop((0, 1124, 3840, 3284))`
- 出力サイズ: 2560x1440 にリサイズ（既存 ss_fullscreen.png 等と統一）

詳細手順は `docs/screenshot-howto.md` を参照。
