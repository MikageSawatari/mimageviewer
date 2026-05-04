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

### Phase 4: AvClock を薄い facade として固定 + ドキュメント整備
- AvClock は **削除しない** (= 当初計画から軌道修正)。
  - 理由 1: 内部状態の大半は既に Phase 2b で `MasterClock` + `AudioBookkeeping` に
    移譲済 (= AvClock は薄い facade になっている)。
  - 理由 2: `decoder.rs` / `audio.rs` / `mod.rs` 計 89 箇所の AvClock 呼び出しを直接
    `EngineActor` API に書き換える変更は範囲が広く、Phase 3e で安定したばかりの
    再生挙動を回帰させるリスクが大きい。
  - 理由 3: AvClock 経由で動かしてもエンジンの **source of truth** は EngineActor
    (= `published_state` + 内部 epoch) なので、設計目標 (= 序盤早送り問題の構造的解決 +
    内部/外部の責務分離) は既に達成されている。
- 代わりに `clock.rs` のドキュメンテーションコメントで **AvClock は互換 facade で
  あり、新規コードからは EngineActor を直接叩くこと** を明記する。
- `docs/video-architecture.md` を新モジュール構成 (engine/) に合わせて改訂。
- `CLAUDE.md` に動画メタ情報の扱いポリシーを追記 (= 外部ダウンローダ名を文書に
  載せない方針、新機能ロードマップで関連)。
- Phase 5 以降の UI/UX 機能ロードマップを本ドキュメント末尾に追加。

将来 AvClock を完全撤去する場合は、Phase 5+ の機能追加で `decoder.rs` / `audio.rs`
側のロジックに手を入れるタイミングで callsite ごとに段階移行するのが安全。

各 phase でリリース可能な状態を保つ (= 中間状態でも動画は再生できる)。

### Phase 9: decoder.rs を 3-thread 構造に分離 (2026-04-30)

Phase 8.K (commit 6685e41) の対症療法 (PACE_LEAD=0.30 / post_seek_frame_sent flag /
generation race check 等) で序盤再生は安定したが、音声付き動画 (Candyfloss_test /
SilentBloom 等) で「audio_tx (bounded=32) 満杯 → 単一 decode thread 全体が block →
video もデコード止まる → buf 0/24 振動」の構造的問題が残っていた。

**Phase 9 で `run_decoder` 単体を 3 thread 構成に分離**:

- `video-decode` thread (= `run_decoder` 本体): demux + seek 調停 + EOF idle wait に
  専念。`Input::packets()` の packet を stream index で振り分けて `video_pkt_tx` /
  `audio_pkt_tx` (各 bounded=64) に enqueue する。
- `video-decode` thread (= `run_video_decode`): 動画 decode + GPU blit / swscale +
  pacing。Phase 8.K の pacing logic を **そのまま移植**。
- `video-audio-decode` thread (= `run_audio_decode`): 音声 decode + resample +
  post-seek trim + EOF drain (= 末尾サンプル出し切り)。

**新 channel メッセージ型**:

```rust
enum VideoWorkerMsg { Packet(Packet), Flush { serial, target_secs }, Eof }
enum AudioWorkerMsg { Packet(Packet), Flush { serial, target_secs }, Eof }
```

順序保証 channel に enqueue するため、Flush と pre/post-seek packet の到着順が
逆転しない。`target_secs.is_some()` (= seek 成功) で受信側は post-seek preroll trim、
`None` (= seek 失敗) で trim なし通常 pacing に戻す。

**SeekCompleted の発火点**: 旧構造と同じく demux thread が `clock.notify_seek_completed`
+ `engine_event_tx::SeekCompleted` を発火。post-seek 1 枚目の表示 (= UI tick で seek
override clear) は video decode thread が `post_seek_frame_sent=false` で待機 →
1 枚送出 → true、の流れ。

**HwDevice の Send**: `unsafe impl Send for HwDevice {}` を追加 (AVBufferRef refcount は
thread-safe)。SW 再試行時は `_hw_device = None` で None 状態に置き換える (旧構造の
`drop(_hw_device)` を変更)。

**Drop / shutdown 順**: VideoPlayer drop → cancel.store → demux thread が break →
関数末尾で `audio_pkt_tx` / `video_pkt_tx` を drop → 各 decode thread が channel
disconnect で recv() 抜け → exit → demux が `audio → video` の順で `join()`。

**期待される効果**:
- audio_tx 満杯時: demux が `audio_pkt_tx.send` で逆圧を受けるだけで video decode は
  止まらない → `buf 0/24` 振動の解消。
- video decode 重い時 (4K HEVC SW): demux は別 thread で steady に packet を流し、
  音声 packet も滞らない。

詳細な thread 境界設計は [docs/video-architecture.md](video-architecture.md) の
`decoder.rs` (3-thread 構成) 節を、各 thread の責務と shared resource は
[docs/async-architecture.md](async-architecture.md) の thread 表を参照。

### Phase 9 シリーズの追加修正 (Phase 9.A〜9.G + Codex P2、2026-04-30)

3-thread 分離後、ユーザー実機テストで判明した問題への対症療法を順次反映。

**Phase 9.A: `set_audio_pts` の wall-rate cap (`clock.rs`)**

動画 open 直後 (~3 秒) に cpal 出力 callback `fill_output` が OS 音声バッファを
pre-fill するために短時間で複数回 fire し、各 callback が `next_pts_secs += want /
samples_per_sec` で期待 wall 進行量分の pts を加算するため、anchor の pts が wall
時刻の **2.5x 速で前進** する事象を観測 (perf-log で pace/wall=2.55x@0-3s 窓)。
結果として `now_secs()` (= pace_now) が wall より 2.5x 速く進み、decoder pacing が
「先読み不足」と誤判定して 2.5x レートで生産しようとし、HW デコード上限を超えて
future_frames が枯渇 (= buf:0/24 振動)。

修正: `set_audio_pts` で前回 anchor が Audio source なら、wall 経過量 + 5ms ジッタ
余裕を上限に pts 進行を cap する。Audio 起動直後 / fallback 直後 / seek 直後
(= 前回が Wall/Frozen) は cap せず絶対値を尊重 (seek target を尊重)。

**Phase 9.B/9.E: cpal warmup silence の範囲 (`audio.rs`)**

Phase 9.A だけでは driver 側の pre-fill burst を完全には抑えられないため、`fill_output`
で engine_state が **LOADING/IDLE のとき silence + 早期 return** で `next_pts_secs`
更新自体を skip。Phase 9.B 初版は `engine ≠ Playing` の全期間 silence にしていたが、
forward seek の Buffering 期間で deadlock を起こしたため (audio_pkt_tx 満杯 → demux
block で seek take_request されない)、Phase 9.E で **LOADING/IDLE のみ** に縮小。
Buffering / Seeking / Paused / Eof は他の経路 (`!clock.is_playing()` 早期 return @
audio.rs:321) で silence される。

**Phase 9.C/9.D: pause-park engine state + Buffering 中 lookahead 許可 (`decoder.rs`)**

旧 Phase 9.C は decoder pacing loop の park 条件を `!clock.is_playing()` にしたが、
動画 open 直後 (autoplay=false で Loading 状態) も is_playing()=false で park され、
post-seek frame が生成されず「動画を準備中…」のまま停止する regression が発生。

Phase 9.D で park 条件を `engine_state in [PAUSED, EOF]` に変更し、Loading /
Buffering / Seeking 中は pacing logic に進ませる。さらに `allow_pace_lead =
engine_playing || engine_st == BUFFERING` で **Buffering 中も PACE_LEAD=0.30s の
lookahead を許可** (= 60fps で 18 frames 先読み)。これにより Buffering→Playing
遷移時には buffer がほぼ満杯で、UI 消費追従の frame batching が起きない。

**Phase 9.E: post-seek 1 枚目の unconditional 送出 (`decoder.rs`)**

forward seek (`+10s` 等) の deadlock 修正。post-seek 1 枚目までは
`audio_buf < AUDIO_SAFE_HI` の seek_burst 条件で gate していたが、Phase 9.B の
warmup silence と組み合わさって audio buffer がいつまでも満杯にならず 1 枚目が
送出されない循環に陥っていた。修正: `clock.is_seeking() && !post_seek_frame_sent`
の場合は無条件で 1 枚送出 (lead cap や audio buffer 状態と関係なく)。

**Phase 9.F: forward seek の A/V desync 修正 (`decoder.rs`)**

`input.seek(target..)` (前方シーク) は video keyframe にスナップして target より
未来に着地するが、mp4 muxing の都合で audio packet が video keyframe より前に
配置されることがある。結果として post-seek の anchor が `audio_pts < video_pts` で
始まり、pace_now が video より遅れて 0.87s stall。

修正: シーク方向に関係なく **常に backward+preroll** を使う (`av_seek_frame +
AVSEEK_FLAG_BACKWARD`)。post-seek の `drop_before_secs` で target より前のフレームを
trim し、target の最初のフレームから再生開始。`direction` 引数は互換のため残すが
実動作では参照しない。

**Phase 9.G: perf overlay graph freeze during pause AND seek (`ui_fullscreen.rs`)**

Phase 9.F まで適用後、ユーザー報告で「seek 直後に黒い空間が出てから赤線で再開する」
事象が判明。原因: perf overlay graph の "now" tick が wall 時刻ベースで進む一方、
seek 処理中 (= override 設定中、UI が post-seek 1 枚目を受け取る前) はサンプル新規
追加が止まり、graph が「データなし区間」として黒く描画されていた。

修正: `VideoPlayer::is_seeking()` を追加し、`is_paused_or_seeking() = engine.PAUSED ||
clock.is_seeking()` を新たな freeze 条件として使う。`sample_video_perf` は freeze 中
サンプル追加を skip + history を pause_dur 分シフト、`draw_video_perf_overlay` は
freeze 中 graph の "now" を最後のサンプル時刻に固定。pre-seek の折れ線が post-seek
の赤線エリアに滑らかに繋がる。

2026-05-03 follow-up: the perf overlay now records `expected_misses` for
displayed-frame intervals that exceed the source FPS cadence. This catches
slow-playback/stutter cases where no decoder channel overflow (`dropped_full`)
and no UI `dropped_past` event occurs, but the presenter still fails to show the
number of frames implied by the nominal FPS. The overlay shows these as `miss:N`
in the header and as thick red vertical bars in the interval graph. The same
condition is emitted to perf JSON as `video/display_miss` with `interval_ms`,
`expected_ms`, `expected_misses`, decoder/UI skip deltas, and the current
render-queue length. GPU/D3D11VA playback uses the same `video/tick`
diagnostics as the CPU path, so UI-side batching is visible in logs on both
paths. While a video is actively playing, the app also clamps the next
`request_repaint_after` delay to at most 16ms. This keeps the fullscreen UI
waking at roughly display cadence even for 24fps sources, avoiding timer
oversleep that previously let one UI tick consume several ready frames at once.

**Codex P2: EOF replay seek の engine epoch 二重 ++ (`mod.rs`)**

`apply_command(Play)` を `handle_seek_request` より先に呼ぶと、state=Eof のとき
`handle_play()` が内部で `handle_seek_request(0.0)` を呼んで epoch++ し、続く明示
`handle_seek_request` で **epoch が二重 ++** されていた。decoder からの
`SeekCompleted { epoch: serial }` (= AvClock seek_serial、+1 のみ) と engine の
`current_seek_epoch` (+2) がズレ、stale 判定で捨てられて engine が Seeking から抜け
ない可能性があった。

修正: 呼び出し順を **handle_seek_request → apply_command(Play)** に逆転。
handle_seek_request 後は state=Seeking なので、続く handle_play は Seeking arm の
`autoplay = true` 設定だけ走り epoch は ++ しない。該当箇所は `toggle_play` EOF
replay / `seek` / `seek_relative` / loop replay の 4 site。

**Codex P? : decoder pause-park の seek_serial check 順序 (`decoder.rs`)**

GPU/CPU 両 pacing loop で、PAUSED/EOF park の 50ms sleep が `seek_serial` チェックより
先にあった。pause 状態で seek 要求が来たケース (現状 UI に経路はないが将来 pause 維持
seek を入れる際に effective) で 50ms sleep を待たないと新世代を検出しない。

修正: `seek_serial` check を park sleep より先に移動。

### Phase 9 後の Post-cleanup refactor (counter consolidation + fill_output bookkeeping)

Phase 9.A〜9.G + Codex P2 で対症療法ベースに修正したものを、構造的にクリーンアップする
2 件の refactor を実施 (commit `10fd50f` / `9eff9b5` / `4bf7d58` / `07f55be`)。

#### 1. Counter consolidation (`AvClock.seek_serial` と `EngineActor.current_seek_epoch`)

**旧設計**: 2 個の seek 世代カウンタを「両方を bump する」規律で同期。Codex P2 で
規律違反 (= 二重 ++) バグが見つかった経緯 (上記節を参照)。

**新設計**: `Arc<AtomicU64>` を 1 個共有 + adaptive `handle_seek_request`:
- `AvClock` と `EngineActor` で同じ `Arc<AtomicU64>` を保持
- `EngineActor::handle_seek_request` は **adaptive** に動作:
  - 観測した counter > `last_observed_serial` → 外部経路 (= caller が clock.request_seek
    で既に bump 済) → 自身は bump せず state 更新のみ
  - 観測した counter == `last_observed_serial` → 内部経路 (= loop replay / EOF replay /
    resume) → `av_clock.request_seek` 経由で bump + SeekRequest publish + state 更新
- `EngineActor` に `Arc<AvClock>` を持たせて内部経路の publish 経路を確保

**caller pattern (mod.rs::seek 系)**: 旧と同じ `clock.request_seek` → `engine.handle_seek_request`
の順。adaptive ロジックが規律違反を構造的に吸収するので、Codex P2 タイプのバグは再発し得ない。

**追加テスト 2 件** (Codex review 提案):
- `external_clock_bump_then_engine_handle_does_not_bump_again`: 外部経路で counter が
  +1 のみであることを直接確認
- `internal_engine_seek_publishes_seek_request_via_av_clock`: 内部経路で counter +1 +
  AvClock.take_seek_request() で SeekRequest が decoder に届くことを確認

#### 2. fill_output bookkeeping 上流移動 + 9.B/9.E silence gate 撤去

**旧問題**: `fill_output` が cpal callback ごとに無条件で `next_pts_secs += want / samples_per_sec`
を加算していた (= 「常に full 期間進める」)。silence 出力中も進むので pre-fill burst
で anchor pts が wall の 2.5x 速で前進。

**旧対症療法 (= 2 段防御)**:
- Phase 9.A: `set_audio_pts` の wall-rate cap (= 後段で異常前進を頭打ち)
- Phase 9.B/9.E: LOADING/IDLE 中 silence + `next_pts_secs` 進行 skip

**新設計**: `fill_output` のドレインループで **実消費サンプル数** を `real_consumed`
として計数し、bookkeeping は `real_consumed` 分のみ進める:

```rust
let mut real_consumed: usize = 0;
while written < want {
    match buf.samples.pop_front() {
        Some(s) => { ...; real_consumed += 1; }
        None => { /* silence rest */ break; }
    }
}
if real_consumed == 0 { publish_buffer_secs(&buf, clock); return; }
let consumed_secs = real_consumed as f64 / buf.samples_per_sec;
buf.next_pts_secs += consumed_secs;
// ... set_audio_pts
```

**撤去**:
- Phase 9.B/9.E LOADING/IDLE silence gate (= 不要)
- `audio.rs::start` / `fill_output` の `engine_state: Arc<AtomicU8>` 引数 (= silence gate
  のためだけに渡していた)

**保持** (Codex review P? 反映):
- Phase 9.A wall-rate cap (`clock.rs::set_audio_pts`) は **defensive safety net**
  として復活保持。Codex の指摘「`real_consumed` は callback で渡した量であり、
  hardware が wall 時間どおりに再生済みの量ではない。buffer 非空での pre-fill burst
  では callback 連続 pop が wall 進行を超える可能性が残る」を踏まえ、bookkeeping
  上流化と cap 復活を組み合わせた belt-and-suspenders 構成。
- 通常動作では `pts_secs - prev.pts_secs ≈ wall_dt` で cap 無効、異常系のみ発動。
- 実機 perf-log smoke で「pace/wall ≈ 1.0 (= cap 無発動)」を確認できた段階で、
  次のリファクタ機会に cap 撤去を再検討。

**追加テスト 3 件** (Codex review 提案 + 回帰用):
- `fill_output_empty_buffer_does_not_advance_pts`: 完全 underrun で pts 進行 0
- `fill_output_partial_drain_advances_only_real_consumed`: 部分 drain で実消費分のみ
- `fill_output_full_drain_advances_full_amount`: 完全 drain で旧版と同じ全消費量

## 検証

### Phase 2 完了時 (resume 未修正)
- 既存挙動と diff なしを確認 (= 旧 perf log と新 perf log を比較)

### Phase 3 完了時 (resume 修正)
- **resume seek を含む再生開始の最初 1 秒**で、`pace_now - wall_offset` の比が 1.0 ±
  10% に収まること
- video.decode の events/100ms が wall 全期間で安定 (60fps 動画なら 6 ± 1 events/100ms)
- 倍速 (1.5x / 0.5x) でもまんべんなく安定
- フルHD/4K/30fps/60fps の 4 組合せ × resume あり/なし の合計 8 ケース手動回帰

### Phase 4 完了時 (= 軌道修正後の最終形)
- `cargo test` 全通過 (テスト数は Phase 3 から増減しない)
- `cargo build --release` 成功
- `clock.rs` のモジュール doc-comment が「AvClock は互換 facade である」旨を明示
- `docs/video-architecture.md` の `clock.rs` 節が新構造 (= engine/ 委譲) を反映
- AvClock を完全削除する場合の段階移行計画が本ドキュメントに記録されている

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

## Phase 5 以降 (UI / UX 改善ロードマップ)

エンジン側 (Phase 1–4) で再生 / シーク / pacing が安定したので、フルスクリーン動画再生の
UX を整える。各項目は独立に着手可能。実装順は本節の番号順を想定 (依存関係順) だが、
ユーザー優先度に応じて入れ替え可。

### 5.1 既定挙動: 開いたら一時停止 + Enter で再生 + Space は選択

**現状**: フルスクリーンで動画を開くと autoplay (`OpenOptions.autoplay = true`) で
即座に再生開始する。Space キーで再生/一時停止トグル。

**問題**:
- 画像と動画が混在するフォルダで連続表示すると、動画だけ突然音が出る・誤クリックで
  即再生開始してしまう体験になる。
- Space キーは画像ビュアの「選択」と被っており、画像/動画混在選択時に挙動が文脈依存
  でブレる。

**変更**:
- `OpenOptions.autoplay` のデフォルトを `false` (= 一時停止状態で開く) に変える。
  最初のフレーム表示後は `EngineState::Paused` に留まる。
- 環境設定の動画 autoplay 既定値も「自動再生しない」に変更 (旧設定を持つユーザーは
  そのまま保持。新規ユーザーが false で始まる)。
- フルスクリーン動画表示中の **Enter キー** で再生開始 / トグル。Shift+Enter で
  外部プレイヤー起動 (既存機能の再アサイン)。
- **Space キーは画像と同じ「選択」アサイン**に固定する (動画でも選択可能)。
- 一時停止中はホバーバー直下の中央に「[Enter] 再生 / [Shift]+[Enter] 外部プレイヤー」
  ヒントオーバーレイを薄く表示 (3 秒で fade)。

**実装ポイント**:
- `OpenOptions::default()` の `autoplay: true` → `false`
- `Settings` の動画 autoplay 既定値変更 + migration (既存ユーザーの設定値はそのまま)
- `ui_fullscreen.rs` の動画キーバインド差し替え: Space → 選択トグル、Enter → 再生トグル
- ヒントオーバーレイは Paused 状態でのみ描画

### 5.2 シークバーサムネイル: 2x サイズ + 時刻ラベル位置変更

**現状**: シークバー hover 時にサムネ + mm:ss を重ねて表示。サムネは小さい。

**変更**:
- サムネサイズを縦横 2x (現行 ~120x68 → ~240x136 程度)。
- mm:ss 表示はサムネ上に重ねず、**サムネの直下** に独立行として配置。背景は
  半透明黒で読みやすく。

**実装ポイント**:
- `ui_fullscreen.rs` のシークバー hover 描画ブロック (thumbnail + label) のレイアウトを
  「サムネ + 直下に label」の縦並びに変更。
- サムネ抽出は既存の `ThumbnailWorker`。出力解像度を 2x に上げる (= スケール時に
  upscale でぼやけないよう、ワーカー側のリクエスト解像度自体を上げる)。

### 5.3 動画サムネイルの優先順位

**現状**: 動画ファイルのグリッドサムネは `IShellItemImageFactory` 経由で
Windows 標準サムネをそのまま使う。

**変更**: 以下の優先順位で決定する。
1. **ユーザーがピン留めした位置のフレーム** (= Phase 5.4 で UI を提供するピン機能の
   出力。詳細は 5.4「ピン / ブックマークの authoring UI」節を参照)
2. **同一ファイル名の画像** (環境設定で「同一ファイル名画像をサムネに使う」が ON のとき)
   例: `movie.mp4` の隣に `movie.jpg` があれば後者をサムネに採用。拡張子優先順は
   既存のサムネカタログと整合させる。
3. **動画自体のデフォルトサムネ** (Windows Shell 経由、現状動作)

**実装ポイント**:
- `video_thumb.rs` の thumbnail 取得ロジックを上記 fallback chain に変更。
- サムネカタログ DB のキーは「動画ファイルパス」のまま。生成時に上記順で source を
  選び、source の path/mtime を一緒に記録 (sidecar 画像の更新検知)。
- ピン留め情報は `rotation_db.rs` と同じ場所の SQLite (新テーブル `video_pins`) に保存:
  `(video_path, pin_pts_secs, thumb_webp)`。

### 5.4 埋め込みメタ情報の取り込み (チャプター等)

**前提**: 動画ファイルに埋め込まれた標準メタデータ (Matroska tags / MP4 udta /
FFmpeg avformat が解釈できる ffmetadata 形式) を読み取り、UI に反映する。
**特定の外部ダウンローダ名は文書に出さない方針** (CLAUDE.md「動画メタ情報の扱いと
外部ダウンローダの言及禁止ポリシー」を参照)。

**右パネル (= メタデータ) の表示**:
- 現状画像で出している EXIF パネルと同じ位置に、動画では:
  - タイトル / 作者 / アルバム / 説明 (avformat の global metadata)
  - チャプター一覧 (start_pts, end_pts, title)
  - 解像度 / fps / duration / コーデック / 音声トラック数 (既に拾える)

**左パネル (= ナビゲーション)**:
画像と動画で **左右パネルの役割を入れ替える**。動画の左パネルは「ジャンプ可能位置の
サムネ縦並び」とし、上から:
1. ユーザーがピン留めした位置 (1 個まで、`📌` アイコン)
2. ユーザーが 🔖 でブックマークした位置 (任意個数)
3. 埋め込みメタデータのチャプター開始位置

各サムネ右側に mm:ss 表示。クリックでその位置に seek。

**実装ポイント**:
- `decoder.rs` 開始時に `AVFormatContext->metadata` + `AVChapter*` 配列を読んで
  `VideoInfo` に追加フィールド `chapters: Vec<Chapter>` / `title: Option<String>` 等。
- `Chapter { start_secs: f64, end_secs: f64, title: String }`。
- ブックマーク・ピンは Phase 5.3 と同じ DB (`video_pins` / `video_bookmarks`)。
- `ui_fullscreen.rs` の左右パネル切り替え分岐に動画モードを追加。

#### 5.4.1 ピン / ブックマーク の authoring UI (Phase 5.3/5.4 共通の前提)

5.3 のサムネ優先順位 1 と 5.4 の左パネル 1/2 行目を成立させるには、ユーザーが
**ピンとブックマークを作成・更新・削除できる UI** が必要。仕様:

- **ピン (📌)**: 動画再生中の現在位置をグリッドサムネとして固定する。1 動画につき
  最大 1 個。
  - 作成: フルスクリーン動画再生中のホバー上ツールバー (Phase 5.6) のピンボタン、
    あるいはコンテキストメニュー「現在のフレームをサムネに固定」。
  - 更新: 既にピンがある動画でもう一度押すと現在フレームで上書き。
  - 削除: 同じボタンの長押しまたはコンテキストメニュー「ピンを削除」。
  - 画像本体: ピンを作った時点でフレームを抽出し、`video_pins.thumb_webp`
    に WebP で格納 (= 後でピン位置までシークしなくても表示できる)。
- **ブックマーク (🔖)**: 任意の位置を任意個数記録。タイトル省略可。
  - 作成: フルスクリーン動画再生中の B キー (新規) で「現在位置をブックマーク」、
    または左パネルのプラスボタン。
  - 削除: 左パネルの各ブックマーク行の右側 × ボタン。
  - 順序: 時刻昇順 (= 自動ソート、ユーザー並び替え不要)。

DB スキーマは `video_pins (video_path, pin_pts_secs, thumb_webp BLOB)` と
`video_bookmarks (video_path, pts_secs, title, thumb_webp BLOB)` の 2 テーブル。
両方 `rotation_db.rs` と同じ SQLite ファイル (= `%APPDATA%/mimageviewer/rotations.db` 等
の現行 DB) に格納する想定。

### 5.5 タイル サムネイル一覧 + クリック シーク (S キー)

**変更**: フルスクリーン動画中に **S キー** でタイルモードに切替。

**キーアサインの優先順位**:
- 現状の `S` キーは画像フルスクリーンでスライドショー トグル ([src/ui_fullscreen.rs](../src/ui_fullscreen.rs))
  に割り当てられている。動画モードでは S をタイルモードに **再アサイン** し、
  画像モードのスライドショー アサインは温存する (= 文脈別に同キーを別機能に振る)。
- もし将来「動画でもスライドショー (= 自動次動画再生) を S にしたい」要件が出たら、
  動画タイルモードを別キー (例: T) に動かす。本ロードマップ着手時にあらためて検討する。

**仕様**:
- 1/2/5/10/20/30 秒、1/2/5/10/20/30 分の **時間間隔候補** から、画面に切れずに
  最大数並ぶ最大の間隔を自動選択。
- 基本横 10 列。動画のアスペクト比をサムネに反映 (16:9 動画なら横長サムネ)。
- 各サムネをクリックするとその時刻に seek してタイルモード解除 + 再生。
- 1 度生成したタイル一覧は VideoPlayer 生存中はメモリにキャッシュ (= 動画切替で破棄)。
  キャッシュは時間間隔ごとに別 (= 同じ動画で違う間隔を選ぶと次回は再生成不要)。

**実装ポイント**:
- 既存 `ThumbnailWorker` を流用してバッチ抽出。一括 seek + decode を decoder thread
  外で行うため、専用のバックグラウンド actor (= 軽量な短命スレッド) を用意し、
  抽出中は通常再生 pacing と競合しないように分離する。
- タイル UI は eframe の `egui::Grid` + クリックハンドラ。
- キャッシュキー: `(video_path, interval_secs, tile_w_px)`。VideoPlayer 内 `HashMap`。
- ESC で抜ける。再表示時はキャッシュ ヒットなら即時、ミスなら抽出進捗バー表示。

### 5.6 ホバー上ツールバーの整理

**現状**: 画像と共通の上ホバーバーに各種ボタン (回転 / 補正 / プリセット / etc.) が
並ぶが、動画では多くが意味を持たない。

**変更**: 動画モード時のホバー上ツールバーを最小化:
- (新規) **タイル サムネイル ボタン** (S キー相当)
- 必要に応じて 外部プレイヤー / 設定 など最低限のアイコン

詳細レイアウトは実装時に再検討。Phase 5.5 ボタンの追加が最優先。

### 5.7 (Phase 4 の追加スコープ — エンジン側で先行実装が望ましい)

Phase 5.1–5.6 のいずれも:
- `EngineActor` の `apply_command(TransportCommand::Pause)` の信頼性
- `OpenOptions.autoplay = false` で開いたときの遷移経路:
  - `Loading` → `Buffering` で **READY (= `FirstFrameReady ∧ (NoAudio ∨ BufferReady)`)** が
    完成するまでは従来通り (= 既存 latch 不変条件を維持)。
  - READY 完成時に `try_transition_from_buffering` が `autoplay=false` を尊重して
    `Playing` ではなく `Paused` に遷移する。
  - Paused に入った後はじめて video frame が UI に表示される (= Buffering 期間中の
    fr ame は依然非表示で、latch invariant を崩さない)。

を前提にする。Phase 4 完了時点で `try_transition_from_buffering` が `autoplay=false`
を尊重して Paused に遷移するパスは既に存在する (Phase 3a で実装済) ので、5.1 の
切り替えは小さい追加で達成できる見込み。

### 5.8 ツールバー: 前のフォルダ / 次のフォルダ ボタン (動画とは独立、最後に着手)

動画機能ではないがロードマップ末尾で追加する: ツールバーには「上のフォルダ」ボタンが
あるが、**前のフォルダ / 次のフォルダ** に飛ぶボタンが無い。Ctrl+↑↓ のキーボード
ショートカットは既存だが、マウス操作では辿れない。

**変更**:
- ツールバーの「上のフォルダ」ボタンの隣に「前のフォルダ」「次のフォルダ」ボタンを
  追加。アイコンは上下矢印になるが、「上のフォルダ」(= 親方向、フォルダアイコン付き
  の上矢印など) と区別できるデザインにする (例: シンプルな上下三角 `▲▼`、または
  両端矢印など)。実装時に複数案を試して見栄え良いものを採用。
- **Ctrl+←→ は使わない**。見開き表示中はページ進行方向 (RTL/LTR) で「次へ」の意味が
  変わるため、左右キーの空間意味が文脈依存になる。フォルダ移動は上下方向に固定する
  (= 既存 Ctrl+↑↓ ショートカットと整合)。
- **ツールバーボタンの取捨選択**: 環境設定の「ツールバー」ページで、表示する
  ボタンを個別にチェックボックスで ON/OFF できるようにする。新規追加の前後フォルダ
  ボタンも候補に含める。既存のツールバーカスタマイズ機構 (`ui_dialogs/toolbar_settings.rs`)
  を拡張する形で実装する想定。

**実装ポイント**:
- フォルダ前後移動のロジックは Ctrl+↑↓ ショートカット側に既にあるので、ボタン
  ハンドラから同じ関数を叩く。
- アイコンは `egui::IconName` の標準セットに無ければ `RichText` の文字 (▲ / ▼ /
  ↑ / ↓ / ⇧ / ⇩) を組合せる。確定はモック描画で見比べる。
- 設定キー追加: `Settings::toolbar_buttons` のような構造を拡張、既存 default は
  互換維持 (= 新規ユーザーは前後フォルダ ON、既存ユーザーは migration で同じく ON
  にしておく)。

**着手順序**: Phase 5.1 〜 5.7 の動画関連を全て完了してから本タスクに入る。
動画と無関係なので独立してレビュー可能。

## Phase 9.H: AVI/DivX timestamp compatibility (2026-05-03)

Some older AVI files produced by Nandub / DivX-style encoders expose MPEG-4
Part 2 video packets with missing PTS (`AV_NOPTS_VALUE`) and only DTS. They may
also set `divx_packed=true`. VLC tolerates this by deriving presentation timing
from FFmpeg's best-effort timestamps and by recovering when audio decode runs
far ahead during seek preroll.

mIV now does the same two defensive things:

- `src/video/decoder.rs` reads `AVFrame.best_effort_timestamp` before falling
  back to `frame.pts()`, for both video and audio frames. Audio packet preroll
  trim also falls back from packet PTS to DTS.
- `src/video/audio.rs` no longer treats `raw_pending` growth as a reason to
  discard earlier audio and re-anchor at the newest decoded frame. Instead, when
  the pre-VST queue reaches the back-pressure threshold, the pump temporarily
  stops reading `audio_rx`. The bounded decoder/pump and demux/audio queues then
  slow demux naturally while preserving the audio order from the seek target.
  The threshold is state-sensitive: playback uses a tight steady-state cap,
  while Loading/Seeking/Buffering allow a larger preroll window so audio
  back-pressure does not block the single demux thread before post-seek video
  packets arrive.

2026-05-03 follow-up: an earlier 2s seek-local soft cap was removed after real
seek-to-start testing. The cap re-anchored at the newest decoded audio frame
once `raw_pending` crossed 2s, which skipped the first ~2s of audio after W-key
seek-to-start and caused audible A/V offset. The WMA/WMV slow-clock issue that
motivated the soft cap is now handled by the synthesized monotonic audio PTS
cursor (Phase 9.J) and the callback-rate wall cap (Phase 9.K). 2026-05-04
follow-up: the remaining 30s overflow re-anchor was also removed after AV1 60fps
testing showed it could be reached during normal demux catch-up, causing W-key
seek-to-start to jump to 30s/60s/90s audio chunks and then silence. Audio now
uses bounded-channel back-pressure rather than destructive recovery.

## Phase 9.I: WMV/ASF frame-rate metadata fallback (2026-05-03)

Some ASF/WMV files report `avg_frame_rate=0/0` while still carrying a usable
stream rate such as `24/1`.

mIV handles that compatibility corner case by deriving display/VPP frame-rate
metadata from `avg_frame_rate` first and falling back to `stream.rate()` when
the average is missing. This avoids `fps=0/0` diagnostics for old WMV/ASF
files.

WMV3/VC-1 continue to follow the user's global hardware decode setting. A
2026-05-03 investigation showed the affected WMV3 sample still stuttered with
software decode (`decode_path=sw`, `hw_effective=false`), so the remaining
issue is tracked as audio/VST raw backlog after seek rather than a D3D11VA
decoder problem.

2026-05-03 follow-up: ASF/WMV files with WMA Pro 5.1ch audio can be bottlenecked
by the stereo-only output path: FFmpeg decodes six planar float channels and
then swresample downmixes to packed stereo for cpal/VST. Because mIV always
outputs stereo, multichannel audio decoders now receive a best-effort
`request_ch_layout=stereo` before `avcodec_open2`. If the decoder honors it,
swresample only has to do the remaining format/rate conversion. The open perf
event now distinguishes output fields (`audio_rate`, `audio_channels`) from
decoder input fields (`audio_input_rate`, `audio_input_channels`,
`audio_input_layout`, `audio_input_format`) and records whether the stereo
request was sent/effective.

The FFmpeg 7.1 build used by the Windows bundle does not expose that decoder
request for WMA Pro (`Option not found`), so mIV also has a narrow fast path for
multichannel input at the output sample rate: f32/s32/s16, planar or packed, is
folded down directly to packed stereo in Rust using FFmpeg's channel-position
metadata. That bypasses swresample's multichannel-to-stereo matrix path while
preserving the stereo-only cpal/VST contract.
The perf log records per-audio-frame diagnostics (`path`, `decode_wait_ms`,
`convert_ms`, `send_wait_ms`, `total_ms`) so WMA Pro and similar files can be
split into decoder, downmix/resample, and queue-backpressure costs.

## Phase 9.J: Synthesized monotonic audio PTS for broken decoder output (2026-05-03)

Some ASF/WMA Pro streams emit decoded audio frames whose raw PTS repeatedly
falls back to `0` between correctly timestamped packet-leading frames. In the
observed WMA Pro 5.1 sample, most decoded audio frames needed synthetic PTS;
without that, the audio master clock repeatedly re-anchored backward and video
pacing collapsed to roughly 0.4x real time even though decode/downmix work was
fast.

The audio decode worker now keeps a monotonic synthetic PTS cursor per seek
generation. It still trusts valid raw timestamps, but when a decoded frame
timestamp would move backward it assigns the next cursor timestamp and advances
the cursor by the frame duration. This is intentionally generic, not WMA Pro
specific, so other decoders that intermittently drop frame timestamps can use
the same recovery path. Perf events expose both `raw_pts` and
`pts_synthesized` for verification.

Follow-up diagnostics for remaining visual hitch reports:

- `video/display_miss` records frames that are skipped at display time because
  the UI side is already late.
- GPU-backed video frames now go through the same `video/tick` diagnostics as
  CPU frames, including displayed-PTS deltas and dropped-past counts.
- During playback, the app caps its normal repaint wait to 16ms so the
  fullscreen video viewport keeps a frame-scale cadence even when other UI
  panels are idle.
- `ui/slow_frame_breakdown` is emitted for frames over 30ms and splits
  `App::update` into poll, keep-range, background polls, fullscreen work,
  root input, fullscreen viewport, menus/dialogs, toolbar/input, search bars,
  grid, and post-grid costs. Use this together with `video/display_miss` to
  distinguish decoder/pacing drops from UI-thread repaint stalls.

## Phase 9.K: Audio clock callback-rate cap follow-up (2026-05-03)

The Phase 9.A wall-rate cap originally allowed each continuous audio-clock
update to advance by `wall_dt + 5ms`. That fixed long wall-clock extrapolation
catch-up, but it also made the tolerance callback-rate dependent: on short
WASAPI/cpal periods the extra 5ms could be granted many times per second, so
the audio master clock advanced at roughly 1.3x-1.4x real time and video paced
to that faster clock.

The cap is now expressed as a small rate multiplier (`wall_dt * 1.02`) instead
of a fixed per-callback slack. This preserves a little scheduling tolerance
without letting short audio callbacks accumulate extra clock time.

## Phase 9.L: 60fps video pacing sleep granularity (2026-05-04)

AV1 60fps playback exposed a demux back-pressure pattern where `video_pkt_tx`
stayed full and the demux thread blocked while sending video packets. Because
demux is the single source for both audio and video packets, video back-pressure
also starved audio packets and produced visible frame misses plus audio gaps.

The video decoder pacing loop now uses a 1ms sleep for short "still ahead of
clock" waits instead of the previous 5ms sleep. At 60fps, one frame is only
16.7ms, so a 5ms sleep can overshoot a large fraction of a frame and reduce the
effective compressed-packet consumption cadence.

2026-05-04 follow-up: 1ms pacing was not enough for some AV1 60fps files. The
video packet channel still stayed full, so the demux thread blocked on video
send and starved audio packets. The demuxer now has a bounded compressed-video
overflow queue (64MiB) in front of `video_pkt_tx`. When the video packet channel
is full, demux stores video packets in that local compressed queue and continues
reading later audio packets from the same `AVFormatContext`; once video decode
catches up, the queued video packets are drained back to `video_pkt_tx` in
order. If the overflow queue reaches its bound, demux falls back to blocking on
oldest queued video packets to avoid unbounded memory growth.

Queued video and audio packets carry the seek serial. A seek clears the
demux-side video overflow queue before sending `Flush`, and each decode thread
drops packets whose serial no longer matches the local serial most recently
established by `Flush`. The first implementation only compared against that
local serial to preserve channel ordering during normal playback and seek
transitions. Later shallow-channel fixes below extend this to the live clock
serial as well, because stale compressed packets can otherwise keep a decoder
busy while audio has already moved to the new timeline.

2026-05-04 follow-up: audio must treat the clock's live seek serial as
authoritative. Audio is the master clock, so if a seek happens while old audio
packets are still queued ahead of the ordered `Flush`, playing those old packets
advances the clock at the wrong timeline position. The audio worker now drops
packet/frame output whenever `clock.current_seek_serial()` has already advanced
beyond the packet/decoder serial, and `audio_tx` sends use short timeout slices
so a blocked old frame can be abandoned promptly when a seek arrives.

2026-05-04 follow-up: a short-lived experiment reduced the direct audio/video
packet channels from 256 packets to 32 packets so `Flush` markers could reach
the decoders sooner. Real 60fps AV1 files showed the opposite failure mode:
32 packets is less than a second of compressed data, so `audio_pkt_tx` and
`video_pkt_tx` filled immediately, demux blocked, and fullscreen stayed in
"preparing" until a manual seek cleared the state. The direct queues are back to
256 packets for burst absorption. Stale cleanup is handled by serial checks
instead: audio and video both drop packets when the live seek serial has already
advanced, so old timeline data can be skipped without starving demux during
normal resume and 60fps playback.

2026-05-04 follow-up: for audio-active playback, video frames no longer clear a
seek override. High-rate files can have several seconds of decoded audio queued
behind the pump; clearing the override from the first target video frame starts
the visual clock before the first audible post-seek samples reach the output,
which causes AV drift after W-key seek-to-start. Video still clears the override
for video-only playback via the fallback anchor, but audio-bearing playback now
waits for `fill_output` to call `clear_seek_target_override` when it actually
publishes post-seek audio.

2026-05-04 follow-up: after sync was stable, remaining red perf-graph ticks were
mostly UI display misses rather than decoder drops (`decoder_skips=0` with a
full render buffer). For 60fps playback, `request_repaint_after(16ms)` can wake
one OS timer tick late and miss a vsync. The render queue now schedules wakeups
before the exact frame time by roughly half the source frame interval
(`0.5 / fps`, clamped to 4-20ms), and the app subtracts the time already spent
inside `poll_video` before issuing `request_repaint_after`. This keeps 60fps
content from losing repaint opportunities without forcing low-fps files into a
tight immediate-repaint loop.

2026-05-04 diagnostics follow-up: if red perf ticks remain after the repaint
prewake fix, `ui/frame_gap` now records stalls between `App::update` frame
boundaries with fullscreen/video/VST context, `ui/fs_viewport_breakdown` splits
the fullscreen viewport closure into input, media draw, HUD, panels, hover bar,
and VST manager costs, and `video_gpu/prepare` / `video_gpu/paint` records slow
GPU video callback import/fence/draw costs. These events are emitted only under
`--perf-log` and are thresholded, so they are intended for reproducing the
remaining UI-thread or GPU-present stalls without changing normal playback.

2026-05-04 diagnostics follow-up 2: `ui/fs_viewport_breakdown` now separates
the full `show_viewport_immediate` wall time into closure time, outer viewport
overhead, closure unaccounted time, and central-panel unaccounted time. This
distinguishes work done by mIV's fullscreen closure from egui/viewport/wgpu work
outside that closure. `video_gpu/prepare` and `video_gpu/paint` also emit a low
frequency sample even when fast, so perf logs can prove the GPU callback path is
active instead of only reporting slow frames.

2026-05-04 present-latency experiment: the eframe/wgpu surface keeps
`PresentMode::AutoNoVsync` as the default and requests
`desired_maximum_frame_latency=1`. A/B runs on 1080p60 and 1080p120 files show
that `mailbox` or `immediate` can change hitch distribution, but the improvement
is not reliable enough to treat presentation mode as a fix. Use
`MIV_WGPU_PRESENT_MODE=mailbox`, `auto_vsync`, `auto_no_vsync`, `immediate`,
`fifo`, or `fifo_relaxed`, and `MIV_WGPU_FRAME_LATENCY=default` or a positive
integer when comparing GPU/driver behavior.

2026-05-04 playback soak test mode: mIV now has a one-file playback automation
entry point for nightly regression checks. `--play-test <FILE>` opens the file
through the normal fullscreen video path, `--play-test-start <SECONDS>` forces a
deterministic start point (the soak harness defaults to 0s instead of saved
resume), `--play-duration <SECONDS>` exits the process after playback has run
for that long (default 30s), and `--play-muted` prevents audible output during
unattended runs. `--play-test-skip-vst3` disables VST3 only for that test
process so video playback can be benchmarked independently from plugin startup
and audio processing. `--perf-log <PATH>` or `--perf-log-path <PATH>` writes
JSONL events to a per-run path instead of the default `%APPDATA%` log, which
lets harnesses keep one log per video. The helper `scripts/video_soak.py`
recursively shuffles one or more folders, launches one mIV process per video,
summarizes `display_miss`, `frame_gap`, decoder drops, packet waits, and
completion status, and can run multiple environment-defined modes (for example
different wgpu present modes) against the same corpus.
