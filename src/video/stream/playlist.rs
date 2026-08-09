use std::collections::VecDeque;

use super::encoder::SEGMENT_DURATION_SECS;

/// 正本 §4.4 の既定 live window: 2 秒 x 30 本 = 60 秒。
#[cfg(test)]
pub(crate) const DEFAULT_SEGMENT_CAPACITY: usize = 30;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MediaSegment {
    pub(crate) sequence: u64,
    pub(crate) duration_secs: f64,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SegmentLookup<'a> {
    Found(&'a MediaSegment),
    Gone,
    NotFound,
}

/// 生成順 sequence を唯一の source of truth とする media segment ring。
#[derive(Debug)]
pub(crate) struct SegmentRing {
    capacity: usize,
    next_sequence: u64,
    segments: VecDeque<MediaSegment>,
}

impl SegmentRing {
    pub(crate) fn new(capacity: usize) -> Result<Self, &'static str> {
        if capacity == 0 {
            return Err("segment ring capacity must be non-zero");
        }
        Ok(Self {
            capacity,
            next_sequence: 0,
            segments: VecDeque::with_capacity(capacity),
        })
    }

    pub(crate) fn push(&mut self, duration_secs: f64, bytes: Vec<u8>) -> Result<u64, &'static str> {
        if !duration_secs.is_finite() || duration_secs <= 0.0 {
            return Err("segment duration must be finite and positive");
        }
        if bytes.is_empty() {
            return Err("media segment must not be empty");
        }
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or("segment sequence exhausted")?;
        self.segments.push_back(MediaSegment {
            sequence,
            duration_secs,
            bytes,
        });
        if self.segments.len() > self.capacity {
            self.segments.pop_front();
        }
        Ok(sequence)
    }

    pub(crate) fn media_sequence(&self) -> u64 {
        self.segments
            .front()
            .map_or(self.next_sequence, |segment| segment.sequence)
    }

    #[cfg(test)]
    pub(crate) fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.segments.len()
    }

    pub(crate) fn get(&self, sequence: u64) -> SegmentLookup<'_> {
        if sequence < self.media_sequence() {
            return SegmentLookup::Gone;
        }
        if sequence >= self.next_sequence {
            return SegmentLookup::NotFound;
        }
        self.segments
            .get((sequence - self.media_sequence()) as usize)
            .map_or(SegmentLookup::NotFound, SegmentLookup::Found)
    }

    pub(crate) fn buffered_duration_secs(&self) -> f64 {
        self.segments
            .iter()
            .map(|segment| segment.duration_secs)
            .sum()
    }

    pub(crate) fn effective_bitrate_bps(&self) -> u64 {
        let duration_secs = self.buffered_duration_secs();
        if duration_secs <= 0.0 {
            return 0;
        }
        let bytes = self
            .segments
            .iter()
            .map(|segment| segment.bytes.len() as u64)
            .sum::<u64>();
        ((bytes as f64 * 8.0) / duration_secs).round() as u64
    }

    /// CMAF media playlist。live playlist なので PLAYLIST-TYPE / ENDLIST は出さない。
    pub(crate) fn media_playlist(&self) -> String {
        let target_duration = self
            .segments
            .iter()
            .map(|segment| segment.duration_secs.ceil() as u32)
            .max()
            .unwrap_or(SEGMENT_DURATION_SECS)
            .max(SEGMENT_DURATION_SECS);
        let mut playlist = format!(
            r#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-TARGETDURATION:{target_duration}
#EXT-X-MEDIA-SEQUENCE:{}
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-MAP:URI="init.mp4"
"#,
            self.media_sequence()
        );
        for segment in &self.segments {
            playlist.push_str(&format!(
                "#EXTINF:{:.6},\n{}.m4s\n",
                segment.duration_secs, segment.sequence
            ));
        }
        playlist
    }
}

/// CODECS は media playlist ではなく Master Playlist の
/// EXT-X-STREAM-INF に属する属性なので、標準に従って 2 層を明示する。
pub(crate) fn master_playlist(
    codecs: &str,
    bandwidth_bps: u64,
    dimensions: Option<(u32, u32)>,
) -> String {
    let resolution = dimensions
        .map(|(width, height)| format!(",RESOLUTION={width}x{height}"))
        .unwrap_or_default();
    format!(
        r#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-STREAM-INF:BANDWIDTH={},CODECS="{}"{}
media.m3u8
"#,
        bandwidth_bps.max(1),
        codecs,
        resolution,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(ring: &mut SegmentRing, value: u8) -> u64 {
        ring.push(f64::from(SEGMENT_DURATION_SECS), vec![value])
            .unwrap()
    }

    #[test]
    fn ring_distinguishes_evicted_from_future_segments() {
        let mut ring = SegmentRing::new(3).unwrap();
        for value in 0..5 {
            assert_eq!(push(&mut ring, value), u64::from(value));
        }
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.media_sequence(), 2);
        assert_eq!(ring.get(0), SegmentLookup::Gone);
        assert_eq!(ring.get(1), SegmentLookup::Gone);
        assert!(matches!(
            ring.get(2),
            SegmentLookup::Found(MediaSegment { sequence: 2, .. })
        ));
        assert_eq!(ring.get(5), SegmentLookup::NotFound);
        assert_eq!(ring.get(u64::MAX), SegmentLookup::NotFound);
    }

    #[test]
    fn ring_reports_observed_buffer_and_bitrate() {
        let mut ring = SegmentRing::new(2).unwrap();
        ring.push(2.0, vec![0; 100]).unwrap();
        ring.push(3.0, vec![0; 200]).unwrap();
        assert!((ring.buffered_duration_secs() - 5.0).abs() < f64::EPSILON);
        assert_eq!(ring.effective_bitrate_bps(), 480);
    }

    #[test]
    fn media_sequence_is_derived_from_the_same_front_that_eviction_changes() {
        let mut ring = SegmentRing::new(2).unwrap();
        assert_eq!(ring.media_sequence(), 0);
        push(&mut ring, 0);
        push(&mut ring, 1);
        assert!(ring.media_playlist().contains("#EXT-X-MEDIA-SEQUENCE:0\n"));
        push(&mut ring, 2);
        assert_eq!(ring.media_sequence(), 1);
        let playlist = ring.media_playlist();
        assert!(playlist.contains("#EXT-X-MEDIA-SEQUENCE:1\n"));
        assert!(!playlist.contains("0.m4s"));
        assert!(playlist.contains("1.m4s"));
        assert!(playlist.contains("2.m4s"));
    }

    #[test]
    fn media_playlist_is_live_and_references_the_init_segment() {
        let mut ring = SegmentRing::new(DEFAULT_SEGMENT_CAPACITY).unwrap();
        ring.push(2.0, vec![1]).unwrap();
        let playlist = ring.media_playlist();
        assert!(playlist.starts_with("#EXTM3U\n#EXT-X-VERSION:7\n"));
        assert!(playlist.contains("#EXT-X-TARGETDURATION:2\n"));
        assert!(playlist.contains(r#"#EXT-X-MAP:URI="init.mp4""#));
        assert!(playlist.contains("#EXTINF:2.000000,\n0.m4s\n"));
        assert!(!playlist.contains("#EXT-X-PLAYLIST-TYPE"));
        assert!(!playlist.contains("#EXT-X-ENDLIST"));
    }

    /// `EXTINF` は固定値ではなく fragment ごとの実測長である。長さの違う segment を
    /// 混ぜる設計を検討するとき、この契約が既に成立していることが出発点になる
    /// (`TARGETDURATION` は最長の切り上げ以上)。
    #[test]
    fn playlist_uses_measured_extinf_and_target_duration_covers_the_longest_segment() {
        let mut ring = SegmentRing::new(DEFAULT_SEGMENT_CAPACITY).unwrap();
        ring.push(0.5, vec![1]).unwrap();
        ring.push(2.002, vec![2]).unwrap();

        let playlist = ring.media_playlist();
        assert!(playlist.contains("#EXT-X-TARGETDURATION:3\n"));
        assert!(playlist.contains("#EXTINF:0.500000,\n0.m4s\n"));
        assert!(playlist.contains("#EXTINF:2.002000,\n1.m4s\n"));
    }

    #[test]
    fn codecs_attribute_is_on_the_master_playlist() {
        let playlist = master_playlist("avc1.42c01f", 400_000, Some((640, 360)));
        assert!(playlist.contains(
            r#"#EXT-X-STREAM-INF:BANDWIDTH=400000,CODECS="avc1.42c01f",RESOLUTION=640x360"#
        ));
        assert!(playlist.ends_with("media.m3u8\n"));
    }

    #[test]
    fn audio_only_master_playlist_omits_resolution() {
        let playlist = master_playlist("mp4a.40.2", 96_000, None);
        assert!(playlist.contains(r#"#EXT-X-STREAM-INF:BANDWIDTH=96000,CODECS="mp4a.40.2""#));
        assert!(!playlist.contains("RESOLUTION="));
    }

    #[test]
    fn invalid_ring_inputs_are_rejected() {
        assert!(SegmentRing::new(0).is_err());
        let mut ring = SegmentRing::new(1).unwrap();
        assert!(ring.push(0.0, vec![1]).is_err());
        assert!(ring.push(f64::NAN, vec![1]).is_err());
        assert!(ring.push(2.0, Vec::new()).is_err());
    }
}
