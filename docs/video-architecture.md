# 動画再生サブシステム アーキテクチャ

mimageviewer の動画インライン再生機能の設計指針と内部構造をまとめる。
NVIDIA RTX VSR 関連の Phase 2 (DComp overlay) を撤回した後の **最終構成** を記述する。
撤回経緯は本書末尾の「Appendix: Phase 2 撤回理由」を参照。

> ⚠️ **動画 HUD UI は `src/video/native_presenter/{mod.rs,overlay_draw.rs}` で描画される**。
> `src/ui_fullscreen.rs` の動画関連コードは error / loading 表示と shortcut 経路のみ active で、
> HUD 描画コードは旧版の残骸 (v0.9.0 で native presenter に移行)。新規 UI 機能を追加する際は
> `native_presenter` 側に書くこと。詳細は本書「採用アーキテクチャ」節と「ファイル責務」節を参照。

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

## 採用アーキテクチャ: native presenter (独立 HWND + D3D11 swap chain) 必須

旧版では「DX12 wgpu backend なら egui_wgpu callback で zero-copy GPU 描画、それ以外
なら CPU readback + `ctx.load_texture` で egui::Image 描画」の二経路 + 自動フォール
バック構成だったが、v0.9 系で **native presenter** (`src/video/native_presenter`、
独立 Win32 HWND + 自前 D3D11 swap chain + DirectComposition) に統一済み。
動画再生は **常に native presenter 経路を必須**とする (旧 egui 描画パスと
`MIV_NATIVE_VIDEO_PRESENTER` フォールバック環境変数は削除済み)。

```
[起動時]
  GpuVideoDevice 作成 (mIV 専用の D3D11 device + VideoProcessor + Fence)
    ├─ 成功: HW decoder (D3D11VA) + GPU blit が使える
    └─ 失敗: 動画は SW decode + CPU upload に fallback (decoder 内部で完結)

[動画フルスクリーン open]
  NativeVideoPresenter (独立 HWND + DComp visual tree) を生成
  decoder thread → video_tx → native_output thread が pull → 自前 swap chain に present
```

`VideoPlayer::tick(_ctx)` は再生制御 / repaint hint / ホバーサムネイル要求のみ扱う。
フレームの実体描画は native presenter 内のスレッドが行うため、`tick` で受け取る
`egui::Context` は実質未使用 (互換のため引数だけ残してある)。

### HUD overlay HWND (v0.9.0+ 後期 — CP1-8 で導入)

VST3 プラグイン GUI がフルスクリーン動画再生中も最前面に維持されるため (= 動画を見ながら EQ
カーブを調整する用途)、以前は **VST GUI が presenter HWND の owned + TOPMOST** になっていた。
Windows の owner rule (= owned は owner より常に手前) で、presenter HWND の DComp tree に
描画された HUD バー / シークバー / hover thumbnail は VST GUI の裏に潜る regression を抱えていた。

**解決策**: HUD overlay を独立 top-level HWND `HudOverlayWindow` (`src/video/native_presenter/hud_window.rs`)
として presenter HWND と同じ owner (= presenter HWND 自身) の sibling 配置にし、VST GUI と
並ぶ z-order group に入れる。両方 `WS_EX_TOPMOST`、HUD を後勝ちで `HWND_TOPMOST` に再アサート
することで VST より前に出す。

```
[Fullscreen presenter HWND]                  [HUD overlay HWND]
  ├─ DComp: background visual                  ├─ owner = presenter (sibling of VST GUI)
  ├─ DComp: video swap chain visual            ├─ WS_EX_TOPMOST | NOACTIVATE
  └─ wndproc: key/IME 入力 (presenter focus)   ├─ SetWindowRgn(実 UI rect だけ)
                                                ├─ wndproc: mouse (region 内のみ)
[VST GUI HWND] (= bridge process が host)      └─ DComp: egui overlay visual (CP4 で移植)
  └─ owner = presenter, WS_EX_TOPMOST                ↑ HUD 用 IDCompositionTarget は
                                                       NativeVideoPresenter で保持
最終 z-order (上から):
  HUD overlay HWND (= bars / interactive UI / hover thumbnail)
  VST GUI HWND (= EQ ノブ等)
  Fullscreen presenter HWND (= video frame + background)
```

**入力 2 層化**:

- **Mouse**: HUD wndproc が region 内で受けて `event_tx` に流す。region 外は `SetWindowRgn` で
  物理的に「存在しない」領域として穴を空けているので、OS が下層 (VST or presenter) に直接 mouse を
  配送する (= クロスプロセスでも安定)。`HTTRANSPARENT` のクロスプロセス透過には頼らない。
- **Keyboard / IME**: HUD では受けない (`WS_EX_NOACTIVATE` で focus を取らない)。presenter HWND の
  既存 wndproc で受けて `NativeEguiOverlay` に流す。HUD 上の mouse-down で `claim_foreground(presenter_hwnd)`
  を発火することで、VST 操作後でも presenter HWND を foreground/focus に戻して keyboard/IME を維持。
  Space / Enter / 矢印 / W / J/K/L/M/B/P/S などの fullscreen ショートカットは、
  overlay 内のボタン focus が残っていても App 側へ転送する。ブックマーク名編集などの文字入力中だけは
  overlay 側がキーを保持し、Space を文字として入力できるようにする。

**Region 計算とアクティベーション検出**:

`NativeEguiOverlay::compute_hud_regions` が egui run 末尾で表示中の各 UI 要素の rect を集めて返す
(= 上 hover bar / 下 HUD / right panel / jump panel / VST3 panel / speed popup / bookmark editor /
normalize blocker / tile overlay / paused center / seek hover thumbnail / checkmark)。**activation zone** (= bar
非表示時の hover 検出範囲、画面上下端の帯) は region に **含めない** — 含めると bar 非表示時に VST の
ノブが上下端と重なったとき入力を奪うため。

bar の hover 表示は presenter thread の **50ms 周期 `GetCursorPos` polling** (`cursor_polling_tick`)
で代替: cursor が presenter HWND client rect 内なら synthetic `MouseMove` を `push_native_event` に流し、
activation zone 内なら HUD raise burst をエンキューする (= VST 手動クリックで HUD が裏に回ったあとの
復帰経路)。

**Z-order 再アサート (HUD raise burst)**:

VST z-order 操作の各経路 (`set_all_guis_topmost` / `set_all_guis_visible_blocking` /
`set_all_guis_app_active` / `send_chain_z_order` / `show_slot_gui` / `hide_slot_gui` /
`user_hide_slot_gui` / `remove_plugin` / `disable_with_reason`) の末尾で `DspBridge::fire_hud_raise_hook`
が unbounded mpsc に `send(())` する → App `update` で `try_iter` drain → 1 件以上来てれば fullscreen 中の
`VideoPlayer::request_hud_raise()` を 1 回呼ぶ (= coalesce) → presenter thread が
`NativeVideoOutputCommand::RaiseHudToTop` を受けて **即時/16ms/64ms の short retry burst** で
`SetWindowPos(hud, HWND_TOPMOST)` を呼ぶ (= 非同期 VST IPC の z-order 反映を確実に拾う)。

各 raise burst 直前で `foreground_allows_hud_raise` を通す (= command / event / polling のすべての
raise 経路で **allowlist 判定**):

- **許可**: foreground が `presenter HWND` / `HUD HWND` / `main HWND` のいずれか、または
  `editor_hwnds` (= 現在 visible な VST editor container HWND の snapshot) に含まれる HWND
  (`GA_ROOT` で正規化、`IsWindow` + `IsWindowVisible` で stale 排除)
- **skip**: VST plugin の右クリックメニュー / file dialog / 独自 popup (`GetLastActivePopup(editor)`
  で検出)、mIV の設定ダイアログ等の未登録 mIV HWND、別 process

詳細は [vst3-integration.md](vst3-integration.md) の "Fullscreen focus handoff" 節を参照。

**Geometry / DPI 同期**: presenter HWND の `WM_WINDOWPOSCHANGED` → `GeometryChanged` event →
HUD HWND の `SetWindowPos` + overlay surface resize。HUD HWND の `WM_DPICHANGED` → `DpiChanged` event →
`set_overlay_pixels_per_point(dpi/96.0)` + HUD `set_hud_geometry(suggested_rect)` +
`resize_overlay_surface_only`。presenter HWND 自身 (= video transform / background) には影響させない。

**フォールバック経路**: HUD HWND 生成失敗 / 環境変数 `MIV_HUD_OVERLAY=0` でフォールバック有効化。
従来通り egui overlay を presenter HWND の DComp tree に attach (`NativeEguiOverlay::new` の
`after_visual=Some(&video_visual)`、`dcomp_hwnd=focus_hwnd=presenter_hwnd`)。VST GUI 裏に bars が
潜る挙動になるが、CP8 以前の動作と完全等価。万が一の regression の retreat 用。

### GPU フレームの内部フロー (HW decoder 利用時)

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
[ video_tx (bounded mpsc) で UI / native presenter thread へ ]
    ↓
NativeVideoPresenter (= 独立 HWND を持つ別スレッド)
    ├─ ID3D11Device::OpenSharedHandle で受信 → ID3D11Texture2D
    ├─ KEYEDMUTEX 取得 + Fence Wait で同期
    └─ CopyResource → swap chain backbuffer → Present (DComp visual tree 内)
```

#### 共有テクスチャ identity (`shared_texture_gen`) — handle 値再利用への防御

`GpuVideoDevice` (アプリ全体で 1 個、全 `VideoPlayer` が `Arc` 共有) は共有出力テクスチャ
の ring (`shared_output_pool`、最大 24 枚) を持つ。動画切替で解像度が変わると
`acquire_shared_output` はサイズ違いの slot を evict (`CloseHandle`) し、新サイズの slot を
`CreateSharedHandle` で作る。このとき **OS は解放直後の NT shared handle 値を新 slot へ
再利用しうる**。

native presenter は開いた共有テクスチャを `shared_texture_cache` にキャッシュするが、
これを **handle 値だけでキーにすると、handle 値が再利用されたとき前動画のテクスチャを
stale なまま返してしまう** (= 動画切替直後に前動画のフレームが 1 枚混入する。2026-05-15
ユーザー報告)。

対策として `SharedOutputSlot` に **プロセス内ユニーク・単調増加の `texture_gen`** を持たせ、
`BlitOutput` → `D3d11Frame.shared_texture_gen` → presenter まで運び、`shared_texture_cache`
のキーを `(handle 値, shared_texture_gen)` の組にする。handle 値が再利用されても
`texture_gen` が必ず異なるので、別エントリとして開き直す。これは `fence` 側の `fence_gen`
(同じ handle 値再利用問題への既存対策) と完全に同じ思想。

#### present 済みフレームの遅延解放 (`present_retire`)

presenter の `CopySubresourceRegion` は **非同期** (GPU コマンドキューに積むだけ) なので、
`present` 直後に `VideoFrame` (= `D3d11Frame`) を drop すると共有出力 slot の `in_use` が
即 `false` になり、producer (decoder) がその slot を再取得して上書きしうる。presenter 側の
GPU コピーがまだ source texture を読み終えていないと、別フレームの内容が混入する
(2026-05-15、別動画フレームの 1 枚混入)。

対策として `run_native_video_output` は present 済みの `VideoFrame` を即 drop せず、
`present_retire` リングバッファに `(VideoFrame, copy_fence_value)` で保持する。presenter は
自前の `ID3D11Fence` (`copy_fence`) を持ち、`copy_frame_into_backbuffer` のコピー後に
`Signal` して値を進める (`outcome.copy_fence_value`)。`present_retire` は
`presenter.copy_fence_completed_value()` がその値へ到達した = **コピーが GPU 上で完了した**
フレームだけを解放する。これで「presenter のコピー完了後に共有出力 slot を返す」ことが
GPU fence で保証され、時間ベースのヒューリスティックではなくなる。

fence 未作成の環境 (`copy_fence_completed_value()` が `None`) や Signal 失敗
(`copy_fence_value == 0`) のフレームは fence ゲートでは解放せず、深さキャップ
`NATIVE_PRESENT_RETIRE_CAP = 4` のみで解放する (= 時間ベースに縮退、旧挙動と等価)。キャップは
fence が万一 stall したときの上限も兼ねるが、**stall 時のフットプリント (= 共有出力プール
24 slot のうち retire が占める数) を 4 に抑える**ことで、decoder の in-flight (~10-15) と
合わせてもプールに余裕が残るようにしている (2026-05-15 に 8 から 4 へ縮小: cap=8 では
stall 時にプール枯渇 → CPU readback フォールバック → スパイラル悪化の実害があったため)。
`fullscreen_present` perf イベントの `retire_queue_len` で長さを観測できる (fence が
効いていれば通常 1〜3)。

`SwitchSource` 受信時にも fence ゲート付きで opportunistic に解放する: rapid swap で
OLD source 由来の retire エントリが滞留して共有出力プールを圧迫するのを防ぐ。未完コピーは
fence ゲートにより解放されないので、frame 114 系の race を再導入しない。

### CPU フレームの内部フロー (HW decoder 失敗 / 非対応コーデック時)

```
FFmpeg SW decoder (or HW フォールバック後の swscale)
    ↓
AVFrame
    ↓
av_hwframe_transfer_data (HW のとき、GPU→CPU、12.5MB/frame@4K)
    ↓
libavfilter bwdif (設定が Auto/On かつ対象フレーム/ストリームの場合。Auto は frame interlaced flag と stream field_order を参照。send_frame、フレームレート維持)
    ↓
swscale (NV12/YUV → RGBA、CPU で 24MB allocation)
    ↓
[ video_tx (bounded mpsc) で native presenter thread へ ]
    ↓
NativeVideoPresenter::present (CPU 経路ブランチ)
    └─ ID3D11DeviceContext::UpdateSubresource で backbuffer に upload → Present
```

旧 egui 描画パス (`gpu_renderer::video_paint::VideoPaintCallback` /
`wgpu_import::import_shared_d3d11_texture` / `VideoPlayer::texture` /
`ctx.load_texture` 経由の `egui::Image` 表示) は撤去済み。互換のため
`gpu_renderer::d3d11_device` (= `GpuVideoDevice`) と `gpu_renderer::ffmpeg_d3d11`
(= FFmpeg D3D11VA hw_device_ctx 共有) は残っており、decoder と native presenter の
共通基盤として機能する。

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
├── gpu_renderer/           # decoder + native presenter の D3D11 共有基盤、unsafe を局所化
│   ├── mod.rs              # 公開 API: GpuVideoDevice, D3d11Frame, GpuVideoError, VideoColorHint
│   ├── d3d11_device.rs     # D3D11 Device + VideoProcessor + Fence (1134 行)
│   └── ffmpeg_d3d11.rs     # FFmpeg D3D11VA hw_device_ctx 共有 (159 行)
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
- native presenter のライフタイム管理 (`native_output: Option<NativeVideoOutput>`)
- `gpu_latest: Option<D3d11Frame>` / `future_frames: VecDeque<VideoFrame>` は native
  presenter 経路を持たない過渡状態用の保持フィールド (通常運用ではほぼ未使用)。

#### `decoder.rs` (3-thread 構成)

1 動画につき 3 thread を起動し、demux / video decode / audio decode を並行動作させる。
旧構造 (1 thread で demux + 全 decode) では `audio_tx` (bounded=32) または
`video_tx` (bounded=24) が満杯になると thread 全体が block して両方の経路が同時に
止まり、`buf 0/24` の周期的な振動 (= ユーザー報告の「Candyfloss_test / SilentBloom
で頻繁にバッファが空になる」現象) を引き起こしていた。これを解消するため Phase A
(audio decode 分離) → Phase B (demux 分離) で段階的にリファクタした。

| thread 名 | 責務 | 入力 | 出力 |
|---|---|---|---|
| `video-demux` (= `run_decoder`) | `Input::packets()` ループ、seek 調停、EOF idle wait、`engine_event_tx` への SeekCompleted 発火。スレッド本体は `catch_unwind` で囲み、panic は `info_tx(Err)` + `DecoderEvent::Failed` に変換して engine/UI に伝える (無言ハング防止) | `Arc<AvClock>` (seek_request) / 動画ファイル | `video_pkt_tx` (bounded=32) / `audio_pkt_tx` (bounded=64) / `video_ctl_tx` / `audio_ctl_tx` |
| `video-decode` (= `run_video_decode`) | HW (`D3D11VA`) → GPU blit / SW + swscale、PACE_LEAD=0.30 の pacing、`new_seek_pending` generation race check | `video_pkt_rx` (`VideoPacketMsg::{Packet, Eof}`) + `video_ctl_rx` (`VideoControlMsg::Flush`) | `video_tx` (bounded=24、`VideoFrame`) |
| `video-audio-decode` (= `run_audio_decode`) | avcodec decode + swresample、post-seek packet/sample trim、PAUSED/EOF park、EOF drain | `audio_pkt_rx` (`AudioPacketMsg::{Packet, Eof}`) + `audio_ctl_rx` (`AudioControlMsg::Flush`) | `audio_tx` (bounded=32、`AudioFrame`) |

**seek 調停**: `clock.take_seek_request()` を pull するのは demux thread のみ
(= 旧構造と同じ単一 puller)。`input.seek` 成否を判定後、packet queue とは別の
control channel で video に `Flush { serial, trim_before_secs, frame_step }`、audio に
`Flush { serial, seek_target_secs, trim_before_secs }` を enqueue する。decode thread は
`select_biased!` で control を優先受信するため、packet queue が満杯でも Flush が古い
compressed packet の後ろに埋もれない。
audio の `seek_target_secs` はユーザー要求 target (= timeline / engine anchor 用)、
`trim_before_secs` は各 worker の post-seek trim 下限。すべての seek は video/audio とも
`trim_before_secs=Some(target)` で target まで preroll drop する。target 前の keyframe
や preroll frame は表示せず、最初に presenter へ届く frame を `FirstFrameReady` /
seek override clear の対象にする。seek 失敗時は両方 `None` で通常 pacing に戻す。
frame-step seek だけは video 側 `trim_before_secs=None` と `frame_step=Some(...)` で流し、
video decoder が decoded PTS を見て base の直前/直後の 1 枚だけを送出する。audio 側は
基準 PTS まで trim し、停止中の余分な音声 decode を抑える。
キーリピートや seekbar drag の連続 seek は UI 側で最新 target に coalesce し、直前の
seek が 1 frame 表示されるか 250ms 経過するまで次の request を発行しない。
native HUD は `clock.is_seeking()` と `current_seek_serial()` を既存 state として参照し、
1 seek 世代が 150ms を超えたときだけ中央に「シーク中...」を表示する。表示後は 300ms
以上保持して短い seek のフリッカを避ける。

**境界 (先頭 / 末尾) での相対シーク抑止**: `VideoPlayer::seek_relative` は既に先頭 /
末尾に居て要求方向へ実質シークできない場合、シークを発行せず
`RelativeSeekOutcome::AtStart` / `AtEnd` を返す。`info.duration_secs` はコンテナ尺で
最終フレームの PTS より後ろのことが多く、末尾でシークを発行すると decoder が target
付近のフレームを返せないまま EOF に達し、`seek_target_override` が解除されず
「シーク中...」表示が固着する (= seek 完了の clear 条件が満たされない)。呼び出し側
(`app/native_video.rs::native_video_seek_relative_with_hint` / `ui_fullscreen.rs`) は
この戻り値を見て、シークの代わりに「動画先頭です」「動画末尾です」の境界トーストを出す。

境界判定は **先頭側と末尾側で判定式そのものが違う**。

- **末尾側** (`delta > 0`): `target <= cur + SEEK_END_BOUNDARY_TOLERANCE_SECS` (0.01s)。
  動画が EOF で停止すると `cur` が clamp 上限 (`duration - 0.1`) 付近に張り付くので
  狭い許容差で単発キーでも拾える。許容差はシーク粒度 (最小 1 秒) より小さく取る必要が
  ある — そうしないと未 clamp の前進シーク (`target = cur + delta`) でも条件が成立する。
- **先頭側** (`delta < 0`): **`cur` の絶対位置**で判定する
  (`cur <= SEEK_START_BOUNDARY_TOLERANCE_SECS`, 1.0s)。再生中は `cur` が 0 から離れる
  方向にしか進まないため、末尾側と同じ `target >= cur - 許容差` 形式にすると、(a) 狭い
  許容差では「再生開始直後の ← で一度も `AtStart` にならない」、(b) 許容差をシーク粒度
  以上に広げると未 clamp の後退シーク (`target = cur - |delta|`) で **常に成立**して
  Shift+← (1 秒) が全く動かなくなる、という二択になってしまう (どちらも 2026-05 報告)。
  絶対位置判定なら粒度に依存せず「先頭から 1 秒以内なら先頭扱い」で済む。

加えて `seek_relative` は境界を検出したとき、**`is_eof_reached()` の場合に限り**
pending な user seek と `seek_target_override` を
`clear_seek_target_override(current_seek_serial())` で明示クリアする。直前の相対シークが
末尾付近を target にして既に「シーク中...」固着状態になっているケースを、この境界判定の
タイミングで回収するため (= 境界トーストと「シーク中...」が同時に出続ける症状の解消)。

`is_eof_reached()` ガードが必須な理由 (Codex P1): 境界判定の `cur` は
`user_seek_base_secs()` (= coalesce 中の pending target を優先) なので、←→ 押しっぱなし
で pending target が clamp に到達すると、**実シークはまだ手前を向いている (= 正当な
進行中 seek)** のに AtStart/AtEnd になり得る。ここで無条件にクリアすると、その正当な
in-flight seek の override とまだ発行されていない pending seek を巻き込んで潰す。
`is_eof_reached()` は demux が末尾まで読み切ったとき (= override がもう post-seek
フレームを得られない固着状態) だけ true になるので、これでガードすれば固着時だけ掃除し、
進行中 seek は通常経路 / tick 側保険に委ねられる。

**stuck seek の tick 側保険解除**: `seek_relative` の境界回収は「次にもう一度 ←→ を
押す」操作が前提なので、放置されたままだと「シーク中...」が残り続ける。これを潰す
最終保険として `VideoPlayer::tick` 冒頭に、`is_seeking() && is_eof_reached()` が
継続して true である時間を `seek_eof_stuck_since` で計測し、`SEEK_STUCK_EOF_TIMEOUT`
(1200ms) を超えたら `seek_target_override` を強制クリアする処理を置く。
`is_eof_reached()` は `request_seek` で一旦クリアされ demux が末尾まで読み切ったとき
だけ true になるので、進行中の通常 seek は誤検出しない。通常の near-end seek は
post-seek フレーム到着で override が clear されて `is_seeking()` が false になり、
timeout に達する前にラッチが解除される。override をクリアするだけで playing / 位置の
更新は行わず、その後の処理は EOF block (native / 非 native とも `set_position_at_eof`
+ `set_playing(false)`) が seek 固着の解けた状態で引き継ぐ。

**native 経路の EOF 停止**: `tick` の native presenter ブロック (early return する
`if self.native_output.is_some()`) は、以前は **ループ ON のときのループ seek しか
処理していなかった**。native 経路はこの early return で抜けるため非 native 経路の
EOF block (`set_position_at_eof` + `set_playing(false)`) に到達せず、ループ OFF で
末尾に達すると `is_playing()` が true のままクロックが `duration` を超えて進み続けた
(2026-05 報告「動画末尾を超えて再生が進む」)。現在は `quiet_now` の発火条件から
`loop_enabled` を外し (ループ ON/OFF どちらでも EOF drain 完了を待つ)、発火後の
アクションだけ `loop_enabled` で分岐する: ON ならループ seek、OFF なら
`set_position_at_eof(duration)` + `set_playing(false)` で duration 位置に凍結停止する。
これにより seek 固着解除 (上記) / `seek_relative` の境界 override クリアの後も、
`is_seeking()` が false になり次第この EOF block がクロックを末尾で止める。

**境界トーストの linger**: native overlay の `NativeOverlayToast` は表示維持時間を
`linger: Duration` フィールドで個別に持つ。`show_toast` の `linger` 引数が `None` の
ときは `centered` から既定値 (centered: 2.5s / それ以外: 1.8s) を導く (= 従来動作)。
←→ ホットキーの境界トースト (「動画先頭です」「動画末尾です」) は
`native_video_seek_relative_with_hint` から `Some(700ms)` を渡す。キーリピート中は
repeat ごとに re-show されて `started_at` が更新され表示が維持され、キーを離すと
700ms で消える (通常トーストの 2.5s 据え置きだとキーを離した後も長く残って煩わしい)。

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

**fast-swap 時の HW decoder lifecycle 制御** (2026-05-13 追加 / 2026-05-15 root cause 特定):
fullscreen 動画から動画へホイール連射で fast-swap を重ねたとき、video decode thread の
`avcodec_send_packet` が hard-stuck し、新動画が「デコード開始中...」のまま固着する事象が
観察された。

**根本原因 (2026-05-15 特定)**: FFmpeg の D3D11VA decode (`avcodec_send_packet` /
`avcodec_receive_frame` が内部で `ID3D11DeviceContext` / `ID3D11VideoContext` を触る) と、
mIV 側 `GpuVideoDevice::blit_nv12_to_rgba` の context 操作が、**同じ `GpuVideoDevice` の
`ID3D11DeviceContext` を共有しているのに直列化されていなかった**。`ffmpeg_d3d11.rs` の
`AVD3D11VADeviceContext.lock/unlock` が `None` で、FFmpeg は context アクセスを直列化せず、
`processor_cache: Mutex` は blit 同士しか直列化しないため FFmpeg decode 側を守れない。
fast-swap 連射で複数 decoder + presenter blit が同 context を並行使用 → driver 内で
hard-stuck。固着動画では video decode thread が `first packet for serial=0` 直後から戻って
こず、seek flush も処理できず、二次的に engine が `Buffering` から抜けられず音声パイプライン
も詰まる (demux が `audio_pkt_tx` 満杯で待機)。

対策は 3 段構え:

1. **D3D11VA context の直列化** (本命、2026-05-15)
   `GpuVideoDevice` に `d3d_lock: UnsafeCell<SRWLOCK>` を持たせる。`ffmpeg_d3d11.rs` は
   `AVD3D11VADeviceContext.lock/unlock` callback にこの SRWLOCK を渡し (`lock_ctx` =
   `GpuVideoDevice::d3d_lock_ptr()`)、`blit_nv12_to_rgba` も context 操作区間で
   `lock_d3d_context` で同じ SRWLOCK を握る。これで FFmpeg decode と blit の
   `ID3D11DeviceContext` / `ID3D11VideoContext` 操作が直列化され、driver hard-stuck が
   構造的に解消する。C callback 跨ぎで lock/unlock を別々に呼ぶため RAII guard を返せない
   `SRWLOCK` を使う (`SRWLOCK::default()` = `SRWLOCK_INIT`)。`acquire_shared_output`
   (最大 500ms wait) は lock の外で済ませ、FFmpeg decode を不必要に長くブロックしない。

2. **`LIVE_VIDEO_DECODE_THREADS` カウンタによる throttle + 待ち合わせ**
   `src/video/decoder.rs` のグローバル `AtomicUsize` で生存中の `run_video_decode` 数を
   追跡する。`VideoDecodeAliveGuard` RAII guard が関数入口で `+1`、関数終了 / panic
   unwind で確実に `-1` する。`try_start_native_video_fast_swap` /
   `try_start_video_tile_fast_swap` は `MAX_LIVE_VIDEO_DECODE_THREADS=1` を超えていれば
   (= `>= 1`) 新 decoder を即時 spawn しない。本カウンタは **正常再生中の player の thread も
   含めた総 live 数**。健全な thread は cancel 観測後すぐ exit するので、swap 開始時点では
   通常 `live_count=1` (現在再生中の 1 個だけ)。2026-05-15 の実機ログで、
   `live_count=2` (= 旧 decoder と新 decoder が重なる状態) でも `send_packet` /
   keyed mutex 待ちが秒単位に伸び、最終的に DXGI device removed へ進むことを確認した。
   複数 HW decode 自体は一般に可能だが、mIV の fast-swap は decoder create/drop と
   shared-output 回収が密に重なるため、安定性優先で閾値を 1 にした。throttle 判定は
   「target / from がともに動画」と確定した後に行うため、動画→画像のような fast-swap
   対象外の navigation は影響を受けない (Codex P1 review 反映)。動画→動画で上限到達時は
   `NativeVideoSourceSwapPending` に積む。旧 player から `NativeVideoOutput` だけを
   退避し、旧 decoder には cancel を立てて `fs_cache` から drop する。native presenter
   HWND / DComp tree はそのまま表示し、`App::update` で `LIVE_VIDEO_DECODE_THREADS` が
   0 になるのを待ってから新 `VideoPlayer` を作り、退避した `NativeVideoOutput` に
   `SwitchSource` を送る。これにより **同時 HW decoder 1 本**のまま、normal open
   fallback で旧 presenter が閉じる 150-300ms の穴 (背後アプリちらつき / 黒画面) を
   作らず最新 target へ切り替えられる。旧 thread が抜けるまでユーザーには
   「切り替えが少し待つ」体感が出るが、
   SW fallback には**絶対に落とさない**: 個別 open でなら HW で動く動画を勝手に SW に
   切り替えると seek 性能が大きく劣化するため。

   2026-05-15 の長時間ホイール試験では、同時 decoder 数を 1 にしても
   **中間 target ごとに decoder create/drop が走る**だけで D3D11VA / shared-output
   経路が荒れ、65 秒で `video_decode_spawn=162`、`shared_output_pool_grow/evict`
   が各約 400 回、最終的に `CreateOutputView: E_OUTOFMEMORY` が出た。通常の
   動画→動画ナビゲーションは `NativeVideoSourceSwapPending` を常に経由し、
   `requested_at` から 120ms の quiet period を待ってから最新 target だけを open
   する。ホイール連射中は pending の `target_idx` を更新するだけで decoder を作らず、
   既存 native presenter に `NativeOverlayNavigationPreview` を表示する。プレビューは
   resume 位置の自動保存サムネがあればそれを全画面 fit で出し、無ければ黒背景 +
   ファイル名バーにする。これにより静止画ホイール移動に近い「移動先を文字/静止画で
   確認できる」状態を保ちつつ、decoder create/drop は 120ms quiet 後の最新 target
   だけに絞る。プレビューは `SwitchSource` 発行時点では消さず、新 source の最初の
   frame が native presenter に `Present` された時点で消す。これにより decoder
   startup 中に旧 source の最後の frame が一瞬露出するのを避ける。タイル fast-swap は既存の `video_tile_swap_pending` が UI 期待と
   異なるため、この 120ms coalesce の対象外とする。

   **UI thread での待ち合わせは導入しない** (2026-05-15、Codex 指摘 #1 反映): 一時的に
   `wait_for_live_decoders_below(max, timeout)` で 5-10ms polling sleep する helper を
   導入したが、UI thread での同期 sleep はホイール連射時の応答性を悪化させたため撤回。
   - **fast-swap (両系統)**: 非ブロッキングな `load` 判定のみ。閾値以上なら新 decoder
     自体は開始せず、`NativeVideoSourceSwapPending` で旧 native presenter を保持したまま
     空きを待つ。
   - **通常 open** (`start_fs_load` → `build_video_player_for_open`):
     `LIVE_VIDEO_DECODE_THREADS >= MAX_LIVE_VIDEO_DECODE_THREADS` かつ HW decode 有効なら `NativeVideoOpenPending`
     に積み、後続 `App::update` tick で再判定する。UI thread は sleep / join しない。
     live decoder 数が下がった時点で `start_fs_load` を再開し、10 秒下がらなければ
     `regular_open_deferred_timeout` を出して fullscreen を閉じる。これにより
     ESC 後の通常 open が `live_count=3/4` まで decoder を増やす経路を塞ぐ。

   **Resume プレビューサムネ**:
   `save_all_video_resume_positions()` は `VideoPlayer::last_displayed_pts_secs()`
   (無ければ clock position) を resume 位置として保存し、同時に
   `TileThumbCache::store_resume_webp()` で動画 1 本につき最新 1 枚の WebP を
   `video_resume_thumbs` テーブルへ upsert する。タイル一覧用の
   `video_tile_thumbs(path, tile_w, timestamp_ms)` とは別テーブルにする理由は、
   resume 位置が数秒ごとに変わるたびタイル用 timestamp 行を増やさないため。
   `video_resume_thumbs` は `path` を primary key にし、`tile_w` は品質判定列として
   使う。lookup 時に mtime 不一致または現在の preview 幅より狭い行を見つけた場合は
   その場で削除し、古い/低品質な 1 行が残って次回生成を妨げないようにする。
   preview 抽出幅は `VIDEO_RESUME_PREVIEW_EXTRACT_WIDTH` (= 1280) で、4K/8K 原寸
   RGBA を overlay へアップロードして VRAM pressure を再発させない一方、全画面
   背景として動画識別に十分な解像度を確保する。wheel update 中に同じ動画/PTS の
   WebP を UI thread で繰り返し decode しないよう、App は直近 8 件だけ session-local
   RGBA preview cache を持つ。永続 DB は WebP のままなので、起動をまたぐメモリ増加はない。

3. **旧 player の eager drop**
   `start_native_video_source_swap` は `take_native_output` で旧 player から
   `NativeVideoOutput` を抜いた直後、`build_video_player_for_open` で新 player を作る
   **前**に `fs_cache.remove(&from_idx)` を呼ぶ。旧 `VideoPlayer::drop` の cancel フラグ
   設定が新 video decode thread の spawn より前に起き、旧 thread が安全点に居れば
   早めに自発 exit する。旧コードは末尾まで drop を遅延していたため、新旧 thread の
   生存期間が build + attach + switch 全工程分だけ不必要に重なっていた。

#1 の直列化で hard-stuck は構造的に解消したが、stuck thread を能動的に殺す手段は無い
(FFmpeg context を他 thread から close するのはクラッシュリスク)。万一 #1 で防ぎ切れない
stuck が起きて live count が 2 まで累積した状態 (= 例: stuck 1 + 現在再生中 1) では
fast-swap が no-op になり、stuck thread が抜けるまでホイールが効かない。診断用に
`run_video_decode` は異常に遅い `send_packet` (>100ms) を 1 行警告ログする。

JoinHandle 保持 + background cleanup pool による join-with-timeout は将来追加候補で、
カウントが永続的に高止まりするときの「stuck と判定して count を強制的にリセットする」
診断系を追加するときに合わせて入れる。本対策は cancel 観測可能な thread の自発 exit に
依存して count を回す最小実装。

**`get_hw_format` 候補選択** (Codex 解析 2026-05-15、別件の固着の根因):
当初の `get_hw_format` callback は「D3D11 が候補に無いとき先頭候補に fallback」していた。
だが `fmt_list` の先頭は `DXVA2_VLD` / `CUDA` / `VAAPI` / `VULKAN` のような **別の
HW pixel format** であることがある。それを返すと FFmpeg は「mIV が選んだ HW format に
対応する `hw_device_ctx` が無い」状態となり、`avcodec_send_packet` が AVERROR(ENOSYS)
("Function not implemented") を**無限に**返し続ける (実測: 1 再生で 4158 件のログ + UI が
「準備中」のまま固着)。本実装は D3D11 のみ受け入れ、無ければ `AV_PIX_FMT_NONE` を返して
明示的に decoder 初期化を失敗させる。SW フォールバックは行わない (ユーザーポリシー:
シーク体感が大きく劣化するためアプリ判断での SW 切替は禁止)。

**`send_packet` エラー分類と thread exit** (Codex 助言 + P1 review 2026-05-15):
旧コードは `send_packet` エラー時に一律 `continue` で全パケットを処理し続けていたため、
HW decode 初期化失敗 (上記 `get_hw_format` 経路や GPU resource pressure) があっても
decode thread は exit しなかった。結果として `LIVE_VIDEO_DECODE_THREADS` が減らず、
fast-swap throttle が永遠に refuse 状態のままになり「動画が一切切り替わらない」
固着につながった。

本実装はエラー種別で分岐する:

- **EAGAIN** (AVERROR(EAGAIN) = errno 11): 致命ではない。decoder 内部の output buffer が
  満杯で「先に `receive_frame` で drain しろ」の意味。packet を `pending_resend_packet:
  Option<(u64, Packet)>` に **保存時の seek serial 付きで** 保持し、同 iteration で
  `receive_frame` ループに進んで drain、次 iteration の頭で recv を skip して再送する。
  **再送前に seek 進行チェック**: `pending_serial != current_seek_serial` か
  `pending_serial != clock.current_seek_serial()` (= UI 側で `request_seek` 済だが Flush
  はまだ ctl_rx 経由で到着していない過渡状態) なら pending を破棄して通常 recv 経路に
  戻し、Flush を確実に処理させる (Codex P1 2026-05-15)。これを怠ると stale な packet を
  flush 前の decoder に注ぎ込んで「seek 後に古いフレームが混ざる」再発要因になる。
  Flush ハンドラも `pending_resend_packet = None` で同様の防御 (双方向のガード)。
  致命系カウンタには加算しない。
- **致命系** (ENOSYS / EINVAL / External / その他): `MAX_CONSECUTIVE_SEND_PACKET_ERRORS=5`
  で連続失敗を打ち切り、`send_packet_exhausted` perf event を出して thread を exit。
  `receive_frame` で 1 枚でも取れたらカウンタリセット (transient な driver pressure を許容)。

これにより:
- 高負荷時の EAGAIN を「decode 失敗」と誤判定しなくなる
- 致命系は確実に thread が exit し、LIVE カウンタが減って fast-swap throttle が再び通る
- thread exit 時に `AvClock::set_decode_failed(true)` で上位に通知。`VideoPlayer::tick()`
  がこのフラグを `error` に転写し、native overlay の「準備中」表示を
  「動画のハードウェアデコードに失敗しました」に切り替える。

**GPU resource pressure 対策と容量設計** (Codex 助言 2026-05-15):
fast-swap 連発時 (= 解像度違い動画をホイールで連射) に `wgpu Out of Memory` panic が
観測された (実測 82 秒で OOM)。perf log 解析で:
- `shared_output_pool_grow=692 / evict=668` (45 秒で全 pool が 15 回入れ替わるペース)
- 同一動画再生中の grow が 633/692 (= 同サイズで 23↔24 を oscillation)
- 最終的に `DwmPresent` が 1 秒ブロック → wgpu allocation 失敗 → panic

真因は wgpu 自体ではなく **D3D11 側 (`shared_output_pool` + `shared_texture_cache` +
`retired_video_surfaces`) が adapter memory を圧迫し、後段の wgpu allocation が
失敗した** こと (Codex 補正)。本来の対策は容量設計の統合 + pressure 時 degradation:

| パラメータ | 旧値 | 新値 | 4K BGRA での想定上限 |
|---|---|---|---|
| `OUTPUT_RING_SIZE` (D3D11 共有 pool) | 24 | **16** | ~512 MB |
| `RETIRED_VIDEO_SURFACE_DEPTH` (swap chain 世代) | 3 | **1** | ~95 MB (旧 ~380 MB) |
| `SHARED_TEXTURE_CACHE_CAPACITY` (presenter 側) | 64 | **8** | ~256 MB (旧 ~2 GB) |
| `video_tx` capacity (decoder → presenter) | 24 | **8** | (pool slot 占有) |
| `MAX_NATIVE_SOURCE_QUEUE` (presenter 内) | 無制限 | **8** | (pool slot 占有) |

加えて `SwitchSource` ハンドラで `presenter.clear_shared_texture_cache()` を呼び、
動画切替時に前動画の共有 texture キャッシュを即時破棄する (4K で 1 動画 ~256 MB を
解放)。

**`source.queue` の back-pressure**: 旧コードは `video_rx.try_recv()` を空になるまで
drain して queue に積み込んでいたため queue が 23 frame まで肥大化。`MAX_NATIVE_SOURCE_QUEUE`
で cap し、超えたら drain を停止することで `video_tx` (cap=8) に逆圧をかける。decoder
側は `try_send` 失敗で古い frame を drop / 待機して自然に pacing する。

**`GpuVideoError::ResourcePressure` バリアント** (新規追加):
shared output pool exhausted、`E_OUTOFMEMORY` (0x8007000E)、
`D3D11_ERROR_TOO_MANY_UNIQUE_VIEW_OBJECTS` (0x887C0003)、`TOO_MANY_UNIQUE_STATE_OBJECTS`
(0x887C0001) 等を `Blt` から分離。`is_resource_pressure()` で判定可能。`E_OUTOFMEMORY`
は以下の経路すべてで振り分ける (Codex P1-2 review 2026-05-15):
- `acquire_shared_output` の pool exhausted (500ms timeout)
- `create_intermediate_rt` の `CreateTexture2D`
- `acquire_shared_output` 内の `create_shared_output` の `CreateTexture2D`
- `ensure_processor` の `CreateVideoProcessorEnumerator` / `CreateVideoProcessor`
- `blit_nv12_to_rgba` の `CreateInputView` / `CreateOutputView` / `VideoProcessorBlt`

**First-frame watchdog + ResourcePressure 連続上限 + recv timeout** (Codex 助言 B/4
2026-05-15、Buffering 固着の fail-fast):
実機テストで「decode thread が `LIVE_VIDEO_DECODE_THREADS` を詰めたまま自発 exit せず、
ESC 後の再 open でも live_count が累積して全 fast-swap が refuse される」固着が観測された。
原因は decode thread が `recv` で永久 block (両 channel 無音) または ResourcePressure
drop を無限ループするケースで、`send_packet_exhausted` (5 連続失敗) 経路を踏まないため。

対策 3 層:

1. **recv timeout 化**: `recv_video_decode_input` を `recv_video_decode_input_with_timeout`
   に置換し、`RECV_TIMEOUT=500ms` で必ず loop 頭に戻る。両 channel 無音でも cancel /
   watchdog の評価ができる。

2. **First-frame watchdog**: `'outer: loop` の頭で `!first_frame_delivered &&
   watchdog_start.elapsed() >= FIRST_FRAME_TIMEOUT (10s)` なら `set_decode_failed(true)`
   + `break 'outer`。`first_frame_delivered` は `video_tx.try_send` 成功後にだけ立てる。
   `first_frame` perf event を出しただけ、または GPU blit 成功後に queue full で落ちた
   だけでは watchdog を解除しない。何かしらの理由で 10 秒以内に first frame が
   `video_tx` に届かない thread を確実に exit させる。`LIVE_VIDEO_DECODE_THREADS`
   カウンタが解放され、後続 open が通る。perf event `first_frame_timeout`。

3. **ResourcePressure 連続上限**: `MAX_CONSECUTIVE_RESOURCE_PRESSURE=60` (≒ 30fps で
   2 秒分) を超えたら `set_decode_failed(true)` + `break 'outer`。最初の frame 前に
   drop だけが続くケースの fail-fast。GPU path 成功 (`try_send` Ok) で counter リセット。
   perf event `resource_pressure_exhausted`。

**Regular open pending** (Codex 助言 2、2026-05-15):
`start_fs_load` の動画専用パスは、古い video cache entry を drop して cancel を先に立てた後、
HW decode 有効かつ `LIVE_VIDEO_DECODE_THREADS >= MAX_LIVE_VIDEO_DECODE_THREADS` なら `VideoPlayer::open` を呼ばず
`NativeVideoOpenPending` をセットする。pending 中は 100ms 以下の cadence で `App::update`
から `LIVE_VIDEO_DECODE_THREADS` を再判定し、空いたら同じ idx / path / from_grid intent で
open を再開する。新しい動画要求が来たら pending は最新 1 件に置き換える。ESC /
fullscreen exit / items 差し替えで stale になった pending は破棄する。

perf event:
- `regular_open_deferred`: live decoder 上限で通常 open を保留
- `regular_open_deferred_start`: 保留していた open を開始
- `regular_open_deferred_timeout`: 10 秒待っても空かず中止

**Fullscreen source-swap pending** (2026-05-15、同時 HW decoder 1 本運用の表示穴対策):
動画→動画の fullscreen / tile fast-swap で `LIVE_VIDEO_DECODE_THREADS >=
MAX_LIVE_VIDEO_DECODE_THREADS` の場合、normal open 経路へ fallback しない。normal open
は旧 `VideoPlayer` と一緒に native presenter も閉じるため、新 presenter が起動して
最初の frame を present するまで 150-300ms 程度 fullscreen が抜け、背後のアプリや
黒画面が見える。

代わりに `NativeVideoSourceSwapPending` を使う:

1. 旧 `VideoPlayer` から `take_native_output()` で `NativeVideoOutput` を抜き、App 側の
   pending に退避する。
2. 旧 `VideoPlayer` は `fs_cache` から drop して decoder / audio を cancel する。
3. pending 中も `ensure_native_video_front` / fullscreen backdrop 判定 /
   `native_video_presenter_hwnd()` は退避した native output の HWND を presenter として扱い、
   DspBridge owner / main cloak の同期を維持する。`fs_cache` には一時的に target idx の
   `VideoPlayer` が存在しないため、ここを `fs_cache` だけで判定すると main HWND cloak と
   黒 backdrop raise が毎フレーム走り、HUD とシークバーだけ進む黒画面になる。
4. `App::update` で live decoder 数が空いたら新 `VideoPlayer` を作り、
   `attach_native_output()` + `SwitchSource` で同じ presenter HWND に新 source を接続する。

pending 中にさらに動画へ移動した場合は target だけを最新へ更新する。画像へ移動した場合は
pending を破棄し、通常の fullscreen 遷移に戻す。perf event:
- `source_swap_deferred`: source-swap を保留
- `source_swap_deferred_update`: pending 中の target 更新
- `source_swap_deferred_start`: 保留していた source-swap を開始
- `source_swap_deferred_timeout`: 10 秒待っても旧 decoder が抜けず中止

**`SharedOutputSlotGuard` (slot 解放 RAII guard)** (Codex P1-1 review 2026-05-15):
`acquire_shared_output` は内部で `in_use=true` + keyed mutex `AcquireSync(0)` した slot を
9-tuple で返す。後段の `CreateInputView` / `CreateOutputView` / `VideoProcessorBlt` /
`Signal` が `?` で早期 return すると、`D3d11Frame` が作られないため `Drop` で
`in_use=false` が走らず **slot が永久占有 (LEAK)** していた。`ResourcePressure` を frame
drop に倒したことでこの失敗パスが日常的に踏まれるようになり、pool slot を 1 つずつ消費
していって最終的に「常時 pool exhausted」状態 → 新動画が「準備中」のまま固着する症状を
発生させていた (実機テスト 2026-05-15 で `live_video_decode_threads` が 4 まで累積し
回復不能になることを観測)。

`SharedOutputSlotGuard` を acquire 直後に作り、状態を 2 フェーズで追跡:
- Phase A (`holding_write_key=true`): `acquire` 後 ～ `ReleaseSync(1)` 前。失敗時 Drop で
  `ReleaseSync(0)` + `in_use=false` + condvar notify。
- Phase B (`holding_write_key=false`): `ReleaseSync(1)` 成功直後。write key は reader
  側に渡っているので、Drop では `ReleaseSync` を呼ばず `in_use=false` のみ。次回 acquire は
  `recover_shared_output_keyed_mutex` 経由で `released_to_reader=true` の slot を取り戻す。

成功時は `BlitOutput` 返却直前に `slot_guard.disarm()` で armed=false にし、slot
ownership を `D3d11Frame::Drop` へ移譲する。診断用 perf event `shared_output_drop_unfinished`
が armed のまま Drop された場合に emit される (= leak 復旧を観測可能)。

**Pressure 時の degradation = frame drop に統一** (Codex 助言、旧 CPU readback 廃止):
`try_gpu_blit_path` が `GpuVideoError::ResourcePressure` を返したら、decoder 側は
**CPU readback fallback を採らず frame を drop して continue する**。CPU 経路は
`av_hwframe_transfer_data` + `swscale` + GPU upload を要求し、pressure 中の adapter
memory をさらに食って OOM 連鎖を加速させる (= スパイラル)。frame 1 枚を捨てる方が
体感上はるかに軽傷。perf event は `video_decode/frame_dropped_resource_pressure`。
通常の GPU エラー (一過性) は従来通り CPU fallback で救う。

**`drain_full_hit` perf event の rate-limit**:
旧コードは demux drain 失敗のたびに perf event を吐いていたため、1 セッションで
417,515 件を観測。perf log 自体の I/O が UI 応答性を悪化させていた。200ms 連続発火
抑制を入れて、集計目的としては十分な粒度に絞る (= 状態変化は `queue_state` event で
別途記録される)。

**SW フォールバック禁止 (HW 要求時)** (Codex P1 2026-05-15):
当初 `preferred_video_decoders` の default candidate は `allow_sw_fallback: true` のまま
だったため、`get_hw_format` を D3D11-only にしても decoder candidate 側で HW init/open
失敗時に SW decoder で開く path が残っていた = ドキュメントと実装の不整合。本実装は
default candidate の `allow_sw_fallback` を `!hw_decode_requested` に変更し、HW 要求時は
SW へ落とさず open エラー扱いにする。HW 非要求 (= 設定で HW disabled) のときだけ SW で
開く (= 元から SW 要求の経路なので問題なし)。ユーザーポリシー (= シーク体感を損なう
SW 切替の禁止) を実装で担保する。

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
- 動画音量は -∞dB〜+18dB の dB フェーダーで手動調整する。保存値は既存互換のため
  `Settings.video_volume` の線形ゲインのまま保持し、UI で dB フェーダー位置へ相互変換する。
  0dB 超の分は `audio-pump` で safety limiter の前に preamp gain として掛け、
  `fill_output` 側の RT 音量は最大 0dB に抑える。これにより 0dB 以下の音量変更は従来通り
  低レイテンシで、boost 時だけ limiter の 5ms lookahead を PDC latency として扱う。
  safety limiter の ceiling は 0 dBFS (= フルスケール) で、これを超えた分だけゲインを
  下げて hard clip を防ぐ。**赤いピークインジケータは「ceiling に触れた瞬間」ではなく、
  ゲインリダクション量が `SAFETY_LIMITER_INDICATOR_GR_DB` (1 dB) 以上に達したブロックで
  だけ点灯する** — タイムストレッチ由来の 1 dB 未満の微小オーバーや f32 演算誤差では
  点かず、VST / 音量 boost / normalize boost で実際に gain staging が破綻したときだけ
  点く。点灯時は `AvClock` の sequence を増やし、native HUD の音量表示右側に約 500ms
  表示する。判定は音量フェーダーに依存しない (リミッターはフェーダー前段で内部信号に
  作用するため、戻り値は内部チェーンが 0 dBFS をどれだけ超えたかをそのまま表す)。
- 現在フレームのクリップボードコピーは `screenshot.rs` の one-shot worker で別 FFmpeg
  input を開き、最後に表示済みの source pts 近傍をフル解像度 RGBA に変換してから
  既存の CF_DIB clipboard helper へ渡す。メイン decode queue / native presenter の GPU
  surface には触れないため、D3D11VA / CPU fallback / native DComp 経路で同じ操作にできる。
- 前/次フレーム送りはスクショ対象フレームを選ぶための機能なので、`avg_fps` 由来の
  推定秒数をそのまま seek target にせず、メインの動画 decoder に frame-step seek を
  発行して現在表示 PTS の前後にある実 decoded frame だけを送出させる。D3D11VA が
  有効な動画では探索自体も HW decode 経路で進むため、別 input / SW decode worker は
  使わない。前フレーム探索の demux target は base の約 1.25 frame 前に寄せ、FFmpeg の
  backward seek で target 以前の keyframe に戻ってから base 直前の decoded frame を選ぶ。
  これにより古い実装のように数秒ぶん余分に decode するケースを避ける。
  ボタン押下時点でまず現在表示 PTS に pause し、探索中に再生 clock が進んで
  base frame がずれることを避ける。連続入力中は
  「最後に表示されたフレーム」ではなく「最後に発行した frame-step target」を基準に
  次の隣接フレームを探し、seek 完了前の連打 / 長押しでも同じ位置へ再 seek しない。
  ただし長押し repeat は、発行時点の `displayed_frame_seq` から新しいフレームが 1 枚
  表示されるまで次 target を出さない。これにより clock target だけが進んで画面が
  追いつかない状態を避ける。
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
  - blit 完了後に fence を Signal (= native presenter の wait 用)
- 色空間 hint (`SetStreamColorSpace1` / `SetOutputColorSpace1`) は SDR/HDR PQ/HLG を明示
  (HDR 表示は非対応。HDR/10-bit 入力も VPP が SDR BGRA8 として出力)

#### `gpu_renderer/ffmpeg_d3d11.rs`
- FFmpeg の `AVHWDeviceContext` (D3D11VA) を **mIV の D3D11 Device で初期化**
- これにより HW デコード結果テクスチャと VPP が同じ D3D11 device 上にある
  (= `CopyResource` 等で device 跨ぎなく扱える)

> **撤去済み**: `gpu_renderer/wgpu_import.rs` (NT 共有 HANDLE → wgpu::Texture import) と
> `gpu_renderer/video_paint.rs` (`egui::PaintCallback` ベースの `VideoPaintCallback`) は、
> 旧 egui 描画パスでのみ使われていたため v0.9 系の native presenter 必須化と同時に削除。

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
- 他アプリからフォーカスを戻すための左クリックは `WM_MOUSEACTIVATE` で
  `MA_ACTIVATEANDEAT` を返して破棄する。Windows がアクティブ化トリガとなった
  `WM_LBUTTONDOWN` を `wnd_proc` に dispatch しないので、再生 toggle (App 経路の
  `handle_native_video_mouse_button` / overlay 経路の `primary_clicked`) どちらも
  発火せず、画像フルスクリーンの `fs_suppress_primary_until_release` と同等の
  挙動になる (HTCLIENT 上の左クリックのみ対象、右/中ボタンはそのまま通す)

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

VST3 再生中パネルは `egui::Area::movable(true)` で overlay 内をドラッグできる。
ドラッグ終了時は native overlay command として UI thread へ戻し、
`settings.vst3_panel_pos` に logical points の左上位置を保存する。復元時は現在の
overlay bounds に clamp するため、解像度・DPI・モニター構成が変わっても画面外に
取り残さない。

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

#### 動画オープン準備中 HUD のデバッグ環境変数

`src/video/avio_progress.rs` は custom AVIO で `avformat_open_input` /
`avformat_find_stream_info` の進捗を `PreparingProgress` に反映し、native presenter
overlay の中央 status に「メタデータ読込中...」「ストリーム解析中...」を表示する。
高速な SSD / OS cache では表示が一瞬で終わるため、実機確認用に以下のデバッグ環境変数を持つ。
どちらも **demux worker だけ**を待たせ、UI thread は止めない。

- `MIV_DEBUG_VIDEO_PREP_DELAY_MS=<ms>`: `phase=OPENING` と file size を設定した直後、
  open 開始前に固定 sleep する。最大 60000ms に clamp。
- `MIV_DEBUG_AVIO_READ_DELAY_MS=<ms>`: custom AVIO の read callback ごとに sleep する。
  `phase != DONE` の準備中だけ有効で、open 完了後の通常 packet 読み込みは遅くしない。
  1 read あたり最大 250ms に clamp。
- `MIV_DISABLE_AVIO_PROGRESS=1`: custom AVIO を使わず旧 `ffmpeg::format::input(&path)`
  へ直接フォールバックする切り分け用。進捗バイト数は取れないが、再生可否の確認に使う。

#### `tile_thumbnails.rs` / `tile_thumb_cache.rs`

フルスクリーン中の **タイルモード** (`S` キー / ホバーバー ▦ ボタン) で使う、
動画から複数フレームを一括抽出して並べる仕組み。

- `tile_thumbnails.rs`: 一括サムネイル抽出 worker。指定動画から N 個の絶対 PTS で
  フレーム取得 (FFmpeg seek 系統は `screenshot.rs` と同じ one-shot 方式)。
  backward seek 後は **`pts >= target_secs` の最初のフレーム**を採用する
  (= 再生で同位置にシークしたとき表示されるフレームと一致させる)。decode 数に
  上限は設けない — 長尺 GOP でも必ず target に到達するため (上限を置くと GOP 長に
  よってサムネが実位置からずれる)。EOF まで target に届かない末尾付近のケースは
  最後にデコードできたフレームを fallback に使う。worker は cancel フラグを
  1 パケットごとに確認するので別 interval / 動画への切替時は自然終了する。
  同じ frame-selection は seek hover サムネ `thumbnail.rs` と共通
- `tile_thumb_cache.rs`: SQLite + WebP の永続キャッシュ。**絶対 PTS をキー**にしているため
  動画の長さが変わっても再ヒットする (Phase 8.C の修正)
- **抽出幅は `settings::VIDEO_TILE_EXTRACT_WIDTH` (640px) に固定**。列数・モニター解像度・
  どのモニターで再生するかに依らず常に同じ幅で抽出・保存するので、キャッシュは
  「動画 × 絶対 PTS」で 1 行に集約され、列数を切り替えても解像感が混ざらない
  (旧実装は接続モニター最大幅 / 最小列数から導出していたため、列数や候補変更で
  抽出幅が動き、640/960 が混在し得た)。表示用の `tile_w` / `tile_h` は別途
  `VideoTileState` が持ち、native overlay が描画時に `tile_rect` へスケールする。
  抽出幅を表示が超える構成 (横長 4K での 4 列など) は egui の拡大描画で許容する。

タイルモードの UI 描画は `native_presenter/overlay_draw.rs` の以下 2 関数で構成する:

- `draw_native_tile_overlay` — 中央 preparing 文言とサムネイルグリッドを描画。
  **`egui::Area` の order は `Order::Background` 固定**。グリッドは全画面を不透明黒で
  塗り、かつ全画面の click sense を登録するため、chrome (上部バー / toast =
  `Order::Foreground`) と同じ order に置くと、egui が click されたレイヤを
  `move_to_top` する仕様により「グリッド背景を 1 回クリック → グリッドが上部バーの
  上に昇格 → 黒塗りが上部バーを丸ごと隠す」という回帰を起こす。order を分けておけば
  描画順が固定され `move_to_top` の影響を受けない。Foreground に戻さないこと。
  あわせて、`render_once` ではタイルモード中 (`tile_overlay.is_some()`) は perf overlay
  (`Order::Middle`) を描画しない。grid が `Order::Background` の不透明塗りなので、
  perf を描くと grid の上に乗ってサムネイルと click (seek) を塞ぐため。
- `draw_native_top_bar_tile` — 通常再生時の `draw_native_top_bar` と同じ 54px の
  上部バーを描画し、タイトル / 解像度 / fps / コーデック / duration / タイル間隔 /
  抽出進捗 (`N/M`) を表示する。右側に 3 ボタン: × (`ToggleTileMode`)、
  5x5 / 3x3 グリッドアイコン (`TileColumnsDelta { delta: ±1 }`)。Ctrl+ホイールでの
  列数切替と等価で、ショートカットの発見性を上げる目的で並べてある。
- `NativeOverlayTileOverlay` には `fallback_file_name: String` を含む。ホイールで
  別動画に切り替わって metadata が None になる数フレームでも上部バーにファイル名を
  出すための fallback。`sync_native_video_tile_overlay` が `state.video_path` から
  詰める。`preparing_with_filename(name)` コンストラクタで preparing 状態にも値を
  通す。
- S タイル表示中は `NativeEguiOverlay::render_once` がタイル overlay を描画して早期
  return するため、通常の center status HUD (`PreparingStatus` →
  `build_preparing_message`) は描画されない。動画→動画の tile fast-swap では
  `NativeOverlayTileOverlay::video_open_status` に `player.prep_progress().snapshot()`
  を詰め、タイル overlay 側で「メタデータ読込中...」「ストリーム解析中...」を表示する。
  実際のタイル抽出待ち (`video_open_status == None`) は「タイルを準備中...」として
  動画オープン待ちと区別する。
- `video_tile_mode_active` は「ユーザーが S でタイルモードを開いている」という mode flag。
  `video_tile_state` は worker / サムネ snapshot を持つ実体 state で、source-swap 中や
  metadata 未到着では一時的に `None` になりうる。mode と state を混同すると、動画切替中の
  path mismatch で `video_tile_state` を捨てた瞬間に、タイルモード自体まで無音で解除される。
- `video_tile_reopen_pending` は「タイル表示中に動画を切り替えたが、次動画の info がまだ無い」
  場合の短期 retry 予約。通常のホイールナビゲーション (`WheelNavigate` /
  `try_start_native_video_fast_swap` / `NativeVideoSourceSwapPending` の `reason=navigation`)
  では、`video_tile_mode_active == false` なら `cancel_stale_video_tile_reopen()` で必ず
  破棄する。これを怠ると、過去の tile timeout / reopen 予約が残り、ホイール移動だけで
  突然タイル画面へ戻る。
- `open_native_video_fullscreen_from_navigation()` は `try_start_video_tile_fast_swap()` を
  通常 fast-swap より先に呼ぶため、tile fast-swap 側は **最初に tile context
  (`video_tile_mode_active` or `video_tile_swap_pending`) を確認してから** `NativeVideoSourceSwapPending`
  を見ること。順序を逆にすると、通常動画モードで `reason=navigation` の source-swap
  pending 中に次のホイールが来た時、tile 側が pending を横取りして `reason=tile` に
  書き換え、ホイール移動だけでタイル画面に入る。

`ui_video_tile.rs` は state 構造体 (`VideoTileState`) と worker spawn
ロジックだけを持ち、egui 描画関数は v0.9 系で削除済み。

`build_video_tile_state_for` のタイル枚数計算 (`max_rows` / `pick_interval`) は
**描画先と同じ画面サイズ**を基準にする必要がある。タイルは native presenter
(モニター全面を覆う別 borderless HWND) に描かれるので、`ctx.content_rect()`
(= メイン egui ウィンドウ。別モニター / 別サイズになり得る) を渡すと、縦横比が
食い違うモニター (特に縦長) で「生成枚数 < 敷き詰められる枚数」になり画面上部
だけ埋まる。`App::video_tile_layout_size` が presenter HWND の `GetClientRect` +
`GetDpiForWindow` から実クライアントサイズ (points) を取り、取得失敗時のみ
`ctx.content_rect()` にフォールバックする。タイル生成系の呼び出し元 (S キー /
▦ ボタン / tile fast-swap / reopen / 列数変更) はすべてこの関数を経由する。

## 経路選択ロジック (起動時 1 回)

`src/main.rs` で以下を実行する。`GpuVideoDevice` は decoder の HW デコード + native
presenter への NT-shared blit に使うので、wgpu backend の種別とは独立に常に作成を
試みる。失敗時は decoder が SW デコード + CPU upload に自動 fallback (どちらの
経路でも native presenter が描画する)。

```rust
let backend = rs.adapter.get_info().backend;
crate::logger::log(format!("wgpu backend selected: {backend:?}"));
match crate::video::gpu_renderer::GpuVideoDevice::new() {
    Ok(dev) => app.gpu_video_device = Some(dev),
    Err(e) => crate::logger::log(format!(
        "GPU video device: failed (will fallback to CPU readback): {e}"
    )),
}
```

`GpuVideoDevice::new` のシグネチャから `vsr_enabled: bool` 引数は削除 (= VSR を扱わなくなるため)。

旧 `init_video_pipeline()` (egui_wgpu の callback_resources に動画 wgpu パイプラインを
登録する起動時処理) は native presenter 必須化と同時に削除済み。

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
    /// CPU 経路。`Vec<u8>` は width * height * 4 の **RGBA8** (decoder 側 swscale が
    /// `Pixel::RGBA` 出力を生成する)。native presenter の
    /// `copy_cpu_rgba_to_swapchain_bgra` が RGBA→BGRA 変換しつつ swap chain backbuffer
    /// に `UpdateSubresource` で upload する。
    Cpu(Vec<u8>),
    /// GPU 経路。NT 共有テクスチャ + fence で native presenter が `OpenSharedHandle` 経由
    /// に取得して自分の swap chain にコピーする。
    #[cfg(windows)]
    Gpu(crate::video::gpu_renderer::D3d11Frame),
}
```

`Nv12Direct` variant は **削除** (Phase 2 で導入したが、その経路自体を撤回するため)。
seek 時の target 前 keyframe / preroll frame は decoder 側で drop されるため、
`VideoFrame` には readiness から除外する preview 用フラグを持たない。

## アスペクト比 (SAR) 補正

アナモフィック動画 (NTSC DVD・一部のキャプチャ素材など) は raw pixel 解像度
(`width × height`) と表示比が一致しない。例えば 720×480 + SAR=97/80 の動画は
DAR ≈ 1.819:1 (= 16:9) で表示すべきで、square pixel で扱うと縦長になる。

mIV は **decoder で SAR を読み取り → 各 `VideoFrame` に同梱 → native presenter の visual
transform で anisotropic scale として適用する**:

- `decoder.rs` の `normalize_sar(num, den) -> (u32, u32)` で `AVCodecParameters.sample_aspect_ratio`
  を正規化 (0/0・0/1・負値はすべて 1/1 に倒す)。値は `VideoInfo { sar_num, sar_den }` で UI 層へ
  伝搬すると同時に、**`VideoFrame { sar_num, sar_den }` として各フレームにも載せる**。
- presenter は `present()` で受け取ったフレーム自身の SAR を `ensure_video_geometry()` に渡す。
  これにより SAR は decoder → `video_tx` → presenter thread の **速い経路**で届き、ソース切替
  (fast-swap) 直後の最初のフレームから正しい SAR で描かれる。`VideoPlayer::tick` 経由の
  `set_native_video_sar(num, den)` コマンド (= decoder → `info_rx` → UI tick → command channel
  の遅い経路) は残してあるが、frame 同梱 SAR が先に反映されるため通常は no-op の safety net。
  mid-stream の SAR 変化は frame 同梱なので自然に追従する (bwdif フィルタも frame.aspect_ratio()
  で keying)。
- `NativeVideoPresenter::update_video_visual_transform()` は `compute_video_visual_transform()`
  helper (純粋関数、unit test 6 件あり) で transform 行列を計算する:
  ```
  display_w = surface_w * sar_num / sar_den
  scale     = min(target_w / display_w, target_h / surface_h)
  M11 = scale * sar    (= 横方向だけ余分に伸ばす)
  M22 = scale
  ```
  SAR=1:1 の動画は `M11 == M22` で従来挙動と完全に同一 (regression-safe)。
- swap chain backbuffer / VPP / CPU upload はすべて raw encoded サイズのまま動く
  (= 余計な GPU/CPU 仕事ゼロ、stretch は DComp 側で 1 度だけ走る)。
- タイルモードのセル比率 (`ui_video_tile.rs`) も同じ SAR を反映する。

### ジオメトリ更新 — swap chain の原子的差し替え

`present()` は毎フレームの先頭でフレーム実寸・SAR を見て分岐する:

- **解像度不変** (`present_reusing_surface`): 既存の `swap_chain` / `backbuffer` をそのまま
  再利用。SAR だけ変わった場合は `update_video_visual_transform` で transform を作り直す
  (swap chain content は正しいので中間状態は生じない)。
- **解像度変更** (`present_with_surface_swap`): **新しい video swap chain を別途生成し、
  原子的に差し替える**。手順:
  1. `create_video_swap_chain` で新 swap chain + backbuffer を生成 (まだ visual には繋がない)。
  2. 最初の正しいフレームを新 backbuffer へ copy。
  3. 新 swap chain を `Present` (= 新 swap chain は「正しいフレーム投入済み」状態)。
  4. `_video_visual.SetContent(新 swap chain)` + `SetTransform2(新 transform)` を
     **1 回の `Commit`** で適用。DComp は 1 Commit のバッチを原子的に適用するので、
     compositor から見ると「旧 swap chain + 旧 transform (整合)」→「新 swap chain
     (フレーム投入済み) + 新 transform (整合)」と一気に切り替わる。
  5. `wait_for_video_transform_commit` (`WaitForCommitCompletion` + `DwmFlush`) で
     Commit が DWM 側の表示 tick まで反映されるのを待つ。
  6. 旧 swap chain を `retired_video_surfaces` (深さ `RETIRED_VIDEO_SURFACE_DEPTH`) へ
     移して遅延破棄。

**なぜ原子的差し替えか**: 旧実装は同一 swap chain に `ResizeBuffers` をかけていたが、
`ResizeBuffers` は旧 content を破棄するため「未提示」の中間状態が生じ、そこを compositor が
拾うと黒や「左上にずれた縮小フレーム」が 1 フレーム見えた。さらに DComp transform commit と
swap chain `Present` は compositor に原子的にラッチされないため、`WaitForCommitCompletion`
だけでは「新フレーム + 旧 transform」の混在を防ぎ切れなかった (2026-05-15、複数回の実機録画で
確認)。新 swap chain を**フレーム投入済みにしてから 1 Commit で content+transform を同時
差し替える**ことで、`ResizeBuffers` を使わず中間状態を構造的にゼロにする。さらに
2026-05-15 の実機検証で `WaitForCommitCompletion` だけでは 3840→1920 の縮小 swap 時に
新 content が旧 transform で 1 refresh 見えることがあったため、黒フレームを挿入せず
`DwmFlush` だけを追加して DWM 側の反映まで同期する。

**旧 surface の遅延破棄**: `SetContent` 切替後も DComp/DWM がしばらく旧 content を参照
しうるため、旧 swap chain を即 drop せず `retired_video_surfaces` に数世代分残す。
`RetiredVideoSurface` の Drop が swap chain + frame-latency waitable を解放する。

`video_surface_swap` perf イベントに `surface_width/height` / `sar` / `geom_ms` /
`commit_sync_ms` / `retired_len` を記録する。

動画→動画 source swap では native presenter HWND / DComp tree を維持するため、
`App::open_fullscreen` は既存 presenter HWND がある場合にメイン HWND を再 cloak しない。
メイン HWND の `DWMWA_CLOAK` true→false をホイール切替ごとに挟むと、OBS には映らない
物理画面側の DWM / MPO 合成面のちらつきとして見えることがあるため、初回動画入場時だけ
cloak を使い、presenter 継続中の source swap は presenter 内の visual 更新だけで完結させる。

UI に出す解像度表記 (動画情報パネル等) は MediaInfo / VLC / FFmpeg の慣例に合わせ
**encoded サイズのまま** (例: `720×480`)。DAR の併記は将来検討。

## ライフサイクル管理

- **VideoPlayer の Drop**: `cancel.store(true)` → decoder thread が exit、`audio.take()` で cpal stream 停止
- **VideoPlayer.shutdown() の用途**: 動画切替時に Drop より早く audio を切るため (= 残音を防ぐ)
- **GpuVideoDevice の Drop**: D3D11 リソース全解放、fence の NT shared handle を `CloseHandle`
- **NativeVideoPresenter** (= `VideoPlayer::open` 時に 1 個生成、`VideoPlayer` Drop で停止):
  独立 Win32 HWND + 自前 D3D11 swap chain + DComp visual tree を所有。decoder からの
  VideoFrame を専用 thread で pull → present。
- **D3d11Frame の所有権**: native presenter thread が channel から受信して自身の Drop
  まで保持。次フレーム到着で旧 frame の Drop が NT HANDLE を `CloseHandle` する
  (= 描画中の HANDLE が close される race を防ぐ)
- **z-order 復旧**: PrintScreen / Snipping Tool などで foreground が一時的に外部へ
  移った後、egui 側の黒 backdrop が presenter より前に残る場合がある。UI thread から
  `SetWindowPos` / `SetForegroundWindow` を直接呼ばず、App が外部 foreground を観測
  した後に mIV foreground へ戻ったエッジで `RaisePresenterToFront` command を
  rate-limit 送信し、presenter 所有スレッド側で `HWND_TOP` と foreground / active /
  focus を再アサートする。
- **main HWND cloak**: native video fullscreen の entry と video-to-video swap では、
  presenter HWND が valid になるまで main HWND に `DWMWA_CLOAK` を設定する。これは
  `IsWindowVisible` を変えないため App::update は継続し、DWM 合成結果からだけ main を
  外す。presenter HWND が valid になった時点、fullscreen exit、app exit では必ず
  uncloak し、foreground reclaim が cloaked main HWND を対象にしないようにする。

### フルスクリーン終了時の foreground 奪還

native presenter の HWND は WS_POPUP として独立に存在し、`owner_hwnd = main_hwnd` で
作成される。Alt+Tab で他アプリが「main」と「popup」の z-order の間に割り込むと、
popup destroy 後に Windows が owner ではなく z-order 順で次の他アプリを foreground に
昇格させ、サムネイル一覧が他アプリの後ろに隠れることがある。

これを補正するため、`close_fullscreen` 時点で奪還候補を凍結し
([src/app.rs](../src/app.rs) `pending_main_foreground_reclaim*` フィールド群)、
presenter HWND の destroy を確認した時点で `SetForegroundWindow(main_hwnd)` を
`AttachThreadInput` 併用で呼び戻す ([src/app/native_video.rs](../src/app/native_video.rs)
`process_pending_main_foreground_reclaim`)。native video entry/swap で main HWND を
cloaked にしていた場合は、`close_fullscreen` 内で先に uncloak してから reclaim 候補を
保存する。

ガード条件:
- 動画フルスクリーンを通った時のみ (`native_video_fullscreen_active_for_main_backdrop()`)
- close_fullscreen 時点で mIV プロセスが foreground を持っていた場合のみ
  ([src/video/native_window.rs](../src/video/native_window.rs)
  `foreground_belongs_to_current_process_strict`、null/pid=0 の不確定ケースは false)
- 保存した presenter HWND の `IsWindow == false` (= destroy 完了) を待ってから claim
- 絶対 deadline (`now + 200ms`) を超えても presenter が destroy されていなければ
  諦めて clear (= destroy 待ちが長引いた間にユーザーが他アプリへ切替えた場合に
  奪い返さない実用上抑制)
- `open_fullscreen` で別 idx を直接 open する継続ナビ経路では reclaim 不要なのでクリア

### `VideoPlayer::open(..., native_output_config=None)` のセマンティクス

`VideoPlayer::open` の `native_output_config: Option<NativeVideoOutputConfig>` 引数で
`None` を渡すのは「呼び出し元が後から `attach_native_output` で output を移植する」
ことを示す**正常なシグナル**で、エラー扱いしない。

- **動画→動画 fast-swap 経路** (`try_start_native_video_fast_swap` /
  `try_start_video_tile_fast_swap` in [src/app/native_video.rs](../src/app/native_video.rs)):
  通常の動画フルスクリーンナビゲーションと動画タイルモード中のホイールナビゲーションで、
  旧 player から `take_native_output()` で取り外した output を新 player に
  `attach_native_output` で移植する。新 player 側の `VideoPlayer::open` には
  `native_output_config=None` を渡す (= 自前で spawn しない)。通常ナビゲーションは
  最初の native frame 表示まで `native_video_fast_swap_pending` で連続入力を抑制し、
  タイルモードは従来どおり `video_tile_swap_pending` で `VideoInfo` 到着後にタイル state を
  再構築する。
- **通常経路** (`start_fs_load` in [src/app.rs](../src/app.rs)):
  `native_video_presenter_config(self.main_hwnd, ...)` で config を取得して
  `Some(config)` を渡す。万一 `None` が返った (= モニター情報取得失敗) ときは、
  呼び出し元が `player.fail_native_init(message)` を呼んで error を立てて
  worker を停止する。
- **責務分担**: 「config が取れなかった = 同期 init エラー」の判断は呼び出し元が
  行う (= 呼び出し元だけが「自分が config を期待していたか」を知っている)。
  presenter thread 内の遅延 init エラーは別系統 (`consume_native_init_error`)
  で `tick()` 中に取り込む。

## 設定との関係

整理後、削除する設定項目:
- `Settings.video_rtx_vsr` (= VSR ON/OFF トグル、撤回により不要)

維持する設定項目:
- `Settings.video_volume` (音量。既定 1.0 = 0dB、手動 boost 上限は +18dB 相当の線形ゲイン)
- `Settings.video_loop_mode` (ループ再生モード: Off / Full / Chapter / Bookmark)。
  旧 `Settings.video_loop: bool` は移行用に残存し、`Settings::load()` 内の
  `migrate_legacy_video_loop` で `video_loop=true && video_loop_mode==Off` を Full へ昇格。
  以降は `video_loop_mode` を source of truth として `Settings::save()` 内 clone で
  旧 bool を `mode != Off` から導出して書き戻す。
- `Settings.video_resume_position` (シーク位置の永続化、ファイル単位)
- `Settings.video_hw_decode` (HW デコードを試みるかのフラグ、トラブルシュート用)
- `Settings.video_deinterlace` (Off / Auto / On。CPU 経路で FFmpeg `bwdif=mode=send_frame` を適用。Auto は frame interlaced flag と stream field_order を参照)

### ループ再生 4 段階モードの実装メモ

`L` キー / HUD ループボタンで `Off → Full → Chapter → Bookmark → Off` を循環。
チャプター / ブックマークが空の段階は `cycle_loop_mode` (`settings.rs`) で自動スキップ。
動画移動でモードを保持し、当該データが無い動画では `effective_loop_mode` (`settings.rs`) が
Chapter/Bookmark を Full に降格する (= 再生挙動だけ Full と等価、HUD 表示はユーザー設定モード
を維持するため、`set_loop_enabled(bool)` と `set_native_loop_mode(VideoLoopMode)` を
分離して送る)。

ループ復帰 seek 先は `VideoPlayer::loop_target_bits: AtomicU64` (秒、`f64::to_bits`) に持つ。
`EngineActor::OpenOptions.loop_enabled` は触らず、EOF 経路は `VideoPlayer::tick` 側で
`loop_target_secs()` → `clamp_seek_target` → `clock.request_seek` の順で呼ぶ。
入力サニタイズ (NaN/inf/負値) は setter で済ませ、duration クランプは EOF 直前に既存の
`clamp_seek_target` を経由する。

CH/BM ループは「次境界の手前で現区間の開始へ seek」を `tick_native_video_loop_boundary`
(`app/native_video.rs`) が `poll_video` Phase 3 (= native_events 反映後) で行う。判定は
**`prev_pos` 側の区間で計算する** 純関数 `decide_boundary_action` (`settings.rs`) に委譲し、
serial 変化や巻き戻り時は baseline 更新のみで誤爆 seek を防ぐ (= シークバー / J/K /
タイル seek 直後に勝手にループ開始点へ戻されない)。境界 Vec は
`boundary_starts_from_chapters` (`video/decoder.rs`) /
`boundary_starts_from_bookmarks` (`video_bookmarks.rs`) で finite + nonneg + sort + dedup
正規化済み `&[f64]` を作り、`start_at` / `first_boundary_after` (`settings.rs`) は
この正規化前提で動く。

`poll_video` (`app.rs`) は 4 段階構成: Phase 0 で `ensure_fullscreen_video_marker_cache`
(= 毎 tick の DB クエリを避ける)、Phase 1 で `iter_mut` 中に `set_loop_enabled` / `set_native_loop_mode`
を effective + display_mode 分離で push + `active_video_indices` 収集 + `native_events` drain、
Phase 2 で `handle_native_video_output_event` (= 入力イベント反映)、Phase 3 で
`tick_native_video_loop_boundary` (= 境界 tick)。順序は P2/P3 を入れ替えると serial guard が
直近 seek を検出できないため固定。

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
- リモデ検証: RDP 経由で起動して動画を開く。HW デコード (`D3D11VA`) が失敗するなら
  decoder が SW デコード + CPU upload に自動 fallback して native presenter が描画
  するので、1080p 程度なら再生できる。`mimageviewer.log` に `GPU video device: failed
  (will fallback to CPU readback)` と出ているか、または `decoder` のログで HW 候補に
  落ちているか確認する。
- native presenter 起動失敗時の挙動: `GetMonitorInfoW` 失敗や thread 生成エラーが
  起きると `VideoPlayer.error` に日本語のエラー文言が入り、フルスクリーンに赤字で
  「動画を再生できません: ...」が表示される (= 旧 egui presenter フォールバックは無い)。

## A/V drift 計装 (動画再生中の音声・映像同期デバッグ)

### 用途

「数分再生していると音声と映像がずれた気がする」「Norm ボタンを ON/OFF するとずれる」
のような **再現困難・低頻度の同期バグ**を、後追いで定量的に確認できるようにする計装。

通常の運用には影響しない (= perf-log 無効時はノーオーバーヘッド)。再現に遭遇したら
`mimageviewer.exe --perf-log` で起動し直して操作を再演し、`%APPDATA%\mimageviewer\
logs\perf_events.jsonl` を `python scripts/analyze_perf.py <path> av_drift [--plot]` で
解析する。

### 用語と単位

- **PTS (Presentation Timestamp)**: 動画ファイルに焼き付けられた各 video frame /
  audio frame の表示時刻 (秒、f64)。FFmpeg avformat が抽出する。
- mIV は **音声マスタークロック方式** (mpv / ffplay と同じ)。音声 pump が物理出力した
  サンプルの audible PTS を `AvClock::set_audio_pts` に渡し、video 側は `now_secs()` を
  見て表示・スキップ・待機を決める。

3 つの異なるメトリクスを区別する:

| 指標 | 計算式 | 用途 | 通常値 |
|---|---|---|---|
| **A/V offset** | `video_displayed_pts − audio_audible_pts` | **ユーザー体感の音映像差** (主指標) | 約 0ms |
| audio lead | `audio_audible_pts − master_clock.now_secs()` (post-apply residual) | `set_audio_pts` 適用後でも残っている clock 乖離 | 約 0ms |
| video pacing (旧 av_drift) | `video_displayed_pts − master_clock.now_secs()` | video pacing 健全性 | 約 0ms |

**audio lead** は **post-apply** で計測することに注意 (Codex 助言、2026-05-11)。
`set_audio_pts` の **直前**で `requested − prev_now` を取ると wall extrapolation 分で
通常時にも +10ms 程度の偽 lead が見える。**直後**に `requested − after_now` を取れば
通常時 ≈ 0、Norm 経路バグで +5000ms 級だけが残る。`audio_pts_jump` event の
`requested_delta_ms` / `applied_delta_ms` は cap 検出用なので別管理。

**重要**: ユーザーが「音と映像がズレる」と訴えるのは A/V offset。video pacing だけ見ていると
**Norm clear バグなど audio が clock から乖離するケース**を取り逃す。
master_clock は wall-rate cap で audio に追従できないことがあり、その場合 video は
clock に追従しているが (= pacing は 0)、audio は clock より数秒先行している (= lead が
+5000ms 級)、結果としてユーザーは「映像が音声より数秒遅れて見える」(= offset が
−5000ms 級) と感じる。

### `AudioOutput::drop` の pump join は UI thread をブロックしない

`AudioOutput::drop` は (a) `cancel` フラグ + `shutdown_tx` で pump 停止を要求、
(b) cpal `Stream` を pause + drop、(c) pump thread を join、の 3 段で終了する。

(c) を **同期 join** で行うと、pump 自身が back-pressure deadlock (decoder/engine
side の停滞で `audio_tx` を drain できず、demux も詰まる連鎖) に陥っていた場合に
**UI thread が無期限ブロックする**。2026-05-15 に Escape で fullscreen を抜けた際
「応答なし」14 秒の実害が観測された (`fs_cache.clear()` → `Drop for VideoPlayer` →
`AudioOutput::drop` の経路)。

そこで pump の join を **専用 thread に spawn して切り離す**:

```rust
if let Some(p) = self.pump.take() {
    let _ = std::thread::Builder::new()
        .name("audio-output-drop-join".to_string())
        .spawn(move || { let _ = p.join(); });
}
```

`NativeVideoOutput::drop` で先行採用していたパターンと揃える。万一 pump が exit
できなくても join thread が park 状態で残るだけで UI には影響しない。pause + drop
で cpal callback は確実に止まっているので音は二重再生されない。

### audio buffer clear の atomic 整合性

`AudioOutput::clear_buffer` は seek / Norm / fast-swap / shutdown で呼ばれる。
clear 直後は **新しい audio frame が届くまで `audio_audible_pts` の旧値を残してはいけない**
(さもないと次の present が旧 audio_pts と新 video_pts を比較して偽の巨大 offset を
出す)。`AudioDiagnostics::clear_audio_position()` で
`audio_audible_pts_valid=false` / `audio_audible_pts=NaN` / `av_offset_ms=NaN` /
`audio_lead_ms=0` を atomic にリセットし、次の callback `set_audio_pts` 呼出で再開する。

publish 順序 (Codex 助言): clear 系は **valid=false を先に**書き、`set_audio_pts`
側は **bits 書き込み → valid=true** の順 (= load 側の `valid → bits` の逆順)。
これで「valid=true で旧 bits」の torn read を防ぐ。

### 既知の症状 (修正済): Norm clear で audio が 5+ 秒先行する

**症状** (修正前、〜 2026-05-11):
`clear_audio_output_buffer` ([src/video/audio.rs:55](src/video/audio.rs:55)) は
seek 文脈で decoder が flush 直前という前提で書かれている。Norm 経路 ([src/app/
native_video.rs](src/app/native_video.rs) の `apply_normalize_gain_with_perf` 経由) は
seek_serial も engine flush も走らせないので、`raw_pending` (= 通常 5 秒分) を捨てた
直後に新しい audio frame は 5 秒先 PTS で届き、`set_audio_pts` の wall-rate cap で
master clock が追従できず、**A/V offset = −5000ms 級の永続ズレ**が残った。
toggle を繰り返すと累積で −10s, −15s, −20s と進行 (Codex 確認、2026-05-10 perf-log)。

**修正** (2026-05-11):
[src/app/native_video.rs::apply_normalize_gain_with_perf](src/app/native_video.rs)
から `clear_audio_output_buffer()` 呼出を削除。`set_normalize_gain` だけ呼んで
buffer は触らない。Codex の A' 案 (`processed` も `raw_pending` も保持) を採用。

採用理由:
- `set_normalize_gain` は atomic store だけ。buffer に触らないので audible PTS は連続。
- 既存 `processed` (~100ms 分) は旧 gain のまま鳴り続けるが、`raw_pending` 経由で
  新 gain は次の chunk から自然に反映される。100ms 程度の音量ズレは知覚しにくい。
- A/V offset は飛ばない。連続再生で永続ズレを起こさない。

却下した代替案:
- **B 案 (seek_serial bump で decoder flush)**: 1-2 秒の音飛びが発生してユーザー体感が
  かえって悪化する。
- **A 案 (`processed` だけ捨てる、`raw_pending` 保持)**: 即時反映と引き換えに 100ms
  分の音切れが残る。A' (clear なし) で十分なので不採用。

検証手順: `apply_normalize_gain_with_perf` 修正前後の `analyze_perf.py av_drift` の
A/V offset を比較。修正前は累積 −20s 級、修正後は ±数十 ms に収まること。

### 共有 atomic bundle: `AudioDiagnostics`

`src/video/audio_diagnostics.rs` に `AudioDiagnostics` 構造体を置き、`VideoPlayer::open`
で `Arc::new(AudioDiagnostics::new(Instant::now()))` を生成して以下に同じ Arc を clone
配布する:

- `audio::start(..., diagnostics.clone())` — cpal RT callback / audio pump の両方が touch
- `NativeVideoOutput::spawn(..., diagnostics.clone())` → `SwitchSourcePayload` →
  `PresenterSourceState` → `Source` (= per-source state) に通す。**fast-swap でも
  同じ Arc が引き継がれる**。

音声なし / cpal 起動失敗時は new() 直後の 0 値のまま動作 (= overlay / JSONL は分岐不要)。

### RT-safe ポリシー

⚠️ **cpal の `fill_output` callback は RT スレッド** (= JSON 構築 + writer mutex は xrun
の元)。本計装は以下のルールを厳守する:

- callback (`fill_output`) では **atomic 書き込みのみ**:
  - underrun begin/end edge に応じて `audio_underrun_active` を切替、`audio_underrun_
    begin/end_seq` を fetch_add
  - silence 累積を `audio_silence_samples_total` に fetch_add
  - 大ジャンプ (`AudioDiagnostics::should_record_pts_jump`) のときだけ
    `audio_pts_jump_*` 系を store + `audio_pts_jump_seq` を fetch_add
- JSONL emit は **audio pump スレッド**で 1Hz snapshot + edge poll する
- `clear_buffer` の `audio_out.buffer_clear` event も **MutexGuard drop 後**に emit
  (lock 中は値 copy のみ)

### perf-log イベント一覧

#### `cat = "video"`

| kind | 説明 | 主な extras |
|---|---|---|
| `av_drift` | drift sample (1Hz + `\|offset\|>30ms` の edge、edge は 100ms rate limit) | `video_pts`, `now_secs`, `drift_ms` (= video pacing), `av_offset_ms` (= 体感ズレ、null の時は audio inactive または offset 未確定), `audio_lead_ms`, `audio_active`, `big_edge` |
| `norm_apply_begin` | Norm 操作 (toggle_on / toggle_off / scan_done) の前 snapshot | `fs_idx`, `gain_db`, `reason`, `now`, `video_pts` |
| `norm_apply_end` | Norm 操作 (`set_normalize_gain` のみ、clear なし) 完了後の snapshot | `fs_idx`, `now` |

#### `cat = "audio_out"`

| kind | 発火元 | 説明 | 主な extras |
|---|---|---|---|
| `snapshot` | pump 1Hz | 直近 1 秒の underrun 状態 / silence ms / バッファ残量 | `underrun_active`, `silence_ms_last_sec`, `processed_secs`, `audio_tx_queued_secs` |
| `underrun_begin` | pump (callback edge) | silence 出力開始 (active false → true) | `edge_wall_ns`, `edge_age_ms` |
| `underrun_end` | pump (callback edge) | silence 出力終了 (active true → false) | `edge_wall_ns`, `edge_age_ms` |
| `audio_pts_jump` | pump (callback edge) | `set_audio_pts` 大ジャンプ (\|requested\|>5ms or cap 乖離) | `requested_pts`, `prev_now`, `after_now`, `requested_delta_ms`, `applied_delta_ms`, `edge_wall_ns`, `edge_age_ms` |
| `buffer_clear` | UI スレッド (`clear_audio_output_buffer`) | seek / fast-swap / shutdown 共通の汎用名。旧版では Norm でも発火していたが 2026-05-11 に削除 (= 5+ 秒 A/V offset バグの直接原因だったため) | `processed_secs_before`, `raw_pending_secs_before`, `audio_tx_queued_before`, `now_secs_at_clear` |

### Norm ボタン関連の判定

通常 seek と Norm toggle のオーディオパス比較 (= 2026-05-11 修正後):

| 経路 | seek_serial bump | engine flush | clear_audio_output_buffer |
|---|---|---|---|
| 通常 seek | ✓ | ✓ (`handle_seek_request`) | ✓ |
| Norm toggle | ✗ | ✗ | ✗ (= 2026-05-11 削除、上の「既知の症状 (修正済)」節参照) |

Norm では `set_normalize_gain` の atomic store のみ行い、`processed` / `raw_pending` /
`audio_tx_queued` のいずれも触らない。新 gain は `raw_pending` を経由した次の chunk
から自然に適用される (= 既存 `processed` の最大 ~100ms は旧 gain で鳴り続けるが、
A/V offset は連続性を保つ)。

修正前の旧仕様 (= Norm でも `clear_audio_output_buffer` を呼んでいた頃) は、
`raw_pending` 5 秒分を捨てて audio audible PTS が clock から 5 秒先行し、
`analyze_perf.py av_drift` で `norm_apply_begin → buffer_clear → underrun_begin/end →
audio_pts_jump` の連鎖と、累積的に成長する負値 `A/V offset` として観測されていた。

### P キー perf overlay 拡張

フルスクリーン再生中に P キーで開く既存の perf overlay (`src/video/native_presenter/
overlay_draw.rs::draw_native_perf_overlay`) には:

- ヘッダ 2 行目右端: `A/V {offset_ms}` (固定幅 monospace、桁ぶれなし。色: |offset|<5
  緑 / <20 黄 / >=20 赤)。audio inactive または seek 直後など offset 未確定時は
  `vid {drift_ms}` (= 旧 av_drift にフォールバック)
- ヘッダ 2 行目: `lead {audio_lead_ms}` (audio が master clock から先行している量、
  通常グレー、|lead|>=50ms で橙)。audio inactive 時は表示しない
- ヘッダ 2 行目: audio active かつ `audio_underrun_active == true` のとき赤 `UNDERRUN`
  (絵文字は使わない、CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」遵守)
- ヘッダ 1 行目: `native {fps}` は graph と同じ直近約 6 秒の visible sample から、
  `interval_ms` の平均で計算する。停止中など sample が無い時間は分母に入れない。
- グラフ rect 内: A/V offset をシアン (alpha=200)、Y 軸スケール ±200ms 中心、0ms
  ラインを点線で描画。Norm clear バグ時の `-5000ms` 級 (= 映像が音声より秒オーダーで
  遅れる方向、`offset = video − audio` で負値) は下端で saturate して「異常」のサインとして
  読める。逆向き (= 映像が音声より進む `+` 方向) も同じく上端 saturate
- グラフ rect 内: 青い折れ線は present 間隔 (`interval_ms`) のブレを表す。frame interval
  の長短だけでは赤縦線を出さない。
- グラフ rect 内: 赤縦線は `late_drop` が増えた sample のみ (= 古い frame を表示前に捨てた
  タイミング) に出す。`drop` カウンタと視覚 marker の意味を揃える。
- グラフ rect 内: audio active 時の underrun 区間に橙背景帯。audio 側の警告は赤縦線とは分ける。

### 検証手順 (修正後の正常動作確認)

1. `cargo build --release` → `target/release/mimageviewer.exe --perf-log` で起動
2. 動画フォルダで動画をフルスクリーン再生 → P キーで perf overlay
3. **シナリオ A (連続再生 5 分)**: A/V シアン線が 0ms 中心で安定、underrun 帯なし、
   ヘッダ "A/V" が緑のままなら正常
4. **シナリオ B (Norm 操作 5 回 ON/OFF)** — 修正後の期待動作:
   - A/V offset がほぼ動かない (= ±数十 ms に収まる、±5000ms 級にならない)
   - audio lead もほぼ動かない (= 0 近辺、+5000ms 級にならない)
   - **`audio_out.buffer_clear` event が出ない** (= Norm では呼ばなくなったため。
     出ているなら別経路 (seek / shutdown / fast-swap) からの clear で、Norm 起源ではない)
   - **`audio_pts_jump` event が大量に出ない** (= 5000ms 級の requested_delta が
     連続で出ているなら修正前の挙動。修正後は出ないはず)
   - underrun 帯 (橙) は短時間 (~10ms 単位) なら無害、それ以上連続するなら別問題
5. `python scripts/analyze_perf.py %APPDATA%/mimageviewer/logs/perf_events.jsonl
   av_drift [--plot]` で:
   - 主判定: **A/V offset の `|max|` が 100ms 未満であること**
   - `audio_pts_jump` の件数が低い (= 通常時の wall-rate cap 起因の小さい jump のみ。
     5000ms 級の requested_delta が出ていなければ OK)
   - `Norm 操作` 一覧と `audio_out.buffer_clear` 一覧を見比べて、Norm 直後に
     buffer_clear がペアになっていないことを確認

### 検証手順 (修正前の症状を再現する場合)

過去の perf-log と比較するときは、修正前の旧ビルドで Norm を toggle した時の動作
は以下の通り (= 2026-05-10 のログで観測した症状):

- A/V offset が toggle 毎に約 −5000ms ずつ累積 (= 最終的に −20000ms 級)
- audio_pts_jump が `requested_delta=+5128ms applied=+0.2ms [CAP]` を毎 callback で
  emit (= 1 秒間に数十〜100 件)
- `norm_apply_begin → buffer_clear → underrun_begin/end → audio_pts_jump` が時系列で連鎖
- video pacing (= 旧 `av_drift`) は 0 近辺で変化なし (= バグ検出ができない指標だった)

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
