# Codex 回答: VST3 seek reset と ResetDone 混入

前提: 2026-05-01 時点の未コミット差分、`docs/archive/vst3/codex-vst3-seek-reset-brief.md`、`src/video/dsp/bridge.rs`、`crates/vst3-host/src/main.cpp`、`crates/vst3-host/src/audio_pipe.cpp` を確認した判断です。

## 結論

`ResetDone` が `query_gui_size` などの同期 `recv()` に混入する問題は確定で、**event-pump で一般 event channel から分離するべき**です。

一方、シーク後に pre-seek 音声が出る問題は、現在の非同期 `Cmd::Reset` では直りません。主因は、Reset が bridge の GUI/control thread で処理され、audio thread の `process_block()` と直列化されていないことです。さらに `PluginLoader::reset()` と `PluginLoader::process_block()` が別 thread から同時に `processor_` や `process_time_samples_` に触れるため、データ race でもあります。

したがって、本命は **B をベースにした同期 reset fence** です。ただし B 単独で「audio thread が in_ring を捨てて reset」だけにすると、Rust 側の `process_block()` が `pull_audio()` timeout になる可能性があります。安全形は:

1. Rust audio-pump が新 seek 世代を検出する。
2. 各 bridge に reset を要求する。
3. bridge では audio thread が、audio 処理と同じ thread 上で in/out ring を drain し、`loader_->reset()` を実行する。
4. reset 完了を専用 ack で Rust に返す。
5. Rust は ack 後に post-seek audio を `process_block()` へ流す。

これで「pre-seek input が plugin に入る」「pre-seek output が out_ring に残る」「reset と process が並行する」の 3 つを同時に潰せます。

## Q1: 真因は GUI/audio thread race か

はい。かなり高いです。

現在の `Cmd::Reset` は `main.cpp` の `handle_message()` で GUI/control loop 側が処理しています。一方、audio は別 thread の `audio_loop()` で `loader_->process_block()` を呼び続けています。両者の間に mutex や audio-thread fence がないため、Reset を送っても:

- Reset が処理される前に audio thread が post-seek block を処理する
- Reset と process が同時に走る
- pre-seek output が out_ring に残ったまま post-seek 側の `pull_audio()` に拾われる

という状態が起きます。

VST3 仕様上も、`setProcessing(false/true)` は process 呼び出し列の外側に置くべき操作です。Steinberg の VST3 Developer Portal では `process -> setProcessing(false) -> ... -> setProcessing(true) -> process` のような順序が示されており、threading model でも `process` と `setProcessing` は例外的に audio thread で呼ばれ得る関数とされています。つまり、mIV 側では **process と reset を同一 audio thread 上で直列化する**のが一番自然です。

## Q2: B と C どちらが良いか

**B + reset ack が最良**です。

案 C、つまり Rust 側で `ResetDone` を待つだけでは不十分です。なぜなら現在の `ResetDone` は GUI/control thread が `loader_->reset()` を呼んだ後に返すだけで、audio thread がその間に process していないことを保証しないからです。

ただし B も「非同期 flag を立てるだけ」では不十分です。Rust が reset 完了前に post-seek audio を push すると、その block が drain で捨てられて `pull_audio()` が timeout するか、タイミング次第で reset 前に処理されます。したがって、B は **audio thread 実行 + Rust 側 ack 待ち**として実装するのがよいです。

実装イメージ:

- `Cmd::Reset` を受けた control thread は `reset_pending_ = true` を立てるだけ。
- audio thread は loop 先頭で `reset_pending_.exchange(false)` を見る。
- true なら ring を drain し、`loader_->reset()` を呼び、`reset_done` を stdout へ出す。
- Rust audio-pump は `reset_plugins_sync()` で専用 ack を待ってから `process_block()` へ進む。

## Q3: in_ring / out_ring drain の方法

`in_ring` だけでは足りません。**in_ring と out_ring の両方を drain** してください。

理由:

- `in_ring`: pre-seek input が bridge 側に未処理で残っている可能性がある。
- `out_ring`: reset 前に生成済みの pre-seek output が Rust の次回 `pull_audio()` に拾われる可能性がある。

`read_in_available(... discard ...)` で in_ring を空にする方法でも動きますが、reset boundary では ring index を直接進める専用 API の方が明確です。

```cpp
void AudioPipe::discard_all() {
    auto& in_w = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->in_write);
    auto& in_r = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->in_read);
    auto& out_w = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->out_write);
    auto& out_r = *reinterpret_cast<std::atomic<uint32_t>*>(&header_->out_read);

    in_r.store(in_w.load(std::memory_order_acquire), std::memory_order_release);
    out_r.store(out_w.load(std::memory_order_acquire), std::memory_order_release);
}
```

これは通常時の SPSC 所有規則を少し破りますが、Rust 側が reset ack 待ちで `push_audio` / `pull_audio` していない reset fence 内なら安全にできます。逆に、Rust が待たない設計では out_ring index を bridge 側から触るのは危険です。

## Q4: A は単独でも価値があるか

あります。`ResetDone` は現在誰も待っていないのに一般 `event_rx` に流れており、`query_gui_size` / `show_gui` の同期応答を壊しています。これは race 本体と独立したバグです。

ただし、将来 sync reset を入れるなら、`ResetDone` を単に drop するより **一般 event channel からは分離し、reset 専用 ack に流す**のが良いです。

推奨:

- 現在の非同期 reset を残すだけなら: `ResetDone` は event-pump で drop。
- `reset_plugins_sync()` を入れるなら: `ResetDone` は event-pump で intercept し、専用 `reset_ack_tx` へ流す。一般 `recv()` には流さない。

`LatencyChanged` と同じく、「非同期/内部制御 event は GUI 同期応答 channel に混ぜない」が原則です。

## Q5: setProcessing を audio thread で呼んでよいか

はい、VST3 的には許容される方向です。Steinberg の threading model では、plug-in exported functions は基本 UI thread から呼ぶが、例外として `IAudioProcessor::process` と `IAudioProcessor::setProcessing` は audio thread から呼ばれ得る、と説明されています。

重要なのは、`setProcessing(false/true)` を `process()` と同時に呼ばないことです。mIV の現状は GUI thread reset と audio thread process が並行し得るため、ここが危険です。audio thread 自身が reset を行えば、少なくとも同一 bridge 内では `process` と `setProcessing` を直列化できます。

なお、`setProcessing(false/true)` で必ず全 plugin の delay-line が消えるとは限りません。仕様上は delay/reverb buffer reset に使える操作ですが、プラグイン実装依存です。mIV Test Latency のような検証 plugin は対応させるとして、一般 plugin では「reset しても tail が残る」可能性は残ります。その場合の最終手段は plugin reload か、seek 後の短い silence/mute policy です。

## revert について

未コミットの reset 追加は、現状では **一度 revert するのが安全**です。

理由:

- 症状を改善していない。
- `ResetDone` が一般 event channel に溜まり、GUI 系同期応答を壊している。
- `reset()` と `process_block()` の並行呼び出しを増やすため、thread safety 上も悪化している。

少なくとも、B + dedicated ResetDone handling を入れるまでは、この reset 呼び出しは無効化した方がよいです。revert 対象は `src/video/audio.rs` の `last_seen_seek_serial/reset_plugins()` 呼び出し追加と、`src/video/dsp/mod.rs` の `reset_plugins()` 追加分だけで十分です。

## 推奨実装順

1. いったん未コミット reset 追加を revert、または feature flag で無効化する。
2. `ResetDone` を一般 `event_rx` に流さない設計へ変更する。sync reset を入れるなら専用 ack channel を作る。
3. bridge 側に `reset_pending_` を追加し、audio thread が reset を実行する。
4. reset fence 内で `AudioPipe::discard_all()` 相当を呼び、in/out ring を両方 drain する。
5. audio thread から `reset_done` を返し、Rust pump は ack 後に post-seek audio を処理する。
6. それでも特定 plugin が tail を出す場合だけ、plugin reload や短時間 mute を個別対策として検討する。

## 参考

- Steinberg VST3 Developer Portal: Threading model では `process` と `setProcessing` が audio thread 例外として挙げられています。
  https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/API%2BDocumentation/Index.html
- Steinberg VST3 Developer Portal: Processing FAQ では `setProcessing(false)` / `setProcessing(true)` と `process()` の順序例が示されています。
  https://steinbergmedia.github.io/vst3_dev_portal/pages/FAQ/Processing.html

