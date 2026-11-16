//! Evaluation `postprocess.py::clean(segments, None)`, in the original rule order.
//! Decode and cleanup share `asr_worker::SETTINGS_VERSION`; bump it for either change.

use std::collections::VecDeque;
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::transcript_db::TranscriptSegment;

// Keep these expressions and constants identical to exp-asr-eval/postprocess.py.
const LOOP: &str = r"(.{1,8}?)\1{4,}";
const STOCK: &str = concat!(
    r"^(最後まで)?(ご視聴|ご清聴)(いただき)?(本当に|どうも)?ありがとうございました$",
    r"|^チャンネル登録(を)?(よろしく)?お願い(します|いたします)$"
);
const STOCK_LONG_S: f64 = 5.0;
const STOCK_SHORT_S: f64 = 1.0;
const REPEAT_LOOKBACK: usize = 5;
const REPEAT_WINDOW_S: f64 = 60.0;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemovedCounts {
    pub stock: u64,
    pub repeat: u64,
}

/// NFKC -> whole-string lowercase -> omit Python whitespace, P*, S*, Cc, Cf.
fn normalize(text: &str) -> String {
    static DROP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[\s\p{P}\p{S}\p{Cc}\p{Cf}\x1c-\x1f]").unwrap());
    let normalized = text.nfkc().collect::<String>().to_lowercase();
    DROP.replace_all(&normalized, "").into_owned()
}

/// Rust regex deliberately has no backreferences. Implement LOOP's exact
/// leftmost, lazy-unit (1..=8 Unicode scalars), greedy-repeat substitution.
fn shorten_loops(text: &str) -> String {
    debug_assert_eq!(LOOP, r"(.{1,8}?)\1{4,}");
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while pos < chars.len() {
        let found = (1..=8).find_map(|width| {
            let end = pos + width;
            if pos + width * 5 > chars.len() || chars[pos..end].contains(&'\n') {
                return None;
            }
            let unit = &chars[pos..end];
            let mut repeats = 1;
            while pos + (repeats + 1) * width <= chars.len()
                && &chars[pos + repeats * width..pos + (repeats + 1) * width] == unit
            {
                repeats += 1;
            }
            (repeats >= 5).then_some((width, repeats))
        });
        if let Some((width, repeats)) = found {
            for _ in 0..3 {
                out.extend(&chars[pos..pos + width]);
            }
            pos += width * repeats;
        } else {
            out.push(chars[pos]);
            pos += 1;
        }
    }
    out
}

pub fn clean(segments: &[TranscriptSegment]) -> (Vec<TranscriptSegment>, RemovedCounts) {
    static STOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(STOCK).unwrap());
    let mut kept = Vec::with_capacity(segments.len());
    let mut counts = RemovedCounts::default();
    // Only segments reaching rule 3 enter history, including repeat removals.
    let mut history = VecDeque::<(u64, String)>::with_capacity(REPEAT_LOOKBACK);
    for (start, end, text) in segments {
        let n = normalize(text);
        if n.is_empty() {
            continue;
        }
        // Python subtracts seconds as floats: preserve its rounding at exact thresholds.
        let dur = *end as f64 / 1000.0 - *start as f64 / 1000.0;
        if STOCK_RE.is_match(&n) && (dur >= STOCK_LONG_S || dur < STOCK_SHORT_S) {
            counts.stock += 1;
            continue;
        }
        let text = shorten_loops(text);
        let n = normalize(&text);
        let seen = history.iter().any(|(st, previous)| {
            *previous == n && (*start as f64 / 1000.0 - *st as f64 / 1000.0) <= REPEAT_WINDOW_S
        });
        if history.len() == REPEAT_LOOKBACK {
            history.pop_front();
        }
        history.push_back((*start, n));
        if seen {
            counts.repeat += 1;
        } else {
            kept.push((*start, *end, text));
        }
    }
    (kept, counts)
}

#[cfg(test)]
mod tests;
