use std::sync::atomic::Ordering;

use ffmpeg_the_third as ffmpeg;

use super::*;

fn options() -> VideoOnlyMp4Options {
    VideoOnlyMp4Options {
        width: 128,
        height: 72,
        fps: 30,
        // Stress OpenH264's bitrate budget: offline export must never skip a frame.
        video_bitrate_bps: 32_000,
        encoder: EncoderPreference::OpenH264,
        replace_existing: false,
    }
}

fn pixels() -> Vec<u8> {
    [33, 127, 219, 255].repeat(128 * 72)
}

#[test]
fn video_only_mp4_roundtrip_preserves_frames_dimensions_duration_and_faststart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("video.mp4");
    let cancel = AtomicBool::new(false);
    let mut writer = VideoOnlyMp4Writer::new(&path, options()).unwrap();
    let mut rgba = pixels();
    for frame in 0..90_u32 {
        for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
            // Deterministic high-frequency changes to exercise frame retention under load.
            let value = (index as u32)
                .wrapping_mul(1_664_525)
                .wrapping_add(frame.wrapping_mul(1_013_904_223));
            pixel[..3].copy_from_slice(&value.to_le_bytes()[..3]);
        }
        writer.write_rgba(&rgba, &cancel).unwrap();
    }
    writer.finish(&cancel).unwrap();
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);

    let mut input = ffmpeg::format::input(&path).unwrap();
    assert_eq!(input.streams().count(), 1, "no extra or audio stream");
    assert!(input.streams().best(ffmpeg::media::Type::Audio).is_none());
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    assert_eq!(stream.parameters().id(), ffmpeg::codec::Id::H264);
    let rate = stream.avg_frame_rate();
    assert_eq!((rate.numerator(), rate.denominator()), (30, 1));
    let time_base = stream.time_base();
    let duration = stream.duration() as f64 * f64::from(time_base);
    assert!((duration - 3.0).abs() < 1.0 / 30.0, "duration={duration}");
    assert_eq!(stream.frames(), 90);
    let index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    assert_eq!((decoder.width(), decoder.height()), (128, 72));
    let mut frames = 0;
    let mut decoded = ffmpeg::util::frame::video::Video::empty();
    let mut drain = |decoder: &mut ffmpeg::codec::decoder::Video| {
        loop {
            match decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    frames += 1;
                    assert_eq!((decoded.width(), decoded.height()), (128, 72));
                    assert_eq!(decoded.color_space(), ffmpeg::color::Space::BT709);
                    assert_eq!(decoded.color_range(), ffmpeg::color::Range::MPEG);
                }
                Err(ffmpeg::Error::Other { errno: 11 } | ffmpeg::Error::Eof) => break,
                Err(error) => panic!("decode: {error}"),
            }
        }
    };
    for packet in input.packets() {
        let (stream, packet) = packet.unwrap();
        assert_eq!(stream.index(), index);
        decoder.send_packet(&packet).unwrap();
        drain(&mut decoder);
    }
    decoder.send_eof().unwrap();
    drain(&mut decoder);
    assert_eq!(frames, 90);

    let bytes = std::fs::read(&path).unwrap();
    let mut offset = 0;
    let mut boxes = Vec::new();
    while offset < bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        assert!(size >= 8, "small test export uses ordinary top-level boxes");
        boxes.push(bytes[offset + 4..offset + 8].to_vec());
        offset += size;
        assert!(offset <= bytes.len());
    }
    let moov = boxes.iter().position(|kind| kind == b"moov").unwrap();
    let mdat = boxes.iter().position(|kind| kind == b"mdat").unwrap();
    assert!(moov < mdat, "faststart places moov before media data");
}

#[test]
fn video_only_cancel_and_drop_remove_partial_and_preserve_existing_destination() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("existing.mp4");
    std::fs::write(&path, b"original").unwrap();
    let options = VideoOnlyMp4Options {
        replace_existing: true,
        ..options()
    };
    let cancel = AtomicBool::new(false);
    {
        let mut writer = VideoOnlyMp4Writer::new(&path, options).unwrap();
        writer.write_rgba(&pixels(), &cancel).unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(writer.finish(&cancel), Err(ExportError::Cancelled));
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    cancel.store(false, Ordering::Relaxed);
    {
        let mut writer = VideoOnlyMp4Writer::new(&path, options).unwrap();
        writer.write_rgba(&pixels(), &cancel).unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn video_only_rejects_mismatched_frames_and_empty_export_without_publishing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("video.mp4");
    let cancel = AtomicBool::new(false);
    let mut writer = VideoOnlyMp4Writer::new(&path, options()).unwrap();
    assert!(matches!(
        writer.write_rgba(&[0; 4], &cancel),
        Err(ExportError::InvalidRequest(_))
    ));
    assert!(matches!(
        writer.finish(&cancel),
        Err(ExportError::InvalidRequest(_))
    ));
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn video_only_options_reject_invalid_geometry_rate_and_bitrate() {
    assert!(matches!(
        VideoOnlyMp4Writer::new(Path::new("animation.gif"), options()),
        Err(ExportError::InvalidRequest(_))
    ));
    for options in [
        VideoOnlyMp4Options {
            width: 127,
            ..options()
        },
        VideoOnlyMp4Options {
            height: 0,
            ..options()
        },
        VideoOnlyMp4Options {
            fps: 0,
            ..options()
        },
        VideoOnlyMp4Options {
            fps: u32::MAX,
            ..options()
        },
        VideoOnlyMp4Options {
            video_bitrate_bps: 0,
            ..options()
        },
    ] {
        assert!(validate_options(options).is_err());
    }
}

#[test]
fn shared_partial_output_keeps_animation_extension_and_publishes_only_on_commit() {
    let directory = tempfile::tempdir().unwrap();
    for extension in ["gif", "png", "mp4"] {
        let path = directory.path().join(format!("animation.{extension}"));
        let partial = PartialOutput::new(&path).unwrap();
        assert_eq!(partial.path().extension().unwrap(), extension);
        std::fs::write(partial.path(), b"finished").unwrap();
        assert!(!path.exists());
        partial
            .commit(&path, false, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"finished");
    }
}
