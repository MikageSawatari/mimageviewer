//! Production App/eframe connections, without a native window or renderer.
use super::*;
use std::cell::RefCell;

#[derive(Clone, Debug)]
struct Checkpoint {
    name: &'static str,
    pass: usize,
    frame: u64,
    primary_down: bool,
    update_pending: bool,
    metadata_pending: bool,
    erase_pending: usize,
    erase_ready: usize,
    local_ready: usize,
    local_pending: usize,
    upload_frame: u64,
    trim_save_pending: bool,
    rescan_pending: bool,
    hydration_ids: Option<(u64, Option<u64>)>,
}

thread_local! {
    static CHECKPOINTS: RefCell<Option<Vec<Checkpoint>>> = const { RefCell::new(None) };
}

/// Read-only, opt-in observations at existing production checkpoints. No fake poll or UI gate.
pub(super) fn observe(name: &'static str, app: &App, ctx: &egui::Context) {
    CHECKPOINTS.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(events) = slot.as_mut() {
            events.push(Checkpoint {
                name,
                pass: ctx.current_pass_index(),
                frame: app.frame_counter,
                primary_down: ctx.input(|input| input.pointer.primary_down()),
                update_pending: app.update_check_pending.is_some(),
                metadata_pending: app.metadata_pending.is_some(),
                erase_pending: app.erase_inpaint_pending.len(),
                erase_ready: app.erase_result_cache.len(),
                local_ready: app.local_adjust_cache.len(),
                local_pending: app.local_adjust_pending.len(),
                upload_frame: app.last_edit_materialize_upload_frame,
                trim_save_pending: app.view_trim_save_pending,
                rescan_pending: app.external_rescan_pending.is_some(),
                hydration_ids: app
                    .sidecar_restore
                    .as_ref()
                    .map(|state| state.connection_request_ids_for_test()),
            });
        }
    });
}

struct Trace;
impl Trace {
    fn start() -> Self {
        CHECKPOINTS.with(|slot| assert!(slot.replace(Some(Vec::new())).is_none()));
        Self
    }
    fn take(&self) -> Vec<Checkpoint> {
        CHECKPOINTS.with(|slot| std::mem::take(slot.borrow_mut().as_mut().unwrap()))
    }
}
impl Drop for Trace {
    fn drop(&mut self) {
        CHECKPOINTS.with(|slot| *slot.borrow_mut() = None);
    }
}

fn prepared() -> AppTestEnvForTest {
    let mut env = setup_app_for_test();
    // The shared fixture already owns a disposable profile and terminal fake startup services.
    // Skip only the unrelated first-launch folder selection, as other handler fixtures do.
    env.initialized = true;
    env.settings.update_check_enabled = false;
    env.settings.minimize_to_tray_on_close = false;
    env.indexer_init = IndexerInit::Unavailable;
    env.last_main_focused = true;
    env
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    // Production installs this while constructing App from CreationContext. The shared
    // windowless App fixture has no CreationContext, so install the same begin-pass producer.
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    ctx
}

fn minimized_input(events: Vec<egui::Event>) -> egui::RawInput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(960.0, 640.0),
        )),
        events,
        ..Default::default()
    };
    let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
    viewport.minimized = Some(true);
    viewport.focused = Some(true);
    input
}

fn full(backend: &mut eframe::HeadlessEpiIntegration, app: &mut App, events: Vec<egui::Event>) {
    let output = backend.update(app, minimized_input(events));
    assert!(output.viewport_output.contains_key(&egui::ViewportId::ROOT));
    assert!(
        !output.viewport_output.values().any(|viewport| {
            viewport
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Minimized(false)))
        }),
        "a minimized Full must not require restoring the root window"
    );
    output.drop_without_applying_deltas();
}

fn image(app: &mut App, ctx: &egui::Context, path: PathBuf) -> usize {
    let idx = app.items.len();
    app.items.push(GridItem::Image(path));
    app.thumbnails.push(ThumbnailState::Pending);
    app.image_metas.push(None);
    app.visible_indices.push(idx);
    static_source(app, ctx, idx);
    idx
}

fn static_source(app: &mut App, ctx: &egui::Context, idx: usize) {
    let pixels = Arc::new(egui::ColorImage::filled([8, 8], egui::Color32::WHITE));
    let tex = ctx.load_texture(
        format!("connection-raw-{idx}"),
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    let load_seq = app.input_seq;
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            tex,
            pixels,
            source_dims: Some([8, 8]),
            load_seq,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
}

fn erase_ready(
    app: &mut App,
    idx: usize,
    purpose: crate::ui_erase::EraseInpaintPurpose,
) -> mpsc::Sender<crate::ui_erase::EraseInpaintResult> {
    use crate::ui_erase::*;
    let (tx, rx) = mpsc::channel();
    let (progress_tx, progress_rx) = mpsc::channel();
    progress_tx
        .send(EraseInpaintProgress::Running {
            pass_index: 0,
            pass_count: 1,
            tile_index: 1,
            tile_count: 2,
        })
        .unwrap();
    tx.send(EraseInpaintResult::Pixels(egui::ColorImage::filled(
        [8, 8],
        egui::Color32::RED,
    )))
    .unwrap();
    app.erase_inpaint_pending.insert(
        EraseInpaintPendingKey {
            idx,
            kind: EraseInpaintKind::Commit,
        },
        EraseInpaintPending {
            idx,
            items_generation: app.items_generation,
            path_key: app.page_path_key(idx),
            rx,
            progress_rx,
            progress: EraseInpaintProgress::Preparing,
            cancel: Arc::new(AtomicBool::new(false)),
            started_at: std::time::Instant::now(),
            input_generation: 0,
            mask_generation: 0,
            purpose,
        },
    );
    tx
}

fn local_ready(app: &mut App, idx: usize) {
    let request = EditMaterializeRequest {
        items_generation: app.items_generation,
        input_seq: app.input_seq,
        kind: EditMaterializeKind::Local {
            key: app.current_local_adjust_key(idx),
            continuation: LocalMaterializeContinuation::Display,
        },
    };
    let (_tx, rx) = mpsc::channel();
    app.local_adjust_pages.insert(idx);
    app.local_adjust_pending.insert(
        idx,
        LocalAdjustRenderPending {
            request,
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            ready: Some(EditMaterializeResult::Ready {
                request,
                payload: EditMaterializePayload::LocalReady {
                    layers: Arc::new(vec![local_adjust_core::LocalAdjustmentLayer::new(
                        "connection",
                        local_adjust_core::LocalMask::Full,
                        local_adjust_core::LocalEffect::Tone(local_adjust_core::ToneParams {
                            brightness: 10.0,
                            ..Default::default()
                        }),
                    )]),
                    image: egui::ColorImage::filled([8, 8], egui::Color32::BLUE),
                },
            }),
        },
    );
}

fn metadata_ready(app: &mut App) -> mpsc::Sender<MetadataLoadResult> {
    let (tx, rx) = mpsc::channel();
    tx.send(MetadataLoadResult {
        key: "connection-metadata".into(),
        metadata: None,
        exif: None,
        xmp: None,
        panorama: None,
        sidecar: None,
    })
    .unwrap();
    app.metadata_pending = Some(MetadataLoadPending {
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    });
    tx
}

fn update_ready(app: &mut App) -> mpsc::Sender<Result<crate::update_check::UpdateInfo, String>> {
    let (tx, rx) = mpsc::channel();
    tx.send(Err("connection-result".into())).unwrap();
    app.update_check_pending = Some(UpdateCheckPending { rx, manual: true });
    tx
}

#[test]
fn egui037_continuation_real_minimized_full_preserves_checkpoints_and_owner_adoption() {
    let mut env = prepared();
    let ctx = context();
    let (_folder, rescan_tx, signature) = rescan_fixture(&mut env, false, false);
    static_source(&mut env, &ctx, 0);
    let update_tx = update_ready(&mut env);
    let metadata_tx = metadata_ready(&mut env);
    let erase_tx = erase_ready(
        &mut env,
        0,
        crate::ui_erase::EraseInpaintPurpose::MaterializeForEntry {
            entry: OriginalMaterialEntry::MaterialOnly,
        },
    );
    local_ready(&mut env, 0);
    let frame = env.frame_counter;
    let sequence = env.input_seq;
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
    let trace = Trace::start();
    for _ in 0..3 {
        let (_output, returned) =
            backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert!(returned.events.is_empty());
        assert_eq!((env.frame_counter, env.input_seq), (frame, sequence));
        assert!(env.update_check_pending.is_some());
        assert!(env.metadata_pending.is_some());
        assert_eq!(env.erase_inpaint_pending.len(), 1);
        assert!(env.erase_result_cache.is_empty());
        assert!(env.local_adjust_cache.is_empty());
        assert!(env.external_rescan_pending.is_some());
        assert_eq!(env.current_folder_watch.as_ref().unwrap().acknowledged, 0);
        assert!(trace.take().is_empty());
    }
    full(&mut backend, &mut env, Vec::new());
    let checkpoints = trace.take();
    let first: Vec<_> = checkpoints
        .iter()
        .filter(|point| point.pass == 0)
        .map(|point| point.name)
        .collect();
    assert_eq!(
        first,
        [
            "before_frame",
            "update_begin",
            "rescan_after",
            "startup_before",
            "startup_after",
            "initial_open",
            "initial_workers",
            "media_before",
            "media_after",
            "search_before",
            "search_after",
            "other_workers",
            "materials_before",
            "materials_after",
            "after_frame"
        ]
    );
    let at = |name| checkpoints.iter().find(|point| point.name == name).unwrap();
    // The existing early mounted-owner traversal also services erase/local material.
    // Its adoption precedes update_begin; F16 is an additional poll, not an exclusive one.
    assert_eq!(at("before_frame").erase_pending, 1);
    assert_eq!(at("before_frame").erase_ready, 0);
    assert_eq!(at("before_frame").local_ready, 0);
    assert_eq!(at("before_frame").local_pending, 1);
    assert!(at("before_frame").rescan_pending);
    assert_eq!(at("update_begin").erase_pending, 0);
    assert_eq!(at("update_begin").erase_ready, 1);
    assert_eq!(at("update_begin").local_ready, 1);
    assert_eq!(at("update_begin").local_pending, 0);
    assert_eq!(at("update_begin").upload_frame, frame);
    assert!(!at("update_begin").rescan_pending);
    assert!(at("startup_before").update_pending);
    assert!(!at("startup_after").update_pending);
    assert_eq!(at("rescan_after").frame, frame);
    assert!(!at("rescan_after").rescan_pending);
    assert_eq!(env.current_folder_signature, Some(signature));
    assert_eq!(env.current_folder_watch.as_ref().unwrap().acknowledged, 7);
    assert!(rescan_tx.send(None).is_err());
    assert_eq!(at("startup_before").frame, frame + 1);
    assert_eq!(at("media_before").erase_pending, 0);
    assert_eq!(at("media_before").erase_ready, 1);
    assert_eq!(at("media_before").local_ready, 1);
    assert_eq!(at("media_after").erase_pending, 0);
    assert_eq!(at("media_after").erase_ready, 1);
    assert_eq!(at("media_after").local_ready, 1);
    assert!(at("search_before").metadata_pending);
    assert!(!at("search_after").metadata_pending);
    assert!(env.metadata_cache.contains_key("connection-metadata"));
    assert_eq!(env.update_check_error.as_deref(), Some("connection-result"));
    assert!(update_tx.send(Err("duplicate".into())).is_err());
    assert!(
        metadata_tx
            .send(MetadataLoadResult {
                key: "duplicate".into(),
                metadata: None,
                exif: None,
                xmp: None,
                panorama: None,
                sidecar: None
            })
            .is_err()
    );
    assert!(
        erase_tx
            .send(crate::ui_erase::EraseInpaintResult::MaskAbsent)
            .is_err()
    );
    let texture = env.erase_result_cache.values().next().unwrap().texture.id();
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(
        env.erase_result_cache.values().next().unwrap().texture.id(),
        texture
    );
    assert!(!env.metadata_cache.contains_key("duplicate"));
}

#[test]
fn egui037_continuation_real_minimized_full_retains_material_upload_budget_through_logic() {
    let mut env = prepared();
    let ctx = context();
    for name in ["a.png", "b.png", "c.png"] {
        let path = env.tmp.path().join(name);
        let idx = image(&mut env, &ctx, path);
        assert!(idx < 3);
    }
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    // Settle the ordinary initial sizing/discard pass before publishing ready workers.
    // The existing early traversal uses the old counter, then F8 increments it before F16.
    // Preserve that released ordering and the upload budget for each explicit counter value.
    full(&mut backend, &mut env, Vec::new());
    let frame = env.frame_counter;
    let previous_upload = env.last_edit_materialize_upload_frame;
    for idx in 0..3 {
        local_ready(&mut env, idx);
    }
    let trace = Trace::start();
    full(&mut backend, &mut env, Vec::new());
    let checkpoints = trace.take();
    assert_local_material_timeline(&checkpoints, frame, 0, previous_upload, true);
    assert_eq!(env.local_adjust_cache.len(), 2);
    assert_eq!(env.local_adjust_pending.len(), 1);
    let budget_frame = env.last_edit_materialize_upload_frame;
    assert_eq!(budget_frame, frame + 1);
    assert_eq!(env.frame_counter, frame + 1);
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert_eq!(env.local_adjust_cache.len(), 2);
        assert_eq!(env.local_adjust_pending.len(), 1);
        assert_eq!(env.frame_counter, frame + 1);
        assert_eq!(env.last_edit_materialize_upload_frame, budget_frame);
        assert!(trace.take().is_empty());
    }
    full(&mut backend, &mut env, Vec::new());
    assert_local_material_timeline(&trace.take(), frame + 1, 2, budget_frame, false);
    assert_eq!(env.local_adjust_cache.len(), 3);
    assert!(env.local_adjust_pending.is_empty());
    assert_eq!(env.last_edit_materialize_upload_frame, frame + 2);
    let textures: HashSet<_> = env
        .local_adjust_cache
        .values()
        .map(|entry| entry.texture.id())
        .collect();
    assert_eq!(textures.len(), 3);
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(
        env.local_adjust_cache
            .values()
            .map(|entry| entry.texture.id())
            .collect::<HashSet<_>>(),
        textures
    );
}

fn assert_local_material_timeline(
    checkpoints: &[Checkpoint],
    old_frame: u64,
    initial_ready: usize,
    previous_upload: u64,
    early_adopts: bool,
) {
    assert!(
        checkpoints.iter().all(|point| point.pass == 0),
        "measured Full unexpectedly discarded: {checkpoints:#?}"
    );
    let names: Vec<_> = checkpoints.iter().map(|point| point.name).collect();
    assert_eq!(
        names,
        [
            "before_frame",
            "update_begin",
            "rescan_after",
            "startup_before",
            "startup_after",
            "initial_open",
            "initial_workers",
            "media_before",
            "media_after",
            "search_before",
            "search_after",
            "other_workers",
            "materials_before",
            "materials_after",
            "after_frame"
        ]
    );
    let mut adoption_by_counter = HashMap::<u64, usize>::new();
    let mut last_ready = initial_ready;
    let early_ready = initial_ready + usize::from(early_adopts);
    for (position, point) in checkpoints.iter().enumerate() {
        let (frame, ready, upload) = if position == 0 {
            (old_frame, initial_ready, previous_upload)
        } else if position <= 2 {
            (
                old_frame,
                early_ready,
                if early_adopts {
                    old_frame
                } else {
                    previous_upload
                },
            )
        } else if position < 8 {
            (
                old_frame + 1,
                early_ready,
                if early_adopts {
                    old_frame
                } else {
                    previous_upload
                },
            )
        } else {
            (old_frame + 1, early_ready + 1, old_frame + 1)
        };
        assert_eq!(
            (
                point.frame,
                point.local_ready,
                point.local_pending,
                point.upload_frame
            ),
            (frame, ready, 3 - ready, upload),
            "{}: {checkpoints:#?}",
            point.name
        );
        let adopted = point
            .local_ready
            .checked_sub(last_ready)
            .expect("ready result was unexpectedly evicted");
        if adopted != 0 {
            assert_eq!(adopted, 1);
            let count = adoption_by_counter.entry(point.frame).or_default();
            *count += adopted;
            assert_eq!(
                *count, 1,
                "multiple uploads at one explicit App frame counter"
            );
        }
        last_ready = point.local_ready;
    }
    assert_eq!(
        adoption_by_counter.get(&old_frame).copied().unwrap_or(0),
        usize::from(early_adopts)
    );
    assert_eq!(adoption_by_counter.get(&(old_frame + 1)), Some(&1));
}

#[test]
fn egui037_continuation_real_full_inpaint_resumes_view_trim_with_new_pointer_level() {
    // Opposite old/new levels catch both stale-down and stale-up regressions.
    for old_down in [false, true] {
        let mut env = prepared();
        let ctx = context();
        let path = env.tmp.path().join("trim.png");
        image(&mut env, &ctx, path);
        env.fullscreen_idx = Some(0);
        env.viewer_presentation = ViewerPresentation::MainWindow;
        env.native_video_in_window_active = true;
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        let pointer = |down| egui::Event::PointerButton {
            pos: egui::pos2(-100.0, -100.0),
            button: egui::PointerButton::Primary,
            pressed: down,
            modifiers: egui::Modifiers::NONE,
        };
        // Enter the actual embedded viewer before capturing the operation fingerprint.
        full(&mut backend, &mut env, vec![pointer(old_down)]);
        assert_eq!(env.fullscreen_idx, Some(0));
        env.view_trim_apply_mode = crate::view_trim::ViewTrimApplyMode::Book;
        env.view_trim_book_settings.enabled = true;
        // Request through the real entry so its private state fingerprint is not forged.
        let raw = env.fs_cache.remove(&0).unwrap();
        let (_tx, rx) = mpsc::channel();
        let input_seq = env.input_seq;
        env.fs_pending.insert(
            0,
            FsPendingValue::new(
                Arc::new(AtomicBool::new(false)),
                rx,
                input_seq,
                FsLoadPurpose::for_page(true),
            ),
        );
        env.request_view_trim_original_action(
            &ctx,
            0,
            crate::ui_fullscreen::SpreadPair::Single,
            crate::ui_view_trim::OriginalViewTrimAction::SeedPage,
        );
        let entry = env
            .fs_pending
            .remove(&0)
            .unwrap()
            .purpose
            .original_entry()
            .unwrap();
        env.fs_cache.insert(0, raw);
        let trace = Trace::start();
        erase_ready(
            &mut env,
            0,
            crate::ui_erase::EraseInpaintPurpose::MaterializeForEntry { entry },
        );
        for _ in 0..2 {
            let (_, input) =
                backend.update_logic_only(&mut env.app, minimized_input(vec![pointer(!old_down)]));
            assert_eq!(input.events, vec![pointer(!old_down)]);
            assert_eq!(ctx.input(|input| input.pointer.primary_down()), old_down);
            assert_eq!(env.erase_inpaint_pending.len(), 1);
            assert!(!env.view_trim_save_pending);
        }
        assert!(trace.take().is_empty());
        full(&mut backend, &mut env, vec![pointer(!old_down)]);
        let checkpoints = trace.take();
        let after = checkpoints
            .iter()
            .find(|point| point.name == "media_after")
            .unwrap();
        assert_eq!(after.primary_down, !old_down);
        assert_eq!(after.erase_pending, 0);
        assert_eq!(after.erase_ready, 1);
        assert_eq!(
            after.trim_save_pending, !old_down,
            "new pointer determines whether resumed trim saves immediately"
        );
        assert!(env.view_trim_page_overrides.contains_key(&0));
    }
}

#[derive(Default)]
struct DiscardFirst;
impl egui::Plugin for DiscardFirst {
    fn debug_name(&self) -> &'static str {
        "connection_discard_first"
    }
    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().current_pass_index() == 0 {
            ui.ctx().request_discard("connection replay");
        }
    }
}

#[test]
#[cfg(windows)]
fn egui037_continuation_real_full_discard_and_sibling_keep_result_owner_exactly_once() {
    let mut env = prepared();
    let ctx = context();
    ctx.add_plugin(DiscardFirst);
    let path = env.tmp.path().join("owned.png");
    image(&mut env, &ctx, path);
    let tx = erase_ready(
        &mut env,
        0,
        crate::ui_erase::EraseInpaintPurpose::Commit { origin: "owned" },
    );
    let metadata_tx = metadata_ready(&mut env);
    let owner = env.stash_mounted_and_start_fresh("connection_sibling");
    let sibling = env.projected_viewer_context_id();
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    full(&mut backend, &mut env, Vec::new());
    assert!(env.erase_result_cache.is_empty());
    assert!(!env.metadata_cache.contains_key("connection-metadata"));
    env.with_viewer_context_state(owner, |app| {
        assert_eq!(app.erase_inpaint_pending.len(), 1);
        assert!(app.metadata_pending.is_some());
    })
    .unwrap();
    let trace = Trace::start();
    let texture = env
        .with_viewer_context(owner, |app| {
            full(&mut backend, app, Vec::new());
            assert!(app.erase_inpaint_pending.is_empty());
            assert!(app.metadata_pending.is_none());
            assert!(app.metadata_cache.contains_key("connection-metadata"));
            app.erase_result_cache.values().next().unwrap().texture.id()
        })
        .unwrap();
    let checkpoints = trace.take();
    let passes: Vec<_> = checkpoints
        .iter()
        .filter(|point| point.name == "media_after")
        .collect();
    assert_eq!(passes.len(), 2);
    assert_eq!((passes[0].erase_pending, passes[0].erase_ready), (0, 1));
    assert_eq!((passes[1].erase_pending, passes[1].erase_ready), (0, 1));
    assert!(
        tx.send(crate::ui_erase::EraseInpaintResult::MaskAbsent)
            .is_err()
    );
    assert!(
        metadata_tx
            .send(MetadataLoadResult {
                key: "duplicate".into(),
                metadata: None,
                exif: None,
                xmp: None,
                panorama: None,
                sidecar: None
            })
            .is_err()
    );
    assert_eq!(env.projected_viewer_context_id(), sibling);
    full(&mut backend, &mut env, Vec::new());
    assert!(env.erase_result_cache.is_empty());
    env.with_viewer_context_state(owner, |app| {
        assert_eq!(
            app.erase_result_cache.values().next().unwrap().texture.id(),
            texture
        );
        assert!(!app.metadata_cache.contains_key("duplicate"));
    })
    .unwrap();
}

fn rescan_fixture(
    env: &mut AppTestEnvForTest,
    different: bool,
    restart: bool,
) -> (
    PathBuf,
    mpsc::Sender<Option<ExternalRescanResult>>,
    folder_scan::FolderScanSignature,
) {
    use book_append_refresh::FolderRefreshCause;
    let folder = env.tmp.path().join("observed");
    std::fs::create_dir_all(&folder).unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255]))
        .save(folder.join("a.png"))
        .unwrap();
    // Seed the already installed baseline without starting an unrelated folder hydration /
    // metadata refresh that can legitimately replace all edit workers at F13.
    env.current_folder = Some(folder.clone());
    env.items = vec![GridItem::Image(folder.join("a.png"))];
    env.thumbnails = vec![ThumbnailState::Pending];
    env.image_metas = vec![None];
    env.visible_indices = vec![0];
    env.selected = Some(0);
    env.prepare_current_listing_page_identity_index(Default::default(), None);
    env.finish_page_identity_preparation_for_test();
    env.seed_folder_observation_for_test(&folder, &folder_scan::scan_directory(&folder));
    let projection_key = env.current_folder_projection_key(&folder);
    let mut watch = CurrentFolderWatch::new(folder.clone(), projection_key);
    // The event/subscription is fake; no OS watcher registration or desktop activity is needed.
    watch.registration = None;
    watch.registration_sender = None;
    watch.dirty.push_back((7, None));
    watch.debounce_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(3600));
    let epoch = watch.epoch;
    env.current_folder_watch = Some(watch);
    let (tx, rx) = mpsc::channel();
    if different {
        image::RgbaImage::from_pixel(2, 2, image::Rgba([60, 40, 20, 255]))
            .save(folder.join("b.png"))
            .unwrap();
    }
    let scan = folder_scan::scan_directory(&folder);
    let signature = signature_from_scan(&scan);
    let mtime = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(42);
    let result = if different {
        ExternalRescanResult::Different(scan, signature, FolderRefreshCause::External)
    } else {
        ExternalRescanResult::Equal(
            signature,
            env.folder_observation.as_ref().unwrap().rows.clone(),
            FolderRefreshCause::External,
            mtime,
        )
    };
    tx.send(Some(result)).unwrap();
    let items_generation = env.items_generation;
    let owner_context_id = env.edit_request_owner_context();
    let scan_options = env.current_external_scan_options();
    env.external_rescan_pending = Some(ExternalRescanPending {
        proof: Some(FolderRescanProof {
            baseline: None,
            projection_key,
            subscription: Some((epoch, 7)),
        }),
        cause: FolderRefreshCause::Operation,
        owner_context_id,
        folder: folder.clone(),
        mtime,
        items_generation,
        scan_options,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        restart_requested: restart,
    });
    env.current_folder_last_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
    env.current_folder_signature = Some(folder_scan::FolderScanSignature::for_test(0));
    (folder, tx, signature)
}

#[test]
fn egui037_continuation_real_full_equal_rescan_acknowledges_cutoff_and_retains_restart_cause() {
    let mut env = prepared();
    let (_folder, tx, signature) = rescan_fixture(&mut env, false, true);
    let ctx = context();
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    let generation = env.items_generation;
    let initial = env.current_folder_signature;
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        let pending = env.external_rescan_pending.as_ref().unwrap();
        assert!(pending.restart_requested);
        assert!(matches!(
            pending.cause,
            book_append_refresh::FolderRefreshCause::Operation
        ));
        assert_eq!(env.current_folder_signature, initial);
        assert_eq!(
            env.current_folder_last_mtime,
            Some(std::time::SystemTime::UNIX_EPOCH)
        );
        assert_eq!(env.current_folder_watch.as_ref().unwrap().acknowledged, 0);
        assert_eq!(env.current_folder_watch.as_ref().unwrap().dirty.len(), 1);
    }
    let trace = Trace::start();
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(env.current_folder_signature, Some(signature));
    assert_eq!(
        env.current_folder_last_mtime,
        Some(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(42))
    );
    assert_eq!(env.items_generation, generation);
    let watch = env.current_folder_watch.as_ref().unwrap();
    assert_eq!(watch.acknowledged, 7);
    assert!(watch.dirty.is_empty());
    let pending = env
        .external_rescan_pending
        .as_ref()
        .expect("retained restart starts another existing scan");
    assert!(!pending.restart_requested);
    assert!(matches!(
        pending.cause,
        book_append_refresh::FolderRefreshCause::Operation
    ));
    assert!(
        tx.send(None).is_err(),
        "old receiver is consumed, rather than copied to a second owner"
    );
    let checkpoints = trace.take();
    assert!(
        checkpoints
            .iter()
            .find(|point| point.name == "rescan_after")
            .unwrap()
            .rescan_pending
    );
}

#[test]
fn egui037_continuation_real_full_different_rescan_installs_and_allocates_hydration_only_in_ui() {
    let mut env = prepared();
    let (folder, tx, signature) = rescan_fixture(&mut env, true, false);
    let ctx = context();
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    let generation = env.items_generation;
    let request_id = env.sidecar_restore_request_seq;
    let trace = Trace::start();
    for _ in 0..2 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert_eq!(env.items.len(), 1);
        assert_eq!(env.items_generation, generation);
        assert_eq!(env.sidecar_restore_request_seq, request_id);
        assert_eq!(env.current_folder_watch.as_ref().unwrap().acknowledged, 0);
    }
    full(&mut backend, &mut env, Vec::new());
    let checkpoints = trace.take();
    let ids = checkpoints
        .iter()
        .find(|point| point.name == "rescan_after")
        .unwrap()
        .hydration_ids
        .expect("accepted Different result creates the existing hydration owner before input");
    assert_eq!(ids.0, request_id + 1);
    assert!(ids.1.is_none_or(|book_id| book_id == ids.0));
    assert!(tx.send(None).is_err());
    assert!(
        env.items
            .iter()
            .any(|item| matches!(item, GridItem::Image(path) if path == &folder.join("b.png")))
    );
    assert!(env.items_generation > generation);
    assert_eq!(env.current_folder_signature, Some(signature));
    assert_eq!(env.sidecar_restore_request_seq, request_id + 1);
    let adopted_generation = env.items_generation;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while env.sidecar_restore_active() {
        assert!(
            std::time::Instant::now() < deadline,
            "Full hydration did not settle"
        );
        full(&mut backend, &mut env, Vec::new());
        std::thread::yield_now();
    }
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(env.items_generation, adopted_generation);
    assert_eq!(env.sidecar_restore_request_seq, request_id + 1);
}

#[test]
#[cfg(windows)]
fn egui037_continuation_real_full_play_test_launches_existing_target_only_once() {
    let mut env = prepared();
    let ctx = context();
    let path = env.tmp.path().join("play-test.mp4");
    env.current_folder = Some(env.tmp.path().to_path_buf());
    env.settings.video_in_window_mode = true;
    env.settings.detached_viewer_enabled = false;
    env.items.push(GridItem::Video(path.clone()));
    env.thumbnails.push(ThumbnailState::Pending);
    env.image_metas.push(None);
    env.visible_indices.push(0);
    // This existing fixture contains disconnected channels, no decoder/audio output/window.
    let player = crate::video::VideoPlayer::stream_ready_disconnected_for_test(path.clone());
    let epoch = player.native_source_epoch();
    env.fs_cache.insert(
        0,
        FsCacheEntry::Video {
            player: Box::new(player),
            load_seq: 0,
        },
    );
    env.play_test = Some(PlayTestState::new(PlayTestConfig {
        path,
        duration: std::time::Duration::from_secs(86_400),
        mute: true,
        start_secs: None,
        skip_vst3: true,
    }));
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert_eq!(env.play_test.as_ref().unwrap().launched_idx, None);
        assert_eq!(env.fullscreen_idx, None);
    }
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(env.play_test.as_ref().unwrap().launched_idx, Some(0));
    assert_eq!(env.fullscreen_idx, Some(0));
    assert_eq!(env.items.len(), 1);
    let generation = env.items_generation;
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(env.play_test.as_ref().unwrap().launched_idx, Some(0));
    assert_eq!(env.items_generation, generation);
    let Some(FsCacheEntry::Video { player, .. }) = env.fs_cache.get(&0) else {
        panic!("the existing fake player must remain the owner");
    };
    assert_eq!(player.native_source_epoch(), epoch);
}

#[test]
#[cfg(windows)]
fn egui037_continuation_real_full_samples_decision_preview_and_physical_input_only_in_ui() {
    let mut env = prepared();
    let ctx = context();
    let path = env.tmp.path().join("display.png");
    image(&mut env, &ctx, path);
    env.fullscreen_idx = Some(0);
    env.viewer_presentation = ViewerPresentation::MainWindow;
    env.native_video_in_window_active = true;
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
    assert_eq!(
        env.egui037_connection_frame_samples(&ctx),
        (None, None, None)
    );
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert_eq!(
            env.egui037_connection_frame_samples(&ctx),
            (None, None, None)
        );
    }
    let frame = ctx.cumulative_frame_nr();
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(
        env.egui037_connection_frame_samples(&ctx),
        (Some(frame), Some(frame), Some(frame))
    );
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert_eq!(
            env.egui037_connection_frame_samples(&ctx),
            (Some(frame), Some(frame), Some(frame))
        );
    }
    let next = ctx.cumulative_frame_nr();
    full(&mut backend, &mut env, Vec::new());
    assert!(next > frame);
    assert_eq!(
        env.egui037_connection_frame_samples(&ctx),
        (Some(next), Some(next), Some(next))
    );
}

#[test]
#[cfg(windows)]
fn egui037_continuation_real_full_pdf_promotion_retry_uses_ui_frame_budget() {
    let mut env = prepared();
    let ctx = context();
    let path = env.tmp.path().join("no-render-job.pdf");
    let idx = image(&mut env, &ctx, path.clone());
    env.items[idx] = GridItem::PdfPage {
        pdf_path: path,
        page_num: 0,
        content_type: None,
    };
    env.fullscreen_idx = Some(idx);
    env.viewer_presentation = ViewerPresentation::MainWindow;
    env.native_video_in_window_active = true;
    let (_tx, rx) = mpsc::channel();
    let input_seq = env.input_seq;
    env.fs_pending.insert(
        idx,
        FsPendingValue::new(
            Arc::new(AtomicBool::new(false)),
            rx,
            input_seq,
            FsLoadPurpose::for_page(true),
        ),
    );
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    full(&mut backend, &mut env, Vec::new());
    let last = match env.fullscreen_pdf_promotion {
        FullscreenPdfPromotionState::Retry {
            last_attempt_frame, ..
        } => last_attempt_frame,
        ref other => panic!("pending PDF with no real pool job must remain retryable: {other:?}"),
    };
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert!(
            matches!(env.fullscreen_pdf_promotion, FullscreenPdfPromotionState::Retry { last_attempt_frame, .. } if last_attempt_frame == last)
        );
    }
    full(&mut backend, &mut env, Vec::new());
    assert!(
        matches!(env.fullscreen_pdf_promotion, FullscreenPdfPromotionState::Retry { last_attempt_frame, .. } if last_attempt_frame > last)
    );
}

#[test]
#[cfg(windows)]
fn egui037_continuation_real_full_summary_budget_waits_for_next_ui_frame() {
    let mut env = prepared();
    let ctx = context();
    for number in 0..6 {
        let path = env.tmp.path().join(format!("summary-{number}.png"));
        image(&mut env, &ctx, path);
    }
    env.fullscreen_idx = Some(5);
    env.viewer_presentation = ViewerPresentation::MainWindow;
    env.native_video_in_window_active = true;
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    full(&mut backend, &mut env, Vec::new());
    for idx in 0..6 {
        let key = env.current_edit_result_key(idx);
        env.edit_result_cache.insert(
            key,
            EditResultEntry {
                pixels: Arc::new(egui::ColorImage::filled([8, 8], egui::Color32::GRAY)),
                texture: None,
            },
        );
    }
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert!(env.colorize_mono_summary_cache.is_empty());
    }
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(
        env.colorize_mono_summary_cache.len(),
        COLORIZE_MONO_SUMMARY_BUDGET_PER_FRAME
    );
    assert!(env.colorize_mono_summary_cache.contains_key(&5));
    let frame = env.colorize_mono_summary_last_reconcile_frame;
    for _ in 0..3 {
        let _ = backend.update_logic_only(&mut env.app, minimized_input(Vec::new()));
        assert_eq!(
            env.colorize_mono_summary_cache.len(),
            COLORIZE_MONO_SUMMARY_BUDGET_PER_FRAME
        );
        assert_eq!(env.colorize_mono_summary_last_reconcile_frame, frame);
    }
    full(&mut backend, &mut env, Vec::new());
    assert_eq!(env.colorize_mono_summary_cache.len(), 6);
    assert!(env.colorize_mono_summary_last_reconcile_frame > frame);
}
