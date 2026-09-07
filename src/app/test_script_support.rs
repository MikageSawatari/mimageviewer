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

fn test_script_active_detached_target_matches(
    app: &App,
    window_id: u64,
    viewport_id: egui::ViewportId,
    owner: &crate::test_script::TestScriptWindowIdentity,
) -> bool {
    app.active_detached_window_id() == Some(window_id)
        && app
            .test_script_window_identity(window_id, viewport_id)
            .as_ref()
            == Some(owner)
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
                backend_token: identity.as_ref().map(|identity| identity.backend_token()),
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
        if hwnd == 0 {
            return None;
        }
        let witness = eframe::miv_test_script_window_witness::latest(viewport_id)?;
        if witness.hwnd() != hwnd {
            return None;
        }
        Some(crate::test_script::TestScriptWindowIdentity::Root {
            context_serial: context_id.serial(),
            hwnd,
            backend_token: witness.token(),
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
        let witness = eframe::miv_test_script_window_witness::latest(viewport_id)?;
        if witness.hwnd() != claim.hwnd {
            return None;
        }
        Some(crate::test_script::TestScriptWindowIdentity::Detached {
            window_id,
            context_serial: context_id.serial(),
            viewport_id,
            host_incarnation: claim.incarnation,
            hwnd: claim.hwnd,
            backend_token: witness.token(),
        })
    }

    pub(crate) fn test_script_action_owner_for_pass(
        &self,
        ctx: &egui::Context,
    ) -> Option<crate::test_script::TestScriptWindowIdentity> {
        let viewport_id = ctx.viewport_id();
        let mounted_context = self.mounted_viewer_context_id()?;
        let identity = if viewport_id == egui::ViewportId::ROOT {
            let identity = self.test_script_root_window_identity(viewport_id)?;
            (identity.context_serial() == mounted_context.serial()).then_some(identity)
        } else {
            let window_id = self.active_detached_window_id()?;
            let identity = self.test_script_window_identity(window_id, viewport_id)?;
            (identity.context_serial() == mounted_context.serial()).then_some(identity)
        }?;
        eframe::miv_test_script_window_witness::active()
            .is_some_and(|witness| identity.matches_backend_witness(witness))
            .then_some(identity)
    }

    /// Join a targeted smoke action to the ordinary detached activation queue.
    ///
    /// The script does not own a manager intent. It waits until every production
    /// intent is absent, validates the exact host identity, queues one ordinary
    /// activation, and commits that queue immediately so a lower window id cannot
    /// be selected in between those steps.
    pub(crate) fn test_script_drive_targeted_activation(&mut self, ctx: &egui::Context) -> bool {
        let Some(owner) = crate::test_script::pending_targeted_detached_owner() else {
            return false;
        };
        let crate::test_script::TestScriptWindowIdentity::Detached {
            window_id,
            viewport_id,
            ..
        } = &owner
        else {
            crate::test_script::finish_targeted_detached_owner(
                &owner,
                Err("only a detached target may require activation".to_string()),
            );
            return false;
        };

        let window_id = *window_id;
        let viewport_id = *viewport_id;

        if self.detached_window_manager.has_activation_intent() {
            return false;
        }

        if self.active_detached_window_id() == Some(window_id) {
            if test_script_active_detached_target_matches(self, window_id, viewport_id, &owner) {
                crate::test_script::finish_targeted_detached_owner(&owner, Ok(()));
                ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Focus);
                ctx.request_repaint_of(viewport_id);
            } else {
                crate::test_script::finish_targeted_detached_owner(
                    &owner,
                    Err(format!(
                        "run_action active target host changed: {}",
                        owner.describe()
                    )),
                );
            }
            return false;
        }

        let Some((_, residence)) = self.locate_window_context(window_id) else {
            crate::test_script::finish_targeted_detached_owner(
                &owner,
                Err(format!(
                    "run_action target has no current viewer context: {}",
                    owner.describe()
                )),
            );
            return false;
        };
        if self.test_script_window_identity(window_id, viewport_id) != Some(owner.clone()) {
            crate::test_script::finish_targeted_detached_owner(
                &owner,
                Err(format!(
                    "run_action target host changed before activation: {}",
                    owner.describe()
                )),
            );
            return false;
        }

        if residence != ContextResidence::AtRest || !self.detached_window_can_activate(window_id) {
            crate::test_script::finish_targeted_detached_owner(
                &owner,
                Err(format!(
                    "run_action target cannot be activated: {} residence={residence:?}",
                    owner.describe()
                )),
            );
            return false;
        }

        self.queue_deferred_detached_window_activation(window_id, "test_script_targeted_action");
        let committed = self.commit_pending_deferred_detached_window_activation(ctx);
        let actual_owner = self
            .active_detached_window_id()
            .filter(|active| *active == window_id)
            .and_then(|_| self.test_script_window_identity(window_id, viewport_id));
        if committed && actual_owner.as_ref() == Some(&owner) {
            crate::test_script::finish_targeted_detached_owner(&owner, Ok(()));
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Focus);
            ctx.request_repaint_of(viewport_id);
        } else {
            crate::test_script::finish_targeted_detached_owner(
                &owner,
                Err(format!(
                    "run_action target activation did not establish the selected owner: expected={} actual={actual_owner:?}",
                    owner.describe()
                )),
            );
        }
        true
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
        witness: eframe::miv_test_script_window_witness::WindowWitness,
        content: Option<crate::test_script::TestScriptContentProof>,
    ) {
        // An immediate viewport obtains its first HWND only after its paint callback.
        // Refresh the read-only table after registration, then associate the local
        // proof with that manager-issued host incarnation.
        self.test_script_publish_window_snapshots();
        let Some(owner) = self.test_script_window_identity(window_id, viewport_id) else {
            return;
        };
        if !owner.matches_backend_witness(witness) {
            return;
        }
        crate::test_script::publish_window_frame(owner, content);
    }

    pub(crate) fn test_script_publish_root_frame(
        &self,
        viewport_id: egui::ViewportId,
        witness: eframe::miv_test_script_window_witness::WindowWitness,
        content: Option<crate::test_script::TestScriptContentProof>,
    ) {
        self.test_script_publish_window_snapshots();
        let Some(owner) = self.test_script_root_window_identity(viewport_id) else {
            return;
        };
        if !owner.matches_backend_witness(witness) {
            return;
        }
        crate::test_script::publish_window_frame(owner, content);
    }

    pub(crate) fn test_script_publish_active_frame(
        &self,
        active_window_id: Option<u64>,
        expected_viewport_id: egui::ViewportId,
        painted: Option<(
            egui::ViewportId,
            eframe::miv_test_script_window_witness::WindowWitness,
            Option<crate::test_script::TestScriptContentProof>,
        )>,
    ) {
        let Some((painted_viewport_id, witness, content)) = painted else {
            return;
        };
        match test_script_active_paint_owner(
            active_window_id,
            expected_viewport_id,
            painted_viewport_id,
        ) {
            Some(TestScriptActivePaintOwner::Root) => {
                self.test_script_publish_root_frame(painted_viewport_id, witness, content);
            }
            Some(TestScriptActivePaintOwner::Detached {
                window_id,
                viewport_id,
            }) => {
                self.test_script_publish_detached_frame(window_id, viewport_id, witness, content);
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
        let backend_ctx = egui::Context::default();
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&backend_ctx, egui::ViewportId::ROOT, 0x4a11);
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
                    ..
                }) if context_serial == root_context.serial()
            ));
        })
        .expect("temporarily mount detached context");

        assert_eq!(app.mounted_viewer_context_id(), Some(root_context));
    }

    #[test]
    fn action_pass_owner_requires_the_identity_context_to_be_mounted() {
        let mut app = crate::app::setup_app_for_test();
        app.main_hwnd = Some(0x4a14);
        let detached_context = app.build_window_context_for_test(704, |_| {});
        let ctx = egui::Context::default();
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&ctx, egui::ViewportId::ROOT, 0x4a14);

        assert!(matches!(
            app.test_script_action_owner_for_pass(&ctx),
            Some(crate::test_script::TestScriptWindowIdentity::Root { .. })
        ));
        app.with_viewer_context(detached_context, |app| {
            assert_eq!(
                app.test_script_action_owner_for_pass(&ctx),
                None,
                "a ROOT viewport must not claim the registry-main owner during a detached mount"
            );
        })
        .expect("temporarily mount detached context");
    }

    #[test]
    fn active_detached_target_is_recognized_while_its_context_is_at_rest() {
        let mut app = crate::app::setup_app_for_test();
        let window_id = 705;
        let context_id = app.build_window_context_for_test(window_id, |_| {});
        let viewport_id = App::detached_image_window_viewport_id(window_id);
        app.detached_window_hwnd_set(window_id, 0x7050);
        app.set_detached_window_live_hwnds_for_test([0x7050]);
        app.begin_active_detached_session(window_id, crate::app::DetachedSource::Image);
        let backend_ctx = egui::Context::default();
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&backend_ctx, viewport_id, 0x7050);
        let owner = app
            .test_script_window_identity(window_id, viewport_id)
            .expect("active detached identity");

        assert_eq!(
            app.viewer_context_residence(context_id),
            ContextResidence::AtRest
        );
        assert!(test_script_active_detached_target_matches(
            &app,
            window_id,
            viewport_id,
            &owner
        ));
    }

    #[test]
    fn non_root_viewport_cannot_be_published_as_the_root_owner() {
        let mut app = crate::app::setup_app_for_test();
        app.main_hwnd = Some(0x4a12);
        let backend_ctx = egui::Context::default();
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&backend_ctx, egui::ViewportId::ROOT, 0x4a12);
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
        let backend_ctx = egui::Context::default();
        let root_backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _root_scope = root_backend.enter(&backend_ctx, egui::ViewportId::ROOT, 0x4a13);
        let detached_viewport = App::detached_image_window_viewport_id(703);
        let detached_backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _detached_scope = detached_backend.enter(&backend_ctx, detached_viewport, 0x7030);

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

    #[test]
    fn same_hwnd_backend_replacement_changes_the_root_identity_token() {
        let mut app = crate::app::setup_app_for_test();
        app.main_hwnd = Some(0x4a15);
        let ctx = egui::Context::default();
        let first = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let (first_owner, first_witness) = {
            let _scope = first.enter(&ctx, egui::ViewportId::ROOT, 0x4a15);
            (
                app.test_script_root_window_identity(egui::ViewportId::ROOT)
                    .expect("first backend owner"),
                eframe::miv_test_script_window_witness::active().unwrap(),
            )
        };
        let replacement = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _replacement_scope = replacement.enter(&ctx, egui::ViewportId::ROOT, 0x4a15);
        let replacement_owner = app
            .test_script_root_window_identity(egui::ViewportId::ROOT)
            .expect("replacement backend owner");

        assert_eq!(first_owner.hwnd(), replacement_owner.hwnd());
        assert_ne!(
            first_owner.backend_token(),
            replacement_owner.backend_token()
        );
        assert!(first_owner.matches_backend_witness(first_witness));
        assert!(
            !replacement_owner.matches_backend_witness(first_witness),
            "paint evidence captured by the old callback must not be relabeled as replacement output"
        );
        assert!(
            !first_owner
                .matches_backend_witness(eframe::miv_test_script_window_witness::active().unwrap()),
            "the old selected owner must not match the replacement callback"
        );
    }
}
