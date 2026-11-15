//! 動画と音楽ビューが共有する、今の1本の表示値。DB/ファイルには触れない。

use std::sync::Arc;

use crate::transcript_db::{TranscriptSegment, TranscriptStatus};
use crate::transcript_display_cache::TranscriptDisplayValue;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TranscriptDataState {
    #[default]
    Missing,
    Loading,
    Failed,
    Stale,
    Available,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TranscriptActivity {
    #[default]
    None,
    Queued,
    Running,
}

/// 保存済みの本文と作成ジョブの案内は直交する。作成待ちへ入っただけで本文を消さない。
#[derive(Clone, Debug, Default)]
pub(crate) struct TranscriptSnapshot {
    pub key: String,
    pub state: TranscriptDataState,
    pub activity: TranscriptActivity,
    pub segments: Arc<Vec<TranscriptSegment>>,
    pub used_audio_stream: Option<usize>,
    pub applied_audio_stream: Option<usize>,
    pub created_at: Option<i64>,
}

impl TranscriptSnapshot {
    /// 要求中のトラックではなく、実際に再生しているトラックから毎回導く。
    pub fn different_track(&self) -> bool {
        matches!(
            self.state,
            TranscriptDataState::Available | TranscriptDataState::Stale
        ) && self.used_audio_stream != self.applied_audio_stream
    }

    /// 長い区間列を比較せず、native の最新値 command が必要かを調べる。
    pub fn same_presentation(&self, other: &Self) -> bool {
        self.key == other.key
            && self.state == other.state
            && self.activity == other.activity
            && (Arc::ptr_eq(&self.segments, &other.segments)
                || (self.segments.is_empty() && other.segments.is_empty()))
            && self.used_audio_stream == other.used_audio_stream
            && self.applied_audio_stream == other.applied_audio_stream
            && self.created_at == other.created_at
    }

    pub fn notice(&self) -> Option<String> {
        let mut notices = Vec::new();
        match self.activity {
            TranscriptActivity::Queued => notices.push("文字起こしの作成を待っています"),
            TranscriptActivity::Running => notices.push("文字起こしを作成しています"),
            TranscriptActivity::None => {}
        }
        match self.state {
            TranscriptDataState::Missing if self.activity == TranscriptActivity::None => {
                notices.push("文字起こしはまだありません。一覧の右クリックで作れます")
            }
            TranscriptDataState::Loading if self.activity == TranscriptActivity::None => {
                notices.push("文字起こしを読み込んでいます")
            }
            TranscriptDataState::Failed => {
                notices.push("文字起こしを表示できませんでした。作り直せます")
            }
            TranscriptDataState::Stale => {
                notices.push("文字起こしが古くなっています。作り直せます")
            }
            _ => {}
        }
        if self.different_track() {
            notices.push("別の音声トラックで作った文字起こしです");
        }
        (!notices.is_empty()).then(|| notices.join("。"))
    }

    pub fn summary(&self) -> String {
        let status = match self.activity {
            TranscriptActivity::Queued => "作成待ち",
            TranscriptActivity::Running => "作成中",
            TranscriptActivity::None => match self.state {
                TranscriptDataState::Missing => "なし",
                TranscriptDataState::Loading => "読み込み中",
                TranscriptDataState::Failed => "失敗",
                TranscriptDataState::Stale => "古い",
                TranscriptDataState::Available if self.different_track() => "別トラック",
                TranscriptDataState::Available => "あり",
            },
        };
        if status == "あり"
            && let Some(created_at) = self.created_at
        {
            // 説明文と同じローカル時刻変換を使い、この1行では日付だけを出す。
            let timestamp = crate::app::format_details_timestamp(created_at / 1000, false);
            let date = timestamp.split(' ').next().unwrap_or_default();
            if !date.is_empty() {
                return format!("文字起こし: あり ({date})");
            }
        }
        format!("文字起こし: {status}")
    }
}

/// snapshot 作成の正本。保存時刻はメディア時刻なので、再生metadataの原点を必要としない。
pub(crate) fn build_transcript_snapshot(
    key: String,
    value: Option<&TranscriptDisplayValue>,
    activity: TranscriptActivity,
    applied_audio_stream: Option<usize>,
) -> TranscriptSnapshot {
    let mut snapshot = TranscriptSnapshot {
        key,
        state: TranscriptDataState::Loading,
        activity,
        applied_audio_stream,
        ..Default::default()
    };
    let Some(value) = value else {
        return snapshot;
    };
    if value.read_failed {
        snapshot.state = TranscriptDataState::Failed;
        return snapshot;
    }
    let Some(row) = value.row.as_ref() else {
        snapshot.state = TranscriptDataState::Missing;
        return snapshot;
    };
    snapshot.used_audio_stream = row.audio_stream;
    snapshot.created_at = Some(row.created_at);
    if row.status == TranscriptStatus::Failed {
        snapshot.state = TranscriptDataState::Failed;
        return snapshot;
    }
    snapshot.segments = Arc::clone(&row.segments);
    let stale = value.is_stale();
    snapshot.state = if stale {
        TranscriptDataState::Stale
    } else {
        TranscriptDataState::Available
    };
    snapshot
}

#[cfg(test)]
#[path = "transcript_display/tests.rs"]
mod tests;
