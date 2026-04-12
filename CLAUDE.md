# mimageviewer - Project Context

## Overview

A Windows 11 native image viewer built in Rust. Inspired by ViX (legacy 32-bit viewer),
modernized with GPU acceleration and AI upscaling. Single-window design replacing ViX's
dual-window approach.

## Tech Stack

- **Language**: Rust (edition 2024, stable MSVC toolchain)
- **GUI**: eframe 0.33 + egui 0.33 (wgpu backend)
- **Image decoding**: `image` crate (JPEG, PNG, GIF, WebP, BMP) + WIC (HEIC, AVIF, JXL, TIFF, RAW)
- **Parallel loading**: `rayon` (dedicated thread pool per folder load)
- **Thumbnail cache**: SQLite via `rusqlite` (bundled), WebP encoding via `webp` crate
- **Video thumbnails**: Windows Shell API (IShellItemImageFactory)
- **ZIP support**: `zip` crate
- **PDF support**: `pdfium-render` crate + PDFium DLL (exe に埋め込み)
- **PDF password**: `windows-dpapi` crate (DPAPI 暗号化でパスワード永続保存)
- **GPU upscaling (Phase 2, planned)**: NVIDIA NGX DLISR via C FFI
- **Build tool**: cargo (MSVC toolchain on Windows)

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
│   ├── grid_item.rs         # GridItem / ThumbnailState 定義
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
│   └── pdfium/              # PDFium DLL（.gitignore、setup-pdfium.sh で取得）
│       └── bin/pdfium.dll   # include_bytes! で exe に埋め込まれる
├── Cargo.toml
└── Cargo.lock
```

## Implementation Phases

1. **Phase 1** ✅ — コアビューワー（グリッド・フルスクリーン・設定永続化）
2. **Phase 1.5** ✅ — サムネイルカタログ（SQLite + WebP）
3. **Phase 2** 🔜 — AI アップスケール（NVIDIA NGX DLISR）
4. **Phase 3** ✅ — お気に入り・ツールバー・ZIP・WIC・動画・アニメーション

## Key Design Decisions

### UI / スクロール
- **Virtual scrolling**: `show_viewport` で全体高さだけ確保し、可視行のみ描画。
  スクロールオフセットは App が自前管理（egui の自動スクロールは使わない）。
- **Row snapping**: オフセットは常に `cell_size` の整数倍。最大オフセットも
  `ceil((total_h - viewport_h) / cell_size) * cell_size` で行境界に揃える。
- **Mouse wheel**: `ctx.input_mut` で MouseWheel イベントを消費し、1行分に変換。

### サムネイルロード
- **Grid contents**: フォルダ先頭（名前順）、画像後続（ソート順設定可）。非画像は無視。
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
- NVIDIA NGX 呼び出し部分のみ `unsafe` ブロックに局所化（Phase 2）。

## Supported Image Formats

- **内蔵**: JPEG, PNG, GIF, WebP, BMP
- **WIC 経由**: HEIC, HEIF, AVIF, JXL, TIFF, TIF, DNG, CR2, CR3, NEF, NRW, ARW, SRF, SR2, RAF, ORF, RW2, PEF, PTX, RWL, IIQ
- **動画（サムネイルのみ）**: MP4, AVI, MOV, MKV, WMV, MPG, MPEG

## Performance Notes

- **デコード時間（リリースビルド実測）**: p50 = 4.2ms, p90 = 10.8ms（JPEG）
- **キャッシュ読み込み**: 2〜3ms/枚（WebP デコード）
- **キャンセル遅延**: 旧タスクが1枚のデコード中の場合、最大1デコード時間待つ
- **ログ**: `cargo run` 時に `mimageviewer.log` へ出力（.gitignore 済み）

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
- **出力**: `installer/Output/mImageViewer_<version>_setup.exe`
- **設定保存先**: `%APPDATA%\mimageviewer`（インストーラ版・単体版共通）

## Git Workflow

- **コミット指示はローカルコミットのみ**。「コミットして」と言われた場合は `git commit` までで止める。
  PR（プルリクエスト）の作成は、明示的に「PRを作って」と指示された場合のみ行う。

## User: Background

- Comfortable reading C++ but not familiar with Rust's borrow checker details
- Has RTX 4090, Windows 11
- AI-assisted development workflow: Claude generates code, user reviews and tests
