//! Pure slide geometry and endpoint paint regressions. No app or OS input.
use super::*;

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> Rect {
    Rect::from_min_max(pos2(left, top), pos2(right, bottom))
}

fn page(id: u64, placement: Rect) -> CanonicalPage {
    CanonicalPage {
        occurrence: Some(id),
        rect: placement,
        clip_rect: rect(0.0, 0.0, 640.0, 480.0),
        material: PageMaterial::Texture {
            texture_id: TextureId::User(id),
            uv: rectangle(unit_uv()).try_into().unwrap(),
            available_uv: unit_uv(),
        },
        underlay: Some(PageMaterial::Blank {
            color: Color32::WHITE,
        }),
        crop: None,
    }
}

fn unit(spread: bool, incoming: bool) -> CanonicalUnit {
    let base = if incoming { 11 } else { 1 };
    CanonicalUnit {
        layout: if spread {
            UnitLayout::Spread
        } else {
            UnitLayout::Single
        },
        pixels_per_point: 1.0,
        spine_x: spread.then_some(320.0),
        gutter_width: if spread { 20.0 } else { 0.0 },
        pages: if spread {
            vec![
                page(base, rect(40.0, 40.0, 310.0, 440.0)),
                page(base + 1, rect(330.0, 40.0, 600.0, 440.0)),
            ]
        } else {
            vec![page(base, rect(200.0, 40.0, 440.0, 440.0))]
        },
    }
}

fn routes() -> impl Iterator<Item = TurnDirection> {
    [false, true].into_iter().flat_map(|reading_rtl| {
        [false, true].into_iter().map(move |forward| TurnDirection {
            reading_rtl,
            forward,
        })
    })
}

fn crop() -> PageCropPaint {
    PageCropPaint {
        full: rectangle(unit_uv()).try_into().unwrap(),
        corners: rectangle(rect(0.18, 0.23, 0.82, 0.79)).try_into().unwrap(),
    }
}

fn assert_close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() <= 0.2, "{actual} != {expected}");
}

fn check_trajectory(old: &CanonicalUnit, new: &CanonicalUnit, direction: TurnDirection) {
    let recipe = prepare_slide(old, new, direction).unwrap();
    let mut last_area = f32::INFINITY;
    for step in 0..=100 {
        let progress = step as f32 / 100.0;
        let pose = slide_scene(old, new, direction, recipe, progress);
        let result = project_slide(old, new, direction, recipe, progress, false).unwrap();
        assert!(pose.leaf.is_none() && pose.black_floor.is_none());
        assert_eq!(pose.shadow, 0.0);
        assert_eq!(result.finished, step == 100);
        assert!(!result.showing_back);
        assert!(
            result
                .meshes
                .iter()
                .map(|m| m.mesh.vertices.len())
                .sum::<usize>()
                <= MAX_PROJECTION_VERTICES
        );
        assert!(
            result
                .meshes
                .iter()
                .map(|m| m.mesh.indices.len() / 3)
                .sum::<usize>()
                <= MAX_PROJECTION_TRIANGLES
        );
        for surface in &pose.surfaces {
            assert!(!vacant(surface.page));
            assert_eq!(surface.alpha, 1.0);
            assert_close(surface.rect.width(), surface.page.rect.width());
            assert_eq!(surface.rect.top(), surface.page.rect.top());
            assert_eq!(surface.rect.bottom(), surface.page.rect.bottom());
            let incoming = new.pages.iter().any(|p| std::ptr::eq(p, surface.page));
            if progress < 1.0 {
                let delta = surface.rect.left() - surface.page.rect.left();
                let left = direction.forward != direction.reading_rtl;
                assert!(if incoming == left {
                    delta >= 0.0
                } else {
                    delta <= 0.0
                });
            } else {
                assert!(incoming);
                assert_eq!(surface.rect, surface.page.rect);
            }
        }
        for (i, a) in pose.surfaces.iter().enumerate() {
            for b in &pose.surfaces[i + 1..] {
                assert_close(polygon_area(&clip_polygon(&a.polygon, &b.polygon)), 0.0);
            }
        }
        let outgoing_area: f32 = pose
            .surfaces
            .iter()
            .filter(|surface| matches!(surface.owner, FaceOwner::OutgoingFront))
            .map(|surface| polygon_area(&surface.polygon))
            .sum();
        assert_close(result.visibility.outgoing_front_area, outgoing_area);
        assert!(outgoing_area <= last_area + 0.2);
        last_area = outgoing_area;
        for receipt in &result.visibility.incoming {
            let surface = pose.surfaces.iter().find(|surface| matches!(surface.owner, FaceOwner::Incoming(id) if id == receipt.occurrence)).unwrap();
            assert_close(
                receipt.visible_area,
                area(surface.rect.intersect(surface.page.clip_rect)),
            );
            assert_close(
                receipt.canonical_visible_area,
                area(surface.page.rect.intersect(surface.page.clip_rect)),
            );
        }
        if step == 100 {
            assert_eq!(outgoing_area, 0.0);
            assert!(
                result
                    .visibility
                    .incoming
                    .iter()
                    .all(|r| (r.fraction() - 1.0).abs() < 0.0001)
            );
            assert_eq!(
                pose.surfaces.len(),
                new.pages.iter().filter(|page| !vacant(page)).count()
            );
        }
    }
    let mut last_area = project_slide(old, new, direction, recipe, 0.9, false)
        .unwrap()
        .visibility
        .outgoing_front_area;
    for progress in [0.99, 0.999, 0.9999, 1.0] {
        let remaining = project_slide(old, new, direction, recipe, progress, false)
            .unwrap()
            .visibility
            .outgoing_front_area;
        assert!(remaining <= last_area + 0.2);
        last_area = remaining;
        if progress == 0.9999 {
            assert!(remaining < 0.2);
        }
    }
}

#[test]
fn page_turn_slide_four_directions_unequal_single_and_spread_are_disjoint() {
    for spread in [false, true] {
        let old = unit(spread, false);
        let mut new = unit(spread, true);
        if spread {
            new.pages[0].rect = rect(80.0, 80.0, 310.0, 400.0);
            new.pages[1].rect = rect(330.0, 60.0, 570.0, 420.0);
        } else {
            new.pages[0].rect = rect(50.0, 140.0, 590.0, 340.0);
        }
        for direction in routes() {
            check_trajectory(&old, &new, direction);
            check_trajectory(&new, &old, direction);
        }
    }
}

#[test]
fn page_turn_slide_normalized_landscape_vacant_and_white_partners_move_as_units() {
    let pair = unit(true, false);
    let mut single = unit(false, true);
    single.pages[0].rect = rect(60.0, 130.0, 580.0, 350.0);
    let landscape = normalize_spread_window(single, SpreadWindow::Landscape).unwrap();
    let mut variants = vec![landscape];
    for right in [false, true] {
        let mut one = unit(false, true);
        one.pages[0].rect = if right {
            rect(330.0, 40.0, 600.0, 440.0)
        } else {
            rect(40.0, 40.0, 310.0, 440.0)
        };
        variants
            .push(normalize_spread_window(one, SpreadWindow::Side { right, gap: 20.0 }).unwrap());
    }
    let mut white = unit(true, true);
    white.pages[0].occurrence = None;
    white.pages[0].material = PageMaterial::Blank {
        color: Color32::WHITE,
    };
    white.pages[0].underlay = None;
    variants.push(white);
    for variant in variants {
        for direction in routes() {
            check_trajectory(&pair, &variant, direction);
            check_trajectory(&variant, &pair, direction);
        }
    }
}

#[test]
fn page_turn_slide_oversize_content_remains_disjoint_and_retires_at_endpoint() {
    let mut old = unit(false, false);
    let mut new = unit(false, true);
    old.pages[0].rect = rect(-500.0, -40.0, 1300.0, 520.0);
    new.pages[0].rect = rect(-200.0, 60.0, 840.0, 420.0);
    for direction in routes() {
        // The shared single-page visibility admission may skip these sizes;
        // the exact same geometry remains supported in a normalized spread.
        let old = normalize_spread_window(old.clone(), SpreadWindow::Landscape).unwrap();
        let new = normalize_spread_window(new.clone(), SpreadWindow::Landscape).unwrap();
        check_trajectory(&old, &new, direction);
    }
}

#[test]
fn page_turn_slide_e0_is_first_meaningful_candidate_and_single_failure_is_explicit() {
    let mut old = unit(false, false);
    let mut new = unit(false, true);
    for unit in [&mut old, &mut new] {
        unit.pages[0].rect = rect(100.0, 40.0, 460.0, 440.0);
        unit.pages[0].clip_rect = rect(0.0, 0.0, 600.0, 480.0);
    }
    for direction in routes() {
        let recipe = prepare_slide(&old, &new, direction).unwrap();
        assert_eq!(recipe.initial_progress, 2.0 / 32.0);
        for step in 0..2 {
            let candidate = SlideRecipe {
                initial_progress: step as f32 / 32.0,
                ..recipe
            };
            assert!(!meaningful(&visibility(
                &slide_scene(&old, &new, direction, candidate, 0.0),
                &new,
                &old
            )));
        }
        assert!(meaningful(
            &project_slide(&old, &new, direction, recipe, 0.0, false)
                .unwrap()
                .visibility
        ));
    }
    old.pages[0].rect = rect(-2000.0, 40.0, 2640.0, 440.0);
    new.pages[0].rect = rect(100.0, 40.0, 460.0, 440.0);
    for direction in routes() {
        assert_eq!(
            prepare_slide(&old, &new, direction).unwrap_err(),
            ProjectionFailure::IncomingNotVisible("initial_target_visibility")
        );
    }
}

#[test]
fn page_turn_slide_blank_leading_partner_starts_before_real_page_is_visible() {
    for direction in routes() {
        let old = unit(true, false);
        let mut new = unit(true, true);
        let leading = if direction.turns_right_slot() { 0 } else { 1 };
        new.pages[leading].occurrence = None;
        new.pages[leading].material = PageMaterial::Blank {
            color: Color32::WHITE,
        };
        new.pages[leading].underlay = None;
        let recipe = prepare_slide(&old, &new, direction).unwrap();
        assert_eq!(recipe.initial_progress, 1.0 / 32.0);
        let first = project_slide(&old, &new, direction, recipe, 0.0, false).unwrap();
        assert_eq!(first.visibility.incoming.len(), 1);
        assert_eq!(first.visibility.incoming[0].visible_area, 0.0);
        let later = project_slide(&old, &new, direction, recipe, 0.75, false).unwrap();
        assert!(later.visibility.incoming[0].visible_area > 0.0);
    }
}

#[test]
fn page_turn_slide_crop_partial_texture_and_underlay_keep_translated_print_basis() {
    let domain = rect(0.23, 0.17, 0.81, 0.89);
    for spread in [false, true] {
        let mut old = unit(spread, false);
        let mut new = unit(spread, true);
        for unit in [&mut old, &mut new] {
            for page in &mut unit.pages {
                page.crop = Some(crop());
                page.material = PageMaterial::PartialTexture {
                    texture_id: TextureId::User(page.occurrence.unwrap()),
                    uv: [
                        pos2(0.9, 0.2),
                        pos2(0.9, 0.8),
                        pos2(0.1, 0.8),
                        pos2(0.1, 0.2),
                    ],
                    domain,
                };
            }
        }
        for direction in routes() {
            let recipe = prepare_slide(&old, &new, direction).unwrap();
            for progress in [0.0, 0.1, 0.5, 0.9, 1.0] {
                let pose = slide_scene(&old, &new, direction, recipe, progress);
                let projected =
                    project_slide(&old, &new, direction, recipe, progress, false).unwrap();
                assert!(
                    projected
                        .meshes
                        .iter()
                        .map(|m| m.mesh.vertices.len())
                        .sum::<usize>()
                        <= MAX_PROJECTION_VERTICES
                );
                assert!(
                    projected
                        .meshes
                        .iter()
                        .map(|m| m.mesh.indices.len() / 3)
                        .sum::<usize>()
                        <= MAX_PROJECTION_TRIANGLES
                );
                for surface in &pose.surfaces {
                    let id = TextureId::User(surface.page.occurrence.unwrap());
                    for mesh in projected.meshes.iter().filter(|m| m.mesh.texture_id == id) {
                        for vertex in &mesh.mesh.vertices {
                            let display = pos2(
                                (vertex.pos.x - surface.rect.left()) / surface.rect.width(),
                                (vertex.pos.y - surface.rect.top()) / surface.rect.height(),
                            );
                            assert!(
                                display.x >= domain.left() - 0.00001
                                    && display.x <= domain.right() + 0.00001
                            );
                            assert!(
                                display.y >= domain.top() - 0.00001
                                    && display.y <= domain.bottom() + 0.00001
                            );
                            assert!(
                                (vertex.uv - material_uv(&surface.page.material, display)).length()
                                    < 0.00001
                            );
                            assert_eq!(vertex.color, Color32::WHITE);
                        }
                    }
                }
                let mut plain_old = old.clone();
                let mut plain_new = new.clone();
                for unit in [&mut plain_old, &mut plain_new] {
                    for page in &mut unit.pages {
                        page.crop = None;
                    }
                }
                let plain =
                    project_slide(&plain_old, &plain_new, direction, recipe, progress, false)
                        .unwrap();
                assert_eq!(
                    projected
                        .visibility
                        .incoming
                        .iter()
                        .map(|r| r.visible_area)
                        .collect::<Vec<_>>(),
                    plain
                        .visibility
                        .incoming
                        .iter()
                        .map(|r| r.visible_area)
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    projected.visibility.outgoing_front_area,
                    plain.visibility.outgoing_front_area
                );
            }
        }
    }
}

#[test]
fn page_turn_slide_shared_preflight_rejects_invalid_clip_roi_scale_and_progress() {
    let old = unit(false, false);
    let new = unit(false, true);
    let direction = routes().next().unwrap();
    let mut changed = new.clone();
    changed.pages[0].clip_rect.max.x -= 1.0;
    assert_eq!(
        prepare_slide(&old, &changed, direction)
            .unwrap_err()
            .sub_check(),
        "destination_clip_changed"
    );
    changed = new.clone();
    changed.pixels_per_point = 1.5;
    assert_eq!(
        prepare_slide(&old, &changed, direction)
            .unwrap_err()
            .sub_check(),
        "destination_pixel_scale"
    );
    changed = new.clone();
    if let PageMaterial::Texture { available_uv, .. } = &mut changed.pages[0].material {
        *available_uv = rect(0.1, 0.1, 0.9, 0.9);
    }
    assert_eq!(
        prepare_slide(&old, &changed, direction)
            .unwrap_err()
            .sub_check(),
        "uv_outside_prepared_roi"
    );
    let recipe = prepare_slide(&old, &new, direction).unwrap();
    for progress in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert_eq!(
            project_slide(&old, &new, direction, recipe, progress, false).unwrap_err(),
            ProjectionFailure::InvalidProgress
        );
    }
    let old = normalize_spread_window(
        old,
        SpreadWindow::Side {
            right: false,
            gap: 0.0,
        },
    )
    .unwrap();
    let new = normalize_spread_window(
        new,
        SpreadWindow::Side {
            right: true,
            gap: 0.0,
        },
    )
    .unwrap();
    let direction = TurnDirection {
        reading_rtl: false,
        forward: true,
    };
    assert_eq!(
        prepare_turn(&old, &new, direction).unwrap_err().sub_check(),
        "vacant_leaf_on_both_sides"
    );
    assert!(prepare_slide(&old, &new, direction).is_ok());
}
