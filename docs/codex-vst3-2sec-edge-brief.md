# mIV VST3 PDC: 2 秒近辺で再発するスタッター + auto-bypass 漏れ

## 状況

前回 ([codex-vst3-microdrop-answer.md](codex-vst3-microdrop-answer.md)) の助言通り、`audio_buf < AUDIO_CRITICAL_LO`
の `pace_lead` bypass 経路を撤去し、`AUDIO_SAFE_LO/HI/CRITICAL_LO` を 300ms cap 前提
(= 0.10 / 0.20 / 0.03) に再調整した。**PDC 1700ms 以下では微小 drop は解消**した。

しかし以下 2 つの問題が残る:

### 問題 A: PDC 2000ms 近辺で再発するスタッター (= 別の症状)
PDC ≈ 2023ms (= mIV Test Latency 1973ms + Insight 2 50ms) で:
- decode 率: 通常 60fps → **30-40fps に半減**
- audio_buf: 平常 1070ms → **0-7ms に崩壊**
- ahead: 平常 347ms → **-268ms ~ 596ms (= ときどき負値、frame が late)**
- displayed: **30-39 frames/sec**
- dropped_full は出なくなった (= bypass 撤去の効果、video queue は溢れていない)

これは旧来の "pace_lead bypass による queue overflow" とは別の現象。**audio chain そのものが starve している**。

### 問題 B: 合計 > 2000ms でも auto-bypass が発火しない
`MAX_PDC_LATENCY_SECS = 2.0` の auto-bypass は **個別 plugin の `slot.latency_samples > max_samples`** で
判定している ([src/video/dsp/mod.rs:175-220](C:/home/mimageviewer/src/video/dsp/mod.rs))。

ログで観測:
```
[ 48.513s] PDC latency changed: 49.977ms -> 2023.968ms  ← 合計 2023ms
                                                          (個別: Test 1973, Insight 50)
```

このとき auto-bypass は **発火しない**。理由:
- mIV Test Latency 個別 latency = 1973ms < 2000ms
- Insight 2 個別 latency = 50ms < 2000ms
- 個別判定では引っかからない、しかし**合計 2023ms > 2000ms**

また、bypass されていた plugin をユーザーが手動で ON にしたとき、`set_bypass(idx, false)` 経路で auto_bypassed_for_latency をクリアしているが、再評価しないので、再度 ON にしてもチェックが走らない (現実装は `latest != s.latency_samples` のときだけ判定する)。

## perf-log 定量データ (問題 A)

ログ: `C:/Users/mikag/AppData/Roaming/mimageviewer/logs/perf_events.jsonl`、
`mimageviewer.log` の `PDC latency changed: 49.977ms -> 2023.968ms` (t=48.513s) 以降。

### 1 秒ごとの統計

| sec | decode | drop | tick | disp | audio_buf 平均 | ahead 平均 |
|---:|---:|---:|---:|---:|---:|---:|
| **35-47 (PDC=49ms)** | 60 | 0 | 78 | **57** | 1072ms | **347ms** |
| 48 (jump 中) | 33 | 0 | 48 | 32 | 1072ms | 348ms |
| **50-52 (PDC=2023ms)** | **3-4** | 0 | 0-2 | **0** | **0ms** | 597ms |
| 53-58 (PDC=2023ms) | 19-81 | 0 | 26-67 | **17-48** | 4-872ms | -268~596ms |
| 60-72 (PDC=2023ms+) | 5-40 | 0 | 0-71 | **0-39** | 0-12ms | 597ms |

要点:
- PDC 上昇直後 audio_buf が 0 に落ち、しばらく回復しない
- decode は常時 30-40fps (= 60fps source の半分)
- ahead が時々 **負値** → frame が late = 表示時に過去フレーム
- audio_buf がほぼ恒常的に 0-12ms (= 完全 underrun に近い)

### 健全状態 (PDC=49ms) との比較

| 指標 | PDC=49ms | PDC=2023ms |
|---|---|---|
| audio_buf | 1072ms | 0-12ms |
| ahead | 347ms (= pace_lead 0.60 内に収まる) | -268~596ms |
| decode 率 | 60fps | 30-40fps |
| displayed 率 | 57fps | 30-39fps |

**pace_lead = 0.60 は PDC 大時には正しく機能している** (= ahead 平均が pace_lead 以下)。
それでも audio_buf が 0 に崩壊しているので、原因は **audio chain が demux/decode 段階で starve** している。

## アーキテクチャの再確認

```
[demux thread (run_decoder)]
   ├── av_read_frame で source packet を読む
   ├── video packet → video_pkt_tx (bounded 64) → blocking on full
   └── audio packet → audio_pkt_tx (bounded 64) → blocking on full

[video decode thread]
   ├── video_pkt_rx ← video_pkt_tx
   ├── decode + pacing wait (= pace_lead 0.60s で wait)
   └── video_tx (bounded 24) ← decoded frame

[audio decode thread]
   ├── audio_pkt_rx ← audio_pkt_tx
   ├── decode + swresample
   └── audio_tx (bounded 32) ← AudioFrame (~21ms each)

[audio-pump thread]
   ├── audio_tx から recv
   ├── bridge.process_block (= push_audio + pull_audio with VST3 plugin)
   └── AudioBuffer (Mutex<VecDeque>, cap 300ms)

[cpal callback (WASAPI Shared)]
   └── AudioBuffer から pop → OS
```

**demux の進行 = video decode の進行 + video_pkt_tx 容量** (= 約 1 秒分の compressed video):
- video pacing: pace_lead 0.60s 先まで
- video_pkt_tx buffer: 約 1 秒 (= 60 packets)
- 合計 demux 進行: pace_now + 約 1.6s

PDC = 2.0s のとき:
- plugin に必要な input lookahead = pace_now + pdc + buffer = pace_now + 2.0 + α
- 必要 demux lookahead ≈ 2.5-2.6s
- **実際の demux lookahead ≈ 1.6s → 不足**

→ audio decode thread が demux 待ちになる時間が出て、audio_tx が空、AudioBuffer underrun。

これは前回の Codex 答 P2/P3 (= "audio packet lookahead と demux backpressure を分離し、PDC 2s でも audio だけ先読みできる構造") で指摘された設計上の問題と一致する。

## 検討中の修正案

### 案A: 短期 — compressed packet queue を拡張
```rust
// src/video/decoder.rs
let (video_pkt_tx, video_pkt_rx) = bounded::<VideoWorkerMsg>(64);  // → 256?
let (audio_pkt_tx, audio_pkt_rx) = bounded::<AudioWorkerMsg>(64);  // → 256?
```
compressed packet は数 KB/packet なので、256 packets = 数 MB で済む。
demux 進行が pace_now + 0.60 + 4s ≈ 4.6s に拡大。PDC 2s 余裕で吸収。

**懸念**: video_pkt_tx を太くしても、demux はそこまで読み込んだら video_pkt_tx が full で
止まる。audio packet は途中で出るが、video packet が大量に詰まると demux が前に進めない。

### 案B: demux backpressure を audio priority に
demux が「video_pkt_tx full でも audio_pkt_tx に空きがあれば audio packet を投入し続ける」設計。
具体的には demux loop で `try_send` を使い、video full で skip しても audio は止めない。
ただし video packet を skip すると frame loss なので、別の queue (= overflow queue) に逃がす必要。

### 案C: 長期 — demux thread から video pacing を切り離す
Codex 前回助言の本命。demux は packet を分配するだけ、video decode 側に独立 pacing。

例えば:
- demux: `loop { read_packet; dispatch; }` (= 完全独立、audio_pkt_tx と video_pkt_tx は両方とも bounded だが大容量)
- video_pkt_tx: bounded 256 (= ~4s 分の compressed video)
- video decode: 既存の pace_lead 0.60s pacing
- audio decode: 既存の bounded backpressure
- video decode が pacing wait 中も demux は audio packet を流せる

これは案A の拡張版とほぼ等価 (= 案A で十分かも?)。

### 案D: PDC 上限を下げる
`MAX_PDC_LATENCY_SECS = 2.0` → 1.5s に下げて、2s 近辺の壊れる範囲を回避。
auto-bypass 閾値も 1.5s に。
**懸念**: ユーザー要望 (= 2s まで対応) に反する。1.5s も "実用範囲" だが、保守的すぎる。

### 案E: AudioBuffer cap を増やす
TARGET_BUFFER_SECS = 0.3 → 0.5 にして、underrun 余裕を取る。
**懸念**: EQ 反応性悪化 (= ユーザー要望と相反)。さらに、根の demux starvation を直さないと
AudioBuffer 増やしても結局 0 に落ちる。

## 問題 B (auto-bypass 漏れ) の修正案

### 案F: 合計 > MAX で auto-bypass
`total_latency_samples` で合計を計算後、`> max_samples` なら最大 latency の slot を bypass。
発生条件:
- 個別 plugin が増えて合計超過 (= ユーザー操作 or latency 動的増)
- 合計 ≤ MAX に戻るまで上から順に bypass

### 案G: set_bypass(idx, false) で事前チェック
ユーザーが ON にする瞬間に、合計が MAX を超えるなら refuse (= bypass=true 維持) + auto_bypassed_for_latency=true。

### 案F + 案G 併用が筋良い
- ユーザー操作: 案G で即時阻止 (= UX が分かりやすい、ON にしようとしたら即「上限超過のため OFF のまま」表示)
- 動的 latency 変化: 案F で事後対応

## 質問

### Q1: 問題 A の真因は demux starvation で正しいか?
audio_buf が 0 に崩壊し ahead が負値に振れる現象は、demux/packet queue が PDC 分の lookahead を
持てないためと判断してよいか?

### Q2: 短期 fix としては案A (= packet queue 拡大) で十分か?
video_pkt_tx と audio_pkt_tx を 64 → 256 (or 384) に増やすだけで PDC 2s が安定するか?
compressed packet なのでメモリコストは軽い。

### Q3: 案B (= audio priority drain) は必要か?
video_pkt_tx が一杯になったら demux が止まる構造 (= 案A だけだと依然として頭打ち) を解消すべきか?
それとも案A で十分実用に達するか?

### Q4: 問題 B の修正 (案F + 案G) のタイミング判定
- 案G の `set_bypass(idx, false)` 内では、新しい total を試算してチェックする (= 計算は軽い)
- 案F の `total_latency_samples` は毎 pump push (~21ms) で呼ばれる、ここで sum > max 判定追加でいいか?
- bypass 対象の選び方: 最大 latency の slot? 最後に ON にされた slot? どれが UX 自然か?

### Q5: 案D (= MAX_PDC を 1.5s に下げる) も併用すべきか?
仮に案 A-C で 2s が安定しても、ジッタや plugin 個性で時々破綻する可能性があるなら、
保守的に上限を 1.5s に下げて、auto-bypass を厳しめにする方が UX 安定するか?

## 関連ファイル + 行番号

- `src/video/decoder.rs:580-920` (= run_decoder = demux thread + dispatch)
- `src/video/decoder.rs:644` (= `bounded::<VideoWorkerMsg>(64)` 拡大候補)
- `src/video/decoder.rs:597` (= `bounded::<AudioWorkerMsg>(64)` 拡大候補)
- `src/video/decoder.rs:1057-1170` (= GPU pacing、最新の audio_escape 撤去版)
- `src/video/decoder.rs:1369-1450` (= CPU pacing)
- `src/video/dsp/mod.rs:175-225` (= total_latency_samples + 個別 auto-bypass)
- `src/video/dsp/mod.rs:880-895` (= set_bypass、auto_bypassed_for_latency クリア)

## 補足

- ユーザーは PDC 上限 2s で十分と言ったが、実際 2s 直前で安定しない。実用域は 1.7s 程度?
- 前回 Codex 助言の P2/P3 (= packet 分離 + audio priority) を実装する時期かもしれない
- ただし大改造になるので、まず簡易な案A or 案D で延命するのもあり
- 問題 B は問題 A と独立。F+G の修正は影響範囲小

ご助言お願いします。
