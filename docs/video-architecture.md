# 動画再生サブシステム アーキテクチャ

mimageviewer の動画インライン再生機能の設計指針と内部構造をまとめる。
NVIDIA RTX VSR 関連の Phase 2 (DComp overlay) を撤回した後の **最終構成** を記述する。
撤回経緯は本書末尾の「Appendix: Phase 2 撤回理由」を参照。

## 設計目標

| 優先順位 | 目標 |
|---|---|
| ★★★ | 4K HEVC を **30/60fps カクつかず再生** (= zero-copy GPU 経路必須) |
| ★★★ | フォーマット網羅 (MP4/MKV/MOV/AVI/WMV/MPG/MPEG with H.264/HEVC/AV1/VP9 等) |
| ★★ | リモートデスクトップでも再生継続 (= GPU 経路が取れなければ自動 fallback) |
| ★★ | 配布 LGPL 互換 (FFmpeg LGPL shared build を `include_bytes!` で同梱、動的リンク) |
| ★ | unsafe は `gpu_renderer/` モジュール内に局所化、外部 API は safe |

**スコープ外**: NVIDIA RTX VSR / Super Resolution、HDR 表示、外部プレイヤー (この機能はあり)、
動画編集機能。

## 採用アーキテクチャ: D 経路 (zero-copy interop) + 自動 fallback

```
[起動時]
  wgpu の backend (cc.adapter.get_info().backend) を確認
  ↓
  ├─ DX12 → GpuVideoDevice 作成 → 「GPU 経路」(zero-copy)
  │       ローカル native の 99% のケース、4K@30/60fps 滑らか
  │
  └─ Vulkan/WARP/etc. → GpuVideoDevice 作成しない → 「CPU 経路」
          リモデ等の限定環境、1080p 程度なら動く、4K は重い
```

`VideoPlayer::tick(ctx)` の API は両経路で統一。経路の違いは内部にカプセル化される。

### GPU 経路の内部フロー

```
FFmpeg HW decoder (D3D11VA)
    ↓
AVFrame (format = AV_PIX_FMT_D3D11、data[0]=ID3D11Texture2D*、data[1]=subresource)
    ↓
ID3D11VideoProcessor (NV12/P010 → BGRA8/RGB10A2、bicubic)
    ↓
NT 共有 ID3D11Texture2D (BGRA8 or RGB10A2、KEYEDMUTEX 付き)
    ↓
ID3D11Fence::Signal (共有 fence で blit 完了通知)
    ↓
[ チャネル経由で UI thread へ ]
    ↓
ID3D12Device::OpenSharedHandle → ID3D12Resource (wgpu DX12 backend)
    ↓
wgpu_hal::dx12::Device::texture_from_raw → wgpu::Texture
    ↓
ID3D12CommandQueue::Wait (fence) で blit 完了を待つ
    ↓
egui_wgpu::CallbackTrait で fullscreen quad に貼って描画
```

### CPU 経路 (fallback) の内部フロー

```
FFmpeg HW decoder (D3D11VA) or SW decoder
    ↓
AVFrame
    ↓
av_hwframe_transfer_data (HW のとき、GPU→CPU、12.5MB/frame@4K)
    ↓
swscale (NV12/YUV → RGBA、CPU で 24MB allocation)
    ↓
ctx.load_texture (CPU→GPU、26-58ms@4K)
    ↓
egui::Image で描画
```

## モジュール構成 (整理後)

```
src/video/
├── mod.rs                  # VideoPlayer 公開 API (open / tick / seek / volume / loop)
├── decoder.rs              # demux + decode worker thread (HW/SW 自動切替)
├── audio.rs                # cpal WASAPI Shared 出力
├── clock.rs                # AV master clock (audio PTS 基準)
├── ffmpeg_loader.rs        # DLL extraction + LoadLibrary (一度だけ実行)
├── thumbnail.rs            # シーク先サムネイル取得 worker
└── gpu_renderer/           # ★ DX12 backend 時のみ active、unsafe を局所化
    ├── mod.rs              # 公開 API: GpuVideoDevice, D3d11Frame, VideoPipeline 等
    ├── d3d11_device.rs     # D3D11 Device + VideoProcessor + Fence (純粋な NV12→RGBA blit のみ)
    ├── ffmpeg_d3d11.rs     # FFmpeg D3D11VA hw_device_ctx 共有 (= GpuVideoDevice の D3D11 を FFmpeg に貸す)
    ├── video_paint.rs      # egui_wgpu Callback で fullscreen quad 描画
    └── wgpu_import.rs      # NT shared HANDLE → wgpu::Texture (wgpu_hal::dx12 経由)
```

### 各ファイルの責務

#### `mod.rs` (`VideoPlayer`)
- 公開 API (`open` / `tick` / `seek` / `set_volume` / `set_loop_enabled` / `shutdown`)
- decoder スレッド・audio スレッドのライフサイクル管理
- `gpu_latest: Option<D3d11Frame>` で **最新 GPU フレームを所有** (= 次フレーム到着まで HANDLE valid 保証)
- `texture: Option<TextureHandle>` で CPU 経路の最新フレーム保持
- `future_frames: VecDeque<VideoFrame>` で FIFO 連続性を保証 (= UI が pts ジャンプしない)

#### `decoder.rs`
- FFmpeg の demux + decode を別スレッドで実行
- HW (D3D11VA、`AV_PIX_FMT_D3D11`) → SW (CPU readback + swscale) に自動 fallback
- GpuVideoDevice が利用可能なら `try_gpu_blit_path` で **NT 共有テクスチャを VideoFrame::Gpu として送出**
- それ以外は CPU readback + swscale で `VideoFrame::Cpu(Vec<u8>)` を送出
- audio フレームも同じワーカーで処理 (mpsc bounded channel で UI に送出)
- pts pacing で channel 過剰生成を抑制

#### `audio.rs`
- cpal で WASAPI Shared mode の出力 stream
- ringbuffer 経由で decoder からのサンプルを取り込み
- AvClock の audio PTS anchor を更新 (= マスタークロック)
- audio 出力失敗時はクロックを wall-clock fallback に切替

#### `clock.rs` (`AvClock`)
- audio PTS を基準とした提示時刻計算 (= UI tick で `now_secs()` を取得)
- seek 時は seek_serial をインクリメント (古いフレームを drop する目印)
- post-seek の override 機構で「初フレーム到着まで時刻凍結」する

#### `gpu_renderer/d3d11_device.rs` (`GpuVideoDevice`)
- D3D11 Device + VideoDevice + VideoContext + VideoContext1 + ID3D11Fence の所有
- VPP enumerator + processor のキャッシュ (= ContentDesc が変わらない限り再利用)
- `blit_nv12_to_rgba` メソッド: AVFrame の NV12 入力を NT 共有 RGBA テクスチャに blit
  - 出力テクスチャは新規作成 (リング管理は呼び出し側)
  - 中間 RT (NT shared なし) → CopyResource で NT/KM 付き共有テクスチャに転送 (NVIDIA driver 仕様)
  - blit 完了後に fence を Signal (= UI thread の wgpu wait 用)
- 色空間 hint (`SetStreamColorSpace1` / `SetOutputColorSpace1`) は SDR/HDR PQ/HLG を明示
  (HDR は VPP がトーンマップして SDR RGB として出力)

#### `gpu_renderer/ffmpeg_d3d11.rs`
- FFmpeg の `AVHWDeviceContext` (D3D11VA) を **mIV の D3D11 Device で初期化**
- これにより HW デコード結果テクスチャと VPP が同じ D3D11 device 上にある
  (= `CopyResource` 等で device 跨ぎなく扱える)

#### `gpu_renderer/wgpu_import.rs`
- NT 共有 HANDLE を `ID3D12Device::OpenSharedHandle` で開く
- `wgpu_hal::dx12::Device::texture_from_raw` で wgpu::Texture に変換
- D3D12 Fence も `OpenSharedHandle` でオープンして command queue に Wait を積む
- Fence 世代 ID (`fence_gen`) でキャッシュ判定 (= HANDLE 値再利用への対策)

#### `gpu_renderer/video_paint.rs`
- `egui::PaintCallback` で発行される `VideoPaintCallback`
- shader: NV12 ではなく RGBA 入力 (= VPP で変換済み) を fullscreen quad に貼る
- bind group は毎フレーム再構築 (テクスチャが毎フレーム別 ID3D11Texture2D なので)

## 経路選択ロジック (起動時 1 回)

`src/main.rs` で以下を実行 (整理後も維持):

```rust
let backend = rs.adapter.get_info().backend;
let is_dx12 = matches!(backend, wgpu::Backend::Dx12);
crate::logger::log(format!(
    "wgpu backend selected: {backend:?} (gpu_video_pipeline={})",
    if is_dx12 { "available" } else { "disabled (non-DX12)" }
));
if is_dx12 {
    crate::video::gpu_renderer::init_video_pipeline(&rs);
    match crate::video::gpu_renderer::GpuVideoDevice::new() {
        Ok(dev) => app.gpu_video_device = Some(dev),
        Err(e) => crate::logger::log(format!(
            "GPU video device: failed (will fallback to CPU readback): {e}"
        )),
    }
}
```

`GpuVideoDevice::new` のシグネチャから `vsr_enabled: bool` 引数は削除 (= VSR を扱わなくなるため)。

## VideoFrame 形式

```rust
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: VideoFrameData,
    pub pts_secs: f64,
    pub seek_serial: u64,
}

pub enum VideoFrameData {
    /// CPU 経路 (旧経路)。`Vec<u8>` は width * height * 4 の RGBA8。
    Cpu(Vec<u8>),
    /// GPU 経路。NT 共有テクスチャ + fence で UI thread が直接 sample。
    #[cfg(windows)]
    Gpu(crate::video::gpu_renderer::D3d11Frame),
}
```

`Nv12Direct` variant は **削除** (Phase 2 で導入したが、その経路自体を撤回するため)。

## ライフサイクル管理

- **VideoPlayer の Drop**: `cancel.store(true)` → decoder thread が exit、`audio.take()` で cpal stream 停止
- **VideoPlayer.shutdown() の用途**: 動画切替時に Drop より早く audio を切るため (= 残音を防ぐ)
- **GpuVideoDevice の Drop**: D3D11 リソース全解放、fence の NT shared handle を `CloseHandle`
- **VideoPipeline (= app 起動時 1 回)**: アプリ終了まで生存、wgpu shader/sampler/bind group layout を保持
- **D3d11Frame の所有権**: `VideoPlayer.gpu_latest` が「現在表示中のフレーム」を所有、次フレーム到着で旧 frame の Drop が NT HANDLE を `CloseHandle` する (= UI が描画中の HANDLE が close される race を防ぐ)

## 設定との関係

整理後、削除する設定項目:
- `Settings.video_rtx_vsr` (= VSR ON/OFF トグル、撤回により不要)

維持する設定項目:
- `Settings.video_volume` (音量)
- `Settings.video_loop` (ループ再生)
- `Settings.video_resume_position` (シーク位置の永続化、ファイル単位)
- `Settings.video_hw_decode` (HW デコードを試みるかのフラグ、トラブルシュート用)

## 配布要件

- FFmpeg LGPL shared build (`avcodec`/`avformat`/`avutil`/`swscale`/`swresample` 5 DLL) を
  `include_bytes!` で exe に埋め込み、`%APPDATA%/mimageviewer/ffmpeg/` に展開
- `SetDllDirectoryW` で動的ロード
- LGPL ライセンス通知をソフトウェア情報パネルに掲載
- ライセンス本文 `vendor/ffmpeg/LICENSE.txt` をリリース成果物に同梱
- 詳細は CLAUDE.md「FFmpeg LGPL DLL 管理」節

## テスト・検証

- 通常: `cargo build --release --bin mimageviewer-core`
- ベンチ: `cargo run --release --bin bench_thumbs` (動画関係なし)
- 実機検証: 4K HEVC ファイルを動画フォルダに置いてフルスクリーン再生、滑らかさ目視
- リモデ検証: RDP 経由で起動して、`logger` の `gpu_video_pipeline=disabled (non-DX12)` を確認、CPU 経路で 1080p 動画が再生できること

---

## Appendix: Phase 2 撤回理由

### 経緯
2026-04 に「NVIDIA コンパネで RTX VSR を『アクティブ』表示にしたい」目標で Phase 2
(DComp overlay 経路) の実装を開始。`docs/dcomp-video-overlay.md` (= 撤回後 archived) に
詳細な経過を記録。Phase 2.0/2.1/2.2/2.3 まで段階実装し、各段階で Codex レビューを
受けて P1/P2/P3 を順次解消した。

### 結論
2026-04-29 の調査で以下が判明し、撤回判断:

1. **driver は `CompositionMode = COMPOSED (DWM)`** から抜け出せず、`OVERLAY` (= MPO 経路、
   VSR active の前提) に到達しなかった。`mode=COMPOSED` のまま swap chain は driver UI で
   「アクティブ」表示にならない。
2. ハードウェア (`IDXGIOutput6::CheckHardwareCompositionSupport`) は **windowed=false / fullscreen=true** を返す。
   driver は「画面全体を覆う単一の borderless top-level window」だけを MPO promotion 候補にする。
3. 我々の構造は eframe (winit) のメイン HWND + fullscreen viewport HWND + overlay HWND の **3 つの top-level**
   が共存。Codex 仮説に従い fs viewport を 1x1 縮小 + main HWND をオフスクリーン移動しても
   `mode=COMPOSED` のまま (= DWM の MPO 判定をパスできず)。
4. **Chromium / Firefox 並みの「単一 top-level HWND + DComp visual tree に video swap chain を入れる」
   architecture でないと MPO に乗らない**。これは eframe のマルチビューポート構造を捨てて
   独自 Win32 message pump + 自前 DComp tree を組む大規模変更が必要 = 画像 viewer の
   side feature の動画再生としては overspec。
5. **NVIDIA 公式は VSR を任意のアプリで使えるとは documented していない**。`SetStreamExtension(NVIDIA_VSR_GUID)`
   は Chromium 等がリバースエンジニアリングで発見した未公式拡張で、driver は process 単位で
   gating している可能性が高い (Codex 調査による)。公式の Developer 経路は **RTX Video SDK
   (Maxine VFX SDK)** だが、これは NN model + CUDA runtime 同梱で配布バイナリが数百 MB 級に肥大、
   ライセンス制約 (NVIDIA branding 表示要件等) もあり、freeware 個人配布では現実的でない。
6. `vsr_probe upscale-test` で同じプロセスから direct VPP blit + SetStreamExtension を試したところ、
   VSR ON/OFF で **完全に同じ画素 (Laplacian variance 901.68 一致)** が出力された = driver は
   process whitelist 外のアプリには VSR を実走させない (推定確実)。

### 撤回内容
- `src/video/dcomp_overlay/` 全削除
- `src/video/gpu_renderer/vsr.rs` 削除
- `src/video/gpu_renderer/frame_dump.rs` 削除 (検証用、VSR 撤回後は不要)
- `src/bin/vsr_probe.rs` 削除 (検証用 CLI)
- `d3d11_device.rs::blit_nv12_to_rgba` から VSR opt-in / `apply_nvidia_vsr_extension` 呼び出し / アップスケール target 計算削除
- `decoder.rs::try_nv12_direct_path` 削除 + `VideoFrameData::Nv12Direct` variant 削除
- App / ui_fullscreen / tray / settings から VSR 関連フィールド + 診断 env vars 削除
- `Cargo.toml` の `Win32_Graphics_DirectComposition` feature 削除

### 将来の再開条件
以下が変われば再検討する:
- NVIDIA が公式に「任意の D3D11 アプリで `SetStreamExtension` 経由 VSR を許可」と明文化
- wgpu が DComp 統合を first-class support
- mIV のメイン用途が動画 viewer に大きくシフト (= eframe マルチビューポート構造を捨てる正当性が出る)
