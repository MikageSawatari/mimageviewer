//! Real demux/video/audio workers, with synthetic fixtures and no window/device.

use super::*;
use crate::video::audio_track_selection::{AudioTrackRequestOutcome, AudioTrackSelection};
use crate::video::engine::EngineEvent;
use crate::video::engine::actor::state_code;
use crate::video::engine::state::DecoderEvent;
use crate::video::keyframe_seek_test_support::{Fixture, OracleFrame, assert_frame_id};
use crate::video::{EngineEventSender, VideoUiWake};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

struct Session {
    handles: DecodeHandles,
    clock: Arc<AvClock>,
    cancel: Arc<AtomicBool>,
    info: VideoInfo,
    selection: Arc<AudioTrackSelection>,
    events: crossbeam_channel::Receiver<EngineEvent>,
    skipped: Arc<AtomicU64>,
}

impl Session {
    fn open(path: PathBuf, clock: Arc<AvClock>) -> Self {
        ffmpeg_the_third::init().unwrap();
        clock.set_playing(true);
        // Decode against a clock past fixture EOF rather than wall time.
        clock.set_audio_pts_jump(100.0);
        let cancel = Arc::new(AtomicBool::new(false));
        let skipped = Arc::new(AtomicU64::new(0));
        let (event_tx, events) = bounded(256);
        let handles = spawn(
            path,
            Arc::clone(&clock),
            Arc::clone(&cancel),
            48_000,
            false,
            crate::settings::VideoDeinterlaceMode::Off,
            #[cfg(windows)]
            None,
            // Preserve every decoded frame under the existing lossless output
            // backpressure. No presenter or audio device consumes these lanes.
            Arc::new(AtomicU8::new(state_code::BUFFERING)),
            EngineEventSender::new(event_tx, Arc::new(VideoUiWake::default())),
            Arc::clone(&skipped),
            Arc::new(VideoDynamicState::default()),
        );
        let info = handles
            .info_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let selection = handles
            .audio_track_selection_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        Self {
            handles,
            clock,
            cancel,
            info,
            selection,
            events,
            skipped,
        }
    }

    fn drain(&self, serial: u64) -> (Vec<VideoFrame>, Vec<AudioFrame>) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut video = Vec::new();
        let mut audio = Vec::new();
        loop {
            while let Ok(frame) = self.handles.video_rx.try_recv() {
                if frame.seek_serial == serial {
                    video.push(frame);
                }
            }
            while let Ok(frame) = self.handles.audio_rx.try_recv() {
                if frame.seek_serial == serial {
                    audio.push(frame);
                }
            }
            if !video.is_empty() {
                // Stand in for the presenter's successful first-frame adoption,
                // then keep decoding headlessly instead of pacing in real time.
                self.clock.clear_seek_target_override(serial);
                self.clock.set_audio_pts_jump(100.0);
            }
            if self.clock.video_decoded_eos(serial)
                && self.clock.audio_decoded_eos(serial)
                && self.clock.is_demux_exhausted()
                && self.handles.video_rx.is_empty()
                && self.handles.audio_rx.is_empty()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "serial={serial} failed to drain: video={} audio={} last_v={:?} last_a={:?}",
                video.len(),
                audio.len(),
                video.last().map(|frame| frame.pts_secs),
                audio.last().map(|frame| frame.pts_secs),
            );
            crossbeam_channel::select! {
                recv(self.handles.video_rx) -> frame => {
                    if let Ok(frame) = frame {
                        if frame.seek_serial == serial { video.push(frame); }
                    }
                }
                recv(self.handles.audio_rx) -> frame => {
                    if let Ok(frame) = frame {
                        if frame.seek_serial == serial { audio.push(frame); }
                    }
                }
                default(Duration::from_millis(5)) => {}
            }
        }
        assert_eq!(self.skipped.load(Ordering::Acquire), 0);
        (video, audio)
    }

    fn assert_only_seek_completed(&self, wanted: u64, obsolete: u64) {
        let mut saw_wanted = false;
        for event in self.events.try_iter() {
            if let EngineEvent::Decoder(DecoderEvent::SeekCompleted { epoch, .. }) = event {
                assert_ne!(epoch, obsolete, "aborted seek was reported completed");
                saw_wanted |= epoch == wanted;
            }
        }
        assert!(saw_wanted, "latest seek completion missing for {wanted}");
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn clock() -> Arc<AvClock> {
    Arc::new(AvClock::new(1.0, Arc::new(AtomicU64::new(0))))
}

#[test]
fn mpeg_keyframe_seek_held_target_video_passes_a_full_audio_lane_with_no_old_pending() {
    // Synthetic packet routing, no windows/device: held audio precedes every
    // target video packet and the audio queue cannot drain until video arrives.
    let clock = clock();
    clock.request_seek(5.0);
    let request = clock.take_seek_request().unwrap();
    let serial = request.serial;
    let cancel = AtomicBool::new(false);
    let packet = |stream, value| {
        let mut packet = ffmpeg_the_third::Packet::copy(&[value]);
        packet.set_stream(stream);
        packet.set_pts(Some(i64::from(value)));
        packet
    };
    let mut reader = super::super::keyframe_seek::PacketReader::from_held_for_test(
        [packet(1, 1), packet(0, 2), packet(1, 3), packet(0, 4)].into(),
    );
    let mut pending = VecDeque::new();
    let mut pending_bytes = 0;
    let (video_tx, video_rx) = bounded(2);
    let (audio_tx, audio_rx) = bounded(1);
    let (video_ctl, video_control_rx) = bounded(1);
    let mut demux_serial = DemuxSerial(0);
    assert_eq!(
        demux_serial.send_seek_flushes(
            serial,
            Some((
                &video_ctl,
                VideoControlMsg::Flush {
                    serial,
                    trim_before_secs: Some(5.0),
                    frame_step: None
                }
            )),
            None,
            &cancel,
            false
        ),
        SeekFlushResult::Accepted
    );
    assert!(
        matches!(video_control_rx.recv().unwrap(), VideoControlMsg::Flush { serial: received, .. } if received == serial)
    );
    assert!(pending.is_empty());
    queue_held_seek_video(
        &mut reader,
        0,
        serial,
        MediaTimeline::default(),
        1.0,
        &mut pending,
        &mut pending_bytes,
    );
    assert_eq!(pending.len(), 2);
    assert_eq!(pending_bytes, 2);
    let fixture = Fixture::load("h264-small.ts");
    let mut input = ffmpeg_the_third::format::input(&fixture.path).unwrap();
    let (audio_stream, held_audio) = reader.next(&mut input).unwrap().unwrap();
    assert_eq!(audio_stream, 1);
    assert_eq!(held_audio.data().unwrap(), &[1]);
    audio_tx
        .send(AudioPacketMsg::Packet {
            serial,
            packet: packet(1, 0),
        })
        .unwrap();
    let guard = std::thread::spawn(move || {
        for expected in [2, 4] {
            let VideoPacketMsg::Packet {
                serial: received,
                packet,
            } = video_rx.recv_timeout(Duration::from_secs(2)).unwrap()
            else {
                panic!("expected video packet");
            };
            assert_eq!(received, serial);
            assert_eq!(packet.data().unwrap(), [expected]);
        }
        // Audio cannot accept the next packet until ALL target video was sent.
        audio_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        audio_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    });
    let result = send_audio_packet_with_video_drain(
        &audio_tx,
        AudioPacketMsg::Packet {
            serial,
            packet: held_audio,
        },
        &clock,
        &cancel,
        &mut pending,
        &mut pending_bytes,
        &video_tx,
    );
    assert!(matches!(result, DemuxPacketSend::Sent));
    assert!(pending.is_empty());
    assert_eq!(pending_bytes, 0);
    let (audio_stream, remaining_audio) = reader.next(&mut input).unwrap().unwrap();
    assert_eq!(audio_stream, 1);
    assert_eq!(remaining_audio.data().unwrap(), &[3]);
    guard.join().unwrap();
}

fn assert_video(frame: &VideoFrame, expected: &OracleFrame, name: &str) {
    let VideoFrameData::Cpu(rgba) = &frame.data else {
        panic!("headless SW test requires a CPU frame");
    };
    assert_frame_id(rgba, frame.width, frame.height, expected.frame_id, name);
    assert!(
        (frame.pts_secs - expected.media_secs).abs() < 1.0e-6,
        "{name}: PTS {} != {}",
        frame.pts_secs,
        expected.media_secs,
    );
}

#[test]
fn mpeg_keyframe_seek_held_video_decodes_target_before_buffering_audio_can_drain() {
    // Use the real demux, both real decoders and their normal queue capacities.
    // Pausing the audio worker at an actually-full PCM output models Buffering
    // backpressure/scheduling: it cannot consume old audio packets during seek.
    // No test code calls queue_held_seek_video or injects/reorders source packets.
    let fixture = Fixture::load("long-gop.ts");
    let target = 17.4;
    let clock = clock();
    let (audio_full, resume_audio) = clock.gate_full_audio_output_for_test();
    let (demux_blocked, resume_demux) = clock.gate_demux_audio_backpressure_for_test();
    let session = Session::open(fixture.path.clone(), clock);
    let (first_frame_tx, first_frame_rx) = bounded(1);
    let video_rx = session.handles.video_rx.clone();
    let consumer_cancel = Arc::clone(&session.cancel);
    let consumer = std::thread::spawn(move || {
        while !consumer_cancel.load(Ordering::Acquire) {
            match video_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(frame) if frame.seek_serial != 0 => {
                    let _ = first_frame_tx.try_send(frame);
                }
                Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    audio_full.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        session.handles.audio_rx.is_full(),
        "PCM output must remain full"
    );
    assert_eq!(
        demux_blocked.recv_timeout(Duration::from_secs(10)).unwrap(),
        0
    );
    let seek_observation = session.clock.observe_next_demux_seek_for_test();
    session.clock.request_seek(target);
    let serial = session.clock.current_seek_serial();
    resume_demux.send(()).unwrap();
    let observed = seek_observation
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert_eq!(observed.serial, serial);
    assert_eq!(
        observed.pending_video_packets, 0,
        "old overflow must not rescue video"
    );
    assert_eq!(observed.audio_packet_queue_len, AUDIO_PACKET_QUEUE_CAP);
    assert!(matches!(
        observed.outcome,
        Some(super::super::keyframe_seek::SeekOutcome::Verified { .. })
    ));
    // Audio has not been received/released anywhere. Only production preload
    // can expose the retained video behind the full old audio packet queue.
    let frame = first_frame_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("real demux did not deliver target video while PCM output stayed full");
    assert_eq!(frame.seek_serial, serial);
    assert_video(
        &frame,
        fixture.first_at_or_after(target - VIDEO_PREROLL_TRIM_TOLERANCE_SECS),
        &fixture.name,
    );
    assert!(session.handles.audio_rx.is_full());
    session.clock.clear_seek_target_override(serial);
    assert!(
        !session.clock.is_seeking(),
        "target display adoption did not progress"
    );
    session.cancel.store(true, Ordering::Release);
    // Unblock the deliberately parked worker before observing real demux cleanup.
    drop(resume_audio);
    assert!(matches!(
        session
            .handles
            .info_rx
            .recv_timeout(Duration::from_secs(10)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    ));
    consumer.join().unwrap();
}

fn assert_audio(audio: &[AudioFrame], target: f64, stream_index: usize, name: &str) {
    assert!(audio.len() > 2, "{name}: not enough post-seek audio");
    assert!(
        (audio[0].pts_secs - target).abs() <= 1.0 / 48_000.0,
        "{name}: first audio {} != requested trim {target}",
        audio[0].pts_secs,
    );
    for frame in audio {
        assert_eq!(frame.stream_index, stream_index, "{name}");
        assert_eq!(frame.seek_target_secs, Some(target), "{name}");
    }
    for pair in audio.windows(2) {
        let expected = pair[0].pts_secs + pair[0].duration_secs;
        assert!(
            (pair[1].pts_secs - expected).abs() <= 1.0 / 48_000.0 + 1.0e-8,
            "{name}: audio discontinuity {} -> {} (expected {expected})",
            pair[0].pts_secs,
            pair[1].pts_secs,
        );
    }
}

/// Finish the actual decoded tail through VideoPlayer's headless tick/EOF path.
/// PCM and video were consumed by Session::drain instead of a device/presenter;
/// acknowledge those tails, then inspect the production published EOF position.
fn assert_tail_eof_position(
    session: &mut Session,
    fixture: &Fixture,
    video: &[VideoFrame],
    audio: &[AudioFrame],
) {
    use crate::video::engine::{actor::EngineActor, state::AudioEvent};
    let serial = session.clock.current_seek_serial();
    let oracle: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .path
                .with_file_name(format!("{}.json", fixture.name)),
        )
        .unwrap(),
    )
    .unwrap();
    let expected_duration = oracle["format"]["duration_us"].as_i64().unwrap() as f64 / 1_000_000.0;
    assert!((session.info.duration_secs - expected_duration).abs() < 0.00001);
    let mut player = crate::video::VideoPlayer::disconnected_for_test(fixture.path.clone(), 0.0);
    player.clock = Arc::clone(&session.clock);
    player.cancel = Arc::clone(&session.cancel);
    player.decode = std::mem::replace(&mut session.handles, crate::video::dummy_decode_handles());
    player.info = Some(session.info.clone());
    let mut engine = EngineActor::new(
        crate::video::OpenOptions {
            autoplay: true,
            ..Default::default()
        },
        session.clock.seek_serial_for_test(),
        Arc::clone(&session.clock),
    );
    engine.begin_loading();
    engine.handle_decoder_event(DecoderEvent::InfoReceived {
        epoch: serial,
        duration_secs: session.info.duration_secs,
        has_audio: true,
        has_video: true,
    });
    engine.handle_decoder_event(DecoderEvent::SeekCompleted {
        epoch: serial,
        actual_pts: audio[0].pts_secs,
    });
    engine.handle_decoder_event(DecoderEvent::FirstFrameReady {
        epoch: serial,
        pts: video[0].pts_secs,
    });
    engine.handle_audio_event(AudioEvent::BufferReady {
        epoch: serial,
        pts: audio[0].pts_secs,
        wall_now: Instant::now(),
    });
    engine.handle_audio_event(AudioEvent::AudioRendered {
        epoch: serial,
        pts: audio.last().unwrap().pts_secs + audio.last().unwrap().duration_secs,
        wall_now: Instant::now(),
    });
    assert_eq!(engine.published_state_code(), state_code::PLAYING);
    player.engine_state_atomic = engine.published_state_handle();
    player.engine = Arc::new(std::sync::Mutex::new(engine));
    player.last_seen_seek_serial = serial;
    player.set_last_displayed_pts_for_test(video.last().unwrap().pts_secs);
    session.clock.clear_seek_target_override(serial);
    session.clock.reset_audio_bookkeeping_only();
    assert!(session.clock.complete_video_tail(serial));
    assert!(session.clock.begin_audio_tail(serial));
    assert!(session.clock.complete_audio_tail(serial));
    assert!(player.decode.video_rx.is_empty() && player.decode.audio_rx.is_empty());
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(1);
    while player.engine_state_code() != state_code::EOF {
        assert!(
            Instant::now() < deadline,
            "{} did not publish EOF",
            fixture.name
        );
        player.tick(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!player.is_playing());
    assert_eq!(session.clock.current_seek_serial(), serial);
    assert!(
        (player.position() - expected_duration).abs() < 0.00001,
        "{} EOF position {} != media duration {}",
        fixture.name,
        player.position(),
        expected_duration
    );
    assert!((session.clock.now_secs() - expected_duration).abs() < 0.00001);
}

#[test]
fn mpeg_keyframe_seek_playback_stop_during_verification_aborts_and_joins_workers() {
    for name in ["h264-small.ts", "mpeg2-missing-pts.mpg"] {
        let fixture = Fixture::load(name);
        let clock = clock();
        let observation = clock.observe_next_demux_seek_for_test();
        let (entered, release) = clock.gate_demux_seek_for_test(3);
        clock.request_seek(5.4);
        let serial = clock.current_seek_serial();
        let session = Session::open(fixture.path, clock);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        session.cancel.store(true, Ordering::Release);
        release.send(()).unwrap();
        let observed = observation.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(observed.serial, serial);
        assert_eq!(
            observed.outcome,
            Some(super::super::keyframe_seek::SeekOutcome::Aborted)
        );
        // info_tx's last owner goes away only after run_decoder's cleanup joins
        // both real workers. Merely setting cancel is not sufficient evidence.
        assert!(matches!(
            session
                .handles
                .info_rx
                .recv_timeout(Duration::from_secs(10)),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected)
        ));
        assert!(session.clock.audio_worker_exited());
        assert!(session.handles.video_rx.is_empty());
        assert!(session.handles.audio_rx.is_empty());
        assert!(matches!(
            session.handles.video_rx.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        ));
        assert!(matches!(
            session.handles.audio_rx.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        ));
        assert!(!session.clock.is_demux_exhausted());
        assert!(!session.clock.video_decoded_eos(serial));
        assert!(!session.clock.audio_decoded_eos(serial));
        assert!(!session.events.try_iter().any(|event| matches!(event, EngineEvent::Decoder(DecoderEvent::SeekCompleted { epoch, .. }) if epoch == serial)));
    }
}

#[test]
fn mpeg_keyframe_seek_playback_frame_ids_audio_trim_and_tail_eof() {
    for name in [
        "h264-small.ts",
        "mpeg2-large.ts",
        "hevc-open.ts",
        "mpeg2-missing-pts.mpg",
    ] {
        let fixture = Fixture::load(name);
        let clock = clock();
        let target = fixture.frames[129].media_secs - 0.007;
        clock.request_seek(target);
        let mut session = Session::open(fixture.path.clone(), clock);
        let serial = session.clock.current_seek_serial();
        let (video, audio) = session.drain(serial);
        let expected = fixture.first_at_or_after(target - VIDEO_PREROLL_TRIM_TOLERANCE_SECS);
        assert_video(video.first().expect("post-seek video"), expected, name);
        assert_audio(
            &audio,
            target,
            session.info.opened_audio_stream_index.unwrap(),
            name,
        );
        assert_video(video.last().unwrap(), fixture.frames.last().unwrap(), name);

        let target = session.info.duration_secs - 0.3;
        session.clock.request_seek(target);
        let serial = session.clock.current_seek_serial();
        let (video, audio) = session.drain(serial);
        assert_video(
            video.first().expect("D - 0.3 seek video"),
            fixture.first_at_or_after(target - VIDEO_PREROLL_TRIM_TOLERANCE_SECS),
            name,
        );
        assert_video(video.last().unwrap(), fixture.frames.last().unwrap(), name);
        assert_audio(
            &audio,
            target,
            session.info.opened_audio_stream_index.unwrap(),
            name,
        );
        assert_tail_eof_position(&mut session, &fixture, &video, &audio);
    }
}

#[test]
fn mpeg_keyframe_seek_playback_frame_step_before_after_and_final_delayed_frame() {
    for name in [
        "h264-small.ts",
        "mpeg2-large.ts",
        "hevc-open.ts",
        "mpeg2-missing-pts.mpg",
    ] {
        let fixture = Fixture::load(name);
        for (base_index, direction, expected_index) in [
            (150, 1, 151),
            (152, -1, 151),
            (fixture.frames.len() - 2, 1, fixture.frames.len() - 1),
        ] {
            let base = fixture.frames[base_index].media_secs;
            let clock = clock();
            clock.request_frame_step_seek((base - 1.0).max(0.0), base, direction);
            let session = Session::open(fixture.path.clone(), clock);
            let serial = session.clock.current_seek_serial();
            let (video, _) = session.drain(serial);
            assert_video(
                video.first().expect("frame-step output"),
                &fixture.frames[expected_index],
                name,
            );
        }
    }
}

#[test]
fn mpeg_keyframe_seek_playback_supersede_after_read_discards_old_completion_and_packets() {
    for name in [
        "h264-small.ts",
        "mpeg2-large.ts",
        "hevc-open.ts",
        "mpeg2-missing-pts.mpg",
    ] {
        let fixture = Fixture::load(name);
        let clock = clock();
        let (entered, release) = clock.gate_demux_seek_for_test(3);
        clock.request_seek(4.2);
        let obsolete = clock.current_seek_serial();
        let session = Session::open(fixture.path.clone(), clock);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let target = fixture.frames[219].media_secs - 0.007;
        session.clock.request_seek(target);
        let latest = session.clock.current_seek_serial();
        release.send(()).unwrap();
        let (video, audio) = session.drain(latest);
        assert_video(
            video.first().expect("latest seek video"),
            fixture.first_at_or_after(target - VIDEO_PREROLL_TRIM_TOLERANCE_SECS),
            name,
        );
        assert_audio(
            &audio,
            target,
            session.info.opened_audio_stream_index.unwrap(),
            name,
        );
        session.assert_only_seek_completed(latest, obsolete);
    }
}

#[test]
fn mpeg_keyframe_seek_playback_supersede_before_new_request_publication() {
    let fixture = Fixture::load("mpeg2-large.ts");
    let clock = clock();
    let (entered, release) = clock.gate_demux_seek_for_test(3);
    clock.request_seek(4.2);
    let obsolete = clock.current_seek_serial();
    let session = Session::open(fixture.path.clone(), clock);
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let target = fixture.frames[219].media_secs - 0.007;
    session
        .clock
        .request_seek_before_publish_for_test(target, || {
            release.send(()).unwrap();
            // Keep publication withheld while the older seek resumes. No decoder
            // output or completion may be adopted under either serial in this gap.
            assert!(
                session
                    .events
                    .recv_timeout(Duration::from_millis(100))
                    .is_err()
            );
            assert!(session.handles.video_rx.is_empty());
            assert!(session.handles.audio_rx.is_empty());
            assert!(!session.clock.peek_seek_request_pending());
        });
    let latest = session.clock.current_seek_serial();
    let (video, audio) = session.drain(latest);
    assert_video(
        video.first().unwrap(),
        fixture.first_at_or_after(target),
        &fixture.name,
    );
    assert_audio(
        &audio,
        target,
        session.info.opened_audio_stream_index.unwrap(),
        &fixture.name,
    );
    session.assert_only_seek_completed(latest, obsolete);
}

#[test]
fn mpeg_keyframe_seek_playback_audio_switch_supersede_discards_prepared_setup() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/audio-tracks/multi.mkv");
    let session = Session::open(path, clock());
    session.drain(0);
    let initial = session.selection.snapshot().applied.stream_index;
    let alternatives = session
        .info
        .audio_tracks
        .iter()
        .filter(|track| track.stream_index != initial)
        .map(|track| track.stream_index)
        .collect::<Vec<_>>();
    assert!(alternatives.len() >= 2);
    let (entered, release) = session.clock.gate_demux_seek_for_test(0);
    assert_eq!(
        session.selection.request(alternatives[0]),
        AudioTrackRequestOutcome::Accepted
    );
    session.clock.request_seek(2.2);
    let obsolete = session.clock.current_seek_serial();
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(session.selection.snapshot().applied.stream_index, initial);
    assert_eq!(
        session.selection.request(alternatives[1]),
        AudioTrackRequestOutcome::Accepted
    );
    let target = 3.2;
    session.clock.request_seek(target);
    let latest = session.clock.current_seek_serial();
    release.send(()).unwrap();
    let (video, audio) = session.drain(latest);
    assert!(!video.is_empty());
    assert_audio(&audio, target, alternatives[1], "audio switch supersede");
    let snapshot = session.selection.snapshot();
    assert_eq!(snapshot.applied, snapshot.desired);
    assert_eq!(snapshot.applied.stream_index, alternatives[1]);
    assert!(snapshot.last_failure.is_none());
    session.assert_only_seek_completed(latest, obsolete);
}
