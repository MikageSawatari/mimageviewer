use super::*;
use image::AnimationDecoder;

fn options(format: DepthExportFormat) -> DepthExportOptions {
    DepthExportOptions {
        format,
        long_edge: 720,
        motion: MotionKind::Circle,
        loops: 2,
    }
}
fn frame(size: [u32; 2], index: u32) -> Result<Vec<u8>, String> {
    Ok((0..size[0] * size[1])
        .flat_map(|p| [((p + index) % 256) as u8, 128, 64, 255])
        .collect())
}

#[test]
fn depth_export_animation_roundtrip_counts_delays_cycle_dimensions_and_loop() {
    let dir = tempfile::tempdir().unwrap();
    for format in [DepthExportFormat::Gif, DepthExportFormat::Apng] {
        for period in [1., 3., 3.7, 10.] {
            let options = options(format);
            let output = dir.path().join(format!("{period}.{}", format.extension()));
            let cancel = AtomicBool::new(false);
            encode_frames(
                &output,
                &options,
                period,
                [32, 18],
                &cancel,
                |i, _| frame([32, 18], i),
                |_, _| {},
            )
            .unwrap();
            let expected = options.cycle_frames(period);
            if format == DepthExportFormat::Gif {
                let bytes = std::fs::read(&output).unwrap();
                assert!(bytes.windows(11).any(|b| b == b"NETSCAPE2.0"));
                let start = bytes.windows(11).position(|b| b == b"NETSCAPE2.0").unwrap() + 11;
                assert_eq!(&bytes[start..start + 5], &[3, 1, 0, 0, 0]);
                let frames = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(
                    std::fs::File::open(&output).unwrap(),
                ))
                .unwrap()
                .into_frames()
                .collect_frames()
                .unwrap();
                assert_eq!(frames.len(), expected as usize);
                let mut ms = 0;
                for (index, f) in frames.iter().enumerate() {
                    assert_eq!(f.buffer().dimensions(), (32, 18));
                    let (numer, denom) = f.delay().numer_denom_ms();
                    assert_eq!(denom, 1);
                    assert_eq!(
                        numer,
                        u32::from(delay_ticks(index as u32, expected, period, 100)) * 10
                    );
                    assert!([60, 70].contains(&numer));
                    ms += numer;
                }
                assert_eq!(ms, (period * 100.).round() as u32 * 10);
            } else {
                let mut reader = png::Decoder::new(std::io::BufReader::new(
                    std::fs::File::open(&output).unwrap(),
                ))
                .read_info()
                .unwrap();
                let control = reader.info().animation_control.unwrap();
                assert_eq!(control.num_frames, expected);
                assert_eq!(control.num_plays, 0);
                let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
                let mut ms = 0;
                for index in 0..expected {
                    let info = reader.next_frame(&mut pixels).unwrap();
                    assert_eq!((info.width, info.height), (32, 18));
                    let control = reader.info().frame_control.unwrap();
                    assert_eq!(control.delay_den, 1000);
                    assert_eq!(
                        control.delay_num,
                        delay_ticks(index, expected, period, 1000)
                    );
                    ms += u32::from(control.delay_num);
                }
                assert_eq!(ms, (period * 1000.).round() as u32);
            }
        }
    }
}

#[test]
fn depth_export_cancel_removes_each_partial_and_preserves_destination() {
    let dir = tempfile::tempdir().unwrap();
    for format in [
        DepthExportFormat::Gif,
        DepthExportFormat::Apng,
        DepthExportFormat::Mp4,
    ] {
        let output = dir.path().join(format!("cancel.{}", format.extension()));
        std::fs::write(&output, b"existing file").unwrap();
        let cancel = AtomicBool::new(false);
        let result = encode_frames(
            &output,
            &options(format),
            1.,
            [32, 18],
            &cancel,
            |i, _| {
                if i == 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
                frame([32, 18], i)
            },
            |_, _| {},
        );
        assert!(
            matches!(result, Err(ExportFailure::Cancelled)),
            "{format:?}: {result:?}"
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"existing file");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        std::fs::remove_file(output).unwrap();
    }
}

#[test]
fn depth_export_dimensions_rotate_crop_and_round_mp4_even() {
    let uv = egui::Rect::from_min_max(egui::pos2(0.25, 0.), egui::pos2(0.75, 1.));
    let mut opt = options(DepthExportFormat::Gif);
    assert_eq!(opt.dimensions([1920, 1080], uv, 0), [640, 720]);
    assert_eq!(opt.dimensions([1920, 1080], uv, 1), [720, 640]);
    assert_eq!(
        opt.dimensions(
            [63, 47],
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            0
        ),
        [63, 47]
    );
    opt.format = DepthExportFormat::Mp4;
    assert_eq!(
        opt.dimensions(
            [63, 47],
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            0
        ),
        [62, 46]
    );
}

#[test]
fn depth_export_job_drop_cancels_and_cleans_each_format_without_ui_wait() {
    let dir = tempfile::tempdir().unwrap();
    for format in [
        DepthExportFormat::Gif,
        DepthExportFormat::Apng,
        DepthExportFormat::Mp4,
    ] {
        let output = dir.path().join(format!("drop.{}", format.extension()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let path = output.clone();
        let job = DepthExportJob::spawn(
            egui::Context::default(),
            egui::ViewportId::ROOT,
            output.clone(),
            move |cancel, progress| {
                let result = encode_frames(
                    &path,
                    &options(format),
                    1.,
                    [32, 18],
                    cancel,
                    |index, _| {
                        if index == 1 {
                            entered_tx.send(()).unwrap();
                            resume_rx.recv().unwrap();
                        }
                        frame([32, 18], index)
                    },
                    progress,
                );
                finished_tx
                    .send(matches!(result, Err(ExportFailure::Cancelled)))
                    .unwrap();
                result
            },
        )
        .unwrap();
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        drop(job);
        resume_tx.send(()).unwrap();
        assert!(
            finished_rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap()
        );
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn depth_export_mp4_roundtrip_has_even_size_two_cycles_and_cfr_timestamps() {
    use ffmpeg_the_third as ffmpeg;
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("cycles.mp4");
    let options = options(DepthExportFormat::Mp4);
    let size = options.dimensions(
        [65, 49],
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        0,
    );
    assert_eq!(size, [64, 48]);
    encode_frames(
        &output,
        &options,
        1.,
        size,
        &AtomicBool::new(false),
        |index, cycle| {
            assert_eq!(cycle, 30);
            frame(size, index % cycle)
        },
        |_, _| {},
    )
    .unwrap();
    let mut input = ffmpeg::format::input(&output).unwrap();
    assert_eq!(input.streams().len(), 1);
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    assert_eq!(stream.avg_frame_rate(), ffmpeg::Rational(30, 1));
    let time_base = f64::from(stream.time_base());
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    assert_eq!((decoder.width(), decoder.height()), (64, 48));
    let index = stream.index();
    let mut timestamps = Vec::new();
    let mut drain = |decoder: &mut ffmpeg::decoder::Video| {
        let mut frame = ffmpeg::util::frame::video::Video::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            timestamps.push(frame.timestamp().unwrap());
        }
    };
    for packet in input.packets() {
        let (stream, packet) = packet.unwrap();
        if stream.index() == index {
            decoder.send_packet(&packet).unwrap();
            drain(&mut decoder);
        }
    }
    decoder.send_eof().unwrap();
    drain(&mut decoder);
    assert_eq!(timestamps.len(), 60);
    for (index, pts) in timestamps.into_iter().enumerate() {
        assert!((pts as f64 * time_base - index as f64 / 30.).abs() < 0.0001);
    }
    assert!((input.duration() as f64 / 1_000_000. - 2.).abs() < 0.01);
}
