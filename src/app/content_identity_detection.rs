use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use super::{App, top_level_grid_view};
use crate::content_identity::{
    ContentIdentityBackfillPending, ContentIdentityDetectionPending, ContentIdentityIndex,
    ContentIdentityIndexLoadPending, ContentIdentityLedgerState, ContentIdentitySource,
    ContentKind, DetectionResult, DetectionTarget, RestoreCandidate,
};

const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Default)]
struct ContentIdentityFolderWork {
    detection_targets: Vec<DetectionTarget>,
    backfill_sources: Vec<ContentIdentitySource>,
}

impl App {
    pub(crate) fn start_content_identity_index_load(&mut self) {
        if !self.settings.edit_restore_prompt_enabled
            || !matches!(
                &self.content_identity_ledger_state,
                ContentIdentityLedgerState::Disabled
            )
            || self.content_identity_index_load_pending.is_some()
        {
            return;
        }
        self.spawn_content_identity_index_load();
    }

    fn spawn_content_identity_index_load(&mut self) {
        self.content_identity_ledger_state = ContentIdentityLedgerState::Loading;
        let io_sem = self
            .indexer_manager
            .as_ref()
            .map(crate::indexer_manager::IndexerManager::io_sem)
            .unwrap_or_else(|| Arc::clone(&self.content_identity_fallback_io_sem));
        match ContentIdentityIndexLoadPending::spawn(io_sem) {
            Ok(pending) => self.content_identity_index_load_pending = Some(pending),
            Err(error) => self.mark_content_identity_ledger_unusable(format!(
                "index loader thread spawn failed: {error}"
            )),
        }
    }

    /// 共通 STORES worker が `edit_origin` の key rewrite / delete を commit した後、
    /// 古い snapshot を stage 0 から即座に外し、authoritative ledger を worker で再読込する。
    pub(crate) fn apply_content_identity_store_mutations(
        &mut self,
        effects: crate::rename_key_migration::StoreMutationEffects,
    ) {
        if !effects.content_identity_index_stale() || !self.settings.edit_restore_prompt_enabled {
            return;
        }
        if let Some(pending) = self.content_identity_index_load_pending.take() {
            pending.cancel();
        }
        self.cancel_content_identity_detection();

        // mutation より前に commit 済みだった増分通知は、authoritative reload 後へ merge すると
        // purge 済みの旧 key を復活させる。ここまでの通知は捨て、以後に届くものだけ Loading 中の
        // queue へ積む。現在の DB 状態は新しい reload が全件回収する。
        self.content_identity_updates_before_load.clear();
        if let Some(recorder) = self.content_identity_recorder.as_ref() {
            let _ = recorder.drain_updates();
        }
        crate::logger::log(
            "content_identity: ledger index stale after shared store mutation; reloading"
                .to_string(),
        );
        self.spawn_content_identity_index_load();
    }

    pub(crate) fn apply_content_identity_ledger_updates(
        &mut self,
        updates: impl IntoIterator<Item = crate::content_identity::LedgerEntry>,
    ) {
        if self.content_identity_ledger_state.is_ready() {
            for update in updates {
                self.content_identity_index.upsert(update);
            }
        } else if matches!(
            &self.content_identity_ledger_state,
            ContentIdentityLedgerState::Loading
        ) {
            self.content_identity_updates_before_load.extend(updates);
        }
    }

    pub(crate) fn sync_content_identity_detection_setting(&mut self, old_enabled: bool) {
        let enabled = self.settings.edit_restore_prompt_enabled;
        if old_enabled == enabled {
            return;
        }
        if !enabled {
            if let Some(pending) = self.content_identity_index_load_pending.take() {
                pending.cancel();
            }
            self.cancel_content_identity_detection();
            self.content_identity_index = ContentIdentityIndex::default();
            self.content_identity_ledger_state = ContentIdentityLedgerState::Disabled;
            self.content_identity_updates_before_load.clear();
        } else {
            self.content_identity_ledger_state = ContentIdentityLedgerState::Disabled;
            self.start_content_identity_index_load();
        }
    }

    pub(crate) fn cancel_content_identity_detection(&mut self) {
        if let Some(pending) = self.content_identity_detection_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.content_identity_backfill_pending.take() {
            pending.cancel();
        }
        self.clear_content_restore_prompt();
    }

    /// 通常フォルダ scan が現在 path へ公開した marker と、最上位 surface の両方を使う。
    /// 拡張子や item variant の寄せ集めでは、検索・スマートフォルダ・ZIP 内一覧を除外できない。
    pub(crate) fn is_physical_folder_listing(&self) -> bool {
        if self.navigation_scope.is_detached_physical()
            || self.grid_is_zip_entries()
            || self.grid_is_pdf_pages()
            || !matches!(
                self.top_level_grid_view.surface(),
                top_level_grid_view::TopLevelGridSurface::Folder
            )
        {
            return false;
        }
        let Some(current) = self.current_folder.as_deref() else {
            return false;
        };
        self.normal_folder_omitted_entries
            .as_ref()
            .is_some_and(|marker| crate::folder_tree::path_eq(&marker.folder, current))
    }

    pub(crate) fn maybe_start_content_identity_detection(&mut self) {
        // OFF は最初の分岐にする。物理一覧判定、size index、backfill 選別の
        // いずれも行わず、folder-open 起因の file read を 0 にする。
        if !self.settings.edit_restore_prompt_enabled {
            return;
        }
        if !self.content_identity_ledger_state.is_ready() || !self.is_physical_folder_listing() {
            return;
        }
        let Some(folder) = self.current_folder.as_deref() else {
            return;
        };
        let folder_key = crate::path_key::normalize_keep_drive(folder);
        let work = self.collect_content_identity_folder_work(folder);
        if work.detection_targets.is_empty() && work.backfill_sources.is_empty() {
            return;
        }
        let io_sem = self
            .indexer_manager
            .as_ref()
            .map(crate::indexer_manager::IndexerManager::io_sem)
            .unwrap_or_else(|| Arc::clone(&self.content_identity_fallback_io_sem));

        if !work.detection_targets.is_empty() {
            if let Some(pending) = self.content_identity_detection_pending.take() {
                pending.cancel();
            }
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "content_identity",
                    "detect_enqueue",
                    Some(&folder_key),
                    self.input_seq,
                    &[(
                        "targets",
                        serde_json::Value::from(work.detection_targets.len()),
                    )],
                );
            }
            let update_tx = self
                .content_identity_recorder
                .as_ref()
                .map(crate::content_identity::ContentIdentityRecorder::update_sender);
            self.content_identity_detection_pending = ContentIdentityDetectionPending::spawn(
                work.detection_targets,
                self.items_generation,
                folder_key.clone(),
                self.input_seq,
                Arc::clone(&io_sem),
                update_tx,
            );
        }

        if !work.backfill_sources.is_empty() {
            if let Some(pending) = self.content_identity_backfill_pending.take() {
                pending.cancel();
            }
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "content_identity",
                    "backfill_enqueue",
                    Some(&folder_key),
                    self.input_seq,
                    &[(
                        "targets",
                        serde_json::Value::from(work.backfill_sources.len()),
                    )],
                );
            }
            let update_tx = self
                .content_identity_recorder
                .as_ref()
                .map(crate::content_identity::ContentIdentityRecorder::update_sender);
            self.content_identity_backfill_pending = ContentIdentityBackfillPending::spawn(
                work.backfill_sources,
                self.input_seq,
                folder_key,
                io_sem,
                update_tx,
            );
        }
    }

    fn collect_content_identity_folder_work(
        &self,
        folder: &std::path::Path,
    ) -> ContentIdentityFolderWork {
        let mut work = ContentIdentityFolderWork::default();
        let mut backfill_keys = BTreeSet::new();
        for (item, meta) in self.items.iter().zip(&self.image_metas) {
            let Some(source) = ContentIdentitySource::for_grid_item(item, None, Some(folder))
            else {
                continue;
            };
            if self.content_identity_target_has_existing_edit(&source) {
                let file_key = crate::path_key::normalize_keep_drive(&source.path);
                if !self
                    .content_identity_index
                    .contains_ledger_file_key(&file_key)
                    && backfill_keys.insert(file_key)
                {
                    work.backfill_sources.push(source);
                }
                continue;
            }
            let Some((_, size)) = meta else {
                continue;
            };
            let Ok(size) = u64::try_from(*size) else {
                continue;
            };
            if let Some(target) =
                crate::content_identity::stage0_target(&self.content_identity_index, source, size)
            {
                work.detection_targets.push(target);
            }
        }
        work
    }

    pub(crate) fn poll_content_identity_detection(&mut self, ctx: &egui::Context) {
        self.poll_content_identity_record_updates();
        self.poll_content_identity_index_load(ctx);
        self.poll_content_identity_backfill(ctx);

        let event = self
            .content_identity_detection_pending
            .as_ref()
            .map(ContentIdentityDetectionPending::try_recv);
        match event {
            Some(Ok(result)) => {
                self.content_identity_detection_pending = None;
                if !self.content_identity_detection_result_is_current(&result) {
                    crate::logger::log(format!(
                        "content_identity: discarded stale detection result folder={} generation={}",
                        result.folder_key, result.items_generation
                    ));
                    return;
                }
                self.apply_content_identity_ledger_updates(result.ledger_updates);
                self.log_content_restore_candidates(&result.candidates);
                self.set_content_restore_candidates(result.candidates);
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "content_identity",
                        "detect_ready",
                        self.current_folder.as_ref().and_then(|path| path.to_str()),
                        self.input_seq,
                        &[(
                            "candidates",
                            serde_json::Value::from(
                                self.content_restore_prompt
                                    .as_ref()
                                    .map(|prompt| prompt.len())
                                    .unwrap_or(0),
                            ),
                        )],
                    );
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.content_identity_detection_pending = None;
            }
            Some(Err(mpsc::TryRecvError::Empty)) => ctx.request_repaint_after(POLL_INTERVAL),
            None => {}
        }
    }

    fn poll_content_identity_backfill(&mut self, ctx: &egui::Context) {
        let event = self
            .content_identity_backfill_pending
            .as_ref()
            .map(ContentIdentityBackfillPending::try_recv);
        match event {
            Some(Ok(result)) => {
                self.content_identity_backfill_pending = None;
                let recorded = result.ledger_updates.len();
                self.apply_content_identity_ledger_updates(result.ledger_updates);
                crate::logger::log(format!(
                    "content_identity: backfill completed recorded={recorded} errors={}",
                    result.errors
                ));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.content_identity_backfill_pending = None;
                crate::logger::log("content_identity: backfill thread disconnected".to_string());
            }
            Some(Err(mpsc::TryRecvError::Empty)) => ctx.request_repaint_after(POLL_INTERVAL),
            None => {}
        }
    }

    fn poll_content_identity_record_updates(&mut self) {
        if !self.settings.edit_restore_prompt_enabled {
            return;
        }
        let updates = self
            .content_identity_recorder
            .as_ref()
            .map(|recorder| recorder.drain_updates())
            .unwrap_or_default();
        self.apply_content_identity_ledger_updates(updates);
    }

    fn poll_content_identity_index_load(&mut self, ctx: &egui::Context) {
        let event = self
            .content_identity_index_load_pending
            .as_ref()
            .map(ContentIdentityIndexLoadPending::try_recv);
        match event {
            Some(Ok(Ok(mut index))) => {
                self.content_identity_index_load_pending = None;
                if let Some(recorder) = self.content_identity_recorder.as_ref() {
                    self.content_identity_updates_before_load
                        .extend(recorder.drain_updates());
                }
                for update in self.content_identity_updates_before_load.drain(..) {
                    index.upsert(update);
                }
                crate::logger::log(format!(
                    "content_identity: loaded {} ledger rows for detection",
                    index.len()
                ));
                self.content_identity_index = index;
                self.content_identity_ledger_state = ContentIdentityLedgerState::Ready;
                self.maybe_start_content_identity_detection();
            }
            Some(Ok(Err(error))) => {
                self.content_identity_index_load_pending = None;
                self.mark_content_identity_ledger_unusable(format!(
                    "detection index load failed: {error}"
                ));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.content_identity_index_load_pending = None;
                self.mark_content_identity_ledger_unusable(
                    "detection index loader disconnected".to_string(),
                );
            }
            Some(Err(mpsc::TryRecvError::Empty)) => ctx.request_repaint_after(POLL_INTERVAL),
            None => {}
        }
    }

    fn mark_content_identity_ledger_unusable(&mut self, detail: String) {
        self.content_identity_index = ContentIdentityIndex::default();
        self.content_identity_updates_before_load.clear();
        self.content_identity_ledger_state = ContentIdentityLedgerState::Unusable(detail.clone());
        crate::logger::log(format!("content_identity: ledger unusable: {detail}"));
        self.show_feedback_toast(
            "コピー・移動したファイルの編集内容の復元を利用できません".to_string(),
        );
    }

    pub(crate) fn content_identity_detection_result_is_current(
        &self,
        result: &DetectionResult,
    ) -> bool {
        self.settings.edit_restore_prompt_enabled
            && self.content_identity_ledger_state.is_ready()
            && self.items_generation == result.items_generation
            && self.is_physical_folder_listing()
            && self.current_folder.as_deref().is_some_and(|folder| {
                crate::path_key::normalize_keep_drive(folder) == result.folder_key
            })
    }

    fn content_identity_target_has_existing_edit(&self, source: &ContentIdentitySource) -> bool {
        let key = crate::path_key::normalize_keep_drive(&source.path);
        let include_page_prefix = content_kind_uses_page_prefix(source.kind);
        [
            &self.adjusted_page_keys,
            &self.mask_page_keys,
            &self.conceal_page_keys,
            &self.local_adjust_page_keys,
            &self.comic_page_keys,
            &self.export_crop_page_keys,
        ]
        .into_iter()
        .any(|set| set_has_key_or_prefix(set, &key, include_page_prefix))
    }

    fn log_content_restore_candidates(&self, candidates: &[RestoreCandidate]) {
        let source_count = candidates
            .iter()
            .map(|candidate| candidate.sources.len())
            .sum::<usize>();
        crate::logger::log(format!(
            "content_identity: detection ready folder={} targets={} sources={}",
            self.current_folder
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            candidates.len(),
            source_count
        ));
        for candidate in candidates {
            for source in &candidate.sources {
                crate::logger::log(format!(
                    "content_identity: candidate target={} source={} relation={}",
                    candidate.target_path.display(),
                    source.path.display(),
                    if source.source_exists { "copy" } else { "move" }
                ));
            }
        }
    }
}

fn set_has_key_or_prefix(set: &BTreeSet<String>, key: &str, include_prefix: bool) -> bool {
    if set.contains(key) {
        return true;
    }
    if !include_prefix {
        return false;
    }
    let prefix = format!("{key}::");
    set.range(prefix.clone()..)
        .next()
        .is_some_and(|candidate| candidate.starts_with(&prefix))
}

fn content_kind_uses_page_prefix(kind: ContentKind) -> bool {
    matches!(
        kind,
        ContentKind::Zip | ContentKind::Pdf | ContentKind::Convertible
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_presence_covers_container_pages_but_not_sibling_names() {
        let set = BTreeSet::from([
            "c:/books/book.zip::chapter/page.jpg".to_string(),
            "c:/books/book.zip.old::page.jpg".to_string(),
        ]);
        assert!(set_has_key_or_prefix(&set, "c:/books/book.zip", true));
        assert!(!set_has_key_or_prefix(&set, "c:/books/book", true));
        assert!(!set_has_key_or_prefix(&set, "c:/books/book.zip", false));
    }

    #[test]
    fn existing_edit_presence_covers_exact_images_and_zip_pdf_page_prefixes() {
        let set = BTreeSet::from([
            "c:/images/edited.png".to_string(),
            "c:/books/book.zip::chapter/page.jpg".to_string(),
            "c:/books/book.pdf::page_3".to_string(),
        ]);

        assert!(set_has_key_or_prefix(
            &set,
            "c:/images/edited.png",
            content_kind_uses_page_prefix(ContentKind::Image)
        ));
        assert!(set_has_key_or_prefix(
            &set,
            "c:/books/book.zip",
            content_kind_uses_page_prefix(ContentKind::Zip)
        ));
        assert!(set_has_key_or_prefix(
            &set,
            "c:/books/book.pdf",
            content_kind_uses_page_prefix(ContentKind::Pdf)
        ));
    }
}
