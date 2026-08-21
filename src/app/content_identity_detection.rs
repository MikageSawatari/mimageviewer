use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use super::{App, top_level_grid_view};
use crate::content_identity::{
    ContentIdentityDetectionPending, ContentIdentityIndex, ContentIdentityIndexLoadPending,
    ContentIdentitySource, ContentKind, DetectionResult, RestoreCandidate,
};

const POLL_INTERVAL: Duration = Duration::from_millis(50);

impl App {
    pub(crate) fn start_content_identity_index_load(&mut self) {
        if !self.settings.edit_restore_prompt_enabled
            || self.content_identity_index_loaded
            || self.content_identity_index_load_pending.is_some()
        {
            return;
        }
        self.content_identity_index_load_pending = ContentIdentityIndexLoadPending::spawn();
        if self.content_identity_index_load_pending.is_none() {
            // spawn 失敗は自動 retry しない。設定を明示的に OFF→ON した場合だけ再試行する。
            self.content_identity_index_loaded = true;
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
            self.content_identity_index_loaded = false;
            self.content_identity_updates_before_load.clear();
        } else {
            self.content_identity_index_loaded = false;
            self.start_content_identity_index_load();
        }
    }

    pub(crate) fn cancel_content_identity_detection(&mut self) {
        if let Some(pending) = self.content_identity_detection_pending.take() {
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
        // OFF は最初の分岐にする。物理一覧判定も size index 参照も一切行わない。
        if !self.settings.edit_restore_prompt_enabled {
            return;
        }
        if !self.content_identity_index_loaded || !self.is_physical_folder_listing() {
            return;
        }
        let Some(folder) = self.current_folder.as_deref() else {
            return;
        };
        let folder_key = crate::path_key::normalize_keep_drive(folder);
        let mut targets = Vec::new();
        for (item, meta) in self.items.iter().zip(&self.image_metas) {
            let Some((_, size)) = meta else {
                continue;
            };
            let Ok(size) = u64::try_from(*size) else {
                continue;
            };
            let Some(source) = ContentIdentitySource::for_grid_item(item, None, Some(folder))
            else {
                continue;
            };
            if self.content_identity_target_has_existing_edit(&source) {
                continue;
            }
            if let Some(target) =
                crate::content_identity::stage0_target(&self.content_identity_index, source, size)
            {
                targets.push(target);
            }
        }
        if targets.is_empty() {
            return;
        }

        if let Some(pending) = self.content_identity_detection_pending.take() {
            pending.cancel();
        }
        let io_sem = self
            .indexer_manager
            .as_ref()
            .map(crate::indexer_manager::IndexerManager::io_sem)
            .unwrap_or_else(|| Arc::clone(&self.content_identity_fallback_io_sem));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "content_identity",
                "detect_enqueue",
                Some(&folder_key),
                self.input_seq,
                &[("targets", serde_json::Value::from(targets.len()))],
            );
        }
        self.content_identity_detection_pending = ContentIdentityDetectionPending::spawn(
            targets,
            self.items_generation,
            folder_key,
            self.input_seq,
            io_sem,
        );
    }

    pub(crate) fn poll_content_identity_detection(&mut self, ctx: &egui::Context) {
        self.poll_content_identity_record_updates();
        self.poll_content_identity_index_load(ctx);

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
                for update in result.ledger_updates {
                    self.content_identity_index.upsert(update);
                }
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

    fn poll_content_identity_record_updates(&mut self) {
        if !self.settings.edit_restore_prompt_enabled {
            return;
        }
        let updates = self
            .content_identity_recorder
            .as_ref()
            .map(|recorder| recorder.drain_updates())
            .unwrap_or_default();
        if self.content_identity_index_loaded {
            for update in updates {
                self.content_identity_index.upsert(update);
            }
        } else if self.content_identity_index_load_pending.is_some() {
            self.content_identity_updates_before_load.extend(updates);
        }
    }

    fn poll_content_identity_index_load(&mut self, ctx: &egui::Context) {
        let event = self
            .content_identity_index_load_pending
            .as_ref()
            .map(ContentIdentityIndexLoadPending::try_recv);
        match event {
            Some(Ok(Ok(mut index))) => {
                self.content_identity_index_load_pending = None;
                for update in self.content_identity_updates_before_load.drain(..) {
                    index.upsert(update);
                }
                crate::logger::log(format!(
                    "content_identity: loaded {} ledger rows for detection",
                    index.len()
                ));
                self.content_identity_index = index;
                self.content_identity_index_loaded = true;
                self.maybe_start_content_identity_detection();
            }
            Some(Ok(Err(error))) => {
                self.content_identity_index_load_pending = None;
                self.content_identity_updates_before_load.clear();
                self.content_identity_index_loaded = true;
                crate::logger::log(format!(
                    "content_identity: detection index load failed: {error}"
                ));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.content_identity_index_load_pending = None;
                self.content_identity_updates_before_load.clear();
                self.content_identity_index_loaded = true;
            }
            Some(Err(mpsc::TryRecvError::Empty)) => ctx.request_repaint_after(POLL_INTERVAL),
            None => {}
        }
    }

    pub(crate) fn content_identity_detection_result_is_current(
        &self,
        result: &DetectionResult,
    ) -> bool {
        self.settings.edit_restore_prompt_enabled
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
