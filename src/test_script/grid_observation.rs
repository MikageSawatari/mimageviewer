//! Read-only evidence from the actual thumbnail ScrollArea, scoped to ROOT.

use rhai::Map;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GridObservation {
    pub(crate) frame: i64,
    pub(crate) generation: i64,
    pub(crate) selected_index: i64,
    pub(crate) selected_key: String,
    pub(crate) selected_name: String,
    pub(crate) scroll_offset: f32,
    pub(crate) viewport: egui::Rect,
    pub(crate) row_rect: Option<egui::Rect>,
    pub(crate) row_content_y: f32,
    pub(crate) cell_height: f32,
    pub(crate) columns: usize,
    pub(crate) content_height: f32,
}

impl Default for GridObservation {
    fn default() -> Self {
        Self {
            frame: -1,
            generation: -1,
            selected_index: -1,
            selected_key: String::new(),
            selected_name: String::new(),
            scroll_offset: 0.0,
            viewport: egui::Rect::NOTHING,
            row_rect: None,
            row_content_y: -1.0,
            cell_height: 0.0,
            columns: 0,
            content_height: 0.0,
        }
    }
}

fn observation_id() -> egui::Id {
    egui::Id::new("test-script-root-grid-observation")
}

pub(crate) fn publish(ctx: &egui::Context, observation: GridObservation) {
    if ctx.viewport_id() == egui::ViewportId::ROOT {
        ctx.data_mut(|data| data.insert_temp(observation_id(), observation));
    }
}

pub(crate) fn snapshot(ctx: &egui::Context) -> GridObservation {
    ctx.data(|data| data.get_temp(observation_id()).unwrap_or_default())
}

impl GridObservation {
    pub(crate) fn to_rhai_map(&self) -> Map {
        let mut map = Map::new();
        macro_rules! insert {
            ($field:ident) => {
                map.insert(stringify!($field).into(), self.$field.clone().into());
            };
        }
        insert!(frame);
        insert!(generation);
        insert!(selected_index);
        insert!(selected_key);
        insert!(selected_name);
        map.insert("scroll_offset".into(), (self.scroll_offset as f64).into());
        map.insert("row_content_y".into(), (self.row_content_y as f64).into());
        map.insert("cell_height".into(), (self.cell_height as f64).into());
        map.insert("content_height".into(), (self.content_height as f64).into());
        map.insert("columns".into(), (self.columns as i64).into());
        map.insert("viewport_top".into(), (self.viewport.top() as f64).into());
        map.insert(
            "viewport_bottom".into(),
            (self.viewport.bottom() as f64).into(),
        );
        map.insert(
            "viewport_height".into(),
            (self.viewport.height() as f64).into(),
        );
        map.insert("row_present".into(), self.row_rect.is_some().into());
        map.insert(
            "visible".into(),
            self.row_rect
                .is_some_and(|rect| rect.height() > 0.0 && self.viewport.contains(rect.center()))
                .into(),
        );
        let row = self.row_rect.unwrap_or(egui::Rect::NOTHING);
        map.insert("row_top".into(), (row.top() as f64).into());
        map.insert("row_bottom".into(), (row.bottom() as f64).into());
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_observation_uses_actual_scrollarea_clip_and_content_translation() {
        let ctx = egui::Context::default();
        for (offset, visible) in [(0.0, false), (300.0, true)] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 650.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::vertical()
                            .vertical_scroll_offset(offset)
                            .show_viewport(ui, |ui, viewport| {
                                let (content, _) = ui.allocate_exact_size(
                                    egui::vec2(800.0, 900.0),
                                    egui::Sense::hover(),
                                );
                                let rect = egui::Rect::from_min_size(
                                    content.min + egui::vec2(0.0, 720.0),
                                    egui::vec2(200.0, 180.0),
                                );
                                publish(
                                    ctx,
                                    GridObservation {
                                        viewport: ui.clip_rect().intersect(ctx.viewport_rect()),
                                        row_rect: Some(rect),
                                        scroll_offset: viewport.min.y,
                                        row_content_y: 720.0,
                                        ..Default::default()
                                    },
                                );
                            });
                    });
                },
            );
            let observed = snapshot(&ctx);
            assert_eq!(
                observed.to_rhai_map()["visible"].clone().cast::<bool>(),
                visible
            );
            assert!((observed.scroll_offset - offset).abs() < 1.0);
            assert_eq!(observed.row_content_y, 720.0);
        }
    }

    #[test]
    fn grid_observation_reports_selected_row_geometry_without_scrolling_it() {
        let ctx = egui::Context::default();
        let observation = GridObservation {
            generation: 42,
            selected_index: 11,
            scroll_offset: 300.0,
            viewport: egui::Rect::from_min_max(egui::pos2(0.0, 50.0), egui::pos2(500.0, 250.0)),
            row_rect: Some(egui::Rect::from_min_max(
                egui::pos2(0.0, 300.0),
                egui::pos2(100.0, 400.0),
            )),
            ..Default::default()
        };
        publish(&ctx, observation);
        let first = snapshot(&ctx).to_rhai_map();
        let second = snapshot(&ctx).to_rhai_map();
        assert!(!first["visible"].clone().cast::<bool>());
        assert_eq!(second["scroll_offset"].clone().cast::<f64>(), 300.0);
        assert_eq!(second["selected_index"].clone().cast::<i64>(), 11);
        assert_eq!(second["generation"].clone().cast::<i64>(), 42);
        assert_eq!(second["row_top"].clone().cast::<f64>(), 300.0);
    }
}
