# mIV VST3 PDC: 高 latency で音声が途切れる問題の方針相談

## 背景

mIV (mimageviewer) は Rust + egui の Windows 動画ビューアで、v0.9.0 で
VST3 プラグイン経由の音声処理機能を実装中。プラグインは C++ bridge プロセス
(`mimageviewer-vst3-host.exe`) で host し、Rust 本体とは stdin/stdout JSON +
共有メモリ + Windows named events で IPC する。

直近で **Plugin Delay Compensation (PDC)** を実装し、
プラグインが報告した latency 分だけ video clock を遅らせて A/V sync を保つ
ようにした。検証用に「mIV Test Latency」プラグイン (= ユーザー自作、UI で
任意の遅延サンプル数を指定して `IAudioProcessor::getLatencySamples()` で
報告するだけのもの) で動作確認している。

**現状報告された問題**: latency を 1000ms 強に設定すると、**音声が ブツブツ
途切れる** (= cpal callback が underrun している模様)。video は前回の修正で
滑らかになったが、それと引き換えに音声が悪化した。**前回の修正前は同じ
1000ms+ でも音声は途切れていなかった**ので、こちらの修正で導入された退行。

ユーザー視点の落とし所: latency は実用上 2 秒もあれば十分で、それ超は
auto-bypass などで弾いて良い。ただし「なぜそうなるか」を理解した上で
方針を決めたい。

## 音声経路の全体像

```
[demux thread] → audio_pkt_tx (bounded=64) → [audio decode thread]
                                                  ↓ swresample
                                            audio_tx (bounded=32 frames, ~21ms each)
                                                  ↓
[audio-pump thread]
   ┌── recv frame
   ├── (VST 有効なら) bridge.process_block(src, dst)
   │       ↓ push_audio: 共有メモリ in_ring (capacity = block_size * 2 * 8 = 80ms@48kHz)
   │       ↓ bridge audio thread: plugin process()
   │       ↓ pull_audio with 100ms timeout: 共有メモリ out_ring
   │       ↓ 部分取得時は残り silence 埋め
   ├── AudioBuffer (Mutex<VecDeque<f32>>, cap = TARGET_BUFFER_SECS * sr * 2)
   │   現在 TARGET_BUFFER_SECS = 0.3 (= 300ms)
   │   pump は cap 超えで `sleep(10ms)` で待機する素朴な実装
   └── publish_buffer_secs(buf, clock)
                                                  ↓
[cpal callback (WASAPI Shared, 周期 ~10-20ms)]
   ↓ fill_output: AudioBuffer から pop、足りなければ silence 埋め
   ↓ clock.set_audio_pts(pts_for_video) で master clock 更新
```

VST OFF 時は bridge 経由なしで直接 AudioBuffer に push される
(= 普通の動画再生)。

## 関連する直近の修正 (= レビュー対象)

### 修正A: PDC 動的 latency 通知 (= LatencyChanged 対応)
プラグインが mid-session で `restartComponent(kLatencyChanged)` を呼んだ時、
bridge がそれを検知して `IAudioProcessor::getLatencySamples()` を再取得し、
stdout で `{"event":"latency_changed", "latency_samples":N}` を mIV に通知。

mIV 側 (`src/video/dsp/bridge.rs`):
- 既存 `Bridge::recv()` は同期 blocking で stdout 読みする設計だった
- 非同期の LatencyChanged を捕捉するため、spawn 時に **bridge-event-pump
  スレッド** を立てた
- pump スレッドが stdout を read → LatencyChanged は atomic
  `cached_latency_samples` に格納 → それ以外は channel で recv() に渡す
- `Bridge::cached_latency_samples_value()` で最新値を pull できる

mIV 側 (`src/video/dsp/mod.rs::total_latency_samples`):
```rust
pub fn total_latency_samples(&self) -> u32 {
    let mut inner = self.inner.lock().unwrap();
    let mut total: u32 = 0;
    for s in inner.slots.iter_mut() {
        if !matches!(s.state, SlotState::Loaded) { continue; }
        let latest = s.bridge.cached_latency_samples_value();
        if latest != u32::MAX && latest != s.latency_samples {
            s.latency_samples = latest;
        }
        if !s.bypass {
            total = total.saturating_add(s.latency_samples);
        }
    }
    total
}
```

### 修正B: PDC ジャンプ + 100ms ジッタ閾値
latency 変化時、video clock の monotonic guard が後退を防ぐため
**latency 増加分 (例: +1s) だけ video が凍結** する問題があった。
ユーザー要望「凍結より映像ジャンプの方が好ましい」を受けて:

- `AvClock::set_audio_pts_jump(pts)` を新設
  (= wall-rate cap + monotonic guard を **両方バイパス** して anchor を
  強制再設定。source は Audio 維持)
- `AudioBuffer.pdc_latency_secs_applied` フィールド追加 (= fill_output が
  最後に適用した値)
- `fill_output` で latency 変化を検出: 変化量 > 100ms なら jump、
  ≤100ms なら通常 `set_audio_pts` (= monotonic で吸収、jitter 抑制)

```rust
const PDC_JUMP_THRESHOLD_SECS: f64 = 0.1;
let delta_secs = pdc_latency - buf.pdc_latency_secs_applied;
let latency_jumped = delta_secs.abs() > PDC_JUMP_THRESHOLD_SECS;
if delta_secs.abs() > 1e-6 {
    buf.pdc_latency_secs_applied = pdc_latency;
}
// ...
if pump_serial >= clock.current_seek_serial() {
    if latency_jumped {
        clock.set_audio_pts_jump(pts_for_video);  // バイパス
    } else {
        clock.set_audio_pts(pts_for_video);        // 通常 (monotonic)
    }
}
```

### 修正C: audio_buf レポートに pdc_latency 加算 (← 退行の疑い)
高 latency 時に **video が定期的にカクカク停止** する問題が出た。perf-log で
解析したところ:

- `audio_buf_secs` が常時 70-80ms と異常に低く報告されていた
- decoder pacing 経路の `audio_escape` モードが発動
  (`audio_buf < AUDIO_SAFE_LO=250ms` で escape ON、その状態で
  `audio_buf < AUDIO_CRITICAL_LO=80ms` なら pacing bypass = pace_lead 無視)
- → decoder が動画フレームを過剰生産 → queue 満杯 → UI 消費が追いつかず
  周期的スタッター (1.7s 沈黙)

修正 (`src/video/audio.rs::publish_buffer_secs`):
```rust
fn publish_buffer_secs(buf: &AudioBuffer, clock: &AvClock) {
    let secs = buf.samples.len() as f64 / buf.samples_per_sec;
    // 旧: clock.set_audio_pump_buf_secs(secs);
    // 新: プラグイン内部 delay-line も「再生待ちバッファ」としてカウント
    clock.set_audio_pump_buf_secs(secs + buf.pdc_latency_secs);
}
```

これで video スタッターは解消したが、**1000ms+ で音声がブツブツ途切れる
ように**なった。修正Cが入る前は同じ条件で音声は途切れていなかった
(ユーザー確認済)。

## 音声途切れのメカニズム (= 仮説)

mIV 側で疑っている要因:

### 仮説1: bridge IPC ring buffer (80ms) のキャパ不足
共有メモリ ring は `block_size * 2 * 8 = 480 * 2 * 8 = 7680 samples = 80ms`。
プラグイン内部 delay-line (1000ms = 384KB stereo f32) のメモリアクセスで
キャッシュミスが増え、各 process() に時々 10ms+ かかると、80ms の余裕は
すぐ消える → pump 側 push が wait → AudioBuffer 補充が遅れる → cpal underrun。

### 仮説2: pull_audio の 100ms タイムアウト
`b.pull_audio(dst, 100)` で 100ms 経っても output が無ければ部分取得 +
残り silence 埋め (= `process_block` 内)。これで silence が混入する。
高 latency でジッタが大きければ 100ms タイムアウトに引っかかる確率増。

### 仮説3: 修正Cで audio_buf を inflated に報告したことの副作用
`total_audio_buffer_secs = pump_buf + tx_queued` で、修正前は実値
(70-80ms)、修正後は実値 + pdc_latency (= 1070-1080ms)。
- decoder pacing は audio_escape を OFF にして normal pacing で動作
- 結果: video decoder が CPU を持続的に消費する状態に
- audio decode thread / audio-pump thread が CPU 不足で時々遅れる
- → AudioBuffer の補充がムラになる → 300ms cap でも吸収しきれない underrun

修正前は decoder が「過剰生産→ queue 満杯→ 多くは drop」だったので
逆に CPU 開放のタイミングが多く、audio path が安定していた可能性。

### 仮説4: pre-warm 不足
`add_plugin` で 200ms 分の silence を pre-warm している。1000ms+ latency の
plugin は内部 delay-line を 1 秒分埋めるまで output が安定しない可能性。
ただしこれは初回のみで、定常状態には関係ないはず。

## 検討中の方針

### 方針A: PDC latency に上限を設ける + 自動 bypass (= ユーザー提案)
`MAX_PDC_LATENCY_SECS = 2.0` を超える latency を報告したスロットは自動で
`bypass=true` に切り替え、警告ログを出す。`total_latency_samples` 内で
チェックして bypass 適用 + `active_slot_count` 再計算。

**利点**: シンプル。実用上、まともなプラグインは latency 数百 ms 程度なので、
2 秒上限で実用シナリオはカバー。
**欠点**: 根本原因 (= 仮説1-3 のどれか) を潰さないので、上限ぎりぎり
(= 1.8s 等) でも症状が出る可能性は残る。

### 方針B: 修正C を撤回する (= publish_buffer_secs を元に戻す)
PDC を audio_buf に加算するのを止める。代わりに別の方法で video decoder の
audio_escape 誤発動を抑える。例:
- decoder pacing 側で「VST 有効中は audio_escape を無効化」のフラグ追加
- もしくは `AUDIO_CRITICAL_LO` 閾値を下げる (= 80ms→ 30ms)、
  `AUDIO_SAFE_LO/HI` も AudioBuffer cap (300ms) に整合させる

**利点**: 仮説3 が真因なら音声退行は解決。
**欠点**: video スタッター (修正前の状態) が再発する。これは別経路で対処。

### 方針C: AudioBuffer cap を増やす (= 300ms → 1s 等)
高 latency 時のジッタ吸収余裕を増やす。pump 側の cap 待ちロジックは
そのままでよい。

**利点**: 仮説1-2 のジッタ要因を吸収できる。
**欠点**: EQ ノブを動かしてから音声に反映されるまでの遅延が増える
(= 旧 1.5s 問題の再来)。ユーザー要望の「EQ 反応性」と相反する。

### 方針D: bridge ring buffer を増やす (= 80ms → 300ms 等)
共有メモリ ring の容量を増やすことで、bridge 側の処理ジッタを吸収。

**利点**: 修正Cを残しつつ仮説1 を解決できる可能性。
**欠点**: 共有メモリ allocation 増加 (= 数 MB)。bridge と mIV 両側の
ring 計算式を変更要。protocol.h と bridge.rs の整合性。

### 方針E: 方針A + 方針D の組み合わせ
2 秒上限を保険として入れつつ、ring buffer も適度に増やす。

## 質問

1. **音声 ブツブツの真因として最も疑わしいのはどの仮説か?** 修正C が
   退行を引き起こした機序として、仮説3 (= CPU 競合) は理にかなっているか?
   それとも別の見落としがあるか?

2. **方針A (= 2 秒上限 + 自動 bypass) を採用する場合**、
   - チェックタイミング: `total_latency_samples` 内で毎 pump push (~21ms)
     ごとに行うのは妥当か? それとも slot.latency_samples 更新時だけ
     1 回行うべきか?
   - bypass 後にユーザーが手動で ON にし直したらどうする?
     再度 latency が 2 秒超ならまた auto-bypass で良いか? (= 単に同じ
     チェックで再発火)
   - settings 側 `Vst3PluginEntry.bypass` への永続化は必要か?
     (= 起動時にも復元される) または auto-bypass はランタイムのみで、
     再起動時は元に戻るべきか?

3. **修正C を撤回するべきか維持するべきか**: 仮説3 が真因なら撤回が
   きれいだが、video decoder pacing の audio_escape ロジックを
   別途修正することになる。どちらの方向が筋が良いか?

4. **bridge ring buffer 80ms** は適切か? Bitwig 等の DAW は同等の
   sandboxed plugin host で IPC ring を持つが、典型的な容量は?

5. **見落としている経路**はないか? 例えば
   - cpal callback の周期がプラグイン処理時間に影響される
   - audio_tx (32 frames bounded) のバックプレッシャーで decode が
     遅れる
   - master clock の anchor 更新が音声経路に影響を与える

## 関連ファイルパス

- `src/video/audio.rs` (= pump thread + fill_output + publish_buffer_secs)
- `src/video/clock.rs` (= AvClock + set_audio_pts/jump、 audio_bookkeeping)
- `src/video/dsp/mod.rs` (= DspBridge + total_latency_samples + add_plugin)
- `src/video/dsp/bridge.rs` (= Bridge struct + event-pump + IPC primitives)
- `src/video/decoder.rs` (= 音声/動画 decode + pacing + audio_escape ロジック)
- `crates/vst3-host/src/main.cpp` (= bridge main loop + poll_latency_change)
- `crates/vst3-host/src/host_app.cpp` (= ComponentHandler::restartComponent)
- `crates/vst3-host/src/plugin_loader.cpp` (= PluginLoader::poll_latency_change
  + process_block + plugin lifecycle)

ご助言いただきたいです。
