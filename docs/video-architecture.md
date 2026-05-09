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
ID3D11VideoProcessor (NV12/P010 → SDR BGRA8、bicubic。現状 GPU 経路のデインターレースは未実装。Auto/On が必要なフレーム/ストリームは CPU bwdif 経路へ fallback)
    ↓
NT 共有 ID3D11Texture2D (BGRA8、KEYEDMUTEX 付き)
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
libavfilter bwdif (設定が Auto/On かつ対象フレーム/ストリームの場合。Auto は frame interlaced flag と stream field_order を参照。send_frame、フレームレート維持)
    ↓
swscale (NV12/YUV → RGBA、CPU で 24MB allocation)
    ↓
ctx.load_texture (CPU→GPU、26-58ms@4K)
    ↓
egui::Image で描画
```

## モジュール構成 (v0.9.0 時点)

```
src/video/
├── mod.rs                  # VideoPlayer 公開 API + NativeVideoOutput 統合 (3445 行 ⚠ 肥大)
├── decoder.rs              # demux + 動画/音声 decode の 3-thread 実装 (4962 行 ⚠ 肥大)
├── audio.rs                # cpal WASAPI Shared 出力 + audio-pump thread + VST3 経由 (1864 行)
├── audio_stretch.rs        # Signalsmith Stretch によるピッチ維持の倍速音声処理 (172 行)
├── clock.rs                # AvClock (薄い facade、engine/ に委譲) — 詳細は下記 (905 行)
├── engine/                 # 動画再生エンジン (state machine + master clock 分割実装)
│   ├── mod.rs              # EngineEvent enum (Decoder/Audio events) (37 行)
│   ├── actor.rs            # EngineActor (state machine の source of truth) (1873 行)
│   ├── state.rs            # EngineState / DecoderEvent / AudioEvent / ReadinessLatch (357 行)
│   ├── clock.rs            # MasterClock + ClockAnchor (純粋な値オブジェクト) (292 行)
│   └── audio_bookkeeping.rs # 音声バッファ会計 (atomic、単独で unit test 可) (316 行)
├── ffmpeg_loader.rs        # DLL extraction + LoadLibrary (57 行)
├── screenshot.rs           # 現在フレームのクリップボードコピー用 one-shot RGBA 抽出 (173 行)
├── thumbnail.rs            # シーク先サムネイル取得 worker (361 行)
├── tile_thumbnails.rs      # タイルモード用一括サムネイル抽出 worker (384 行)
├── tile_thumb_cache.rs     # タイル サムネ SQLite WebP 永続キャッシュ (358 行)
├── native_window.rs        # ネイティブ Win32 message loop + 入力イベント変換 (577 行)
├── native_presenter/       # ネイティブ DComp プレゼンター + egui overlay
│   ├── mod.rs              # NativeVideoPresenter / NativeEguiOverlay impl (3900 行級)
│   └── overlay_draw.rs     # native overlay 描画・layout helper (2300 行級)
├── gpu_renderer/           # ★ DX12 backend 時のみ active、unsafe を局所化
│   ├── mod.rs              # 公開 API: GpuVideoDevice, D3d11Frame, VideoPipeline 等 (57 行)
│   ├── d3d11_device.rs     # D3D11 Device + VideoProcessor + Fence (1134 行)
│   ├── ffmpeg_d3d11.rs     # FFmpeg D3D11VA hw_device_ctx 共有 (159 行)
│   ├── video_paint.rs      # egui_wgpu Callback で fullscreen quad 描画 (557 行)
│   └── wgpu_import.rs      # NT shared HANDLE → wgpu::Texture (148 行)
├── dsp/                    # VST3 プラグインチェーン (詳細は docs/vst3-integration.md)
│   ├── mod.rs              # DspBridge 公開 API + チェーン管理 (2102 行 ⚠ 肥大)
│   ├── bridge.rs           # bridge 子プロセス管理 + IPC (1033 行)
│   ├── gui.rs              # プラグイン GUI Win32 親ウィンドウ管理 (1164 行)
│   ├── scanner.rs          # VST3 plugin スキャン (291 行)
│   └── extract.rs          # bridge exe APPDATA 展開 (30 行)
└── upscale/                # オフライン動画アップスケール (詳細は本書「オフラインアップスケール」節)
    ├── mod.rs              # 公開 API (6 行)
    ├── job.rs              # ジョブ実行 (resumable segment 化) (2551 行 ⚠ 肥大)
    ├── queue.rs            # 永続キュー (465 行)
    ├── manifest.rs         # マニフェスト (進捗 / セグメント完了状態) (408 行)
    ├── sidecar.rs          # サイドカーファイル管理 (284 行)
    ├── disk.rs             # ディスク I/O (92 行)
    └── paths.rs            # パス管理 (188 行)
```

⚠ マークは「設計ドキュメントが想定する単一責務に対して、ファイルが太りすぎているか責務が
混ざっている」ファイル。詳細は本書末尾「抽象化の現状と既知の負債」節を参照。

エンジン側のリデザイン経緯は [docs/video-engine-redesign.md](video-engine-redesign.md) を
参照。Phase 1 (skeleton) → Phase 2 (facade 化、AvClock を MasterClock + AudioBookkeeping に
分割) → Phase 3 (state machine 配線) → Phase 4 (薄い facade 化を最終形として固定) の
順で導入された。

### 各ファイルの責務

#### `mod.rs` (`VideoPlayer`)
- 公開 API (`open` / `tick` / `seek` / `set_volume` / `set_loop_enabled` / `shutdown`)
- decoder スレッド・audio スレッドのライフサイクル管理
- `gpu_latest: Option<D3d11Frame>` で **最新 GPU フレームを所有** (= 次フレーム到着まで HANDLE valid 保証)
- `texture: Option<TextureHandle>` で CPU 経路の最新フレーム保持
- `future_frames: VecDeque<VideoFrame>` で FIFO 連続性を保証 (= UI が pts ジャンプしない)

#### `decoder.rs` (3-thread 構成)

1 動画につき 3 thread を起動し、demux / video decode / audio decode を並行動作させる。
旧構造 (1 thread で demux + 全 decode) では `audio_tx` (bounded=32) または
`video_tx` (bounded=24) が満杯になると thread 全体が block して両方の経路が同時に
止まり、`buf 0/24` の周期的な振動 (= ユーザー報告の「Candyfloss_test / SilentBloom
で頻繁にバッファが空になる」現象) を引き起こしていた。これを解消するため Phase A
(audio decode 分離) → Phase B (demux 分離) で段階的にリファクタした。

| thread 名 | 責務 | 入力 | 出力 |
|---|---|---|---|
| `video-demux` (= `run_decoder`) | `Input::packets()` ループ、seek 調停、EOF idle wait、`engine_event_tx` への SeekCompleted 発火 | `Arc<AvClock>` (seek_request) / 動画ファイル | `video_pkt_tx` / `audio_pkt_tx` (各 bounded=64) |
| `video-decode` (= `run_video_decode`) | HW (`D3D11VA`) → GPU blit / SW + swscale、PACE_LEAD=0.30 の pacing、`new_seek_pending` generation race check | `video_pkt_rx` (`VideoWorkerMsg::{Packet, Flush, Eof}`) | `video_tx` (bounded=24、`VideoFrame`) |
| `video-audio-decode` (= `run_audio_decode`) | avcodec decode + swresample、post-seek packet/sample trim、PAUSED/EOF park、EOF drain | `audio_pkt_rx` (`AudioWorkerMsg::{Packet, Flush, Eof}`) | `audio_tx` (bounded=32、`AudioFrame`) |

**seek 調停**: `clock.take_seek_request()` を pull するのは demux thread のみ
(= 旧構造と同じ単一 puller)。`input.seek` 成否を判定後、両 decode thread に
`Flush { serial, target_secs: Option<f64> }` を順序保証 channel で enqueue する。
`target_secs.is_some()` (= seek 成功) なら受信側で post-seek preroll trim を実施
(動画は `drop_before_secs` + `post_seek_frame_sent=false` で 1 枚目を保護、音声は
packet/sample 段階の trim)。`target_secs.is_none()` (= seek 失敗) なら trim なしで
通常 pacing に戻す。
video packet は direct queue が満杯になると demux 側の `pending_video_packets`
overflow に退避する。seek preroll 中に audio packet send が満杯で待っている場合も、
audio の timeout 待ちごとにこの video overflow を opportunistic に drain し、
FirstFrameReady に必要な post-seek video packet が audio back-pressure の後ろに
取り残されないようにする。

**EOF**: demux thread が `input.packets()` 空を検出 → `clock.notify_eof_reached()`
+ 両 channel に `Eof` を送る。動画は内部残フレームを失っても許容なので drain なし、
音声は `avcodec_send_packet(NULL)` + receive_frame ループで残サンプルを drain
(= 末尾の数十 ms の音声を出し切る)。demux thread はその後 `peek_seek_request_pending`
の idle wait に入り、cancel か新 seek 要求まで待機。

**swresample 出力 frame の pre-allocation (⚠ 重要)**: `emit_audio_frame` は
`setup.resampler.run(input, output)` を呼ぶ前に **output frame を正しいサイズで
明示確保** する。`ffmpeg-the-third 3.0.2` の `Context::run()` 実装は `output.is_empty()`
の場合に `output.alloc(format, input.samples(), layout)` で確保するが、これは
sample-rate 変換時に出力サンプル数として誤った値 (= 入力サンプル数そのまま) を
使う上流バグ。32kHz AAC → 44.1kHz cpal 出力の場合、本来 `1024 × 44100 ÷ 32000 ≈ 1411`
samples 必要なのに 1024 しか確保されず、約 27% (= `1 - in_rate/out_rate`) のサンプルが
swr 内部 delay に取り残される。これが累積し audio 残量が想定より速く尽きて、動画
末尾が無音になる事象を引き起こす (2026-05、bipbop 32kHz AAC で再現確認)。

回避策として `emit_audio_frame` では `resample_output_buffer_samples` helper で
標準 FFmpeg パターン (av_rescale_rnd 相当):

```text
out_samples = ceil(in_samples * out_rate / in_rate) + delay_output + SAFETY
```

を計算し、`av_frame_get_buffer` 済みの frame を渡すことで `Context::run()` の
誤った alloc 経路をスキップしている。⚠️ `Delay::output` は **既に出力サンプル
単位** なのでレート換算をかけずにそのまま加算する (delay にもレート換算を
かけてしまうと downsample 96k→44.1k で過小見積もりになり swr 内部 delay 残留が
再発する。Codex P2 指摘で修正済み)。

回帰テストは `decoder_candidate_tests::` の以下 6 件で固定:
- `resample_buffer_size_{upsample,downsample,same_rate,adds_delay}_*` — formula
  単体テスト (ffmpeg 不要、純粋計算)
- `resample_run_with_preallocated_output_returns_full_output_samples_upsample`
  — 32k→44.1k 実 swr で in_samples のまま返らないことを確認
- `resample_run_downsample_no_cumulative_drift` — 96k→44.1k で 8 iteration
  回した累積出力が理論値内に収まることを確認 (delay 過小見積もり回帰検知)

`FastDownmixToStereo` path は同一レート時のみ動作するので bug の影響を受けない。

**Drop / shutdown 順**: VideoPlayer drop → `cancel.store(true)` → demux thread が
break → 関数末尾で `audio_pkt_tx` / `video_pkt_tx` を順次 drop → 各 decode thread が
channel disconnect で recv() 抜け → exit。demux thread が両 decode thread を
**audio → video** の順で `join()` する (cpal stream の bookkeeping を Drop より前に
完了させたい)。

**HW デコード fallback**: `try_init_d3d11va` 失敗 → SW デコードに fallback。`HwDevice`
は AVBufferRef の RAII ラッパーで、`unsafe impl Send for HwDevice` を付けて video
decode thread に move する (= AVBufferRef refcount は thread-safe)。SW 再試行時は
`_hw_device = None` で None 状態に置き換える。

**AV1 decoder 選択**: `hw_decode` 有効時、AV1 は既定 decoder (`libdav1d` になり得る)
の前に native `av1` decoder を HW 専用 candidate として試す。native `av1` が存在しない、
D3D11VA config を持たない、HW device 初期化や open に失敗した場合は既定 decoder に戻り、
従来通り SW decode する。H.264 / HEVC 等は既定 decoder 1 個だけを使い、既存経路を維持する。

**HW デコード診断**: open 時に stream codec id (`h264` / `hevc` / `av1` / `vp9`
等)、FFmpeg が選択した decoder 名、D3D11VA HW config の有無、実際に初期化を試みた
decode path を通常ログと perf `video/open` に記録する。左パネルの動画情報と
P キーの perf overlay にも codec / decoder / HW-SW / GPU-CPU / D3D11VA 候補を表示する。
AV1 などで `libdav1d` 等の SW decoder が選ばれているのか、H.264/HEVC 等で本来 HW 候補が
あるのに fallback しているのかを切り分けるための初期診断として使う。

**pacing 設計**: 既存の Phase 8.K 仕様 (`PACE_LEAD_SECS=0.30` / `AUDIO_SAFE_LO=0.25` /
`SEEK_BURST_LEAD_MAX_SECS=0.20` / `post_seek_frame_sent` flag / generation race
check) は **そのまま video decode thread に移植**。動作対象だけが変わる (= 旧構造の
demux+decode 同居から video decode 単独 thread に)。詳細は
[docs/video-engine-redesign.md](video-engine-redesign.md) の「Decoder pacing 規定」
節を参照。

Phase 9 分離後に追加した 9.A〜9.G + Codex P2/P? 修正 (set_audio_pts wall-rate cap、
LOADING/IDLE silence、Buffering 中 lookahead 許可、post-seek 1 枚目 unconditional、
forward seek 常時 backward+preroll、perf overlay seek freeze、seek epoch 二重 ++ 修正
等) は engine-redesign.md の「Phase 9 シリーズの追加修正」節に記述。

**PAUSED/EOF park**: 動画 decode thread だけでなく音声 decode thread も
`EngineState::{Paused,Eof}` では packet decode と `audio_tx` 送信を止める。`audio.rs`
の `fill_output` は PLAYING 以外で silence を返し processed queue を drain しないため、
音声だけが先読みを続けると `raw_pending → processed → audio_tx → audio_pkt_tx` の順に
逆圧が連鎖し、demux が audio packet 送信で停止して post-seek video packet が供給されない。
park 中も `seek_serial` 変化は即時に検知し、stale packet を捨てて `Flush` を受け取れるようにする。
さらに seek 世代が進んだときは audio pump が `audio_tx` に残った stale `AudioFrame` を
`try_recv` で一括 drain し、最初の新世代 frame だけ既存 intake 経路へ defer する。これにより
短い park 後の `Buffering` 中でも stale audio frame が `audio_tx` を塞ぎ続けない。

#### `audio.rs`
- cpal で WASAPI Shared mode の出力 stream
- ringbuffer 経由で decoder からのサンプルを取り込み
- AvClock の audio PTS anchor を更新 (内部は `engine::clock::MasterClock` 経由)
- audio 出力失敗時はクロックを wall-clock fallback に切替
- 音声バッファ ≥100ms に達したら `EngineEvent::Audio(AudioEvent::BufferReady)` を発火
  (Phase 8.K で 500ms から下げた、典型的 audio_buf hover 帯に合わせた)
- 再生速度が 1.0x 以外の場合は、VST3 plugin chain の前段で
  `audio_stretch.rs` の Signalsmith Stretch wrapper を通し、pitch を維持したまま
  output/wall 秒の音声へ変換する。`ProcessedChunk::source_secs_per_output_sec` で
  「出力 1 秒が source timeline 何秒ぶんか」を保持し、`fill_output` はこの値で
  audio PTS を進める。
- VST3 plugin chain 統合 (v0.9.0+): `audio-pump` thread が `audio_rx` から受領した
  AudioFrame を必要なら Signalsmith Stretch で time-stretch した後、
  `DspBridge::process_block` 経由で bridge プロセスに送り、戻ってきた処理済みサンプルを
  ring buffer に push する (= IPC roundtrip ~1-2ms、AudioBuffer processed queue 100ms
  で吸収)
- 動画音量は 0〜150% の手動調整。100% 超の分は `audio-pump` で safety limiter の前に
  preamp gain として掛け、`fill_output` 側の RT 音量は最大 100% に抑える。これにより
  100% 以下の音量変更は従来通り低レイテンシで、boost 時だけ limiter の 5ms lookahead を
  PDC latency として扱う。
- 現在フレームのクリップボードコピーは `screenshot.rs` の one-shot worker で別 FFmpeg
  input を開き、最後に表示済みの source pts 近傍をフル解像度 RGBA に変換してから
  既存の CF_DIB clipboard helper へ渡す。メイン decode queue / native presenter の GPU
  surface には触れないため、D3D11VA / CPU fallback / native DComp 経路で同じ操作にできる。
- 前/次フレーム送りは `VideoPlayer::step_frame()` が `avg_fps` から 1 frame 秒を求め、
  precise seek + pause を発行する。連続入力中は「最後に表示されたフレーム」ではなく
  「最後に発行した frame-step target」を基準にして target を積み、seek 完了前の
  連打 / 長押しでも同じ位置へ再 seek しない。ただし長押し repeat は、発行時点の
  `displayed_frame_seq` から新しいフレームが 1 枚表示されるまで次 target を出さない。
  これにより clock target だけが進んで画面が追いつかない状態を避ける。戻り方向は
  preroll trim が現在フレームへ吸われないよう、1 frame + 最大 4ms 手前を seek target にする。
  `frame_step_active` は通常 pause と UI を分離するための共有フラグで、frame-step pause 中は
  中央の resume controls を出さない。さらに frame-step pause は音声 callback が drain されないため、
  最初の表示フレームで `set_paused_position()` + `clear_seek_target_override()` を実行し、
  seek 中扱いが残って後続フレームを強制表示し続けることを防ぐ。上部ボタン長押しは
  UI/overlay 側の 100ms repeat state だけで実現し、decoder 側には通常の seek として流す。
- 動画ブックマークの任意名称は `video_bookmarks.title` に保存する。左ジャンプパネルの
  ✏ 操作だけが名称を更新し、追加時は従来通り title=NULL のままにする。native DComp
  overlay 側は `WM_CHAR` から egui `Event::Text` を渡すだけでなく、`WM_IME_*` を
  egui `Event::Ime` に変換し、`PlatformOutput::ime` のカーソル矩形を IMM32 の
  composition / candidate window 位置へ返す。これにより独立 overlay 上の TextEdit でも
  日本語 IME の変換文字列・候補が入力位置に追従し、保存時だけ UI thread の DB 更新イベントへ戻す。
- 動画メタデータパネルの記号・絵文字・数学英字 fallback は通常 UI と同じ
  `ui_fonts::configure_fonts()` で登録する `miv-user-text` family を使う。通常 UI の
  proportional family は既存幅を保つため Windows fallback を egui 既定 font の後ろに置き、
  ユーザー由来の長文だけ Meiryo text symbols / Cambria Math / Segoe UI Emoji /
  Segoe UI Historic / Segoe UI Symbol を優先する。絵文字の縦位置は ttf-parser で Yu Gothic の日本語 glyph と
  Segoe UI Emoji の代表 glyph の中心を読み、egui の `FontTweak` に入れる補正量を
  起動時に計算してベースラインずれを抑える。egui 0.33 は `ab_glyph` の outline
  描画を使うため、計測も outline bbox を優先し、raster image bounds は bbox が
  取れない場合の fallback にする。サンプルの外れ値は中央値で抑える。`✉` / `⋈`
  のような text-presentation 記号は Cambria Math や Segoe UI Emoji より前の
  Meiryo fallback で拾わせ、数学英字は Cambria Math、色付き絵文字は
  Segoe UI Emoji へ回す。Cambria Math の数学英字も代表 glyph の中心から
  `FontTweak` 補正を導出し、`…` など主フォント側の句読点と極端に上下ずれしないようにする。
- `fill_output` の bookkeeping (Phase 9 後の cleanup refactor):
  - **実消費サンプル数ベース**: `pop_front` で取り出した分 (= `real_consumed`) のみ
    `next_pts_secs` を進める。silence 出力中は pts 進行 0 (= 旧版の「常に full want
    分進める」バグを修正、上流で正確化)。
  - 早期 return: `pump_seek_serial < clock_serial` (= pre-seek サンプル全消去) と
    `engine_state != PLAYING` (= silence + processed 非 drain)、および `!clock.is_playing()`
    のみ。非 PLAYING 中の逆圧連鎖は decoder 側の audio park で上流から抑制する。
    詳細は [docs/video-engine-redesign.md] の「Phase 9 後の Post-cleanup refactor」節。

#### `clock.rs` (`AvClock` — 薄い facade)
- 公開 API は変更しないまま内部実装を `engine/` に委譲する **薄い facade**。
- 委譲先:
  - 時刻計算 (`now_secs` / `set_audio_pts` / `set_fallback_anchor` / `notify_seek_completed` の anchor 部分) → `engine::clock::MasterClock`
  - 音声バッファ会計 (`set_audio_pump_buf_secs` / `add_audio_tx_queued_secs` / `total_audio_buffer_secs`) → `engine::audio_bookkeeping::AudioBookkeeping`
- AvClock 自身が保持する状態:
  - **`seek_serial: Arc<AtomicU64>`** (counter consolidation 後): `EngineActor` と
    **同一インスタンスを共有**。`AvClock::request_seek` で fetch_add(1)、
    `EngineActor::handle_seek_request` は adaptive ロジックで「外部 bump 検知時は
    state 更新のみ」「内部 bump 必要時は av_clock.request_seek 経由で publish」を
    自動判別。詳細は [docs/video-engine-redesign.md] の「counter consolidation」節。
  - **再生制御の互換複製** (`playing` / `audio_active` / `eof_reached` / `seek_request` / `seek_target_override`): `EngineActor` の `published_state` (`Arc<AtomicU8>`) と並列管理されている **複製**。新規コードはこれらを AvClock からは読まず、EngineActor 経由で取得すること (source of truth は EngineActor)。
  - **AvClock 単独で保持しているレガシー所有状態** (`volume` / `muted`): TransportCommand::SetVolume / SetMuted は EngineActor 側では no-op で、現状 `audio.rs` が `clock.output_volume()` / `clock.pre_limiter_gain()` を直接読んでいる。これらは将来的に `EngineActor` (もしくは独立の `VolumeController`) に移すべきだが、Phase 4 時点では AvClock が source of truth のまま。
- `playback_speed` は AvClock と EngineActor の anchor speed に伝搬し、`now_secs()` は
  source timeline を `speed` 倍で進める。速度変更時は現在 PTS で anchor を張り直し、
  `audio_tx_accounting_epoch` を進めて旧速度で enqueue 済みの tx 会計を無効化する。
  epoch は偶数を安定状態、奇数を速度変更中として使い、decoder の enqueue 会計 snapshot は
  安定状態だけを採用する。
- `set_audio_pts` の wall-rate cap: defensive safety net として保持。bookkeeping は
  上流 (`fill_output`) で `source_secs_per_output_sec` により正確化済だが、buffer 非空での
  pre-fill burst (= callback 連続 pop が wall 進行を超える) シナリオへの保険として
  `wall_dt * playback_speed` を基準に頭打ちにする。0.5x など低速時は callback jitter で
  過剰発火しないよう、speed<1.0 の cap だけ少し広めに取る。
- ⚠️ **新規コードからは AvClock を直接呼び出さない**。新しい状態を扱う処理は必ず
  `EngineActor` 経由 (= `apply_command` / `handle_seek_request` / イベント送信) で書く。
  volume / muted を engine 側に移す改修も Phase 5+ で個別タスクとして扱う。

#### `gpu_renderer/d3d11_device.rs` (`GpuVideoDevice`)
- D3D11 Device + VideoDevice + VideoContext + VideoContext1 + ID3D11Fence の所有
- VPP enumerator + processor のキャッシュ (= ContentDesc が変わらない限り再利用)
- `blit_nv12_to_rgba` メソッド: AVFrame の NV12 入力を NT 共有 RGBA テクスチャに blit
  - 出力テクスチャは新規作成 (リング管理は呼び出し側)
  - 中間 RT (NT shared なし) → CopyResource で NT/KM 付き共有テクスチャに転送 (NVIDIA driver 仕様)
  - blit 完了後に fence を Signal (= UI thread の wgpu wait 用)
- 色空間 hint (`SetStreamColorSpace1` / `SetOutputColorSpace1`) は SDR/HDR PQ/HLG を明示
  (HDR 表示は非対応。HDR/10-bit 入力も VPP が SDR BGRA8 として出力)

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

#### `native_window.rs` (`NativeVideoWindow`)

ネイティブ Win32 メッセージループ + 入力イベント変換。フルスクリーン動画再生時に
**eframe (winit) のメインビューポートとは別の独立 HWND** を作って、DWM の合成を
迂回するために用意した薄い層。

- `CreateWindowExW` で borderless top-level window を作成、message pump を別スレッドで回す
- `WM_KEYDOWN` / `WM_LBUTTONDOWN` / `WM_MOUSEWHEEL` 等を `NativeVideoWindowEvent` enum
  に正規化して内部 channel に push (UI スレッドが受信)
- `NativeVideoMouseButton` (L/M/R/X1/X2) / `NativeVideoMouseWheelEvent` 等の型は
  egui の Event との 1:1 翻訳を意図しており、`native_presenter/mod.rs` 側で
  `egui::Event` に変換される

責務は単一 (= 単純な入力 marshalling)。設計上の懸念はなし。

#### `native_presenter/` (`NativeVideoPresenter` + `NativeEguiOverlay`)

フルスクリーン動画用の DirectComposition 経路を一手に引き受ける大型モジュール。
2026-05-09 の Tier 1 #2 で描画自由関数群を `overlay_draw.rs` に分離し、
`mod.rs` は D3D11 / DComp / egui overlay state と入力変換を担当する形に整理した。

現状の内部構成:

| ファイル / 範囲 | 責務 | 主な型 |
|---|---|---|
| `mod.rs` 前半 | 公開型定義 (overlay 状態 / イベント / コマンド) | `NativePresenterConfig`, `NativeVideoPresenter`, `NativeEguiOverlay`, 各種 `NativeOverlay*` 構造体 (15+ 個) |
| `mod.rs` 中盤 | D3D11 デバイス + swap chain + 共有テクスチャ + keyed mutex + 動画 present | `NativeVideoPresenter` |
| `mod.rs` 中盤 | 黒背景レイヤ / egui overlay state / wgpu surface 管理 / 入力変換 | `NativeBlackBackground`, `NativeEguiOverlay` |
| `overlay_draw.rs` | overlay 描画関数群、panel 矩形計算、format helper、タイムライン marker / icon 描画 | `NativeOverlay*` 値型 |
| `mod.rs` 末尾 | wgpu surface format 選択、DPI / egui key 変換、D3D11 test helper | — |

native overlay から UI thread へ戻るコマンドの App 側 dispatch は
`src/app/native_video.rs` に分離している。`VideoPlayer` / `NativeVideoOutput` は
event channel で App に通知し、App 側がシーク、ブックマーク、ピン、VST3 操作、
外部 URL open などの状態更新を行う。

ネイティブ DComp 経路を採用した理由:

- eframe の `show_viewport_immediate` で借りる winit ビューポートは DWM 合成下で
  動作するため、4K 60fps + perf overlay + 動画フレーム描画の合成が DWM の
  `vblank` バジェットを超えて hitch する事例があった
- ネイティブ HWND + DComp で「動画レイヤ」「黒背景レイヤ」「egui overlay レイヤ」を
  別々の swap chain に分離し、動画レイヤだけを高頻度 present、overlay は必要時のみ
  redraw する構造に変えることで pacing が安定した
- メタデータパネルは FFmpeg format metadata から title / artist / description /
  HTTP(S) の元動画 URL (`comment` / `PURL` / `webpage_url` 等) を受け取り、description 内 URL も
  `ui_text_links` でリンク化する。リンククリックは native overlay command として
  UI thread へ戻し、`VideoPlayer::set_playing(false)` 後に
  `opener` 経由で既定ブラウザを起動する。URL は `external_links` で HTTP(S) のみに制限する。
- 経緯と設計判断は [docs/dcomp-native-presenter-integration-plan.md](dcomp-native-presenter-integration-plan.md)
  に詳細あり (Phase A〜D の段階的移行)

#### `tile_thumbnails.rs` / `tile_thumb_cache.rs`

フルスクリーン中の **タイルモード** (`S` キー / ホバーバー ▦ ボタン) で使う、
動画から複数フレームを一括抽出して並べる仕組み。

- `tile_thumbnails.rs`: 一括サムネイル抽出 worker。指定動画から N 個の絶対 PTS で
  フレーム取得 (FFmpeg seek 系統は `screenshot.rs` と同じ one-shot 方式)
- `tile_thumb_cache.rs`: SQLite + WebP の永続キャッシュ。**絶対 PTS をキー**にしているため
  動画の長さが変わっても再ヒットする (Phase 8.C の修正)

タイルモードの UI 描画は `native_presenter/overlay_draw.rs` の `draw_native_tile_overlay` と
`ui_video_tile.rs` (eframe 経路の旧実装) の二重実装になっている (= ネイティブ DComp
経路と eframe 経路の両方で動かすため)。

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
- `Settings.video_volume` (音量。既定 1.0、手動 boost 上限 1.5)
- `Settings.video_loop` (ループ再生)
- `Settings.video_resume_position` (シーク位置の永続化、ファイル単位)
- `Settings.video_hw_decode` (HW デコードを試みるかのフラグ、トラブルシュート用)
- `Settings.video_deinterlace` (Off / Auto / On。CPU 経路で FFmpeg `bwdif=mode=send_frame` を適用。Auto は frame interlaced flag と stream field_order を参照)

## 配布要件

- FFmpeg LGPL shared build (`avcodec`/`avformat`/`avutil`/`avfilter`/`swscale`/`swresample`) を
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

## 抽象化の現状と既知の負債

v0.9.0 リリース直前 (2026-05-08) に行ったアーキテクチャ レビューの所見を残す。
**設計レイヤ自体は妥当だが、実装ファイルが太りすぎている**箇所が複数ある。

### レイヤ階層自体の評価

| レイヤ | 状態 | 評価 |
|---|---|---|
| `engine/` (state machine + master clock + audio bookkeeping) | ✅ 良好 | Phase 1〜9 の段階的リファクタで責務が綺麗に分離されている。`actor.rs` は state machine の中核として 1873 行あるが、`apply_command` / `handle_decoder_event` / `handle_audio_event` の 3 つに大別され、unit test 9 件が通っている |
| `gpu_renderer/` | ✅ 良好 | `unsafe` を局所化する目的で 5 ファイルに分割され、各ファイルの責務が単一 (D3D11 device / FFmpeg interop / wgpu import / paint) |
| `clock.rs` (`AvClock` facade) | ⚠️ 計画的負債 | 設計上は engine に委譲する薄い facade だが、905 行と肥大。理由は legacy 互換のため `volume` / `muted` / `seek_serial` 等を所有したまま (= EngineActor への完全移行が Phase 5+ 以降に持ち越し)。**新規コードは AvClock を直接呼ばずに EngineActor 経由で書くこと** |
| `native_window.rs` | ✅ 良好 | 単一責務 (Win32 → enum 変換) で 577 行。問題なし |

### ファイル規模の負債

以下のファイル / モジュールはまだ責務が混ざって肥大しており、Phase 10 以降の
リファクタ対象。`native_presenter/` は Tier 1 #2 で描画関数だけ分離済みだが、
残りの core / overlay state 分割は中期課題として扱う。新機能を入れる時に
「ついでに分けられないか」 を検討する、という運用にする。

#### `native_presenter/` — Tier 1 #2 で描画関数を分離済み、残りは中期負債

DirectComposition プレゼンター本体と egui overlay 本体は `mod.rs` に残し、egui の
描画自由関数群は `overlay_draw.rs` に移動済み。今後さらに分けるなら次の粒度が自然:

```
native_presenter/
├── (型定義 ~450 行)        → 現状維持
├── (D3D11 + present ~970 行)→ native_presenter/core.rs (推奨残し)
├── (NativeBlackBackground ~120) → core.rs に同居でよい
├── (NativeEguiOverlay ~1610)→ native_presenter/overlay.rs
├── overlay_draw.rs (描画関数群、現状)
│   ├── perf overlay        → native_presenter/overlay/perf.rs
│   ├── jump panel          → native_presenter/overlay/jump.rs
│   ├── top bar             → native_presenter/overlay/top_bar.rs
│   ├── VST3 panel          → native_presenter/overlay/vst3.rs
│   ├── metadata panel      → native_presenter/overlay/metadata.rs
│   ├── tile overlay        → native_presenter/overlay/tile.rs
│   └── center status / icons → native_presenter/overlay/icons.rs
└── (helper ~1000 行)        → native_presenter/util.rs
```

なぜ元々 1 ファイルだったか: ネイティブプレゼンター実装は短期間で
Phase A〜D を回しながら追加機能 (perf overlay → bookmark 編集 → VST3 panel → tile
mode) を織り込んできたため、機能ごとの drawing fn を追加する場所として
`native_presenter.rs` 末尾が選ばれ続けた。Tier 1 #2 では impl block を割らず、
自由関数だけを移動した。

#### `decoder.rs` (4962 行) — demux + video + audio + HW + probe の同居

3-thread 構成は設計通り (= demux / video decode / audio decode の thread 分離) だが、
それぞれの thread の `run_*` 関数 + その helper 群が 1 ファイルに同居している。
自然な分割:

```
decoder.rs (4962)
├── decoder/mod.rs          # 公開型 (VideoFrame / AudioFrame / VideoInfo) + spawn
├── decoder/demux.rs        # run_decoder + packet send 系 helper (~1100)
├── decoder/video.rs        # run_video_decode + GPU blit path (~1300)
├── decoder/audio.rs        # run_audio_decode + downmix + layout 正規化 (~1100)
├── decoder/hw.rs           # HwDevice / try_init_d3d11va / probe_d3d11va (~600)
└── decoder/codec.rs        # codec 候補解決 / open_video_decoder_with_candidates (~400)
```

GPU blit path (`try_gpu_blit_path` 等) は HW device と一緒に `hw.rs` に入れても良い。

#### `mod.rs` (3445 行) — VideoPlayer + NativeVideoOutput の同居

`VideoPlayer` impl が 1400 行あり、その中の `tick()` メソッドが特に長い (デコーダフレーム
ポーリング / engine event dispatch / audio buffer 会計 / native presenter 呼び出し /
UI texture アップロードがすべて入っている)。さらに同じファイルに `NativeVideoOutput` の
入出力 channel 管理 (~200 行) が同居している。

自然な分割:

```
mod.rs (3445)
├── mod.rs                  # VideoPlayer struct + 公開 API + Drop (~700 行残し)
├── tick.rs                 # VideoPlayer::tick + sub-routines (~1700 行)
└── native_output.rs        # NativeVideoOutput / NativeVideoOutputCommand 系 (~500 行)
```

#### `audio.rs` (1864 行) — pump + cpal callback + VST bridge の同居

`audio-pump` thread (decoder からの AudioFrame 受信 → time stretch → VST3 IPC → ring
buffer push) と cpal RT callback (`fill_output`) と SafetyLimiter が同じファイル。
3 つのスレッドが ring buffer (`AudioBuffer`) を介して連携するため、buffer の所有を
中心に分けるのが自然:

```
audio.rs (1864)
├── audio/mod.rs            # AudioOutput 公開 API + AudioBuffer (~400)
├── audio/pump.rs           # audio-pump thread + VST3 結線 + time stretch (~700)
├── audio/callback.rs       # fill_output + cpal stream + SafetyLimiter (~500)
└── audio/device.rs         # cpal device 列挙 + warmup (~250)
```

#### `dsp/mod.rs` (2102 行) と `upscale/job.rs` (2551 行)

VST3 と offline upscale。これらの分割方針は別ドキュメント
([docs/vst3-integration.md](vst3-integration.md), 後述の offline upscale design) で
扱う。

### 抽象化の境界として正しい分け方になっているか

**結論: 大きな線引きは正しい**。

- `engine/` ↔ `decoder.rs` ↔ `audio.rs` ↔ `gpu_renderer/` ↔ `native_presenter/` の
  境界は妥当。各層が他の層に対して「event channel + Arc<X> 共有」という最小 API で
  接続されており、内部を入れ替えやすい (実際 native presenter は eframe ビューポート版
  と切り替え可能になっている)
- `engine/` 内部の `MasterClock` / `EngineActor` / `AudioBookkeeping` の 3 分割は
  Codex レビューで明示的に推奨された分割で、適切なグラニュラリティ
- `gpu_renderer/` は unsafe 境界としても綺麗 (4 つのモジュールにまたがる D3D11 / D3D12
  / wgpu interop が型レベルで境界を持っている)

**問題なのは「層の中の責務」が太っていること**で、層をまたいだ抽象化リークではない。
将来の Phase 10+ で機械的にファイル分割すれば解消できる範囲の負債。

### Codex レビューを定期的に取る運用について

video サブシステムは Codex P1〜P3 反映を多数行ってきた経緯がある (=
[docs/video-engine-redesign.md](video-engine-redesign.md) の Phase 9.A〜9.G、Phase 9
後の counter consolidation 等)。今後も `cargo build` と単体テストが通っただけで
「設計上正しい」とは限らないので、新機能や挙動変更を入れたときは

```bash
codex exec --sandbox read-only -o /tmp/codex-video.txt \
  "Review video subsystem changes since <baseline>. Focus on engine state machine
   invariants, decoder pacing, audio buffer accounting, native presenter z-order /
   keyed mutex / fence ordering. Return findings ordered by severity." < /dev/null
```

の形で第二意見を取ることを推奨する (CLAUDE.md「Codex CLI レビュー」節)。

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
