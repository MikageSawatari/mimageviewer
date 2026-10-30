//! Cancellation terminals that do not adopt display results or upload textures.
//! The normal UI polls keep their existing acceptance rules and call order.

use super::*;

impl App {
    /// The shared receiver and existing backlog have one collection implementation.
    /// Callers retain their own terminal policy, including the legacy UI disconnect tail.
    pub(super) fn collect_final_ai_results(&mut self) -> (Vec<FinalAiResult>, bool) {
        let mut completed = std::mem::take(&mut self.final_ai_result_backlog);
        let mut disconnected = false;
        if let Some(rx) = self.final_ai_rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(result) => completed.push(result),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        (completed, disconnected)
    }

    /// Consume only the exact retained orphan. This is the existing CPU cache tail;
    /// it neither populates the live final cache nor invalidates display effects.
    pub(super) fn finish_retained_final_ai_orphan_result(
        &mut self,
        result: FinalAiResult,
    ) -> Result<(), FinalAiResult> {
        let (key, job_id) = Self::final_ai_result_key_job(&result);
        if self.retained_final_ai_orphans.get(&key) != Some(&job_id) {
            return Err(result);
        }
        self.retained_final_ai_orphans.remove(&key);
        let diagnostic = result.diagnostic().clone();
        diagnostic_stage(&diagnostic, JobStage::Adopting);
        match result {
            FinalAiResult::Ready {
                canonical_dims,
                retained_key,
                retained_epoch,
                source_size,
                image,
                used_upscale,
                ..
            } => {
                let retained_stored = retained_key.is_some_and(|retained_key| {
                    self.insert_retained_final_ai_with_key_and_dimensions(
                        key.edit_key.idx,
                        retained_key,
                        retained_epoch,
                        Arc::new(image),
                        used_upscale,
                        canonical_dims,
                    )
                });
                diagnostic_stage(
                    &diagnostic,
                    if retained_stored {
                        JobStage::Completed
                    } else {
                        JobStage::Discarded
                    },
                );
                let action = if retained_stored { "stored" } else { "skipped" };
                crate::logger::log(format!(
                    "[AI] Final AI orphan result {action} for retained cache only idx={} \
                     source={}x{} job_id={job_id}",
                    key.edit_key.idx, source_size[0], source_size[1]
                ));
            }
            FinalAiResult::Cancelled { .. } => diagnostic_stage(&diagnostic, JobStage::Discarded),
            FinalAiResult::Failed { error, .. } => {
                diagnostic_stage(&diagnostic, JobStage::Failed);
                crate::logger::log(format!(
                    "[AI] Final AI orphan failed idx={} job_id={job_id} error={error}",
                    key.edit_key.idx
                ));
            }
        }
        Ok(())
    }

    /// Remote/Close can reap discarded work without invoking a display poll.
    /// Live results remain in the existing backlog for the owning viewer's UI.
    pub(crate) fn service_cancelled_ai_terminals(&mut self) {
        self.ai_upscale_pending.retain(|_, (cancel, rx)| {
            !cancel.load(Ordering::Relaxed)
                || matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty))
        });

        let (completed, disconnected) = self.collect_final_ai_results();
        let contexts = self.viewer_context_ids();
        for result in completed {
            let (key, job_id) = Self::final_ai_result_key_job(&result);
            let mut live_owner = false;
            let mut retired = false;
            for &context in &contexts {
                let disposition = self.with_viewer_context_state(context, |owner| {
                    let pending = owner.final_ai_pending.get(&key)?;
                    if pending.job_id != job_id {
                        return None;
                    }
                    if pending.cancel.load(Ordering::Relaxed)
                        || matches!(result, FinalAiResult::Cancelled { .. })
                    {
                        owner.final_ai_pending.remove(&key);
                        owner.final_ai_failed.remove(&key);
                        Some(true)
                    } else {
                        Some(false)
                    }
                });
                if let Ok(Some(was_retired)) = disposition {
                    retired = was_retired;
                    live_owner = !was_retired;
                    break;
                }
            }
            if retired {
                // A cancel racing Ready must never publish either live or retained pixels.
                if self.retained_final_ai_orphans.get(&key) == Some(&job_id) {
                    self.retained_final_ai_orphans.remove(&key);
                }
            } else if live_owner {
                self.final_ai_result_backlog.push(result);
            } else {
                let _ = self.finish_retained_final_ai_orphan_result(result);
            }
        }
        if disconnected {
            // No terminal can follow a disconnected shared receiver. Retire only
            // canceled owners here; the UI retains its existing live failure policy.
            for context in contexts {
                let _ = self.with_viewer_context_state(context, |owner| {
                    owner
                        .final_ai_pending
                        .retain(|_, pending| !pending.cancel.load(Ordering::Relaxed));
                });
            }
            self.retained_final_ai_orphans.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> FinalAiKey {
        FinalAiKey {
            edit_key: EditResultKey {
                item_id: crate::item_identity::ItemId::NONE,
                items_generation: 0,
                idx: 0,
                source_gen: 0,
                erase_mask_gen: 0,
                local_gen: 0,
                conceal_mask_gen: 0,
                conceal_gen: 0,
            },
            color_ai_hash: 0xCAFE,
            bg: 0,
        }
    }

    fn pending(job_id: u64, cancelled: bool) -> FinalAiPending {
        FinalAiPending {
            job_id,
            cancel: Arc::new(AtomicBool::new(cancelled)),
            retained_key: None,
            retained_epoch: 0,
            allow_retained_orphan: false,
            cancel_on_drop: true,
        }
    }

    fn ready(key: FinalAiKey, job_id: u64) -> FinalAiResult {
        FinalAiResult::Ready {
            diagnostic: None,
            job_id,
            key,
            canonical_dims: None,
            retained_key: None,
            retained_epoch: 0,
            source_size: [1, 1],
            image: egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            used_upscale: false,
        }
    }

    #[test]
    fn egui037_continuation_ai_cancelled_upscale_ready_retires_without_upload() {
        let mut app = crate::app::setup_app_for_test();
        let (cancelled_tx, cancelled_rx) = mpsc::channel();
        let (live_tx, live_rx) = mpsc::channel();
        app.ai_upscale_pending
            .insert((0, 0), (Arc::new(AtomicBool::new(true)), cancelled_rx));
        app.ai_upscale_pending
            .insert((1, 0), (Arc::new(AtomicBool::new(false)), live_rx));
        for (tx, idx) in [(cancelled_tx, 0), (live_tx, 1)] {
            tx.send(crate::ai::upscale::UpscaleResult {
                idx,
                image: egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            })
            .unwrap();
        }
        let generations = app.adjustment_generation.clone();
        app.service_cancelled_ai_terminals();
        assert!(!app.ai_upscale_pending.contains_key(&(0, 0)));
        assert!(app.ai_upscale_pending.contains_key(&(1, 0)));
        assert_eq!(app.ai_upscale_pending[&(1, 0)].1.try_recv().unwrap().idx, 1);
        assert!(app.ai_upscale_cache.is_empty());
        assert!(app.ai_upscale_failed.is_empty());
        assert_eq!(app.adjustment_generation, generations);
    }

    #[test]
    fn egui037_continuation_ai_live_result_waits_in_existing_backlog_for_ui() {
        let mut app = crate::app::setup_app_for_test();
        let (tx, rx) = mpsc::channel();
        app.final_ai_rx = Some(rx);
        let key = key();
        app.final_ai_pending.insert(key, pending(7, false));
        tx.send(ready(key, 7)).unwrap();
        app.service_cancelled_ai_terminals();
        app.service_cancelled_ai_terminals();
        assert_eq!(app.final_ai_result_backlog.len(), 1);
        assert_eq!(app.final_ai_pending[&key].job_id, 7);
        assert!(app.final_ai_cache.is_empty());
        app.poll_final_ai(&egui::Context::default());
        assert!(app.final_ai_cache.contains_key(&key));
        assert!(app.final_ai_pending.is_empty());
        assert!(app.final_ai_result_backlog.is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn egui037_continuation_ai_cancelled_ready_retires_only_exact_context_job() {
        let mut app = crate::app::setup_app_for_test();
        let (tx, rx) = mpsc::channel();
        app.final_ai_rx = Some(rx);
        let key = key();
        app.final_ai_pending.insert(key, pending(10, false));
        let detached =
            app.build_active_context_for_test(Some(836), DetachedSource::Image, |owner| {
                owner.final_ai_pending.insert(key, pending(11, true));
            });
        let mut cancelled_ready = ready(key, 11);
        if let FinalAiResult::Ready {
            retained_key,
            retained_epoch,
            ..
        } = &mut cancelled_ready
        {
            *retained_key = Some(RetainedFinalAiKey {
                raw_source: None,
                item_key: "c:/books/cancelled.pdf::page_0".into(),
                edit_size: [1, 1],
                color_ai_hash: key.color_ai_hash,
                bg: key.bg,
            });
            *retained_epoch = app.retained_final_ai_epoch;
        }
        tx.send(cancelled_ready).unwrap();
        let generations = app.adjustment_generation.clone();
        app.service_cancelled_ai_terminals();
        assert_eq!(app.final_ai_pending[&key].job_id, 10);
        assert!(!app.final_ai_pending[&key].cancel.load(Ordering::Relaxed));
        assert!(app.final_ai_cache.is_empty());
        assert!(app.retained_final_ai_cache.is_empty());
        assert!(app.final_ai_result_backlog.is_empty());
        assert_eq!(app.adjustment_generation, generations);
        app.with_viewer_context_state(detached, |owner| {
            assert!(owner.final_ai_pending.is_empty());
            assert!(owner.final_ai_cache.is_empty());
            assert!(owner.final_ai_failed.is_empty());
        })
        .unwrap();
    }

    #[test]
    fn egui037_continuation_ai_disconnect_retires_cancelled_pending_and_keeps_live_for_ui() {
        let mut app = crate::app::setup_app_for_test();
        let (tx, rx) = mpsc::channel();
        app.final_ai_rx = Some(rx);
        let cancelled_key = key();
        let mut live_key = cancelled_key;
        live_key.edit_key.idx = 1;
        app.final_ai_pending
            .insert(cancelled_key, pending(13, true));
        app.final_ai_pending.insert(live_key, pending(14, false));
        app.retained_final_ai_orphans.insert(cancelled_key, 15);
        drop(tx);
        app.service_cancelled_ai_terminals();
        assert!(!app.final_ai_pending.contains_key(&cancelled_key));
        assert_eq!(app.final_ai_pending[&live_key].job_id, 14);
        assert!(
            !app.final_ai_pending[&live_key]
                .cancel
                .load(Ordering::Relaxed)
        );
        assert!(app.retained_final_ai_orphans.is_empty());
        assert!(
            app.final_ai_rx.is_some(),
            "UI retains its disconnect terminal"
        );
        assert!(app.final_ai_cache.is_empty());
    }

    #[test]
    fn egui037_continuation_ai_orphan_terminal_releases_barrier_with_cpu_cache_only() {
        let mut app = crate::app::setup_app_for_test();
        let (tx, rx) = mpsc::channel();
        app.final_ai_rx = Some(rx);
        let key = key();
        let retained_key = RetainedFinalAiKey {
            raw_source: None,
            item_key: "c:/books/orphan.pdf::page_0".into(),
            edit_size: [1, 1],
            color_ai_hash: key.color_ai_hash,
            bg: key.bg,
        };
        app.retained_final_ai_orphans.insert(key, 12);
        assert!(!app.local_ai_remote_barrier_snapshot().is_quiesced());
        tx.send(FinalAiResult::Ready {
            diagnostic: None,
            job_id: 12,
            key,
            canonical_dims: None,
            retained_key: Some(retained_key.clone()),
            retained_epoch: app.retained_final_ai_epoch,
            source_size: [1, 1],
            image: egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            used_upscale: false,
        })
        .unwrap();
        let generations = app.adjustment_generation.clone();
        app.service_cancelled_ai_terminals();
        assert!(app.retained_final_ai_orphans.is_empty());
        assert!(app.retained_final_ai_cache.contains_key(&retained_key));
        assert!(app.local_ai_remote_barrier_snapshot().is_quiesced());
        assert!(app.final_ai_cache.is_empty());
        assert!(app.ai_upscale_cache.is_empty());
        assert_eq!(app.adjustment_generation, generations);
    }
}
