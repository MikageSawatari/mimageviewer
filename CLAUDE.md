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
| ZIP / PDF 対応が必要な機能 | [docs/virtual-folders.md](docs/virtual-folders.md) |
| 補正 / プリセット / AI アップスケール / 消しゴム | [docs/preset-and-adjustment.md](docs/preset-and-adjustment.md) |

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
- **ZIP support**: `zip` crate
- **PDF support**: `pdfium-render` crate + PDFium DLL (exe に埋め込み) + マルチプロセスワーカープール (3 プロセス並列レンダリング)
- **PDF password**: `windows-dpapi` crate (DPAPI 暗号化でパスワード永続保存)
- **AI upscaling**: `ort` crate (ONNX Runtime v2 + DirectML EP)。Real-ESRGAN / waifu2x ONNX モデルでタイル分割 4x アップスケール
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
│   ├── ui_fullscreen.rs     # フルスクリーン表示
│   ├── ui_helpers.rs        # UI ヘルパー関数
│   ├── ui_metadata_panel.rs # フルスクリーン メタデータパネル（AI + EXIF）
│   ├── ui_dialogs/          # ダイアログ群
│   │   ├── mod.rs
│   │   ├── preferences.rs        # 環境設定
│   │   ├── cache_manager.rs      # キャッシュ管理
│   │   ├── cache_policy.rs       # キャッシュ生成設定
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
│   ├── video_thumb.rs       # 動画サムネイル取得（Windows Shell API）
│   ├── zip_loader.rs        # ZIP アーカイブ内画像列挙・読み込み
│   ├── pdf_loader.rs        # PDF ページ列挙・レンダリング（PDFium）
│   ├── pdf_passwords.rs     # PDF パスワード DPAPI 暗号化保存
│   ├── fs_animation.rs      # アニメーション GIF / APNG デコード
│   ├── gpu_info.rs          # GPU 情報取得（VRAM サイズ等）
│   ├── monitor.rs           # モニター情報取得（DPI 等）
│   ├── stats.rs             # 読み込み統計
│   ├── logger.rs            # パフォーマンス分析用ファイルロガー
│   └── bin/
│       └── bench_thumbs.rs  # サムネイル生成ベンチマーク
├── scripts/
│   └── setup-pdfium.sh      # PDFium DLL ダウンロードスクリプト
├── vendor/
│   ├── pdfium/              # PDFium DLL（.gitignore、setup-pdfium.sh で取得）
│   │   └── bin/pdfium.dll   # include_bytes! で exe に埋め込まれる
│   └── models/              # AI ONNX モデル（.gitignore、配布スクリプトなし）
│       └── *.onnx           # include_bytes! で exe に埋め込まれる。
│                            # 新規開発環境では %APPDATA%\mimageviewer\models\
│                            # (インストール済み環境が展開したもの) からコピーする
├── Cargo.toml
└── Cargo.lock
```

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

## Distribution

- **mikage.to**: インストーラ (.exe) + exe 単体の両方を提供
- **窓の杜・Vector**: インストーラ (.exe) のみで申請
- **インストーラ**: Inno Setup 6（`installer/mimageviewer.iss`）
- **ビルド**: `cargo build --release` → `ISCC.exe installer\mimageviewer.iss`
- **出力**: `installer/Output/mImageViewer_setup.exe`
- **設定保存先**: `%APPDATA%\mimageviewer`（インストーラ版・単体版共通）

## コード修正時のドキュメント同時更新

機能の追加・変更・削除を行った場合は、以下のドキュメントも同時に更新すること:

- `htdocs/mimageviewer/manual/` — ユーザー向けマニュアル（設定・操作方法の変更を反映）
- `htdocs/mimageviewer/index.html` — 製品ページ（新機能の紹介・機能一覧の更新）
- `docs/spec.md` — 仕様書（設定項目・内部仕様の変更を反映）
- `docs/architecture-overview.md` — モジュールが増減した、永続化ストアを追加した等の構造変化
- `docs/display-pipeline.md` — 表示テクスチャ優先順位・変換合成順序・変換適用ポイントを変えたとき
- `docs/async-architecture.md` — ワーカーを増やした、共有アトミック/チャネルを追加した、キャンセル規約を変えたとき
- `docs/virtual-folders.md` — ZIP/PDF の分岐表・キャッシュキー規則・DB キー正規化を変えたとき
- `docs/preset-and-adjustment.md` — キャッシュ無効化ルール・補正/AI の適用順序・プリセットの保存先を変えたとき

コードだけ修正してドキュメントを放置しない。設計ドキュメントが腐ると次の修正で同じ罠を踏む。

## リリース手順チェックリスト

リリース時は以下を漏れなく更新すること:

1. `Cargo.toml` — バージョン番号
2. `installer/mimageviewer.iss` — `MyAppVersion`
3. `htdocs/mimageviewer/index.html` — ダウンロードセクションのバージョン表記
4. `htdocs/mimageviewer/manual/index.html` — マニュアルのバージョン表記
5. `README.md` — 更新履歴セクションに新バージョンの変更点を追加
6. `htdocs/` 以下 — 新機能がマニュアル・製品ページに反映されていることを確認
7. PDFium の更新確認（`bash scripts/setup-pdfium.sh check`）

## Git Workflow

- **コミット指示はローカルコミットのみ**。「コミットして」と言われた場合は `git commit` までで止める。
  PR（プルリクエスト）の作成は、明示的に「PRを作って」と指示された場合のみ行う。
- **デフォルトブランチ**: GitHub 上は `main`、ローカルは `master`。リリース時は両方に push する。

## User: Background

- Comfortable reading C++ but not familiar with Rust's borrow checker details
- Has RTX 4090, Windows 11
- AI-assisted development workflow: Claude generates code, user reviews and tests
