use std::fmt;

use encoding_rs::SHIFT_JIS;

use crate::{LyricLine, LyricsLayer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LrcEncoding {
    Utf8,
    ShiftJis,
}

impl LrcEncoding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::ShiftJis => "Shift_JIS",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LrcParseReport {
    pub lines: Vec<LyricLine>,
    pub encoding: LrcEncoding,
    pub offset_millis: i64,
    pub skipped_metadata_tags: usize,
    pub skipped_unsupported_tags: usize,
    pub inline_time_tags_removed: usize,
}

impl LrcParseReport {
    pub fn skipped_elements(&self) -> usize {
        self.skipped_metadata_tags + self.skipped_unsupported_tags + self.inline_time_tags_removed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LrcWriteReport {
    pub skipped_unset_lines: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveLyric<'a> {
    pub line_index: usize,
    pub text: &'a str,
    pub opacity: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LrcParseError {
    InvalidTextEncoding,
}

impl fmt::Display for LrcParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTextEncoding => {
                f.write_str("UTF-8 と Shift_JIS のどちらでも歌詞を読み取れません")
            }
        }
    }
}

impl std::error::Error for LrcParseError {}

/// Returns the latest timed line at the requested time; a later row wins at the same time.
pub fn active_lyric_at(layer: &LyricsLayer, t_secs: f64) -> Option<ActiveLyric<'_>> {
    if !t_secs.is_finite() {
        return None;
    }
    let (line_index, start) = layer
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            line.time_secs
                .filter(|time| time.is_finite() && *time <= t_secs)
                .map(|time| (index, time))
        })
        .max_by(|(left_index, left_time), (right_index, right_time)| {
            left_time
                .total_cmp(right_time)
                .then(left_index.cmp(right_index))
        })?;
    let next = layer
        .lines
        .iter()
        .filter_map(|line| line.time_secs)
        .filter(|time| time.is_finite() && *time > start)
        .min_by(f64::total_cmp);
    let fade_in = finite_nonnegative(layer.fade_in_secs);
    let fade_out = finite_nonnegative(layer.fade_out_secs);
    let in_alpha = if fade_in > 0.0 {
        ((t_secs - start) / f64::from(fade_in)).clamp(0.0, 1.0) as f32
    } else {
        1.0
    };
    let out_alpha = if fade_out > 0.0 {
        next.map_or(1.0, |end| {
            ((end - t_secs) / f64::from(fade_out)).clamp(0.0, 1.0) as f32
        })
    } else {
        1.0
    };
    Some(ActiveLyric {
        line_index,
        text: &layer.lines[line_index].text,
        opacity: in_alpha.min(out_alpha),
    })
}

pub fn parse_lrc(bytes: &[u8]) -> Result<LrcParseReport, LrcParseError> {
    let (text, encoding) = decode_lrc(bytes)?;
    let mut offset_millis = 0_i64;
    for raw_line in text.lines() {
        for tag in leading_tags(raw_line).0 {
            if let Some(value) = tag
                .split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("offset"))
                .and_then(|(_, value)| value.trim().parse::<i64>().ok())
            {
                offset_millis = value;
            }
        }
    }

    let mut report = LrcParseReport {
        lines: Vec::new(),
        encoding,
        offset_millis,
        skipped_metadata_tags: 0,
        skipped_unsupported_tags: 0,
        inline_time_tags_removed: 0,
    };
    for raw_line in text.lines() {
        let (tags, body) = leading_tags(raw_line);
        let had_tags = !tags.is_empty();
        let mut times = Vec::new();
        for tag in tags {
            if let Some(time) = parse_timestamp(tag) {
                times.push((time + offset_millis as f64 / 1000.0).max(0.0));
            } else if is_metadata_tag(tag) {
                report.skipped_metadata_tags += 1;
            } else if tag.to_ascii_lowercase().starts_with("offset:") {
                report.skipped_metadata_tags += 1;
            } else {
                report.skipped_unsupported_tags += 1;
            }
        }
        let (body, removed) = strip_inline_time_tags(body.trim_start());
        report.inline_time_tags_removed += removed;
        if !times.is_empty() {
            report
                .lines
                .extend(times.into_iter().map(|time_secs| LyricLine {
                    text: body.clone(),
                    time_secs: Some(time_secs),
                }));
        } else if !had_tags || !body.is_empty() {
            report.lines.push(LyricLine {
                text: body,
                time_secs: None,
            });
        }
    }
    Ok(report)
}

pub fn write_lrc(lines: &[LyricLine]) -> (String, LrcWriteReport) {
    let mut output = String::new();
    let skipped_unset_lines = lines
        .iter()
        .filter(|line| !line.time_secs.is_some_and(|time| time.is_finite()))
        .count();
    let mut timed = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            line.time_secs
                .filter(|time| time.is_finite())
                .map(|time| (index, time, line))
        })
        .collect::<Vec<_>>();
    timed.sort_by(|(left_index, left_time, _), (right_index, right_time, _)| {
        left_time
            .total_cmp(right_time)
            .then(left_index.cmp(right_index))
    });
    for (_, time, line) in timed {
        let centiseconds = (time.max(0.0) * 100.0).round() as u64;
        let minutes = centiseconds / 6000;
        let seconds = (centiseconds / 100) % 60;
        let fraction = centiseconds % 100;
        output.push_str(&format!(
            "[{minutes:02}:{seconds:02}.{fraction:02}]{}\n",
            line.text
        ));
    }
    (
        output,
        LrcWriteReport {
            skipped_unset_lines,
        },
    )
}

fn decode_lrc(bytes: &[u8]) -> Result<(String, LrcEncoding), LrcParseError> {
    let utf8 = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(utf8) {
        return Ok((text.to_owned(), LrcEncoding::Utf8));
    }
    let (text, had_errors) = SHIFT_JIS.decode_without_bom_handling(bytes);
    if had_errors {
        Err(LrcParseError::InvalidTextEncoding)
    } else {
        Ok((text.into_owned(), LrcEncoding::ShiftJis))
    }
}

fn leading_tags(mut line: &str) -> (Vec<&str>, &str) {
    let mut tags = Vec::new();
    loop {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix('[') else {
            return (tags, trimmed);
        };
        let Some(end) = rest.find(']') else {
            return (tags, trimmed);
        };
        tags.push(&rest[..end]);
        line = &rest[end + 1..];
    }
}

fn parse_timestamp(tag: &str) -> Option<f64> {
    let (minutes, seconds) = tag.trim().split_once(':')?;
    if minutes.is_empty() || seconds.is_empty() {
        return None;
    }
    let minutes = minutes.parse::<u64>().ok()?;
    let seconds = seconds.parse::<f64>().ok()?;
    (seconds.is_finite() && (0.0..60.0).contains(&seconds))
        .then_some(minutes as f64 * 60.0 + seconds)
}

fn is_metadata_tag(tag: &str) -> bool {
    tag.split_once(':').is_some_and(|(key, _)| {
        ["ti", "ar", "al", "by", "au", "re", "ve", "length"]
            .iter()
            .any(|known| key.eq_ignore_ascii_case(known))
    })
}

fn strip_inline_time_tags(text: &str) -> (String, usize) {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    let mut removed = 0;
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('>') else {
            output.push_str(&rest[start..]);
            return (output, removed);
        };
        let tag = &after_start[..end];
        if parse_timestamp(tag).is_some() {
            removed += 1;
        } else {
            output.push_str(&rest[start..start + end + 2]);
        }
        rest = &after_start[end + 1..];
    }
    output.push_str(rest);
    (output, removed)
}

fn finite_nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Animatable, Fill, Rgba8, TextAlign};

    fn layer(lines: Vec<LyricLine>) -> LyricsLayer {
        LyricsLayer {
            lines,
            fade_in_secs: 0.0,
            fade_out_secs: 0.0,
            font_key: "default".to_owned(),
            size_px: 64.0,
            align: TextAlign::Center,
            fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
        }
    }

    #[test]
    fn active_line_covers_unset_empty_duplicate_and_time_boundaries() {
        let lyrics = layer(vec![
            LyricLine {
                text: "未設定".into(),
                time_secs: None,
            },
            LyricLine {
                text: "最初".into(),
                time_secs: Some(1.0),
            },
            LyricLine {
                text: "同時の前".into(),
                time_secs: Some(2.0),
            },
            LyricLine {
                text: "同時の後".into(),
                time_secs: Some(2.0),
            },
            LyricLine {
                text: String::new(),
                time_secs: Some(3.0),
            },
            LyricLine {
                text: "最後".into(),
                time_secs: Some(4.0),
            },
        ]);
        assert_eq!(active_lyric_at(&lyrics, 0.999), None);
        assert_eq!(active_lyric_at(&lyrics, 1.0).unwrap().text, "最初");
        assert_eq!(active_lyric_at(&lyrics, 2.0).unwrap().text, "同時の後");
        assert_eq!(active_lyric_at(&lyrics, 3.5).unwrap().text, "");
        assert_eq!(active_lyric_at(&lyrics, 99.0).unwrap().text, "最後");
    }

    #[test]
    fn fades_are_applied_around_line_boundaries() {
        let mut lyrics = layer(vec![
            LyricLine {
                text: "一".into(),
                time_secs: Some(1.0),
            },
            LyricLine {
                text: "二".into(),
                time_secs: Some(3.0),
            },
        ]);
        lyrics.fade_in_secs = 0.5;
        lyrics.fade_out_secs = 0.25;
        assert!((active_lyric_at(&lyrics, 1.125).unwrap().opacity - 0.25).abs() < 1e-6);
        assert!((active_lyric_at(&lyrics, 2.875).unwrap().opacity - 0.5).abs() < 1e-6);
    }

    #[test]
    fn lrc_parses_offset_multiple_tags_metadata_and_inline_tags() {
        let parsed = parse_lrc(
            b"[ti:title]\n[offset:-500]\n[x:unknown]\n[00:01.00][00:02.50] <00:01.10>Hello<00:01.20>!\nplain",
        )
        .unwrap();
        assert_eq!(parsed.encoding, LrcEncoding::Utf8);
        assert_eq!(parsed.offset_millis, -500);
        assert_eq!(parsed.skipped_metadata_tags, 2);
        assert_eq!(parsed.skipped_unsupported_tags, 1);
        assert_eq!(parsed.inline_time_tags_removed, 2);
        assert_eq!(
            parsed.lines,
            vec![
                LyricLine {
                    text: "Hello!".into(),
                    time_secs: Some(0.5)
                },
                LyricLine {
                    text: "Hello!".into(),
                    time_secs: Some(2.0)
                },
                LyricLine {
                    text: "plain".into(),
                    time_secs: None
                },
            ]
        );
    }

    #[test]
    fn shift_jis_is_detected_and_decoded() {
        let (bytes, _, _) = SHIFT_JIS.encode("[00:01.25]星空の歌");
        let parsed = parse_lrc(&bytes).unwrap();
        assert_eq!(parsed.encoding, LrcEncoding::ShiftJis);
        assert_eq!(parsed.lines[0].text, "星空の歌");
    }

    #[test]
    fn utf8_bom_is_accepted_without_becoming_lyric_text() {
        let mut bytes = b"\xef\xbb\xbf[00:00.50]".to_vec();
        bytes.extend_from_slice("日本語".as_bytes());
        let parsed = parse_lrc(&bytes).unwrap();
        assert_eq!(parsed.encoding, LrcEncoding::Utf8);
        assert_eq!(parsed.lines[0].text, "日本語");
    }

    #[test]
    fn lrc_writer_is_utf8_ready_and_reports_unset_lines() {
        let (text, report) = write_lrc(&[
            LyricLine {
                text: "後".into(),
                time_secs: Some(62.0),
            },
            LyricLine {
                text: "未設定".into(),
                time_secs: None,
            },
            LyricLine {
                text: "日本語".into(),
                time_secs: Some(61.239),
            },
        ]);
        assert_eq!(text, "[01:01.24]日本語\n[01:02.00]後\n");
        assert_eq!(report.skipped_unset_lines, 1);
        assert_eq!(std::str::from_utf8(text.as_bytes()).unwrap(), text);
    }
}
