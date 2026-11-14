//! Noninteractive regression tests through the actual demux/decode lanes.
use super::*;
use crate::video::engine::actor::state_code;
use crate::video::{EngineEventSender, VideoUiWake};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/timeline-origin")
        .join(name)
}

struct Session {
    handles: DecodeHandles,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    info: VideoInfo,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn start(name: &str, step: Option<(f64, i32)>) -> Session {
    ffmpeg_the_third::init().unwrap();
    let clock = Arc::new(AvClock::new(1.0, Arc::new(AtomicU64::new(0))));
    clock.set_playing(true);
    // This is decoder verification, with no device or wall-clock playback.
    clock.set_audio_pts_jump(100.0);
    if let Some((base, direction)) = step {
        clock.request_frame_step_seek((base - 1.0).max(0.0), base, direction);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, _rx) = bounded(64);
    let handles = spawn(
        fixture(name),
        Arc::clone(&clock),
        Arc::clone(&cancel),
        48_000,
        false,
        if name == "interlaced-origin.ts" {
            crate::settings::VideoDeinterlaceMode::On
        } else {
            crate::settings::VideoDeinterlaceMode::Off
        },
        #[cfg(windows)]
        None,
        Arc::new(AtomicU8::new(if step.is_some() {
            state_code::SEEKING
        } else {
            state_code::PLAYING
        })),
        EngineEventSender::new(tx, Arc::new(VideoUiWake::default())),
        Arc::new(AtomicU64::new(0)),
        Arc::new(VideoDynamicState::default()),
    );
    let info = handles
        .info_rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    Session {
        handles,
        clock,
        cancel,
        info,
    }
}

fn assert_colour(frame: &VideoFrame, second: usize) {
    // Palette from the lavfi generator. MPEG-2 YUV conversion is lossy.
    const COLOURS: [[u8; 3]; 12] = [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
        [0, 0, 0],
        [255, 128, 0],
        [128, 0, 255],
        [0, 128, 128],
        [128, 128, 128],
    ];
    let VideoFrameData::Cpu(bytes) = &frame.data else {
        panic!("software frame required")
    };
    let offset = ((frame.height / 2 * frame.width + frame.width / 2) * 4) as usize;
    let actual = &bytes[offset..offset + 3];
    let expected = COLOURS[second];
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) < 25),
        "PTS {} second {second}: {actual:?} != {expected:?}",
        frame.pts_secs
    );
}

fn drain(session: &Session, serial: u64, min_pts: f64) -> (Vec<VideoFrame>, Vec<AudioFrame>) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut video = Vec::new();
    let mut audio = Vec::new();
    loop {
        while let Ok(frame) = session.handles.video_rx.try_recv() {
            if frame.seek_serial == serial && frame.pts_secs >= min_pts {
                video.push(frame);
            }
        }
        while let Ok(frame) = session.handles.audio_rx.try_recv() {
            if frame.seek_serial == serial && frame.pts_secs >= min_pts {
                audio.push(frame);
            }
        }
        if session.clock.video_decoded_eos(serial)
            && session.clock.is_demux_exhausted()
            && session.handles.video_rx.is_empty()
            && session.handles.audio_rx.is_empty()
            && audio
                .last()
                .is_some_and(|f| f.pts_secs + f.duration_secs > session.info.duration_secs - 0.1)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "decode did not drain: video={} audio={} last_v={:?} last_a={:?}",
            video.len(),
            audio.len(),
            video.last().map(|f| f.pts_secs),
            audio.last().map(|f| f.pts_secs)
        );
        crossbeam_channel::select! {
            recv(session.handles.video_rx) -> frame => {
                if let Ok(frame) = frame { if frame.seek_serial == serial && frame.pts_secs >= min_pts { video.push(frame); } }
            }
            recv(session.handles.audio_rx) -> frame => {
                if let Ok(frame) = frame { if frame.seek_serial == serial && frame.pts_secs >= min_pts { audio.push(frame); } }
            }
            default(Duration::from_millis(5)) => {}
        }
    }
    (video, audio)
}

#[test]
fn timeline_origin_ts_decoder_first_frames_seek_tail_colour_and_eof() {
    for name in [
        "small-origin.ts",
        "large-origin.ts",
        "wrap-origin.ts",
        "interlaced-origin.ts",
        "late-audio-origin.ts",
    ] {
        let session = start(name, None);
        assert!(session.info.timeline_origin_secs.abs() > 1.0, "{name}");
        let (video, audio) = drain(&session, 0, -1.0);
        assert!(
            (video[0].pts_secs - 0.021333).abs() < 0.002,
            "{name}: {}",
            video[0].pts_secs
        );
        assert!(
            audio[0].pts_secs.abs() < 0.03,
            "{name}: {}",
            audio[0].pts_secs
        );
        assert_colour(&video[0], 0);
        assert_colour(video.last().unwrap(), 11);
        assert!((video.last().unwrap().pts_secs - session.info.duration_secs).abs() < 0.1);
        let target = session.info.duration_secs - 0.5;
        session.clock.request_seek(target);
        let serial = session.clock.current_seek_serial();
        let deadline = Instant::now() + Duration::from_secs(10);
        let selected = loop {
            while session.handles.audio_rx.try_recv().is_ok() {}
            if let Ok(frame) = session
                .handles
                .video_rx
                .recv_timeout(Duration::from_millis(10))
            {
                if frame.seek_serial == serial {
                    break frame;
                }
            }
            assert!(Instant::now() < deadline, "no tail seek frame for {name}");
        };
        assert!(
            (selected.pts_secs - target).abs() < 0.05,
            "{name}: {} != {target}",
            selected.pts_secs
        );
        assert_colour(&selected, 11);
    }
}

#[test]
fn timeline_origin_bwdif_frame_step_uses_media_before_and_after_filter() {
    for (base, direction, expected) in [
        (5.021333333, 1, 5.061333333),
        (5.101333333, -1, 5.061333333),
        (11.941333333, 1, 11.981333333),
    ] {
        let session = start("interlaced-origin.ts", Some((base, direction)));
        let serial = session.clock.current_seek_serial();
        let deadline = Instant::now() + Duration::from_secs(10);
        let selected = loop {
            while session.handles.audio_rx.try_recv().is_ok() {}
            if let Ok(frame) = session
                .handles
                .video_rx
                .recv_timeout(Duration::from_millis(10))
            {
                if frame.seek_serial == serial {
                    break frame;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no frame step from {base} direction {direction}"
            );
        };
        assert!(
            (selected.pts_secs - expected).abs() < 0.002,
            "{} != {expected}",
            selected.pts_secs
        );
        assert_colour(&selected, expected.floor() as usize);
    }
}

#[test]
fn timeline_origin_late_track_metadata_and_availability_are_media() {
    use crate::video::audio_track_selection::audio_track_available_at;
    let session = start("late-audio-origin.ts", None);
    assert_eq!(session.info.audio_tracks.len(), 2);
    let late = &session.info.audio_tracks[1];
    assert!((late.start_secs.unwrap() - 4.0).abs() < 0.002);
    assert!(!audio_track_available_at(late, 2.0));
    assert!(audio_track_available_at(late, 6.0));
    // TS stream duration is estimated (5.824s), not the decoded six-second span.
    assert!((9.5..10.1).contains(&late.end_secs.unwrap()));
}

#[test]
fn timeline_origin_positive_mkv_chapters_and_mp3_keep_container_zero() {
    let session = start("positive-origin.mkv", None);
    assert_eq!(session.info.timeline_origin_secs, 0.0);
    assert!((session.info.duration_secs - 22.0).abs() < 0.01);
    assert_eq!(
        session
            .info
            .chapters
            .iter()
            .map(|c| c.start_secs)
            .collect::<Vec<_>>(),
        vec![10.0, 14.0, 18.0]
    );
    let first = session
        .handles
        .video_rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!((first.pts_secs - 10.0).abs() < 0.001);
    assert_colour(&first, 0);
    let mp3 = start("encoder-delay.mp3", None);
    assert_eq!(mp3.info.timeline_origin_secs, 0.0);
    let first = mp3
        .handles
        .audio_rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    // pkt_timebase/encoder-delay PTS handling is intentionally unchanged.
    assert!(
        (first.pts_secs - 0.0).abs() < 0.000001,
        "{}",
        first.pts_secs
    );
}

fn player_for_info(info: &VideoInfo, position: f64) -> crate::video::VideoPlayer {
    use crate::video::engine::state::{AudioEvent, DecoderEvent};
    let mut player =
        crate::video::VideoPlayer::stream_ready_disconnected_for_test(fixture("large-origin.ts"));
    player.info = Some(info.clone());
    player.audio_track_selection = Some(Arc::new(crate::video::AudioTrackSelection::new(
        info.opened_audio_stream_index.unwrap(),
    )));
    {
        let mut actor = player.engine.lock().unwrap();
        actor.begin_loading();
        actor.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: info.duration_secs,
            has_audio: true,
            has_video: true,
        });
        actor.handle_audio_event(AudioEvent::BufferReady {
            epoch: 0,
            pts: position,
            wall_now: Instant::now(),
        });
        actor.handle_decoder_event(DecoderEvent::FirstFrameReady {
            epoch: 0,
            pts: position,
        });
        actor.apply_command(crate::video::engine::actor::TransportCommand::Play);
    }
    player.set_last_displayed_pts_for_test(position);
    player.clock.set_audio_pts_jump(position);
    player
}

#[test]
fn timeline_origin_player_seek_relative_and_engine_eof_use_media_duration() {
    use crate::video::engine::state::DecoderEvent;
    use crate::video::{RelativeSeekOutcome, SeekOrigin};
    let session = start("large-origin.ts", None);
    let duration = session.info.duration_secs;
    let player = player_for_info(&session.info, 0.0);
    player.seek(duration - 1.0, SeekOrigin::User);
    assert!(
        (player.clock.take_seek_request().unwrap().target_secs - (duration - 1.0)).abs() < 0.001
    );
    let player = player_for_info(&session.info, duration - 2.0);
    assert_eq!(
        player.seek_relative(0.5, SeekOrigin::User),
        RelativeSeekOutcome::Seeked
    );
    assert!(
        (player.clock.take_seek_request().unwrap().target_secs - (duration - 1.5)).abs() < 0.03
    );
    let player = player_for_info(&session.info, duration - 0.05);
    player
        .engine
        .lock()
        .unwrap()
        .handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: duration,
        });
    assert!(player.is_at_eof());
    assert!((player.position() - duration).abs() < 1e-6);
    assert!(!player.clock.is_playing());
}

#[test]
fn timeline_origin_player_defers_late_audio_and_adopts_it_inside_media_range() {
    use crate::video::AudioTrackSelectOutcome;
    let session = start("late-audio-origin.ts", None);
    let player = player_for_info(&session.info, 2.0);
    assert_eq!(
        player.select_audio_track(2).outcome,
        AudioTrackSelectOutcome::Deferred
    );
    assert!(player.clock.take_seek_request().is_none());
    let player = player_for_info(&session.info, 6.0);
    assert_eq!(
        player.select_audio_track(2).outcome,
        AudioTrackSelectOutcome::Requested
    );
    assert!((player.clock.take_seek_request().unwrap().target_secs - 6.0).abs() < 0.03);
}
