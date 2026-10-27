use super::*;
use crate::video::native_presenter::overlay_draw::OverlaySliderDrag;

struct Fixture {
    ctx: egui::Context,
    chrome: MusicChromeViewState,
    drag: Option<OverlaySliderDrag>,
    clock: f64,
    volume_rect: egui::Rect,
    play_rect: egui::Rect,
    display_id: usize,
}

impl Fixture {
    fn new() -> Self {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let mut chrome = music_hud_snapshot_chrome();
        chrome.normalize_ui_state = crate::video::normalize_types::NormalizeUiState::Off;
        let mut fixture = Self {
            ctx,
            chrome,
            drag: None,
            clock: 0.0,
            volume_rect: egui::Rect::NOTHING,
            play_rect: egui::Rect::NOTHING,
            display_id: 0,
        };
        fixture.frame(vec![]);
        fixture.frame(vec![]);
        fixture
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> MusicHudRowActions {
        self.visible_frame(events, true)
    }

    fn visible_frame(&mut self, events: Vec<egui::Event>, visible: bool) -> MusicHudRowActions {
        self.clock += 1.0 / 60.0;
        let mut result = MusicHudRowActions::default();
        self.ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 360.0),
                    )),
                    time: Some(self.clock),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|root| {
                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(root, |ui| {
                                if !visible {
                                    return;
                                }
                                let hud = egui::Rect::from_min_max(
                                    egui::pos2(0.0, 280.0),
                                    egui::pos2(1200.0, 360.0),
                                );
                                let layout = music_hud_row_layout(
                                    hud,
                                    measure_music_hud_row(ui, &self.chrome, &[]),
                                );
                                self.volume_rect =
                                    layout.rects[MusicHudItem::Volume as usize].unwrap();
                                self.play_rect = layout.rects[MusicHudItem::Play as usize].unwrap();
                                result = draw_music_playback_row(
                                    ui,
                                    hud,
                                    self.display_id,
                                    &self.chrome,
                                    &layout,
                                    &[],
                                    ui.scope_id().with("track"),
                                    false,
                                    MusicHudPresentation::Interactive,
                                    &crate::keymap::Keymap::empty(),
                                    &mut self.drag,
                                    &mut false,
                                );
                            });
                    })
                },
            )
            .drop_without_applying_deltas();
        if let Some(volume) = result.set_vol {
            self.chrome.volume = volume;
        }
        result
    }
}

#[test]
fn hud_slider_music_volume_closed_view_cannot_claim_next_play_click() {
    for release_while_hidden in [true, false] {
        let mut f = Fixture::new();
        let start = f.volume_rect.center();
        f.frame(vec![egui::Event::PointerMoved(start)]);
        f.frame(vec![button(start, true)]);
        f.frame(vec![egui::Event::PointerMoved(
            start + egui::vec2(18.0, 0.0),
        )]);
        let last = f.chrome.volume;
        assert!(f.drag.is_some());
        // The view-close action consumes Escape before HUD drawing. Also cover
        // an omitted pass without Escape, while egui still owns the held drag.
        f.visible_frame(vec![], false);
        let other = f.play_rect.center();
        if release_while_hidden {
            f.visible_frame(vec![button(other, false)], false);
        }
        assert!(f.frame(vec![]).set_vol.is_none());
        assert_eq!(f.chrome.volume, last);
        if !release_while_hidden {
            assert!(f.frame(vec![button(other, false)]).set_vol.is_none());
        }
        f.frame(vec![egui::Event::PointerMoved(other)]);
        assert!(f.frame(vec![button(other, true)]).set_vol.is_none());
        let result = f.frame(vec![button(other, false)]);
        assert!(result.toggle_play);
        assert!(
            result.set_vol.is_none(),
            "another widget cannot finish an abandoned drag"
        );
        assert_eq!(f.chrome.volume, last);
        assert!(f.drag.is_none());
    }
}

#[test]
fn hud_slider_music_volume_response_id_change_ends_old_owner() {
    let mut f = Fixture::new();
    let start = f.volume_rect.center();
    f.frame(vec![egui::Event::PointerMoved(start)]);
    f.frame(vec![button(start, true)]);
    f.frame(vec![egui::Event::PointerMoved(
        start + egui::vec2(18.0, 0.0),
    )]);
    let last = f.chrome.volume;
    f.display_id += 1;
    assert!(f.frame(vec![]).set_vol.is_none());
    assert!(
        f.frame(vec![button(f.play_rect.center(), false)])
            .set_vol
            .is_none()
    );
    assert!(f.drag.is_none());
    assert_eq!(f.chrome.volume, last);

    // A press that never reached drag_started also belongs to the old display.
    let mut f = Fixture::new();
    let start = f.volume_rect.center();
    let original = f.chrome.volume;
    f.frame(vec![egui::Event::PointerMoved(start)]);
    f.frame(vec![button(start, true)]);
    f.visible_frame(vec![], false);
    assert!(f.frame(vec![]).set_vol.is_none());
    assert!(f.frame(vec![button(start, false)]).set_vol.is_none());
    assert_eq!(f.chrome.volume, original);
    // A new press/release in one pass is still an ordinary slider click.
    assert!(
        f.frame(vec![button(start, true), button(start, false)])
            .set_vol
            .is_some()
    );
}

#[test]
fn hud_slider_music_seek_hidden_display_cannot_resume_held_drag() {
    let ctx = egui::Context::default();
    let bar = egui::Rect::from_min_size(egui::pos2(30.0, 30.0), egui::vec2(200.0, 8.0));
    let mut position = 5.0;
    let mut frame = |visible: bool, events| {
        let mut result = None;
        ctx.run_pass(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        if visible {
                            let response = ui.interact(
                                bar.expand(7.0),
                                egui::Id::unique("seek"),
                                egui::Sense::click_and_drag(),
                            );
                            result = music_seek_slider_value(&response, bar, position, 100.0);
                        }
                    });
                });
            },
        )
        .drop_without_applying_deltas();
        if let Some(value) = result {
            position = value;
        }
        result
    };
    let start = egui::pos2(70.0, 34.0);
    frame(true, vec![]);
    frame(true, vec![]);
    frame(true, vec![egui::Event::PointerMoved(start)]);
    frame(true, vec![button(start, true)]);
    assert_eq!(
        frame(
            true,
            vec![egui::Event::PointerMoved(egui::pos2(130.0, 34.0))]
        ),
        Some(50.0)
    );
    frame(false, vec![]);
    assert_eq!(
        frame(
            true,
            vec![egui::Event::PointerMoved(egui::pos2(180.0, 34.0))]
        ),
        None
    );
    assert_eq!(
        frame(true, vec![button(egui::pos2(0.0, 200.0), false)]),
        None
    );
    frame(true, vec![button(egui::pos2(0.0, 200.0), true)]);
    assert_eq!(
        frame(true, vec![button(egui::pos2(0.0, 200.0), false)]),
        None
    );
    assert_eq!(position, 50.0);
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    button_for(pos, pressed, egui::PointerButton::Primary)
}

fn button_for(pos: egui::Pos2, pressed: bool, button: egui::PointerButton) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        pressed,
        button,
        modifiers: Default::default(),
    }
}

#[test]
fn hud_slider_music_volume_release_uses_last_frame_position() {
    for kind in [egui::PointerButton::Primary, egui::PointerButton::Secondary] {
        let mut f = Fixture::new();
        let start = f.volume_rect.center();
        f.frame(vec![egui::Event::PointerMoved(start)]);
        f.frame(vec![button_for(start, true, kind)]);
        let dragged = f
            .frame(vec![egui::Event::PointerMoved(
                start + egui::vec2(10.0, 0.0),
            )])
            .set_vol
            .unwrap();
        let release = start + egui::vec2(22.0, 0.0);
        let expected = crate::settings::video_volume_fader_pos_to_linear(
            ((release.x - f.volume_rect.left()) / f.volume_rect.width()).clamp(0.0, 1.0) as f64,
        );
        let other = if kind == egui::PointerButton::Primary {
            egui::PointerButton::Secondary
        } else {
            egui::PointerButton::Middle
        };
        let held = start + egui::vec2(10.0, 0.0);
        f.frame(vec![button_for(held, true, other)]);
        assert!(
            f.frame(vec![button_for(held, false, other)])
                .set_vol
                .is_none()
        );
        assert!(
            f.drag.is_some(),
            "another button cannot terminate the owned drag"
        );
        let result = f.frame(vec![
            egui::Event::PointerButton {
                pos: egui::Pos2::ZERO,
                button: if kind == egui::PointerButton::Primary {
                    egui::PointerButton::Secondary
                } else {
                    egui::PointerButton::Middle
                },
                pressed: false,
                modifiers: Default::default(),
            },
            button_for(release, false, kind),
            egui::Event::PointerMoved(start),
        ]);
        assert_ne!(expected, dragged);
        assert_eq!(result.set_vol, Some(expected));
        assert!(result.vol_persist);
        assert!(f.drag.is_none());
        assert!(f.frame(vec![]).set_vol.is_none());
        f.frame(vec![egui::Event::PointerMoved(start)]);
        f.frame(vec![button_for(
            start,
            true,
            egui::PointerButton::Secondary,
        )]);
        assert!(
            f.frame(vec![button_for(
                start,
                false,
                egui::PointerButton::Secondary
            )])
            .set_vol
            .is_none(),
            "secondary click without a drag keeps the old no-op behavior"
        );
    }
}

#[test]
fn hud_slider_music_volume_escape_restores_and_ignores_later_release() {
    let mut f = Fixture::new();
    let original = f.chrome.volume;
    let start = f.volume_rect.center();
    f.frame(vec![egui::Event::PointerMoved(start)]);
    f.frame(vec![button(start, true)]);
    f.frame(vec![egui::Event::PointerMoved(
        start + egui::vec2(10.0, 0.0),
    )]);
    let result = f.frame(vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }]);
    assert_eq!(result.set_vol, Some(original));
    assert!(result.vol_persist);
    assert!(
        f.frame(vec![egui::Event::PointerMoved(
            start + egui::vec2(20.0, 0.0)
        )])
        .set_vol
        .is_none()
    );
    assert!(
        f.frame(vec![button(start + egui::vec2(30.0, 0.0), false)])
            .set_vol
            .is_none()
    );
    assert!(f.drag.is_none());
}

#[test]
fn hud_slider_music_seek_release_and_escape_use_shared_terminal_policy() {
    for (cancel, kind) in [
        (false, egui::PointerButton::Primary),
        (true, egui::PointerButton::Primary),
        (false, egui::PointerButton::Secondary),
        (true, egui::PointerButton::Secondary),
    ] {
        let ctx = egui::Context::default();
        let bar = egui::Rect::from_min_size(egui::pos2(30.0, 30.0), egui::vec2(200.0, 8.0));
        let mut position = 5.0;
        let mut clock = 0.0;
        let mut frame = |events| {
            clock += 0.02;
            let mut result = None;
            ctx.run_pass(
                egui::RawInput {
                    events,
                    time: Some(clock),
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|root| {
                        egui::CentralPanel::default().show(root, |ui| {
                            let response = ui.interact(
                                bar.expand(7.0),
                                egui::Id::unique("music_hud_seek"),
                                egui::Sense::click_and_drag(),
                            );
                            result = music_seek_slider_value(&response, bar, position, 100.0);
                        });
                    })
                },
            )
            .drop_without_applying_deltas();
            if let Some(value) = result {
                position = value;
            }
            result
        };
        let start = egui::pos2(70.0, 34.0);
        frame(vec![]);
        frame(vec![]);
        frame(vec![egui::Event::PointerMoved(start)]);
        frame(vec![button_for(start, true, kind)]);
        assert_eq!(
            frame(vec![egui::Event::PointerMoved(egui::pos2(130.0, 34.0))]),
            Some(50.0)
        );
        if cancel {
            assert_eq!(
                frame(vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default()
                }]),
                Some(5.0)
            );
            assert_eq!(
                frame(vec![button_for(egui::pos2(180.0, 34.0), false, kind)]),
                None
            );
        } else {
            assert_eq!(
                frame(vec![
                    button_for(egui::pos2(180.0, 34.0), false, kind),
                    egui::Event::PointerMoved(start)
                ]),
                Some(75.0)
            );
        }
        assert_eq!(frame(vec![]), None);
    }
}
