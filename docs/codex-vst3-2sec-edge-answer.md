# Codex 回答: VST3 PDC 2 秒近辺の starvation と auto-bypass 漏れ

前提: 2026-05-01 時点の未コミット変更、`docs/codex-vst3-2sec-edge-brief.md`、および `perf_events.jsonl` / `mimageviewer.log` を確認した判断です。

## 結論

問題 A は、前回の `audio_escape` queue overflow とは別物です。今回のログでは `dropped_full=0` なので、decoded video queue が溢れているのではなく、**PDC 2 秒近辺に必要な audio input lookahead を demux/packet queue が供給できず、audio chain が starve している**と見るのが妥当です。

こちらで PDC 2023ms 以降を再集計しても、代表値は以下でした。

- PDC 49ms 健全区間: `audio_buf` 約 1070ms、decode ほぼ 60fps、`ahead` 約 347ms
- PDC 2023ms 異常区間: `audio_buf` 0-12ms 中心、decode/display 30-40fps、`ahead` は 600ms cap に張り付き
- `dropped_full`: 0

つまり video pacing cap は効いています。しかし cap が効いた結果、demux が video packet queue の先で止まり、PDC 2s 分の音声 input を先読みできていません。

問題 B は実装漏れです。現在の auto-bypass は **個別 slot latency** しか見ておらず、**active chain の合計 latency** を見ていません。また `set_bypass(idx, false)` で手動 ON に戻す瞬間の事前チェックもありません。F+G は入れるべきです。

## Q1: 真因は demux starvation か

はい、かなり高い確度で demux / packet queue horizon 不足です。

現構造では demux thread が packet を 1 個読むたびに、video なら `video_pkt_tx.send(...)`、audio なら `audio_pkt_tx.send(...)` で blocking enqueue します。`video_pkt_tx` が full になると、次の audio packet を読むところまで進めません。

現在の horizon は概算で:

- video decode pacing: `VIDEO_QUEUE_LEAD_CAP_SECS = 0.60s`
- `video_pkt_tx`: 64 packets、60fps なら約 1.07s
- 合計 demux horizon: 約 1.6-1.7s

PDC 2.0s では、plugin delay line を満たしつつ output buffer も持つために `2.0s + normal buffer` の input lookahead が要ります。最低でも 2.3s 前後、実運用では 2.5s 程度の余裕が欲しいです。現在の 1.6s では足りません。

`ahead` が負値に振れる区間があるのも、video queue overflow ではなく、demux/audio starvation 後に clock と frame supply が崩れている症状として説明できます。

## Q2: 短期 fix は packet queue 拡大で十分か

最初の短期 fix としては **案A が妥当**です。まず `audio_pkt_tx` / `video_pkt_tx` を 64 から 256 へ増やしてください。

```rust
const AUDIO_PACKET_QUEUE_CAP: usize = 256;
const VIDEO_PACKET_QUEUE_CAP: usize = 256;
```

60fps なら video packet 256 個で約 4.3s、120fps でも約 2.1s です。`pace_lead 0.60s` と合わせれば、PDC 2.0s + output buffer ぶんを概ね吸収できます。compressed packet なので decoded frame queue を増やすよりはるかに安全です。

ただし、これは「十分である可能性が高い短期策」であって、設計上の根治ではありません。動画が高 fps、packet が細かい、音声 packet の interleave が悪い、VST processing が遅い、といった条件ではまだ不足し得ます。そのため、同時に perf log へ以下を入れると次の判断が速くなります。

- `video_pkt_rx.len()` または送信側から見た queue fullness
- `audio_pkt_rx.len()`
- `audio_tx queued secs`
- `pump_buf_secs`
- `vst3_pdc_latency_secs`

## Q3: audio priority drain は必要か

最終的には必要になる可能性がありますが、最初に入れるべき修正ではありません。

理由は、demux は逐次 stream なので、読んだ packet が video で `video_pkt_tx` が full だった場合、その video packet を保持・退避・破棄しない限り、未来の audio packet だけを先に読むことはできません。つまり「video full でも audio だけ流す」は、単純な `try_send` だけでは成立しません。video packet を失わずにやるなら、結局どこかに compressed video packet の退避 queue が必要です。

したがって実装順は:

1. まず案Aで compressed packet queue を 256 に増やす。
2. それでも 2s 近辺で `audio_buf=0` が出るなら、queue len ログで詰まり箇所を確認する。
3. 必要なら案Cとして、demux / packet backlog / audio decode priority を設計し直す。

案Bを急いで中途半端に入れるより、案Aを「安全な compressed backlog 拡大」として入れる方がよいです。

## Q4: auto-bypass 修正 F+G

F+G 併用が筋良いです。これは問題 A と独立に P1 で直すべきです。

### G: 手動 ON 時の事前チェック

`set_bypass(idx, false)` の中で、「この slot を ON にしたら active total が `MAX_PDC_LATENCY_SECS` を超えるか」を試算してください。超えるなら:

- `slot.bypass = true` のまま維持
- `slot.auto_bypassed_for_latency = true`
- ログに「合計 PDC 超過のため ON を拒否」を出す
- `active_slot_count` は変えない

この場合の bypass 対象は **ユーザーが今 ON にしようとした slot** が自然です。UI 操作への反応として最も分かりやすいです。

### F: 動的変化時の合計チェック

`total_latency_samples()` では、個別 latency 更新の有無に関係なく、最終的な active total を計算した後に `total > max_samples` をチェックしてください。超える場合は、合計が max 以下になるまで active slot を auto-bypass します。

動的変化時の bypass 対象は **active slot のうち latency_samples が最大の slot** がよいです。理由は:

- 最小数の bypass で合計を下げやすい
- `Test Latency 1973ms + Insight 50ms` のようなケースでは、原因の大半である Test Latency を落とせる
- 小さな meter/analyzer だけが犠牲になるより説明しやすい

実装時は、auto-bypass 後に total を再計算し、必要なら loop で複数 slot を落とします。ログ連打を避けるため、既に `auto_bypassed_for_latency && bypass` の slot には同じログを繰り返さない形がよいです。

## Q5: MAX_PDC を 1.5s に下げるべきか

まだ下げない方がよいです。

今回の 2023ms はそもそも上限 2.0s を超えているので、本来は F+G で止まるべき状態です。まず「合計 2.0s 超は確実に auto-bypass / refuse」し、そのうえで「2.0s 未満、例えば 1.8-1.95s が案Aで安定するか」を見た方が判断がきれいです。

保守策としては、hard cap は 2.0s のまま、soft warning を 1.5s から出すのがよいです。

- `total_pdc >= 1.5s`: 警告表示、重い PDC で不安定化する可能性を示す
- `total_pdc > 2.0s`: auto-bypass / manual ON refuse

これならユーザー要望の「2s まで対応」を保ちつつ、危険域の UX も改善できます。

## 案 A-G の判定

- A: P1。短期 fix として入れる。まず 256。
- B: P2/P3。単純な `try_send` では不十分。packet 退避設計が必要。
- C: 長期本命。PDC 2s を堅牢にするなら最終的にはここ。
- D: 今は非推奨。hard cap は 2.0s 維持、soft warning 1.5s がよい。
- E: 非推奨。AudioBuffer cap 増量は反応性を悪化させ、demux starvation の根を直さない。
- F: P1。active total > max の auto-bypass を追加する。
- G: P1。手動 ON 時に合計超過を拒否する。

## 推奨実装順

1. F+G: 合計 PDC 上限チェックを先に直す。今回の 2023ms はここで止まるべき。
2. A: `audio_pkt_tx` / `video_pkt_tx` を 256 に増やす。
3. perf log: packet queue len と audio bookkeeping を追加する。
4. 1.8-1.95s で再検証する。
5. まだ `audio_buf=0` が出るなら C の設計に進む。

## 補足

今回の「合計 2023ms でも再生が続く」は、ユーザー操作としてはかなり危険です。音声が壊れるだけでなく、PDC jump によって video clock anchor も大きく動きます。したがって packet queue 拡大より先に、合計上限の enforcement を入れる価値があります。

