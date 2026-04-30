# mIV VST3 PDC: queue cap 後も微小な周期 frame drop が残る

## 状況

前回 ([docs/codex-vst3-stutter-still-answer.md](codex-vst3-stutter-still-answer.md)) の Codex 助言通り、video pace_lead に `VIDEO_QUEUE_LEAD_CAP_SECS = 0.60` の cap を追加した:

```rust
let pace_lead = if allow_pace_lead {
    (PACE_LEAD_SECS + pdc_latency).min(VIDEO_QUEUE_LEAD_CAP_SECS)  // = min(0.30+pdc, 0.60)
} else {
    0.0
};
```

これで 1.95 秒周期の長時間スタッターは消えた。
ただし PDC = **1979ms** (= mIV Test Latency 1930ms + Insight 2 50ms) で、画面に **赤い縦線が多数** (= 細かい frame drop) が現れる。
60fps source に対して 50fps 程度しか表示できていない (= 1 秒あたり 10 frame ほど消える)。

## perf-log の定量データ

ログ: `C:/Users/mikag/AppData/Roaming/mimageviewer/logs/perf_events.jsonl`、
`mimageviewer.log` の `PDC latency changed: 1738ms -> 1979ms` (t=46.521s) 以降。

### 1 秒ごとの統計

| sec | decode events | dropped_full | tick events | displayed | dropped_past |
|---:|---:|---:|---:|---:|---:|
| 47 | 59 | 10 | 87 | **49** | 0 |
| 48 | 60 | 10 | 90 | **50** | 0 |
| 49 | 60 | 10 | 89 | **50** | 0 |
| 50 | 60 | 10 | 85 | **50** | 0 |
| 51 | 61 | 10 | 86 | **50** | 0 |
| 60 | 60 | 11 | 75 | **48** | 1 |
| 65 | 60 | 11 | 71 | **44** | 4 |

- decoder は source 60fps で動作、約 17% (= **10 frames/sec**) が `dropped_full=true` で channel への送出に失敗
- UI tick perf event は 70-90/sec 発火しているが、`displayed_pts` が non-None なのは 44-50/sec → **ほぼ 1 秒ごとに 10-15 frame の表示が抜ける**
- `dropped_past` (UI 側 skip) は 0-6/sec で軽微

### 個別 decode 値 (t=50.0-50.5s から抜粋)

```
t=50.006 ab=72ms  pace_now=43.910 pts=44.850 ahead= 940ms
t=50.077 ab=72ms  pace_now=43.980 pts=44.917 ahead= 936ms
t=50.117 ab=78ms  pace_now=44.018 pts=44.983 ahead= 965ms [DROP]
t=50.142 ab=72ms  pace_now=44.041 pts=45.000 ahead= 959ms
t=50.213 ab=71ms  pace_now=44.111 pts=45.067 ahead= 956ms
t=50.303 ab=74ms  pace_now=44.201 pts=45.150 ahead= 949ms
... (60fps、約 17% が DROP)
```

要点:
- `ahead = 940-965ms` ≫ `pace_lead = 0.60s` → **decoder が pace_lead を超えて生産している**
- `audio_buf_secs ≈ 65-80ms` (= `AUDIO_CRITICAL_LO = 80ms` のすぐ下を常に hover)

### 150ms 表示 gap の例 (t=50.46-50.62)

```
t=50.464 TICK pulled=0 disp=44.383 now=44.369   ← 最後の表示
t=50.469 TICK pulled=1 disp=None   now=44.373
... (150ms の間、video/tick perf event 発火なし)
... (frame/begin は 25 回発火 = UI thread 動作中)
... (video/decode は 10 回発火、すべて dropped_full=True)
t=50.623 TICK pulled=0 disp=44.533 now=44.518   ← 表示再開、9 frames silent skip
```

## 原因仮説 (= mIV 側)

[src/video/decoder.rs:1118-1185](C:/home/mimageviewer/src/video/decoder.rs) の pacing wait loop:

```rust
let audio_buf = clock.total_audio_buffer_secs();  // = pump_buf + tx_queued、PDC は含まない
if audio_active {
    if audio_buf < AUDIO_SAFE_LO {           // 0.25
        in_audio_escape = true;
    } else if audio_buf >= AUDIO_SAFE_HI {   // 0.75
        in_audio_escape = false;
    }
}
let ahead = pts_secs - clock.video_pacing_now_secs();
...
let pace_lead = (PACE_LEAD_SECS + pdc_latency).min(VIDEO_QUEUE_LEAD_CAP_SECS);  // = 0.60
if ahead <= pace_lead {
    break;  // 通常の pacing 完了
}
if in_audio_escape {
    if ahead < SEEK_BURST_LEAD_MAX_SECS  // 0.20
        || audio_buf < AUDIO_CRITICAL_LO  // 0.08
    {
        break;  // audio_escape bypass
    }
}
std::thread::sleep(std::time::Duration::from_millis(5));
```

### 問題のメカニズム
1. PDC = 1979ms 時、`audio_buf` (= AudioBuffer + audio_tx queued) は **常時 65-80ms** で hover
   (= AUDIO_CRITICAL_LO=80ms すぐ下)
2. `audio_buf < 80ms` → audio_escape bypass 発動 → pace_lead 無視で decoder 続行
3. decoder は queue が full まで produce、ahead は 940ms まで成長
   (= queue 800ms 分に制限される)
4. queue 内の frames は pts 範囲 `[pace_now + 140ms, pace_now + 940ms]` (= 全部 future)
5. UI tick: front pts > now + lead_tol (= 16ms) → 表示せず
6. clock が 140ms 進むまで表示 gap (= 観測された 150ms gap)
7. 表示再開時に 8-9 frames silent skip
8. cycle 反復 → 約 50fps 表示 (= 60fps source - 10 drops/sec)

### audio_buf が低い理由 (= 仮説)
- AudioBuffer cap = 300ms
- cpal が realtime drain、pump も realtime push (= 1:1)
- 定常状態の AudioBuffer level は ~0-50ms (= cpal callback period × jitter)
- `audio_tx queued` も pump consume rate と均衡で ~20-40ms
- 合計 ≈ 70-80ms (= 観測値と一致)

つまり VST 有効時の steady state では `audio_buf` は構造的に 80ms 前後で、
**AUDIO_CRITICAL_LO = 80ms の閾値設定がたまたま境界線上**になっている。

## 検討中の修正案

### 案A: AUDIO_CRITICAL_LO を下げる
```rust
const AUDIO_CRITICAL_LO: f64 = 0.030;  // 80ms → 30ms
```
80ms は VST 経由時の steady state レベルとほぼ同じ。30ms にすれば実 underrun 直前のみ bypass。
**懸念**: VST OFF 時に audio_buf がもっと健全 (= 200ms+) で動作していた前提が崩れていないか不明。

### 案B: AUDIO_CRITICAL_LO bypass 条件を撤去
```rust
if in_audio_escape {
    if ahead < SEEK_BURST_LEAD_MAX_SECS {
        break;
    }
    // audio_buf < CRITICAL_LO 経路を削除
}
```
ahead が 200ms 以下のときのみ audio_escape bypass。
**理由**: audio_low で video を急いで作っても audio decoder thread (= 別 thread) は加速しない。
動画フレームを過剰生産するメリットが薄い。

### 案C: audio_escape の閾値も下げる
```rust
const AUDIO_SAFE_LO: f64 = 0.10;   // 0.25 → 0.10
const AUDIO_SAFE_HI: f64 = 0.20;   // 0.75 → 0.20 (= AudioBuffer cap 300ms 内)
const AUDIO_CRITICAL_LO: f64 = 0.030;  // 0.08 → 0.03
```
audio_escape そのものが PDC 時に常時発動するのを防ぐ。

### 案D: pace_lead 計算を見直す
PDC が大きいほど queue 不足が深刻化するので、`VIDEO_QUEUE_LEAD_CAP_SECS` を少し大きく。
ただし decoded frame queue を増やすと CPU 経路でメモリコスト大。

### 案E: audio_buf に「PDC 内部 delay-line 量」を加算 (= 過去の修正Cを別形式で復活)
audio_buf として publish する値は実 buffer のみ、しかし decoder pacing が見る値は
`actual_audio_buf + pdc_latency` を使う。
```rust
// decoder.rs
let actual_audio_buf = clock.total_audio_buffer_secs();
let pdc_lookahead = clock.vst3_pdc_latency_secs();
let audio_buf_for_pacing = actual_audio_buf + pdc_lookahead;
```
**懸念**: 前回の修正C と同じ「実 underrun を隠す」副作用が出ないか?
実 underrun 検出 (= `audio_buf < AUDIO_CRITICAL_LO`) は actual で行い、
audio_escape 判定 (= `audio_buf < AUDIO_SAFE_LO`) は PDC 込みで行う、という分離もあり得る。

## 質問

### Q1: 微小 frame drop の真因は audio_escape bypass で正しいか?
データから見ると `audio_buf ≈ 75ms < AUDIO_CRITICAL_LO 80ms` で bypass 発動 → ahead 940ms まで生産 → queue overflow → UI 表示 gap、という解釈で合っているか?

### Q2: 案A-E のどれが筋が良いか?
特に案E (= 実 buffer と pacing 用 buffer の分離) は前回の Step 1 で否定された
「PDC 加算」と紙一重。"分離" の正当性をどう担保するか?

### Q3: VST OFF 時の audio_buf レベル
VST OFF 時 (= 通常動画再生) の audio_buf は実測どの程度になるはずか?
`AUDIO_SAFE_LO = 0.25` / `AUDIO_CRITICAL_LO = 0.08` という threshold は VST 無し時を
前提にした値で、VST 有り時には適切でない可能性。

### Q4: AudioBuffer cap (300ms) を増やすべきか?
旧値 1.5s からユーザー要望で 300ms に縮小したが、VST 有効時はもう少し余裕が
ほしいかもしれない。例えば 500ms とすれば audio_buf level も 200ms 程度には
上がる可能性。EQ 反応性とのトレードオフ。

### Q5: audio decoder 側の pacing
audio decoder は専用 thread でバックプレッシャー (= bounded channel) のみで動作している。
PDC 時に audio decoder を「video clock + pdc」付近まで先読みさせる仕組みは必要か?
現状は demux thread が video pacing に従うので、pace_lead 0.60 なら audio decode も
0.60 までしか進めないはず (= でも実測では audio decode は demux の後追いで成立している?)。

## 関連ファイル

- `src/video/decoder.rs:1099-1190` (= GPU pacing、audio_escape ロジック)
- `src/video/decoder.rs:1393-1455` (= CPU pacing、同様)
- `src/video/audio.rs:215-235` (= publish_buffer_secs、PDC は別 metric)
- `src/video/audio.rs:284` (= TARGET_BUFFER_SECS = 0.3、AudioBuffer cap)
- `src/video/clock.rs:572-600` (= audio_bookkeeping アクセス)
- `src/video/dsp/mod.rs:30-50` (= MAX_PDC_LATENCY_SECS = 2.0)

## 補足

- 1601ms PDC 時には症状が軽微だった (= 体感的にはほぼ気付かない)
- 1979ms PDC で顕著 (= drop 数自体は同程度かもしれないが、画面の赤線として可視化される閾値超え)
- PDC 上限 2 秒の auto-bypass は既に実装済 → これ以上は来ない
- 実用シナリオ (= 一般的な VST plugin の latency) は 数十-数百 ms なので、
  根治するなら案A-E のいずれかで steady state を改善するのが本筋
- queue cap 0.60s 自体は維持してよい (= スタッター回避に必要)

ご助言お願いします。
