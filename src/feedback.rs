//! Timing is resolved once, when a notification is published, never while drawing it.
use crate::settings::NotificationDuration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FeedbackKind {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FeedbackTiming {
    Text,
    Seconds(f32),
    /// Preserve the existing egui/native baseline difference for saved-output notices.
    SurfaceSeconds {
        egui: f32,
        native: f32,
    },
    /// Repeated input boundary feedback; independent of reading preferences.
    Immediate(f32),
}

impl FeedbackKind {
    pub(crate) fn result(needs_attention: bool) -> Self {
        if needs_attention {
            Self::Warning
        } else {
            Self::Info
        }
    }
}

pub(crate) const FEEDBACK_TOAST_DURATION: f32 = 1.2;

pub(crate) fn feedback_toast_duration(text: &str) -> f32 {
    let extra = text.chars().count().saturating_sub(20);
    (FEEDBACK_TOAST_DURATION + extra as f32 * 0.12).min(5.5)
}

/// Fully resolved lifetime and legacy native fade. Transport/renderers never read preferences.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResolvedFeedbackTiming {
    pub(crate) seconds: f32,
    native_linger: std::time::Duration,
    native_fade_end: [f32; 2],
    native_fade_duration: f32,
}

impl ResolvedFeedbackTiming {
    pub(crate) fn native_linger(self) -> std::time::Duration {
        self.native_linger
    }

    pub(crate) fn native_opacity_at(self, elapsed: f32, centered: bool) -> f32 {
        let fade_end = self.native_fade_end[usize::from(centered)];
        if elapsed > fade_end - self.native_fade_duration {
            ((fade_end - elapsed) / self.native_fade_duration).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

/// The only preference multiplier. Preserve each surface's pre-setting lifetime and fade,
/// including native fade ending before a long explicit linger (and after a short one).
pub(crate) fn resolve_timing(
    setting: NotificationDuration,
    kind: FeedbackKind,
    text: &str,
    timing: FeedbackTiming,
) -> ResolvedFeedbackTiming {
    let (base, native_base) = match timing {
        FeedbackTiming::Text => {
            let seconds = feedback_toast_duration(text);
            (seconds, seconds)
        }
        FeedbackTiming::Seconds(seconds) | FeedbackTiming::Immediate(seconds) => (seconds, seconds),
        FeedbackTiming::SurfaceSeconds { egui, native } => (egui, native),
    };
    let multiplier = match (timing, setting) {
        (FeedbackTiming::Immediate(_), _) => 1.0,
        (_, NotificationDuration::Short) if kind == FeedbackKind::Info => 0.6,
        (_, NotificationDuration::Long) => 1.5,
        _ => 1.0,
    };
    ResolvedFeedbackTiming {
        seconds: base * multiplier,
        native_linger: std::time::Duration::from_secs_f32(native_base * multiplier),
        native_fade_end: [1.8 * multiplier, 2.5 * multiplier],
        native_fade_duration: 0.35 * multiplier,
    }
}

#[cfg(test)]
pub(crate) fn resolve_duration(
    setting: NotificationDuration,
    kind: FeedbackKind,
    text: &str,
    timing: FeedbackTiming,
) -> f32 {
    resolve_timing(setting, kind, text, timing).seconds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_duration_text_steps_and_cap() {
        for (chars, standard) in [
            (0, 1.2),
            (20, 1.2),
            (21, 1.32),
            (40, 3.6),
            (56, 5.5),
            (10000, 5.5),
        ] {
            for (setting, factor) in [
                (NotificationDuration::Short, 0.6),
                (NotificationDuration::Standard, 1.0),
                (NotificationDuration::Long, 1.5),
            ] {
                let text = "あ".repeat(chars);
                let actual =
                    resolve_duration(setting, FeedbackKind::Info, &text, FeedbackTiming::Text);
                assert!(
                    (actual - standard * factor).abs() < 0.00001,
                    "{chars}: {actual}"
                );
            }
        }
    }

    #[test]
    fn notification_duration_protects_warnings_and_errors_including_explicit_seconds() {
        for text in ["失敗", &"長".repeat(1000)] {
            for timing in [FeedbackTiming::Text, FeedbackTiming::Seconds(4.5)] {
                let standard = resolve_duration(
                    NotificationDuration::Standard,
                    FeedbackKind::Info,
                    text,
                    timing,
                );
                for kind in [FeedbackKind::Warning, FeedbackKind::Error] {
                    assert_eq!(
                        resolve_duration(NotificationDuration::Short, kind, text, timing),
                        standard
                    );
                    assert_eq!(
                        resolve_duration(NotificationDuration::Long, kind, text, timing),
                        standard * 1.5
                    );
                }
            }
        }
    }

    #[test]
    fn notification_duration_explicit_and_immediate() {
        for (setting, factor) in [
            (NotificationDuration::Short, 0.6),
            (NotificationDuration::Standard, 1.0),
            (NotificationDuration::Long, 1.5),
        ] {
            assert_eq!(
                resolve_duration(
                    setting,
                    FeedbackKind::Info,
                    "",
                    FeedbackTiming::Seconds(3.5)
                ),
                3.5 * factor
            );
            assert_eq!(
                resolve_duration(
                    setting,
                    FeedbackKind::Info,
                    "",
                    FeedbackTiming::Immediate(0.7)
                ),
                0.7
            );
        }
    }
}
