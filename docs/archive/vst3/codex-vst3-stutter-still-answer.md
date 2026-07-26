# Codex 回答: VST3 PDC 後も残る周期的スタッター

前提: 2026-05-01 時点の未コミット変更を含むコード読解ベースです。実機ログの再解析はブリーフ記載値を前提にしています。

## 結論

スタッター周期が `pace_lead = 0.30 + 1.650975 = 約1.95s` と一致しているのは偶然ではありません。主因は、PDC latency を **video frame の decode/render queue 用 pace_lead にそのまま足した**ことです。

現在の queue 容量は:

- `src/video/decoder.rs:231` `video_tx = bounded(24)`
- `src/video/mod.rs:158` `MAX_RENDER_QUEUE = 24`
- 合計 48 frames、60fps なら約 800ms

一方、decoder は `src/video/decoder.rs:1176-1180` / `1434-1438` で `pace_lead = 1.95s` 先まで decoded frame を作ろうとします。queue は 0.8s 分しか持てないので、UI 側 queue の先頭が `now + (1.95 - 0.8) ≒ now + 1.15s` になり、`src/video/mod.rs:711` の `front.pts_secs <= now + 0.016` を満たせません。その間 tick は呼ばれても表示可能 frame がなく、同じ frame が静止します。

したがって、前回の「demux/audio decode は PDC 分だけ先へ走れる必要がある」という助言の実装先がズレています。必要なのは **audio input / packet の lookahead** であって、decoded video frame queue を PDC 秒ぶん先に伸ばすことではありません。

## Q1: なぜ周期 1.95s でスタッターするのか

仮説1でほぼ正しいです。より正確には:

1. decoder は `pace_now + 1.95s` 付近まで video frame を生産する。
2. `video_tx + future_frames` は約 0.8s しか保持できない。
3. queue が future frames で満杯になり、以後 `dropped_full=true` が続く。
4. UI tick は queue 先頭を見るが、先頭 PTS が `now + 約1.0s` 以上先なので表示しない。
5. clock が queue 先頭に追いつくまで表示が静止する。
6. 追いついた後、溜まっていた約0.8sぶんを表示して、また同じ構造に戻る。

観測された「約770ms滑らか + 約980ms静止」は、この「queue span」と「pace_lead - queue span」の交互動作として説明できます。

## Q2: pace_lead を pdc 分増やす設計は必要か

video frame pacing に対しては不要、むしろ有害です。

PDC のために必要なのは、plugin に `now + pdc` 付近の **audio input** を供給することです。video frame を `now + pdc` まで decode して queue に置く必要はありません。現在の実装では demux/video decode と audio packet 供給が結びついているため、video `pace_lead` を増やすことで副作用的に demux を進めようとしていますが、decoded video frame queue の容量制約にぶつかっています。

短期修正としては、video の `pace_lead` は queue 容量以下に cap してください。

```rust
// src/video/decoder.rs
let pdc_latency = clock
    .vst3_pdc_latency_secs()
    .min(crate::video::dsp::MAX_PDC_LATENCY_SECS);

// decoded video queue が保持できる秒数を超えて先読みしない。
// 60fps 前提なら 48 frames = 0.8s。安全側に 0.60s 程度から開始。
const VIDEO_QUEUE_LEAD_CAP_SECS: f64 = 0.60;

let desired_audio_lookahead = pdc_latency;
let video_pace_lead = if allow_pace_lead {
    (PACE_LEAD_SECS + desired_audio_lookahead).min(VIDEO_QUEUE_LEAD_CAP_SECS)
} else {
    0.0
};

if ahead <= video_pace_lead {
    break;
}
```

ただし、これは「動画スタッターを止める」短期策で、PDC 用 audio lookahead の根治ではありません。

根本策は demux/audio lookahead と video decode queue を分離することです。理想形は:

- demux thread は compressed packet を audio/video packet queue に分配する。
- audio packet queue は `pdc + normal buffer` 分まで先読みを許可する。
- video decoder は `now + 0.3s` 程度だけ decode/render queue に積む。
- video の decoded frame queue は PDC latency に比例して増やさない。

## Q3: queue cap を増やすべきか

第一選択ではありません。

GPU 経路だけなら HANDLE 中心なので軽く見えますが、`future_frames` は CPU 経路も共通です。2 秒 PDC まで対応するには 60fps で `2.3s * 60 ≒ 138 frames` 近い decoded queue が必要になります。CPU 1080p RGBA なら 1GB 級になり得ます。

一時的な検証として GPU 経路限定で増やすのはありですが、設計としては「PDC 秒数ぶん decoded video を溜める」方向に進まない方がよいです。compressed video packet queue を増やす、または demux/audio だけを先に進める方が筋が良いです。

## Q4: lead_tol を PDC-aware にするべきか

しない方がよいです。

現状値は `src/video/clock.rs:148` の `DISPLAY_LEAD_TOLERANCE_SECS = 0.016` です。これは「1 vsync 程度先の frame は今出してよい」という表示誤差の許容です。

ここを PDC に合わせて 1.15s などに広げると、`front pts = now + 1.15s` の frame を今表示することになります。これは動画を 1.15s 早出しするのと同じで、PDC で守ろうとしている A/V sync を破壊します。スタッターを隠せても、映像と音声はズレます。

許容できる調整幅はせいぜい 16ms → 25ms / 33ms 程度です。PDC latency と連動させるべき値ではありません。

## Q5: 設計の根本見直しは必要か

「PDC で video clock を遅らせる」自体は間違いではありません。ただし、その結果として必要になる lookahead を decoded video queue で実現しようとしている点が問題です。

選択肢の評価:

- (a) video output 自体を pdc 秒だけ追加バッファする: A/V sync は理論上きれいですが、decoded frame メモリが大きすぎます。CPU 経路では厳しい。
- (b) audio-only PDC: 実装は簡単ですが、A/V ずれを受け入れることになり、PDC の意味が薄れます。デバッグ用 option ならあり。
- (c) plugin pre-roll: 再生開始時の安定化には有効ですが、再生中の steady-state lookahead 問題は解決しません。

推奨は (d) として、packet/demux レベルの lookahead 分離です。

```text
demux thread
  -> audio_pkt_tx: pdc + normal buffer ぶん先読み許可
  -> video_pkt_tx: compressed packet なら pdc 分持っても decoded frame より軽い

audio decode thread
  -> audio_tx / audio-pump / VST bridge

video decode thread
  -> decoded video_tx は now + 0.3s 程度に制限
```

現在すぐに大改造できないなら、短期策は次の順です。

## 推奨修正順

1. P1: video decoder の `pace_lead` から PDC 加算を外す、または queue 容量以下に cap する。
   - まず `PACE_LEAD_SECS` の 0.30s に戻すのが最も安全。
   - 音声途切れが再発するなら、audio lookahead を別経路で増やす。

2. P1: perf log に `pdc_latency`, `video_pace_lead`, `future_frames_len`, `video_rx_len` 相当を追加する。
   - `crossbeam_channel::Receiver::len()` が使えるなら tick log に入れる。
   - 「queue 先頭 PTS - now」もログに入れると今回の仮説を直接検証できます。

3. P2: demux/audio lookahead を video decoded queue から分離する。
   - 最終的には demux thread と video decode thread を分けるのが堅いです。
   - compressed packet queue で PDC 分を保持する方が decoded frame queue より安いです。

4. P3: GPU 経路限定の queue 拡大は検証用途に留める。
   - 本修正として queue を PDC 秒数ぶん増やすのは CPU 経路で破綻しやすいです。

## 最小パッチ案

まずスタッターを止めるだけなら、GPU/CPU 両方の `pace_lead` 計算を以下へ戻します。

```rust
let pace_lead = if allow_pace_lead {
    PACE_LEAD_SECS
} else {
    0.0
};
```

または、音声の再発リスクを見ながら中間案:

```rust
const VIDEO_QUEUE_LEAD_CAP_SECS: f64 = 0.60;
let pace_lead = if allow_pace_lead {
    (PACE_LEAD_SECS + pdc_latency).min(VIDEO_QUEUE_LEAD_CAP_SECS)
} else {
    0.0
};
```

この cap は `DISPLAY_LEAD_TOLERANCE` ではなく decoder 側に入れるべきです。表示側で未来フレームを許すと A/V sync が崩れますが、decoder 側で「decoded video を作りすぎない」ように制限するのは正しい方向です。

## 補足: なぜ音声は途切れていないのか

今回のログでは `audio_buf_secs ≈ 1070ms` で安定しています。これは Step 1 で actual buffer と PDC metric を分離した結果、音声側の補充は機能しているということです。残っている問題は audio underrun ではなく、video decoded queue が PDC-aware pace_lead に対して小さすぎることです。

したがって、ここで `lead_tol` や clock anchor を触るより、decoder pacing と queue 設計を分けて直すのが安全です。
