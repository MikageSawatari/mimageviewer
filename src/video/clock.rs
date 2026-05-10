//! 動画再生用 AV マスタークロック (= 互換 facade)。
//!
//! 音声 PTS をマスターにする一般的な実装 (mpv / ffplay 等と同じ)。
//! 音声スレッドは出力したサンプル列の PTS を [`AvClock::set_audio_pts`] で報告し、
//! 動画スレッドは [`AvClock::now_secs`] で「現在の理想的な再生位置」を取得する。
//! 動画フレームの PTS が `now_secs` より十分小さければドロップ、十分大きければ待機する。
//!
//! 一時停止・シークの状態もここに集約する。
//!
//! 大半のフィールドが atomic で Mutex 不要。f64 は `to_bits` / `from_bits` で
//! `AtomicU64` に格納する。シーク要求 (target / serial) はペアで
//! 整合性が必要なので [`Mutex<SeekRequest>`] で保護する。fill_output などの RT 経路は
//! 触らないので RT パフォーマンスに影響しない。
//!
//! # ⚠️ Phase 4 以降: 新規コードからは直接呼ばない
//!
//! Phase 2b 以降、`AvClock` は実装の大半を `engine::clock::MasterClock` (anchor 部分)
//! および `engine::audio_bookkeeping::AudioBookkeeping` (バッファ会計) に委譲した
//! **薄い facade**。残りの状態は所有関係が 2 種類に分かれている:
//!
//! - **EngineActor と並列管理されている互換複製** (`playing` / `audio_active` /
//!   `eof_reached` / `seek_request` / `seek_serial` / `seek_target_override`):
//!   `EngineActor` の `published_state` (`Arc<AtomicU8>`) + 内部 epoch が source of
//!   truth。新規コードはこれらを `EngineActor` 経由で読むこと。
//! - **AvClock 単独で source of truth を保持しているレガシー所有状態** (`volume` /
//!   `muted`): `TransportCommand::SetVolume / SetMuted` は `EngineActor` 側では
//!   no-op で、`audio.rs` が `clock.output_volume()` / `clock.pre_limiter_gain()` を
//!   直接読む構造になっている。
//!   将来 `EngineActor` (もしくは独立の `VolumeController`) に移すべきだが、
//!   Phase 4 時点では AvClock 所有のまま。
//!
//! AvClock を残しているのは [`decoder.rs`] / [`audio.rs`] / [`super::VideoPlayer`] の
//! 既存呼び出し点 (89 箇所) を壊さないための互換レイヤとして。**新規コードでは
//! [`super::engine::actor::EngineActor`] を直接叩く** こと:
//!
//! - 状態遷移: `apply_command(TransportCommand::*)` を経由
//! - シーク: `handle_seek_request(target_secs)` を経由
//! - decoder/audio からの events は `EngineEvent` channel に push
//!
//! 詳細は [docs/video-engine-redesign.md] の「Phase 4」節を参照。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::video::engine::audio_bookkeeping::AudioBookkeeping;
use crate::video::engine::clock::{ClockAnchor, ClockSource, MasterClock};

pub(crate) const MIN_PLAYBACK_SPEED: f64 = 0.25;
pub(crate) const MAX_PLAYBACK_SPEED: f64 = 4.0;
pub(crate) const PLAYBACK_SPEED_CHOICES: [f64; 11] =
    [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0];

pub(crate) fn clamp_playback_speed(speed: f64) -> f64 {
    if speed.is_finite() {
        speed.clamp(MIN_PLAYBACK_SPEED, MAX_PLAYBACK_SPEED)
    } else {
        1.0
    }
}

pub(crate) fn format_playback_speed(speed: f64) -> String {
    let speed = clamp_playback_speed(speed);
    if (speed.fract()).abs() < 1.0e-6 {
        format!("x{}", speed as i32)
    } else if ((speed * 10.0).fract()).abs() < 1.0e-6 {
        format!("x{speed:.1}")
    } else {
        format!("x{speed:.2}")
    }
}

/// シーク種別 (= 速度と精度のトレードオフ選択)。
///
/// **Fast** はユーザー要望 (= ←→キーの体感を速くしたい) に応える hybrid 設計の
/// 一部。シークバー / ブックマーク / 自動復元など「正確な位置」が要る経路は
/// **Precise** を使い、ホットキーによる相対シークだけ **Fast** を使う。
///
/// どちらも `av_seek_frame(AVSEEK_FLAG_BACKWARD)` で keyframe ≤ target に着地する
/// (= 共通基底)。違いは **video の preroll trim** を行うかどうか:
///
/// - **Precise**: video / audio 両方を target まで trim → target ぴったりに再生開始。
/// - **Fast**: **video は trim 無し** (= keyframe pts から即時再生)、**audio は
///   target まで trim** (= 通常 1x 再生)。Codex 2 巡目 P1 助言 (2026-05-01):
///   audio も trim 無しにすると、keyframe から物理再生する audio の `set_audio_pts`
///   monotonic guard が clock を target で凍結し、audio が target に追いつくまで
///   (= 数秒) clock が進まず video pacing も停止する (= 6-7 秒の動画フリーズ)。
///   Fast では video のみ trim を省略して即時 keyframe 再生し、audio は target から
///   スタートさせて target 直後から clock を 1x で進めることで freeze を回避する。
///
/// **target 情報は両モードで `Flush.seek_target_secs = Some(target)` で送る** (Codex
/// 1 巡目 P1 助言): trim 有無と target 情報を分離管理することで、Fast でも pump が
/// BufferReady の audio_anchor pts に target を反映でき、Buffering→Playing 入場時の
/// clock anchor が target に維持される (= timeline 表示が target 固定)。
///
/// **Fast モードの視覚的トレードオフ (= 設計の意図)**:
/// video が trim 無しで keyframe から再生されるため、最初の数 100ms 〜 数秒は GOP 1 個分
/// の pre-target 内容が見える。decoder が keyframe → target を burst 消費し UI tick が
/// その都度「最後の displayable」を表示するため、視覚的には早送り (~5x 〜 10x) で
/// target に追いつく形になる。**audio は target からスタート** するので音の頭出し
/// は target ぴったり。視覚 burst 完了後は audio / video / clock すべて target+wall で
/// 1x 同期する (= freeze なし)。←→ 連打の skim 用途で受け入れるトレードオフ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekKind {
    /// 正確な位置への seek (シークバー / ブックマーク / loop 再生 / EOF replay /
    /// 自動復元)。preroll decode + trim あり、target ぴったりに着地。
    Precise,
    /// 高速 seek (←→ キー)。preroll trim を省略、keyframe 即時再生。
    Fast,
}

/// シーク要求の整合性ある 1 件分。AvClock の `seek_request` Mutex が保護する。
/// `decoder` モジュールが `take_seek_request()` で取り出す。
#[derive(Clone, Copy, Debug)]
pub(super) struct SeekRequest {
    pub target_secs: f64,
    /// シーク世代。request 時点で `seek_serial` から取得・進めた値。
    pub serial: u64,
    /// シーク種別 (Precise / Fast)。decoder が preroll trim の有無を切替える。
    pub kind: SeekKind,
}

/// 動画再生用 AV マスタークロック (facade)。
///
/// Phase 2b: 内部状態は `MasterClock` (anchor 部分) と `AudioBookkeeping`
/// (pump/tx queued 会計) に分割。AvClock は旧 API を維持したまま委譲する。
/// 公開 API は変えていないので、`decoder.rs` / `audio.rs` / `VideoPlayer` から
/// 見える挙動は等価。Phase 4 (= 当初計画の AvClock 撤去) は規模を縮小し、
/// AvClock を **薄い互換 facade として残したまま** 状態の source of truth を
/// `EngineActor` に確立する方針に軌道修正された。詳細は
/// [docs/video-engine-redesign.md] の「Phase 4」節を参照。
pub struct AvClock {
    /// anchor 部分。`audio_pts` / `audio_recorded_at` / `fallback_pts` /
    /// `fallback_recorded_at` を 1 つの `ClockAnchor` (Audio/Wall/Frozen) に統合した。
    /// audio_active=true → Audio source、false → Wall source (= 旧 fallback path)。
    /// playing=false / is_seeking=true のときは内部で Frozen に置換される。
    /// 実体は `engine::clock::MasterClock` 側で、本フィールドは委譲先。
    master_clock: MasterClock,
    /// 再生中フラグ。false の間は `now_secs()` が進まない (= MasterClock を Frozen
    /// 風に扱う)。`EngineActor` の `published_state` PLAYING bit と並列に管理されている
    /// 互換用複製。新規読み手は EngineActor の `published_state` を見ること。
    playing: AtomicBool,
    /// pending なシーク要求。`None` = 未消費なし。
    /// take/request の両方が Mutex を取り、整合性のある (target, serial)
    /// ペアを観測できるようにする。
    seek_request: Mutex<Option<SeekRequest>>,
    /// 直近のシーク要求の世代。`request_seek` のたびに +1。
    /// 音声 RT コールバック (`fill_output`) と UI の `tick` がポーリングで読むので
    /// atomic で公開する。Mutex を取らずに「自分が処理中の世代より新しい seek が
    /// 走ったか」だけ見れる。
    ///
    /// `Arc<AtomicU64>` で `EngineActor` と **同一インスタンスを共有** する
    /// (= 旧版は AvClock と EngineActor がそれぞれ別カウンタを持ち、`mod.rs::seek`
    /// 系の caller が両方を bump する規律で同期していたが、規律違反で二重 ++ する
    /// バグ (Codex P2) があったため、構造的に共有化した)。
    seek_serial: Arc<AtomicU64>,
    /// **シーク中の表示位置 override** (秒、f64 bits、SEEK_NONE = 無効)。
    /// 設定中は `now_secs()` が audio_pts ではなく target を返し続け、UI のフリッカ
    /// (target → target+ε → target に戻る) を防ぐ。fill_output / UI tick が
    /// 「post-seek 後最初の target 近傍フレーム」を消費した時点で解除する。
    seek_target_override_bits: AtomicU64,
    /// override がセットされた時の seek_serial。clear 時にこれと現在の seek_serial を
    /// CAS 比較し、**新しいシーク要求が来ていたら clear をスキップ**する。これで
    /// 「古い fill_output コールバックが、新たに発生した override を誤クリアする」
    /// race を排除する。
    seek_override_serial: AtomicU64,
    /// 音声バッファ会計 (pump 残量 + tx queued)。
    /// Phase 2a で `AudioBookkeeping` に切り出し、AvClock は委譲のみ。動作は等価。
    audio_bookkeeping: AudioBookkeeping,
    /// 再生速度倍率。1.0 = 等速。MasterClock anchor の speed と同じ値を保持する。
    playback_speed_bits: AtomicU64,
    /// 再生速度変更は anchor と audio_tx 会計 epoch をまとめて更新するため直列化する。
    playback_speed_update_lock: Mutex<()>,
    /// audio_tx queued 会計の世代。偶数は安定状態、奇数は速度変更中。
    /// 速度変更で tx 会計をゼロ化し、旧 speed で enqueue 済みの frame が
    /// 新会計を壊さないようにする。
    audio_tx_accounting_epoch: AtomicU64,
    /// audio が「healthy」状態か。decoder が `notify_audio_active(true)` で開始、
    /// audio 出力起動失敗 / 音声ストリーム不在のとき false。
    /// false なら `now_secs()` はフォールバック wall clock を使う (= MasterClock の
    /// `ClockSource::Wall`)。
    audio_active: AtomicBool,
    /// decoder が EOF (= demux 末端) に到達したか。post-EOF seek を検出する。
    /// `notify_eof_reached` で立て、`request_seek` / `clear_eof_reached` で降ろす。
    eof_reached: AtomicBool,
    /// 音量 (0.0-1.0、f64 bits)。
    volume_bits: AtomicU64,
    /// ミュート。
    muted: AtomicBool,
    /// 音量ノーマライズの線形ゲイン (f64 bits)。1.0 = 素通し、>1.0 = boost、<1.0 = attenuation。
    /// `[10^(-24/20), 10^(24/20)]` (= 約 0.063 〜 15.85) にクランプされる。
    /// 状態判定 (Off / OnApplied / OnUnmeasured) はこの値で行わず、App 側の
    /// `NormalizeUiState` enum で扱う (gain = 1.0 でも測定済みのケースがあるため)。
    normalize_gain_bits: AtomicU64,
}

const SEEK_NONE: u64 = u64::MAX;

/// `seek_override_clear` perf event の `result` 値 (analyze_perf.py が grep する)。
/// 文字列リテラルを散らさず const にして typo を防ぐ。
const RES_STALE_SERIAL: &str = "stale_serial";
const RES_CLEARED: &str = "cleared";
const RES_CAS_FAILED: &str = "cas_failed";

/// シーク完了判定の許容差 (秒)。
/// 用法は呼び出し側で異なる:
/// - video tick (`pts_clears_seek_override`): **片側**。pts > target は無制限許容
///   (forward seek で keyframe が target+GOP 先に飛ぶ 4K HEVC 等のケース)、
///   pts < target は本値だけ許容 (それより前は backward seek 失敗で元位置に戻った
///   ケース → 解除するとシークバーがスナップバックするので保留)。
/// - audio fill_output: **両側**。post-seek の最初のサンプルは ≈ target なので
///   `(pts - target).abs() <= TOL` で判定 (audio には GOP overshoot が無い)。
pub(crate) const SEEK_TARGET_TOLERANCE_SECS: f64 = 0.75;

/// 表示しようとしているフレーム pts が override クリア / 強制表示の対象になるか。
/// `force_display_seek` と override クリア両方で使う共通判定で、両者の規則を
/// 1 箇所に集約してロジックの解離を防ぐ。
pub(crate) fn pts_clears_seek_override(frame_pts: f64, now: f64) -> bool {
    frame_pts - now >= -SEEK_TARGET_TOLERANCE_SECS
}

/// 「フレーム pts <= now + DISPLAY_LEAD_TOLERANCE_SECS なら displayable」と
/// 判定する許容差 (秒)。これは 1 vsync 先のフレームを出すための猶予ではなく、
/// UI tick から実際の present までのごく小さい遅延と起床誤差を吸収するための
/// 固定マージン。60fps/120fps で未来フレームを過剰に拾わないよう 1ms に抑える。
pub(crate) const DISPLAY_LEAD_TOLERANCE_SECS: f64 = 0.001;

/// `clear_seek_target_override` の 4 通りの結果を 1 つの perf event で記録するヘルパ。
/// `crate::perf::is_enabled()` が false なら何もせず即 return (= 引数の評価コスト
/// だけ、JSON 値のアロケーションは発生しない)。
fn log_clear_result(
    completed_serial: u64,
    override_serial: Option<u64>,
    target: Option<f64>,
    result: &'static str,
) {
    if !crate::perf::is_enabled() {
        return;
    }
    let mut fields: Vec<(&str, serde_json::Value)> = Vec::with_capacity(4);
    fields.push((
        "completed_serial",
        serde_json::Value::from(completed_serial as i64),
    ));
    if let Some(s) = override_serial {
        fields.push(("override_serial", serde_json::Value::from(s as i64)));
    }
    if let Some(t) = target {
        fields.push(("target", serde_json::Value::from(t)));
    }
    fields.push(("result", serde_json::Value::from(result)));
    crate::perf::event("video", "seek_override_clear", None, 0, &fields);
}

impl AvClock {
    /// `seek_serial` は `EngineActor` と共有する `Arc<AtomicU64>`。
    /// 構築側 (`VideoPlayer::open`) が 1 個作って両方に clone を渡す。
    pub fn new(initial_volume: f64, seek_serial: Arc<AtomicU64>) -> Self {
        // 初期 anchor は (pts=0.0、wall=now、Frozen)。
        // playing=false / audio_active=false の間は now_secs() が anchor PTS を
        // そのまま返す挙動を再現するため、Frozen で開始するのが等価。
        let master_clock = MasterClock::with_anchor(ClockAnchor::frozen_at(0.0));
        Self {
            master_clock,
            playing: AtomicBool::new(false),
            seek_request: Mutex::new(None),
            seek_serial,
            seek_target_override_bits: AtomicU64::new(SEEK_NONE),
            seek_override_serial: AtomicU64::new(0),
            audio_bookkeeping: AudioBookkeeping::new(),
            playback_speed_bits: AtomicU64::new(1.0_f64.to_bits()),
            playback_speed_update_lock: Mutex::new(()),
            audio_tx_accounting_epoch: AtomicU64::new(0),
            audio_active: AtomicBool::new(false),
            eof_reached: AtomicBool::new(false),
            volume_bits: AtomicU64::new(
                crate::settings::clamp_video_volume(initial_volume).to_bits(),
            ),
            muted: AtomicBool::new(false),
            normalize_gain_bits: AtomicU64::new(1.0_f64.to_bits()),
        }
    }

    pub fn playback_speed(&self) -> f64 {
        clamp_playback_speed(f64::from_bits(
            self.playback_speed_bits.load(Ordering::Acquire),
        ))
    }

    pub fn set_playback_speed(&self, speed: f64) {
        let _update_guard = self
            .playback_speed_update_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let speed = clamp_playback_speed(speed);
        let old = self.playback_speed();
        if (old - speed).abs() < 1.0e-9 {
            return;
        }

        let pts_now = self.now_secs();
        let mut anchor = self.master_clock.anchor();
        anchor.pts_secs = pts_now;
        anchor.wall_at_anchor = Instant::now();
        anchor.speed = speed;
        if self.audio_tx_accounting_epoch() % 2 != 0 {
            self.audio_tx_accounting_epoch
                .fetch_add(1, Ordering::AcqRel);
        }
        // Odd epoch = accounting transition. Decoders wait for a stable even epoch,
        // while in-flight old-epoch add/sub calls become no-ops.
        self.audio_tx_accounting_epoch
            .fetch_add(1, Ordering::AcqRel);
        let _epoch_guard = AudioTxAccountingEpochGuard {
            epoch: &self.audio_tx_accounting_epoch,
        };
        self.master_clock.set_anchor(anchor);
        self.playback_speed_bits
            .store(speed.to_bits(), Ordering::Release);
        // Frames already in audio_tx were accounted with the previous speed.
        // Keep the audio samples valid, but invalidate only their tx accounting.
        self.zero_audio_tx_queued_secs();
    }

    pub fn audio_tx_accounting_epoch(&self) -> u64 {
        self.audio_tx_accounting_epoch.load(Ordering::Acquire)
    }

    pub fn audio_tx_accounting_snapshot(&self) -> (f64, u64) {
        loop {
            let epoch_before = self.audio_tx_accounting_epoch();
            if epoch_before % 2 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let speed = self.playback_speed();
            let epoch_after = self.audio_tx_accounting_epoch();
            if epoch_before == epoch_after {
                return (speed, epoch_before);
            }
        }
    }

    pub fn add_audio_tx_queued_secs_for_epoch(&self, delta_secs: f64, epoch: u64) {
        if self.audio_tx_accounting_epoch() == epoch {
            self.add_audio_tx_queued_secs(delta_secs);
        }
    }

    /// 音声スレッドが「ちょうどこの PTS のサンプルを出力した」と報告する。
    /// **audio_active 状態は変更しない** — silent な fill_output (= 真の音声無し) で
    /// この関数が呼ばれた場合、勝手に audio master 化してしまうのを防ぐため、
    /// activate は `notify_audio_active` 経由で明示的に行う。
    ///
    /// ⚠️ 単調性ガード: 入力 pts が **前回 anchor の audio pts** より小さい (= 後退)
    /// 場合、前回値で固定して後退させない。post-seek の最初の数フレームで古い
    /// pts が後追いで届くケースで UI が逆方向にジャンプしないように。
    ///
    /// 過去実装は `pts_secs.max(self.now_secs())` で **wall extrapolation 値** と
    /// 比較していたが、これだと cpal callback の wall jitter で `now_secs()` が
    /// `pts_secs` より進む → anchor が wall に強制ロック → 長時間 wall pace で進む
    /// → video PTS (30fps 一定) を pace_now が確実に追い越し、decoder が永遠に
    /// catch-up モード = 全フレームカクつき、という実害が出た。前回 anchor PTS と
    /// 比較するなら、wall extrapolation の影響を受けない。
    ///
    /// **設計メモ (= Phase 9.A wall-rate cap の位置付け)**:
    /// 旧版は `fill_output` が silence 出力中も `next_pts_secs` を full want 分
    /// 進めるバグがあり、cpal pre-fill burst で anchor pts が wall の 2.5x 速で
    /// 前進していた。これを後段で cap する `wall_dt + 5ms jitter` 上限ロジックは
    /// 当時 **対症療法** だった。
    ///
    /// 現行版は `fill_output` 側で **実消費サンプル数のみ pts 進行** するよう上流
    /// で正確化済 (= bookkeeping バグ自体が消えた)。理論的には cap 不要だが、
    /// **buffer 非空での stream pre-fill burst** (= pump thread が `stream.play()`
    /// より先に samples を push してしまうケース) では、callback が wall より速く
    /// 連続 pop して `real_consumed` が wall 進行を超える可能性が残る。
    /// 2026-05-03 の実機ログでは、旧式の固定 5ms slack が短周期 callback ごとに
    /// 再付与され、audio master が約 1.39x で進むケースが確認された。
    ///
    /// そのため cap は **defensive safety net** として保持する:
    /// - 通常動作では `pts_secs - prev.pts_secs ≈ wall_dt` で cap 無効
    /// - pre-fill burst 等の異常系で `wall_dt * 1.02` を超える進行を頭打ち
    /// - コスト: 1 atomic load + Instant::now + 比較 ≈ 数十 ns (RT 影響無視できる)
    ///
    /// 実機 perf-log smoke で「pace/wall ≈ 1.0 (= cap 無発動)」を確認できた段階で、
    /// 次のリファクタ機会に cap 撤去を再検討する。
    pub fn set_audio_pts(&self, pts_secs: f64) {
        let prev = self.master_clock.anchor();
        let wall = Instant::now();
        let prev_audio_pts = if matches!(prev.source, ClockSource::Audio) {
            prev.pts_secs
        } else {
            f64::NEG_INFINITY
        };
        // wall-rate cap: defensive safety net (詳細は doc コメント参照)。
        let capped = if matches!(prev.source, ClockSource::Audio) {
            // 前回 anchor が Audio source = 連続的 audio update。wall 経過量で cap。
            //
            // 旧実装は `wall_dt + 5ms` を許容していたが、WASAPI/cpal callback が
            // 小さい間隔で続く環境では、その 5ms が callback ごとに積み上がって
            // audio master clock が 1.3x 以上で進むことがあった。許容は固定値ではなく
            // wall 経過に対する小さい倍率にして、短周期 callback でも累積しないようにする。
            let wall_dt = wall
                .saturating_duration_since(prev.wall_at_anchor)
                .as_secs_f64();
            let speed = self.playback_speed();
            let max_audio_clock_rate = if speed < 1.0 { 1.10 } else { 1.02 };
            let max_advance = wall_dt * max_audio_clock_rate * speed;
            (pts_secs - prev.pts_secs).min(max_advance).max(0.0) + prev.pts_secs
        } else {
            // 前回 Wall/Frozen → audio 起動直後 / seek 直後の起点。cap 無効化
            // (= seek target 等の絶対位置を尊重)。
            pts_secs
        };
        let monotonic = capped.max(prev_audio_pts);
        self.write_audio_anchor_at(monotonic, wall);
    }

    /// **Wall** anchor で全置換する内部ヘルパ。
    /// 用途: 音声無し (audio_active=false) の場合の wall extrapolation 起点設定。
    /// `set_fallback_anchor` から呼ばれる。`notify_seek_completed` の audio_active=false
    /// 経路もここを通る。
    ///
    /// 旧版は audio_active を見て Audio/Wall を選んでいたが、`set_fallback_anchor()`
    /// の呼び出し直後に pump が `notify_audio_active()` した場合、video frame PTS が
    /// audio master anchor になる race があった。
    /// この helper は **常に Wall** を書く。Audio source は `write_audio_anchor_at`
    /// 経由でのみ書ける。
    fn write_fallback_anchor_at(&self, pts: f64, wall: Instant) {
        self.master_clock
            .set_anchor(ClockAnchor::wall(pts, wall).with_speed(self.playback_speed()));
    }

    /// **Audio** anchor で全置換する内部ヘルパ。
    /// 用途: audio actor (cpal callback / pump) が報告した実 audio PTS を anchor 化する。
    /// Phase 2b: MasterClock に Audio source で書く。
    fn write_audio_anchor_at(&self, pts: f64, wall: Instant) {
        self.master_clock
            .set_anchor(ClockAnchor::audio(pts, wall).with_speed(self.playback_speed()));
    }

    /// **PDC latency 変化時専用** の anchor 強制再設定。
    /// `set_audio_pts` の wall-rate cap + monotonic guard を **両方バイパス** して
    /// 指定値で anchor を全置換する (= source は Audio に維持)。
    ///
    /// 使い道: VST プラグインの latency が変化した瞬間。video clock の anchor を
    /// 新しい `pts_for_video` に飛ばすことで、長時間の凍結 (= monotonic guard が
    /// 後退を防ぐため、latency 増加分だけ pts_now が追いつくまで止まる) を回避し、
    /// **映像が前後にジャンプ**する挙動にする (= ユーザー要望)。
    ///
    /// シーク本体 (= `notify_seek_completed`) との違い:
    /// - シークは seek_serial を進めて pump も flush するが、こちらは latency 変化のみで
    ///   seek_serial は維持する (= 通常の audio 経路を継続)
    /// - 短時間の audio バッファ ~300ms 分は古い latency で処理されているが、
    ///   その間は新 latency 基準で映像位置が表示される (= 厳密には 300ms 以内
    ///   ずれる可能性あり、許容)
    pub fn set_audio_pts_jump(&self, pts_secs: f64) {
        let wall = Instant::now();
        let pts = pts_secs.max(0.0);
        self.write_audio_anchor_at(pts, wall);
    }

    /// EOF 時に再生位置を duration に進めたいが、audio_active 状態は変えたくない
    /// ケース (audioless 動画でも duration まで進めて停止アニメを揃えるため)。
    pub fn set_position_at_eof(&self, pts_secs: f64) {
        // EOF 時は時間進行を止める (= 元コードでも recorded_at を今にしていたので
        // 直後の経過は 0)。ここでは Frozen anchor で書く。
        self.master_clock
            .set_anchor(ClockAnchor::frozen_at(pts_secs).with_speed(self.playback_speed()));
    }

    /// 一時停止状態の表示位置を指定 PTS に固定する。
    ///
    /// frame-step の post-seek では音声 callback が drain されないため、通常の
    /// audio 側 override 解除を待つと seek 中扱いが残り、後続フレームを強制表示
    /// してしまう。表示した 1 枚の PTS で Frozen anchor を張り直すための helper。
    pub fn set_paused_position(&self, pts_secs: f64) {
        self.master_clock.set_anchor(
            ClockAnchor::frozen_at(pts_secs.max(0.0)).with_speed(self.playback_speed()),
        );
    }

    /// 真の音声フレームが pump に到達した時に呼ぶ。これで `audio_active = true`
    /// になり、`now_secs()` が audio master モードに切り替わる。
    pub fn notify_audio_active(&self) {
        self.audio_active.store(true, Ordering::Release);
    }

    /// 音声ストリームが無い / 出力起動失敗のときに呼ぶ。`now_secs()` は wall clock
    /// fallback で進行する。
    pub fn mark_audio_inactive(&self) {
        self.audio_active.store(false, Ordering::Release);
    }

    pub fn is_audio_active(&self) -> bool {
        self.audio_active.load(Ordering::Acquire)
    }

    /// 現在の理想的な再生位置 (秒) を返す。
    /// - シーク中 (override 設定中) は target を返す
    /// - playing=true → MasterClock の extrapolation を返す
    /// - playing=false → anchor PTS をそのまま返す (時間進行停止)
    ///
    /// Phase 2b: 旧 audio_active / audio_pts / fallback_pts の分岐は MasterClock の
    /// `ClockSource` (Audio/Wall) で表現済。`set_audio_pts` / `set_fallback_anchor`
    /// が anchor source を適切に設定しているので、この関数では override と playing
    /// の制御だけ行う。
    pub fn now_secs(&self) -> f64 {
        let override_bits = self.seek_target_override_bits.load(Ordering::Acquire);
        if override_bits != SEEK_NONE {
            return f64::from_bits(override_bits);
        }
        let playing = self.playing.load(Ordering::Acquire);
        let anchor = self.master_clock.anchor();
        if !playing {
            // 一時停止中はスナップショット (extrapolation 停止)。
            return anchor.pts_secs;
        }
        // playing=true: MasterClock の Audio/Wall extrapolation を活かす。
        // Frozen anchor (= 構築直後で set_*_anchor が一度も呼ばれていない) は、
        // playing=true でも extrapolation せず anchor PTS をそのまま返す
        // (= 旧コード `recorded == 0` 判定と等価)。
        if matches!(anchor.source, ClockSource::Frozen) {
            return anchor.pts_secs;
        }
        self.master_clock.now_secs()
    }

    /// decoder のペーシングが「自分が今の clock からどれだけ先行しているか」を
    /// 判定するために使う。`now_secs()` をそのまま返す (audio master / fallback
    /// wall のどちらかに対して 100ms 先までしか先行させない)。
    ///
    /// 注意: `displayed_video_pts + wall経過` を `max` で混ぜると、tick が表示
    /// しない期間 (queue 全部 future) でも wall が進んで extrapolation が暴走し、
    /// decoder が UI 進捗を過大評価 → pacing 止まる → drop に陥る。
    pub fn video_pacing_now_secs(&self) -> f64 {
        self.now_secs()
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire)
    }

    /// 再生/一時停止を切り替える。一時停止時は anchor を「現在時刻スナップショット」
    /// に固定し、再生再開時は wall 起点を「今」に巻き戻して時間ジャンプを抑える。
    /// Phase 2b: MasterClock 経由で anchor を更新。
    pub fn set_playing(&self, playing: bool) {
        if playing == self.playing.load(Ordering::Acquire) {
            return;
        }
        let wall_now = Instant::now();
        if playing {
            // 再生再開: 現 anchor の pts をそのまま使い、wall 起点を「今」に書き換える。
            // audio_active=true → Audio source、false → Wall source。
            let cur = self.master_clock.anchor();
            let speed = self.playback_speed();
            let source = if self.audio_active.load(Ordering::Acquire) {
                ClockSource::Audio
            } else {
                ClockSource::Wall
            };
            self.master_clock.set_anchor(ClockAnchor {
                pts_secs: cur.pts_secs,
                wall_at_anchor: wall_now,
                speed,
                source,
            });
        } else {
            // 一時停止: 現時点の now_secs() で Frozen に固定。
            let frozen_pts = self.now_secs();
            self.master_clock
                .set_anchor(ClockAnchor::frozen_at(frozen_pts).with_speed(self.playback_speed()));
        }
        self.playing.store(playing, Ordering::Release);
    }

    /// シーク要求を出す (= 既定 [`SeekKind::Precise`])。デコーダは
    /// [`AvClock::take_seek_request`] で取り出す。
    ///
    /// シークバー・ブックマーク・loop 再生・EOF replay・自動復元など「正確な位置」が
    /// 要る経路はこの API を使う。←→ キーのような「速く飛ばしたい」経路は
    /// [`AvClock::request_seek_with_kind`] に [`SeekKind::Fast`] を渡す。
    pub fn request_seek(&self, target_secs: f64) {
        self.request_seek_with_kind(target_secs, SeekKind::Precise);
    }

    /// シーク種別を明示してシーク要求を出す。
    ///
    /// [`SeekKind::Fast`] は preroll trim を省略するため keyframe pts に着地する
    /// (= target ぴったりではない)。動画 timeline 表示は target で固定されるが、
    /// 視聴コンテンツは 0〜3 秒程度先行する。詳細は [`SeekKind`] のコメント参照。
    pub fn request_seek_with_kind(&self, target_secs: f64, kind: SeekKind) {
        let clamped = target_secs.max(0.0);
        // post-EOF seek サポート: tick が EOF を見て pause しないように先にクリア。
        // decoder の EOF wait ループも peek_seek_request_pending で起床する。
        self.eof_reached.store(false, Ordering::Release);
        let new_serial = self.seek_serial.fetch_add(1, Ordering::AcqRel) + 1;
        self.seek_override_serial
            .store(new_serial, Ordering::Release);
        self.seek_target_override_bits
            .store(clamped.to_bits(), Ordering::Release);
        let mut guard = self.seek_request.lock().unwrap();
        *guard = Some(SeekRequest {
            target_secs: clamped,
            serial: new_serial,
            kind,
        });
        if crate::perf::is_enabled() {
            let kind_str = match kind {
                SeekKind::Precise => "precise",
                SeekKind::Fast => "fast",
            };
            crate::perf::event(
                "video",
                "seek_override_set",
                None,
                0,
                &[
                    ("target", serde_json::Value::from(clamped)),
                    ("serial", serde_json::Value::from(new_serial as i64)),
                    // **field name は `seek_kind`** (Codex P3 助言): perf JSON の top-level
                    // `kind` (= イベント種別 = "seek_override_set") と衝突するため、
                    // 単に "kind" にすると JSON シリアライザでどちらかが上書きされ、
                    // ログに残らないケースがあった (実ログで kind=seek_override_set 固定)。
                    ("seek_kind", serde_json::Value::from(kind_str)),
                ],
            );
        }
    }

    /// 現在 seek の override が立っているか (= seek 完了待ち)。tick が EOF 検出を
    /// 抑制するために使う。
    pub fn is_seeking(&self) -> bool {
        self.seek_target_override_bits.load(Ordering::Acquire) != SEEK_NONE
    }

    pub fn notify_eof_reached(&self) {
        self.eof_reached.store(true, Ordering::Release);
    }
    pub fn clear_eof_reached(&self) {
        self.eof_reached.store(false, Ordering::Release);
    }
    pub fn is_eof_reached(&self) -> bool {
        self.eof_reached.load(Ordering::Acquire)
    }
    /// decoder の EOF wait が seek 要求を非破壊で確認するための peek。
    pub fn peek_seek_request_pending(&self) -> bool {
        self.seek_request.lock().unwrap().is_some()
    }

    /// デコーダ側で呼ぶ。pending なシーク要求があれば取り出す。
    pub(super) fn take_seek_request(&self) -> Option<SeekRequest> {
        self.seek_request.lock().unwrap().take()
    }

    /// 現在のシーク世代を返す。音声 RT コールバックが「自分のバッファが post-seek
    /// な世代に追いついているか」を判定するために使う。
    pub fn current_seek_serial(&self) -> u64 {
        self.seek_serial.load(Ordering::Acquire)
    }

    /// シーク完了通知。デコーダが seek 後の最初のフレームを送出した時点で呼ぶ。
    /// audio / fallback 両 anchor を `(target_pts, 今)` に書き直し、pre-seek の audio
    /// バッファ会計を 0 にリセットする。`set_audio_pts` の単調性ガードが、後追いで届く
    /// fill_output の値が一時的に過去にあっても巻き戻りを防ぐ。
    ///
    /// **注意**: この関数は `seek_target_override` をクリアしない。override クリアは
    /// fill_output 側 (post-seek 後最初の有効サンプル消費時) または UI tick 側
    /// (post-seek 後最初の動画フレーム到着時) で行う。
    pub fn notify_seek_completed(&self, new_pts: f64) {
        // recorded_at = 今 にしておき、now_secs を wall 推定で進める。後追いで届く実
        // audio pts が現在値より小さくても `set_audio_pts` の単調性ガードで後退を防ぐ。
        // sentinel 0 を入れる旧設計は cpal の HW バッファ drain (~500ms) 中 video が
        // 凍結する現象を起こしていた。
        // Phase 2b: 旧 audio anchor + fallback anchor の 2 経路 write を、MasterClock
        // の単一 anchor write に統合。audio_active に応じて Audio/Wall を選ぶ。
        let wall = Instant::now();
        if self.audio_active.load(Ordering::Acquire) {
            self.write_audio_anchor_at(new_pts, wall);
        } else {
            self.write_fallback_anchor_at(new_pts, wall);
        }
        // pre-seek の audio バッファ会計を 0 に (pacing が「audio 十分」と誤認して
        // decoder が sleep し新世代 audio packet を読まなくなる post-seek hang を防止)。
        self.audio_bookkeeping.reset();
    }

    /// post-seek 後の有効フレーム / サンプルを最初に消費した時に呼ぶ。
    /// **`completed_serial` (= 自分が処理した世代)** が現在の override 世代以上
    /// であり、かつ override_bits が cur_bits のままの場合のみ SEEK_NONE に置く。
    /// 音声経路 (fill_output) と UI 経路 (VideoPlayer::tick) の両方から呼んで OK。
    pub fn clear_seek_target_override(&self, completed_serial: u64) {
        // ⚠️ load 順序が重要。**bits を先に load** → serial を validate → CAS の順。
        //
        // 逆順 (serial → bits → CAS) だと、serial load から bits load の間に
        // request_seek が来ると、cur_bits が **新世代 target** になり、後続の CAS が
        // 成功して新 override を誤って消してしまう (race)。
        //
        // bits を先に取れば、その後 request_seek が割り込んでも:
        //   - serial が進む → completed_serial < override_serial で早期 return、または
        //   - bits が新世代に変わる → CAS が expected 不一致で失敗 (誤クリアなし)
        let cur_bits = self.seek_target_override_bits.load(Ordering::Acquire);
        if cur_bits == SEEK_NONE {
            // 通常再生中の audio/video コールバックも毎フレーム到達するため
            // ここでは perflog しない (= flooding 回避)。
            return;
        }
        let override_serial = self.seek_override_serial.load(Ordering::Acquire);
        if completed_serial < override_serial {
            log_clear_result(
                completed_serial,
                Some(override_serial),
                None,
                RES_STALE_SERIAL,
            );
            return;
        }
        let cas_result = self.seek_target_override_bits.compare_exchange(
            cur_bits,
            SEEK_NONE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        let result_str = if cas_result.is_ok() {
            RES_CLEARED
        } else {
            RES_CAS_FAILED
        };
        log_clear_result(
            completed_serial,
            Some(override_serial),
            Some(f64::from_bits(cur_bits)),
            result_str,
        );
    }

    /// seek 失敗時の後始末: anchor は触らずに pre-seek 音声会計だけ 0 リセット。
    /// `notify_seek_completed` を呼ぶと anchor が target に書き換わり、demux 位置と
    /// 整合しないクロック状態を作ってしまうため、失敗経路ではこちらを使う。
    pub fn reset_audio_bookkeeping_only(&self) {
        self.audio_bookkeeping.reset();
    }

    /// fallback wall clock を `(pts, 今)` に再アンカー。
    /// post-seek の override クリア前に呼ぶことで、`notify_seek_completed` 時点の
    /// 古い anchor が clear 直後に時間ジャンプするのを防ぐ (= 見た目フリッカー)。
    /// audio 経路は `set_audio_pts` で同等の役割を果たす。
    /// Phase 2b: MasterClock の Wall anchor として書く (audio_active=false 前提)。
    pub fn set_fallback_anchor(&self, pts: f64) {
        self.write_fallback_anchor_at(pts, Instant::now());
    }

    /// pump 後段 (= processed) ringbuffer 残量 (秒) を報告 (audio.rs から)。
    /// Phase 2a: `AudioBookkeeping` に委譲。
    pub fn set_audio_pump_buf_secs(&self, secs: f64) {
        self.audio_bookkeeping.set_pump_buf_secs(secs);
    }

    /// pump 後段 (= processed) ringbuffer 残量 (秒) を返す。診断ログ用。
    pub fn audio_processed_secs(&self) -> f64 {
        self.audio_bookkeeping.pump_buf_secs()
    }

    /// pump 前段 (= raw_pending) queue 残量 (秒) を報告 (audio.rs から、Codex 助言)。
    pub fn set_audio_raw_pending_secs(&self, secs: f64) {
        self.audio_bookkeeping.set_raw_pending_secs(secs);
    }

    /// pump 前段 (= raw_pending) queue 残量 (秒) を返す。診断ログ用。
    pub fn audio_raw_pending_secs(&self) -> f64 {
        self.audio_bookkeeping.raw_pending_secs()
    }

    /// audio_tx に積まれているフレーム合計時間 (秒) の差分を加える。
    /// decoder の send 直後に +duration、pump の recv 直後に -duration を呼ぶ。
    /// Phase 2a: `AudioBookkeeping` に委譲。
    pub fn add_audio_tx_queued_secs(&self, delta_secs: f64) {
        self.audio_bookkeeping.add_tx_queued(delta_secs);
    }

    /// audio_tx queued 合計を 0 に強制リセット (Codex P2、2026-05-01)。
    /// pump の seek staleness cleanup から呼ばれ、旧世代の tx_queued が `total_audio_buffer_secs()`
    /// (= playable) に残るのを防ぐ。旧世代 frame の `add_tx_queued(-duration)` は clamp される。
    pub fn zero_audio_tx_queued_secs(&self) {
        self.audio_bookkeeping.zero_tx_queued();
    }

    /// audio_tx queued 合計を返す。診断ログ用。
    pub fn audio_tx_queued_secs(&self) -> f64 {
        self.audio_bookkeeping.tx_queued_secs()
    }

    /// 現在の **pacing_audio_secs** (= processed + audio_tx_queued)。
    /// decoder pacing が「audio が枯渇しそう (= 沈黙する) を回避すべきか」判定する値。
    ///
    /// **厳密な playable ではない**: tx_queued は pre-VST/pre-pump なので cpal が
    /// 今すぐ鳴らせる audio ではない。`processed` (= cpal-ready playable) と
    /// `tx_queued` (= bounded 予測補助、cap ≒ 0.7 秒) を合算した折衷値。
    ///
    /// **raw_pending は含めない** (= Codex 助言、2026-05-01 改訂、VST 詰まり / PDC trim
    /// drop で raw が playable にならない退行を防ぐ)。raw 状態は
    /// [`audio_supply_secs`](Self::audio_supply_secs) / [`audio_raw_pending_secs`]
    /// (Self::audio_raw_pending_secs) で別取得。
    /// PDC latency も含まない (`vst3_pdc_latency_secs` で別取得)。
    pub fn total_audio_buffer_secs(&self) -> f64 {
        self.audio_bookkeeping.total_secs()
    }

    /// 現在の **pre-VST supply** 音声秒数 (= raw_pending + audio_tx_queued)。診断用。
    /// decoder pacing は本値を pacing_audio_secs とは別に参照し、starvation 復旧の
    /// 予兆等の判断材料として使う。
    pub fn audio_supply_secs(&self) -> f64 {
        self.audio_bookkeeping.supply_secs()
    }

    /// VST3 PDC latency (秒) を pump push 時に publish する。
    pub fn set_vst3_pdc_latency_secs(&self, secs: f64) {
        self.audio_bookkeeping.set_vst3_pdc_latency_secs(secs);
    }

    /// VST3 PDC latency (秒) を返す。decoder pacing が「先読み許可量 = PACE_LEAD + pdc」
    /// で必要な未来入力量を確保するために使う。
    pub fn vst3_pdc_latency_secs(&self) -> f64 {
        self.audio_bookkeeping.vst3_pdc_latency_secs()
    }

    pub fn volume(&self) -> f64 {
        f64::from_bits(self.volume_bits.load(Ordering::Acquire))
    }

    pub fn set_volume(&self, v: f64) {
        self.volume_bits.store(
            crate::settings::clamp_video_volume(v).to_bits(),
            Ordering::Release,
        );
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }

    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Release);
    }

    /// RT 出力コールバックで掛ける音量 (mute 中は 0、100% 超 boost はここでは掛けない)。
    pub fn output_volume(&self) -> f32 {
        if self.is_muted() {
            0.0
        } else {
            self.volume().min(1.0) as f32
        }
    }

    /// 音声ポンプ側で safety limiter の前に掛ける boost。100% 以下では 1.0。
    pub fn pre_limiter_gain(&self) -> f32 {
        self.volume().max(1.0) as f32
    }

    /// 音量ノーマライズ用の線形ゲイン (1.0 = 素通し)。
    pub fn normalize_gain(&self) -> f64 {
        f64::from_bits(self.normalize_gain_bits.load(Ordering::Acquire))
    }

    /// 音量ノーマライズ用の線形ゲインを設定 (内部で `[10^(-24/20), 10^(24/20)]` にクランプ)。
    pub fn set_normalize_gain(&self, gain: f64) {
        self.normalize_gain_bits
            .store(clamp_normalize_gain(gain).to_bits(), Ordering::Release);
    }
}

/// 音量ノーマライズの ±24dB 線形値。
pub const NORMALIZE_GAIN_DB_LIMIT: f64 = 24.0;
/// 上限 +24dB の線形値 (= 10^(24/20) ≈ 15.849)。
pub fn normalize_gain_max_linear() -> f64 {
    10.0_f64.powf(NORMALIZE_GAIN_DB_LIMIT / 20.0)
}
/// 下限 -24dB の線形値 (= 10^(-24/20) ≈ 0.0631)。
pub fn normalize_gain_min_linear() -> f64 {
    10.0_f64.powf(-NORMALIZE_GAIN_DB_LIMIT / 20.0)
}

/// `set_normalize_gain` で適用するクランプ関数。NaN / Inf は 1.0 にフォールバック。
pub fn clamp_normalize_gain(gain: f64) -> f64 {
    if !gain.is_finite() || gain <= 0.0 {
        return 1.0;
    }
    gain.clamp(normalize_gain_min_linear(), normalize_gain_max_linear())
}

struct AudioTxAccountingEpochGuard<'a> {
    epoch: &'a AtomicU64,
}

impl Drop for AudioTxAccountingEpochGuard<'_> {
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn speed_change_invalidates_old_audio_tx_accounting_epoch() {
        let clock = AvClock::new(1.0, Arc::new(AtomicU64::new(0)));

        let (_, old_epoch) = clock.audio_tx_accounting_snapshot();
        clock.add_audio_tx_queued_secs_for_epoch(0.4, old_epoch);
        assert!((clock.audio_tx_queued_secs() - 0.4).abs() < 1.0e-9);

        clock.set_playback_speed(2.0);
        assert_eq!(clock.audio_tx_accounting_epoch() % 2, 0);
        assert!(clock.audio_tx_queued_secs().abs() < 1.0e-9);

        clock.add_audio_tx_queued_secs_for_epoch(-0.4, old_epoch);
        assert!(clock.audio_tx_queued_secs().abs() < 1.0e-9);

        let (speed, new_epoch) = clock.audio_tx_accounting_snapshot();
        assert!((speed - 2.0).abs() < 1.0e-9);
        assert_ne!(new_epoch, old_epoch);

        clock.add_audio_tx_queued_secs_for_epoch(0.2, new_epoch);
        assert!((clock.audio_tx_queued_secs() - 0.2).abs() < 1.0e-9);
    }

    #[test]
    fn volume_supports_manual_boost_but_caps_rt_output_gain() {
        let clock = AvClock::new(2.0, Arc::new(AtomicU64::new(0)));
        assert!((clock.volume() - crate::settings::VIDEO_VOLUME_MAX).abs() < 1.0e-9);
        assert!((clock.output_volume() - 1.0).abs() < 1.0e-6);
        assert!((clock.pre_limiter_gain() - 1.5).abs() < 1.0e-6);

        clock.set_muted(true);
        assert_eq!(clock.output_volume(), 0.0);
        assert!((clock.pre_limiter_gain() - 1.5).abs() < 1.0e-6);
    }

    #[test]
    fn normalize_gain_default_is_unity() {
        let clock = AvClock::new(1.0, Arc::new(AtomicU64::new(0)));
        assert!((clock.normalize_gain() - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn normalize_gain_clamps_to_24db_range() {
        let clock = AvClock::new(1.0, Arc::new(AtomicU64::new(0)));
        // +30dB を入れても +24dB に丸められる
        clock.set_normalize_gain(31.62);
        assert!((clock.normalize_gain() - normalize_gain_max_linear()).abs() < 1.0e-9);
        // -30dB を入れても -24dB に丸められる
        clock.set_normalize_gain(0.0316);
        assert!((clock.normalize_gain() - normalize_gain_min_linear()).abs() < 1.0e-9);
        // 範囲内の値はそのまま
        clock.set_normalize_gain(2.0);
        assert!((clock.normalize_gain() - 2.0).abs() < 1.0e-9);
    }

    #[test]
    fn normalize_gain_falls_back_on_non_finite() {
        let clock = AvClock::new(1.0, Arc::new(AtomicU64::new(0)));
        clock.set_normalize_gain(f64::NAN);
        assert_eq!(clock.normalize_gain(), 1.0);
        clock.set_normalize_gain(f64::INFINITY);
        assert_eq!(clock.normalize_gain(), 1.0);
        clock.set_normalize_gain(-1.0);
        assert_eq!(clock.normalize_gain(), 1.0);
        clock.set_normalize_gain(0.0);
        assert_eq!(clock.normalize_gain(), 1.0);
    }
}
