# mimageviewer - Project Context

## 応答言語

**ユーザーへの応答は常に日本語で書くこと。** レビュー結果・調査報告・完了報告・確認質問など、
すべてのユーザー向けテキストは日本語にする。英語に切り替えない (技術用語・コード識別子・
コマンド・ログ抜粋など、そのまま示すべき固有部分は原文のままでよい)。コミットメッセージや
コード内コメントは従来どおり (本書の各節の方針に従う)。

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
| **動画 HUD・音量・seek bar・動画フルスクリーン UI** | [docs/video-architecture.md](docs/video-architecture.md) (特に native presenter の節)。**`ui_fullscreen.rs` ではなく `src/video/native_presenter/` を見ること** |
| ワーカー追加・キャッシュ・キャンセル処理 | [docs/async-architecture.md](docs/async-architecture.md) |
| UI スレッドから新しい同期 I/O / GPU アップロード / read_dir 走査を呼ぶ | [docs/ui-responsiveness.md](docs/ui-responsiveness.md) (§4 チェックリスト) |
| ZIP / PDF 対応が必要な機能 | [docs/virtual-folders.md](docs/virtual-folders.md) |
| ファイル名 prefix スタック（同接頭辞の画像を畳む集約表示・フラット読書・Shift+↓↑ ジャンプ） | [docs/filename-stack-plan.md](docs/filename-stack-plan.md) |
| 補正 / プリセット / AI アップスケール / 消しゴム | [docs/preset-and-adjustment.md](docs/preset-and-adjustment.md) |
| 補正レイヤー / ローカル調整 / レイヤー合成 | [docs/local-adjustment-layer-v1.1.0-plan.md](docs/local-adjustment-layer-v1.1.0-plan.md) と [docs/local-adjust-filter-candidates.md](docs/local-adjust-filter-candidates.md) |
| キーボード操作 / ショートカット / `consume_key` / `key_pressed` / native VK 判定 | [docs/keymap-spec.md](docs/keymap-spec.md) と [docs/key-customization-impl-plan.md](docs/key-customization-impl-plan.md)。新しいキー操作は原則 `KeyAction` + keymap helper 経由にする |
| UI の見た目・配色を変える修正 | [docs/ui-snapshot-policy.md](docs/ui-snapshot-policy.md) (egui_kittest スナップショットの更新手順) |
| **detached viewer / F12 別ウィンドウ / 複数ウィンドウ** | [docs/detached-rework-plan.md](docs/detached-rework-plan.md) ⚠️ **リワーク中につき凍結ルールあり** (下の「Detached viewer リワーク中の凍結ルール」参照) |

**設計を変えたら該当ドキュメントも同時に更新する** (下の「コード修正時のドキュメント同時更新」参照)。

## Detached viewer リワーク中の凍結ルール (2026-07-05〜)

detached viewer (F12 別ウィンドウ / 複数ウィンドウ) は構造リワーク中
(正本: [docs/detached-rework-plan.md](docs/detached-rework-plan.md)、実装 = Codex /
検収 = ClaudeCode Fable)。リワーク完了までの間:

- **detached 周りへの症状パッチを新規に入れない**。バグを見つけたら、プラン §2 (憲法)
  に従い BA 番号 (壊れた前提の分類) に対応付けて報告する。緊急の応急処置が必要な
  場合はプラン §8 の形式 (stopgap 明記 + 撤去予定ステージ記載) でのみ許可。
- **リワークのステージ外で detached 関連コード・テストを触らない** (他機能の修正が
  detached 述語や viewport 経路に触れる場合は、着手前にプラン §2 を読み最小限にする)。
- リワーク作業自体は `detached-rework` ブランチで行い、他セッションと並行して
  detached を触らない。

## バグ修正の一般原則

- 修正前に、観測された失敗、守るべき不変条件、違反を作ったコード経路をログ・テスト・
  source inspection で特定する。症状を消す guard、delay、retry、追加 repaint、一括 reset、
  silent fallback を根本原因の代わりに追加しない。
- 共有状態、複数の入力入口、非同期完了、複数 viewer context をまたぐ問題では、再現した
  経路だけで完了しない。同じ状態の producer / consumer と open / switch / close / cancel /
  error lifecycle を列挙し、同型の入口・終了経路も確認してから修正する。
- 相互排他的な状態を複数の bool / `Option` / pending、または field の有無を sentinel として
  表現している場合、新しい分岐を足さず、単一の typed request / state owner / router への
  集約を検討する。正しい構造修正が現在の範囲を超えるなら、症状パッチを入れずに報告する。
- items、cache、texture、queue、channel、generation、cancel token、worker など context 固有の
  resource は、create / mutate / drain / cancel / invalidation / drop が所有 context だけに作用する
  ことを確認する。読み取り専用の open / close は、変更時用の失効通知や別 context の再ロードを
  発生させてはならない。
- 状態バグには状態遷移テスト、入力 routing バグには handler-level test、非同期・multi-window
  lifecycle には request 相関と sibling context 不変を検証するテストまたはログ検査を追加する。
  v2.7.0 出荷前の具体的な横断監査は
  [docs/archive/review-v2.7.0/systemic-review-plan.md](docs/archive/review-v2.7.0/systemic-review-plan.md) を正本とする。

キー操作を追加・変更するときは、ユーザーが明示していなくても keymap 対応を検討する。
閲覧・編集・動画の通常ショートカットは `KeyAction` に追加し、`ini_name()` / `context()` /
`trigger()` / `default_chords()` / `ALL_ACTIONS` / 呼び出し側 helper / `docs/keymap.ini.default` を
揃える。IME 確定、OS clipboard、D&D、右クリック、マウス、ゲームパッドなど固定扱いにする
入力は、固定である理由を `docs/keymap-spec.md` に残す。

## Overview

A Windows 11 native image viewer built in Rust. Inspired by ViX (legacy 32-bit viewer),
modernized with GPU acceleration and AI upscaling. Single-window design replacing ViX's
dual-window approach.

## Tech Stack

- **Language**: Rust (edition 2024, stable MSVC toolchain)
- **GUI**: eframe 0.33 + egui 0.33 (wgpu backend)
- **Image decoding**: `image` crate (PNG, GIF, WebP, BMP) + `turbojpeg` (JPEG, libjpeg-turbo SIMD) + WIC (HEIC, AVIF, JXL, TIFF, RAW)
- **JPEG 高速デコード**: `turbojpeg` クレート (libjpeg-turbo スタティックリンク、SIMD 最適化)。サムネ生成時は **DCT スケール (1/8〜1/1)** で decode して 5-30MB カメラ JPEG を 2.5-6× 高速化 ([docs/dct-scale-plan.md](docs/dct-scale-plan.md))。圧縮入力 128MB 超は image クレート / WIC chain にフォールバック (並列ワーカー × 圧縮 buffer の積算メモリ圧迫を回避)。ビルドに cmake + NASM が必要。
- **Parallel loading**: `rayon` (dedicated thread pool per folder load)
- **Thumbnail cache**: SQLite via `rusqlite` (bundled), WebP encoding via `webp` crate
- **Video thumbnails**: Windows Shell API (IShellItemImageFactory)
- **Video inline playback**: `ffmpeg-the-third` クレート + FFmpeg LGPL shared DLL (BtbN ビルド) + `cpal` (WASAPI Shared 音声出力)。フルスクリーンで動画を MP4 / MKV / MOV / AVI / WMV / MPG / MPEG / HEVC / AV1 として再生する。`avcodec / avformat / avutil / avfilter / swscale / swresample` の 6 DLL を launcher (`crates/launcher/`) が `include_bytes!` で内包し、初回起動時に `%APPDATA%/mimageviewer/runtime/<version>/` へ展開して本体 (`mimageviewer-core.exe`) を spawn する。本体側は exe と同じディレクトリの DLL を Windows ローダが解決するだけで個別ロード処理は持たない。ビルドに libclang (LLVM/Clang) が必要。詳細は「FFmpeg LGPL DLL 管理」節を参照
- **ZIP support**: `zip` crate
- **PDF support**: `pdfium-render` crate + PDFium DLL (exe に埋め込み) + マルチプロセスワーカープール (5 プロセス並列レンダリング、1 つを Critical 予約)
- **PDF password**: `windows-dpapi` crate (DPAPI 暗号化でパスワード永続保存)
- **AI upscaling**: `ort` crate (ONNX Runtime v2、`load-dynamic` モード、`directml` + `cuda` + `tensorrt` features)。Real-ESRGAN / Real-CUGAN / NMKD-Siax ONNX モデルでタイル分割 4x アップスケール。バックエンドは Settings の `ai_backend` で DirectML / TensorRT を切替 (TRT は NVIDIA 専用)
- **ONNX Runtime DLL**: `onnxruntime.dll` / `onnxruntime_providers_shared.dll` (Microsoft.ML.OnnxRuntime.DirectML NuGet v1.24.2) を exe に `include_bytes!` で埋め込み、初回 AiRuntime 作成時に `%APPDATA%/mimageviewer/` に展開。`ort::init_from()` で動的ロードする。これにより VC++ 再頒布可能パッケージ不要
- **TensorRT 対応 (NVIDIA GPU 高速化、オプション)**: `Microsoft.ML.OnnxRuntime.Gpu.Windows` + NVIDIA CUDA Runtime / cuBLAS / cuFFT / cuRAND / cuSOLVER / cuSPARSE / NVRTC / nvJitLink / cuDNN / TensorRT (合計 ~6.8 GB) を `%APPDATA%/mimageviewer/tensorrt/` に展開して使用。pack DL は `scripts/setup-tensorrt-pack.ps1` (PoC 版、アプリ内 DL UI は将来実装)。実測 1.4-3.4x (アップスケール) / 4.5x (デノイズ) 高速化。エンジンビルダーは `mimageviewer.exe --tensorrt-build <model>` 子プロセス。詳細は [docs/archive/ai/tensorrt-batching-feasibility.md](docs/archive/ai/tensorrt-batching-feasibility.md)
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
│   ├── folder_tree.rs       # フォルダツリー走査ヘルパー
│   ├── folder_thumb_pins.rs # 親コンテナ（Folder/ZipFile/PdfFile）の代表サムネ手動ピン DB（v0.9.x、`#pin:` cache key suffix で identity を表現）
│   ├── grid_item.rs         # GridItem（Folder/Image/Video/Audio/ZipFile/PdfFile/ZipImage/ZipDir/PdfPage/Stack）/ ThumbnailState 定義
│   ├── filename_stack.rs    # ファイル名 prefix スタック（v2.0.0）の純ロジック（StackGroup/StackView/group_media/materialize_*）
│   ├── filename_stack_ui.rs # 上記の App グルー（トグル・集約⇔フラット切替・Shift+↓↑ ジャンプ）
│   ├── thumb_loader.rs      # サムネイル並列ロード
│   ├── wic_decoder.rs       # WIC 画像デコード（HEIC/AVIF/JXL/TIFF/RAW）
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
5. `setup-twemoji.sh` — `vendor/twemoji/svg/*.svg` を取得 (注釈スタンプの絵文字。
   build.rs が exe へ `include_bytes!` で同梱。未配置でもビルドは通るがスタンプは無効)
6. `vendor/models/*.onnx` を `%APPDATA%/mimageviewer/models/` から自動 copy
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
    ```
    CMake target は `vendor/vst3-host/mimageviewer-vst3-host.exe` へ直接出力するので
    `build/Release/` からの手動 cp は不要。詳細は本書「VST3 host bridge 管理」節を参照。
    ⚠ **古い別 worktree の exe を流用しないこと**: `crates/vst3-host/` の C++ ソース
    (`protocol.h` 含む) は頻繁に変わる。IPC プロトコルが現行 Rust 側とずれた exe を
    使うと bridge が起動直後にクラッシュし、VST3 有効時に動画再生が「激重」になる
    (2026-05-14 実害)。バージョンが合うか不明なら必ず再ビルドする。
- **TensorRT pack** (`vendor/tensorrt-cache/`): 開発時には不要。pack 配布作業時
  だけ `scripts/setup-tensorrt-pack.ps1` を回す

### 消えると再取得できないもの — ツリー外バックアップ必須

`bootstrap-vendor.sh` で再取得できる物 (pdfium / ort / ffmpeg / susie) と違い、
**以下は失うと復旧手段が限られる / 無い**:

- **`vendor/models/*.onnx`** — DL スクリプトが**存在しない**。`%APPDATA%\mimageviewer\
  models\` (インストール済み環境が展開した物) からコピーするしかない。動く mIV
  インストールも APPDATA も両方失うと**永久に取得不可**。
- **`vendor/vst3-host/mimageviewer-vst3-host.exe`** — 再ビルドには VST3 SDK
  (~490 MB) + cmake が要る。古い exe の流用は上記の通り不可。

そのため `vendor/models/` と `vendor/vst3-host/` は**リポジトリツリー
(`C:\home\mimageviewer` 配下) の外**へコピーを置く。現在のバックアップ先:

```
C:\home\mimageviewer_vendor_backup\
  ├ models\        (*.onnx 一式)
  └ vst3-host\     (mimageviewer-vst3-host.exe)
```

- **定期ジョブは不要**。`vendor/` の中身は静的なので、モデル追加や vst3-host を
  再ビルドした**ときだけ**バックアップを取り直す。
- **復旧手順**: `vendor/` 消失時、`bootstrap-vendor.sh` を流した後に
  `cp -r C:/home/mimageviewer_vendor_backup/models vendor/` と
  `cp -r C:/home/mimageviewer_vendor_backup/vst3-host vendor/` で埋め戻す。
- **背景**: worktree junction 事故 (本書「Git Workflow」節参照) で `vendor/` は
  2026-05 に複数回全消失している。再取得可能な物は bootstrap で戻るが、models は
  戻らないため、この外部バックアップが最後の砦になる。

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
- **Popup / menu wheel passthrough**: `menu_button` / `ComboBox` / popup 内に
  `ScrollArea` を置く場合、popup が開いている frame では `raw_scroll_delta` /
  `smooth_scroll_delta` / `Event::MouseWheel` を明示的に消費すること。これを忘れると
  popup 内をスクロールしたつもりの wheel が背面のサムネイル一覧にも通り、一覧が動く。
  さらに、グローバルな一覧ホイール処理 (`App::process_scroll`) はメニュー/ツールバー/
  アドレスバー/ファセットバーを描いた後、グリッド描画の直前で呼ぶこと。早く呼ぶと
  popup が wheel を処理する前に背面グリッドが先に動く。既存例: `src/ui_main.rs` の
  `suppress_menu_button_wheel_passthrough` とツールバー ComboBox の open guard。

### ダイアログ (egui::Window)
- **テーマと文字色の所有境界**: 通常 UI の配色は `os_theme::apply_resolved_with_contrast`
  が Light / Dark 両 Style をまとめて所有する。通常文字・補足文字・警告・エラーは
  `ui.visuals().text_color()` / `weak_text_color()` / `warn_fg_color` / `error_fg_color` を使い、
  画面ごとの固定 gray を追加しない。フルスクリーン等の暗色固定子 UI は
  `os_theme::apply_dark_ui`、ComboBox / popup は `dark_popup_style` / `dark_menu_popup`、
  タイトルバーを含む暗色 `egui::Window` は `with_dark_context_style` を使う。
  popup / Window のために `ctx.set_theme` / `ctx.set_visuals` を直接呼ばない。限定スコープが
  Light / Dark 両 Style と theme preference を復元することが、メインテーマへ漏らさない不変条件。
- **ドラッグ移動**: `anchor()` を使うとウィンドウが固定されドラッグできなくなる。
  必ず `default_pos()` を使う。定番の初期位置は `ctx.content_rect().min + egui::vec2(60.0, 40.0)`。
- **閉じるボタン**: `.open(&mut open)` でタイトルバーに × ボタンが付く。
  `open` が `false` になったら `show_*` フラグを落とす。
- **背面への wheel / key 伝播防止**: モーダル相当の表示状態は `App::common_modal_dialog_open`
  へ追加し、main / fullscreen で別々の一覧を持たない。`App::process_scroll` は描画済みの
  `Order::Middle` / `Foreground` layer がポインタ直下にある場合も背面グリッドの wheel を
  止めるため、モデルレス Window と将来の登録漏れにも安全側で動く。state が存在しても
  Window を描かない phase がある処理は、state の有無ではなく「現在表示されるか」を返す
  helper を登録すること。
- 数十分以上続く取得・管理 UI (TensorRT パック等) はモデルレス tool window とし、
  `common_modal_dialog_open` へ含めて閲覧全体を停止しない。Window 上の入力だけは floating-layer
  guard で背面へ漏らさない。
- **ScrollArea の横幅**: ダイアログ右端に縦スクロールバーを置く一覧は
  `.auto_shrink([false, ...])` を指定し、利用可能幅を使い切る。既定の横 shrink のままだと
  内容幅まで縮み、スクロールバーがダイアログの途中に出る。
- **折り返し本文と floating scrollbar**: 長文を折り返すダイアログでは、floating scrollbar の
  `floating_allocated_width` を少なくとも `bar_inner_margin + bar_width` 確保する。共通 style は
  一覧の表示面積を優先して予約幅 0 のため、そのまま使うと右端の文字へバーが重なる。
- **パターン**: `ui_dialogs/` に 1 ファイル 1 メソッドで追加。
  `mod.rs` に `mod xxx;` を追加し、`app.rs` の `update()` 内で `self.show_xxx(ctx)` を呼ぶ。
  `App` 構造体に `show_xxx: bool` フィールドを追加し、`Default` impl で `false` 初期化。

### パネル (Area + Frame::popup)
- フルスクリーン上の固定パネルで `egui::Area::fixed_pos` + `Frame::popup` +
  `ScrollArea` を組み合わせる場合、`ScrollArea::max_height` だけに頼らない。
  egui 0.33 は親 `Ui` の `available_rect_before_wrap()` を上限にするため、自動サイズの
  `Area` / `Frame::popup` 内ではコンテンツ高に縮むことがある。
- 下端近くまで伸ばしたいパネルは、ScrollArea の前に
  `ui.allocate_ui_with_layout(egui::vec2(width, body_height), ...)` で親領域を明示確保し、
  その内側で `ScrollArea::vertical().max_height(body_height).auto_shrink([false, false])`
  を使う。
- タイトル / プレビュー / 閉じるボタンのヘッダは ScrollArea の外に固定する。
  ScrollArea 内へ戻すと、縦スクロールバーが × ボタンに重なる退行を起こす。
- パネルが下端近くまで伸びる場合、クリック吸収 sink とキャンバス入力抑制 rect も
  同じ高さから動的に作る。固定 1000px は 1440p / 4K / 縦長ウィンドウで不足する。

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
- **Grid contents**: 実フォルダ / アーカイブ類 / 画像 / 動画・音声を、設定された 4 行へ
  カテゴリごとに配置する。同じ行は共通の `sort_order` で混在ソートし、空行は読み飛ばす。
  既定は「実フォルダ + アーカイブ類」先頭、「画像 + 動画・音声」後続の従来互換。
  ZIP/PDF ファイルは 1 枚目/1 ページ目のサムネイル＋種別バッジで表示。非対応ファイルは無視。
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

### PDF レンダ pool の context epoch (2026-05)

`PdfWorkerPool` は **3 段階優先度** (`Critical / HighNormal / Normal`) + **context epoch**
で管理されている。UI ナビゲーション (フォルダ移動 / Ctrl+G) で
`pdf_loader::bump_render_context_epoch()` が呼ばれると、それ以前に enqueue された
HighNormal/Normal ジョブが pool 内で stale 化 → bump 時の一括 prune + dispatcher pop 時の
個別 stale 判定で skip される。これで PDF 多数フォルダを Ctrl+↑↓ で高速にめくっても、
旧 PDF のページレンダリングが queue に溜まって新 PDF の cover を遅らせる事象を防ぐ。

- `LoadRequest.context_epoch` を **UI スレッドの enqueue 時点で** 焼き付ける (TOCTOU 防止、
  worker から `current_render_context_epoch()` を呼ばない)
- `context_epoch=0` は background / Critical 用 sentinel (= プルーン対象外)
- background 経路 (CatchupQueue / NeighborPrefetch / build_and_save_one_pdf / cache creator /
  `get_document_info` / `enumerate_pages_with_cancel`) は全て epoch=0
- 詳細: [docs/pdf-pool-context-epoch-plan.md](docs/pdf-pool-context-epoch-plan.md)

### PDF レンダ pool の HarvestOnCancel (2026-05)

`CancelWaitPolicy` enum で `pool.execute` の cancel 時挙動を選択可能:

- `AbortOnCancel` (既定): cancel 検出と同時に `Err(Interrupted)` で抜ける。in-flight IPC
  結果は dispatcher が `reply.send` で silently 捨てる。
- `HarvestOnCancel` (thumbnail PDF render の cache-savable 経路のみ): cancel が立っても
  reply を待ち続け、in-flight IPC 結果を harvest。caller (= `load_one_cached`) が cache
  保存に進めて、PDFium が既に処理した結果を投資回収する。再エントリ時の再 render を防ぐ。

ポリシー選択は `process_load_request` で行う:
- `req.pdf_page.is_some() && !req.skip_cache && catalog.is_some()` → `HarvestOnCancel`
- それ以外 (enumerate / Critical / fullscreen / bulk / neighbor / background) → `AbortOnCancel`

perf イベント: `pool_cancel_harvest_wait` (待ち開始時) / `pdf_thumb_cache_saved_after_cancel`
(cache 保存成功時)。詳細: [docs/pdf-pool-harvest-on-cancel-plan.md](docs/pdf-pool-harvest-on-cancel-plan.md)

### スクロール後の visible 昇格 + 計装 (2026-05)

スクロール中に prefetch (priority=False) で enqueue された PDF render ジョブは、
`reload_queue` は毎フレーム re-tag されるが、**既に pdf_pool.normal に積まれたものは
priority=False のまま居座る**。可視範囲到達後も Normal lane で処理されて 3 秒以上待たされる。

対策: `pdf_loader::promote_to_high_normal(visible_keys)` を毎フレーム呼び、現可視 PDF の
perf_key と match するジョブを Normal → HighNormal lane に移す。dedup は
`last_promoted_visible_keys` で行い、lock 取得を最小化。

新 perf イベント:
- `ui/scroll_settle`: スクロール停止 (300ms) 検出。pool snapshot + target_count + already_loaded
- `ui/visible_thumb_first_ready` / `visible_thumb_all_ready`: settle 後の可視サムネ Loaded latency
- `pdf/pool_queue_snapshot`: 1 秒 tick で queue 状態 + in_flight age (max/p95/p50)
- `pdf/pool_promote_visible`: promote 件数の stats (promoted / already_high / not_found)

`scripts/analyze_perf.py scroll` で settle ごとの first/all_ready latency を集計可能。
詳細: [docs/scroll-visibility-priority-plan.md](docs/scroll-visibility-priority-plan.md)

### スクロール中の prefetch 抑制 (2026-05)

`promote_to_high_normal` で promote しても、in-flight に既に居る prefetch render は
PDFium cancel 不可なので最低 1.5 秒待ち。これが settle 後の visible_thumb_first_ready
を最悪 9 秒以上ブロックする (実測)。

対策: **`reload_queue` / `heavy_io_queue` の prefetch (`req.priority=false`) を、スクロール中 /
visible 待ち中は enqueue しないし、queue に既に居る prefetch も毎フレ prune する**。
pool に流れる前に止めるので cancel 不可問題に影響されない。

判定は `decide_prefetch_allowed(now, last_prefetch_scroll_at, visible_pending)` 純関数。
- 最後の scroll 入力から **100ms 未満** → Block (`ScrollNotIdle`)
- visible 範囲に Pending/Evicted/Requested が **1 つでもあれば** → Block (`VisibleStillLoading`)
- 最後の scroll から **3 秒経過** → 無条件 Allow (`Backstop3s`、永久 stall 防止)

`last_prefetch_scroll_at` は **`emit_scroll_settle_event` で clear されない** 専用 timestamp
(`last_scroll_event_at` とは別)。`App::update` 冒頭の `detect_scroll_input_intent`
(ctx.input wheel + arrow keys + Page/Home/End) で即時更新する。
これで `update_keep_range_and_requests` の gate 判定時に同フレーム入力が反映済み。
scrollbar drag / touch は `update_scroll_settle_state` の offset 変化 fallback で 1 フレ遅れて拾う。

新 perf イベント:
- `ui/prefetch_suppressed`: `transition`(start/continue/end) + `allow_reason`(unblock 時) +
  `backstop_hit` + 抑制件数 (suppressed_regular/heavy + pruned_regular/heavy)

`scripts/analyze_perf.py scroll` が settle 直前の suppression end を併記する。
詳細: [docs/prefetch-suppression-during-scroll-plan.md](docs/prefetch-suppression-during-scroll-plan.md)

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

- **JPEG デコード**: TurboJPEG (SIMD) + **DCT スケール** で カメラ JPEG (5-30MB / 20-60MP) のサムネ生成を 2.5-6× 高速化。target_px (= max(display_px, thumb_px)) に応じて 1/8〜1/1 のスケール係数を選び、decoder 内側で縮小デコード。圧縮入力 128MB 超は image クレート (zune-jpeg) / WIC にフォールバック。詳細: [docs/dct-scale-plan.md](docs/dct-scale-plan.md)
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

AI 機能 (アップスケール・ノイズ除去・消しゴム) は ONNX Runtime + DirectML EP を
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
1. `cargo build --release --bin mimageviewer-core` → 本体生成 (package `mimageviewer` 内の bin なので `-p` 不要)
2. `cargo build --release -p mimageviewer-launcher --bin mimageviewer` → ランチャー生成 (本体を include_bytes!)。**bare `cargo build --release --bin mimageviewer` は失敗する** (`no bin target named mimageviewer in default-run packages`。`mimageviewer` bin は package `mimageviewer-launcher` にあり workspace default-members 外なので `-p` 必須)

ラッパーは両方の cargo 呼び出しで `CARGO_INCREMENTAL=0` を明示する。`Cargo.toml` の
release profile はローカル rebuild 高速化のため `incremental = true` だが、ThinLTO +
rust-lld で stale incremental object が残ると `fast_image_resize` などの SIMD symbol が
release link 時に未解決になることがあるため、配布ビルドは安定優先で incremental を切る。

`cargo build --release` を直接打つ場合は ① → ② の順で 2 回打つこと。
ランチャー側 build.rs が `target/release/mimageviewer-core.exe` の存在をチェックし、
無ければ復旧手順付きで止まる。

**配布物**:
- 単体 exe 版: `mimageviewer.exe` 1 ファイル (約 365MB、内包する core + DLL を含む)
- インストーラ版: `mImageViewer_setup.exe` (Inno Setup が同じランチャーを配置)
- どちらも初回起動時に APPDATA に展開、2 回目以降は展開済みなのでスキップして高速

### セットアップ (メインビルド前に必須)

```bash
bash scripts/setup-ffmpeg.sh           # BtbN の n7.1 系・最新の版付き LGPL shared build を取得
bash scripts/setup-ffmpeg.sh check     # 新しい版付きビルドがあるか確認のみ
```

- 取得元: [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases)
  の `ffmpeg-n7.1*-win64-lgpl-shared-7.1.zip`
  - ローリング `latest` release の `...-latest-...zip` は使わない。日付付き `autobuild-*`
    release にある commit hash 込みの版付き資産だけを選び、`vendor/ffmpeg/VERSION` に記録する
- 出力先:
  - `vendor/ffmpeg/bin/{avcodec,avformat,avutil,avfilter,swscale,swresample}-*.dll`
    — launcher の `include_bytes!` で `mimageviewer.exe` に埋め込み
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
1. 初回のみ `%APPDATA%\mimageviewer\runtime\<CARGO_PKG_VERSION>\` (例: `runtime\0.9.0\`) に core + 6 DLL を展開
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

mIV は既定のプライマリ proportional フォントに **Yu Gothic Medium** (Windows 標準) を
使い、v2.7.0 から `Settings.ui_font` で Windows のシステムフォントまたはユーザー追加
TTF/OTF/TTC/OTC の日本語 upright face を選べる。日本語なし / Italic / Oblique は候補外
([src/ui_fonts.rs](src/ui_fonts.rs),
[src/ui_font_catalog.rs](src/ui_font_catalog.rs))。通常 UI と native overlay は同じ
`ui_fonts::configure_fonts_with_settings()` を通り、動画メタデータなどユーザー由来テキスト向けに
`miv-user-text` family を別途登録する。この family は text-presentation 記号、
数学英字、絵文字の fallback を明示し、ttf-parser で選択 face / Yu Gothic の代表 glyph と
Segoe UI Emoji の代表 glyph の中心を読んで
baseline 補正を計算する。egui 0.33 は `ab_glyph` の outline 描画を使うため、
計測も outline bbox を優先し、raster image bounds は bbox が取れない場合の fallback
とする。動画・音声 HUD の固定サイズ control label は選択 face ではなく `miv-hud-text`
(無補正の既定日本語フォント) を使い、VST 等の固定 glyph はベクター描画する。
複数サンプルは中央値で扱う。ツールバーは既定 Yu Gothic の実測補正値に
選択 face との差分を加え、環境設定の ±4pt 微調整を最後に加える。`✉` / `⋈` のような text-presentation 記号は
Cambria Math や Segoe UI Emoji より前の Meiryo fallback で拾わせ、ブラウザに近い
文字表示へ寄せる。数学英字は Cambria Math、色付き絵文字は Segoe UI Emoji へ回す。
Cambria Math の数学英字も代表 glyph の中心から `FontTweak` 補正を導出し、`…` など
主フォント側の句読点と極端に上下ずれしないようにする。
Segoe UI Historic / Segoe UI Symbol も
縦位置補正付きで使う。通常 UI の proportional family では選択 face を先頭、既定日本語
font を次順位に置き、各 fallback は egui 既定 font の後ろに置く。選択 face が持たない
日本語 glyph は既定日本語 font へ回る。Yu Gothic は日本語 + 基本 Latin
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

## 開発中のビルド・テスト選択

通常の編集ループで毎回 `cargo test --workspace` を実行しない。まず変更範囲に応じて
次の最小targetを使い、共有境界を変更したときや最終確認時だけ範囲を広げる。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo test -p mimageviewer --lib <filter>
cargo test -p mimageviewer --test <integration-test-name>
.\scripts\build-dev.ps1
```

全workspace・統合・doc・pack builderテストは `.\scripts\test-full.ps1` に集約する。
これは共有基盤を横断する変更の最終確認、リリース前、または明示的に全体確認を求められた
場合に実行する。機能追加中の試行錯誤ごとには実行しない。詳細と判断表は
[docs/development-build-and-test.md](docs/development-build-and-test.md) を参照。

### 修正完了時の実機確認用バイナリ

ユーザーから指示されたアプリ機能・実行時挙動の修正が完了し、関連する自動テストが通ったら、
最終回答の前に原則として次を実行する。

```powershell
.\scripts\build-dev.ps1
```

エージェントは成果物を起動しない。最終回答には、リポジトリルートからユーザーが実行する
次のコマンドと、今回確認してほしい具体的な操作シナリオを記載する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

このバイナリはビルド時間だけを短縮する `dev-runtime` Cargo profileで、`portable` featureは
使わない。引数なしでは通常版と同じ実利用中の `%APPDATA%\mimageviewer` を開くため、設定・
キャッシュ・ログを更新し得ることを最終回答で警告する。エージェント自身は起動しない。
single-instance mutexも通常版と共有するため、起動前にインストール版／常駐tray版を終了して
もらう。
隔離確認が明示的に必要な場合だけ、同じバイナリへ
`--data-dir .\target\dev-runtime\data` を渡す（build flavorは切り替えない）。

ドキュメントのみ、テストのみ、build scriptのみなど実行アプリへ影響しない変更、ユーザーが
build不要と明示した場合、必要な依存物が無い場合は省略できる。通常なら必要なbuildを作れなかった
場合は、その理由を最終回答に明記する。

launcher、release-only cfg／最適化、exact release performance、埋め込みasset展開、変更した
VST3 bridge、署名、packagingなどrelease構成そのものを確認する変更では `build-dev.ps1` ではなく、
後述の「実機検証用バイナリの準備」に従って `build-release.ps1` を使う。動画native presenter、
フルスクリーン、動画→音声モードなどcore内のWindows native挙動は、上の通常profile
`build-dev.ps1` でユーザー実機確認できる。

## タグ書き込みの互換性検証 (ExifTool)

mIV は JPEG / PNG / WebP のタグを XMP `dc:subject` にだけ書く (IPTC Keywords は書かない)。
そのため **Windows エクスプローラーの「タグ」欄には表示されない** (Explorer は IPTC Keywords を
優先して読むため)。`docs/archive/search-metadata/tag-feature.md` および `htdocs/mimageviewer/manual/tags.html` の互換性
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

## モザイク・成人向け画像処理の表記ポリシー

モザイク機能 (および関連する補正・エクスポート機能) のユーザー向け文書・UI に、
**特定の画像投稿サイトの名前・基準名・基準への適合判定を一切記載しない**。
適用範囲はユーザー向けマニュアル (`htdocs/mimageviewer/manual/`)・製品ページ
(`htdocs/mimageviewer/index.html`)・README.md・更新履歴・GitHub Releases の body・
コミットメッセージ・配布同梱の readme・アプリ UI 文字列・ツールチップ・ダイアログ文言。

**理由**: 投稿サイトの修正基準は規約改定で変わる例があり (透過モザイクの可否、最小
タイルサイズの扱いなど)、本アプリが「この設定なら投稿可能」「この設定は不可」と表示
すると、利用者を誤った方向へ誘導するリスクがある。基準への適合判断は **利用者自身が
行うべき**もので、アプリは技術的な処理内容を客観的に提示するに留める。

### 書き方の例

| NG (公開文書・UI) | OK (公開文書・UI) |
|---|---|
| 「✓ <サイト名> 1/100 基準クリア」 | 「= 14px @ 1400px 長辺」(px 数値のみ) |
| 「不透明モザイク (配信用)」 | 「不透明モザイク」(中立的なモード名のみ) |
| 「半透明モザイクは <サイト名> 規約違反」 | 「マスクを含むタイルをマスクの割合に応じた不透明度で描画」(処理内容を具体的に書く) |
| 「<サイト名> ガイドライン準拠の設定」 | 「画像長辺の 1/100 を 1 倍サイズとして設定できます。最小 4px に補正されます」(機構の説明のみ) |

### モード説明は「処理内容」を書く

「強い隠蔽」「弱い隠蔽」「綺麗」「投稿用」のような **評価語・用途判定的な表現は使わない**。
利用者が自分の用途 (投稿先の規定など) に合うかを判断するには、処理の機構そのものを
知る必要があるため。

| 内部名 (例) | 推奨ラベル (処理内容を具体的に書く) |
|---|---|
| `Opaque` | マスクを含むタイルを不透明で描画 |
| `Translucent` | マスクを含むタイルをマスクの割合に応じた不透明度で描画 |
| `MaskShape` | マスクの形に沿って描画 (マスク内の各画素をその画素が属するタイルの平均色で塗る) |

### `docs/` 配下 (技術者向け) の扱い

設計ドキュメントでも、ユーザーに直接読まれる本文には特定サイト名を書かない。ただし
**「このポリシー自体を説明する文脈」では、書かない理由を述べるために抽象的に言及する
ことは許容**する (= 「特定の投稿サイト名を書かない」のような文)。

### 適合確認 (レビュー時)

`git grep -i -E '<該当サイト名>' -- '*.md' '*.html' '*.txt' '*.rs'` で機械的に検出する。
(具体名を CLAUDE.md にも書かないため、レビュー時はその場で対象サイト名を補完する。
動画ダウンローダポリシーと同じ運用。)

なお、AI 生成画像のメタデータ (PNG tEXt の AI prompt 等) を読む / 書く処理自体は、
特定の生成ツール名を出さずに「PNG tEXt チャンクの形式」「EXIF UserComment」など
**標準仕様レベルの用語**で記述する。生成ツール固有のフォーマット (A1111 形式 /
ComfyUI 形式 等) はパーサ内部の実装詳細としてのみ言及し、ユーザー向け UI には出さない。

## Distribution

3 つの配布形態がある (v1.1.0 以降):

| 配布形態 | ファイル名 | 中身 / 性質 | data 保存先 | 管理者権限 |
| --- | --- | --- | --- | --- |
| **単体exe版** (旧称「ポータブル版」) | `mimageviewer.exe` | launcher。core + FFmpeg DLL を `include_bytes!` 内包、起動時に APPDATA へ展開して spawn | `%APPDATA%\mimageviewer` | 不要 |
| **インストーラ版** | `mImageViewer_setup.exe` | Inno Setup 出力 | `%APPDATA%\mimageviewer` | **要 (UAC)** |
| **ポータブル版** (v1.1.0 新) | `mImageViewer_portable_v<VER>.zip` | loose-deps。native 依存を埋め込まず exe 隣に同梱、展開ゼロ | `<exe_dir>\data` (APPDATA 不使用) | 不要 |

> ⚠ **用語**: `mimageviewer.exe` (launcher 単体 exe) は APPDATA を使うので「ポータブル版」と
> **呼ばない**。「単体exe版 / オールインワン版」と呼ぶ。「ポータブル」は loose-deps zip 専用。

- **mikage.to**: インストーラ版 + 単体exe版 + ポータブル版 zip の 3 つを提供
- **窓の杜**: インストーラ (.exe) を zip にまとめて申請
- **Vector**: インストーラ (.exe) + `installer/readme.txt` (利用者向け説明書) を zip にまとめて申請。
  readme 同梱を Vector が要件化しているため、単体 exe やインストーラ単独での申請は不可。
- **インストーラ**: Inno Setup 6（`installer/mimageviewer.iss`）
- **配布ビルド**: `.\scripts\build-dist.ps1` (全体テスト → clean → core → launcher → ISCC → portable を 1 コマンド、
  stale 配布物を構造的に防ぐ)。開発中の素早い反復だけ `.\scripts\build-release.ps1` 単体を使う
  (clean/ガードなしなので配布物には使わない)。
- **出力**: `installer/Output/mImageViewer_setup.exe`
- **Vector 申請用 zip**: `mImageViewer_installer_v<VERSION>.zip` に `mImageViewer_setup.exe` と
  `installer/readme.txt` を同梱する (v1.1.0 で `mImageViewer_v<VERSION>.zip` から改名。ポータブル
  zip と接尾辞 `_installer_` / `_portable_` で区別する。**リリース済みの過去版は遡って改名しない**)。
  readme の内容 (動作環境・連絡先・インストール手順・取り扱い種別) は Vector のファイル掲載基準
  (https://www.vector.co.jp/for_authors/upload/standard.html) を満たしていること。
- **ポータブル版ビルド**: 配布時は build-dist.ps1 が内部で `.\scripts\build-portable.ps1` を呼ぶ。
  `cargo build --release --bin mimageviewer-core --features portable --target-dir target-portable`
  (非portable core を上書きしないよう専用 target dir に分離) → loose 同梱フォルダ +
  `dist\mImageViewer_portable_v<VER>.zip` を生成する。`portable` feature で native 依存
  (pdfium / onnxruntime / susie / vst3-host / モデル) を埋め込まず exe 隣から解決し、`data_dir` を
  `<exe_dir>\data` に向ける。launcher は使わず core を `mimageviewer.exe` にリネームして同梱。
  設計・保守方針 (CI guard 等) は [docs/portable-build-plan.md](docs/portable-build-plan.md)。
  `portable` feature の cfg 分岐は `.git/hooks/pre-push` の `cargo check --features portable` が番人。
- **CRT 静的リンク**: `.cargo/config.toml` でメイン exe (x86_64) と Susie ワーカー (i686)
  の両方に `+crt-static` を有効にしている。これにより `VCRUNTIME140.dll` / `MSVCP140.dll`
  など Visual C++ 再頒布可能パッケージへの依存を排除している。
  - メイン exe は `ort` クレートの `load-dynamic` 機能と組み合わせて成立している
    (静的リンク版 `onnxruntime.lib` は動的 CRT 前提でビルドされており、crt-static と
    両立しない)。どちらも触らないこと。
  - **解除すると Vector の「要ソフト」欄指摘が再発する**。
- **設定保存先**: インストーラ版・単体exe版は `%APPDATA%\mimageviewer`、
  ポータブル版は `<exe_dir>\data` (書込不可なら APPDATA へフォールバックせずエラー起動拒否)。

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
- `docs/keymap-spec.md` / `docs/key-customization-impl-plan.md` / `docs/keymap.ini.default` —
  キーボード操作やショートカット割り当てを追加・変更したとき。新しいキー操作を固定扱いに
  する場合も、keymap 対象外である理由を明記する

コードだけ修正してドキュメントを放置しない。設計ドキュメントが腐ると次の修正で同じ罠を踏む。

## 永続データ・スキーマ変更時の判断 (リリース済み / 未リリース)

永続ストア (SQLite DB・設定ファイル・キャッシュ DB・ディスク上のデータ形式) の
スキーマや、そこに保存するデータの内容・意味を変える修正をするときは、
**その機能・データが「前回リリース以降に追加されたもの (= まだ出荷されていない)」か
「リリース済みバージョンに既に存在するもの」かを毎回確認してから進める**。
判断を曖昧にしたまま着手しない。

| 対象 | 方針 |
| --- | --- |
| **未リリース** (前回リリース以降に追加され、まだ出荷していない機能 / データ) | マイグレーション不要。破壊的変更も許容する (スキーマ作り直し・キャッシュ全削除・キー体系変更も OK)。ユーザーの手元に旧データが無いため。開発機にテストで溜まったデータは手動削除で足りる |
| **リリース済み** (既存バージョンに存在する機能 / データ) | マイグレーション必須。ユーザーの手元に旧形式データがあるので、黙って壊すと設定消失・キャッシュ全損・起動不能などの実害になる。スキーマ version gate / 旧形式の読み替え / 段階移行を必ず用意する |

判断材料 / 進め方:

- 前回リリースの範囲は **README.md の更新履歴** と **GitHub Releases** で確認する。
  どのバージョンで出した機能か不明なら git log / git tag で追う。
- コード上は完成して見えても「リリース済み」とは限らない。実装済みでも未出荷の
  サブシステムがあり得る。判断に迷ったら **ユーザーに確認する** (推測で決め打ちしない)。
- 「未リリースなのでマイグレーション不要」と判断したら、その旨をコミットメッセージに
  一言残す (後のレビューで「移行コード忘れ」と誤認されないように)。
- レビュー観点としても恒久的に有効: 永続ストアのスキーマ差分を見たら「対象はリリース
  済みか」を必ず確認し、リリース済みなら移行コード + 後方互換テストが揃っているかを見る。

## リリース手順チェックリスト

リリース時は以下を漏れなく更新・作成すること。

**過去リリースで踏んだ落とし穴・判断基準・復旧手順は
[docs/release-operations.md](docs/release-operations.md) に集約している。**
本チェックリストが手順の正本で、release-operations.md はその補助 (stale core cache /
署名セッション切れ / タグ再打ち直し / FFmpeg LGPL ソース同一性 / ポータブル AV 誤検知 /
配布チャネル別の注意など)。リリースを別セッションや Codex に引き継ぐ前に一読する。

### Phase 0: 変更履歴の準備とユーザーレビュー (必ず最初)

更新履歴は `update_check.rs` 経由でアプリ内アップデート通知ダイアログにそのまま
表示される (= GitHub Releases の `body` がユーザーに読まれる)。誤字・内部用語の
混入・粗い表現があると公開後に取り返しが付かないので、以下の順で進める。

- **README.md の更新履歴セクション** にこのバージョンの新エントリを追加 (旧版と
  同じ `### vX.Y.Z (YYYY-MM-DD)` ヘッダ + 箇条書きフォーマット)。書き方は CLAUDE.md
  「マニュアル・製品ページの記述方針」に従う:
  - **見出しにリリース日を `(YYYY-MM-DD)` 形式で付ける** (他バージョンと揃える)。
    日付はそのリリースの GitHub タグ公開日 (= Phase 1 で製品ページの「最終更新」に
    書くのと同じ日)。`gen-changelog-html.py` がこの括弧内の日付をパースして
    マニュアルの更新履歴ページ (changelog.html) の見出しに併記するので、形式を崩さない
  - 内部実装語 (Tantivy / SQLite / 並行処理用語) は使わない
  - バージョン番号タグ (v0.8.2+ 等) は本文に書かない (見出しに 1 回だけ)
  - 過去のユーザー報告日付 / コミットハッシュは書かない
  - 「⚠️ 」プレフィックスは初回起動時に何かが起きる注意 (索引再構築等) に限定
- **ユーザーに README.md の更新履歴を見せて承認を得る**。OK が出るまで Phase 1 へ
  進まない。
- (任意) GitHub Release 用 body は基本 README.md の該当セクションをそのまま
  コピペで OK。手作業で別途書き直さない (= 表記ゆれを生まない)。
- **⚠️ 8KB 上限チェック (大型リリースで必須)**: アプリ内更新通知は
  `update_check.rs` の `BODY_CAP = 8 * 1024` で **先頭 8KB (UTF-8 バイト)** に切られる。
  README の該当セクションが 8KB を超えると、後半の項目 (= 末尾に置きがちな新機能 /
  バグ修正) が通知に出ない。リリース前に必ずバイト数を測る:
  ```bash
  # 見出しは "### vX.Y.Z (YYYY-MM-DD)" 形式なので、版番号の後ろは空白か行末で区切る
  awk '/^### vX\.Y\.Z( |$)/{f=1} /^### v<前版>( |$)/{f=0} f' README.md | wc -c
  ```
  - **8KB 以内**: README セクションをそのまま Release body に使う (上記)。
  - **8KB 超過**: README はフル版のまま残し、**`docs/release-body-<version>.md` に
    8KB 以内の短縮版を別途作成**する (BOM なし、Markdown。通知ダイアログは Markdown
    レンダリング対応なので見出し・箇条書き可)。短縮版は「目玉の新機能 → 主な改善 →
    主なバグ修正」の順で前方に重要項目を寄せ、全項目は README を参照する旨のリンクを
    冒頭に入れる。**この短縮版ファイルを Phase 4 の Release body に使う** (下記)。
    作成後 `wc -c docs/release-body-<version>.md` で 8192 以下を確認。
    - 注意: 8KB 上限は **更新を受け取る側 (= 旧バージョンのバイナリ)** に焼かれている
      ので、今リリースで `BODY_CAP` を上げても今回の通知には効かない (効くのは次版以降)。
      短縮版で回避するのが確実。
    - 実例: v1.0.0 (README セクション 17.5KB → [docs/archive/release/release-body-v1.0.0.md](docs/archive/release/release-body-v1.0.0.md) 6KB)。

### Phase 1: バージョン番号・関連ファイル更新

承認を得た上で以下を更新:

1. `Cargo.toml` — バージョン番号
2. `installer/mimageviewer.iss` — `MyAppVersion`
3. `installer/readme.txt` — 先頭の版表記・更新履歴リンク (Vector 同梱用)
3.5. `installer/readme_portable.txt` — 先頭の版表記 (ポータブル版 zip 同梱用、v1.1.0+)
4. `htdocs/mimageviewer/index.html` — ダウンロードセクションのバージョン表記と
   **「最終更新: YYYY-MM-DD」の日付** (`.download-info` の `.meta` 行)。日付は今回の
   リリース日 (= GitHub Release 公開日と揃える) を記入する。
   **ポータブル版のダウンロードリンク URL もバージョンを含む** (`mImageViewer_portable_v<VER>.zip`)
   ので、バージョン表記と一緒に link href も更新すること (単体exe / setup.exe は非バージョン名で固定)。
4.5. **マニュアルの更新履歴ページを再生成** — Phase 0 で README の更新履歴セクションが
   承認されたら、`python scripts/gen-changelog-html.py` を実行して
   `htdocs/mimageviewer/manual/changelog.html` を作り直す。changelog.html は README の
   `## 更新履歴` から生成される**生成物**なので手で編集しない (編集すると次回再生成で消える)。
   生成後 `git diff` で最新版エントリが反映されていることを確認する。
5. `htdocs/mimageviewer/manual/index.html` — マニュアルのバージョン表記
5.5. **「重要な変更点」テーブルの追記** (操作・既定の変更があるリリースのみ) —
   [src/version_highlights.rs](src/version_highlights.rs) の `TABLE` に今回バージョンの
   エントリ (`must_read` = 操作・既定の変更、`highlights` = 主な新機能) を追加する。
   更新後初回起動でユーザーに自動表示される (= ④ version-highlights、display-only、内部用語禁止)。
   操作・既定の変更が無いリリースでは追記不要。追記したら
   `cargo test --lib version_highlights::` でテーブルがパースできることを確認。
6. `htdocs/` 以下 — 新機能がマニュアル・製品ページに反映されていることを確認
   - マニュアル左サイドバーを持つ通常ページ 26 ページでリンク一覧が揃っているか
     `htdocs/mimageviewer/manual/` 配下で一括確認:
     ```bash
     cd htdocs/mimageviewer/manual && for f in *.html; do
       grep -q 'sidebar-section' "$f" || continue
       echo "=== $f ==="
       sed -n '/sidebar-section/,/<\/nav>/p' "$f" \
         | grep -E 'href="[a-z-]+\.html"' | wc -l
     done
     ```
     各ページが 26 以外 (= いずれかのページ名リンクが抜けている) なら同期を合わせる。
     `tut-*.html` など `sidebar-section` を持たないチュートリアルページは別レイアウトなので対象外。
     新規の通常ページを追加した際はサイドバーを持つ全ページを更新すること
     (`changelog.html` のサイドバーは `gen-changelog-html.py` が `getting-started.html` から
     コピーするので、他の通常ページを更新してから再生成すれば自動で揃う)。

### Phase 2: 依存物の確認 + 性能回帰チェック

7. PDFium の更新確認（`bash scripts/setup-pdfium.sh check`）
8. ONNX Runtime DLL の配置確認（`bash scripts/setup-ort.sh`、ort クレート更新時は必須）
9. Susie ワーカーの再ビルド（`bash scripts/setup-susie-worker.sh`）
10. FFmpeg LGPL shared build の更新確認（`bash scripts/setup-ffmpeg.sh check` はローリング資産を除外し、
    最新の版付き `autobuild-*` 資産と比較する）。バージョンを上げる場合は
    `vendor/ffmpeg/VERSION`・実 DLL の `ProductVersion`・対応ソースを同じ commit に揃え、
    `src/video/ffmpeg_loader.rs` の DLL 名も一致するか確認。LGPL 通知の更新も忘れずに
    (本ファイル「FFmpeg LGPL DLL 管理」節)
11. VST3 host bridge の確認 (v0.9.0+):
    - `vendor/vst3sdk/` が配置済み (`bash scripts/setup-vst3-sdk.sh`)
    - `vendor/vst3-host/mimageviewer-vst3-host.exe` が最新の C++ ソースでビルド済み
      (`cmake --build crates/vst3-host/build --config Release`)
    - 商用プラグイン (Pro-Q 4 等) で実機確認: 環境設定→動画タブで VST3 を ON →
      管理ウィンドウからプラグインをロード → 動画再生で音声がプラグインを通る /
      HUD の VST ボタンで GUI 開閉 (V キーショートカットは存在しない。VST ボタンは
      フル機能モードのメインウィンドウ・フルスクリーンのみ = detached では非表示)
    ※ `vendor/` 直下の必須ファイルは `build.rs` で起動時にチェックしており、
      不足していると `cargo build` が復旧手順付きで止まるようになっている。

9.5. **bench 回帰チェック** (検索周りに変更を入れたリリースで実施):
   ```bash
   cargo run --release --features dev-tools --bin bench_search -- --docs 50000 --json /tmp/bench_new.json
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

9.7. **idle health smoke** (毎リリース必須。静止中の高速 repaint / 再投入ループを検出):
   ```powershell
   .\scripts\check-idle-health.ps1 -Scenario static-foreground
   .\scripts\check-idle-health.ps1 -NoLaunch -Scenario static-background
   .\scripts\check-idle-health.ps1 -NoLaunch -Scenario video-pin-background
   ```
   - 最初のコマンドが `mimageviewer-core.exe --perf-log` を起動する。各シナリオを準備して
     Enter を押す。前面シナリオは warmup 5 秒中にアプリへフォーカスを戻し、測定 15 秒の
     開始表示後は入力しない。背面シナリオは別ウィンドウを前面に保つ。
   - CPU one-core ratio、update rate、repaint reason streak、同一 thumbnail work、通常 / perf
     ログ増加量のどれかが上限を超えたら exit 1。失敗を閾値緩和だけで通さず、
     `target/idle-health/*-perf.json` の原因と反復 key を確認する。
   - `-Scenario` は上記3値だけを受け付ける。perf log の session PID と測定対象 PID が違えば失敗し、
     空の測定窓は同一 session を確認できた場合だけ完全 sleep として扱う。動画ピンシナリオは
     準備開始後の `thumb.idle_upgrade_ineligible` が無ければセットアップ不成立として失敗する。
   - ZIP / PDF / スマートフォルダ、AI、動画・音楽の非同期経路を変更した場合は、その処理が
     完了した静止状態も追加測定する。詳細は `docs/idle-health-check.md`。

9.8. **Rust 全体テストゲート**:
   ```powershell
   .\scripts\test-full.ps1
   ```
   - `cargo test --workspace --features pack-build-tools --no-fail-fast` で、workspace全体と
     単体テストを持つ補助bin 2本を同じlib buildとして実行する。
   - `build-dist.ps1` が clean 前に自動実行するため、通常は配布ビルドとは別に実行不要。
   - `build-dist.ps1 -SkipRustTests` は、同一ソースが既にこのゲートを通過し、署名や
     packaging だけを再試行する場合に限る。ソース変更後の初回配布では使わない。

### Phase 3: ビルド・配布成果物

10. **配布ビルドは `.\scripts\build-dist.ps1` を使う** (1 コマンドで全体テスト → clean → core → launcher → ISCC → portable)。
    - build-dist.ps1 は Rust 全体テストを通してから
      `cargo clean --release -p mimageviewer -p mimageviewer-launcher` (+ `target-portable` の
      `-p mimageviewer`) してから実コンパイルするので、cargo の偽 up-to-date 由来の **stale 配布物を構造的に防ぐ**
      ([docs/release-operations.md](docs/release-operations.md) §2.1 参照)。内部で build-release.ps1 (常駐 mIV を自動停止して
      LNK1104 を回避) と build-portable.ps1 を子 PowerShell で呼び、各 `$LASTEXITCODE` を検査する。
    - VST3 bridge の C++ を変えていなければ `.\scripts\build-dist.ps1 -SkipVst3Bridge` (cmake 再ビルドを省く)。
    - **コード署名は build-dist.ps1 が既定で ON** (Certum Open Source Code Signing 証明書、SimplySign Desktop
      のクラウド鍵)。配布する全 PE に Authenticode 署名 + RFC3161 タイムスタンプを付ける: 単体exe (launcher) /
      core / susie32 / vst3-host / pdfium / FFmpeg 6 DLL / `mImageViewer_setup.exe` / portable の各 loose PE。
      **`include_bytes!` で埋め込む物は「埋め込み前」に署名する** (内側 vendor PE → core → launcher → setup.exe の順)。
      でないと APPDATA へ展開されたコピーが未署名になり、AV 誤検知
      ([docs/release-operations.md](docs/release-operations.md) §7) が
      再発する。`onnxruntime*.dll` は Microsoft 署名済みなので**再署名しない**、`*.onnx` は PE でないので対象外。
      **前提: 署名前に SimplySign Desktop を起動しログインしておく** (未ログインなら clean 前に `Assert-MivSignReady`
      で即停止)。署名なしで配布物を作るなら `.\scripts\build-dist.ps1 -NoSign`。実装は `scripts\sign-files.ps1`
      (証明書選択は既定 subject `/n "Open Source Developer Taku Sano"`。証明書更新で拇印を固定したいときは
      `$env:MIV_SIGN_SHA1`、TS 変更は `$env:MIV_SIGN_TS`)。ポータブルの vst3-host は署名対応後も当面**非同梱据え置き**。
    - **通常の開発反復は `.\scripts\build-dev.ps1`**。通常feature set（portableなし）を
      `dev-runtime` profileで `target\dev-runtime` へcoreだけビルドし、launcherなしで必要な
      FFmpeg DLLだけを変更時に配置する。引数なしの起動は通常版と同じ
      `%APPDATA%\mimageviewer` を使う。launcher／release最適化／埋め込みasset／変更したVST3
      bridgeまで含む実機確認は `.\scripts\build-release.ps1` (incremental・cleanなし、署名は
      既定OFF) を使う。どちらも stale 検出ガードは持たないので
      **配布物の生成には使わない** (必ず build-dist.ps1)。
    - portable core は専用 target dir `target-portable` に分離して焼くので、非portable の
      `target\release\mimageviewer-core.exe` を上書きしない (フレーバー混入防止)。
    - Vector 申請用 zip は build-dist.ps1 では作らない (下記 11 の手順で別途 `Compress-Archive`)。
11. 配布成果物を 4 種類用意する:
    - `mimageviewer.exe` (単体exe版、mikage.to のみ)
    - `mImageViewer_setup.exe` (インストーラ版、mikage.to・窓の杜・Vector 共通)
    - `mImageViewer_installer_v<VERSION>.zip` (Vector 申請用。`mImageViewer_setup.exe` +
      `installer/readme.txt` を同梱。v1.1.0 で `mImageViewer_v<VERSION>.zip` から改名)
    - `mImageViewer_portable_v<VERSION>.zip` (ポータブル版、mikage.to のみ。
      build-dist.ps1 が内部の `.\scripts\build-portable.ps1` 経由で生成。loose 同梱フォルダ全体を含む)

    単体exe (`target\release\mimageviewer.exe`) と `mImageViewer_setup.exe` も build-dist.ps1 が生成する。
    Vector zip は build-dist 後に `Compress-Archive mImageViewer_setup.exe, installer\readme.txt → dist\` で別途作る。
11.5. **ポータブル版 smoke** (v1.1.0+):
    - `dist\mImageViewer_portable_v<VER>\` を **C ドライブ以外の書込可フォルダ** (D:\ / USB 等) へ
      解凍し、`mimageviewer.exe` を起動 → PDF 表示 / 動画再生 / AI アップスケール / Susie の
      いずれかを 1 回ずつ確認。`<exe_dir>\data` が作られ APPDATA が触られないこと、
      インストール版をトレイ常駐させたまま起動しても両方独立に動く (mutex 分離) ことを確認。
      検証チェックリスト全項目は [docs/portable-build-plan.md](docs/portable-build-plan.md) §8。
12. 依存 DLL の回帰チェック — リリース exe に対して `dumpbin /dependents` を走らせ、
    `VCRUNTIME140.dll` / `MSVCP140.dll` が現れていないことを確認する。もし現れていたら:
    - メイン exe: `ort` クレート機能から `load-dynamic` が抜けていないか確認
    - Susie ワーカー: `.cargo/config.toml` の `i686-pc-windows-msvc` 向け
      `+crt-static` 設定が残っているか確認
12.5. **コード署名の回帰チェック** — 配布成果物 (単体exe / setup.exe / portable の mimageviewer.exe) に
    `signtool verify /pa /v <exe>` を走らせ、`Open Source Developer Taku Sano` 名義の証明書チェーン
    (Certum Code Signing → Certum Trusted Network CA) と **RFC3161 タイムスタンプ**が付いていることを
    目視確認する。build-dist は署名時に検証も済ませているが、最終成果物で 1 度確認する。未署名・失効なら
    SimplySign Desktop のログイン状態を確認 (証明書は 1 年ごとに更新 → 更新時は `sign-files.ps1` の
    既定 subject が一致するか、または `$env:MIV_SIGN_SHA1` を新拇印に更新)。

### Phase 4: GitHub Release 公開

13. ローカルで `git tag v<VERSION>` → `git push origin v<VERSION>` (GitHub `main` も同期)
14. GitHub Releases UI で新リリースを作成。**body の出所は Phase 0 で確定した版に従う**:
    - **通常 (README セクションが 8KB 以内)**: README.md の該当 `### vX.Y.Z` セクション
      本文をそのままコピペ。
    - **短縮版を作った場合 (README セクションが 8KB 超)**: Phase 0 で作成した
      `docs/release-body-<version>.md` の本文をコピペする (README フル版ではなく
      **こちらを使う**)。短縮版が存在するかは `ls docs/release-body-<version>.md` で確認。
      別セッションで Phase 4 を実施する場合もこのファイルの有無で判断できる。
    - どちらの場合も、この body がアプリ内アップデート通知にそのまま表示される。
      **Phase 3 の配布成果物 4 種類すべて** を Assets として添付する:
      `mimageviewer.exe` (単体exe版) / `mImageViewer_setup.exe` (インストーラ版) /
      `mImageViewer_installer_v<VERSION>.zip` (Vector 申請用 zip) /
      `mImageViewer_portable_v<VERSION>.zip` (ポータブル版)。過去リリース (v1.5.0 等) は
      この 4 点を添付済みなので、`gh release view v<前版> --json assets` で添付漏れがないか
      照合する。
    - 実例: v1.0.0 は短縮版 [docs/archive/release/release-body-v1.0.0.md](docs/archive/release/release-body-v1.0.0.md) を使用。
15. 公開後、別マシンから `mimageviewer.exe` を起動 → 起動時更新通知ダイアログで
    body が想定どおりに表示されることを目視確認 (改行・見出し・リンクの崩れチェック)。
    短縮版を使った場合は、目玉の新機能が末尾で切れずに表示されているかを特に確認する。

### Phase 5: 配布チャネルへの反映・申請 (公開後)

GitHub Release 公開後、各配布チャネルへ反映・申請する。**Vector と MS Store は忘れやすいので、
リリースのたびに必ずこのリストを通し、ユーザーに作業を案内すること。**

16. **mikage.to へ反映**:
    - 3 直接DL成果物 (`mImageViewer_setup.exe` / `mimageviewer.exe` /
      `mImageViewer_portable_v<VER>.zip`) を配置。
    - 製品ページ (`htdocs/mimageviewer/index.html`) のダウンロード欄・バージョン表記・「最終更新」
      日付を新版に更新。ダウンロード欄に **「Microsoft Store でも入手可能」バッジ** (`.btn-store`、
      リンク先 `https://apps.microsoft.com/detail/xp8jlwdwv5ls01`) が入っていることを確認。
17. **Vector 申請**: `mImageViewer_installer_v<VERSION>.zip` (`mImageViewer_setup.exe` +
    `installer/readme.txt`) を申請。readme の版表記更新を忘れずに (本書「Distribution」節参照)。
18. **窓の杜** (任意): これまで見送り実績が多い。掲載する場合はインストーラ zip を申請
    (過去の申請知見は [docs/release-operations.md](docs/release-operations.md) §8 参照)。
19. **Microsoft Store 更新申請** (区切りの良い版で。毎リリース必須ではない):
    - **前提**: EXE/MSI 掲載は **Store が既存ユーザーを自動更新しない** (自動更新は MSIX のみ)。
      **既存ユーザーは mIV 自身の更新通知で自己更新**するので、Store 更新は「新規 Store インストール
      の初期版を新しく保つ」ためのもの。**毎リリースで出す必要はなく、区切りの良い版で更新すれば十分**。
    - **① 版付きの直リンクにインストーラを配置**: 署名済み `mImageViewer_setup.exe` を mikage.to の
      版付きパスに**直リンク**で置く。例: `https://mikage.to/mimageviewer/download/v<VER>/mImageViewer_setup.exe`。
      - ⚠ **GitHub Release の URL は使えない** (実体 CDN へ 302 リダイレクトし、Store に
        「リダイレクトのないダウンロード URL を指定して」と却下される)。
      - ⚠ **非バージョン名の setup.exe (上書き運用) も使わない** (Store は提出後のバイナリ変更を
        許さないので、版でパスを分ける)。**提出後はそのファイルを消さない・上書きしない**
        (Store が再DLして再検証する)。
      - リダイレクト無しを確認: `curl -sI <URL>` が `200 OK` (301/302 が出ないこと)、
        `Content-Length` が署名済み setup.exe と一致すること。
    - **② Partner Center で更新**: [partner.microsoft.com](https://partner.microsoft.com/) →
      mImageViewer → 「アプリを更新」→ **パッケージ**のパッケージ URL を新 URL に差し替え →
      **各ページで必ず「下書きの保存」** (保存せず「次へ」だと入力が消える) → 「すべて保存」→
      **Package validation を「実行」** (署名・サイレントインストール・リダイレクトを検証、~30分〜数時間) →
      合格を確認 → **送信**。
    - **③ 認定待ち**: 新規提出と同様 **約1〜3営業日**。合格でメール通知。年齢区分アンケートの回答が
      変わらなければ **IARC レーティングはそのまま引き継がれる** (mIV はローカルビューアなので通常不変)。
    - **確定値の控え** (変更なければ毎回同じ): Architecture=**x64** / Language=**ja** /
      App type=**EXE** / Installer parameters=**`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`** /
      Installation successful=**0** / プライバシーURL=`https://mikage.to/mimageviewer/privacy.html`。
      Microsoft Store ID=**`XP8JLWDWV5LS01`**、公開ページ=`https://apps.microsoft.com/detail/xp8jlwdwv5ls01`。
      コード署名は build-dist.ps1 が実施済み (本書 Phase 3 / 「code signing」)。詳細な経緯・ハマりどころは
      [docs/release-operations.md](docs/release-operations.md) §8 参照。

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

### 基本コマンド (1 ラウンド目)

read-only サンドボックス (ファイル書き換え・ネットワーク・パッケージ操作は禁止) で実行する。
以下は **同じタスクで Codex に意見をもらう最初の 1 回** で使うコマンド。2 ラウンド目以降は
次節「同じタスクは同じセッションで継続する」を使う (毎回 `codex exec` で新規セッションを
始めない)。

| 目的 | コマンド |
|---|---|
| 直前のコミット (`HEAD~1` との差分) をレビュー | `codex exec --sandbox read-only "Review the changes since HEAD~1. Use git diff HEAD~1 and inspect relevant files. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first, ordered by severity."` |
| ブランチ全体 (`main` からの分岐) をレビュー | `codex exec --sandbox read-only "Review this branch against main. Use git merge-base main HEAD and git diff from that base. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first."` |
| 未コミットの作業変更だけレビュー | `codex exec --sandbox read-only "Review the uncommitted changes in this repo. Use git diff and git diff --cached. Focus on bugs, regressions, missing tests, and compatibility risks. Return findings first."` |

### 同じタスクは同じセッションで継続する (resume)

**同じタスク内でレビュー往復をするときは、毎回 `codex exec` で新セッションを開かず、
`codex exec resume` で前回セッションの会話履歴を引き継ぐこと**。新セッションを開き直すと、
Codex は前回どこに着目したか・どの指摘を出したかを覚えておらず、毎回ゼロから差分を読み
直すので精度が落ち、こちらの追問 ("P2 について詳しく" / "この修正で直ったか確認して") も
通じなくなる。

判断基準 (タスク単位):

- **同じタスク** = ユーザーから受け取った 1 つの作業指示の中で、レビュー → 修正 → 再レビュー
  と往復する局面。**同じセッションを使い続ける**。
- **別タスク** = 新しいユーザー指示で作業を始める / コミット粒度が変わる / レビュー観点が
  変わる (例: bug review → security review)。**新セッションを開く** (`codex exec` で開始)。

#### 推奨フロー

1 ラウンド目だけ `codex exec` で開始し、以降は `--last` で直近セッションを継続する。
`--last` は cwd 一致の最新セッションを自動で拾うので SID 管理が不要:

```bash
# 1 ラウンド目 (新セッション開始)
codex exec --sandbox read-only -o /tmp/codex-1.txt \
    "Review the changes since HEAD~1. ..." \
    < /dev/null > /tmp/codex-1-events.log 2>&1
cat /tmp/codex-1.txt   # P1/P2/P3 サマリ

# 2 ラウンド目以降 (同じセッションを継続)
# 注意 1: `codex exec resume` には `--sandbox` フラグが**無い** (codex 0.124 で確認、
#         元セッションの sandbox 設定が引き継がれる)。指定すると `Usage: ...` でエラーになる。
# 注意 2: prompt に `backtick` / $var を含めるケースが多いので、stdin 経由 (`-`) で渡す。
#         heredoc を temp ファイルに書いてから流し込むのが一番安全 (詳細: 後述 §stdin の取り扱い)。
cat > /tmp/codex-prompt.txt <<'PROMPT'
P2 の修正を <commit-hash> で入れた。意図通り直っているか確認して。
PROMPT
codex exec resume --last -o /tmp/codex-2.txt - < /tmp/codex-prompt.txt > /tmp/codex-2-events.log 2>&1
cat /tmp/codex-2.txt
```

#### 明示的に SID を指定したいとき

並行で複数タスクの Codex セッションが走っていて `--last` が別タスクのものを拾う恐れが
あるときは、1 ラウンド目で `--json` を付けて **thread_id** を控え、resume 時に SID を
明示する:

```bash
# 1 ラウンド目 (SID 取得)
codex exec --json --sandbox read-only -o /tmp/codex-1.txt \
    "Review the changes since HEAD~1. ..." \
    < /dev/null > /tmp/codex-1-events.jsonl 2>&1
SID=$(head -1 /tmp/codex-1-events.jsonl | jq -r .thread_id)

# 2 ラウンド目以降 (SID 指定で resume、--sandbox なし、prompt は stdin 経由)
cat > /tmp/codex-prompt.txt <<'PROMPT'
P2 について詳しく説明して
PROMPT
codex exec resume "$SID" -o /tmp/codex-2.txt - < /tmp/codex-prompt.txt > /tmp/codex-2-events.log 2>&1
```

#### 制約・注意点

- **`--ephemeral` を付けない**。これを付けるとセッションがディスクに残らず resume 不可。
  デフォルトは永続化なので普段は何もしなくて OK。
- **resume には `-C / --cd` がない**。1 ラウンド目とは別 cwd で実行できないので、
  レビュー往復は必ず同じディレクトリ (= mimageviewer ルート) から打つ。
- **タスクが切り替わったら新セッションを開く** (= `codex exec` から始め直す)。古いセッション
  の文脈が混ざると逆に精度が落ちる。
- **basis コミット (差分の起点) を resume 中に動かさない**。会話履歴は残るが、Codex が
  参照したファイルの中身は記録されていないので、`HEAD~1` 等で渡した基準コミットが
  rebase で動くと差分の意味が変わる。

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

### stdin の取り扱い (必須)

`codex exec` / `codex exec resume` は **stdin を常に明示的に制御する**こと。Claude Code
の Bash tool 経由で起動すると stdin が必ず何かに繋がっているので、放置すると codex は
それを `<stdin>` ブロックとして読みに行き、EOF まで `Reading additional input from
stdin...` で固まる (5 分以上ハングして手で kill する羽目)。2 つのパターンがある:

**(A) positional 引数で prompt を渡すとき → `< /dev/null` で stdin を閉じる**

```bash
codex exec --sandbox read-only -o /tmp/codex-out.txt "短い prompt" < /dev/null > /tmp/log 2>&1
```

短く `$` / backtick / 改行を含まない prompt 向け。

**(B) `-` (stdin から prompt を読む) を指定するとき → `<` で prompt ソースを渡す**

```bash
# heredoc を temp ファイルに書いてから流し込む (一番安全、長文 OK)
cat > /tmp/codex-prompt.txt <<'PROMPT'
複数行の prompt。`backtick` や $var もそのままリテラル扱いされる。
PROMPT
codex exec resume --last -o /tmp/codex-out.txt - < /tmp/codex-prompt.txt > /tmp/log 2>&1

# process substitution でもよい (短文向け)
codex exec resume --last -o /tmp/codex-out.txt - \
    < <(echo "短い prompt") > /tmp/log 2>&1
```

**(B) を使う理由**: positional 引数は bash の通常評価を通るので、prompt に backtick が
あると **command substitution として実行されて構文エラーや意図しないコマンド実行**が
起きる (`Usage: codex exec ...` で死ぬ実例あり)。`'...'` で single-quote しても、prompt
内に `'` を含めるたびにエスケープ手間が増えるので、長文 / 特殊文字含みは (B) が確実。

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

## 実機検証用バイナリの準備 (Windows ネイティブ機能)

**ユニットテストで再現できない Windows ネイティブ挙動を変更したら、ユーザーに実機検証を
依頼する前に、こちら側で通常profileの検証用バイナリを用意する。core内の挙動は
`.\scripts\build-dev.ps1`、release構成依存の挙動は `.\scripts\build-release.ps1` を使う。**
「実機で確認してください」と口頭で頼むだけで済ませず、その時点で起動できるビルドまで揃えてから
依頼する (対象例: 動画 native presenter / フルスクリーン / 動画→音声モード / VST / D3D11 /
HWND owner・focus・z-order / IME の実挙動 / マルチモニター DPI など)。

### 検証起動時の設定データ保護（必須）

- エージェントは `target\dev-runtime\mimageviewer-core.exe`、
  `target\release\mimageviewer.exe`、
  `target\release\mimageviewer-core.exe`、インストール済み mImageViewer、
  `%APPDATA%\mimageviewer\runtime\*` を UI / Computer Use 検証のために**起動しない**。
  通常profileのdev/releaseビルドは起動だけで実利用中の `%APPDATA%\mimageviewer` を開き、
  設定DBの migration・bak rotation・quarantine・保存を行い得る。ビルドしてユーザーへ
  渡すことは可、起動は不可。
- エージェントが画面を操作するテストは、必ず
  `.\scripts\prepare-portable-smoke.ps1` で作った使い捨てコピー
  `target\portable-smoke\mimageviewer.exe` を使う。データ保存先が
  `target\portable-smoke\data` であることを確認し、利用者のポータブル環境や通常設定を
  コピーして使わない。
- 利用者の実設定が必要なシナリオは、こちらではバイナリ準備までで止め、具体的な確認手順を
  利用者へ渡す。テスト準備として通常設定の削除・リネーム・復元・初期化を行わない。
- 背景: 2026-07-19、古い設定型の検証バイナリが新しい `DetailsColumnId::PageCount` を
  `Corrupted` と誤分類し、main と bak1〜bak10 を順に quarantine した。設定ロード側でも
  unknown variant / field は `Incompatible` として save 抑止し、DB family を変更しない。

- **前提**: `cargo build` / `cargo test` が緑・`cargo fmt --check` 済み・(UI 文言を触ったなら)
  `python scripts/check_ui_glyphs.py` が 0 件であること。**コンパイルが通らない状態で検証
  バイナリを作らない**。
- **core内Windows native挙動の通常実行**: `.\scripts\build-dev.ps1`。通常feature setを
  軽量な `dev-runtime` Cargo profileでビルドし、出力は
  `target\dev-runtime\mimageviewer-core.exe`。引数なしでは通常版と同じ
  `%APPDATA%\mimageviewer` を使う。`dev-runtime` はアプリのデータprofile名ではない。
- **release構成依存の実行**: launcher、release-only cfg／最適化、exact release performance、
  埋め込みasset展開、変更したVST3 bridge、署名、packagingに依存する場合は
  `.\scripts\build-release.ps1`。内部でcore → launcherの2段cargo buildを回し、常駐mIVを
  自動停止してLNK1104を回避する。出力は `target\release\mimageviewer.exe` (launcher) と
  `target\release\mimageviewer-core.exe` (本体)。
- **配布ではないので `build-dist.ps1` は使わない**。配布物を作るときだけclean-firstの
  `build-dist.ps1`。
- **⚠️ エージェントのツールから呼ぶときは `*>&1` を付けない**。PowerShell の `-ErrorAction Stop`
  下で cargo の stderr が terminating error 化して即失敗する
  ([docs/release-operations.md](docs/release-operations.md) §2.2)。PowerShell ツールは stderr を自前で拾うので、素の
  `.\scripts\build-release.ps1` で呼ぶ。失敗する場合は core → launcher の 2 段 cargo build を
  直接叩く。
- **依頼のしかた**: core検証なら
  `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`、release構成検証なら
  `Start-Process -FilePath .\target\release\mimageviewer.exe` をリポジトリルートから実行して
  もらい、検証シナリオを具体的に添える。どちらも引数なしでは実利用中の
  `%APPDATA%\mimageviewer` を使うこと、および起動前にインストール版／常駐tray版を終了する
  ことを明記する。
- **コミットのタイミング**: この種のネイティブ機能はユーザーが実機で確認してからコミットする
  運用 (ユーザーが「OK、コミットして」と言ってから)。検証で不具合が出たら修正 → 再ビルド →
  再依頼を繰り返す。

## Formatting

- **コミット前に必ずワークスペース全体へ `cargo fmt` をかける** (引数なし)。
  リポジトリは常に 100% `cargo fmt` 済みの状態を保つので、全体 `cargo fmt` をかけても
  **編集したファイル以外はゼロ差分**になる (整形すべき箇所が他に残っていないため)。
  かつての「編集ファイル限定」運用は未整形コードが蓄積していた時期の回避策で、
  `cargo fmt -- <path>` はそもそもスコープを絞れず (クレート全体を整形する)、
  実態と乖離していたため全体 fmt に統一した。
- **`cargo fmt` を回したら同じ作業セッション内でコミットする**。fmt 差分を未コミットの
  まま次セッションに持ち越さない (= 前セッションの leftover が新セッションの編集に
  混ざる事故の原因)。通常の編集なら fmt 差分はそのコミットの一部になるだけで、
  整形専用コミットは不要。
- **pre-commit フックが番人**: `.git/hooks/pre-commit` が `cargo fmt --check` を回し、
  未整形コードが混じったコミットを機械的に弾く。Claude Code / Codex / 手作業の
  どの経路でも効く。フックが無い環境 (新規 clone 等) では以下で再作成する:
  ```sh
  cat > .git/hooks/pre-commit <<'HOOK'
  #!/bin/sh
  # Keep the repo 100% `cargo fmt` clean. Policy: CLAUDE.md "## Formatting".
  if ! cargo fmt --check >/dev/null 2>&1; then
      echo "[pre-commit] Rust code is not formatted. Run 'cargo fmt', then re-stage." >&2
      cargo fmt --check 2>/dev/null | grep "^Diff in" | sed 's/ at line.*//' | sort -u >&2
      exit 1
  fi
  HOOK
  chmod +x .git/hooks/pre-commit
  ```
  (git worktree は共通 git dir の hooks を共有するので、worktree 側でも自動で効く。)
- **rustfmt バージョン更新時のみ整形専用コミット**: ツールチェーンを上げると新 rustfmt
  が既存コードを整形し直すことがある。その場合に限り、機能変更と混ぜず
  `style: Apply cargo fmt` の単独コミットで吸収する。
- Claude Code / Codex / 手作業のいずれでも同じ方針を使い、リポジトリを常に
  整形済みに保つ。

## Git Workflow

- **セッション開始時に `git status` を一度確認する**。CLAUDE.md 冒頭の `gitStatus`
  ヘッダは conversation 開始時点の snapshot だが、初手で自分でも `git status` を見て
  「working tree が本当に clean か / 前セッションからの未コミット差分が残っていないか」を
  確かめる。未コミット差分があれば user に「現在以下の未コミット変更があります。今回の
  作業に取り込みますか? それとも別途扱いますか?」と **必ず確認する** (前セッションの
  leftover を黙って自分のコミットに混ぜない)。
- **コミット指示はローカル `master` への統合まで**。「コミットして」と言われた場合は、
  対象差分をコミットしたうえで、同じ作業内でローカル `master` ブランチに merge する。
  ただし user が明示的に feature/worktree ブランチへ残すよう指示した場合、または conflict /
  unrelated dirty worktree により安全に merge できない場合は止めて状況を報告する。push や
  PR（プルリクエスト）の作成は、明示的に指示された場合のみ行う。
- **デフォルトブランチ**: GitHub 上は `main`、ローカルは `master`。リリース時は両方に push する。

### 複数の Claude Code セッションを並行で動かす (lab と本体の同時編集) ⚠️

ラボ (`crates/comic-core` / `tools/comic_lab`) と本体 (`src/` ほか) は編集ファイルが
重ならないので**並行作業自体は可能**だが、**2 つのセッションが同じ作業ツリー
(= 同じ `.git` の index と HEAD) を共有すると git 操作が衝突する**。2026-06 に実害:
一方の `git add -A` がもう一方のステージ変更を巻き込んで誤ったコミットに混入し、さらに
一方の `git reset` が他方の commit を HEAD から落とした (2 回発生)。ファイル編集自体は
衝突していなくても、index・HEAD の取り合いで起きる。

**運用実態 (2026-07 追記)**: 実際には ClaudeCode / Codex の複数セッションで **互いに独立した
領域** を並行開発するのが常態化している。大きな変更や編集ファイルが重なる作業では worktree を
分けるが、**多くは独立した部分の修正なので、確認作業のしやすさを優先して単一の master 作業
ツリー上で並行**させている。worktree は「やむを得ず」ではなく状況次第で選ぶ選択肢であり、
master 共有で進めるときは下記「1 つの作業ツリーを共有する場合の規律」を **通常運用として** 守る。

**推奨 = 編集が重なる / 大規模なときは `git worktree` を分ける**。index・HEAD が独立し、
取り合いが原理的に起きなくなる:

- ラボ用 worktree は **vendor/ 不要**。`cargo {check,test} -p comic_lab -p comic-core`
  は本体パッケージ (`mimageviewer`) をビルドしないので **root の build.rs が走らず**、
  pdfium/ffmpeg/ort/susie の DLL チェックに引っかからない。junction も張らないので下記
  「worktree + Windows junction の地雷」のリスクも無い:
  ```bash
  git worktree add ../mimageviewer-lab -b lab    # vendor セットアップ不要
  cd ../mimageviewer-lab
  cargo test -p comic-core && cargo check -p comic_lab
  ```
  ⚠ ラボ worktree では **bare `cargo build` を打たない** (全 member をビルドして本体
  build.rs が走り vendor チェックで失敗する)。必ず `-p comic_lab` / `-p comic-core` を付ける。
- 各 worktree は**自分のブランチ**にコミットし、作業完了時に**一方ずつ** master へ merge する。
- 撤収は必ず `scripts/safe-worktree-remove.ps1` 経由 (下記「worktree + Windows junction」)。

**やむを得ず 1 つの作業ツリーを共有する場合の規律** (worktree を分けないとき):

- **コミットは必ず pathspec commit `git commit -- <自分のパス>` を使う**。bare `git commit`
  は**共有 index 全体**をコミットするため、`git add` した直後でも、相手が並行で stage した
  ファイルを巻き込む (2026-06 実害: 相手の `src/*` が自分のコミットに混入)。`git commit --
  <paths>` は指定パスだけを working tree からコミットし、index の他の内容を無視するので
  race-proof。**`-F msgfile` / `-m` 等のオプションは `--` の前に置く** (後ろに置くと pathspec
  扱いされてエラー)。例: `git commit -F /tmp/msg.txt -- crates/comic-core tools/comic_lab`。
- **`git add -A` / `git add .` 禁止**。明示 add でも上記のとおり bare commit だと相手の stage
  を拾うので、最終的な防御は pathspec commit。
- **HEAD が自分のコミットでないときに `git reset` / `rebase` / 履歴書き換えをしない**。
  実行前に必ず `git log --oneline -5` で「最新が本当に自分の commit か」を確認する。相手の
  commit を巻き戻すと相手の作業が HEAD から消える。
- **コミット直後に `git log --oneline -1` で自分の commit が HEAD にあるか確認**する。消えて
  いたら相手の操作に巻き込まれている → `git reflog` か退避ブランチから復旧する。
- **大きめの作業は退避ブランチを作っておく**: `git branch -f <name> HEAD`。相手の reset で
  master が巻き戻っても `git reset --hard <name>` で復旧できる (2026-06 はこれで救済した)。
- **`git diff` / `git status` に出る変更を、自分 (や自分の subagent = Codex) の変更と決めつけない**。
  共有ツリーでは **別セッションの concurrent 編集が混ざって見える**。想定外の変更を「subagent が
  スコープ逸脱した」と誤認して `git checkout HEAD -- <path>` / `git restore` で消すと、**別セッションの
  未コミット作業を破壊する (未コミットなので git で復元不能)**。2026-07 実害: 別セッションの MS Store
  作業 (index.html の 35 行) を「Codex の逸脱」と誤認して `git checkout HEAD` で破棄した (Codex の
  「触っていない」報告は実は正確だった)。**対処**: 想定外ファイルは **working tree から消さず、
  自分の pathspec commit から外すだけ** にする。subagent の変更範囲は subagent 自身の report と
  突き合わせる (diff に出た ≠ 自分の subagent の変更)。判断が付かなければユーザーに「これは別
  セッションの作業か」を確認してから触る。

### worktree + Windows junction の地雷 ⚠️

並行作業のため `git worktree add` で別 worktree を切るときに、`vendor/` を main worktree から
**junction (NTFS reparse point) で共有する設計**を選ぶと、撤収時に災害が起きる:

```
[NG] git worktree remove で junction ごと再帰削除される事故 (2026-05-13 / 2026-05-14 実害):
  C:\home\mimageviewer-sqlite\          ← worktree
    └─ vendor\  (junction → C:\home\mimageviewer\vendor\)
  ↓ git worktree remove C:\home\mimageviewer-sqlite
  → git は junction を **通常ディレクトリ扱いで** 中身まで再帰削除
  → main worktree の C:\home\mimageviewer\vendor\ の中身が全消失
  → cargo build が pdfium.dll / FFmpeg DLL / ONNX Runtime DLL / Susie ワーカーすべて
    無いと言って失敗、bootstrap-vendor.sh で再 DL する羽目になる
```

#### 鉄則: `git worktree remove` を直接呼ばない

**この repo では `git worktree remove` を直接実行しない。必ず
`scripts/safe-worktree-remove.ps1` を経由する**。文書化だけでは 2026-05-13 → 翌日
2026-05-14 と連続で事故ったので、機械的に防ぐ仕組みを置いた。

ラッパーが実行する手順:
1. 対象 worktree を再帰スキャン (junction には**降りない**) して reparse point を列挙
2. 見つけた junction を `cmd /c rmdir` で **リンクだけ** 削除 (リンク先は無事)
3. 全部 unlink できてから `git worktree remove <path>` を実行

```powershell
# 全 worktree の junction 状況を一覧 (非破壊)
.\scripts\safe-worktree-remove.ps1 -Audit

# 何が起きるかを確認するだけ (非破壊)
.\scripts\safe-worktree-remove.ps1 <worktree-path> -DryRun

# 実際に安全に削除
.\scripts\safe-worktree-remove.ps1 <worktree-path>
.\scripts\safe-worktree-remove.ps1 <worktree-path> -Force   # dirty worktree 用
```

ラッパーは main worktree (= cwd repo) を対象に指定すると refuse する。

**Claude セッション中の運用**: worktree を撤収する指示を受けたら、`git worktree remove ...`
の前に必ず `.\scripts\safe-worktree-remove.ps1 ...` に置換する。`gh`/`git` の他コマンドと
混ぜて連続実行する場合も同じ。これは取り返しがつかない事故なので例外なし。

#### 設計面の鉄則

`vendor/`, `target/`, runtime DLL / model / SDK などは worktree 間で **junction / symlink /
reparse point 共有しない**。個別サブディレクトリ単位でも禁止。過去に worktree 撤収時の
再帰削除で main 側の実体を消した事故が複数回あるため、リンク共有は選択肢に入れない。

新規 worktree で依存ファイルが必要な場合は、以下のどちらかにする:

- `bash scripts/bootstrap-vendor.sh` を worktree 側で流して per-worktree な `vendor/` を作る。
- 既存 worktree / backup から必要なサブディレクトリを `Copy-Item -Recurse` などで**実体コピー**
  する。例: `Copy-Item -Recurse C:\home\mimageviewer\vendor\ffmpeg vendor\ffmpeg`

削除・撤収時は、対象が意図した worktree 配下であることと junction / symlink / reparse point で
ないことを確認してから消す。worktree 自体の撤収は引き続き
`scripts/safe-worktree-remove.ps1` 経由にする。

## User: Background

- Comfortable reading C++ but not familiar with Rust's borrow checker details
- Has RTX 4090, Windows 11
- AI-assisted development workflow: Claude generates code, user reviews and tests

## Claude Code tool call reliability rules（ツール呼び出しの生テキスト漏れ対策）

**症状**: ツール呼び出しの直前に `call` / `court` / `<invoke name="...">` /
`<parameter name="...">` のような生テキストが出て、ツールが実行されずに失敗する。

**根本原因 (2 段構え)**:
1. **制御トークン破損** — ツール呼び出しの開始タグが、サンプリング時に語彙的に隣接する
   別トークン (`call` / `court` 等) に化ける。ハーネスがパースできず生テキストとして漏れる。
2. **自己汚染 (in-context few-shot poisoning)** — 壊れた呼び出しが会話履歴に残ると、
   モデルがそれを「正しい例」と誤認し、以降の呼び出しでも決定論的に同じ壊れ方を繰り返す。
   → **セッション内でリトライを重ねても直らない。むしろ悪化する。**

### 発生確率を下げる (予防)

- **ツール呼び出しはメッセージの先頭・単独で出す**。同じメッセージ内で呼び出し前に説明文を
  書かない (説明はツール結果が返ってから書く)。
- **1 メッセージ 1 ツール呼び出し**。多数の Read/Edit/Bash を 1 応答にまとめて撃たない。
- **ツール引数は短く保つ**。段落級の長い引数は避ける。長い / 複雑なシェルコマンドは一旦
  スクリプトファイルに書いてから実行する。
- 大きな全文 Write より、小さな targeted Edit を優先する。
- **セッションを長く引きずらない**。複数日 resume や 1M トークン級の巨大文脈で発生率が上がる。
  区切りごとに git / メモに状態を保存し、必要なら新セッションに移る。
- マークアップ密度の高い大型 skill は途中ロードを避け、セッション冒頭で読み込む。
- 同時起動の MCP サーバや background Bash を増やしすぎない。

### 発生してしまったときの対応 (最重要)

- 生テキストが出たら、そのツール呼び出しは**実行されていない**とみなす。
- **リトライは最大 1〜2 回まで**。それで直らなければ**それ以上リトライしない** (履歴汚染が
  進み、決定論的に失敗し続けるため。粘るほど悪化する)。
- 直らない場合は**ユーザーに「新しいセッションを開始 (`/clear`) してほしい」と平文で伝える**。
  セッション内で確実に回復する手段は `/clear` (汚染履歴の切り離し) だけ。
- **`/compact` を回復手段にしない** (compact 直後に再発する)。フルな新セッションを選ぶ。
- 「もっと注意して」系の自己叱咤・追加指示は効かない (注意の問題ではなく文脈アンカーの問題)。
- 更新で直る保証はない (公式修正は未出荷の時期がある) が、Claude Code は最新版に保つ。
- (未確認の回避策) Opus 4.8 で頻発する場合、モデルを Opus 4.7 / Sonnet に切り替えると
  減るという報告がある。確実ではないので、まずは上記の `/clear` を優先する。

### 一般の後処理

- After any Write/Edit, verify with Read or git diff that the file actually changed.
