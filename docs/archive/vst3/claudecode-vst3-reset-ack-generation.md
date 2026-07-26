# ClaudeCode 指示: VST3 ResetDone ack に generation id を追加

## 背景

現在の VST3 seek reset 実装は、`Cmd::Reset` を bridge に送り、bridge audio thread が `reset_pending_` を処理して `reset_done` を返す構造になっている。

ただし、Codex review で以下の指摘が出ている。

> `drain_reset_acks(); send(reset); wait_reset_done()` still has a stale-ack race after any prior timeout: an old ResetDone can arrive after the drain but before the current reset completes, then `wait_reset_done` accepts it and post-seek audio is sent before the real reset fence. Add a monotonically increasing reset id to Cmd::Reset and reset_done, or keep a per-bridge pending generation so the waiter only accepts the matching ack.

つまり、過去の reset が timeout した後、その古い `ResetDone` が次回 reset の `drain` 後に到着すると、次回 reset の ack と誤認される可能性がある。

## 目的

`ResetDone` ack に世代 ID を持たせ、`wait_reset_done` が **現在送った reset に対応する ack だけ** を受理するようにする。

この修正では、seek 後 pre-seek 音声残留そのものの根治までは扱わない。まず ack race を確実に潰す。

## 対象ファイル

- `src/video/dsp/bridge.rs`
- `src/video/dsp/mod.rs`
- `crates/vst3-host/src/main.cpp`
- 必要なら `crates/vst3-host/include/protocol.h`

## 実装方針

### 1. Rust `Cmd::Reset` に `reset_id` を追加

`src/video/dsp/bridge.rs` の `Cmd::Reset` を、unit variant から ID 付き variant に変更する。

例:

```rust
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    Reset { reset_id: u64 },
    ...
}
```

既存の JSON は `{"cmd":"reset"}` だったが、修正後は:

```json
{"cmd":"reset","reset_id":123}
```

### 2. Rust `Event::ResetDone` に `reset_id` を追加

`Event::ResetDone` も ID 付きにする。

例:

```rust
ResetDone { reset_id: u64 },
```

### 3. `Bridge` に reset id counter を持たせる

`Bridge` に `AtomicU64` などを追加する。

例:

```rust
next_reset_id: AtomicU64,
```

`Bridge::reset_sync(timeout)` のような helper を作るのが望ましい。

推奨 API:

```rust
pub fn reset_sync(&self, timeout: Duration) -> bool {
    let id = self.next_reset_id.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    self.send(&Cmd::Reset { reset_id: id })?;
    self.wait_reset_done(id, timeout)
}
```

`DspBridge::reset_plugins_sync()` 側で `drain_reset_acks()` / `send()` / `wait()` を個別に組み立てるのではなく、`Bridge` 内に閉じ込める方が race を作りにくい。

### 4. reset ack channel は `u64` を流す

現状の `reset_ack_rx: Receiver<()>` を `Receiver<u64>` に変更する。

event-pump:

```rust
Ok(Event::ResetDone { reset_id }) => {
    let _ = reset_ack_tx.try_send(reset_id);
}
```

`wait_reset_done(expected_id, timeout)` は、timeout までに届いた ack を読み、`expected_id` と一致するものだけ成功扱いにする。

古い ID は捨てる。未来 ID が来る設計は通常ないが、来た場合はログを出して捨ててよい。

疑似コード:

```rust
pub fn wait_reset_done(&self, expected_id: u64, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        match self.reset_ack_rx.recv_timeout(deadline - now) {
            Ok(id) if id == expected_id => return true,
            Ok(id) => {
                crate::logger::log(format!(
                    "[VST3] ignored stale ResetDone ack id={id}, expected={expected_id}"
                ));
            }
            Err(_) => return false,
        }
    }
}
```

この形なら `drain_reset_acks()` は不要になる可能性が高い。残す場合でも、正しさは ID matching に依存させる。

### 5. C++ bridge で reset_id を保持して返す

`crates/vst3-host/src/main.cpp` の reset command handler で、JSON から `reset_id` を読む。

現状:

```cpp
if (cmd == "reset") {
    reset_pending_.store(true, std::memory_order_release);
    return true;
}
```

修正案:

```cpp
if (cmd == "reset") {
    auto id = extract_number_field(msg, "reset_id");
    pending_reset_id_.store(id, std::memory_order_release);
    reset_pending_.store(true, std::memory_order_release);
    return true;
}
```

audio loop 側:

```cpp
if (reset_pending_.exchange(false, std::memory_order_acq_rel)) {
    auto id = pending_reset_id_.load(std::memory_order_acquire);
    pipe_.discard_all();
    if (loader_) {
        loader_->reset();
    }
    write_message("{\"event\":\"reset_done\",\"reset_id\":" + std::to_string(id) + "}");
    continue;
}
```

`pending_reset_id_` は `std::atomic<uint64_t>` でよい。

### 6. 複数 reset の coalescing 注意

現在の C++ 側は `reset_pending_` が bool なので、reset が連続すると最後の `pending_reset_id_` だけが有効になる。

audio-pump は同期的に待つので通常は同時に複数 reset は飛ばないはずだが、堅牢にするなら:

- bool + latest id で十分と割り切る
- もしくは pending reset id queue にする

今回の最小修正では bool + latest id でよい。ただしコメントで「reset は同期呼び出し前提で coalesce される」と明記する。

## 受け入れ条件

1. `ResetDone` が一般 `event_rx` に流れないこと。
2. 古い `ResetDone` が次回 reset の成功として扱われないこと。
3. `DspBridge::reset_plugins_sync()` が、各 bridge の現在の reset id に対応する ack だけを待つこと。
4. reset timeout 後、遅延して古い ack が到着しても、次回 reset を誤完了させないこと。
5. C++ bridge の stdout message が壊れないこと。

## 推奨テスト

可能なら unit test か小さな integration test で、以下を確認する。

- `wait_reset_done(expected=2)` 中に `ack=1` が届いても成功しない。
- `ack=1` の後に `ack=2` が届けば成功する。
- timeout 後に古い ack が channel に残っても、次回 expected id と一致しなければ捨てられる。

実機確認:

1. PDC 1500ms 以上の plugin を有効化。
2. seek / W キーで reset が発火する。
3. timeout ログが出ない。
4. `unexpected ResetDone` が出ない。

## 注意

今回の修正は ack race の修正であり、seek 後にまだ古い音が聞こえる問題の完全な根治ではない可能性がある。

もし ack generation 修正後も古い音が残る場合、次に疑うべきは:

- plugin 側が `setProcessing(false) -> true` で delay-line を実際には消していない
- reset 後に PDC latency 分の post-seek pre-roll discard が必要
- WASAPI / cpal / AudioBuffer 側の既出力分がユーザーに聞こえている

ただし、このファイルの作業スコープではそこまで実装しない。

