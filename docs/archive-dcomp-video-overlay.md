# [ARCHIVED] DComp video overlay 経路 実装計画 (Plan B)

> **このドキュメントは歴史資料 (2026-04-29 撤回済) です。**
> 現行の動画再生サブシステム設計は [video-architecture.md](video-architecture.md) を参照してください。
> 本書に登場する `src/video/dcomp_overlay/`, `src/video/gpu_renderer/vsr.rs`,
> `src/video/gpu_renderer/frame_dump.rs`, `src/bin/vsr_probe.rs` および
> `Settings.video_rtx_vsr` は **すべて削除済み**で、actionable な計画ではありません。
> 撤回理由は [video-architecture.md の Appendix](video-architecture.md#appendix-phase-2-撤回理由) に要約してあります。

---

NVIDIA RTX VSR を **driver UI で「アクティブ」と認識させる** + **動画再生のカクつきを解消**するために、Chrome 同等の DirectComposition + DXGI YUV swap chain 経路を新規実装する。既存の wgpu サンプリング経路は構造的限界に達しているので置換する。

## 経緯と背景 (2026-04-29 時点)

### 既存の wgpu サンプリング経路で達成できたこと
- 動画インライン再生は動作する (黒画面解消、blit success 100%)
- D3D11Fence ↔ D3D12Fence 共有同期が機能 (黒画面の D3D11→D3D12 sync race を解決)
- KEYEDMUTEX 必須 (NVIDIA driver 仕様、`vsr_probe flags-probe` で確定)
- color space hints (`VideoProcessorSetStreamColorSpace1` / `SetOutputColorSpace1`) を毎 Blt で reassert
- VSR opt-in `VideoProcessorSetStreamExtension(NVIDIA_VSR_GUID, {1,2,1})` は driver から `S_OK` 受理
- `VideoFrameCache` で 100% cache hit 達成、prepare 0.01ms

### しかし達成できなかったこと
- **NVIDIA コンパネが「非アクティブ」のまま** — driver UI の active 判定は overlay/DComp present 経路のみ反映する仕様 (Codex 確認、Chromium ソース参照)
- **カクつき体感** — 構造由来 (UI 60-120Hz vsync で sample shader 経由、video frame rate hint なし、judder cadence)

### Chrome の経路 (Codex 調査)
Chromium は `ui/gl/swap_chain_presenter.cc` + `ui/gl/dc_layer_tree.cc` で実装。要点:
- `IDCompositionDevice` の visual tree に YUV NV12 swap chain を直接置く
- `IDXGIFactoryMedia::CreateSwapChainForCompositionSurfaceHandle` で `DXGI_SWAP_CHAIN_FLAG_YUV_VIDEO | FULLSCREEN_VIDEO` 付きの video swap chain を作成
- VPP 出力 view を `swap_chain->GetBuffer(0)` で直接作って `VideoProcessorBlt` → `swap_chain->Present(1, DXGI_PRESENT_USE_DURATION)`
- VSR opt-in (`SetStreamExtension`) は **この overlay swap chain 経路の中で** 呼ぶ — 同じ呼び出しでも driver 側の認識が変わる
- pacing は waitable swap chain + `SetMaximumFrameLatency(1 or 2)` + `IDXGISwapChainMedia::SetPresentDuration`

## アーキテクチャ方針

**フルスクリーン動画再生時のみ** 別 HWND を立ち上げる方式 (Plan B)。

```
[ 通常 UI ]
  Main HWND (winit + wgpu D3D12, 通常通り)
  └─ egui がグリッド・ダイアログ等を描く
  
[ フルスクリーン動画時 ]
  Main HWND は隠す or 残す
  Video overlay HWND (WS_POPUP, borderless, owned by main HWND)
  └─ IDCompositionTarget
      └─ root visual
          └─ swap chain (NV12, YUV_VIDEO|FULLSCREEN_VIDEO)
              ← VideoProcessorBlt 直接ここに出力
              ← Present で表示
  HUD 用第三 HWND (オプション、後フェーズ)
    WS_POPUP + WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOREDIRECTIONBITMAP
    └─ egui_wgpu で透明背景 HUD 描画 (alpha 透過)
```

### Window styles
- 動画 HWND: `WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN`、`WS_EX_NOREDIRECTIONBITMAP`、optional `WS_EX_NOACTIVATE`
- 動画 HWND に `WS_EX_LAYERED` は **付けない** (overlay/VSR 取得を阻害)
- HUD HWND は最後に追加 (Phase 2.2)、初期実装では HUD なし

## 段階的実装ステップ

### Phase 2.0: モジュール骨格
- `src/video/dcomp_overlay/mod.rs` 公開 API
- `window.rs`: borderless overlay HWND の create/destroy
- `compositor.rs`: `IDCompositionDevice` + `IDCompositionTarget` + visual tree
- `swap_chain.rs`: YUV NV12 swap chain 作成
- `presenter.rs`: フレーム描画 + Present のメインループ
- 既存の `src/video/gpu_renderer/` は **当面残す** (env var で切替)
- env var: `MIV_VIDEO_DCOMP=1` で新経路を有効化

### Phase 2.1: 赤い矩形を表示する POC
Codex 推奨の最小スキャフォールド (BGRA で先に動かす):
1. owned borderless HWND 作成 (WS_POPUP + WS_EX_NOREDIRECTIONBITMAP)
2. `ID3D11Device` を Cast → `IDXGIDevice`
3. `DCompositionCreateDevice(dxgi_device, ...)` → `IDCompositionDevice`
4. `dcomp.CreateTargetForHwnd(hwnd, TRUE, &target)`
5. `dcomp.CreateVisual(&root)` + `target.SetRoot(root)`
6. **BGRA** swap chain を `CreateSwapChainForComposition` で作成 (NV12 はその次の段)
7. `root.SetContent(swap_chain)` + `dcomp.Commit()`
8. `swap_chain.GetBuffer(0)` → RTV 作って ClearRenderTargetView で赤
9. `swap_chain.Present(1, 0)`
10. → 全画面に赤い矩形が出れば DComp パイプライン疎通 OK

### Phase 2.2: NV12 swap chain + VPP 直接出力
1. swap chain を `IDXGIFactoryMedia::CreateSwapChainForCompositionSurfaceHandle` + `DXGI_FORMAT_NV12` + `YUV_VIDEO|FULLSCREEN_VIDEO` flags に置換
2. `DCompositionCreateSurfaceHandle(COMPOSITIONOBJECT_ALL_ACCESS, nullptr, &handle)` で composition surface handle を取る
3. `swap_chain.GetBuffer(0)` → ID3D11Texture2D
4. `CreateVideoProcessorOutputView(back_buffer, ...)` で VPP output view を直接 back buffer に
5. `VideoProcessorSetStreamColorSpace1` / `SetOutputColorSpace1` を毎 Blt
6. `VideoProcessorSetStreamExtension(NVIDIA_VSR_GUID, ...)` を毎 Blt
7. `VideoProcessorBlt(...)` で NV12 入力 → NV12 swap chain back buffer
8. `swap_chain.Present(1, 0)` (まずは plain Present)
9. → コンパネで VSR アクティブ表示 + 動画が滑らかに表示されたら成功

### Phase 2.3: pacing 最適化
- waitable swap chain (`DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT`)
- `IDXGISwapChain3::SetMaximumFrameLatency(1 or 2)`
- `IDXGISwapChainMedia::SetPresentDuration(actual_video_duration_ns)` ← Chromium #1647-1653
- `Present(1, DXGI_PRESENT_USE_DURATION)` ← Chromium #1084-1097

### Phase 2.4: ライフサイクル + フルスクリーン入退場
- 動画フルスクリーン entry: overlay HWND を作って Show
- exit: VPP submission を止め、D3D11Fence で GPU idle を待ち、visual content をクリア → Commit → swap chain 解放 → HWND DestroyWindow
- 多モニタ: モニター変更時は swap chain を作り直し (adapter/output 違い)
- DPI: 物理ピクセルで size。`WM_DPICHANGED` / `WM_SIZE` 監視
- フォーカス: Main HWND がフォーカス失った時の挙動 (overlay 自動で隠す等)

### Phase 2.5: HUD 統合 (オプション)
- 第三 HWND: `WS_POPUP | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOREDIRECTIONBITMAP`
- 動画上に重ねる、透明背景で egui を描画
- alpha HUD 表示中は overlay/VSR が driver でデモートされる可能性あり、表示時のみ表示する短時間運用にする

### Phase 2.6: 旧 wgpu サンプリング経路の撤去
- env var なしでも DComp が default になるよう
- `src/video/gpu_renderer/` の以下を削除:
  - `video_paint.rs` (egui_wgpu Callback)
  - `wgpu_import.rs` (D3D12 OpenSharedHandle 関連)
  - `D3d11Frame.fence_value/fence_shared_handle/fence_gen` 等のフィールド
  - `GpuVideoDevice.fence/fence_shared_handle/next_fence_value/fence_gen`
  - `create_intermediate_rt` / `create_shared_output` (= 2 段経路)
  - `create_vpp_shared_output` (= 試行錯誤の残骸)
- `D3d11Frame` 自体を撤去または再設計 (DComp swap chain back buffer 経由なら不要)

## 重要な driver / API の地雷ポイント (Codex 助言)

1. **`DCompositionCreateDevice` は `IDXGIDevice` を要求** (`ID3D11Device` ではない、`.cast::<IDXGIDevice>()` 経由)
2. **YUV swap chain は `CreateSwapChainForComposition` 単独では不可**、`IDXGIFactoryMedia::CreateSwapChainForCompositionSurfaceHandle` を使う
3. **`DCompositionCreateSurfaceHandle`** で取れた raw HANDLE が swap chain の親
4. **swap chain format**: SDR は NV12 / HDR は P010。BGRA は VSR 対象外
5. **Window styles**: 動画 HWND に `WS_EX_LAYERED` 付けると overlay 取得失敗
6. **fullscreen exclusive 不可**、borderless + flip + DComp の組合せのみ
7. **teardown 順序**: 描画 stop → fence で GPU idle 確認 → visual content clear + Commit → swap chain Release → HWND Destroy
8. **multi-monitor**: surface handle 自体は monitor 紐付けないが、HDR/colorspace/DPI/refresh 等は実質モニタ依存。adapter or output 変更時は swap chain 再作成

## 既存コードベースの再利用ポイント

- `src/video/gpu_renderer/d3d11_device.rs::GpuVideoDevice::new` の D3D11 device + video_device + video_context + video_context1 + Fence 作成は **そのまま再利用** (FFmpeg HW decoder と device 共有)
- `vsr.rs::apply_nvidia_vsr_extension` は SetStreamExtension 呼び出しを集約しているのでそのまま使える
- `d3d11_device.rs::ensure_processor` (= VPP enumerator + processor キャッシュ) もそのまま (ただし ContentDesc の OutputWidth/Height を swap chain back buffer サイズに合わせる)
- VPP の SourceRect/DestRect/SetStreamColorSpace1/SetOutputColorSpace1 ロジックも流用
- `frame_dump.rs` は overlay 経路でも使える (back buffer staging copy → PNG)

## 検証手順

`vsr_probe` バイナリは活用できる:
- `vsr_probe flags-probe`: driver の flag 受容性確認 (済)
- `vsr_probe device-info`: GPU/feature level 確認 (済)
- `vsr_probe frame-drop-bench`: blit per-frame latency 測定 (済、mean 1.22ms / p99 10.26ms)

新規追加が望ましい:
- `vsr_probe dcomp-poc`: モジュール完成後、合成 NV12 を DComp + Present で表示する独立テスト

実機検証:
1. **赤い矩形 POC**: `MIV_VIDEO_DCOMP=1` 起動、フルスクリーン動画開いて赤い矩形が画面全面に出れば DComp パイプライン正常
2. **NV12 + VPP**: 動画再生に切替、画が出ればパイプライン疎通
3. **VSR active**: NVIDIA コンパネ「アクティブ」表示
4. **滑らかさ**: judder/カクつき消失
5. **frame_dump で sharpness 計測**: VSR ON / OFF 切り替えで Laplacian variance に差が出る

## ログ・診断項目 (実装時に組み込む)

- DComp/swap chain 作成成否 + 各 HRESULT
- swap chain format/size/flags の起動時 1 行ログ
- Present 100 frame ごとの latency / GetFrameStatistics
- VSR `SetStreamExtension` hr の transition log (既存 `vsr.rs` の仕組みを流用)
- teardown 順序の各ステップ ログ (異常時の追跡用)

## 追加ファイル参照 (Chromium 実装、Codex 調査済み)

| 機能 | URL |
|---|---|
| DComp target 作成 | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/dc_layer_tree.cc#281 |
| video swap chain visual integration | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/dc_layer_tree.cc#1155 |
| YUV swap chain 作成 | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/swap_chain_presenter.cc#1519 |
| swap chain flags | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/swap_chain_presenter.cc#1523 |
| VPP Blt + GetBuffer | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/swap_chain_presenter.cc#1380 |
| NVIDIA VSR opt-in | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/swap_chain_presenter.cc#179 |
| Present + duration | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/swap_chain_presenter.cc#1084 |
| SetMaximumFrameLatency | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/swap_chain_presenter.cc#1610 |
| SetPresentDuration | https://chromium.googlesource.com/chromium/src/+/master/ui/gl/swap_chain_presenter.cc#1647 |
| 子 HWND スタイル例 | https://chromium.googlesource.com/chromium/src/+/refs/heads/main/ui/gl/child_window_win.cc#92 |

## 次セッション開始時の最初の指示文 (テンプレ)

> このプロジェクトの動画再生で NVIDIA VSR を Chrome 並みに動作させたい。これまでの調査結果と実装計画を [docs/dcomp-video-overlay.md](docs/dcomp-video-overlay.md) にまとめてある。Phase 2.0 (モジュール骨格) と Phase 2.1 (赤い矩形 POC) から始めて、`MIV_VIDEO_DCOMP=1` で env var gating する形で実装してほしい。既存 `src/video/gpu_renderer/` は当面残す。
