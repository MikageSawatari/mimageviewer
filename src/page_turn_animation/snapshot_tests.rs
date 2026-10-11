//! Fixed-time GPU snapshots of the actual projected textured meshes. These
//! prove headless render appearance, not application routing or OS present.
use super::*;
use crate::displayed_image_transform::{
    DisplayedImageTransform, DisplayedImageTransformInput, FullscreenFitScaleLimits, RectPixelFit,
    ResolvedDisplayPlacement,
};
use crate::fullscreen_specified_scale::FullscreenFitInput;
use egui::{TextureHandle, TextureOptions, vec2};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const POSES: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
const CELL_WIDTH: f32 = 224.0;
const SIZE: egui::Vec2 = vec2(1144.0, 292.0);

#[derive(Clone, Copy)]
enum SceneGeometry {
    Equal,
    PortraitPdf,
    UnequalScan,
    TallerIncoming,
    OpaqueBlack,
    SpineEdge,
    FixedSingleSpine,
    ResidentQuality,
}

impl SceneGeometry {
    fn name(self) -> &'static str {
        match self {
            Self::Equal => "",
            Self::PortraitPdf => "pdf_letterbox_",
            Self::UnequalScan => "unequal_scan_",
            Self::TallerIncoming => "height_continuity_",
            Self::OpaqueBlack => "opaque_black_",
            Self::SpineEdge => "spine_edge_",
            Self::FixedSingleSpine => "fixed_spine_",
            Self::ResidentQuality => "resident_quality_",
        }
    }

    fn poses(self) -> [f32; 5] {
        match self {
            // The first two cells deliberately expose a discontinuity that
            // endpoint-only visual tests cannot find. The later cells show
            // the growing outer footprint and the canonical destination.
            Self::TallerIncoming => [0.0, 0.0001, 0.1, 0.25, 1.0],
            Self::SpineEdge => [0.0, 0.425, 0.433, 0.44, 1.0],
            Self::FixedSingleSpine => [0.0, 0.15, 0.5, 0.85, 1.0],
            _ => POSES,
        }
    }
}

struct SceneTextures {
    old: [TextureHandle; 2],
    new: [TextureHandle; 2],
    checker: TextureHandle,
}

// Pixel lettering belongs to the page texture, so UV mirroring, clipping and
// face ownership affect the letters themselves rather than a UI annotation.
fn glyph(character: char) -> [u8; 7] {
    match character {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '>' => [16, 8, 4, 2, 4, 8, 16],
        _ => panic!("unsupported texture fixture glyph"),
    }
}

fn page_image(letter: char, number: char, base: Color32) -> egui::ColorImage {
    let mut image = egui::ColorImage::filled([112, 168], Color32::TRANSPARENT);
    for y in 4..164 {
        for x in 4..108 {
            // Transparent bottom windows expose the independently projected
            // checker underlay. Unequal side stripes make orientation obvious.
            if (148..158).contains(&y) && (x / 12) % 2 == 0 {
                continue;
            }
            image[(x, y)] = if x < 10 {
                Color32::from_rgb(211, 58, 66)
            } else if x >= 102 {
                Color32::from_rgb(248, 152, 38)
            } else if y % 24 == 0 {
                Color32::from_rgb(74, 94, 116)
            } else {
                base
            };
        }
    }
    for y in [18, 70, 122] {
        for x in [12, 44, 76] {
            for (index, character) in [letter, number, '>'].into_iter().enumerate() {
                for (row, bits) in glyph(character).into_iter().enumerate() {
                    for col in 0..5 {
                        if bits & (1 << (4 - col)) != 0 {
                            for dy in 0..2 {
                                for dx in 0..2 {
                                    image[(x + index * 10 + col * 2 + dx, y + row * 2 + dy)] =
                                        Color32::from_rgb(18, 26, 42);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    image
}

fn load_textures(ctx: &egui::Context, opaque: bool, resident_quality: bool) -> SceneTextures {
    let mut checker = egui::ColorImage::filled([112, 168], Color32::WHITE);
    for y in 0..168 {
        for x in 0..112 {
            checker[(x, y)] = if (x / 8 + y / 8) % 2 == 0 {
                Color32::from_rgb(222, 226, 232)
            } else {
                Color32::from_rgb(120, 132, 148)
            };
        }
    }
    let page = |name: &str, letter, number, base, divisor: usize| {
        let mut image = page_image(letter, number, base);
        if opaque {
            for pixel in &mut image.pixels {
                if pixel.a() == 0 {
                    *pixel = base;
                }
            }
        }
        if divisor > 1 {
            // Sample the same repo-native print at preview/thumbnail size.
            // Its lower texel count must never become canonical page geometry.
            let size = [image.size[0] / divisor, image.size[1] / divisor];
            let mut reduced = egui::ColorImage::filled(size, Color32::TRANSPARENT);
            for y in 0..size[1] {
                for x in 0..size[0] {
                    reduced[(x, y)] = image[(x * divisor, y * divisor)];
                }
            }
            image = reduced;
        }
        ctx.load_texture(name, image, TextureOptions::NEAREST)
    };
    let preview = if resident_quality { 2 } else { 1 };
    let thumbnail = if resident_quality { 4 } else { 1 };
    SceneTextures {
        old: [
            page(
                "turn-old-A1",
                'A',
                '1',
                Color32::from_rgb(140, 203, 246),
                preview,
            ),
            page(
                "turn-old-B2",
                'B',
                '2',
                Color32::from_rgb(201, 174, 242),
                preview,
            ),
        ],
        new: [
            page(
                "turn-new-C3",
                'C',
                '3',
                Color32::from_rgb(254, 226, 142),
                thumbnail,
            ),
            page(
                "turn-new-D4",
                'D',
                '4',
                Color32::from_rgb(154, 235, 183),
                preview,
            ),
        ],
        checker: ctx.load_texture("turn-underlay", checker, TextureOptions::NEAREST),
    }
}

fn assert_same_projection_geometry(resident: &TurnProjection, normal: &TurnProjection) {
    assert_eq!(resident.hinge_x, normal.hinge_x);
    assert_eq!(resident.angle_degrees, normal.angle_degrees);
    assert_eq!(resident.showing_back, normal.showing_back);
    assert_eq!(resident.finished, normal.finished);
    assert_eq!(resident.meshes.len(), normal.meshes.len());
    for (resident, normal) in resident.meshes.iter().zip(&normal.meshes) {
        assert_eq!(resident.clip_rect, normal.clip_rect);
        assert_eq!(resident.mesh.indices, normal.mesh.indices);
        assert_eq!(resident.mesh.vertices.len(), normal.mesh.vertices.len());
        for (resident, normal) in resident.mesh.vertices.iter().zip(&normal.mesh.vertices) {
            assert_eq!(resident.pos, normal.pos);
            assert_eq!(resident.uv, normal.uv);
            assert_eq!(resident.color, normal.color);
        }
    }
    assert_eq!(
        resident.visibility.incoming.len(),
        normal.visibility.incoming.len()
    );
    for (resident, normal) in resident
        .visibility
        .incoming
        .iter()
        .zip(&normal.visibility.incoming)
    {
        assert_eq!(resident.occurrence, normal.occurrence);
        assert_eq!(resident.visible_area, normal.visible_area);
        assert_eq!(
            resident.canonical_visible_area,
            normal.canonical_visible_area
        );
        assert_eq!(resident.short_edge, normal.short_edge);
    }
    assert_eq!(
        resident.visibility.outgoing_front_area,
        normal.visibility.outgoing_front_area
    );
}

fn material(texture: &TextureHandle) -> PageMaterial {
    PageMaterial::Texture {
        texture_id: texture.id(),
        uv: [
            pos2(0.0, 0.0),
            pos2(1.0, 0.0),
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
        ],
        available_uv: unit_uv(),
    }
}

fn units(textures: &SceneTextures, spread: bool, cell: Rect) -> (CanonicalUnit, CanonicalUnit) {
    let page_width = if spread { 88.0 } else { 128.0 };
    let unit_width = if spread { 184.0 } else { page_width };
    let left = cell.center().x - unit_width * 0.5;
    let page = |slot, incoming: bool| CanonicalPage {
        occurrence: Some(if incoming {
            3 + slot as u64
        } else {
            1 + slot as u64
        }),
        rect: Rect::from_min_size(
            pos2(left + slot as f32 * 96.0, 72.0),
            vec2(page_width, 168.0),
        ),
        clip_rect: cell,
        material: material(if incoming {
            &textures.new[slot as usize]
        } else {
            &textures.old[slot as usize]
        }),
        underlay: Some(material(&textures.checker)),
    };
    let unit = |incoming| CanonicalUnit {
        pixels_per_point: 1.0,
        spine_x: spread.then_some(cell.center().x),
        gutter_width: if spread { 8.0 } else { 0.0 },
        layout: if spread {
            UnitLayout::Spread
        } else {
            UnitLayout::Single
        },
        pages: (0..if spread { 2 } else { 1 })
            .map(|slot| page(slot, incoming))
            .collect(),
    };
    (unit(false), unit(true))
}

fn paint_canonical(painter: &egui::Painter, unit: &CanonicalUnit) {
    for page in &unit.pages {
        // The product's normal page paint uses this transform's TL->BR
        // diagonal. Painter::image uses the opposite diagonal, which can
        // sample different texels at NEAREST ties despite equal mathematical
        // quads. Compare against the actual normal primitive, not that proxy.
        let transform = DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: page.occurrence.unwrap() as usize,
                viewport_rect: page.clip_rect,
                source_size: page.rect.size(),
                texture_size: page.rect.size(),
                rotation: crate::rotation_db::Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitInput::legacy(crate::settings::FullscreenFitMode::Page),
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.0,
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                pixel_fit: RectPixelFit::Proportional,
            },
            page.rect,
        )
        .unwrap();
        assert_eq!(transform.paint_rect, page.rect);
        for layer in page.underlay.iter().chain(std::iter::once(&page.material)) {
            let PageMaterial::Texture { texture_id, .. } = layer else {
                unreachable!()
            };
            transform.paint_texture(
                &painter.with_clip_rect(page.clip_rect),
                *texture_id,
                Color32::WHITE,
            );
        }
    }
}

fn snapshot_route(spread: bool, reading_rtl: bool, forward: bool) {
    snapshot_scene(spread, reading_rtl, forward, SceneGeometry::Equal).unwrap();
}

fn snapshot_scene(
    spread: bool,
    reading_rtl: bool,
    forward: bool,
    geometry: SceneGeometry,
) -> egui_kittest::SnapshotResults {
    let name = format!(
        "page_turn_projection_{}{}_{}_{}",
        geometry.name(),
        if spread { "spread" } else { "single" },
        if reading_rtl { "rtl" } else { "ltr" },
        if forward { "forward" } else { "back" }
    );
    let canonical_endpoint = Arc::new(AtomicBool::new(false));
    let endpoint_flag = canonical_endpoint.clone();
    // Keep every TextureHandle in the render closure until both GPU images
    // have been consumed. No texture lifetime is inferred from a texture id.
    let mut textures = None;
    let mut normal_textures = None;
    let mut harness = crate::test_ui::builder().with_size(SIZE).build(move |ctx| {
        if textures.is_none() {
            crate::ui_fonts::configure_fonts(ctx);
            let opaque = matches!(geometry, SceneGeometry::OpaqueBlack | SceneGeometry::FixedSingleSpine);
            let resident_quality = matches!(geometry, SceneGeometry::ResidentQuality);
            textures = Some(load_textures(ctx, opaque, resident_quality));
            if resident_quality {
                normal_textures = Some(load_textures(ctx, opaque, false));
            }
            ctx.request_repaint();
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.rect_filled(ctx.content_rect(), 0.0,
            if matches!(geometry, SceneGeometry::OpaqueBlack | SceneGeometry::FixedSingleSpine) { Color32::BLACK } else { Color32::from_rgb(29, 35, 46) });
        painter.text(pos2(12.0, 16.0), egui::Align2::LEFT_TOP,
            format!("{} / {} / {}   old=A1,B2  new=C3,D4  > reads right", if spread { "SPREAD" } else { "SINGLE" },
                if reading_rtl { "RTL" } else { "LTR" }, if forward { "FORWARD" } else { "BACK" }),
            egui::FontId::monospace(14.0), Color32::WHITE);
        for (index, progress) in geometry.poses().into_iter().enumerate() {
            let cell = Rect::from_min_size(pos2(12.0 + index as f32 * CELL_WIDTH, 48.0), vec2(CELL_WIDTH, 208.0));
            if matches!(geometry, SceneGeometry::PortraitPdf) {
                let viewport = Rect::from_min_size(pos2(cell.left(), 72.0), vec2(CELL_WIDTH, CELL_WIDTH * 9.0 / 16.0));
                painter.rect_stroke(viewport, 0.0, egui::Stroke::new(1.0, Color32::from_gray(100)), egui::StrokeKind::Outside);
            }
            painter.text(pos2(cell.center().x, if matches!(geometry, SceneGeometry::TallerIncoming) { 36.0 } else { 48.0 }), egui::Align2::CENTER_TOP, format!("{progress:.4}"),
                egui::FontId::monospace(14.0), Color32::WHITE);
            let (old, mut new) = if matches!(geometry, SceneGeometry::PortraitPdf) {
                let (mut old, mut new) = super::tests::portrait_pdf_fixture(spread);
                let scale = CELL_WIDTH / 2560.0;
                let origin = pos2(cell.left(), 72.0);
                for (unit, incoming) in [(&mut old, false), (&mut new, true)] {
                    unit.pixels_per_point /= scale;
                    unit.spine_x = unit.spine_x.map(|x| origin.x + x * scale);
                    unit.gutter_width *= scale;
                    for (slot, page) in unit.pages.iter_mut().enumerate() {
                        let map = |r: Rect| Rect::from_min_max(origin + r.min.to_vec2() * scale, origin + r.max.to_vec2() * scale);
                        page.rect = map(page.rect);
                        page.clip_rect = map(page.clip_rect);
                        page.material = material(if incoming { &textures.as_ref().unwrap().new[slot] } else { &textures.as_ref().unwrap().old[slot] });
                        page.underlay = Some(material(&textures.as_ref().unwrap().checker));
                    }
                }
                (old, new)
            } else {
                units(textures.as_ref().unwrap(), spread, cell)
            };
            match geometry {
                SceneGeometry::UnequalScan => {
                    // Ordinary independently rounded scan fits: the two
                    // pages keep their own dimensions and tiny offsets.
                    new.pages[0].rect.min.x += 0.35;
                    new.pages[0].rect.max.y -= 0.55;
                    if spread {
                        new.pages[1].rect.max.x -= 0.2;
                        new.pages[1].rect.min.y += 0.4;
                        new.spine_x = new.spine_x.map(|x| x + 0.15);
                    }
                }
                SceneGeometry::TallerIncoming => {
                    // Ratio 600→900 at a fixed spine/width, with the old
                    // rect vertically centered in the incoming footprint.
                    for page in &mut new.pages {
                        let center = page.rect.center();
                        page.rect = Rect::from_center_size(center, vec2(page.rect.width(), 184.0));
                    }
                }
                SceneGeometry::Equal | SceneGeometry::PortraitPdf | SceneGeometry::OpaqueBlack | SceneGeometry::SpineEdge | SceneGeometry::FixedSingleSpine | SceneGeometry::ResidentQuality => {}
            }
            let mut old = old;
            if matches!(geometry, SceneGeometry::TallerIncoming) {
                for page in &mut old.pages {
                    page.rect = Rect::from_center_size(page.rect.center(), vec2(page.rect.width(), 184.0 / 1.5));
                }
            }
            if matches!(geometry, SceneGeometry::OpaqueBlack | SceneGeometry::FixedSingleSpine) {
                for unit in [&mut old, &mut new] {
                    for page in &mut unit.pages {
                        page.underlay = None;
                    }
                }
            }
            if matches!(geometry, SceneGeometry::FixedSingleSpine | SceneGeometry::ResidentQuality) && !spread {
                let moving = if forward { &old.pages[0] } else { &new.pages[0] };
                let hinge = if reading_rtl { moving.rect.right() } else { moving.rect.left() };
                painter.line_segment([pos2(hinge, 64.0), pos2(hinge, 248.0)], egui::Stroke::new(1.0, Color32::from_rgb(241, 92, 104)));
                painter.text(pos2(hinge, 252.0), egui::Align2::CENTER_TOP, "SPINE",
                    egui::FontId::monospace(10.0), Color32::from_rgb(241, 92, 104));
            }
            if index == 4 && endpoint_flag.load(Ordering::Relaxed) {
                paint_canonical(&painter, &new);
            } else {
                let projection = project_turn(&old, &new, TurnDirection { reading_rtl, forward }, progress)
                    .expect("fixed supported scene must project at every sampled pose");
                if let Some(normal_textures) = &normal_textures {
                    let (normal_old, normal_new) = units(normal_textures, spread, cell);
                    let normal = project_turn(&normal_old, &normal_new, TurnDirection { reading_rtl, forward }, progress)
                        .expect("normal-resolution comparison must project at the same pose");
                    assert_same_projection_geometry(&projection, &normal);
                }
                for projected in projection.meshes {
                    painter.with_clip_rect(projected.clip_rect).add(egui::Shape::mesh(projected.mesh));
                }
            }
        }
        painter.text(pos2(12.0, 270.0), egui::Align2::LEFT_TOP,
            if matches!(geometry, SceneGeometry::ResidentQuality) {
                "Resident A1/B2/D4 preview 56x84; C3 thumbnail 28x42. Normal 112x168: identical mesh/UV/spine; last = canonical incoming."
            } else if matches!(geometry, SceneGeometry::FixedSingleSpine) {
                if reading_rtl {
                    "Fixed RIGHT spine in both directions: current departs / previous arrives. Printed > never mirrors. Last = canonical incoming."
                } else {
                    "Fixed LEFT spine in both directions: current departs / previous arrives. Printed > never mirrors. Last = canonical incoming."
                }
            } else if matches!(geometry, SceneGeometry::PortraitPdf) {
                "Portrait PDF fit in outlined 16:9 fullscreen; letterbox and real paint-grid gap. Last pose = canonical incoming."
            } else if matches!(geometry, SceneGeometry::TallerIncoming) {
                "Height 600 -> 900: p=0 and next clock pose; outer footprint grows continuously. Last = canonical incoming."
            } else if matches!(geometry, SceneGeometry::SpineEdge) {
                "Near 90 degrees: front -> thin paper edge -> printed back; the resting page never turns. Last = canonical incoming."
            } else if matches!(geometry, SceneGeometry::OpaqueBlack) {
                "Opaque printed pages on black: shading stays readable, with no background band used as paper. Last = canonical incoming."
            } else {
                "One leaf rotates at the spine; checker stays under transparent print. Last pose = canonical incoming."
            },
            egui::FontId::monospace(12.0), Color32::from_rgb(206, 216, 228));
    });
    harness.run();
    harness.snapshot(&name);
    let projected = harness.render().unwrap();
    canonical_endpoint.store(true, Ordering::Relaxed);
    harness.step();
    harness.run();
    let canonical = harness.render().unwrap();
    if projected != canonical {
        let evidence = std::path::Path::new("target/page-turn-endpoint-debug");
        std::fs::create_dir_all(evidence).unwrap();
        projected
            .save(evidence.join(format!("{name}-projected.png")))
            .unwrap();
        canonical
            .save(evidence.join(format!("{name}-canonical.png")))
            .unwrap();
    }
    assert!(
        projected == canonical,
        "finished projected endpoint must render exactly like canonical incoming: {name}"
    );
    harness.take_snapshot_results()
}

fn moving_spine_units(textures: &SceneTextures, cell: Rect) -> (CanonicalUnit, CanonicalUnit) {
    // Two portrait scans exchange their wide/narrow physical slots. Normal
    // layout centers each total width independently, so the spine moves by
    // 38 points rather than remaining at the viewport center. Heights also
    // differ independently; none of these sizes come from texture resolution.
    let unit = |incoming: bool| {
        let sizes = if incoming {
            [vec2(104.0, 180.0), vec2(66.0, 164.0)]
        } else {
            [vec2(68.0, 170.0), vec2(106.0, 182.0)]
        };
        let gutter = 8.0;
        let left = cell.center().x - (sizes[0].x + gutter + sizes[1].x) * 0.5;
        CanonicalUnit {
            pixels_per_point: 1.0,
            spine_x: Some(left + sizes[0].x + gutter * 0.5),
            gutter_width: gutter,
            layout: UnitLayout::Spread,
            pages: sizes
                .into_iter()
                .enumerate()
                .map(|(slot, size)| CanonicalPage {
                    occurrence: Some(if incoming { 3 } else { 1 } + slot as u64),
                    rect: Rect::from_min_size(
                        pos2(
                            left + if slot == 0 { 0.0 } else { sizes[0].x + gutter },
                            cell.center().y - size.y * 0.5,
                        ),
                        size,
                    ),
                    clip_rect: cell,
                    material: material(if incoming {
                        &textures.new[slot]
                    } else {
                        &textures.old[slot]
                    }),
                    underlay: Some(material(&textures.checker)),
                })
                .collect(),
        }
    };
    (unit(false), unit(true))
}

fn snapshot_moving_spine(reading_rtl: bool, forward: bool) -> egui_kittest::SnapshotResults {
    const MOVING_POSES: [f32; 9] = [0.0, 0.0001, 0.2, 0.49, 0.5, 0.51, 0.8, 0.9999, 1.0];
    // Keep the widest translated resting page inside its destination. A
    // viewport crop must not hide loss of support at the moving spine.
    const MOVING_CELL_WIDTH: f32 = 260.0;
    let name = format!(
        "page_turn_projection_moving_spine_spread_{}_{}",
        if reading_rtl { "rtl" } else { "ltr" },
        if forward { "forward" } else { "back" }
    );
    let canonical_endpoint = Arc::new(AtomicBool::new(false));
    let endpoint_flag = canonical_endpoint.clone();
    let mut textures = None;
    let mut harness = crate::test_ui::builder()
        .with_size(vec2(1324.0, 520.0))
        .build(move |ctx| {
            if textures.is_none() {
                crate::ui_fonts::configure_fonts(ctx);
                textures = Some(load_textures(ctx, false, false));
                ctx.request_repaint();
                return;
            }
            let painter = ctx.layer_painter(egui::LayerId::background());
            painter.rect_filled(ctx.content_rect(), 0.0, Color32::from_rgb(29, 35, 46));
            painter.text(
                pos2(12.0, 16.0),
                egui::Align2::LEFT_TOP,
                format!(
                    "MOVING SPREAD SPINE / {} / {}   old=A1,B2  new=C3,D4  > reads right",
                    if reading_rtl { "RTL" } else { "LTR" },
                    if forward { "FORWARD" } else { "BACK" }
                ),
                egui::FontId::monospace(14.0),
                Color32::WHITE,
            );
            for index in 0..10 {
                let row = index / 5;
                let cell = Rect::from_min_size(
                    pos2(12.0 + (index % 5) as f32 * MOVING_CELL_WIDTH, 64.0 + row as f32 * 212.0),
                    vec2(MOVING_CELL_WIDTH, 208.0),
                );
                let (old, new) = moving_spine_units(textures.as_ref().unwrap(), cell);
                let label = if index == 0 {
                    "canonical old".to_owned()
                } else {
                    format!("{:.4}", MOVING_POSES[index - 1])
                };
                painter.text(
                    pos2(cell.center().x, cell.top() - 16.0),
                    egui::Align2::CENTER_TOP,
                    label,
                    egui::FontId::monospace(14.0),
                    Color32::WHITE,
                );
                if index == 0 {
                    paint_canonical(&painter, &old);
                } else if index == 9 && endpoint_flag.load(Ordering::Relaxed) {
                    paint_canonical(&painter, &new);
                } else {
                    let projection = project_turn(
                        &old,
                        &new,
                        TurnDirection { reading_rtl, forward },
                        MOVING_POSES[index - 1],
                    )
                    .expect("portrait width swap must animate at every sampled pose");
                    for projected in projection.meshes {
                        painter
                            .with_clip_rect(projected.clip_rect)
                            .add(egui::Shape::mesh(projected.mesh));
                    }
                }
            }
            painter.text(
                pos2(12.0, 490.0),
                egui::Align2::LEFT_TOP,
                "Portrait widths 68/106 -> 104/66; heights 170/182 -> 180/164; moving spine and full printed support. Last = canonical incoming.",
                egui::FontId::monospace(12.0),
                Color32::from_rgb(206, 216, 228),
            );
        });
    harness.run();
    harness.snapshot(&name);
    let projected = harness.render().unwrap();
    canonical_endpoint.store(true, Ordering::Relaxed);
    harness.step();
    harness.run();
    let canonical = harness.render().unwrap();
    if projected != canonical {
        let evidence = std::path::Path::new("target/page-turn-endpoint-debug");
        std::fs::create_dir_all(evidence).unwrap();
        projected
            .save(evidence.join(format!("{name}-projected.png")))
            .unwrap();
        canonical
            .save(evidence.join(format!("{name}-canonical.png")))
            .unwrap();
    }
    assert!(
        projected == canonical,
        "moving-spine endpoint must render exactly like canonical incoming: {name}"
    );
    harness.take_snapshot_results()
}

#[test]
fn page_turn_projection_snapshot_moving_spine_four_routes() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for reading_rtl in [false, true] {
        for forward in [false, true] {
            snapshots.extend(snapshot_moving_spine(reading_rtl, forward));
        }
    }
    snapshots.unwrap();
}

#[test]
fn page_turn_projection_snapshot_pdf_letterbox_single_ltr() {
    snapshot_scene(false, false, true, SceneGeometry::PortraitPdf).unwrap();
}

#[test]
fn page_turn_projection_snapshot_pdf_letterbox_single_rtl() {
    snapshot_scene(false, true, true, SceneGeometry::PortraitPdf).unwrap();
}

#[test]
fn page_turn_projection_snapshot_pdf_letterbox_spread_ltr() {
    snapshot_scene(true, false, true, SceneGeometry::PortraitPdf).unwrap();
}

#[test]
fn page_turn_projection_snapshot_pdf_letterbox_spread_rtl() {
    snapshot_scene(true, true, true, SceneGeometry::PortraitPdf).unwrap();
}

#[test]
fn page_turn_projection_snapshot_unequal_scans() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for reading_rtl in [false, true] {
        for forward in [false, true] {
            snapshots.extend(snapshot_scene(
                true,
                reading_rtl,
                forward,
                SceneGeometry::UnequalScan,
            ));
        }
    }
    snapshots.unwrap();
}

#[test]
fn page_turn_projection_snapshot_different_height_continuity() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for spread in [false, true] {
        for (reading_rtl, forward) in [(false, true), (true, false)] {
            snapshots.extend(snapshot_scene(
                spread,
                reading_rtl,
                forward,
                SceneGeometry::TallerIncoming,
            ));
        }
    }
    snapshots.unwrap();
}

#[test]
fn page_turn_projection_snapshot_opaque_black() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    snapshots.extend(snapshot_scene(true, true, true, SceneGeometry::OpaqueBlack));
    snapshots.extend(snapshot_scene(
        false,
        false,
        false,
        SceneGeometry::OpaqueBlack,
    ));
    snapshots.unwrap();
}

#[test]
fn page_turn_projection_snapshot_spine_edge() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    snapshots.extend(snapshot_scene(true, false, true, SceneGeometry::SpineEdge));
    snapshots.extend(snapshot_scene(true, true, true, SceneGeometry::SpineEdge));
    snapshots.unwrap();
}

#[test]
fn page_turn_projection_snapshot_fixed_single_spine_four_routes() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    for reading_rtl in [false, true] {
        for forward in [false, true] {
            snapshots.extend(snapshot_scene(
                false,
                reading_rtl,
                forward,
                SceneGeometry::FixedSingleSpine,
            ));
        }
    }
    snapshots.unwrap();
}

#[test]
fn page_turn_projection_snapshot_resident_quality() {
    let mut snapshots = egui_kittest::SnapshotResults::default();
    // All single routes exercise fixed-spine outgoing departure and incoming
    // arrival with visibly different resident resolutions. Two spreads expose
    // both moving slots, including low-resolution front/back/revealed faces.
    for reading_rtl in [false, true] {
        for forward in [false, true] {
            snapshots.extend(snapshot_scene(
                false,
                reading_rtl,
                forward,
                SceneGeometry::ResidentQuality,
            ));
        }
        snapshots.extend(snapshot_scene(
            true,
            reading_rtl,
            true,
            SceneGeometry::ResidentQuality,
        ));
    }
    snapshots.unwrap();
}

macro_rules! route_test {
    ($name:ident, $spread:expr, $rtl:expr, $forward:expr) => {
        #[test]
        fn $name() {
            snapshot_route($spread, $rtl, $forward);
        }
    };
}

route_test!(
    page_turn_projection_snapshot_single_ltr_forward,
    false,
    false,
    true
);
route_test!(
    page_turn_projection_snapshot_single_ltr_back,
    false,
    false,
    false
);
route_test!(
    page_turn_projection_snapshot_single_rtl_forward,
    false,
    true,
    true
);
route_test!(
    page_turn_projection_snapshot_single_rtl_back,
    false,
    true,
    false
);
route_test!(
    page_turn_projection_snapshot_spread_ltr_forward,
    true,
    false,
    true
);
route_test!(
    page_turn_projection_snapshot_spread_ltr_back,
    true,
    false,
    false
);
route_test!(
    page_turn_projection_snapshot_spread_rtl_forward,
    true,
    true,
    true
);
route_test!(
    page_turn_projection_snapshot_spread_rtl_back,
    true,
    true,
    false
);
