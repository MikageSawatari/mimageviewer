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
    /// 音量 (0.0-1.0、f64 bits)。
    volume_bits: AtomicU64,
    /// ミュート。
    muted: AtomicBool,
}

const SEEK_NONE: u64 = u64::MAX;

/// シーク完了判定の許容差 (秒)。
/// frame.pts と現在の override target の差がこれ以内なら「target に到達した」
/// とみなして override を解除する。典型的な keyframe 間隔 (0.5-2 秒) を見越した値。
/// これを超えるズレは decoder 側の seek 失敗を疑う合図。
pub(crate) const SEEK_TARGET_TOLERANCE_SECS: f64 = 0.75;

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
            volume_bits: AtomicU64::new(initial_volume.clamp(0.0, 1.0).to_bits()),
            muted: AtomicBool::new(false),
        }
    }

    /// 音声スレッドが「ちょうどこの PTS のサンプルを出力した」と報告する。
    pub fn set_audio_pts(&self, pts_secs: f64) {
        let now_ns = self.epoch.elapsed().as_nanos() as i64;
        // pts と recorded_at は本来「同時に書き換わる」べきだが、独立 atomic なので
        // 厳密には torn read が起きうる。実際は now_secs() の誤差が 1 サンプル分
        // (~20µs @ 48kHz) の範囲に収まるので無害。
        self.audio_pts_bits.store(pts_secs.to_bits(), Ordering::Release);
        self.audio_recorded_at_nanos.store(now_ns, Ordering::Release);
    }

    /// 現在の理想的な再生位置 (秒) を返す。
    /// 一時停止中は audio_pts のスナップショットを返す (時間が進まない)。
    /// シーク中 (override が設定されている間) は target を返し続ける。
    pub fn now_secs(&self) -> f64 {
        // ── seek override が立っていれば最優先で target を返す ──
        let override_bits = self.seek_target_override_bits.load(Ordering::Acquire);
        if override_bits != SEEK_NONE {
            return f64::from_bits(override_bits);
        }
        let pts = f64::from_bits(self.audio_pts_bits.load(Ordering::Acquire));
        if !self.playing.load(Ordering::Acquire) {
            return pts;
        }
        let recorded = self.audio_recorded_at_nanos.load(Ordering::Acquire);
        if recorded == 0 {
            // まだ音声出力が始まっていない → 0 を返す (動画は冒頭で待機)
            return pts;
        }
        let now = self.epoch.elapsed().as_nanos() as i64;
        let elapsed = (now - recorded).max(0) as f64 / 1_000_000_000.0;
        pts + elapsed
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
        if playing {
            // 再生再開: recorded_at を今に巻き戻す (PTS は据え置き)
            let now_ns = self.epoch.elapsed().as_nanos() as i64;
            self.audio_recorded_at_nanos.store(now_ns, Ordering::Release);
        } else {
            // 一時停止: PTS を「今の now_secs」で固定 (= 巻き戻し抑止)
            let frozen = self.now_secs();
            self.audio_pts_bits.store(frozen.to_bits(), Ordering::Release);
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
        // serial を先に進めて、override / request を同じ serial で揃える。
        // (順序: seek_serial → override → seek_request の順。read 側は seek_request
        // を Mutex で取るので、Mutex 解放までに override も serial も書き終わっている。)
        let new_serial = self.seek_serial.fetch_add(1, Ordering::AcqRel) + 1;
        // override は新世代の serial で先に書き、now_secs() からは即座に target が見える。
        self.seek_override_serial
            .store(new_serial, Ordering::Release);
        self.seek_target_override_bits
            .store(clamped.to_bits(), Ordering::Release);
        // request 本体は Mutex で整合的に書き、decoder に new_serial で渡す。
        let mut guard = self.seek_request.lock().unwrap();
        *guard = Some(SeekRequest {
            target_secs: clamped,
            direction,
            serial: new_serial,
        });
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
    /// クロックを seek 位置にリセットする。
    ///
    /// `audio_recorded_at_nanos` は **0 (sentinel)** にして「audio が追いつくまでは
    /// elapsed を加算しない」状態にする。これがないと、notify_seek_completed の直後から
    /// fill_output が post-seek フレームを実際に消費するまでの数 10ms の間、
    /// `now_secs() = target + elapsed_since_notify` になってシークバーが target を
    /// 超えて少し進み、その後 fill_output で `set_audio_pts(target + 5ms)` が呼ばれた
    /// 瞬間に「target+5ms」に巻き戻って見える (ユーザー報告: 「シークしたが少し戻る」)。
    /// 0 にしておけば `now_secs()` は target で凍結し、fill_output が初回更新するまで
    /// 待つので、正しく target → target+5ms と単調増加する。
    ///
    /// **注意**: この関数は `seek_target_override` をクリアしない。override クリアは
    /// fill_output 側 (post-seek 後最初の有効サンプル消費時) または UI tick 側
    /// (post-seek 後最初の動画フレーム到着時) で行う。
    pub fn notify_seek_completed(&self, new_pts: f64) {
        self.audio_pts_bits.store(new_pts.to_bits(), Ordering::Release);
        self.audio_recorded_at_nanos.store(0, Ordering::Release);
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
