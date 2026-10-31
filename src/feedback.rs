//! Timing is resolved once, when a notification is published, never while drawing it.
use crate::ai::failure::FailureDiagnostic;
use crate::settings::NotificationDuration;

/// Only allowlisted values cross from a failed job into visible/copyable text.
#[derive(Clone, Debug)]
pub(crate) struct AiFailureReport {
    owner: u64,
    summary: String,
    body: String,
}
impl AiFailureReport {
    pub(crate) fn new(
        diagnostic: FailureDiagnostic,
        observed_at: Option<crate::diagnostic_time::LocalDateTime>,
        owner: u64,
        generation: u64,
        job: u64,
        page: usize,
        total: usize,
        source: [usize; 2],
    ) -> Self {
        let label = diagnostic.kind.label();
        let code = diagnostic
            .ort_code
            .map_or_else(|| "不明".into(), |code| code.to_string());
        let observed_at = crate::diagnostic_time::display(observed_at);
        Self {
            owner,
            summary: format!("AI処理に失敗（{label}）・クリックでコピー"),
            body: format!(
                "AI処理に失敗しました（{label}）。\n観測時刻: {observed_at}\nエラー番号: {code}\n画面番号 {owner} / 更新番号 {generation} / 処理番号 {job}\nページ {page}/{total} / 入力画像の寸法 {}x{}\nこの記録は処理の失敗です。異常終了やGPUの喪失を示すものではありません。\nエラーの原文は既存の詳細ログに残ります。",
                source[0], source[1]
            ),
        }
    }
    pub(crate) fn owner(&self) -> u64 {
        self.owner
    }
    pub(crate) fn summary(&self) -> &str {
        &self.summary
    }
    pub(crate) fn body(&self) -> &str {
        &self.body
    }
}

/// One action owns the toast: saved-file reveal or diagnostic copy, never both.
#[derive(Clone, Debug)]
pub(crate) enum FeedbackAction {
    RevealPath(std::path::PathBuf),
    Diagnostic(std::sync::Arc<AiFailureReport>),
}
impl FeedbackAction {
    pub(crate) fn response(&self, ui: &egui::Ui, rect: egui::Rect) -> egui::Response {
        self.response_with_cursor_extent(ui, rect, None)
    }
    fn response_with_cursor_extent(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        cursor_extent: Option<(f32, f32)>,
    ) -> egui::Response {
        use crate::ui_helpers::HoverTipExt as _;
        let response = ui
            .interact(
                rect,
                egui::Id::unique("feedback_toast_reveal_capture"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        match self {
            Self::RevealPath(_) => response.hover_tip_dark("クリックで保存ファイルを表示"),
            Self::Diagnostic(report) => match cursor_extent {
                Some(extent) => crate::ui_helpers::hover_tip_dark_with_cursor_extent(
                    response,
                    report.body(),
                    extent,
                ),
                None => response.hover_tip_dark(report.body()),
            },
        }
    }
    pub(crate) fn activate(&self, ctx: &egui::Context) {
        match self {
            Self::RevealPath(path) => crate::capture::reveal_path_async(path.clone()),
            Self::Diagnostic(report) => ctx.copy_text(report.body().to_owned()),
        }
    }
    #[cfg(test)]
    pub(crate) fn reveal_path(&self) -> Option<&std::path::PathBuf> {
        match self {
            Self::RevealPath(path) => Some(path),
            Self::Diagnostic(_) => None,
        }
    }
}

fn ai_failure_snapshot_report() -> std::sync::Arc<AiFailureReport> {
    std::sync::Arc::new(AiFailureReport::new(
        FailureDiagnostic {
            kind: crate::ai::failure::FailureKind::Inference,
            ort_code: Some(6),
        },
        crate::diagnostic_time::LocalDateTime::with_offset(1_793_355_825_000, 540),
        3,
        17,
        655,
        12,
        40,
        [2400, 3600],
    ))
}

/// Pure UI fixture shared by the integration snapshot and lib input regression.
/// Fonts/theme come from the standard snapshot harness; cursor extent is fixed.
#[doc(hidden)]
pub fn draw_ai_failure_feedback_snapshot_fixture(
    ui: &mut egui::Ui,
    cursor_extent: (f32, f32),
) -> egui::Rect {
    let report = ai_failure_snapshot_report();
    let action = FeedbackAction::Diagnostic(report.clone());
    let galley = ui.painter().layout_no_wrap(
        report.summary().to_owned(),
        egui::FontId::proportional(18.0),
        egui::Color32::WHITE,
    );
    let rect = egui::Rect::from_min_size(
        ui.max_rect().min + egui::vec2(12.0, 12.0),
        galley.size() + egui::vec2(32.0, 20.0),
    );
    ui.painter()
        .rect_filled(rect, 8.0, egui::Color32::from_gray(30));
    ui.painter().galley(
        rect.min + egui::vec2(16.0, 10.0),
        galley,
        egui::Color32::WHITE,
    );
    // Match tests/ui_snapshot.rs: physical pixels above/below the hotspot.
    if action
        .response_with_cursor_extent(ui, rect, Some(cursor_extent))
        .clicked()
    {
        action.activate(ui.ctx());
    }
    // The standard integration helper uses a wrapping Frame. A paint-only
    // fixture must reserve its viewport so that frame paints its background.
    ui.allocate_space(ui.available_size());
    rect
}

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

#[cfg(test)]
mod ai_failure_ui_tests {
    use super::*;
    use crate::ai::failure::{FailureDiagnostic, FailureKind};
    use crate::os_theme::ResolvedTheme;

    fn report() -> std::sync::Arc<AiFailureReport> {
        ai_failure_snapshot_report()
    }

    #[test]
    fn ai_failure_diagnostic_report_only_copies_allowlisted_values_for_every_kind() {
        for kind in [
            FailureKind::Inference,
            FailureKind::ModelLoad,
            FailureKind::ImageProcessing,
            FailureKind::Io,
            FailureKind::Unknown,
        ] {
            let report = AiFailureReport::new(
                FailureDiagnostic {
                    kind,
                    ort_code: None,
                },
                None,
                1,
                2,
                3,
                4,
                5,
                [6, 7],
            );
            assert!(report.body().contains("エラー番号: 不明"));
            assert!(report.body().contains("観測時刻: 時刻不明"));
            assert!(
                report
                    .body()
                    .contains("画面番号 1 / 更新番号 2 / 処理番号 3")
            );
            assert!(report.body().contains("入力画像の寸法 6x7"));
            for internal in ["ORT code", "unknown", "context ", "世代", "job ", "source "] {
                assert!(
                    !report.body().contains(internal),
                    "internal term: {internal}"
                );
            }
            assert!(
                report
                    .body()
                    .contains("異常終了やGPUの喪失を示すものではありません")
            );
            assert!(report.summary().chars().count() < 30);
        }
    }

    struct State {
        rect: egui::Rect,
        copied: Vec<String>,
    }
    impl Default for State {
        fn default() -> Self {
            Self {
                rect: egui::Rect::NOTHING,
                copied: Vec::new(),
            }
        }
    }

    fn harness(scale: f32) -> egui_kittest::Harness<'static, State> {
        let mut fonts_ready = false;
        crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui_state(
                move |ui, state: &mut State| {
                    let ctx = ui.ctx().clone();
                    crate::os_theme::apply_resolved(&ctx, ResolvedTheme::Light);
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(&ctx);
                        crate::settings::apply_ui_scale_factor(&ctx, scale);
                        fonts_ready = true;
                        return;
                    }
                    for theme in [egui::Theme::Light, egui::Theme::Dark] {
                        ctx.style_mut_of(theme, |style| style.interaction.tooltip_delay = 0.0);
                    }
                    crate::test_ui::full_viewport_ui(ui, |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            state.rect = draw_ai_failure_feedback_snapshot_fixture(ui, (0.0, 34.0));
                            assert!(
                                ctx.content_rect().contains_rect(state.rect),
                                "toast fits at {scale}"
                            );
                        });
                    });
                    state.copied.extend(ctx.output_mut(|output| {
                        std::mem::take(&mut output.commands)
                            .into_iter()
                            .filter_map(|command| {
                                if let egui::OutputCommand::CopyText(text) = command {
                                    Some(text)
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                    }));
                },
                State::default(),
            )
    }

    #[test]
    fn ai_failure_feedback_hover_and_explicit_copy() {
        assert!(report().body().contains("2026-10-30 19:23:45 (UTC+09:00)"));
        let manual = include_str!("../htdocs/mimageviewer/manual/troubleshooting.html");
        let example = manual
            .split("<pre>AI処理に失敗しました")
            .nth(1)
            .unwrap()
            .split("</pre>")
            .next()
            .unwrap();
        assert_eq!(
            report().body(),
            format!("AI処理に失敗しました{}", example.replace("\r\n", "\n"))
        );
        for scale in [1.0, 2.0] {
            let mut h = harness(scale);
            h.run_steps(12);
            assert!(h.state().copied.is_empty());
            let pos = h.state().rect.center();
            h.hover_at(pos);
            h.run_steps(30);
            assert!(h.state().copied.is_empty(), "hover must not copy");
            // PNG belongs to tests/ui_snapshot.rs; this test covers input/copy.
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                h.step();
            }
            assert_eq!(h.state().copied, [report().body().to_owned()]);
        }
    }
}
