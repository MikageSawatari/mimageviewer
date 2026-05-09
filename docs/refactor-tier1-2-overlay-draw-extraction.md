# Refactor Tier 1 #2: native_presenter.rs から overlay_draw.rs を分離

Date: 2026-05-09
Reviewer target: Codex (計画レビュー + 実装確認)
Author: Claude
Status: 実装済み (2026-05-09)

## Implementation result

- `src/video/native_presenter.rs` を `src/video/native_presenter/mod.rs` に移動
- `src/video/native_presenter/overlay_draw.rs` を新設
- `mod.rs`: 3909 行、`overlay_draw.rs`: 2283 行
- `impl NativeVideoPresenter` / `impl NativeBlackBackground` /
  `impl NativeEguiOverlay` / `impl NativeTestOverlay` は `mod.rs` に残した
- D3D11 device 作成、wgpu surface format 選択、DPI / egui input 変換、
  swapchain copy / pixel probe test helper は `mod.rs` に残した
- 検証:
  - `cargo build --bin mimageviewer-core`
  - `cargo test --lib --no-fail-fast`

検証時はローカル環境の都合で `LIBCLANG_PATH` に Visual Studio BuildTools の
LLVM ディレクトリを指定し、`cargo test` では `vendor/ffmpeg/bin` を `PATH` に追加した。

## Goal

`src/video/native_presenter.rs` (現在 6179 行) から **drawing function 群** を
`src/video/native_presenter/overlay_draw.rs` に分離し、本体ファイルを 4500 行前後まで
縮小する。

**impl block は分割しない**。`impl NativeVideoPresenter` / `impl NativeBlackBackground` /
`impl NativeEguiOverlay` / `impl NativeTestOverlay` はすべて `mod.rs` に残す。
分離するのは「`fn` (= self を取らない自由関数)」のみ。

## Why

- 現状 `native_presenter.rs` 6179 行のうち約 1900 行が drawing 自由関数群
- 自由関数だけを別ファイルに移すのは「機械的な move」で、impl ブロックを跨がないため
  state coupling のリスクなし
- 移動後の行数: `mod.rs` ~4250 行、`overlay_draw.rs` ~1900 行 (~30% 削減)
- 視認性向上: D3D11 / DComp / overlay state 機械の核と、純粋な egui 描画ロジックが
  視覚的に分離する

## ファイル構造の変更

```
変更前:
src/video/native_presenter.rs   (6179 行、単一ファイル)

変更後:
src/video/native_presenter/
├── mod.rs                       (~4250 行、impl + 残存 helper + 入力処理 + D3D11 経路)
└── overlay_draw.rs              (~1900 行、egui 描画自由関数群)
```

## 移動対象の関数一覧 (32 個)

### Drawing functions (主役)

| 関数名 | 旧位置 | 概要 |
|---|---:|---|
| `draw_native_perf_overlay` | 3410 | 性能オーバーレイ (P キー) |
| `draw_native_jump_panel` | 3584 | 左ジャンプパネル本体 |
| `draw_native_jump_row` | 3740 | ジャンプパネルの 1 行 |
| `draw_native_bookmark_title_editor` | 3887 | ブックマーク名編集 |
| `draw_native_top_button` | 3968 | 上部ボタン (info / 回転 / 速度等) |
| `draw_native_frame_step_button` | 4000 | フレーム送りボタン |
| `draw_native_checkmark` | 4210 | スライドショー チェック |
| `draw_native_center_status` | 4250 | 中央ステータス (一時停止表示等) |
| `draw_native_center_pause_controls` | 4310 | 中央「最初から / 続きから」 |
| `draw_native_toast` | 4407 | トースト通知 |
| `draw_native_top_bar` | 4469 | 上部ホバーバー |
| `draw_native_vst3_panel` | 4650 | VST3 プレイバックパネル |
| `draw_native_vst3_slot_row` | 4794 | VST3 スロット行 |
| `draw_native_metadata_panel` | 4892 | 右メタ情報パネル |
| `draw_native_tile_overlay` | 5026 | タイルモード一覧 |
| `draw_timeline_marker` | 5353 | タイムラインマーカー (ピン/ブックマーク/チャプター) |

### アイコン描画

| 関数名 | 旧位置 |
|---|---:|
| `draw_overlay_frame_step_icon` | 4046 |
| `draw_overlay_camera_icon` | 4069 |
| `draw_overlay_tile_grid_icon` | 4084 |
| `draw_overlay_perf_graph_icon` | 4101 |
| `draw_overlay_vst3_top_icon` | 4123 |
| `draw_overlay_close_icon` | 4172 |
| `draw_overlay_vst3_gui_icon` | 4180 |
| `draw_overlay_button_bg` | 5381 |
| `draw_overlay_play_icon` | 5392 |
| `draw_overlay_pause_icon` | 5404 |
| `draw_overlay_replay_icon` | 5422 |
| `draw_overlay_loop_icon` | 5444 |
| `draw_overlay_bookmark_icon` | 5483 |
| `draw_overlay_pencil_icon` | 5499 |
| `draw_overlay_pin_icon` | 5526 |
| `draw_overlay_speaker_icon` | 5556 |

### Layout / 計算 helper

| 関数名 | 旧位置 | 概要 |
|---|---:|---|
| `fit_rect_in_rect` | 5225 | アスペクト比フィット |
| `native_jump_panel_width` | 5234 | 定数 (数値だが fn) |
| `native_metadata_panel_width` | 5238 | 定数 |
| `native_panel_top` | 5242 | 定数 |
| `native_panel_hover_bottom` | 5246 | 計算 |
| `native_panel_hover_rect` | 5250 | 矩形計算 |
| `native_jump_panel_rect` | 5259 | 矩形計算 |
| `native_metadata_panel_rect` | 5268 | 矩形計算 |
| `native_vst3_panel_rect` | 5278 | 矩形計算 |
| `native_vst3_slot_list_height` | 5294 | リスト高さ |

### Format / 比較 helper

| 関数名 | 旧位置 |
|---|---:|
| `metadata_clean_text` | 5299 |
| `timeline_markers_match` | 5325 |
| `jump_entries_match` | 5335 |
| `target_has_marker` | 5347 |
| `format_overlay_time` | 5643 |
| `format_tile_interval` | 5655 |
| `format_fps` | 5664 |
| `format_bitrate` | 5672 |
| `truncate_overlay_text` | 5684 |
| `finite_nonnegative` | 5627 |
| `finite_video_volume` | 5635 |
| `thumbnail_rgba_key` | 5220 |
| `native_perf_expected_frame_ms` | 5171 |
| `native_perf_expected_frame_ms_from_samples` | 5182 |
| `native_perf_expected_frame_ms_from_values` | 5191 |
| `native_perf_sample_has_frame_gap` | 5206 |
| `native_vst3_chain_slot_tooltip` | 4885 |

### 関連 enum

| 名称 | 旧位置 | 注記 |
|---|---:|---|
| `enum NativeTopButtonGlyph` | 3960 | drawing 専用 enum (`draw_native_top_button` の引数型)、一緒に移動 |

## 移動 **しない** 関数 (= mod.rs に残す)

drawing でも overlay でもない、D3D11 / 入力処理 / wgpu surface セットアップ系:

| 関数名 | 旧位置 | 残す理由 |
|---|---:|---|
| `create_present_d3d11_device` | 507 | D3D11 device 作成 |
| `configure_overlay_fonts` | 544 | wgpu Renderer 構築の一部 |
| `choose_overlay_surface_format` | 5608 | wgpu surface format 選択 |
| `pixels_per_point_for_hwnd` | 5699 | DPI 計算 (Win32 API) |
| `egui_modifiers` | 5709 | 入力イベント変換 |
| `egui_key_from_virtual_key` | 5719 | 入力イベント変換 |
| `log_event` | 5964 | logger 出力 |
| `copy_cpu_rgba_to_swapchain_bgra` | 5968 | D3D11 swapchain 操作 |
| `sample_cpu_rgba_pixel` | 5998 | D3D11 pixel sample (test) |
| `compare_pixel_probe` | 6034 | D3D11 pixel compare (test) |
| `channel_delta` | 6083 | D3D11 helper (test) |

## 実装手順

### Step 1: ディレクトリ化
```bash
mkdir src/video/native_presenter
git mv src/video/native_presenter.rs src/video/native_presenter/mod.rs
```

### Step 2: 新ファイル `overlay_draw.rs` を作成
- 関数群をコピー (オリジナルの mod.rs からは未削除)
- 必要な `use` 宣言を冒頭に集約
- `super::` で参照する型の解決を確認:
  - `NativeOverlay*` 系構造体: 既に `pub` なので `use super::Native*` で OK
  - `NativeTopButtonGlyph` enum: 一緒に移動するので overlay_draw 内で完結
  - `egui::*` / `egui_wgpu::*`: 通常 use

### Step 3: mod.rs に `mod overlay_draw;` 宣言を追加
- ファイル冒頭の `use` の直下に
- 移動した関数を `pub(super)` で公開 (super = native_presenter mod、つまり mod.rs から呼べる)

### Step 4: mod.rs から元の関数定義を削除
- Step 2 で overlay_draw に移動済みの関数は mod.rs から物理削除
- mod.rs 内の呼び出し点は `overlay_draw::draw_native_perf_overlay(...)` のように変える、
  あるいは mod.rs の冒頭で `use overlay_draw::*;` してそのまま呼ぶ

  どちらが好ましいか:
  - **`use overlay_draw::*;`** にすると呼び出し側のコード差分がゼロになる (= 純粋な移動)
  - **`overlay_draw::xxx(...)`** にすると依存関係が明示的になる
  - **推奨: `use overlay_draw::*;` で差分を最小化** (= レビュー容易性)

### Step 5: ビルド確認
```bash
cargo build --bin mimageviewer-core
cargo test --lib --no-fail-fast
```

### Step 6: 静的検証
```bash
# 移動した関数の参照が overlay_draw 内 + mod.rs 経由 use のみであること
grep -rn "draw_native_perf_overlay\|draw_native_jump_panel\|..." src/
# テストや bench から直接参照されていないかチェック
```

## 期待される効果

- `mod.rs`: 6179 → ~4250 行 (-31%)
- `overlay_draw.rs`: 0 → ~1900 行 (drawing が一望できる)
- ファイル分割で次の Tier 3 (NativeEguiOverlay impl 分割) の足場が整う
- レビュー時の認知負荷低減

## リスク評価

| リスク | 影響 | 緩和 |
|---|---|---|
| 移動した関数で private 型を参照していて compile error | 中 | Step 5 のビルドで即検知。もし出たら該当型を `pub(super)` 化 |
| `use overlay_draw::*;` で名前衝突 | 低 | 関数名は全て `draw_*` / `native_*` プレフィックスで一意 |
| 既存テストが変わる | 低 | 自由関数なので `mod tests` 内テストはそのまま動く |
| エディタ / IDE の "Goto definition" が壊れる | 低 | rust-analyzer がモジュール越しに追える。問題なし |

## ロールバック

不具合が出たら:
```bash
git checkout src/video/native_presenter/
git mv src/video/native_presenter/mod.rs src/video/native_presenter.rs
rmdir src/video/native_presenter
```

## Codex に確認してほしい点

1. **「impl は分割せず、自由関数のみ移動」で本当にコンパイルが通るか**
   - 自由関数が `impl NativeEguiOverlay::draw_*` のメソッドから `&mut self` の private
     field 経由で何かを呼んでいる可能性が無いか (= 関数シグネチャ的にはなさそうだが)
2. **「移動しない」リストに漏れが無いか**
   - 残すべき関数を勝手に移動していないか (= D3D11 系・入力変換系を drawing 扱いにしていないか)
3. **`pub(super)` の選定で良いか**
   - `pub(crate)` まで広げる必要があるテストや bench からの呼び出しが無いか
   - `tests/ui_snapshot.rs` 等が直接これらを叩いていないか
4. **`use overlay_draw::*;` のワイルドカード use が許容できるか**
   - 名前衝突の懸念 / clippy warning レベルが上がる可能性
5. **`NativeTopButtonGlyph` enum を一緒に移動して妥当か**
   - mod.rs 側で参照している箇所が無いか (= drawing fn の引数としてのみ使われているはず)

## 進め方

1. (この計画書を Codex に投げてレビュー)
2. Codex P1 ゼロで合意 → 実装着手
3. 実装 → cargo build / cargo test 確認
4. 実装 diff を Codex に再レビュー
5. P1 ゼロでマージ → 実機 smoke (動画再生 / VST3 / TRT 切替)
6. 完了 → Tier 1 完了。Tier 2 #3 (upscale/job.rs の options 抽出) に進む
