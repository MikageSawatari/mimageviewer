# Codex 回答: VST3 PDC 高 latency 時の音声途切れ

前提: 2026-05-01 時点の `C:/home/mimageviewer` の未コミット変更を含むコード読解ベースです。実機再現はしていません。

## 結論

最も疑わしい真因は、仮説3を少し言い換えたものです。`publish_buffer_secs` が PDC latency を `audio_pump_buf_secs` に足したことで、decoder pacing が「実際の音声出力バッファ残量」ではなく「構造的 latency を含む見かけの残量」を見ています。

特に問題なのは、`src/video/audio.rs:515-520` の完全 underrun 経路でも `publish_buffer_secs(&buf, clock)` が呼ばれ、`src/video/audio.rs:220-223` で `secs + buf.pdc_latency_secs` が publish されることです。実際には `AudioBuffer.samples` が空なのに、1000ms latency なら `clock.total_audio_buffer_secs()` は 1 秒以上に見えます。その結果、`src/video/decoder.rs:1118-1123` / `1379-1384` の `audio_escape` が発動せず、`AUDIO_CRITICAL_LO` の emergency 補充も効きません。これは音切れ時ほど decoder が「音声は安全」と誤認する構造です。

もう一つの本質は、現在の PDC が「映像クロックを遅らせる」方式なのに、音声入力側を latency 分だけ先に供給する仕組みが足りないことです。1000ms latency の plugin を鳴らすには、plugin delay-line へ少なくとも約 1 秒ぶん先の input を供給し続ける必要があります。現状の物理的な即時リザーバは概算で `AudioBuffer 300ms + audio_tx 32 frames * 約21ms = 約980ms` なので、1000ms 強で破綻しやすいです。修正C以前は actual buffer が低く見えて `audio_escape` が暴れ気味に補充していたため、音声だけは偶然保っていた可能性が高いです。

## 質問 1: 真因として最も疑わしいもの

優先度順です。

1. P1: `publish_buffer_secs` の PDC 加算が underrun 検出を無効化している
   - 場所: `src/video/audio.rs:220-223`, `src/video/audio.rs:515-520`, `src/video/decoder.rs:1118-1167`, `src/video/decoder.rs:1379-1414`
   - 実バッファが 0 でも `audio_buf >= 1.0s` に見えるため、decoder が補充しない。

2. P1: PDC latency 分の decode / demux lookahead が不足している
   - 場所: `src/video/decoder.rs:1057` / `1346` の `PACE_LEAD_SECS = 0.30`, `src/video/decoder.rs:232` の `audio_tx` cap 32, `src/video/audio.rs:284` の `TARGET_BUFFER_SECS = 0.3`
   - video clock を 1 秒遅らせたのに、demux/audio decode が 1 秒以上先へ走る設計になっていない。

3. P2: `pull_audio(..., 100)` の timeout による silence 埋め
   - 場所: `src/video/dsp/mod.rs:916`, `src/video/dsp/bridge.rs:461-508`
   - 高 latency そのものより、plugin 処理が時々 100ms を超えると silence が混ざる。ただし Test Latency の単純 delay-line なら主因とは考えにくい。

4. P3: bridge ring 80ms 不足
   - 場所: `src/video/dsp/bridge.rs:274-276`, `crates/vst3-host/include/protocol.h`
   - ジッタ吸収としては薄いが、今回の「修正C後に退行」とは直接の説明力が弱い。

仮説3の CPU 競合説は「あり得るが第一候補ではない」です。修正Cが decoder pacing を変えたのは確かですが、より直接的には「actual buffer underrun を隠してしまう」ことと「PDC 分の先読みを許可していない」ことが問題です。

## 質問 2: 2 秒上限 + auto-bypass

方針Aは保険として妥当です。ただし根本対策ではなく、PDC/decoder pacing 修正と併用するものです。

チェックタイミングは `total_latency_samples()` の毎 pump push ではなく、latency 値を slot に反映したタイミングに寄せるのがよいです。毎 ~21ms で bypass 判定とログを行うと、ログ連打や UI 状態の揺れを招きます。

推奨:

```rust
const MAX_PDC_LATENCY_SECS: f64 = 2.0;

fn refresh_slot_latency_and_policy(&self, idx: usize) {
    let sr = self.sample_rate().max(1);
    let max_samples = (MAX_PDC_LATENCY_SECS * sr as f64) as u32;
    let mut inner = self.inner.lock().unwrap();
    let Some(s) = inner.slots.get_mut(idx) else { return };
    let latest = s.bridge.cached_latency_samples_value();
    if latest == u32::MAX || latest == s.latency_samples {
        return;
    }
    s.latency_samples = latest;
    if latest > max_samples {
        s.bypass = true;
        s.auto_bypassed_for_latency = true;
        crate::logger::log(format!(
            "[VST3 PDC] auto-bypass '{}' latency={} samples exceeds {:.1}s",
            s.plugin_name.as_deref().unwrap_or("?"),
            latest,
            MAX_PDC_LATENCY_SECS,
        ));
    }
    self.recalc_active_count(&inner);
}
```

ユーザーが手動で ON に戻した場合、同じ latency がまだ 2 秒超なら再度 auto-bypass でよいです。ただし UI では「latency が上限超過のため ON にできません」相当の表示にした方が親切です。

settings への永続化はしない方がよいです。auto-bypass は安全装置であり、ユーザーの明示設定ではありません。再起動後に plugin 側 latency が戻っている可能性もあるため、runtime only が自然です。手動 bypass だけを `Vst3PluginEntry.bypass` に保存するのがよいです。

## 質問 3: 修正Cを撤回するべきか

そのまま維持はおすすめしません。ただし単純に撤回すると、ブリーフにある video スタッターが戻ります。

筋が良いのは「物理バッファ残量」と「PDC 構造 latency」を分離することです。

- `audio_pump_buf_secs`: actual `AudioBuffer.samples` のみ
- `audio_tx_queued_secs`: actual `audio_tx` queue のみ
- `pdc_latency_secs`: 別 atomic で公開
- decoder の underrun / `audio_escape`: actual buffer を見る
- decoder の先読み許容量: `PACE_LEAD_SECS + pdc_latency_secs + margin` を見る

コード方針:

```rust
// src/video/audio.rs
fn publish_buffer_secs(buf: &AudioBuffer, clock: &AvClock) {
    let secs = buf.samples.len() as f64 / buf.samples_per_sec;
    clock.set_audio_pump_buf_secs(secs); // PDC は足さない
    clock.set_vst3_pdc_latency_secs(buf.pdc_latency_secs); // 新規
}
```

decoder 側:

```rust
let physical_audio_buf = clock.total_audio_buffer_secs();
let pdc = clock.vst3_pdc_latency_secs();

if audio_active {
    if physical_audio_buf < AUDIO_SAFE_LO {
        in_audio_escape = true;
    } else if physical_audio_buf >= AUDIO_SAFE_HI {
        in_audio_escape = false;
    }
}

let pace_lead = if allow_pace_lead {
    PACE_LEAD_SECS + pdc.min(MAX_PDC_LATENCY_SECS)
} else {
    pdc.min(MAX_PDC_LATENCY_SECS)
};
```

この方向なら、video clock は遅らせつつ、demux/audio decode は plugin に必要な未来入力を供給できます。`audio_escape` は実残量で発動するので、完全 underrun 時にも補充が止まりません。

## 質問 4: bridge ring buffer 80ms は適切か

低 latency 再生だけを見るなら 80ms は過剰なくらいですが、mIV のような別プロセス bridge + 動画デコード併用では少し薄いです。特に `pull_audio` は 100ms deadline なので、ring が 80ms しかないと「bridge 側が一瞬詰まる」「Rust 側が少し寝る」の両方を吸収しにくいです。

ただし、1000ms PDC の主因を bridge ring で解決しようとするのは違います。plugin の構造 latency 1 秒分を IPC ring に持つ必要はありません。必要なのは demux/audio decode の lookahead と actual output buffer の underrun 検出です。

実用案:

- まず 80ms → 250ms or 300ms に増やす
- memory cost は stereo f32 でも小さい
- protocol の両側定数を合わせる

```rust
// src/video/dsp/bridge.rs
const AUDIO_PIPE_BLOCK_MARGIN: u32 = 32; // 8 -> 32, 約320ms@480 block
let capacity = block_size * 2 * AUDIO_PIPE_BLOCK_MARGIN;
```

```cpp
// crates/vst3-host/include/protocol.h
// capacity = block_size * channels * AUDIO_PIPE_BLOCK_MARGIN
```

DAW の sandbox host は実装により差がありますが、通常は「数 block から数十 ms の realtime IPC」と「別途 PDC/lookahead scheduling」を分けます。構造 latency を IPC ring 容量で吸収する設計ではありません。

## 質問 5: 見落とし候補

### P1: 完全 underrun 時も PDC 加算で安全に見える
これが最大の見落としです。`fill_output` が silence を出しているときほど、decoder に actual shortage を知らせる必要があります。

### P1: PDC latency 分の先読み不足
`AudioBuffer 300ms + audio_tx 約680ms` は 1000ms 強の latency に対してほぼ限界です。2 秒まで許容するなら、2 秒ぶんの input を plugin に供給できる decode lookahead と queue 設計が必要です。単純に output buffer cap を 2 秒にする必要はありませんが、demux/audio decode が PDC 分だけ先へ走れる必要があります。

### P2: `audio_pkt_tx` は未処理 packet なので PDC 済み出力ではない
`audio_pkt_tx` 64 packets は余裕として存在しますが、plugin delay-line へ投入済みではありません。PDC の実効リザーバとして数えるなら、少なくとも audio decode + VST process が済んでいる必要があります。

### P2: event-pump channel の backpressure
`Bridge::spawn` の `event_tx` は bounded 64 です。`LatencyChanged` は channel に流さないので今回の主因ではなさそうですが、Error など通常 event が大量に出ると pump が stdout read を止め、bridge 側 `write_message` が詰まる可能性はあります。audio thread から `send_event_error` する経路があるので、将来的には `try_send` + drop/log の方が安全です。

### P3: `total_latency_samples()` の毎 frame Mutex
これは主因ではなさそうです。slots 数が少なければ軽い。ただし auto-bypass の副作用をここに入れるなら、ログ連打を避ける状態管理が必要です。

## 推奨する修正順

1. `publish_buffer_secs` から PDC 加算を撤去し、PDC latency を別 metric に分離する。
2. decoder pacing を PDC-aware にし、actual audio buffer で `audio_escape`、`PACE_LEAD + pdc` で先読み許可に分ける。
3. `MAX_PDC_LATENCY_SECS = 2.0` の runtime auto-bypass を追加する。永続設定には保存しない。
4. bridge ring を 250-300ms 程度へ増やす。これは主修正ではなくジッタ保険。
5. 計測ログを追加する。最低限、`AudioBuffer.samples actual`, `audio_tx_queued`, `pdc_latency`, `pull_audio partial`, `cpal underrun count`, `audio_escape state` を同じ perf-log に出す。

## すぐ入れる診断ログ案

```rust
// fill_output: underrun 時
if real_consumed < want {
    clock.report_audio_underrun(want - real_consumed);
}
```

```rust
// DspBridge::process_block: partial pull
let n = bridge.pull_audio(dst, 100)?;
if n < dst.len() {
    crate::logger::log(format!(
        "[VST3 audio] partial pull: got={} want={} timeout_ms=100",
        n,
        dst.len(),
    ));
}
```

これで「plugin/bridge が遅い」のか「upstream が供給していない」のかを分けられます。今回の症状説明としては、後者が第一候補です。
