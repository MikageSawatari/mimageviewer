use super::*;
use crate::video::native_presenter::overlay_draw::OverlaySliderDrag;

struct Fixture {
    ctx: egui::Context,
    chrome: MusicChromeViewState,
    drag: Option<OverlaySliderDrag>,
    clock: f64,
    volume_rect: egui::Rect,
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
        };
        fixture.frame(vec![]);
        fixture.frame(vec![]);
        fixture
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> MusicHudRowActions {
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
                                result = draw_music_playback_row(
                                    ui,
                                    hud,
                                    0,
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
