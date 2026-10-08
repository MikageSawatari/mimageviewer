//! egui_kittest による UI スナップショットテスト (v0.7.0〜)。
//!
//! ## 目的
//!
//! 3 テーマ (Light / Dark / System) × 主要 UI (メイン・環境設定・メタデータパネル等) の
//! 見た目を PNG スナップショットとして保存し、意図しない見た目変化を回帰として検出する。
//! カラースキーム・パネル崩れ・余白計算の回帰を自動検知するのが狙い。
//!
//! ## 実行
//!
//! ```
//! cargo test --test ui_snapshot
//! ```
//!
//! ## スナップショット更新 (意図的に見た目を変えたとき)
//!
//! ```
//! UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot
//! ```
//!
//! 更新後は `tests/snapshots/ui_snapshot/*.png` の差分を目視確認してからコミットする。
//!
//! ## 参考
//!
//! - [egui_kittest docs](https://docs.rs/egui_kittest/)
//! - mimageviewer 側のポリシー: [docs/ui-snapshot-policy.md](../docs/ui-snapshot-policy.md)

use egui_kittest::{Harness, kittest::Queryable};

#[test]
fn chrome_suppression_settings_light() {
    chrome_suppression_settings_snapshot(
        "chrome_suppression_settings_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        egui::vec2(720.0, 240.0),
    );
}

#[test]
fn chrome_suppression_settings_dark() {
    chrome_suppression_settings_snapshot(
        "chrome_suppression_settings_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(720.0, 240.0),
    );
}

#[test]
fn chrome_suppression_settings_narrow_dark() {
    chrome_suppression_settings_snapshot(
        "chrome_suppression_settings_narrow_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(360.0, 320.0),
    );
}

fn chrome_suppression_settings_snapshot(
    name: &str,
    theme: mimageviewer::os_theme::ResolvedTheme,
    size: egui::Vec2,
) {
    let mut targets = mimageviewer::settings::FullscreenChromeSuppression {
        top: true,
        bottom: true,
        info: true,
        navigator: true,
    };
    snapshot_with_theme_contrast_and_size(
        name,
        theme,
        mimageviewer::settings::TextContrast::Standard,
        size,
        |ui| {
            mimageviewer::ui_helpers::draw_fullscreen_chrome_suppression_setting(ui, &mut targets);
        },
    );
}

const STARTUP_DIALOG_CASES: &[(&str, &[&str])] = &[
    ("first_setup", &["開始"]),
    ("boot_incompatible", &["設定の復元を開く", "アプリを終了"]),
    ("boot_unreadable", &["設定の復元を開く", "アプリを終了"]),
    ("whats_new", &["すべての変更を見る", "閉じる"]),
    (
        "update_notice",
        &[
            "リリースページを開く",
            "このバージョンの通知をオフ",
            "閉じる",
        ],
    ),
    ("update_current", &["リリースページを開く", "閉じる"]),
    ("update_error", &["リリースページを開く", "閉じる"]),
    (
        "network_data_dir",
        &["閉じる", "この保存先では今後表示しない"],
    ),
    ("restore_result", &["アプリを終了して再起動を促す"]),
    ("restore_success", &["アプリを終了"]),
    ("restore_recoverable", &["閉じる"]),
    (
        "restore_remote",
        &["リモート設定readerを再接続", "アプリを終了"],
    ),
    ("restore_list", &["設定を完全リセット…"]),
    ("restore_confirm", &["復元して終了", "キャンセル"]),
    ("restore_reset", &["リセットして終了", "キャンセル"]),
    ("pdf_notice", &["閉じる"]),
    ("susie_notice", &["閉じる"]),
    ("trt_notice", &["ワーカーを再起動", "閉じる"]),
    ("mouse_migration", &["標準にする", "従来どおり"]),
    (
        "rename_recovery",
        &["再読み込み", "壊れた記録を退避して再開", "閉じる"],
    ),
    ("rename_quarantining", &["キャンセル"]),
    ("archive_scanning", &["キャンセル"]),
    ("archive_confirm", &["変換して開く", "キャンセル"]),
    ("archive_empty", &["変換して開く", "キャンセル"]),
    ("archive_sibling", &["ZIP ファイルに変換", "キャンセル"]),
    ("archive_converting", &["キャンセル"]),
    ("archive_error", &["閉じる"]),
];

#[test]
fn startup_status_light() {
    startup_status_snapshot(
        "startup_status_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
    );
}

#[test]
fn startup_status_dark() {
    startup_status_snapshot(
        "startup_status_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
    );
}

fn startup_status_snapshot(name: &str, theme: mimageviewer::os_theme::ResolvedTheme) {
    snapshot_with_theme_and_contrast_settling(
        name,
        theme,
        mimageviewer::settings::TextContrast::Standard,
        Some(4),
        |ui| {
            mimageviewer::ui_startup::draw_status(
                ui,
                "検索の記録を読み込んでいます",
                std::time::Duration::from_millis(1250),
                std::time::Duration::from_millis(12500),
            );
        },
    );
}

#[test]
fn startup_preparing_dark() {
    snapshot_with_theme(
        "startup_preparing_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::ui_startup::draw_preparing,
    );
}

#[test]
fn startup_status_strong_light() {
    startup_strong_snapshot(
        "startup_status_strong_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
    );
}

#[test]
fn startup_status_strong_dark() {
    startup_strong_snapshot(
        "startup_status_strong_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
    );
}

fn startup_strong_snapshot(name: &str, theme: mimageviewer::os_theme::ResolvedTheme) {
    snapshot_with_theme_and_contrast_settling(
        name,
        theme,
        mimageviewer::settings::TextContrast::Strong,
        Some(4),
        |ui| {
            mimageviewer::ui_startup::draw_status(
                ui,
                "検索の記録を読み込んでいます",
                std::time::Duration::from_secs(999),
                std::time::Duration::from_secs(1005),
            );
        },
    );
}

#[test]
fn startup_status_small_long_stage() {
    snapshot_with_theme_options(
        "startup_status_small_long_stage",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(240.0, 240.0),
        Some(4),
        |ui| {
            mimageviewer::ui_startup::draw_status(
                ui,
                "検索するフォルダーの変更を確認しています",
                std::time::Duration::from_secs(999),
                std::time::Duration::from_secs(1005),
            );
        },
        |_| {},
    );
}

#[test]
fn startup_dialogs_small_viewport() {
    use egui_kittest::kittest::NodeT;
    #[derive(Default)]
    struct Actions {
        watched: Vec<egui::Id>,
        clicked: std::collections::HashSet<egui::Id>,
    }
    let mut results = egui_kittest::SnapshotResults::new();
    for (width, height) in [(1093, 614), (1366, 728)] {
        for scale in [1.0_f32, 2.0] {
            for &(kind, buttons) in STARTUP_DIALOG_CASES {
                let size = egui::vec2(width as f32, height as f32);
                let mut fonts_ready = false;
                let mut harness = Harness::builder().with_size(size).build_state(
                    move |ctx, actions: &mut Actions| {
                        mimageviewer::os_theme::apply_resolved(
                            ctx,
                            mimageviewer::os_theme::ResolvedTheme::Light,
                        );
                        if !fonts_ready {
                            install_app_fonts(ctx);
                            mimageviewer::settings::apply_ui_scale_factor(ctx, scale);
                            fonts_ready = true;
                            ctx.request_repaint();
                            return;
                        }
                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(ctx, |ui| {
                                mimageviewer::ui_dialogs::draw_startup_dialog_snapshot_fixture(
                                    ui, kind,
                                );
                            });
                        for &id in &actions.watched {
                            if ctx.read_response(id).is_some_and(|r| r.clicked()) {
                                actions.clicked.insert(id);
                            }
                        }
                    },
                    Actions::default(),
                );
                // Scanning/quarantine spinners intentionally repaint continuously.
                // Fixed steps also settle Area sizing without relying on immediate repaint.
                harness.run_steps(12);
                let viewport = harness.ctx.content_rect();
                assert!(
                    (viewport.size() * scale - size).length() < 1.0,
                    "{kind}: wrong scale/viewport {viewport:?}"
                );
                let mut labels = buttons.to_vec();
                if harness.query_by_label("Close window").is_some() {
                    labels.push("Close window");
                }
                let mut ids = Vec::new();
                for button in labels {
                    let node = harness.get_by_label(button);
                    let rect = node.rect();
                    assert!(
                        egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect),
                        "{kind} {size:?} scale {scale} {button}: {rect:?}"
                    );
                    // AccessKit stores egui's original high-entropy widget id.
                    let id =
                        unsafe { egui::Id::from_high_entropy_bits(node.accesskit_node().id().0) };
                    let disabled = kind == "archive_empty" && button == "変換して開く";
                    let response = harness.ctx.read_response(id).unwrap();
                    assert!(
                        viewport.contains_rect(response.rect),
                        "{kind} {button}: logical viewport"
                    );
                    assert_eq!(response.enabled(), !disabled);
                    // AccessKit bounds are physical pixels; raw egui events use
                    // logical points. Node::hover/click does not convert at zoom2.
                    harness.hover_at(response.rect.center());
                    harness.run_steps(3);
                    let response = harness.ctx.read_response(id).unwrap();
                    assert!(
                        response.contains_pointer(),
                        "{kind} scale {scale} {button}: clipped/covered"
                    );
                    assert!(
                        response.interact_rect.contains_rect(response.rect),
                        "{kind} {button}: partial clip"
                    );
                    if !disabled {
                        assert!(response.hovered());
                    }
                    ids.push((button, id, disabled));
                }
                harness.hover_at(egui::Pos2::ZERO);
                harness.run_steps(3);
                // Geometry and pointer assertions cover every case. Keep PNGs
                // only for four representative views and two settled highlights.
                let snapshot = ((width, height) == (1093, 614)
                    && matches!(kind, "first_setup" | "archive_confirm"))
                    || (kind == "whats_new" && scale == 1.0);
                if snapshot {
                    let suffix = if scale == 1.0 { "" } else { "_ui200" };
                    results.add(
                        harness.try_snapshot(&format!("startup_{kind}_{width}x{height}{suffix}")),
                    );
                }
                // Activate each action with mouse events and latch the release frame.
                // Pure fixtures discard action results: no save, worker, URL, restore or exit.
                harness.state_mut().watched = ids.iter().map(|(_, id, _)| *id).collect();
                for (button, id, disabled) in ids {
                    let pos = harness.ctx.read_response(id).unwrap().rect.center();
                    harness.hover_at(pos);
                    for pressed in [true, false] {
                        harness.event(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                    harness.run_steps(3);
                    assert_eq!(
                        harness.state().clicked.contains(&id),
                        !disabled,
                        "{kind} scale {scale} {button}: click"
                    );
                }
            }
        }
    }
}

#[test]
fn startup_dialogs_refit_after_viewport_resize() {
    use egui_kittest::kittest::NodeT;
    for scale in [1.0, 2.0] {
        for &(kind, buttons) in STARTUP_DIALOG_CASES {
            let mut fonts_ready = false;
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1920.0, 1440.0))
                .build_state(
                    move |ctx, state: &mut (Vec<egui::Id>, Vec<egui::Response>)| {
                        if !fonts_ready {
                            install_app_fonts(ctx);
                            mimageviewer::settings::apply_ui_scale_factor(ctx, scale);
                            fonts_ready = true;
                            ctx.request_repaint();
                            return;
                        }
                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(ctx, |ui| {
                                mimageviewer::ui_dialogs::draw_startup_dialog_snapshot_fixture(
                                    ui, kind,
                                );
                            });
                        // Capture the current pass rather than read_response after
                        // Context::run, which can prefer discarded-pass widgets.
                        state.1 = state
                            .0
                            .iter()
                            .map(|id| ctx.read_response(*id).unwrap())
                            .collect();
                    },
                    (Vec::new(), Vec::new()),
                );
            harness.run_steps(12);
            let mut labels = buttons.to_vec();
            if kind == "first_setup" {
                labels.push("初回設定");
            }
            if harness.query_by_label("Close window").is_some() {
                labels.push("Close window");
            }
            let ids: Vec<_> = labels
                .iter()
                .map(|label| {
                    let node = harness.get_by_label(*label);
                    unsafe { egui::Id::from_high_entropy_bits(node.accesskit_node().id().0) }
                })
                .collect();
            harness.state_mut().0 = ids;
            for size in [
                egui::vec2(1093.0, 614.0),
                egui::vec2(1920.0, 1440.0),
                egui::vec2(1093.0, 614.0),
            ] {
                // Match egui-winit: resized RawInput is already in zoomed points.
                harness.set_size(size / scale);
                // Some fixtures animate spinners, so cannot run to idle.
                // At most two native frames, never twelve settling frames.
                harness.run_steps(2);
                let viewport = harness.ctx.content_rect();
                assert!((viewport.size() * scale - size).length() < 1.0);
                for (label, response) in labels.iter().zip(&harness.state().1) {
                    assert!(
                        viewport.contains_rect(response.rect),
                        "{kind} scale {scale}: {label} {:?} in {viewport:?}",
                        response.rect
                    );
                    assert!(
                        response.interact_rect.contains_rect(response.rect),
                        "{kind} scale {scale}: {label} clipped"
                    );
                }
            }
        }
    }
}

#[test]
fn raw_license_information_light() {
    snapshot_with_theme_contrast_and_size(
        "raw_license_information_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(620.0, 340.0),
        mimageviewer::ui_dialogs::draw_raw_license_snapshot_fixture,
    );
}

#[test]
fn raw_license_information_dark() {
    snapshot_with_theme_contrast_and_size(
        "raw_license_information_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(620.0, 340.0),
        mimageviewer::ui_dialogs::draw_raw_license_snapshot_fixture,
    );
}

#[test]
fn raw_license_information_expanded_dark() {
    snapshot_with_theme_contrast_and_size_with_interaction(
        "raw_license_information_expanded_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(620.0, 920.0),
        mimageviewer::ui_dialogs::draw_raw_license_snapshot_fixture,
        |harness| {
            for title in [
                "LibRaw ライセンス・著作権表記 全文",
                "zlib License 全文",
                "libjpeg-turbo ライセンス・著作権表記 全文",
            ] {
                harness.get_by_label(title).click();
                harness.run();
            }
            harness.remove_cursor();
            harness.run();
        },
    );
}

#[test]
fn raw_settings_light() {
    // Render the actual dedicated preferences page, including its heading.
    snapshot_with_theme(
        "raw_settings_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::draw_raw_settings_snapshot_fixture,
    );
}

#[test]
fn raw_settings_dark() {
    snapshot_with_theme(
        "raw_settings_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_raw_settings_snapshot_fixture,
    );
}

#[test]
fn raw_progress_dark() {
    snapshot_with_theme(
        "raw_progress_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_raw_progress_snapshot_fixture,
    );
}

#[test]
fn raw_blocked_preview_notice_dark() {
    snapshot_with_theme_contrast_and_size(
        "raw_blocked_preview_notice_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(800.0, 120.0),
        mimageviewer::draw_raw_blocked_preview_snapshot_fixture,
    );
}

/// The status label must yield a row to the minimum-width field, and may itself
/// wrap within that row. Persist only the two distinct inline/stacked layouts.
#[test]
fn folder_toolbar_snapshot_status_label() {
    use egui_kittest::kittest::Queryable;
    use mimageviewer::ui_toolbar_layout as layout;
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for width in [240.0, 360.0, 960.0] {
        for zoom in [1.0, 1.5] {
            for changed in [false, true] {
                let label = if changed {
                    "(スナップショット中 674件 / filter 変更後)"
                } else {
                    "(スナップショット中 674件)"
                };
                let mut fonts_ready = false;
                let mut path = String::from(r"C:\Pictures\日本語フォルダ");
                let mut harness = Harness::builder()
                    .with_size(egui::vec2(width * zoom, 120.0 * zoom))
                    .build(move |ctx| {
                        ctx.set_zoom_factor(zoom);
                        mimageviewer::os_theme::apply_resolved(
                            ctx,
                            mimageviewer::os_theme::ResolvedTheme::Dark,
                        );
                        if !fonts_ready {
                            install_app_fonts(ctx);
                            fonts_ready = true;
                            ctx.request_repaint();
                            return;
                        }
                        let panel = egui::TopBottomPanel::top("folder").show(ctx, |ui| {
                            let input = layout::address_input(
                                ui,
                                Some(
                                    egui::RichText::new(label)
                                        .color(egui::Color32::from_rgb(58, 110, 165))
                                        .into(),
                                ),
                                |ui| {
                                    ui.add_enabled(
                                        false,
                                        egui::TextEdit::singleline(&mut path)
                                            .desired_width(f32::INFINITY),
                                    )
                                    .rect
                                },
                            );
                            assert!(input.inner.width() >= layout::ADDRESS_INPUT_MIN_WIDTH);
                            input.response.rect
                        });
                        assert!(panel.response.rect.expand(0.5).contains_rect(panel.inner));
                    });
                harness.run();
                let input = harness.get_by_role(egui::accesskit::Role::TextInput).rect();
                let label_rect = harness.get_by_label(label).rect();
                if width == 240.0 || (width == 360.0 && changed) {
                    assert!(
                        label_rect.bottom() <= input.top(),
                        "width={width} zoom={zoom} changed={changed}: {label_rect:?} {input:?}"
                    );
                } else if width == 960.0 {
                    assert!(label_rect.right() <= input.left());
                    assert!(label_rect.top() < input.bottom() && input.top() < label_rect.bottom());
                } else {
                    assert!(
                        label_rect.bottom() <= input.top() || label_rect.right() <= input.left()
                    );
                }
                for rect in [input, label_rect] {
                    assert!(rect.left() >= 0.0 && rect.right() <= width * zoom);
                }
                if changed && zoom == 1.0 && width >= 360.0 {
                    harness.snapshot(format!(
                        "folder_toolbar_snapshot_label_{}",
                        if width == 360.0 { "stacked" } else { "inline" }
                    ));
                    snapshots.extend(harness.take_snapshot_results());
                }
            }
        }
    }
}

/// Actual control styles, including the reported count and omitted-entry badge.
/// All positions/row settings get geometry checks; persist only unique pixels.
#[test]
fn folder_toolbar_flexible_snapshots() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    let mut unique = std::collections::HashSet::new();
    for (size, width, zoom) in [
        ("normal", 1120.0, 1.0),
        ("narrow", 360.0, 1.0),
        ("dpi150", 1080.0, 1.5),
    ] {
        for (position, before) in [("first", 0), ("middle", 1), ("last", 2)] {
            for new_row in [true, false] {
                for buttons in [true, false] {
                    let mut fonts_ready = false;
                    let mut path = String::from(r"C:\Pictures\日本語フォルダ");
                    let mut harness = Harness::builder()
                        .with_size(egui::vec2(width, 240.0 * zoom))
                        .build(move |ctx| {
                            ctx.set_zoom_factor(zoom);
                            mimageviewer::os_theme::apply_resolved(
                                ctx,
                                mimageviewer::os_theme::ResolvedTheme::Dark,
                            );
                            if !fonts_ready {
                                install_app_fonts(ctx);
                                fonts_ready = true;
                                ctx.request_repaint();
                                return;
                            }
                            egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                                draw_folder_toolbar_fixture(
                                    ui, before, new_row, buttons, &mut path,
                                );
                            });
                        });
                    harness.run();
                    // Verify the alias policy with pixels too, so a regression
                    // cannot silently turn a required baseline into a duplicate.
                    // First position: no preceding row. Narrow: every combination
                    // wraps. Full controls also wrap at the last normal position
                    // and both non-first positions at 150% DPI.
                    let duplicate = !new_row
                        && (before == 0
                            || size == "narrow"
                            || (buttons && (size == "dpi150" || before == 2)));
                    let pixels = harness.render().unwrap();
                    assert_eq!(
                        unique.insert((pixels.width(), pixels.height(), pixels.into_raw())),
                        !duplicate,
                        "unexpected snapshot alias: {size} {position} row={new_row} buttons={buttons}"
                    );
                    if !duplicate {
                        let name = format!(
                            "folder_toolbar_{size}_{position}_{}{}",
                            if new_row { "row" } else { "inline" },
                            if buttons { "" } else { "_minimal" }
                        );
                        println!("folder baseline: {name}");
                        harness.snapshot(name);
                        snapshots.extend(harness.take_snapshot_results());
                    }
                }
            }
        }
    }
}

fn draw_folder_toolbar_fixture(
    ui: &mut egui::Ui,
    before: usize,
    new_row: bool,
    buttons: bool,
    path: &mut String,
) {
    use mimageviewer::ui_toolbar_layout as layout;
    ui.horizontal_wrapped(|ui| {
        for text in ["ツリー / 列: 5", "比率: 自動 / タグ: 旅行"]
            .iter()
            .take(before)
        {
            let _ = ui.button(*text);
        }
        let count = if buttons { "(674/674)" } else { "(25/120)" };
        let mut measured = layout::FolderBarWidth::with_input();
        measured.label(ui, "フォルダ:");
        if buttons {
            for text in ["←", "→"] {
                measured.button(ui, text, 0.0);
            }
            for text in ["A", "B"] {
                measured.button(ui, egui::RichText::new(text).monospace(), 24.0);
            }
            measured.space(6.0 + ui.spacing().item_spacing.x);
            for text in ["⬆", "▲", "▼"] {
                measured.button(ui, text, 0.0);
            }
            measured.space(6.0 + ui.spacing().item_spacing.x);
        }
        measured.button(ui, "場所▼", 0.0);
        measured.space(4.0 + 6.0 + ui.spacing().item_spacing.x);
        let mut right_width = layout::FolderBarWidth::controls();
        if buttons {
            right_width.button(
                ui,
                egui::RichText::new("非表示 321 件").small().strong(),
                0.0,
            );
            right_width.space(4.0);
        }
        right_width.label(ui, egui::RichText::new(count).size(11.0).monospace());
        right_width.space(4.0);
        if buttons {
            for text in ["スタック", "サブ展開", "📌", "履歴▼", "♡"] {
                right_width.button_with_frame(ui, text, 0.0, !matches!(text, "📌" | "♡"));
                right_width.space(4.0);
            }
        }
        let minimum = (measured.width() + right_width.width()).ceil();
        let slot = layout::flexible_section(ui, minimum, new_row, |ui, compact| {
            layout::folder_controls(ui, compact, |ui| {
                let mut left = vec![ui.label("フォルダ:").rect];
                if buttons {
                    for text in ["←", "→"] {
                        left.push(ui.button(text).rect);
                    }
                    for text in ["A", "B"] {
                        left.push(
                            ui.add(
                                egui::Button::new(egui::RichText::new(text).monospace())
                                    .min_size(egui::vec2(24.0, 20.0)),
                            )
                            .rect,
                        );
                    }
                    ui.separator();
                    for text in ["⬆", "▲", "▼"] {
                        left.push(ui.button(text).rect);
                    }
                    ui.separator();
                }
                left.push(layout::folder_menu_button(ui, true, "場所▼", |_| {}).rect);
                ui.add_space(4.0);
                ui.separator();
                let mut right = Vec::new();
                let mut input = None;
                layout::folder_tail(
                    ui,
                    layout::FolderBarWidth::with_input().width(),
                    right_width.width(),
                    |ui, part| match part {
                        layout::FolderTailPart::RightControls => {
                            if buttons {
                                right.push(
                                    egui::containers::menu::MenuButton::new(
                                        egui::RichText::new("非表示 321 件").small().strong(),
                                    )
                                    .ui(ui, |_| {})
                                    .0
                                    .rect,
                                );
                                ui.add_space(4.0);
                            }
                            right.push(
                                ui.label(
                                    egui::RichText::new(count)
                                        .size(11.0)
                                        .monospace()
                                        .color(ui.visuals().weak_text_color()),
                                )
                                .rect,
                            );
                            ui.add_space(4.0);
                            if buttons {
                                right.push(ui.selectable_label(false, "スタック").rect);
                                ui.add_space(4.0);
                                right
                                    .push(ui.add(egui::Button::selectable(false, "サブ展開")).rect);
                                ui.add_space(4.0);
                                right.push(ui.add(egui::Button::new("📌").frame(false)).rect);
                                ui.add_space(4.0);
                                right.push(
                                    layout::folder_menu_button(ui, true, "履歴▼", |_| {}).rect,
                                );
                                ui.add_space(4.0);
                                right.push(ui.add(egui::Button::new("♡").frame(false)).rect);
                                ui.add_space(4.0);
                            }
                        }
                        layout::FolderTailPart::Input => {
                            layout::address_input(ui, None, |ui| {
                                input = Some(
                                    ui.add(
                                        egui::TextEdit::singleline(&mut *path)
                                            .desired_width(f32::INFINITY),
                                    )
                                    .rect,
                                );
                            });
                        }
                    },
                );
                let input = input.unwrap();
                let precedes = |a: egui::Rect, b: egui::Rect| {
                    a.bottom() <= b.top() + 0.5
                        || (a.right() <= b.left() + 0.5
                            && a.top() < b.bottom()
                            && b.top() < a.bottom())
                };
                assert!(
                    left.iter().all(|rect| precedes(*rect, input)),
                    "left/input order: {left:?} {input:?}"
                );
                assert!(
                    right.iter().all(|rect| precedes(input, *rect)),
                    "input/right order: {input:?} {right:?}"
                );
                assert!(input.width() >= layout::ADDRESS_INPUT_MIN_WIDTH);
                assert!(input.right() <= ui.clip_rect().right() + 0.5);
            });
        })
        .response
        .rect;
        for text in ["ツリー / 列: 5", "比率: 自動 / タグ: 旅行"]
            .iter()
            .skip(before)
        {
            let following = ui.button(*text).rect;
            assert!(
                following.top() >= slot.bottom(),
                "subsequent sections need their own row"
            );
        }
    });
}

fn snapshot_color_presets(name: &str, width: f32) {
    let mut fonts_ready = false;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(width, 160.0))
        .build(move |ctx| {
            mimageviewer::os_theme::apply_resolved(
                ctx,
                mimageviewer::os_theme::ResolvedTheme::Dark,
            );
            if !fonts_ready {
                install_app_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                let available = ui.available_width();
                let response = mimageviewer::draw_color_presets_snapshot_fixture(ui);
                assert!(response.rect.width() <= available + 0.1);
                assert!(response.rect.height() >= 48.0);
            });
        });
    harness.run();
    harness.snapshot(name);
}

#[test]
fn color_presets_popup_width() {
    snapshot_color_presets("color_presets_popup_dark", 308.0);
}

#[test]
fn color_presets_narrow_width() {
    snapshot_color_presets("color_presets_narrow_dark", 224.0);
}

#[test]
fn preferences_transfer_entry_disabled_dark() {
    snapshot_with_theme(
        "preferences_transfer_entry_disabled_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_preferences_transfer_disabled_entry_snapshot_fixture,
    );
}

#[test]
fn preferences_transfer_export_explanation_light() {
    snapshot_with_theme(
        "preferences_transfer_export_explanation_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| mimageviewer::draw_preferences_transfer_explanation_snapshot_fixture(ui, false),
    );
}

#[test]
fn preferences_transfer_import_explanation_dark() {
    snapshot_with_theme(
        "preferences_transfer_import_explanation_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| mimageviewer::draw_preferences_transfer_explanation_snapshot_fixture(ui, true),
    );
}

#[test]
fn preferences_transfer_entry_light() {
    snapshot_with_theme(
        "preferences_transfer_entry_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::draw_preferences_transfer_entry_snapshot_fixture,
    );
}

#[test]
fn preferences_transfer_entry_dark() {
    snapshot_with_theme(
        "preferences_transfer_entry_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_preferences_transfer_entry_snapshot_fixture,
    );
}

#[test]
fn preferences_transfer_light() {
    snapshot_with_theme(
        "preferences_transfer_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| mimageviewer::draw_preferences_transfer_settings_snapshot_fixture(ui, false),
    );
}

#[test]
fn preferences_transfer_dark() {
    snapshot_with_theme(
        "preferences_transfer_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| mimageviewer::draw_preferences_transfer_settings_snapshot_fixture(ui, false),
    );
}

#[test]
fn preferences_transfer_narrow_result() {
    snapshot_with_theme_at_size(
        "preferences_transfer_narrow_result",
        mimageviewer::os_theme::ResolvedTheme::Light,
        egui::vec2(320.0, 420.0),
        None,
        |ui| mimageviewer::draw_preferences_transfer_settings_snapshot_fixture(ui, false),
    );
}

#[test]
fn preferences_transfer_busy_dark() {
    snapshot_with_theme_and_contrast_settling(
        "preferences_transfer_busy_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::Standard,
        Some(4),
        |ui| mimageviewer::draw_preferences_transfer_settings_snapshot_fixture(ui, true),
    );
}

#[test]
fn preferences_file_organize_light() {
    snapshot_with_theme(
        "preferences_file_organize_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::draw_file_organize_destinations_settings_snapshot_fixture,
    );
}

#[test]
fn preferences_file_organize_dark() {
    snapshot_with_theme(
        "preferences_file_organize_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_file_organize_destinations_settings_snapshot_fixture,
    );
}

#[test]
fn file_organize_destinations_light() {
    use mimageviewer::settings::FileOrganizeDestination;
    use mimageviewer::shell_file_ops::ShellTransferOperation;
    let sources = vec![std::path::PathBuf::from(r"C:\写真\画像.jpg")];
    let destinations = vec![
        FileOrganizeDestination {
            name: "保管".into(),
            path: r"D:\写真\保管".into(),
        },
        FileOrganizeDestination {
            name: "確認".into(),
            path: r"\\server\share\長い名前の写真フォルダ\整理先".into(),
        },
    ];
    let mut focus = Some((1, Some(ShellTransferOperation::Copy)));
    snapshot_file_organize_modal(
        "file_organize_destinations_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        egui::vec2(1000.0, 620.0),
        sources,
        destinations,
        focus.take(),
    );
}

#[test]
fn file_organize_empty_dark() {
    snapshot_file_organize_modal(
        "file_organize_empty_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(1000.0, 620.0),
        vec![r"C:\写真\画像.jpg".into()],
        vec![],
        None,
    );
}

#[test]
fn file_organize_many_dark() {
    snapshot_file_organize_modal(
        "file_organize_many_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(1000.0, 620.0),
        vec![r"C:\写真\画像.jpg".into()],
        (0..30)
            .map(|row| mimageviewer::settings::FileOrganizeDestination {
                name: format!("整理先 {row}"),
                path: r"\\server\share\長い名前の写真フォルダ\さらに長い名前のフォルダ\整理先"
                    .into(),
            })
            .collect(),
        Some((
            2,
            Some(mimageviewer::shell_file_ops::ShellTransferOperation::Move),
        )),
    );
}

#[test]
fn file_organize_narrow_light() {
    snapshot_file_organize_modal(
        "file_organize_narrow_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        egui::vec2(420.0, 320.0),
        vec![r"C:\写真\画像.jpg".into()],
        vec![mimageviewer::settings::FileOrganizeDestination {
            name: "長い名前の整理先".into(),
            path: r"\\server\share\長い名前の写真フォルダ\整理先".into(),
        }],
        Some((
            0,
            Some(mimageviewer::shell_file_ops::ShellTransferOperation::Copy),
        )),
    );
}

fn snapshot_file_organize_modal(
    name: &str,
    theme: mimageviewer::os_theme::ResolvedTheme,
    size: egui::Vec2,
    sources: Vec<std::path::PathBuf>,
    destinations: Vec<mimageviewer::settings::FileOrganizeDestination>,
    mut focus: Option<(
        usize,
        Option<mimageviewer::shell_file_ops::ShellTransferOperation>,
    )>,
) {
    let mut fonts_ready = false;
    let mut harness = Harness::builder().with_size(size).build(move |ctx| {
        mimageviewer::os_theme::apply_resolved_with_contrast(
            ctx,
            theme,
            mimageviewer::settings::TextContrast::Standard,
        );
        if !fonts_ready {
            install_app_fonts(ctx);
            fonts_ready = true;
            ctx.request_repaint();
            return;
        }
        let _ = mimageviewer::ui_dialogs::file_organize::show_file_organize_modal(
            ctx,
            &sources,
            &destinations,
            &mut focus,
        );
    });
    harness.run();
    harness.snapshot(name);
}

#[test]
fn preferences_clipboard_capture_light() {
    snapshot_with_theme(
        "preferences_clipboard_capture_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| mimageviewer::draw_clipboard_capture_settings_snapshot_fixture(ui, false),
    );
}

#[test]
fn clipboard_html_settings_light() {
    snapshot_with_theme_contrast_and_size(
        "clipboard_html_settings_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(720.0, 400.0),
        |ui| mimageviewer::draw_clipboard_capture_html_settings_snapshot_fixture(ui, false),
    );
}

#[test]
fn clipboard_html_settings_dark() {
    snapshot_with_theme_contrast_and_size(
        "clipboard_html_settings_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(720.0, 400.0),
        |ui| mimageviewer::draw_clipboard_capture_html_settings_snapshot_fixture(ui, true),
    );
}

#[test]
fn clipboard_html_selection_fetching_light() {
    snapshot_with_theme_contrast_and_size(
        "clipboard_html_selection_fetching_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(760.0, 500.0),
        |ui| mimageviewer::draw_capture_selection_snapshot_fixture(ui, false),
    );
}

#[test]
fn clipboard_html_selection_saving_dark() {
    snapshot_with_theme_contrast_and_size(
        "clipboard_html_selection_saving_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::default(),
        egui::vec2(760.0, 500.0),
        |ui| mimageviewer::draw_capture_selection_snapshot_fixture(ui, true),
    );
}

#[test]
fn preferences_clipboard_capture_pending_default_light() {
    snapshot_with_theme(
        "preferences_clipboard_capture_pending_default_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::draw_clipboard_capture_settings_pending_snapshot_fixture,
    );
}

#[test]
fn preferences_clipboard_capture_failed_dark() {
    snapshot_with_theme(
        "preferences_clipboard_capture_failed_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| mimageviewer::draw_clipboard_capture_settings_snapshot_fixture(ui, true),
    );
}

/// テスト用に本体と同じフォント fallback を `ctx` に登録する。
/// これをしないと `豆腐` 文字だらけのスナップショットになり、ラベル・見出しや
/// 絵文字混じりテキストの実際のレイアウトを検証できない。
fn install_app_fonts(ctx: &egui::Context) {
    mimageviewer::ui_fonts::configure_fonts(ctx);
}

#[test]
fn offline_change_scan_setting_light() {
    let mut skip = false;
    snapshot_with_theme(
        "offline_change_scan_setting_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        move |ui| {
            mimageviewer::ui_helpers::draw_offline_change_scan_setting(ui, &mut skip);
            mimageviewer::ui_helpers::draw_index_full_check_button(ui);
        },
    );
}

#[test]
fn offline_change_scan_setting_dark() {
    let mut skip = true;
    snapshot_with_theme(
        "offline_change_scan_setting_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        move |ui| {
            mimageviewer::ui_helpers::draw_offline_change_scan_setting(ui, &mut skip);
            mimageviewer::ui_helpers::draw_index_full_check_button(ui);
        },
    );
}

/// テストハーネスのユーティリティ: 指定テーマで UI を描画し、`name` でスナップショットを取る。
fn snapshot_with_theme(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    build_ui: impl FnMut(&mut egui::Ui),
) {
    snapshot_with_theme_and_contrast(
        name,
        resolved,
        mimageviewer::settings::TextContrast::Standard,
        build_ui,
    );
}

fn snapshot_with_theme_and_contrast(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    contrast: mimageviewer::settings::TextContrast,
    build_ui: impl FnMut(&mut egui::Ui),
) {
    snapshot_with_theme_contrast_and_size(
        name,
        resolved,
        contrast,
        egui::vec2(480.0, 360.0),
        build_ui,
    );
}

fn snapshot_with_theme_contrast_and_size(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    contrast: mimageviewer::settings::TextContrast,
    size: egui::Vec2,
    build_ui: impl FnMut(&mut egui::Ui),
) {
    snapshot_with_theme_contrast_and_size_with_interaction(
        name,
        resolved,
        contrast,
        size,
        build_ui,
        |_| {},
    );
}

fn snapshot_with_theme_contrast_and_size_with_interaction(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    contrast: mimageviewer::settings::TextContrast,
    size: egui::Vec2,
    build_ui: impl FnMut(&mut egui::Ui),
    interact: impl FnOnce(&mut Harness<'_>),
) {
    snapshot_with_theme_options(name, resolved, contrast, size, None, build_ui, interact);
}

fn snapshot_with_theme_and_contrast_settling(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    contrast: mimageviewer::settings::TextContrast,
    animated_steps: Option<usize>,
    build_ui: impl FnMut(&mut egui::Ui),
) {
    snapshot_with_theme_options(
        name,
        resolved,
        contrast,
        egui::vec2(480.0, 360.0),
        animated_steps,
        build_ui,
        |_| {},
    );
}

fn snapshot_with_theme_options(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    contrast: mimageviewer::settings::TextContrast,
    size: egui::Vec2,
    animated_steps: Option<usize>,
    mut build_ui: impl FnMut(&mut egui::Ui),
    interact: impl FnOnce(&mut Harness<'_>),
) {
    let mut fonts_ready = false;
    let mut harness = Harness::builder().with_size(size).build(move |ctx| {
        mimageviewer::os_theme::apply_resolved_with_contrast(ctx, resolved, contrast);
        if !fonts_ready {
            install_app_fonts(ctx);
            fonts_ready = true;
            ctx.request_repaint();
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                egui::Frame::central_panel(ui.style())
                    .outer_margin(8.0)
                    .inner_margin(0.0)
                    .show(ui, |ui| build_ui(ui));
            });
    });
    if let Some(steps) = animated_steps {
        harness.run_steps(steps); // A production spinner intentionally never settles.
    } else {
        harness.run();
    }
    interact(&mut harness);
    harness.snapshot(name);
}

#[cfg(windows)]
fn snapshot_with_ui_font(
    name: &str,
    settings: mimageviewer::settings::UiFontSettings,
    mut build_ui: impl FnMut(&mut egui::Ui),
) {
    let mut fonts_ready = false;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(480.0, 260.0))
        .build(move |ctx| {
            mimageviewer::os_theme::apply_resolved(
                ctx,
                mimageviewer::os_theme::ResolvedTheme::Dark,
            );
            if !fonts_ready {
                mimageviewer::ui_fonts::configure_fonts_with_settings(ctx, &settings);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| build_ui(ui));
        });
    harness.run();
    harness.snapshot(name);
}

#[cfg(windows)]
fn windows_font_settings(
    display_name: &str,
    path: &str,
    family: &str,
) -> mimageviewer::settings::UiFontSettings {
    let mut db = fontdb::Database::new();
    db.load_font_file(path)
        .unwrap_or_else(|err| panic!("{display_name} should load from {path}: {err}"));
    let face = db
        .faces()
        .find(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
        })
        .unwrap_or_else(|| panic!("{path} should contain the {family} face"));
    mimageviewer::settings::UiFontSettings {
        selection: mimageviewer::settings::UiFontSelection::Face {
            display_name: display_name.to_owned(),
            path: std::path::PathBuf::from(path),
            face_index: face.index,
            post_script_name: face.post_script_name.clone(),
        },
        vertical_adjust: 0.0,
    }
}

#[cfg(windows)]
fn recommended_ui_font_fixture(ui: &mut egui::Ui, label: &str, typographic_points: f32) {
    // Windows の 9/10pt を 96 DPI 時の egui logical point へ換算する。
    let size = typographic_points * (96.0 / 72.0);
    let body_font = egui::FontId::new(size, egui::FontFamily::Proportional);
    let toolbar_font = egui::FontId::new(
        size,
        egui::FontFamily::Name(std::sync::Arc::<str>::from(
            mimageviewer::ui_fonts::TOOLBAR_TEXT_FAMILY_NAME,
        )),
    );

    ui.set_width(440.0);
    ui.heading(format!("{label}  {typographic_points}pt"));
    ui.label(egui::RichText::new("mImageViewer  表示サンプル  Aa 0123").font(body_font.clone()));
    ui.label(
        egui::RichText::new("日本語・簡体字测试・한글・💗・𝓈𝒸𝓇𝑒𝒶𝓂")
            .font(mimageviewer::ui_fonts::user_text_font(size)),
    );
    ui.add_space(8.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(430.0, 34.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new("フォルダー:").font(toolbar_font.clone()));
                let _ = ui.add_sized(
                    [68.0, 28.0],
                    egui::Button::new(egui::RichText::new("前へ").font(toolbar_font.clone())),
                );
                let _ = ui.add_sized(
                    [68.0, 28.0],
                    egui::Button::new(egui::RichText::new("次へ").font(toolbar_font)),
                );
            },
        );
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("動画・音声 HUD:").weak());
        let (speed_rect, _) = ui.allocate_exact_size(egui::vec2(43.0, 28.0), egui::Sense::hover());
        ui.painter().text(
            speed_rect.center() + egui::vec2(0.0, 4.0),
            egui::Align2::CENTER_CENTER,
            "x1",
            mimageviewer::ui_fonts::hud_text_font(12.0),
            egui::Color32::from_rgb(238, 238, 238),
        );
        let (norm_rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(norm_rect, 5.0, egui::Color32::from_rgb(55, 105, 170));
        ui.painter().text(
            norm_rect.center() + egui::vec2(0.0, 4.0),
            egui::Align2::CENTER_CENTER,
            "Norm",
            mimageviewer::ui_fonts::hud_text_font(11.0),
            egui::Color32::from_rgb(255, 198, 62),
        );
        ui.painter().text(
            egui::pos2(norm_rect.max.x + 68.0, norm_rect.center().y + 4.0),
            egui::Align2::RIGHT_CENTER,
            "0.0dB",
            mimageviewer::ui_fonts::hud_text_font(13.0),
            egui::Color32::from_rgb(238, 238, 238),
        );
        ui.add_space(72.0);
        ui.painter().text(
            egui::pos2(norm_rect.max.x + 82.0, norm_rect.center().y + 4.0),
            egui::Align2::LEFT_CENTER,
            "01:23 / 04:56",
            mimageviewer::ui_fonts::hud_text_font(14.0),
            egui::Color32::from_rgb(238, 238, 238),
        );
        ui.add_space(110.0);
    });
    ui.separator();
    ui.label(egui::RichText::new("実メトリクスから自動補正・手動調整 0pt").weak());
}

fn contrast_fixture(ui: &mut egui::Ui) {
    ui.set_min_width(440.0);
    ui.heading("文字コントラスト");
    ui.label("通常文字：ツールバーやメニューと共通の色です。");
    ui.label(egui::RichText::new("薄い文字：補足情報や件数表示です。").weak());
    ui.horizontal(|ui| {
        let _ = ui.button("通常ボタン");
        ui.add_enabled(false, egui::Button::new("無効ボタン"));
    });
    ui.label(egui::RichText::new("注意表示").color(ui.visuals().warn_fg_color));
    ui.label(egui::RichText::new("エラー表示").color(ui.visuals().error_fg_color));
}

#[test]
fn fullscreen_prefetch_direction_dark() {
    snapshot_with_theme(
        "fullscreen_prefetch_direction_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_fs_prefetch_indicator_snapshot_fixture,
    );
}

fn snapshot_with_theme_at_size(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    size: egui::Vec2,
    hover_pos: Option<egui::Pos2>,
    build_ui: impl FnMut(&mut egui::Ui),
) {
    snapshot_with_theme_contrast_and_size_with_interaction(
        name,
        resolved,
        mimageviewer::settings::TextContrast::Standard,
        size,
        build_ui,
        |harness| {
            if let Some(pos) = hover_pos {
                harness.hover_at(pos);
                harness.run();
            }
        },
    );
}

#[test]
fn details_icons_light() {
    snapshot_with_theme_at_size(
        "details_icons_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        egui::vec2(480.0, 480.0),
        Some(egui::pos2(28.0, 120.0)),
        mimageviewer::draw_details_icons_snapshot_fixture,
    );
}

#[test]
fn details_icons_dark() {
    snapshot_with_theme_at_size(
        "details_icons_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(480.0, 480.0),
        Some(egui::pos2(28.0, 120.0)),
        mimageviewer::draw_details_icons_snapshot_fixture,
    );
}

#[test]
fn fullscreen_page_wait_indicator_dark() {
    snapshot_with_theme(
        "fullscreen_page_wait_indicator_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_fs_page_wait_indicator_snapshot_fixture,
    );
}

#[test]
fn preferences_video_bar_visibility_dark() {
    snapshot_with_theme(
        "preferences_video_bar_visibility_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::draw_video_bar_visibility_snapshot_fixture(ui);
        },
    );
}

#[test]
#[cfg(not(feature = "portable"))]
fn preferences_effetune_input_limit_dark() {
    snapshot_with_theme(
        "preferences_effetune_input_limit_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_effetune_input_limit_snapshot_fixture,
    );
}

#[test]
fn preferences_book_resume_meter_light() {
    snapshot_with_theme(
        "preferences_book_resume_meter_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::draw_book_resume_meter_settings_snapshot_fixture(ui);
        },
    );
}

#[test]
fn preferences_book_resume_meter_dark() {
    snapshot_with_theme(
        "preferences_book_resume_meter_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::draw_book_resume_meter_settings_snapshot_fixture(ui);
        },
    );
}

#[test]
fn preferences_video_thumbnail_indicator_dark() {
    snapshot_with_theme(
        "preferences_video_thumbnail_indicator_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::draw_video_thumbnail_indicator_settings_snapshot_fixture(ui);
        },
    );
}

#[test]
fn preferences_media_thumbnail_sources_light() {
    snapshot_with_theme(
        "preferences_media_thumbnail_sources_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::draw_video_thumbnail_indicator_settings_snapshot_fixture(ui);
        },
    );
}

#[test]
fn metadata_panel_information_tab_dark() {
    snapshot_with_theme_at_size(
        "metadata_panel_information_tab_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(400.0, 260.0),
        None,
        mimageviewer::draw_paused_metadata_panel_snapshot_fixture,
    );
}

#[test]
fn metadata_panel_comfyui_provenance_dark() {
    snapshot_with_theme_at_size(
        "metadata_panel_comfyui_provenance_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(400.0, 390.0),
        None,
        mimageviewer::draw_comfyui_provenance_snapshot_fixture,
    );
}

#[test]
fn metadata_panel_similar_results_dark() {
    snapshot_with_theme_at_size(
        "metadata_panel_similar_results_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        // スクロール領域を保ちつつ、本名・識別用パス・明示的な [移動]・帯と次の候補まで
        // 一枚で確認できる高さにする。情報タブ側は収まるため、2 枚で溝の有無も比較できる。
        egui::vec2(400.0, 800.0),
        None,
        |ui| mimageviewer::draw_similar_panel_snapshot_fixture(ui, true),
    );
}

#[test]
fn metadata_panel_similar_states_dark() {
    snapshot_with_theme_at_size(
        "metadata_panel_similar_states_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(400.0, 640.0),
        None,
        mimageviewer::draw_similar_states_snapshot_fixture,
    );
}

#[test]
fn fullscreen_fit_cycle_single_mode_dark() {
    snapshot_with_theme_at_size(
        "fullscreen_fit_cycle_single_mode_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(480.0, 300.0),
        None,
        mimageviewer::draw_fullscreen_fit_cycle_settings_snapshot_fixture,
    );
}

#[test]
fn preferences_favorite_view_state_dark() {
    snapshot_with_theme_at_size(
        "preferences_favorite_view_state_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(620.0, 260.0),
        None,
        |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.set_width(580.0);
                mimageviewer::draw_favorite_view_state_settings_snapshot_fixture(ui);
            });
        },
    );
}

#[test]
fn video_thumbnail_indicator_modes_and_dense_badges_dark() {
    snapshot_with_theme_at_size(
        "video_thumbnail_indicator_modes_and_dense_badges_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(480.0, 420.0),
        // The pointer is inside the narrow cell's top-left tag badge, exercising the real hover
        // tooltip branch while the screenshot also covers its dense bottom-left badge stack.
        Some(egui::pos2(274.0, 190.0)),
        mimageviewer::draw_video_thumbnail_indicator_snapshot_fixture,
    );
}

#[test]
fn cut_item_appearance_dark() {
    snapshot_with_theme_at_size(
        "cut_item_appearance_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(500.0, 350.0),
        None,
        mimageviewer::draw_cut_item_appearance_snapshot_fixture,
    );
}

#[test]
fn cut_item_appearance_light() {
    snapshot_with_theme_at_size(
        "cut_item_appearance_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        egui::vec2(500.0, 350.0),
        None,
        mimageviewer::draw_cut_item_appearance_snapshot_fixture,
    );
}

#[test]
fn collection_placeholder_dark() {
    snapshot_with_theme_at_size(
        "collection_placeholder_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(500.0, 180.0),
        None,
        mimageviewer::draw_collection_placeholder_snapshot_fixture,
    );
}

#[test]
fn collection_placeholder_light() {
    snapshot_with_theme_at_size(
        "collection_placeholder_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        egui::vec2(500.0, 180.0),
        None,
        mimageviewer::draw_collection_placeholder_snapshot_fixture,
    );
}

#[test]
fn still_touch_panel_handles_latched_dark() {
    snapshot_with_theme(
        "still_touch_panel_handles_latched_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::draw_still_panel_reach_snapshot_fixture(ui, true, false, false, false);
        },
    );
}

#[test]
fn still_seek_strip_and_hover_preview_dark() {
    snapshot_with_theme(
        "still_seek_strip_and_hover_preview_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::draw_still_seek_strip_snapshot_fixture,
    );
}

#[test]
fn music_touch_panel_handles_observed_dark() {
    snapshot_with_theme(
        "music_touch_panel_handles_observed_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::draw_music_panel_reach_snapshot_fixture(ui, true, false, false);
        },
    );
}

#[test]
fn still_touch_first_run_help_unlearned_100_dark() {
    snapshot_with_theme(
        "still_touch_first_run_help_unlearned_100_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::draw_still_touch_first_run_help_snapshot_fixture(ui, false, 1.0);
        },
    );
}

#[test]
fn still_touch_first_run_help_learned_hidden_dark() {
    snapshot_with_theme(
        "still_touch_first_run_help_learned_hidden_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::draw_still_touch_first_run_help_snapshot_fixture(ui, true, 1.0);
        },
    );
}

#[cfg(windows)]
#[test]
fn native_video_touch_first_run_help_unlearned_100_dark() {
    snapshot_with_theme(
        "native_video_touch_first_run_help_unlearned_100_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::video::native_presenter::draw_native_video_touch_first_run_help_snapshot_fixture(
                ui, false, 1.0,
            );
        },
    );
}

#[cfg(windows)]
#[test]
fn native_video_touch_first_run_help_learned_hidden_dark() {
    snapshot_with_theme(
        "native_video_touch_first_run_help_learned_hidden_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::video::native_presenter::draw_native_video_touch_first_run_help_snapshot_fixture(
                ui, true, 1.0,
            );
        },
    );
}

#[test]
fn still_touch_panel_handles_unlatched_dark() {
    snapshot_with_theme(
        "still_touch_panel_handles_unlatched_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::draw_still_panel_reach_snapshot_fixture(ui, false, false, false, false);
        },
    );
}

#[test]
fn still_mouse_panel_callout_dark() {
    snapshot_with_theme(
        "still_mouse_panel_callout_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::draw_still_panel_reach_snapshot_fixture(ui, false, true, false, false);
        },
    );
}

#[test]
fn still_touch_left_panel_open_has_only_right_handle_dark() {
    snapshot_with_theme(
        "still_touch_left_panel_open_has_only_right_handle_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            mimageviewer::draw_still_panel_reach_snapshot_fixture(ui, true, false, true, false);
        },
    );
}

#[test]
fn text_contrast_strong_light() {
    snapshot_with_theme_and_contrast(
        "text_contrast_strong_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::settings::TextContrast::Strong,
        contrast_fixture,
    );
}

#[test]
fn text_contrast_strong_dark() {
    snapshot_with_theme_and_contrast(
        "text_contrast_strong_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::Strong,
        contrast_fixture,
    );
}

/// シンプルなラベル+ボタンを Light テーマで描画して、基盤が動くことを確認する
/// スモークテスト。
#[test]
fn smoke_label_and_button_light() {
    snapshot_with_theme(
        "smoke_label_and_button_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.heading("mImageViewer");
            ui.label("UI スナップショット基盤のスモークテストです。");
            ui.separator();
            let _ = ui.button("OK");
        },
    );
}

/// 同じ UI を Dark テーマで描画。Light/Dark で差が出ることを目視確認用に保存しておく。
#[test]
fn smoke_label_and_button_dark() {
    snapshot_with_theme(
        "smoke_label_and_button_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.heading("mImageViewer");
            ui.label("UI スナップショット基盤のスモークテストです。");
            ui.separator();
            let _ = ui.button("OK");
        },
    );
}

#[test]
fn metadata_text_fallback_emoji_symbols_dark() {
    snapshot_with_theme(
        "metadata_text_fallback_emoji_symbols_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.set_width(440.0);
            ui.label(egui::RichText::new("説明").strong());
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("愛💗𝓈𝒸𝓇𝑒𝒶𝓂…𝓈𝒸𝓇𝑒𝒶𝓂…💗")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("🍧original  🧠今までのおうたの再生リスト")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("🐾今までのおうたの再生リスト")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("CJK mix: 简体字测试 / 繁體字測試 / 日本語 / 한글")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("✉Contact form")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("★お気に入り  ♪BGM  ※注釈  ☎Info")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("🧠今までのおうたの再生リスト")
                    .font(mimageviewer::ui_fonts::user_text_font(12.0)),
            );
            ui.label(
                egui::RichText::new("⋈ -------------------------------- ⋈")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let _ = ui.button("🔖");
                let _ = ui.button("✏");
                let _ = ui.button("↻ プラグインを再読み込み");
            });
        },
    );
}

/// v2.7.0: 任意 UI フォントを設定した場合の通常文字・ツールバー文字と、
/// 日本語 / 記号 fallback の縦位置を同時に固定する。
#[cfg(windows)]
#[test]
fn custom_ui_font_meiryo_bold_alignment_dark() {
    let settings = mimageviewer::settings::UiFontSettings {
        selection: mimageviewer::settings::UiFontSelection::Face {
            display_name: "Meiryo Bold".to_string(),
            path: std::path::PathBuf::from(r"C:\Windows\Fonts\meiryob.ttc"),
            face_index: 0,
            post_script_name: String::new(),
        },
        vertical_adjust: 0.75,
    };
    snapshot_with_ui_font(
        "custom_ui_font_meiryo_bold_alignment_dark",
        settings,
        |ui| {
            ui.set_width(440.0);
            ui.heading("UI フォント");
            ui.label("mImageViewer  表示サンプル  Aa 0123");
            ui.label(
                egui::RichText::new("日本語・簡体字测试・한글・💗・𝓈𝒸𝓇𝑒𝒶𝓂")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let toolbar_font = egui::FontId::new(
                    14.0,
                    egui::FontFamily::Name(std::sync::Arc::<str>::from(
                        mimageviewer::ui_fonts::TOOLBAR_TEXT_FAMILY_NAME,
                    )),
                );
                ui.label(egui::RichText::new("フォルダー:").font(toolbar_font.clone()));
                let _ = ui.button(egui::RichText::new("前へ").font(toolbar_font.clone()));
                let _ = ui.button(egui::RichText::new("次へ").font(toolbar_font));
            });
            ui.separator();
            ui.label(egui::RichText::new("自動補正 + 0.75 pt").weak());
        },
    );
}

#[cfg(windows)]
#[test]
fn recommended_ui_font_biz_udp_gothic_9pt_alignment_dark() {
    snapshot_with_ui_font(
        "recommended_ui_font_biz_udp_gothic_9pt_alignment_dark",
        windows_font_settings(
            "BIZ UDPGothic",
            r"C:\Windows\Fonts\BIZ-UDGothicR.ttc",
            "BIZ UDPGothic",
        ),
        |ui| recommended_ui_font_fixture(ui, "BIZ UDPGothic", 9.0),
    );
}

#[cfg(windows)]
#[test]
fn recommended_ui_font_meiryo_10pt_alignment_dark() {
    snapshot_with_ui_font(
        "recommended_ui_font_meiryo_10pt_alignment_dark",
        windows_font_settings("Meiryo", r"C:\Windows\Fonts\meiryo.ttc", "Meiryo"),
        |ui| recommended_ui_font_fixture(ui, "Meiryo", 10.0),
    );
}

#[cfg(windows)]
#[test]
fn recommended_ui_font_meiryo_ui_10pt_alignment_dark() {
    snapshot_with_ui_font(
        "recommended_ui_font_meiryo_ui_10pt_alignment_dark",
        windows_font_settings("Meiryo UI", r"C:\Windows\Fonts\meiryo.ttc", "Meiryo UI"),
        |ui| recommended_ui_font_fixture(ui, "Meiryo UI", 10.0),
    );
}

/// `draw_cell_filename` のフォント family が `miv-user-text` であることの回帰防止。
/// 絵文字 (💎) / 数学英字 (𝓈𝒸𝓇𝑒𝒶𝓂) / 日本語 / ASCII を 1 つのラベルに混ぜて、
/// ベースラインのずれが PNG 差分として検知できる状態にする。
#[test]
fn cell_filename_mixed_glyphs_dark() {
    snapshot_with_theme(
        "cell_filename_mixed_glyphs_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            let (response, painter) =
                ui.allocate_painter(egui::vec2(220.0, 160.0), egui::Sense::hover());
            let cell = response.rect;
            let inner = cell.shrink(4.0);
            painter.rect_filled(inner, 2.0, egui::Color32::from_gray(60));
            let empty_tags = Vec::new();
            let layout = mimageviewer::thumb_overlay_layout::layout_thumbnail_overlays(
                mimageviewer::thumb_overlay_layout::ThumbnailOverlayLayoutInput {
                    cell,
                    inner,
                    book_resume_meter: false,
                    checked: false,
                    stack_count: None,
                    filter_match_count: None,
                    media_duration: None,
                    bookmark_time: None,
                    upscaled_video: false,
                    edit_badges: Default::default(),
                    tags: &empty_tags,
                    bottom_container: None,
                    rating_text: None,
                    filename: Some("001 - お返事まだカナ💎𝓈𝒸𝓇𝑒𝒶𝓂おじ"),
                },
                |text, style| {
                    mimageviewer::ui_helpers::measure_thumbnail_badge_text(&painter, text, style)
                },
            );
            let placement = layout.bottom_left.filename.as_ref().expect("filename");
            mimageviewer::ui_helpers::draw_cell_filename(
                &painter,
                placement,
                egui::Color32::WHITE,
                true,
            );
        },
    );
}

/// ZIP / PDF / RAR 等の形式バッジが、中央の代替アイコンを変えずに左下へコンパクトに
/// 収まり、ファイル名プレートとも重ならないことを確認する。
#[test]
fn compact_file_format_badges_light() {
    snapshot_with_theme(
        "compact_file_format_badges_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.set_width(440.0);
            let draw_cell = |ui: &mut egui::Ui,
                             icon: &str,
                             name: &str,
                             label: &str,
                             kind: mimageviewer::thumb_overlay_layout::BottomContainerKind,
                             rating: Option<&str>| {
                let (response, painter) =
                    ui.allocate_painter(egui::vec2(210.0, 145.0), egui::Sense::hover());
                let cell = response.rect;
                let inner = cell.shrink(4.0);
                painter.rect_filled(inner, 3.0, egui::Color32::from_gray(228));
                painter.text(
                    inner.center() - egui::vec2(0.0, 10.0),
                    egui::Align2::CENTER_CENTER,
                    icon,
                    egui::FontId::proportional(32.0),
                    egui::Color32::from_gray(70),
                );
                let empty_tags = Vec::new();
                let filename = matches!(
                    kind,
                    mimageviewer::thumb_overlay_layout::BottomContainerKind::Format(_)
                )
                .then_some(name);
                let layout = mimageviewer::thumb_overlay_layout::layout_thumbnail_overlays(
                    mimageviewer::thumb_overlay_layout::ThumbnailOverlayLayoutInput {
                        cell,
                        inner,
                        book_resume_meter: false,
                        checked: false,
                        stack_count: None,
                        filter_match_count: None,
                        media_duration: None,
                        bookmark_time: None,
                        upscaled_video: false,
                        edit_badges: Default::default(),
                        tags: &empty_tags,
                        bottom_container: Some(
                            mimageviewer::thumb_overlay_layout::BottomContainerInput {
                                kind,
                                label,
                            },
                        ),
                        rating_text: rating,
                        filename,
                    },
                    |text, style| {
                        mimageviewer::ui_helpers::measure_thumbnail_badge_text(
                            &painter, text, style,
                        )
                    },
                );
                let container = layout.bottom_left.container.as_ref().expect("container");
                match kind {
                    mimageviewer::thumb_overlay_layout::BottomContainerKind::Folder => {
                        mimageviewer::ui_helpers::draw_overlay_folder_badge(&painter, container);
                    }
                    mimageviewer::thumb_overlay_layout::BottomContainerKind::Format(
                        format_kind,
                    ) => {
                        mimageviewer::ui_helpers::draw_overlay_format_badge(
                            &painter,
                            container,
                            format_kind,
                        );
                    }
                }
                if let Some(filename) = layout.bottom_left.filename.as_ref() {
                    mimageviewer::ui_helpers::draw_cell_filename(
                        &painter,
                        filename,
                        egui::Color32::from_gray(35),
                        false,
                    );
                }
                if let Some(rating) = layout.bottom_left.rating.as_ref() {
                    mimageviewer::ui_helpers::draw_overlay_rating_badge(&painter, rating, true);
                }
            };
            ui.horizontal(|ui| {
                draw_cell(
                    ui,
                    "📁",
                    "とても長い日本語フォルダー名",
                    "とても長い日本語フォルダー名",
                    mimageviewer::thumb_overlay_layout::BottomContainerKind::Folder,
                    Some("📁★★★★★"),
                );
                draw_cell(
                    ui,
                    "📦",
                    "comic-book-long-name.zip",
                    "ZIP",
                    mimageviewer::thumb_overlay_layout::BottomContainerKind::Format(
                        mimageviewer::thumb_overlay_layout::FormatBadgeKind::Zip,
                    ),
                    None,
                );
            });
            ui.horizontal(|ui| {
                draw_cell(
                    ui,
                    "📄",
                    "document-long-name.pdf",
                    "PDF",
                    mimageviewer::thumb_overlay_layout::BottomContainerKind::Format(
                        mimageviewer::thumb_overlay_layout::FormatBadgeKind::Pdf,
                    ),
                    Some("📁★★★★"),
                );
                draw_cell(
                    ui,
                    "🗜",
                    "archive-long-name.rar",
                    "RAR",
                    mimageviewer::thumb_overlay_layout::BottomContainerKind::Format(
                        mimageviewer::thumb_overlay_layout::FormatBadgeKind::Archive,
                    ),
                    None,
                );
            });
        },
    );
}

/// 動画セルはコンテナバッジを持たず、ファイル名プレートは中央寄せなので、評価は左下の角に
/// 残る。無条件に 1 段上げると星が絵の中に浮いて見える (2026-07-31 の実機報告)。
#[test]
fn rating_shares_the_bottom_row_with_a_centred_filename_dark() {
    snapshot_with_theme(
        "rating_with_centred_filename_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            let (response, painter) =
                ui.allocate_painter(egui::vec2(300.0, 170.0), egui::Sense::hover());
            let cell = response.rect;
            let inner = cell.shrink(4.0);
            painter.rect_filled(inner, 3.0, egui::Color32::from_gray(58));
            let empty_tags = Vec::new();
            let layout = mimageviewer::thumb_overlay_layout::layout_thumbnail_overlays(
                mimageviewer::thumb_overlay_layout::ThumbnailOverlayLayoutInput {
                    cell,
                    inner,
                    book_resume_meter: false,
                    checked: false,
                    stack_count: None,
                    filter_match_count: None,
                    media_duration: None,
                    bookmark_time: Some("0:25"),
                    upscaled_video: false,
                    edit_badges: Default::default(),
                    tags: &empty_tags,
                    bottom_container: None,
                    rating_text: Some("★★★"),
                    filename: Some("「名探偵」エンディング.mp4"),
                },
                |text, style| {
                    mimageviewer::ui_helpers::measure_thumbnail_badge_text(&painter, text, style)
                },
            );
            if let Some(time) = layout.top_left.bookmark_time.as_ref() {
                mimageviewer::ui_helpers::draw_overlay_bookmark_time_badge(&painter, time, false);
            }
            if let Some(filename) = layout.bottom_left.filename.as_ref() {
                mimageviewer::ui_helpers::draw_cell_filename(
                    &painter,
                    filename,
                    egui::Color32::from_gray(230),
                    true,
                );
            }
            if let Some(rating) = layout.bottom_left.rating.as_ref() {
                mimageviewer::ui_helpers::draw_overlay_rating_badge(&painter, rating, false);
            }
        },
    );
}

fn media_duration_badges_fixture(ui: &mut egui::Ui) {
    use mimageviewer::thumb_overlay_layout::{
        BadgeKind, BottomContainerInput, BottomContainerKind, EditBadgeFlags, FormatBadgeKind,
        ThumbnailOverlayLayoutInput, layout_thumbnail_overlays,
    };
    let dark = ui.visuals().dark_mode;
    let draw_cell = |ui: &mut egui::Ui,
                     width: f32,
                     duration: &str,
                     count: Option<u32>,
                     dense: bool| {
        let (response, painter) =
            ui.allocate_painter(egui::vec2(width, width.min(120.0)), egui::Sense::hover());
        let cell = response.rect;
        let inner = cell.shrink(4.0);
        painter.rect_filled(
            inner,
            3.0,
            egui::Color32::from_gray(if dark { 58 } else { 215 }),
        );
        let layout = layout_thumbnail_overlays(
            ThumbnailOverlayLayoutInput {
                cell,
                inner,
                book_resume_meter: false,
                checked: false,
                stack_count: None,
                filter_match_count: count,
                media_duration: Some(duration),
                bookmark_time: None,
                upscaled_video: dense,
                edit_badges: EditBadgeFlags {
                    crop: dense,
                    pin: dense,
                    ..Default::default()
                },
                tags: &[],
                bottom_container: dense.then_some(BottomContainerInput {
                    kind: BottomContainerKind::Format(FormatBadgeKind::Video),
                    label: "MOV",
                }),
                rating_text: dense.then_some("★★★"),
                filename: dense.then_some("holiday.mp4"),
            },
            |text, style| {
                mimageviewer::ui_helpers::measure_thumbnail_badge_text(&painter, text, style)
            },
        );
        for placement in layout.badge_placements() {
            match placement.kind {
                BadgeKind::MediaDuration => {
                    mimageviewer::ui_helpers::draw_overlay_media_duration_badge(&painter, placement)
                }
                BadgeKind::UpscaledVideo => {
                    mimageviewer::ui_helpers::draw_overlay_upscaled_video_badge(&painter, placement)
                }
                BadgeKind::Edit(kind) => {
                    mimageviewer::ui_helpers::draw_overlay_edit_badge(&painter, placement, kind)
                }
                BadgeKind::EditOverflow => {
                    mimageviewer::ui_helpers::draw_overlay_edit_overflow_badge(&painter, placement)
                }
                BadgeKind::BottomContainer(BottomContainerKind::Format(kind)) => {
                    mimageviewer::ui_helpers::draw_overlay_format_badge(&painter, placement, kind)
                }
                BadgeKind::Rating => {
                    mimageviewer::ui_helpers::draw_overlay_rating_badge(&painter, placement, false)
                }
                BadgeKind::Filename => mimageviewer::ui_helpers::draw_cell_filename(
                    &painter,
                    placement,
                    ui.visuals().text_color(),
                    dark,
                ),
                BadgeKind::FilterMatchCount => {
                    painter.rect_filled(
                        placement.rect,
                        3.0,
                        egui::Color32::from_rgb(0xE6, 0x7E, 0x22),
                    );
                    painter.text(
                        placement.text_pos(),
                        egui::Align2::LEFT_TOP,
                        &placement.text,
                        egui::FontId::proportional(placement.style.font_size),
                        egui::Color32::WHITE,
                    );
                }
                _ => {}
            }
        }
    };
    ui.label("Media duration: short and long");
    ui.horizontal(|ui| {
        draw_cell(ui, 200.0, "0:07", None, false);
        draw_cell(ui, 200.0, "100:02:03", None, false);
    });
    ui.label("Filter count owns the corner; small cells omit duration");
    ui.horizontal(|ui| {
        draw_cell(ui, 200.0, "1:02:03", Some(42), true);
        draw_cell(ui, 100.0, "1:02:03", Some(42), true);
        draw_cell(ui, 32.0, "1:02:03", Some(42), true);
    });
}

#[test]
fn media_duration_badges_dark() {
    snapshot_with_theme(
        "media_duration_badges_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        media_duration_badges_fixture,
    );
}

#[test]
fn media_duration_badges_light() {
    snapshot_with_theme(
        "media_duration_badges_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        media_duration_badges_fixture,
    );
}

#[test]
fn bookmark_time_and_tag_badges_dark() {
    snapshot_with_theme(
        "bookmark_time_and_tag_badges_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            let (response, painter) =
                ui.allocate_painter(egui::vec2(420.0, 180.0), egui::Sense::hover());
            let cell = response.rect;
            let inner = cell.shrink(4.0);
            painter.rect_filled(inner, 3.0, egui::Color32::from_gray(55));
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                "ブックマークタイトル",
                mimageviewer::ui_fonts::user_text_font(13.0),
                egui::Color32::WHITE,
            );
            let tags = vec!["#長い日本語タグ".to_owned(), "旅行".to_owned()];
            let layout = mimageviewer::thumb_overlay_layout::layout_thumbnail_overlays(
                mimageviewer::thumb_overlay_layout::ThumbnailOverlayLayoutInput {
                    cell,
                    inner,
                    book_resume_meter: false,
                    checked: false,
                    stack_count: None,
                    filter_match_count: None,
                    media_duration: None,
                    bookmark_time: Some("12:34"),
                    upscaled_video: false,
                    edit_badges: mimageviewer::thumb_overlay_layout::EditBadgeFlags {
                        page_override: true,
                        crop: true,
                        pin: true,
                        ..Default::default()
                    },
                    tags: &tags,
                    bottom_container: None,
                    rating_text: None,
                    filename: None,
                },
                |text, style| {
                    mimageviewer::ui_helpers::measure_thumbnail_badge_text(&painter, text, style)
                },
            );
            mimageviewer::ui_helpers::draw_overlay_bookmark_time_badge(
                &painter,
                layout.top_left.bookmark_time.as_ref().expect("time"),
                false,
            );
            for placement in &layout.top_left.edit_badges {
                let mimageviewer::thumb_overlay_layout::BadgeKind::Edit(kind) = placement.kind
                else {
                    continue;
                };
                mimageviewer::ui_helpers::draw_overlay_edit_badge(&painter, placement, kind);
            }
            mimageviewer::ui_helpers::draw_overlay_tag_badge(
                &painter,
                layout.top_left.tag.as_ref().expect("tag"),
            );
        },
    );
}

#[test]
fn stats_histogram_compact_columns_light() {
    snapshot_with_theme(
        "stats_histogram_compact_columns_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.set_width(440.0);
            ui.heading("読み込み時間 (decode + display)");
            ui.add_space(4.0);

            let mut hist = [0_u64; mimageviewer::stats::LOAD_TIME_BUCKETS];
            for (bucket, count) in [
                47, 6, 6, 0, 0, 1, 2, 0, 4, 3, 4, 2, 2, 0, 0, 0, 0, 1, 0, 0, 9,
            ]
            .into_iter()
            .enumerate()
            {
                hist[bucket] = count;
            }

            mimageviewer::ui_helpers::draw_histogram(
                ui,
                &hist,
                mimageviewer::stats::ThumbStats::load_time_label,
                None,
            );
        },
    );
}

/// 更新後「重要な変更点」ダイアログ本体 (version_highlights::render) の回帰防止。
/// 複数バージョンまたぎの合成 payload を食わせて、必読 (⚠) / 新機能 (・) の 2 段構成と
/// バージョン見出しのレイアウト崩れを実機なしで検知する (docs/version-highlights-plan.md §5)。
#[test]
fn whats_new_dialog_multi_version_dark() {
    use mimageviewer::version_highlights::{HighlightItem, VersionHighlights};
    const MUST_15: &[HighlightItem] = &[HighlightItem {
        title: "操作の既定が変わりました",
        body: "従来の動作は設定から選べます。",
    }];
    const MUST_20: &[HighlightItem] = &[HighlightItem {
        title: "ツールバーの設定は右クリックに変わりました",
        body: "ツールバーを右クリックして表示項目・並び順・表示形式を変更します。",
    }];
    const HIGH_20: &[HighlightItem] = &[HighlightItem {
        title: "よく使う本をツールバーにピン留め",
        body: "本棚の管理画面で本を固定すると、ツールバーにボタンが並びます。",
    }];
    const V15: VersionHighlights = VersionHighlights {
        version: "1.5.0",
        must_read: MUST_15,
        highlights: &[],
    };
    const V20: VersionHighlights = VersionHighlights {
        version: "2.0.0",
        must_read: MUST_20,
        highlights: HIGH_20,
    };
    // ダイアログは新しいバージョンを上に出す (= 更新履歴と同じ並び)。テストもその順で渡す。
    let entries = [&V20, &V15];
    snapshot_with_theme(
        "whats_new_multi_version_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        move |ui| {
            ui.set_width(440.0);
            mimageviewer::version_highlights::render(ui, &entries);
        },
    );
}

#[test]
fn network_data_dir_notice_dark() {
    snapshot_with_theme(
        "network_data_dir_notice_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::ui_dialogs::network_data_dir_notice::render_network_data_dir_notice_content(
                ui,
                std::path::Path::new(r"\\NAS\photos\mImageViewer\data"),
            );
        },
    );
}

// ---------------------------------------------------------------------------
// Susie 診断 UI (PoolStatus 各バリアントのレンダリング) のスナップショット
// ---------------------------------------------------------------------------

use mimageviewer::susie_loader::{PluginInfo, PoolStatus, SusieWorkerHealth};
use mimageviewer::ui_susie_diagnostic::render_diagnostic;
use std::path::PathBuf;

fn snapshot_diagnostic_themed(
    name: &str,
    theme: mimageviewer::os_theme::ResolvedTheme,
    status: PoolStatus,
    plugins: Vec<PluginInfo>,
) {
    snapshot_with_theme(name, theme, move |ui| {
        ui.label(egui::RichText::new("ロード済みプラグイン").strong());
        ui.add_space(4.0);
        render_diagnostic(ui, &status, &plugins);
    });
}

fn snapshot_diagnostic(name: &str, status: PoolStatus, plugins: Vec<PluginInfo>) {
    snapshot_diagnostic_themed(
        name,
        mimageviewer::os_theme::ResolvedTheme::Light,
        status,
        plugins,
    );
}

#[test]
fn susie_diagnostic_disabled_by_settings() {
    snapshot_diagnostic(
        "susie_diagnostic_disabled",
        PoolStatus::DisabledBySettings,
        Vec::new(),
    );
}

#[test]
fn susie_diagnostic_not_initialized() {
    snapshot_diagnostic(
        "susie_diagnostic_not_initialized",
        PoolStatus::NotInitialized,
        Vec::new(),
    );
}

#[test]
fn susie_diagnostic_worker_missing() {
    snapshot_diagnostic(
        "susie_diagnostic_worker_missing",
        PoolStatus::WorkerExeMissing {
            expected_path: PathBuf::from(
                "C:\\Users\\example\\AppData\\Roaming\\mimageviewer\\mimageviewer-susie32.exe",
            ),
        },
        Vec::new(),
    );
}

#[test]
fn susie_diagnostic_worker_spawn_failed() {
    snapshot_diagnostic(
        "susie_diagnostic_worker_spawn_failed",
        PoolStatus::WorkerSpawnFailed,
        Vec::new(),
    );
}

#[test]
fn susie_diagnostic_ready_but_empty() {
    snapshot_diagnostic(
        "susie_diagnostic_ready_but_empty",
        PoolStatus::ReadyButEmpty,
        Vec::new(),
    );
}

fn ready_with_plugins_fixture() -> Vec<PluginInfo> {
    // レトロ専用 (本体優先がない) プラグイン + シャドウありのプラグインを混在させ、
    // 「⚠」マーカーと「本体優先」バッジ・注記が両方表示されるケースをカバーする。
    vec![
        PluginInfo {
            name: "ifpi.spi (PC-98 PI)".to_string(),
            extensions: vec!["pi".to_string()],
        },
        PluginInfo {
            name: "ifmag.spi (PC-98 MAG)".to_string(),
            extensions: vec!["mag".to_string()],
        },
        PluginInfo {
            name: "ifjpegt.spi (JPEG 再実装)".to_string(),
            extensions: vec!["jpg".to_string(), "jpeg".to_string()],
        },
    ]
}

/// 何も起きていない状態。復帰履歴は出ない。
fn healthy_workers() -> SusieWorkerHealth {
    SusieWorkerHealth {
        started_workers: 3,
        live_workers: 3,
        ..SusieWorkerHealth::default()
    }
}

#[test]
fn susie_diagnostic_ready_with_plugins() {
    let plugins = ready_with_plugins_fixture();
    snapshot_diagnostic(
        "susie_diagnostic_ready_with_plugins",
        PoolStatus::ReadyWithPlugins {
            count: plugins.len(),
            health: healthy_workers(),
        },
        plugins,
    );
}

/// クラッシュから復帰した後。プラグイン一覧の下に履歴が付く。
#[test]
fn susie_diagnostic_ready_after_recovery() {
    let plugins = ready_with_plugins_fixture();
    snapshot_diagnostic(
        "susie_diagnostic_ready_after_recovery",
        PoolStatus::ReadyWithPlugins {
            count: plugins.len(),
            health: SusieWorkerHealth {
                started_workers: 3,
                live_workers: 2,
                restarts: 4,
                gave_up_workers: 1,
                crashing_subjects: 2,
                last_failure: Some("unexpected end of file".to_string()),
            },
        },
        plugins,
    );
}

/// 枠を使い切った後。以前はこれが「起動に失敗しました」と表示されていた。
#[test]
fn susie_diagnostic_workers_exhausted() {
    snapshot_diagnostic(
        "susie_diagnostic_workers_exhausted",
        PoolStatus::WorkersExhausted {
            health: SusieWorkerHealth {
                started_workers: 3,
                live_workers: 0,
                restarts: 15,
                gave_up_workers: 3,
                crashing_subjects: 3,
                last_failure: Some("unexpected end of file".to_string()),
            },
        },
        Vec::new(),
    );
}

/// Light / Dark でも診断 UI が破綻せず読めることを確認する。
#[test]
fn susie_diagnostic_ready_with_plugins_dark() {
    let plugins = ready_with_plugins_fixture();
    snapshot_diagnostic_themed(
        "susie_diagnostic_ready_with_plugins_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        PoolStatus::ReadyWithPlugins {
            count: plugins.len(),
            health: healthy_workers(),
        },
        plugins,
    );
}

// ---------------------------------------------------------------------------
// 更新履歴 (GitHub release body) の Markdown 描画スナップショット
// ---------------------------------------------------------------------------

/// バージョン更新ダイアログに表示する release body の代表サンプル。
/// 見出し / 箇条書き / ネスト / `**強調**` / `` `コード` `` / `<kbd>キー</kbd>` を網羅し、
/// 整形描画 ([mimageviewer::changelog_markdown]) の見た目を回帰検出できるようにする。
fn changelog_body_fixture() -> &'static str {
    "### v0.9.1\n\
     - **キャプチャ保存**: 画像フルスクリーン中に <kbd>Ctrl</kbd>+<kbd>S</kbd> を押すと、\
     表示中の画像を保存できます。保存形式は環境設定の `キャプチャ保存` ページで設定します\n\
     - **比較ビュー**: <kbd>X</kbd> でピン留めし、<kbd>C</kbd> でトグル表示します\n\
     \u{0020}\u{0020}- <kbd>Shift</kbd>+<kbd>C</kbd> で左右に並べたワイプ比較\n\
     - 設定ファイル `settings.db` は初回起動時に自動移行されます"
}

#[test]
fn changelog_markdown_light() {
    snapshot_with_theme(
        "changelog_markdown_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::changelog_markdown::render(ui, changelog_body_fixture());
        },
    );
}

#[test]
fn changelog_markdown_dark() {
    snapshot_with_theme(
        "changelog_markdown_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::changelog_markdown::render(ui, changelog_body_fixture());
        },
    );
}

/// 実データの長さの絶対パスが複数候補で並ぶ場合。コピー元セレクタは折り返さないので、
/// ここが伸びるとモーダルがウィンドウの外へ出て、両端の文字が読めなくなる
/// (2026-08-26 の実機報告)。既存の `content_restore_prompt_light` は短いパスしか
/// 持っていなかったので、この形を通していなかった。
#[test]
fn content_restore_prompt_long_paths() {
    use mimageviewer::ui_dialogs::content_restore::{
        ContentRestoreUiRow, ContentRestoreUiSource, render_content_restore_modal,
    };

    let long_sources: Vec<ContentRestoreUiSource> = [
        r"h:/home/mimageviewer_old/testimage/y/chatgpt image 2026-06-07 15_49_45 (2).png",
        r"h:/home/mimageviewer_old/testimage/x2/y/chatgpt image 2026-06-07 15_49_45 (2).png",
        r"h:/home/mimageviewer_old/testimage/xxx/chatgpt image 2026-06-07 15_49_45 (2).png",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, path)| ContentRestoreUiSource {
        path: path.to_string(),
        source_exists: index % 2 == 0,
    })
    .collect();

    let mut rows = vec![
        ContentRestoreUiRow {
            file_name: "chatgpt image 2026-06-07 15_49_45 (2).png".to_string(),
            selected: true,
            source_index: 0,
            sources: long_sources,
        },
        ContentRestoreUiRow {
            file_name: "avif_Mexico - コピー2.avif".to_string(),
            selected: true,
            source_index: 0,
            sources: vec![ContentRestoreUiSource {
                path: r"h:/home/mimageviewer_old/testimage/photo/avif_Mexico.avif".to_string(),
                source_exists: true,
            }],
        },
    ];
    let mut dont_ask_again = false;
    let mut fonts_ready = false;
    const WINDOW_SIZE: egui::Vec2 = egui::vec2(980.0, 600.0);
    let mut harness = Harness::builder().with_size(WINDOW_SIZE).build(move |ctx| {
        mimageviewer::os_theme::apply_resolved(ctx, mimageviewer::os_theme::ResolvedTheme::Light);
        if !fonts_ready {
            install_app_fonts(ctx);
            fonts_ready = true;
            ctx.request_repaint();
            return;
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("画像一覧");
        });
        let _ = render_content_restore_modal(ctx, &mut rows, &mut dont_ask_again);
    });
    harness.run();
    let modal = harness
        .ctx
        .memory(|m| m.area_rect(egui::Id::new("content_restore_modal")))
        .expect("the modal has to be on screen");
    assert!(
        modal.width() <= WINDOW_SIZE.x,
        "モーダルがウィンドウ ({}px) からはみ出している: {}px",
        WINDOW_SIZE.x,
        modal.width()
    );
    harness.snapshot("content_restore_prompt_long_paths");
}

#[test]
fn content_restore_prompt_light() {
    use mimageviewer::ui_dialogs::content_restore::{
        ContentRestoreUiRow, ContentRestoreUiSource, render_content_restore_modal,
    };

    let mut rows = vec![
        ContentRestoreUiRow {
            file_name: "IMG_0421.jpg".to_string(),
            selected: true,
            source_index: 0,
            sources: vec![ContentRestoreUiSource {
                path: r"D:\photo\2025\IMG_0421.jpg".to_string(),
                source_exists: false,
            }],
        },
        ContentRestoreUiRow {
            file_name: "chapter03.cbz".to_string(),
            selected: true,
            source_index: 0,
            sources: vec![
                ContentRestoreUiSource {
                    path: r"D:\manga\chapter03.cbz".to_string(),
                    source_exists: true,
                },
                ContentRestoreUiSource {
                    path: r"E:\archive\chapter03.cbz".to_string(),
                    source_exists: true,
                },
            ],
        },
        ContentRestoreUiRow {
            file_name: "scan.pdf".to_string(),
            selected: false,
            source_index: 0,
            sources: vec![ContentRestoreUiSource {
                path: r"E:\old\scan.pdf".to_string(),
                source_exists: true,
            }],
        },
    ];
    let mut dont_ask_again = false;
    let mut fonts_ready = false;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(980.0, 600.0))
        .build(move |ctx| {
            mimageviewer::os_theme::apply_resolved(
                ctx,
                mimageviewer::os_theme::ResolvedTheme::Light,
            );
            if !fonts_ready {
                install_app_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("画像一覧");
                ui.label("モーダル表示中は背景の一覧を操作できません。");
                ui.add_space(12.0);
                for name in ["IMG_0421.jpg", "chapter03.cbz", "scan.pdf"] {
                    ui.group(|ui| {
                        ui.set_min_size(egui::vec2(180.0, 56.0));
                        ui.label(name);
                    });
                }
            });
            let _ = render_content_restore_modal(ctx, &mut rows, &mut dont_ask_again);
        });
    harness.run();
    harness.snapshot("content_restore_prompt_light");
}

#[test]
fn audio_thumbnail_indicator_modes_dark() {
    snapshot_with_theme_at_size(
        "audio_thumbnail_indicator_modes_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        egui::vec2(480.0, 430.0),
        None,
        mimageviewer::draw_audio_thumbnail_indicator_snapshot_fixture,
    );
}

#[test]
fn audio_thumbnail_indicator_modes_light() {
    snapshot_with_theme_at_size(
        "audio_thumbnail_indicator_modes_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        egui::vec2(480.0, 430.0),
        None,
        mimageviewer::draw_audio_thumbnail_indicator_snapshot_fixture,
    );
}
