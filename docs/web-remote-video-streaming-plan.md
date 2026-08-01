# リモート動画ストリーミング 計画書

外出先のブラウザから自宅 PC の動画を視聴するために、**mIV 本体が実時間でデコード・
音声処理した結果を H.264 + AAC へ再エンコードし、HLS で配信する**。本書がこの機能の正本。

- 親計画: [web-remote-plan.md](web-remote-plan.md) (リモート閲覧機能全体の正本)
- ブランチ: `web-remote` (worktree: `C:\home\mimageviewer-web`)
- 現在のフェーズ: **第 1 段 増分 2/7 実装済み** (encoder 抽象 / fallback / 画質
  preset + fMP4 segmenter / ring / m3u8、2026-08-01)

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
  つまり本体の再生エンジンをリモート専用に占有できる。ミラー方式と相性が良い
- 音声は VST3 チェイン・ラウドネスノーマライズ・セーフティリミッタを通した**最終 PCM が
  1 箇所に揃っている** ([src/video/audio.rs](../src/video/audio.rs) の audio pump)。
  ここを分岐させるだけで「PC で聞いているのと同じ音」が pts 付きで取れる
- 解像度・ビットレートを送信側で決められるので、帯域に合わせて画質を落とせる

**したがって remux + 音声フォールバックは不要になり、本方式がそれを完全に置き換える。**

---

## 2. 確定事項

| 項目 | 決定 | 理由 |
|---|---|---|
| 取得方式 | **本体の再生セッションを実時間で tap** (§4) | VST・ノーマライズがそのまま乗る。本体ロック中の占有と整合 |
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
- `src/video/stream/mod.rs` の `#![allow(dead_code)]` は、増分 5〜6 で
  セッションから接続した時点で**外す**。それまでの間、このモジュール内の本当の
  dead code は検出されない

x264 は使わない (GPLv2。mIV は MIT なので持ち込めない)。OBS が x264 を同梱できるのは
OBS 自身が GPLv2 だからであり、**OBS のコードも設定値の写経以上の流用はしない** (§9)。

---

## 4. パイプライン設計

```
       ┌──────────────── mimageviewer-core.exe ────────────────┐
       │                                                       │
 file ─┼─▶ decoder ─┬─▶ presenter (PC 画面表示。従来どおり)      │
       │            └─▶ [video tap] ─▶ scale ─▶ H.264 encoder ─┐│
       │                                                      ││
       │    audio pump ─┬─▶ cpal (PC スピーカー。従来どおり)     ││
       │  (VST/normalize)└─▶ [audio tap] ─▶ AAC encoder ───────┤│
       │                                                      ▼│
       │                                          fMP4 セグメンタ│
       │                                        (リングバッファ) │
       └──────────────────────────┬────────────────────────────┘
                                  │ IPC (pull 型)
       ┌──────────────────────────▼────────────────────────────┐
       │  mimageviewer-remote.exe : m3u8 / init.mp4 / seg.m4s   │
       └──────────────────────────┬────────────────────────────┘
                                  │ HTTP (tailscale serve 経由)
                          ブラウザ (<video> or hls.js)
```

### 4.1 音声 tap — VST とノーマライズがそのまま乗る

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

tap は `Option<Sender<ProcessedChunk>>` を pump に渡す形とし、**tap が無いときは
現行コードと完全に同一の経路を通る**こと。pump は realtime 制約下にあるので、
tap の送信は非ブロッキング (満杯なら落として `dropped` を計上) とする。
tap 側の詰まりが PC 側の音を絶対に途切れさせない、が不変条件。

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

### 4.3 A/V 同期

- 音声: `ProcessedChunk::audible_pts_secs` (source timeline、PDC 補正済み)
- 映像: frame の pts (source timeline)

両方を **source timeline のまま**セグメンタへ渡し、セッション開始時刻を 0 とする相対
タイムスタンプへ写す。音声側が既に PDC を吸収しているので、リップシンクは自動的に合う。

変速再生 (`playback_speed != 1.0`) は第 1 段では**非対応**とし、ストリーミング中は等速に
固定する (`source_secs_per_output_sec` の扱いが増えるため。§12)。

### 4.4 セグメンタ

- **fMP4 (CMAF)**: init segment (`ftyp` + `moov`) 1 個 + media segment (`moof` + `mdat`) 列
- セグメント長の目標は **2 秒**、GOP も 2 秒 (`keyint = fps * 2`, `scenecut` 無効) とし、
  各セグメントを必ず IDR から始める。tap / encoder の frame skip で予定境界の IDR が
  欠けた場合は停止せず、次の IDR まで現在セグメントを延長する
- avformat の mp4 muxer を `movflags=frag_custom+empty_moov+default_base_is_moof+cmaf` 相当で
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

##### 後続増分への申し送り

- **作りかけの fragment には上限が無い。** 完成済みセグメントは ring が 30 本に
  抑えるが、IDR を待っている最中の fragment はメモリ上で伸び続ける。GOP は固定なので
  通常は 2 秒で IDR が来るが、encoder が過負荷で IDR を続けて落とした場合に歯止めが
  無い。**増分 5 のセッション管理で、延長の上限 (時間かバイト数) と、超えたときの
  扱い (セッションを畳む / 世代を切り替える) を決めること**
- `CfrTimelineFrameIndex` は「落ちた frame も欠番として残す CFR source timeline 上の
  位置」である。**増分 4 の映像 tap は、投入できた frame で番号を詰め直してはいけない**
  (詰め直すと forced-IDR の位置が encoder の負荷次第でずれる)

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
if (video.canPlayType('application/vnd.apple.mpegurl')) {
    video.src = playlistUrl;          // iOS / iPadOS / macOS Safari
} else {
    const hls = new Hls({ ... });     // Android Chrome / PC
    hls.loadSource(playlistUrl);
    hls.attachMedia(video);
}
```

- hls.js は **Apache-2.0**。`dist/hls.min.js` 1 ファイルを `crates/remote-web/web/vendor/` へ
  置いて静的配信する。バンドラも TypeScript も導入しない
  ([web-remote-plan.md](web-remote-plan.md) §3.4 の「ビルドステップを導入しない」を維持)
- iOS Safari では hls.js を**読み込まない** (MSE 制限があり動かないため、ネイティブに委ねる)
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

1. Web → `POST /api/video/seek` → IPC → 本体が実際に seek
2. 本体はエンコーダとセグメント列をリセットし、**新しい session generation** を発行
3. m3u8 の URL に generation を含めるため、クライアントは `video.src` を差し替える
   (hls.js 側は `loadSource` をやり直す)
4. 再バッファは 2〜4 秒

`#EXT-X-DISCONTINUITY` による同一プレイリスト継続は、iOS のネイティブ実装との相性を
確認できるまで採らない。URL を切り替える方が確実で、実装も単純。

---

## 6. API

### 6.1 HTTP (remote-web、すべて認証必須)

| エンドポイント | 内容 |
|---|---|
| `POST /api/video/start` | `{fav, path, quality}` → `{session, generation, playlist, duration_secs, codec, encoder}` |
| `POST /api/video/control` | `{session, action: play\|pause\|volume\|quality}` |
| `POST /api/video/seek` | `{session, position_secs}` → 新 `generation` と `playlist` |
| `GET /api/video/state` | 再生位置・尺・バッファ秒数・実効ビットレート・選択エンコーダ |
| `POST /api/video/stop` | セッション終了。本体はストリーミングを止める |
| `GET /stream/<session>/<gen>/index.m3u8` | CODECS を宣言する Master Playlist |
| `GET /stream/<session>/<gen>/media.m3u8` | MEDIA-SEQUENCE を持つ live Media Playlist |
| `GET /stream/<session>/<gen>/init.mp4` | init segment |
| `GET /stream/<session>/<gen>/<n>.m4s` | media segment |

- `/stream/` 配下も**認証必須**。同一オリジンなので Cookie は `<video>` / hls.js の
  どちらからも送られる
- セグメントは `Cache-Control: no-store`、init segment だけ `immutable`
- 未生成 / 存在しないセグメントは 404、ring から巻き取られたセグメントは 410 Gone、
  セッション不一致は 409、本体未接続は 503 と既存の `miv_not_running` を返す

### 6.2 IPC (protocol v11)

既存の長寿命 duplex 多重化接続 ([web-remote-plan.md](web-remote-plan.md) §9.5-9.6) に
`ClientMessage` / `ServerMessage` の variant を追加する。**セグメントは pull 型**とし、
remote-web が HTTP 要求を受けた時に取りに行く。push 型の非同期メッセージを新設しない
(現行プロトコルの request/response 構造をそのまま使えるため)。

| request | response |
|---|---|
| `VideoStreamStart { address, quality }` | `{ session, generation, duration_secs, encoder, video_size }` |
| `VideoStreamControl { session, action }` | `SessionStatus` |
| `VideoStreamSeek { session, position_secs }` | `{ generation }` |
| `VideoStreamPlaylist { session, generation, kind }` | master / media m3u8 本文 |
| `VideoStreamSegment { session, generation, index }` | セグメントのバイト列 / `NotFound` / `Gone` |
| `VideoStreamState { session }` | 再生位置・バッファ・ビットレート実績 |
| `VideoStreamStop { session }` | — |

セグメント IPC は既存の **heavy queue ではなく専用 lane** に置く。エンコード済みバイトを
返すだけで CPU をほぼ使わないため、サムネイル生成やページレンダリングと同じ枠で待たせると
再生が途切れる。`address` は既存の `RemoteAddress` をそのまま使い、本体側で favorite
allowlist と canonical containment を再検証する ([web-remote-plan.md](web-remote-plan.md) §12.1)。

### 6.3 セッションと既存ロックの関係

ストリーミングセッションは既存の remote session owner に**従属**する。

- 操作権が別端末へ移った / ローカルへ戻った時点で、既存の「media pause」経路
  ([web-remote-plan.md](web-remote-plan.md) §2.2) がそのままストリーミングも止める
- 放置タイムアウト (10 分) の抑止条件である「再生中」にストリーミング中を含める
- ストリーミング中は生存タイムアウト (60 秒) の判定にセグメント取得も活動として数える

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

- シークバーは自前 DOM。位置は `/api/video/state` のポーリング (1 秒間隔) と
  `<video>` の `currentTime` の合成で表示する
- バッファ不足 (`waiting` イベント) が 3 秒以上続いたら画質を 1 段下げる提案を出す
  (自動では下げない。§2 のとおり ABR は持たない)
- iOS のバックグラウンド復帰時はセッション生存を `/api/video/state` で確認し、
  失効していれば同じ位置で `start` をやり直す

---

## 8. 設定項目

`Settings` に追加する (本体側。remote-web は read-only で読む)。

| キー | 既定 | 内容 |
|---|---|---|
| `remote_video_streaming_enabled` | `true` | ストリーミング機能そのものの可否 |
| `remote_video_encoder` | `Auto` | `Auto` / `Nvenc` / `Qsv` / `Amf` / `MediaFoundation` / `OpenH264` |
| `remote_video_quality_default` | `Standard` | §6.4 のプリセット |
| `remote_video_segment_window` | `30` | 保持セグメント数 (= 60 秒) |
| `remote_video_mute_local_output` | `true` | ストリーミング中に PC 側スピーカーを無音にするか (§10-2) |

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

1. **画面表示に依存しないか (要実機検証)** — 映像を decoder から tap するため、原理上は
   presenter (PC 画面表示) と独立している。ただし現行の再生エンジンがフレーム消費を
   present と結び付けている場合、ウィンドウ最小化・モニタスリープで再生が止まる可能性がある。
   **最初に検証すべき項目**。止まる場合は、ストリーミング中に `ES_DISPLAY_REQUIRED` を
   追加で立てる (現行 watchdog は `ES_CONTINUOUS | ES_SYSTEM_REQUIRED` のみ) か、
   ヘッドレス消費経路を用意する
2. **PC 側の音** — mIV は音声出力がマスタークロックであり、デバイスを止めると映像も進まない
   恐れがある。`remote_video_mute_local_output` は**デバイスを回したまま音量 0** にする実装とし、
   ストリームは tap 点 (音量適用より前の最終 PCM) から取るため無音化の影響を受けない
3. **エンコード遅延の蓄積** — 実時間 1x でしか生成できないため、回線が平均的に足りない場合は
   遅延が伸び続ける。バッファ秒数を `/api/video/state` で監視し、閾値超過で画質降格を提案する
4. **同時 1 セッション** — 既存のセッションロックと同一なので追加の排他は不要
5. **HDR 素材** — 第 1 段は BT.709 固定。HDR (PQ/HLG) 素材はトーンマッピングせずに送ると
   眠い絵になる。実機で確認し、必要なら `zscale` / `tonemap` の導入を第 2 段で検討する
   (両フィルタとも LGPL ビルドに含まれる)
6. **インターレース素材** — deinterlace を通す必要がある。`yadif` は LGPL ビルドに含まれる
7. **可変フレームレート素材** — セグメント境界の GOP 固定と噛み合わない場合がある。
   エンコーダ入力側で CFR 化する
8. **通信量** — 標準画質 1 時間で約 730MB。従量回線での利用は画質選択と通信量表示で支援する

---

## 11. 実装段階

### 第 1 段 (本計画の主対象)

| 内容 | 概算 |
|---|---|
| エンコーダ抽象 + フォールバック階段 + H.264/AAC エンコード | 500〜700 行 |
| fMP4 セグメンタ (custom AVIO + リングバッファ + m3u8 生成) | 400〜600 行 |
| 映像 tap (decoder 分岐 + HW download + scale/NV12) | 300〜400 行 |
| 音声 tap (pump 分岐 + resample + AAC 供給) | 200〜300 行 |
| ストリーミングセッション管理 (本体側、既存 session lock 連携) | 300〜400 行 |
| IPC protocol v11 拡張 + remote-web 側の中継と HTTP ルート | 400〜500 行 |
| フロント (video 要素 + hls.js 分岐 + 自前コントロール + 画質 UI) | 400〜600 行 |
| 合計 | **2,500〜3,500 行** |

### 第 2 段 (第 1 段の実測後に判断)

- 映像補正 (grade pipeline) の反映 — presenter とは別のレンダーターゲットへ同じパスを通す
- 音楽ファイル / 動画の音声モードを同じ HLS 経路で配信 (audio-only variant)
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
- A/V 同期: 既知の pts 列に対してセグメントのタイムスタンプが単調増加し、
  音声 `audible_pts_secs` と映像 pts のずれが 1 フレーム以内であること
- セッション: 操作権喪失 / 生存タイムアウト / 放置タイムアウトでストリーミングが停止すること
- `cargo test -p mimageviewer --lib` と `cargo test -p mimageviewer-remote` が緑

### 実機 (ユーザーが実施)

- iPhone / iPad の Safari で、HEVC / AV1 / WMV / MKV いずれの素材でも再生できる
- VST3 チェインと音量ノーマライズが**リモート側の音に反映されている**
- 5G / 公衆 Wi-Fi それぞれで標準画質が途切れずに再生できる
- シーク・一時停止・画質変更が動作し、シーク後の再バッファが 5 秒以内
- **PC 側のウィンドウを最小化 / モニタをスリープさせても再生が継続する** (§10-1)
- ローカルへ操作権を戻すとストリーミングが停止し、本体の再生状態が壊れない

### 実機で記録する値

- 選択されたエンコーダとエンコード所要時間 (実時間比)
- 標準画質での実効ビットレートと 10 分あたりの通信量
- 再生開始までの時間、シーク後の再開までの時間
- バッファ枯渇 (`waiting`) の発生回数
