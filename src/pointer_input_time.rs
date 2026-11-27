//! Observe canonical button time once per viewport pass, before input owners
//! consume/cancel edges. Disabled perf does not enumerate pointer input.

pub(crate) fn install(ctx: &egui::Context) {
    if crate::perf::is_enabled() && ctx.plugin_opt::<PointerLag>().is_none() {
        ctx.add_plugin(PointerLag);
    }
}

struct PointerLag;

fn lag_ms(time: egui::EdgeTime) -> Option<f64> {
    let lag = time.pass - time.input?;
    (lag >= 0.100).then_some(lag * 1000.0)
}

impl egui::Plugin for PointerLag {
    fn debug_name(&self) -> &'static str {
        "mIV native pointer lag"
    }

    fn on_begin_pass(&mut self, ui: &mut egui::Ui) {
        if !crate::perf::is_enabled() {
            return;
        }
        ui.input(|input| {
            for edge in input.pointer.button_edges() {
                if let Some(lag) = lag_ms(edge.time) {
                    crate::perf::event(
                        "input",
                        "pointer_lag",
                        None,
                        0,
                        &[
                            ("button", serde_json::json!(format!("{:?}", edge.button))),
                            ("pressed", edge.pressed.into()),
                            ("lag_ms", lag.into()),
                            (
                                "viewport",
                                serde_json::json!(format!("{:?}", input.raw.viewport_id)),
                            ),
                            (
                                "fullscreen",
                                input.viewport().fullscreen.unwrap_or(false).into(),
                            ),
                        ],
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pointer_lag_919_requires_known_input_and_at_least_100ms() {
        use egui::EdgeTime;
        assert_eq!(
            super::lag_ms(EdgeTime {
                pass: 0.1,
                input: Some(0.0)
            }),
            Some(100.0)
        );
        assert_eq!(
            super::lag_ms(EdgeTime {
                pass: 0.099,
                input: Some(0.0)
            }),
            None
        );
        assert_eq!(super::lag_ms(EdgeTime::from(1.0)), None);
    }
}
