//! Post-decode bass inference. The scope owner is the sole snapshot publisher.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use music_core::bass::{TimelineBassSegment, TimelineBassTrack, calculate_timeline_bass_segment};

const PUBLICATION_INTERVAL: Duration = Duration::from_millis(250);

struct BassPublisher {
    track: TimelineBassTrack,
    remaining: usize,
    last_publication: Duration,
}

impl BassPublisher {
    fn new(track: TimelineBassTrack) -> Self {
        Self {
            remaining: track.segment_count(),
            track,
            last_publication: Duration::ZERO,
        }
    }

    fn accept(
        &mut self,
        segment: TimelineBassSegment,
        elapsed: Duration,
    ) -> Option<Arc<TimelineBassTrack>> {
        if !self.track.merge_segment(segment) {
            return None;
        }
        self.remaining -= 1;
        if self.remaining == 0
            || elapsed.saturating_sub(self.last_publication) >= PUBLICATION_INTERVAL
        {
            self.last_publication = elapsed;
            Some(Arc::new(self.track.clone()))
        } else {
            None
        }
    }
}

/// Called only after successful decode and TimelineComplete publication, including LRU hits.
pub(super) fn run(
    pcm: &crate::ui_music_spectrum::MusicPcm,
    cancel: &AtomicBool,
    mut publish: impl FnMut(Arc<TimelineBassTrack>) -> bool,
) {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    pcm.with_prefix(|samples, rate| {
        let mut publisher = BassPublisher::new(TimelineBassTrack::new(samples.len() / 2, rate));
        if publisher.remaining == 0 || cancel.load(Ordering::Relaxed) {
            return;
        }
        let count = publisher.remaining;
        let next = AtomicUsize::new(0);
        let started = Instant::now();
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel();
            for worker in 0..threads {
                let tx = tx.clone();
                let next = &next;
                let spawned = std::thread::Builder::new()
                    .name(format!("miv-music-bass-{worker}"))
                    .spawn_scoped(scope, move || {
                        crate::similar_index::lower_current_thread_priority();
                        while !cancel.load(Ordering::Relaxed) {
                            let segment = next.fetch_add(1, Ordering::Relaxed);
                            if segment >= count {
                                break;
                            }
                            let Some(result) =
                                calculate_timeline_bass_segment(samples, rate, segment, &|| {
                                    cancel.load(Ordering::Relaxed)
                                })
                            else {
                                break;
                            };
                            if cancel.load(Ordering::Relaxed) || tx.send(result).is_err() {
                                break;
                            }
                        }
                    });
                if let Err(error) = spawned {
                    log::warn!("music bass worker could not start: {error}");
                }
            }
            drop(tx);
            for segment in rx {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(snapshot) = publisher.accept(segment, started.elapsed()) {
                    if cancel.load(Ordering::Relaxed) || !publish(snapshot) {
                        break;
                    }
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_core::bass::BassEstimate;

    #[test]
    fn music_bass_publication_reversed_completion_is_monotone_and_throttled() {
        // Four full segments. The extra frame needed for EOF rounding is in segment four.
        let track = TimelineBassTrack::new(3_999 * 960, 48_000);
        assert_eq!(track.segment_count(), 4);
        let mut publisher = BassPublisher::new(track);
        let mut previous: Option<Arc<TimelineBassTrack>> = None;
        for (number, millis) in [(2, 250), (0, 251), (3, 500), (1, 501)] {
            let segment = TimelineBassSegment {
                segment_index: number,
                estimates: vec![
                    BassEstimate {
                        midi: 40 + number as u8,
                        confidence: 0.75
                    };
                    1000
                ],
            };
            let published = publisher.accept(segment, Duration::from_millis(millis));
            if millis == 251 {
                assert!(published.is_none());
                continue;
            }
            let current = published.unwrap();
            if let Some(old) = previous {
                for frame in 0..4000 {
                    let time = frame as f64 * 0.02;
                    if let Some(value) = old.at_secs(time) {
                        assert_eq!(current.at_secs(time), Some(value));
                    }
                }
            }
            previous = Some(current);
        }
        assert_eq!(publisher.remaining, 0);
    }

    #[test]
    fn music_bass_cancelled_worker_never_publishes() {
        let pcm = crate::ui_music_spectrum::MusicPcm::with_capacity(48_000, 960);
        pcm.append(&vec![0.0; 1920]).unwrap();
        pcm.mark_complete();
        run(&pcm, &AtomicBool::new(true), |_| {
            panic!("cancelled publication")
        });
    }

    #[test]
    fn music_bass_cancel_after_first_snapshot_stops_partial_publication() {
        let pcm = crate::ui_music_spectrum::MusicPcm::with_capacity(48_000, 40 * 48_000);
        pcm.append(&vec![0.0; 40 * 48_000 * 2]).unwrap();
        pcm.mark_complete();
        let cancel = AtomicBool::new(false);
        let mut publications = 0;
        run(&pcm, &cancel, |snapshot| {
            publications += 1;
            assert!(snapshot.computed_frames() > 0);
            assert!(snapshot.computed_frames() < snapshot.frame_count());
            cancel.store(true, Ordering::Relaxed);
            true
        });
        assert_eq!(publications, 1);
    }

    #[test]
    fn music_bass_decode_success_orders_final_before_bass_and_lru_hit_still_computes() {
        let path = std::path::Path::new("testdata/audio-tail/tail30ms.wav");
        let meta = std::fs::metadata(path).unwrap();
        let hit_meta = Some((crate::ui_helpers::mtime_secs(&meta), meta.len() as i64));
        for want_analysis in [true, false] {
            let (tx, rx) = mpsc::channel();
            super::super::run_music_analysis(
                path,
                0,
                &AtomicBool::new(false),
                &tx,
                want_analysis,
                hit_meta,
            );
            drop(tx);
            let mut timeline = false;
            let mut bass = 0;
            for message in rx {
                match message {
                    super::super::MusicAnalysisMsg::TimelineComplete { .. } => {
                        assert_eq!(bass, 0);
                        timeline = true;
                    }
                    super::super::MusicAnalysisMsg::Bass(track) => {
                        assert_eq!(timeline, want_analysis);
                        assert!(track.at_secs(0.0).is_some());
                        bass += 1;
                    }
                    super::super::MusicAnalysisMsg::Timeline(Err(error)) => {
                        panic!("decode: {error}")
                    }
                    _ => {}
                }
            }
            assert_eq!(bass, 1);
            assert_eq!(timeline, want_analysis);
        }
    }

    #[test]
    fn music_bass_failed_or_cancelled_decode_does_not_start_bass() {
        for cancelled in [false, true] {
            let (tx, rx) = mpsc::channel();
            super::super::run_music_analysis(
                std::path::Path::new("testdata/no-such-music.wav"),
                0,
                &AtomicBool::new(cancelled),
                &tx,
                true,
                None,
            );
            drop(tx);
            for message in rx {
                assert!(!matches!(message, super::super::MusicAnalysisMsg::Bass(_)));
            }
        }
    }

    #[test]
    fn music_bass_poll_coalesces_final_and_four_snapshots_once_and_rejects_old_source() {
        use super::super::{FsCacheEntry, MusicAnalysisMsg, MusicAnalysisPending};
        let mut app = super::super::tests::phase_c_support::setup_app();
        let path = app.tmp.path().join("bass-poll.mkv");
        let mut player = crate::video::VideoPlayer::stream_ready_disconnected_for_test(path);
        player.set_opened_audio_stream_for_test(1, 1);
        app.fs_cache.insert(
            0,
            FsCacheEntry::Video {
                player: Box::new(player),
                load_seq: 1,
            },
        );
        let source = app.music_analysis_source_for_player(0).unwrap();
        app.music_analysis_source = Some(source.clone());
        let (tx, rx) = mpsc::channel();
        tx.send(MusicAnalysisMsg::TimelineComplete {
            analysis: music_core::TimelineAnalysis::default(),
            meta: Some((1, 1)),
        })
        .unwrap();
        let mut latest = None;
        for frames in [100, 200, 300, 400] {
            let bass = Arc::new(TimelineBassTrack::new(frames, 48_000));
            tx.send(MusicAnalysisMsg::Bass(bass.clone())).unwrap();
            latest = Some(bass);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        app.music_analysis_pending = Some(MusicAnalysisPending {
            source: source.clone(),
            cancel: cancel.clone(),
            rx,
        });
        let version = app.music_analysis_version;
        app.poll_music_analysis(&egui::Context::default());
        assert_eq!(app.music_analysis_version, version + 1);
        assert!(Arc::ptr_eq(
            app.music_bass.as_ref().unwrap(),
            latest.as_ref().unwrap()
        ));
        assert_eq!(app.music_analysis_lru.len(), 1);

        // Same path, different applied stream/load generation is an old source, not a bass update.
        for stale in ["stream", "load", "path"] {
            let mut old = source.clone();
            match stale {
                "stream" => old.stream_index += 1,
                "load" => old.load_seq += 1,
                _ => old.path = app.tmp.path().join("old.mkv"),
            }
            let (old_tx, old_rx) = mpsc::channel();
            old_tx
                .send(MusicAnalysisMsg::Bass(Arc::new(TimelineBassTrack::new(
                    1, 48_000,
                ))))
                .unwrap();
            let old_cancel = Arc::new(AtomicBool::new(false));
            app.music_analysis_pending = Some(MusicAnalysisPending {
                source: old,
                cancel: old_cancel.clone(),
                rx: old_rx,
            });
            app.poll_music_analysis(&egui::Context::default());
            assert!(old_cancel.load(Ordering::Relaxed));
            assert_eq!(app.music_analysis_version, version + 1);
            assert!(Arc::ptr_eq(
                app.music_bass.as_ref().unwrap(),
                latest.as_ref().unwrap()
            ));
        }
        app.clear_music_view_state();
        assert!(app.music_bass.is_none());
    }
}
