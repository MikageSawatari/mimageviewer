# 動画倍速再生機能 仕様

更新日: 2026-05-08

この文書は、動画再生に 0.5x から 3.0x の倍速再生を追加するための実装仕様である。
Claude Code 案は参考にしたが、現在のコードと合わない箇所、または根拠が弱い箇所は採用しない。

## 1. 決定事項

- 音声タイムストレッチは **Signalsmith Stretch** を採用する。
- 音声はピッチを維持する。主な評価軸は音楽よりもセリフの聞き取りやすさとする。
- VST3 には「タイムストレッチ後の等倍サンプル列」を渡す。
- 速度は設定に保存する。動画を切り替えても、アプリ再起動後も維持する。
- UI はシークバー下部 HUD に速度ボタンを追加し、クリックで速度候補を選択できるポップアップを出す。

## 2. Claude Code 案から修正する点

次の点は、現在のコードまたは Signalsmith の公開 API と合わないため修正する。

| 項目 | Claude Code 案 | Codex 仕様 |
| --- | --- | --- |
| 速度の共有先 | `src/video/engine/state.rs` の `Shared.playback_speed` を前提 | 現在の実動作は `AvClock` が中心。まず `src/video/clock.rs` に速度を持たせ、既存の decoder/audio/native path が読む。`EngineActor` は将来統合とテスト整合のためにミラーする |
| `VideoPlayer` からの速度送信 | `transport_tx` 経由を前提 | 現在の facade に合わせて `VideoPlayer::set_playback_speed()` から `AvClock` と `EngineActor` の両方を更新する。存在しないフィールドを前提にしない |
| Signalsmith API | `SignalsmithStretch::new()` や `process_interleaved()` を想定 | crate `signalsmith-stretch` 0.1.3 の `Stretch::preset_default()` / `preset_cheaper()` / `process()` / `seek()` / `reset()` / `input_latency()` / `output_latency()` を前提にする |
| 音声 PTS | `audible_pts = raw.pts - pdc_latency` のままでよいとする | 速度 != 1.0 では VST/Safety/Signalsmith の出力レイテンシ秒を source timeline に換算する必要がある。原則は `latency_source_secs = latency_output_secs * playback_speed` |
| `ProcessedChunk.duration_secs` | source 秒と output 秒が曖昧 | `duration_secs` は cpal に出す output/wall 秒として維持する。別に `source_secs_per_output_sec` を持つ |
| audio tx queue 秒数 | speed と直交として扱う | `total_audio_buffer_secs()` は再生可能な wall 秒でそろえる。decoder が積む `audio_tx_queued` は `source_duration / speed_at_enqueue` で加算し、同じ値を pump 側で減算する。速度変更時は tx 会計をゼロ化し、旧世代 frame の減算で新世代会計を壊さない。epoch は偶数を安定状態、奇数を速度変更中として扱う |
| native presenter / repaint delay | 未整理 | source PTS 差を wall 待ち時間に変換する箇所は `source_delta / speed` にする |
| 3.0x 音質 | 高音質を断定 | Signalsmith 公式は 0.75x から 1.5x 程度を得意範囲としている。3.0x は UI 要件として対応するが、音質は実素材で検証する |

## 3. ユーザー向け仕様

### 3.1 速度候補

UI に表示する速度は次の 11 段階とする。

```text
0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0
```

内部 clamp は 0.25x から 4.0x まで許容してよいが、通常 UI から選べるのは 0.5x から 3.0x までにする。

### 3.2 UI

- HUD の mute ボタンと volume slider の間に速度ボタンを置く。
- ボタン表示は `x1`、`x1.25`、`x2` のように短くする。
- クリックで HUD の上側にポップアップを出す。
- ポップアップは 11 個のボタンを横並びまたは折り返しグリッドで表示する。
- 画面端ではポップアップ位置を clamp し、シークバーや時間表示に重ならないようにする。
- legacy egui path と native overlay path の両方に実装する。

### 3.3 設定への保持

- `Settings` 側に `video_playback_speed: f64` を追加し、初期値は 1.0 にする。
- `App` 側にも `video_playback_speed: f64` を保持し、起動時に `Settings` から復元する。
- 動画を開き直した直後に、現在の `video_playback_speed` を新しい `VideoPlayer` に適用する。
- 速度変更イベントを受けたら `App.video_playback_speed` / `Settings.video_playback_speed` /
  `VideoPlayer` を更新し、設定へ保存する。

## 4. アーキテクチャ

### 4.1 再生速度の責務

現在の再生 path では、実際の位置取得に `src/video/clock.rs` の `AvClock` が使われている。
`src/video/engine/clock.rs` の `MasterClock` と `ClockAnchor.speed` は存在するが、`EngineActor` だけを更新しても現在の `VideoPlayer::position()` や decoder/audio path には十分に届かない。

そのため、初回実装では次の構造にする。

- `AvClock` が実再生速度の source of truth になる。
- `AvClock::playback_speed() -> f64` を decoder/audio/native presenter から読む。
- `AvClock::set_playback_speed(speed)` は現在 PTS を維持したまま anchor を張り替える。
- `AvClock::set_playback_speed(speed)` は audio tx 会計をゼロ化し、`audio_tx_accounting_epoch` を進める。epoch は odd/even の 2 段階で更新し、decoder の snapshot は安定した偶数 epoch だけを採用する。
- `EngineActor::TransportCommand::SetSpeed` は no-op のままにせず、内部 `MasterClock` と状態ログの整合用に実装する。ただし現在の実再生制御は `AvClock` 優先と明記する。

### 4.2 Clock anchor

`AvClock` で anchor を作り直す箇所は、現在速度を必ず保持する。

対象:

- `set_playing(true)`
- `set_audio_pts()`
- `set_audio_pts_jump()`
- `write_audio_anchor_at()`
- `write_fallback_anchor_at()`
- `set_fallback_anchor()`
- `set_position_at_eof()`
- `notify_seek_completed()`
- EOF からの再開
- loop wrap

`write_audio_anchor_at()` と `write_fallback_anchor_at()` は、`ClockAnchor::audio()` / `ClockAnchor::wall()` の戻り値に必ず `with_speed(self.playback_speed())` を適用する。これにより `set_audio_pts_jump()`、`notify_seek_completed()`、`set_fallback_anchor()` などの呼び出し元をまとめて保護する。

`set_position_at_eof()` は表示位置を duration で凍結するが、`AvClock` が持つ playback speed 自体は変更しない。EOF から再生し直すときは、その保存済み speed で新しい anchor を作る。

`clamp_playback_speed()` は `src/video/clock.rs` の `pub(crate)` helper として定義し、`AvClock` / `VideoPlayer` / `EngineActor` / `App` から同じ clamp を使う。

`set_audio_pts()` の wall-rate cap は速度に応じて拡大する。speed < 1.0 では cpal callback の早着ジッタで cap が過剰発火しやすいため、等速時より広い倍率余裕を使う。ただし、過去に固定 5ms 加算で clock が速く進みすぎた経緯があるので、callback ごとに固定秒を足す方式は避ける。

```rust
let speed = self.playback_speed();
let max_audio_clock_rate = if speed < 1.0 { 1.10 } else { 1.02 };
let max_advance = wall_dt * max_audio_clock_rate * speed;
```

速度変更時は、変更前の `now_secs()` を先に取得し、その PTS を新しい速度の anchor にする。

```rust
let pts_now = self.now_secs();
let speed = clamp_playback_speed(speed);
self.set_anchor_at(pts_now, Instant::now(), speed, current_source);
```

seek と速度変更が同時に発生した場合は、seek 完了時の `notify_seek_completed()` がその時点の `playback_speed()` を読んで anchor を作る。pre-seek の audio queue 会計は seek flush と `zero_audio_tx_queued_secs()` で清算する。

### 4.3 動画 pacing

decoder 側の pacing は source timeline で比較しているため、先読み量は速度に応じて source 秒で増やす。

```rust
let speed = clock.playback_speed();
let pdc_source_latency = clock.vst3_pdc_latency_secs();
let pace_lead = (PACE_LEAD_SECS * speed + pdc_source_latency)
    .min(VIDEO_QUEUE_LEAD_CAP_SECS);
```

`SEEK_BURST_LEAD_MAX_SECS` も同じ考え方で speed を掛ける。ただし最初は既存 cap を維持し、3.0x で video queue starvation が出た場合だけ cap または queue 容量を広げる。

3.0x で decoder が追いつかない場合は、通常の slow disk / slow decode と同じ扱いにする。video queue が wall-time で継続的に枯渇したら既存の Buffering 遷移へ入り、clock を一時停止して再バッファを待つ。倍速専用の busy wait や同期ブロックは追加しない。

UI tick や native presenter の待ち時間は wall 秒で指定される。source PTS 差をそのまま sleep/repaint delay にしない。

```rust
let source_delta = next_pts_secs - clock.now_secs();
let wall_delay = source_delta.max(0.0) / clock.playback_speed().max(0.25);
```

対象:

- `VideoPlayer::tick()` の次回 repaint 計算
- native presenter の present delay / frame sleep 計算
- overlay redraw cadence で動画フレーム PTS をもとに待つ箇所

## 5. 音声処理

### 5.1 処理順

採用する順序は次の通り。

```text
decoded source audio
  -> Signalsmith Stretch
  -> VST3 chain
  -> safety limiter
  -> processed queue
  -> cpal callback
```

VST3 は通常速度のサンプル列だけを見る。VST3 側に playback speed を渡さない。

### 5.2 Signalsmith Stretch

依存は `signalsmith-stretch = "0.1.3"` を候補にする。

公開 API として次を前提にする。

- `Stretch::new(channel_count, block_length, interval)`
- `Stretch::preset_default(channel_count, sample_rate)`
- `Stretch::preset_cheaper(channel_count, sample_rate)`
- `Stretch::process(input, output)`
- `Stretch::exact(input, output)`
- `Stretch::flush(output)`
- `Stretch::seek(input, playback_rate)`
- `Stretch::reset()`
- `Stretch::input_latency()`
- `Stretch::output_latency()`

品質優先の初期値は `preset_default` とする。CPU 負荷が問題になる場合のみ `preset_cheaper` を検討する。

Signalsmith は入力と出力の buffer 長を変えることで time-stretch する。`process()` の input/output は interleaved `f32` で、channel count は `Stretch` 構築時の値に合わせる。2.0x 再生では、同じ source 入力から約 1/2 の output 長を作る。

```rust
let output_frames = (input_frames as f64 / playback_speed).round() as usize;
stretch.process(input_interleaved, output_interleaved);
```

実装では丸め誤差が蓄積しないよう、`source_frames_consumed` と `output_frames_produced` の比率を accumulator で管理する。

`Stretch::seek(input, playback_rate)` は AV seek ではなく、Signalsmith 内部状態を温める pre-roll API である。wrapper では `prime()` のような名前に包み、timeline seek と混同しないようにする。

実装 Phase 3 の最初に `cargo doc` または docs.rs で `signalsmith-stretch` の実シグネチャを確認し、crate version を上げる場合はこの節も同時に更新する。

### 5.3 新規 wrapper

`src/video/audio_stretch.rs` を追加する。

責務:

- `signalsmith_stretch::Stretch` の所有
- interleaved stereo f32 の入出力
- 速度ごとの output frame 数計算
- `input_latency()` / `output_latency()` の source timeline 換算
- seek serial 変更時の `reset()` / 必要なら `seek()`
- 1.0x 近傍の bypass

`Stretch` は `Send + Sync` だが、実際には audio-pump thread 内だけで所有する。

### 5.4 1.0x bypass

速度が 1.0x のときは Signalsmith を通さず、既存処理と同じサンプル列を VST3 に渡す。

ただし、1.0x と非 1.0x を切り替える瞬間には内部 buffer の残りで PTS が曖昧になりやすい。初期実装では **1.0x bypass と非 1.0x の境界**、および AV seek 後だけ stretcher を reset し、古い stretched pending output は破棄する。

1.25x から 1.5x のような非 1.0x 同士の変更では reset しない。Signalsmith は呼び出しごとの input/output 長の比率で time-stretch するため、ratio の連続変更として扱う。クリック時に非常に短い音切れが出る可能性は許容し、必要なら後続タスクで 30ms から 50ms の crossfade を追加する。

### 5.5 `ProcessedChunk` metadata

`ProcessedChunk` は output/wall 秒と source timeline を分けて持つ。

```rust
struct ProcessedChunk {
    samples: Vec<f32>,
    audible_pts_secs: f64,              // source timeline
    duration_secs: f64,                 // output/wall seconds
    source_secs_per_output_sec: f64,    // usually playback_speed_at_process
    seek_serial: u64,
    latency_source_secs_at_process: f64,
}
```

`fill_output()` が cpal に渡す途中 PTS は、output 秒を source 秒に戻して計算する。

```rust
let output_consumed_secs = drain_samples as f64 / samples_per_sec;
let pts_for_video = chunk.audible_pts_secs
    + output_consumed_secs * chunk.source_secs_per_output_sec;
clock.set_audio_pts(pts_for_video);
```

### 5.6 latency と PDC

VST3/Safety limiter/Signalsmith の latency は、cpal に出る output 秒として発生する。
しかし `audible_pts_secs` と video pacing は source timeline で扱うため、速度を掛けて source 秒に換算する。

```rust
let latency_output_secs =
    vst_pdc_output_secs + safety_limiter_output_secs + stretcher_output_latency_secs;
let latency_source_secs = latency_output_secs * playback_speed_at_process;
let audible_pts_secs = (raw.pts_secs - latency_source_secs).max(0.0);
```

Signalsmith は `input_latency()` と `output_latency()` の 2 種類を返す。初期実装では次のように扱う。

- `output_latency()` は output 秒として PTS 補正に含める。
- `input_latency()` は seek/pre-roll のために使う。実装が複雑になる場合、最初は reset 後の短い立ち上がりずれを許容し、ログで確認する。
- `vst3_pdc_latency_secs()` という既存名は残してもよいが、値の意味は「audio path 全体の source timeline latency」に寄せる。可能なら後続で `audio_latency_source_secs()` に改名する。

### 5.7 queue 秒数の単位

キュー詰まり対策として、秒数の単位を混ぜない。

| 値 | 単位 | 仕様 |
| --- | --- | --- |
| `AudioFrame.duration_secs` | source 秒 | decoder が読んだ元音声の長さ |
| `ProcessedChunk.duration_secs` | output/wall 秒 | cpal が実際に消費する長さ |
| `raw_pending_secs` | source 秒 | pump 内部の raw supply 管理 |
| `audio_tx_queued_secs` | output/wall 秒 | decoder から pump へ送ったがまだ pump が受けていない再生可能秒数 |
| `total_audio_buffer_secs()` | output/wall 秒 | pacing と BufferReady 判定に使う |

`AudioFrame` に `queued_wall_secs` を追加するか、同等の sidecar 値を持たせる。

```rust
let queued_wall_secs = frame.duration_secs / speed_at_enqueue;
clock.add_audio_tx_queued_secs(queued_wall_secs);
frame.queued_wall_secs = queued_wall_secs;
```

pump が `AudioFrame` を受け取ったら、必ず同じ `queued_wall_secs` を減算する。受信時の現在速度で再計算しない。

`decoder.rs` の `add_audio_tx_queued_secs` 呼び出しは、enqueue と drop/abort 経路を対称に更新する。現時点の対象は少なくとも次の 4 箇所である。

- enqueue 前の `+duration_secs`
- seek serial 変化時の rollback
- engine park 時の rollback
- channel disconnected 時の rollback

drop/abort 経路では `duration_secs` を直接使わず、enqueue 時に local bind した `queued_wall_secs` を使って減算する。`audio.rs` の pump intake 側も `-frame.duration_secs` ではなく `-frame.queued_wall_secs` にする。

速度変更時には、すでに audio_tx に入っている frame の `queued_wall_secs` が旧 speed ベースで残る。これをそのまま残すと `total_audio_buffer_secs()` が過大または過小になり、短時間の starvation 判定を誤る。速度変更時は次を行う。

1. `audio_tx_accounting_epoch` を奇数に進め、会計遷移中にする。
2. `zero_audio_tx_queued_secs()` で tx 会計をゼロにする。
3. `audio_tx_accounting_epoch` を偶数に進め、新しい安定世代にする。
4. decoder は enqueue 時に、安定した偶数 epoch の snapshot として `queued_wall_secs` と `audio_tx_accounting_epoch` を `AudioFrame` に入れる。
5. pump/drop 経路は frame の epoch が現在 epoch と一致する場合だけ `queued_wall_secs` を減算する。

旧 epoch frame は raw 音声としては有効なので捨てない。pump に届いた時点の現在 speed で stretch する。ただし tx 会計上は速度変更時に無効化済みとして扱う。

## 6. native overlay と legacy UI

### 6.1 legacy egui path

`src/ui_fullscreen.rs` の `draw_video_hud()` に速度ボタンとポップアップを追加する。

UI helper はできるだけ共有する。

```rust
fn draw_video_speed_button(
    ui: &mut egui::Ui,
    current_speed: f64,
    popup_open: &mut bool,
) -> Option<f64>
```

戻り値が `Some(speed)` のとき、`App.video_playback_speed` / `Settings.video_playback_speed` /
`VideoPlayer` を更新する。

### 6.2 native overlay path

`src/video/native_presenter.rs` にも同等 UI を追加する。

追加するイベント/コマンド:

```rust
NativeOverlayCommand::SetPlaybackSpeed { speed: f64 }
NativeVideoOutputEvent::SetPlaybackSpeed { speed: f64 }
```

native overlay の state に `playback_speed: f64` を追加し、UI thread から毎 frame 最新値を渡す。この値はボタン表示と選択状態のためだけに使う。present delay、frame sleep、動画 PTS の計算は常に `clock.playback_speed()` を読む。

## 7. 変更対象ファイル

| ファイル | 変更内容 |
| --- | --- |
| `Cargo.toml` | `signalsmith-stretch = "0.1.3"` を追加 |
| `src/video/audio_stretch.rs` | Signalsmith wrapper 新設 |
| `src/video/audio.rs` | raw -> VST の前に stretcher を挿入。`ProcessedChunk` と queue 秒数を修正。pump intake は `queued_wall_secs` と accounting epoch を使う |
| `src/video/clock.rs` | playback speed の保持、anchor 再構築、wall-rate cap 修正、`clamp_playback_speed()`、audio tx accounting epoch |
| `src/video/decoder.rs` | pacing lead と audio tx queued 秒数を speed 対応。enqueue/drop/abort の全経路で `queued_wall_secs` を対称に加減算 |
| `src/video/mod.rs` | `VideoPlayer::set_playback_speed()` / `playback_speed()` 追加、tick delay 修正 |
| `src/video/engine/actor.rs` | `TransportCommand::SetSpeed` を実装し、内部 `MasterClock` の速度を更新 |
| `src/video/native_presenter.rs` | native HUD 速度 UI、イベント、present delay 修正 |
| `src/ui_fullscreen.rs` | legacy HUD 速度 UI |
| `src/app.rs` | speed の保持と設定保存、動画切替時の再適用 |
| `docs/video-architecture.md` | 実装後に倍速再生の構造を追記 |
| `docs/video-engine-redesign.md` | 実装後に Phase 4 speed 配線の完了状態を更新 |
| `htdocs/mimageviewer/manual/video.html` | 実装後に操作説明を追記 |

## 8. 実装順序

### Phase 1: clock と UI なしの速度配線

1. `AvClock` に playback speed を追加する。
2. `clamp_playback_speed()` を `src/video/clock.rs` に追加し、速度設定経路で共通利用する。
3. anchor 再構築箇所で speed を保持する。`write_audio_anchor_at()` / `write_fallback_anchor_at()` を中心に修正し、`set_position_at_eof()` も確認する。
4. `set_audio_pts()` の wall-rate cap を speed 対応にする。speed < 1.0 の cap hit は perf/log で確認する。
5. `audio_tx_accounting_epoch` と speed 変更時の `zero_audio_tx_queued_secs()` を追加する。
6. `VideoPlayer::set_playback_speed()` / `playback_speed()` を追加する。
7. `EngineActor::SetSpeed` を実装し、内部 clock のテスト整合を取る。

### Phase 2: video pacing

1. decoder の `PACE_LEAD_SECS` と seek burst lead を speed 対応にする。
2. `VideoPlayer::tick()` の次回 repaint delay を `source_delta / speed` にする。
3. native presenter の present delay と frame sleep を `source_delta / speed` にする。

### Phase 3: audio stretcher

1. docs.rs または `cargo doc` で `signalsmith-stretch` の実 API を確認し、この仕様と差があれば先に文書を直す。
2. `audio_stretch.rs` を追加する。
3. `audio.rs` の raw -> VST 前に Signalsmith を挿入する。
4. `ProcessedChunk` に `source_secs_per_output_sec` と latency metadata を追加する。
5. `fill_output()` の PTS 計算を source-aware にする。
6. `audio_tx_queued_secs` を wall 秒にそろえる。decoder enqueue/drop/abort と pump intake の全経路を `queued_wall_secs` + epoch で更新する。
7. AV seek と 1.0x bypass 境界で stretcher を reset する。非 1.0x 同士の速度変更では reset しない。
8. perf event `audio/stretch` を追加し、Signalsmith 処理時間、input frames、output frames、speed を記録する。

### Phase 4: UI

1. `Settings.video_playback_speed` と `App.video_playback_speed` を追加する。
2. legacy HUD に速度ボタンとポップアップを追加する。
3. native overlay に同じ UI とイベントを追加する。
4. 動画切替時に保存済み speed を再適用する。

### Phase 5: documentation と manual

1. `docs/video-architecture.md` を更新する。
2. `docs/video-engine-redesign.md` を更新する。
3. `htdocs/mimageviewer/manual/video.html` を更新する。
4. 実装中に本仕様と差分が出た場合は、この `docs/playback-speed-design.md` も最終状態に更新する。

## 9. テスト計画

### 9.1 unit tests

- `AvClock` の speed anchor:
  - 2.0x で `now_secs()` が wall の約 2 倍進む。
  - 速度変更直前と直後で PTS が飛ばない。
  - seek/play/pause/loop/EOF 後も speed が 1.0 に戻らない。
  - `set_position_at_eof()` 後に再生再開しても保存済み speed が使われる。
- `set_audio_pts()`:
  - 2.0x で wall-rate cap が過剰に PTS を抑制しない。
  - 0.5x で通常 callback cadence の PTS を cap が過剰に抑制しない。
- audio queue bookkeeping:
  - `AudioFrame.duration_secs = 0.2`、speed=2.0 の enqueue で `audio_tx_queued_secs` が 0.1 増える。
  - pump 受信時に同じ 0.1 が減る。
  - speed 変更で `audio_tx_queued_secs` がゼロ化され、旧 epoch frame の減算が新 epoch の会計を壊さない。
  - decoder の seek/park/disconnect drop 経路が enqueue と同じ `queued_wall_secs` を rollback する。
- audio stretcher:
  - AV seek と 1.0x bypass 境界では reset する。
  - 1.25x から 1.5x のような非 1.0x 同士の変更では reset しない。
- `ProcessedChunk`:
  - speed=2.0 で output を 50ms 消費したとき、source PTS が 100ms 進む。
  - pre-target trim が source timeline で正しく切れる。
- UI/presenter delay:
  - speed=2.0 で 33ms source delta の待ち時間が約 16.5ms になる。

### 9.2 実機確認

次の速度を確認する。

```text
0.5x, 1.0x, 1.5x, 2.0x, 3.0x
```

確認項目:

- ピッチが維持される。
- セリフが実用的に聞き取れる。
- 速度変更直後に再生が止まらない。
- seek 連打と速度変更の組み合わせで buffer が詰まらない。
- VST3 有効時に音が途切れ続けない。
- native overlay path と legacy path の両方で UI が動く。
- 3.0x で decode が追いつかない場合、queue cap や lead cap の調整が必要か perf-log で判断する。
- `audio/stretch` perf event で Signalsmith 処理時間が継続的に audio budget を超えない。

### 9.3 音質評価

Signalsmith 公式は time-stretch の得意範囲を 0.75x から 1.5x 程度としているため、2.0x 以上は必ず実素材で確認する。

評価素材:

- セリフ中心の動画
- BGM ありのセリフ動画
- 効果音や拍手など transient が多い動画
- VST3 を有効にした動画

3.0x の音質が実用に届かない場合でも、UI 要件として速度自体は残す。必要なら後続で「2.0x 超は音質低下がありうる」旨の扱い、または別アルゴリズム A/B テストを検討する。

## 10. 参考リンク

- [signalsmith-stretch crate 0.1.3 docs.rs](https://docs.rs/signalsmith-stretch/latest/signalsmith_stretch/)
- [signalsmith_stretch::Stretch API](https://docs.rs/signalsmith-stretch/latest/signalsmith_stretch/struct.Stretch.html)
- [Signalsmith Stretch official repository](https://github.com/Signalsmith-Audio/signalsmith-stretch)

## 11. 実装レビュー観点

実装後のレビューでは、次を重点的に見る。

1. source 秒と output/wall 秒が混在していないか。
2. 速度変更、seek、pause/play、loop で anchor speed が 1.0 に戻っていないか。
3. `set_audio_pts()` の cap が speed を考慮しているか。
4. Signalsmith latency と VST3 PDC が source timeline に換算されているか。
5. `audio_tx_queued_secs` が wall 秒として加算/減算されているか。
6. 速度変更時の `audio_tx_accounting_epoch` 更新で旧 frame の減算が新会計を壊していないか。
7. native presenter と legacy path の両方で frame wait が `source_delta / speed` になっているか。
8. 1.0x bypass が既存音声 path を極力変えないか。
9. 1.0x bypass 境界と AV seek 以外で stretcher reset が過剰に発生していないか。
10. 3.0x で queue 詰まりが起きたときに、cap/queue 調整で原因を切り分けられるログが残っているか。
