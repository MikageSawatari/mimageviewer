//! Scalar-only connections at existing viewer/AI ownership boundaries.
use super::{App, FinalAiKey, FinalCompositeKey};
use crate::ai::ModelKind;
use crate::gpu_info::diagnostic_sampler::AdapterProbe;
use crate::previous_run_diagnostics::checkpoint::{
    CheckpointPublisher, DiagnosticBackend, DiagnosticModel, Dimensions, DisplaySnapshot, JobLease,
    JobMetadata, JobStage, JobUpdate,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

fn dimensions(size: [usize; 2]) -> Option<Dimensions> {
    Some(Dimensions {
        width: size[0].try_into().ok()?,
        height: size[1].try_into().ok()?,
    })
}

impl App {
    pub(crate) fn install_diagnostic_checkpoint(
        &mut self,
        publisher: Option<CheckpointPublisher>,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        if let (Some(publisher), Some(render_state)) = (&publisher, render_state) {
            publisher.set_adapter(AdapterProbe::from_render_state(render_state));
        }
        if let Some(publisher) = &publisher {
            self.ai_runtime_init
                .install_diagnostic_checkpoint(publisher.clone());
        }
        self.diagnostic_checkpoint = publisher;
        self.publish_diagnostic_display();
    }

    pub(crate) fn publish_diagnostic_display(&self) {
        let Some(publisher) = &self.diagnostic_checkpoint else {
            return;
        };
        // Existing source-list positions only: do not rebuild navigation/seek
        // caches, classify pages, format a path, or enumerate the viewer here.
        publisher.publish_display(DisplaySnapshot {
            context_id: self.projected_viewer_context_id().serial(),
            items_generation: self.items_generation,
            page: self
                .fullscreen_idx
                .and_then(|idx| idx.checked_add(1)?.try_into().ok()),
            total: self.items.len().try_into().ok(),
        });
    }

    pub(crate) fn diagnostic_job_metadata(
        &self,
        job_id: u64,
        key: FinalAiKey,
        source: [usize; 2],
        upscale_model: Option<ModelKind>,
        denoise_model: Option<ModelKind>,
    ) -> JobMetadata {
        let current = key.edit_key.items_generation == self.items_generation
            && key.edit_key.idx < self.items.len();
        JobMetadata {
            context_id: Some(self.projected_viewer_context_id().serial()),
            items_generation: Some(key.edit_key.items_generation),
            job_id,
            page: current
                .then(|| key.edit_key.idx.checked_add(1)?.try_into().ok())
                .flatten(),
            total: current.then(|| self.items.len().try_into().ok()).flatten(),
            upscale_model: upscale_model.map(Into::into),
            denoise_model: denoise_model.map(Into::into),
            // A configured backend is not evidence of the effective provider.
            backend: DiagnosticBackend::Unknown,
            source: dimensions(source),
            source_bytes: u64::try_from(source[0])
                .ok()
                .and_then(|width| width.checked_mul(u64::try_from(source[1]).ok()?))
                .and_then(|pixels| pixels.checked_mul(4)),
        }
    }

    pub(crate) fn diagnostic_post_ai(
        &self,
        key: FinalCompositeKey,
        source: [usize; 2],
        cancel: Arc<AtomicBool>,
    ) -> Option<JobLease> {
        let publisher = self.diagnostic_checkpoint.as_ref()?;
        let idx = key.edit_key.idx;
        let params = self.effective_params(idx);
        let ai_key = self.final_ai_key_for_pixels(key.edit_key, source, params)?;
        let metadata = self
            .final_ai_cache
            .get(&ai_key)
            .and_then(|entry| entry.diagnostic_metadata)
            .unwrap_or_else(|| {
                let upscale_request = self.effective_upscale_request(params);
                let upscale = upscale_request.flatten().or_else(|| {
                    upscale_request?;
                    self.ai_classify_cache.get(&idx).and_then(|category| {
                        self.settings.ai_feature_mode.auto_upscale_model(*category)
                    })
                });
                let mut metadata = self.diagnostic_job_metadata(
                    0,
                    ai_key,
                    source,
                    upscale,
                    self.effective_denoise_request(params),
                );
                if upscale_request.is_some() && metadata.upscale_model.is_none() {
                    metadata.upscale_model = Some(DiagnosticModel::Unknown);
                }
                metadata
            });
        let lease = publisher.begin_job(metadata, cancel);
        lease.update(JobUpdate {
            stage: JobStage::FinalEffectPending,
            ..Default::default()
        });
        Some(lease)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::phase_c_support;
    use crate::app::{FinalAiEntry, FinalAiPending, FinalAiResult, GridItem};
    use crate::previous_run_diagnostics::test_support;
    use std::path::PathBuf;

    fn setup() -> (phase_c_support::AppTestEnv, CheckpointPublisher) {
        let mut app = phase_c_support::setup_app();
        app.items = vec![GridItem::Image(PathBuf::from(
            "diagnostic-fixture-no-read.png",
        ))];
        app.fullscreen_idx = Some(0);
        let publisher = test_support::publisher();
        app.install_diagnostic_checkpoint(Some(publisher.clone()), None);
        (app, publisher)
    }

    #[test]
    fn display_uses_only_installed_context_generation_and_item_position() {
        let (mut app, publisher) = setup();
        assert_eq!(
            publisher.display_for_test(),
            Some(DisplaySnapshot {
                context_id: app.projected_viewer_context_id().serial(),
                items_generation: app.items_generation,
                page: Some(1),
                total: Some(1),
            })
        );
        app.fullscreen_idx = None;
        app.publish_diagnostic_display();
        assert_eq!(publisher.display_for_test().unwrap().page, None);
        assert_eq!(publisher.active_count_for_test(), 0);
    }

    #[test]
    fn real_result_backlog_keeps_activity_after_pending_is_removed() {
        let (mut app, publisher) = setup();
        let key = FinalAiKey {
            edit_key: app.current_edit_result_key(0),
            color_ai_hash: 1,
            bg: 0,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let lease = publisher.begin_job(
            app.diagnostic_job_metadata(
                7,
                key,
                [2, 3],
                Some(ModelKind::UpscaleRealEsrGeneralV3),
                None,
            ),
            cancel.clone(),
        );
        app.final_ai_pending.insert(
            key,
            FinalAiPending {
                job_id: 7,
                cancel,
                retained_key: None,
                retained_epoch: 0,
                allow_retained_orphan: true,
                cancel_on_drop: true,
            },
        );
        app.final_ai_result_backlog.push(FinalAiResult::Cancelled {
            diagnostic: Some(lease),
            job_id: 7,
            key,
        });
        drop(app.final_ai_pending.remove(&key));
        assert!(app.final_ai_pending.is_empty());
        assert_eq!(publisher.active_count_for_test(), 1);
        app.final_ai_result_backlog.clear();
        assert_eq!(publisher.active_count_for_test(), 0);
    }

    #[test]
    fn post_ai_reuses_only_exact_cached_metadata_and_cache_does_not_keep_activity() {
        let (mut app, publisher) = setup();
        app.settings.ai_feature_mode = crate::settings::AiFeatureMode::Light;
        app.settings.global_preset.upscale_model = Some("realesr_general_v3".into());
        let source = [2, 3];
        let edit = app.current_edit_result_key(0);
        let params = app.effective_params(0);
        let key = app.final_composite_key_for_pixels(edit, source, params);
        let ai_key = app.final_ai_key_for_pixels(edit, source, params).unwrap();
        let metadata = app.diagnostic_job_metadata(
            9,
            ai_key,
            source,
            Some(ModelKind::UpscaleRealEsrGeneralV3),
            None,
        );
        app.final_ai_cache.insert(
            ai_key,
            FinalAiEntry {
                diagnostic_metadata: Some(metadata),
                canonical_dims: None,
                source_size: source,
                pixels: Arc::new(egui::ColorImage::filled(source, egui::Color32::WHITE)),
                used_upscale: true,
            },
        );
        assert_eq!(publisher.active_count_for_test(), 0);
        let lease = app
            .diagnostic_post_ai(key, source, Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert_eq!(lease.metadata(), metadata);
        assert_eq!(publisher.active_count_for_test(), 1);
        drop(lease);
        assert_eq!(publisher.active_count_for_test(), 0);
        app.final_ai_cache.clear();
        let restored = app
            .diagnostic_post_ai(key, source, Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert_eq!(restored.metadata().job_id, 0);
        assert_eq!(
            restored.metadata().context_id,
            Some(app.projected_viewer_context_id().serial())
        );
        drop(restored);
        assert_eq!(publisher.active_count_for_test(), 0);
        app.settings.ai_feature_mode = crate::settings::AiFeatureMode::Disabled;
        assert!(
            app.diagnostic_post_ai(key, source, Arc::new(AtomicBool::new(false)))
                .is_none()
        );
    }
}
