# mIV VST3 PDC: シーク後にも pre-seek 音声が残留 + reset 副作用

## 状況

PDC = 1500-2000ms の高 latency plugin (mIV Test Latency) を有効にした状態で:

- 動画シーク (= シークバー) または **W キー (= 先頭戻り)** を押すと:
  - シーク処理中は音声が一旦停止 (= 想定通り)
  - シーク完了後、**シーク前の音声が ~1.5-2 秒間そのまま再生**される
  - その後ようやくシーク先の音声が出始める
- ユーザー体感としては「シーク前のバッファに残った音が遅れて出てくる」

実際は **plugin の内部 delay-line にあった pre-seek input が flush されず、post-seek
input を push したときに plugin が delay-line から pre-seek output を吐き出している** と推測。

## 試した修正 (= 効果なし)

`Cmd::Reset` (= bridge 側で `setProcessing(false) → setProcessing(true)` で plugin
内部状態 flush) は元々実装されていたが**呼び出されていなかった**。これを発火するように:

### mIV 側 ([src/video/dsp/mod.rs](C:/home/mimageviewer/src/video/dsp/mod.rs))
```rust
pub fn reset_plugins(&self) {
    if !self.is_enabled() { return; }
    let bridges: Vec<Arc<Bridge>> = {
        let inner = self.inner.lock().unwrap();
        inner.slots.iter()
            .filter(|s| matches!(s.state, SlotState::Loaded))
            .map(|s| s.bridge.clone())
            .collect()
    };
    for b in &bridges {
        if let Err(e) = b.send(&Cmd::Reset) {
            crate::logger::log(format!("[VST3] reset_plugins: bridge send failed: {e}"));
        }
    }
}
```

### audio-pump ([src/video/audio.rs](C:/home/mimageviewer/src/video/audio.rs))
```rust
let mut last_seen_seek_serial: u64 = 0;
// ... pump loop ...
if frame.seek_serial > last_seen_seek_serial {
    if let Some(b) = &dsp_bridge {
        if b.is_enabled() && b.active_slot_count() > 0 {
            b.reset_plugins();
        }
    }
    last_seen_seek_serial = frame.seek_serial;
}
// ... その後 b.process_block() を呼ぶ ...
```

→ **症状変わらず**、さらに副作用として以下のログが頻発:

```
vst3 query_gui_size: unexpected ResetDone, fallback 1200x800
```

(プラグイン GUI 表示時の `Bridge::recv()` が `Event::GuiSize` を期待していたが、
代わりに `ResetDone` が channel に積まれていて先に取り出されている)

## bridge アーキテクチャ

```
mIV (Rust)                              bridge (C++)
─────────                                ────────────
DspBridge                                main.cpp
  ↓ Bridge::send(Cmd::Reset)             ↓ stdin pump thread → cmd_queue
                                         ↓ GUI thread (= run_gui_loop)
                                         ↓   if cmd == "reset":
                                         ↓     loader_->reset()  (= setProcessing false→true)
                                         ↓     write_message(ResetDone)
                                         ← stdout event ← ResetDone
                                         
                                         並行して:
audio-pump → push_audio (= shm in_ring)  ← read_in_available
                                            ↓ loader_->process_block()
                                              ↓ processor_->process()
audio-pump ← pull_audio (= shm out_ring) ← write_out
```

つまり bridge 内では:
- **GUI thread** が control commands (= reset, query_gui_size, etc.) を処理
- **audio thread** が plugin process を回し続ける
- 両者は同時並行で走る

mIV の Bridge には event-pump スレッドがあり、stdout を継続 read して以下を分離:
- `Event::LatencyChanged` → atomic に格納 (= channel に流さない)
- `Event::Loaded` → channel + cached_latency 更新
- その他 (`ResetDone`, `GuiAttached`, `GuiSize`, `GuiDetached`, `Closed`, `Error`) → channel 経由で `Bridge::recv()` に渡す

`recv()` を呼ぶのは:
- `add_plugin`: `Event::Loaded` を待つ (= 1 度のみ)
- `show_slot_gui`: `Event::GuiAttached` / `Event::GuiSize` を待つ
- `hide_slot_gui`: `Event::GuiDetached` を待つ
- `query_gui_size`: `Event::GuiSize` を待つ

`ResetDone` を待つコードは**ない**。だから ResetDone は channel に溜まっていく。

## 推測される真因

### 副作用 1: ResetDone 溜め込み
誰も `recv` しないので channel に積まれ続け、後続の同期 recv (= 例 `show_slot_gui` の
`Event::GuiAttached` 待ち) が ResetDone を先に拾ってしまう。
event-pump で **ResetDone を intercept** して channel に流さない (= LatencyChanged と同じ扱い) のが筋。

### 副作用 2: GUI thread と audio thread の race
- mIV pump が `b.send(&Cmd::Reset)` した直後に `b.process_block(post-seek)` を呼ぶ
- bridge 側で:
  - audio thread は in_ring から既に push された post-seek audio を読み始める
  - GUI thread はまだ Reset を処理していない (= 8ms 周期 polling、cmd_cv_.wait_for で sleep 中)
  - audio thread が plugin process() を呼ぶとき、plugin の delay-line には **pre-seek の input** が残っている
  - plugin output = pre-seek delayed audio
- pump pull → AudioBuffer に pre-seek audio が積まれる
- cpal が pre-seek audio を再生

reset を audio thread と同期させない限り、pre-seek が必ず ~latency 秒漏れる。

### 副作用 3: bridge in_ring の残留
理屈上 mIV pump が pre-seek frame を skip するので、seek 後に in_ring に新規 pre-seek が
入ることはないはず。ただし、過去に push された pre-seek audio が audio thread によって
まだ完全には drain されていない可能性はある (= bridge audio_loop が wait しているケース等)。

## 検討中の修正案

### 案 A: event-pump で ResetDone も intercept
mIV 側の Bridge spawn 時 event-pump スレッドで:
```rust
match read_event_blocking(&mut stdout) {
    Ok(Event::LatencyChanged { ... }) => { /* atomic 反映 */ }
    Ok(Event::ResetDone) => { /* drop、channel に流さない */ }
    Ok(other) => { event_tx.send(Ok(other)) }
    ...
}
```

ResetDone は誰も待たないので drop して問題ない。
これで「unexpected ResetDone」副作用は解消するが、**根本の race 問題は残る**。

### 案 B: bridge audio thread が reset を実行する
bridge 側で `Cmd::Reset` 受信時に GUI thread が `setProcessing` を呼ぶのではなく、
`std::atomic<bool> reset_pending_` を立てて return。
audio thread のループ先頭で `reset_pending_.exchange(false)` で確認:
- true なら:
  - in_ring を完全 drain (= 残りの pre-seek input を捨てる)
  - `loader_->reset()` を呼ぶ (= setProcessing false→true、audio thread 自身が実行)
  - 通常 loop 継続
- false なら通常処理

これで GUI thread と audio thread の race が消える。in_ring drain も同時に行えば
pre-seek input が plugin に流れ込まない。

### 案 C: mIV 側で Reset 後に同期待ち
`b.send(Cmd::Reset)` 後に `recv()` で `Event::ResetDone` を待つ。
- 利点: 確実に reset 完了後に process_block を呼べる
- 欠点: pump thread が ResetDone 待ちで blocking する (= 数 ms 程度のはず、許容範囲?)
- 副作用 1 も同時に解消

ただし audio thread と GUI thread の race は依然として残る (= ResetDone が出るタイミングは
GUI thread が setProcessing 呼んだ後、audio thread の動きとは独立)。

### 案 D: Reset 後に N silence block を push
reset 直後に pump 側が silence block を push して bridge 内部を flush する。
N = latency_samples / block_size 程度 (= 例: 96000 / 480 = 200 blocks 必要)。
- IPC roundtrip が大量に走る (= 数百ブロック × 数 ms = 数百 ms の wait)
- pump が長時間 blocking → AudioBuffer underrun → cpal silence
- ユーザー体感: シーク後 N 秒の silence (= 元の問題と同じくらい悪い)

## 質問

### Q1: 真因は GUI thread と audio thread の race か?
mIV 側で送った Cmd::Reset が bridge GUI thread で処理される前に、bridge audio thread が
post-seek input を pre-seek delay-line で処理してしまう、という解釈で正しいか?

### Q2: 案 B (= bridge audio thread に reset 実行) が筋良いか?
それとも案 C (= mIV 側で同期待ち) で十分?

### Q3: 案 B 実装時、in_ring drain の方法
```cpp
while (pipe_.read_in_available(scratch, ...) > 0) { /* discard */ }
```
で良いか? out_ring の drain も必要か?

### Q4: 案 A (= ResetDone intercept) は単独で意味があるか?
race 問題は別途解決するとして、副作用 1 (= unexpected ResetDone) を防ぐためだけにも
event-pump で drop する価値はあるか? (= LatencyChanged と同じパターン)

### Q5: bridge の `processor_->setProcessing(false)` を audio thread で呼んで問題ないか?
VST3 仕様上 setProcessing は I/O thread (= audio thread) と同じスレッドから呼ぶのが
推奨? それとも GUI thread から呼ぶべき?

## 関連ファイル

- `src/video/audio.rs:268-285` (= run_pump、last_seen_seek_serial + reset_plugins 呼び出し)
- `src/video/dsp/mod.rs:475-505` (= DspBridge::reset_plugins)
- `src/video/dsp/bridge.rs:200-260` (= Bridge::spawn の event-pump スレッド)
- `crates/vst3-host/src/main.cpp:262-275` (= run_gui_loop の reset コマンド処理)
- `crates/vst3-host/src/main.cpp:407-525` (= audio_loop)
- `crates/vst3-host/src/plugin_loader.cpp:507-514` (= PluginLoader::reset)

## ログ

`mimageviewer.log` に以下が頻発 (= reset 副作用):
```
[   6.627s][t  1] vst3 query_gui_size: unexpected ResetDone, fallback 1200x800
[   6.797s][t  1] vst3 query_gui_size: unexpected ResetDone, fallback 1200x800
[   7.419s][t  1] vst3 query_gui_size: unexpected ResetDone, fallback 1200x800
...
```

(plugin GUI 表示時の query_gui_size が GuiSize ではなく ResetDone を受け取っている)

## 補足

- pump push 内の `frame.seek_serial > buf.pump_seek_serial` 判定で AudioBuffer は
  きちんと clear されている (= mIV 側 buffer は問題なし)
- fill_output の `pump_seek_serial < clock_serial` チェックでも cpal callback 中の
  pre-seek audio は silence で塗り潰される (= cpal 経路は問題なし)
- 残るのは bridge plugin 内部 delay-line の flush タイミングだけ
- PDC = 0 (= VST OFF) でシークしても問題ない (= reset 不要、当然)

ご助言お願いします。
