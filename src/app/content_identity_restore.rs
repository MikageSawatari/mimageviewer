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
        for candidate in candidates {
            if candidate.sources.is_empty() {
                continue;
            }
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

    /// グリッド badge / smart-folder 集計が参照する global page-key presence を増分反映する。
    pub(crate) fn apply_content_restore_presence(
        &mut self,
        presence: crate::content_identity::RestorePresence,
    ) {
        self.adjusted_page_keys.extend(presence.adjusted);
        self.mask_page_keys.extend(presence.masks);
        self.conceal_page_keys.extend(presence.conceals);
        self.local_adjust_page_keys
            .extend(presence.local_adjustments);
        self.comic_page_keys.extend(presence.comics);
    }

    /// 復元完了後の idx-keyed edit state と rating / tag cache の共通 invalidation。
    /// A3b は worker report の sidecar / presence を適用した後にこの関数を呼ぶ。
    pub(crate) fn finish_content_identity_restore(&mut self, errors: Vec<String>) {
        self.finish_book_page_edit_mapping("restore", errors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
}
