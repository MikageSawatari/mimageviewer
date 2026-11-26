use super::*;
use std::cell::Cell;

fn tree_hit(path: PathBuf) -> Hit {
    Hit::Folder {
        path,
        place: DropPlace::Tree,
    }
}

fn internal(paths: &[&str]) -> InternalDrag {
    InternalDrag {
        paths: paths.iter().map(PathBuf::from).collect(),
        excluded_virtual: false,
        failed_paths: 0,
    }
}

fn tree(destination: &str, source: Option<&InternalDrag>, keys: u32) -> Decision {
    decide(
        Some(tree_hit(destination.into())),
        true,
        source,
        true,
        true,
        keys,
    )
}

fn operation(decision: Decision) -> Option<(ShellTransferOperation, bool)> {
    match decision {
        Decision::Folder {
            operation,
            default_move_needs_volume_check,
            ..
        } => Some((operation, default_move_needs_volume_check)),
        _ => None,
    }
}

#[test]
fn main_drop_operation_modifiers_roots_unc_case_and_trailing_separators() {
    use ShellTransferOperation::{Copy, Move};
    for (sources, destination, default) in [
        (vec![r"C:\src\a", r"c:/src/b"], r"c:\dst\", (Move, true)),
        (vec![r"C:\src\a", r"D:\src\b"], r"C:\dst", (Copy, false)),
        (
            vec![r"\\SERVER\Share\src\a"],
            r"\\server\share\dst\",
            (Move, true),
        ),
        (
            vec![r"\\server\share\src\a"],
            r"\\server\share2\dst",
            (Copy, false),
        ),
        (
            vec![r"\\?\UNC\SERVER\Share\src\a"],
            r"\\server\share\dst",
            (Move, true),
        ),
        (
            vec![r"\\?\UNC\server\share\src\a"],
            r"\\?\unc\server\share2\dst",
            (Copy, false),
        ),
        (vec![r"\\?\C:\src\a"], r"c:\dst", (Move, true)),
    ] {
        let source = internal(&sources);
        for (keys, expected) in [
            (0, Some(default)),
            (CONTROL, Some((Copy, false))),
            (SHIFT, Some((Move, false))),
            (SHIFT | CONTROL, None),
            (ALT, None),
            (ALT | SHIFT, None),
        ] {
            assert_eq!(
                operation(tree(destination, Some(&source), keys)),
                expected,
                "{destination}, keys={keys}"
            );
        }
    }
}

#[test]
fn main_drop_internal_rejects_whole_mixed_payload_and_lexical_invalid_targets() {
    let mut source = internal(&[r"C:\src\a", r"C:\src\folder"]);
    for destination in [
        r"C:\src\folder",
        r"C:\src\FOLDER\child",
        r"C:\src\folder\child\..",
    ] {
        for keys in [0, CONTROL, SHIFT] {
            assert!(!tree(destination, Some(&source), keys).accepted());
        }
    }
    for keys in [0, SHIFT] {
        assert!(!tree(r"C:\src\", Some(&source), keys).accepted());
    }
    assert!(tree(r"C:\src", Some(&source), CONTROL).accepted());
    assert!(tree(r"C:\src\folder-old", Some(&source), 0).accepted());
    source.excluded_virtual = true;
    assert!(!tree(r"C:\dst", Some(&source), CONTROL).accepted());
    source.excluded_virtual = false;
    source.failed_paths = 1;
    assert!(!tree(r"C:\dst", Some(&source), CONTROL).accepted());
    assert!(!tree(r"C:\dst", Some(&internal(&[])), 0).accepted());
}

#[test]
fn main_drop_external_is_copy_for_every_modifier_and_only_if_source_allows_it() {
    for keys in [0, SHIFT, CONTROL, SHIFT | CONTROL, ALT] {
        assert_eq!(
            operation(tree(r"C:\dst", None, keys)),
            Some((ShellTransferOperation::Copy, false))
        );
        for (has_paths, allows_copy) in [(false, true), (true, false), (false, false)] {
            assert!(
                !decide(
                    Some(tree_hit(r"C:\dst".into())),
                    true,
                    None,
                    has_paths,
                    allows_copy,
                    keys
                )
                .accepted()
            );
        }
    }
}

#[test]
fn main_drop_blockers_and_tree_blank_never_fall_through_to_grid() {
    let source = internal(&[r"C:\src\a"]);
    for internal in [None, Some(&source)] {
        assert!(
            !decide(
                Some(tree_hit(r"C:\dst".into())),
                false,
                internal,
                true,
                true,
                0
            )
            .accepted()
        );
        for hit in [None, Some(Hit::TreeNonTarget)] {
            assert!(!decide(hit, true, internal, true, true, 0).accepted());
        }
    }
    assert!(matches!(
        decide(Some(Hit::ExistingDropArea), false, None, true, true, SHIFT),
        Decision::Grid
    ));
    assert!(
        !decide(
            Some(Hit::ExistingDropArea),
            true,
            Some(&source),
            true,
            true,
            0
        )
        .accepted()
    );
}

#[test]
fn main_drop_nodrop_rejected_hits_use_none_and_accepted_external_grid_uses_invalid() {
    let source = internal(&[r"C:\src\a"]);
    for hit in [None, Some(Hit::TreeNonTarget), Some(Hit::ExistingDropArea)] {
        for (internal, has_paths, allows_copy) in [
            (Some(&source), true, true),
            (None, false, true),
            (None, true, false),
        ] {
            let decision = decide(hit.clone(), true, internal, has_paths, allows_copy, 0);
            assert!(!decision.accepted(), "{hit:?}");
            assert_eq!(decision.description(), Description::None, "{hit:?}");
        }
    }
    let no_paths = decide(Some(tree_hit(r"C:\dst".into())), true, None, false, true, 0);
    assert!(!no_paths.accepted());
    assert_eq!(no_paths.description(), Description::None);
    let accepted = decide(Some(Hit::ExistingDropArea), true, None, true, true, 0);
    assert!(matches!(accepted, Decision::Grid));
    assert_eq!(accepted.description(), Description::Invalid);
}

#[test]
fn main_drop_nodrop_internal_row_to_grid_replaces_transfer_without_reading_paths() {
    let data = Rc::new(FakeData::default());
    let session = DragSession::new(data.clone(), true, true);
    let source = internal(&[r"C:\src\a"]);
    session.describe(tree(r"C:\dst", Some(&source), 0).description());
    let rejected = decide(
        Some(Hit::ExistingDropArea),
        true,
        Some(&source),
        false,
        true,
        0,
    );
    session.describe(rejected.description());
    session.describe(rejected.description());
    assert_eq!(
        data.descriptions.borrow().as_slice(),
        &[
            Description::Transfer(ShellTransferOperation::Move, r"C:\dst".into()),
            Description::None,
        ]
    );
    assert!(
        session
            .drop_request(&rejected, Some(&source))
            .unwrap()
            .is_none()
    );
    assert_eq!(data.reads.get(), 0);
    assert_eq!(data.queries.get(), 0);
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
}

fn geometry(path: &str, ppp: f32) -> DropGeometry {
    DropGeometry {
        pane_rect: rect(0.0, 0.0, 200.0, 400.0),
        tree_clip: rect(0.0, 30.0, 185.0, 390.0),
        rows: vec![DropRow {
            rect: rect(20.0, 40.0, 185.0, 60.0),
            path: path.into(),
        }],
        grid_clip: egui::Rect::NOTHING,
        grid_folders: Vec::new(),
        grid_blocked: Vec::new(),
        client_size_px: [800, 600],
        dpi: 96,
        pixels_per_point: ppp,
    }
}

#[test]
fn grid_drop_858_hit_edges_clip_blocked_and_tree_priority() {
    for ppp in [1.0, 1.5] {
        let mut geometry = geometry(r"C:\tree", ppp);
        geometry.grid_clip = rect(0.0, 30.0, 780.0, 590.0);
        geometry.grid_folders = vec![
            DropRow {
                rect: rect(190.0, 40.0, 300.0, 90.0),
                path: r"C:\grid".into(),
            },
            // Deliberately extend beyond the clip to prove hit_test checks it too.
            DropRow {
                rect: rect(750.0, 560.0, 800.0, 620.0),
                path: r"C:\edge".into(),
            },
            DropRow {
                rect: rect(20.0, 40.0, 185.0, 60.0),
                path: r"C:\behind-tree".into(),
            },
        ];
        geometry.grid_blocked = vec![rect(320.0, 40.0, 420.0, 90.0)];
        let geometries = VecDeque::from([(1, geometry)]);
        for (point, expected) in [
            (
                [201, 40],
                Hit::Folder {
                    path: r"C:\grid".into(),
                    place: DropPlace::Grid,
                },
            ),
            (
                [300, 90],
                Hit::Folder {
                    path: r"C:\grid".into(),
                    place: DropPlace::Grid,
                },
            ),
            (
                [779, 589],
                Hit::Folder {
                    path: r"C:\edge".into(),
                    place: DropPlace::Grid,
                },
            ),
            ([781, 589], Hit::ExistingDropArea),
            ([779, 591], Hit::ExistingDropArea),
            ([320, 40], Hit::GridNonTarget),
            ([420, 90], Hit::GridNonTarget),
            ([100, 50], tree_hit(r"C:\tree".into())),
            ([195, 50], Hit::TreeNonTarget),
            // Nonfolder and stack cells are deliberately absent, so remain blank.
            ([500, 50], Hit::ExistingDropArea),
            ([600, 100], Hit::ExistingDropArea),
        ] {
            let point = [
                (point[0] as f32 * ppp) as i32,
                (point[1] as f32 * ppp) as i32,
            ];
            assert_eq!(
                hit_test(&geometries, Some(1), metrics(), point),
                Some(expected)
            );
        }
    }
}

#[test]
fn grid_drop_858_uses_only_presented_grid_with_matching_metrics() {
    let make = |path: &str| {
        let mut geometry = geometry("tree", 1.0);
        geometry.grid_clip = rect(200.0, 30.0, 780.0, 590.0);
        geometry.grid_folders = vec![DropRow {
            rect: rect(250.0, 40.0, 350.0, 90.0),
            path: path.into(),
        }];
        geometry
    };
    let geometries = VecDeque::from([(1, make("shown")), (2, make("unshown"))]);
    for (pass, path) in [(1, "shown"), (2, "unshown")] {
        assert_eq!(
            hit_test(&geometries, Some(pass), metrics(), [300, 50]),
            Some(Hit::Folder {
                path: path.into(),
                place: DropPlace::Grid
            })
        );
    }
    for pass in [None, Some(0), Some(3)] {
        assert_eq!(hit_test(&geometries, pass, metrics(), [300, 50]), None);
    }
    for metrics in [
        WindowMetrics {
            dpi: 144,
            ..metrics()
        },
        WindowMetrics {
            client_size_px: [900, 600],
            ..metrics()
        },
    ] {
        assert_eq!(hit_test(&geometries, Some(1), metrics, [300, 50]), None);
    }
    for ppp in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let mut invalid = make("invalid");
        invalid.pixels_per_point = ppp;
        assert_eq!(
            hit_test(
                &VecDeque::from([(1, invalid)]),
                Some(1),
                metrics(),
                [300, 50]
            ),
            None
        );
    }
    let empty = VecDeque::from([(1, geometry("tree", 1.0))]);
    assert_eq!(
        hit_test(&empty, Some(1), metrics(), [300, 50]),
        Some(Hit::ExistingDropArea)
    );
}

#[test]
fn grid_drop_858_folder_decisions_match_tree_for_internal_and_external_sources() {
    let clean = internal(&[r"C:\src\folder", r"C:\src\image.jpg"]);
    let cross_drive = internal(&[r"D:\src\folder"]);
    let mixed_virtual = InternalDrag {
        excluded_virtual: true,
        ..clean.clone()
    };
    let partial = InternalDrag {
        failed_paths: 1,
        ..clean.clone()
    };
    let empty = internal(&[]);
    for source in [
        None,
        Some(&clean),
        Some(&cross_drive),
        Some(&mixed_virtual),
        Some(&partial),
        Some(&empty),
    ] {
        for destination in [
            r"C:\dst",
            r"C:\src",
            r"C:\src\folder",
            r"C:\src\folder\child",
        ] {
            for keys in [0, CONTROL, SHIFT, CONTROL | SHIFT, ALT] {
                for admission in [false, true] {
                    for allows_copy in [false, true] {
                        let evaluate = |place| {
                            decide(
                                Some(Hit::Folder {
                                    path: destination.into(),
                                    place,
                                }),
                                admission,
                                source,
                                true,
                                allows_copy,
                                keys,
                            )
                        };
                        let tree = evaluate(DropPlace::Tree);
                        let grid = evaluate(DropPlace::Grid);
                        assert_eq!(grid.accepted(), tree.accepted());
                        assert_eq!(grid.description(), tree.description());
                        assert_eq!(operation(grid), operation(tree));
                    }
                }
            }
        }
    }
    for source in [None, Some(&clean)] {
        let decision = decide(Some(Hit::GridNonTarget), true, source, true, true, 0);
        assert!(!decision.accepted());
        assert_eq!(decision.description(), Description::None);
    }
}

#[test]
fn grid_drop_858_folder_request_retains_payload_and_external_copy() {
    let data = Rc::new(FakeData::default());
    let source = internal(&[r"C:\src\a", r"C:\src\folder"]);
    let session = DragSession::new(data.clone(), true, true);
    let decision = decide(
        Some(Hit::Folder {
            path: r"C:\dst".into(),
            place: DropPlace::Grid,
        }),
        true,
        Some(&source),
        false,
        true,
        0,
    );
    let Some(DropRequest::FolderTransfer(request)) =
        session.drop_request(&decision, Some(&source)).unwrap()
    else {
        panic!("folder transfer expected")
    };
    assert_eq!(request.sources, source.paths);
    assert_eq!(request.operation, ShellTransferOperation::Move);
    assert!(request.default_move_needs_volume_check);
    assert_eq!(data.reads.get(), 0);
    assert_eq!(data.queries.get(), 0);

    let session = DragSession::new(data.clone(), true, false);
    for keys in [0, CONTROL, SHIFT, CONTROL | SHIFT, ALT] {
        let decision = decide(
            Some(Hit::Folder {
                path: r"C:\dst".into(),
                place: DropPlace::Grid,
            }),
            true,
            None,
            true,
            true,
            keys,
        );
        let Some(DropRequest::FolderTransfer(request)) =
            session.drop_request(&decision, None).unwrap()
        else {
            panic!("external folder transfer expected")
        };
        assert_eq!(request.operation, ShellTransferOperation::Copy);
        assert!(!request.default_move_needs_volume_check);
    }
}

fn metrics() -> WindowMetrics {
    WindowMetrics {
        client_size_px: [800, 600],
        dpi: 96,
    }
}

#[test]
fn main_drop_hit_test_full_row_edges_clip_indent_blank_header_scrollbar_and_overlay() {
    for ppp in [1.0, 1.5] {
        let geometries = VecDeque::from([(1, geometry(r"C:\row", ppp))]);
        for point in [[20, 40], [184, 59], [100, 50]] {
            let point = [
                (point[0] as f32 * ppp) as i32,
                (point[1] as f32 * ppp) as i32,
            ];
            assert_eq!(
                hit_test(&geometries, Some(1), metrics(), point),
                Some(tree_hit(r"C:\row".into()))
            );
        }
        for point in [[10, 50], [100, 100], [100, 10], [190, 50]] {
            let point = [
                (point[0] as f32 * ppp) as i32,
                (point[1] as f32 * ppp) as i32,
            ];
            assert_eq!(
                hit_test(&geometries, Some(1), metrics(), point),
                Some(Hit::TreeNonTarget)
            );
        }
        assert_eq!(
            hit_test(&geometries, Some(1), metrics(), [400, 100]),
            Some(Hit::ExistingDropArea)
        );
        let mut occluded = geometries.clone();
        occluded[0].1.rows.clear(); // App excludes the entire row intersecting a front layer.
        assert_eq!(
            hit_test(
                &occluded,
                Some(1),
                metrics(),
                [(100.0 * ppp) as i32, (50.0 * ppp) as i32]
            ),
            Some(Hit::TreeNonTarget)
        );
    }
}

#[test]
fn main_drop_hit_test_requires_exact_presented_geometry_and_current_size_dpi() {
    let geometries = VecDeque::from([
        (10, geometry("shown A", 1.0)),
        (11, geometry("unshown B", 1.0)),
    ]);
    // After a skipped present, even the next frame before painting must still use A.
    assert_eq!(
        hit_test(&geometries, Some(10), metrics(), [100, 50]),
        Some(tree_hit("shown A".into()))
    );
    // A discarded first pass is never eligible once the second pass is presented.
    assert_eq!(
        hit_test(&geometries, Some(11), metrics(), [100, 50]),
        Some(tree_hit("unshown B".into()))
    );
    for presented in [None, Some(9), Some(12)] {
        assert_eq!(hit_test(&geometries, presented, metrics(), [100, 50]), None);
    }
    for mismatch in [
        WindowMetrics {
            dpi: 144,
            ..metrics()
        },
        WindowMetrics {
            client_size_px: [801, 600],
            ..metrics()
        },
    ] {
        assert_eq!(hit_test(&geometries, Some(10), mismatch, [100, 50]), None);
        assert_eq!(hit_test(&geometries, Some(10), mismatch, [400, 100]), None);
    }
    let mut no_tree = geometry("unused", 1.0);
    no_tree.pane_rect = egui::Rect::NOTHING;
    no_tree.rows.clear();
    assert_eq!(
        hit_test(
            &VecDeque::from([(1, no_tree)]),
            Some(1),
            metrics(),
            [100, 50]
        ),
        Some(Hit::ExistingDropArea)
    );
}

#[test]
fn main_drop_geometry_ring_preserves_shown_pass_across_skipped_and_discarded_passes() {
    let shared = Rc::new(RefCell::new(MainDropShared::default()));
    let ctx = egui::Context::default();
    let shown_key = egui::Id::new(("egui_wgpu::last_presented_pass", egui::ViewportId::ROOT));
    let first = ctx.run_ui(Default::default(), |ui| {
        publish_geometry(&shared, ui.ctx(), geometry("shown", 1.0))
    });
    first.drop_without_applying_deltas();
    let shown = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| data.insert_temp(shown_key, shown)); // Fake presenter, no product/GPU.
    for _ in 0..10 {
        let output = ctx.run_ui(Default::default(), |ui| {
            publish_geometry(&shared, ui.ctx(), geometry("not presented", 1.0))
        });
        output.drop_without_applying_deltas();
    }
    assert!(shared.borrow().geometries.len() <= 4);
    assert_eq!(
        hit_test(
            &shared.borrow().geometries,
            Some(shown),
            metrics(),
            [100, 50]
        ),
        Some(tree_hit("shown".into()))
    );
    let mut passes = 0;
    let output = ctx.run_ui(Default::default(), |ui| {
        passes += 1;
        if passes == 1 {
            ui.ctx().request_discard("drop geometry regression");
        }
        publish_geometry(
            &shared,
            ui.ctx(),
            geometry(
                if passes == 1 {
                    "discarded"
                } else {
                    "presented"
                },
                1.0,
            ),
        );
    });
    output.drop_without_applying_deltas();
    assert_eq!(passes, 2);
    let presented = ctx.cumulative_pass_nr();
    assert_eq!(
        hit_test(
            &shared.borrow().geometries,
            Some(presented),
            metrics(),
            [100, 50]
        ),
        Some(tree_hit("presented".into()))
    );
}

#[derive(Default)]
struct FakeData {
    reads: Cell<usize>,
    queries: Cell<usize>,
    descriptions: RefCell<Vec<Description>>,
    reenter: Option<SharedMainDrop>,
}

impl DragData for FakeData {
    fn has_paths(&self) -> bool {
        self.queries.set(self.queries.get() + 1);
        true
    }
    fn paths(&self) -> Result<Vec<PathBuf>, String> {
        self.reads.set(self.reads.get() + 1);
        Ok(vec![r"C:\external\only".into()])
    }
    fn set_description(&self, description: &Description) -> Result<(), String> {
        if let Some(shared) = &self.reenter {
            let _borrow = shared.borrow_mut();
        }
        self.descriptions.borrow_mut().push(description.clone());
        Ok(())
    }
}

#[test]
fn main_drop_internal_request_uses_all_original_paths_without_querying_data_object() {
    let data = Rc::new(FakeData::default());
    let session = DragSession::new(data.clone(), true, true);
    let source = internal(&[r"C:\src\a", r"C:\src\b"]);
    let request = session
        .drop_request(&tree(r"C:\dst", Some(&source), 0), Some(&source))
        .unwrap()
        .unwrap();
    let DropRequest::FolderTransfer(request) = request else {
        panic!("tree request expected")
    };
    assert_eq!(request.sources, source.paths);
    assert_eq!(request.operation, ShellTransferOperation::Move);
    assert!(request.default_move_needs_volume_check);
    assert_eq!(data.reads.get(), 0);
    assert_eq!(data.queries.get(), 0);
}

// §1.898: 本物の Shell を実パスに呼ばせない。実ドライブのルートはドロップ判定だけに渡す。
// 実行経路は一時フォルダ内の偽パスと差し替えで確かめる。
#[cfg(windows)]
#[test]
fn drive_root_898_drop_root_decision_and_temporary_transfer_execution() {
    // 旧コードは別ドライブへの Shift 移動を FolderTransfer にし、拒否通知／非受理の検査が落ちる。
    let data = Rc::new(FakeData::default());
    let session = DragSession::new(data.clone(), true, true);
    let source = internal(&[r"C:\"]);
    // D: is outside C:, so the pre-existing ancestor guard cannot cause this refusal.
    let decision = tree(r"D:\destination", Some(&source), SHIFT);
    let Some(DropRequest::Failed(message)) =
        session.drop_request(&decision, Some(&source)).unwrap()
    else {
        panic!("root must stop at drop admission")
    };
    assert!(!decision.accepted());
    let mut app = crate::app::setup_app_for_test();
    app.main_hwnd = Some(1);
    app.indexer_init.mark_unavailable_if_not_ready();
    let ctx = egui::Context::default();
    // Only a reconstructed Failed is consumed. A root transfer request is never executed.
    app.main_drop
        .borrow_mut()
        .drops
        .push(DropRequest::Failed(message));
    app.consume_main_drops(&ctx);
    assert_eq!(
        app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
        Some(
            crate::grid_item::FileOperationRefusal::VolumeRoot
                .message("移動")
                .as_str()
        )
    );
    let copy = tree(r"D:\destination", Some(&source), CONTROL);
    assert!(copy.accepted());
    let Some(DropRequest::FolderTransfer(request)) =
        session.drop_request(&copy, Some(&source)).unwrap()
    else {
        panic!("copy must remain available")
    };
    assert_eq!(request.operation, ShellTransferOperation::Copy);
    assert_eq!(request.sources, source.paths);
    // Root Copy is inspected above, never submitted. Only temporary paths execute below.
    let scope = app.tmp.path().to_owned();
    let destination = scope.join("destination");
    let mut calls = 0;
    for path in [
        scope.join("foo"),
        scope.join(r"foo\..\bar"),
        scope.join("foo.jpg"),
    ] {
        let source = InternalDrag {
            paths: vec![path.clone()],
            excluded_virtual: false,
            failed_paths: 0,
        };
        for keys in [SHIFT, CONTROL] {
            let decision = tree(destination.to_str().unwrap(), Some(&source), keys);
            assert!(decision.accepted(), "{path:?}");
            let request = session
                .drop_request(&decision, Some(&source))
                .unwrap()
                .unwrap();
            let before = calls;
            drive_root_898_submit_drop_with_fake_transfer(
                &mut app, &ctx, &scope, request, &mut calls,
            );
            assert_eq!(calls, before + 1);
        }
    }
    assert_eq!(data.reads.get(), 0);
}

#[cfg(windows)]
fn drive_root_898_submit_drop_with_fake_transfer(
    app: &mut crate::app::App,
    ctx: &egui::Context,
    scope: &Path,
    request: DropRequest,
    calls: &mut usize,
) {
    match request {
        DropRequest::Failed(message) => {
            // Only a reconstructed Failed enters the real consumer, never a transfer request.
            app.main_drop
                .borrow_mut()
                .drops
                .push(DropRequest::Failed(message));
            app.consume_main_drops(ctx);
        }
        DropRequest::FolderTransfer(request) => {
            assert!(request.sources.iter().all(|path| path.starts_with(scope)));
            assert!(request.destination.starts_with(scope));
            app.file_organize_request =
                crate::ui_dialogs::file_organize::FileOrganizeRequest::Hidden;
            app.submit_shell_transfer_with(
                ctx,
                request,
                |_| scope.join("fake-books"),
                |_, _, _| {
                    *calls += 1;
                    std::sync::mpsc::channel().1
                },
            );
        }
        DropRequest::GridFiles(_) => panic!("unexpected grid file open"),
    }
}

#[test]
fn main_drop_external_requests_copy_or_grid_and_only_description_writes_change() {
    let shared = Rc::new(RefCell::new(MainDropShared::default()));
    let data = Rc::new(FakeData {
        reenter: Some(shared),
        ..Default::default()
    });
    let session = DragSession::new(data.clone(), true, false);
    let decision = tree(r"C:\dst", None, SHIFT);
    for _ in 0..3 {
        session.describe(decision.description());
    }
    session.describe(Description::Invalid); // Accepted list / DragLeave / after Drop.
    session.describe(Description::Invalid);
    assert_eq!(
        data.descriptions.borrow().as_slice(),
        &[
            Description::Transfer(ShellTransferOperation::Copy, r"C:\dst".into()),
            Description::Invalid
        ]
    );
    let DropRequest::FolderTransfer(request) =
        session.drop_request(&decision, None).unwrap().unwrap()
    else {
        panic!("tree request expected")
    };
    assert_eq!(request.operation, ShellTransferOperation::Copy);
    assert!(!request.default_move_needs_volume_check);
    assert_eq!(request.sources, vec![PathBuf::from(r"C:\external\only")]);
    assert!(matches!(
        session.drop_request(&Decision::Grid, None).unwrap(),
        Some(DropRequest::GridFiles(_))
    ));
    assert_eq!(data.reads.get(), 2);
    assert!(
        session
            .drop_request(&Decision::None(Description::None), None)
            .unwrap()
            .is_none()
    );
    assert_eq!(data.reads.get(), 2);
}
