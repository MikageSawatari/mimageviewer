//! 音量ノーマライズ機能で video / app / native_presenter の各層が共有する POD 型。
//!
//! ## 配置理由
//! `NormalizeUiState` 等を `src/app/normalize.rs` に置くと、native_presenter から
//! 参照する際に video → app の依存逆転が発生する。共有 POD 型は video 層に置き、
//! app 層 (`src/app/normalize.rs`) には worker handle や mpsc rx 等の制御専用構造体
//! (`NormalizeScanState`) のみを置く。
//!
//! ## 設計方針
//! - 全て `Clone + Copy + Debug + PartialEq + Send + Sync + 'static` の軽量 POD
//! - mpsc / Arc 等で multi-thread 経由しても安全
//! - 状態判定は **enum で行う** (gain 値ではない、Codex P1 review 反映)。
//!   理由: -14 LUFS 近辺の動画は `gain_db = 0.0` = `gain = 1.0` で測定済みになるため、
//!   gain 値だけでは「未測定」と「測定済みで補正不要」が区別できない。

/// ノーマライズボタンの 5 状態。
///
/// 動画 1 本ごと (= fs_idx ごと) に App 側 `normalize_ui_states: HashMap<usize, _>`
/// で持つ。`Off` 以外はグローバル設定 ON が前提。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NormalizeUiState {
    /// グローバル設定が OFF。ボタンはグレー。クリックで ON 化 + DB lookup。
    Off,
    /// グローバル ON + DB ヒットで gain を適用済み。ボタンは強調色 (黄)。
    /// クリックで OFF 化。
    OnApplied {
        /// 適用中のゲイン (dB)。表示用 (0 dB ぴったりもありうる)。
        gain_db: f32,
    },
    /// 長尺動画の先頭側だけで仮 gain を適用済み。最終 scan はバックグラウンド継続中。
    /// DB には保存せず、確定値が届いたら `OnApplied` に遷移する。
    ProvisionalApplied {
        /// 適用中の仮ゲイン (dB)。表示用。
        gain_db: f32,
    },
    /// グローバル ON + DB ミス。gain は 1.0 (素通し) のまま、ボタンはオレンジ点滅。
    /// 左クリックでスキャン起動、右クリックでグローバル OFF 化 (救済経路)。
    OnUnmeasured,
    /// スキャン中の一時状態。ボタンは disable。ESC でキャンセル可。
    Scanning,
}

impl NormalizeUiState {
    /// 現在プレイヤーに適用中のゲイン (dB)。適用していない状態 (Off / Scanning / Unmeasured) は
    /// 0.0 (= 等倍)。スペクトラム鍵盤の明るさ反映などに使う。
    pub fn applied_gain_db(self) -> f32 {
        match self {
            NormalizeUiState::OnApplied { gain_db }
            | NormalizeUiState::ProvisionalApplied { gain_db } => gain_db,
            _ => 0.0,
        }
    }
}

impl Default for NormalizeUiState {
    fn default() -> Self {
        Self::Off
    }
}

/// スキャン進捗の即時 snapshot。App 側の atomic 構造体から `update` 毎に作られて
/// native overlay に渡される。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct NormalizeProgressSnapshot {
    pub pts_processed_ms: u64,
    pub duration_ms: u64,
    /// ライブストリーム / duration 不明動画では true (= 進捗バーではなくスピナー表示)。
    pub indeterminate: bool,
}

/// native overlay にまとめて渡す状態。`SetNormalizeOverlayState` command で送る。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct NormalizeOverlayState {
    pub ui_state: NormalizeUiState,
    /// `Scanning` 中のみ Some。それ以外では None。
    pub progress: Option<NormalizeProgressSnapshot>,
}

/// scanner が返す測定値。DB に保存する基本単位。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizeResult {
    /// 適用すべきゲイン (dB)。`±24dB` にクランプ済み + true_peak 制限済み。
    pub gain_db: f32,
    /// 元動画の integrated LUFS (BS.1770-4)。
    pub integrated_lufs: f32,
    /// 元動画の true peak (dBTP)。
    pub true_peak_db: f32,
    /// スキャン時のターゲット (LUFS の千分の一単位、整数)。
    /// DB の主キー条件に含まれるため、Settings 経由で値を変えると別エントリ扱いになる。
    pub target_lufs_milli: i32,
}
