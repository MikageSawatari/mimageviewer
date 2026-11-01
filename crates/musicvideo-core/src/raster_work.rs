use super::*;

const TILE_EDGE: usize = 256;
const WORK_BYTES_PER_OUTPUT_PIXEL: u64 = 24;

/// Geometry never changes to satisfy this storage budget. Oversized work is
/// evaluated through inverse-viewport tiles instead of a full layer bitmap.
#[derive(Clone, Copy)]
pub(super) struct WorkPlan {
    geometry: LayerPixelRect,
    width: u32,
    height: u32,
    rotation: f32,
}

#[derive(Clone, Copy, Debug)]
struct WorkTile {
    destination: PixelRect,
    source: PixelRect,
}

impl WorkPlan {
    pub(super) fn new(geometry: LayerPixelRect, width: u32, height: u32, rotation: f32) -> Self {
        Self {
            geometry,
            width,
            height,
            rotation,
        }
    }

    pub(super) fn full_work_fits(self, glow: Option<Glow>) -> Result<bool, RasterError> {
        let rect = self.geometry.local();
        let (sin, cos) = f64::from(self.rotation).to_radians().sin_cos();
        let w = rect.width() as f64;
        let h = rect.height() as f64;
        let rw = (w * cos.abs() + h * sin.abs()).ceil() + 2.0;
        let rh = (h * cos.abs() + w * sin.abs()).ceil() + 2.0;
        let pad = match glow {
            Some(g) => GlowRasterPlan::new(g)?.pad as f64,
            None => 0.0,
        };
        // Body RGBA, mask construction, rounded keyboard coverage and rotation;
        // padded Glow alpha and its reduced filter/transpose intermediates.
        let estimate = w * h * 12.0
            + rw * rh * 2.0
            + if glow.is_some() {
                (rw + pad * 2.0) * (rh + pad * 2.0) * 6.0
            } else {
                0.0
            };
        let pixels = u64::from(self.width) * u64::from(self.height);
        let edge_limit = f64::from(self.width.max(self.height).max(1)) * 4.0;
        Ok(w <= edge_limit
            && h <= edge_limit
            && estimate <= pixels.max(1) as f64 * WORK_BYTES_PER_OUTPUT_PIXEL as f64)
    }

    fn tiles(self) -> impl Iterator<Item = WorkTile> {
        let edge = (self.width.min(self.height) as usize).clamp(1, TILE_EDGE);
        (0..self.height as usize).step_by(edge).flat_map(move |y| {
            (0..self.width as usize).step_by(edge).map(move |x| {
                let destination = PixelRect {
                    x0: x,
                    y0: y,
                    x1: (x + edge).min(self.width as usize),
                    y1: (y + edge).min(self.height as usize),
                };
                WorkTile {
                    destination,
                    source: self.inverse_window(destination),
                }
            })
        })
    }

    fn inverse_window(self, destination: PixelRect) -> PixelRect {
        self.inverse_world_window(
            destination.x0 as i64,
            destination.y0 as i64,
            destination.x1 as i64,
            destination.y1 as i64,
        )
    }

    fn inverse_world_window(self, left: i64, top: i64, right: i64, bottom: i64) -> PixelRect {
        let local = self.geometry.local();
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        let center_x = (f64::from(self.geometry.x0) + f64::from(self.geometry.x1)) * 0.5;
        let center_y = (f64::from(self.geometry.y0) + f64::from(self.geometry.y1)) * 0.5;
        let (sin, cos) = (-f64::from(self.rotation).to_radians()).sin_cos();
        for (x, y) in [(left, top), (right, top), (left, bottom), (right, bottom)] {
            let dx = x as f64 - center_x;
            let dy = y as f64 - center_y;
            let sx = center_x + dx * cos - dy * sin - f64::from(self.geometry.x0);
            let sy = center_y + dx * sin + dy * cos - f64::from(self.geometry.y0);
            x0 = x0.min(sx);
            y0 = y0.min(sy);
            x1 = x1.max(sx);
            y1 = y1.max(sy);
        }
        let support = if self.rotation.abs() > 1.0e-4 {
            2.0
        } else {
            0.0
        };
        let x0 = (x0.floor() - support).clamp(0.0, local.width() as f64) as usize;
        let y0 = (y0.floor() - support).clamp(0.0, local.height() as f64) as usize;
        let x1 = (x1.ceil() + support).clamp(0.0, local.width() as f64) as usize;
        let y1 = (y1.ceil() + support).clamp(0.0, local.height() as f64) as usize;
        PixelRect {
            x0,
            y0,
            x1: x1.max(x0),
            y1: y1.max(y0),
        }
    }
}

/// Native-resolution body rendering. Storage coordinates retain their position
/// in the complete local rectangle, including when only a small patch is stored.
#[cfg(test)]
pub(super) fn draw_tiled_body(
    frame: &mut Frame,
    plan: WorkPlan,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
) -> Result<(), RasterError> {
    draw_tiled_body_impl(frame, plan, item, assets, report, false)
}

pub(super) fn draw_tiled_item(
    frame: &mut Frame,
    plan: WorkPlan,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
    glow: Option<Glow>,
) -> Result<(), RasterError> {
    if let Some(glow) = glow {
        draw_tiled_glow(frame, plan, item, assets, report, glow)?;
    }
    draw_tiled_body_impl(frame, plan, item, assets, report, glow.is_some())
}

fn draw_tiled_body_impl(
    frame: &mut Frame,
    plan: WorkPlan,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
    flatten_solid: bool,
) -> Result<(), RasterError> {
    let local = plan.geometry.local();
    let rotated = plan.rotation.abs() > 1.0e-4;
    let geometry_mask = MaskSurface {
        origin_x: plan.geometry.x0,
        origin_y: plan.geometry.y0,
        width: local.width(),
        height: local.height(),
        alpha: Vec::new(),
    };
    for tile in plan.tiles().filter(|tile| !tile.source.is_empty()) {
        if !rotated && flatten_solid && solid_body_color(item).is_some() {
            let mut source = Frame::window(tile.source)?;
            source.rgba.fill(0);
            draw_item(&mut source, local, item, assets, report, true)?;
            let mask = MaskSurface {
                origin_x: (i64::from(plan.geometry.x0) + tile.source.x0 as i64) as i32,
                origin_y: (i64::from(plan.geometry.y0) + tile.source.y0 as i64) as i32,
                width: tile.source.width(),
                height: tile.source.height(),
                alpha: source.rgba.chunks_exact(4).map(|p| p[3]).collect(),
            };
            composite_solid_body(frame, &mask, item);
            continue;
        }
        let mut body = Frame::window(tile.source)?;
        if !rotated {
            for y in tile.source.y0..tile.source.y1 {
                let global_y = (y as i64 + i64::from(plan.geometry.y0)) as usize;
                let global_x = (tile.source.x0 as i64 + i64::from(plan.geometry.x0)) as usize;
                let from = frame.offset(global_x, global_y);
                let to = body.offset(tile.source.x0, y);
                let bytes = tile.source.width() * 4;
                body.rgba[to..to + bytes].copy_from_slice(&frame.rgba[from..from + bytes]);
            }
            draw_item(&mut body, local, item, assets, report, false)?;
            for y in tile.source.y0..tile.source.y1 {
                let global_y = (y as i64 + i64::from(plan.geometry.y0)) as usize;
                let global_x = (tile.source.x0 as i64 + i64::from(plan.geometry.x0)) as usize;
                let to = frame.offset(global_x, global_y);
                let from = body.offset(tile.source.x0, y);
                let bytes = tile.source.width() * 4;
                frame.rgba[to..to + bytes].copy_from_slice(&body.rgba[from..from + bytes]);
            }
            continue;
        }
        draw_item(&mut body, local, item, assets, report, false)?;
        let mut mask = Frame::window(tile.source)?;
        mask.rgba.fill(0);
        draw_item(&mut mask, local, item, assets, report, true)?;
        for y in tile.destination.y0..tile.destination.y1 {
            for x in tile.destination.x0..tile.destination.x1 {
                let (sx, sy) = inverse_rotated_source(
                    &geometry_mask,
                    x as f32 + 0.5,
                    y as f32 + 0.5,
                    plan.rotation,
                );
                if sx < -0.5
                    || sy < -0.5
                    || sx > local.width() as f32 - 0.5
                    || sy > local.height() as f32 - 0.5
                {
                    continue;
                }
                // Clamp against the full geometry, never the tile's edges.
                let sx =
                    sx.clamp(0.0, local.width().saturating_sub(1) as f32) - tile.source.x0 as f32;
                let sy =
                    sy.clamp(0.0, local.height().saturating_sub(1) as f32) - tile.source.y0 as f32;
                let alpha = sample_bilinear_rgba_channel(
                    &mask.rgba,
                    mask.width as usize,
                    mask.height as usize,
                    sx,
                    sy,
                    3,
                ) as u32;
                if alpha == 0 {
                    continue;
                }
                let offset = frame.offset(x, y);
                for channel in 0..3 {
                    let premultiplied = sample_bilinear_rgba_channel(
                        &body.rgba,
                        body.width as usize,
                        body.height as usize,
                        sx,
                        sy,
                        channel,
                    ) as u32;
                    frame.rgba[offset + channel] = (premultiplied
                        + (frame.rgba[offset + channel] as u32 * (255 - alpha) + 127) / 255)
                        .min(255) as u8;
                }
            }
        }
    }
    Ok(())
}

/// Generate the post-rotation silhouette only for a native-resolution viewport
/// window. Each source patch retains the full geometry and rotation center.
fn render_world_mask(
    plan: WorkPlan,
    window: GlowRect,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
) -> Result<MaskSurface, RasterError> {
    let mut alpha = vec![0; alpha_len(window.width, window.height)?];
    let local = plan.geometry.local();
    let geometry_mask = MaskSurface {
        origin_x: plan.geometry.x0,
        origin_y: plan.geometry.y0,
        width: local.width(),
        height: local.height(),
        alpha: Vec::new(),
    };
    let edge = (plan.width.min(plan.height) as usize).clamp(1, TILE_EDGE);
    let bottom = i64::from(window.y0) + window.height as i64;
    let right = i64::from(window.x0) + window.width as i64;
    for top in (i64::from(window.y0)..bottom).step_by(edge) {
        for left in (i64::from(window.x0)..right).step_by(edge) {
            let tile_right = (left + edge as i64).min(right);
            let tile_bottom = (top + edge as i64).min(bottom);
            let source = plan.inverse_world_window(left, top, tile_right, tile_bottom);
            if source.is_empty() {
                continue;
            }
            let mut mask = Frame::window(source)?;
            mask.rgba.fill(0);
            draw_item(&mut mask, local, item, assets, report, true)?;
            for y in top..tile_bottom {
                for x in left..tile_right {
                    let value = if plan.rotation.abs() <= 1.0e-4 {
                        let sx = x - i64::from(plan.geometry.x0);
                        let sy = y - i64::from(plan.geometry.y0);
                        if sx < 0
                            || sy < 0
                            || sx >= local.width() as i64
                            || sy >= local.height() as i64
                        {
                            0
                        } else {
                            mask.rgba[mask.offset(sx as usize, sy as usize) + 3]
                        }
                    } else {
                        let (sx, sy) = inverse_rotated_source(
                            &geometry_mask,
                            x as f32 + 0.5,
                            y as f32 + 0.5,
                            plan.rotation,
                        );
                        if sx < -0.5
                            || sy < -0.5
                            || sx > local.width() as f32 - 0.5
                            || sy > local.height() as f32 - 0.5
                        {
                            0
                        } else {
                            let sx = sx.clamp(0.0, local.width().saturating_sub(1) as f32)
                                - source.x0 as f32;
                            let sy = sy.clamp(0.0, local.height().saturating_sub(1) as f32)
                                - source.y0 as f32;
                            sample_bilinear_rgba_channel(
                                &mask.rgba,
                                mask.width as usize,
                                mask.height as usize,
                                sx,
                                sy,
                                3,
                            )
                        }
                    };
                    alpha[(y - i64::from(window.y0)) as usize * window.width
                        + (x - i64::from(window.x0)) as usize] = value;
                }
            }
        }
    }
    Ok(MaskSurface {
        origin_x: window.x0,
        origin_y: window.y0,
        width: window.width,
        height: window.height,
        alpha,
    })
}

fn draw_tiled_glow(
    frame: &mut Frame,
    plan: WorkPlan,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
    glow: Glow,
) -> Result<(), RasterError> {
    let filter = GlowRasterPlan::new(glow)?;
    // The next bilinear reduced cell and max-pool block both contribute to a
    // destination pixel. Include their complete support, then align to the same
    // layer-origin grid as the full allocation path.
    let halo = (filter.pad + filter.scale) as i64;
    let anchor = (plan.geometry.x0, plan.geometry.y0);
    for tile in plan.tiles() {
        let window = GlowRect::aligned(
            tile.destination.x0 as i64 - halo,
            tile.destination.y0 as i64 - halo,
            tile.destination.x1 as i64 + halo,
            tile.destination.y1 as i64 + halo,
            anchor,
            filter.scale,
        )?;
        let mask = render_world_mask(plan, window, item, assets, report)?;
        filter_and_composite_glow(frame, mask, filter, glow.color, tile.destination)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "raster_glow_work_tests.rs"]
mod glow_work_tests;

#[cfg(test)]
mod work_tests {
    use super::*;
    use crate::*;

    #[test]
    fn p5a_streamed_overlapping_spectrum_mask_matches_painted_reference() {
        let rect = PixelRect {
            x0: 0,
            y0: 0,
            x1: 64,
            y1: 48,
        };
        let stops = GradientStops::new(vec![
            GradientStop {
                position: 0.0,
                color: Rgba8::new(40, 80, 120, 55),
            },
            GradientStop {
                position: 1.0,
                color: Rgba8::new(200, 160, 80, 210),
            },
        ])
        .unwrap();
        for shape in [
            SpectrumShape::Bars {
                gap_ratio: 0.0,
                min_height_px: 8.0,
                thickness_px: 40.0,
            },
            SpectrumShape::Radial {
                inner_ratio: 0.3,
                thickness_px: 30.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            },
        ] {
            for color_mode in [MusicColorMode::Custom, MusicColorMode::Miv] {
                for fill in [
                    Fill::Solid(Rgba8::new(90, 150, 200, 135)),
                    Fill::BandGradient {
                        stops: stops.clone(),
                    },
                ] {
                    let item = SceneItem {
                        rect: NormRect::FULL,
                        opacity: 0.55,
                        rotation_deg: 0.0,
                        corner_radius_px: 5.0,
                        glow: None,
                        kind: SceneItemKind::Spectrum {
                            values: vec![0.2, 0.7, 0.4, 1.0],
                            miv_frame: MivSpectrumFrame::default(),
                            miv_dimensions: MivSpectrumDimensions::default(),
                            minimum_thickness_px: 1.0,
                            point_count: 64,
                            shape,
                            color_mode,
                            miv_brightness: 0.0,
                            fill,
                        },
                    };
                    let assets = Assets::default();
                    let mask = render_item_mask(rect, &item, &assets, &mut RenderReport::default())
                        .unwrap();
                    let painted = render_item_mask_fallback(
                        rect,
                        &item,
                        &assets,
                        &mut RenderReport::default(),
                    )
                    .unwrap();
                    assert!(mask.alpha.iter().any(|&a| a > 0));
                    assert_eq!(mask.alpha, painted.alpha, "{shape:?} {color_mode:?}");
                }
            }
        }
    }

    #[test]
    fn p5a_render_frame_scale_99_no_glow_limits_frame_dimensions() {
        // Check the probe itself with tiny dimensions: denial is pre-allocation,
        // other test threads are unaffected, and scope exit restores no limit.
        let ((denied, unrelated), dimensions) = frame_allocation_probe::with_limits(1, 1, || {
            let denied = Frame::new(2, 2);
            let unrelated = std::thread::spawn(|| Frame::new(2, 2).is_ok())
                .join()
                .unwrap();
            (denied, unrelated)
        });
        assert_eq!(denied, Err(RasterError::DimensionsOverflow));
        assert!(unrelated);
        assert_eq!(dimensions, [(2, 2)]);
        assert!(Frame::new(2, 2).is_ok());

        let mut project = default_project(10.0);
        project.output.width = 1920;
        project.output.height = 1080;
        let mut layer = new_solid_layer(&project);
        layer.rect = Animatable::Const(NormRect::new(0.0, 0.0, 1.0, 1.0));
        layer.opacity = Animatable::Const(1.0);
        layer.corner_radius = Animatable::Const(0.0);
        layer.glow = Animatable::Const(None);
        layer.modulations = vec![ModulationBinding {
            target: ModulationTarget::Scale,
            amount: 99.0,
            modulator: ModulatorId(1),
            curve: ModulationCurve::Linear,
        }];
        project.layers = vec![layer];
        let assets = Assets::default();
        let output_pixels = u64::from(project.output.width) * u64::from(project.output.height);
        let edge_limit = project.output.width.max(project.output.height) * 4;
        for angle in [0.0, 23.0, 90.0] {
            project.layers[0].rotation = Animatable::Const(angle);
            let scene = evaluate(&project, 2.5, &crate::p5a_tests::frame());
            assert_eq!(
                scene.items[0].rect,
                NormRect::new(-49.5, -49.5, 100.0, 100.0)
            );
            assert!(scene.items[0].glow.is_none());
            let mut frame = Frame::new(1, 1).unwrap();
            let (result, dimensions) =
                frame_allocation_probe::with_limits(output_pixels * 3, edge_limit, || {
                    render_frame(&scene, &assets, &mut frame)
                });
            assert!(
                result.is_ok(),
                "angle {angle}: {result:?}; requested {dimensions:?}"
            );
            assert!(result.unwrap().unresolved_assets.is_empty());
            assert_eq!(dimensions.first(), Some(&(scene.width, scene.height)));
            assert!(
                dimensions.len() > 1,
                "must exercise work Frames at angle {angle}"
            );
            for &(width, height) in &dimensions {
                assert!(u64::from(width) * u64::from(height) <= output_pixels * 3);
                assert!(width.max(height) <= edge_limit);
            }
            assert!(
                frame
                    .rgba
                    .chunks_exact(4)
                    .all(|pixel| pixel == [64, 120, 220, 255]),
                "the enormous solid must cover the complete output at angle {angle}"
            );
        }
    }

    #[test]
    fn p5a_inverse_viewport_bounds_scale_99_work_without_allocating_layer() {
        for (width, height) in [(1920, 1080), (8, 8192), (8192, 8)] {
            let geometry = LayerPixelRect {
                x0: -(width as i32) * 49,
                y0: -(height as i32) * 49,
                x1: width as i32 * 51,
                y1: height as i32 * 51,
            };
            for angle in [0.0, 17.0, 45.0, 90.0] {
                let plan = WorkPlan::new(geometry, width, height, angle);
                for glow in [
                    None,
                    Some(Glow {
                        radius_px: 3200.0,
                        spread_px: 1200.0,
                        color: Rgba8::WHITE,
                    }),
                ] {
                    assert!(!plan.full_work_fits(glow).unwrap());
                }
                let output_pixels = u64::from(width) * u64::from(height);
                let mut scan = 0u64;
                for tile in plan.tiles() {
                    let pixels = (tile.source.width() * tile.source.height()) as u64;
                    assert!(
                        pixels <= output_pixels * 3,
                        "{width}x{height} @ {angle}: {tile:?}"
                    );
                    scan += pixels;
                }
                assert!(scan <= output_pixels * 5, "scan {scan} vs {output_pixels}");
            }
        }
    }

    #[test]
    fn p5a_tiled_body_matches_full_unclipped_geometry_all_kinds() {
        let assets = crate::p5a_tests::assets();
        let mut project = default_project(10.0);
        project.output.width = 80;
        project.output.height = 48;
        let layers = vec![
            new_image_layer(&project, ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID)),
            new_solid_layer(&project),
            new_radial_spectrum_layer(&project),
            new_linear_spectrum_layer(&project),
            new_miv_spectrum_layer(&project),
            new_pitch_keyboard_layer(&project),
            new_progress_bar_layer(&project),
            new_time_text_layer(&project),
            new_text_layer(&project),
            new_lyrics_layer(&project),
        ];
        let mut audio = crate::p5a_tests::frame();
        audio.miv_spectrum.bands = vec![
            music_core::MivSpectrumBandFrame {
                value: 0.6,
                trail: 0.8,
                onset: 0.7,
            };
            96
        ];
        for (kind, mut layer) in layers.into_iter().enumerate() {
            layer.rect = Animatable::Const(NormRect::new(-0.4, -0.25, 1.5, 1.4));
            layer.opacity = Animatable::Const(0.65);
            layer.glow = Animatable::Const(None);
            if let LayerKind::Lyrics(text) = &mut layer.kind {
                text.lines = vec![LyricLine {
                    text: "Lyrics".into(),
                    time_secs: Some(0.0),
                }];
            }
            match &mut layer.kind {
                LayerKind::Text(text) => {
                    text.size_px = 18.0;
                    text.style = Animatable::Const(crate::p5a_tests::decorated_style());
                }
                LayerKind::TimeText(text) => {
                    text.size_px = 18.0;
                    text.style = Animatable::Const(crate::p5a_tests::decorated_style());
                }
                LayerKind::Lyrics(text) => {
                    text.size_px = 18.0;
                    text.style = Animatable::Const(crate::p5a_tests::decorated_style());
                }
                _ => {}
            }
            project.layers = vec![layer];
            let scene = evaluate(&project, 2.5, &audio);
            let item = &scene.items[0];
            let geometry = LayerPixelRect::new(item.rect, scene.width, scene.height).unwrap();
            for angle in [0.0, 23.0, 90.0] {
                let mut full = Frame::new(scene.width, scene.height).unwrap();
                let mut tiled = full.clone();
                // A colored backdrop detects accidental flattening/alpha rounding.
                for frame in [&mut full, &mut tiled] {
                    for pixel in frame.rgba.chunks_exact_mut(4) {
                        pixel.copy_from_slice(&[17, 23, 31, 255]);
                    }
                }
                let mut a = RenderReport::default();
                let mut b = a.clone();
                draw_unclipped_item_full(&mut full, geometry, item, &assets, &mut a, angle, None)
                    .unwrap();
                draw_tiled_body(
                    &mut tiled,
                    WorkPlan::new(geometry, scene.width, scene.height, angle),
                    item,
                    &assets,
                    &mut b,
                )
                .unwrap();
                assert_eq!(a, b, "report kind {kind}");
                assert_eq!(full.rgba, tiled.rgba, "kind {kind}, angle {angle}");
            }
        }
    }
}
