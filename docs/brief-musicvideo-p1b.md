# ブリーフ: 音楽ビデオ書き出し P1b — MP4 として書き出す

対象 worktree: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）
正本: [docs/music-video-export-plan.md](music-video-export-plan.md)。前段: [docs/brief-musicvideo-p1a.md](brief-musicvideo-p1a.md)（完了済み）。

P1a で `musicvideo-core` が 1920x1080 の RGBA フレームを 1.4ms で作れるところまで来た。
P1b は**そのフレームを既存のエンコーダに流し、MP4 ファイルとして書き出す**。

`vendor/` は既にこの worktree へ実コピー済み（junction ではない）。本体パッケージがビルドできる。

---

## 0. 絶対に守ること

1. **既存のリモート配信経路の挙動を変えない。** `src/video/stream/` は mIV Remote の HLS 配信が
   使っている。共通化のためのリファクタは、**本当に詰まったときだけ**行い、行った場合は
   ストリーミング側の挙動が変わっていないことを根拠付きで報告する。中立な値を渡して済むなら
   そちらを選ぶ。
2. **`crates/musicvideo-core` にエンコード・FFmpeg 依存を持ち込まない。** あの crate は純ロジックで、
   ラボが `vendor/` 無しでビルドできることが要件。エンコードは本体パッケージ側に置く。
3. **`comic-core` / `music-core` の公開 API を変えない。**
4. **エンコーダを新規に書かない。** H.264 と AAC は既にある（§2）。新規に作るのは MP4 muxer だけ。
5. `cargo fmt` を通す（pre-commit フックが番人）。commit は不要（こちらで行う）。

---

## 1. 作るもの

### 1.1 `src/video/mvexport/`（本体パッケージ）

```
mod.rs      公開入口: export_music_video(request, progress, cancel) -> Result<ExportSummary, ExportError>
driver.rs   フレームループ（解析 → シーン評価 → 合成 → 色変換 → エンコード → mux）
mux.rs      MP4 muxer（ffmpeg::format::output）
```

`Cargo.toml` の `[dependencies]` に `musicvideo-core = { path = "crates/musicvideo-core" }` を足す。

#### 入口の形

```rust
pub struct MusicVideoExportRequest {
    pub project: musicvideo_core::Project,
    pub assets: musicvideo_core::Assets,   // デコード済み（呼び出し側が用意する）
    pub audio_path: PathBuf,
    pub output_path: PathBuf,
    pub encoder: EncoderPreference,        // 既定 Auto
    pub video_bitrate_bps: u32,
}

pub struct ExportSummary {
    pub frames: u64,
    pub duration_secs: f64,
    pub encoder: String,                   // 実際に選ばれたエンコーダ名
    pub render_mean_ms: f64,
    pub render_p95_ms: f64,
    pub convert_mean_ms: f64,
    pub encode_mean_ms: f64,
    pub wall_secs: f64,
    pub realtime_ratio: f64,
}
```

- **進捗とキャンセル**: `progress: &dyn Fn(u64, u64)` と `cancel: &AtomicBool`。
  キャンセルされたら書きかけのファイルを消して `ExportError::Cancelled` を返す
  （中途半端な MP4 を残さない）。
- **UI スレッドから同期で呼ばれない前提**にする。呼び出し側がワーカーで回す
  （[docs/ui-responsiveness.md](ui-responsiveness.md) §4）。

#### 処理の流れ

```
音声: decode_audio_file_to_stereo_f32(audio_path)  →  stereo f32 / 48kHz（正本 D8）
        ├→ SpectrumSource（t 昇順で駆動）→ シーン評価に渡す
        └→ AAC エンコード → mux

映像: n = 0..frames { t = n/fps → evaluate → render_frame → RGBA→NV12 or YUV420p → H.264 } → mux
```

- **まず素直な逐次ループで書くこと。** 合成 1.4ms に色変換とエンコードを足しても目標
  （実時間の 2 倍）に届く見込み。**先に測ってから**、届かない場合だけ合成とエンコードを
  別スレッドに分けて有界リングで繋ぐ。最初からパイプライン化しない。
- 色変換は `src/video/swscale_helpers.rs` を見て、使えるものは使う。
  変換先は `OpenedH264Encoder::input_format`（`Nv12` か `Yuv420p`）に従う。
- PTS は `t = n/fps` から決まる CFR。実時間・壁時計から作らない（正本 D3）。

#### MP4 muxer（`mux.rs`）

- `ffmpeg::format::output(&path)` で mp4 を開き、映像 1・音声 1 のストリームを作る。
- **非フラグメント MP4 + faststart**（`movflags=+faststart`）。既存の
  `src/video/stream/segmenter.rs` は HLS 用のリングバッファなので**使わない**（正本 D7）。
- 映像・音声のパケットを DTS 順にインターリーブする。
- タイムベースはストリームごとに設定し、エンコーダのタイムベースから正しく `rescale` する
  （ここを間違えると音ズレになる。§4 の検証で必ず確認すること）。

### 1.2 開発用 CLI（検証の足場）

ラボ（`tools/musicvideo_lab`）は `vendor/` 非依存が要件なので、MP4 書き出しはラボに置けない。
本体パッケージに **dev-tools feature の bin** を足す（`bench_search` と同じ運用）。

```
cargo run --release -p mimageviewer --features dev-tools --bin mvexport_cli -- \
    --audio <path> --image <path> --out <path.mp4> [--seconds N]
```

- P1a の既定プロジェクト相当（背景画像 + 直線バー + 時間表示 + 進捗バー）を組んで書き出す。
- 終了時に `ExportSummary` を 1 行で出す。
- **書き出した MP4 を avformat で開き直して検証結果も出す**:
  ストリーム数 / コーデック名 / 解像度 / fps / 映像の総フレーム数 / 音声のサンプル数 /
  映像と音声それぞれの長さ（秒）。これが P1b の自動化された確認になる。

---

## 2. 再利用するもの（書き直さない）

| 用途 | 使うもの |
| --- | --- |
| H.264 エンコーダ（NVENC/QSV/AMF/MF/OpenH264 自動選択） | `src/video/stream/encoder.rs` の `open_h264_encoder(preference, StreamOutputParameters, FrameRate)` |
| 出力パラメータ | `StreamOutputParameters { dimensions, video_bitrate_bps, audio_bitrate_bps }` を直接組む（`QualityPreset` は HLS 用なので経由しない） |
| AAC エンコーダ | `src/video/stream/audio_encoder.rs` の `open_aac_encoder` |
| 色変換 | `src/video/swscale_helpers.rs` |
| 音声の全尺デコード | `src/audio_decode.rs` の `decode_audio_file_to_stereo_f32` |
| フレーム生成 | `musicvideo_core::{evaluate, render_frame, SpectrumSource}` |

`open_aac_encoder` は HLS 由来の引数（`expected_seek_serial` / `StreamTimeline`）を取る。
**中立な値を渡して済ませること。** 済まないと判断した場合だけ最小限の切り出しを行い、
ストリーミング側の挙動不変を根拠付きで示す。

---

## 3. スコープ外

グロー / グラデーション / 円形スペクトラム / 鍵盤 / キーフレーム UI / 本体 UI への組み込み /
プロジェクトの保存・読み込み。これらは P2 以降。

---

## 4. 完了条件

1. `cargo check -p mimageviewer --bin mimageviewer-core` が通る。
2. `cargo test -p mimageviewer --lib mvexport` が緑（純ロジック部分。FFmpeg を要する経路は
   CLI での確認に回してよい）。
3. **5 分程度の音声で 1920x1080 / 60fps の MP4 が書き出せる**。CLI の検証出力で、
   映像の総フレーム数が `round(duration * 60)` と一致し、**映像長と音声長の差が 100ms 以内**
   であること。
4. **決定性**: 同じ入力で 2 回書き出し、**エンコーダへ渡す直前の RGBA フレーム列のハッシュが
   一致する**こと。
   ⚠️ **MP4 ファイル自体のバイト一致を条件にしない。** ハードウェアエンコーダの出力や
   muxer のメタデータは実行ごとに変わり得る。正本 §7 の「フレームがビット一致」はこの意味。
   CLI に「フレームハッシュを出す」オプションを付けて確認できるようにすること。
5. **速度を実測して §6 に記録する**: 合成 / 色変換 / エンコードの内訳と実時間比、
   選ばれたエンコーダ名。逐次ループで目標に届いたかどうかを明記する。
6. `cargo fmt --check` が通る。

## 5. 検証コマンド

```
cargo check -p mimageviewer --bin mimageviewer-core
cargo test -p mimageviewer --lib mvexport
cargo run --release -p mimageviewer --features dev-tools --bin mvexport_cli -- --audio ... --image ... --out ...
cargo fmt
```

`cargo build` / `cargo test` を `-p` 無しで打たない（launcher の build.rs が
`target/release` の core / remote を要求して落ちる）。

## 6. 実測値（実装後に追記する）

2026-09-04、release build の逐次ループで測定した。

| 項目 | 実測値 |
| --- | ---: |
| フレーム数 | 18,000 |
| 出力 | 1920x1080 / 60fps / 300.000秒 |
| 背景画像 | 3840x2400 JPEG（`Fit::Cover`、初回に 1 回だけ 1920x1080 へ拡縮） |
| 選択エンコーダ | `h264_nvenc`（NV12、映像 12Mbps） |
| 合成 mean / p95 | 1.578ms / 2.003ms |
| RGBA→NV12 色変換 mean | 3.913ms |
| H.264 エンコード mean | 0.412ms |
| wall time / 実時間比 | 109.672秒 / **2.735倍** |
| 再デコード映像 | H.264、18,000フレーム、300.000000秒 |
| 再デコード音声 | AAC、48kHz、14,400,512 samples/ch、300.010667秒 |
| 映像長と音声長の差 | **10.667ms** |
| CPU / GPU | Intel Core Ultra 9 285 / NVIDIA GeForce RTX 4090 |

合成値は各フレームの `SpectrumSource::frame_at`、scene 評価、CPU rasterize を含む。
色変換値は RGBA の FFmpeg frame へのコピーと、BT.709 / limited-range を明示した swscale の
NV12/YUV420p 変換を含む。H.264 値は `avcodec_send_frame` と packet drain（末尾 flush を含む）で、
mux 書き込みは含まない。wall time は音声の全尺デコード、AAC-LC 192kbps エンコード、上記の映像処理、
DTS 順の MP4 mux、`+faststart` の moov 移動、一時ファイルから完成ファイルへの rename を含む。
呼び出し側が事前に行う画像・フォントのデコード、終了後の MP4 再デコード検証、任意の RGBA SHA-256
計算は wall time に含まない。

測定用音声は OS 付属 PCM を 300 秒へ合わせ、元音声より後ろを無音でパディングした。全 18,000
フレームで実 FFT と AAC encoder は駆動しているが、AAC の信号複雑度は実楽曲より低い。
映像側の合成・色変換・H.264 と全体 wall time の判断材料として採用し、逐次ループのまま性能目標
（実時間の 2 倍）を上回ったためパイプライン化は行わない。

決定性は同じ入力の 10 秒 / 600 フレーム書き出しを 2 回行い、エンコーダ投入直前の RGBA 列が双方
`5a45396237682efae5a345bf8c20ba75a04ffb3043e28324952067ca03f64b09` (SHA-256) になることを確認した。
MP4 ファイル自体のバイト一致は判定に使っていない。両方とも再デコード時の映像は 600 フレーム、
映像 10.000000秒 / 音声 10.005333秒（差 5.333ms）だった。
