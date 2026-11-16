use super::*;

fn raw(viewport: egui::ViewportId) -> egui::RawInput {
    let mut input = egui::RawInput {
        viewport_id: viewport,
        ..Default::default()
    };
    input.viewports.insert(viewport, Default::default());
    input
}

#[test]
fn section1768_pass_start_owner_survives_memory_open_close_and_switch() {
    let first = egui::Id::unique("first-menu");
    let second = egui::Id::unique("second-menu");
    for (initial, later) in [
        (None, Some(second)),
        (Some(first), None),
        (Some(first), Some(second)),
    ] {
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        if let Some(id) = initial {
            egui::Popup::open_id(&ctx, id);
        }
        ctx.begin_pass(raw(egui::ViewportId::ROOT));
        egui::Popup::close_all(&ctx);
        if let Some(id) = later {
            egui::Popup::open_id(&ctx, id);
        }
        assert_eq!(app_escape_allowed(&ctx), initial.is_none());
        for id in [first, second] {
            assert_eq!(
                popup_owned_escape_at_pass_start(&ctx, id),
                initial == Some(id)
            );
        }
        ctx.end_pass().drop_without_applying_deltas();
    }
}

#[test]
fn section1768_preinput_owner_uses_incoming_viewport_in_both_directions() {
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    let root = egui::ViewportId::ROOT;
    let sibling = egui::ViewportId::from_hash_of("preinput-sibling");
    let popup = egui::Id::unique("root-retained-menu");
    egui::Popup::open_id(&ctx, popup);
    for viewport in [root, sibling, root] {
        let mut input = raw(viewport);
        input.viewports.insert(sibling, Default::default());
        ctx.begin_pass(input);
        assert_eq!(app_escape_allowed(&ctx), viewport == sibling);
        assert_eq!(
            popup_owned_escape_at_pass_start(&ctx, popup),
            viewport == root
        );
        // Keep the real root menu alive without re-reading it for Escape ownership.
        if viewport == root {
            egui::Popup::open_id(&ctx, popup);
        }
        ctx.end_pass().drop_without_applying_deltas();
    }
}

#[test]
fn section1768_manual_pass_and_repeated_install_keep_the_first_observation() {
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    crate::ime_focus::install_ime_input_policy(&ctx);
    let popup = egui::Id::unique("installed-menu");
    ctx.begin_pass(Default::default());
    egui::Popup::open_id(&ctx, popup);
    crate::ime_focus::install_ime_input_policy(&ctx);
    assert!(app_escape_allowed(&ctx));
    assert!(!popup_owned_escape_at_pass_start(&ctx, popup));
    ctx.end_pass().drop_without_applying_deltas();
    ctx.begin_pass(Default::default());
    assert!(!app_escape_allowed(&ctx));
    assert!(popup_owned_escape_at_pass_start(&ctx, popup));
    ctx.end_pass().drop_without_applying_deltas();
}
