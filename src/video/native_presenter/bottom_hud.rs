//! Frame-local bottom-row geometry. No stored compaction or scan layout state.

use egui::{Color32, Painter, Rect};

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub(super) enum Item {
    Replay,
    Play,
    Loop,
    Continuous,
    Prev,
    Next,
    PrevMarker,
    NextMarker,
    PrevFrame,
    CopyFrame,
    SaveFrame,
    NextFrame,
    Time,
    Speed,
    Track,
    Mute,
    Norm,
    Volume,
    Db,
    Limiter,
    Strip,
    Lock,
}

const ITEMS: [Item; 22] = [
    Item::Replay,
    Item::Play,
    Item::Loop,
    Item::Continuous,
    Item::Prev,
    Item::Next,
    Item::PrevMarker,
    Item::NextMarker,
    Item::PrevFrame,
    Item::CopyFrame,
    Item::SaveFrame,
    Item::NextFrame,
    Item::Time,
    Item::Speed,
    Item::Track,
    Item::Mute,
    Item::Norm,
    Item::Volume,
    Item::Db,
    Item::Limiter,
    Item::Strip,
    Item::Lock,
];

#[derive(Clone)]
pub(super) struct Metrics {
    widths: [Option<f32>; 22],
    time: String,
    short: String,
    short_width: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_tracks_hud_legacy_coordinates_and_monotonic_drop_order() {
        use Item::*;
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let metrics = Metrics::measure(
                    ui.painter(),
                    28.0,
                    "0:05 / 0:30".into(),
                    "0:05".into(),
                    "x1",
                    "0.0dB",
                    Some(1),
                    true,
                );
                let wide = Layout::resolve(
                    Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2000.0, 40.0)),
                    28.0,
                    metrics.clone(),
                );
                for (item, x) in [
                    (Replay, 10.0),
                    (Play, 46.0),
                    (Loop, 90.0),
                    (Prev, 162.0),
                    (Next, 198.0),
                    (PrevFrame, 322.0),
                    (Lock, 1962.0),
                    (Strip, 1926.0),
                ] {
                    assert_eq!(wide.get(item).unwrap().left(), x);
                }
                assert!((wide.get(Time).unwrap().left() - 1358.6).abs() < 0.001);
                let order = [
                    Limiter, Db, Speed, Norm, Continuous, Loop, Volume, PrevFrame, CopyFrame,
                    SaveFrame, NextFrame, PrevMarker, NextMarker, Replay,
                ];
                let mut previous = [true; 22];
                let mut descending = Vec::new();
                for width in (1..=2000).rev() {
                    let layout = Layout::resolve(
                        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, 40.0)),
                        28.0,
                        metrics.clone(),
                    );
                    let visible = layout.rects.map(|r| r.is_some());
                    let mut retained = false;
                    for item in order {
                        if visible[item as usize] {
                            retained = true;
                        } else {
                            assert!(!retained, "drop order at {width}: {item:?}");
                        }
                    }
                    for (&now, &before) in visible.iter().zip(&previous) {
                        assert!(!now || before, "reappeared on shrink at {width}");
                    }
                    previous = visible;
                    descending.push(visible);
                }
                for width in 1..=2000 {
                    let layout = Layout::resolve(
                        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, 40.0)),
                        28.0,
                        metrics.clone(),
                    );
                    assert_eq!(layout.rects.map(|r| r.is_some()), descending[2000 - width]);
                }
            });
        });
    }

    #[test]
    fn native_tracks_hud_measured_layout_packs_every_width_and_keeps_essentials() {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for time in ["0:05 / 0:30", "100:59:59 / 999:59:59"] {
                    for strip in [false, true] {
                        let metrics = Metrics::measure(
                            ui.painter(),
                            28.0,
                            time.into(),
                            "0:05".into(),
                            "x1",
                            "+6.0 dB",
                            Some(3),
                            strip,
                        );
                        for width in 1..=2000 {
                            let row = Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width as f32, 28.0),
                            );
                            let layout = Layout::resolve(row, 28.0, metrics.clone());
                            for item in [
                                Item::Play,
                                Item::Prev,
                                Item::Next,
                                Item::Track,
                                Item::Mute,
                                Item::Lock,
                            ] {
                                assert!(
                                    layout.get(item).unwrap().is_positive(),
                                    "{item:?} at {width}"
                                );
                            }
                            for (i, a) in layout.rects.iter().enumerate() {
                                if let Some(a) = a {
                                    assert!(
                                        a.left() >= -0.001 && a.right() <= width as f32 + 0.001
                                    );
                                    for b in layout.rects.iter().skip(i + 1).flatten() {
                                        let intersection = a.intersect(*b);
                                        assert!(
                                            intersection.width() <= 0.001
                                                || intersection.height() <= 0.001,
                                            "overlap at {width}: {a:?} / {b:?}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            });
        });
    }
}

impl Metrics {
    pub(super) fn measure(
        painter: &Painter,
        button: f32,
        time: String,
        short: String,
        speed: &str,
        db: &str,
        ordinal: Option<usize>,
        strip: bool,
    ) -> Self {
        use Item::*;
        let measure = |text: &str, size| {
            painter
                .layout_no_wrap(
                    text.into(),
                    crate::ui_fonts::hud_text_font(size),
                    Color32::WHITE,
                )
                .size()
                .x
        };
        let mut widths = [Some(button); 22];
        widths[Time as usize] = Some(measure(&time, 14.0).max(132.0));
        widths[Speed as usize] = Some(measure(speed, 11.0).max(button * 1.55));
        widths[Track as usize] = ordinal.map(|n| measure(&format!("音声 {n}"), 12.0).max(62.0));
        widths[Norm as usize] = Some(measure("Norm", 11.0).max(button));
        widths[Volume as usize] = Some(144.0);
        widths[Db as usize] = Some(measure(db, 13.0).max(60.0));
        widths[Limiter as usize] = Some(14.0);
        widths[Strip as usize] = strip.then_some(button);
        let short_width = measure(&short, 14.0);
        Self {
            widths,
            time,
            short,
            short_width,
        }
    }
}

pub(super) struct Layout {
    pub(super) rects: [Option<Rect>; 22],
    pub(super) time: String,
}

impl Layout {
    pub(super) fn get(&self, item: Item) -> Option<Rect> {
        self.rects[item as usize]
    }
    pub(super) fn navigation(&self) -> [Rect; 2] {
        [self.get(Item::Prev).unwrap(), self.get(Item::Next).unwrap()]
    }

    pub(super) fn resolve(row: Rect, button: f32, metrics: Metrics) -> Self {
        use Item::*;
        let rect = |x, width| {
            Rect::from_center_size(
                egui::pos2(x + width * 0.5, row.center().y),
                egui::vec2(width, button),
            )
        };
        // Preserve released coordinates only when the complete row fits.
        let mut rects = [None; 22];
        let mut x = row.left() + 10.0;
        for item in ITEMS.into_iter().take(12) {
            if matches!(item, Loop | PrevMarker | PrevFrame) {
                x += 8.0;
            }
            let width = metrics.widths[item as usize].unwrap();
            rects[item as usize] = Some(rect(x, width));
            x += width + 8.0;
        }
        let mut right = row.right() - 10.0;
        for item in ITEMS.into_iter().skip(12).rev() {
            if let Some(width) = metrics.widths[item as usize] {
                rects[item as usize] = Some(rect(right - width, width));
                right -= width;
                if !matches!(item, Limiter | Time) {
                    right -= 8.0;
                }
            }
        }
        if rects[Time as usize].unwrap().left() >= x {
            return Self {
                rects,
                time: metrics.time,
            };
        }
        let padding = 10.0_f32.min(row.width().max(0.0) * 0.1);
        let available = (row.width() - padding * 2.0).max(0.0);
        let core_count = 5.0
            + usize::from(metrics.widths[Track as usize].is_some()) as f32
            + usize::from(metrics.widths[Strip as usize].is_some()) as f32;
        let gap = 8.0_f32.min(available / (core_count + 1.0));
        let mut widths = metrics.widths;
        let total = |widths: &[Option<f32>; 22]| {
            widths.iter().flatten().sum::<f32>()
                + gap * widths.iter().flatten().count().saturating_sub(1) as f32
        };
        let drop_order: &[&[Item]] = &[
            &[Limiter],
            &[Db],
            &[Speed],
            &[Norm],
            &[Continuous],
            &[Loop],
            &[Volume],
            &[PrevFrame, CopyFrame, SaveFrame, NextFrame],
            &[PrevMarker, NextMarker],
            &[Replay],
        ];
        for group in drop_order {
            if total(&widths) <= available {
                break;
            }
            for item in *group {
                widths[*item as usize] = None;
            }
        }
        let mut time = metrics.time;
        if total(&widths) > available {
            widths[Time as usize] = Some(metrics.short_width);
            time = metrics.short;
        }
        if total(&widths) > available {
            widths[Time as usize] = None;
        }
        if total(&widths) > available {
            let width = (available - gap * (core_count - 1.0)).max(0.0) / core_count;
            for slot in widths.iter_mut().flatten() {
                *slot = width;
            }
        }
        rects = [None; 22];
        x = row.left() + padding;
        for item in ITEMS.into_iter().take(12) {
            if let Some(width) = widths[item as usize] {
                rects[item as usize] = Some(rect(x, width));
                x += width + gap;
            }
        }
        right = row.right() - padding;
        for item in ITEMS.into_iter().skip(12).rev() {
            if let Some(width) = widths[item as usize] {
                rects[item as usize] = Some(rect(right - width, width));
                right -= width + gap;
            }
        }
        Self { rects, time }
    }
}
