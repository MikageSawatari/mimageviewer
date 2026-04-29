# 動画再生エンジン リデザイン (草案 v0)

## 背景

[docs/video-architecture.md](video-architecture.md) でまとめた現行構成の `AvClock` が、
**マスタークロック・再生状態・シーク要求・音量・EOF・音声バッファ会計** を 1 オブジェクトに
詰め込んでいるため、外部操作 (UI からの seek / pause / resume / volume) と
エンジン内部 (decoder thread / audio thread / UI tick) が同じ可変状態を直接書き換える。

これに起因して以下の実害が出ている (2026-04-29 観測):

1. **resume 開始時に序盤が早送り** — `VideoPlayer::open` で AvClock を `playing=true` で
   スタートさせた直後に UI tick から `request_seek(resume)` が走り、anchor が `0 →
   resume` にジャンプ。decoder は seek 完了後しばらく `audio_buf < AUDIO_SAFE_LO` で
   pacing skip し続けて video 急送出 → 動画開始の数百 ms が 2.5–3x 速度。
2. **暗黙の状態遷移が散らばっている** — 「seek 完了 → override clear」「resume 受領 →
   anchor ジャンプ」「audio frame 到着 → audio_active true」など、pause/seek の途中状態が
   名前付きステートにならず、コードで分岐ガード (`is_seeking()` / `is_audio_active()` /
   `audio_buf < SAFE_LO`) を **複数同時に評価** する必要があり、副作用が読みにくい。
3. **倍速再生の追加が困難** — 速度倍率を入れるには `now_secs()` の extrapolation 計算と
   audio resample rate の両方を協調更新する必要があるが、一つのオブジェクトに混ざって
   いる現状ではテストが書きづらい。

## 目標

| 優先 | 目標 |
|---|---|
| ★★★ | resume / 通常再生 / pause / seek / EOF 周回ループ を **明示的な state machine** に載せ、序盤早送り問題を構造的に解決 |
| ★★★ | 内部エンジン (decoder/audio thread) と外部 UI を **Controller API** で分離し、外部から触れる可変状態を絞る |
| ★★ | マスタークロックを単独の値オブジェクトとして抽出 (= unit test 可能) |
| ★★ | 倍速再生 (0.5x / 1.0x / 1.5x / 2.0x) を後から追加できる構造 |
| ★ | unsafe / Win32 依存は `gpu_renderer/` に閉じたまま |

スコープ外: 動画編集、字幕、HDR トーンマップ。

## 現行 `AvClock` の責務分解

70 箇所以上のメソッド呼び出しを 4 つの concern にグルーピング:

| Concern | 旧メソッド (例) | 観測される問題 |
|---|---|---|
| **MasterClock** (時計) | `now_secs` / `set_audio_pts` / `set_fallback_anchor` / `notify_seek_completed` (anchor 部分) / `notify_audio_active` / `mark_audio_inactive` | wall extrapolation が外部 UI 操作からも触れる。anchor 書き換えタイミングが分散。 |
| **TransportState** (再生状態) | `set_playing` / `is_playing` / `notify_eof_reached` / `clear_eof_reached` / `is_eof_reached` | playing と seeking と EOF が独立 atomic で、組合せ状態がコードに分散。 |
| **SeekController** (シーク調停) | `request_seek` / `take_seek_request` / `current_seek_serial` / `peek_seek_request_pending` / `is_seeking` / `clear_seek_target_override` / `seek_override_serial` | UI thread / decoder thread / audio thread から CAS でクリア合戦。serial と override の load 順がデリケート。 |
| **AudioBookkeeping** (音声バッファ会計) | `set_audio_pump_buf_secs` / `add_audio_tx_queued_secs` / `total_audio_buffer_secs` | decoder pacing 用の参照値だが、所有者が不明 (clock?audio?) |
| **Volume** | `volume` / `set_volume` / `is_muted` / `set_muted` | 純粋に表示状態。clock とは無関係。 |

## 採用する分解

```
┌─────────────────────────────────────────────────────────────────┐
│   VideoEngine (旧 VideoPlayer)                                  │
│                                                                 │
│   ┌──────────────────────┐    ┌──────────────────────────────┐  │
│   │ TransportController  │    │ EngineState (state machine)  │  │
│   │ (外部 UI が叩く API) │◄───┤ Idle/Loading/Buffering/      │  │
│   │ play, pause, seek,   │    │ Playing/Paused/Seeking/Eof   │  │
│   │ set_volume, …        │    └──────────────────────────────┘  │
│   └──────────┬───────────┘                                      │
│              │ command channel                                  │
│              ▼                                                  │
│   ┌──────────────────────┐    ┌──────────────────────────────┐  │
│   │ DecoderActor         │    │ AudioActor (cpal pump)       │  │
│   │ (demux+decode+blit)  │◄──►│ ringbuf, fill_output         │  │
│   └──────────┬───────────┘    └──────────────┬───────────────┘  │
│              │                               │                  │
│              ▼                               ▼                  │
│         ┌────────────────────────────────────────────┐          │
│         │  MasterClock (純粋な時計、状態を持たない)  │          │
│         │  read: now_secs(at_wall) / current_speed   │          │
│         │  write: set_anchor(pts, wall, source)      │          │
│         └────────────────────────────────────────────┘          │
└─────────────────────────────────────────────────────────────────┘
```

### 1. `MasterClock` (純粋な時計)

```rust
pub struct MasterClock { /* atomic anchor 1 個だけ */ }

#[derive(Clone, Copy, Debug)]
pub struct ClockAnchor {
    pub pts_secs: f64,
    pub wall_at_anchor: Instant,
    pub speed: f64,           // 1.0 = 等速、0.5 = 半速
    pub source: ClockSource,  // Audio | Wall | Frozen (= paused/seeking)
}

impl MasterClock {
    /// 現在 PTS を問い合わせる。Frozen なら anchor PTS を返す。
    /// Audio/Wall なら anchor PTS + (now - wall_at_anchor) * speed。
    pub fn now_secs(&self) -> f64 { ... }

    /// anchor を全置換する。**caller は EngineActor のみ** (Codex v2 P2 反映)。
    /// 他のスレッドからは絶対に呼ばない (= type-level に強制したいので、関数は
    /// `pub(super)` にして video モジュール内からだけ呼べるようにする)。
    pub(super) fn set_anchor(&self, anchor: ClockAnchor) { ... }
}
```

- **書き手は EngineActor のみ** (Codex P1 反映)。`AudioActor` / `DecoderActor` / UI tick
  は anchor を直接書かず、events として `EngineActor` に送る。**全 events は seek_epoch
  を含む** (= stale 検出のため):
  - `AudioRendered { epoch, pts, wall_now }` (cpal callback ごと)
  - `SeekCompleted { epoch, actual_pts }` (decoder seek 完了)
  - `FirstFrameReady { epoch, pts }` (post-seek/open の最初の動画 frame)
  - `BufferReady { epoch, pts, wall_now }` (audio buffer が READY_THRESHOLD 到達)
  - `BufferStarved { epoch }` (audio underrun = SAFE_LO 未満)
  - `EofReached { epoch, duration }` (decoder EOF)
  - `AudioInactive` (audio 出力起動失敗、epoch 非依存)
  EngineActor はこれらの events を直列に処理して `MasterClock.set_anchor` を呼ぶ。
  → 旧 AvClock の「複数経路から同じ atomic を書く race」が消える。
  - 追加 events: `FirstFrameReady` (decoder)、`BufferReady` (audio)、
    `BufferStarved` (audio underrun)。Buffering↔Playing の遷移トリガに使う
    (Codex v2 P2 反映、event リスト網羅性)。
- 単調性ガードはここに **残さない**。EngineActor が「直前の event 内容と比較」で
  書き込み判定する。書き込み箇所が 1 箇所に集約されるので unit test も容易。
- `speed` を持たせることで倍速再生の基盤を最初から仕込んでおく。
- `Frozen` は paused / loading / seeking 中の状態。time-stop により tick で
  extrapolation が暴走しない。

### 2. `EngineState` (state machine)

**重要**: `Buffering` などの非 `Playing` 状態は「Clock を Frozen にする」だけでは
不十分。Codex P1 指摘:

- decoder pacing は Clock で pacing するため、Frozen のままでは pace_now が進まず
  `ahead = pts - pace_now` が常に大きく、PACE_LEAD で sleep する。これは正しい挙動。
- しかし pacing loop の **`audio_buf < AUDIO_SAFE_LO` escape** が走ると pacing skip。
  この escape を **`state == Playing` のときだけ有効** に変更する (Buffering / Seeking
  中は escape させない)。
- さらに UI 側でも `Buffering` 中は video frame を **表示しない** (= 既存サムネ or
  最初の post-seek frame だけを hold)。これで「画面上は Frozen 期間がある」+「pacing
  も走らない」の二重保証が成立。


```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineState {
    /// VideoEngine 構築直後。decoder thread 未起動。
    Idle,
    /// open 中: decoder spawn 済み、info_rx 待ち。
    Loading,
    /// info 受領 + resume seek 適用中。最初の post-seek frame 到着待ち。
    /// Clock は Frozen で時間進行なし。UI には既存の thumbnail を表示。
    Buffering { resume_target: Option<f64> },
    /// 再生中。Clock は Audio/Wall で進行。
    Playing,
    /// 一時停止中。Clock は Frozen。
    Paused,
    /// シーク中。pre-seek frames を drain して target 到達まで待つ。
    /// Clock は Frozen (= override target 値で固定)。
    Seeking { target: f64 },
    /// EOF 到達 (loop_enabled=false で停止状態)。Clock は Frozen at duration。
    Eof,
}
```

遷移図:

凡例: `READY = FirstFrameReady ∧ (NoAudio ∨ BufferReady)` (= readiness latch、
詳細は「latch ロジック」節)

```
Idle ── open() ──► Loading ── info_received(no_resume) ──► Buffering ── READY ──► Playing
                            └ info_received(resume) ──► Seeking{resume} ── seek_completed ──► Buffering ── READY ──► Playing
Playing ── pause() ──► Paused
Paused ── resume_play() ──► Playing
Playing ── seek(t) ──► Seeking{t} ── seek_completed ──► Buffering ── READY ──► Playing
Paused ── seek(t) ──► Seeking{t} ── seek_completed ──► Buffering ── READY ──► Paused (※ paused 維持)
Playing ── eof_reached ──► Eof
Eof ── seek(t) ──► Seeking{t} ── … ──► Playing
Eof ── loop=true ──► Seeking{0} ── … ──► Playing
```

備考: 簡潔化のため `Buffering` の前に必ず `Seeking` を経由しない経路 (= 通常 open
かつ resume なし) は `Loading → Buffering` を直結。READY 遷移は **latch 揃い** が
唯一のトリガで、単体イベントでは遷移しない (= 「first_frame だけで Playing」は誤り)。

**重要な不変条件**:
- `Loading` / `Buffering` / `Seeking` / `Paused` / `Eof` のとき MasterClock は
  必ず `Frozen` source。**時間が暴走することがない**。
- `Buffering` から `Playing` への遷移トリガは **`FirstFrameReady ∧ (NoAudio ∨ BufferReady)`**
  の latch。両 readiness イベントは seek_epoch スコープで管理し、片方が先着しても
  もう片方を待つ (詳細は「latch ロジック」節)。これにより resume 中に「anchor は
  target だが decoder はまだ pts=0 をデコード中」というギャップが Engine API レベル
  で存在しない (= UI からは Buffering 中、Clock は Frozen at target、画面はサムネイル静止)。
- 序盤早送りの根本原因 (= AUDIO_SAFE_LO による pacing skip) も `Buffering` 中は
  「audio が満たされてから Playing に遷移」する保証で消える。

### 3. `TransportController` (外部 API)

```rust
pub struct TransportController { /* command_tx + state subscription */ }

impl TransportController {
    pub fn play(&self);
    pub fn pause(&self);
    pub fn toggle_play(&self);
    pub fn seek_absolute(&self, secs: f64);
    pub fn seek_relative(&self, delta_secs: f64);
    pub fn set_speed(&self, speed: f64);   // 0.25..4.0
    pub fn set_volume(&self, v: f64);
    pub fn set_muted(&self, m: bool);
    pub fn set_loop_enabled(&self, e: bool);
    pub fn position_secs(&self) -> f64;    // = MasterClock.now_secs()
    pub fn state(&self) -> EngineState;
    pub fn duration_secs(&self) -> Option<f64>;
}
```

- 操作はすべて **command channel** 経由で `EngineActor` に渡る。
- 外部から直接書ける可変状態は **Volume と LoopEnabled のみ** (これらは clock 進行に
  影響しないので atomic で公開しても安全)。
- `position_secs()` は MasterClock を読むだけ。

### State / Anchor の publish 順序 (Codex v2 P2 反映)

EngineActor が `state = Playing` と `clock.set_anchor(...)` を続けて行うとき、
**必ず anchor を先に書いてから state を書く**。`engine.state()` を読む側
(decoder pacing) は state を後発で観測した時点で anchor が古い frozen のままだと
「state=Playing なのに pace_now が動かない」race を起こす。

```rust
// 正: anchor → state の順
self.clock.set_anchor(ClockAnchor::audio(pts, wall_now));
self.published_state.store(EngineState::Playing as u8, Ordering::Release);

// 誤: state → anchor の順 だと race
```

decoder pacing 側は `Acquire` で state を読み、その後 `Acquire` で anchor を読む。
release/acquire の同期で anchor の最新値を確実に観測できる。

### 4. `EngineActor` (内部調停)

ロックフリー I/O (RT 経路: cpal callback、wgpu paint) と分離するため、状態遷移は
**専用 worker thread** で命令 + イベントの統合ループとして処理する。

```rust
fn run(...) {
    let mut state = EngineState::Idle;
    loop {
        select! {
            cmd = command_rx => apply_command(&mut state, cmd, &clock, ...),
            evt = decoder_event_rx => apply_event(&mut state, evt, &clock, ...),
            evt = audio_event_rx => apply_event(&mut state, evt, &clock, ...),
        }
        publish_state(&state);
    }
}
```

- **シーク要求の調停**もここで行う。`apply_command(SeekAbsolute(t))` 受信で
  `handle_seek_request(t, ...)` (= epoch++ + latch reset + decoder へ SeekTo)、
  state を `Seeking { target: t }` に遷移。
- decoder からの `SeekCompleted { epoch, actual_pts }` イベントで `ev.epoch <
  current_seek_epoch` を捨てた上で Clock anchor を `Frozen at actual_pts` に書き換え、
  state を `Buffering` に遷移。**まだ Playing ではない** — readiness latch
  (`FirstFrameReady ∧ (NoAudio ∨ BufferReady)`) が揃って初めて
  `transition_to_playing(audio_anchor)` が呼ばれる。
- `BufferReady` 単独では Playing には遷移しない (= 動画 frame の準備が先に揃って
  いれば BufferReady で latch 完成、揃っていなければ FirstFrameReady を待つ)。

これにより `AvClock::is_seeking()` / `clear_seek_target_override` のような
**多方向 CAS 競合が消える**。状態は単一の Actor が直列に管理する。

### 5. AudioBookkeeping の整理

`audio_pump_buf_secs` / `audio_tx_queued_secs` は decoder pacing が「audio 枯渇しそうか」
を判定するためのもの。これは **AudioActor が所有** し、`AudioStatus { buf_secs }` という
read-only スナップショットを atomic で公開する。

decoder の pacing は `engine.audio_status().buf_secs < SAFE_LO` で参照する。

## resume の atomic open path

これが本リデザインの最大の利益:

```rust
impl VideoEngine {
    /// resume_secs を含めた open。state は完了するまで Loading→Buffering で
    /// MasterClock は Frozen のまま動かない。
    pub fn open(path: PathBuf, opts: OpenOptions) -> Self {
        // OpenOptions { initial_volume, autoplay, resume_secs, loop_enabled, hw_decode }
        let clock = Arc::new(MasterClock::new_frozen_at(0.0));
        let (cmd_tx, cmd_rx) = ...;

        // EngineActor は state=Idle で起動、すぐに自己 cmd::Open を投げる
        std::thread::spawn(move || run_engine(clock, cmd_rx, opts));

        Self { transport: TransportController { cmd_tx, ... } }
    }
}

// EngineActor 内:
match state {
    Idle => {
        decoder.open(path)?;
        state = Loading;
    }
    Loading => match decoder_event {
        InfoReceived(info) => {
            duration = Some(info.duration_secs);
            if let Some(resume) = opts.resume_secs.and_then(in_range) {
                decoder.send(DecoderCmd::SeekTo(resume));
                state = Buffering { resume_target: Some(resume) };
                clock.set_anchor(ClockAnchor::frozen_at(resume));
            } else {
                state = Buffering { resume_target: None };
                clock.set_anchor(ClockAnchor::frozen_at(0.0));
            }
        }
    }
    Buffering => {
        // 詳細は「latch ロジック」節 — handle_readiness_event で epoch スコープ latch
        // を維持し、両 latch が揃った時点で transition_to_playing(anchor) helper 経由で
        // **anchor → state の順** で publish する (Codex v3 P2)。
        // ここでは概要のみ:
        //   first_frame_ready && (!has_audio || buffer_ready) →
        //     if opts.autoplay { transition_to_playing(audio_anchor) }
        //     else             { transition_to_paused_at(audio_anchor.pts) }
    }
    ...
}
```

**ポイント**:
- `Loading` / `Buffering` で Clock は **Frozen** = 時間進行 0。decoder pacing の
  pace_now も Frozen 値。pacing skip 条件 (`audio_buf < SAFE_LO`) を踏まないので
  decoder は通常 pacing で送出。
- audio buffer が **READY_THRESHOLD (500ms)** 溜まってから `Playing` 遷移
  (Codex P2 反映: 旧 AUDIO_SAFE_LO = 250ms と同値だと low-water level に張り付き、
  jitter で即 buffering に戻る。ready 500ms / low 250ms の hysteresis を採用)。
- VBR audio でも sample-count ベースで 500ms = 24000 stereo pairs @ 48kHz と決まるので
  PTS 揺らぎの影響を受けない。
- `Playing` 遷移と同時に `set_anchor(audio_pts, wall_now)` で、wall extrapolation 起点を
  「buffering 完了時刻」に揃える。

## Buffering deadlock 回避 (Codex v2 P1 反映)

「Buffering 中は decoder pacing が sleep するなら、audio fill も止まり、READY_THRESHOLD
500ms に永久に到達できないのでは?」という deadlock リスクは、後述の「Decoder pacing
規定 (確定版)」で **Buffering 中は pacing skip + audio decode/send は state 不問で常時
実施** とすることで構造的に消える。詳細は当該節を参照。

**latch ロジック** (Buffering→Playing 遷移):

Codex v3 P1 反映: latches は **必ず seek_epoch でスコープ化** する。stale な pre-seek
event が新世代の latch を埋めて誤遷移するのを防ぐ。

```rust
struct ReadinessLatch {
    epoch: u64,
    first_frame: bool,
    /// FirstFrameReady event が返した actual_pts (= post-seek の最初の動画 frame PTS)。
    /// 音声無し動画の anchor source として使う。
    first_frame_pts: Option<f64>,
    buffer_ready: bool,
    /// BufferReady event が返した最初の有効 audio PTS / wall (anchor 設定用)。
    audio_anchor: Option<(f64, Instant)>,
}

// すべての readiness event は seek_epoch を含む:
//   DecoderEvent::FirstFrameReady { epoch, pts }
//   AudioEvent::BufferReady       { epoch, pts, wall_now }
//   AudioEvent::BufferStarved     { epoch }

fn handle_readiness_event(&mut self, ev: ReadinessEvent) {
    if ev.epoch() < self.current_seek_epoch { return; }  // stale を捨てる
    if ev.epoch() > self.latch.epoch {
        // 新世代: latch を全 reset
        self.latch = ReadinessLatch::new(ev.epoch());
    }
    match ev {
        FirstFrameReady { pts, .. } => {
            self.latch.first_frame = true;
            self.latch.first_frame_pts = Some(pts);
        }
        BufferReady { pts, wall_now, .. } => {
            self.latch.buffer_ready = true;
            self.latch.audio_anchor = Some((pts, wall_now));
        }
        ...
    }
    self.try_transition_to_playing();
}

fn try_transition_to_playing(&mut self) {
    if self.state != Buffering { return; }
    let audio_ready = !self.has_audio || self.latch.buffer_ready;
    if self.latch.first_frame && audio_ready {
        // helper: anchor → state の順を強制 (Codex v3 P2 反映)
        let anchor = if self.has_audio {
            let (pts, wall) = self.latch.audio_anchor.expect("buffer_ready implies anchor");
            ClockAnchor::audio(pts, wall)
        } else {
            let pts = self.latch.first_frame_pts.expect("first_frame implies pts");
            ClockAnchor::wall(pts, Instant::now())
        };
        if self.opts.autoplay_after_buffer {
            self.transition_to_playing(anchor);
        } else {
            self.transition_to_paused_at(anchor.pts);
        }
    }
}

// 状態遷移 helper: 必ずこれを通すこと
fn transition_to_playing(&mut self, anchor: ClockAnchor) {
    debug_assert!(matches!(anchor.source, ClockSource::Audio | ClockSource::Wall));
    self.clock.set_anchor(anchor);                              // ① anchor を Release で先に書く
    self.published_state.store(Playing as u8, Release);          // ② state を Release で後に書く
}

// epoch の唯一の発行点 = seek 要求受領時 (SeekAbsolute / SeekRelative / Resume)
fn handle_seek_request(&mut self, target: f64, source: SeekSource) {
    self.current_seek_epoch += 1;          // ★ epoch++ はここだけ
    self.latch = ReadinessLatch::new(self.current_seek_epoch);
    let dir = match source { ... };
    self.decoder_cmd.send(DecoderCmd::SeekTo { target, epoch: self.current_seek_epoch, dir });
    self.transition_to_seeking(target);
}

// 状態遷移 helper: enter_buffering / enter_seeking は **epoch を進めない**
//   (= 既に handle_seek_request で進んでいる、Codex v3 P1 反映の epoch ownership 整理)
fn transition_to_seeking(&mut self, target: f64) {
    self.clock.set_anchor(ClockAnchor::frozen_at(target));
    self.published_state.store(Seeking as u8, Release);
}

// SeekCompleted は **epoch++ せず**、decoder の actual_pts を受け取って Buffering に遷移
fn handle_seek_completed(&mut self, ev: SeekCompleted) {
    if ev.epoch < self.current_seek_epoch { return; }  // stale
    // この時点で latch は handle_seek_request 時に reset 済 → そのまま使う
    self.clock.set_anchor(ClockAnchor::frozen_at(ev.actual_pts));
    self.published_state.store(Buffering as u8, Release);
    // 以降 readiness events を待つ
}

// Open path 初期化 (resume なし) は epoch=0 から開始、enter_buffering は epoch++ なしで Buffering へ
fn enter_initial_buffering(&mut self) {
    self.latch = ReadinessLatch::new(self.current_seek_epoch);  // epoch=0 のまま
    self.published_state.store(Buffering as u8, Release);
}
```

両 event を seek_epoch スコープで **latch** することで:
- 片方が先に着いてももう片方を待つ → Playing への遷移が確実に起きる (deadlock 解消)
- pre-seek の stale event は epoch 不一致で捨てられる → 古い frame で誤遷移しない
- transition は helper 経由で必ず anchor → state の順 (publish 順序の race 防止)

## Decoder pacing 規定 (確定版、Codex v3 P2 反映)

旧 decoder pacing:
```rust
if clock.is_audio_active() && clock.total_audio_buffer_secs() < AUDIO_SAFE_LO {
    break;  // escape (急送出開始)
}
```

新 decoder pacing (state 別、唯一の規定):

| State | video pacing | video try_send 失敗時 | audio decode/send | 備考 |
|---|---|---|---|---|
| `Playing` | 通常: `pts - pace_now <= PACE_LEAD` で break、それ未満なら 5ms sleep | 通常 (Full なら drop) | 常時実施 | 唯一 escape (audio_buf<SAFE_LO) が有効 |
| `Buffering` (preroll) | **pacing なし即送出** | drop (UI に表示しない約束) | 最大速で実施 → READY_THRESHOLD まで進める | escape 不要 (元から sleep しない) |
| `Loading` | (= Buffering と同じ動き、まだ info も来てないので decode 自体起こらない) | — | — | — |
| `Seeking` | pacing なし即送出 (pre-seek frame は UI で drop) | drop | 同上 | epoch++ 後の post-seek frame を急いで送る |
| `Paused` / `Eof` | decoder thread park (= `cancel.load==false && state in {Paused,Eof}` で wait_condvar) | — | park | thread が起きるのは外部 command 受信時のみ |

```rust
// 単一の pacing 規定 (擬似コード):
loop {
    let s = engine.published_state();
    match s {
        Paused | Eof => { wait_condvar(); continue; }  // thread park
        _ => {}
    }
    let frame = decode_one_frame()?;
    let pts = frame.pts;
    if matches!(s, Playing) {
        // 通常 pacing (Buffering/Seeking ではここをスキップ)
        loop {
            let ahead = pts - clock.now_secs();
            if ahead <= PACE_LEAD { break; }
            if audio_status.buf_secs < AUDIO_SAFE_LO { break; }  // escape
            sleep(5ms);
        }
    }
    let _ = video_tx.try_send(frame);  // Full なら drop (Buffering/Seeking では正常)
}
```

→ **resume のような post-seek 直後は `Buffering` 状態なので pacing 自体が走らず、
audio が READY_THRESHOLD まで溜まってから `Playing` に遷移**。Playing 遷移と同時に
通常 pacing が有効化される。これで「序盤早送り」問題が **構造的に消える**。

「Buffering 中は audio fill が止まらないか?」(deadlock 懸念) はこの規定で解消:
audio decode/send は state 不問で常時実施されるため、`READY_THRESHOLD` (500ms) は
必ず到達する。

## Command 順序保証 (Codex P3 反映)

`TransportController` から `EngineActor` への command channel:

| Command | drop 可? | coalesce 可? |
|---|---|---|
| `Play` / `Pause` | NG (取りこぼすと UI と engine の play 状態がズレる) | OK (連打で最後だけ反映) |
| `SeekAbsolute` | NG | スクラブ中のみ最新値で coalesce、コミット seek は coalesce 不可 |
| `SeekRelative` | NG | 不可 (各 +5s が累積するため) |
| `SetVolume` | OK | OK (連打で最新だけ反映) |
| `SetSpeed` | OK | OK |
| `SetMuted` / `SetLoopEnabled` | OK | OK |
| `Shutdown` | NG | OK |

実装方針:
- channel は `crossbeam_channel::unbounded` ではなく **`bounded(64)`** + drop 不可
  command 用 priority lane (overflow 時は別経路で送る)
- coalesce はクライアント側 (TransportController) で「同種 command が pending なら
  上書き」を実装。EngineActor 内では基本的にすべての command を順次処理。

## Paused / Eof からの seek 後の状態 (Codex P3 反映)

| 元状態 | seek 後の遷移先 |
|---|---|
| `Playing` | `Seeking { target } → Buffering → Playing` |
| `Paused` | `Seeking { target } → Buffering → Paused` (autoplay しない) |
| `Eof` (loop=false) | `Seeking { target } → Buffering → Playing` (Eof 解除) |
| `Eof` (loop=true) | `Seeking { 0 } → Buffering → Playing` (周回) |

→ 一時停止中のシーク UI 操作で意図せず再生開始しない。

## Shutdown 順 (Codex P3 反映)

VideoEngine drop 時:
1. `TransportController::shutdown()` で EngineActor に `Shutdown` 命令送出
2. EngineActor は `cancel.store(true)` + decoder/audio を join
   - 順序: **AudioActor を先に止める** (cpal stream stop で RT callback 完了待ち)
   - 次に DecoderActor を cancel + join
3. EngineActor 自身も exit
4. VideoEngine の最後に `gpu_latest: D3d11Frame` を drop (= NT shared HANDLE close)
   - ここで wgpu (D3D12) 側の paint がまだ texture を持っていても、**fence_gen + 内部
     refcount で生存** (旧コードと同じ保証)

## migration plan (4 phase、各 phase で動作維持)

### Phase 1: skeleton 導入 (動作変化なし)
- 新規ファイル `src/video/clock_v2.rs` に `MasterClock` を導入
- `EngineState` enum を `src/video/engine_state.rs` で定義
- 旧 `AvClock` はそのまま残す。新型は誰も使わない。
- compile-only PR、テストは `cargo test` で型チェックのみ

### Phase 2: AvClock を facade として再実装
- AvClock は **薄い facade** (= 各フィールドの単一所有者は新型側、AvClock は read-only
  の互換レイヤー)。並列実装ではなく、状態の真実は新型のみが持つ (Codex P2 反映)。
- 4 つの concern (Clock / TransportState / SeekController / AudioBookkeeping) を
  別オブジェクトに分け、AvClock のメソッドは新型へ delegate
- decoder.rs / audio.rs / VideoPlayer は依然 AvClock を見るが、書き込みは新型に渡る
- ここまでで動作は変わらないが、**resume の急送出問題はまだ残る** (open path 未変更)

### Phase 3: 外部 API を `TransportController` に切替 + open path atomic 化
- VideoPlayer の API を `TransportController` 経由に置換
- `VideoPlayer::open(path, opts)` で `OpenOptions { resume_secs, ... }` を受け取り、
  EngineActor が `Loading→Buffering→Playing` で resume を atomic に処理
- **ここで序盤早送り問題が消える**
- UI 側 (ui_fullscreen.rs) も TransportController に書き換え

### Phase 4: 旧 AvClock 削除 + ドキュメント整備
- AvClock とそれに依存する全 API を撤去
- `docs/video-architecture.md` を新構造に合わせて全面改訂
- `CLAUDE.md` に「動画再生エンジンの操作は TransportController 経由のみ」と明記

各 phase でリリース可能な状態を保つ (= 中間状態でも動画は再生できる)。

## 検証

### Phase 2 完了時 (resume 未修正)
- 既存挙動と diff なしを確認 (= 旧 perf log と新 perf log を比較)

### Phase 3 完了時 (resume 修正)
- **resume seek を含む再生開始の最初 1 秒**で、`pace_now - wall_offset` の比が 1.0 ±
  10% に収まること
- video.decode の events/100ms が wall 全期間で安定 (60fps 動画なら 6 ± 1 events/100ms)
- 倍速 (1.5x / 0.5x) でもまんべんなく安定
- フルHD/4K/30fps/60fps の 4 組合せ × resume あり/なし の合計 8 ケース手動回帰

### Phase 4 完了時
- `cargo test` 全通過
- 静的検査: `grep -r AvClock src/` がヒット 0

## 倍速再生 (Codex P2 反映)

`MasterClock.speed: f64` を最初から設計に入れるが、実装は段階的に:

- Phase 4 までは `speed = 1.0` 固定 (= 既存挙動維持)
- 倍速 UI 追加時:
  - **動画 PTS pacing**: speed 倍率で `now_secs()` の extrapolation を係数化
  - **音声 resample**: swresample の output rate を `cpal_rate / speed` に動的変更
    - swresample の ratio change はサポートされているが click が出やすい
    - `swr_set_compensation` で 100ms 程度かけて smooth に切替
    - **pitch-preserving** (speed 変えても声が高くならない) は外部 lib (SoundTouch /
      Rubber Band) が必要 → 初版では pitch shift で OK の前提

## 旧 monotonic guard の扱い

`set_audio_pts` の `pts_secs.max(prev_anchor)` ガードは **新型でも維持** する
(post-seek の古い callback で anchor が後退するケースの保護)。ただし設計位置は
EngineActor の `AudioRendered` event ハンドラ内に置く:

```rust
fn handle_audio_rendered(&mut self, ev: AudioRendered) {
    // seek_epoch で世代管理。post-seek (epoch++) では last_audio_pts を reset し、
    // 古い epoch の callback を捨てる (Codex v2 P1 反映)。
    if ev.seek_epoch < self.current_seek_epoch { return; }
    if ev.seek_epoch > self.last_audio_epoch {
        // 新世代の最初のサンプル: ガードをリセット
        self.last_audio_epoch = ev.seek_epoch;
        self.last_audio_pts = ev.pts;
    } else if ev.pts < self.last_audio_pts {
        return;  // 同世代内で後退する callback は捨てる
    } else {
        self.last_audio_pts = ev.pts;
    }
    if matches!(self.state, EngineState::Playing) {
        self.clock.set_anchor(ClockAnchor::audio(ev.pts, ev.wall_now));
    }
    // Buffering / Seeking / Paused のときは anchor 更新しない (= Frozen 維持)
}

// 注: handle_seek_completed は **epoch++ しない** (epoch++ は handle_seek_request の
// 1 箇所のみ — v4 epoch ownership 整理参照)。ここでは latch がすでに新世代に reset 済
// なので、stale な AudioRendered は ev.epoch < current_seek_epoch で自動的に捨てられる。
```

`Buffering` 中に `AudioRendered` が来ても anchor を進めない。これで「buffering 中に
wall ジャンプで pace_now が動画 PTS を抜く」問題が起きない。

backward seek (例: 30秒位置 → 10秒位置) でも、`handle_seek_request` で epoch++ した
タイミングで latch が reset され、次の `AudioRendered` (= 新 epoch + pts=10) が
`last_audio_pts` を 10 にリセットするので、「古い 30 と比較して 10 を捨てる」誤動作が
起きない (epoch++ は seek 要求受領時の 1 回のみで、SeekCompleted では進めない)。

## Codex レビュー反映ログ (2026-04-29)

### v0 → v1 (初回 P1/P2/P3 対応)

| 指摘 | 反映先 |
|---|---|
| P1: Buffering は Clock frozen だけでは不十分、pacing escape も封じる必要 | 「Pacing escape の構造変更」節を追加 + EngineState のコメント追補 |
| P1: anchor writer を 1 箇所に絞る (EngineActor のみ) | MasterClock 節の「書き手は EngineActor のみ」 + events リスト |
| P2: ready 閾値 250ms は低水位、500ms ready / 250ms low の hysteresis | atomic open path 節のコメント追補 |
| P2: adapter は facade (single source of truth)、parallel impl は ng | Phase 2 の説明を facade と明記 |
| P2: 倍速は swresample で技術的に可能、pitch 維持は外部 lib | 「倍速再生」節を追加 |
| P3: command channel ordering policy 明文化 | 「Command 順序保証」節を追加 |
| P3: Paused/Eof からの seek 完了後の状態 | 「Paused / Eof からの seek」節を追加 |
| P3: shutdown 順 (audio → decoder → GPU frame) | 「Shutdown 順」節を追加 |

### v1 → v2 (再レビュー対応)

| 指摘 | 反映先 |
|---|---|
| P1: Buffering 永久ハングのリスク (audio fill 自体も止まる) | 「Buffering deadlock 回避」節を追加 — preroll モードで decoder pacing を完全 skip |
| P1: FirstFrameReady と BufferReady の latch がない | 同節 — 両 event を latch して両方揃ってから Playing 遷移 |
| P1: monotonic guard を seek epoch でリセットしない | 「旧 monotonic guard の扱い」節 — seek_epoch を導入して epoch++ で reset |
| P2: `set_anchor` の doc が「caller=TransportController/AudioActor」のまま | MasterClock signature を `pub(super)` に + コメント修正 |
| P2: state と anchor の publish 順序が未規定 | 「State / Anchor の publish 順序」節を追加 — anchor → state の Release 順、reader は Acquire |
| P2: events リストに FirstFrameReady / BufferReady が漏れていた | events リストに追記 |

### v2 → v3 (3 度目のレビュー対応)

| 指摘 | 反映先 |
|---|---|
| P1: latch が generation-scope なし、stale event が新世代の latch を満たすリスク | 「latch ロジック」節を全面書き換え — `ReadinessLatch { epoch }` で seek_epoch スコープ + 入場時 reset + stale event を捨てる |
| P2: pseudocode で `state = Playing; set_anchor` の順になっていた | `transition_to_playing(anchor)` helper を導入、open path も helper 経由に書き換え |
| P2: BufferReady の anchor source が underspecified | `BufferReady { epoch, pts, wall_now }` と event 構造体に明示、latch の `audio_anchor` フィールドに保存 |
| P2: Buffering pacing の規定が複数節で矛盾 | 「Decoder pacing 規定 (確定版)」節を 1 つに集約、state 別表で唯一の規定に |

### v3 → v4 (4 度目のレビュー対応)

| 指摘 | 反映先 |
|---|---|
| P1/P2: epoch ownership が enter_buffering と handle_seek_completed の両方で進む | epoch++ を `handle_seek_request` の 1 箇所に集約、SeekCompleted や enter_buffering は epoch を進めない |
| P2: Buffering 旧記述と確定版の矛盾 | 旧 deadlock 節を確定版へのリンクに置換 |
| P2: Buffering→Playing trigger が「最初の frame だけ」と「2 latch」で矛盾 | invariants 節を 2 latch 表現に統一 |
| P2: no-audio path の `first_frame_pts` field が ReadinessLatch 未定義 | `first_frame_pts: Option<f64>` を field に追加、FirstFrameReady ハンドラで保存 |

### v4 → v5 (5 度目のレビュー対応)

| 指摘 | 反映先 |
|---|---|
| P1/P2: monotonic guard pseudocode の `handle_seek_completed` で `epoch += 1` が残っていた | 該当行を「epoch++ しない」コメントに置換、stale 検出が ev.epoch 比較で完結することを明記 |
| P2: 遷移図に `── first_frame ──► Playing` の単独 trigger 表記が残っていた | 遷移図を全面改訂、`READY = FirstFrameReady ∧ (NoAudio ∨ BufferReady)` の latch 表現に統一、Buffering→Playing は単独 event ではなく READY 揃いがトリガと明記 |
| P2: EngineActor 散文に「BufferReady で Playing 遷移」と書かれていた | `BufferReady` 単独では遷移しない旨を明記、latch 完成のみがトリガと書き直し |

### v5 → v6 (6 度目のレビュー対応)

| 指摘 | 反映先 |
|---|---|
| P1/P2: backward seek 解説に「SeekCompleted で epoch 進む」記述が残っていた | epoch++ は handle_seek_request のみ、SeekCompleted では進めない旨に修正 |
| P2: events リストの `SeekCompleted { actual_pts }` 等に epoch field が無かった | 全 events に epoch field を含める方針を明記、各 event の signature を修正 (SeekCompleted/FirstFrameReady/BufferReady/BufferStarved/EofReached/AudioRendered) |

## 参考

- mpv: <https://github.com/mpv-player/mpv/blob/master/player/playloop.c> の state
  machine が同様の Idle/Loading/Buffering/Playing/Paused/Eof 構造
- Chromium: `media::PipelineStatus` + `Renderer` の分離が本提案と同形
- ffplay: `is->paused` / `is->seek_req` を 1 つの `VideoState` 構造体に詰めている
  → 我々の AvClock と同じ問題を持つ (移行先として参考にしない)
