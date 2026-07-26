# Codex レビュー依頼: audio pipeline 再設計 (raw/processed 分離) + seek baseline 遅延の調査

## 背景

`ca1baa2 → 4409d5a → 692dfd5 → f67617a → 00226cf` で導入した「fill_output の
engine_state gate + preroll buffer」は、ユーザー報告の「動画準備中の早送り」を
解消するつもりだったが、以下の退行を生んだ:

1. **VST 設定変更が長時間反映されない** (= preroll に最大 30 秒の post-VST
   音声が貯まる → EQ 知覚 latency = preroll 全長)
2. **シークが遅い** (= 復帰時に preroll 排出 + reset_plugins_sync 同期 wait)

`c526ee1` で全面 revert 済み。**現在は `engine_state` gate も preroll も無し**で、
原状回復した stable 状態。

ただしユーザー報告:
- ✅ VST 設定変更の即時反映が復旧
- ✅ シークの大幅遅延が解消
- ⚠️ **VST 全 OFF でも seek が他のプレーヤーに比べて遅い** (= 残存課題)

そして、AV1 等の長 GOP コーデックでの「resume seek 後の早送り」は revert で再発。
これを根本対応する必要がある。

## 提案する設計: raw / processed queue 分離

Codex の前回助言を反映した設計:

```
audio_decoder → audio_rx ─→ pump_thread ─→ raw_pending  (cap=10秒、AudioFrame の VecDeque)
                                |              ↓
                                |       (samples が cap 未満なら)
                                ↓        VST process (= bridge.process_block)
                            engine_event_tx       ↓
                            (BufferReady)     samples  (cap=300ms、interleaved f32 VecDeque)
                                                  ↓
                                              fill_output → cpal
```

### AudioBuffer (struct 拡張)

```rust
struct AudioBuffer {
    // post-VST: cpal が drain。cap = 0.3秒。EQ latency の指標。
    samples: VecDeque<f32>,
    next_pts_secs: f64,

    // pre-VST: pump が積む。cap = 10秒 (memory safety net)。
    // 各 frame の samples + pts + seek_serial を保持。
    raw_pending: VecDeque<AudioFrame>,

    // ... 既存 field
}
```

### pump (`run_pump`) の動作

```rust
loop {
    let frame = recv from audio_rx;

    // 1. stale 破棄 (= 既存)
    if frame.seek_serial < clock.current_seek_serial() { continue; }

    // 2. seek serial 切替: samples + raw_pending 両方クリア (= 既存と整合)
    if frame.seek_serial > buf.pump_seek_serial {
        buf.samples.clear();
        buf.raw_pending.clear();
        buf.next_pts_secs = frame.pts_secs;
        buf.pump_seek_serial = frame.seek_serial;
    }

    // 3. raw_pending に積む。cap 超過なら frame drop (= fail-closed)
    let raw_total = buf.raw_pending_total_samples();
    if raw_total + frame.samples.len() <= cap_samples_raw {
        buf.raw_pending.push_back(frame);
    }
    // else: drop (= log warning)

    // 4. raw → VST process → samples を「samples が cap 未満」の間繰り返す
    while buf.samples.len() < cap_samples_processed {
        let chunk = pop raw_pending or break;
        let processed = vst_process(&chunk.samples);  // = bridge.process_block
        buf.samples.extend(processed);
    }

    // 5. BufferReady 判定は (samples + raw_pending) 合計
    if total_secs >= READY_THRESHOLD { emit; }
}
```

### fill_output (engine_state gate を再導入)

```rust
fn fill_output(out, buffer, clock, engine_state) {
    // 1. pre-seek discard (= samples + raw_pending 両方クリア)
    // 2. engine_state gate: PLAYING 以外なら silence + 非 drain
    // 3. 通常 drain (= samples から pop)
}
```

### 効果

| シナリオ | raw_pending | samples | EQ latency | demux 状態 |
|---|---|---|---|---|
| 通常再生 | ≈0 (= 即 process) | 0.3秒 (= cap) | 0.3 秒 | OK |
| Buffering (= gate ON) | 増加 (max 10秒) | 0.3秒 (= 静止、再生されない) | 0.3 秒 | OK (= back-pressure なし) |
| Playing 復帰 | 排出中 | 0.3秒 (= drain と process が拮抗) | 0.3 秒 | OK |

ユーザー EQ 変更 → 次の VST process_block で即反映 → samples の前 (= 0.3秒以内) に到達。
`raw_pending` には未処理音声しか入っていないので **VST 設定変更が遅延しない**。

## 確認したい設計判断

### A. raw_pending の cap = 10 秒は妥当か?

- AV1 長 GOP の forward decode 時間: 典型 1-5 秒、稀に 8 秒
- audio decoder は 12-23x real-time なので、forward decode 中の audio decode が早く先回りする
- 10 秒あれば worst case (= 5 秒 forward decode 中に 5 秒先まで audio decode) も収まる
- memory: 10 sec * 96000 sample/sec * 4 byte = ~4 MB
- cap 超過時は **frame drop** (= fail-closed) で対応 → drop 範囲は audio に gap が発生するが、Buffering 中だけなので user 体験には影響しない

### B. fill_output の engine_state gate を再追加して問題ないか?

旧版 (= ca1baa2) の gate そのものに問題はなく、組み合わせた preroll の cap 仕様が
deadlock を作っていた。今回の raw_pending は **cap 超過時 drop** なので pump が
詰まらない → gate を再追加しても deadlock に戻らない。

懸念点:
- gate ON 中に audio_rx が cap 超過するほど来た場合は frame drop しか方法がない
  (= 音声 gap が発生)。10 秒 cap で実質 unreachable と判断
- gate OFF (= PLAYING) 中の挙動は今と同じ (= raw_pending ≈ 0、即 process → samples)

### C. seek baseline 遅延 (VST 全 OFF) の調査

ユーザー報告: VST 全 OFF でも他プレーヤー比で seek が遅い。

mIV 現状の seek 戦略 ([decoder.rs:721-742](C:/home/mimageviewer/src/video/decoder.rs)):

> Phase 9.F (2026-04-30): 前方/後方/絶対に関係なく **常に backward+preroll** を使う。
> backward+preroll なら video/audio 両方が **target 直前の keyframe** で始まり
> drop_before_secs で target にトリム → 確実に同位置で再生開始。
> GOP が長い動画では preroll decode のために 0.5〜3 秒余分にかかる

つまり「target 以前の最寄り keyframe から target まで forward decode して target frame
で再生開始」のため、**長 GOP コーデックで構造的に 0.5-3 秒の遅延**がある。

他プレーヤーは forward seek (= target 後の最寄り keyframe にスナップ) で速い代わりに、
audio との同期が崩れる (= video が target+1-3 秒、audio が target、不一致を後で吸収)。

**質問**:
- mIV の backward+preroll は A/V sync 担保のため。これを変えると同期が崩れる
- 折衷策として「短い seek (= 1 秒未満) は backward+preroll、長い seek は forward
  snap + 同期スキップ」のような hybrid は妥当か?
- もしくは mIV の現状 seek は「正しい」設計で、ユーザーの「他より遅い」感覚は
  受け入れる方針が正しいか?

### D. reset_plugins_sync の 2 秒 wait

VST 有効時、seek 後の最初の audio frame で `reset_plugins_sync()` を同期実行
([dsp/mod.rs:625](C:/home/mimageviewer/src/video/dsp/mod.rs))。各 active bridge ごとに
**最大 2 秒 wait** で逐次。N plugins → N × 2 秒の最悪ケース。

VST OFF なら影響なしだが、VST ON 時の seek 遅延要因として残る。

**質問**: 並列化 + timeout 短縮 (= 200ms 目安) は現実的か?

### E. 旧 preroll 関連コメントの掃除

`actor.rs` 周辺に preroll 前提の古いコメントが残っているが挙動には影響しない。
次に修正する人が混乱する可能性あり (Codex 前回助言)。

raw/processed 実装と同じセッションで掃除する予定。

## レビュー希望ポイント

1. **[P1] 設計判断 A-E のうちバグ / 抜け漏れ**:
   - 特に raw_pending cap 超過時の drop が seek 後再生開始に影響しないか
   - engine_state gate の再導入で deadlock が再発しないか (= raw_pending cap drop が
     真の back-pressure 解消になっているか)

2. **[P2] 改善案**:
   - raw → process → samples のループを pump 内 (single-thread) で行う設計で
     RT 性能上の問題はないか? 別 thread に分けるべきか?
   - PDC latency の更新タイミング (= raw 時点 / process 時点)

3. **[P2] seek baseline (= 設計判断 C)**:
   - 短 seek だけ forward snap にする hybrid は試す価値があるか?
   - もしくは現状の backward+preroll を許容するか?

4. **[P3] reset_plugins_sync 並列化** (= 設計判断 D):
   - bridge ごとに spawn して join するパターンで安全か?
   - timeout 200ms に短縮した場合、heavy plugin (= LUFS analyzer 等) で
     reset 失敗する確率は許容範囲か?

返答は P1/P2/P3 サマリ形式で。

## 触る予定のファイル

- `src/video/audio.rs`: AudioBuffer、run_pump、fill_output
- `src/video/mod.rs`: audio::start 引数 + engine_state_handle 引き渡し
- `src/video/engine/actor.rs`: 古い preroll コメント掃除
- `src/video/dsp/mod.rs`: reset_plugins_sync 並列化 (= 別タスクにしてもよい)

## 想定スコープ

- raw/processed 分離: ~300 行
- gate 再追加: ~30 行
- reset_plugins_sync 並列化: ~50 行 (= 採用する場合)
- テスト: ~100 行

## 関連コミット

- `c526ee1`: 旧 gate / preroll の全 revert (= 現在の base state)
- `f67617a`: latch preservation + anchor wall fix (= 保持中の独立 fix)
