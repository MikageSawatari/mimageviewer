# mIV VST3 PDC: Codex 推奨修正後も周期的スタッターが残る問題

## 状況

前回 ([docs/codex-vst3-audio-dropout-brief.md](codex-vst3-audio-dropout-brief.md)) の Codex 助言に従い、以下を実装しました:

- **Step 1**: `publish_buffer_secs` から PDC 加算を撤去、`vst3_pdc_latency_secs` という別 metric に分離
- **Step 2**: decoder pacing を PDC-aware に (`pace_lead = PACE_LEAD_SECS + pdc_latency.min(MAX_PDC_LATENCY_SECS)`)、`audio_escape` 判定は actual buffer のみ
- **Step 3**: `MAX_PDC_LATENCY_SECS = 2.0` の auto-bypass

これで音声ブツブツは解消したものの、**動画の定期的なカクカク (= 周期的スタッター) が依然として残っている**。

## 再現条件

- mIV Test Latency: 1601ms
- Insight 2: 50ms
- 合計 PDC latency: **1650.975ms** (= 安定値、ジッタなし)
- 動作: 動画再生中、1.75 秒程度の周期で「~770ms 滑らか → ~980ms カクカク (= 同じフレーム静止)」を繰り返す

## perf-log 解析結果

ログ: `C:/Users/mikag/AppData/Roaming/mimageviewer/logs/perf_events.jsonl`、
`mimageviewer.log` の `[VST3 PDC] PDC latency changed: ... -> 1650.975ms` 以降。

### スタッター周期 (= video tick gap)

```
t=33.159 gap=979ms  now=26.138 now_dt=975.9ms
t=35.111 gap=978ms  now=28.090 now_dt=959.0ms  (= 周期 ≒ 1.95s 後の次の gap)
t=37.075 gap=960ms  now=30.054 now_dt=951.7ms
t=39.027 gap=950ms  now=32.006 now_dt=946.8ms
```

→ 周期約 1.95s = **`pace_lead = 0.3 + 1.65 = 1.95s` と完全一致**。

### gap 期間中 (= 33.0 - 33.16) の decode events

すべて `dropped_full=True` で `ahead = 1947-1951ms` (= pace_lead 1.95s に張り付く):

```
t=33.003 DEC pts=27.93 pace_now=25.99 ahead=1948ms ab=1070ms [DROP]
t=33.019 DEC pts=27.95 pace_now=26.00 ahead=1950ms ab=1073ms [DROP]
t=33.034 DEC pts=27.97 pace_now=26.02 ahead=1951ms ab=1086ms [DROP]
... (約 60fps で連続、全て DROP)
```

`audio_buf_secs ≈ 1070ms` で健康そう (= actual buffer のみ計上、PDC は別 metric なので副作用なし)。

### gap 期間中 (34.133 - 35.111、980ms) の event 集計

```
frame/begin: 152 (= eframe update が ~155fps で稼働、UI thread alive)
video/decode: 58 (= 60fps で decoder 稼働)
video/tick:   2 (← 980ms 中に video tick perf event が 2 回しか出ていない)
```

`video/tick` perf event は `(pulled > 0 || dropped_old_serial > 0 || dropped_past > 0 || displayed_pts.is_some())` のときのみ emit する仕様 ([src/video/mod.rs:881](C:/home/mimageviewer/src/video/mod.rs))。
そのため "video tick が呼ばれたが 1 frame も pull/display しなかった" 状態が gap 期間中ずっと続いている。

### gap 終了直後の状態 (t=35.111)

```
TICK pulled=0 disp=28.100 now=28.090
```

→ clock は 27.131 (= gap 開始時) から 28.090 へ +959ms 進んでいる
→ 表示 PTS は 27.131 (= gap 開始時の最終表示) から **一気に 28.100 へジャンプ** (+970ms 分のフレームを silent skip)

### gap 開始時 (t=34.133) の最終 tick

```
TICK pulled=1 disp=None now=27.131
```

→ frame を 1 枚 pull したが、displayable でない (= future frame のため)
→ この後 980ms にわたり、tick は呼ばれているが pull=0 / displayed=None で perf event が emit されない

### active 期間中 (33.7 - 34.13) の典型 tick

```
t=33.717 TICK pulled=1 disp=26.717 now=26.701  (= disp == now + 16ms = 60fps ジャスト)
t=33.722 TICK pulled=1 disp=None  now=26.713  (= 次 frame が future)
t=33.741 TICK pulled=0 disp=26.733 now=26.720
...
```

## 仮説

GPU pacing 経路 ([src/video/decoder.rs:1153-1167](C:/home/mimageviewer/src/video/decoder.rs)):

```rust
#[cfg(windows)]
let pdc_latency = clock
    .vst3_pdc_latency_secs()
    .min(crate::video::dsp::MAX_PDC_LATENCY_SECS);
let pace_lead = if allow_pace_lead {
    PACE_LEAD_SECS + pdc_latency  // = 0.3 + 1.65 = 1.95s
} else {
    pdc_latency                   // = 1.65s
};
if ahead <= pace_lead {
    break;
}
```

queue 構造:

- `video_tx` channel: `bounded(24)` ([src/video/decoder.rs:231](C:/home/mimageviewer/src/video/decoder.rs))
- `future_frames` deque: `MAX_RENDER_QUEUE = 24` ([src/video/mod.rs:158](C:/home/mimageviewer/src/video/mod.rs))
- 合計 48 frames ≒ 800ms@60fps

### 仮説1: `pace_lead` (1.95s) > queue capacity (800ms) のミスマッチ

decoder は pace_now + 1.95s 先まで生産しようとするが、queue は 800ms しか保持できない。
**queue の back pts は常に pace_now + 1.95s** (= pace_lead 境界)、
**queue の front pts は pace_now + 1.95 - 0.8 = pace_now + 1.15s**。

UI tick: `now ≈ pace_now`, front pts = `pace_now + 1.15s` → `front pts > now + lead_tol` で表示不可。

新フレームが入る (= 30+ frames/sec) たびに古いフレームが押し出される、ではなく、
**channel が full で newer frames が dropped されていく** ので queue の front は徐々に古くなっていく。

clock が前進して front pts に追いつくまで (= 1.15s 必要)、UI は表示しない。

### 仮説2: `request_repaint_after` の next_due 計算

[src/video/mod.rs:905-920](C:/home/mimageviewer/src/video/mod.rs):

```rust
if self.is_playing() || seek_in_flight_for_display {
    let mut due = next_due.unwrap_or_else(|| std::time::Duration::from_millis(33));
    ...
    ctx.request_repaint_after(due);
}
```

`next_due = front.pts_secs - now` ([src/video/mod.rs:719-722](C:/home/mimageviewer/src/video/mod.rs))。

front pts が clock より 1.15s 先なら `next_due = 1150ms`。
egui は 1.15s 後まで repaint しない → tick が長期間呼ばれない。

ただし frame/begin events が 980ms 中に 152 回出ているので、egui は実際には 6.5ms 周期で update を回している (= 別の repaint 要求源があるはず)。

video tick は呼ばれているが、毎回 pulled=0/displayed=None で perf event が emit されないだけ、
というのが実態と思われる。

### 仮説3: `lead_tol` (= `DISPLAY_LEAD_TOLERANCE_SECS`) の値が小さすぎる

```rust
if front.pts_secs <= now + lead_tol {
    // displayable
}
```

`lead_tol` は値が大きいほど「先のフレームも今表示してよい」になる。
小さいと「ぴったり時刻が来るまで待つ」厳格モード。

PDC で clock が遅延しているケースでは、もしかすると `lead_tol` を pdc_latency と関連付ける
必要があるかもしれない (= 推測、未検証)。

## 質問事項

### Q1: なぜ周期 1.95s でスタッターするのか?

`pace_lead = 1.95s` と一致する周期で発生しているのは偶然でない。
スタッターのメカニズムは仮説1 (= queue cap < pace_lead) で正しいか? 別の機序があるか?

### Q2: pace_lead を pdc 分増やす設計は本当に必要か?

前回の Codex 助言:
> demux/audio decode が PDC 分だけ先へ走れる必要があります

しかし実際の挙動は「decoder が pace_lead = 1.95s 先まで video frame を作り、queue が 800ms しか
持てないので大量に drop」となっている。

video pace_lead を増やす意味は:
- (a) 「動画フレームの先読み」 ← queue cap で制限される
- (b) 「demux を先に走らせて audio packet を引き出す副作用」 ← これが本来の目的?

もし (b) が目的なら、video frame queue を増やすのではなく、demux を別途進める
仕組みが必要では? (例: video frame は drop しても demux 側 reader が止まらないようにする)

または、video の `pace_lead` は元の 0.3s に戻し、別の機構で audio 側 lookahead を確保すべきか?

### Q3: queue cap を増やすか?

`video_tx (24) + future_frames (24) = 48 ≒ 800ms` を `pace_lead (1.95s) ≒ 117 frames` に
拡大すれば仮説1 は解消するが、メモリ消費が大きい (= 60fps 1080p で ~250MB の YUV/RGBA)。

GPU 経路 (`VideoFrameData::Gpu`) では HANDLE のみで実 pixels は GPU side なので軽いが、
HANDLE leak のリスクや synchronisation の複雑性が増す。

### Q4: lead_tol を PDC-aware にするべきか?

現状の `DISPLAY_LEAD_TOLERANCE_SECS` の値を確認したい (検索すれば出るが Codex の見解を聞きたい):

- PDC active 時に front pts と clock のズレを許容する閾値を上げる
- 「PDC 1.65s で clock が 1.65s 遅延しているので、front pts が clock + 1.15s でも今表示してよい」
  という判定にする

これで仮説1 のスタッターは表示側で吸収できる可能性。
ただし、本来の A/V sync (= audio と video が一致) は壊れるかもしれない。

### Q5: 設計の根本見直しは必要か?

「PDC で video clock を遅らせる」アプローチ自体に問題があるか?
代替として:
- **(a) video output 自体を遅らせる**: decoded video frame を pdc 秒だけ追加バッファに溜めてから display
  (= queue を pdc + α 大きくする発想)
- **(b) audio-only PDC**: video clock は遅らせない、audio に対して video が遅れている扱いで
  ユーザーには A/V ずれが見える (= 受け入れ可)
- **(c) plugin pre-roll**: 動画再生開始時に plugin を pdc 分先まで warm-up してから cpal 起動
  (= 開始時の長時間待ちが必要)

## 関連ファイル + 行番号

- `src/video/audio.rs:215-225` (= publish_buffer_secs、PDC は別 metric として publish)
- `src/video/audio.rs:506-560` (= fill_output、PDC jump 処理)
- `src/video/clock.rs:236-260` (= AvClock::set_audio_pts)
- `src/video/clock.rs:280-301` (= AvClock::set_audio_pts_jump)
- `src/video/clock.rs:572-600` (= total_audio_buffer_secs / set_vst3_pdc_latency_secs)
- `src/video/decoder.rs:1099-1170` (= GPU 経路 pacing、PDC-aware pace_lead 適用)
- `src/video/decoder.rs:1393-1450` (= CPU 経路 pacing、同様)
- `src/video/decoder.rs:231` (= `video_tx` channel `bounded(24)`)
- `src/video/mod.rs:158` (= `MAX_RENDER_QUEUE = 24`)
- `src/video/mod.rs:680-730` (= UI tick frame consumption)
- `src/video/mod.rs:880-901` (= perf event emission condition)
- `src/video/dsp/mod.rs:30-50` (= `MAX_PDC_LATENCY_SECS = 2.0` 定数)
- `src/video/dsp/mod.rs:165-220` (= total_latency_samples + auto-bypass)
- `src/video/engine/audio_bookkeeping.rs:23-100` (= 新 vst3_pdc_latency_secs metric)

## 補足

- ユーザーは PDC latency 上限 2 秒で十分と言っているので、超えるケースは auto-bypass で対処済
- 1601ms は実用的なリミット内 (= 一般的な linear-phase EQ や lookahead プラグインで起きうる)
- 音声は途切れていない (= Step 1 修正で解消済)
- 残るのは動画の周期的カクつきのみ
- 動画ジャンプ (= PDC change 時) は問題なく動作している

ご助言お願いします。
