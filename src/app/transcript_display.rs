//! 共通 snapshot と native への軽量配信。UI はメモリと reader の要求 channel だけを扱う。

use super::App;
use crate::fs_animation::FsCacheEntry;
use crate::transcript_display::{TranscriptSnapshot, build_transcript_snapshot};

#[derive(Clone, Default)]
struct TranscriptVisit {
    key: Option<String>,
    frame: u64,
}

impl TranscriptVisit {
    fn observe(&mut self, key: &str, frame: u64) -> bool {
        let refresh = self.key.as_deref() != Some(key)
            || (frame != self.frame && frame != self.frame.saturating_add(1));
        self.key = Some(key.to_owned());
        self.frame = frame;
        refresh
    }
}

impl App {
    /// 動画 presenter と音楽ビューの唯一の snapshot 作成入口。要求中の音声は参照しない。
    pub(crate) fn transcript_snapshot_for_player(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> TranscriptSnapshot {
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return TranscriptSnapshot::default();
        };
        let path = player.path().clone();
        let key = crate::path_key::normalize_keep_drive(&path);
        let applied = player.applied_audio_stream_index();
        #[cfg(windows)]
        let activity = self
            .transcript_job
            .as_ref()
            .map(|job| job.display_activity(&key))
            .unwrap_or_default();
        #[cfg(not(windows))]
        let activity = crate::transcript_display::TranscriptActivity::None;
        let visit_id = egui::Id::new((
            "transcript_display_visit",
            self.virtual_list_context_id(),
            ctx.viewport_id(),
        ));
        let frame = ctx.cumulative_frame_nr();
        let refresh = ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<TranscriptVisit>(visit_id)
                .observe(&key, frame)
        });
        if let Err(error) = self.transcript_display.request(ctx, &key, &path, refresh) {
            self.show_feedback_warning(format!("文字起こしを読み込めませんでした: {error}"));
        }
        build_transcript_snapshot(
            key.clone(),
            self.transcript_display.get(&key),
            activity,
            applied,
        )
    }

    /// marker の変更判定や音楽モードの gate に従属させない。
    #[cfg(windows)]
    pub(crate) fn sync_native_transcript_snapshot(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let snapshot = self.transcript_snapshot_for_player(ctx, fs_idx);
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_transcript_snapshot(snapshot);
        }
    }
}

#[cfg(test)]
#[path = "transcript_display_tests.rs"]
mod tests;
