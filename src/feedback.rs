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

/// The only preference multiplier. Explicit durations are standard-time bases, not overrides.
pub(crate) fn resolve_duration(
    setting: NotificationDuration,
    kind: FeedbackKind,
    text: &str,
    timing: FeedbackTiming,
) -> f32 {
    let base = match timing {
        FeedbackTiming::Text => feedback_toast_duration(text),
        FeedbackTiming::Seconds(seconds) => seconds,
        FeedbackTiming::Immediate(seconds) => return seconds,
    };
    let multiplier = match setting {
        NotificationDuration::Short if kind == FeedbackKind::Info => 0.6,
        NotificationDuration::Long => 1.5,
        _ => 1.0,
    };
    base * multiplier
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
