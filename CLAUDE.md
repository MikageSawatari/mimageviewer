# mimageviewer - Project Context

## Overview

A Windows 11 native image viewer built in Rust. Inspired by ViX (legacy 32-bit viewer),
modernized with GPU acceleration and AI upscaling. Single-window design replacing ViX's
dual-window approach.

## Tech Stack

- **Language**: Rust 1.94.1 (stable, MSVC toolchain)
- **GUI**: eframe 0.31 + egui 0.31 (wgpu backend)
- **Image decoding**: `image` crate (JPEG, PNG, WebP, BMP)
- **Parallel loading**: `rayon` (dedicated 8-thread pool per folder load)
- **Thumbnail cache (planned)**: SQLite via `rusqlite` (bundled)
- **GPU upscaling (fullscreen, Phase 2)**: NVIDIA NGX DLISR via C FFI
- **Build tool**: cargo (MSVC toolchain on Windows)

## Project Structure

```
mimageviewer/
├── CLAUDE.md
├── docs/
│   ├── spec.md             # 全体仕様書（実装状況チェックリスト付き）
│   └── catalog-design.md  # サムネイルカタログ設計書
├── src/
│   ├── main.rs             # エントリポイント + フォント設定 + logger::init()
│   ├── app.rs              # App 構造体 + eframe::App 実装（UI全体）
│   └── logger.rs           # パフォーマンス分析用ファイルロガー
├── Cargo.toml
└── Cargo.lock
```

## Implementation Phases

1. **Phase 1** ✅（ほぼ完了）— コアビューワー
2. **Phase 1.5** 🔜（次）— サムネイルカタログ（SQLite）
3. **Phase 2** — AI アップスケール（NVIDIA NGX DLISR）
4. **Phase 3** — お気に入り・設定永続化

## Key Design Decisions

### UI / スクロール
- **Virtual scrolling**: `show_viewport` で全体高さだけ確保し、可視行のみ描画。
  スクロールオフセットは App が自前管理（egui の自動スクロールは使わない）。
- **Row snapping**: オフセットは常に `cell_size` の整数倍。最大オフセットも
  `ceil((total_h - viewport_h) / cell_size) * cell_size` で行境界に揃える。
- **Mouse wheel**: `ctx.input_mut` で MouseWheel イベントを消費し、1行分に変換。

### サムネイルロード
- **Grid contents**: フォルダ先頭（名前順）、画像後続（名前順）。非画像は無視。
- **Cancellation**: `Arc<AtomicBool>` キャンセルトークン。`load_folder` 呼び出し時に
  旧トークンを `true` にして旧タスクを中断。
- **Per-load thread pool**: フォルダごとに新規 `rayon::ThreadPool`（8スレッド）を作成。
  旧フォルダのプールと競合せず新タスクが即座に開始できる。
- **Priority loading**: Phase1（可視範囲）→ Phase2（残り）の2フェーズ並列処理。
- **Repaint loop**: `Pending` なサムネイルがある間は毎フレーム `ctx.request_repaint()`。
  バックグラウンド完了後も egui が起きない問題を回避。
- **Thumbnail size**: `max(last_cell_size, 512).min(1200)` px。4K・4列では ≈900px。

### フォルダ走査
- **Folder tree navigation (Ctrl+↑↓)**: 深さ優先前順トラバーサル。
  次 = 最初の子 → 次の兄弟 → 祖先の次の兄弟（再帰）。
  前 = 前の兄弟の最後の子孫 → 親。
- **BS key**: 親フォルダへ。
- **Path comparison**: Windows の大文字小文字非区別に対応するため小文字化して比較。

### セキュリティ
- `image` クレート（純粋Rust、メモリ安全）で画像デコード。WIC は使用しない。
- NVIDIA NGX 呼び出し部分のみ `unsafe` ブロックに局所化（Phase 2）。

## Supported Image Formats

JPEG (.jpg, .jpeg), PNG (.png), WebP (.webp), BMP (.bmp)

## Settings (persisted as JSON, Phase 3)

- `grid_cols`: サムネイルグリッド列数（デフォルト: 4）
- `grid_rows`: サムネイルグリッド行数（デフォルト: 3）
- `favorites`: お気に入りフォルダパス一覧

## Performance Notes

- **デコード時間（実測）**: 1枚あたり 100〜500ms（JPEG・サイズ依存）
- **キャンセル遅延**: 旧タスクが1枚のデコード中の場合、最大1デコード時間待つ
- **ログ**: `cargo run` 時に `mimageviewer.log` へ出力（.gitignore 済み）

## Git Workflow

- **コミット指示はローカルコミットのみ**。「コミットして」と言われた場合は `git commit` までで止める。
  PR（プルリクエスト）の作成は、明示的に「PRを作って」と指示された場合のみ行う。

## User: Background

- Comfortable reading C++ but not familiar with Rust's borrow checker details
- Has RTX 4090, Windows 11
- AI-assisted development workflow: Claude generates code, user reviews and tests
