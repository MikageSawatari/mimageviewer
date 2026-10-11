//! Bounded, allocation-small page-sheet projection. This module never resolves,
//! prepares, uploads, or owns image resources. The caller retains the resources
//! for every returned mesh and submits the complete unit only after success.
//!
//! A0 supports stable, axis-aligned singles and equal-size spread slots (including
//! a fixed gutter and explicit Blank). Changed placements, unequal spread sizes,
//! Vacant slots, and incomplete prepared ROIs reject the entire projection. A
//! rejection means flat submission of the *same* already-prepared incoming unit.
//! The cylinder's lower half is occluded by its upper half. Its projection and
//! the flat folded portion are monotone strips, so visibility needs no depth
//! buffer, guessed z ordering, or per-pixel work.

use egui::{Color32, Mesh, Pos2, Rect, TextureId, pos2};

const CURL_SEGMENTS: usize = 12;
const MIN_VISIBLE_FRACTION: f32 = 0.05;
const MIN_VISIBLE_SHORT_EDGE: f32 = 8.0;
pub(crate) const MAX_PROJECTION_VERTICES: usize = 256;
pub(crate) const MAX_PROJECTION_TRIANGLES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitLayout {
    Single,
    /// Pages are canonical screen-left, screen-right; never logical file order.
    Spread,
}

#[derive(Clone, Debug)]
pub(crate) enum PageMaterial {
    Texture {
        texture_id: TextureId,
        /// TL, TR, BR, BL in display order. Rotation/trim are already encoded.
        uv: [Pos2; 4],
        /// Prepared texture coordinates actually backed by the resource.
        available_uv: Rect,
    },
    Blank {
        color: Color32,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct CanonicalPage {
    /// Caller-local occurrence identity. Blank has no occurrence.
    pub occurrence: Option<u64>,
    pub rect: Rect,
    pub clip_rect: Rect,
    pub material: PageMaterial,
    /// Existing underlay, including checker textures and their canonical UVs.
    pub underlay: Option<PageMaterial>,
}

#[derive(Clone, Debug)]
pub(crate) struct CanonicalUnit {
    pub layout: UnitLayout,
    pub pages: Vec<CanonicalPage>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TurnDirection {
    pub reading_rtl: bool,
    pub forward: bool,
}

impl TurnDirection {
    fn turns_right_slot(self) -> bool {
        self.forward != self.reading_rtl
    }
}

#[derive(Debug)]
pub(crate) struct ProjectedMesh {
    pub mesh: Mesh,
    pub clip_rect: Rect,
}

#[derive(Clone, Debug)]
pub(crate) struct PageVisibility {
    pub occurrence: u64,
    /// Exact clipped and occlusion-subtracted projected page-surface area.
    pub visible_area: f32,
    pub canonical_visible_area: f32,
    /// Largest connected visible rectangular span, not a union bounding box.
    pub short_edge: f32,
}

impl PageVisibility {
    pub(crate) fn fraction(&self) -> f32 {
        self.visible_area / self.canonical_visible_area
    }
}

#[derive(Clone, Debug)]
pub(crate) struct VisibilityReceipt {
    pub incoming: Vec<PageVisibility>,
    pub outgoing_front_area: f32,
    pub outgoing_front_canonical_area: f32,
}

#[derive(Debug)]
pub(crate) struct TurnProjection {
    /// Back-to-front, already clipped; underlay precedes its image layer.
    pub meshes: Vec<ProjectedMesh>,
    pub visibility: VisibilityReceipt,
    pub finished: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionFailure {
    UnsupportedGeometry,
    IncompleteRoi,
    IncomingNotVisible,
    MidpointNotBroad,
    InvalidProgress,
}

#[derive(Clone, Copy)]
enum FaceOwner {
    Incoming(u64),
    OutgoingFront,
    Other,
}

struct Face {
    rect: Rect,
    owner: FaceOwner,
}

struct Builder {
    meshes: Vec<ProjectedMesh>,
    faces: Vec<Face>,
}

impl Builder {
    fn new() -> Self {
        Self {
            meshes: Vec::with_capacity(32),
            faces: Vec::with_capacity(16),
        }
    }

    fn face(
        &mut self,
        page: &CanonicalPage,
        rect: Rect,
        display_uv: Rect,
        shade: f32,
        owner: FaceOwner,
    ) {
        let clipped = rect.intersect(page.clip_rect);
        if !valid_rect(clipped) {
            return;
        }
        // Both layers use exactly the same projected surface, but each material
        // has its own canonical UV basis (checker UV need not equal image UV).
        if let Some(underlay) = &page.underlay {
            self.layer(underlay, rect, clipped, display_uv, shade, page.clip_rect);
        }
        self.layer(
            &page.material,
            rect,
            clipped,
            display_uv,
            shade,
            page.clip_rect,
        );
        self.faces.push(Face {
            rect: clipped,
            owner,
        });
    }

    fn layer(
        &mut self,
        material: &PageMaterial,
        rect: Rect,
        clipped: Rect,
        display_uv: Rect,
        shade: f32,
        clip_rect: Rect,
    ) {
        let (texture_id, color) = match material {
            PageMaterial::Texture { texture_id, .. } => (*texture_id, Color32::WHITE),
            PageMaterial::Blank { color } => (TextureId::default(), *color),
        };
        let mut mesh = Mesh::with_texture(texture_id);
        let color = shade_color(color, shade);
        for p in [
            clipped.left_top(),
            clipped.right_top(),
            clipped.right_bottom(),
            clipped.left_bottom(),
        ] {
            let x = (p.x - rect.left()) / rect.width();
            let y = (p.y - rect.top()) / rect.height();
            let display = pos2(
                display_uv.left() + display_uv.width() * x,
                display_uv.top() + display_uv.height() * y,
            );
            let uv = match material {
                PageMaterial::Texture { uv, .. } => bilinear_uv(*uv, display),
                PageMaterial::Blank { .. } => egui::epaint::WHITE_UV,
            };
            mesh.vertices
                .push(egui::epaint::Vertex { pos: p, uv, color });
        }
        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        self.meshes.push(ProjectedMesh { mesh, clip_rect });
    }
}

/// All-or-none geometry preflight and projection. Success is proof of submitted
/// face visibility only after the caller actually submits these exact meshes;
/// it is not evidence of GPU completion or OS present.
pub(crate) fn project_turn(
    outgoing: &CanonicalUnit,
    incoming: &CanonicalUnit,
    direction: TurnDirection,
    progress: f32,
) -> Result<TurnProjection, ProjectionFailure> {
    if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
        return Err(ProjectionFailure::InvalidProgress);
    }
    preflight(outgoing, incoming)?;
    let turning_slot = if incoming.layout == UnitLayout::Spread && direction.turns_right_slot() {
        1
    } else {
        0
    };
    let old = &outgoing.pages[turning_slot];
    let old_area = area(old.rect.intersect(old.clip_rect));
    let mut builder = Builder::new();
    if progress == 1.0 {
        for page in &incoming.pages {
            builder.face(page, page.rect, unit_uv(), 1.0, incoming_owner(page));
        }
    } else {
        let sign = if direction.turns_right_slot() {
            1.0
        } else {
            -1.0
        };
        let width = old.rect.width();
        let gap = if incoming.layout == UnitLayout::Spread {
            outgoing.pages[1].rect.left() - outgoing.pages[0].rect.right()
        } else {
            0.0
        };
        let inner = gap * 0.5;
        let hinge = if sign > 0.0 {
            old.rect.left() - inner
        } else {
            old.rect.right() + inner
        };
        // Non-zero initial curl exposes real incoming surfaces in the very first
        // ready frame. A slower early fold keeps the outgoing front broad at 50%.
        let eased = progress * progress * (3.0 - 2.0 * progress);
        let fold = (inner + width) * 0.86 * (1.0 - eased).powf(0.45);
        let radius = width * 0.025 * (std::f32::consts::PI * (0.1 + 0.9 * progress)).sin();
        let arc_end = fold + std::f32::consts::PI * radius;
        let sheet_end = inner + width;
        let flat_start = arc_end.max(inner).min(sheet_end);
        let back_min = fold - (sheet_end - arc_end);
        let front_end = fold.min(back_min).min(sheet_end);
        let screen_rect = |lo: f32, hi: f32| {
            let x0 = hinge + sign * lo;
            let x1 = hinge + sign * hi;
            Rect::from_min_max(
                pos2(x0.min(x1), old.rect.top()),
                pos2(x0.max(x1), old.rect.bottom()),
            )
        };
        let display_range = |s0: f32, s1: f32, back: bool| {
            let mut u0 = (s0 - inner) / width;
            let mut u1 = (s1 - inner) / width;
            // Geometry reflects reading direction. Canonical UVs never do: the
            // front and back each increase in screen-x at their flat endpoints.
            if (sign < 0.0) != back {
                u0 = 1.0 - u0;
                u1 = 1.0 - u1;
            }
            Rect::from_min_max(pos2(u0.min(u1), 0.0), pos2(u0.max(u1), 1.0))
        };
        let target_below = &incoming.pages[turning_slot];
        builder.face(
            target_below,
            target_below.rect,
            unit_uv(),
            1.0,
            incoming_owner(target_below),
        );
        if incoming.layout == UnitLayout::Spread {
            let other = &outgoing.pages[1 - turning_slot];
            builder.face(other, other.rect, unit_uv(), 1.0, FaceOwner::Other);
        }
        if front_end > inner {
            builder.face(
                old,
                screen_rect(inner, front_end),
                display_range(inner, front_end, false),
                1.0,
                FaceOwner::OutgoingFront,
            );
        }
        let single_back;
        let back = if incoming.layout == UnitLayout::Spread {
            &incoming.pages[1 - turning_slot]
        } else {
            // There is no supplied image of the old single sheet's reverse.
            // Reuse its normal underlay rather than fabricating mirrored print.
            // With no opaque canonical underlay, use only a translucent paper
            // tint; transparent images must not gain a new opaque white backing.
            let material = old.underlay.clone().unwrap_or(PageMaterial::Blank {
                color: Color32::from_rgba_unmultiplied(238, 238, 238, 48),
            });
            single_back = CanonicalPage {
                occurrence: None,
                rect: old.rect,
                clip_rect: old.clip_rect,
                material,
                underlay: None,
            };
            &single_back
        };
        let back_owner = if incoming.layout == UnitLayout::Spread {
            incoming_owner(back)
        } else {
            FaceOwner::Other
        };
        if flat_start < sheet_end {
            let hi = fold - (flat_start - arc_end);
            builder.face(
                back,
                screen_rect(back_min, hi),
                display_range(flat_start, sheet_end, true),
                1.0 - 0.06 * (1.0 - eased),
                back_owner,
            );
        }
        // Only the upper cylinder half survives analytic self-occlusion. Strip
        // ordering is immaterial inside it because projections do not overlap.
        for segment in 0..CURL_SEGMENTS {
            let t0 = std::f32::consts::FRAC_PI_2
                + segment as f32 / CURL_SEGMENTS as f32 * std::f32::consts::FRAC_PI_2;
            let t1 = std::f32::consts::FRAC_PI_2
                + (segment + 1) as f32 / CURL_SEGMENTS as f32 * std::f32::consts::FRAC_PI_2;
            let s0 = (fold + radius * t0).max(inner);
            let s1 = (fold + radius * t1).min(sheet_end);
            if s0 >= s1 || radius <= 0.0 {
                continue;
            }
            let a0 = (s0 - fold) / radius;
            let a1 = (s1 - fold) / radius;
            let x0 = fold + radius * a0.sin();
            let x1 = fold + radius * a1.sin();
            let curve_shade = 0.72 + 0.22 * (-((a0 + a1) * 0.5).cos());
            let shade = 1.0 - (1.0 - curve_shade) * (1.0 - eased);
            builder.face(
                back,
                screen_rect(x1, x0),
                display_range(s0, s1, true),
                shade,
                back_owner,
            );
        }
    }
    let receipt = visibility(&builder.faces, incoming, old_area);
    if receipt.incoming.iter().any(|p| {
        p.fraction() + 0.00001 < MIN_VISIBLE_FRACTION || p.short_edge < MIN_VISIBLE_SHORT_EDGE
    }) {
        return Err(ProjectionFailure::IncomingNotVisible);
    }
    if progress == 0.5
        && (receipt.outgoing_front_area / old_area < 0.2
            || receipt.incoming.iter().any(|p| p.fraction() < 0.2))
    {
        return Err(ProjectionFailure::MidpointNotBroad);
    }
    debug_assert!(
        builder
            .meshes
            .iter()
            .map(|m| m.mesh.vertices.len())
            .sum::<usize>()
            <= MAX_PROJECTION_VERTICES
    );
    debug_assert!(
        builder
            .meshes
            .iter()
            .map(|m| m.mesh.indices.len() / 3)
            .sum::<usize>()
            <= MAX_PROJECTION_TRIANGLES
    );
    Ok(TurnProjection {
        meshes: builder.meshes,
        visibility: receipt,
        finished: progress == 1.0,
    })
}

fn preflight(outgoing: &CanonicalUnit, incoming: &CanonicalUnit) -> Result<(), ProjectionFailure> {
    let expected = if incoming.layout == UnitLayout::Single {
        1
    } else {
        2
    };
    if outgoing.layout != incoming.layout
        || outgoing.pages.len() != expected
        || incoming.pages.len() != expected
    {
        return Err(ProjectionFailure::UnsupportedGeometry);
    }
    for (old, new) in outgoing.pages.iter().zip(&incoming.pages) {
        if !valid_rect(old.rect)
            || !valid_rect(new.rect)
            || !valid_rect(old.clip_rect)
            || !valid_rect(new.clip_rect)
            || old.rect != new.rect
            || old.clip_rect != new.clip_rect
            || old.occurrence.is_some() != new.occurrence.is_some()
        {
            return Err(ProjectionFailure::UnsupportedGeometry);
        }
    }
    if expected == 2 {
        let left = &incoming.pages[0];
        let right = &incoming.pages[1];
        if left.rect.width() != right.rect.width()
            || left.rect.top() != right.rect.top()
            || left.rect.bottom() != right.rect.bottom()
            || left.rect.right() > right.rect.left()
            || left.clip_rect != right.clip_rect
        {
            return Err(ProjectionFailure::UnsupportedGeometry);
        }
    }
    for unit in [outgoing, incoming] {
        let mut seen = None;
        for page in &unit.pages {
            if let Some(id) = page.occurrence {
                if seen == Some(id) {
                    return Err(ProjectionFailure::UnsupportedGeometry);
                }
                seen = Some(id);
            }
            if matches!(page.material, PageMaterial::Texture { .. }) != page.occurrence.is_some() {
                return Err(ProjectionFailure::UnsupportedGeometry);
            }
            for material in std::iter::once(&page.material).chain(page.underlay.iter()) {
                if let PageMaterial::Texture {
                    uv, available_uv, ..
                } = material
                {
                    let u_axis = uv[1] - uv[0];
                    let v_axis = uv[3] - uv[0];
                    let uv_area = u_axis.x * v_axis.y - u_axis.y * v_axis.x;
                    if !valid_rect(*available_uv)
                        || !uv_area.is_finite()
                        || uv_area == 0.0
                        || uv.iter().any(|p| {
                            !p.x.is_finite() || !p.y.is_finite() || !available_uv.contains(*p)
                        })
                    {
                        return Err(ProjectionFailure::IncompleteRoi);
                    }
                }
            }
            if !valid_rect(page.rect.intersect(page.clip_rect)) {
                return Err(ProjectionFailure::IncomingNotVisible);
            }
        }
    }
    Ok(())
}

fn visibility(faces: &[Face], incoming: &CanonicalUnit, old_area: f32) -> VisibilityReceipt {
    let mut receipt = VisibilityReceipt {
        incoming: incoming
            .pages
            .iter()
            .filter_map(|p| {
                p.occurrence.map(|occurrence| PageVisibility {
                    occurrence,
                    visible_area: 0.0,
                    canonical_visible_area: area(p.rect.intersect(p.clip_rect)),
                    short_edge: 0.0,
                })
            })
            .collect(),
        outgoing_front_area: 0.0,
        outgoing_front_canonical_area: old_area,
    };
    let mut regions: Vec<(u64, Rect)> = Vec::with_capacity(16);
    for (index, face) in faces.iter().enumerate() {
        let mut visible = vec![face.rect];
        for occluder in &faces[index + 1..] {
            visible = visible
                .into_iter()
                .flat_map(|r| subtract_rect(r, occluder.rect))
                .collect();
        }
        match face.owner {
            FaceOwner::Incoming(id) => regions.extend(visible.into_iter().map(|r| (id, r))),
            FaceOwner::OutgoingFront => {
                receipt.outgoing_front_area += visible.into_iter().map(area).sum::<f32>()
            }
            FaceOwner::Other => {}
        }
    }
    // Adjacent cylinder strips are one connected visible face. Merge exact
    // horizontal neighbors rather than treating tiny tessellation strips as
    // independent 8-point visibility proofs or bridging disconnected regions.
    regions.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.top().total_cmp(&b.1.top()))
            .then(a.1.bottom().total_cmp(&b.1.bottom()))
            .then(a.1.left().total_cmp(&b.1.left()))
    });
    let mut merged: Vec<(u64, Rect)> = Vec::with_capacity(regions.len());
    for (id, r) in regions {
        if let Some((last_id, last)) = merged.last_mut()
            && *last_id == id
            && last.top() == r.top()
            && last.bottom() == r.bottom()
            && r.left() <= last.right()
        {
            last.max.x = last.right().max(r.right());
        } else {
            merged.push((id, r));
        }
    }
    for (id, r) in merged {
        if let Some(page) = receipt.incoming.iter_mut().find(|p| p.occurrence == id) {
            page.visible_area += area(r);
            page.short_edge = page.short_edge.max(r.width().min(r.height()));
        }
    }
    receipt
}

fn subtract_rect(rect: Rect, cover: Rect) -> Vec<Rect> {
    let hit = rect.intersect(cover);
    if !valid_rect(hit) {
        return vec![rect];
    }
    [
        Rect::from_min_max(rect.min, pos2(rect.right(), hit.top())),
        Rect::from_min_max(pos2(rect.left(), hit.bottom()), rect.max),
        Rect::from_min_max(pos2(rect.left(), hit.top()), pos2(hit.left(), hit.bottom())),
        Rect::from_min_max(
            pos2(hit.right(), hit.top()),
            pos2(rect.right(), hit.bottom()),
        ),
    ]
    .into_iter()
    .filter(|r| valid_rect(*r))
    .collect()
}

fn incoming_owner(page: &CanonicalPage) -> FaceOwner {
    page.occurrence
        .map_or(FaceOwner::Other, FaceOwner::Incoming)
}

fn unit_uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0))
}
fn area(r: Rect) -> f32 {
    if valid_rect(r) {
        r.width() * r.height()
    } else {
        0.0
    }
}
fn valid_rect(r: Rect) -> bool {
    r.min.x.is_finite()
        && r.min.y.is_finite()
        && r.max.x.is_finite()
        && r.max.y.is_finite()
        && r.width() > 0.0
        && r.height() > 0.0
}
fn bilinear_uv(uv: [Pos2; 4], p: Pos2) -> Pos2 {
    let top = uv[0] + (uv[1] - uv[0]) * p.x;
    let bottom = uv[3] + (uv[2] - uv[3]) * p.x;
    top + (bottom - top) * p.y
}
fn shade_color(c: Color32, shade: f32) -> Color32 {
    let [r, g, b, a] = c.to_array();
    Color32::from_rgba_premultiplied(
        (r as f32 * shade) as u8,
        (g as f32 * shade) as u8,
        (b as f32 * shade) as u8,
        a,
    )
}

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: u64, rect: Rect) -> CanonicalPage {
        CanonicalPage {
            occurrence: Some(id),
            rect,
            clip_rect: Rect::from_min_max(pos2(-50.0, -50.0), pos2(1000.0, 700.0)),
            material: PageMaterial::Texture {
                texture_id: TextureId::User(id),
                uv: [
                    pos2(0.0, 0.0),
                    pos2(1.0, 0.0),
                    pos2(1.0, 1.0),
                    pos2(0.0, 1.0),
                ],
                available_uv: unit_uv(),
            },
            underlay: Some(PageMaterial::Blank {
                color: Color32::WHITE,
            }),
        }
    }

    fn fixture(spread: bool) -> (CanonicalUnit, CanonicalUnit) {
        let rect = Rect::from_min_max(pos2(100.0, 60.0), pos2(460.0, 600.0));
        let mut pages = vec![page(1, rect)];
        if spread {
            pages.push(page(2, rect.translate(egui::vec2(376.0, 0.0))));
        }
        let old = CanonicalUnit {
            layout: if spread {
                UnitLayout::Spread
            } else {
                UnitLayout::Single
            },
            pages,
        };
        let mut new = old.clone();
        for p in &mut new.pages {
            p.occurrence = p.occurrence.map(|id| id + 10);
            if let PageMaterial::Texture { texture_id, .. } = &mut p.material {
                *texture_id = TextureId::User(p.occurrence.unwrap());
            }
        }
        (old, new)
    }

    #[test]
    fn five_poses_all_routes_have_real_broad_incoming_and_bounded_meshes() {
        for spread in [false, true] {
            let (old, new) = fixture(spread);
            for reading_rtl in [false, true] {
                for forward in [false, true] {
                    for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                        let projection = project_turn(
                            &old,
                            &new,
                            TurnDirection {
                                reading_rtl,
                                forward,
                            },
                            t,
                        )
                        .unwrap();
                        assert_eq!(projection.visibility.incoming.len(), new.pages.len());
                        assert!(
                            projection
                                .visibility
                                .incoming
                                .iter()
                                .all(|p| p.fraction() >= 0.05 && p.short_edge >= 8.0)
                        );
                        assert!(
                            projection
                                .meshes
                                .iter()
                                .map(|m| m.mesh.vertices.len())
                                .sum::<usize>()
                                <= MAX_PROJECTION_VERTICES
                        );
                        assert!(projection.meshes.iter().all(|m| m.mesh.vertices.iter().all(
                            |v| m.clip_rect.contains(v.pos)
                                && v.uv.x >= 0.0
                                && v.uv.x <= 1.0
                                && v.uv.y >= 0.0
                                && v.uv.y <= 1.0
                        )));
                        if t == 0.5 {
                            assert!(
                                projection.visibility.outgoing_front_area
                                    / projection.visibility.outgoing_front_canonical_area
                                    >= 0.2
                            );
                            assert!(
                                projection
                                    .visibility
                                    .incoming
                                    .iter()
                                    .all(|p| p.fraction() >= 0.2)
                            );
                        }
                        if t == 1.0 {
                            assert!(projection.finished);
                            assert!(
                                projection
                                    .visibility
                                    .incoming
                                    .iter()
                                    .all(|p| p.fraction() == 1.0)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn backside_text_never_mirrors_and_lands_in_canonical_slot() {
        let (old, new) = fixture(true);
        for reading_rtl in [false, true] {
            for forward in [false, true] {
                let direction = TurnDirection {
                    reading_rtl,
                    forward,
                };
                let backside = new.pages[usize::from(!direction.turns_right_slot())]
                    .occurrence
                    .unwrap();
                for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    let p = project_turn(&old, &new, direction, t).unwrap();
                    let matching: Vec<_> = p
                        .meshes
                        .iter()
                        .filter(|m| m.mesh.texture_id == TextureId::User(backside))
                        .collect();
                    assert!(!matching.is_empty());
                    for m in matching {
                        assert!(m.mesh.vertices[1].uv.x > m.mesh.vertices[0].uv.x);
                    }
                }
            }
        }
    }

    #[test]
    fn roi_clip_cull_and_unit_changes_reject_before_any_submission() {
        let (old, mut new) = fixture(true);
        if let PageMaterial::Texture { available_uv, .. } = &mut new.pages[1].material {
            available_uv.max.x = 0.8;
        }
        assert!(matches!(
            project_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: true
                },
                0.0
            ),
            Err(ProjectionFailure::IncompleteRoi)
        ));
        let (mut old, mut new) = fixture(true);
        let clip = Rect::from_min_max(pos2(100.0, 60.0), pos2(460.0, 600.0));
        for unit in [&mut old, &mut new] {
            for page in &mut unit.pages {
                page.clip_rect = clip;
            }
        }
        assert!(
            project_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: true
                },
                0.0
            )
            .is_err()
        );
        let (old, mut new) = fixture(true);
        new.pages[1].rect.max.y -= 10.0;
        assert!(matches!(
            project_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: true
                },
                0.0
            ),
            Err(ProjectionFailure::UnsupportedGeometry)
        ));
        let (old, mut new) = fixture(true);
        new.pages.pop(); // Vacant is no surface, not a manufactured white page.
        assert!(
            project_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: true
                },
                0.0
            )
            .is_err()
        );
    }

    #[test]
    fn blank_has_geometry_but_never_a_real_occurrence_receipt() {
        let (mut old, mut new) = fixture(true);
        for unit in [&mut old, &mut new] {
            unit.pages[0].occurrence = None;
            unit.pages[0].material = PageMaterial::Blank {
                color: Color32::WHITE,
            };
        }
        for reading_rtl in [false, true] {
            for forward in [false, true] {
                for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    let p = project_turn(
                        &old,
                        &new,
                        TurnDirection {
                            reading_rtl,
                            forward,
                        },
                        t,
                    )
                    .unwrap();
                    assert_eq!(p.visibility.incoming.len(), 1);
                    assert_eq!(p.visibility.incoming[0].occurrence, 12);
                }
            }
        }
    }

    #[test]
    fn occluded_texture_presence_does_not_prove_visibility() {
        let (_, new) = fixture(false);
        let rect = new.pages[0].rect;
        let receipt = visibility(
            &[
                Face {
                    rect,
                    owner: FaceOwner::Incoming(11),
                },
                Face {
                    rect,
                    owner: FaceOwner::Other,
                },
            ],
            &new,
            area(rect),
        );
        assert_eq!(receipt.incoming[0].visible_area, 0.0);
        assert_eq!(receipt.incoming[0].short_edge, 0.0);
    }

    #[test]
    fn endpoint_preserves_rotated_trimmed_uv_and_existing_checker_underlay() {
        let (mut old, mut new) = fixture(false);
        let uv = [
            pos2(0.2, 0.8),
            pos2(0.2, 0.1),
            pos2(0.9, 0.1),
            pos2(0.9, 0.8),
        ];
        for unit in [&mut old, &mut new] {
            if let PageMaterial::Texture { uv: basis, .. } = &mut unit.pages[0].material {
                *basis = uv;
            }
            unit.pages[0].underlay = Some(PageMaterial::Texture {
                texture_id: TextureId::User(99),
                uv: [
                    pos2(0.0, 0.0),
                    pos2(8.0, 0.0),
                    pos2(8.0, 12.0),
                    pos2(0.0, 12.0),
                ],
                available_uv: Rect::from_min_max(Pos2::ZERO, pos2(8.0, 12.0)),
            });
        }
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let p = project_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: true,
                },
                t,
            )
            .unwrap();
            assert!(
                p.meshes
                    .iter()
                    .any(|m| m.mesh.texture_id == TextureId::User(99))
            );
            if t == 1.0 {
                assert_eq!(p.meshes.len(), 2);
                let image = &p.meshes[1].mesh;
                for (vertex, expected_uv) in image.vertices.iter().zip(uv) {
                    assert!((vertex.uv - expected_uv).length() < 0.00001);
                    assert_eq!(vertex.color, Color32::WHITE);
                }
                assert_eq!(image.vertices[0].pos, new.pages[0].rect.left_top());
                assert_eq!(image.vertices[2].pos, new.pages[0].rect.right_bottom());
            }
        }
    }

    #[test]
    fn initial_pose_rejects_sub_eight_point_exposure_and_degenerate_uv() {
        let (mut old, mut new) = fixture(false);
        for unit in [&mut old, &mut new] {
            unit.pages[0].rect.max.x = unit.pages[0].rect.min.x + 40.0;
        }
        assert!(matches!(
            project_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: true
                },
                0.0
            ),
            Err(ProjectionFailure::IncomingNotVisible)
        ));
        let (old, mut new) = fixture(false);
        if let PageMaterial::Texture { uv, .. } = &mut new.pages[0].material {
            *uv = [pos2(0.5, 0.5); 4];
        }
        assert!(matches!(
            project_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: true
                },
                0.0
            ),
            Err(ProjectionFailure::IncompleteRoi)
        ));
    }
}
