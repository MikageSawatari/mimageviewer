//! Pure editor coverage. No App construction, disk mutation, or product launch.
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use mimageviewer::book_composition::{PageOverride, PageSide, ReadingDirection, SinglePlacement};
use mimageviewer::ui_composition_editor::*;

#[path = "support/ui_harness.rs"]
mod test_ui;

#[path = "support/logical_bounds.rs"]
mod logical_bounds;

fn fixture(phase: EditorPhase) -> EditorView {
    let pages = (0..600)
        .map(|id| EditorPageView {
            id,
            name: format!(
                "長い日本語のページ名_第{}章_見開きと余白を確認するための原稿.png",
                id + 1
            ),
            physical_number: id + 1,
            hidden: id == 7,
            missing: id == 8,
            excluded_by_policy: id == 9,
            thumbnail: None,
            directive: if id == 1 {
                Some(PageOverride::BlankOpposite {
                    page_side: PageSide::Leading,
                })
            } else if id == 4 {
                Some(PageOverride::Alone)
            } else {
                None
            },
        })
        .collect();
    let mut units = vec![
        EditorUnitView::Single {
            page: 0,
            placement: SinglePlacement::Center,
            companion: EditorCompanionView::Vacant,
        },
        EditorUnitView::Single {
            page: 1,
            placement: SinglePlacement::Leading,
            companion: EditorCompanionView::Blank,
        },
        EditorUnitView::Pair {
            first: 2,
            second: 3,
        },
        EditorUnitView::Single {
            page: 4,
            placement: SinglePlacement::Trailing,
            companion: EditorCompanionView::Cover(0),
        },
    ];
    units.extend((10..600).map(|page| EditorUnitView::Single {
        page,
        placement: SinglePlacement::Center,
        companion: EditorCompanionView::Vacant,
    }));
    EditorView {
        book_name: "長い名前の本_本の構成とページの保存先を確認するための書庫.zip".into(),
        saved: true,
        phase,
        direction: ReadingDirection::Rtl,
        cover_enabled: true,
        cover: Some(0),
        page_order: (0..600).collect(),
        results_updating: false,
        pages,
        units,
        can_undo: true,
        can_redo: false,
        can_save: phase == EditorPhase::Editing,
        can_remove: true,
        landing: "23ページ (現在のページが非表示になるため、次のページへ移動)".into(),
        error: Some(
            "ページが変更されています。読み直してください。編集中の変更は保持しています。".into(),
        ),
    }
}

fn harness(
    physical: egui::Vec2,
    scale: f32,
    theme: mimageviewer::os_theme::ResolvedTheme,
    phase: EditorPhase,
) -> Harness<'static, Vec<EditorUiAction>> {
    let view = fixture(phase);
    let state = EditorUiState {
        selected_pages: vec![1],
        ..Default::default()
    };
    view_harness(physical, scale, theme, view, state)
}

fn view_harness(
    physical: egui::Vec2,
    scale: f32,
    theme: mimageviewer::os_theme::ResolvedTheme,
    view: EditorView,
    mut state: EditorUiState,
) -> Harness<'static, Vec<EditorUiAction>> {
    let mut fonts_ready = false;
    test_ui::builder().with_size(physical).build_ui_state(
        move |ui, actions: &mut Vec<EditorUiAction>| {
            let ctx = &ui.ctx().clone();
            mimageviewer::os_theme::apply_resolved(ctx, theme);
            // Geometry and interaction tests do not need real-time scrolling
            // animation; the production ScrollArea still uses its normal style.
            ctx.global_style_mut(|style| {
                style.scroll_animation = egui::style::ScrollAnimation::none()
            });
            if !fonts_ready {
                mimageviewer::ui_fonts::configure_fonts(ctx);
                mimageviewer::settings::apply_ui_scale_factor(ctx, scale);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            *actions = show_window(
                ui,
                egui::Id::unique("composition_snapshot"),
                &view,
                &mut state,
                DialogKeys::default(),
            );
        },
        Vec::new(),
    )
}

#[test]
fn composition_editor_small_screen_footer_and_snapshots() {
    use mimageviewer::os_theme::ResolvedTheme::{Dark, Light};
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for (width, height) in [(1093, 614), (1366, 728)] {
        for scale in [1.0, 2.0] {
            for (theme, name) in [(Light, "light"), (Dark, "dark")] {
                let mut h = harness(
                    egui::vec2(width as f32, height as f32),
                    scale,
                    theme,
                    EditorPhase::Editing,
                );
                h.run();
                assert!(
                    (h.ctx.content_rect().size() * h.ctx.pixels_per_point()
                        - egui::vec2(width as f32, height as f32))
                    .length()
                        < 1.0
                );
                assert!((h.ctx.pixels_per_point() - scale).abs() < 0.01);
                for label in ["元に戻す", "やり直す", "保存", "キャンセル", "構成を解除"]
                {
                    assert_control_inside(&h, label, &format!("{width}x{height} scale={scale}"));
                }
                assert_control_inside(
                    &h,
                    "Close window",
                    &format!("{width}x{height} scale={scale}"),
                );
                for label in [
                    "自動",
                    "単独で表示",
                    "隣を白紙にする",
                    "左",
                    "右",
                    "1ページ",
                ] {
                    assert_control_inside(
                        &h,
                        label,
                        &format!("result controls/card {width}x{height} scale={scale}"),
                    );
                }
                let requests: Vec<_> = h
                    .state()
                    .iter()
                    .filter_map(|a| match a {
                        EditorUiAction::RequestThumbnails(pages) => Some(pages),
                        _ => None,
                    })
                    .collect();
                assert!(requests.iter().all(|p| p.len() <= 24));
                assert!(
                    !requests.iter().any(|p| p.contains(&9)),
                    "policy-excluded pages never request a decode"
                );
                h.snapshot(format!(
                    "composition_editor_{width}_{height}_{}_{}",
                    (scale * 100.0) as u32,
                    name
                ));
                snapshots.extend(h.take_snapshot_results());
            }
        }
    }
}

#[test]
fn composition_editor_all_phases_keep_cancel_reachable() {
    for phase in [
        EditorPhase::Preparing,
        EditorPhase::Editing,
        EditorPhase::Saving,
        EditorPhase::Reloading,
        EditorPhase::Blocked,
    ] {
        let mut h = harness(
            egui::vec2(1093.0, 614.0),
            2.0,
            mimageviewer::os_theme::ResolvedTheme::Dark,
            phase,
        );
        h.run();
        for label in ["元に戻す", "やり直す", "保存", "構成を解除"] {
            assert_control_inside(&h, label, &format!("phase={phase:?}"));
        }
        assert_control_inside(&h, "Close window", &format!("phase={phase:?}"));
        let cancel = assert_control_inside(&h, "キャンセル", &format!("phase={phase:?}"));
        assert_eq!(
            cancel.enabled(),
            !matches!(phase, EditorPhase::Saving | EditorPhase::Reloading)
        );
    }
}

#[test]
fn composition_editor_remove_requires_confirmation() {
    let mut h = harness(
        egui::vec2(1093.0, 614.0),
        1.0,
        mimageviewer::os_theme::ResolvedTheme::Light,
        EditorPhase::Editing,
    );
    h.run();
    h.get_by_label("構成を解除").click();
    h.run_steps(4);
    assert!(!h.state().contains(&EditorUiAction::Remove));
    assert!(h.get_by_label("解除する").rect().is_positive());
    h.get_by_label("解除する").click();
    h.run_steps(4);
    // The action is frame-local; the confirmation disappears after acceptance.
    assert!(h.query_by_label("解除する").is_none());
}

#[test]
fn composition_editor_confirmation_and_blocked_snapshots_at_200_percent() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for (theme, name) in [
        (mimageviewer::os_theme::ResolvedTheme::Light, "light"),
        (mimageviewer::os_theme::ResolvedTheme::Dark, "dark"),
    ] {
        for kind in ["blocked", "updating", "remove"] {
            let mut view = fixture(if kind == "blocked" {
                EditorPhase::Blocked
            } else {
                EditorPhase::Editing
            });
            let mut state = EditorUiState::default();
            if kind == "blocked" {
                view.error = Some("保存された構成を読み取れません。元のデータは保持しています。読み直すか、今回は構成を使わず開いてください。".into());
            } else if kind == "updating" {
                view.results_updating = true;
                view.can_save = false;
            } else {
                state.confirmation = EditorConfirmation::Remove;
            }
            let mut h = view_harness(egui::vec2(1093.0, 614.0), 2.0, theme, view, state);
            h.run_steps(8);
            for label in ["元に戻す", "やり直す", "保存", "キャンセル", "構成を解除"]
            {
                assert_control_inside(&h, label, &format!("{kind}/{name}"));
            }
            assert_control_inside(&h, "Close window", &format!("{kind}/{name}"));
            if kind == "remove" {
                assert_control_inside(&h, "解除する", &format!("{kind}/{name}"));
            }
            h.snapshot(format!("composition_editor_{kind}_1093_614_200_{name}"));
            snapshots.extend(h.take_snapshot_results());
        }
    }
}

#[test]
fn composition_editor_confirmation_modals_small_screen_and_background_isolation() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for scale in [1.0, 2.0] {
        for (theme, name) in [
            (mimageviewer::os_theme::ResolvedTheme::Light, "light"),
            (mimageviewer::os_theme::ResolvedTheme::Dark, "dark"),
        ] {
            for (kind, confirmation, accept) in [
                ("remove", EditorConfirmation::Remove, "解除する"),
                ("reload", EditorConfirmation::Reload, "読み直す"),
                ("reload_long", EditorConfirmation::Reload, "読み直す"),
            ] {
                let mut view = fixture(EditorPhase::Editing);
                if kind == "reload_long" {
                    view.error = Some("ページが変更されています。C:/とても長い日本語の書庫名/ページ名.pngを読み直してください。".repeat(80));
                }
                let state = EditorUiState {
                    confirmation,
                    ..Default::default()
                };
                let mut h = view_harness(egui::vec2(1093.0, 614.0), scale, theme, view, state);
                h.run_steps(8);
                assert_control_inside(&h, accept, kind);
                assert_control_inside(&h, "キャンセル", kind);
                for label in [
                    "保存",
                    "構成を解除",
                    "元に戻す",
                    "右開き",
                    "表紙あり",
                    "ページを読み直す",
                    "Close window",
                ] {
                    assert!(
                        !assert_control_inside(&h, label, kind).enabled(),
                        "background {label}"
                    );
                }
                let background = assert_control_inside(&h, "右開き", kind);
                click_logical_response(&mut h, &background);
                h.run();
                assert!(
                    h.state()
                        .iter()
                        .all(|action| matches!(action, EditorUiAction::RequestThumbnails(_)))
                );
                assert!(h.query_by_label(accept).is_some());
                if !(kind == "remove" && scale == 2.0) {
                    h.snapshot(format!(
                        "composition_editor_{kind}_1093_614_{}_{}",
                        (scale * 100.0) as u32,
                        name
                    ));
                    snapshots.extend(h.take_snapshot_results());
                }
                let cancel = assert_control_inside(&h, "キャンセル", kind);
                click_logical_response(&mut h, &cancel);
                h.run_steps(4);
                assert!(h.query_by_label(accept).is_none(), "{kind}/{name}/{scale}");
                assert!(assert_control_inside(&h, "保存", kind).enabled());
                assert!(!h.state().contains(&EditorUiAction::Cancel));
                let entry = assert_control_inside(
                    &h,
                    if kind == "remove" {
                        "構成を解除"
                    } else {
                        "ページを読み直す"
                    },
                    kind,
                );
                click_logical_response(&mut h, &entry);
                h.run_steps(4);
                let button = assert_control_inside(&h, accept, kind);
                click_logical_response(&mut h, &button);
                h.step();
                assert!(
                    h.state().contains(&if kind == "remove" {
                        EditorUiAction::Remove
                    } else {
                        EditorUiAction::Reload
                    }),
                    "{kind}/{name}/{scale}: {:?}",
                    h.state()
                );
                assert!(!h.state().contains(&EditorUiAction::Save));
                h.run_steps(2);
                assert!(h.query_by_label(accept).is_none());
            }
        }
    }
}

#[test]
fn composition_editor_high_dpi_controls_receive_pointer_clicks() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1366.0, 728.0)] {
        let mut h = harness(
            size,
            2.0,
            mimageviewer::os_theme::ResolvedTheme::Light,
            EditorPhase::Editing,
        );
        h.run();
        for label in ["保存", "キャンセル", "元に戻す", "やり直す", "Close window"] {
            assert_control_inside(&h, label, "200% editor boundary");
        }
        let remove = assert_control_inside(&h, "構成を解除", "200% remove entry");
        click_logical_response(&mut h, &remove);
        h.run_steps(4);
        let cancel = assert_control_inside(&h, "キャンセル", "200% confirmation cancel");
        click_logical_response(&mut h, &cancel);
        h.run_steps(4);
        assert!(h.query_by_label("解除する").is_none());
        assert!(!h.state().contains(&EditorUiAction::Cancel));
        let remove = assert_control_inside(&h, "構成を解除", "200% remove reentry");
        click_logical_response(&mut h, &remove);
        h.run_steps(4);
        let accept = assert_control_inside(&h, "解除する", "200% confirmation accept");
        click_logical_response(&mut h, &accept);
        h.step();
        assert!(h.state().contains(&EditorUiAction::Remove));
        assert!(!h.state().contains(&EditorUiAction::Save));
    }
}

// Exercise real pointer hit testing at the logical response rect.
fn click_logical_response(h: &mut Harness<'_, Vec<EditorUiAction>>, response: &egui::Response) {
    let pos = response.rect.center();
    h.event(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
}

/// Node::rect and egui responses both use logical points.
/// Check both, including the actual unclipped pointer interaction rectangle.
fn assert_control_inside(
    h: &Harness<'_, Vec<EditorUiAction>>,
    label: &str,
    detail: &str,
) -> egui::Response {
    let nodes = h.get_all_by_label(label).collect::<Vec<_>>();
    // The modal and disabled editor both have a cancel button. Inspect the
    // active response when present, retaining disabled controls for other cases.
    let node = nodes
        .iter()
        .find(|node| {
            let id = egui_id_from_accesskit(node.accesskit_node().id());
            h.ctx
                .read_response(id)
                .is_some_and(|response| response.enabled())
        })
        .unwrap_or(&nodes[0]);
    let logical_screen = h.ctx.content_rect();
    logical_bounds::assert_node_inside(&h.ctx, node.rect(), &format!("{detail} {label}"));
    // AccessKit stores the original egui widget id (same bridge as ui_snapshot).
    let id = egui_id_from_accesskit(node.accesskit_node().id());
    let response = h
        .ctx
        .read_response(id)
        .expect("current editor widget response");
    assert!(
        logical_screen.contains_rect(response.rect),
        "{detail} {label}: logical screen={logical_screen:?} rect={:?}",
        response.rect
    );
    assert!(
        response.interact_rect.contains_rect(response.rect),
        "{detail} {label}: clipped interaction rect={:?} widget={:?}",
        response.interact_rect,
        response.rect
    );
    response
}

#[test]
fn composition_editor_hud_real_entry_and_indicator_c9() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for available in [false, true] {
        for (theme, name) in [
            (mimageviewer::os_theme::ResolvedTheme::Light, "light"),
            (mimageviewer::os_theme::ResolvedTheme::Dark, "dark"),
        ] {
            let mut fonts_ready = false;
            let mut h = test_ui::builder()
                .with_size(egui::vec2(450.0, 200.0))
                .build_ui_state(
                    move |ui, activated: &mut bool| {
                        let ctx = &ui.ctx().clone();
                        mimageviewer::os_theme::apply_resolved(ctx, theme);
                        if !fonts_ready {
                            mimageviewer::ui_fonts::configure_fonts(ctx);
                            fonts_ready = true;
                            ctx.request_repaint();
                            return;
                        }
                        test_ui::full_viewport_ui(ui, |ui| {
                            egui::CentralPanel::default().show(ui, |ui| {
                                *activated |=
                                    mimageviewer::draw_composition_editor_hud_snapshot_fixture(
                                        ui, available, true,
                                    );
                            });
                        });
                    },
                    false,
                );
            h.run();
            assert!(h.get_by_label("本の構成を編集").rect().is_positive());
            h.snapshot(format!(
                "composition_editor_hud_{}_{}",
                if available { "available" } else { "blocked" },
                name
            ));
            snapshots.extend(h.take_snapshot_results());
            h.get_by_label("本の構成を編集").click();
            h.run();
            assert_eq!(
                *h.state(),
                available,
                "disabled HUD entry cannot emit editor admission"
            );
        }
    }
}

#[test]
fn composition_editor_accepts_only_ime_filtered_dialog_keys() {
    let ctx = egui::Context::default();
    let view = fixture(EditorPhase::Editing);
    let mut state = EditorUiState::default();
    let mut actions = Vec::new();
    for key in [egui::Key::Enter, egui::Key::Escape] {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1093.0, 614.0),
                )),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                let ctx = &ui.ctx().clone();
                actions = show_window(
                    ctx,
                    egui::Id::unique("filtered_keys"),
                    &view,
                    &mut state,
                    DialogKeys::default(),
                );
            },
        )
        .drop_without_applying_deltas();
        assert!(!actions.contains(&EditorUiAction::Save));
        assert!(!actions.contains(&EditorUiAction::Cancel));
    }
    ctx.run_ui(egui::RawInput::default(), |ui| {
        let ctx = &ui.ctx().clone();
        actions = show_window(
            ctx,
            egui::Id::unique("filtered_keys"),
            &view,
            &mut state,
            DialogKeys {
                enter: false,
                escape: true,
            },
        );
    })
    .drop_without_applying_deltas();
    assert!(actions.contains(&EditorUiAction::Cancel));
}

// accesskit_consumer 0.39's public FullNodeId conversion packs the local node ID
// above the tree index. The local node ID is egui's original widget identity.
fn egui_id_from_accesskit(node_id: impl Into<u128>) -> egui::Id {
    let local_id = (node_id.into() >> 64) as u64;
    unsafe { egui::Id::from_high_entropy_bits(local_id) }
}
