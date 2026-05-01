# AV1 D3D11VA HW Decode 実装方針レビュー

作成日: 2026-05-01

## 背景

ユーザー環境で `C:\home\youtube\download\001 - お返事まだカナ💦❓おじさん構文😁❗️ ⧸ 雨衣 [8E8aWeY-pAc].mp4`
を再生したところ、P キー perf overlay / `mimageviewer.log` で以下を確認した。

```text
codec=av1 decoder=libdav1d hw_requested=true d3d11va_supported=false hw_active_initially=false gpu_path=true d3d11va_config=none
```

つまり、AV1 stream 自体は検出できているが、FFmpeg の既定 decoder が `libdav1d` に
なっており、この decoder は D3D11VA HW config を出していない。結果として AV1 は
現状 SW decode になっている。

一方、H.264 では以下を確認済み。

```text
codec=h264 decoder=h264 hw_requested=true d3d11va_supported=true hw_active_initially=true gpu_path=true d3d11va_config=idx=2,pix_fmt=AV_PIX_FMT_D3D11,methods=0x3,device_ctx=true
```

既存の D3D11VA device 共有、GPU blit、CPU fallback の基盤は動作している。

## 目的

AV1 で D3D11VA HW decode を利用できる環境では HW decode を優先し、利用できない環境では
従来通り `libdav1d` / SW decode に自動 fallback する。

副目的:

- AV1 以外でも「既定 decoder は HW 非対応だが、別 decoder なら D3D11VA 対応」という
  コーデックがあれば同じ仕組みで拾えるようにする。
- ただし今回の初回実装では AV1 を主対象にし、影響範囲を広げすぎない。
- P キー perf overlay とログで、どの decoder が選ばれたかを確認できる状態を維持する。

## 現行実装の要点

対象: `src/video/decoder.rs`

- `Context::from_parameters(video_params)` で codec context を作る。
- `codec_id = video_decoder_ctx.id()` を取得する。
- `hw_decode_requested` が true の場合、`try_init_d3d11va(codec_id, &mut video_decoder_ctx, gpu_video_device.as_ref())`
  を呼ぶ。
- `try_init_d3d11va` は内部で `avcodec_find_decoder(codec_id)` を呼び、選ばれた decoder に
  D3D11VA `AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX` + `AV_PIX_FMT_D3D11` config があるかを見る。
- HW device ctx を `AVCodecContext.hw_device_ctx` に設定し、`get_format` callback で
  `AV_PIX_FMT_D3D11` を選ぶ。
- その後 `video_decoder_ctx.decoder().video()` で decoder を open する。
- HW open 失敗時は context を作り直して SW retry する。

AV1 で問題になる点:

- `avcodec_find_decoder(AV_CODEC_ID_AV1)` が `libdav1d` を返す環境では、D3D11VA config が無い。
- そのため native `av1` decoder が D3D11VA に対応していても、現行経路では試されない。

## 実装案

### 1. Decoder candidate を明示的に選ぶ

新しい helper を追加する。

```rust
struct VideoDecoderCandidate {
    name: String,
    reason: &'static str,
}

fn preferred_video_decoders(codec_id: ffmpeg::codec::Id, hw_decode_requested: bool) -> Vec<VideoDecoderCandidate>
```

初回実装の候補:

- `AV1` + `hw_decode_requested=true`
  - まず native decoder `"av1"` を試す。
  - その後に既定 decoder (`avcodec_find_decoder(codec_id)`) を試す。
- その他 codec
  - 既定 decoder のみ。

理由:

- AV1 では `libdav1d` が既定になっている可能性が高く、D3D11VA を使うには native decoder を
  明示選択する必要がある。
- H.264 / HEVC は現状の既定 decoder で D3D11VA config が取れており、不要な挙動変更を避ける。

実装前提の確認:

- BtbN の同梱 FFmpeg build に native `"av1"` decoder が存在することを、`find_by_name("av1")`
  の戻り値と candidate ログで確認する。
- native `"av1"` decoder が存在しても D3D11VA config を持たない build では、この実装は
  fallback するだけで HW decode にはならない。

将来拡張:

- `VP9` などで同様の問題が確認されたら候補 table に追加する。
- 候補 table は codec id ごとの局所的な定義にして、広い decoder 探索は初回では行わない。

### 2. Context open を candidate 単位にする

現行の `video_decoder_ctx.decoder().video()` は `Decoder::video()` 内で再び `super::find(self.id())`
を使うため、明示 decoder を使えない。

そのため、候補 decoder を使う場合は `open_as(codec).and_then(|o| o.video())` 相当を使う。
`ffmpeg-the-third` には `ffmpeg::codec::decoder::find_by_name(name)` と
`Context::decoder().open_as(codec)` がある。

想定フロー:

1. candidate ごとに video stream parameters から fresh `Context` を作る。
2. candidate decoder を選ぶ。
3. HW requested の場合、その candidate の D3D11VA config を確認する。
4. D3D11VA 対応なら `hw_device_ctx` / `get_format` を設定する。
5. `open_as(candidate_codec)` で開く。
6. 成功したら採用。
7. 失敗したら次 candidate へ。
8. すべて失敗したら既存と同等のエラーにする。

### 3. `try_init_d3d11va` を codec pointer 対応にする

現行:

```rust
fn try_init_d3d11va(codec_id, ctx, gpu_video_device) -> Option<HwDevice>
```

変更案:

```rust
fn try_init_d3d11va_for_codec(
    codec_name_for_log: &str,
    codec: ffmpeg::Codec,
    ctx: &mut ffmpeg::codec::context::Context,
    gpu_video_device: Option<&Arc<GpuVideoDevice>>,
) -> Option<HwDevice>
```

内部の `avcodec_get_hw_config` は `codec.as_ptr()` に対して実行する。
これで `libdav1d` と native `av1` の D3D11VA config を個別に判定できる。

既存の `probe_d3d11va(codec_id)` も candidate decoder 名を受け取る形に寄せる。
P overlay / log には、最終採用した decoder の D3D11VA config を表示する。
`video/open` perf event の既存 field name (`d3d11va_supported` / `d3d11va_config`) は
維持し、解析スクリプトや手動 grep の互換性を保つ。

### 4. fallback 方針

fallback は必ず維持する。

- native `av1` + D3D11VA init 成功 + open 成功
  - HW decode 採用。
- native `av1` が存在しない / D3D11VA config なし / device 作成失敗 / open 失敗
  - 次 candidate に進む。
- 既定 decoder が `libdav1d` で open 成功
  - 従来通り SW decode 採用。
- `hw_decode_requested=false`
  - 従来通り既定 decoder を使う。native `av1` 優先はしない。

注意:

- native `av1` decoder が D3D11VA config を持たない FFmpeg build では改善しない。
- GPU / driver が AV1 decode 非対応の場合も `get_format` や open で fallback する想定。
- P overlay には `codec av1 / av1 HW/GPU D3D11VA:yes` もしくは
  `codec av1 / libdav1d SW/GPU D3D11VA:no` のように出る。

known limitation:

- D3D11VA config 宣言と実 decode 成功は同義ではない。native `"av1"` で HW init/open が
  成功しても、1 frame 目の `get_format` で D3D11 が候補に出ず SW format が選ばれる可能性がある。
  この場合は native `"av1"` の SW decode で走り続け、`libdav1d` SW より遅くなる恐れがある。
  初回実装では自動再構成までは行わず、P overlay / log / perf の `first_frame` 診断で検出する。

### 5. ログ / perf

既存の診断情報を継続し、candidate 試行ログを追加する。

通常ログ:

```text
video decoder candidate: codec=av1 decoder=av1 reason=av1_hw_preferred d3d11va_supported=true
video decoder selected: codec=av1 decoder=av1 decode_path=hw_d3d11va gpu_path=true
video decoder candidate failed: codec=av1 decoder=av1 stage=open err=...
```

perf `video/open`:

- `video_codec`
- `video_decoder`
- `hw_decode_requested`
- `d3d11va_supported`
- `d3d11va_config`
- `decode_path`

perf `video/first_frame`:

- `frame_pix_fmt` は既存通り維持する。
- 可能なら `get_format_chosen` 相当を後続タスクで追加し、D3D11VA init 成功後の silent SW fallback
  を判定しやすくする。

初回では per-candidate perf event は増やさず、通常ログ中心で十分と考える。

## 影響範囲

主に以下のみ。

- `src/video/decoder.rs`
  - decoder candidate 選択
  - D3D11VA probe / init helper の引数変更
  - open fallback の整理
- `src/ui_fullscreen.rs`
  - 既に P overlay 表示済み。必要なら文言微調整のみ。
- `src/ui_video_panels.rs`
  - 既に左パネル表示済み。必要なら文言微調整のみ。
- `docs/video-architecture.md`
  - 実装後に candidate fallback の説明を追記。

thumbnail / tile thumbnail worker は対象外。
これらは再生用 decoder とは独立しており、初回実装で HW decode を入れると影響が広がるため。

## リスクと確認ポイント

### R1: `open_as(codec)` への移行で既存 codec の挙動が変わる

対策:

- AV1 + HW requested 以外は既定 decoder 1 個だけにして、現行と同じ decoder を使う。
- `codec_id` と candidate decoder の id が一致することを確認し、不一致なら skip する。

### R2: `HwDevice` の lifetime / retry 時の drop

対策:

- candidate ごとに fresh `Context` と fresh `HwDevice` を作る。
- candidate 失敗時に `HwDevice` を drop して次へ進む。
- 採用した `HwDevice` だけを既存通り `_hw_device` として保持し、video decode thread の lifetime まで維持する。

### R3: native `av1` が SW でも `libdav1d` より遅い

対策:

- `hw_decode_requested=true` かつ D3D11VA config がある場合だけ native `av1` を優先。
- D3D11VA config が無い場合は native `av1` を SW fallback としては使わず、既定 decoder に進む。

### R4: HW init は成功したが実際の frame が D3D11 ではない

既存でもあり得る。現行の `first_frame` perf event が `hw_d3d11va` /
`sw_fallback_after_hw_init` / `sw` を出すので、これで検出する。初回実装ではこの状態を
検出対象に留め、採用済み native `"av1"` から `libdav1d` へ再構成する処理は将来課題とする。

## テスト計画

自動:

```powershell
$env:LIBCLANG_PATH='C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\Llvm\x64\bin'
cargo test --lib video::
```

追加 unit test:

- `preferred_video_decoders(Id::H264, true)` と `preferred_video_decoders(Id::HEVC, true)` が
  既定 decoder 1 個だけを返すことを確認し、AV1 対応で H.264 / HEVC を巻き込まないようにする。

手動:

1. `C:\home\youtube\download\001 - お返事まだカナ💦❓おじさん構文😁❗️ ⧸ 雨衣 [8E8aWeY-pAc].mp4`
   を開く。
2. P overlay で以下を確認する。
   - 成功期待: `codec av1 / av1  HW/GPU  D3D11VA:yes`
   - fallback: `codec av1 / libdav1d  SW/GPU  D3D11VA:no`
3. `mimageviewer.log` で candidate 試行と採用 decoder を確認する。
4. シークを複数回行い、AV1 のシーク体感とフリーズ有無を見る。
5. 既存 H.264 / HEVC サンプルで HW decode が維持されることを確認する。

## ClaudeCode レビュー依頼事項

1. `libdav1d` が既定 decoder になる環境で、native `av1` を明示選択する方針は妥当か。
2. `hw_decode_requested=true` のときだけ native `av1` を優先し、SW fallback では既定
   decoder に戻す方針に抜けがないか。
3. `try_init_d3d11va` を codec pointer ベースにする設計で lifetime / ownership の問題がないか。
4. candidate ごとに fresh `Context` + fresh `HwDevice` を作る fallback 設計で、既存の
   H.264 / HEVC path に regression が入りにくいか。
5. 初回実装で AV1 のみに絞るべきか、VP9 なども同じ candidate table に含めるべきか。
6. 手動テスト観点として、AV1 seek の重さを見るために追加すべきログや perf 項目があるか。
