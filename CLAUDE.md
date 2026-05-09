# mimageviewer - Project Context

## 作業開始時に必読

**修正作業を始める前に、必ず `docs/README.md` から関連する設計ドキュメントを開いて全体像を把握すること。**

このプロジェクトはサムネイル / フルスクリーン / 仮想フォルダ (ZIP/PDF) / 補正プリセット / AI /
消しゴムなど、複数のサブシステムが絡み合っている。片側だけ修正すると逆側で表示が崩れる、
補正結果が一瞬で消える、ZIP/PDF だけ動かない、といった手戻りが頻発する。

最低限、以下の 2 本はどんな修正でも目を通す:

- [docs/architecture-overview.md](docs/architecture-overview.md) — 全体のレイヤー構造・モジュールマップ
- [docs/README.md](docs/README.md) — ドキュメント索引 (領域別に何を読むべきか書いてある)

修正対象の領域に応じて追加で読むべきドキュメント:

| 触る領域 | 読むドキュメント |
| --- | --- |
| サムネイル / フルスクリーン描画 / 回転 / 表示変換 | [docs/display-pipeline.md](docs/display-pipeline.md) |
| ワーカー追加・キャッシュ・キャンセル処理 | [docs/async-architecture.md](docs/async-architecture.md) |
| UI スレッドから新しい同期 I/O / GPU アップロード / read_dir 走査を呼ぶ | [docs/ui-responsiveness.md](docs/ui-responsiveness.md) (§4 チェックリスト) |
| ZIP / PDF 対応が必要な機能 | [docs/virtual-folders.md](docs/virtual-folders.md) |
| 補正 / プリセット / AI アップスケール / 消しゴム | [docs/preset-and-adjustment.md](docs/preset-and-adjustment.md) |
| UI の見た目・配色を変える修正 | [docs/ui-snapshot-policy.md](docs/ui-snapshot-policy.md) (egui_kittest スナップショットの更新手順) |

**設計を変えたら該当ドキュメントも同時に更新する** (下の「コード修正時のドキュメント同時更新」参照)。

## Overview

A Windows 11 native image viewer built in Rust. Inspired by ViX (legacy 32-bit viewer),
modernized with GPU acceleration and AI upscaling. Single-window design replacing ViX's
dual-window approach.

## Tech Stack

- **Language**: Rust (edition 2024, stable MSVC toolchain)
- **GUI**: eframe 0.33 + egui 0.33 (wgpu backend)
- **Image decoding**: `image` crate (PNG, GIF, WebP, BMP) + `turbojpeg` (JPEG, libjpeg-turbo SIMD) + WIC (HEIC, AVIF, JXL, TIFF, RAW)
- **JPEG 高速デコード**: `turbojpeg` クレート (libjpeg-turbo スタティックリンク、SIMD 最適化)。5MB 以下のファイルに適用、大容量は `image` クレートにフォールバック。ビルドに cmake + NASM が必要。
- **Parallel loading**: `rayon` (dedicated thread pool per folder load)
- **Thumbnail cache**: SQLite via `rusqlite` (bundled), WebP encoding via `webp` crate
- **Video thumbnails**: Windows Shell API (IShellItemImageFactory)
- **Video inline playback**: `ffmpeg-the-third` クレート + FFmpeg LGPL shared DLL (BtbN ビルド) + `cpal` (WASAPI Shared 音声出力)。フルスクリーンで動画を MP4 / MKV / MOV / AVI / WMV / MPG / MPEG / HEVC / AV1 として再生する。`avcodec/avformat/avutil/swscale/swresample` の 5 DLL を `include_bytes!` で exe に埋め込み、初回起動時に `%APPDATA%/mimageviewer/ffmpeg/` に展開して `SetDllDirectoryW` 経由で動的ロード。ビルドに libclang (LLVM/Clang) が必要。詳細は「FFmpeg LGPL DLL 管理」節を参照
- **ZIP support**: `zip` crate
- **PDF support**: `pdfium-render` crate + PDFium DLL (exe に埋め込み) + マルチプロセスワーカープール (3 プロセス並列レンダリング)
- **PDF password**: `windows-dpapi` crate (DPAPI 暗号化でパスワード永続保存)
- **AI upscaling**: `ort` crate (ONNX Runtime v2、`load-dynamic` モード、`directml` + `cuda` + `tensorrt` features)。Real-ESRGAN / Real-CUGAN / NMKD-Siax ONNX モデルでタイル分割 4x アップスケール。バックエンドは Settings の `ai_backend` で DirectML / TensorRT を切替 (TRT は NVIDIA 専用)
- **ONNX Runtime DLL**: `onnxruntime.dll` / `onnxruntime_providers_shared.dll` (Microsoft.ML.OnnxRuntime.DirectML NuGet v1.24.2) を exe に `include_bytes!` で埋め込み、初回 AiRuntime 作成時に `%APPDATA%/mimageviewer/` に展開。`ort::init_from()` で動的ロードする。これにより VC++ 再頒布可能パッケージ不要
- **TensorRT 対応 (NVIDIA GPU 高速化、オプション)**: `Microsoft.ML.OnnxRuntime.Gpu.Windows` + NVIDIA CUDA Runtime / cuBLAS / cuFFT / cuRAND / cuSOLVER / cuSPARSE / NVRTC / nvJitLink / cuDNN / TensorRT (合計 ~6.8 GB) を `%APPDATA%/mimageviewer/tensorrt/` に展開して使用。pack DL は `scripts/setup-tensorrt-pack.ps1` (PoC 版、アプリ内 DL UI は将来実装)。実測 1.4-3.4x (アップスケール) / 4.5x (デノイズ) 高速化。エンジンビルダーは `mimageviewer.exe --tensorrt-build <model>` 子プロセス。詳細は [docs/tensorrt-batching-feasibility.md](docs/tensorrt-batching-feasibility.md)
- **AI image classification**: deepghs/anime_classification MobileNetV3 (ONNX) + ヒューリスティクス。イラスト/漫画/CG/写真を自動判別
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
│   ├── main.rs              # エントリポイント + フォント設定 + logger::init()
│   ├── app.rs               # App 構造体 + eframe::App 実装
│   ├── ai/                  # AI 機能モジュール
│   │   ├── mod.rs           # ModelKind, ImageCategory, AiError 型定義
│   │   ├── runtime.rs       # ONNX Runtime (DirectML EP) セッション管理
│   │   ├── model_manager.rs # モデル埋め込み・展開・パス管理
│   │   ├── classify.rs      # 画像タイプ分類 (MobileNetV3 + ヒューリスティクス)
│   │   ├── denoise.rs       # JPEG ノイズ除去推論
│   │   └── upscale.rs       # タイル分割 4x アップスケール推論
│   ├── ui_main.rs           # メイン画面 UI（グリッド描画）
│   ├── ui_fullscreen.rs     # フルスクリーン表示（上部ホバーバー含む）
│   ├── ui_helpers.rs        # UI ヘルパー関数
│   ├── ui_metadata_panel.rs # フルスクリーン メタデータパネル（AI + EXIF）
│   ├── ui_susie_diagnostic.rs # Susie プラグイン診断パネル描画（環境設定から切り出し、kittest でスナップショットテスト）
│   ├── ui_dialogs/          # ダイアログ群
│   │   ├── mod.rs
│   │   ├── preferences.rs        # 環境設定（表示・パフォーマンス・フォルダ・ファイル処理・UI テーマ・Susie プラグイン…）
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
│   ├── settings.rs          # 設定の読み書き（JSON 永続化）
│   ├── catalog.rs           # SQLite サムネイルカタログ
│   ├── folder_tree.rs       # フォルダツリー走査ヘルパー
│   ├── grid_item.rs         # GridItem（Folder/Image/Video/ZipFile/PdfFile/ZipImage/PdfPage/ZipSeparator）/ ThumbnailState 定義
│   ├── thumb_loader.rs      # サムネイル並列ロード
│   ├── wic_decoder.rs       # WIC 画像デコード（HEIC/AVIF/JXL/TIFF/RAW）
│   ├── susie_loader.rs      # Susie プラグイン 32bit ワーカープール + IPC（v0.7.0、PI/MAG/Q0/PIC/MAKI…）
│   ├── os_theme.rs          # UI テーマ（System/Light/Dark）Windows レジストリ連携（v0.7.0）
│   ├── video/               # 動画インライン再生 (FFmpeg LGPL DLL)
│   │   ├── mod.rs           # VideoPlayer 公開 API (open / tick / seek / volume…)
│   │   ├── ffmpeg_loader.rs # DLL を APPDATA に展開し SetDllDirectoryW で検索パス設定
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
│   ├── zip_loader.rs        # ZIP アーカイブ内画像列挙・読み込み（ZIP in ZIP フラット展開、画像判定は is_recognized_image_ext 経由）
│   ├── archive_converter.rs # 7z/LZH → 無圧縮 ZIP 変換（sevenz-rust2 / delharc）（v0.7.0）
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
│       ├── bin/             # avcodec/avformat/avutil/swscale/swresample DLL
│       ├── include/         # ffmpeg-the-third のビルド時参照 (FFMPEG_DIR)
│       ├── lib/             # import library (.lib)
│       └── LICENSE.txt      # LGPLv3-or-later 本文 (ソフトウェア情報に転載する)
├── Cargo.toml
└── Cargo.lock
```

## vendor/ 一括セットアップ (新規 clone / vendor/ 消失時の復旧)

`vendor/` 配下は全て `.gitignore` 対象 (DLL / SDK / モデル等を git に入れないため)。
新規 clone した直後や、誤って `vendor/` を消してしまった場合は、ビルド前に必要
ファイルを揃え直す必要がある。`build.rs` が起動時に必須ファイルの存在を検証して
復旧手順付きでエラーを出すので、**何が足りないかは cargo build のエラーで分かる**。

### ワンコマンドで揃える

```bash
bash scripts/bootstrap-vendor.sh           # 不足分のみ取得
bash scripts/bootstrap-vendor.sh --force   # 既存ファイルも再取得 (デバッグ用)
```

このスクリプトは以下を順に実行する:

1. `setup-pdfium.sh` — `vendor/pdfium/bin/pdfium.dll` を取得
2. `setup-ort.sh` — `vendor/ort/onnxruntime*.dll` を取得
3. `setup-ffmpeg.sh` — `vendor/ffmpeg/{bin,include,lib}/` を取得
4. `setup-susie-worker.sh` — `vendor/susie-worker/mimageviewer-susie32.exe` を再ビルド
5. `vendor/models/*.onnx` を `%APPDATA%/mimageviewer/models/` から自動 copy
   (= 一度 mIV をインストール / 起動して APPDATA に展開させた後でないと取れない)

### bootstrap で取れないもの

- **`vendor/vst3-host/mimageviewer-vst3-host.exe`**: VST3 SDK の DL が ~490 MB と
  大きいので bootstrap には含めていない。以下のいずれかで配置する:
  - **既存ビルド済み exe をコピー** (推奨): 別 worktree やバックアップに残っている
    `mimageviewer-vst3-host.exe` を `vendor/vst3-host/` に置く。SDK 不要で即解決
  - **CMake で再ビルド**:
    ```bash
    bash scripts/setup-vst3-sdk.sh
    cmake -S crates/vst3-host -B crates/vst3-host/build -G "Visual Studio 18 2026" -A x64
    cmake --build crates/vst3-host/build --config Release
    cp crates/vst3-host/build/Release/mimageviewer-vst3-host.exe vendor/vst3-host/
    ```
    詳細は本書「VST3 host bridge 管理」節を参照
- **TensorRT pack** (`vendor/tensorrt-cache/`): 開発時には不要。pack 配布作業時
  だけ `scripts/setup-tensorrt-pack.ps1` を回す

### 前提環境変数 / ツール

- `gh` (GitHub CLI) 認証済み — pdfium / ffmpeg の release 取得に必要
- `rustup target add i686-pc-windows-msvc` 済み — susie 32bit ワーカービルド
- `LIBCLANG_PATH` 環境変数登録済み — FFmpeg の bindgen 用 (本書「FFmpeg LGPL DLL
  管理」節に詳細)
- `cmake` + Visual Studio 2026 (18) BuildTools + NASM — TurboJPEG / vst3-host ビルド
- `unzip`, `tar`, `curl` — setup スクリプト群が使う

## Implementation Phases

1. **Phase 1** ✅ — コアビューワー（グリッド・フルスクリーン・設定永続化）
2. **Phase 1.5** ✅ — サムネイルカタログ（SQLite + WebP）
3. **Phase 2** ✅ — AI アップスケール（ONNX Runtime + DirectML、Real-ESRGAN / Real-CUGAN / NMKD-Siax）+ 画像タイプ自動判別 + JPEG ノイズ除去 + 消しゴムツールでの MI-GAN 補完
4. **Phase 3** ✅ — お気に入り・ツールバー・ZIP・WIC・動画・アニメーション

## Key Design Decisions

### UI / スクロール
- **Virtual scrolling**: `show_viewport` で全体高さだけ確保し、可視行のみ描画。
  スクロールオフセットは App が自前管理（egui の自動スクロールは使わない）。
- **Row snapping**: オフセットは常に `cell_size` の整数倍。最大オフセットも
  `ceil((total_h - viewport_h) / cell_size) * cell_size` で行境界に揃える。
- **Mouse wheel**: `ctx.input_mut` で MouseWheel イベントを消費し、1行分に変換。

### ダイアログ (egui::Window)
- **ドラッグ移動**: `anchor()` を使うとウィンドウが固定されドラッグできなくなる。
  必ず `default_pos()` を使う。定番の初期位置は `ctx.content_rect().min + egui::vec2(60.0, 40.0)`。
- **閉じるボタン**: `.open(&mut open)` でタイトルバーに × ボタンが付く。
  `open` が `false` になったら `show_*` フラグを落とす。
- **パターン**: `ui_dialogs/` に 1 ファイル 1 メソッドで追加。
  `mod.rs` に `mod xxx;` を追加し、`app.rs` の `update()` 内で `self.show_xxx(ctx)` を呼ぶ。
  `App` 構造体に `show_xxx: bool` フィールドを追加し、`Default` impl で `false` 初期化。

### IME 対応 (日本語入力) ⚠️ 重要
TextEdit を含むダイアログで Enter / Escape を拾うときは **必ず専用ヘルパーを使う**こと。
直接 `ctx.input(|i| i.key_pressed(Key::Enter/Escape))` を呼ぶと、**日本語 IME 変換中の Enter
(変換確定) や Escape (変換キャンセル) をダイアログが奪ってしまい、変換が破壊される**。

- **確定用**: `self.dialog_enter_pressed(ctx)` — IME 変換中は常に false
- **キャンセル用**: `self.dialog_escape_pressed(ctx)` — IME 変換中は常に false
- **判定ロジック**: `App::ime_input_active()` は `ime_composing` フラグ (Ime イベントで更新) と
  直近 300ms 以内の Ime イベント有無の OR で判定。300ms グレースは Windows IME で
  `Ime::Disabled` と `Key::Escape` が別フレームに届くケースを吸収するため。

**ビューポート別のイベントキュー**:
egui の `show_viewport_immediate` は独立したイベントキューを持つ。メインビューポートと
フルスクリーンビューポートは別キュー。IME 状態はビューポートごとに追跡が必要なので、
`App::update_ime_state(ctx)` は **各ビューポートの入り口**で呼ばれている:
- メイン: [src/app.rs](src/app.rs) の `App::update` 先頭
- フルスクリーン: [src/ui_fullscreen.rs](src/ui_fullscreen.rs) の `show_viewport_immediate` closure 先頭

新しいビューポートを追加した場合、closure 先頭で `self.update_ime_state(ctx)` を必ず呼ぶこと。

**借用の注意**:
`egui::Window::show(ctx, |ui| {...})` の closure 内で `self` 経由のメソッド呼び出しは
借用衝突になりやすい。`dialog_enter_pressed` / `dialog_escape_pressed` は closure の**前**で
ローカル変数にキャプチャしてから closure 内で参照する:
```rust
let enter_pressed = self.dialog_enter_pressed(ctx);
let escape_pressed = self.dialog_escape_pressed(ctx);
egui::Window::new("...").show(ctx, |ui| {
    if response.lost_focus() && enter_pressed { ... }
    if escape_pressed { cancel = true; }
});
```
深いネスト (例: `preferences.rs` の `draw_page` → `page_exif_display`) では一時構造体
(`PreferencesState::enter_pressed` 等) のフィールドに載せて伝搬する。

### サムネイルロード
- **Grid contents**: フォルダ・ZIP・PDF 先頭（名前順）、画像後続（ソート順設定可）。
  ZIP/PDF ファイルは 1 枚目/1 ページ目のサムネイル＋種別バッジで表示。非画像は無視。
- **Cancellation**: `Arc<AtomicBool>` キャンセルトークン。`load_folder` 呼び出し時に
  旧トークンを `true` にして旧タスクを中断。
- **Per-load thread pool**: フォルダごとに新規 `rayon::ThreadPool` を作成。
  旧フォルダのプールと競合せず新タスクが即座に開始できる。
- **Priority loading**: Phase1（可視範囲）→ Phase2（残り）の2フェーズ並列処理。
- **Repaint loop**: `Pending` なサムネイルがある間は毎フレーム `ctx.request_repaint()`。
- **Page-based eviction**: 前後数ページ分のみ GPU メモリに保持、範囲外は Evicted。
- **Cache**: SQLite に WebP (q=75) で保存。Off / Auto / Always の 3 モード。

### フォルダ走査
- **Folder tree navigation (Ctrl+↑↓)**: 深さ優先前順トラバーサル。
  次 = 最初の子 → 次の兄弟 → 祖先の次の兄弟（再帰）。
  前 = 前の兄弟の最後の子孫 → 親。
- **BS key**: 親フォルダへ。
- **Path comparison**: Windows の大文字小文字非区別に対応するため小文字化して比較。
- **AppleDouble 除外**: macOS/iPhone 由来の `._*` ファイルを自動除外。

### セキュリティ
- `image` クレート（純粋Rust、メモリ安全）で画像デコード。
- HEIC/AVIF/JXL/TIFF/RAW は WIC 経由（`unsafe` ブロックに局所化）。
- ONNX Runtime (ort crate) 経由の AI 推論は safe Rust API。DirectML EP で GPU アクセラレーション。

### 並行処理: try_lock + sleep は使わない ⚠️

「`Mutex::try_lock` に失敗したら sleep して再試行」というループは **飢餓 (starvation) を
起こす既知のアンチパターン**。2026-04 に PDF ワーカープールで Critical 要求が 10 秒
ブロックされる実害が発生した (詳細は [docs/async-architecture.md §5.5](docs/async-architecture.md))。

- 複数スレッドが同じリソースを取り合う場合は **`Mutex + Condvar` で保護した優先度キュー +
  専用ディスパッチャースレッド** の構造にする
- リソース利用者は Job を enqueue して `mpsc::Receiver` で応答待ち、ディスパッチャーは
  `Condvar::wait` で起床してキューから pop する
- 実例: `src/pdf_loader.rs` の `PdfWorkerPool` / `JobQueue` / `run_dispatcher`
- `try_lock` 自体は「取れなければ今回は諦める」best-effort 用途のみ OK

### UI スレッドでの同期 I/O は即 worker 化する ⚠️

`App::update` から (呼び出し先を含めて) 同期実行される処理で以下を行うと、
Ctrl+↑↓ 連打や Ctrl+F 検索で UI が引っかかる。**新機能追加時は必ず
[docs/ui-responsiveness.md](docs/ui-responsiveness.md) §4 のチェックリストを通す**。

禁止 / worker 化必須な処理:

- **ファイル全体を読む** (`std::fs::read`, `File::open` + `read_to_end`): 画像デコード・
  XMP/EXIF/PNG メタ抽出など。worker thread + mpsc + cancel トークンに移す。
  実例: `start_fs_load`, `start_metadata_load`, `execute_search`。
- **`ctx.load_texture` を 1 フレームに複数回**: 20MP RGBA で 26-58ms/枚。`fs_upload_backlog`
  パターンで 1 フレーム 1 枚、現在ページのみ即時にする。
- **`std::fs::read_dir` ループで `Path::is_dir()` / `is_file()`**: Windows で per-entry
  `GetFileAttributes` syscall が走り、数百ファイルで 500-1000ms ブロック。
  **必ず `entry.file_type()` を使う** (`FindFirstFile` のキャッシュ再利用)。
- **SQLite の `CatalogDb::open` 相当の cold open**: warm で 5ms 以下だが cold で
  150ms 超のことあり。毎ステップ走る箇所は避ける。
- **DFS フォルダ走査**: `navigate_folder_with_skip` は HDD で 1 秒超の事例あり。
  `spawn_folder_nav` のように別スレッドで走らせる。

worker 化の定型パターン (`XxxPending { cancel, rx }` + `start_xxx` / `poll_xxx` + 3 箇所
cancel) は [docs/ui-responsiveness.md §2](docs/ui-responsiveness.md) に実装テンプレを
載せている。

### レビュー時: UI 応答性の性能観点

レビューでは、差分に次の呼び出しが出たら **UI スレッドから同期到達しないか** を必ず確認する。
`App::update`、描画関数、入力ハンドラ、`open_fullscreen`、`load_folder`、`execute_*`、
`ensure_*` / `start_*` の呼び出し元まで追うこと。

- `std::fs::read`, `File::open`, `read_to_end`, `std::fs::read_dir`, `metadata`,
  `Path::is_dir` / `Path::is_file`
- `xmp_reader::read_tweet_info`, `png_metadata::extract_*`, `exif_reader::read_*`,
  `zip_loader::read_entry_bytes`
- `ctx.load_texture`
- `rusqlite` / `search_index_db` / catalog DB の open・全件 load・search

UI 同期なら、worker 化・キャンセル・結果適用時の世代/idx 整合・1 フレーム予算・perf 計装が
揃っていることを確認する。特に `decide_partial` のような lazy 化でも、cheap miss や
除外トークンだけの検索など「追加情報が必要になる最悪ケース」で UI を止めないかを見る。

測定は `--perf-log` + `python scripts/analyze_perf.py <path> {startup,nav,hitches}` で。
追加した同期処理の区間には perf::event を必ず差し込む (悪化を検知できるように)。

**ユーザーが perf-log を取って解析を依頼してきた場合**は、まず以下の場所を確認する:

```
%APPDATA%\mimageviewer\logs\
  ├ perf_events.jsonl       # 直近の perf-log (--perf-log の既定出力先)
  ├ perf_*.jsonl            # 任意名で取った過去ログ
  ├ mimageviewer.log        # 通常の logger 出力
  └ panic.log               # クラッシュログ
```

(Windows の絶対パス: `C:\Users\<user>\AppData\Roaming\mimageviewer\logs\`)。
ユーザーが「perf-log 取りました」とだけ伝えてきたらこのディレクトリの最新
`perf_*.jsonl` をそのまま `analyze_perf.py` に食わせる。パスを毎回聞かない。

## Supported Image Formats

- **内蔵**: JPEG, PNG, GIF, WebP, BMP
- **WIC 経由**: HEIC, HEIF, AVIF, JXL, TIFF, TIF, DNG, CR2, CR3, NEF, NRW, ARW, SRF, SR2, RAF, ORF, RW2, PEF, PTX, RWL, IIQ
- **動画（サムネイルのみ）**: MP4, AVI, MOV, MKV, WMV, MPG, MPEG

## Performance Notes

- **JPEG デコード**: TurboJPEG (SIMD) で小〜中 JPEG を 1.5-2.4 倍高速化。5MB 超は image クレート (zune-jpeg) にフォールバック
- **PDF レンダリング**: 3 ワーカープロセス並列で Cold 1441ms → 10ms (99% 改善)。各プロセスが独立に PDFium を初期化
- **キャッシュ読み込み**: 2〜3ms/枚（WebP デコード）
- **キャンセル遅延**: 旧タスクが1枚のデコード中の場合、最大1デコード時間待つ
- **ログ**: `cargo run` 時に `mimageviewer.log` へ出力（.gitignore 済み）
- **ベンチマーク**: `docs/bench-scroll-report.md` に詳細結果あり

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

## PDFium 管理

PDF サポートは PDFium ライブラリ (Google Chrome の PDF エンジン) を使用する。
DLL は exe に `include_bytes!` で埋め込まれ、初回起動時に
`%APPDATA%\mimageviewer\pdfium.dll` に展開される。

### マルチプロセス並列レンダリング

PDFium はスレッドセーフではないため、マルチプロセスで並列化している。
`mimageviewer.exe --pdf-worker` で起動したワーカープロセス (デフォルト 3 個) が
各自独立に PDFium を初期化し、stdin/stdout バイナリプロトコルでメインプロセスと通信する。
ワーカーは GUI を持たず、メインプロセス終了時に自動終了する。

### セットアップ

```bash
bash scripts/setup-pdfium.sh        # DLL をダウンロード (vendor/pdfium/bin/pdfium.dll)
bash scripts/setup-pdfium.sh check  # 新しいバージョンの有無を確認
```

- **ソース**: [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)
  (毎週月曜に Chromium 最新版から自動ビルド)
- **アセット**: `pdfium-win-x64.tgz` (V8 なし版、軽量)
- **現在のバージョン**: `vendor/pdfium/VERSION` を参照

### リリース前チェック (必須)

**リリースビルドの前に必ず以下を確認すること:**

1. `bash scripts/setup-pdfium.sh check` を実行し、新しいバージョンがないか確認
2. 新しいバージョンがある場合は `bash scripts/setup-pdfium.sh` で更新
3. 更新後は PDF の表示が正常か動作確認してからリリース

## ONNX Runtime 管理

AI 機能 (アップスケール・ノイズ除去・画像分類・消しゴム) は ONNX Runtime + DirectML EP を
使用する。`ort` クレートの `load-dynamic` 機能で、メイン exe は静的リンクせず、起動時に
`%APPDATA%\mimageviewer\onnxruntime.dll` を動的ロードする。DLL は PDFium と同様
`include_bytes!` で exe に埋め込み、初回 `AiRuntime::new()` で APPDATA へ展開する。

この方式の利点:
- Visual C++ 再頒布可能パッケージへの依存が不要になる (VCRUNTIME140.dll 等が消える)
- 利用者は単体 exe 版・インストーラ版どちらでも追加セットアップ不要

### セットアップ

```bash
bash scripts/setup-ort.sh           # 既定バージョン (1.24.2) をダウンロード
bash scripts/setup-ort.sh 1.24.5    # バージョン指定
```

- **ソース**: NuGet `Microsoft.ML.OnnxRuntime.DirectML` (Microsoft 公式ビルド)
- **CDN**: `https://globalcdn.nuget.org/packages/microsoft.ml.onnxruntime.directml.<VERSION>.nupkg`
- **現在のバージョン**: `vendor/ort/VERSION` を参照

### バージョン互換性

`ort` クレートの C API バージョンと一致する ONNX Runtime DLL を使う必要がある。
`ort-sys` の `build/download/dist.txt` に `ms@X.Y.Z` というタグが入っており、これが
対応する ONNX Runtime バージョン。ort クレートをアップデートしたら、このバージョンを
確認して `setup-ort.sh` のデフォルトバージョン (`VERSION` 変数) を揃えること。

- ort 2.0.0-rc.12 ↔ onnxruntime 1.24.2

### DirectML.dll について

Windows 10 1903 (2019年5月) 以降は `DirectML.dll` が OS に標準同梱されているので、
個別に配布する必要はない。`onnxruntime.dll` は System32 の DirectML.dll を自動検出する。

## Susie 32bit ワーカー管理

Susie 画像プラグイン (`.spi`, 32bit DLL) は 64bit メインプロセスから直接ロードできないため、
`mimageviewer-susie32.exe` という 32bit のワーカープロセスを子プロセスとして起動して使う。

### exe 埋め込み + APPDATA 展開方式 (PDFium と同じパターン)

- 32bit ワーカーは `include_bytes!` でメインの `mimageviewer.exe` に埋め込む。
- 初回起動時に `%APPDATA%\mimageviewer\mimageviewer-susie32.exe` に展開される。
- インストール先 (`Program Files`) には追加ファイルを置かない。
- 展開時のサイズ一致チェックで、同一バイナリなら書き戻しをスキップ。

### セットアップ (メインビルド前に必須)

```bash
bash scripts/setup-susie-worker.sh
# 内部で以下を実行:
#   cargo build --release --target i686-pc-windows-msvc -p mimageviewer-susie32
#   cp target/i686-pc-windows-msvc/release/mimageviewer-susie32.exe vendor/susie-worker/
```

- 前提: `rustup target add i686-pc-windows-msvc` 済みであること。
- 出力: `vendor/susie-worker/mimageviewer-susie32.exe` (.gitignore)。
- メイン exe のリリースビルド前には **必ず** 実行する。未実行だとコンパイル時に
  `include_bytes!` が失敗する。

## FFmpeg LGPL DLL 管理

動画インライン再生 (フルスクリーンで MP4 / MKV / MOV / AVI / WMV / MPG / MPEG /
HEVC / AV1 を再生する) のために、FFmpeg の **LGPL shared build** を vendor に置く。

⚠️ **配布形態はランチャー方式**。`ffmpeg-the-third` は MSVC import library 経由で
通常リンクされ、Windows ローダが exe ロード時 (= Rust コードが走るより前) に DLL を
解決するため、PDFium / ONNX Runtime のような「include_bytes! → APPDATA 展開」方式は
直接の本体には適用できない (ローダの解決タイミングに間に合わない)。`/DELAYLOAD` も
rustc 経由の link.exe で機能しない (Delay Import Directory が空のまま、原因未解明)。

そこで **ランチャー + 本体の 2 段構成** で「単体 exe 配布」を実現している:

```
配布する mimageviewer.exe (= ランチャー、crates/launcher/ が生成)
├── include_bytes! で内包:
│   ├── mimageviewer-core.exe   (本体、ffmpeg-the-third を import library リンク)
│   ├── avcodec-61.dll
│   ├── avformat-61.dll
│   ├── avutil-59.dll
│   ├── swscale-8.dll
│   └── swresample-5.dll
└── 起動時の動作:
    1. %APPDATA%\mimageviewer\runtime\<version>\ に上記 6 ファイルを展開
       (サイズ一致チェックでスキップ、不一致なら .tmp → atomic rename)
    2. std::process::Command で mimageviewer-core.exe を spawn (引数 forward)
    3. ランチャー即終了 (GUI なので exit code を待たない)
```

ランチャーは **FFmpeg API を一切呼ばない** ので Windows ローダの DLL 解決問題に
直撃しない。core を spawn する時点で展開済み DLL が同じディレクトリにあるので、
Windows の DLL 検索順 (exe 同居が最優先) で確実に解決される。

**バージョン別 runtime ディレクトリ**: `runtime\<version>\` のように分けることで、
古い core が走行中に新ランチャーが上書きしようとして file lock で失敗する事象を回避
(Codex レビュー助言)。古いバージョンの runtime ディレクトリはユーザーが手動で
削除可能 (将来的にランチャー側で「最新 N 世代だけ残す」掃除処理を追加するかも)。

**ビルド順序**: cargo は同一ワークスペース内 bin の依存順序を表現できないので
`scripts/build-release.{sh,ps1}` が 2 段階に分けて呼ぶ:
1. `cargo build --release --bin mimageviewer-core` → 本体生成
2. `cargo build --release --bin mimageviewer` → ランチャー生成 (本体を include_bytes!)

`cargo build --release` を直接打つ場合は ① → ② の順で 2 回打つこと。
ランチャー側 build.rs が `target/release/mimageviewer-core.exe` の存在をチェックし、
無ければ復旧手順付きで止まる。

**配布物**:
- 単体 exe 版: `mimageviewer.exe` 1 ファイル (約 365MB、内包する core + DLL を含む)
- インストーラ版: `mImageViewer_setup.exe` (Inno Setup が同じランチャーを配置)
- どちらも初回起動時に APPDATA に展開、2 回目以降は展開済みなのでスキップして高速

### セットアップ (メインビルド前に必須)

```bash
bash scripts/setup-ffmpeg.sh           # BtbN の n7.1 系 LGPL shared build を取得
bash scripts/setup-ffmpeg.sh check     # 新版があるか確認のみ
```

- 取得元: [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases)
  の `ffmpeg-n7.1*-win64-lgpl-shared-7.1.zip`
- 出力先:
  - `vendor/ffmpeg/bin/{avcodec,avformat,avutil,swscale,swresample}-*.dll`
    — `include_bytes!` で exe に埋め込み
  - `vendor/ffmpeg/include/`, `vendor/ffmpeg/lib/`
    — `ffmpeg-the-third` の build.rs が `FFMPEG_DIR` 経由で参照
    (`.cargo/config.toml` に `FFMPEG_DIR=vendor/ffmpeg` を設定済み)
  - `vendor/ffmpeg/LICENSE.txt` — LGPLv3-or-later の本文
- 前提: `gh` (GitHub CLI), `unzip`, `curl` が PATH にあること。
  さらに `ffmpeg-sys-the-third` のビルドに **libclang (LLVM/Clang)** が必要。
  Visual Studio Installer の「C++ Clang コンパイラ」コンポーネントを入れるか、
  独立した [LLVM インストーラ](https://github.com/llvm/llvm-project/releases) を使う。
- **環境変数 `LIBCLANG_PATH` を必ず永続登録すること** (一度だけでよい):
  ```powershell
  [Environment]::SetEnvironmentVariable(
      "LIBCLANG_PATH",
      "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin",
      "User"
  )
  ```
  パスは VS のエディションによって変わる: `Microsoft Visual Studio\<バージョン>\<エディション>\VC\Tools\Llvm\x64\bin\libclang.dll` を `find`/`Get-ChildItem -Recurse` で探して使う。
  登録後は **PowerShell を開き直す** (現セッションだけ反映するなら `$env:LIBCLANG_PATH = "..."`)。
  未設定だと `cargo build` が `Cannot find clang: 'clang.dll', 'libclang.dll'` で失敗する。

### バージョンを上げる場合

FFmpeg メジャーが上がると DLL のメジャー番号が変わる (例: 7.x の `avcodec-61.dll` →
8.x の `avcodec-62.dll`)。以下を **3 箇所すべて** 揃えて変更する:

- `scripts/setup-ffmpeg.sh` の `ASSET_GLOB` (例: `ffmpeg-n8.0*-win64-lgpl-shared-8.0.zip`)
- `src/video/ffmpeg_loader.rs` の `include_bytes!` パスと `DLLS` 配列のファイル名
- `build.rs` の `vendor/ffmpeg/bin/avcodec-XX.dll` 等のチェックパス
- `build.rs` の `/DELAYLOAD:avcodec-XX.dll` 等のリンカフラグ (DLL 名一致が必須)

### 動作確認

リリースビルド成果物:
```
target/release/mimageviewer.exe          # 配布用 (ランチャー、~365MB)
target/release/mimageviewer-core.exe     # 本体 (内包される実体、単体では起動不可)
```

`mimageviewer.exe` をダブルクリックすると:
1. 初回のみ `%APPDATA%\mimageviewer\runtime\0.8.2\` に core + 5 DLL を展開
2. core が spawn されて自身のウィンドウを開く
3. 動画フォルダで動画をダブルクリック → フルスクリーンでインライン再生開始

2 回目以降は展開済みなのでスキップ (サイズ一致チェック)、起動が速い。

### 開発時に core を直接起動したいとき

ランチャー経由ではなく `target/release/mimageviewer-core.exe` を直接実行したい場合
(デバッガアタッチ等)、FFmpeg DLL を手動でコピーする:
```powershell
Copy-Item vendor/ffmpeg/bin/*.dll target/release/
```
core 側はその時点で同居 DLL を直接使う (APPDATA 展開はランチャー専属の責務)。

### LGPL ライセンス対応 (リリース時に必須)

mIV 自身は MIT、FFmpeg は LGPLv3-or-later。**動的リンク** (DLL を別ファイルとして配布し
`LoadLibrary` する形式) なら互換。`include_bytes!` で exe に埋め込んでも、最終的に
APPDATA の DLL ファイルとして展開されるので動的リンクとして扱える。

リリース前に以下を確認・更新する:

1. **ソフトウェア情報 (環境設定 → ヘルプ)** に LGPL 通知を追記。
   通知文面の canonical 版は `docs/ffmpeg-lgpl-source-distribution.md` の
   "Notice Template" を参照する。
2. **mikage.to に LGPL 対応ソース tarball を配置**: BtbN がビルドに使った FFmpeg の
   ソース tarball ([ffmpeg.org の Old Releases](https://ffmpeg.org/releases/)) を
   `htdocs/mimageviewer/ffmpeg-<VERSION>-source.tar.xz` として転載。
   現行 BtbN build は外部ライブラリを DLL 内に含むため、詳細は
   `docs/ffmpeg-lgpl-source-distribution.md` と
   `scripts/collect-ffmpeg-lgpl-info.ps1` の出力で確認する。
3. **DLL ファイル名を改変しない**: `avcodec-61.dll` 等のオリジナル名のまま展開する。
4. **`installer/readme.txt` (Vector 同梱用)** にも LGPL 注記を 1 行追加。
5. **GPL build を絶対に使わない**: BtbN の `*-gpl-shared-*` や gyan.dev の
   "release-essentials" を使うと mIV 全体が GPL 汚染される。`setup-ffmpeg.sh` は
   `*-lgpl-shared-*` のみを取りに行くようになっている。

### Codec 対応範囲

LGPL build には以下が含まれる (主要なもののみ):

| コーデック | 対応 | 出元 |
| --- | --- | --- |
| H.264 / AVC | ✓ | OpenH264 (Cisco) を内蔵 |
| HEVC / H.265 | ✓ | LGPL 互換の libdav1d + 内蔵デコーダ |
| AV1 | ✓ | libdav1d |
| VP9 | ✓ | libvpx |
| MPEG-2 / MPEG-4 / MJPEG / WMV / VC-1 | ✓ | FFmpeg 内蔵 |
| AAC / MP3 / Opus / FLAC / Vorbis / AC-3 / DTS | ✓ | FFmpeg 内蔵 |

x264 / x265 (エンコーダ) は GPL なので含まれない (mIV はデコードしか使わないので無問題)。

### テスト

統合テスト (`tests/susie_integration.rs`) は `MIV_SUSIE_WORKER` 環境変数で
ワーカー exe のパスを直接指定できる。`setup-susie-worker.sh` を走らせて
`vendor/susie-worker/` に配置済みであれば、テストは自動でそれを拾う。

## VST3 host bridge 管理 (v0.9.0+)

動画音声に VST3 プラグインを挿入する機能 (LUFS 測定 / EQ / 等) のために、
**C++ で書かれた bridge プロセス** (`mimageviewer-vst3-host.exe`) を別途配置する。
詳細は [docs/vst3-integration.md](docs/vst3-integration.md) を参照。

VST3 SDK は **MIT ライセンス化されている** (3.8.0、2025-10-20 以降) ので mIV (MIT) と
互換。bridge ビルド成果物は通常の `include_bytes!` で本体に内包する (PDFium / Susie ワーカーと
同じパターン)。

### セットアップ (メインビルド前に必須)

```bash
# 1. VST3 SDK を vendor/vst3sdk/ に取得 (~490 MB)
bash scripts/setup-vst3-sdk.sh

# 2. CMake で C++ bridge をビルド
cmake -S crates/vst3-host -B crates/vst3-host/build -G "Visual Studio 18 2026" -A x64
cmake --build crates/vst3-host/build --config Release
# → vendor/vst3-host/mimageviewer-vst3-host.exe (~640 KB)
```

- 前提:
  - Visual Studio 2026 (18) BuildTools (MSVC C++ デスクトップ開発ワークロード)
  - CMake 3.20+
  - 一度ビルドしたら、C++ ソースを変更しない限り再ビルド不要
- 出力: `vendor/vst3-host/mimageviewer-vst3-host.exe` (.gitignore)。
  メイン exe のリリースビルド時に `include_bytes!` で内包される
- 動作確認用: `crates/vst3-host-tester/` (Rust 単独 GUI exe)。本体に依存せずに
  プラグインのロード / GUI / 音声パススルーをテストできる。`cargo run -p vst3-host-tester`

### ライセンス対応 (リリース時)

VST3 SDK 3.8.0+ は MIT 単一ライセンスなので、**追加の法務作業は不要**。
ただし以下を環境設定→ヘルプの「ソフトウェア情報」と `installer/readme.txt` に追記する:

```
This software supports VST3 plugins via the Steinberg VST3 SDK
(https://github.com/steinbergmedia/vst3sdk) under the MIT License.
```

**VST トレードマーク (ロゴ) は使わない**。「VST3 プラグインをサポート」テキスト表記のみで運用
(= トレードマークガイドライン回避)。

## Markdown / テキストファイルのエンコーディング (BOM 必須ケース)

Claude Code の `Write` tool は **UTF-8 (BOM なし)** で書き出す。Linux / macOS や
モダンエディタはこれで問題ないが、以下のツール群は BOM 無し UTF-8 を
**Windows ANSI (= CP932 in JP)** として誤読し、日本語が mojibake になる:

- **Codex GUI** (= ローカル Codex デスクトップツール) — ユーザー報告 2026-04
  「ブリーフが文字化けしている」
- **Windows メモ帳 (旧バージョン)** — Win11 22H2 以降は UTF-8 既定だが、社用 PC で
  古い設定が残っているケースあり
- **PowerShell 5.1** の `Get-Content` (デフォルト) — `-Encoding utf8` 明示しない限り
  CP932 で読む

このため、外部ツールに渡す **Markdown / テキストブリーフ** には UTF-8 BOM
(`EF BB BF`) を **明示的に付与**する。

### 付与方法

Claude Code 経由で書き出した直後に、以下のヘルパースクリプトで BOM を付ける:

```bash
python scripts/write_utf8_bom.py docs/foo-brief.md
```

冪等 (= 既に BOM があれば no-op)。複数ファイル指定可。

### 検証

```bash
file docs/foo-brief.md
# Expected: "Unicode text, UTF-8 (with BOM) text"
head -c 3 docs/foo-brief.md | xxd
# Expected: "00000000: efbb bf"
```

### 過去の経緯

- 2026-04: PowerShell スクリプト `upload-trt-pack.ps1` の日本語文字列が CP932
  読みで mojibake → 該当スクリプトは ASCII オンリーに置換 (BOM の代わり)
  (commit f81fec7)。ASCII で書ける用途は ASCII 化が単純で堅い。
- 2026-04: Codex GUI に渡す Markdown ブリーフが mojibake → BOM 付与で回避
  (本ポリシー策定)。Markdown / 日本語文章は ASCII 化できないので BOM を使う。

### 使い分け

| ファイル種別                              | 推奨方式            |
|-------------------------------------------|---------------------|
| `*.rs` / `*.cpp` / `*.h`                  | UTF-8 BOM **無し**  |
| `*.md` / `*.txt` (リポジトリ内、開発用)   | UTF-8 BOM **無し**  |
| `*.md` / `*.txt` (**外部ツールに渡す用**) | UTF-8 BOM **あり**  |
| `*.ps1` (PowerShell スクリプト)           | ASCII オンリー推奨  |

リポジトリ内の通常 Markdown ドキュメント (`docs/*.md`、`README.md` 等) は
BOM 不要 (= Linux / macOS / git diff の互換性のため)。

## UI 文字列の Unicode グリフ選定ルール

mIV はプライマリ proportional フォントに **Yu Gothic Medium** (Windows 標準) を
使う ([src/ui_fonts.rs](src/ui_fonts.rs))。通常 UI と native overlay は同じ
`ui_fonts::configure_fonts()` を通り、動画メタデータなどユーザー由来テキスト向けに
`miv-user-text` family を別途登録する。この family は Segoe UI Emoji を優先し、
Cambria Math / Segoe UI Historic / Segoe UI Symbol も縦位置補正付きで使う。通常 UI の
proportional family ではこれらを egui 既定 font の後ろに置き、固定 UI 文言の幅変化を
抑える。Yu Gothic は日本語 + 基本 Latin
+ Latin-1 Supplement までは網羅するが、**Misc Symbols** や **絵文字**は欠落
していることが多い。fallback はユーザー由来テキストの文字化けを軽減するためのもので、
UI の固定文言・ボタン・状態表示へ絵文字や環境依存記号を新規採用する理由にはしない。
過去に複数の文字化けバグが繰り返されている (2026-04 までに 🎚 / ✕ で発生)。

### 安全な代替表

| 危険 (フォント依存) | 安全 (Latin-1 / ASCII)         |
|---------------------|---------------------------------|
| ✕ (U+2715)          | × (U+00D7 multiplication sign)  |
| ✖ (U+2716)          | × (U+00D7)                      |
| 🎚 (U+1F39A)         | "VST" 等のテキスト              |
| 🟢⚫🔴 (status emoji)  | "[ON]" "[OFF]" 等              |

**矢印** ↑ U+2191 / ↓ U+2193 は Yu Gothic に含まれており OK。
**チェック** ✓ U+2713 / ✗ U+2717 は環境依存 (= 一部 Yu Gothic バリアントには
ある)。新しい場所で使う前に lint と実機で確認すること。

### コミット前 lint

```bash
python scripts/check_ui_glyphs.py
```

`src/` 配下の .rs ファイルから「実際に tofu 化したと確認された Unicode 文字」
を含む行を列挙する。0 件で exit 0、見つかれば exit 1。
新たに tofu 報告があった文字は同スクリプトの `DANGEROUS` dict に追加する。
誤検出の場合は該当行に `// glyph-lint:skip` を付ける。

CI には現状未統合だが、UI 文字列を新規追加・変更したコミット前に手動で
1 度走らせる慣行にする。

## UI スナップショットテスト

`tests/ui_snapshot.rs` + `tests/snapshots/*.png` に、`egui_kittest` を使った
UI 回帰テストを置いている。配色・レイアウトを変える修正を入れた際の
「意図しない見た目変化」を `cargo test` 段階で検知するのが目的。

```bash
cargo test --test ui_snapshot              # 既存スナップショットと比較
UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot  # 意図的な見た目変更の反映
```

更新後は `tests/snapshots/*.png` を目視で確認してから PNG とコード変更を
同時にコミットすること。詳細な設計方針・新規テスト追加手順は
[docs/ui-snapshot-policy.md](docs/ui-snapshot-policy.md) を参照。

## タグ書き込みの互換性検証 (ExifTool)

mIV は JPEG / PNG / WebP のタグを XMP `dc:subject` にだけ書く (IPTC Keywords は書かない)。
そのため **Windows エクスプローラーの「タグ」欄には表示されない** (Explorer は IPTC Keywords を
優先して読むため)。`docs/tag-feature.md` および `htdocs/mimageviewer/manual/tags.html` の互換性
記述はこの前提を反映している。

Lightroom / Bridge / digiKam / XnView MP 等の XMP 対応ソフトとは互換があるが、開発環境で
Adobe 製品を持っていなくても **ExifTool でラウンドトリップ検証できる**:

```powershell
# UTF-8 対応のためコンソールを切り替え (CP932 のままだと日本語タグが化ける)
chcp 65001
# または
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

exiftool -XMP-dc:Subject -IPTC:Keywords "path\to\tagged.jpg"
# 期待される出力:
#   Subject: #原神, #風景       ← mIV で付与したタグ (XMP 側)
#   Keywords:                    ← 空 (IPTC は書いていない前提)
```

ExifTool (exiftool.org) は業界標準の XMP 解釈リファレンス実装。ここで期待どおり出れば
Lightroom / Bridge / digiKam / XnView MP でも同じタグが見える。

### 文字化け対処

PowerShell で日本語タグが `繝励Μ繧ｭ繝･繧｢` のような mojibake になるのはコンソール出力
エンコーディングが CP932 のまま UTF-8 バイト列を解釈しているだけで、**ファイル内容自体は
正しい UTF-8**。`chcp 65001` か `[Console]::OutputEncoding = [System.Text.Encoding]::UTF8`
で解消。確実に機械可読にしたいなら JSON で取り出す:

```powershell
exiftool -j -XMP:Subject "path\to\tagged.jpg" | ConvertFrom-Json
```

JSON は仕様上 UTF-8 固定なのでコンソール設定に依存しない。

### 将来的な IPTC 併記 (dual-write) 検討メモ

Windows エクスプローラー互換も取りたいときは、APP13 Photoshop Image Resource Block 中の
IIM データセット 25 (Keywords) への併記が必要。Adobe IRB / IIM バイナリ構築は工数大
(200-400 行 + 相互作用テスト)。現状は優先度低、マニュアル側でエクスプローラー非対応を
明記して回避。

## 動画メタ情報の扱いと外部ダウンローダの言及禁止ポリシー

動画ファイルに埋め込まれているチャプター情報・タイトル・説明文等のメタ情報を mIV が
読んで右パネルやサムネイル一覧に表示する機能は、**FFmpeg の avformat が抽出する
標準メタデータ** (Matroska tags / MP4 udta / ffmetadata 形式の `;FFMETADATA1` 等) を
ソースとして扱う。**特定の外部ダウンローダの名前・URL・使用例は、ユーザー向けマニュアル
(`htdocs/mimageviewer/manual/`)・製品ページ (`htdocs/mimageviewer/index.html`)・
README.md・更新履歴・GitHub Releases の body・コミットメッセージ・配布同梱の readme 等、
公開される文書には一切記載しない**。

理由: 一部の動画サイト (YouTube 等) は ToS でサードパーティダウンロードを制限しており、
特定ツール名を明記すると mIV がそのワークフローを推奨していると誤解されかねないため。
mIV は **既にローカルに存在する動画ファイルを開くだけ** のビューワであり、ダウンロード
手段を提供したり推奨したりしない。

書き方の例 (どちらも内容は同じ):

| NG (公開文書) | OK (公開文書) |
|---|---|
| 「<外部ツール名> で `<オプション>` を付けると…」 | 「動画にチャプターメタデータが埋め込まれていれば…」 |
| 「<外部ツール> でダウンロードした動画の…」 | 「ローカルに保存済みの動画の…」 |
| 「外部ツール (<具体名>, <別名> 等) で取得した…」 | 「外部ツールで取得した…」(具体名なし) |

`docs/` 配下の設計ドキュメント (技術者向け) でも、特定ツール名を出さず「FFmpeg avformat が
解釈できる標準的なメタデータ形式」と書く。コミットメッセージも同様。

このポリシーはレビュー観点として恒久的に有効。新機能を追加・更新する際、文書差分に
特定ダウンローダ名が出ていないか確認すること。レビュー時のセルフチェックは
`git grep -i -E '<該当ツール名>' -- '*.md' '*.html' '*.txt'` 等で機械的に確認する
(具体名を CLAUDE.md にも書かないため、レビュー時はその場で対象名を補完する)。

## Distribution

- **mikage.to**: インストーラ (.exe) + exe 単体の両方を提供
- **窓の杜**: インストーラ (.exe) を zip にまとめて申請
- **Vector**: インストーラ (.exe) + `installer/readme.txt` (利用者向け説明書) を zip にまとめて申請。
  readme 同梱を Vector が要件化しているため、単体 exe やインストーラ単独での申請は不可。
- **インストーラ**: Inno Setup 6（`installer/mimageviewer.iss`）
- **ビルド**: `cargo build --release` → `ISCC.exe installer\mimageviewer.iss`
- **出力**: `installer/Output/mImageViewer_setup.exe`
- **Vector 申請用 zip**: `mImageViewer_v<VERSION>.zip` に `mImageViewer_setup.exe` と
  `installer/readme.txt` を同梱する。readme の内容 (動作環境・連絡先・インストール手順・
  取り扱い種別) は Vector のファイル掲載基準 (https://www.vector.co.jp/for_authors/upload/standard.html)
  を満たしていること。
- **CRT 静的リンク**: `.cargo/config.toml` でメイン exe (x86_64) と Susie ワーカー (i686)
  の両方に `+crt-static` を有効にしている。これにより `VCRUNTIME140.dll` / `MSVCP140.dll`
  など Visual C++ 再頒布可能パッケージへの依存を排除している。
  - メイン exe は `ort` クレートの `load-dynamic` 機能と組み合わせて成立している
    (静的リンク版 `onnxruntime.lib` は動的 CRT 前提でビルドされており、crt-static と
    両立しない)。どちらも触らないこと。
  - **解除すると Vector の「要ソフト」欄指摘が再発する**。
- **設定保存先**: `%APPDATA%\mimageviewer`（インストーラ版・単体版共通）

## コード修正時のドキュメント同時更新

機能の追加・変更・削除を行った場合は、以下のドキュメントも同時に更新すること:

- `htdocs/mimageviewer/manual/` — ユーザー向けマニュアル（設定・操作方法の変更を反映）
- `htdocs/mimageviewer/index.html` — 製品ページ（新機能の紹介・機能一覧の更新）

### マニュアル・製品ページの記述方針

ユーザーが見るマニュアル・製品ページ (`htdocs/mimageviewer/manual/` と
`htdocs/mimageviewer/index.html`) には **バージョン固有の表記を残さない**。

- NG: 「グリッドのタグバッジ (v0.7.2+)」「OR 検索 (v0.7.2+)」「タグ機能 (v0.8.0 新)」
  「(v1.0 非対応)」「このマニュアルは v0.7 に対応しています」
- OK: 「グリッドのタグバッジ」「OR 検索」「タグ機能」「(非対応)」

現行版に存在する機能は現在形で書き、過去版との差分を気にさせない。バージョン番号が
見える場所はトップページの「ダウンロード」セクションのバッジ 1 箇所に集約する。

実装詳細 (内部用語) も書かない。特に、**ライブラリ名 / データ構造名 / 実装戦略の名前**
はユーザーにとってノイズなので、該当機能の一般名で書き換える:

| NG (実装語) | OK (一般語) |
|---|---|
| Tantivy / bigram 索引 | 全文索引 |
| SQLite データベース | キャッシュ / データベース |
| notify-rs 監視 / reconciliation | 自動監視 / 整合性チェック |
| PDFium エンジン | PDF 表示エンジン |
| DPAPI | (Windows の機能で) 暗号化 |
| アトミック rename | (削除して「ファイル本体を書き換えます」だけにする) |
| STORE モード / 無圧縮 STORE 方式 | 再圧縮なし / そのまま格納 |
| マルチプロセス並列レンダリング | 並列処理 |
| RDF Bag | (削除、XMP dc:subject だけで十分) |
| ONNX Runtime DLL (カテゴリ名として) | AI 機能用 DLL |
| panic ログ | エラーログ |

また、アプリ動作を説明するときに「ワーカー / スレッド / mpsc / mutex / atomic」等の
並行処理用語も出さない。「バックグラウンドで処理します」で十分。

`docs/` 配下の設計ドキュメントは技術者向けなので内部用語 OK、バージョンタグも歴史的経緯の
コメントとして残してよい。
- `docs/spec.md` — 仕様書（設定項目・内部仕様の変更を反映）
- `docs/architecture-overview.md` — モジュールが増減した、永続化ストアを追加した等の構造変化
- `docs/display-pipeline.md` — 表示テクスチャ優先順位・変換合成順序・変換適用ポイントを変えたとき
- `docs/async-architecture.md` — ワーカーを増やした、共有アトミック/チャネルを追加した、キャンセル規約を変えたとき
- `docs/ui-responsiveness.md` — UI スレッドから新しい I/O / GPU アップロード / read_dir 走査を呼ぶとき、または関連の計装/チェックリストを改定したとき
- `docs/virtual-folders.md` — ZIP/PDF の分岐表・キャッシュキー規則・DB キー正規化を変えたとき
- `docs/preset-and-adjustment.md` — キャッシュ無効化ルール・補正/AI の適用順序・プリセットの保存先を変えたとき

コードだけ修正してドキュメントを放置しない。設計ドキュメントが腐ると次の修正で同じ罠を踏む。

## リリース手順チェックリスト

リリース時は以下を漏れなく更新・作成すること。

### Phase 0: 変更履歴の準備とユーザーレビュー (必ず最初)

更新履歴は `update_check.rs` 経由でアプリ内アップデート通知ダイアログにそのまま
表示される (= GitHub Releases の `body` がユーザーに読まれる)。誤字・内部用語の
混入・粗い表現があると公開後に取り返しが付かないので、以下の順で進める。

- **README.md の更新履歴セクション** にこのバージョンの新エントリを追加 (旧版と
  同じ `### vX.Y.Z` ヘッダ + 箇条書きフォーマット)。書き方は CLAUDE.md
  「マニュアル・製品ページの記述方針」に従う:
  - 内部実装語 (Tantivy / SQLite / 並行処理用語) は使わない
  - バージョン番号タグ (v0.8.2+ 等) は本文に書かない (見出しに 1 回だけ)
  - 過去のユーザー報告日付 / コミットハッシュは書かない
  - 「⚠️ 」プレフィックスは初回起動時に何かが起きる注意 (索引再構築等) に限定
- **ユーザーに README.md の更新履歴を見せて承認を得る**。OK が出るまで Phase 1 へ
  進まない。
- (任意) GitHub Release 用 body は基本 README.md の該当セクションをそのまま
  コピペで OK。手作業で別途書き直さない (= 表記ゆれを生まない)。

### Phase 1: バージョン番号・関連ファイル更新

承認を得た上で以下を更新:

1. `Cargo.toml` — バージョン番号
2. `installer/mimageviewer.iss` — `MyAppVersion`
3. `installer/readme.txt` — 先頭の版表記・更新履歴リンク (Vector 同梱用)
4. `htdocs/mimageviewer/index.html` — ダウンロードセクションのバージョン表記
5. `htdocs/mimageviewer/manual/index.html` — マニュアルのバージョン表記
6. `htdocs/` 以下 — 新機能がマニュアル・製品ページに反映されていることを確認
   - マニュアル左サイドバーのリンク一覧が全 14 ページで揃っているか
     `htdocs/mimageviewer/manual/` 配下で一括確認:
     ```bash
     cd htdocs/mimageviewer/manual && for f in *.html; do
       echo "=== $f ==="
       sed -n '/sidebar-section/,/<\/nav>/p' "$f" \
         | grep -E 'href="[a-z-]+\.html"' | wc -l
     done
     ```
     14 以外 (= いずれかのページ名リンクが抜けている) なら同期を合わせる。
     新規ページを追加した際は 14 ページ全部のサイドバーを更新すること。

### Phase 2: 依存物の確認 + 性能回帰チェック

7. PDFium の更新確認（`bash scripts/setup-pdfium.sh check`）
8. ONNX Runtime DLL の配置確認（`bash scripts/setup-ort.sh`、ort クレート更新時は必須）
9. Susie ワーカーの再ビルド（`bash scripts/setup-susie-worker.sh`）
10. FFmpeg LGPL shared build の更新確認（`bash scripts/setup-ffmpeg.sh check`、
    バージョンを上げる場合は `vendor/ffmpeg/VERSION` と `src/video/ffmpeg_loader.rs` の
    DLL 名が一致するか確認）。LGPL 通知の更新も忘れずに (本ファイル「FFmpeg LGPL DLL 管理」節)
11. VST3 host bridge の確認 (v0.9.0+):
    - `vendor/vst3sdk/` が配置済み (`bash scripts/setup-vst3-sdk.sh`)
    - `vendor/vst3-host/mimageviewer-vst3-host.exe` が最新の C++ ソースでビルド済み
      (`cmake --build crates/vst3-host/build --config Release`)
    - 商用プラグイン (Pro-Q 4 等) で実機確認: 環境設定→動画タブで VST3 を ON →
      管理ウィンドウからプラグインをロード → 動画再生で音声がプラグインを通る /
      V キーで GUI トグル
    ※ `vendor/` 直下の必須ファイルは `build.rs` で起動時にチェックしており、
      不足していると `cargo build` が復旧手順付きで止まるようになっている。

9.5. **bench 回帰チェック** (検索周りに変更を入れたリリースで実施):
   ```bash
   cargo run --release --bin bench_search -- --docs 50000 --json /tmp/bench_new.json
   python scripts/check_bench_regression.py vendor/bench_baseline.json /tmp/bench_new.json
   ```
   - 初回 (vendor/bench_baseline.json の queries が空) は `--save` で baseline を登録:
     `python scripts/check_bench_regression.py --save vendor/bench_baseline.json /tmp/bench_new.json`
   - 既存 baseline と比較して **+30% 超の劣化**で exit 1。原因を切り分けてから先に進む。
     Tantivy 等の依存更新で正当な変動なら `--save` で baseline を更新する。

9.6. **perf smoke** (UI 周り / I/O 経路に変更を入れたリリースで実施):
   ```bash
   bash scripts/perf_smoke.sh
   ```
   - `--perf-log` 付きで mImageViewer を起動 → 手動で起動・Ctrl+↓ 連打・Ctrl+G 検索を実行
     → スクリプトが `analyze_perf.py hitches` で 16ms 超のフレーム間隔を集計。
   - 「ヒッチ: 0 件」または既知の長時間 nav (PDF cold open ~700ms 等) のみなら OK。
     nav イベント無しのヒッチは UI スレッド同期 I/O 退行の疑い (docs/ui-responsiveness.md §4)。

### Phase 3: ビルド・配布成果物

10. `cargo build --release` → `ISCC.exe installer\mimageviewer.iss` でインストーラを生成
    - 開発機では mIV をタスクトレイに常駐させているケースが多い。常駐中の `mimageviewer.exe` は
      `target\release\mimageviewer.exe` を握っているので、cargo がリンク段階で
      LNK1104 (アクセスが拒否されました) になって失敗する。
      その場合は `scripts\build-release.ps1` (PowerShell) もしくは
      `bash scripts/build-release.sh` を使うと、実行中の `mimageviewer.exe` /
      `mimageviewer-susie32.exe` を自動停止してからビルドできる。手動の
      `Stop-Process` + `cargo build` を毎回打つ手間を省くだけのラッパー。
11. 配布成果物を 3 種類用意する:
    - `mimageviewer.exe` (ポータブル版、mikage.to のみ)
    - `mImageViewer_setup.exe` (インストーラ版、mikage.to・窓の杜・Vector 共通)
    - `mImageViewer_v<VERSION>.zip` (Vector 申請用。`mImageViewer_setup.exe` +
      `installer/readme.txt` を同梱)
12. 依存 DLL の回帰チェック — リリース exe に対して `dumpbin /dependents` を走らせ、
    `VCRUNTIME140.dll` / `MSVCP140.dll` が現れていないことを確認する。もし現れていたら:
    - メイン exe: `ort` クレート機能から `load-dynamic` が抜けていないか確認
    - Susie ワーカー: `.cargo/config.toml` の `i686-pc-windows-msvc` 向け
      `+crt-static` 設定が残っているか確認

### Phase 4: GitHub Release 公開

13. ローカルで `git tag v<VERSION>` → `git push origin v<VERSION>` (GitHub `main` も同期)
14. GitHub Releases UI で新リリースを作成。**body は README.md の該当 `### vX.Y.Z`
    セクション本文をそのままコピペ** (この本文がアプリ内アップデート通知に表示される)。
    成果物 (3 種類) を Assets として添付する。
15. 公開後、別マシンから `mimageviewer.exe` を起動 → 起動時更新通知ダイアログで
    body が想定どおりに表示されることを目視確認 (改行・見出し・リンクの崩れチェック)。

## Codex CLI レビュー

ユーザーから「Codex にレビューしてもらって」「Codex レビューを取って」等と指示された場合は、
**ユーザーに手作業で中継してもらわず、`codex` CLI を直接叩いて結果を取り込む**。

### 自発的に Codex を呼ぶタイミング (明示指示がなくても)

ユーザーから個別の指示がなくても、以下のタイミングでは **自分から `codex exec` を起動して
意見を取り込む**こと。Claude 単体で先に進むより、別モデルの目を入れた方が早く正解に
辿り着くため。

1. **作業の一塊が終わった時点でレビューを取る**
   - まとまった機能追加・バグ修正・リファクタが「ローカルで動くようになった」段階で
     コミットを作り、そのコミットを基準に `codex exec --sandbox read-only -o <FILE>`
     でレビューを依頼する。
   - 一塊の目安: 単一の機能/修正で論理的にまとまっている、または PR にする粒度
     (1〜数コミット)。「small fix を 1 個入れた」程度では呼ばなくて良い。
   - 出てきた P1/P2 はその場で対応 (または false positive 判定の理由を添えて
     ユーザーに報告)、P3 はメモして後でまとめて。
   - 「これからユーザーに完了報告する直前」が呼びどころ。完了報告前に Codex の
     チェックを通しておくと、ユーザーが追加レビュー指示を出さずに済む。

2. **調査・デバッグで仮説と検証を 2-3 周してもまだ詰まっているとき**
   - 「黒画面の原因はこれだ」と思って直したのに直らない、ログを追加したが何も
     見えない、修正が別の箇所を壊す — のような **試行錯誤が繰り返し発生する局面**
     では、Codex に **現状の症状 + これまで試したこと + 関連ファイル** を渡して
     第二意見を取る。プロンプト例:
     ```
     codex exec --sandbox read-only -o /tmp/codex-2nd.txt \
       "Symptom: <症状>. Tried so far: <試したこと>. Suspected files: <file:line>.
        Look at the code with fresh eyes, propose 2-3 alternative root causes I might
        have missed. Return findings ordered by likelihood." < /dev/null
     ```
   - Claude が自分の仮説に固執して同じパターンの修正を繰り返す失敗を防ぐ。
   - 1 周目の修正で直らなかったらすぐ呼ぶ必要はないが、**3 周目に入る前に必ず**
     一度第二意見を取る (作業の手戻りコストの方が Codex 1 回分より高い)。

3. **設計判断で複数案あるが選び切れないとき**
   - 「VSR ON 時の D3D11 → wgpu 共有経路を Keyed Mutex で再有効化する案 vs 同期
     Flush で押し切る案」のような構造的な選択肢が複数ある局面では、Codex に
     trade-off を意見させる。Claude の最初の選好に縛られない設計判断ができる。

### 基本コマンド

read-only サンドボックス (ファイル書き換え・ネットワーク・パッケージ操作は禁止) で実行する。

| 目的 | コマンド |
|---|---|
| 直前のコミット (`HEAD~1` との差分) をレビュー | `codex exec --sandbox read-only "Review the changes since HEAD~1. Use git diff HEAD~1 and inspect relevant files. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first, ordered by severity."` |
| ブランチ全体 (`main` からの分岐) をレビュー | `codex exec --sandbox read-only "Review this branch against main. Use git merge-base main HEAD and git diff from that base. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first."` |
| 未コミットの作業変更だけレビュー | `codex exec --sandbox read-only "Review the uncommitted changes in this repo. Use git diff and git diff --cached. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first."` |

### 差分範囲の決め方

基準となるコミットは「**前回 Codex にレビューしてもらった地点より後**」にする。具体的には:

- セッション内で前回 `codex exec` を走らせたなら、そのときレビューした最新コミットハッシュを覚えておき、
  `codex exec --sandbox read-only "Review the changes since <HASH>. Use git diff <HASH>..."` を使う。
- セッション内で Codex に出したのが初めてなら、**今セッションで Claude が作った最初のコミットの親** (= 作業を始める前の HEAD) を基準にすると、今回の作業分をまとめて見てもらえる。
- 分からない場合はユーザーに「基準コミットはどこにしますか?」と聞く。推測で `HEAD~1` を決め打ちしない
  (直近 1 コミット以外にもレビュー対象があるケースが多い)。

### 結果の扱い

Codex の出力は `[P1]` / `[P2]` / `[P3]` のような severity 付きで返る。指摘 1 件ごとに:

1. 内容を読み、該当コード (`file:line`) を Read で確認する
2. 真の bug か false positive か判断し、ユーザーに要旨を報告する
3. 修正を入れる場合は同じコミットメッセージに `Codex P<N> 対応` を書いておく (レビュー履歴の紐付け用)
4. false positive と判断した場合は、**理由を添えて** ユーザーに伝える (ユーザーが最終判断できるように)

### 出力の取りこぼしに注意 (`tail -N` を使わない)

`codex exec ... 2>&1 | tail -80` のような **末尾固定行数の切り取りはレビュー指摘を失う**。
Codex は探索ステップ (grep / ファイル参照等) を流してから最後に `P1/P2/P3` サマリを
出すので、探索ログが長いと重要な指摘が tail の外に流れ出す可能性がある。

**推奨: `-o` で最終メッセージだけ抽出する** (codex 0.124 以降):

```bash
codex exec --sandbox read-only -o /tmp/codex-final.txt "…レビュー依頼…" \
    < /dev/null > /tmp/codex-events.log 2>&1
cat /tmp/codex-final.txt   # ここに P1/P2/P3 サマリだけが入る
```

`codex exec --output-last-message <FILE>` (短縮形 `-o <FILE>`) で**最終回答 (= P1/P2/P3
サマリ) だけ**をファイルに直接書き出せる。awk で抽出するより確実。

### 必須: stdin を `< /dev/null` で閉じる

`codex exec` は stdin がパイプ判定されるとそれを `<stdin>` ブロックとして読みに行き、
EOF まで待機する (`Reading additional input from stdin...` と表示されたまま固まる)。
Claude Code の Bash tool 経由で起動するときは stdin が常に何かに繋がっているので、
**必ず `< /dev/null` を付ける**。これを忘れると 5 分以上ハングして手で kill する羽目になる。

### awk で結論抽出する旧フォーマット (`-o` が使えない古い codex 用)

```bash
codex exec --sandbox read-only "…" < /dev/null > /tmp/codex-out.txt 2>&1
# 最終結論 (最後の "codex" 行以降) を抽出
awk '/^codex$/{found=1; next} found' /tmp/codex-out.txt
```

短いタスクなら `tail -200` 程度にとること (目安: Codex 探索が 5 分超なら十分)。

### 起動できないとき

`codex` コマンドが PATH にない / Codex CLI が入っていない場合は、ユーザーに環境確認を依頼する。
勝手に従来の「ユーザーに Codex 実行を依頼してレビュー結果をペーストしてもらう」フローに戻さない
(ユーザーの手間が増えるだけなので明示的に確認する)。

### モデル指定について

利用モデルは `~/.codex/config.toml` の `model` フィールドに書いてあり、`codex exec` は
そこで指定されたモデルで動く (対話モードで表示される `model: <name>` と同じ)。
以下のケースで失敗することがある:

- **CLI が古い**: 新モデル (例: `gpt-5.5`) は CLI 更新が必要。対話起動時に
  `Update available! X.Y.Z -> A.B.C` が出ていたら `npm install -g @openai/codex` で更新。
  対話モードでは動くのに `codex exec` で「model doesn't exist」が出る場合、
  まさにこの状態。
- **アカウントの制限**: ChatGPT アカウントでは `gpt-5` / `o3` 等の生モデル ID は
  使えず、Codex 向けに用意された ID (`gpt-5.5` / `gpt-5.4` 等) のみ。
- **config のタイポ**: 利用可能なモデル一覧は `~/.codex/models_cache.json` の
  `models[].slug` で確認できる。

`-c model="<name>"` で 1 回限りの override も可能。デフォルト設定を書き換える前に
ユーザーに相談すること (config はユーザーの個人設定で、勝手に変えない)。

## Formatting

- Rust のコード変更後は `cargo fmt` を実行してからコミットする。
- 既存の未整形ファイルへ初めてワークスペース全体の `cargo fmt` を適用する場合は、
  機能変更と混ぜず、`style: Apply cargo fmt` のような整形専用コミットに分ける。
- 他の未コミット作業が混ざっている状態では、合意なしにワークスペース全体へ
  `cargo fmt` をかけない。必要な場合は対象ファイルを限定するか、先に作業を整理する。
- Claude Code / Codex / 手作業のいずれでも同じ方針を使い、レビュー時に整形差分と
  ロジック差分が混ざらないようにする。

## Git Workflow

- **コミット指示はローカルコミットのみ**。「コミットして」と言われた場合は `git commit` までで止める。
  PR（プルリクエスト）の作成は、明示的に「PRを作って」と指示された場合のみ行う。
- **デフォルトブランチ**: GitHub 上は `main`、ローカルは `master`。リリース時は両方に push する。

## User: Background

- Comfortable reading C++ but not familiar with Rust's borrow checker details
- Has RTX 4090, Windows 11
- AI-assisted development workflow: Claude generates code, user reviews and tests
