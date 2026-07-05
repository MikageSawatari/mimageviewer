# detached-rework Stage R0 report

Stage instruction: [detached-rework-stage-r0.md](detached-rework-stage-r0.md)  
Plan constitution: [detached-rework-plan.md §2](detached-rework-plan.md#2-%E6%86%B2%E6%B3%95-%E5%85%A8%E3%82%B9%E3%83%86%E3%83%BC%E3%82%B8%E5%85%B1%E9%80%9A%E3%81%AE%E4%B8%8D%E5%A4%89%E6%9D%A1%E4%BB%B6%E7%A6%81%E6%AD%A2%E4%BA%8B%E9%A0%85--%E6%9C%80%E9%87%8D%E8%A6%81)

## Summary

R0 の結論は次の通り。

- eframe / egui-winit 0.33.3 の public API から、`show_viewport_immediate` で生成された child viewport の native HWND を直接取得する経路は見つからなかった。
- eframe の wgpu backend 内部には `WindowId <-> ViewportId` と `ViewportId -> Arc<Window>` の対応表が存在するが、integration 内 private state であり、アプリ側へ公開されていない。
- R1 の本命候補は、提案書 §5.3 の **`EnumThreadWindows` before/after diff**。R0 ではこれを挙動不変の並走ログとして実装した。ログは `MIV_DETACHED_WINDOW_DEBUG=1` のときだけ `[detached-r0]` で出る。
- R0 プロトタイプは新方式を detached HWND の採用には使わない。既存の rect capture は従来どおり動き、R0 は旧方式との比較ログだけを追加する。

## 3.1 Public API investigation

### `egui::Context::show_viewport_immediate`

`egui-0.33.3/src/context.rs`:

- `Context::set_immediate_viewport_renderer` は backend integration が登録する callback で、immediate viewport 作成時に呼ばれる (`context.rs:3810`, `3817`)。
- `Context::show_viewport_immediate` は `ImmediateViewport { ids, builder, viewport_ui_cb }` を組み立てて renderer callback に渡す (`context.rs:3943`, `3980`)。
- `ImmediateViewport` 自体の public fields は `ids`, `builder`, `viewport_ui_cb` であり、native window handle は無い (`egui-0.33.3/src/viewport.rs:1242`)。

このため、アプリ側の `show_viewport_immediate` closure から raw-window-handle / HWND を得る public field は無い。

### `egui::ViewportInfo` / `ViewportOutput`

`egui-0.33.3/src/viewport.rs`:

- `ViewportOutput` は `parent`, `class`, `builder`, `viewport_ui_cb`, `commands` などの viewport 操作用出力を持つ (`viewport.rs:1186`, `1206`)。
- `ViewportBuilder` / `ViewportInfo` は位置・サイズ・フォーカス・表示状態などの viewport 情報を扱うが、native HWND / raw handle は公開していない。

したがって、`ctx.input(|i| i.viewport())` や `RawInput::viewports` からも native HWND は得られない。

### `eframe::Frame::window_handle()`

`eframe-0.33.3/src/epi.rs`:

- `CreationContext` は `HasWindowHandle` を実装しているが、保持している `raw_window_handle` は app creation 時の root window 用 (`epi.rs:53`, `96`)。
- `Frame` も `HasWindowHandle` を実装しているが、`Frame` に渡される raw handle は root window のもの (`epi.rs:641`, `677`)。

immediate child viewport closure には `eframe::Frame` が渡らないため、これを child viewport HWND 取得には使えない。

### eframe wgpu backend internals

`eframe-0.33.3/src/native/wgpu_integration.rs`:

- `SharedState` は `viewport_from_window: HashMap<WindowId, ViewportId>` と `viewports` を持つ (`wgpu_integration.rs:68`, `72`)。
- 内部 helper として `window(&self, window_id) -> Option<Arc<Window>>` と `window_id_from_viewport_id(&self, id)` がある (`wgpu_integration.rs:349`, `360`)。
- immediate viewport は `paint_immediate_viewport` 内で `initialize_or_update_viewport(..., ViewportClass::Immediate, ...)` を通り、window が無ければ `viewport.initialize_window(...)` される (`wgpu_integration.rs:986`, `1000`)。

この内部状態を使えば理論上 `ViewportId -> winit::Window -> raw-window-handle` へ到達できるが、public API ではない。R0 指示書の「eframe/egui の fork・パッチは選択肢に入れない」に従い、不採用。

## 3.2 EnumThreadWindows diff prototype

R0 の代替本命として、`show_viewport_immediate` の直前と直後で UI thread の top-level HWND snapshot を採取し、`S1 - S0` をログに出すプロトタイプを追加した。

### 追加したログ

`MIV_DETACHED_WINDOW_DEBUG=1` のときだけ出る。

- `[detached-r0] diff label=...`
  - `label`: `active_render`, `passive`, `keep_alive_holdover`, `keepalive_backstop`
  - `viewport`: 対象 `ViewportId`
  - `before_count` / `after_count`
  - `created_count` / `removed_count`
  - `created=[hwnd=..., class=..., title=..., rect=...]`
  - `removed=[...]`
  - `host=...`: 現在 App が保持している detached host の状態
- `[detached-r0] rect_capture_result`
  - 既存 rect capture が同フレームで返した candidate と期待 rect
  - R0 では比較用ログのみ。採用ロジックは既存のまま。

### 実装範囲

- `src/dwm_transitions.rs`
  - `debug_thread_window_snapshot(main_hwnd)` を追加。
  - HWND / visible / iconic / rect / class name / title を記録する。
  - `find_visible_thread_window_matching_rect*` には一切手を入れていない。
- `src/app.rs`
  - R0 用 snapshot / diff logging helper を追加。
  - `capture_detached_viewer_host_hwnd_from_logical_rect` に旧 rect capture 結果ログを追加。
  - 新しい App 状態フィールドは追加していない。
- `src/ui_fullscreen.rs`
  - detached immediate viewport の主要 4 経路で before/after diff を記録。
  - 新方式の HWND は挙動に使っていない。

### 実機で確認すること

このローカル環境では real HWND の同期生成タイミングは機械テストできないため、ユーザー実機ログで次を確認する。

1. detached viewer を F12 で開いた最初の `active_render` で `created_count=1` になるか。
2. `created` の class/title/rect が winit child viewport と判断できるか。
3. 旧 `rect_capture_result candidate=...` と `created` が一致するか。旧方式が passive / default geometry を拾ったとき、新方式が違う HWND を示すか。
4. passive 2 枚 + active 1 枚、F12 main↔detached 往復、Ctrl+↑↓ reopen で `created_count` が 0/1 の妥当な推移になるか。

## 3.3 Alternative notes

- **winit class name + creation order**: `debug_thread_window_snapshot` は class name / title も記録するため、diff が複数 HWND を返した場合の補助材料として使える。ただし class/order のみを primary key にするのは geometry heuristic と同様に脆い。
- **WH_CBT hook**: child viewport 作成を OS hook で直接捕捉できる可能性はあるが、hook lifetime / reentrancy / security surface が重く、R0/R1 の本命にはしない。
- **eframe fork / patch**: 内部 state からなら direct handle へ到達できるが、保守コストが高いため stage instruction に従い不採用。

## Recommendation for R1

実機ログで `show_viewport_immediate` 直後に `S1 - S0` が安定して 1 件に収束するなら、R1 は `EnumThreadWindows` diff を detached HWND の生成時確定に使う。R1 では次を守る。

- HWND は viewport 生成イベントで 1 回だけ確定し、その後は `IsWindow` 生存確認だけにする。
- rect capture は detached host 同定から撤去する。
- 差分が 0 件または複数件になるケースを state machine 上の explicit pending / ambiguous として扱い、geometry fallback へ戻さない。
- R0 の `[detached-r0]` ログは R1 実装で置き換える前提の使い捨てとする。

## User smoke log checklist

Release build 後、`MIV_DETACHED_WINDOW_DEBUG=1` を付けて次を実行する。

1. 静止画または PDF を F12 で detached にする。
2. detached 窓をリサイズ・移動・最大化する。
3. always-new / pin 等で passive 2 枚 + active 1 枚の状態を作り、active を切り替える。
4. 動画を F12 で main↔detached 往復する。
5. detached のまま Ctrl+↑↓ の folder-nav reopen を行う。

ログ確認の入口:

- `[detached-r0] diff`
- `[detached-r0] rect_capture_result`
- 既存の `[detached-viewer] captured host`

## Current status

- Public API investigation: complete.
- Prototype code: implemented as log-only instrumentation.
- Real hardware log summary: pending user smoke.
- R1 readiness: pending confirmation that `S1 - S0` is stable in the four smoke cases.
