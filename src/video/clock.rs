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
//!   no-op で、`audio.rs` が `clock.effective_volume()` を直接読む構造になっている。
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

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use crate::video::engine::audio_bookkeeping::AudioBookkeeping;
use crate::video::engine::clock::{ClockAnchor, ClockSource, MasterClock};

/// シーク要求の整合性ある 1 件分。AvClock の `seek_request` Mutex が保護する。
/// `decoder` モジュールが `take_seek_request()` で取り出す。
#[derive(Clone, Copy, Debug)]
pub(super) struct SeekRequest {
    pub target_secs: f64,
    /// シーク世代。request 時点で `seek_serial` から取得・進めた値。
    pub serial: u64,
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
/// 判定する許容差 (秒)。約 1 vsync 周期 (60Hz = 16.7ms) を許容することで、
/// vsync 直後に来た「ほぼ now のフレーム」を確実に 1 tick で表示できる
/// (= 60fps コンテンツが 30fps 表示に落ちる現象を回避)。
/// AV 同期上は 16ms 程度の lead は知覚されない (audio-video sync 許容窓は ±50ms)。
pub(crate) const DISPLAY_LEAD_TOLERANCE_SECS: f64 = 0.016;

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
        let master_clock =
            MasterClock::with_anchor(ClockAnchor::frozen_at(0.0));
        Self {
            master_clock,
            playing: AtomicBool::new(false),
            seek_request: Mutex::new(None),
            seek_serial,
            seek_target_override_bits: AtomicU64::new(SEEK_NONE),
            seek_override_serial: AtomicU64::new(0),
            audio_bookkeeping: AudioBookkeeping::new(),
            audio_active: AtomicBool::new(false),
            eof_reached: AtomicBool::new(false),
            volume_bits: AtomicU64::new(initial_volume.clamp(0.0, 1.0).to_bits()),
            muted: AtomicBool::new(false),
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
    /// 連続 pop して `real_consumed` が wall 進行を超える可能性が残る (Codex P? 指摘)。
    ///
    /// そのため cap は **defensive safety net** として保持する:
    /// - 通常動作では `pts_secs - prev.pts_secs ≈ wall_dt` で cap 無効
    /// - pre-fill burst 等の異常系で `wall_dt + 5ms` を超える進行を頭打ち
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
            let wall_dt = wall.saturating_duration_since(prev.wall_at_anchor).as_secs_f64();
            const JITTER_TOLERANCE_SECS: f64 = 0.005;
            let max_advance = wall_dt + JITTER_TOLERANCE_SECS;
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
        self.master_clock.set_anchor(ClockAnchor::wall(pts, wall));
    }

    /// **Audio** anchor で全置換する内部ヘルパ。
    /// 用途: audio actor (cpal callback / pump) が報告した実 audio PTS を anchor 化する。
    /// Phase 2b: MasterClock に Audio source で書く。
    fn write_audio_anchor_at(&self, pts: f64, wall: Instant) {
        self.master_clock.set_anchor(ClockAnchor::audio(pts, wall));
    }

    /// EOF 時に再生位置を duration に進めたいが、audio_active 状態は変えたくない
    /// ケース (audioless 動画でも duration まで進めて停止アニメを揃えるため)。
    pub fn set_position_at_eof(&self, pts_secs: f64) {
        // EOF 時は時間進行を止める (= 元コードでも recorded_at を今にしていたので
        // 直後の経過は 0)。ここでは Frozen anchor で書く。
        self.master_clock
            .set_anchor(ClockAnchor::frozen_at(pts_secs));
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
            let source = if self.audio_active.load(Ordering::Acquire) {
                ClockSource::Audio
            } else {
                ClockSource::Wall
            };
            self.master_clock.set_anchor(ClockAnchor {
                pts_secs: cur.pts_secs,
                wall_at_anchor: wall_now,
                speed: 1.0,
                source,
            });
        } else {
            // 一時停止: 現時点の now_secs() で Frozen に固定。
            let frozen_pts = self.now_secs();
            self.master_clock
                .set_anchor(ClockAnchor::frozen_at(frozen_pts));
        }
        self.playing.store(playing, Ordering::Release);
    }

    /// シーク要求を出す。デコーダは [`AvClock::take_seek_request`] で取り出す。
    ///
    /// 旧 API では `direction` 引数で前方/後方/絶対のヒントを渡していたが、
    /// Phase 9.F で「方向に関係なく **常に backward+preroll**」に統一したため、
    /// decoder 側で direction を参照しなくなった。引数は撤去済み。
    pub fn request_seek(&self, target_secs: f64) {
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
        });
        if crate::perf::is_enabled() {
            crate::perf::event(
                "video",
                "seek_override_set",
                None,
                0,
                &[
                    ("target", serde_json::Value::from(clamped)),
                    ("serial", serde_json::Value::from(new_serial as i64)),
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

    /// pump 内ringbuffer 残量 (秒) を報告 (audio.rs から)。
    /// Phase 2a: `AudioBookkeeping` に委譲。
    pub fn set_audio_pump_buf_secs(&self, secs: f64) {
        self.audio_bookkeeping.set_pump_buf_secs(secs);
    }

    /// audio_tx に積まれているフレーム合計時間 (秒) の差分を加える。
    /// decoder の send 直後に +duration、pump の recv 直後に -duration を呼ぶ。
    /// Phase 2a: `AudioBookkeeping` に委譲。
    pub fn add_audio_tx_queued_secs(&self, delta_secs: f64) {
        self.audio_bookkeeping.add_tx_queued(delta_secs);
    }

    /// 現在の総音声バッファ秒数 (= pump + audio_tx queued)。
    /// decoder pacing が「audio が枯渇しそう (= 沈黙する) を回避すべきか」判定する。
    /// Phase 2a: `AudioBookkeeping` に委譲。
    pub fn total_audio_buffer_secs(&self) -> f64 {
        self.audio_bookkeeping.total_secs()
    }


    pub fn volume(&self) -> f64 {
        f64::from_bits(self.volume_bits.load(Ordering::Acquire))
    }

    pub fn set_volume(&self, v: f64) {
        self.volume_bits
            .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Release);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }

    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Release);
    }

    /// 実効音量 (mute 中は 0)。音声コールバックが毎回参照する。
    pub fn effective_volume(&self) -> f32 {
        if self.is_muted() {
            0.0
        } else {
            self.volume() as f32
        }
    }
}
