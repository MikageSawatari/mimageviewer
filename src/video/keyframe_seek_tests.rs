use super::*;
use crate::video::keyframe_seek_test_support::{Fixture, WORKER_FIXTURES};
use ffmpeg::util::frame::video::Video;

fn open(fixture: &Fixture) -> (Input, usize, ffmpeg::Rational, ffmpeg::decoder::Video) {
    ffmpeg::init().unwrap();
    let input = ffmpeg::format::input(&fixture.path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    let index = stream.index();
    let tb = stream.time_base();
    let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    (input, index, tb, decoder)
}

fn oracle(fixture: &Fixture) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(
            fixture
                .path
                .with_file_name(format!("{}.json", fixture.name)),
        )
        .unwrap(),
    )
    .unwrap()
}

fn id(frame: &Video) -> u32 {
    // Source fixture is gray-neutral YUV420P; read the encoded luma directly.
    assert_eq!((frame.width(), frame.height()), (96, 64));
    let mut result = 0;
    for bit in 0..16 {
        if frame.data(0)[16 * frame.stride(0) + 4 + 6 * bit] >= 128 {
            result |= 1 << bit;
        }
    }
    result
}

fn decode(
    reader: &mut PacketReader,
    input: &mut Input,
    index: usize,
    decoder: &mut ffmpeg::decoder::Video,
) -> Vec<(u32, i64)> {
    let mut frames = Vec::new();
    loop {
        let draining = match reader.next(input) {
            Some(Ok((stream, packet))) => {
                if stream != index {
                    continue;
                }
                if decoder.send_packet(&packet).is_err() {
                    continue;
                }
                false
            }
            Some(Err(error)) => panic!("read failed: {error}"),
            None => {
                decoder.send_eof().unwrap();
                true
            }
        };
        loop {
            let mut frame = Video::empty();
            if decoder.receive_frame(&mut frame).is_err() {
                break;
            }
            if let Some(ts) = crate::video::decoder::video_frame_timestamp(&frame) {
                frames.push((id(&frame), ts));
            }
        }
        if draining {
            break;
        }
    }
    frames
}

#[test]
fn real_material_seek_contains_the_oracle_display_frame_at_every_boundary() {
    for name in WORKER_FIXTURES
        .iter()
        .chain(["pts-delayed.ts", "short-key-gap.ts"].iter())
    {
        let fixture = Fixture::load(name);
        let table = oracle(&fixture);
        let (input, _, tb, _) = open(&fixture);
        let timeline = crate::video::media_timeline::MediaTimeline::from_input(&input);
        let first = fixture.frames.first().unwrap().media_secs;
        let last = fixture.frames.last().unwrap().media_secs;
        let mut targets = (0..=(last * 10.0).ceil() as usize)
            .map(|v| timeline.media_to_stream_ticks(v as f64 / 10.0, tb))
            .collect::<Vec<_>>();
        targets.extend(
            [first - 0.1, first + 0.1, last, last + 0.2, last + 2.0]
                .map(|t| timeline.media_to_stream_ticks(t, tb)),
        );
        for packet in table["packets"].as_array().unwrap() {
            if packet["key"] != true {
                continue;
            }
            for key in ["pts", "dts"] {
                if let Some(raw) = packet[key].as_i64() {
                    // Key PTS/DTS exact ticks never round-trip through f64.
                    targets.push(raw);
                    if packet["pts"].is_null() {
                        targets.extend([raw - 1, raw + 1]);
                    }
                }
            }
        }
        targets.sort_unstable();
        targets.dedup();
        for raw in targets {
            // Every boundary is a cold seek: a preceding full decode must not
            // supply the search index which this regression is meant to test.
            let (mut input, index, _, mut decoder) = open(&fixture);
            let mut reader = PacketReader::default();
            let target = timeline
                .raw_to_media_secs(raw as f64 * tb.numerator() as f64 / tb.denominator() as f64);
            let outcome = match reader.seek(&mut input, index as i32, raw, Some(index), || false) {
                Ok(outcome) => outcome,
                Err(error) if *name == "control.mp4" => {
                    // A Direct format may reject a locator before its first
                    // indexed key. Preserve and compare that released result.
                    let (mut baseline, _, _, _) = open(&fixture);
                    let ret = unsafe {
                        ffmpeg::ffi::av_seek_frame(
                            baseline.as_mut_ptr(),
                            index as i32,
                            raw,
                            ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
                        )
                    };
                    assert!(ret < 0);
                    assert_eq!(error, ffmpeg::Error::from(ret));
                    continue;
                }
                Err(error) => panic!("{name} target={raw}: {error}"),
            };
            assert_ne!(outcome, SeekOutcome::Unverified, "{name} target={target}");
            if *name == "control.mp4" {
                assert_eq!(outcome, SeekOutcome::Direct);
            }
            if *name != "control.mp4" && target <= first {
                assert_eq!(outcome, SeekOutcome::StartOfStream);
            }
            decoder.flush();
            let frames = decode(&mut reader, &mut input, index, &mut decoder);
            let expected = table["frames"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|f| f["best_effort_timestamp"].as_i64().unwrap() <= raw)
                .unwrap_or(&table["frames"][0]);
            let expected_id = expected["frame_id"].as_u64().unwrap() as u32;
            let expected_ts = expected["best_effort_timestamp"].as_i64().unwrap();
            assert!(
                frames
                    .iter()
                    .any(|&(frame_id, timestamp)| frame_id == expected_id
                        && timestamp == expected_ts),
                "{name} target={target:.9} raw={raw} outcome={outcome:?} expected=({expected_id},{expected_ts}) decoded={frames:?}",
            );
            if matches!(outcome, SeekOutcome::StartOfStream) {
                assert_eq!(frames[0].0, fixture.frames[0].frame_id, "{name} head");
            }
        }
    }
}

#[test]
fn ps_missing_pts_really_uses_b_and_decodes_the_independent_first_key() {
    let fixture = Fixture::load("mpeg2-missing-pts.mpg");
    let table = oracle(&fixture);
    let case = &table["special_targets"]["missing_pts_b"];
    let target = case["target_ticks"].as_i64().unwrap();
    let (mut input, index, _, mut decoder) = open(&fixture);
    let mut reader = PacketReader::default();
    assert_eq!(
        reader
            .seek(&mut input, index as i32, target, Some(index), || false)
            .unwrap(),
        SeekOutcome::Verified { back: 1 }
    );
    let mut keys = WindowKeys::default();
    for packet in &reader.held {
        if packet.stream() == index {
            keys.observe(packet.is_key(), packet.pts(), packet.dts(), target);
        }
    }
    assert!(!keys.at_or_before);
    assert!(!keys.known_pts);
    assert!(keys.count >= 2);
    decoder.flush();
    let frames = decode(&mut reader, &mut input, index, &mut decoder);
    let expected = &case["first_key_decoded"];
    let expected_id = expected["frame_id"].as_u64().unwrap() as u32;
    let frame = frames.iter().find(|(id, _)| *id == expected_id).unwrap();
    assert_eq!(frame.1, expected["best_effort_timestamp"].as_i64().unwrap());
    assert!(frame.1 <= target);
}

#[test]
fn long_gop_doubles_backoff_and_metrics_measure_each_material_and_abort() {
    for name in WORKER_FIXTURES
        .iter()
        .chain(["pts-delayed.ts", "short-key-gap.ts"].iter())
    {
        let fixture = Fixture::load(name);
        let (mut input, index, _, _) = open(&fixture);
        let timeline = crate::video::media_timeline::MediaTimeline::from_input(&input);
        let mut reader = PacketReader::default();
        let target = if *name == "long-gop.ts" { 17.4 } else { 5.4 };
        let result = reader
            .seek(
                &mut input,
                -1,
                timeline.media_to_av_time_base(target),
                Some(index),
                || false,
            )
            .unwrap();
        if *name == "long-gop.ts" {
            assert!(matches!(result, SeekOutcome::Verified { back: 4.. }));
            assert!(reader.metrics.attempts >= 3);
        }
        eprintln!(
            "MPEG_SEEK_METRIC {name} target={target} result={result:?} {:?}",
            reader.metrics
        );
        let mut checks = 0;
        let mut cancelled_at = None;
        let result = reader
            .seek(
                &mut input,
                -1,
                timeline.media_to_av_time_base(target),
                Some(index),
                || {
                    checks += 1;
                    // Inject immediately AFTER a successful poll: the caller
                    // cannot observe it until one more FFmpeg call returns.
                    // On TS/PS this includes av_read_frame; Direct includes
                    // av_seek_frame. Measure the call as well as cleanup/log.
                    if cancelled_at.is_some() {
                        return true;
                    }
                    if checks == if *name == "control.mp4" { 1 } else { 3 } {
                        cancelled_at = Some(Instant::now());
                    }
                    false
                },
            )
            .unwrap();
        assert_eq!(result, SeekOutcome::Aborted, "{name}");
        assert!(reader.held.is_empty());
        eprintln!(
            "MPEG_SEEK_CANCEL {name} latency_ms={:.6}",
            cancelled_at.unwrap().elapsed().as_secs_f64() * 1000.0
        );
    }
}

#[test]
fn small_budgets_fall_back_in_the_original_large_origin_units() {
    let fixture = Fixture::load("mpeg2-large.ts");
    for budget in [
        Budget {
            held_bytes: 0,
            read_bytes: u64::MAX,
        },
        Budget {
            held_bytes: usize::MAX,
            read_bytes: 0,
        },
    ] {
        let (mut input, index, _, _) = open(&fixture);
        let timeline = crate::video::media_timeline::MediaTimeline::from_input(&input);
        let locator = timeline.media_to_av_time_base(5.4);
        let mut reader = PacketReader::default();
        assert_eq!(
            reader
                .seek_with_budget(&mut input, -1, locator, Some(index), || false, budget)
                .unwrap(),
            SeekOutcome::Unverified
        );
        assert!(reader.held.is_empty());
        let mut first = Packet::empty();
        first.read(&mut input).unwrap();
        // Compare the actual fallback with the original FFI call on this same
        // Input, whose search index has the same preceding probe history.
        unsafe {
            assert!(
                ffmpeg::ffi::av_seek_frame(
                    input.as_mut_ptr(),
                    -1,
                    locator,
                    ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32
                ) >= 0
            );
        }
        let mut expected = Packet::empty();
        expected.read(&mut input).unwrap();
        assert_eq!(fingerprint(&first), fingerprint(&expected));
        assert!(
            first.dts().unwrap() > 3_240_000_000,
            "must preserve the 36000s origin"
        );
    }
}

fn fingerprint(
    packet: &Packet,
) -> (
    usize,
    Option<i64>,
    Option<i64>,
    i32,
    i64,
    isize,
    Vec<u8>,
    Vec<(String, Vec<u8>)>,
) {
    (
        packet.stream(),
        packet.pts(),
        packet.dts(),
        packet.flags().bits(),
        packet.duration(),
        packet.position(),
        packet.data().unwrap_or_default().to_vec(),
        packet
            .side_data()
            .map(|data| (format!("{:?}", data.kind()), data.data().to_vec()))
            .collect(),
    )
}

#[test]
fn reader_replays_only_the_successful_window_and_preserves_side_data_and_input_independence() {
    for name in ["h264-small.ts", "mpeg2-missing-pts.mpg", "long-gop.ts"] {
        let fixture = Fixture::load(name);
        let (mut input, index, tb, _) = open(&fixture);
        let (mut reference, _, _, _) = open(&fixture);
        let timeline = crate::video::media_timeline::MediaTimeline::from_input(&input);
        let target =
            timeline.media_to_stream_ticks(if name == "long-gop.ts" { 17.4 } else { 5.4 }, tb);
        let mut reader = PacketReader::default();
        let SeekOutcome::Verified { back } = reader
            .seek(&mut input, index as i32, target, Some(index), || false)
            .unwrap()
        else {
            panic!("must verify");
        };
        // Reproduce only FFmpeg seeks/reads, with no helper or key judgement.
        // Earlier failed attempts must not leak into the returned packet stream.
        for seconds in BACKOFF_SECONDS
            .into_iter()
            .take_while(|&seconds| seconds <= back)
        {
            let delta = unsafe {
                ffmpeg::ffi::av_rescale_q(
                    i64::from(seconds),
                    ffmpeg::ffi::AVRational { num: 1, den: 1 },
                    tb.into(),
                )
            };
            unsafe {
                assert!(
                    ffmpeg::ffi::av_seek_frame(
                        reference.as_mut_ptr(),
                        index as i32,
                        target - delta,
                        ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32
                    ) >= 0
                );
            }
            if seconds == back {
                break;
            }
            loop {
                let mut packet = Packet::empty();
                if packet.read(&mut reference).is_err() {
                    break;
                }
                if packet.stream() == index && packet.dts().is_some_and(|dts| dts > target) {
                    break;
                }
            }
        }
        let mut other_reader = PacketReader::default();
        let (mut other, other_index, _, _) = open(&fixture);
        other_reader
            .seek(
                &mut other,
                -1,
                timeline.media_to_av_time_base(0.1),
                Some(other_index),
                || false,
            )
            .unwrap();
        other_reader.clear(); // must not affect reader or reference
        let mut packets = 0;
        loop {
            let actual = reader.next(&mut input);
            let mut expected = Packet::empty();
            let status = expected.read(&mut reference);
            match (actual, status) {
                (None, Err(ffmpeg::Error::Eof)) => break,
                (Some(Ok((_, actual))), Ok(())) => {
                    assert_eq!(
                        fingerprint(&actual),
                        fingerprint(&expected),
                        "{name} packet {packets}"
                    );
                    packets += 1;
                }
                _ => panic!("packet read mismatch: {name}"),
            }
        }
        assert!(packets > 0);
    }
}

#[test]
fn known_delayed_pts_does_not_accept_a_window_on_key_count_alone() {
    let fixture = Fixture::load("pts-delayed.ts");
    let table = oracle(&fixture);
    let target = table["special_targets"]["known_pts_after_target"]["target_ticks"]
        .as_i64()
        .unwrap();
    let (mut input, index, _, _) = open(&fixture);
    let mut reader = PacketReader::default();
    let result = reader
        .seek(&mut input, index as i32, target, Some(index), || false)
        .unwrap();
    assert!(
        !matches!(result, SeekOutcome::Verified { back: 1 }),
        "known future PTS cannot use B"
    );
}

#[test]
fn short_key_gap_terminating_key_is_replayed_but_excluded_from_j() {
    let fixture = Fixture::load("short-key-gap.ts");
    let table = oracle(&fixture);
    let case = &table["special_targets"]["window_end_key_is_not_j"];
    let target = case["target_ticks"].as_i64().unwrap();
    let (mut input, index, _, _) = open(&fixture);
    let mut reader = PacketReader::default();
    assert!(matches!(
        reader
            .seek(&mut input, index as i32, target, Some(index), || false)
            .unwrap(),
        SeekOutcome::Verified { .. }
    ));
    let end = reader.held.back().unwrap();
    assert_eq!(end.stream(), index);
    assert!(end.is_key());
    assert_eq!(end.dts(), case["end_key_dts"].as_i64());
    let mut keys = WindowKeys::default();
    keys.observe(end.is_key(), end.pts(), end.dts(), target);
    assert_eq!(keys.count, 0);
    assert!(!keys.verified(true));
}

#[test]
fn direct_mp4_and_no_video_keep_original_seek_and_reject_invalid_indices_safely() {
    for (name, video) in [("control.mp4", true), ("h264-small.ts", false)] {
        let fixture = Fixture::load(name);
        let (mut input, index, _, _) = open(&fixture);
        let (mut reference, _, _, _) = open(&fixture);
        let timeline = crate::video::media_timeline::MediaTimeline::from_input(&input);
        let locator = timeline.media_to_av_time_base(5.4);
        let mut reader = PacketReader::default();
        assert_eq!(
            reader
                .seek(&mut input, -1, locator, video.then_some(index), || false)
                .unwrap(),
            SeekOutcome::Direct
        );
        assert!(reader.held.is_empty());
        assert_eq!(reader.metrics.attempts, 1);
        unsafe {
            assert!(
                ffmpeg::ffi::av_seek_frame(
                    reference.as_mut_ptr(),
                    -1,
                    locator,
                    ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32
                ) >= 0
            );
        }
        for _ in 0..12 {
            let (_, actual) = reader.next(&mut input).unwrap().unwrap();
            let mut expected = Packet::empty();
            expected.read(&mut reference).unwrap();
            assert_eq!(fingerprint(&actual), fingerprint(&expected));
        }
        assert_eq!(
            reader.seek(&mut input, 99999, locator, None, || false),
            Err(ffmpeg::Error::from(-22))
        );
        assert_eq!(reader.metrics.attempts, 0);
        assert_eq!(
            reader
                .seek(&mut input, 99999, locator, None, || true)
                .unwrap(),
            SeekOutcome::Aborted
        );
        assert_eq!(
            reader
                .seek(&mut input, -1, locator, video.then_some(index), || true)
                .unwrap(),
            SeekOutcome::Aborted
        );
        assert_eq!(reader.metrics.attempts, 0);
    }
}

#[test]
fn missing_stream_start_rescales_format_start_and_missing_both_uses_direct() {
    let fixture = Fixture::load("mpeg2-large.ts");
    let (mut input, index, _, _) = open(&fixture);
    let timeline = crate::video::media_timeline::MediaTimeline::from_input(&input);
    let locator = timeline.media_to_av_time_base(5.4);
    // SAFETY: test exclusively owns this Input. Mutate only missing metadata,
    // not pointers or packet/index ownership.
    unsafe {
        (**(*input.as_mut_ptr()).streams.add(index)).start_time = ffmpeg::ffi::AV_NOPTS_VALUE;
    }
    let mut reader = PacketReader::default();
    assert!(matches!(
        reader
            .seek(&mut input, -1, locator, Some(index), || false)
            .unwrap(),
        SeekOutcome::Verified { .. }
    ));
    unsafe {
        (*input.as_mut_ptr()).start_time = ffmpeg::ffi::AV_NOPTS_VALUE;
    }
    assert_eq!(
        reader
            .seek(&mut input, -1, locator, Some(index), || false)
            .unwrap(),
        SeekOutcome::Direct
    );
    assert!(reader.held.is_empty());
    assert_eq!(reader.metrics.attempts, 1);
}
