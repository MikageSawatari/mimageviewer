# Codex 回答: VST3 PDC queue cap 後の微小 frame drop

前提: 2026-05-01 時点の未コミット変更と `docs/archive/vst3/codex-vst3-microdrop-brief.md`、および `perf_events.jsonl` の PDC 1979ms 以降を確認した判断です。

## 結論

真因解釈はかなり正しいです。PDC 1979ms 以降の実ログを切ると、概算で:

- decode events: 2972
- `dropped_full=true`: 540、約 18.2%
- `audio_buf_secs`: min 33.9ms / avg 74.2ms / max 84.2ms
- `ahead`: min 707ms / avg 954ms / max 973ms

となっており、`pace_lead = 0.60s` を入れたのに `ahead` が約 0.95s まで膨らんでいます。これは通常の pacing 条件では説明できず、`audio_escape` の bypass が効いていると見るのが自然です。

ただし、根は `AUDIO_CRITICAL_LO = 80ms` だけではありません。現在の `in_audio_escape` は各 frame の pacing loop 内で `false` 初期化され、その直後に `audio_buf < AUDIO_SAFE_LO` なら `true` になります。PDC 時の steady state が 70-80ms なら、実質的に毎 frame `in_audio_escape = true` です。その上で `audio_buf < AUDIO_CRITICAL_LO` が頻繁に真になり、`pace_lead` を超えた送出が許可されています。

したがって、修正の主眼は **audio low を理由に decoded video frame queue を future frame で満杯にしない** ことです。

## Q1: 真因は audio_escape bypass か

はい。ログ上の整合性は高いです。

`audio_buf` が平均 74ms で `AUDIO_CRITICAL_LO = 80ms` の境界をまたぎ、`ahead` が 0.60s cap を超えて 0.95s まで伸び、同時に `dropped_full` が約 18% 出ています。48 frame queue の 60fps 換算 span は約 800ms なので、queue 先頭が未来側に押し出され、UI が `front.pts <= now + lead_tol` を満たせず 100-150ms 程度の表示 gap を作る、という説明で合います。

特に重要なのは、`audio_buf < CRITICAL` による bypass が「音声を助ける」経路になっていない点です。現在は demux / audio decode / video decode が分離済みなので、video decoded frame を過剰生産しても audio buffer が直接増えるわけではありません。むしろ video queue full と future frame 化で表示側を壊しています。

## Q2: A-E のどれが良いか

推奨は **B + C-lite** です。

### 第 1 候補: B

`audio_buf < AUDIO_CRITICAL_LO` を、video frame pacing の `pace_lead` bypass 条件から外すのが本命です。

```rust
if in_audio_escape {
    if ahead < SEEK_BURST_LEAD_MAX_SECS {
        break;
    }
    // audio_buf < AUDIO_CRITICAL_LO だけでは decoded video を先へ出さない
}
```

理由は単純で、actual audio buffer が少ないことは「decoded video frame を 0.95s 先まで queue に詰めてよい」根拠にならないためです。audio emergency と video frame enqueue は別の制御に分けるべきです。

### 併用推奨: C-lite

`AUDIO_SAFE_LO/HI` は 300ms cap 時代には高すぎます。少なくとも VST 有効時、`SAFE_LO=250ms` は常時 escape になる値です。

まずは以下程度に下げるのが妥当です。

```rust
const AUDIO_SAFE_LO: f64 = 0.10;
const AUDIO_SAFE_HI: f64 = 0.20;
const AUDIO_CRITICAL_LO: f64 = 0.03;
```

ただし `AUDIO_CRITICAL_LO` を 30ms に下げるだけの案 A は、今回の本質修正としては弱いです。30ms を下回った瞬間に同じ「unbounded video bypass」が再発するため、A 単独では再発条件を遠ざけるだけです。

### E は限定的に可

E は「何に使うか」を分ければ正当化できます。

良い使い方:

- `audio_buf_actual`: underrun / critical 判定用
- `audio_buf_for_escape_mode = audio_buf_actual + pdc_latency`: safe/escape mode の誤発動抑制用

悪い使い方:

- actual underrun 判定そのものに PDC を足す
- `audio_buf_actual` が空でも「PDC があるから buffer は十分」と扱う

つまり、E は `AUDIO_SAFE_LO` への常時突入を防ぐ補助としてはありですが、`audio_buf < CRITICAL` の video bypass を残したままだと設計がまた曖昧になります。順番としては B を先に入れ、その後に必要なら E の分離形です。

### D は非推奨

`VIDEO_QUEUE_LEAD_CAP_SECS` を増やすのは今回の症状には逆方向です。queue cap を増やせば gap は短くなる可能性がありますが、decoded frame memory と future frame 表示不能問題を抱えたままです。CPU path では特に危険です。

## Q3: VST OFF 時の audio_buf 前提

コードコメント上の前提は「典型的な audio_buf は 200-400ms 範囲」です。`src/video/audio.rs` でも、旧 500ms ready threshold に届かないため 150ms に下げた経緯が書かれています。

ただし現在の `total_audio_buffer_secs()` は `AudioBuffer.samples + audio_tx queued` なので、AudioBuffer cap 300ms を超える値も普通に出ます。VST OFF 時は 200-400ms 程度、条件によっては queued 分を含めてそれ以上、という前提だったはずです。

その意味で、`AUDIO_SAFE_LO=250ms` は VST OFF ではまだ意味があり得ますが、VST ON かつ大 PDC では actual buffer が 70-80ms に落ちるため、同じ閾値をそのまま使うのは適切ではありません。VST の有無で閾値を分けるか、`audio_buf_for_escape_mode` を別定義にする方が自然です。

## Q4: AudioBuffer cap 300ms を増やすべきか

今回の第一修正ではありません。

cap を 500ms にすれば actual buffer が増える可能性はありますが、cap はあくまで上限です。今回の低水位は、PDC 2s 近辺で demux/audio/video の先読み構造が詰まり、pump が 300ms まで安定して溜められていないことの症状に見えます。cap を増やしても根の制御が直らなければ、遅延だけ増えて同じ問題が残る可能性があります。

ユーザー操作への音声反映を重視して 300ms に縮めた経緯もあるので、500ms 固定へ戻すより、必要なら VST 有効時だけ設定可能にする程度がよいです。

## Q5: audio decoder 側の独立 pacing は必要か

長期的には必要です。ただし今回の微小 drop を止めるための最初の一手ではありません。

今の構造は audio decode thread 自体は独立していますが、demux は `video_pkt_tx` / `audio_pkt_tx` の bounded queue による backpressure を受けます。video decode 側を 0.60s cap で止めると、video packet queue が詰まり、demux の進行もそこで制約されます。PDC 2s の plugin に対して audio だけ十分に先読みするには、decoded video frame queue ではなく、audio packet / compressed packet レベルで PDC 分の余裕を持たせる設計が必要です。

推奨する長期形は:

- decoded video queue は `now + 0.3-0.6s` 程度に維持
- audio packet / audio decode は `now + pdc + normal_buffer` まで進める
- demux が video packet queue full で完全停止しないよう、audio priority drain か packet queue policy を分ける
- actual audio underrun は actual buffer で検出し、PDC latency で隠さない

## 推奨実装順

1. P1: GPU/CPU 両 pacing loop から `audio_buf < AUDIO_CRITICAL_LO` による `pace_lead` bypass を外す。
2. P1: `AUDIO_SAFE_LO/HI/CRITICAL_LO` を 300ms cap 前提に再調整する。まず `0.10 / 0.20 / 0.03`。
3. P1: perf log に `in_audio_escape`, `pace_lead`, `audio_buf_actual`, `ahead`, `video_tx_is_full` 相当を追加し、bypass が消えたことを確認する。
4. P2: まだ音声 buffer が不安定なら、E の分離形を safe/escape mode 判定だけに導入する。critical underrun 判定には PDC を足さない。
5. P2/P3: audio packet lookahead と demux backpressure を分離し、PDC 2s でも audio だけ先読みできる構造にする。

## 最小パッチ方針

まずは以下の方向です。

```rust
const AUDIO_SAFE_LO: f64 = 0.10;
const AUDIO_SAFE_HI: f64 = 0.20;
const SEEK_BURST_LEAD_MAX_SECS: f64 = 0.20;
const AUDIO_CRITICAL_LO: f64 = 0.03;

if audio_active {
    if audio_buf < AUDIO_SAFE_LO {
        in_audio_escape = true;
    } else if audio_buf >= AUDIO_SAFE_HI {
        in_audio_escape = false;
    }
}

if ahead <= pace_lead {
    break;
}

if in_audio_escape && ahead < SEEK_BURST_LEAD_MAX_SECS {
    break;
}
```

`AUDIO_CRITICAL_LO` はこの patch ではログ・将来の audio 専用 emergency 用に残してもよいですが、decoded video frame を `pace_lead` 超えで送る条件には使わない方がよいです。

## 判定まとめ

- A: 単独では弱い。B と併用するなら可。
- B: 本命。今回の video microdrop には最も直接効く。
- C: 閾値の再調整として必要。ただし B なしでは不十分。
- D: 非推奨。queue 増量で隠す方向。
- E: safe/escape mode 判定への限定利用なら可。actual underrun 判定に PDC を足すのは不可。

