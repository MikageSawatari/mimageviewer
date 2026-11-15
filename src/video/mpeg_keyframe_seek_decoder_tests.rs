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
    use ffmpeg_the_third as ffmpeg;
    let fixture = Fixture::load("h264-small.ts");
    let target = 5.4;
    let expected = fixture.first_at_or_after(target - VIDEO_PREROLL_TRIM_TOLERANCE_SECS);
    let expected_id = expected.frame_id;
    let expected_pts = expected.media_secs;
    let mut input = ffmpeg::format::input(&fixture.path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    let video_index = stream.index();
    let tb = stream.time_base();
    let seconds_per_tick = tb.numerator() as f64 / tb.denominator() as f64;
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let audio_index = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .unwrap()
        .index();
    let timeline = MediaTimeline::from_input(&input);
    let raw_target = timeline.media_to_stream_ticks(target, tb);
    let mut reader = super::super::keyframe_seek::PacketReader::default();
    assert!(matches!(
        reader
            .seek(
                &mut input,
                -1,
                timeline.media_to_av_time_base(target),
                Some(video_index),
                || false
            )
            .unwrap(),
        super::super::keyframe_seek::SeekOutcome::Verified { .. }
    ));
    // Inspect/reconstitute the actual verified window through the one reading
    // mouth. The target video really follows held audio, with no old overflow.
    let mut held = VecDeque::new();
    loop {
        let (index, packet) = reader.next(&mut input).unwrap().unwrap();
        let end = index == video_index && packet.dts().is_some_and(|dts| dts > raw_target);
        held.push_back(packet);
        if end {
            break;
        }
    }
    let first_audio = held.iter().position(|p| p.stream() == audio_index).unwrap();
    let target_video = held
        .iter()
        .position(|p| p.stream() == video_index && p.pts().is_some_and(|pts| pts >= raw_target))
        .unwrap();
    assert!(first_audio < target_video);
    let mut reader = super::super::keyframe_seek::PacketReader::from_held_for_test(held);
    let clock = clock();
    clock.request_seek(target);
    let serial = clock.take_seek_request().unwrap().serial;
    let cancel = AtomicBool::new(false);
    let (video_tx, video_rx) = bounded(1);
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
                    trim_before_secs: Some(target),
                    frame_step: None
                }
            )),
            None,
            &cancel,
            false
        ),
        SeekFlushResult::Accepted
    );
    assert!(matches!(
        video_control_rx.recv().unwrap(),
        VideoControlMsg::Flush { .. }
    ));
    decoder.flush();
    let mut pending = VecDeque::new();
    let mut pending_bytes = 0;
    assert!(pending.is_empty());
    queue_held_seek_video(
        &mut reader,
        video_index,
        serial,
        timeline,
        seconds_per_tick,
        &mut pending,
        &mut pending_bytes,
    );
    let (index, packet) = reader.next(&mut input).unwrap().unwrap();
    assert_eq!(index, audio_index);
    audio_tx
        .send(AudioPacketMsg::Packet {
            serial,
            packet: ffmpeg::Packet::copy(&[0]),
        })
        .unwrap();
    let video_clock = Arc::clone(&clock);
    let consumer = std::thread::spawn(move || {
        'decode: loop {
            let VideoPacketMsg::Packet {
                serial: received,
                packet,
            } = video_rx.recv_timeout(Duration::from_secs(10)).unwrap()
            else {
                panic!("expected held video");
            };
            assert_eq!(received, serial);
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            let mut frame = ffmpeg::util::frame::video::Video::empty();
            while decoder.receive_frame(&mut frame).is_ok() {
                let ts = video_frame_timestamp(&frame).unwrap();
                let pts = timeline.raw_to_media_secs(ts as f64 * seconds_per_tick);
                if pts < target - VIDEO_PREROLL_TRIM_TOLERANCE_SECS {
                    continue;
                }
                let mut frame_id = 0;
                for bit in 0..16 {
                    if frame.data(0)[16 * frame.stride(0) + 4 + 6 * bit] >= 128 {
                        frame_id |= 1 << bit;
                    }
                }
                assert_eq!(frame_id, expected_id);
                assert!((pts - expected_pts).abs() < 1.0e-6);
                // Headless equivalent of accepting the first display frame.
                video_clock.clear_seek_target_override(serial);
                break 'decode;
            }
        }
        // Until the target frame was decoded/adopted, audio stays FULL.
        audio_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        audio_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    assert!(matches!(
        send_audio_packet_with_video_drain(
            &audio_tx,
            AudioPacketMsg::Packet { serial, packet },
            &clock,
            &cancel,
            &mut pending,
            &mut pending_bytes,
            &video_tx
        ),
        DemuxPacketSend::Sent
    ));
    consumer.join().unwrap();
    assert!(
        !clock.is_seeking(),
        "target display adoption did not progress"
    );
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
        let session = Session::open(fixture.path.clone(), clock);
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
