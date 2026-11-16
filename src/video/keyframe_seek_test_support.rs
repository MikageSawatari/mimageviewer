//! Worker assertions use frame IDs from a full sequential decode, independently
//! of the seek implementation and of its keyframe verification decisions.

use std::path::{Path, PathBuf};

pub(crate) const WORKER_FIXTURES: &[&str] = &[
    "h264-small.ts",
    "mpeg2-large.ts",
    "hevc-open.ts",
    "mpeg2-missing-pts.mpg",
    "mid-gop.ts",
    "mid-gop.mpg",
    "long-gop.ts",
    "control.mp4",
];

pub(crate) struct OracleFrame {
    pub(crate) frame_id: u32,
    pub(crate) media_secs: f64,
    pub(crate) key_frame: bool,
}

pub(crate) struct Fixture {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) frames: Vec<OracleFrame>,
}

fn integer(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

impl Fixture {
    pub(crate) fn load(name: &str) -> Self {
        use ffmpeg_the_third as ffmpeg;
        ffmpeg::init().unwrap();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/mpeg-keyframe-seek")
            .join(name);
        let oracle_path = path.with_file_name(format!("{name}.json"));
        let oracle: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&oracle_path)
                .unwrap_or_else(|e| panic!("{}: {e}", oracle_path.display())),
        )
        .unwrap();
        let input = ffmpeg::format::input(&path).unwrap();
        let timeline = crate::video::media_timeline::MediaTimeline::from_input(&input);
        let stream_idx = integer(&oracle["video_stream"]["index"]).unwrap() as usize;
        let time_base = input.stream(stream_idx).unwrap().time_base();
        let tick_secs = time_base.numerator() as f64 / time_base.denominator() as f64;
        let frames = oracle["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|frame| OracleFrame {
                frame_id: integer(&frame["frame_id"]).unwrap() as u32,
                media_secs: timeline.raw_to_media_secs(
                    integer(&frame["best_effort_timestamp"]).unwrap() as f64 * tick_secs,
                ),
                key_frame: frame["key_frame"]
                    .as_bool()
                    .unwrap_or_else(|| integer(&frame["key_frame"]) == Some(1)),
            })
            .collect::<Vec<_>>();
        assert!(!frames.is_empty(), "{name}: oracle has no decoded frames");
        Self {
            path,
            name: name.to_owned(),
            frames,
        }
    }

    /// Interior points and both EOF cases exercise the existing worker adoption
    /// rules without placing a target on an ambiguous floating-point boundary.
    pub(crate) fn targets(&self) -> Vec<f64> {
        let count = self.frames.len();
        let step = self.frames[count - 1].media_secs - self.frames[count - 2].media_secs;
        let mut targets = [count / 3, count * 2 / 3, count - 3, count - 1]
            .into_iter()
            .map(|idx| self.frames[idx].media_secs - step * 0.25)
            .collect::<Vec<_>>();
        targets.push(self.frames[count - 1].media_secs + step * 0.5);
        targets
    }

    pub(crate) fn first_at_or_after(&self, target_secs: f64) -> &OracleFrame {
        self.frames
            .iter()
            .find(|frame| frame.media_secs >= target_secs)
            .unwrap_or_else(|| self.frames.last().unwrap())
    }

    pub(crate) fn nearest(&self, target_secs: f64) -> &OracleFrame {
        self.frames
            .iter()
            .min_by(|a, b| {
                (a.media_secs - target_secs)
                    .abs()
                    .total_cmp(&(b.media_secs - target_secs).abs())
                    // Screenshot's existing rule prefers the later frame on ties.
                    .then_with(|| b.media_secs.total_cmp(&a.media_secs))
            })
            .unwrap()
    }
}

pub(crate) fn assert_frame_id(rgba: &[u8], width: u32, height: u32, expected: u32, context: &str) {
    // The fixture stores 16 little-endian bits at source coordinates
    // (4 + 6 * bit, 16), in a 96x64 image. Use pixel centres after scaling.
    let mut actual = 0;
    for bit in 0..16 {
        let x = (((4 + 6 * bit) as f64 + 0.5) * width as f64 / 96.0).floor() as u32;
        let y = (16.5 * height as f64 / 64.0).floor() as u32;
        let offset = ((y.min(height - 1) * width + x.min(width - 1)) * 4) as usize;
        let pixel = &rgba[offset..offset + 3];
        if pixel.iter().map(|&v| v as u32).sum::<u32>() >= 3 * 128 {
            actual |= 1 << bit;
        }
    }
    assert_eq!(actual, expected, "{context}: decoded frame ID mismatch");
}
