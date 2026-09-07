//! Read-only App adapters for the opt-in UI smoke-test protocol.
//!
//! This module observes the existing viewer-context registry and detached host
//! claims. It does not mount a context, allocate a window identity, or advance a
//! lifecycle transition.

use super::{App, ContextResidence, FsCacheEntry, GridItem};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestScriptActivePaintOwner {
    Root,
    Detached {
        window_id: u64,
        viewport_id: egui::ViewportId,
    },
}

fn test_script_active_paint_owner(
    active_window_id: Option<u64>,
    expected_viewport_id: egui::ViewportId,
    painted_viewport_id: egui::ViewportId,
) -> Option<TestScriptActivePaintOwner> {
    match active_window_id {
        None if painted_viewport_id == egui::ViewportId::ROOT => {
            Some(TestScriptActivePaintOwner::Root)
        }
        Some(window_id) if painted_viewport_id == expected_viewport_id => {
            Some(TestScriptActivePaintOwner::Detached {
                window_id,
                viewport_id: painted_viewport_id,
            })
        }
        None | Some(_) => None,
    }
}

impl App {
    pub(crate) fn test_script_window_snapshots(
        &self,
    ) -> Vec<crate::test_script::TestScriptWindowSnapshot> {
        let root_context = self.viewer_context_main();
        let mut windows = self
            .viewer_context_ids()
            .into_iter()
            .filter_map(|context_id| {
                let window_id = self.viewer_context_window(context_id)?;
                let viewport_id = Self::detached_image_window_viewport_id(window_id);
                let identity = self.test_script_window_identity(window_id, viewport_id);
                self.test_script_context_window_snapshot(
                    context_id,
                    "detached",
                    Some(window_id),
                    viewport_id,
                    identity,
                )
            })
            .collect::<Vec<_>>();
        if let Some(root) = self.test_script_context_window_snapshot(
            root_context,
            "root",
            None,
            egui::ViewportId::ROOT,
            self.test_script_root_window_identity(egui::ViewportId::ROOT),
        ) {
            windows.push(root);
        }
        windows.sort_by_key(|window| window.window_id);
        windows
    }

    fn test_script_context_window_snapshot(
        &self,
        context_id: super::ViewerContextId,
        role: &'static str,
        window_id: Option<u64>,
        viewport_id: egui::ViewportId,
        identity: Option<crate::test_script::TestScriptWindowIdentity>,
    ) -> Option<crate::test_script::TestScriptWindowSnapshot> {
        let residence = self.viewer_context_residence(context_id);
        self.with_viewer_context_ref(context_id, |context| {
            let page_index = context.fullscreen_idx();
            let item = page_index.and_then(|idx| context.items().get(idx));
            let item_identity = item.map(GridItem::perf_key).unwrap_or_default();
            let media_kind = item.map(test_script_media_kind).unwrap_or("none");
            let page_ready = page_index.is_some_and(|idx| {
                matches!(
                    context.fs_cache().get(&idx),
                    Some(FsCacheEntry::Static { .. } | FsCacheEntry::Animated { .. })
                )
            });
            crate::test_script::TestScriptWindowSnapshot {
                host_incarnation: identity
                    .as_ref()
                    .and_then(|identity| identity.host_incarnation()),
                hwnd: identity.as_ref().map(|identity| identity.hwnd()),
                identity,
                role: role.to_string(),
                window_id,
                context_serial: context_id.serial(),
                viewport_id,
                residence: test_script_residence(residence).to_string(),
                media_kind: media_kind.to_string(),
                page_index,
                items_generation: context.items_generation(),
                item_identity,
                page_ready,
                viewport_rendered: false,
                viewport_revision: 0,
                paint_matches_current_page: false,
                full_texture_painted: false,
                paint_source: String::new(),
                paint_source_texture: String::new(),
                painted_page_index: None,
                paint_revision: 0,
            }
        })
    }

    pub(crate) fn test_script_root_window_identity(
        &self,
        viewport_id: egui::ViewportId,
    ) -> Option<crate::test_script::TestScriptWindowIdentity> {
        if viewport_id != egui::ViewportId::ROOT {
            return None;
        }
        let context_id = self.viewer_context_main();
        if !matches!(
            self.viewer_context_residence(context_id),
            ContextResidence::Mounted | ContextResidence::AtRest
        ) {
            return None;
        }
        let hwnd = self.main_hwnd? as u64;
        (hwnd != 0).then_some(crate::test_script::TestScriptWindowIdentity::Root {
            context_serial: context_id.serial(),
            hwnd,
        })
    }

    pub(crate) fn test_script_window_identity(
        &self,
        window_id: u64,
        viewport_id: egui::ViewportId,
    ) -> Option<crate::test_script::TestScriptWindowIdentity> {
        if viewport_id != Self::detached_image_window_viewport_id(window_id) {
            return None;
        }
        let (context_id, residence) = self.locate_window_context(window_id)?;
        if !matches!(
            residence,
            ContextResidence::Mounted | ContextResidence::AtRest
        ) {
            return None;
        }
        let claim = self.detached_window_manager.host_claim_alive(window_id)?;
        Some(crate::test_script::TestScriptWindowIdentity::Detached {
            window_id,
            context_serial: context_id.serial(),
            viewport_id,
            host_incarnation: claim.incarnation,
            hwnd: claim.hwnd,
        })
    }

    pub(crate) fn test_script_paint_source_kind(
        &self,
        page_index: usize,
        texture: &egui::TextureHandle,
    ) -> crate::test_script::TestScriptPaintSourceKind {
        let thumbnail = self.thumbnails.get(page_index).is_some_and(|thumbnail| {
            matches!(thumbnail, crate::grid_item::ThumbnailState::Loaded { tex, .. }
                if tex.id() == texture.id())
        });
        if thumbnail {
            crate::test_script::TestScriptPaintSourceKind::CatalogThumbnail
        } else {
            crate::test_script::TestScriptPaintSourceKind::FullOrProcessed
        }
    }

    pub(crate) fn test_script_content_proof(
        &self,
        page_index: usize,
        texture: &egui::TextureHandle,
        source_kind: crate::test_script::TestScriptPaintSourceKind,
    ) -> Option<crate::test_script::TestScriptContentProof> {
        let context_id = self.mounted_viewer_context_id()?;
        let context = self.with_viewer_context_ref(context_id, |context| {
            let current = (context.fullscreen_idx() == Some(page_index))
                .then(|| context.items().get(page_index))
                .flatten()?;
            Some((context.items_generation(), current.perf_key()))
        })??;
        Some(crate::test_script::TestScriptContentProof {
            context_serial: context_id.serial(),
            items_generation: context.0,
            page_index,
            item_identity: context.1,
            source_texture_id: texture.id(),
            source_kind,
        })
    }

    pub(crate) fn test_script_publish_window_snapshots(&self) {
        crate::test_script::publish_window_snapshots(self.test_script_window_snapshots());
    }

    pub(crate) fn test_script_publish_detached_frame(
        &self,
        window_id: u64,
        viewport_id: egui::ViewportId,
        content: Option<crate::test_script::TestScriptContentProof>,
    ) {
        // An immediate viewport obtains its first HWND only after its paint callback.
        // Refresh the read-only table after registration, then associate the local
        // proof with that manager-issued host incarnation.
        self.test_script_publish_window_snapshots();
        let Some(owner) = self.test_script_window_identity(window_id, viewport_id) else {
            return;
        };
        crate::test_script::publish_window_frame(owner, content);
    }

    pub(crate) fn test_script_publish_root_frame(
        &self,
        viewport_id: egui::ViewportId,
        content: Option<crate::test_script::TestScriptContentProof>,
    ) {
        self.test_script_publish_window_snapshots();
        let Some(owner) = self.test_script_root_window_identity(viewport_id) else {
            return;
        };
        crate::test_script::publish_window_frame(owner, content);
    }

    pub(crate) fn test_script_publish_active_frame(
        &self,
        active_window_id: Option<u64>,
        expected_viewport_id: egui::ViewportId,
        painted: Option<(
            egui::ViewportId,
            Option<crate::test_script::TestScriptContentProof>,
        )>,
    ) {
        let Some((painted_viewport_id, content)) = painted else {
            return;
        };
        match test_script_active_paint_owner(
            active_window_id,
            expected_viewport_id,
            painted_viewport_id,
        ) {
            Some(TestScriptActivePaintOwner::Root) => {
                self.test_script_publish_root_frame(painted_viewport_id, content);
            }
            Some(TestScriptActivePaintOwner::Detached {
                window_id,
                viewport_id,
            }) => {
                self.test_script_publish_detached_frame(window_id, viewport_id, content);
            }
            None => {}
        }
    }
}

fn test_script_residence(residence: ContextResidence) -> &'static str {
    match residence {
        ContextResidence::Mounted => "mounted",
        ContextResidence::AtRest => "at_rest",
        ContextResidence::Building => "building",
        ContextResidence::Retiring => "retiring",
        ContextResidence::Retired => "retired",
        ContextResidence::Unknown => "unknown",
    }
}

fn test_script_media_kind(item: &GridItem) -> &'static str {
    match item {
        GridItem::PdfPage { .. } | GridItem::PdfFile(_) => "pdf",
        GridItem::Image(_) | GridItem::ZipImage { .. } => "image",
        GridItem::Video(_) => "video",
        GridItem::Audio(_) => "audio",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn window_snapshot_reads_an_at_rest_context_without_mounting_it() {
        let mut app = crate::app::setup_app_for_test();
        let context_id = app.build_window_context_for_test(701, |app| {
            app.items = vec![GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\books\readonly.pdf"),
                page_num: 3,
                content_type: None,
            }];
            app.fullscreen_idx = Some(0);
        });
        assert_eq!(
            app.viewer_context_residence(context_id),
            ContextResidence::AtRest
        );

        let windows = app.test_script_window_snapshots();

        assert_eq!(
            app.viewer_context_residence(context_id),
            ContextResidence::AtRest
        );
        let window = windows
            .iter()
            .find(|window| window.window_id == Some(701))
            .expect("at-rest window snapshot");
        assert_eq!(window.context_serial, context_id.serial());
        assert_eq!(window.page_index, Some(0));
        assert_eq!(window.media_kind, "pdf");
        assert!(window.item_identity.contains("readonly.pdf#3"));
        assert!(
            window.identity.is_none(),
            "no host was created by observation"
        );
    }

    #[test]
    fn root_snapshot_uses_registry_main_while_a_detached_context_is_mounted() {
        let mut app = crate::app::setup_app_for_test();
        app.main_hwnd = Some(0x4a11);
        app.items = vec![GridItem::PdfPage {
            pdf_path: PathBuf::from(r"C:\books\root.pdf"),
            page_num: 1,
            content_type: None,
        }];
        app.fullscreen_idx = Some(0);
        let root_context = app.viewer_context_main();
        let detached_context = app.build_window_context_for_test(702, |app| {
            app.items = vec![GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\books\detached.pdf"),
                page_num: 8,
                content_type: None,
            }];
            app.fullscreen_idx = Some(0);
        });

        app.with_viewer_context(detached_context, |app| {
            assert_eq!(app.mounted_viewer_context_id(), Some(detached_context));
            let windows = app.test_script_window_snapshots();
            let root = windows
                .iter()
                .find(|window| window.role == "root")
                .expect("root snapshot");
            assert_eq!(root.window_id, None);
            assert_eq!(root.context_serial, root_context.serial());
            assert_eq!(root.viewport_id, egui::ViewportId::ROOT);
            assert_eq!(root.residence, "at_rest");
            assert!(root.item_identity.contains("root.pdf#1"));
            assert!(matches!(
                root.identity,
                Some(crate::test_script::TestScriptWindowIdentity::Root {
                    context_serial,
                    hwnd: 0x4a11,
                }) if context_serial == root_context.serial()
            ));
        })
        .expect("temporarily mount detached context");

        assert_eq!(app.mounted_viewer_context_id(), Some(root_context));
    }

    #[test]
    fn non_root_viewport_cannot_be_published_as_the_root_owner() {
        let mut app = crate::app::setup_app_for_test();
        app.main_hwnd = Some(0x4a12);
        assert!(
            app.test_script_root_window_identity(egui::ViewportId::from_hash_of(
                "dedicated-fullscreen"
            ))
            .is_none()
        );
        assert!(matches!(
            app.test_script_root_window_identity(egui::ViewportId::ROOT),
            Some(crate::test_script::TestScriptWindowIdentity::Root { hwnd: 0x4a12, .. })
        ));

        let dedicated = egui::ViewportId::from_hash_of("dedicated-fullscreen");
        assert_eq!(
            test_script_active_paint_owner(None, dedicated, dedicated),
            None,
            "a non-embedded fullscreen callback is not the root window"
        );
        assert_eq!(
            test_script_active_paint_owner(None, dedicated, egui::ViewportId::ROOT),
            Some(TestScriptActivePaintOwner::Root),
            "embedded fullscreen is identified by the viewport that actually painted"
        );
        let detached = egui::ViewportId::from_hash_of("detached-window");
        assert_eq!(
            test_script_active_paint_owner(Some(99), detached, detached),
            Some(TestScriptActivePaintOwner::Detached {
                window_id: 99,
                viewport_id: detached,
            })
        );
        assert_eq!(
            test_script_active_paint_owner(Some(99), detached, egui::ViewportId::ROOT),
            None,
            "a detached session must not publish a root observation"
        );
    }

    #[test]
    fn a_bound_main_context_is_listed_as_distinct_root_and_detached_owners() {
        let mut app = crate::app::setup_app_for_test();
        app.main_hwnd = Some(0x4a13);
        let main_context = app.viewer_context_main();
        app.bind_mounted_context_for_test(703);
        app.detached_window_hwnd_set(703, 0x7030);
        app.set_detached_window_live_hwnds_for_test([0x7030]);

        let windows = app.test_script_window_snapshots();
        let root = windows
            .iter()
            .find(|window| window.role == "root")
            .expect("root owner");
        let detached = windows
            .iter()
            .find(|window| window.window_id == Some(703))
            .expect("detached owner");
        assert_eq!(root.context_serial, main_context.serial());
        assert_eq!(detached.context_serial, main_context.serial());
        assert_ne!(root.identity, detached.identity);
        assert!(matches!(
            detached.identity,
            Some(crate::test_script::TestScriptWindowIdentity::Detached {
                window_id: 703,
                hwnd: 0x7030,
                ..
            })
        ));
    }
}
