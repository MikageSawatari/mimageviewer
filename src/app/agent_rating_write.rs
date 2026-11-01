//! Shared rating post-write effects. Agent admission and filesystem preparation
//! happen before this boundary; this module never enters UI edit history.
use super::*;
use crate::rating_db::{RatingItemKind, RatingMeta};

#[derive(Clone, Debug)]
pub(crate) struct PreparedRatingTarget {
    pub(crate) key: String,
    pub(crate) legacy_key: Option<String>,
    pub(crate) source: PathBuf,
    pub(crate) meta: RatingMeta,
    pub(crate) xmp_target: Option<PathBuf>,
    /// Worker-resolved using Settings::books_root_path(), including its OS
    /// default-folder lookup. Do not repeat that lookup on the UI thread.
    pub(crate) compiled_book_page: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RatingTargetStatus {
    Changed,
    Unchanged,
    Skipped,
}

#[derive(Clone, Debug)]
pub(crate) struct RatingTargetOutcome {
    pub(crate) key: String,
    pub(crate) before: Option<u8>,
    pub(crate) after: Option<u8>,
    pub(crate) status: RatingTargetStatus,
    pub(crate) error: Option<String>,
}

/// Preserve the released UI entry points' post-write policies while sharing
/// their effects. This is synchronous call intent, not persistent App state.
#[derive(Clone, Copy)]
pub(super) enum RatingWritePolicy {
    Item,
    NamedContainer,
    Agent,
}

pub(super) struct RatingWriteEffects {
    pub(super) policy: RatingWritePolicy,
    pub(super) slots: Vec<usize>,
    pub(super) leaf: bool,
    pub(super) container: bool,
    pub(super) xmp_target: Option<PathBuf>,
    pub(super) identity: Option<crate::content_identity::ContentIdentitySource>,
}

impl App {
    /// Canonical per-entry form of the shared user writer. This is the only
    /// additional DB path required for legacy ZIP names; it shares the ordinary
    /// UI writer's hydration fence and committed ledger/log publication.
    fn write_user_rating_canonical_shared_deferred_from(
        &mut self,
        target: &PreparedRatingTarget,
        stars: u8,
        slot: Option<usize>,
    ) -> Result<crate::rating_db::CanonicalRatingWrite, String> {
        self.rating_xmp_hydration
            .invalidate_for_user_write(&target.key);
        let write = self
            .rating_db
            .as_ref()
            .ok_or_else(|| "レーティング DB を利用できません".to_owned())?
            .set_user_rating_canonical(
                &target.key,
                target.legacy_key.as_deref(),
                stars,
                &target.meta,
            )
            .map_err(|error| format!("レーティング DB への保存に失敗しました: {error}"))?;
        if write.changed || write.legacy_removed {
            self.invalidate_book_append_undo();
            let reader_member = self
                .fullscreen_idx
                .is_some_and(|index| self.visible_indices.binary_search(&index).is_ok());
            self.record_committed_user_rating_shared(
                "agent",
                &target.key,
                Some(write.before),
                stars,
                slot,
                reader_member,
            );
        }
        Ok(write)
    }

    /// All successful UI and agent item/container writes converge here. The
    /// caller owns publication so a set of targets rebuilds membership once.
    pub(super) fn finish_rating_write_deferred(
        &mut self,
        key: &str,
        before: u8,
        stars: u8,
        effects: RatingWriteEffects,
    ) {
        for index in effects.slots {
            self.rating_cache.insert(index, stars);
        }
        self.invalidate_rating_counts_cache();
        if effects.leaf {
            self.apply_rating_delta_to_folder_counts(key, before, stars);
        }
        if effects.container {
            match effects.policy {
                RatingWritePolicy::Item => self.current_folder_rating_cache = None,
                RatingWritePolicy::NamedContainer | RatingWritePolicy::Agent => {
                    if self.showing_container_rating_key(key) {
                        self.current_folder_rating_cache = Some(stars);
                    }
                }
            }
        }
        if matches!(effects.policy, RatingWritePolicy::Item) || before != stars {
            self.schedule_current_smart_folder_metadata_refresh(
                smart_folder::SmartFolderMetadataDependency::Rating,
            );
        }
        if self.settings.write_rating_to_xmp
            && let Some(path) = effects.xmp_target
        {
            self.ensure_rating_write_handle();
            if let Some(handle) = self.rating_write_handle.as_ref() {
                handle.submit(crate::rating_write_worker::RatingWriteJob {
                    path,
                    rating: (stars != 0).then_some(stars),
                });
            }
        }
        if let Some(source) = effects.identity {
            self.record_content_identity_source(
                source,
                crate::content_identity::ContentIdentityTrigger::Edit,
            );
        }
    }

    /// Called only after the controller's side-effect-free atomic admission.
    /// Each entry commits separately; failures do not prevent later targets.
    pub(crate) fn apply_agent_rating_targets(
        &mut self,
        targets: &[PreparedRatingTarget],
        stars: u8,
    ) -> Vec<RatingTargetOutcome> {
        let stars = stars.min(5);
        let mut slots: HashMap<String, Vec<usize>> = HashMap::new();
        for index in 0..self.items.len() {
            if let Some(key) = self.rating_path_key(index) {
                slots.entry(key).or_default().push(index);
            }
        }
        let mut outcomes = Vec::with_capacity(targets.len());
        for target in targets {
            let matching = slots.get(&target.key).cloned().unwrap_or_default();
            let result = self.write_user_rating_canonical_shared_deferred_from(
                target,
                stars,
                matching.first().copied(),
            );
            let write = match result {
                Ok(write) => write,
                Err(error) => {
                    self.report_rating_write_error(&error);
                    outcomes.push(RatingTargetOutcome {
                        key: target.key.clone(),
                        before: None,
                        after: None,
                        status: RatingTargetStatus::Skipped,
                        error: Some(error),
                    });
                    continue;
                }
            };
            if write.changed || write.legacy_removed {
                // Compiled-book pages remain DB-only, including off-list paths.
                // The prepared target already checks its physical image format.
                let compiled = target.compiled_book_page;
                self.finish_rating_write_deferred(
                    &target.key,
                    write.before,
                    stars,
                    RatingWriteEffects {
                        policy: RatingWritePolicy::Agent,
                        slots: matching,
                        leaf: matches!(
                            target.meta.kind,
                            RatingItemKind::Image
                                | RatingItemKind::Video
                                | RatingItemKind::Audio
                                | RatingItemKind::ZipImage
                                | RatingItemKind::PdfPage
                        ),
                        container: matches!(
                            target.meta.kind,
                            RatingItemKind::Folder
                                | RatingItemKind::ZipFile
                                | RatingItemKind::PdfFile
                                | RatingItemKind::ConvertibleArchive
                                | RatingItemKind::ZipDir
                        ),
                        xmp_target: (!compiled && write.changed)
                            .then(|| target.xmp_target.clone())
                            .flatten(),
                        identity: (!compiled)
                            .then(|| {
                                crate::content_identity::ContentIdentitySource::from_path(
                                    &target.source,
                                )
                            })
                            .flatten(),
                    },
                );
            }
            outcomes.push(RatingTargetOutcome {
                key: target.key.clone(),
                before: Some(write.before),
                after: Some(stars),
                status: if write.changed {
                    RatingTargetStatus::Changed
                } else {
                    RatingTargetStatus::Unchanged
                },
                error: None,
            });
        }
        self.publish_current_context_rating_writes();
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::phase_c_support::setup_app;
    use super::*;

    fn target(path: PathBuf, kind: RatingItemKind) -> PreparedRatingTarget {
        PreparedRatingTarget {
            key: crate::adjustment_db::normalize_path(&path),
            legacy_key: None,
            meta: RatingMeta::new(kind).with_source_path(&path),
            xmp_target: (kind == RatingItemKind::Image
                && crate::xmp_writer::is_writable_format(&path))
            .then(|| path.clone()),
            source: path,
            compiled_book_page: false,
        }
    }

    #[test]
    fn ui_rating_item_and_named_container_preserve_distinct_cache_policies() {
        let mut env = setup_app();
        let current = env.tmp.path().join("current");
        let other = env.tmp.path().join("other");
        for path in [&current, &other] {
            std::fs::create_dir_all(path).unwrap();
        }
        let other_target = target(other.clone(), RatingItemKind::Folder);
        let current_target = target(current.clone(), RatingItemKind::Folder);
        env.current_folder = Some(current.clone());
        env.items = vec![
            GridItem::Folder(other.clone()),
            GridItem::Folder(current.clone()),
        ];
        env.rating_cache = [(0, 0), (1, 0)].into_iter().collect();
        env.current_folder_rating_cache = Some(2);

        // Item writes lazily invalidate the address-bar cache even when the
        // changed container is another folder.
        assert!(env.set_rating_result_deferred(0, 3).unwrap());
        assert_eq!(env.current_folder_rating_cache, None);
        env.current_folder_rating_cache = Some(2);
        assert!(
            env.write_container_rating_for_target_with_publication(
                &other_target.key,
                &other,
                &other_target.meta,
                4,
                false,
                false,
            )
            .unwrap()
        );
        assert_eq!(env.current_folder_rating_cache, Some(2));

        // Named-container same-value calls retain their existing early return:
        // no DB/ledger/effects work and no publication.
        let generation = env.rating_session_write_generation;
        assert!(
            !env.write_container_rating_for_target_with_publication(
                &other_target.key,
                &other,
                &other_target.meta,
                4,
                false,
                false,
            )
            .unwrap()
        );
        assert_eq!(env.rating_session_write_generation, generation);
        assert_eq!(env.current_folder_rating_cache, Some(2));
        assert_eq!(env.rating_publication_count, 0);

        // For the shown container, the item path still clears the cache while
        // the explicitly named writer installs its known committed value.
        assert!(env.set_rating_result_deferred(1, 3).unwrap());
        assert_eq!(env.current_folder_rating_cache, None);
        assert!(
            env.write_container_rating_for_target_with_publication(
                &current_target.key,
                &current,
                &current_target.meta,
                5,
                false,
                false,
            )
            .unwrap()
        );
        assert_eq!(env.current_folder_rating_cache, Some(5));
        assert_eq!(env.meta_undo.undo_len(), 0);
    }

    #[test]
    fn agent_rating_shared_pipeline_updates_off_list_counts_and_preserves_user_draft() {
        let mut env = setup_app();
        let root = env.tmp.path().join("root");
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let first = target(child.join("a.png"), RatingItemKind::Image);
        let second = target(child.join("b.png"), RatingItemKind::Image);
        let container = target(child.clone(), RatingItemKind::Folder);
        let app = &mut env.app;
        app.items = vec![
            GridItem::Image(first.source.clone()),
            GridItem::Folder(child.clone()),
        ];
        app.thumbnails = vec![ThumbnailState::Pending; 2];
        app.image_metas = vec![None; 2];
        app.visible_indices = vec![0, 1];
        app.current_folder = Some(root.clone());
        app.current_folder_rating_cache = Some(2);
        app.rating_db
            .as_ref()
            .unwrap()
            .set_user_rating(&first.key, 3, Some(&first.meta))
            .unwrap();
        app.rating_cache.insert(0, 3);
        app.rating_cache.insert(1, 0);
        app.rating_counts_cache = Some([1, 0, 0, 1, 0, 0]);
        app.folder_rating_counts_folder_key = Some(crate::adjustment_db::normalize_path(&root));
        app.folder_rating_counts_loaded = true;
        let aggregate = crate::adjustment_db::normalize_path(&child);
        app.folder_rating_counts
            .insert(aggregate.clone(), [0, 0, 1, 0, 0]);
        let owner = app.capture_page_edit_owner(0).unwrap();
        let mut draft = crate::adjustment::AdjustParams::default();
        draft.brightness = 0.25;
        app.adjustment_drag_session = Some(AdjustmentDragSession {
            fs_idx: 0,
            owner: owner.clone(),
            before: None,
            draft: Some(draft.clone()),
        });
        app.adjustment_dragging = true;
        let undo = app.meta_undo.undo_len();
        let result = app.apply_agent_rating_targets(
            &[first.clone(), second.clone(), first.clone(), container],
            4,
        );
        assert_eq!(
            result.iter().map(|row| row.status).collect::<Vec<_>>(),
            [
                RatingTargetStatus::Changed,
                RatingTargetStatus::Changed,
                RatingTargetStatus::Unchanged,
                RatingTargetStatus::Changed
            ]
        );
        assert_eq!(result[0].before, Some(3));
        assert_eq!(result[2].before, Some(4));
        assert_eq!(app.rating_cache.get(&0), Some(&4));
        assert_eq!(app.rating_cache.get(&1), Some(&4));
        assert_eq!(app.folder_rating_counts[&aggregate], [0, 0, 0, 2, 0]);
        assert_eq!(app.current_folder_rating_cache, Some(2));
        assert!(app.rating_counts_cache.is_none());
        assert_eq!(app.rating_publication_count, 1);
        assert_eq!(app.meta_undo.undo_len(), undo);
        let session = app
            .adjustment_drag_session
            .as_ref()
            .expect("AI write must not settle the user's draft");
        assert_eq!(session.owner, owner);
        assert_eq!(session.draft, Some(draft));
        assert!(app.adjustment_dragging);
        let generation = app.rating_session_write_generation;
        assert_eq!(app.apply_agent_rating_targets(&[first, second], 4).len(), 2);
        assert_eq!(app.rating_session_write_generation, generation);
        assert_eq!(app.rating_publication_count, 1);
    }

    #[test]
    fn agent_rating_xmp_respects_setting_format_compiled_pages_and_same_value() {
        let mut env = setup_app();
        let root = env.tmp.path().to_path_buf();
        let normal = target(root.join("normal.png"), RatingItemKind::Image);
        let unsupported = target(root.join("unsupported.bmp"), RatingItemKind::Image);
        std::fs::write(&unsupported.source, b"unchanged media bytes").unwrap();
        let mut compiled = target(
            root.join("books").join("one").join("page.png"),
            RatingItemKind::Image,
        );
        compiled.compiled_book_page = true;
        std::fs::create_dir_all(compiled.source.parent().unwrap()).unwrap();
        for path in [&normal.source, &compiled.source] {
            image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
                .save(path)
                .unwrap();
        }
        env.settings.book_root = Some(root.join("books"));
        env.settings.write_rating_to_xmp = false;
        env.apply_agent_rating_targets(&[normal.clone()], 2);
        assert!(env.rating_write_handle.is_none());
        env.settings.write_rating_to_xmp = true;
        env.apply_agent_rating_targets(&[normal.clone(), compiled.clone(), unsupported.clone()], 4);
        assert_eq!(env.rating_publication_count, 2);
        drop(env.rating_write_handle.take()); // drain the real shared XMP worker
        assert_eq!(crate::xmp_reader::read_xmp_rating(&normal.source), Some(4));
        assert_eq!(crate::xmp_reader::read_xmp_rating(&compiled.source), None);
        assert_eq!(
            std::fs::read(&unsupported.source).unwrap(),
            b"unchanged media bytes"
        );
        let bytes = std::fs::read(&normal.source).unwrap();
        env.apply_agent_rating_targets(&[normal.clone(), compiled], 4);
        assert!(
            env.rating_write_handle.is_none(),
            "unchanged call queues no XMP job"
        );
        assert_eq!(std::fs::read(&normal.source).unwrap(), bytes);
        assert_eq!(env.rating_publication_count, 2);
    }

    #[test]
    fn agent_rating_records_off_list_content_identity_and_updates_current_container() {
        let mut env = setup_app();
        let path = env.tmp.path().join("off-list.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        env.content_identity_recorder =
            crate::content_identity::ContentIdentityRecorder::spawn_at_for_test(
                env.tmp.path().join("rating-identity.db"),
            );
        let item = target(path.clone(), RatingItemKind::Image);
        env.apply_agent_rating_targets(&[item], 5);
        let recorded = env
            .content_identity_recorder
            .as_ref()
            .unwrap()
            .recv_update_for_test(std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            recorded.file_key,
            crate::adjustment_db::normalize_path(&path)
        );
        assert!(recorded.has_restorable_content);
        let folder = env.tmp.path().join("current");
        std::fs::create_dir_all(&folder).unwrap();
        env.current_folder = Some(folder.clone());
        env.current_folder_rating_cache = Some(0);
        env.apply_agent_rating_targets(&[target(folder, RatingItemKind::Folder)], 3);
        assert_eq!(env.current_folder_rating_cache, Some(3));
    }

    #[test]
    fn agent_rating_two_legacy_zip_clears_survive_later_normal_open_migration() {
        let mut env = setup_app();
        let archive = env.tmp.path().join("book.zip");
        let mut targets = Vec::new();
        for (legacy, canonical, stars) in [("legacy-A", "A", 4), ("legacy-B", "B", 5)] {
            let key = crate::adjustment_db::zip_entry_key(&archive, canonical);
            let old = crate::adjustment_db::zip_entry_key(&archive, legacy);
            env.rating_db
                .as_ref()
                .unwrap()
                .set_user_rating(&old, stars, None)
                .unwrap();
            targets.push(PreparedRatingTarget {
                key,
                legacy_key: Some(old),
                source: archive.clone(),
                meta: RatingMeta::new(RatingItemKind::ZipImage).with_source_path(&archive),
                xmp_target: None,
                compiled_book_page: false,
            });
        }
        for target in &targets {
            let result = env.apply_agent_rating_targets(std::slice::from_ref(target), 0);
            assert_eq!(result[0].status, RatingTargetStatus::Changed);
        }
        crate::zip_key_migration::run_migration_at(
            env.tmp.path(),
            &archive,
            &[
                ("legacy-A".into(), "A".into()),
                ("legacy-B".into(), "B".into()),
            ],
        );
        for target in targets {
            assert_eq!(env.rating_db.as_ref().unwrap().get(&target.key), 0);
            assert_eq!(
                env.rating_db
                    .as_ref()
                    .unwrap()
                    .get(target.legacy_key.as_deref().unwrap()),
                0
            );
        }
        assert_eq!(env.meta_undo.undo_len(), 0);
    }
}
