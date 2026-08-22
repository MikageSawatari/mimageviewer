use std::sync::mpsc;
use std::time::Duration;

use super::App;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub(crate) struct ContentRestorePrompt {
    candidates: Vec<crate::content_identity::RestoreCandidate>,
    pub(crate) ui_rows: Vec<crate::ui_dialogs::content_restore::ContentRestoreUiRow>,
    pub(crate) dont_ask_again: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentRestorePromptAction {
    Restore,
    Close,
}

#[derive(Debug)]
enum ContentRestorePromptDecision {
    Restore {
        selected: Vec<crate::content_identity::SelectedRestore>,
        declined: Vec<crate::content_identity::DeclinedRestore>,
        disable_future_prompts: bool,
    },
    Close {
        disable_future_prompts: bool,
    },
}

fn apply_disable_choice_to_settings(
    settings: &mut crate::settings::Settings,
    disable: bool,
) -> bool {
    if !disable || !settings.edit_restore_prompt_enabled {
        return false;
    }
    settings.edit_restore_prompt_enabled = false;
    true
}

fn merge_content_restore_sidecars(
    sidecars: &mut std::collections::HashMap<std::path::PathBuf, crate::sidecar::SidecarFile>,
    mirrors: Vec<crate::content_identity::RestoreSidecarMirror>,
    sidecar_bases: Vec<crate::sidecar::SidecarFile>,
) {
    let mut sidecar_bases = sidecar_bases
        .into_iter()
        .map(|sidecar| (sidecar.folder().to_path_buf(), sidecar))
        .collect::<std::collections::HashMap<_, _>>();
    for mirror in mirrors {
        if !sidecars.contains_key(&mirror.folder)
            && let Some(sidecar) = sidecar_bases.remove(&mirror.folder)
        {
            sidecars.insert(mirror.folder.clone(), sidecar);
        }
        if let Some(sidecar) = sidecars.get_mut(&mirror.folder) {
            sidecar.replace_edit_bundle(&mirror.rel_key, mirror.entry);
        }
    }
}

pub(crate) struct ContentIdentityRestorePending {
    rx: mpsc::Receiver<crate::content_identity::ContentRestoreReport>,
}

impl ContentRestorePrompt {
    pub(crate) fn from_candidates(
        candidates: Vec<crate::content_identity::RestoreCandidate>,
    ) -> Option<Self> {
        let mut retained = Vec::new();
        let mut ui_rows = Vec::new();
        for mut candidate in candidates {
            if candidate.sources.is_empty() {
                continue;
            }
            crate::content_identity::sort_restore_sources(&mut candidate.sources);
            let file_name = candidate
                .target_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| candidate.target_path.display().to_string());
            let sources = candidate
                .sources
                .iter()
                .map(
                    |source| crate::ui_dialogs::content_restore::ContentRestoreUiSource {
                        path: source.path.display().to_string(),
                        source_exists: source.source_exists,
                    },
                )
                .collect();
            ui_rows.push(crate::ui_dialogs::content_restore::ContentRestoreUiRow {
                file_name,
                selected: true,
                source_index: 0,
                sources,
            });
            retained.push(candidate);
        }
        (!retained.is_empty()).then_some(Self {
            candidates: retained,
            ui_rows,
            dont_ask_again: false,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.candidates.len()
    }

    fn decide(self, action: ContentRestorePromptAction) -> ContentRestorePromptDecision {
        if action == ContentRestorePromptAction::Close {
            return ContentRestorePromptDecision::Close {
                disable_future_prompts: self.dont_ask_again,
            };
        }
        let mut selected = Vec::new();
        let mut declined = Vec::new();
        for (candidate, row) in self.candidates.into_iter().zip(self.ui_rows) {
            if row.selected {
                if let Some(source) = candidate.sources.get(row.source_index).cloned() {
                    selected.push(crate::content_identity::SelectedRestore { candidate, source });
                }
            } else {
                declined.push(crate::content_identity::DeclinedRestore {
                    full_hash: candidate.full_hash,
                    target_key: candidate.target_key,
                });
            }
        }
        ContentRestorePromptDecision::Restore {
            selected,
            declined,
            disable_future_prompts: self.dont_ask_again,
        }
    }
}

impl ContentIdentityRestorePending {
    fn spawn(
        selected: Vec<crate::content_identity::SelectedRestore>,
        declined: Vec<crate::content_identity::DeclinedRestore>,
        input_seq: u64,
        load_sidecar_bases: bool,
    ) -> Result<Self, String> {
        let data_dir = crate::data_dir::get();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("content-identity-restore".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let report = crate::content_identity::restore_candidates_at(
                    &data_dir,
                    &selected,
                    &declined,
                    load_sidecar_bases,
                );
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "content_identity",
                        "restore_end",
                        None,
                        input_seq,
                        &[
                            (
                                "ms",
                                serde_json::Value::from(started.elapsed().as_secs_f64() * 1000.0),
                            ),
                            (
                                "restores",
                                serde_json::Value::from(report.requested_restores),
                            ),
                            (
                                "declines",
                                serde_json::Value::from(report.requested_declines),
                            ),
                            ("errors", serde_json::Value::from(report.errors.len())),
                            (
                                "database_opens",
                                serde_json::Value::from(report.database_opens),
                            ),
                        ],
                    );
                }
                let _ = tx.send(report);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self { rx })
    }

    fn try_recv(
        &self,
    ) -> Result<crate::content_identity::ContentRestoreReport, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
}

impl App {
    pub(crate) fn set_content_restore_candidates(
        &mut self,
        candidates: Vec<crate::content_identity::RestoreCandidate>,
    ) {
        self.content_restore_prompt = ContentRestorePrompt::from_candidates(candidates);
    }

    pub(crate) fn clear_content_restore_prompt(&mut self) {
        self.content_restore_prompt = None;
    }

    /// 背面入力を止める述語と描画側が共有する唯一の可視性判定。
    /// 先行 batch の完了待ち中に次フォルダの候補が届いた場合も、候補を保持して順番に出す。
    pub(crate) fn content_restore_window_visible(&self) -> bool {
        self.fullscreen_idx.is_none()
            && self.content_identity_restore_pending.is_none()
            && self.content_restore_prompt.is_some()
    }

    pub(crate) fn handle_content_restore_prompt_action(
        &mut self,
        ctx: &egui::Context,
        action: ContentRestorePromptAction,
    ) {
        let Some(prompt) = self.content_restore_prompt.take() else {
            return;
        };
        match prompt.decide(action) {
            ContentRestorePromptDecision::Close {
                disable_future_prompts,
            } => self.apply_content_restore_disable_choice(disable_future_prompts),
            ContentRestorePromptDecision::Restore {
                selected,
                declined,
                disable_future_prompts,
            } => {
                self.apply_content_restore_disable_choice(disable_future_prompts);
                self.start_content_identity_restore(ctx, selected, declined);
            }
        }
    }

    fn apply_content_restore_disable_choice(&mut self, disable: bool) {
        if self.apply_content_restore_disable_choice_to_runtime(disable) {
            self.settings.save();
        }
    }

    fn apply_content_restore_disable_choice_to_runtime(&mut self, disable: bool) -> bool {
        let old_enabled = self.settings.edit_restore_prompt_enabled;
        if !apply_disable_choice_to_settings(&mut self.settings, disable) {
            return false;
        }
        self.sync_content_identity_detection_setting(old_enabled);
        true
    }

    fn start_content_identity_restore(
        &mut self,
        ctx: &egui::Context,
        selected: Vec<crate::content_identity::SelectedRestore>,
        declined: Vec<crate::content_identity::DeclinedRestore>,
    ) {
        if selected.is_empty() && declined.is_empty() {
            return;
        }
        if self.content_identity_restore_pending.is_some() {
            crate::logger::log(
                "content_identity: restore request ignored while another restore is active"
                    .to_string(),
            );
            return;
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "content_identity",
                "restore_enqueue",
                self.current_folder.as_ref().and_then(|path| path.to_str()),
                self.input_seq,
                &[
                    ("restores", serde_json::Value::from(selected.len())),
                    ("declines", serde_json::Value::from(declined.len())),
                ],
            );
        }
        match ContentIdentityRestorePending::spawn(
            selected,
            declined,
            self.input_seq,
            self.settings.sidecar_backup_enabled,
        ) {
            Ok(pending) => {
                self.content_identity_restore_pending = Some(pending);
                ctx.request_repaint_after(POLL_INTERVAL);
            }
            Err(error) => {
                crate::logger::log(format!(
                    "content_identity: restore thread spawn failed: {error}"
                ));
                self.show_feedback_toast("編集内容の復元を開始できませんでした".to_string());
            }
        }
    }

    pub(crate) fn poll_content_identity_restore(&mut self, ctx: &egui::Context) {
        let event = self
            .content_identity_restore_pending
            .as_ref()
            .map(ContentIdentityRestorePending::try_recv);
        let report = match event {
            Some(Ok(report)) => report,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.content_identity_restore_pending = None;
                self.apply_content_identity_store_mutations(
                    crate::rename_key_migration::StoreMutationEffects::
                        for_content_identity_index_stale(),
                );
                crate::logger::log("content_identity: restore thread disconnected".to_string());
                self.show_feedback_toast("編集内容の復元が中断されました".to_string());
                return;
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {
                ctx.request_repaint_after(POLL_INTERVAL);
                return;
            }
            None => return,
        };
        self.content_identity_restore_pending = None;

        let restored = report.ledger_entries.len();
        let failures = report.errors.len();
        crate::logger::log(format!(
            "content_identity: restore completed requested={} restored={} declined={} errors={} database_opens={}",
            report.requested_restores,
            restored,
            report.requested_declines,
            failures,
            report.database_opens
        ));
        if restored != report.requested_restores {
            // STORES copy が edit_origin の destination 行を commit した後、promotion だけが
            // 失敗した部分成功では ledger_entries に増分通知が無い。件数不一致を曖昧な
            // per-key 推測で埋めず、authoritative snapshot を引き直す。
            self.apply_content_identity_store_mutations(
                crate::rename_key_migration::StoreMutationEffects::for_content_identity_index_stale(
                ),
            );
        }
        self.apply_content_identity_ledger_updates(report.ledger_entries);
        self.apply_content_restore_sidecar_mirrors(report.sidecar_mirrors, report.sidecar_bases);
        self.apply_content_restore_presence(report.presence);
        if report.requested_restores > 0 {
            self.finish_content_identity_restore(report.errors);
        } else if !report.errors.is_empty() {
            self.show_feedback_toast(format!(
                "確認結果の保存に失敗しました ({} 件)",
                report.errors.len()
            ));
        }
    }

    /// Worker が DB の最終状態から作った edit bundle と disk から読んだ base を、既存の
    /// in-memory sidecar owner へ反映する。ここでは disk を読まず、flush も従来どおり
    /// `flush_all_sidecars` の既存境界だけが行う。
    pub(crate) fn apply_content_restore_sidecar_mirrors(
        &mut self,
        mirrors: Vec<crate::content_identity::RestoreSidecarMirror>,
        sidecar_bases: Vec<crate::sidecar::SidecarFile>,
    ) {
        if !self.settings.sidecar_backup_enabled {
            return;
        }
        merge_content_restore_sidecars(&mut self.sidecars, mirrors, sidecar_bases);
    }

    /// Worker が直接更新した destination page について、DB の miss まで materialize する
    /// read-once cache だけを page key 単位で失効させる。idx-keyed page state と rating / tags
    /// は後続の `finish_book_page_edit_mapping` が既存の ownership 境界で処理する。
    fn invalidate_content_restore_destination_caches(
        &mut self,
        presence: &crate::content_identity::RestorePresence,
    ) {
        // `comic_docs` の空 Vec は「DB 読込済み・row なし」の sentinel。restore が作った row を
        // 隠したまま保存すると ComicDb::set(empty) がその row を削除するため、実際に comic row
        // がある destination key だけを未読へ戻す。
        for key in &presence.comics {
            self.comic_docs.remove(key);
        }

        // rotation_cache も Rotation::None を cache する。現在 items に materialize 済みの
        // destination だけを外し、次の get_rotation で DB から再読込させる。
        let rotation_indices = (0..self.items.len())
            .filter(|&idx| {
                self.rotation_key_for_idx(idx)
                    .is_some_and(|key| presence.rotations.contains(&key))
            })
            .collect::<Vec<_>>();
        for idx in rotation_indices {
            self.rotation_cache.remove(&idx);
        }

        // edit_preview_cache.db も worker が App service の event を通らず更新する。すでに raw / old
        // preview を materialize した destination thumbnail だけを通常の再要求経路へ戻す。
        let thumbnail_indices = (0..self.items.len())
            .filter(|&idx| {
                self.page_path_key(idx)
                    .is_some_and(|key| presence.page_edits.contains(&key))
                    || self
                        .thumb_edit_preview_keys
                        .get(&idx)
                        .is_some_and(|key| presence.page_edits.contains(key))
            })
            .collect::<Vec<_>>();
        for idx in thumbnail_indices {
            self.evict_thumbnail_for_reload(idx);
        }
    }

    /// グリッド badge / smart-folder 集計が参照する global page-key presence を増分反映する。
    pub(crate) fn apply_content_restore_presence(
        &mut self,
        presence: crate::content_identity::RestorePresence,
    ) {
        self.invalidate_content_restore_destination_caches(&presence);
        self.adjusted_page_keys.extend(presence.adjusted);
        self.mask_page_keys.extend(presence.masks);
        self.conceal_page_keys.extend(presence.conceals);
        self.local_adjust_page_keys
            .extend(presence.local_adjustments);
        self.comic_page_keys.extend(presence.comics);
        self.rotation_page_keys.extend(presence.rotations);
    }

    /// 復元完了後の idx-keyed edit state と rating / tag cache の共通 invalidation。
    /// A3b は worker report の sidecar / presence を適用した後にこの関数を呼ぶ。
    pub(crate) fn finish_content_identity_restore(&mut self, errors: Vec<String>) {
        // Detection / prompt は通常の物理フォルダ一覧だけで始まる。ただし restore worker の
        // 完了までに view が変わる可能性もあるため、完了時にも同じ authoritative predicate で
        // search / snapshot 等を除外する。prefix は rename 完了と同じく current_folder を使う。
        if self.is_physical_folder_listing()
            && let Some(folder) = self.current_folder.clone()
        {
            self.rehydrate_page_edit_state_for_current_items(&folder);
        } else {
            // 合成 view はページ編集 overlay を出さない既存契約を維持する。
            self.clear_page_edit_state();
        }
        self.finish_book_page_edit_mapping_after_idx_refresh("restore", errors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn set_physical_folder_listing(app: &mut App, folder: &Path) {
        app.current_folder = Some(folder.to_path_buf());
        app.normal_folder_omitted_entries = Some(crate::app::NormalFolderOmittedEntries {
            folder: folder.to_path_buf(),
            counts: Default::default(),
        });
        app.top_level_grid_view
            .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
    }

    fn seed_restored_idx_page_edits(
        app: &mut App,
        target: &Path,
    ) -> (String, crate::export_crop::CropSettings) {
        let key = crate::path_key::normalize_keep_drive(target);
        let layers = vec![local_adjust_core::LocalAdjustmentLayer::new(
            "restored",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        )];
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .set_layers(&key, &layers)
            .unwrap();
        app.conceal_db
            .as_ref()
            .unwrap()
            .set(&key, &[true], &[], 1, 1)
            .unwrap();
        let crop = crate::export_crop::CropSettings {
            rect: crate::export_crop::CropRect {
                min_x: 0.1,
                min_y: 0.2,
                max_x: 0.8,
                max_y: 0.9,
            },
            aspect_mode: crate::export_crop::CropAspectMode::Free,
            source_size: Some([100, 100]),
        };
        app.export_crop_db
            .as_ref()
            .unwrap()
            .set(&key, crop)
            .unwrap();
        (key, crop)
    }

    fn candidate(name: &str) -> crate::content_identity::RestoreCandidate {
        let target_path = PathBuf::from(format!("C:/copied/{name}.png"));
        crate::content_identity::RestoreCandidate {
            target_key: crate::path_key::normalize_keep_drive(&target_path),
            target_path,
            target_kind: crate::content_identity::ContentKind::Image,
            full_hash: format!("full-{name}"),
            sources: vec![crate::content_identity::RestoreSourceCandidate {
                file_key: format!("c:/original/{name}.png"),
                path: PathBuf::from(format!("C:/original/{name}.png")),
                kind: crate::content_identity::ContentKind::Image,
                last_edit_at: 10,
                source_exists: true,
            }],
        }
    }

    #[test]
    fn close_records_nothing_and_the_same_candidates_can_prompt_again() {
        let candidates = vec![candidate("a")];
        let prompt = ContentRestorePrompt::from_candidates(candidates.clone()).unwrap();
        let decision = prompt.decide(ContentRestorePromptAction::Close);

        assert!(matches!(
            decision,
            ContentRestorePromptDecision::Close {
                disable_future_prompts: false
            }
        ));
        assert!(ContentRestorePrompt::from_candidates(candidates).is_some());
    }

    #[test]
    fn escape_produces_the_same_close_decision_with_and_without_dont_ask_again() {
        for disable_future_prompts in [false, true] {
            let mut prompt = ContentRestorePrompt::from_candidates(vec![candidate("escape")])
                .expect("candidate must create a prompt");
            prompt.dont_ask_again = disable_future_prompts;
            let close_decision = prompt.clone().decide(ContentRestorePromptAction::Close);
            let escape_action =
                crate::ui_dialogs::content_restore::resolve_content_restore_prompt_action(
                    true, None,
                )
                .expect("Escape must resolve to an action");
            let escape_decision = prompt.decide(escape_action);

            for decision in [close_decision, escape_decision] {
                assert!(matches!(
                    decision,
                    ContentRestorePromptDecision::Close {
                        disable_future_prompts: actual
                    } if actual == disable_future_prompts
                ));
            }
        }
    }

    #[test]
    fn restore_records_only_unchecked_rows_as_declined() {
        let mut prompt =
            ContentRestorePrompt::from_candidates(vec![candidate("off"), candidate("on")]).unwrap();
        prompt.ui_rows[0].selected = false;
        let decision = prompt.decide(ContentRestorePromptAction::Restore);

        let ContentRestorePromptDecision::Restore {
            selected, declined, ..
        } = decision
        else {
            panic!("restore action must produce a batch");
        };
        assert_eq!(selected.len(), 1);
        assert!(selected[0].candidate.target_key.ends_with("/on.png"));
        assert_eq!(declined.len(), 1);
        assert!(declined[0].target_key.ends_with("/off.png"));
    }

    #[test]
    fn choosing_another_source_changes_the_source_used_for_restore() {
        let mut candidate = candidate("target");
        candidate.sources = vec![
            crate::content_identity::RestoreSourceCandidate {
                file_key: "c:/original/z.png".to_string(),
                path: PathBuf::from("C:/original/z.png"),
                kind: crate::content_identity::ContentKind::Image,
                last_edit_at: 0,
                source_exists: false,
            },
            crate::content_identity::RestoreSourceCandidate {
                file_key: "c:/original/a.png".to_string(),
                path: PathBuf::from("C:/original/a.png"),
                kind: crate::content_identity::ContentKind::Image,
                last_edit_at: 0,
                source_exists: true,
            },
        ];
        let mut prompt = ContentRestorePrompt::from_candidates(vec![candidate]).unwrap();
        assert_eq!(prompt.ui_rows[0].sources[0].path, "C:/original/a.png");
        prompt.ui_rows[0].source_index = 1;

        let decision = prompt.decide(ContentRestorePromptAction::Restore);
        let ContentRestorePromptDecision::Restore { selected, .. } = decision else {
            panic!("restore action must produce a batch");
        };

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].source.file_key, "c:/original/z.png");
        assert!(!selected[0].source.source_exists);
    }

    #[test]
    fn dont_ask_again_disables_the_a2_setting_gate() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.edit_restore_prompt_enabled = true;
        app.content_identity_ledger_state =
            crate::content_identity::ContentIdentityLedgerState::Ready;
        let (pending, cancel) =
            crate::content_identity::ContentIdentityDetectionPending::for_test(None);
        app.content_identity_detection_pending = Some(pending);
        let (backfill, backfill_cancel) =
            crate::content_identity::ContentIdentityBackfillPending::for_test();
        app.content_identity_backfill_pending = Some(backfill);
        app.set_content_restore_candidates(vec![candidate("pending")]);

        assert!(app.apply_content_restore_disable_choice_to_runtime(true));
        assert!(!app.settings.edit_restore_prompt_enabled);
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(backfill_cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(app.content_identity_detection_pending.is_none());
        assert!(app.content_identity_backfill_pending.is_none());
        assert!(app.content_restore_prompt.is_none());
        assert_eq!(
            app.content_identity_ledger_state,
            crate::content_identity::ContentIdentityLedgerState::Disabled
        );
        assert!(!app.apply_content_restore_disable_choice_to_runtime(true));
    }

    #[test]
    fn sidecar_mirror_merges_worker_loaded_base_without_ui_io() {
        let folder = PathBuf::from("C:/copied");
        let mut cached = crate::sidecar::SidecarFile::new(folder.clone());
        cached.set_tags("target.png", ["current"]);
        let mut stale_worker_base = crate::sidecar::SidecarFile::new(folder.clone());
        stale_worker_base.set_tags("target.png", ["stale"]);
        let mirror = crate::content_identity::RestoreSidecarMirror {
            folder: folder.clone(),
            rel_key: "target.png".to_string(),
            entry: crate::sidecar::SidecarEntry {
                adjust: Some(crate::adjustment::AdjustParams::default()),
                ..crate::sidecar::SidecarEntry::default()
            },
        };
        let mut sidecars = std::collections::HashMap::from([(folder.clone(), cached)]);

        merge_content_restore_sidecars(&mut sidecars, vec![mirror], vec![stale_worker_base]);

        let entry = sidecars[&folder].items().get("target.png").unwrap();
        assert!(entry.adjust.is_some());
        assert_eq!(
            entry.tags.as_deref(),
            Some(["#current".to_string()].as_slice())
        );
    }

    /// `edit_preview_close` の実ログで観測された退行: restore worker が DB へコピーした
    /// conceal / local-adjust / export-crop は、フォルダ再読込なしで current idx に見えること。
    #[test]
    fn restore_completion_rehydrates_idx_page_edits_without_folder_reload() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("restored");
        let target = folder.join("page.png");
        app.items = vec![crate::grid_item::GridItem::Image(target.clone())];
        set_physical_folder_listing(&mut app, &folder);
        let (key, crop) = seed_restored_idx_page_edits(&mut app, &target);

        // Worker は App の idx maps を通らず DB と path-keyed presence report を更新する。
        let mut presence = crate::content_identity::RestorePresence::default();
        presence.local_adjustments.insert(key.clone());
        presence.conceals.insert(key);
        app.apply_content_restore_presence(presence);
        app.finish_content_identity_restore(Vec::new());

        let has_source_edits = app.local_adjust_pages.contains(&0)
            || app.mask_pages.contains(&0)
            || app.conceal_pages.contains(&0);
        let has_crop = app.export_crop_pages.contains(&0);
        assert!(
            has_source_edits,
            "restore completion must make has_source_edits=true without a folder reload"
        );
        assert!(
            app.local_adjust_pages.contains(&0),
            "restored local adjustment presence must be idx-keyed immediately"
        );
        assert!(
            app.conceal_pages.contains(&0),
            "restored concealment presence must be idx-keyed immediately"
        );
        assert!(
            has_crop,
            "restore completion must make has_crop=true without a folder reload"
        );
        assert_eq!(app.export_crop_page_settings.get(&0), Some(&crop));
        assert!(
            app.local_adjust_page_layers.is_empty(),
            "large local-adjust JSON remains lazy after rehydrate"
        );
    }

    #[test]
    fn restore_completion_does_not_rehydrate_search_view_overlays() {
        let mut app = crate::app::setup_app_for_test();
        let folder = app.tmp.path().join("restored");
        let target = folder.join("page.png");
        app.items = vec![crate::grid_item::GridItem::Image(target.clone())];
        set_physical_folder_listing(&mut app, &folder);
        seed_restored_idx_page_edits(&mut app, &target);
        app.top_level_grid_view.replace_surface(
            crate::app::top_level_grid_view::TopLevelGridSurface::Search(
                crate::app::top_level_grid_view::TopLevelSearchView::Global,
            ),
        );
        app.local_adjust_pages.insert(0);
        app.conceal_pages.insert(0);
        app.export_crop_pages.insert(0);

        app.finish_content_identity_restore(Vec::new());

        assert!(app.local_adjust_pages.is_empty());
        assert!(app.conceal_pages.is_empty());
        assert!(app.export_crop_pages.is_empty());
        assert!(app.export_crop_page_settings.is_empty());
    }
}
