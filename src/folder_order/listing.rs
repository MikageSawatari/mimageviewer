//! Memory-only ownership and resolution of physical-folder manual order.

use super::*;

/// A producer must explicitly carry the completed read (including no record).
/// Stack suppression is transient and never changes the stored active bit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FolderOrderListingInput {
    #[default]
    NotApplicable,
    Read {
        result: Result<Option<FolderOrderRecord>, FolderOrderError>,
        stack_active: bool,
    },
}

impl FolderOrderListingInput {
    pub(crate) fn from_read(result: OrderResult<Option<FolderOrderRecord>>) -> Self {
        Self::Read {
            result,
            stack_active: false,
        }
    }

    /// Worker-only: wait for this request's FIFO reply, never on the UI thread.
    pub(crate) fn read_worker(handle: Option<&FolderOrderHandle>, path: &Path) -> Self {
        Self::from_read(
            handle
                .ok_or_else(|| FolderOrderError::new("folder order actor unavailable"))
                .and_then(|handle| handle.read(path))
                .and_then(|rx| {
                    rx.recv()
                        .map_err(|_| FolderOrderError::new("folder order actor stopped"))?
                }),
        )
    }

    pub(crate) fn with_stack(self, enabled: bool) -> Self {
        match self {
            Self::NotApplicable => Self::NotApplicable,
            Self::Read { result, .. } => Self::Read {
                result,
                stack_active: enabled,
            },
        }
    }

    pub(crate) fn is_read(&self) -> bool {
        matches!(self, Self::Read { .. })
    }

    fn result(&self) -> Option<&OrderResult<Option<FolderOrderRecord>>> {
        match self {
            Self::NotApplicable => None,
            Self::Read { result, .. } => Some(result),
        }
    }

    pub(crate) fn error(&self) -> Option<&FolderOrderError> {
        self.result().and_then(|result| result.as_ref().err())
    }

    pub(crate) fn active_record(&self) -> Option<&FolderOrderRecord> {
        match self {
            Self::Read {
                result: Ok(Some(record)),
                stack_active: false,
            } if record.active => Some(record),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FolderOrderMode {
    #[default]
    Normal,
    ManualUnrecorded,
    ManualRecorded,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FolderOrderState {
    pub folder_key: Option<String>,
    pub mode: FolderOrderMode,
    pub input: FolderOrderListingInput,
    pub record: Option<FolderOrderRecord>,
    pub inventory: HashSet<String>,
    /// Context revision, independent of the actor's persisted record revision.
    pub order_revision: u64,
}

#[derive(Clone, Debug)]
pub(crate) enum FolderOrderWriteIntent {
    Save(FolderOrderSave),
    SetActive { active: bool, revision: u64 },
    Reset { revision: u64 },
}

impl FolderOrderState {
    pub(crate) fn from_listing(
        path: &Path,
        input: FolderOrderListingInput,
        inventory: HashSet<String>,
    ) -> Self {
        let record = input
            .result()
            .and_then(|result| result.as_ref().ok())
            .cloned()
            .flatten();
        let mode = if record.as_ref().is_some_and(|record| record.active) {
            FolderOrderMode::ManualRecorded
        } else {
            FolderOrderMode::Normal
        };
        Self {
            folder_key: (!matches!(input, FolderOrderListingInput::NotApplicable))
                .then(|| folder_key(path)),
            mode,
            input,
            record,
            inventory,
            order_revision: 1,
        }
    }

    fn advance(&mut self) {
        self.order_revision = self
            .order_revision
            .checked_add(1)
            .expect("folder order revision exhausted");
    }

    pub(crate) fn manual_active(&self) -> bool {
        self.mode != FolderOrderMode::Normal
    }

    pub(crate) fn applies_to_physical_listing(
        &self,
        settings: &crate::settings::Settings,
        path: &Path,
        items: &[crate::grid_item::GridItem],
    ) -> bool {
        self.manual_active()
            && matches!(
                self.input,
                FolderOrderListingInput::Read {
                    stack_active: false,
                    ..
                }
            )
            && physical_folder_order_eligible(settings, path, items)
    }

    pub(crate) fn writable(&self) -> bool {
        self.folder_key.is_some() && self.input.error().is_none()
    }

    /// Refresh adopts the same context's snapshot without rolling its revision back.
    pub(crate) fn adopt_listing(
        &mut self,
        input: FolderOrderListingInput,
        inventory: HashSet<String>,
    ) {
        let Some(folder) = self.folder_key.clone() else {
            return;
        };
        let unrecorded = self.mode == FolderOrderMode::ManualUnrecorded;
        let revision = self.order_revision;
        *self = Self::from_listing(Path::new(&folder), input, inventory);
        self.order_revision = revision;
        if unrecorded && self.record.is_none() && self.writable() {
            self.mode = FolderOrderMode::ManualUnrecorded;
        }
        self.advance();
    }

    pub(crate) fn listing_input(&self) -> FolderOrderListingInput {
        if matches!(self.input, FolderOrderListingInput::NotApplicable) {
            return FolderOrderListingInput::NotApplicable;
        }
        if let Some(error) = self.input.error() {
            return FolderOrderListingInput::from_read(Err(error.clone()));
        }
        let mut record = self.record.clone();
        if let Some(record) = &mut record {
            record.active = self.mode == FolderOrderMode::ManualRecorded;
        }
        FolderOrderListingInput::from_read(Ok(record))
    }

    pub(crate) fn listing_input_for_stack(&self, enabled: bool) -> FolderOrderListingInput {
        self.listing_input().with_stack(enabled)
    }

    fn sync_input(&mut self) {
        let stack_active = matches!(
            self.input,
            FolderOrderListingInput::Read {
                stack_active: true,
                ..
            }
        );
        self.input = self.listing_input().with_stack(stack_active);
    }

    pub(crate) fn enter_manual(&mut self) -> Option<FolderOrderWriteIntent> {
        if !self.writable() || self.manual_active() {
            return None;
        }
        self.advance();
        let intent = if let Some(record) = &mut self.record {
            record.active = true;
            self.mode = FolderOrderMode::ManualRecorded;
            Some(FolderOrderWriteIntent::SetActive {
                active: true,
                revision: self.order_revision,
            })
        } else {
            self.mode = FolderOrderMode::ManualUnrecorded;
            None
        };
        self.sync_input();
        intent
    }

    pub(crate) fn leave_manual(&mut self) -> Option<FolderOrderWriteIntent> {
        if !self.manual_active() {
            return None;
        }
        self.advance();
        self.mode = FolderOrderMode::Normal;
        let intent = self.record.as_mut().map(|record| {
            record.active = false;
            FolderOrderWriteIntent::SetActive {
                active: false,
                revision: self.order_revision,
            }
        });
        self.sync_input();
        intent
    }

    pub(crate) fn reset(&mut self) -> Option<FolderOrderWriteIntent> {
        if !self.writable() || self.record.is_none() {
            return None;
        }
        self.advance();
        self.record = None;
        self.input = FolderOrderListingInput::from_read(Ok(None));
        self.mode = FolderOrderMode::ManualUnrecorded;
        Some(FolderOrderWriteIntent::Reset {
            revision: self.order_revision,
        })
    }

    /// Commit only an actual visible permutation. Hidden listing slots and recorded
    /// inventory-only children retain their positions through the two shared merges.
    pub(crate) fn commit_reorder(
        &mut self,
        full: &[String],
        visible: &HashSet<String>,
        reordered: &[String],
    ) -> OrderResult<Option<FolderOrderWriteIntent>> {
        if !self.writable() || !self.manual_active() {
            return Ok(None);
        }
        let listing_order = merge_visible_order(full, visible, reordered)?;
        if listing_order == full {
            return Ok(None);
        }
        let previous = self
            .record
            .as_ref()
            .map_or(&[][..], |record| record.items.as_slice());
        let items = merge_inventory_order(
            self.folder_key.as_deref().unwrap(),
            previous,
            &listing_order,
            &self.inventory,
        )?;
        self.advance();
        self.record = Some(FolderOrderRecord {
            active: true,
            revision: self.record.as_ref().map_or(0, |record| record.revision),
            items,
        });
        self.mode = FolderOrderMode::ManualRecorded;
        self.sync_input();
        Ok(Some(FolderOrderWriteIntent::Save(FolderOrderSave {
            listing_order,
            inventory: self.inventory.clone(),
            revision: self.order_revision,
        })))
    }

    pub(crate) fn rename(&mut self, mappings: &[FolderOrderRename]) -> bool {
        let before = (
            self.folder_key.clone(),
            self.record.clone(),
            self.inventory.clone(),
        );
        self.folder_key = self
            .folder_key
            .as_deref()
            .and_then(|key| rewrite_key_with_mappings(key, mappings));
        self.inventory = self
            .inventory
            .iter()
            .filter_map(|key| rewrite_key_with_mappings(key, mappings))
            .collect();
        if let Some(record) = &mut self.record {
            record.items = record
                .items
                .iter()
                .filter_map(|key| rewrite_key_with_mappings(key, mappings))
                .collect();
        }
        let changed = before
            != (
                self.folder_key.clone(),
                self.record.clone(),
                self.inventory.clone(),
            );
        if changed {
            self.advance();
            self.sync_input();
        }
        changed
    }

    pub(crate) fn delete(&mut self, keys: &[String]) -> bool {
        let retained = |key: &String| {
            !keys
                .iter()
                .any(|removed| key == removed || key.starts_with(&direct_child_prefix(removed)))
        };
        let inventory_len = self.inventory.len();
        self.inventory.retain(retained);
        let mut changed = self.inventory.len() != inventory_len;
        if let Some(record) = &mut self.record {
            let len = record.items.len();
            record.items.retain(retained);
            changed |= len != record.items.len();
        }
        if changed {
            self.advance();
            self.sync_input();
        }
        changed
    }
}

/// The same structural page-order boundary is used by PC, Remote, and Agent.
/// The physical-listing caller supplies provenance; synthetic views use NotApplicable.
pub(crate) fn physical_folder_order_eligible(
    settings: &crate::settings::Settings,
    path: &Path,
    items: &[crate::grid_item::GridItem],
) -> bool {
    use crate::grid_item::GridItem;
    // A Stack is exclusively a group of physical still images. Eligibility asks
    // about the source content even while that content is displayed aggregated.
    let still_images_only = !items.is_empty()
        && items
            .iter()
            .all(|item| matches!(item, GridItem::Image(_) | GridItem::Stack { .. }));
    !crate::folder_tree::path_eq(&settings.books_root_path(), path)
        && !crate::app::physical_page_order_locked_with_still_images(
            settings,
            path,
            still_images_only,
        )
}

pub(crate) fn physical_item_key(item: &crate::grid_item::GridItem) -> Option<String> {
    use crate::grid_item::GridItem;
    match item {
        GridItem::Folder(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path)
        | GridItem::Image(path)
        | GridItem::Video(path)
        | GridItem::Audio(path)
        | GridItem::ConvertibleArchive { path, .. } => Some(folder_key(path)),
        _ => None,
    }
}

/// Output-to-input permutation over the complete base arrangement, across every
/// display row (including rows with no assigned kind). Unknowns remain stable.
pub(crate) fn resolve_listing_order(
    keys: &[Option<String>],
    input: &FolderOrderListingInput,
    eligible: bool,
) -> Vec<usize> {
    let mut order = (0..keys.len()).collect::<Vec<_>>();
    let Some(record) = input.active_record().filter(|_| eligible) else {
        return order;
    };
    let positions = record
        .items
        .iter()
        .enumerate()
        .map(|(index, key)| (key.as_str(), index))
        .collect::<HashMap<_, _>>();
    order.sort_by_key(|index| {
        keys[*index]
            .as_deref()
            .and_then(|key| positions.get(key).copied())
            .unwrap_or(usize::MAX)
    });
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> String {
        folder_key(&Path::new(r"C:\manual").join(name))
    }
    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| key(name)).collect()
    }
    fn input(active: bool, names: &[&str]) -> FolderOrderListingInput {
        FolderOrderListingInput::from_read(Ok(Some(FolderOrderRecord {
            active,
            revision: 6,
            items: keys(names),
        })))
    }
    fn state(input: FolderOrderListingInput) -> FolderOrderState {
        FolderOrderState::from_listing(
            Path::new(r"C:\manual"),
            input,
            keys(&["a.jpg", "b.jpg", "hidden.png", "unknown.txt"])
                .into_iter()
                .collect(),
        )
    }

    #[test]
    fn section1911_resolver_known_first_missing_skipped_unknown_base_order_and_case_identity() {
        let base = keys(&["unknown-folder", "a.jpg", "b.jpg", "unknown-image.jpg"]);
        let input = input(true, &["B.JPG", "gone.jpg", "A.JPG"]);
        assert_eq!(
            resolve_listing_order(
                &base.into_iter().map(Some).collect::<Vec<_>>(),
                &input,
                true
            ),
            [2, 1, 0, 3]
        );
        // Synthetic/uncategorized rows are retained as stable unknowns as well.
        assert_eq!(
            resolve_listing_order(&[None, Some(key("a.jpg")), None], &input, true),
            [1, 0, 2]
        );
    }

    #[test]
    fn section1911_resolver_unrecorded_inactive_stack_and_page_locked_use_base_order() {
        let base = keys(&["a.jpg", "b.jpg"])
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        for (input, eligible) in [
            (FolderOrderListingInput::from_read(Ok(None)), true),
            (input(false, &["b.jpg", "a.jpg"]), true),
            (input(true, &["b.jpg", "a.jpg"]).with_stack(true), true),
            (input(true, &["b.jpg", "a.jpg"]), false),
        ] {
            assert_eq!(resolve_listing_order(&base, &input, eligible), [0, 1]);
        }
    }

    #[test]
    fn section1911_manual_transition_records_only_first_changed_reorder_and_recorded_toggles() {
        let mut state = state(FolderOrderListingInput::from_read(Ok(None)));
        let initial = state.order_revision;
        assert!(state.enter_manual().is_none());
        assert_eq!(state.mode, FolderOrderMode::ManualUnrecorded);
        assert!(state.order_revision > initial);
        let full = keys(&["a.jpg", "b.jpg"]);
        let visible = full.iter().cloned().collect();
        assert!(
            state
                .commit_reorder(&full, &visible, &full)
                .unwrap()
                .is_none()
        );
        assert!(state.record.is_none());
        assert!(matches!(
            state
                .commit_reorder(&full, &visible, &keys(&["b.jpg", "a.jpg"]))
                .unwrap(),
            Some(FolderOrderWriteIntent::Save(_))
        ));
        assert_eq!(state.mode, FolderOrderMode::ManualRecorded);
        assert!(matches!(
            state.leave_manual(),
            Some(FolderOrderWriteIntent::SetActive { active: false, .. })
        ));
        assert_eq!(
            state.record.as_ref().unwrap().items,
            keys(&["b.jpg", "a.jpg"])
        );
        assert!(matches!(
            state.enter_manual(),
            Some(FolderOrderWriteIntent::SetActive { active: true, .. })
        ));
        let revision = state.order_revision;
        assert!(matches!(
            state.reset(),
            Some(FolderOrderWriteIntent::Reset { .. })
        ));
        assert_eq!(state.mode, FolderOrderMode::ManualUnrecorded);
        assert!(state.record.is_none());
        assert!(state.order_revision > revision);
        // Opening another context does not inherit the transient selection.
        let next = FolderOrderState::from_listing(
            Path::new(r"C:\other"),
            FolderOrderListingInput::from_read(Ok(None)),
            HashSet::new(),
        );
        assert_eq!(next.mode, FolderOrderMode::Normal);
    }

    #[test]
    fn section1911_reorder_preserves_facet_and_inventory_hidden_slots_drops_missing_and_leaves_unknown_unrecorded()
     {
        let mut state = state(input(true, &["a.jpg", "hidden.png", "b.jpg", "gone.jpg"]));
        let full = keys(&["a.jpg", "facet.jpg", "b.jpg"]);
        state.inventory.insert(key("facet.jpg"));
        let visible = keys(&["a.jpg", "b.jpg"]).into_iter().collect();
        let intent = state
            .commit_reorder(&full, &visible, &keys(&["b.jpg", "a.jpg"]))
            .unwrap();
        assert!(matches!(intent, Some(FolderOrderWriteIntent::Save(_))));
        assert_eq!(
            state.record.unwrap().items,
            keys(&["b.jpg", "hidden.png", "facet.jpg", "a.jpg"])
        );
    }

    #[test]
    fn section1911_adoption_revision_never_rolls_back_and_forks_are_independent() {
        let mut first = state(input(true, &["b.jpg", "a.jpg"]));
        first.order_revision = 40;
        let sibling = first.clone();
        first.adopt_listing(FolderOrderListingInput::from_read(Ok(None)), HashSet::new());
        assert_eq!(first.order_revision, 41);
        assert_eq!(sibling.order_revision, 40);
        assert!(sibling.manual_active());
        first.enter_manual();
        first.adopt_listing(FolderOrderListingInput::from_read(Ok(None)), HashSet::new());
        assert_eq!(first.mode, FolderOrderMode::ManualUnrecorded);
        assert_eq!(first.order_revision, 43);
    }

    #[test]
    fn section1911_rename_delete_update_record_and_inventory_before_next_save() {
        let mut state = state(input(true, &["a.jpg", "b.jpg", "hidden.png"]));
        // Unknown source replaces stale recorded destination without inheriting it.
        state.inventory.insert(key("source.jpg"));
        assert!(state.rename(&[FolderOrderRename::new(
            Path::new(&key("source.jpg")),
            Path::new(&key("b.jpg")),
            RenameScope::Exact
        )]));
        assert_eq!(
            state.record.as_ref().unwrap().items,
            keys(&["a.jpg", "hidden.png"])
        );
        assert!(state.delete(&keys(&["hidden.png"])));
        assert!(!state.inventory.contains(&key("hidden.png")));
        let full = keys(&["a.jpg", "b.jpg"]);
        let visible = full.iter().cloned().collect();
        state
            .commit_reorder(&full, &visible, &keys(&["b.jpg", "a.jpg"]))
            .unwrap();
        assert_eq!(
            state.record.as_ref().unwrap().items,
            keys(&["b.jpg", "a.jpg"])
        );
    }

    #[test]
    fn section1911_corrupt_read_keeps_base_order_and_disables_writes() {
        let mut state = state(FolderOrderListingInput::from_read(Err(
            FolderOrderError::new("corrupt fixture"),
        )));
        assert!(!state.writable());
        assert!(state.enter_manual().is_none());
        assert!(state.reset().is_none());
        assert!(state.listing_input().error().is_some());
    }

    #[test]
    fn section1911_bookshelf_root_and_direct_book_are_ineligible() {
        let mut settings = crate::settings::Settings::default();
        settings.book_root = Some(PathBuf::from(r"C:\shelf"));
        assert!(!physical_folder_order_eligible(
            &settings,
            Path::new(r"C:\shelf"),
            &[]
        ));
        assert!(!physical_folder_order_eligible(
            &settings,
            Path::new(r"C:\shelf\book"),
            &[]
        ));
        assert!(physical_folder_order_eligible(
            &settings,
            Path::new(r"C:\ordinary"),
            &[]
        ));
    }

    #[test]
    fn section1911_physical_directory_suffix_keeps_manual_order_locked() {
        use crate::grid_item::GridItem;
        let mut settings = crate::settings::Settings::default();
        settings.book_root = Some(PathBuf::from(r"C:\shelf"));
        settings.auto_fullscreen_image_folders = false;
        for suffix in ["epub", "pdf", "zip", "cbz", "rar", "cbr"] {
            let path = PathBuf::from(format!(r"C:\ordinary.{suffix}"));
            let items = [GridItem::Image(path.join("a.jpg"))];
            // Manual order retains the same suffix lock as ordinary sorting,
            // including a physical directory with a container-looking name.
            assert!(!physical_folder_order_eligible(&settings, &path, &items));
            assert!(crate::app::physical_page_order_locked(
                &settings, &path, &items
            ));
        }
    }

    #[test]
    fn section1911_stack_eligibility_uses_source_still_images_without_changing_existing_sort_lock()
    {
        use crate::grid_item::GridItem;
        let path = Path::new(r"C:\ordinary");
        let image = GridItem::Image(path.join("a.jpg"));
        let stack = GridItem::Stack {
            key: "group".into(),
            representative: path.join("b.jpg"),
            count: 2,
        };
        let video = GridItem::Video(path.join("movie.mp4"));
        let mut settings = crate::settings::Settings::default();
        settings.book_root = Some(PathBuf::from(r"C:\shelf"));
        settings.auto_fullscreen_zip_pdf = true;
        settings.auto_fullscreen_image_folders = true;
        assert!(!physical_folder_order_eligible(
            &settings,
            path,
            &[image.clone()]
        ));
        assert!(!physical_folder_order_eligible(
            &settings,
            path,
            &[stack.clone()]
        ));
        assert!(!physical_folder_order_eligible(
            &settings,
            path,
            &[image.clone(), stack.clone()]
        ));
        assert!(physical_folder_order_eligible(
            &settings,
            path,
            &[stack.clone(), video]
        ));
        assert!(physical_folder_order_eligible(&settings, path, &[]));
        // The released nonmanual sort predicate deliberately retains its old
        // interpretation of aggregate cells; only manual eligibility uses proxies.
        assert!(!crate::app::physical_page_order_locked(
            &settings,
            path,
            &[stack.clone()]
        ));
        settings.auto_fullscreen_image_folders = false;
        assert!(physical_folder_order_eligible(
            &settings,
            path,
            &[image, stack]
        ));
    }

    #[test]
    fn section1911_explicit_shutdown_drains_accepted_save_and_closes_even_with_live_handle() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join(DB_NAME);
        let service = FolderOrderService::start_at(database.clone()).unwrap();
        let handle = service.handle.clone();
        let root = Path::new(r"C:\manual");
        handle
            .save(
                root,
                FolderOrderSave {
                    listing_order: keys(&["b.jpg", "a.jpg"]),
                    inventory: keys(&["a.jpg", "b.jpg"]).into_iter().collect(),
                    revision: 1,
                },
            )
            .unwrap();
        service.shutdown();
        assert!(handle.read(root).is_err());
        let persisted = FolderOrderDb::open_at(&database).unwrap();
        let record = persisted.read(&folder_key(root)).unwrap().unwrap();
        assert_eq!(record.items, keys(&["b.jpg", "a.jpg"]));
        drop(persisted);
        // The joined actor released its connection before the fixture directory.
        std::fs::remove_file(database).unwrap();
    }
}
