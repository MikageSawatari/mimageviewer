use super::*;

fn edge(button: egui::PointerButton, pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        button,
        pos,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn button_press_origin_907_uses_last_press_and_only_held_button_fallback() {
    use egui::PointerButton::{Middle, Primary};
    let ctx = egui::Context::default();
    let first = egui::pos2(20.0, 30.0);
    let last = egui::pos2(80.0, 90.0);
    let cases = [
        (vec![], None, None),
        (vec![edge(Primary, first, true)], Some(first), Some(first)),
        (vec![], Some(first), Some(first)),
        (vec![edge(Primary, first, false)], None, None),
        (
            vec![edge(Primary, first, true), edge(Primary, last, false)],
            Some(first),
            Some(first),
        ),
        (
            vec![
                edge(Primary, first, true),
                egui::Event::PointerButtonReleased {
                    button: Primary,
                    modifiers: egui::Modifiers::NONE,
                },
                edge(Primary, last, true),
            ],
            Some(last),
            Some(last),
        ),
        (
            vec![egui::Event::PointerButtonReleased {
                button: Primary,
                modifiers: egui::Modifiers::NONE,
            }],
            None,
            None,
        ),
        (vec![edge(Middle, last, true)], None, Some(last)),
        (vec![], None, Some(last)),
        (vec![edge(Middle, last, false)], None, None),
    ];
    for (events, primary, any) in cases {
        ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                ui.input(|input| {
                    assert_eq!(button_press_origin(input, Primary), primary);
                    assert_eq!(any_button_press_origin(input), any);
                });
            },
        )
        .drop_without_applying_deltas();
    }
}
