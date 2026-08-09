# リモート動画ストリーミング 計画書

外出先のブラウザから自宅 PC の動画を視聴するために、入力ファイルを **再生器とは独立した
時計なし経路でデコードし、H.264 + AAC へ再エンコードして HLS で配信する**。本書がこの機能の正本。
本体再生の decoder/audio tap を使う実時間経路から時計なし経路へ切り替え済みであり、
production の配信 worker に旧経路や silent fallback は残さない。

- 親計画: [web-remote-plan.md](web-remote-plan.md) (リモート閲覧機能全体の正本)
- ブランチ: `web-remote` (worktree: `C:\home\mimageviewer-web`)
- 現在のフェーズ: **時計なし移行 3 段の 2 段目 (session / IPC / Web 配線済み)**。
  `clockless_transcode` がファイルを独立 open し、demux / decode / 既存 encoder / AAC /
  `Fmp4Segmenter` を時計なしで駆動する。generation、60 秒 ring、端末 timeline、seek、
  EOF flush / ENDLIST まで production 経路へ接続した (2026-08-03)

---

## 1. 方針転換の理由

[web-remote-plan.md](web-remote-plan.md) §5 は当初「動画のトランスコードは実装しない。
コンテナ非対応は remux で救い、コーデック非対応は音声フォールバックに逃がす」としていた。
この方針では次の 2 つのリスクが構造的に残る。

1. **形式が合わず見られない** — HEVC / AV1 / WMV / VC-1 / 10bit などは remux では救えず、
   音声だけになる。ライブラリの実態として非対応形式は普通に存在する
2. **回線帯域が足りず見られない** — remux は元ファイルのビットレートをそのまま流すため、
   20Mbps のファイルは 20Mbps の回線を要求する。5G / 公衆 Wi-Fi では成立しない

トランスコードはこの両方を同時に解消する。さらに mIV 固有の事情として、

- **リモート操作中は本体がロックされる** ([web-remote-plan.md](web-remote-plan.md) §2.2)。
  session / generation を 1 経路に限定でき、時計なし worker の所有権と相性が良い
- 音声も同じファイルから独立 decode できる。音量正規化 gain は metadata player から snapshot
  し、リモートセッション専用の VST3 チェーンとともに PC と同じ順序で AAC 前段へ適用する
- 解像度・ビットレートを送信側で決められるので、帯域に合わせて画質を落とせる

**したがって remux + 音声フォールバックは不要になり、本方式がそれを完全に置き換える。**

---

## 2. 確定事項

| 項目 | 決定 | 理由 |
|---|---|---|
| 取得方式 | **同じファイルを独立 open する時計なし経路** (§10.1) | 再生器・サウンドカードの 1x clock と表示 back-pressure から分離する。旧 tap worker は削除済み |
| 配信プロトコル | **HLS (fMP4 / CMAF セグメント) 一本** | iOS ネイティブ対応。tiny_http のまま配れる。追加のサーバ依存ゼロ |
| クライアント | iOS/macOS Safari は**ネイティブ再生**、それ以外は **hls.js** | サーバ実装は 1 本のまま両対応。独自 MSE 実装を書かない |
| 映像コーデック | **H.264 (High profile, 8bit, BT.709)** 固定 | 全端末が確実にハードウェアデコードできる唯一の選択肢 |
| 音声コーデック | **AAC-LC** 固定 | 同上 |
| エンコーダ | NVENC → QSV → AMF → MF → OpenH264 の**自動フォールバック** | vendor DLL に全部入っている (§3)。GPU ベンダー制約を設けない |
| 同時セッション | **1** | 既存の remote session lock と同一。2 台目は操作権ごと奪う |
| 画質選択 | **手動プリセット** (§6.4)。ABR / マルチバリアントは持たない | エンコードを 1 本に保つ。負荷と実装量の両方を抑える |
| 遅延 | **6〜10 秒を許容** | 視聴用途でありライブ配信ではない。バッファを厚く持てる方が回線変動に強い |
| 実装段階 | **最初から映像込み** (音声のみの先行段階を置かない) | |

**採らないもの**: WebRTC (ICE/DTLS/SRTP の実装コストに対し、遅延短縮の価値が用途に見合わない)、
独自 fMP4 + MSE ストリーム (hls.js で足りる)、HEVC / AV1 / VP9 (§9 の特許・互換性)、
マルチビットレート同時エンコード。

---

## 3. エンコーダ — vendor DLL の実測

`vendor/ffmpeg/bin/*.dll` (BtbN `ffmpeg-n7.1.5-10-g2aefd64d48-win64-lgpl-shared`) の
ビルド構成文字列を実際に読み出して確認した結果:

```
--enable-ffnvcodec   → h264_nvenc / hevc_nvenc     (NVIDIA)
--enable-libvpl      → h264_qsv                    (Intel QSV)
--enable-amf         → h264_amf                    (AMD)
(mediafoundation)    → h264_mf                     (Windows 標準・ベンダー非依存)
--enable-libopenh264 → libopenh264                 (ソフトウェア)
--disable-libx264 --disable-libx265                (GPL 品は無効化済み)
音声: 内蔵 aac / libopus / libmp3lame
```

`avcodec-61.dll` 内にエンコーダ名の実在も確認済み (`h264_nvenc` / `h264_qsv` / `h264_amf` /
`h264_mf` / `libopenh264`)。

**セットアップスクリプトも DLL も変更不要**。`scripts/setup-ffmpeg.sh` は現状のままでよい。

### 3.1 フォールバック階段

起動時ではなく**ストリーミングセッション開始時**に、次の順で
`avcodec_find_encoder_by_name` → 実際に `open` を試み、最初に成功したものを使う。
open 失敗 (デバイス非搭載 / ドライバ非対応 / セッション上限) は想定内として次へ進む。

| 順 | encoder | 条件 | 備考 |
|---|---|---|---|
| 1 | `h264_nvenc` | NVIDIA GPU | 最も高速・低負荷。RTX 世代なら 1080p は実時間の数十分の一 |
| 2 | `h264_qsv` | Intel iGPU/dGPU | |
| 3 | `h264_amf` | AMD GPU | |
| 4 | `h264_mf` | Windows 全般 | OS の Media Foundation。HW/SW は OS が選ぶ |
| 5 | `libopenh264` | 常に可 | 最終手段。CPU 負荷が高いので既定画質を下げる |

設定で明示指定も可能にする (`Auto` / 各エンコーダ名)。選ばれた encoder 名・実効ビットレート・
エンコード所要時間は診断ログに残す。明示指定が開けなかったときは**黙って別の段へ落ちない**。
階段を降りるのは `Auto` のときだけで、明示指定の失敗は失敗として返す。

#### 実測 (増分 1、2026-08-01、開発機 RTX 4090 / Windows 11)

vendor の 6 DLL だけを配置した状態で 5 段すべての open を実測した。

| encoder | 結果 | 入力形式 |
|---|---|---|
| `h264_nvenc` | 成功 | NV12 |
| `h264_qsv` | 成功 | NV12 |
| `h264_amf` | 失敗 (`amfrt64.dll failed to open`) | — |
| `h264_mf` | 成功 | NV12 |
| `libopenh264` | 成功 | YUV420P |

- **`libopenh264` は追加 DLL なしで開けた。** 最終段は実在するので、この表の変更は不要
- `h264_amf` の失敗は AMD GPU 非搭載の想定内。階段はこの段を飛ばして次へ進む
- 再現手段:
  `cargo test -p mimageviewer --lib probe_local_h264_encoder_open_results -- --ignored --nocapture`
  (通常の test suite は hardware / DLL に依存しない)

#### 後続増分への申し送り

- **m3u8 の `CODECS` 属性と CMAF init segment は、encoder が実際に出した SPS
  (extradata) から導くこと。** 想定の High profile を定数から書かない。
  `libopenh264` の encoder は Constrained Baseline しか出さないため、最終段まで
  落ちたときに宣言と中身がずれる。互換性の向きとしては安全側 (Baseline の方が広く
  再生できる) だが、宣言が実体と違うと iOS のネイティブ経路で弾かれ得る
- `src/video/stream/mod.rs` の暫定 `#![allow(dead_code)]` は、増分 6 で IPC から
  セッションへ接続した時点で外した

x264 は使わない (GPLv2。mIV は MIT なので持ち込めない)。OBS が x264 を同梱できるのは
OBS 自身が GPLv2 だからであり、**OBS のコードも設定値の写経以上の流用はしない** (§9)。

---

## 4. パイプライン設計

```
       ┌────────────────── mimageviewer-core.exe ──────────────────┐
       │                                                           │
 file ─┼─▶ clockless demux/decode ─┬─▶ scale ─▶ H.264 encoder ───┐ │
       │                           └─▶ normalize ─▶ VST3 ─▶ limiter ─▶ AAC ─┤ │
       │                                                        ▼ │
       │                              fMP4 segmenter + 60 秒 ring │
       │                                                           │
       │  paused headless VideoPlayer: metadata / resume / thumbnail│
       └────────────────────────────┬──────────────────────────────┘
                                  │ IPC (pull 型)
       ┌──────────────────────────▼────────────────────────────┐
       │  mimageviewer-remote.exe : m3u8 / init.mp4 / seg.m4s   │
       └──────────────────────────┬────────────────────────────┘
                                  │ HTTP (tailscale serve 経由)
                          ブラウザ (<video> or hls.js)
```

remote session が所有する headless `VideoPlayer` は pause のまま metadata、resume origin、
音量正規化 gain、seek thumbnail を提供する。配信 frame と transport clock は一切供給せず、
generation worker が同じファイルを独立 open する。

### 4.1 時計なし音声 — normalize → VST3 → safety limiter

production の時計なし worker は decoded PCM に `VideoPlayer::normalize_gain()` の確定値を
固定 gain として掛け、リモートセッション専用 `DspBridge` の active plugin、既存
`SafetyLimiter` の順に通してから AAC encoder へ渡す。これは PC の
**time stretch → normalize gain → VST3 `process_block` → safety limiter** から時計依存の
time stretch だけを除いた順序である。VST の sample rate に PCM を resample し、VST PDC と
limiter lookahead は AAC へ渡す `audible_pts_secs` から差し引く。

paused headless player は引き続き metadata / thumbnail 専用なので `DspBridge` を渡さない。
VST は generation ではなく streaming session が所有する別ホストで処理する。chain load は
generation worker 内で一度だけ行い、全世代が同じ `Arc` を共有する (§10.2)。ロード全体は start
の 15 秒予算の残りから encoder/playlist 用 3 秒を予約した値（上限 10 秒）で打ち切り、
全失敗または process 失敗時は normalized dry で
動画配信を継続する。ただし IPC/Web の VST 状態と本体のリモート接続 modal に警告を表示し、
黙った pass-through にはしない。

以下の tap 設計は移行元の記録であり、production 配信には使わない。

[src/video/audio.rs](../src/video/audio.rs) の audio pump は
**time stretch → normalize gain → VST3 `process_block` → safety limiter** の順に処理した
最終サンプルを `ProcessedChunk` にまとめ、`buf.processed.push_back(chunk)` で
出力キューへ積む (現行 `audio.rs:1369-1408`)。

**この push 直前が唯一の tap 点**。`ProcessedChunk` は次を持つため、そのままエンコーダへ渡せる。

| フィールド | 用途 |
|---|---|
| `samples` (f32 interleaved) | AAC エンコーダ入力 (要 f32→fltp 変換とサンプルレート整合) |
| `audible_pts_secs` | **A/V 同期の基準**。PDC (プラグイン遅延) 補正済みの source timeline |
| `duration_secs` | セグメント長の計算 |
| `seek_serial` | シーク世代。世代が変わった chunk は破棄する |
| `source_secs_per_output_sec` | 変速判定。第 1 段は 1.0 以外を明示的に拒否する |

tap は `Option<Sender<ProcessedChunk>>` を pump に渡す形とし、**tap が無いときは
現行コードと完全に同一の経路を通る**こと。pump は realtime 制約下にあるので、
tap の送信は非ブロッキング (満杯なら落として `dropped` を計上) とする。
tap 側の詰まりが PC 側の音を絶対に途切れさせない、が不変条件。

#### 増分 3 実装記録 (audio tap、2026-08-01)

- `AudioOutput` が `AudioTapController` を所有し、将来の streaming session が bounded
  receiver と owner-id 付き `AudioTapLease` を所有する。古い lease の Drop は新 owner を
  detach しない。増分 3 では接続者を置かない
- production の payload 分岐は `buf.processed.push_back(chunk)` の直前 1 箇所だけ。
  command / payload とも `try_*` だけを使い、満杯は `dropped` を加算して PC 再生を優先する
- 未接続時は samples に clone / 再確保 / 書き換えを行わず、元 `Vec` の pointer・capacity と
  全 sample bit が push 後も一致する unit test で固定した。接続時だけ PC queue と worker の
  独立所有に必要な `Vec<f32>` 1 clone を行うが、cpal と共有する mutex の外で実行する。
  満杯が分かっている場合は clone 自体を行わない

### 4.2 映像 tap

映像は presenter ではなく**デコーダ出力から分岐する**。

- presenter のバックバッファには HUD オーバーレイが合成済みで、解像度も PC 画面依存。
  スマホへ送る絵としては不適
- decoder tap なら、PC 側ウィンドウの表示状態と独立して絵が取れる (§10-1 参照)

HW デコード時 (`AV_PIX_FMT_D3D11`) は既存の `av_hwframe_transfer_data`
([src/video/decoder.rs](../src/video/decoder.rs) に前例あり) で SW frame へ落とし、
`swscale` でストリーム解像度・NV12 へ変換してエンコーダへ渡す。
既存の [src/video/swscale_helpers.rs](../src/video/swscale_helpers.rs) の
`prepare_frame_for_swscale` を使い、HW frame をそのまま swscale へ渡して
`av_assert0` を踏む既知の罠を避ける。

720p30 で readback + 変換の帯域は約 40MB/s であり、PCIe と CPU の両方で余裕がある。

**映像補正 (v2.9.0 の補正スロット) と VSR / AI アップスケールは第 1 段では反映しない。**
これらは presenter の D3D11 パス
([src/video/native_presenter/grade_pipeline.rs](../src/video/native_presenter/grade_pipeline.rs))
にあり、リモート用に別レンダーターゲットを回す実装が必要になる。§11 の第 2 段で扱う。

#### 増分 4 実装記録 (映像 tap、2026-08-01)

- tap 点は `run_video_decode` が decoded PTS / seek serial / preroll を確定した後、D3D11
  GPU blit と CPU readback の分岐へ入る直前の 1 箇所。ここは (1) D3D11VA + GPU blit の
  `VideoFrameData::Gpu`、(2) D3D11VA + deinterlace / 通常 GPU blit 失敗による CPU fallback
  の `VideoFrameData::Cpu`、(3) HW format 拒否後を含む SW decode の
  `VideoFrameData::Cpu` の共通祖先であり、2 箇所の `video_tx.try_send` を両方覆う
- decoder thread は bounded tap queue の空きを先に確認する。D3D11 frame は空きがある時だけ
  producer 呼び出し中に `prepare_frame_for_swscale` で即時 readback し、独立した SW frame を
  `try_send` する。SW frame だけは `av_frame_clone` の浅い参照を送る。満杯なら readback / clone
  とも行わず `dropped` を加算し、未接続時も AVFrame clone / HW readback / swscale / encoder
  処理を一切行わない
- queue payload は private な `SoftwareTapFrame` を必須とし、D3D11 / `Pixel::None` を構築時に
  拒否する。したがって `attach(software_frame_capacity)` の容量や worker の遅延にかかわらず、
  queue が保持する decoder-pool HW surface は 0 枚。同期 readback 中の現 source 1 枚だけが上限で、
  `VIDEO_TAP_MAX_QUEUED_DECODER_HW_SURFACES = 0` と
  `VIDEO_TAP_MAX_SYNCHRONOUS_DECODER_HW_SURFACES = 1` を増分 5 の呼び出し側へ公開する
- `VideoStreamEncoder` は将来の streaming session worker が所有する受信側部品。
  queue から受けた SW frame を `QualityPreset::output_parameters` の非拡大・比率維持・偶数寸法へ
  scale する。出力 pixel format は選択済み encoder の `input_format` (NV12 / YUV420P) を使う
- 映像 PTS も音声と同じ `StreamTimeline` で session 相対時刻へ写す。VFR は
  `round(relative_source_secs * fps_num / fps_den)` で最寄り CFR slot を選ぶ。同じ slot に
  入る複数 frame は後着を coalesce し、tap drop や source gap の後続 frame は slot を
  詰め直さない。これにより forced-IDR の CFR source timeline 上の位置を負荷と独立に保つ
- 増分 4 では controller / owner-id 付き lease / receiver と worker 側変換・encoder 部品までを
  用意し、実 session はまだ attach しない。packet の A/V 順序付けと session lifecycle は増分 5

### 4.3 A/V 同期

- 音声: `ProcessedChunk::audible_pts_secs` (source timeline、PDC 補正済み)
- 映像: frame の pts (source timeline)

両方を **source timeline のまま**セグメンタへ渡し、セッション開始時刻を 0 とする相対
タイムスタンプへ写す。音声側が既に PDC を吸収しているので、リップシンクは自動的に合う。

レジューム位置で session を attach すると、最初の post-DSP 音声区間は PDC 分だけ session
原点より前から始まり得る。AAC 入力境界は現 seek 世代の初期 chunk を点ではなく
`[audible_pts, audible_pts + duration)` の区間として扱い、全体が原点前なら捨てて件数を数え、
原点を跨ぐ場合だけ先頭を sample 単位で削る。保持した最初の sample は実際の source PTS を
維持し、映像と同じ `StreamTimeline` へ渡すため、別の音声原点や timestamp clamp は導入しない。
`StreamTimeline` は意味のある原点前 timestamp を従来どおり error にし、診断には source PTS、
session 原点、差分秒を含める。1ns の既存浮動小数点許容だけは共通定数として維持する。
初期 chunk の先行量は `pdc_latency_secs_at_process + 1 chunk の source duration` を上限とする。
これは宣言済み DSP delay と sample 境界の余裕だけを許す値で、超過時は silent drop を続けず、
audible PTS、session 原点、先行量、PDC、chunk duration、許容量、超過量を持つ worker error にする。

変速再生 (`playback_speed != 1.0`) は第 1 段では**非対応**とし、ストリーミング中は等速に
固定する (`source_secs_per_output_sec` の扱いが増えるため。§12)。

#### 増分 3 実装記録 (音声 timestamp、2026-08-01)

- `StreamTimeline` を音声・映像共通の写像 owner とし、session start の source PTS を
  0 とする。増分 3 の AAC input PTS は `audible_pts_secs` をこの型で sample tick へ写し、
  増分 4 の映像も同じ型を使う
- `audible_pts_secs = input_pts - (DSP latency output 秒 × source/output 比)` と、
  `pdc_latency_secs_at_process` に入る source 秒を同じ純関数から返すようにし、等速 / 2x /
  0 秒 clamp を unit test で固定した。既存計算と一致し、異常は見つからなかった
- `seek_serial` が session の期待世代と違う chunk は PCM assembler より前で捨てる。
  `source_secs_per_output_sec != 1.0` は黙って連続音声として扱わず error にして session 側へ返す
- 実機検証是正 (2026-08-02): session attach 時の現在位置と PDC 補正済み先頭音声区間のずれを
  AAC encoder の初期区間 trim で吸収した。完全な pre-session chunk の drop と、原点を跨ぐ
  chunk の sample trim は PCM assembler より前で行い、その後の chunk 連続性検証は維持する。
  drop の許容は各 chunk が宣言する PDC + 1 chunk までとし、超過は数値付き `Failed` にする

### 4.4 セグメンタ

- **fMP4 (CMAF)**: init segment (`ftyp` + `moov`) 1 個 + media segment (`moof` + `mdat`) 列
- セグメント長の目標は **2 秒**、GOP も 2 秒 (`keyint = fps * 2`, `scenecut` 無効) とし、
  各セグメントを必ず IDR から始める。tap / encoder の frame skip で予定境界の IDR が
  欠けた場合は停止せず、次の IDR まで現在セグメントを延長する
- generation の最初のセグメントだけ 0.5 秒境界も forced-IDR 候補にし、実際に閉じた
  境界から後続の 2 秒 GOP を再開する。エンコーダが短い IDR 指定を無視した場合は元の
  2 秒境界を保険として残し、壊さず従来の初回 2 秒へ戻る
- avformat の mp4 muxer を `movflags=frag_custom+delay_moov+default_base_is_moof+cmaf` 相当で
  使い、出力は `avio_alloc_context` によるメモリ書き出しにする。
  raw FFI は `ffmpeg_the_third::ffi::` 経由で既に本体各所で使っており、新しい依存にはならない
- 生成済みセグメントは**メモリ上のリングバッファ**に保持する。既定 30 本 = 60 秒
  (標準画質で約 12MB)。ディスクには書かない
- m3u8 はライブ (`#EXT-X-PLAYLIST-TYPE` を出さない) とし、`#EXT-X-MEDIA-SEQUENCE` を進める。
  `#EXTINF` は packet DTS から得た実時間、`#EXT-X-TARGETDURATION` は保持中セグメントの
  観測最大時間の切り上げ (最低 2 秒) とする

#### 増分 2 実装記録 (2026-08-01)

- src/video/stream/segmenter.rs: custom write AVIO + mp4 muxer。FFmpeg の実オプション名
  default_base_moof が ISO BMFF の default-base-is-moof flag に対応するため、
  frag_custom+empty_moov+default_base_moof+cmaf を指定する。header 書き込み直後を
  init (ftyp+moov)、av_write_frame(ctx, NULL) の明示 fragment flush ごとの出力を
  media (moof+mdat) として切り出す。ディスク I/O は持たない
- 2 秒境界 frame は segmenter の prepare_video_frame が I frame 指定し、encoder の
  forced-IDR 設定と組み合わせる。引数は submitted frame の連番ではなく、drop された
  frame も位置を詰めない `CfrTimelineFrameIndex` とする。受け取った各 fragment 先頭
  packet も key + IDR NAL であることを検証してから mux する
- 予定境界の IDR が欠けた場合は次の IDR まで fragment を延長する。発生回数は
  `Fmp4SegmenterStats::delayed_idr_boundaries` と通常ログに残し、延長後の実時間を
  `#EXTINF` / `#EXT-X-TARGETDURATION` に反映する
- src/video/stream/playlist.rs: 既定 30 本の ring。先頭要素の sequence を
  EXT-X-MEDIA-SEQUENCE の唯一の source of truth とし、先頭より古い要求を Gone、
  未生成を NotFound として型で分離する
- HLS の CODECS は media playlist ではなく Master Playlist の
  EXT-X-STREAM-INF 属性なので、index.m3u8 (master) → media.m3u8 (live media)
  の 2 層とする。avc1.PPCCLL は encoder extradata の SPS にある
  profile_idc / constraint flags / level_idc から生成する。libopenh264 の in-process
  fixture (320x180/30fps) での実測は avc1.42c00d (Constrained Baseline, Level 1.3)。
  level は解像度・fps 等で変わるため定数化しない

#### 増分 3 実装記録 (AAC + 2-stream mux、2026-08-01)

- `src/video/stream/audio_encoder.rs`: 増分 1 の `AUDIO_ENCODER_NAME` / `AUDIO_PROFILE_ID` と
  preset の `audio_bitrate_bps` を使って AAC-LC を open する。encoder の対応 sample rate
  に pump rate があれば同率のまま manual deinterleave、無ければ最寄りの対応 rate へ
  swresample しながら fltp 化する。50kHz → 48kHz fixture で EOF delay まで 50,000 →
  48,000 samples を回収することを固定した
- chunk 境界は `PlanarPcmAssembler` に蓄積し、AAC 固定 1024 samples ごとにだけ frame を
  送る。333 / 901 / 17 samples 等の不一致境界を跨いで、padding を除く全 sample が欠落・
  重複なしで復元できる unit test を置いた
- segmenter は video stream 0 + audio stream 1 を同じ fMP4 へ多重化する。audio-only では
  AAC を stream 0 とし、同じ muxer / ring / playlist 実装が AAC DTS の 2 秒境界で fragment を
  確定する。master playlist の
  `CODECS` は video SPS と AAC AudioSpecificConfig の実出力から
  `avc1.PPCCLL,mp4a.40.2` を作り、bandwidth は両 encoder の実効 bitrate の和とする
- AAC encoder の先頭 priming packet は DTS -1024 なので、増分 2 の `empty_moov` は
  `delay_moov` へ置き換えた。最初の custom flush で確定する `ftyp+moov` を最初の
  `moof+mdat` から top-level box 単位で分離する。したがって init segment は最初の media
  segment 完成と同時に利用可能になる
- init の状態は `InitSegmentState::Pending { muxer_prefix } / Ready(Vec<u8>)` に集約し、
  `init_segment()` は `Option<&[u8]>` を返す。空 bytes を未確定 sentinel として扱わない。
  最初の media segment の ring 追加が成功してから `Ready` へ遷移するため、外部から
  init だけ、または media だけが先に見える状態は作らない
- segmenter の master / media playlist accessor も同じ readiness で `Option<String>` を返す。
  init 未確定中は両方とも `None`、`Some` なら init と最初の media segment を取得できることを
  増分 3 が保証する。増分 6 はこの `None` を **200 + 空 body に変換してはならない**。
  HTTP 503 を返すか、上限付きで ready を待ってから 200 を返すかは HTTP / IPC 層で決める
- fragment 確定前に `av_interleaved_write_frame(ctx, NULL)` で interleave queue を drain
  してから `av_write_frame(ctx, NULL)` を呼ぶ。さらに、すでに muxer へ渡した audio packet
  の末尾が video boundary を覆うことを確認し、flush 済み境界より古い audio packet の
  後着を error にする。libopenh264 + AAC の 4 秒 fixture を同じ FFmpeg avformat で
  init+各 media segment として読み返し、全 AAC packet の個数と PTS 列が入力と一致する
  ことを固定した

##### 増分 5 で解消済みの申し送り

- 作りかけ fragment と未 mux interleave queue は source-timeline **6.0 秒**を上限とした。
  超過時は generation の自動再試行ではなく streaming session を停止する。詳細は
  §6.4 の増分 5 実装記録を参照
- `CfrTimelineFrameIndex` は「落ちた frame も欠番として残す CFR source timeline 上の
  位置」である。**増分 4 の映像 tap は、投入できた frame で番号を詰め直してはいけない**
  (詰め直すと forced-IDR の位置が encoder の負荷次第でずれる)

#### 実機待ち時間是正 B: 初回セグメント短縮は撤回 (2026-08-03)

初回だけ 0.5 秒で閉じる案は実装後に `212933fc` で revert した。実時間生成では短縮して
先に渡した分が直後の不足として残り、総待ち時間と回線揺れ耐性を改善できないためである。
セグメント境界は従来の 2 秒を維持し、時計なし先行生成によって端末を生成先端から離す。

---

## 5. 配信プロトコルとクライアント

### 5.1 なぜ HLS 一本か

| 方式 | iOS Safari | Android Chrome | PC ブラウザ | サーバ依存 |
|---|---|---|---|---|
| **HLS (fMP4)** | **ネイティブ** | hls.js | hls.js | なし (tiny_http) |
| 独自 fMP4 + MSE | iOS 17.1+ の ManagedMediaSource のみ | MSE | MSE | なし |
| WebRTC | 対応 | 対応 | 対応 | ICE/DTLS/SRTP |

実機検証できるのは iPhone / iPad のみである一方、Android 利用者も想定する。
**HLS ならサーバ実装は 1 本のまま、iOS はネイティブ再生 (最も検証済みの経路) で動き、
Android / PC は hls.js という広く使われた実装に委ねられる**。検証できない環境のリスクを
自前実装で抱え込まない。

### 5.2 クライアント分岐

```js
const nativeHls = video.canPlayType('application/vnd.apple.mpegurl');
const managedMse = typeof ManagedMediaSource === 'function';
const mse = typeof MediaSource === 'function' || managedMse;
if (managedMse && nativeHls) {
    video.src = playlistUrl;          // iOS / iPadOS (native HLS)
} else if (mse && (await loadHlsJs())?.isSupported()) {
    const hls = new Hls({ ... });     // Android Chrome / PC
    hls.loadSource(playlistUrl);
    hls.attachMedia(video);
} else if (nativeHls) {
    video.src = playlistUrl;          // MSE が無い Safari
} else {
    showUnsupportedPlayback();
}
```

- hls.js は **Apache-2.0**。`dist/hls.min.js` 1 ファイルを `crates/remote-web/web/vendor/` へ
  置いて静的配信する。バンドラも TypeScript も導入しない
  ([web-remote-plan.md](web-remote-plan.md) §3.4 の「ビルドステップを導入しない」を維持)
- `ManagedMediaSource` と native HLS の両方がある WebKit では native HLS を選ぶ。通常の
  `MediaSource` だけを持つ Chrome / Firefox は、native HLS を再生できなくても `canPlayType` が
  `maybe` を返すことがあるため hls.js を第一候補にする。どちらも無ければ明示的な再生非対応とする
- **MSE を mIV が実装するわけではない。** MSE はブラウザ側の API であり、それを駆動するのは
  hls.js である。mIV 側の成果物は上記の分岐と hls.js の静的配信だけで、サーバの HLS 出力は
  iOS 向けとまったく同一のものを使う
- **hls.js 経路は Android 専用ではない。** PC の Chrome / Edge / Firefox からのリモート視聴も
  同じ経路を通るため、開発中の主な検証環境はこちらになる (§13.1)
- `<video>` のネイティブコントロールは使わない (`controls` を付けない)。
  再生・一時停止・シーク・音量はすべて既存のコマンド層 ([web-remote-plan.md](web-remote-plan.md)
  §6.5.6) を経由して本体へ送る

### 5.3 シーク

HLS のライブウィンドウ (直近 60 秒) 内は `<video>` 上で完結するが、**UI 上のシークバーは
常に動画全体を表す**。ウィンドウ外へのシークは次の流れになる。

1. Web → `POST /api/video/seek` → IPC。端末の media element が持つ全体位置を送る
2. 本体は旧 worker を段境界で停止し、入力ファイルを指定位置から独立 open/seek する
   **新しい session generation** を発行する
3. m3u8 の URL に generation を含めるため、クライアントは `video.src` を差し替える
   (hls.js 側は `loadSource` をやり直す)
4. 新 generation は実時間 clock を待たず、ring 上限まで先行生成する

シークバーのドラッグ開始から、中央表示は `シーク中` (ドラッグ中は `移動先を確認中`)
→実際にデコードできた seek thumbnail→再生、の順で遷移する。thumbnail は既存
`VideoPlayer` の latest-wins `ThumbnailWorker` を使い、応答の実 frame PTS が現在の要求位置に
合う場合だけ表示する。新しいドラッグ位置は古い取得を中止して置き換える。`playing` が先に
到着した場合は thumbnail を待たずに破棄し、配信 readiness や generation switch の条件には
thumbnail を含めない。

#### 実機仕上げ是正 (generation 起点 / 終端 / gesture、2026-08-03)

- generation の transcode seek は backward keyframe seek 後、映像は要求 origin 未満の PTS を
  decode-and-discard し、音声は origin を跨ぐ packet だけ trim する。実機 HTTP ログで先頭取得が
  `1.m4s` / `2.m4s` から始まった世代があり、2 / 4 秒ずれは decoder の着地ではなく live edge を
  選んだ端末側の開始位置だった。media playlist は `EXT-X-START:TIME-OFFSET=0,PRECISE=YES` を
  宣言し、hls.js にも `startPosition: 0` と segment boundary 開始を明示して generation 先頭を選ぶ
- seek bar はドラッグ終了後も `VideoSeekPreviewOwner` の要求位置を表示の正本とする。
  新 generation の `playing` / `loadeddata` で playback 所有へ戻るまでは、旧 media element の
  `currentTime` で range / counter を巻き戻さない。±10 秒の連続入力も同じ owner に載せ、
  実位置ではなく直前の要求位置へ増分を累積する。要求失敗時は request revision が一致する
  preview だけを playback 所有へ戻す。着地時も request revision と実際に attach 済みの
  generation が一致する場合だけ解除し、古い世代の失敗・再生開始で新しい連打位置を消さない
- 動画 seek の native range は keyboard / accessibility の正本として残し、pointer 操作だけを
  44px 高の全幅 hit area と pointer capture で所有する。画像・動画とも、pointer-down からの最大
  2 次元距離が 6 CSS px 未満なら tap として押下位置へ absolute seek し、6 px 以上なら押下時の
  表示位置へ `deltaX / track width * range` を加える relative drag とする。いったん閾値を越えた操作は
  指が戻っても drag のままとし、離した位置への飛びを起こさない。画像 viewer は離散 page-group と
  RTL の向きを維持し、判定と正規化された位置計算だけを動画と共有する。preview は移動中、seek command は
  pointer-up の 1 回だけ発行する。設定値を選ぶ通常の slider はこの seek 固有の tap/drag owner の対象外とする
- 先読み窓を埋めた有限素材では、次の frame/chunk の capacity 待ちが demux の EOF 観測より先に
  起き、未公開の終端を端末が取得して release することもできない循環があった。advertised target
  の外に公開可能な working fragment 1 本を有界に許可し、ring は target + working + terminal の
  2 予約 slot を持つ。working fragment が公開されれば通常の取得で再開でき、EOF 観測後の codec
  drain は従来どおり terminal slot で完了する
- 動画 surface は静止画の transform owner を通らず、tap zone が再生 / ±10 秒 command を所有する。
  動画に拡大状態は設けない。連続 tap と pinch は Safari の native page zoom を明示的に抑止し、
  静止画 viewer の拡大・reset 操作には影響させない
- 動画の終端規則は端末設定を別に持たず、本体の `video_continuous_mode` と
  `video_loop_mode` を stream start 時に解決して `end_behavior` として渡す。連続再生は表示順の
  次動画へ進み末尾で停止、連続ループは末尾から先頭へ戻る。連続再生 OFF ではループ OFF は停止、
  Full は 0 秒、Chapter / Bookmark は現在区間の開始へ戻る。区間データが無い Chapter /
  Bookmark は本体と同じく Full へ降格する。スライドショーの終端動作は静止画専用で動画には
  適用しない。次動画は Web の既存 `renderVideoViewer` → `VideoStreamViewer.start` 経路で開く
- clockless worker は generation 開始 (origin / 先読み目標)、最初の映像 PTS、EOF から Finishing
  への遷移、先読み満杯の park / resume、最終 fragment 公開と ended、停止理由
  (complete / cancel / error) を `remote-stream clockless ...` の通常ログへ残す

`#EXT-X-DISCONTINUITY` による同一プレイリスト継続は、iOS のネイティブ実装との相性を
確認できるまで採らない。URL を切り替える方が確実で、実装も単純。

---

## 6. API

### 6.1 HTTP (remote-web、すべて認証必須)

| エンドポイント | 内容 |
|---|---|
| `POST /api/video/start` | `{fav, path, quality}` → `{session, generation, playlist, duration_secs, source_origin_secs, buffer_target_secs, codec, encoder, end_behavior}` |
| `POST /api/video/control` | `{session, action: play\|pause\|volume\|quality}`。quality は端末の `position_secs` も送る |
| `POST /api/video/seek` | `{session, position_secs}` → 新 `generation` と `playlist` |
| `POST /api/video/thumbnail` | `{session, position_secs}`。実 frame PTS 付き WebP、生成中は 202、`null` は要求解除 |
| `GET /api/video/state` | generation、source origin、生成済み/ring 範囲、尺、先読み目標、実効ビットレート、終端、再生 intent。実 playhead は返さない |
| `POST /api/video/stop` | セッション終了。本体はストリーミングを止める |
| `GET /stream/<session>/<gen>/index.m3u8` | CODECS を宣言する Master Playlist |
| `GET /stream/<session>/<gen>/media.m3u8` | MEDIA-SEQUENCE を持つ live Media Playlist |
| `GET /stream/<session>/<gen>/init.mp4` | init segment |
| `GET /stream/<session>/<gen>/<n>.m4s` | media segment |

- `/stream/` 配下も**認証必須**。同一オリジンなので Cookie は `<video>` / hls.js の
  どちらからも送られる
- セグメントは `Cache-Control: no-store`、init segment だけ `immutable`
- 未生成 / 存在しないセグメントは 404、ring から巻き取られたセグメントは 410 Gone、
  session / generation 不一致はどちらも 409 とするが、JSON の `error` をそれぞれ
  `stream_session_mismatch` / `stream_generation_mismatch` に分ける。本体未接続は 503 と
  既存の `miv_not_running` を返す。動画 API の JSON `error` は計測ログの
  `details.video_stream.error_code` にも記録する
- 利用者へ返す `message` は再試行・再オープンなどの一般向け案内だけとし、start の段、
  deadline、player / seek / encoder / playlist、内部状態は含めない。本体から返った詳細は
  `details.video_stream.internal_message` と本体ログに残し、安定した JSON `error` は従来どおり
  機械判定と計測へ使う
- `POST /api/video/stop` は generation を受け取らず、指定 streaming session が無い場合も
  成功する冪等操作とする。別の有効 session ID を誤って停止しない
- generation 開始直後は init と master / media playlist がまだ未確定になり得る。増分 6 は
  segmenter の typed `None` を保持して HTTP 503 または上限付き wait へ写像し、200 では
  必ず非空の init、または init と最初の media segment を参照できる playlist を返す

### 6.2 IPC (動画 API v15、timeout v17、thumbnail v18、時計なし v19、終端 v20、VST 状態 v21、audio-only v39)

既存の長寿命 duplex 多重化接続 ([web-remote-plan.md](web-remote-plan.md) §9.5-9.6) に
`ClientMessage` / `ServerMessage` の variant を追加する。**セグメントは pull 型**とし、
remote-web が HTTP 要求を受けた時に取りに行く。push 型の非同期メッセージを新設しない
(現行プロトコルの request/response 構造をそのまま使えるため)。

| request | response |
|---|---|
| `VideoStreamStart { address, quality }` | `{ session, generation, duration_secs, source_origin_secs, buffer_target_secs, has_video, encoder, video_size, audio_processing, end_behavior }` |
| `VideoStreamControl { session, action }` | `SessionStatus` |
| `VideoStreamSeek { session, position_secs }` | `{ generation }` |
| `VideoStreamThumbnail { session, position_secs }` | `Pending` / 実 frame PTS + WebP / `Cleared` |
| `VideoStreamPlaylist { session, generation, kind }` | master / media m3u8 本文 |
| `VideoStreamSegment { session, generation, index }` | セグメントのバイト列 / `NotFound` / `Gone` |
| `VideoStreamState { session }` | generation/source origin、生成済み/ring 範囲、先読み目標、終端、再生 intent、バッファ/ビットレート実績、最新の `audio_processing` |
| `VideoStreamStop { session }` | — |

セグメント IPC は既存の **heavy queue ではなく専用 lane** に置く。エンコード済みバイトを
返すだけで CPU をほぼ使わないため、サムネイル生成やページレンダリングと同じ枠で待たせると
再生が途切れる。`address` は既存の `RemoteAddress` をそのまま使い、本体側で
お気に入りへの登録有無とは独立に、絶対パス・実在・対象種別を再検証して canonicalize する
([web-remote-plan.md](web-remote-plan.md) §12.1)。

`VideoStreamStart` の `address` は照合用ではなく、再生対象を指定する正本である。本体 UI
thread は headless `VideoPlayer` を開き、metadata、duration、pending resume、normalize gain が
確定するまで typed `Opening` state を poll する。player は pause のまま transport には使わず、
typed `Starting` が同じファイルを独立 open する時計なし worker の encoder と最初の playlist
readiness を確認してから generation を publish する。

`Opening` の門は `RemoteStreamStartInputs` (duration、video/audio track、source origin、
normalize gain) の確定である。`pending_resume_secs` は metadata から duration を得た後に
末尾 guard を含む正規化を行ってから消費され、採用した seek target は `request_seek` が
`position_secs()` へ同期的に公開する。このため `pending_resume_secs == None` になった時点で
source origin は確定している。pause のままでは frame/audio を再生消費しないため
`clock.is_seeking()` が残り得るが、これは metadata player の transport 状態であって generation
入力ではなく、門には含めない。normalize gain は player open 前の DB lookup で決まり、
remote player は autoplay=false のため deferred normalize scan を開始しない。門を通る際に
これらを同じ typed snapshot へ固定し、generation は player の後続 transport 状態を再読しない。
音声 track は必須、timed video track は任意とする。後者の有無だけで video decode / H.264 encode を
有効化し、seek、generation、playlist、segment、session owner の実装は分けない。

start の wall-clock 予算は `mimageviewer_ipc::VIDEO_STREAM_START_BUDGET` の **15 秒だけ**を
正本とする。本体が IPC request を stream queue へ積んだ時刻から deadline を一度だけ作り、
UI 受付、player と start input snapshot、encoder、最初の non-empty master playlist
まで同じ typed budget を移送する。各段は独自 timeout を開始せず、残時間だけを使う。
別の項目が開いていても remote owner が操作権を占有しているため要求先へ切り替える。
stop・owner 解放・失敗時は remote が開いた動画をその位置で pause して残し、開始前の項目へは
戻さない。復元用の並行 state を持つと §2.2 の「位置を保持して手動再開」と競合するためである。
player open と `fs_cache` / `fullscreen_idx` の更新だけを UI thread が担当し、encoder open と
stream worker の teardown / join は引き続き UI thread 外で行う。

`PROTOCOL_VERSION` 20 の `VideoStreamEndBehavior` は `Stop`、境界開始秒列を持つ `Loop`、
`wrap` を持つ `Next` の typed union とする。本体がローカル再生と同じ helper / 設定から決め、
remote-web は判断材料となる別設定を保存しない。Chapter / Bookmark の全境界を渡すのは、同じ
stream session 内で seek した後も端末が現在区間を選び直せるようにするためである。

`PROTOCOL_VERSION` 21 は start / state に `VideoStreamAudioProcessing` を追加する。
`vst3_requested`、`vst3_active`、active slot 数、利用者向け warning を持ち、ロード後だけでなく
処理中の dry fallback も state poll で端末へ伝える。端末は warning を notice と診断行へ残し、
本体側も同じ status owner をリモート接続 modal に表示する。

#### 増分 6 実装記録 (IPC + HTTP、2026-08-02)

- `PROTOCOL_VERSION` は 14 から **15** へ更新した。動画操作、playlist、segment、state、
  stop は既存の request/response 多重化へ追加し、segment を含め push 通知は追加していない
- 本体は容量 32 / 4 worker の `stream` queue、remote-web は 4 枠の stream admission を持つ。
  既存の IPC 合計 6 / heavy 4 には数えず、HTTP worker 12 のうち常に 2 枠以上を IPC 外要求へ
  残す。stream 飽和中も heavy (thumbnail) と Home、および remote-web の `/api/list` が
  進む回帰テストで固定する
- `/api/video/*` と `/stream/*` は PWA shell asset の後にある fail-closed 認証 guard の
  **下**へ配置した。全新規 route の未認証要求が path/body/IPC 処理より先に 401 となる
  route-level test を置いた
- generation 開始直後の playlist/init `None` は HTTP **503** + `Retry-After: 1` とする。
  playlist/init/segment は空の成功 body を防御的にも拒否する。増分 6 時点では encoder readiness
  だけに根拠未記録の 7 秒 / 503 を置いたが、実機 timeout 是正で撤去し、後述の単一 15 秒 budget と
  stage 別 504 に置き換えた
- remote-web と本体の両方で `RemoteAddress` の favorite allowlist、実ファイル、
  canonical containment、対応動画拡張子を独立に検証する。seek / 画質変更後の旧 generation
  は新しい resource を返さず 409 とする
- API seek は通常 UI の連打 coalescing を通さず decoder seek serial を即時に進めてから
  generation を発行する。これにより応答した generation と実際の seek を 1 対 1 に保つ

#### 実機検証是正 (start/409/stop、2026-08-02)

- start が generation access を clone した後、UI poll が opening/resume seek を検出して
  current generation を交換しても、旧 clone の encoder readiness 待ちだけが成功して旧番号を
  応答していた。`Opening → Starting → Streaming` に分け、seek と generation が一致するまで
  claimed request を UI state に残すことで、返却境界を current generation の publish 後へ移した
- stop は ownership 判定後の通常 control と分け、generation を参照せず streaming session ID
  だけで処理する。既に停止済み / 未知 ID は成功、未知 ID が別の active stream を指さない場合は
  その active stream を維持する
- 409 本文の error code を session / generation で分離し、動画 API の JSON error code を
  remote-web 計測ログへ複写する

#### 実機 timeout 是正 (単一 start budget、protocol v17、2026-08-02)

start の **15 秒**は、次の根拠で置く運用上限である。

- repository 内で実測済みの cold RAW decode は 1.7 秒で、通常 IPC 応答上限 10 秒はその約 6 倍を
  根拠としている。動画 start は source open/probe に加えて D3D11 decoder、audio device、resume
  seek、H.264/AAC encoder、playlist 初期化を直列に含むため、同じ 10 秒を上限にはしない
- 15 秒なら 1.7 秒の約 8.8 倍で、Web 側の既存 playlist recovery horizon 15 秒とも一致する。
  一方、4 本の stream worker を 60 秒 liveness 近く保持する値にはしない
- これは cold 大容量動画の percentile 実測値ではない。v17 の stage 別 error code を計測ログへ
  残し、`stream_start_player_timeout` / `stream_start_seek_timeout` /
  `stream_start_encoder_timeout` / `stream_start_playlist_timeout` の分布を根拠に再調整する

動画 path に直接関係する wall-clock timeout は次の **9 個**で、同じ仕事を二重に打ち切らない。

| 境界 | 値 | 守るもの | start との関係 |
|---|---:|---|---|
| core start budget | 15 秒 | queue 受付から最初の usable playlist までの start 全体 | 機能上の唯一の start deadline |
| remote-web start IPC response | 20 秒 (15+5) | named-pipe dispatch / response routing | transport guard。必ず core start より後 |
| generation resource response (`RESOURCE_TIMEOUT`) | 2 秒 | 4 本の stream worker を停止した generation worker から解放する | 通常の playlist/init/media/state のみ。start playlist は残予算を渡す |
| 通常 IPC response | 10 秒 | start 以外の pending IPC / HTTP worker | start には使わない |
| 通常 UI request accept | 2 秒 | control/seek/stop 等で UI queue 未受理を検出 | start は 2 秒を使わず start 残予算を使う |
| browser generation switch | 15 秒 | start 後の generation mismatch / 一時 503 からの回復を単一 owner で直列化 | start 完了後の client recovery |
| browser first media segment | 15 秒 | playlist attach 後に media segment が 0 件のままの開始不能を検出 | core start 完了後。通常の再生中 buffering とは分離 |
| remote session liveness | 60 秒 | client 消失時の owner / stream 解放 | request timeout ではない lifecycle guard |
| paused-stream idle | 10 分 | 放置された停止中 stream の解放 | request timeout ではない lifecycle guard |

`RESOURCE_TIMEOUT` の resource 読み出しは segment を生成する処理ではなく、generation worker が
所有する in-memory segmenter/ring の snapshot command である。未生成 segment は待たずに
`NotFound` / typed `None` を返す。したがって 2 秒は media I/O の所要時間ではなく、encoder 内で
worker が停止した時に stream lane を占有し続けないための circuit breaker として維持する。
timeout message は要求種別 (master/media playlist、init/media segment、state) を含み、HTTP は
`stream_resource_timeout` を `details.video_stream.error_code` へ記録する。

start が `AcquiringRemote` の barrier 待ちに入った時間も core start budget の queue stage に
含める。barrier が開かなければ `stream_start_queue_timeout` で終端し、claim 前の待機だけが
15 秒の外に残ることを許さない。また `/api/video/start` は state を作る POST なので、Web 側は
`503` を無期限に自動再送しない。1 回の試行を error と「再試行」で終え、playlist attach 後の
generation recovery だけを従来どおり専用の 15 秒 owner で扱う。

増分 5 の **6.0 秒**は wall-clock timeout ではなく、2 秒 GOP の 3 倍を許す source-timeline 上の
unfinished fragment / interleave backlog 上限である。超過時は session を停止し、resource request
の 2 秒や start の 15 秒とは合算しない。named-pipe connect 1 秒と reconnect backoff は接続基盤の
guard で、動画処理開始後の timeout 9 個には数えない。

### 6.3 セッションと既存ロックの関係

ストリーミングセッションは既存の remote session owner に**従属**する。

- 操作権が別端末へ移った / ローカルへ戻った時点で、既存の「media pause」経路
  ([web-remote-plan.md](web-remote-plan.md) §2.2) がそのままストリーミングも止める
- 放置タイムアウト (10 分) の抑止条件である「再生中」にストリーミング中を含める
- ストリーミング中は生存タイムアウト (60 秒) の判定にセグメント取得も活動として数える
- `DrainingRemote` は UI の stream handle を外した時点では完了しない。cancel 済み generation
  worker が返り、stack 所有の FFmpeg / D3D11 / encoder resource を破棄して worker lease を
  外すまで final release を待つ。join 自体は UI thread で行わず worker 側の lease lifetime を
  drain accounting に使う。再生制御用 registration は generation handle が別に所有し、worker が
  EOF まで生成し終えた後も、端末が公開済みの末尾バッファを消費する間の play / pause と
  segment activity を受け付ける。これにより旧 worker が process-wide generation resource lease を
  持つ間に次 owner の acquire / start が成功することを防ぎつつ、worker 完了を session 切断と
  誤分類しない
- 端末の一度の再接続操作は、この final release が済むまで同じ acquire intent として待つ。
  streaming worker の終了には固定の wall-clock 上限がないため、取得待ちを任意の総時間で
  打ち切らない。これは状態を作る `/api/video/start` の無期限再送とは分離し、session ID の
  発行前だけを backoff 付きで再照会する

### 6.4 画質プリセット

| プリセット | 解像度 (長辺) | 映像 | 音声 | 1 時間あたり |
|---|---|---|---|---|
| 最小 | 640 | 400 kbps | 64 kbps | 約 210 MB |
| 低 | 854 | 800 kbps | 96 kbps | 約 400 MB |
| 標準 (既定) | 1280 | 1.5 Mbps | 128 kbps | 約 730 MB |
| 高 | 1920 | 3 Mbps | 160 kbps | 約 1.4 GB |

- 元動画より大きい解像度へは拡大しない
- アスペクト比は保持し、縦動画は縦を長辺として扱う
- 画質変更はシークと同じく generation を切り替える (エンコーダ再初期化が要るため)
- 通信量の目安を Web の画質選択 UI に表示する
  ([web-remote-plan.md](web-remote-plan.md) §6.5.6 の「従量課金回線への配慮」)

#### 増分 5 実装記録 (旧実時間 session + settings、2026-08-01、時計なし移行で廃止)

以下は旧 tap 経路の履歴であり、現在の production 配線ではない。現在の所有関係は §4 と
§10.2 を正本とする。

- `src/video/stream/session.rs` の generation worker が audio/video tap lease と receiver、
  H.264/AAC encoder、fMP4 segmenter/ring を一括所有する。H.264/AAC の open は worker 内で
  のみ行い、`App::poll_remote_session` は owner、再生 intent、`seek_serial`、worker status の
  照合だけを行う。generation の drop 時の FFmpeg teardown/join も別 join thread へ逃がす
- streaming registration は既存 remote owner generation に従属する。別端末の acquire、
  local disconnect、60 秒 liveness、停止中の 10 分 idle で cancel flag を立てる。
  streaming playing は idle 抑止へ含め、media segment 取得だけを liveness の `last_ping` へ
  数える (idle の user activity 時刻は更新しない)
- seek は tap payload の `seek_serial` 変化を `App` polling で検出し、encoder/segmenter を持つ
  worker を交換して新 streaming generation を発行する。各 encoder も generation 開始時の
  `expected_seek_serial` と違う旧 payload を捨てる。画質変更も同じ worker 交換を使い、resource
  accessor は requested generation が current と違えば worker/ring を読む前に拒否する
- 作りかけ fragment と encoder interleave queue の source-timeline 長は **6.0 秒**を上限とする。
  2 秒 GOP の 3 倍まで IDR 遅延を許し、超過時は再生成ループにせず session を停止する。
  映像 tap の software queue は **3 frames** (30fps で 100ms、60fps で 50ms、4K YUV420 の
  目安で約 36MiB)。HW surface は従来どおり queue 内 0 のまま
- playlist/init/media は同じ worker の segmenter から直列に snapshot する。segmenter の
  readiness gate を迂回しないため、playlist が `Some` なら init と最初の media segment が
  同時に取得可能という不変条件を維持する
- ローカル mute は audio device や PCM queue を止めず、owner-scoped lease が cpal callback の
  `output_volume` だけを 0 にする。post-DSP tap はその前 (processed queue push 前) なので、
  リモート PCM は mute の影響を受けない
- remote video の `VideoPlayer` は fullscreen cache ではなく streaming UI state が headless で
  所有し、UI frame ごとに tick する。decoder、video/audio tap、encoder は動かすが native
  presenter、folder load、`open_fullscreen` は作動させない。本体の既存表示状態は変えず、
  「リモート接続中」modal を配信中も表示し続ける。ローカルの既存 player / 音声モードの
  `Music` 表示も remote video start では切り替えない
- headless player だけは `RemoteHeadless` output consumer を所有し、decoder の通常
  `video_tx` を pacing せず連続 drain する。受け取った frame は GPU slot を返して破棄し、
  seek 世代ごとの最初の frame で `FirstFrameReady` を発火する。通常 player は
  `Presentation` のままで、native presenter / UI receiver の所有関係を変更しない
- headless でも `audio::start` は通常どおり default output device、audio pump、cpal callback を
  起動する。ローカル mute lease は callback の `output_volume` だけを 0 にし、processed queue は
  Playing 中に消費し続ける。tap は processed queue push の直前なので device callback の mute より
  上流にあり、最終 PCM の生成と remote 送信を止めない
- 設定の enum は runtime の nested `EncoderPreference` 等を直接永続化せず、単純な scalar variant
  名を持つ `RemoteVideoEncoder` / `RemoteVideoQuality` として分離した。未知 variant は既存の
  forward-incompatible 判定に入り、`Corrupted` quarantine / backup 自動復旧を行わない。
  本機能は未リリースのため既存値の migration は不要
- ローカル出力 bool も文字列の未知値を serde の `unknown variant` として分類し、
  `Incompatible` で原本を温存する
- `Opening` / `Starting` / `Streaming` の全所有状態で headless player を UI frame ごとに
  `tick` する。`tick` 冒頭の engine event drain が `SeekCompleted`、headless
  `FirstFrameReady`、audio pump の `BufferReady` を actor へ届ける唯一の経路である。
  増分 16 直後は `Starting` だけ tick が欠け、両 readiness event が channel にあっても
  engine は `Seeking` のままだった。その間 audio pump は `audio_tx` を約 5 秒分
  `raw_pending` へ取り込んだ後、cpal callback が非 Playing で processed を drain しないため
  非破壊 back-pressure 上限に達して停止し、decoder 側の bounded `audio_tx` が満杯になった。
  `Starting` も同じ tick 契約へ戻すことで `Seeking → Buffering` 後に `BufferReady` が有効になり、
  video/audio readiness が揃って `Playing` へ進む
- headless worker の開始・現在世代の最初の frame 消費・`FirstFrameReady` 送信、audio pump の
  世代ごとの最初の `audio_tx` 消費、engine 側の readiness event 受領を通常ログに残す。
  start 失敗時は 1 行に engine state、epoch、video/audio の required/ready、未処理 engine
  event 数、audio raw/processed/`audio_tx` 秒数をまとめ、構造化 perf にも同じ条件を記録する
- headless consumer の開始ログに出る epoch は開始時 snapshot であり、consumer は
  `clock.current_seek_serial()` の変化へ追随する。世代変更自体もログに残し、旧世代 frame は
  drain だけして readiness には使わず、現世代最初の frame だけを `FirstFrameReady` にする
- generation worker は video/audio frame がまだ無い間を idle として 20ms 単位で待ち続ける。
  明示 cancel だけを `Stopped` とし、tap / resource channel 切断や encoder / muxer error は
  `Failed(reason)` に保存する。registration / tap lease は `Result` を分類して status とログへ
  reason を発行するまで drop しない。この reason は IPC の `VideoStreamError.message` から
  remote-web 計測の `internal_message` まで保持し、利用者向け本文には露出しない

---

## 7. フロントエンド

既存の viewer と同じコマンド層に載せる。新しい入力概念を増やさない。

| 操作 | touch | keyboard | コマンド |
|---|---|---|---|
| 再生 / 一時停止 | 中央タップ | `Space` | `media_toggle_play` |
| 前後 10 秒 | 左右タップゾーン | `←` / `→` | `media_seek_relative` |
| ファイル送り | 左右スワイプ | `↑` / `↓` | `next_page` / `prev_page` (既存) |
| 音量 | メニュー内スライダ | — | `media_volume` |
| 画質 | メニュー | — | `media_quality` |
| 全画面 | メニュー | `F11` | `toggle_fullscreen` (既存) |

- シークバーは自前 DOM。generation ごとに state の `source_origin_secs` を 1 度だけ基準点へ
  置き、以後は `<video>.currentTime` を足して表示する。生成端は playhead に使わない
- バッファ不足 (`waiting` イベント) が 3 秒以上続いたら画質を 1 段下げる提案を出す
  (自動では下げない。§2 のとおり ABR は持たない)
- iOS のバックグラウンド復帰時はセッション生存を `/api/video/state` で確認し、
  失効していれば同じ位置で `start` をやり直す

#### 増分 7 実装記録 (フロントエンド、2026-08-02。timeline anchor は段 2 で更新)

- native HLS もある `ManagedMediaSource` 端末は native HLS を使う。それ以外で MSE がある端末は
  完全一致の public shell asset `/vendor/hls.min.js` を遅延ロードして `Hls.isSupported()` を評価し、
  false の場合だけ native HLS を試す。両方無ければ明示的な非対応表示にする。playlist / segment /
  video API は従来どおり認証 guard 下で、
  native video、playlist probe、hls.js XHR のすべてを同一 origin Cookie 経路にした
- 初版の server position/live edge 差分 anchor は段 2 で廃止した。現在は generation の
  `source_origin_secs` と media time 0 を一度だけ anchor にし、対象を `seekable` へ逆写像できる
  場合は video 内だけで移動し、範囲外は `/api/video/seek` の新 generation / playlist へ差し替える
- `waiting` 継続 3 秒で 1 段下の画質と 1 時間あたり通信量を提示し、利用者がボタンを押した時だけ
  `media_quality` を送る。503 は `Retry-After` 後に再試行、410 は ring に追いつけなかった表示と
  明示再接続、generation mismatch の 409 は state の current generation から URL を組み直す。
  session mismatch の 409 は再取得対象にしない
- seek 応答、state poll の generation 変化、hls.js の generation mismatch 409 は
  `VideoGenerationSwitchOwner` の 1 本へ合流する。owner は `idle / switching / attached` の
  排他的状態を持ち、同一 generation と古い要求は進行中 Promise に相乗りし、より新しい
  generation だけが古い per-switch `AbortController` を中止して置き換える。切替を受理した
  同期時点で旧 hls.js / native source を停止し、旧世代からの追加 error event を無視する
- generation switch の 15 秒 budget は owner が切替決定時に一度だけ作り、必要なら current
  generation を state から解決する段階と、playlist が取得可能になるまでの probe 全体で
  持ち回る。回数上限は置かない。503 と 409 の retry は `Retry-After` 以上かつ
  250ms から最大 2 秒までの backoff とし、残予算を超えて待たない。予算内の 503 は待機表示の
  まま継続し、予算満了時だけ `playlist_recovery_budget_exhausted` を内部 telemetry に残して
  「動画の再生を続けられませんでした。」と再接続操作を表示する
- playlist attach ごとに15秒の初回 media segment watchdog を持つ。hls.js は init を除く
  `FRAG_LOADED`、native は `loadeddata` / `canplay` で解除する。0件のままなら通常 buffering と
  分けて開始不能を表示し、telemetry に `no_media_segment_loaded_before_deadline`、playback mode、
  ready/network state、session/generation を残す
- HTTP 本文や例外の内部メッセージを notice へ連結しない。start は
  「動画を開始できませんでした。もう一度お試しください。」、session/generation 失効は
  「動画の配信が終了しました。もう一度開いてください。」へ正規化し、HLS、encoder、
  playlist、HTTP status などの実装語は利用者向けエラーへ出さない
- 動画のタップ、スワイプ、Space、矢印、音量、画質、F11 は `command-core.mjs` の既存 command
  dispatch に統合した。`<video controls>` は使わず、シークバー、音量、画質 UI は自前 DOM とした。
  visibility 復帰時に session が失効していれば、表示中の全体動画位置を保持して start + seek する

#### 実機停止是正: playback layer telemetry / runtime stall terminal (2026-08-06)

- hls.js `ERROR` は `type` / `details` / `fatal`、HTTP status、fragment sequence/type と
  video element の ready/network/error、buffered ahead、`hls.loadingEnabled` を telemetry に残す。
  URL、response body、例外 message、remote session ID は載せない。fatal と video element error は即時送信、
  non-fatal は同じ signature を 5 秒以内に重複送信しない
- video element の `error` / `stalled` も再生層の event として記録する。hls.js 使用中の
  media error は hls.js の typed `ERROR` が recovery authority、native HLS は video element が
  terminal authority となる
- runtime `waiting` は `playbackStallWatch` が一つだけ所有する。`timeupdate` の前進を watch 内で
  累積し、0.25 秒以上の実再生進行を純関数の `resolved` として watch と waiting/buffering notice を
  解除する。一瞬の 50ms 未満の揺れや paused 中の seek は復帰に数えない。3 秒の画質提案も同じ純関数を
  通るため、play intent が消えた watch や既に復帰した watch は通知を出さない。表示中・再生要求中で
  generation switch 外にもかかわらず 15 秒進まなければ、即時 telemetry を
  残して停止し、「現在位置から再接続」できる terminal 表示へ移る。hidden / generation switching
  中は判定を延期し、意図的 pause / session block / ended は watch を解除する
- 従来は非 HTTP の fatal HLS error で `stopLoad()` しても waiting timer を解除せず、fatal の
  再接続表示を 3 秒 notice が上書きできた。fatal / native terminal / stall terminal はすべて
  waiting owner を先に解除してから再接続表示へ入る
- 2026-08-06 の実機ログでは `/api/video/seek` を伴わず、同一 generation の `seekable` 内を
  動く local MSE seek 後に playlist/segment 取得だけが停止した。したがってこの再現は generation
  switch 競合ではない。fatal の具体的 `type/details` は旧 telemetry には無く、上記計測で次回
  再現時に確定する。buffer量の設定変更はこの是正には含めない

#### 動画健全度 HUD / 定期 telemetry / 2 段階記録 (2026-08-06)

- 動画 viewer は `video_health` snapshot を一つの builder で作る。`currentTime` と全体動画位置、
  buffered ahead / range 数、ready/network/media error、play intent / paused / waiting、frame drop、
  hls.js bandwidth estimate / loading state、直近 fragment load 時間・sequence、利用可能な場合だけ
  Network Information API の effective type / RTT / downlink を含む。URL は snapshot の通常 fields に
  コピーしない。`getVideoPlaybackQuality` / Network Information API が無い場合は `null` とする
- HUD は動画 viewer が存在する間だけ画像/一覧行を動画行へ差し替え、位置、先読み秒、readyState、
  dropped/total frames、帯域推定、segment 時間、回線情報を小さい 3 行にまとめる。動画中だけ下部の
  前後ファイルボタンより上へ退避し、viewer の destroy で snapshot と位置指定を解除して従来の
  画像/一覧 HUD へ戻す
- stream session があり、play intent が ON、session block 外なら、再生時刻の進行とは独立した timer で
  10 秒ごとに `trigger=periodic` を送る。hls.js が止まり currentTime が変わらなくても timer は継続する。
  意図的 pause、viewer destroy、session block では止める。3 秒 waiting 表示時、15 秒 stall terminal、
  hls fatal、video element error、`video.play()` rejection は周期を待たず health snapshot を即時送信する。
  health は play attempt / success / rejection / pending の累計と直近 rejection 名も持つ。開始時に
  `paused=true` のままになる症状は既存記録だけでは play 未呼び出しと Promise rejection を区別できない
  ため、原因未確定の自動 retry は入れず、この計測で次回再現時に確定する
- telemetry は queue へ入れる直前に `normal` / `debug` の 2 段へ正規化する。通常段は既定で、
  数値・boolean・固定列挙を残し、path/address/resource、message/stack、client/session 識別子を再帰的に
  除く。これにより既存の window/fetch/image event も通常段では同じ境界に従う。詳細段だけ remote
  address (favorite ID + relative path + subresource)、server/diagnostic message、端末 ID を残す
- 詳細段の ON/OFF は端末 local setting `telemetryDebugDetails` (version 1 への後方互換な加法、既定
  `false`) が所有する。ON 中は HUD を隠せず「詳細記録 ON」と橙色枠を常時表示し、tap で端末設定へ
  戻る。端末は PC の絶対 path を持たないため新たに公開せず、remote address と本体 perf log を相関する
- remote session ID はどちらの段にも生値を出さない。Web Crypto の SHA-256 を 96 bit に短縮した
  `remote_session_correlation` だけを詳細段へ付け、Web Crypto 非対応時は省略する。client 側でも既知の
  session 生値を置換し、disk 書き込みは従来どおり `permanent_log_secrets` と
  `redact_serialized_secret` を通る。PIN / Bearer token の例外段は作らない

#### iOS autoplay / EOF 後の再生制御是正 (2026-08-07)

- `video.play()` の `NotAllowedError` は通信待ちや再生層停止ではなく
  `user_activation_required` gate として所有する。この間は waiting/stall watch を解除し、canplay、
  playlist attach、visibility 復帰などの自動経路から `play()` を再発行しない。notice 自体に
  「タップして再生」ボタンを置き、その click handler が user activation を失う await より前に
  `play()` を 1 回だけ呼ぶ。成功時だけ gate を通常状態へ戻す
- play intent と gate は `play_requested / stopped / user_activation_required` の単一状態で所有する。
  native `playing` event は個々の `play()` Promise と相関せず、iOS では拒否後にも届きうるため、
  許可成功の根拠にはしない。poll の play intent、`canplay`、非利用者起点の成功通知でも activation
  待ちは維持し、明示タップから呼んだ `play()` の成功だけが `play_requested` へ戻して案内を消す。
  拒否直後の `playing` で gate が落ち、続く `canplay` が 2 回目を発行していた経路も同じ遷移で閉じる
- `AbortError` は source 交換や pause により play 要求の owner が中断した状態であり、autoplay
  案内へ写像しない。それ以外の play promise rejection は再生層失敗として明示的な terminal へ
  終える。拒否名の分類と、activation 待ち中に stall 判定を開始しない条件を純関数テストで固定する
- clockless worker は動画末尾まで生成すると正常終了するが、公開済み HLS resource と playback
  session はその後も Active である。worker が持っていた streaming registration まで終了時に
  drop していたため、末尾バッファ再生中の Pause だけが `active.streaming == None` で 422 になった。
  worker drain lease と control registration を分離し、前者は実 worker 終了、後者は generation
  handle の寿命へ一致させる。session payload / generation の喪失は `SessionMismatch`、registration
  欠落・不一致は内部 lifecycle invariant failure として型で区別する

#### 実機待ち時間是正 A: seek preview (2026-08-03)

- protocol v18 の thumbnail request は UI thread で既存 worker の要求と cache snapshot だけを
  行い、WebP encode は IPC stream worker へ逃がす。再生 decoder / generation worker / audio
  readiness は待たず、thumbnail 完了も一切待たない
- Web は `VideoSeekPreviewOwner` が request revision と `seeking / thumbnail / playback` を
  単独所有する。range `input` は AbortController で旧 request を捨て、常に最新位置だけを poll する
- thumbnail の要求位置ではなく応答 header の実 frame PTS を照合・表示する。thumbnail 未到着でも
  `<video>` の `playing` を受けた時点で preview を終了し、通常再生を即座に前面へ戻す

---

## 8. 設定項目

`Settings` に追加する (本体側。remote-web は read-only で読む)。

| キー | 既定 | 内容 |
|---|---|---|
| `remote_video_streaming_enabled` | `true` | ストリーミング機能そのものの可否 |
| `remote_video_encoder` | `Auto` | `Auto` / `Nvenc` / `Qsv` / `Amf` / `MediaFoundation` / `OpenH264` |
| `remote_video_quality_default` | `Standard` | §6.4 のプリセット |
| `remote_video_segment_window` | `30` | 保持セグメント数 (= 60 秒) |
| `remote_video_mute_local_output` | `true` | 旧実時間経路の DB 互換キー。時計なし移行後は UI に表示せず、動作には使わない |
| `remote_video_hide_local_output` | `true` | 旧設定 DB 互換キー。headless remote player 化後は UI に表示せず、動作には使わない |

---

## 9. ライセンス・特許

| 対象 | 結論 |
|---|---|
| **FFmpeg (LGPLv3)** | **追加義務なし**。現行の動的リンク構成のままエンコーダを呼べる。GPL 品 (x264 / x265) は BtbN の LGPL ビルドで無効化済み。ただし CLAUDE.md とソフトウェア情報にある「mIV はデコードしか使わない」という記述は**更新が必要** |
| **NVENC (ffnvcodec)** | ヘッダ (nv-codec-headers) は MIT。実行時に NVIDIA ドライバ同梱の `nvEncodeAPI64.dll` を動的ロードするだけで、SDK バイナリの再配布はない。**追加義務なし**。コンシューマドライバの同時セッション制限も 1 セッション利用なので無関係 |
| **QSV (libvpl) / AMF** | ランタイムは各 GPU ドライバ同梱。FFmpeg 側のラッパは MIT/BSD。**追加義務なし** |
| **Media Foundation** | Windows の OS 機能。**追加義務なし** |
| **OpenH264 (Cisco)** | BSD ライセンス。ただし Cisco による特許料肩代わりは「Cisco が配布するバイナリを実行時に取得する場合」に限られ、静的リンク版は対象外という解釈が主流。下の H.264 特許の項に帰着する |
| **H.264 特許 (Via LA プール)** | エンコード製品にロイヤリティの建付けがあるが、**年間 10 万ユニット未満は無償**の枠があり、個人フリーソフトの配布規模では実務上問題にならない。加えてハードウェアエンコーダ経由では GPU / OS ベンダーがライセンス済みの実装を呼ぶ形になる。保守的にするなら「ハードウェアエンコーダを既定、ソフトウェアは明示選択」という切り方も取れる (本書は法的助言ではない) |
| **HEVC / AV1 / VP9** | **使わない**。HEVC は特許プールが AVC より格段に複雑。AV1 は iPhone 15 Pro 未満でデコード不可。VP9 は iOS のハードウェアデコード事情が悪い |
| **HLS / fMP4 (CMAF)** | 仕様は公開 (RFC 8216 / ISO BMFF)。muxer は FFmpeg 内蔵。**追加ライセンスなし** |
| **hls.js** | **Apache-2.0**。MIT 配布と互換。`NOTICE` 相当の表示をソフトウェア情報と `installer/readme.txt` へ追加する |
| **OBS Studio** | **GPLv2。コードを流用しない**。設計の考え方を読むのは自由だが、写経すると mIV 全体が GPL 汚染され、MIT 配布・Vector・MS Store 配布のいずれとも両立しなくなる。実装の参照先は FFmpeg 公式サンプル (`doc/examples/`、MIT) とする |

### 9.1 リリース時に更新する箇所

- ソフトウェア情報 (環境設定 → ヘルプ): FFmpeg の用途に「エンコード」を追記、hls.js の表示を追加
- `installer/readme.txt` / `installer/readme_portable.txt`: 同上
- CLAUDE.md「FFmpeg LGPL DLL 管理」節: 「mIV はデコードしか使わないので無問題」の記述を改める

---

## 10. 制約・リスクと要検証事項

1. **画面表示への非依存** — 時計なし worker は presenter queue を使わないため、native window の
   表示状態や monitor sleep に進行を依存させない
2. **PC 側の音への非依存** — cpal / audio master clock を使わず、decoded PCM を直接 AAC へ渡す。
   headless metadata player は pause のままなので PC スピーカーへ出力しない
3. **先行生成の上限** — 2 秒 × 30 segment = **60 秒**。時計なし worker は segment ring に
   空きがある間だけ走り、満杯なら `Condvar` で park する。端末の segment 取得で解放して再開する。
   同じ 60 秒を state の `buffer_target_secs` で端末へ渡し、hls.js の `maxBufferLength` と
   `maxMaxBufferLength` の両方へ設定するため、server と端末を別々には変更できない
4. **同時 1 セッション** — 既存のセッションロックと同一なので追加の排他は不要
5. **HDR 素材** — 第 1 段は BT.709 固定。HDR (PQ/HLG) 素材はトーンマッピングせずに送ると
   眠い絵になる。実機で確認し、必要なら `zscale` / `tonemap` の導入を第 2 段で検討する
   (両フィルタとも LGPL ビルドに含まれる)
6. **インターレース素材** — deinterlace を通す必要がある。`yadif` は LGPL ビルドに含まれる
7. **可変フレームレート素材** — セグメント境界の GOP 固定と噛み合わない場合がある。
   エンコーダ入力側で CFR 化する
8. **通信量** — 標準画質 1 時間で約 730MB。従量回線での利用は画質選択と通信量表示で支援する

### 10.1 時計なし駆動部の実測 (移行 1/3、2026-08-03)

`clockless_transcode_bench` を本体と別プロセス・別 runtime directory で実行した。
標準画質 1280x720 H.264 NVENC、D3D11VA decode、30 秒区間、RTX 4090 / 24 logical CPU。
HEVC は指定素材置場に 4K 実素材が無かったため、実 H.264 1080p60 素材から作った
HEVC Main10 4K fixture であり、4K の行は合成入力として扱う。終端の未完 2 秒 fragment は
今回の throughput 集計に含めない。

| 入力 | 音声なし | AAC 込み | 音声による低下 | CPU | NVENC / NVDEC |
|---|---:|---:|---:|---:|---:|
| H.264 1080p30 | **8.37x** | **7.33x** | 12.5% | 約 1.0 core (全体の 4.2%) | 11–12% / 27–29% 同時 |
| AV1 1080p30 | **9.15x** | **8.37x** | 8.5% | 約 1.0 core (全体の 4.2%) | 12–16% / 12–19% 同時 |
| HEVC Main10 4K30 (fixture) | **2.28x** | **2.20x** | 3.6% | 約 1.0 core (全体の 4.1%) | 同時利用を確認 |
| HEVC Main10 4K60 (fixture、重負荷) | **1.12x** | **1.09x** | 2.3% | 約 1.0 core (全体の 4.1%) | 3–4% / 11–16% 同時 |

4K60 は **1.5x を下回る**。30 秒・音声なしの段階時間は demux 0.03 秒、decode 0.66 秒、
GPU→CPU readback 14.45 秒、scale + encode 11.43 秒で、codec decode ではなく frame readback と
CPU swscale が律速である。追加 profiler では 10 秒分の 4K60 swscale 単体が 3.20 秒、同区間の
scale + encode が 3.87 秒だった。1080p も最大区間は readback、次が scale + encode であり、
demux / codec decode は支配的でない。将来 4K60 に十分な余裕を持たせるには、D3D11 texture を
CPU に戻さず GPU scale して NVENC へ渡す経路が次の性能投資候補になる。

### 10.2 時計なし経路での状態所有方針

- **VST3**: mIV を全停止して再測定した実 chain (active 5、44.1 kHz、60 秒 fast-feed) は
  `wall_secs=2.040909`、**29.399x realtime**。1080p 映像込み全体の 8.4x より十分速く、律速ではない。
  前回の `SSL Meter Pro` 20 秒 timeout は同時に 3 host を立てた測定条件が原因だった。このため
  ローカル再生の App-global host へ高速配信を混在させず、streaming session が **専用 host を 1 個だけ**
  所有する。`ClocklessAudioProcessing` の processor / failure / status を全 generation が `Arc` clone
  し、既存 generation resource lease が旧 worker と新 worker の VST 使用も直列化する。したがって
  generation 切替中も **ローカル 1 + リモート 1 = 最大 2 host** であり、新旧世代に比例して増えない。
  bypass plugin はリモート host へ load せず、設定順を保った active plugin だけを一度 load する。
  load 時間は start 残予算から後段用 3 秒を予約した値（上限 10 秒）に制限する
- **音量正規化**: remote player は autoplay=false で開くため deferred scan を持たず、open 前の
  DB lookup (未測定なら 1.0) で `normalize_gain` が確定する。`RemoteStreamStartInputs` を
  generation 作成時に snapshot し、時計なし PCM の AAC 前段で固定 gain として適用する
- **位置**: server は generation、source origin、生成済み範囲、ring の earliest/latest、duration、
  再生 intent を所有する。実 playhead は端末の media element が source of truth であり、
  `/api/video/state` の本体位置を端末位置として返さない。resume/history が必要なときだけ端末が
  playhead を報告する
- **一時停止 / seek / 終端**: 一時停止中も ring 上限まで生成して park する。seek は現在の
  generation 交換規約を再利用し、旧 worker を段境界 cancel、新 generation は独立 open + seek
  で開始する。終端は decoder、resampler、H.264/AAC encoder、A/V mux、既存
  `Fmp4Segmenter::finish()` の順に flush し、最終 fragment を ring に記録してから `Ended` と
  session 側の `#EXT-X-ENDLIST` を公開する。単なる ring 満杯とは区別する。live 30 本とは別に
  EOF を観測するための working fragment 1 slot と terminal fragment 1 slot を ring に予約する。
  working fragment は公開・取得可能にし、入力 EOF を観測した後の有限な codec drain は未公開
  fragment の取得待ちへ入れない
- **世代資源の排他**: generation worker の auxiliary decoder / FFmpeg 所有 D3D11 device / H.264
  encoder の全寿命を process-wide lease で直列化する。seek や start が旧 handle の非同期 join より
  先に到着しても、旧 worker が戻り FFmpeg context が drop されるまでは次世代を open しない。
  streaming registration は GPU 資源ではなく generation handle の寿命で保持し、worker が EOF へ
  到達した後の play/pause を ownership loss にしない
- **端末 seek の正本**: seek bar は指を離した時点の全体位置を absolute command として保持する。
  session acquire 後に playhead を再加算しない。通常の初回 start は保存 resume を受け入れるが、
  `restartAt` の明示位置は 0 秒も含めて start 応答の origin と照合し、異なれば `/api/video/seek` する

---

## 11. 実装段階

### 時計なし移行 (初回リリース対象)

1. **駆動部 + 実測 (完了)** — 独立 Input、demux/decode、既存 H.264/AAC encoder と
   `Fmp4Segmenter`、ring 容量 gate、段境界 cancel、dev-tools benchmark
2. **session 配線 (完了)** — generation owner を時計なし worker へ接続。60 秒先読み、
   terminal playhead、seek、EOF flush / ENDLIST、session 共有 VST + normalize を確定し、旧 tap worker を削除
3. **Web/実機仕上げ (次)** — iPhone 実機で起動・シーク・回線揺れ・自然終端を検証する

### 既存の実時間経路 第 1 段 (実装済み、移行元)

| 内容 | 概算 |
|---|---|
| エンコーダ抽象 + フォールバック階段 + H.264/AAC エンコード | 500〜700 行 |
| fMP4 セグメンタ (custom AVIO + リングバッファ + m3u8 生成) | 400〜600 行 |
| 映像 tap (decoder 分岐 + HW download + scale/NV12) | 300〜400 行 |
| 音声 tap (pump 分岐 + resample + AAC 供給) | 200〜300 行 |
| ストリーミングセッション管理 (本体側、既存 session lock 連携) | 300〜400 行 |
| IPC protocol v15 拡張 + remote-web 側の中継と HTTP ルート | 400〜500 行 |
| フロント (video 要素 + hls.js 分岐 + 自前コントロール + 画質 UI) | 400〜600 行 |
| 合計 | **2,500〜3,500 行** |

### 第 2 段 (第 1 段の実測後に判断)

- 映像補正 (grade pipeline) の反映 — presenter とは別のレンダーターゲットへ同じパスを通す
- 字幕の焼き込み
- LL-HLS による遅延短縮 (部分セグメント + blocking playlist reload)
- HDR トーンマッピング
- 変速再生

---

## 12. 非スコープ (第 1 段)

- ABR / マルチビットレート同時エンコード
- WebRTC
- 変速再生・コマ送り
- 字幕・映像補正・VSR・AI アップスケールの反映
- 複数同時セッション
- リモートからの動画編集操作 (トリム・キャプチャ等)
- 元ファイルの直接ダウンロード
- 一覧の動画・音声サムネイル。第 1 段の `/api/thumb` は通常画像・フォルダ代表と
  container page だけを扱い、実在する動画への要求は `Unsupported` (HTTP 415) とする。
  一覧は type badge / placeholder を表示する。後続で対応する場合は本体ローカル表示と同じ
  Windows Shell API・catalog/cache 経路を core IPC 側から再利用し、remote-web 独自生成は
  追加しない

---

## 13. 受け入れ条件

### 13.1 検証環境

Android 実機を保有していないため、**検証できる範囲と委ねる範囲を明示的に分ける**。

| 対象 | 検証手段 | 位置づけ |
|---|---|---|
| **HLS 出力の適合性** | 自動テスト (生成 bytes を custom read AVIO で同じ FFmpeg avformat へ戻し、タイムスタンプ連続性・IDR 境界・コーデックパラメータを固定する。外部 ffprobe 不要) | 一次防衛線。ここが正しければクライアント差は小さい |
| **hls.js 経路** | **PC の Chrome / Edge / Firefox** | Android Chrome と同じ Blink + 同じ hls.js。開発中の主戦場でもある |
| **iOS ネイティブ経路** | iPhone / iPad 実機 | ユーザーが実施 |
| タッチ UI / PWA / バックグラウンド復帰 (Android 固有) | Android Studio の AVD (Pixel + Google Play イメージ) | 任意。優先度は低い |

- **AVD を使う場合、tailscale は不要**。エミュレータからは `10.0.2.2:<port>` がホスト PC の
  localhost にマップされるので、remote-web へ直接到達できる。x86_64 イメージ + WHPX なら
  実用的な速度で動く。ただしエミュレータの H.264 デコードはソフトウェアになることがあり、
  **性能特性は実機を代表しない** (機能確認用と割り切る)
- Windows Subsystem for Android は 2025-03 にサポート終了しており、選択肢にならない
- **Android 実機固有の不具合は hls.js の実績に委ねる**。hls.js は Android Chrome で広く
  使われており、mIV 固有のリスクは「サーバが吐く HLS が仕様に沿っているか」に集中している。
  そこは上記の自動テストと PC Chrome で押さえられる
- ユーザー向けマニュアルには**動作確認済み環境を正直に書く** (iOS/iPadOS Safari は実機確認済み、
  Android Chrome は未確認だが対応、と明記する)

### 自動テスト

- エンコーダ選択: 候補が無い / 一部 open 失敗 のときに正しく次段へ落ちること。
  全滅時は機械可読なエラーになること
- セグメンタ: セグメント境界が必ず IDR で始まること、PTS/DTS が単調増加して
  segment 間でも連続すること、init の codec parameters と SPS 由来 CODECS が一致すること、
  `EXT-X-MEDIA-SEQUENCE` がリングバッファの巻き取りと一致すること、破棄済み要求が
  `Gone` になること
- 音声 tap: tap 未接続時に pump の出力が現行と 1 サンプルも変わらないこと。
  tap 側が詰まったとき pump がブロックせず `dropped` を計上すること
- 映像 tap: tap 未接続時に decoder が AVFrame ref / readback / swscale を行わず現行の
  GPU / CPU presenter 経路を維持すること。tap が満杯なら clone せず即 drop を計上すること。
  worker を停止したまま SW queue を満杯にしても decoder HW surface の保持が同期中 1 枚・queue
  内 0 枚を超えないこと。source PTS から CFR slot を導き、欠落 frame 後も index を詰め直さないこと
- AAC: 1024 samples と一致しない chunk 列でも欠落・重複がなく、必要な sample rate 変換の
  EOF delay まで回収し、異なる `seek_serial` と 1.0x 以外を assembler 前で拒否すること
- A/V mux: raw YUV video tap → preset scale → libopenh264 と AAC の同一 fMP4 を同じ FFmpeg で
  in-process に読み返し、両 codec、全 audio packet の個数・PTS、各 video IDR 境界での
  audio coverage が一致すること
- A/V 同期: 既知の pts 列に対してセグメントのタイムスタンプが単調増加し、
  音声 `audible_pts_secs` と映像 pts のずれが 1 フレーム以内であること
- セッション: 操作権喪失 / 生存タイムアウト / 放置タイムアウトでストリーミングが停止すること
- start/stop: metadata player の resume origin から開始した generation を playlist resource が
  current として受理すること。stop は未知 / 停止済み ID でも成功し、
  active な別 ID は停止しないこと
- Web: generation mismatch 409 後に state の current generation へ URL を更新して回復し、
  mismatch が続く場合は有限回で利用者向け失敗表示になること。session mismatch と区別すること
- metadata player: streaming 中も pause のまま duration / resume origin / normalize gain /
  seek thumbnail を提供し、frame/audio transport は clockless worker だけが所有すること。
  resume origin 確定後も paused player の `clock.is_seeking()` が true のままのケースで
  `poll_remote_video_opening` が `Starting` へ進むこと
- audio DSP: fixed normalize gain の後に session 共有 VST3、その後に safety limiter を通ること。
  generation 交換で processor の `Arc` が同一で host 数が増えず、VST load / process 失敗時も
  normalized dry の動画配信を継続して start/state と本体 modal に warning が出ること
- `cargo test -p mimageviewer --lib` と `cargo test -p mimageviewer-remote` が緑

### 実機 (ユーザーが実施)

- iPhone / iPad の Safari で、HEVC / AV1 / WMV / MKV いずれの素材でも再生できる
- 音量ノーマライズと active VST3 chain がリモート音声へ順番どおり反映される。
  VST3 を意図的に失敗させても動画は dry 音声で継続し、端末と本体の両方で未適用警告が見える
- 5G / 公衆 Wi-Fi それぞれで標準画質が途切れずに再生できる
- シーク・一時停止・画質変更が動作し、シーク後の再バッファが 5 秒以内
- **PC 側のウィンドウを最小化 / モニタをスリープさせても再生が継続する** (§10-1)
- ローカルへ操作権を戻すとストリーミングが停止し、本体の再生状態が壊れない
- 既定設定では本体側の映像と音声が出ず、「リモート接続中」と配信中ファイル名が見える。
  ストリーミング終了後は本体の映像表示が元に戻る

### 実機で記録する値

- 選択されたエンコーダとエンコード所要時間 (実時間比)
- 標準画質での実効ビットレートと 10 分あたりの通信量
- 再生開始までの時間、シーク後の再開までの時間
- バッファ枯渇 (`waiting`) の発生回数
