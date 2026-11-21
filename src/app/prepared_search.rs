//! Final search rows and their reading-position index cross the worker boundary together.
use super::file_type_resume::{LegacyResumeSource, PreparedPageIdentityIndex};
use crate::{
    file_type_visibility::FileTypeVisibility,
    grid_item::GridItem,
    settings::{EpubFileHandling, Settings},
};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchRowConditions {
    visibility: FileTypeVisibility,
    epub: EpubFileHandling,
}

impl SearchRowConditions {
    pub(crate) fn capture(settings: &Settings) -> Self {
        Self {
            visibility: settings.file_type_visibility.clone(),
            epub: settings.epub_file_handling,
        }
    }

    pub(crate) fn matches(&self, settings: &Settings) -> bool {
        self == &Self::capture(settings)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedSearchRows {
    pub(crate) items: Vec<GridItem>,
    pub(crate) image_metas: Vec<Option<(i64, i64)>>,
    pub(crate) video_items: Vec<(usize, PathBuf, u64)>,
    pub(crate) page_identity_index: Arc<PreparedPageIdentityIndex>,
    pub(crate) conditions: SearchRowConditions,
}

impl PreparedSearchRows {
    /// Called after final membership and order are fixed, on the search worker.
    /// Empty Ctrl+S submissions also use this pure, constant-size preparation inline.
    pub(crate) fn prepare(
        items: Vec<GridItem>,
        image_metas: Vec<Option<(i64, i64)>>,
        video_items: Vec<(usize, PathBuf, u64)>,
        settings: &Settings,
        surface: &'static str,
    ) -> Self {
        let started = std::time::Instant::now();
        let page_identity_index = Arc::new(PreparedPageIdentityIndex::prepare_for_source(
            &items,
            None,
            settings.file_type_visibility.clone(),
            LegacyResumeSource::General,
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "search",
                "page_identity_prepare",
                None,
                0,
                &[
                    ("surface", surface.into()),
                    ("items", items.len().into()),
                    ("ms", (started.elapsed().as_secs_f64() * 1000.0).into()),
                ],
            );
        }
        Self {
            items,
            image_metas,
            video_items,
            page_identity_index,
            conditions: SearchRowConditions::capture(settings),
        }
    }
}
