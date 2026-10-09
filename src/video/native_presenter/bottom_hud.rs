//! Frame-local bottom-row geometry. No stored compaction or scan layout state.

use egui::{Color32, Galley, Painter, Rect};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
enum LabelKey {
    Time(u64, u64),
    Position(u64),
    Speed(u64),
    Track(usize),
    Db(u64),
    Norm,
}

#[derive(Clone, Default)]
pub(super) struct TextCache {
    font_identity: Option<Arc<Galley>>,
    labels: [Option<(LabelKey, Arc<Galley>)>; 6],
    fitted_time: Option<(Arc<Galley>, egui::Vec2, Option<Arc<Galley>>)>,
    #[cfg(test)]
    pub(super) label_layouts: usize,
}

impl TextCache {
    fn label(
        &mut self,
        painter: &Painter,
        slot: usize,
        key: LabelKey,
        size: f32,
        format: impl FnOnce() -> String,
    ) -> Arc<Galley> {
        let entry = &mut self.labels[slot];
        if let Some((previous, galley)) = entry {
            if *previous == key {
                return galley.clone();
            }
        }
        let galley = painter.layout_no_wrap(
            format(),
            crate::ui_fonts::hud_text_font(size),
            Color32::PLACEHOLDER,
        );
        #[cfg(test)]
        {
            self.label_layouts += 1;
        }
        *entry = Some((key, galley.clone()));
        galley
    }

    pub(super) fn measure(
        &mut self,
        painter: &Painter,
        button: f32,
        position: f64,
        duration: f64,
        speed: f64,
        volume: f64,
        ordinal: Option<usize>,
        strip: bool,
    ) -> Metrics {
        // A single empty-job lookup keeps this identity alive in egui's galley cache.
        // Font/atlas resets, DPI changes and eviction replace it: never reuse stale UVs.
        // The empty job has no owned label text and allocates no String.
        let identity = painter
            .ctx()
            .fonts_mut(|fonts| fonts.layout_job(egui::text::LayoutJob::default()));
        if !self
            .font_identity
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &identity))
        {
            self.labels = Default::default();
            self.fitted_time = None;
            self.font_identity = Some(identity);
        }
        let seconds = |value: f64| {
            if value.is_finite() && value >= 0.0 {
                value.round() as u64
            } else {
                0
            }
        };
        let p = seconds(position);
        let d = seconds(duration);
        let time = self.label(painter, 0, LabelKey::Time(p, d), 14.0, || {
            format!(
                "{} / {}",
                super::overlay_draw::format_overlay_time(position),
                super::overlay_draw::format_overlay_time(duration)
            )
        });
        let short = self.label(painter, 1, LabelKey::Position(p), 14.0, || {
            super::overlay_draw::format_overlay_time(position)
        });
        let speed = crate::video::clock::clamp_playback_speed(speed);
        let speed = self.label(painter, 2, LabelKey::Speed(speed.to_bits()), 12.0, || {
            crate::video::clock::format_playback_speed(speed)
        });
        let track = ordinal
            .map(|n| self.label(painter, 3, LabelKey::Track(n), 12.0, || format!("音声 {n}")));
        let volume = super::overlay_draw::finite_video_volume(volume);
        let db = self.label(painter, 4, LabelKey::Db(volume.to_bits()), 13.0, || {
            super::render_core::format_video_volume_db_compact(volume)
        });
        let norm = self.label(painter, 5, LabelKey::Norm, 11.0, || "Norm".into());
        Metrics::from_labels(
            button,
            time,
            short,
            Labels {
                speed,
                track,
                db,
                norm,
            },
            strip,
        )
    }

    pub(super) fn fit_time(
        &mut self,
        painter: &Painter,
        rect: Rect,
        label: &Arc<Galley>,
    ) -> Option<Arc<Galley>> {
        let size = rect.size();
        if label.size().x <= size.x
            && label.size().y <= size.y
            && size.y * 0.72 >= 14.0
            && size.x * 0.72 >= 14.0
        {
            return Some(label.clone());
        }
        if let Some((old, old_size, fitted)) = &self.fitted_time {
            if Arc::ptr_eq(old, label) && *old_size == size {
                return fitted.clone();
            }
        }
        let fitted = super::render_core::fitted_strip_text_galley(
            painter,
            label.text(),
            rect,
            14.0,
            Color32::PLACEHOLDER,
        );
        self.fitted_time = Some((label.clone(), size, fitted.clone()));
        fitted
    }
}

#[derive(Clone)]
pub(super) struct Labels {
    pub(super) speed: Arc<Galley>,
    pub(super) track: Option<Arc<Galley>>,
    pub(super) db: Arc<Galley>,
    pub(super) norm: Arc<Galley>,
}

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
    time: Arc<Galley>,
    short: Arc<Galley>,
    short_width: f32,
    labels: Labels,
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
    #[cfg(test)]
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
        let measure = |text: String, size| {
            painter.layout_no_wrap(
                text,
                crate::ui_fonts::hud_text_font(size),
                Color32::PLACEHOLDER,
            )
        };
        Self::from_labels(
            button,
            measure(time, 14.0),
            measure(short, 14.0),
            Labels {
                speed: measure(speed.into(), 12.0),
                track: ordinal.map(|n| measure(format!("音声 {n}"), 12.0)),
                norm: measure("Norm".into(), 11.0),
                db: measure(db.into(), 13.0),
            },
            strip,
        )
    }

    fn from_labels(
        button: f32,
        time: Arc<Galley>,
        short: Arc<Galley>,
        labels: Labels,
        strip: bool,
    ) -> Self {
        use Item::*;
        let mut widths = [Some(button); 22];
        widths[Time as usize] = Some(time.size().x.max(132.0));
        widths[Speed as usize] = Some(labels.speed.size().x.max(button * 1.55));
        widths[Track as usize] = labels.track.as_ref().map(|label| label.size().x.max(62.0));
        widths[Norm as usize] = Some(labels.norm.size().x.max(button));
        widths[Volume as usize] = Some(144.0);
        widths[Db as usize] = Some(labels.db.size().x.max(60.0));
        widths[Limiter as usize] = Some(14.0);
        widths[Strip as usize] = strip.then_some(button);
        let short_width = short.size().x;
        Self {
            widths,
            time,
            short,
            short_width,
            labels,
        }
    }
}

pub(super) struct Layout {
    pub(super) rects: [Option<Rect>; 22],
    pub(super) time: Arc<Galley>,
    pub(super) labels: Labels,
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
                labels: metrics.labels,
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
        Self {
            rects,
            time,
            labels: metrics.labels,
        }
    }
}
