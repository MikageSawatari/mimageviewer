use super::*;
use crate::app::final_prepare::tests::app;

#[test]
fn typed_preflight_reports_the_first_broken_proof_and_bool_uses_the_same_result() {
    for reason in [
        PreparedEditReject::Cancelled,
        PreparedEditReject::ItemsGeneration,
        PreparedEditReject::ItemIdentity,
        PreparedEditReject::EditGeneration,
        PreparedEditReject::CompositeKey,
        PreparedEditReject::Context,
        PreparedEditReject::PageIdentity,
        PreparedEditReject::Params,
        PreparedEditReject::ProducerConflict,
        PreparedEditReject::LayersChanged,
        PreparedEditReject::NormalizationRequired,
        PreparedEditReject::FinalBaseChanged,
        PreparedEditReject::SourceMissing,
        PreparedEditReject::SourceArcChanged,
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let (key, _) = app.seed_prepared_edit_entry_test(&ctx, 1, false);
        let mut pending = app.final_effect_pending.remove(&key).unwrap();
        assert_eq!(app.validate_final_prepare_pending(key, &pending), Ok(()));
        let mut lookup = key;
        match reason {
            PreparedEditReject::Cancelled => pending.cancel.store(true, Ordering::Relaxed),
            PreparedEditReject::ItemsGeneration => app.items_generation += 1,
            PreparedEditReject::Context => {
                let FinalEffectOrigin::Speculative(unit) = &mut pending.origin else {
                    unreachable!()
                };
                unit.context = ViewerContextId::for_test(98765);
            }
            PreparedEditReject::CompositeKey => lookup.params_hash ^= 1,
            PreparedEditReject::EditGeneration => {
                *app.local_adjust_generation.entry(1).or_default() += 1
            }
            PreparedEditReject::Params => {
                app.adjustment_page_params.get_mut(&1).unwrap().brightness += 1.0
            }
            PreparedEditReject::ProducerConflict => app.maybe_start_local_adjust_render(1),
            PreparedEditReject::LayersChanged => {
                app.local_adjust_page_layers
                    .insert(1, local_adjust_core::LocalAdjustmentLayers::new(Vec::new()));
            }
            PreparedEditReject::SourceMissing => {
                app.fs_cache.remove(&1);
            }
            PreparedEditReject::SourceArcChanged => {
                let FsCacheEntry::Static { pixels, .. } = app.fs_cache.get_mut(&1).unwrap() else {
                    unreachable!()
                };
                *pixels = Arc::new((**pixels).clone());
            }
            _ => {
                let FinalEffectState::CpuReady(FinalEffectReady::Edited(ready)) =
                    &mut pending.state
                else {
                    unreachable!()
                };
                let input = Arc::get_mut(&mut ready.input).unwrap();
                match reason {
                    PreparedEditReject::ItemIdentity => input.key.edit_key.item_id = app.item_id(2),
                    PreparedEditReject::PageIdentity => input.page_key.push_str("-changed"),
                    PreparedEditReject::NormalizationRequired => input.local = None,
                    PreparedEditReject::FinalBaseChanged => {
                        input.final_input = PreparedFinalInput::Completed {
                            pixels: input.source.clone(),
                            basis: input.basis,
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
        assert_eq!(
            app.validate_final_prepare_pending(lookup, &pending),
            Err(reason),
            "{reason:?}"
        );
        assert!(
            !app.final_prepare_pending_is_current(lookup, &pending),
            "{reason:?}"
        );
    }
}

#[test]
fn cancellation_records_survive_perf_disabled_and_precede_owner_removal() {
    for reason in [
        PreparedEditCancel::EditingEntry,
        PreparedEditCancel::KeepSetEviction,
        PreparedEditCancel::DemandObsolete,
        PreparedEditCancel::PipelineInvalidation(FinalPipelineInvalidation::AdjustmentChanged),
    ] {
        let trace = PreparedEditEvents::start();
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let (key, cancel) = app.seed_prepared_edit_entry_test(&ctx, 1, false);
        app.frame_counter = 37;
        let context = app.projected_viewer_context_id().serial();
        match reason {
            PreparedEditCancel::EditingEntry => app.cancel_prepared_edit_for_entry(1),
            PreparedEditCancel::KeepSetEviction => {
                app.evict_final_pipeline_cache_for_keep_set(&std::collections::HashSet::from([0]))
            }
            PreparedEditCancel::DemandObsolete => {
                app.settings.page_turn_animation_enabled = false;
                app.prefetch_next_final_unit(&ctx, 0);
            }
            PreparedEditCancel::PipelineInvalidation(cause) => {
                app.clear_all_final_pipeline_caches(cause)
            }
            _ => unreachable!(),
        }
        let events = trace.take();
        assert_eq!(events.len(), 1, "{events:?}");
        let event = &events[0];
        assert_eq!(event.reason, PreparedEditDisposition::Cancelled(reason));
        assert_eq!(event.key, key);
        assert_eq!(event.context, context);
        assert_eq!(event.state, "cpu_ready");
        assert_eq!(event.frame, Some(37));
        assert!(event.entry.line() > 0);
        assert!(!app.final_effect_pending.contains_key(&key));
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}
