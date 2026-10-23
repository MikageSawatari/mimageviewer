use super::*;
use crate::adjustment::AdjustParams;

pub(super) struct PageParamWritePlan {
    target: PageAdjustmentTarget,
    document: Option<AdjustParams>,
    new_effective: AdjustParams,
    old_effective: AdjustParams,
    old_by_idx: Vec<(usize, AdjustParams)>,
    clear: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
    use crate::undo_stack::{AdjustUndoScope, AdjustmentChange};

    fn params(brightness: f32) -> AdjustParams {
        AdjustParams {
            brightness,
            ..Default::default()
        }
    }

    fn pages() -> AppTestEnv {
        let mut app = setup_app();
        let root = app.tmp.path().join("pages");
        std::fs::create_dir_all(&root).unwrap();
        app.items = (0..3)
            .map(|idx| GridItem::Image(root.join(format!("p{idx}.jpg"))))
            .collect();
        app.current_folder = Some(root);
        app.thumbnails = vec![ThumbnailState::Pending; 3];
        app.visible_indices = vec![0, 1, 2];
        app
    }

    fn assert_rows(app: &App, expected: &[Option<AdjustParams>]) {
        for (idx, value) in expected.iter().enumerate() {
            let key = app.page_path_key(idx).unwrap();
            assert_eq!(
                app.adjustment_page_params.get(&idx),
                value.as_ref(),
                "runtime {idx}"
            );
            assert_eq!(
                app.adjustment_db.as_ref().unwrap().get_page_params(&key),
                *value,
                "DB {idx}"
            );
            assert_eq!(
                app.adjusted_page_keys.contains(&key),
                value.is_some(),
                "presence {idx}"
            );
        }
    }

    #[test]
    fn bulk_slot_roundtrip_matches_single_writes_with_defaults_and_ai() {
        let mut app = pages();
        let slot = params(10.0);
        // A favorite directory applies only to the first page's separate location.
        let favorite_root = app.tmp.path().join("favorite");
        app.items[0] = GridItem::Image(favorite_root.join("p0.jpg"));
        let favorite = crate::settings::FavoriteEntry::new("default".into(), favorite_root);
        app.adjustment_favorite_params
            .insert(favorite.id, slot.clone());
        app.settings.favorites.push(favorite);
        let mut ai = params(3.0);
        ai.upscale_model = Some("test-ai".into());
        app.set_page_params(0, params(2.0));
        app.set_page_params(1, ai.clone());
        let before = vec![Some(params(2.0)), Some(ai), None];
        let expected: Vec<_> = (0..3)
            .map(|idx| {
                let target = app.page_adjustment_target_for_idx(idx).unwrap();
                (slot != app.adjustment_standard_params_for_target(&target)).then(|| slot.clone())
            })
            .collect();
        assert!(expected[0].is_none());
        for idx in 0..3 {
            app.set_page_params(idx, slot.clone());
        }
        assert_rows(&app, &expected);
        for (idx, value) in before.iter().enumerate() {
            let target = app.page_adjustment_target_for_idx(idx).unwrap();
            app.restore_page_params_for_target(&target, value.clone());
        }
        app.settings.preset_slots.slots[0] = Some(crate::adjustment::PresetSlot {
            name: "bulk".into(),
            params: slot,
        });
        app.checked.extend([0, 1, 2]);
        app.capture_adjust_full("bulk".into(), |app| app.apply_slot_to_grid_selection(0));
        assert_rows(&app, &expected);
        app.apply_meta_undo();
        assert_rows(&app, &before);
        app.apply_meta_redo();
        assert_rows(&app, &expected);
        app.clear_page_params_for_indices(&[0, 1, 2]);
        assert_rows(&app, &[None, None, None]);
    }

    #[test]
    fn bulk_restore_keeps_global_favorite_and_page_segment_order() {
        let mut app = pages();
        let root = app.current_folder.clone().unwrap();
        let favorite = crate::settings::FavoriteEntry::new("fav".into(), root);
        let id = favorite.id;
        app.settings.favorites.push(favorite);
        app.settings.global_preset = params(30.0);
        app.adjustment_favorite_params.insert(id, params(40.0));
        let target = app.page_adjustment_target_for_idx(0).unwrap();
        app.push_adjustment_undo_entry(
            vec![
                AdjustmentChange {
                    scope: AdjustUndoScope::Global,
                    before: Some(params(10.0)),
                    after: Some(params(30.0)),
                },
                AdjustmentChange {
                    scope: AdjustUndoScope::Favorite(id),
                    before: Some(params(20.0)),
                    after: Some(params(40.0)),
                },
                AdjustmentChange {
                    scope: AdjustUndoScope::PageKey(target),
                    before: Some(params(40.0)),
                    after: None,
                },
                AdjustmentChange {
                    scope: AdjustUndoScope::Page(1),
                    before: Some(params(40.0)),
                    after: None,
                },
            ],
            "mixed".into(),
        );
        app.apply_meta_undo();
        assert_eq!(app.settings.global_preset, params(10.0));
        assert_eq!(app.adjustment_favorite_params[&id], params(20.0));
        assert_rows(&app, &[Some(params(40.0)), Some(params(40.0)), None]);
        app.apply_meta_redo();
        assert_rows(&app, &[None, None, None]);
    }

    #[test]
    fn failed_bulk_write_keeps_all_db_and_sidecar_rows_and_runtime_policy() {
        let mut app = pages();
        for idx in 0..3 {
            app.set_page_params(idx, params(2.0));
        }
        let sidecars: Vec<_> = app
            .sidecars
            .iter()
            .map(|(folder, sidecar)| {
                (
                    folder.clone(),
                    serde_json::to_value(sidecar.items()).unwrap(),
                )
            })
            .collect();
        let connection =
            rusqlite::Connection::open(crate::adjustment_db::AdjustmentDb::db_path()).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_second BEFORE UPDATE ON page_params
            WHEN NEW.page_path LIKE '%p1.jpg' BEGIN SELECT RAISE(ABORT, 'injected bulk failure'); END;").unwrap();
        let writes: Vec<_> = (0..3)
            .map(|idx| {
                (
                    app.page_adjustment_target_for_idx(idx).unwrap(),
                    Some(params(8.0)),
                )
            })
            .collect();
        let epoch = &crate::page_edit_write_epoch::PAGE_EDIT_WRITES;
        let before = epoch.sample();
        let toast_count = app.feedback_toasts_for_test.len();
        app.restore_page_params_for_targets(&writes);
        let notifications = &app.feedback_toasts_for_test[toast_count..];
        assert_eq!(
            notifications.len(),
            1,
            "one failure notification for the whole segment"
        );
        assert_eq!(notifications[0].1, crate::feedback::FeedbackKind::Warning);
        assert!(notifications[0].0.contains("injected bulk failure"));
        let after = epoch.sample();
        assert_eq!(after.completed_writes, before.completed_writes + 1);
        let mut changed = epoch
            .changed_keys_since(before.completed_writes, after.completed_writes)
            .unwrap();
        changed.sort();
        let mut expected: Vec<_> = writes
            .iter()
            .map(|(target, _)| target.page_key.clone())
            .collect();
        expected.sort();
        assert_eq!(changed, expected, "only the batch keys are invalidated");
        for (idx, (target, _)) in writes.iter().enumerate() {
            assert_eq!(
                app.adjustment_db
                    .as_ref()
                    .unwrap()
                    .get_page_params(&target.page_key),
                Some(params(2.0))
            );
            assert_eq!(
                app.adjustment_page_params[&idx],
                params(8.0),
                "retain single-write failure policy"
            );
        }
        for (folder, previous) in sidecars {
            assert_eq!(
                serde_json::to_value(app.sidecars[&folder].items()).unwrap(),
                previous
            );
        }
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("injected bulk failure")
        );
    }

    #[cfg(windows)]
    #[test]
    fn bulk_publication_updates_same_keys_and_preserves_other_context_pages() {
        let mut app = pages();
        let items = app.items.clone();
        let sibling = app.build_window_context_for_test(117600, |owner| {
            owner.items = items;
            owner.thumbnails = vec![ThumbnailState::Pending; 3];
            owner.adjustment_page_params.insert(2, params(90.0));
        });
        let writes: Vec<_> = (0..2)
            .map(|idx| {
                (
                    app.page_adjustment_target_for_idx(idx).unwrap(),
                    Some(params(15.0)),
                )
            })
            .collect();
        app.restore_page_params_for_targets(&writes);
        app.with_viewer_context(sibling, |owner| {
            assert_eq!(owner.adjustment_page_params.get(&0), Some(&params(15.0)));
            assert_eq!(owner.adjustment_page_params.get(&1), Some(&params(15.0)));
            assert_eq!(owner.adjustment_page_params.get(&2), Some(&params(90.0)));
        });
        let clears: Vec<_> = writes
            .into_iter()
            .map(|(target, _)| (target, None))
            .collect();
        app.restore_page_params_for_targets(&clears);
        app.with_viewer_context(sibling, |owner| {
            assert!(!owner.adjustment_page_params.contains_key(&0));
            assert!(!owner.adjustment_page_params.contains_key(&1));
            assert_eq!(owner.adjustment_page_params.get(&2), Some(&params(90.0)));
        });
    }
}

impl App {
    pub(super) fn plan_page_param_write(
        &self,
        target: &PageAdjustmentTarget,
        params: Option<AdjustParams>,
    ) -> PageParamWritePlan {
        let standard = self.adjustment_standard_params_for_target(target);
        let clear = params.is_none();
        let new_effective = params.clone().unwrap_or_else(|| standard.clone());
        PageParamWritePlan {
            target: target.clone(),
            document: params.filter(|params| *params != standard),
            new_effective,
            old_effective: self.effective_params_for_target(target),
            old_by_idx: self
                .page_adjustment_indices(target)
                .into_iter()
                .map(|idx| (idx, self.effective_params(idx).clone()))
                .collect(),
            clear,
        }
    }

    pub(super) fn persist_page_param_plans(
        &mut self,
        plans: &[PageParamWritePlan],
    ) -> EditStoreOutcome {
        let writes: Vec<_> = plans
            .iter()
            .map(|plan| (plan.target.page_key.clone(), plan.document.clone()))
            .collect();
        Self::edit_store_write(
            self.adjustment_db
                .as_mut()
                .map(|db| db.apply_page_param_writes(&writes)),
        )
    }

    pub(super) fn reflect_page_param_plan(&mut self, plan: &PageParamWritePlan, committed: bool) {
        for &(idx, _) in &plan.old_by_idx {
            match &plan.document {
                Some(params) => {
                    self.adjustment_page_params.insert(idx, params.clone());
                }
                None => {
                    self.adjustment_page_params.remove(&idx);
                }
            }
            self.sync_prepared_page_edit_key_for_idx(idx);
        }
        Self::set_page_key_presence(
            &mut self.adjusted_page_keys,
            &plan.target.page_key,
            plan.document.is_some(),
        );
        // R-26: 正本が拒否した値はバックアップへ写さない。
        if committed {
            let document = plan.document.clone();
            self.with_sidecar_coords_mut(
                plan.target.sidecar_coords.as_ref(),
                move |sidecar, rel| match document {
                    Some(params) => sidecar.set_adjust(rel, params),
                    None => sidecar.remove_adjust(rel),
                },
            );
        }
        for (idx, old) in &plan.old_by_idx {
            let new = if plan.clear {
                self.effective_params(*idx).clone()
            } else {
                plan.new_effective.clone()
            };
            self.adjustment_cache.remove(idx);
            self.invalidate_compare_prepared_for_idx(*idx);
            self.bump_adjustment_generation(*idx);
            if !old.color_settings_eq(&new) {
                self.thumb_adjust_tex.remove(idx);
            }
            if plan.clear && !old.ai_settings_eq(&new) {
                self.purge_upscale_for_idx(*idx);
            }
        }
        if !plan.old_effective.color_settings_eq(&plan.new_effective) {
            self.pinned_adjustment_refresh_keys
                .insert(plan.target.page_key.clone());
        }
        if let Some((idx, _)) = plan.old_by_idx.first() {
            self.record_content_identity_for_idx(
                *idx,
                crate::content_identity::ContentIdentityTrigger::Edit,
            );
        }
    }

    pub(crate) fn restore_page_params_for_targets(
        &mut self,
        writes: &[(PageAdjustmentTarget, Option<AdjustParams>)],
    ) {
        if writes.is_empty() {
            return;
        }
        let total = crate::perf::is_enabled()
            .then(|| (std::time::Instant::now(), Self::thread_cycles_now()));
        let plans: Vec<_> = writes
            .iter()
            .map(|(target, params)| self.plan_page_param_write(target, params.clone()))
            .collect();
        let db_start = total.map(|_| (std::time::Instant::now(), Self::thread_cycles_now()));
        let result = self.persist_page_param_plans(&plans);
        let db_cost = db_start.map(|(time, cycles)| {
            (
                time.elapsed().as_secs_f64() * 1000.0,
                Self::thread_cycles_now().saturating_sub(cycles),
            )
        });
        let runtime_start = total.map(|_| (std::time::Instant::now(), Self::thread_cycles_now()));
        let what = if writes.iter().all(|(_, params)| params.is_none()) {
            "補正の解除"
        } else {
            "補正"
        };
        let committed = self.edit_store_write_succeeded(what, result);
        let mut documents = HashMap::new();
        for plan in &plans {
            self.reflect_page_param_plan(plan, committed);
            if !plan.clear {
                for (idx, old) in &plan.old_by_idx {
                    self.clear_caches_for_param_change(*idx, old, &plan.new_effective);
                }
            }
            if committed {
                documents.insert(plan.target.page_key.clone(), plan.document.clone());
            }
        }
        self.schedule_current_smart_folder_metadata_refresh(
            smart_folder::SmartFolderMetadataDependency::Edits,
        );
        let runtime_cost = runtime_start.map(|(time, cycles)| {
            (
                time.elapsed().as_secs_f64() * 1000.0,
                Self::thread_cycles_now().saturating_sub(cycles),
            )
        });
        let publish_start = total.map(|_| (std::time::Instant::now(), Self::thread_cycles_now()));
        self.publish_accepted_adjustment_page_edits(&documents);
        if crate::perf::is_enabled() {
            let (time, cycles) = total.unwrap();
            let (db_ms, db_cycles) = db_cost.unwrap();
            let (runtime_ms, runtime_cycles) = runtime_cost.unwrap();
            let (publish_time, publish_cycles) = publish_start.unwrap();
            crate::perf::event(
                "adjust",
                "bulk_page_write",
                None,
                0,
                &[
                    ("n", serde_json::json!(writes.len())),
                    ("db_ms", serde_json::json!(db_ms)),
                    ("db_cycles", serde_json::json!(db_cycles)),
                    ("runtime_ms", serde_json::json!(runtime_ms)),
                    ("runtime_cycles", serde_json::json!(runtime_cycles)),
                    (
                        "publish_ms",
                        serde_json::json!(publish_time.elapsed().as_secs_f64() * 1000.0),
                    ),
                    (
                        "publish_cycles",
                        serde_json::json!(Self::thread_cycles_now().saturating_sub(publish_cycles)),
                    ),
                    (
                        "total_ms",
                        serde_json::json!(time.elapsed().as_secs_f64() * 1000.0),
                    ),
                    (
                        "total_cycles",
                        serde_json::json!(Self::thread_cycles_now().saturating_sub(cycles)),
                    ),
                ],
            );
        }
    }
}
