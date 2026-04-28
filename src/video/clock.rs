//! 動画再生用 AV マスタークロック。
//!
//! 音声 PTS をマスターにする一般的な実装 (mpv / ffplay 等と同じ)。
//! 音声スレッドは出力したサンプル列の PTS を [`AvClock::set_audio_pts`] で報告し、
//! 動画スレッドは [`AvClock::now_secs`] で「現在の理想的な再生位置」を取得する。
//! 動画フレームの PTS が `now_secs` より十分小さければドロップ、十分大きければ待機する。
//!
//! 一時停止・シークの状態もここに集約する。
//!
//! 大半のフィールドが atomic で Mutex 不要。f64 は `to_bits` / `from_bits` で
//! `AtomicU64` に格納する。シーク要求 (target / direction / serial) はトリプルで
//! 整合性が必要なので [`Mutex<SeekRequest>`] で保護する。fill_output などの RT 経路は
//! 触らないので RT パフォーマンスに影響しない。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::Instant;

/// シーク要求の整合性ある 1 件分。AvClock の `seek_request` Mutex が保護する。
/// `decoder` モジュールが `take_seek_request()` で取り出す。
#[derive(Clone, Copy, Debug)]
pub(super) struct SeekRequest {
    pub target_secs: f64,
    /// `1` = 前方相対 (`target..` のキーフレームに飛ぶ。preroll なし)
    /// `-1` = 後方相対 (`..target` のキーフレームに飛ぶ。preroll で target に進む)
    /// `0` = 絶対 (シークバー直接クリック。`..target` のキーフレーム)
    pub direction: i8,
    /// シーク世代。request 時点で `seek_serial` から取得・進めた値。
    pub serial: u64,
}

pub struct AvClock {
    /// `Instant::now()` の参照点 (構築時刻)。以降のすべての時刻はこれからの差分 ns で扱う。
    epoch: Instant,
    /// 最後に報告された音声 PTS (秒、f64 bits)。
    /// 動画ストリームしか無い (音声無し) ファイルでは、デコーダ側が
    /// `set_audio_pts` で video PTS を直接報告するフォールバックを使う。
    audio_pts_bits: AtomicU64,
    /// その PTS を報告した時刻 (epoch からの ns)。
    audio_recorded_at_nanos: AtomicI64,
    /// 再生中フラグ。false の間は now_secs() が進まない。
    playing: AtomicBool,
    /// pending なシーク要求。`None` = 未消費なし。
    /// take/request の両方が Mutex を取り、整合性のある (target, direction, serial)
    /// トリプルを観測できるようにする。
    seek_request: Mutex<Option<SeekRequest>>,
    /// 直近のシーク要求の世代。`request_seek` のたびに +1。
    /// 音声 RT コールバック (`fill_output`) と UI の `tick` がポーリングで読むので
    /// atomic で公開する。Mutex を取らずに「自分が処理中の世代より新しい seek が
    /// 走ったか」だけ見れる。
    seek_serial: AtomicU64,
    /// **シーク中の表示位置 override** (秒、f64 bits、SEEK_NONE = 無効)。
    /// 設定中は `now_secs()` が audio_pts ではなく target を返し続け、UI のフリッカ
    /// (target → target+ε → target に戻る) を防ぐ。fill_output / UI tick が
    /// 「post-seek 後最初の target 近傍フレーム」を消費した時点で解除する。
    seek_target_override_bits: AtomicU64,
    /// override がセットされた時の seek_serial。clear 時にこれと現在の seek_serial を
    /// CAS 比較し、**新しいシーク要求が来ていたら clear をスキップ**する。これで
    /// 「古い fill_output コールバックが、新たに発生した override を誤クリアする」
    /// race を排除する (Codex P1 指摘)。
    seek_override_serial: AtomicU64,
    /// pump リングバッファの残量 (秒、f64 bits)。pump push / fill_output pop で更新。
    audio_pump_buf_secs_bits: AtomicU64,
    /// audio_tx に積まれているフレーム合計時間 (秒、f64 bits)。decoder の send 後 +,
    /// pump の recv 後 -。`pump_buf + tx_queued` で総音声バッファ秒数になる。
    audio_tx_queued_secs_bits: AtomicU64,
    /// audio が「healthy」状態か。decoder が `notify_audio_active(true)` で開始、
    /// audio 出力起動失敗 / 音声ストリーム不在のとき false。
    /// false なら `now_secs()` はフォールバック wall clock を使う。
    audio_active: AtomicBool,
    /// フォールバック wall clock の基準 PTS (秒、f64 bits)。
    fallback_pts_bits: AtomicU64,
    /// 同 recorded_at_nanos (epoch から)。
    fallback_recorded_at_nanos: AtomicI64,
    /// decoder が EOF (= demux 末端) に到達したか。post-EOF seek を検出する。
    /// `notify_eof_reached` で立て、`request_seek` / `clear_eof_reached` で降ろす。
    eof_reached: AtomicBool,
    /// 音量 (0.0-1.0、f64 bits)。
    volume_bits: AtomicU64,
    /// ミュート。
    muted: AtomicBool,
}

const SEEK_NONE: u64 = u64::MAX;

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

impl AvClock {
    pub fn new(initial_volume: f64) -> Self {
        Self {
            epoch: Instant::now(),
            audio_pts_bits: AtomicU64::new(0.0_f64.to_bits()),
            audio_recorded_at_nanos: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            seek_request: Mutex::new(None),
            seek_serial: AtomicU64::new(0),
            seek_target_override_bits: AtomicU64::new(SEEK_NONE),
            seek_override_serial: AtomicU64::new(0),
            audio_pump_buf_secs_bits: AtomicU64::new(0.0_f64.to_bits()),
            audio_tx_queued_secs_bits: AtomicU64::new(0.0_f64.to_bits()),
            audio_active: AtomicBool::new(false),
            fallback_pts_bits: AtomicU64::new(0.0_f64.to_bits()),
            fallback_recorded_at_nanos: AtomicI64::new(0),
            eof_reached: AtomicBool::new(false),
            volume_bits: AtomicU64::new(initial_volume.clamp(0.0, 1.0).to_bits()),
            muted: AtomicBool::new(false),
        }
    }

    /// 音声スレッドが「ちょうどこの PTS のサンプルを出力した」と報告する。
    /// **audio_active 状態は変更しない** — silent な fill_output (= 真の音声無し) で
    /// この関数が呼ばれた場合、勝手に audio master 化してしまうのを防ぐため
    /// (Codex 指摘)。activate は `notify_audio_active` 経由で明示的に行う。
    ///
    /// ⚠️ 単調性ガード: 入力 pts が現在の `now_secs()` より小さい (= 過去) 場合、
    /// 現在値で固定して **後退させない**。これは notify_seek_completed が wall
    /// 推定で先行させた `now_secs` に対して、後追いで届いた実 audio pts が少しだけ
    /// 過去に位置するケース (post-seek の最初の数フレーム) で UI が逆方向にジャンプ
    /// しないようにする。
    pub fn set_audio_pts(&self, pts_secs: f64) {
        let now_ns = self.epoch.elapsed().as_nanos() as i64;
        let monotonic = pts_secs.max(self.now_secs());
        self.write_audio_anchor(monotonic, now_ns);
    }

    /// 真の音声フレームが pump に到達した時に呼ぶ。これで `audio_active = true`
    /// になり、`now_secs()` が audio master モードに切り替わる。
    pub fn notify_audio_active(&self) {
        self.audio_active.store(true, Ordering::Release);
    }

    /// fallback wall clock を `(pts, now_ns)` で書き直す内部ヘルパ。
    /// `notify_seek_completed` / `set_position_at_eof` / `set_fallback_anchor` で共有。
    fn write_fallback_anchor(&self, pts: f64, now_ns: i64) {
        self.fallback_pts_bits.store(pts.to_bits(), Ordering::Release);
        self.fallback_recorded_at_nanos.store(now_ns, Ordering::Release);
    }

    /// audio anchor を `(pts, now_ns)` で書き直す内部ヘルパ。
    fn write_audio_anchor(&self, pts: f64, now_ns: i64) {
        self.audio_pts_bits.store(pts.to_bits(), Ordering::Release);
        self.audio_recorded_at_nanos.store(now_ns, Ordering::Release);
    }

    /// EOF 時に再生位置を duration に進めたいが、audio_active 状態は変えたくない
    /// ケース (audioless 動画でも duration まで進めて停止アニメを揃えるため)。
    pub fn set_position_at_eof(&self, pts_secs: f64) {
        let now_ns = self.epoch.elapsed().as_nanos() as i64;
        self.write_audio_anchor(pts_secs, now_ns);
        self.write_fallback_anchor(pts_secs, now_ns);
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
    /// - audio_active = true なら audio_pts + 経過 (audio master)
    /// - そうでなければ fallback_pts + 経過 (wall clock master)
    /// 一時停止中はスナップショット (extrapolation 停止)。
    pub fn now_secs(&self) -> f64 {
        let override_bits = self.seek_target_override_bits.load(Ordering::Acquire);
        if override_bits != SEEK_NONE {
            return f64::from_bits(override_bits);
        }
        let playing = self.playing.load(Ordering::Acquire);
        if self.audio_active.load(Ordering::Acquire) {
            let pts = f64::from_bits(self.audio_pts_bits.load(Ordering::Acquire));
            if !playing {
                return pts;
            }
            let recorded = self.audio_recorded_at_nanos.load(Ordering::Acquire);
            if recorded == 0 {
                return pts;
            }
            let now = self.epoch.elapsed().as_nanos() as i64;
            return pts + (now - recorded).max(0) as f64 / 1_000_000_000.0;
        }
        // ── 音声無し / 出力失敗時は wall clock anchor で進行 ──
        let pts = f64::from_bits(self.fallback_pts_bits.load(Ordering::Acquire));
        if !playing {
            return pts;
        }
        let recorded = self.fallback_recorded_at_nanos.load(Ordering::Acquire);
        if recorded == 0 {
            return pts;
        }
        let now = self.epoch.elapsed().as_nanos() as i64;
        pts + (now - recorded).max(0) as f64 / 1_000_000_000.0
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

    /// 再生/一時停止を切り替える。一時停止時は audio_recorded_at を「今」に
    /// 巻き戻して、再生再開時に時間ジャンプが起きないようにする。
    pub fn set_playing(&self, playing: bool) {
        if playing == self.playing.load(Ordering::Acquire) {
            return;
        }
        let now_ns = self.epoch.elapsed().as_nanos() as i64;
        if playing {
            // 再生再開: audio / fallback 両方の recorded_at を今に巻き戻す
            self.audio_recorded_at_nanos.store(now_ns, Ordering::Release);
            // fallback も同時アンカー (audio が無いケースで now_secs が wall 進行する)
            let cur_fallback = f64::from_bits(self.fallback_pts_bits.load(Ordering::Acquire));
            // 一時停止前の fallback_pts はそのまま、recorded_at だけ更新
            let _ = cur_fallback;
            self.fallback_recorded_at_nanos.store(now_ns, Ordering::Release);
        } else {
            // 一時停止: 現時刻スナップショットで固定 (どちらの master でも巻き戻し抑止)
            let frozen = self.now_secs();
            if self.audio_active.load(Ordering::Acquire) {
                self.audio_pts_bits.store(frozen.to_bits(), Ordering::Release);
            }
            self.fallback_pts_bits.store(frozen.to_bits(), Ordering::Release);
        }
        self.playing.store(playing, Ordering::Release);
    }

    /// シーク要求を出す。デコーダは [`AvClock::take_seek_request`] で取り出す。
    ///
    /// `direction` はシーク方向のヒント:
    ///   - `1` = 前方相対 (`seek_relative(+5)` 等)。decoder は `target_pts..` で
    ///     target 以降のキーフレームに飛ぶ → preroll なしで即時表示
    ///   - `-1` = 後方相対 (`seek_relative(-5)` 等)。decoder は `..target_pts` で
    ///     target 以前のキーフレームに飛ぶ → preroll で target に進む
    ///   - `0` = 絶対 (シークバー直接クリック)。decoder は `..target_pts`
    pub fn request_seek(&self, target_secs: f64, direction: i8) {
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
            direction,
            serial: new_serial,
        });
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
        let now_ns = self.epoch.elapsed().as_nanos() as i64;
        // recorded_at = 今 にしておき、now_secs を wall 推定で進める。後追いで届く実
        // audio pts が現在値より小さくても `set_audio_pts` の単調性ガードで後退を防ぐ。
        // sentinel 0 を入れる旧設計は cpal の HW バッファ drain (~500ms) 中 video が
        // 凍結する現象を起こしていた。
        self.write_audio_anchor(new_pts, now_ns);
        self.write_fallback_anchor(new_pts, now_ns);
        // pre-seek の audio バッファ会計を 0 に (pacing が「audio 十分」と誤認して
        // decoder が sleep し新世代 audio packet を読まなくなる post-seek hang を防止)。
        self.audio_pump_buf_secs_bits
            .store(0.0_f64.to_bits(), Ordering::Release);
        self.audio_tx_queued_secs_bits
            .store(0.0_f64.to_bits(), Ordering::Release);
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
        // 成功して新 override を誤って消してしまう (Codex 指摘の race)。
        //
        // bits を先に取れば、その後 request_seek が割り込んでも:
        //   - serial が進む → completed_serial < override_serial で早期 return、または
        //   - bits が新世代に変わる → CAS が expected 不一致で失敗 (誤クリアなし)
        let cur_bits = self.seek_target_override_bits.load(Ordering::Acquire);
        if cur_bits == SEEK_NONE {
            return;
        }
        let override_serial = self.seek_override_serial.load(Ordering::Acquire);
        if completed_serial < override_serial {
            return;
        }
        let _ = self.seek_target_override_bits.compare_exchange(
            cur_bits,
            SEEK_NONE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// fallback wall clock を `(pts, 今)` に再アンカー。
    /// post-seek の override クリア前に呼ぶことで、`notify_seek_completed` 時点の
    /// 古い anchor が clear 直後に時間ジャンプするのを防ぐ (= 見た目フリッカー)。
    /// audio 経路は `set_audio_pts` で同等の役割を果たす。
    pub fn set_fallback_anchor(&self, pts: f64) {
        let now_ns = self.epoch.elapsed().as_nanos() as i64;
        self.write_fallback_anchor(pts, now_ns);
    }

    /// pump 内ringbuffer 残量 (秒) を報告 (audio.rs から)。
    pub fn set_audio_pump_buf_secs(&self, secs: f64) {
        self.audio_pump_buf_secs_bits
            .store(secs.to_bits(), Ordering::Release);
    }

    /// audio_tx に積まれているフレーム合計時間 (秒) の差分を加える。
    /// decoder の send 直後に +duration、pump の recv 直後に -duration を呼ぶ。
    pub fn add_audio_tx_queued_secs(&self, delta_secs: f64) {
        // f64 の atomic 加算は CAS ループで実装。短時間の write 競合のみなのでコスト低。
        let mut cur = self.audio_tx_queued_secs_bits.load(Ordering::Relaxed);
        loop {
            let new_val = (f64::from_bits(cur) + delta_secs).max(0.0);
            match self.audio_tx_queued_secs_bits.compare_exchange_weak(
                cur,
                new_val.to_bits(),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => cur = actual,
            }
        }
    }

    /// 現在の総音声バッファ秒数 (= pump + audio_tx queued)。
    /// decoder pacing が「audio が枯渇しそう (= 沈黙する) を回避すべきか」判定する。
    pub fn total_audio_buffer_secs(&self) -> f64 {
        let pump = f64::from_bits(self.audio_pump_buf_secs_bits.load(Ordering::Acquire));
        let tx = f64::from_bits(self.audio_tx_queued_secs_bits.load(Ordering::Acquire));
        pump + tx
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
