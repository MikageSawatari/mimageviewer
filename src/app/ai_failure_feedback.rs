//! Notify only at the existing live result-adoption boundary. No I/O or retry.
use super::*;

#[derive(Clone, Copy)]
pub(crate) struct FinalAiFeedbackRequest {
    pub(crate) priority: AiJobPriority,
    pub(crate) source: [usize; 2],
}
impl Default for FinalAiFeedbackRequest {
    fn default() -> Self {
        Self {
            priority: AiJobPriority::Prefetch,
            source: [0; 2],
        }
    }
}

impl App {
    /// Inspect before removing Pending: its Drop sets the shared cancel token.
    pub(super) fn final_ai_failure_feedback_request(
        &self,
        key: FinalAiKey,
        job_id: u64,
    ) -> Option<FinalAiFeedbackRequest> {
        let pending = self.final_ai_pending.get(&key)?;
        let request = pending.feedback_request;
        let idx = key.edit_key.idx;
        (pending.job_id == job_id
            && !pending.cancel.load(Ordering::Relaxed)
            && request.priority == AiJobPriority::Display
            && self.fullscreen_idx == Some(idx)
            && idx < self.items.len()
            && key.edit_key == self.current_edit_result_key(idx)
            && self.final_ai_key_for_pixels(
                key.edit_key,
                request.source,
                self.effective_params(idx),
            ) == Some(key))
        .then_some(request)
    }

    pub(super) fn notify_final_ai_failure(
        &mut self,
        key: FinalAiKey,
        job_id: u64,
        request: FinalAiFeedbackRequest,
        diagnostic: crate::ai::failure::FailureDiagnostic,
    ) {
        let report = Arc::new(crate::feedback::AiFailureReport::new(
            diagnostic,
            self.projected_viewer_context_id().serial(),
            self.items_generation,
            job_id,
            key.edit_key.idx.saturating_add(1),
            self.items.len(),
            request.source,
        ));
        self.show_feedback_result_on(
            report.summary().to_owned(),
            ActionSurface::Viewer,
            crate::feedback::FeedbackKind::Error,
        );
        self.fs_feedback_toast_action = Some(crate::feedback::FeedbackAction::Diagnostic(report));
    }

    pub(crate) fn feedback_action_matches_mounted_context(&self) -> bool {
        match &self.fs_feedback_toast_action {
            Some(crate::feedback::FeedbackAction::Diagnostic(report)) => {
                report.owner() == self.projected_viewer_context_id().serial()
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{AiError, failure::FinalAiFailure};
    use crate::app::tests::phase_c_support;
    const PRIVATE: &str = r"C:\Users\private\secret.zip\page.png search=query driver raw OOM";

    fn setup() -> (phase_c_support::AppTestEnv, FinalAiKey) {
        let mut app = phase_c_support::setup_app();
        app.items = vec![GridItem::Image(PathBuf::from(PRIVATE))];
        app.fullscreen_idx = Some(0);
        app.settings.ai_feature_mode = crate::settings::AiFeatureMode::Light;
        app.settings.global_preset.upscale_model = Some("realesr_general_v3".into());
        let key = app
            .final_ai_key_for_pixels(
                app.current_edit_result_key(0),
                [800, 600],
                app.effective_params(0),
            )
            .unwrap();
        (app, key)
    }
    fn pending(job: u64, priority: AiJobPriority) -> FinalAiPending {
        FinalAiPending {
            feedback_request: FinalAiFeedbackRequest {
                priority,
                source: [800, 600],
            },
            job_id: job,
            cancel: Arc::new(AtomicBool::new(false)),
            retained_key: None,
            retained_epoch: 0,
            allow_retained_orphan: false,
            cancel_on_drop: true,
        }
    }
    fn failed(key: FinalAiKey, job: u64) -> FinalAiResult {
        FinalAiResult::Failed {
            diagnostic: None,
            job_id: job,
            key,
            error: FinalAiFailure::from(AiError::OrtStatus {
                code: 6,
                detail: PRIVATE.into(),
            }),
        }
    }
    fn report(app: &App) -> &crate::feedback::AiFailureReport {
        let Some(crate::feedback::FeedbackAction::Diagnostic(report)) =
            &app.fs_feedback_toast_action
        else {
            panic!("missing diagnostic action");
        };
        report
    }

    #[test]
    fn ai_failure_feedback_live_display_is_safe_and_notified_once_per_failed_key() {
        let (mut app, key) = setup();
        app.final_ai_pending
            .insert(key, pending(7, AiJobPriority::Display));
        app.final_ai_result_backlog.push(failed(key, 7));
        let ctx = egui::Context::default();
        app.poll_final_ai(&ctx);
        assert_eq!(app.feedback_toasts_for_test.len(), 1);
        assert_eq!(
            app.feedback_toasts_for_test[0].1,
            crate::feedback::FeedbackKind::Error
        );
        assert_eq!(app.fs_feedback_toast_surface, Some(ActionSurface::Viewer));
        let body = report(&app).body().to_owned();
        assert!(body.contains("エラー番号: 6"));
        assert!(body.contains("処理番号 7"));
        assert!(body.contains("ページ 1/1"));
        for forbidden in [
            PRIVATE,
            "secret.zip",
            "page.png",
            "search=query",
            "driver raw",
            "OOM",
        ] {
            assert!(!body.contains(forbidden));
            assert!(!app.feedback_toasts_for_test[0].0.contains(forbidden));
        }
        app.final_ai_result_backlog.push(failed(key, 7));
        app.poll_final_ai(&ctx);
        // Even an artificial duplicate live entry cannot bypass failed-key lifetime.
        app.final_ai_pending
            .insert(key, pending(8, AiJobPriority::Display));
        app.final_ai_result_backlog.push(failed(key, 8));
        app.poll_final_ai(&ctx);
        assert_eq!(app.feedback_toasts_for_test.len(), 1);
        assert_eq!(report(&app).body(), body);
        app.final_ai_failed.remove(&key); // Existing invalidation owns the lifetime.
        app.final_ai_pending
            .insert(key, pending(9, AiJobPriority::Display));
        app.final_ai_result_backlog.push(failed(key, 9));
        app.poll_final_ai(&ctx);
        assert_eq!(app.feedback_toasts_for_test.len(), 2);
    }

    #[test]
    fn ai_failure_feedback_ignores_prefetch_cancelled_stale_job_key_page_and_orphan() {
        for scenario in 0..9 {
            let (mut app, key) = setup();
            let pending = pending(
                7,
                if scenario == 0 {
                    AiJobPriority::Prefetch
                } else {
                    AiJobPriority::Display
                },
            );
            if scenario == 1 {
                pending.cancel.store(true, Ordering::Relaxed);
            }
            app.final_ai_pending.insert(key, pending);
            match scenario {
                2 => {
                    app.items_generation += 1;
                }
                3 => {
                    app.fullscreen_idx = None;
                }
                4 => {
                    app.settings.global_preset.brightness = 10.0;
                }
                5 => {
                    app.input_generation.insert(0, 1);
                }
                6 => {
                    app.final_ai_pending.remove(&key);
                    app.retained_final_ai_orphans.insert(key, 7);
                }
                7 => {
                    app.items[0] = GridItem::Image(PathBuf::from("different-item.png"));
                }
                _ => {}
            }
            app.final_ai_result_backlog
                .push(failed(key, if scenario == 8 { 6 } else { 7 }));
            app.poll_final_ai(&egui::Context::default());
            assert!(
                app.feedback_toasts_for_test.is_empty(),
                "scenario {scenario}"
            );
            assert!(
                app.fs_feedback_toast_action.is_none(),
                "scenario {scenario}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn ai_failure_feedback_is_deferred_and_scoped_to_the_owning_context() {
        let (mut app, key) = setup();
        app.final_ai_pending
            .insert(key, pending(7, AiJobPriority::Display));
        let owner = app.stash_mounted_as_active_for_test(65507);
        app.final_ai_result_backlog.push(failed(key, 7));
        app.poll_final_ai(&egui::Context::default());
        assert!(app.feedback_toasts_for_test.is_empty());
        assert_eq!(app.final_ai_result_backlog.len(), 1);
        app.with_active_viewer_context(|mounted| {
            mounted.poll_final_ai(&egui::Context::default());
            assert_eq!(report(mounted).owner(), owner.serial());
            assert!(mounted.feedback_action_matches_mounted_context());
        });
        assert_eq!(app.feedback_toasts_for_test.len(), 1);
        assert!(
            !app.feedback_action_matches_mounted_context(),
            "sibling must never draw/copy this toast"
        );
        assert!(app.final_ai_failed.is_empty());
    }
}
