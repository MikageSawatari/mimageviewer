//! Pure book-leaf projection. Materials and canonical placements are immutable;
//! this module performs no source resolution, decoding, upload or I/O.
use egui::{Color32, Mesh, Pos2, Rect, TextureId, pos2};

pub(crate) const MAX_PROJECTION_VERTICES: usize = 4096;
pub(crate) const MAX_PROJECTION_TRIANGLES: usize = 8192;
const MIN_VISIBLE_FRACTION: f32 = 0.05;
const MIN_VISIBLE_SHORT_EDGE: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitLayout {
    Single,
    Spread,
}
#[derive(Clone, Debug)]
pub(crate) enum PageMaterial {
    /// An unoccupied canonical slot: no texture, ROI or occurrence. A turning
    /// face may temporarily cover it with the existing unprinted black paper.
    Vacant,
    Texture {
        texture_id: TextureId,
        uv: [Pos2; 4],
        available_uv: Rect,
    },
    /// Printed material occupies only this part of the canonical page. The
    /// page frame and underlay still cover the entire leaf. UV corners belong
    /// to `domain`, not the full page, and remain within the prepared sampler.
    PartialTexture {
        texture_id: TextureId,
        uv: [Pos2; 4],
        domain: Rect,
    },
    Blank {
        color: Color32,
    },
}
#[derive(Clone, Debug)]
pub(crate) struct CanonicalPage {
    pub occurrence: Option<u64>,
    pub rect: Rect,
    pub clip_rect: Rect,
    pub material: PageMaterial,
    pub underlay: Option<PageMaterial>,
    pub crop: Option<PageCropPaint>,
}

/// Crop paint in the immutable page's display basis, independent of texture
/// quality, source resolution and sampler UV. Full source corners can extend
/// beyond the displayed page when display trim is active.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PageCropPaint {
    pub full: [Pos2; 4],
    pub corners: [Pos2; 4],
}
impl PageCropPaint {
    pub(crate) fn from_screen_corners(
        full: [Pos2; 4],
        corners: [Pos2; 4],
        page_rect: Rect,
    ) -> Self {
        let normalize = |p: Pos2| {
            pos2(
                (p.x - page_rect.left()) / page_rect.width(),
                (p.y - page_rect.top()) / page_rect.height(),
            )
        };
        Self {
            full: full.map(normalize),
            corners: corners.map(normalize),
        }
    }

    fn half(self, right: bool) -> Self {
        let half = |p: Pos2| pos2(p.x * 2.0 - if right { 1.0 } else { 0.0 }, p.y);
        Self {
            full: self.full.map(half),
            corners: self.corners.map(half),
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct CanonicalUnit {
    pub layout: UnitLayout,
    pub pixels_per_point: f32,
    /// Normal layout's spine, before paint rounding/trim. None is for pure
    /// fixtures and single windows; production Pair supplies its layout spine.
    pub spine_x: Option<f32>,
    pub gutter_width: f32,
    pub pages: Vec<CanonicalPage>,
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum SpreadWindow {
    Landscape,
    Side { right: bool, gap: f32 },
}

/// Normalize one normally placed actual page into the two projection faces.
/// The page's displayed rectangle/UV is authoritative, including rotation.
/// Projection face IDs are local; the presenter maps these to actual pages.
pub(crate) fn normalize_spread_window(
    mut unit: CanonicalUnit,
    kind: SpreadWindow,
) -> Result<CanonicalUnit, ProjectionFailure> {
    if unit.pages.len() != 1 || vacant(&unit.pages[0]) || unit.pages[0].occurrence.is_none() {
        return Err(ProjectionFailure::UnsupportedGeometry(
            "spread_window_actual_page",
        ));
    }
    let mut actual = unit.pages.remove(0);
    if !valid_rect(actual.rect) {
        return Err(ProjectionFailure::UnsupportedGeometry("invalid_page_rect"));
    }
    unit.layout = UnitLayout::Spread;
    match kind {
        SpreadWindow::Landscape => {
            let h = actual.rect.center().x;
            let mut right = actual.clone();
            actual.rect.max.x = h;
            right.rect.min.x = h;
            actual.occurrence = Some(0);
            right.occurrence = Some(1);
            actual.material = half_material(&actual.material, false);
            right.material = half_material(&right.material, true);
            actual.underlay = actual.underlay.as_ref().map(|m| half_material(m, false));
            right.underlay = right.underlay.as_ref().map(|m| half_material(m, true));
            actual.crop = actual.crop.map(|crop| crop.half(false));
            right.crop = right.crop.map(|crop| crop.half(true));
            unit.spine_x = Some(h);
            unit.gutter_width = 0.0;
            unit.pages = vec![actual, right];
        }
        SpreadWindow::Side { right, gap } => {
            if !gap.is_finite() || gap < 0.0 {
                return Err(ProjectionFailure::UnsupportedGeometry(
                    "invalid_layout_gutter",
                ));
            }
            let h = if right {
                actual.rect.left() - gap * 0.5
            } else {
                actual.rect.right() + gap * 0.5
            };
            let empty = CanonicalPage {
                occurrence: None,
                rect: Rect::from_min_max(
                    pos2(2.0 * h - actual.rect.right(), actual.rect.top()),
                    pos2(2.0 * h - actual.rect.left(), actual.rect.bottom()),
                ),
                clip_rect: actual.clip_rect,
                material: PageMaterial::Vacant,
                underlay: None,
                crop: None,
            };
            actual.occurrence = Some(0);
            unit.spine_x = Some(h);
            unit.gutter_width = gap;
            unit.pages = if right {
                vec![empty, actual]
            } else {
                vec![actual, empty]
            };
        }
    }
    Ok(unit)
}
fn half_material(material: &PageMaterial, right: bool) -> PageMaterial {
    match material {
        PageMaterial::PartialTexture {
            texture_id,
            uv,
            domain,
        } => PageMaterial::PartialTexture {
            texture_id: *texture_id,
            uv: *uv,
            domain: Rect::from_min_max(
                pos2(
                    domain.min.x * 2.0 - if right { 1.0 } else { 0.0 },
                    domain.min.y,
                ),
                pos2(
                    domain.max.x * 2.0 - if right { 1.0 } else { 0.0 },
                    domain.max.y,
                ),
            ),
        },
        PageMaterial::Texture {
            texture_id,
            uv,
            available_uv,
        } => {
            let top = uv[0] + (uv[1] - uv[0]) * 0.5;
            let bottom = uv[3] + (uv[2] - uv[3]) * 0.5;
            PageMaterial::Texture {
                texture_id: *texture_id,
                uv: if right {
                    [top, uv[1], uv[2], bottom]
                } else {
                    [uv[0], top, bottom, uv[3]]
                },
                available_uv: *available_uv,
            }
        }
        _ => material.clone(),
    }
}
fn vacant(page: &CanonicalPage) -> bool {
    matches!(page.material, PageMaterial::Vacant)
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

    /// A single-page window always shows the same side of the binding. Reverse
    /// navigation changes which sheet arrives, never which edge is the spine.
    fn single_leaf_sign(self) -> f32 {
        if self.reading_rtl { -1.0 } else { 1.0 }
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
    pub visible_area: f32,
    pub canonical_visible_area: f32,
    /// Conservative connected span: an inscribed axis-aligned rectangle in an
    /// actual convex visible polygon, never its union bounding box.
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
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProjectionCost {
    pub silhouette_visibility_ms: f64,
    /// Subset of silhouette/visibility cost: constructing static outer/slot
    /// masks and the four-corner leaf before polygon occlusion subtraction.
    pub outer_mask_cpu_ms: f64,
    pub tessellation_ms: f64,
    pub static_clip_loss_fraction: f32,
    pub static_support_loss_fraction: f32,
    pub static_clip_loss_surface: &'static str,
    pub static_clip_loss_hinge: f32,
    pub static_support_loss_surface: &'static str,
    pub static_support_loss_hinge: f32,
}
#[derive(Debug)]
pub(crate) struct TurnProjection {
    pub meshes: Vec<ProjectedMesh>,
    pub visibility: VisibilityReceipt,
    pub finished: bool,
    pub cost: ProjectionCost,
    pub angle_degrees: f32,
    pub hinge_x: f32,
    pub showing_back: bool,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct TurnRecipe {
    lift: f32,
    grid_u: usize,
    grid_v: usize,
    cost: ProjectionCost,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionFailure {
    UnsupportedGeometry(&'static str),
    IncompleteRoi(&'static str),
    IncomingNotVisible(&'static str),
    InvalidProgress,
}
impl ProjectionFailure {
    pub(crate) fn sub_check(self) -> &'static str {
        match self {
            Self::UnsupportedGeometry(s) | Self::IncompleteRoi(s) | Self::IncomingNotVisible(s) => {
                s
            }
            Self::InvalidProgress => "invalid_progress",
        }
    }
}
#[derive(Clone, Copy)]
enum FaceOwner {
    Incoming(u64),
    OutgoingFront,
    Other,
}
struct Surface<'a> {
    page: &'a CanonicalPage,
    /// Destination placement of this immutable page, independent of the
    /// revealed polygon. Translation preserves its size and printed UV basis.
    rect: Rect,
    polygon: Vec<Pos2>,
    owner: FaceOwner,
    alpha: f32,
}
struct Leaf<'a> {
    page: &'a CanonicalPage,
    owner: FaceOwner,
    sign: f32,
    theta: f32,
    hinge: f32,
    inner: f32,
    outer: f32,
    top: f32,
    bottom: f32,
    focal: f32,
    center_y: f32,
    clip: Rect,
    back: bool,
    /// Printed image in the paper's display coordinates. Its material keeps
    /// the original UV basis; black padding owns no page occurrence.
    image_domain: Rect,
    black_paper: bool,
}
impl Leaf<'_> {
    fn position(&self, u: f32, v: f32) -> Pos2 {
        let d = self.inner + (self.outer - self.inner) * u;
        let z = d * self.theta.sin();
        let perspective = self.focal / (self.focal - z);
        pos2(
            self.hinge + self.sign * d * self.theta.cos() * perspective,
            self.center_y + (self.top + (self.bottom - self.top) * v - self.center_y) * perspective,
        )
    }
    fn display(&self, u: f32, v: f32) -> Pos2 {
        let u = if (self.sign < 0.0) != self.back {
            1.0 - u
        } else {
            u
        };
        pos2(u, v)
    }
    fn image_parameters(&self) -> Rect {
        let domain = self.image_domain;
        if (self.sign < 0.0) != self.back {
            Rect::from_min_max(
                pos2(1.0 - domain.right(), domain.top()),
                pos2(1.0 - domain.left(), domain.bottom()),
            )
        } else {
            domain
        }
    }
    fn silhouette(&self) -> Vec<Pos2> {
        self.domain_silhouette(unit_uv())
    }
    fn image_silhouette(&self) -> Vec<Pos2> {
        if vacant(self.page) {
            Vec::new()
        } else {
            self.domain_silhouette(self.image_parameters())
        }
    }
    fn domain_silhouette(&self, domain: Rect) -> Vec<Pos2> {
        let mut p: Vec<_> = rectangle(domain)
            .iter()
            .map(|p| self.position(p.x, p.y))
            .collect();
        if signed_area(&p) < 0.0 {
            p.reverse();
        }
        clip_rect(&p, self.clip)
    }
}
struct Scene<'a> {
    surfaces: Vec<Surface<'a>>,
    leaf: Option<Leaf<'a>>,
    shadow: f32,
    /// Full common paper below the stationary actual image.
    black_floor: Option<(Rect, Rect)>,
}

#[cfg(test)]
pub(crate) fn project_turn(
    old: &CanonicalUnit,
    new: &CanonicalUnit,
    direction: TurnDirection,
    progress: f32,
) -> Result<TurnProjection, ProjectionFailure> {
    project_turn_measured(old, new, direction, progress, false)
}
/// Timers exist only with the existing perf switch enabled. Silhouette/visibility
/// uses four leaf corners and convex polygons, independent of tessellation size.
#[cfg(test)]
pub(crate) fn project_turn_measured(
    old: &CanonicalUnit,
    new: &CanonicalUnit,
    direction: TurnDirection,
    progress: f32,
    measure: bool,
) -> Result<TurnProjection, ProjectionFailure> {
    let recipe = prepare_turn(old, new, direction)?;
    project_prepared(old, new, direction, recipe, progress, measure)
}
pub(crate) fn prepare_turn(
    old: &CanonicalUnit,
    new: &CanonicalUnit,
    direction: TurnDirection,
) -> Result<TurnRecipe, ProjectionFailure> {
    preflight(old, new)?;
    let cost = admission(old, new, direction)?;
    let lift = if old.layout == UnitLayout::Single && !direction.forward {
        80.0_f32.to_radians()
    } else {
        let mut accepted = None;
        for degrees in [30.0_f32, 45.0, 60.0] {
            let angle = degrees.to_radians();
            let scene = scene(old, new, direction, 0.0, angle);
            let receipt = visibility(&scene, new, old);
            if meaningful(&receipt) {
                accepted = Some(angle);
                break;
            }
        }
        if old.layout == UnitLayout::Spread {
            // Canonical flat adoption alone completes navigation. A narrow
            // incoming under-page may initially be hidden by a wider leaf;
            // start the compatible turn without inventing visible area or
            // forcing a dramatic initial tilt. Its real first-visible receipt
            // arrives later as the leaf clears it.
            accepted.unwrap_or(30.0_f32.to_radians())
        } else {
            accepted.ok_or(ProjectionFailure::IncomingNotVisible(
                "initial_target_visibility",
            ))?
        }
    };
    let grid = if mesh_error_bound(old, new, 16, 4) <= 0.5 {
        (16, 4)
    } else if mesh_error_bound(old, new, 32, 8) <= 0.5 {
        (32, 8)
    } else {
        return Err(ProjectionFailure::UnsupportedGeometry(
            "perspective_mesh_error",
        ));
    };
    Ok(TurnRecipe {
        lift,
        grid_u: grid.0,
        grid_v: grid.1,
        cost,
    })
}
pub(crate) fn project_prepared(
    old: &CanonicalUnit,
    new: &CanonicalUnit,
    direction: TurnDirection,
    recipe: TurnRecipe,
    progress: f32,
    measure: bool,
) -> Result<TurnProjection, ProjectionFailure> {
    let start = measure.then(std::time::Instant::now);
    if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
        return Err(ProjectionFailure::InvalidProgress);
    }
    let mut cost = recipe.cost;
    let scene = scene(old, new, direction, progress, recipe.lift);
    if let Some(start) = start {
        cost.outer_mask_cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
    }
    let receipt = visibility(&scene, new, old);
    if progress == 0.0 && old.layout == UnitLayout::Single && !meaningful(&receipt) {
        return Err(ProjectionFailure::IncomingNotVisible(
            "initial_target_visibility",
        ));
    }
    if let Some(start) = start {
        cost.silhouette_visibility_ms = start.elapsed().as_secs_f64() * 1000.0;
    }
    let mesh_start = measure.then(std::time::Instant::now);
    let mut meshes = Vec::with_capacity(24);
    if let Some((paper, clip)) = scene.black_floor {
        let mut mesh = Mesh::default();
        append_solid_polygon(&mut mesh, &rectangle(paper.intersect(clip)), Color32::BLACK);
        meshes.push(ProjectedMesh {
            mesh,
            clip_rect: clip,
        });
    }
    for surface in &scene.surfaces {
        flat_surface(&mut meshes, surface);
    }
    if let Some(leaf) = &scene.leaf {
        shadow(&mut meshes, &scene, leaf);
        leaf_meshes(&mut meshes, leaf, recipe.grid_u, recipe.grid_v);
    }
    if let Some(start) = mesh_start {
        cost.tessellation_ms = start.elapsed().as_secs_f64() * 1000.0;
    }
    if meshes.iter().map(|m| m.mesh.vertices.len()).sum::<usize>() > MAX_PROJECTION_VERTICES
        || meshes
            .iter()
            .map(|m| m.mesh.indices.len() / 3)
            .sum::<usize>()
            > MAX_PROJECTION_TRIANGLES
    {
        return Err(ProjectionFailure::UnsupportedGeometry(
            "projection_mesh_budget",
        ));
    }
    let (angle_degrees, hinge_x, showing_back) = scene.leaf.as_ref().map_or(
        (
            if old.layout == UnitLayout::Spread {
                let s = usize::from(direction.turns_right_slot());
                spread_angle(&old.pages[s], progress, recipe.lift).to_degrees()
            } else if direction.forward {
                90.0
            } else {
                0.0
            },
            if old.layout == UnitLayout::Single {
                paper_hinge(single_paper(old, new), direction.single_leaf_sign())
            } else {
                moving_hinge(spine(old, 1.0), spine(new, 1.0), progress)
            },
            false,
        ),
        |leaf| (leaf.theta.to_degrees(), leaf.hinge, leaf.back),
    );
    Ok(TurnProjection {
        meshes,
        visibility: receipt,
        finished: progress == 1.0,
        cost,
        angle_degrees,
        hinge_x,
        showing_back,
    })
}
/// Bound rational perspective's second/mixed derivatives over the entire
/// trajectory. This includes triangle interpolation between rows, not only
/// subdivision of the silhouette edge. Convert to destination physical pixels.
fn mesh_error_bound(old: &CanonicalUnit, new: &CanonicalUnit, nu: usize, nv: usize) -> f32 {
    let (width, height) = paper_extent(old, new);
    let distance = max_leaf_distance(old, new);
    let focal = width.max(height).max(distance) * 8.0;
    let near = focal - distance;
    let du = 1.0 / nu as f32;
    let dv = 1.0 / nv as f32;
    let x_second = 2.0 * focal * focal * width * width / (near * near * near);
    let y_second = height * focal * width * width / (near * near * near);
    let mixed = height * focal * width / (near * near);
    let x_error = x_second * du * du / 8.0;
    let y_error = y_second * du * du / 8.0 + mixed * du * dv / 4.0;
    x_error.hypot(y_error) * old.pixels_per_point
}
fn meaningful(receipt: &VisibilityReceipt) -> bool {
    receipt
        .incoming
        .iter()
        .any(|p| p.fraction() >= MIN_VISIBLE_FRACTION && p.short_edge >= MIN_VISIBLE_SHORT_EDGE)
}
/// The common single paper covers both canonical placements, including
/// non-nested rectangles and independent paint-edge rounding.
fn single_paper(old: &CanonicalUnit, new: &CanonicalUnit) -> Rect {
    old.pages[0].rect.union(new.pages[0].rect)
}
fn paper_hinge(paper: Rect, sign: f32) -> f32 {
    if sign > 0.0 {
        paper.left()
    } else {
        paper.right()
    }
}
fn paper_extent(old: &CanonicalUnit, new: &CanonicalUnit) -> (f32, f32) {
    if old.layout == UnitLayout::Single {
        let paper = single_paper(old, new);
        (paper.width(), paper.height())
    } else {
        old.pages
            .iter()
            .chain(&new.pages)
            .fold((1.0_f32, 1.0_f32), |(w, h), p| {
                (w.max(p.rect.width()), h.max(p.rect.height()))
            })
    }
}
fn max_leaf_distance(old: &CanonicalUnit, new: &CanonicalUnit) -> f32 {
    if old.layout == UnitLayout::Single {
        return single_paper(old, new).width();
    }
    [old, new]
        .into_iter()
        .map(|unit| {
            let h = spine(unit, 1.0);
            unit.pages
                .iter()
                .flat_map(|p| [(p.rect.left() - h).abs(), (p.rect.right() - h).abs()])
                .fold(1.0_f32, f32::max)
        })
        .fold(1.0_f32, f32::max)
}
fn smooth(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}
/// Binding trajectory for spread option A.
fn moving_hinge(h0: f32, h1: f32, progress: f32) -> f32 {
    h0 + (h1 - h0) * smooth(progress)
}
fn spine(unit: &CanonicalUnit, sign: f32) -> f32 {
    unit.spine_x.unwrap_or_else(|| {
        if unit.layout == UnitLayout::Spread {
            (unit.pages[0].rect.right() + unit.pages[1].rect.left()) * 0.5
        } else if sign > 0.0 {
            unit.pages[0].rect.left()
        } else {
            unit.pages[0].rect.right()
        }
    })
}
fn spread_angle(front: &CanonicalPage, p: f32, lift: f32) -> f32 {
    let start = if vacant(front) { 0.0 } else { lift };
    let end = std::f32::consts::PI;
    if p == 0.0 {
        start
    } else if p == 1.0 {
        end
    } else {
        start + (end - start) * smooth(p)
    }
}
fn slot(rect: Rect, hinge: f32, gap: f32, sign: f32) -> Rect {
    let mut rect = rect;
    if sign > 0.0 {
        rect.min.x = rect.min.x.max(hinge + gap * 0.5);
    } else {
        rect.max.x = rect.max.x.min(hinge - gap * 0.5);
    }
    rect
}
fn outer_gate(old: Rect, new: Rect, p: f32) -> Rect {
    let r = smooth(p * 2.0);
    Rect::from_min_max(
        pos2(
            old.left() + r * (old.left().min(new.left()) - old.left()),
            old.top() + r * (old.top().min(new.top()) - old.top()),
        ),
        pos2(
            old.right() + r * (old.right().max(new.right()) - old.right()),
            old.bottom() + r * (old.bottom().max(new.bottom()) - old.bottom()),
        ),
    )
}
fn scene<'a>(
    old: &'a CanonicalUnit,
    new: &'a CanonicalUnit,
    d: TurnDirection,
    p: f32,
    lift: f32,
) -> Scene<'a> {
    let mut surfaces = Vec::with_capacity(6);
    if p == 1.0 {
        for page in &new.pages {
            if !vacant(page) {
                surfaces.push(surface(page, page.rect, incoming_owner(page), 1.0));
            }
        }
        return Scene {
            surfaces,
            leaf: None,
            shadow: 0.0,
            black_floor: None,
        };
    }
    let pair = old.layout == UnitLayout::Spread;
    let sign = if pair {
        if d.turns_right_slot() { 1.0 } else { -1.0 }
    } else {
        d.single_leaf_sign()
    };
    let s = if pair && sign > 0.0 { 1 } else { 0 };
    let front = &old.pages[s];
    let back = &new.pages[if pair { 1 - s } else { 0 }];
    let theta = if pair {
        spread_angle(front, p, lift)
    } else if !d.forward {
        lift * (1.0 - smooth(p))
    } else {
        lift + (std::f32::consts::FRAC_PI_2 - lift) * smooth(p)
    };
    let morph = if pair {
        smooth((theta.to_degrees() - 60.0) / 60.0)
    } else if d.forward {
        0.0
    } else {
        1.0
    };
    let h0 = spine(old, sign);
    let h1 = spine(new, sign);
    // A vacant face is unprinted black paper covering the stationary page on
    // that side. Use that page's footprint, even when the other slot is smaller.
    let front_rect = if pair && vacant(front) {
        new.pages[s].rect.translate(egui::vec2(h0 - h1, 0.0))
    } else {
        front.rect
    };
    let back_rect = if pair && vacant(back) {
        old.pages[1 - s].rect.translate(egui::vec2(h1 - h0, 0.0))
    } else {
        back.rect
    };
    let paper = (!pair).then(|| single_paper(old, new));
    let hinge = if pair {
        moving_hinge(h0, h1, p)
    } else {
        paper_hinge(paper.unwrap(), sign)
    };
    let mut leaf_clip = front.clip_rect;
    if !pair {
        leaf_clip = slot(leaf_clip, hinge, 0.0, sign);
    }
    if pair {
        let under = &new.pages[s];
        // Both floor pages follow the same binding. Their own canonical
        // dimensions and UVs stay intact; only their destination placement
        // moves. In particular, never clip a fixed page against a moving H.
        let old_shift = egui::vec2(hinge - h0, 0.0);
        let new_shift = egui::vec2(hinge - h1, 0.0);
        let under_rect = under.rect.translate(new_shift);
        if !vacant(under) {
            let rect = if vacant(front) {
                under_rect
            } else {
                under_rect.intersect(outer_gate(front.rect.translate(old_shift), under_rect, p))
            };
            surfaces.push(placed_surface(
                under,
                under_rect,
                rect,
                incoming_owner(under),
                1.0,
            ));
        }
        let rest = &old.pages[1 - s];
        let rest_rect = rest.rect.translate(old_shift);
        let back_rect = back_rect.translate(new_shift);
        // Only canonical-footprint remnants retire in the final tenth.
        let alpha = 1.0 - smooth((p - 0.9) / 0.1);
        if !vacant(rest) {
            if vacant(back) {
                // The arriving black paper covers this page naturally. Keep
                // its entire stationary support until ordinary flat adoption.
                surfaces.push(placed_surface(
                    rest,
                    rest_rect,
                    rest_rect,
                    FaceOwner::Other,
                    1.0,
                ));
            } else {
                let hit = rest_rect.intersect(back_rect);
                if valid_rect(hit) {
                    surfaces.push(placed_surface(rest, rest_rect, hit, FaceOwner::Other, 1.0));
                }
                for r in subtract_rect(rest_rect, back_rect) {
                    surfaces.push(placed_surface(rest, rest_rect, r, FaceOwner::Other, alpha));
                }
            }
        }
    } else if d.forward {
        surfaces.push(surface(back, back.rect, incoming_owner(back), 1.0));
    } else {
        // Single-window turns move only the leaf. The receiving page already
        // occupies its full canonical placement; it never grows or retires.
        // At completion the ordinary incoming flat replaces this stack.
        surfaces.push(surface(front, front.rect, FaceOwner::Other, 1.0));
    }
    let front_inner = if sign > 0.0 {
        front_rect.left() - h0
    } else {
        h0 - front_rect.right()
    };
    let front_outer = front_inner + front_rect.width();
    let back_inner = if -sign > 0.0 {
        back_rect.left() - h1
    } else {
        h1 - back_rect.right()
    };
    let back_outer = back_inner + back_rect.width();
    let is_back = pair && theta > std::f32::consts::FRAC_PI_2;
    let (page, owner) = if is_back {
        (back, incoming_owner(back))
    } else if !pair && !d.forward {
        (back, incoming_owner(back))
    } else {
        (
            front,
            if vacant(front) {
                FaceOwner::Other
            } else {
                FaceOwner::OutgoingFront
            },
        )
    };
    let interpolate = |a: f32, b: f32| a + (b - a) * morph;
    let (inner, outer, top, bottom) = if pair {
        (
            interpolate(front_inner, back_inner),
            interpolate(front_outer, back_outer),
            interpolate(front_rect.top(), back_rect.top()),
            interpolate(front_rect.bottom(), back_rect.bottom()),
        )
    } else {
        let paper = paper.unwrap();
        (0.0, paper.width(), paper.top(), paper.bottom())
    };
    let (width, height) = paper_extent(old, new);
    let leaf = Leaf {
        page,
        owner,
        sign,
        theta,
        hinge,
        inner,
        outer,
        top,
        bottom,
        focal: width.max(height).max(max_leaf_distance(old, new)) * 8.0,
        center_y: (top + bottom) * 0.5,
        clip: leaf_clip,
        back: is_back,
        image_domain: paper.map_or_else(unit_uv, |paper| {
            Rect::from_min_max(
                pos2(
                    (page.rect.left() - paper.left()) / paper.width(),
                    (page.rect.top() - paper.top()) / paper.height(),
                ),
                pos2(
                    (page.rect.right() - paper.left()) / paper.width(),
                    (page.rect.bottom() - paper.top()) / paper.height(),
                ),
            )
        }),
        black_paper: paper.is_some() || vacant(page),
    };
    Scene {
        surfaces,
        leaf: Some(leaf),
        shadow: theta.sin().abs(),
        black_floor: paper.map(|paper| (paper, front.clip_rect)),
    }
}
fn surface(page: &CanonicalPage, rect: Rect, owner: FaceOwner, alpha: f32) -> Surface<'_> {
    placed_surface(page, page.rect, rect, owner, alpha)
}
fn placed_surface(
    page: &CanonicalPage,
    rect: Rect,
    visible: Rect,
    owner: FaceOwner,
    alpha: f32,
) -> Surface<'_> {
    Surface {
        page,
        rect,
        polygon: rectangle(visible.intersect(page.clip_rect)),
        owner,
        alpha,
    }
}
fn preflight(old: &CanonicalUnit, new: &CanonicalUnit) -> Result<(), ProjectionFailure> {
    if !new.pixels_per_point.is_finite()
        || new.pixels_per_point <= 0.0
        || old.pixels_per_point != new.pixels_per_point
    {
        return Err(ProjectionFailure::UnsupportedGeometry(
            "destination_pixel_scale",
        ));
    }
    let count = if new.layout == UnitLayout::Single {
        1
    } else {
        2
    };
    if old.layout != new.layout || old.pages.len() != count || new.pages.len() != count {
        return Err(ProjectionFailure::UnsupportedGeometry(
            "layout_or_slot_count",
        ));
    }
    for (a, b) in old.pages.iter().zip(&new.pages) {
        if a.clip_rect != b.clip_rect {
            return Err(ProjectionFailure::UnsupportedGeometry(
                "destination_clip_changed",
            ));
        }
    }
    for unit in [old, new] {
        if unit.spine_x.is_some_and(|h| !h.is_finite()) {
            return Err(ProjectionFailure::UnsupportedGeometry(
                "invalid_layout_spine",
            ));
        }
        if !unit.gutter_width.is_finite() || unit.gutter_width < 0.0 {
            return Err(ProjectionFailure::UnsupportedGeometry(
                "invalid_layout_gutter",
            ));
        }
        if count == 2 && unit.pages[0].rect.right() > unit.pages[1].rect.left() {
            return Err(ProjectionFailure::UnsupportedGeometry("spread_overlap"));
        }
        // Normal layout's spine belongs to the gap. Independently rounded
        // paint edges may differ by at most one destination physical pixel.
        if count == 2 {
            let h = spine(unit, 1.0);
            let rounding = 1.0 / unit.pixels_per_point;
            if h < unit.pages[0].rect.right() - rounding || h > unit.pages[1].rect.left() + rounding
            {
                return Err(ProjectionFailure::UnsupportedGeometry(
                    "spread_spine_outside_gap",
                ));
            }
        }
        if count == 2 && unit.pages[0].clip_rect != unit.pages[1].clip_rect {
            return Err(ProjectionFailure::UnsupportedGeometry("spread_slot_clip"));
        }
        for (i, page) in unit.pages.iter().enumerate() {
            if !valid_rect(page.rect) {
                return Err(ProjectionFailure::UnsupportedGeometry("invalid_page_rect"));
            }
            if !valid_rect(page.clip_rect) {
                return Err(ProjectionFailure::UnsupportedGeometry("invalid_clip_rect"));
            }
            if vacant(page) {
                if page.occurrence.is_some() || page.underlay.is_some() {
                    return Err(ProjectionFailure::UnsupportedGeometry(
                        "vacant_page_resource",
                    ));
                }
                continue;
            }
            if page
                .underlay
                .as_ref()
                .is_some_and(|m| matches!(m, PageMaterial::Vacant))
            {
                return Err(ProjectionFailure::UnsupportedGeometry("vacant_underlay"));
            }
            if !valid_rect(page.rect.intersect(page.clip_rect)) {
                return Err(ProjectionFailure::IncomingNotVisible("incoming_clip_empty"));
            }
            if page.occurrence.is_some()
                && unit.pages[..i]
                    .iter()
                    .any(|p| p.occurrence == page.occurrence)
            {
                return Err(ProjectionFailure::UnsupportedGeometry(
                    "duplicate_occurrence",
                ));
            }
            for material in std::iter::once(&page.material).chain(page.underlay.iter()) {
                if let PageMaterial::PartialTexture { uv, domain, .. } = material {
                    if !valid_rect(*domain)
                        || signed_area(uv).abs() < f32::EPSILON
                        || uv.iter().any(|p| !p.is_finite() || !unit_uv().contains(*p))
                    {
                        return Err(ProjectionFailure::IncompleteRoi("invalid_partial_material"));
                    }
                }
                if let PageMaterial::Texture {
                    uv, available_uv, ..
                } = material
                {
                    if signed_area(uv).abs() < f32::EPSILON {
                        return Err(ProjectionFailure::IncompleteRoi("degenerate_uv_basis"));
                    }
                    if uv
                        .iter()
                        .any(|p| !p.x.is_finite() || !p.y.is_finite() || !available_uv.contains(*p))
                    {
                        return Err(ProjectionFailure::IncompleteRoi("uv_outside_prepared_roi"));
                    }
                }
            }
        }
    }
    Ok(())
}
/// Floor pages move with H. Shape-normalized landscape windows have zero gap;
/// the presenter's frozen content/binding checks own user setting changes.
fn admission(
    old: &CanonicalUnit,
    new: &CanonicalUnit,
    d: TurnDirection,
) -> Result<ProjectionCost, ProjectionFailure> {
    if old.layout == UnitLayout::Spread {
        let slot = usize::from(d.turns_right_slot());
        if vacant(&old.pages[slot]) && vacant(&new.pages[1 - slot]) {
            // No actual sheet occupies either face of this directional leaf.
            // Do not announce a turn made only of moving floor surfaces.
            return Err(ProjectionFailure::UnsupportedGeometry(
                "vacant_leaf_on_both_sides",
            ));
        }
    }
    Ok(static_surface_loss(old, new, d))
}
/// Kept for the existing perf schema. With a translated floor surface, its
/// inner edge and binding both gain H(t)-Hcanonical, so their difference is
/// invariant throughout the trajectory. No moving half-plane cuts printed
/// content, and no extra support gap is created. Destination clipping and
/// intentional outer reveal/retirement remain actual visibility operations,
/// not the old fixed-surface spine losses.
pub(crate) fn static_surface_loss(
    _old: &CanonicalUnit,
    _new: &CanonicalUnit,
    _d: TurnDirection,
) -> ProjectionCost {
    ProjectionCost::default()
}
fn visibility(scene: &Scene<'_>, new: &CanonicalUnit, old: &CanonicalUnit) -> VisibilityReceipt {
    // One physical face can have less printed content than its opaque paper.
    // Subtract its occlusion once; separately subtracting its printed subset
    // re-splits coplanar boundaries and can duplicate f32 polygon fragments.
    struct VisibilityFace {
        visible: Vec<Pos2>,
        occlusion: Vec<Pos2>,
        owner: FaceOwner,
    }
    let mut faces = Vec::with_capacity(scene.surfaces.len() + 2);
    if let Some((paper, clip)) = scene.black_floor {
        faces.push(VisibilityFace {
            visible: Vec::new(),
            occlusion: rectangle(paper.intersect(clip)),
            owner: FaceOwner::Other,
        });
    }
    faces.extend(
        scene
            .surfaces
            .iter()
            .filter(|s| !vacant(s.page) && s.alpha > 0.0 && !s.polygon.is_empty())
            .map(|s| VisibilityFace {
                visible: s.polygon.clone(),
                occlusion: s.polygon.clone(),
                owner: s.owner,
            }),
    );
    if let Some(leaf) = &scene.leaf {
        let visible = leaf.image_silhouette();
        let occlusion = leaf.silhouette();
        if polygon_area(&occlusion) > 0.001 {
            faces.push(VisibilityFace {
                visible,
                occlusion,
                owner: leaf.owner,
            });
        }
    }
    let mut receipt = VisibilityReceipt {
        incoming: new
            .pages
            .iter()
            .filter(|p| !vacant(p))
            .filter_map(|p| {
                p.occurrence.map(|id| PageVisibility {
                    occurrence: id,
                    visible_area: 0.0,
                    canonical_visible_area: area(p.rect.intersect(p.clip_rect)),
                    short_edge: 0.0,
                })
            })
            .collect(),
        outgoing_front_area: 0.0,
        outgoing_front_canonical_area: old
            .pages
            .iter()
            .filter(|p| !vacant(p))
            .map(|p| area(p.rect.intersect(p.clip_rect)))
            .sum(),
    };
    for (i, face) in faces.iter().enumerate() {
        let mut pieces = vec![face.visible.clone()];
        for cover in &faces[i + 1..] {
            pieces = pieces
                .into_iter()
                .flat_map(|p| subtract_polygon(&p, &cover.occlusion))
                .collect();
        }
        for polygon in pieces {
            match &face.owner {
                FaceOwner::Incoming(id) => {
                    if let Some(v) = receipt.incoming.iter_mut().find(|v| v.occurrence == *id) {
                        v.visible_area += polygon_area(&polygon);
                        v.short_edge = v.short_edge.max(inscribed_short_edge(&polygon));
                    }
                }
                FaceOwner::OutgoingFront => receipt.outgoing_front_area += polygon_area(&polygon),
                FaceOwner::Other => {}
            }
        }
    }
    receipt
}
fn inscribed_short_edge(poly: &[Pos2]) -> f32 {
    if poly.len() < 3 {
        return 0.0;
    }
    let center = poly
        .iter()
        .fold(egui::Vec2::ZERO, |sum, p| sum + p.to_vec2())
        / poly.len() as f32;
    // Largest centered square admitted by every convex polygon half-plane.
    let mut radius = f32::INFINITY;
    for (a, b) in edges(poly) {
        let edge = b - a;
        let distance = cross(edge, center - a.to_vec2());
        let norm = edge.x.abs() + edge.y.abs();
        if norm > 0.0 {
            radius = radius.min(distance / norm);
        }
    }
    (radius.max(0.0) * 2.0).max(0.0)
}
fn flat_surface(meshes: &mut Vec<ProjectedMesh>, surface: &Surface<'_>) {
    if vacant(surface.page) || surface.polygon.len() < 3 || surface.alpha <= 0.0 {
        return;
    }
    for material in surface
        .page
        .underlay
        .iter()
        .chain(std::iter::once(&surface.page.material))
    {
        let (texture, color) = material_style(material);
        let mut mesh = Mesh::with_texture(texture);
        let partial_polygon;
        let polygon = if matches!(material, PageMaterial::PartialTexture { .. }) {
            let material_rect = material_domain(material);
            partial_polygon = clip_rect(
                &surface.polygon,
                Rect::from_min_max(
                    pos2(
                        surface.rect.left() + material_rect.left() * surface.rect.width(),
                        surface.rect.top() + material_rect.top() * surface.rect.height(),
                    ),
                    pos2(
                        surface.rect.left() + material_rect.right() * surface.rect.width(),
                        surface.rect.top() + material_rect.bottom() * surface.rect.height(),
                    ),
                ),
            );
            &partial_polygon
        } else {
            &surface.polygon
        };
        if polygon.len() < 3 {
            continue;
        }
        for p in polygon {
            let display = pos2(
                (p.x - surface.rect.left()) / surface.rect.width(),
                (p.y - surface.rect.top()) / surface.rect.height(),
            );
            mesh.vertices.push(egui::epaint::Vertex {
                pos: *p,
                uv: material_uv(material, display),
                color: color.gamma_multiply(surface.alpha),
            });
        }
        for i in 1..polygon.len() - 1 {
            mesh.indices
                .extend_from_slice(&[0, i as u32, (i + 1) as u32]);
        }
        meshes.push(ProjectedMesh {
            mesh,
            clip_rect: surface.page.clip_rect,
        });
    }
    if let Some(crop) = surface.page.crop {
        append_flat_crop(
            meshes,
            surface.rect,
            surface.page.clip_rect,
            &surface.polygon,
            crop,
            surface.alpha,
        );
    }
}

/// Ordinary prepared paint uses the same crop primitives as projected flat
/// surfaces. The caller has already selected the immutable crop binding.
pub(crate) fn project_flat_crop(
    page_rect: Rect,
    clip_rect: Rect,
    visible: &[Pos2],
    crop: PageCropPaint,
) -> Vec<ProjectedMesh> {
    let mut meshes = Vec::new();
    append_flat_crop(&mut meshes, page_rect, clip_rect, visible, crop, 1.0);
    meshes
}

/// Separate polygons retain alpha blending against the already painted face.
/// Border widths are measured in canonical points before leaf projection.
fn crop_polygons(crop: PageCropPaint, size: egui::Vec2) -> Vec<(Vec<Pos2>, Color32)> {
    let orient = |mut points: Vec<Pos2>| {
        if signed_area(&points) < 0.0 {
            points.reverse();
        }
        points
    };
    let scale = |p: Pos2| pos2(p.x * size.x, p.y * size.y);
    let normalize = |p: Pos2| pos2(p.x / size.x, p.y / size.y);
    let full = orient(crop.full.map(scale).to_vec());
    let corners = orient(crop.corners.map(scale).to_vec());
    let mut polygons: Vec<_> = subtract_polygon(&full, &corners)
        .into_iter()
        .map(|poly| {
            (
                poly.into_iter().map(normalize).collect(),
                Color32::from_black_alpha(145),
            )
        })
        .collect();
    // Each closed mitered ring is four non-overlapping convex strips, so
    // translucent dimming is applied exactly once at every interior point.
    for (width, color) in [
        (3.0, Color32::BLACK),
        (2.0, Color32::from_rgb(255, 230, 100)),
    ] {
        let screen = &corners;
        let normals: Vec<_> = edges(&screen)
            .map(|(a, b)| {
                let edge = (b - a).normalized();
                egui::vec2(edge.y, -edge.x)
            })
            .collect();
        let mut outer = Vec::with_capacity(4);
        let mut inner = Vec::with_capacity(4);
        for i in 0..4 {
            let before = normals[(i + 3) % 4];
            let after = normals[i];
            let offset = (before + after) * (width * 0.5 / (1.0 + before.dot(after)));
            outer.push(normalize(screen[i] + offset));
            inner.push(normalize(screen[i] - offset));
        }
        for i in 0..4 {
            let j = (i + 1) % 4;
            polygons.push((orient(vec![outer[i], outer[j], inner[j], inner[i]]), color));
        }
    }
    polygons
}

fn append_flat_crop(
    meshes: &mut Vec<ProjectedMesh>,
    rect: Rect,
    clip: Rect,
    visible: &[Pos2],
    crop: PageCropPaint,
    alpha: f32,
) {
    if !valid_rect(rect) || visible.len() < 3 {
        return;
    }
    let mut mesh = Mesh::default();
    for (poly, color) in crop_polygons(crop, rect.size()) {
        let screen: Vec<_> = poly
            .iter()
            .map(|p| {
                pos2(
                    rect.left() + p.x * rect.width(),
                    rect.top() + p.y * rect.height(),
                )
            })
            .collect();
        let clipped = clip_polygon(&screen, visible);
        append_solid_polygon(&mut mesh, &clipped, color.gamma_multiply(alpha));
    }
    if !mesh.indices.is_empty() {
        meshes.push(ProjectedMesh {
            mesh,
            clip_rect: clip,
        });
    }
}
fn leaf_meshes(meshes: &mut Vec<ProjectedMesh>, leaf: &Leaf<'_>, grid_u: usize, grid_v: usize) {
    // Preserve the existing neutral thickness/edge and printed-face shading.
    // Single-page padding is an opaque black paper below the original image.
    let silhouette = leaf.silhouette();
    let paper = PageMaterial::Blank {
        color: Color32::from_rgb(238, 236, 230),
    };
    // A narrow paper thickness behind the free edge makes edge-on poses legible.
    let mut edge = Mesh::default();
    let a = leaf.position(1.0, 0.0);
    let b = leaf.position(1.0, 1.0);
    let shift = egui::vec2(leaf.sign * leaf.theta.sin() * 0.8, 0.4 * leaf.theta.sin());
    let poly = clip_rect(&[a, b, b + shift, a + shift], leaf.clip);
    append_solid_polygon(&mut edge, &poly, Color32::from_rgb(206, 203, 194));
    if !edge.indices.is_empty() {
        meshes.push(ProjectedMesh {
            mesh: edge,
            clip_rect: leaf.clip,
        });
    }
    if silhouette.len() < 3 {
        return;
    }
    let black = PageMaterial::Blank {
        color: Color32::BLACK,
    };
    let mut materials = Vec::with_capacity(3);
    if leaf.black_paper {
        materials.push((&black, unit_uv()));
    }
    if leaf.theta.cos().abs() < 0.00001 {
        materials.push((&paper, unit_uv()));
    } else if !vacant(leaf.page) {
        for material in leaf
            .page
            .underlay
            .iter()
            .chain(std::iter::once(&leaf.page.material))
        {
            let image = leaf.image_parameters();
            if !matches!(material, PageMaterial::PartialTexture { .. }) {
                materials.push((material, image));
                continue;
            }
            let mut partial = material_domain(material);
            if (leaf.sign < 0.0) != leaf.back {
                partial = Rect::from_min_max(
                    pos2(1.0 - partial.right(), partial.top()),
                    pos2(1.0 - partial.left(), partial.bottom()),
                );
            }
            let domain = Rect::from_min_max(
                pos2(
                    image.left() + partial.left() * image.width(),
                    image.top() + partial.top() * image.height(),
                ),
                pos2(
                    image.left() + partial.right() * image.width(),
                    image.top() + partial.bottom() * image.height(),
                ),
            )
            .intersect(image);
            if valid_rect(domain) {
                materials.push((material, domain));
            }
        }
    }
    for (material, domain) in materials {
        let (texture, color) = material_style(material);
        let mut mesh = Mesh::with_texture(texture);
        // Subdivide only this printed domain, with the same whole-paper light
        // basis. Grid-local display coordinates preserve original ROI corners
        // exactly instead of roundtripping through the normalized paper rect.
        for y in 0..=grid_v {
            for x in 0..=grid_u {
                let local_u = x as f32 / grid_u as f32;
                let local_v = y as f32 / grid_v as f32;
                let interpolate = |a: f32, b: f32, t: f32| {
                    if t == 0.0 {
                        a
                    } else if t == 1.0 {
                        b
                    } else {
                        a + (b - a) * t
                    }
                };
                let u = interpolate(domain.left(), domain.right(), local_u);
                let v = interpolate(domain.top(), domain.bottom(), local_v);
                let light = (0.84 + 0.16 * leaf.theta.cos().abs()
                    - 0.10 * leaf.theta.sin().abs() * u)
                    .max(0.72);
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: leaf.position(u, v),
                    uv: material_uv(
                        material,
                        if matches!(material, PageMaterial::PartialTexture { .. }) {
                            leaf.display(
                                (u - leaf.image_parameters().left())
                                    / leaf.image_parameters().width(),
                                (v - leaf.image_parameters().top())
                                    / leaf.image_parameters().height(),
                            )
                        } else {
                            leaf.display(local_u, local_v)
                        },
                    ),
                    color: shade_color(color, light),
                });
            }
        }
        for y in 0..grid_v {
            for x in 0..grid_u {
                let a = (y * (grid_u + 1) + x) as u32;
                let b = a + 1;
                let c = a + (grid_u + 1) as u32;
                mesh.indices.extend_from_slice(&[a, b, c + 1, a, c + 1, c]);
            }
        }
        meshes.push(ProjectedMesh {
            mesh,
            clip_rect: leaf.clip,
        });
    }
    if leaf.theta.cos().abs() >= 0.00001
        && !vacant(leaf.page)
        && let Some(crop) = leaf.page.crop
    {
        append_leaf_crop(meshes, leaf, crop, grid_u, grid_v);
    }
}

fn append_leaf_crop(
    meshes: &mut Vec<ProjectedMesh>,
    leaf: &Leaf<'_>,
    crop: PageCropPaint,
    grid_u: usize,
    grid_v: usize,
) {
    let image = leaf.image_parameters();
    let mirror = (leaf.sign < 0.0) != leaf.back;
    let mut mesh = Mesh::default();
    for (poly, color) in crop_polygons(crop, leaf.page.rect.size()) {
        let parameters: Vec<_> = poly
            .iter()
            .map(|p| {
                pos2(
                    image.left() + (if mirror { 1.0 - p.x } else { p.x }) * image.width(),
                    image.top() + p.y * image.height(),
                )
            })
            .collect();
        let mut parameters = parameters;
        if signed_area(&parameters) < 0.0 {
            parameters.reverse();
        }
        let bounds = Rect::from_points(&parameters).intersect(image);
        if !valid_rect(bounds) {
            continue;
        }
        let cells = |min: f32, max: f32, start: f32, extent: f32, count: usize| {
            let first = (((min - start) / extent) * count as f32).floor().max(0.0) as usize;
            let last = (((max - start) / extent) * count as f32)
                .ceil()
                .min(count as f32) as usize;
            first..last
        };
        let columns = cells(
            bounds.left(),
            bounds.right(),
            image.left(),
            image.width(),
            grid_u,
        );
        let rows = cells(
            bounds.top(),
            bounds.bottom(),
            image.top(),
            image.height(),
            grid_v,
        );
        // Use exactly the face's image subdivision and its triangle diagonals.
        // Mapping the clipped vertices through that affine triangle keeps the
        // overlay on the face, including at crop edges inside a mesh cell.
        for y in rows {
            for x in columns.clone() {
                let u0 = image.left() + image.width() * x as f32 / grid_u as f32;
                let u1 = image.left() + image.width() * (x + 1) as f32 / grid_u as f32;
                let v0 = image.top() + image.height() * y as f32 / grid_v as f32;
                let v1 = image.top() + image.height() * (y + 1) as f32 / grid_v as f32;
                let a = pos2(u0, v0);
                let b = pos2(u1, v0);
                let c = pos2(u0, v1);
                let d = pos2(u1, v1);
                for triangle in [[a, b, d], [a, d, c]] {
                    let poly = clip_polygon(&parameters, &triangle);
                    if poly.len() < 3 || polygon_area(&poly) == 0.0 {
                        continue;
                    }
                    let positions = triangle.map(|p| leaf.position(p.x, p.y));
                    let mapped: Vec<_> = poly
                        .iter()
                        .map(|p| {
                            let [a, b, c] = triangle;
                            let denominator = cross(b - a, c - a);
                            let wb = cross(*p - a, c - a) / denominator;
                            let wc = cross(b - a, *p - a) / denominator;
                            positions[0]
                                + (positions[1] - positions[0]) * wb
                                + (positions[2] - positions[0]) * wc
                        })
                        .collect();
                    append_solid_polygon(&mut mesh, &mapped, color);
                }
            }
        }
    }
    if !mesh.indices.is_empty() {
        meshes.push(ProjectedMesh {
            mesh,
            clip_rect: leaf.clip,
        });
    }
}
fn shadow(meshes: &mut Vec<ProjectedMesh>, scene: &Scene<'_>, leaf: &Leaf<'_>) {
    if scene.shadow < 0.001 {
        return;
    }
    let silhouette = leaf.silhouette();
    if silhouette.len() < 3 {
        return;
    }
    let center = pos2(leaf.hinge, leaf.center_y);
    // Six low-alpha rings approximate a soft contact shadow. Single turns
    // receive on the full paper; spreads retain their actual outer gates.
    let receivers: Vec<_> = if let Some((paper, clip)) = scene.black_floor {
        vec![(rectangle(paper.intersect(clip)), 1.0)]
    } else {
        scene
            .surfaces
            .iter()
            .map(|s| (s.polygon.clone(), s.alpha))
            .collect()
    };
    let mut mesh = Mesh::default();
    for ring in (0..6).rev() {
        let spread = (ring as f32 + 1.0) * 1.8 * scene.shadow;
        let shifted: Vec<_> = silhouette
            .iter()
            .map(|p| {
                *p + (*p - center).normalized() * spread
                    + egui::vec2(-leaf.sign * scene.shadow * 7.0, scene.shadow * 5.0)
            })
            .collect();
        for (receiver, alpha) in &receivers {
            if *alpha <= 0.0 {
                continue;
            }
            let poly = clip_polygon(&shifted, receiver);
            append_solid_polygon(
                &mut mesh,
                &poly,
                Color32::from_black_alpha((6.0 * scene.shadow * alpha) as u8),
            );
        }
    }
    if !mesh.indices.is_empty() {
        meshes.push(ProjectedMesh {
            mesh,
            clip_rect: leaf.clip,
        });
    }
}
fn append_solid_polygon(mesh: &mut Mesh, poly: &[Pos2], color: Color32) {
    if poly.len() < 3 {
        return;
    }
    let base = mesh.vertices.len() as u32;
    mesh.vertices
        .extend(poly.iter().map(|p| egui::epaint::Vertex {
            pos: *p,
            uv: egui::epaint::WHITE_UV,
            color,
        }));
    for i in 1..poly.len() - 1 {
        mesh.indices
            .extend_from_slice(&[base, base + i as u32, base + i as u32 + 1]);
    }
}
fn material_style(material: &PageMaterial) -> (TextureId, Color32) {
    match material {
        PageMaterial::Texture { texture_id, .. }
        | PageMaterial::PartialTexture { texture_id, .. } => (*texture_id, Color32::WHITE),
        PageMaterial::Blank { color } => (TextureId::default(), *color),
        PageMaterial::Vacant => (TextureId::default(), Color32::TRANSPARENT),
    }
}
fn material_uv(material: &PageMaterial, display: Pos2) -> Pos2 {
    match material {
        PageMaterial::Texture { uv, .. } => bilinear_uv(*uv, display),
        PageMaterial::PartialTexture { uv, domain, .. } => bilinear_uv(
            *uv,
            pos2(
                ((display.x - domain.left()) / domain.width()).clamp(0.0, 1.0),
                ((display.y - domain.top()) / domain.height()).clamp(0.0, 1.0),
            ),
        ),
        PageMaterial::Blank { .. } | PageMaterial::Vacant => egui::epaint::WHITE_UV,
    }
}
fn material_domain(material: &PageMaterial) -> Rect {
    match material {
        PageMaterial::PartialTexture { domain, .. } => *domain,
        _ => unit_uv(),
    }
}
fn incoming_owner(page: &CanonicalPage) -> FaceOwner {
    page.occurrence
        .map_or(FaceOwner::Other, FaceOwner::Incoming)
}
fn unit_uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0))
}
fn area(rect: Rect) -> f32 {
    if valid_rect(rect) {
        rect.width() * rect.height()
    } else {
        0.0
    }
}
fn valid_rect(rect: Rect) -> bool {
    rect.is_finite() && rect.width() > 0.0 && rect.height() > 0.0
}
fn rectangle(rect: Rect) -> Vec<Pos2> {
    if valid_rect(rect) {
        vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ]
    } else {
        vec![]
    }
}
fn cross(a: egui::Vec2, b: egui::Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}
fn edges(poly: &[Pos2]) -> impl Iterator<Item = (Pos2, Pos2)> + '_ {
    poly.iter()
        .copied()
        .zip(poly.iter().copied().cycle().skip(1))
        .take(poly.len())
}
fn signed_area(poly: &[Pos2]) -> f32 {
    edges(poly).map(|(a, b)| a.x * b.y - a.y * b.x).sum::<f32>() * 0.5
}
fn polygon_area(poly: &[Pos2]) -> f32 {
    signed_area(poly).abs()
}
fn clip_half(poly: &[Pos2], a: Pos2, b: Pos2, inside: bool) -> Vec<Pos2> {
    let mut result = Vec::with_capacity(poly.len() + 1);
    for (p, q) in edges(poly) {
        let dp = cross(b - a, p - a);
        let dq = cross(b - a, q - a);
        let pin = if inside { dp >= 0.0 } else { dp <= 0.0 };
        let qin = if inside { dq >= 0.0 } else { dq <= 0.0 };
        if pin {
            result.push(p);
        }
        if pin != qin {
            result.push(p + (q - p) * (dp / (dp - dq)));
        }
    }
    result
}
fn clip_polygon(poly: &[Pos2], clip: &[Pos2]) -> Vec<Pos2> {
    if clip.len() < 3 {
        return vec![];
    }
    let mut result = poly.to_vec();
    for (a, b) in edges(clip) {
        result = clip_half(&result, a, b, true);
        if result.len() < 3 {
            return vec![];
        }
    }
    result
}
fn clip_rect(poly: &[Pos2], rect: Rect) -> Vec<Pos2> {
    clip_polygon(poly, &rectangle(rect))
}
fn subtract_polygon(poly: &[Pos2], cover: &[Pos2]) -> Vec<Vec<Pos2>> {
    // Match the physical-face visibility gate: empty or degenerate cover
    // has no opaque interior and cannot remove a polygon.
    if cover.len() < 3 || polygon_area(cover) <= 0.001 {
        return vec![poly.to_vec()];
    }
    let mut remaining = poly.to_vec();
    let mut result = Vec::with_capacity(cover.len());
    for (a, b) in edges(cover) {
        // Scissor intersections can repeat a corner. A zero-length edge has
        // dp == 0 for every point, so inside and outside would both retain the
        // entire remainder and incorrectly count it twice. It defines no
        // half-plane; ignore it rather than adding an epsilon or moving edges.
        if a == b {
            continue;
        }
        let outside = clip_half(&remaining, a, b, false);
        if polygon_area(&outside) > 0.001 {
            result.push(outside);
        }
        remaining = clip_half(&remaining, a, b, true);
        if remaining.len() < 3 {
            break;
        }
    }
    result
}
fn subtract_rect(rect: Rect, cover: Rect) -> Vec<Rect> {
    let hit = rect.intersect(cover);
    if !valid_rect(hit) {
        return if valid_rect(rect) { vec![rect] } else { vec![] };
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
fn bilinear_uv(uv: [Pos2; 4], p: Pos2) -> Pos2 {
    // Preserve normal-helper corner coordinates exactly; subtraction/addition
    // can otherwise perturb a rotated/trimmed UV endpoint by an f32 ulp.
    match (p.x, p.y) {
        (0.0, 0.0) => return uv[0],
        (1.0, 0.0) => return uv[1],
        (1.0, 1.0) => return uv[2],
        (0.0, 1.0) => return uv[3],
        _ => {}
    }
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

    fn crop_fixture() -> PageCropPaint {
        PageCropPaint {
            full: rectangle(unit_uv()).try_into().unwrap(),
            corners: rectangle(Rect::from_min_max(pos2(0.17, 0.23), pos2(0.81, 0.76)))
                .try_into()
                .unwrap(),
        }
    }

    #[test]
    fn crop_flat_clips_dim_and_border_to_actual_visible_polygon_once() {
        let rect = Rect::from_min_max(pos2(100.0, 40.0), pos2(600.0, 840.0));
        let visible = [pos2(100.0, 40.0), pos2(600.0, 40.0), pos2(350.0, 840.0)];
        let crop = crop_fixture();
        let meshes = project_flat_crop(rect, rect, &visible, crop);
        assert_eq!(meshes.len(), 1);
        let mut dim_area = 0.0;
        for triangle in meshes[0].mesh.indices.chunks_exact(3) {
            let points: Vec<_> = triangle
                .iter()
                .map(|i| meshes[0].mesh.vertices[*i as usize].pos)
                .collect();
            for (a, b) in edges(&visible) {
                for p in &points {
                    assert!(cross(b - a, *p - a) >= -0.1);
                }
            }
            if meshes[0].mesh.vertices[triangle[0] as usize].color == Color32::from_black_alpha(145)
            {
                dim_area += polygon_area(&points);
            }
        }
        let crop_screen: Vec<_> = crop
            .corners
            .iter()
            .map(|p| {
                pos2(
                    rect.left() + p.x * rect.width(),
                    rect.top() + p.y * rect.height(),
                )
            })
            .collect();
        let expected = polygon_area(&visible) - polygon_area(&clip_polygon(&visible, &crop_screen));
        assert!(
            (dim_area - expected).abs() < 0.1,
            "dim must cover the visible outside exactly once"
        );
    }

    #[test]
    fn crop_geometry_follows_front_back_split_and_canonical_without_affecting_receipts() {
        for pair in [false, true] {
            for rtl in [false, true] {
                for forward in [false, true] {
                    let (mut old, mut new) = fixture(pair);
                    let direction = TurnDirection {
                        reading_rtl: rtl,
                        forward,
                    };
                    let baseline_recipe = prepare_turn(&old, &new, direction).unwrap();
                    for unit in [&mut old, &mut new] {
                        for page in &mut unit.pages {
                            page.crop = Some(crop_fixture());
                        }
                    }
                    for p in [0.0, 0.25, 0.5, 0.75, 1.0] {
                        let scene = scene(&old, &new, direction, p, baseline_recipe.lift);
                        let result =
                            project_prepared(&old, &new, direction, baseline_recipe, p, false)
                                .unwrap();
                        assert!(
                            result
                                .meshes
                                .iter()
                                .flat_map(|m| &m.mesh.vertices)
                                .all(|v| v.pos.x.is_finite() && v.pos.y.is_finite())
                        );
                        if let Some(leaf) = scene.leaf {
                            let mut projected = Vec::new();
                            append_leaf_crop(
                                &mut projected,
                                &leaf,
                                crop_fixture(),
                                baseline_recipe.grid_u,
                                baseline_recipe.grid_v,
                            );
                            // Crop paint immediately follows this same printed face.
                            assert_eq!(projected.len(), 1);
                            let shape = leaf.image_silhouette();
                            for vertex in &projected[0].mesh.vertices {
                                if leaf.clip.contains(vertex.pos) {
                                    for (a, b) in edges(&shape) {
                                        assert!(cross(b - a, vertex.pos - a) >= -0.2);
                                    }
                                }
                            }
                        }
                        let mut no_crop_old = old.clone();
                        let mut no_crop_new = new.clone();
                        for unit in [&mut no_crop_old, &mut no_crop_new] {
                            for page in &mut unit.pages {
                                page.crop = None;
                            }
                        }
                        let baseline = project_prepared(
                            &no_crop_old,
                            &no_crop_new,
                            direction,
                            baseline_recipe,
                            p,
                            false,
                        )
                        .unwrap();
                        assert_eq!(
                            result
                                .visibility
                                .incoming
                                .iter()
                                .map(|r| r.visible_area)
                                .collect::<Vec<_>>(),
                            baseline
                                .visibility
                                .incoming
                                .iter()
                                .map(|r| r.visible_area)
                                .collect::<Vec<_>>()
                        );
                    }
                }
            }
        }
        let (mut old, _) = fixture(false);
        old.pages[0].crop = Some(crop_fixture());
        let split = normalize_spread_window(old.clone(), SpreadWindow::Landscape).unwrap();
        for (i, page) in split.pages.iter().enumerate() {
            let crop = page.crop.unwrap();
            for (original, split_corner) in
                old.pages[0].crop.unwrap().corners.iter().zip(crop.corners)
            {
                assert!(
                    (page.rect.left() + split_corner.x * page.rect.width()
                        - (old.pages[0].rect.left() + original.x * old.pages[0].rect.width()))
                    .abs()
                        < 0.001
                );
            }
            assert_eq!(page.crop, Some(crop_fixture().half(i == 1)));
        }
    }

    #[test]
    fn crop_projection_stays_within_mesh_budget_at_pdf_dpi() {
        for pair in [false, true] {
            let (mut old, mut new) = portrait_pdf_fixture(pair);
            for unit in [&mut old, &mut new] {
                for page in &mut unit.pages {
                    page.crop = Some(crop_fixture());
                }
            }
            for reading_rtl in [false, true] {
                for forward in [false, true] {
                    let direction = TurnDirection {
                        reading_rtl,
                        forward,
                    };
                    let recipe = prepare_turn(&old, &new, direction).unwrap();
                    for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
                        project_prepared(&old, &new, direction, recipe, progress, false).unwrap();
                    }
                }
            }
        }
    }
    fn page(id: u64, rect: Rect) -> CanonicalPage {
        CanonicalPage {
            occurrence: Some(id),
            rect,
            clip_rect: Rect::from_min_max(pos2(-100.0, -100.0), pos2(2600.0, 1500.0)),
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
            crop: None,
        }
    }
    fn fixture(pair: bool) -> (CanonicalUnit, CanonicalUnit) {
        let rect = Rect::from_min_max(pos2(100.0, 60.0), pos2(460.0, 600.0));
        let mut pages = vec![page(1, rect)];
        if pair {
            pages.push(page(2, rect.translate(egui::vec2(376.0, 0.0))));
        }
        let old = CanonicalUnit {
            layout: if pair {
                UnitLayout::Spread
            } else {
                UnitLayout::Single
            },
            pixels_per_point: 1.0,
            spine_x: pair.then_some(468.0),
            gutter_width: if pair { 16.0 } else { 0.0 },
            pages,
        };
        let mut new = old.clone();
        for page in &mut new.pages {
            page.occurrence = page.occurrence.map(|id| id + 10);
            if let PageMaterial::Texture { texture_id, .. } = &mut page.material {
                *texture_id = TextureId::User(page.occurrence.unwrap());
            }
        }
        (old, new)
    }

    #[test]
    fn source_coverage_partial_material_projects_inside_full_paper_for_single_and_pair_turns() {
        let domain = Rect::from_min_max(pos2(0.25, 0.125), pos2(0.75, 0.875));
        for pair in [false, true] {
            for quarter in 0..4 {
                let (mut old, mut new) = fixture(pair);
                for unit in [&mut old, &mut new] {
                    unit.pixels_per_point = 1.375;
                    for page in &mut unit.pages {
                        let uv = rectangle(unit_uv());
                        page.material = PageMaterial::PartialTexture {
                            texture_id: TextureId::User(page.occurrence.unwrap()),
                            uv: std::array::from_fn(|i| uv[(i + quarter) % 4]),
                            domain,
                        };
                    }
                }
                for reading_rtl in [false, true] {
                    for forward in [false, true] {
                        let direction = TurnDirection {
                            reading_rtl,
                            forward,
                        };
                        let recipe = prepare_turn(&old, &new, direction).unwrap();
                        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
                            let projected =
                                project_prepared(&old, &new, direction, recipe, progress, false)
                                    .unwrap();
                            assert_eq!(projected.finished, progress == 1.0);
                            for draw in &projected.meshes {
                                if matches!(draw.mesh.texture_id, TextureId::User(_)) {
                                    assert!(
                                        draw.mesh.vertices.iter().all(|v| unit_uv().contains(v.uv)),
                                        "never sample absent crop pixels"
                                    );
                                }
                            }
                            if progress == 1.0 {
                                assert_eq!(projected.visibility.incoming.len(), new.pages.len());
                                for page in &new.pages {
                                    let texture = TextureId::User(page.occurrence.unwrap());
                                    let printed = projected
                                        .meshes
                                        .iter()
                                        .find(|m| m.mesh.texture_id == texture)
                                        .unwrap();
                                    let bounds = printed.mesh.vertices.iter().fold(
                                        Rect::NOTHING,
                                        |bounds, vertex| {
                                            bounds.union(Rect::from_min_max(vertex.pos, vertex.pos))
                                        },
                                    );
                                    let expected = Rect::from_min_max(
                                        pos2(
                                            page.rect.left() + domain.left() * page.rect.width(),
                                            page.rect.top() + domain.top() * page.rect.height(),
                                        ),
                                        pos2(
                                            page.rect.left() + domain.right() * page.rect.width(),
                                            page.rect.top() + domain.bottom() * page.rect.height(),
                                        ),
                                    );
                                    assert!((bounds.min - expected.min).length() < 0.001);
                                    assert!((bounds.max - expected.max).length() < 0.001);
                                    assert!(
                                        projected.meshes.iter().any(|m| m.mesh.texture_id
                                            == TextureId::default()
                                            && m.mesh
                                                .vertices
                                                .iter()
                                                .any(|v| v.pos == page.rect.left_top())),
                                        "full paper remains outside partial material"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn source_coverage_landscape_halves_keep_original_crop_uv_and_empty_half_is_presented() {
        let (mut unit, _) = fixture(false);
        let domain = Rect::from_min_max(pos2(0.25, 0.125), pos2(0.75, 0.875));
        unit.pages[0].material = PageMaterial::PartialTexture {
            texture_id: TextureId::User(1),
            uv: [
                pos2(0.0, 0.0),
                pos2(1.0, 0.0),
                pos2(1.0, 1.0),
                pos2(0.0, 1.0),
            ],
            domain,
        };
        let halves = normalize_spread_window(unit, SpreadWindow::Landscape).unwrap();
        assert_eq!(
            material_uv(&halves.pages[0].material, pos2(1.0, 0.5)),
            pos2(0.5, 0.5)
        );
        assert_eq!(
            material_uv(&halves.pages[1].material, pos2(0.0, 0.5)),
            pos2(0.5, 0.5)
        );
        assert!(
            halves
                .pages
                .iter()
                .all(|p| p.underlay.is_some() && p.occurrence.is_some())
        );
        let mut left_only = halves.clone();
        left_only.pages[1].material = PageMaterial::Blank {
            color: Color32::TRANSPARENT,
        };
        let direction = TurnDirection {
            reading_rtl: false,
            forward: true,
        };
        let projected = project_turn(&left_only, &left_only, direction, 1.0).unwrap();
        assert!(projected.finished);
        assert_eq!(projected.visibility.incoming.len(), 2);
    }
    pub(super) fn portrait_pdf_fixture(pair: bool) -> (CanonicalUnit, CanonicalUnit) {
        let (mut old, mut new) = fixture(pair);
        for unit in [&mut old, &mut new] {
            unit.pixels_per_point = 1.5;
            unit.spine_x = pair.then_some(1280.0);
            unit.gutter_width = if pair { 1.33 } else { 0.0 };
            for (i, page) in unit.pages.iter_mut().enumerate() {
                page.rect = Rect::from_min_size(
                    pos2(
                        if pair {
                            512.67 + i as f32 * 768.0
                        } else {
                            896.67
                        },
                        44.0,
                    ),
                    egui::vec2(766.67, 1183.33),
                );
                page.clip_rect = Rect::from_min_max(pos2(0.0, 44.0), pos2(2560.0, 1227.34));
            }
        }
        new.pages[0].rect.min.x -= 0.67;
        new.pages[0].rect.max.y -= 0.67;
        (old, new)
    }
    #[test]
    fn page_turn_white_paper_projects_all_routes_and_never_adds_real_visibility() {
        for old_white in [None, Some(0), Some(1)] {
            for new_white in [None, Some(0), Some(1)] {
                if old_white.is_none() && new_white.is_none() {
                    continue;
                }
                let (mut old, mut new) = fixture(true);
                for (unit, white) in [(&mut old, old_white), (&mut new, new_white)] {
                    if let Some(slot) = white {
                        unit.pages[slot].occurrence = None;
                        unit.pages[slot].material = PageMaterial::Blank {
                            color: Color32::WHITE,
                        };
                        unit.pages[slot].underlay = None;
                    }
                }
                for reading_rtl in [false, true] {
                    for forward in [false, true] {
                        let direction = TurnDirection {
                            reading_rtl,
                            forward,
                        };
                        let recipe = prepare_turn(&old, &new, direction).unwrap();
                        let expected = new
                            .pages
                            .iter()
                            .filter_map(|p| p.occurrence)
                            .collect::<Vec<_>>();
                        for step in 0..=96 {
                            let projection = project_prepared(
                                &old,
                                &new,
                                direction,
                                recipe,
                                step as f32 / 96.0,
                                false,
                            )
                            .unwrap();
                            let projected_scene =
                                scene(&old, &new, direction, step as f32 / 96.0, recipe.lift);
                            assert_eq!(
                                projection
                                    .visibility
                                    .incoming
                                    .iter()
                                    .map(|p| p.occurrence)
                                    .collect::<Vec<_>>(),
                                expected
                            );
                            for visible in &projection.visibility.incoming {
                                assert!(visible.visible_area >= 0.0);
                                // Perspective can enlarge a near-flat leaf beyond
                                // its canonical rectangle. Occlusion must never
                                // duplicate the actual projected printed area.
                                let owned = |owner: FaceOwner| matches!(owner, FaceOwner::Incoming(id) if id == visible.occurrence);
                                let surface_area: f32 = projected_scene
                                    .surfaces
                                    .iter()
                                    .filter(|surface| {
                                        owned(surface.owner)
                                            && surface.alpha > 0.0
                                            && !vacant(surface.page)
                                    })
                                    .map(|surface| polygon_area(&surface.polygon))
                                    .sum();
                                let leaf_area = projected_scene
                                    .leaf
                                    .as_ref()
                                    .filter(|leaf| owned(leaf.owner) && !vacant(leaf.page))
                                    .map_or(0.0, |leaf| polygon_area(&leaf.image_silhouette()));
                                let projected_area = surface_area + leaf_area;
                                assert!(
                                    visible.visible_area <= projected_area + 0.1,
                                    "old_white={old_white:?}/new_white={new_white:?}/rtl={reading_rtl}/forward={forward}/step={step}/progress={:.8}/occurrence={}/visible={:.8}/canonical={:.8}/projected={projected_area:.8}/angle={:.8}",
                                    step as f32 / 96.0,
                                    visible.occurrence,
                                    visible.visible_area,
                                    visible.canonical_visible_area,
                                    projection.angle_degrees,
                                );
                                if step == 96 {
                                    assert_eq!(
                                        visible.visible_area,
                                        visible.canonical_visible_area
                                    );
                                }
                            }
                            assert!(
                                !projection.meshes.is_empty(),
                                "white is opaque paper even when it occupies both leaf faces"
                            );
                            for draw in &projection.meshes {
                                if draw.mesh.texture_id == TextureId::default() {
                                    assert!(
                                        draw.mesh
                                            .vertices
                                            .iter()
                                            .all(|v| v.uv == egui::epaint::WHITE_UV)
                                    );
                                }
                            }
                            if step == 96 {
                                assert!(meaningful(&projection.visibility));
                                if new_white.is_some() {
                                    assert!(
                                        projection.meshes.iter().any(|draw| draw.mesh.texture_id
                                            == TextureId::default()
                                            && draw
                                                .mesh
                                                .vertices
                                                .iter()
                                                .all(|v| v.color == Color32::WHITE)),
                                        "normal white endpoint color is preserved"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn four_routes_move_one_leaf_and_hide_unseen_incoming_back() {
        let (old, new) = fixture(true);
        for rtl in [false, true] {
            for forward in [false, true] {
                let d = TurnDirection {
                    reading_rtl: rtl,
                    forward,
                };
                let s = usize::from(d.turns_right_slot());
                let first = project_turn(&old, &new, d, 0.0).unwrap();
                assert!(meaningful(&first.visibility));
                assert_eq!(first.visibility.incoming[1 - s].visible_area, 0.0);
                for p in [0.0, 0.25, 0.5, 0.75, 0.99, 1.0] {
                    let projection = project_turn(&old, &new, d, p).unwrap();
                    assert!(
                        projection
                            .meshes
                            .iter()
                            .map(|m| m.mesh.vertices.len())
                            .sum::<usize>()
                            <= 4096
                    );
                    if p == 1.0 {
                        assert!(projection.finished);
                        assert!(
                            projection
                                .visibility
                                .incoming
                                .iter()
                                .all(|v| (v.fraction() - 1.0).abs() < 0.0001)
                        );
                    }
                }
                let landed = scene(&old, &new, d, 0.99, 30.0_f32.to_radians());
                let leaf = landed.leaf.unwrap();
                assert!(leaf.back);
                let left = leaf.position(if leaf.sign > 0.0 { 1.0 } else { 0.0 }, 0.0);
                let uv = leaf.display(if leaf.sign > 0.0 { 1.0 } else { 0.0 }, 0.0);
                assert!(left.x.is_finite());
                assert_eq!(uv.x, 0.0);
            }
        }
    }
    #[test]
    fn single_backward_is_arrival_at_the_same_reading_spine() {
        let (old, new) = fixture(false);
        for rtl in [false, true] {
            let d = TurnDirection {
                reading_rtl: rtl,
                forward: false,
            };
            let scene = scene(&old, &new, d, 0.0, 80.0_f32.to_radians());
            let leaf = scene.leaf.unwrap();
            assert!(matches!(leaf.owner, FaceOwner::Incoming(11)));
            assert_eq!(
                leaf.hinge,
                if rtl {
                    new.pages[0].rect.right()
                } else {
                    new.pages[0].rect.left()
                }
            );
            assert!(project_turn(&old, &new, d, 0.0).is_ok());
        }
    }
    #[test]
    fn single_four_routes_keep_the_reading_spine_and_print_orientation_continuous() {
        for unequal in [false, true] {
            let (mut old, mut new) = fixture(false);
            if unequal {
                new.pages[0].rect = Rect::from_min_max(pos2(101.0, 59.0), pos2(459.0, 601.0));
            }
            let basis = [
                pos2(0.17, 0.83),
                pos2(0.17, 0.11),
                pos2(0.91, 0.11),
                pos2(0.91, 0.83),
            ];
            for page in old.pages.iter_mut().chain(&mut new.pages) {
                if let PageMaterial::Texture { uv, .. } = &mut page.material {
                    *uv = basis;
                }
            }
            for reading_rtl in [false, true] {
                for forward in [false, true] {
                    let direction = TurnDirection {
                        reading_rtl,
                        forward,
                    };
                    let recipe = prepare_turn(&old, &new, direction).unwrap();
                    let moving = if forward {
                        &old.pages[0]
                    } else {
                        &new.pages[0]
                    };
                    let paper = single_paper(&old, &new);
                    let hinge = paper_hinge(paper, direction.single_leaf_sign());
                    let mut previous_angle = if forward { 0.0 } else { 90.0 };
                    let mut previous_free_x: Option<f32> = None;
                    for step in 0..200 {
                        let progress = step as f32 / 200.0;
                        let pose = scene(&old, &new, direction, progress, recipe.lift);
                        let leaf = pose.leaf.as_ref().unwrap();
                        assert_eq!(leaf.hinge, hinge);
                        assert_eq!(leaf.position(0.0, 0.0), pos2(hinge, paper.top()));
                        assert_eq!(leaf.position(0.0, 1.0), pos2(hinge, paper.bottom()));
                        assert!(!leaf.back);
                        assert!(std::ptr::eq(leaf.page, moving));
                        assert!(matches!(leaf.owner, FaceOwner::OutgoingFront) == forward);
                        let angle = leaf.theta.to_degrees();
                        assert!(if forward {
                            angle >= previous_angle
                        } else {
                            angle <= previous_angle
                        });
                        previous_angle = angle;
                        let free_x = leaf.position(1.0, 0.5).x;
                        if let Some(previous) = previous_free_x {
                            // Dense adjacent clock poses must move the same free
                            // edge continuously, without swapping hinges or sheets.
                            assert!((free_x - previous).abs() < paper.width() * 0.025);
                        }
                        previous_free_x = Some(free_x);
                        let screen_left_u = if reading_rtl { 1.0 } else { 0.0 };
                        let screen_right_u = 1.0 - screen_left_u;
                        assert!(
                            leaf.position(screen_left_u, 0.0).x
                                < leaf.position(screen_right_u, 0.0).x
                        );
                        for (u, v, expected) in [
                            (screen_left_u, 0.0, basis[0]),
                            (screen_right_u, 0.0, basis[1]),
                            (screen_right_u, 1.0, basis[2]),
                            (screen_left_u, 1.0, basis[3]),
                        ] {
                            assert_eq!(
                                material_uv(&leaf.page.material, leaf.display(u, v)),
                                expected
                            );
                        }
                        let projected =
                            project_prepared(&old, &new, direction, recipe, progress, false)
                                .unwrap();
                        assert_eq!(projected.hinge_x, hinge);
                        if !forward {
                            assert!(
                                pose.surfaces
                                    .iter()
                                    .all(|surface| std::ptr::eq(surface.page, &old.pages[0]))
                            );
                        }
                    }
                    let finished =
                        project_prepared(&old, &new, direction, recipe, 1.0, false).unwrap();
                    assert_eq!(finished.hinge_x, hinge);
                    assert!(finished.finished);
                    assert!((finished.visibility.incoming[0].fraction() - 1.0).abs() < 0.0001);
                    assert_eq!(finished.angle_degrees, if forward { 90.0 } else { 0.0 });
                }
            }
        }
    }
    #[test]
    fn single_common_paper_preserves_actual_image_and_static_floor_in_eight_routes() {
        let mut poses = 0;
        let viewport = Rect::from_min_max(pos2(0.0, 44.0), pos2(1600.0, 944.0));
        let portrait = Rect::from_min_max(pos2(488.5, 44.0), pos2(1111.5, 944.0));
        let landscape = viewport;
        let basis = [
            pos2(0.17, 0.83),
            pos2(0.17, 0.11),
            pos2(0.91, 0.11),
            pos2(0.91, 0.83),
        ];
        for (old_rect, new_rect) in [(portrait, landscape), (landscape, portrait)] {
            let (mut old, mut new) = fixture(false);
            old.pages[0].rect = old_rect;
            new.pages[0].rect = new_rect;
            for page in old.pages.iter_mut().chain(&mut new.pages) {
                page.clip_rect = viewport;
                if let PageMaterial::Texture { uv, .. } = &mut page.material {
                    *uv = basis;
                }
            }
            for reading_rtl in [false, true] {
                for forward in [false, true] {
                    let d = TurnDirection {
                        reading_rtl,
                        forward,
                    };
                    let recipe = prepare_turn(&old, &new, d).unwrap();
                    let (moving, floor) = if forward {
                        (&old.pages[0], &new.pages[0])
                    } else {
                        (&new.pages[0], &old.pages[0])
                    };
                    let paper = old_rect.union(new_rect);
                    let hinge = paper_hinge(paper, d.single_leaf_sign());
                    for progress in [0.0, 0.000001, 0.0001]
                        .into_iter()
                        .chain((1..200).map(|i| i as f32 / 200.0))
                        .chain([0.999999])
                    {
                        let pose = scene(&old, &new, d, progress, recipe.lift);
                        assert_eq!(pose.black_floor, Some((paper, viewport)));
                        assert_eq!(pose.surfaces.len(), 1, "only one stationary actual image");
                        let rest = &pose.surfaces[0];
                        assert!(std::ptr::eq(rest.page, floor));
                        assert_eq!(rest.rect, floor.rect);
                        assert_eq!(rest.polygon, rectangle(floor.rect.intersect(viewport)));
                        assert_eq!(rest.alpha, 1.0);
                        let leaf = pose.leaf.as_ref().unwrap();
                        assert!(std::ptr::eq(leaf.page, moving));
                        assert!(leaf.black_paper);
                        assert_eq!(leaf.top, paper.top());
                        assert_eq!(leaf.bottom, paper.bottom());
                        assert_eq!(leaf.outer - leaf.inner, paper.width());
                        assert_eq!(leaf.hinge, hinge);
                        assert_eq!(leaf.inner, 0.0);
                        let domain = leaf.image_domain;
                        assert!(
                            (paper.left() + domain.left() * paper.width() - moving.rect.left())
                                .abs()
                                < 0.001
                        );
                        assert!(
                            (paper.right()
                                - (1.0 - domain.right()) * paper.width()
                                - moving.rect.right())
                            .abs()
                                < 0.001
                        );
                        assert!(matches!(leaf.owner, FaceOwner::OutgoingFront) == forward);
                        let projection =
                            project_prepared(&old, &new, d, recipe, progress, false).unwrap();
                        assert!(!projection.finished);
                        // Black meshes have no borrowed texture; original image
                        // grid spans only its domain and uses original UV corners.
                        let texture = match moving.material {
                            PageMaterial::Texture { texture_id, .. } => texture_id,
                            _ => unreachable!(),
                        };
                        if let Some(printed) = projection
                            .meshes
                            .iter()
                            .find(|m| m.mesh.texture_id == texture)
                        {
                            let params = leaf.image_parameters();
                            for (index, uv, parameter) in [
                                (
                                    0,
                                    if reading_rtl { basis[1] } else { basis[0] },
                                    params.left_top(),
                                ),
                                (
                                    recipe.grid_u,
                                    if reading_rtl { basis[0] } else { basis[1] },
                                    params.right_top(),
                                ),
                                (
                                    recipe.grid_v * (recipe.grid_u + 1),
                                    if reading_rtl { basis[2] } else { basis[3] },
                                    params.left_bottom(),
                                ),
                                (
                                    (recipe.grid_v + 1) * (recipe.grid_u + 1) - 1,
                                    if reading_rtl { basis[3] } else { basis[2] },
                                    params.right_bottom(),
                                ),
                            ] {
                                assert_eq!(printed.mesh.vertices[index].uv, uv);
                                assert_eq!(
                                    printed.mesh.vertices[index].pos,
                                    leaf.position(parameter.x, parameter.y)
                                );
                            }
                        } else {
                            assert!(leaf.theta.cos().abs() < 0.00001);
                        }
                        assert_eq!(projection.meshes[0].mesh.vertices.len(), 4);
                        assert!(
                            projection.meshes[0]
                                .mesh
                                .vertices
                                .iter()
                                .all(|v| v.color == Color32::BLACK)
                        );
                        let receipt = &projection.visibility;
                        if forward {
                            assert!(
                                (receipt.outgoing_front_area
                                    - polygon_area(&leaf.image_silhouette()))
                                .abs()
                                    < 0.1
                            );
                            let expected: f32 = subtract_polygon(&rest.polygon, &leaf.silhouette())
                                .iter()
                                .map(|p| polygon_area(p))
                                .sum();
                            assert!((receipt.incoming[0].visible_area - expected).abs() < 0.5);
                            assert!(
                                receipt.incoming[0].visible_area
                                    <= receipt.incoming[0].canonical_visible_area + 0.5,
                                "static floor visibility cannot gain area from polygon subdivision"
                            );
                        } else {
                            assert!(
                                (receipt.incoming[0].visible_area
                                    - polygon_area(&leaf.image_silhouette()))
                                .abs()
                                    < 0.5
                            );
                        }
                        poses += 1;
                    }
                    let landed = scene(&old, &new, d, 1.0, recipe.lift);
                    assert!(landed.leaf.is_none());
                    assert!(
                        landed.black_floor.is_none(),
                        "no black paper remains over a customized canvas at terminal"
                    );
                    assert_eq!(landed.surfaces.len(), 1);
                    assert!(std::ptr::eq(landed.surfaces[0].page, &new.pages[0]));
                    assert_eq!(landed.surfaces[0].rect, new_rect);
                }
            }
        }
        assert_eq!(poses, 8 * 203);
    }
    #[test]
    fn single_padding_occludes_floor_without_counting_as_printed_visibility() {
        let (mut old, mut new) = fixture(false);
        old.pages[0].rect = Rect::from_min_max(pos2(488.5, 44.0), pos2(1111.5, 944.0));
        new.pages[0].rect = Rect::from_min_max(pos2(0.0, 44.0), pos2(1600.0, 944.0));
        let d = TurnDirection {
            reading_rtl: false,
            forward: true,
        };
        let recipe = prepare_turn(&old, &new, d).unwrap();
        let pose = scene(&old, &new, d, 0.0, recipe.lift);
        let leaf = pose.leaf.as_ref().unwrap();
        let actual = polygon_area(&leaf.image_silhouette());
        let paper = polygon_area(&leaf.silhouette());
        assert!(paper > actual * 2.0);
        let receipt = visibility(&pose, &new, &old);
        assert!((receipt.outgoing_front_area - actual).abs() < 0.1);
        let image_only_reveal: f32 =
            subtract_polygon(&pose.surfaces[0].polygon, &leaf.image_silhouette())
                .iter()
                .map(|p| polygon_area(p))
                .sum();
        assert!(image_only_reveal > receipt.incoming[0].visible_area + 10000.0);
    }
    #[test]
    fn single_shifted_image_rect_union_bounds_perspective_and_mesh_error() {
        let (mut old, mut new) = fixture(false);
        old.pages[0].rect = Rect::from_min_max(pos2(100.0, 60.0), pos2(460.0, 600.0));
        new.pages[0].rect = Rect::from_min_max(pos2(500.0, 180.0), pos2(860.0, 720.0));
        let paper = single_paper(&old, &new);
        assert_eq!(paper_extent(&old, &new), (760.0, 660.0));
        assert_eq!(max_leaf_distance(&old, &new), paper.width());
        for reading_rtl in [false, true] {
            let d = TurnDirection {
                reading_rtl,
                forward: false,
            };
            let recipe = prepare_turn(&old, &new, d).unwrap();
            assert!(mesh_error_bound(&old, &new, recipe.grid_u, recipe.grid_v) <= 0.5);
            let pose = scene(&old, &new, d, 0.2, recipe.lift);
            let leaf = pose.leaf.unwrap();
            assert_eq!(leaf.focal, paper.width() * 8.0);
            assert_eq!(leaf.hinge, paper_hinge(paper, d.single_leaf_sign()));
        }
    }
    #[test]
    fn unequal_scan_and_height_are_admitted_but_a_spine_outside_its_gap_is_not() {
        let d = TurnDirection {
            reading_rtl: true,
            forward: true,
        };
        let (old, new) = portrait_pdf_fixture(true);
        assert!(project_turn(&old, &new, d, 0.0).is_ok());
        let (old, mut new) = fixture(true);
        new.pages[0].rect.min.y -= 100.0;
        new.pages[0].rect.max.y += 100.0;
        assert!(project_turn(&old, &new, d, 0.0).is_ok());
        new.spine_x = Some(350.0);
        assert_eq!(
            project_turn(&old, &new, d, 0.0).unwrap_err().sub_check(),
            "spread_spine_outside_gap"
        );
    }
    #[test]
    fn reported_scan_aspects_fit_letterbox_with_independent_spines_in_four_routes() {
        // Real-app report: 1153x1671 -> 1151x1670. Normal Pair layout centers
        // the total width, so changing either page also moves the layout spine.
        let clip = Rect::from_min_size(pos2(120.0, 80.0), egui::vec2(2560.0, 1440.0));
        let fitted_pair = |sources: [egui::Vec2; 2], first_id: u64| {
            let scale = clip.height() / sources[0].y.max(sources[1].y);
            let sizes = sources.map(|source| source * scale);
            let start = clip.center().x - (sizes[0].x + sizes[1].x) * 0.5;
            let pages = sizes
                .into_iter()
                .enumerate()
                .map(|(i, size)| {
                    let mut p = page(
                        first_id + i as u64,
                        Rect::from_min_size(
                            pos2(
                                start + if i == 0 { 0.0 } else { sizes[0].x },
                                clip.center().y - size.y * 0.5,
                            ),
                            size,
                        ),
                    );
                    p.clip_rect = clip;
                    p
                })
                .collect();
            CanonicalUnit {
                layout: UnitLayout::Spread,
                pixels_per_point: 1.5,
                spine_x: Some(start + sizes[0].x),
                gutter_width: 0.0,
                pages,
            }
        };
        let old = fitted_pair([egui::vec2(1153.0, 1671.0); 2], 1);
        let new = fitted_pair([egui::vec2(1151.0, 1670.0), egui::vec2(1152.0, 1670.0)], 11);
        assert!((old.pages[0].rect.left() - new.pages[0].rect.left()).abs() > 1.0 / 1.5);
        assert_ne!(old.spine_x, new.spine_x);
        for reading_rtl in [false, true] {
            for forward in [false, true] {
                let direction = TurnDirection {
                    reading_rtl,
                    forward,
                };
                let recipe = prepare_turn(&old, &new, direction).unwrap();
                for progress in [0.0, 0.0001, 0.25, 0.5, 0.75, 0.999, 1.0] {
                    let projected =
                        project_prepared(&old, &new, direction, recipe, progress, false).unwrap();
                    if progress == 1.0 {
                        assert!(
                            projected
                                .visibility
                                .incoming
                                .iter()
                                .all(|page| { (page.fraction() - 1.0).abs() < 0.0001 })
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn new_outer_edges_are_continuous_at_zero_and_next_pose() {
        let old = Rect::from_min_max(pos2(0.0, 200.0), pos2(300.0, 800.0));
        let new = Rect::from_min_max(pos2(0.0, 50.0), pos2(300.0, 950.0));
        assert_eq!(outer_gate(old, new, 0.0), old);
        assert_eq!(outer_gate(old, new, 0.25).top(), 125.0);
        assert_eq!(outer_gate(old, new, 0.5), new);
        let extra = |p| area(outer_gate(old, new, p)) - area(old);
        assert!(extra(0.001) < extra(0.002));
        assert!(extra(0.001) < 4.0);
    }
    #[test]
    fn silhouette_occlusion_is_polygonal_and_not_mesh_or_bbox_area() {
        let rect = rectangle(Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)));
        let triangle = vec![pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(0.0, 100.0)];
        let pieces = subtract_polygon(&rect, &triangle);
        assert!((pieces.iter().map(|p| polygon_area(p)).sum::<f32>() - 5000.0).abs() < 0.01);
        assert!(pieces.iter().all(|p| inscribed_short_edge(p) < 100.0));
    }
    #[test]
    fn polygon_occlusion_ignores_repeated_scissor_corners_without_duplicate_fragments() {
        let subject = rectangle(Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)));
        let cover = rectangle(Rect::from_min_max(pos2(50.0, 0.0), pos2(100.0, 100.0)));
        let repeated: Vec<_> = cover.iter().flat_map(|p| [*p, *p]).collect();
        let expected = subtract_polygon(&subject, &cover);
        let actual = subtract_polygon(&subject, &repeated);
        assert_eq!(actual, expected);
        assert!((actual.iter().map(|p| polygon_area(p)).sum::<f32>() - 5000.0).abs() < 0.01);
        // Exercise the producer too: clipping a boundary point can emit the
        // same corner for both retained endpoint and crossing intersection.
        let clipped = clip_rect(
            &rectangle(Rect::from_min_max(pos2(50.0, -10.0), pos2(100.0, 110.0))),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)),
        );
        let actual = subtract_polygon(&subject, &clipped);
        assert!((actual.iter().map(|p| polygon_area(p)).sum::<f32>() - 5000.0).abs() < 0.01);
    }
    #[test]
    fn empty_or_degenerate_occlusion_preserves_the_subject() {
        let subject = rectangle(Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)));
        for cover in [
            vec![],
            vec![pos2(50.0, 50.0); 4],
            vec![
                pos2(20.0, 20.0),
                pos2(50.0, 50.0),
                pos2(80.0, 80.0),
                pos2(20.0, 20.0),
            ],
        ] {
            assert_eq!(subtract_polygon(&subject, &cover), vec![subject.clone()]);
        }
    }
    #[test]
    fn visibility_is_identical_at_two_independent_tessellation_densities() {
        let (old, new) = portrait_pdf_fixture(true);
        let direction = TurnDirection {
            reading_rtl: true,
            forward: true,
        };
        let recipe = prepare_turn(&old, &new, direction).unwrap();
        for progress in [0.0, 0.25, 0.425, 0.433, 0.44, 0.75, 0.99] {
            let coarse = project_prepared(
                &old,
                &new,
                direction,
                TurnRecipe {
                    grid_u: 16,
                    grid_v: 4,
                    ..recipe
                },
                progress,
                false,
            )
            .unwrap();
            let fine = project_prepared(
                &old,
                &new,
                direction,
                TurnRecipe {
                    grid_u: 32,
                    grid_v: 8,
                    ..recipe
                },
                progress,
                false,
            )
            .unwrap();
            assert!(
                fine.meshes
                    .iter()
                    .map(|m| m.mesh.vertices.len())
                    .sum::<usize>()
                    > coarse
                        .meshes
                        .iter()
                        .map(|m| m.mesh.vertices.len())
                        .sum::<usize>()
            );
            assert_eq!(
                coarse.visibility.outgoing_front_area,
                fine.visibility.outgoing_front_area
            );
            for (coarse, fine) in coarse
                .visibility
                .incoming
                .iter()
                .zip(&fine.visibility.incoming)
            {
                assert_eq!(coarse.visible_area, fine.visible_area);
                assert_eq!(coarse.short_edge, fine.short_edge);
            }
        }
    }
    #[test]
    fn asymmetric_rotated_back_uv_keeps_canonical_corner_order_in_all_directions() {
        let (old, mut new) = fixture(true);
        let basis = [
            pos2(0.17, 0.83),
            pos2(0.17, 0.11),
            pos2(0.91, 0.11),
            pos2(0.91, 0.83),
        ];
        for page in &mut new.pages {
            if let PageMaterial::Texture { uv, .. } = &mut page.material {
                *uv = basis;
            }
        }
        for reading_rtl in [false, true] {
            for forward in [false, true] {
                let direction = TurnDirection {
                    reading_rtl,
                    forward,
                };
                let recipe = prepare_turn(&old, &new, direction).unwrap();
                for progress in [0.6, 0.75, 0.99] {
                    let scene = scene(&old, &new, direction, progress, recipe.lift);
                    let leaf = scene.leaf.unwrap();
                    assert!(leaf.back);
                    let screen_left_u = if leaf.sign > 0.0 { 1.0 } else { 0.0 };
                    let screen_right_u = 1.0 - screen_left_u;
                    assert!(
                        leaf.position(screen_left_u, 0.0).x < leaf.position(screen_right_u, 0.0).x
                    );
                    assert_eq!(
                        material_uv(&leaf.page.material, leaf.display(screen_left_u, 0.0)),
                        basis[0]
                    );
                    assert_eq!(
                        material_uv(&leaf.page.material, leaf.display(screen_right_u, 0.0)),
                        basis[1]
                    );
                    assert_eq!(
                        material_uv(&leaf.page.material, leaf.display(screen_right_u, 1.0)),
                        basis[2]
                    );
                    assert_eq!(
                        material_uv(&leaf.page.material, leaf.display(screen_left_u, 1.0)),
                        basis[3]
                    );
                }
            }
        }
    }
    #[test]
    fn final_endpoint_retains_rotated_uv_and_checker_without_shading() {
        let (old, mut new) = fixture(false);
        let uv = [
            pos2(0.2, 0.8),
            pos2(0.2, 0.1),
            pos2(0.9, 0.1),
            pos2(0.9, 0.8),
        ];
        if let PageMaterial::Texture { uv: basis, .. } = &mut new.pages[0].material {
            *basis = uv;
        }
        let p = project_turn(
            &old,
            &new,
            TurnDirection {
                reading_rtl: false,
                forward: true,
            },
            1.0,
        )
        .unwrap();
        let image = p.meshes.last().unwrap();
        for (vertex, uv) in image.mesh.vertices.iter().zip(uv) {
            assert_eq!(vertex.uv, uv);
            assert_eq!(vertex.color, Color32::WHITE);
        }
    }
    #[test]
    fn roi_rejects_before_submission_and_timing_is_opt_in() {
        let (old, mut new) = fixture(true);
        if let PageMaterial::Texture { available_uv, .. } = &mut new.pages[1].material {
            available_uv.max.x = 0.8;
        }
        let d = TurnDirection {
            reading_rtl: false,
            forward: true,
        };
        assert_eq!(
            project_turn(&old, &new, d, 0.0).unwrap_err().sub_check(),
            "uv_outside_prepared_roi"
        );
        let (old, new) = fixture(true);
        let p = project_turn(&old, &new, d, 0.25).unwrap();
        assert_eq!(p.cost.silhouette_visibility_ms, 0.0);
        let p = project_turn_measured(&old, &new, d, 0.25, true).unwrap();
        assert!(p.cost.silhouette_visibility_ms > 0.0);
        assert!(p.cost.tessellation_ms > 0.0);
    }
    #[test]
    fn perspective_affine_uv_error_is_bounded_at_pdf_dpi_and_4k() {
        for pair in [false, true] {
            let (mut old, mut new) = portrait_pdf_fixture(pair);
            for unit in [&mut old, &mut new] {
                unit.pixels_per_point = 2.0;
                for page in &mut unit.pages {
                    page.rect = Rect::from_min_max(page.rect.min * 0.75, page.rect.max * 0.75);
                    page.clip_rect =
                        Rect::from_min_max(page.clip_rect.min * 0.75, page.clip_rect.max * 0.75);
                }
                unit.spine_x = unit.spine_x.map(|x| x * 0.75);
                unit.gutter_width *= 0.75;
            }
            for rtl in [false, true] {
                for forward in [false, true] {
                    let d = TurnDirection {
                        reading_rtl: rtl,
                        forward,
                    };
                    let recipe = prepare_turn(&old, &new, d).unwrap();
                    assert!(mesh_error_bound(&old, &new, recipe.grid_u, recipe.grid_v) <= 0.5);
                    for p in [0.0, 0.1, 0.25, 0.4, 0.6, 0.75, 0.99] {
                        let scene = scene(&old, &new, d, p, recipe.lift);
                        let leaf = scene.leaf.unwrap();
                        for y in 0..recipe.grid_v {
                            for x in 0..recipe.grid_u {
                                let u = x as f32 / recipe.grid_u as f32;
                                let v = y as f32 / recipe.grid_v as f32;
                                let du = 1.0 / recipe.grid_u as f32;
                                let dv = 1.0 / recipe.grid_v as f32;
                                let a = leaf.position(u, v);
                                let b = leaf.position(u + du, v + dv);
                                let error = (leaf.position(u + du * 0.5, v + dv * 0.5)
                                    - (a + (b - a) * 0.5))
                                    .length()
                                    * old.pixels_per_point;
                                assert!(error <= 0.5, "{error} physicalpx at {p}");
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn width_swap_moves_floor_pages_without_clipping_print_or_creating_support_gaps() {
        let (mut old, mut new) = fixture(true);
        old.gutter_width = 0.0;
        new.gutter_width = 0.0;
        old.spine_x = Some(650.0);
        new.spine_x = Some(350.0);
        old.pages[0].rect = Rect::from_min_max(pos2(50.0, 0.0), pos2(650.0, 600.0));
        old.pages[1].rect = Rect::from_min_max(pos2(650.0, 0.0), pos2(950.0, 600.0));
        new.pages[0].rect = Rect::from_min_max(pos2(50.0, 0.0), pos2(350.0, 600.0));
        new.pages[1].rect = Rect::from_min_max(pos2(350.0, 0.0), pos2(950.0, 600.0));
        for rtl in [false, true] {
            for forward in [false, true] {
                let d = TurnDirection {
                    reading_rtl: rtl,
                    forward,
                };
                let cost = static_surface_loss(&old, &new, d);
                assert_eq!(cost.static_clip_loss_fraction, 0.0);
                assert_eq!(cost.static_support_loss_fraction, 0.0);
                let recipe = prepare_turn(&old, &new, d).unwrap();
                let s = usize::from(d.turns_right_slot());
                let initial = project_prepared(&old, &new, d, recipe, 0.0, false).unwrap();
                if s == 0 {
                    assert_eq!(recipe.lift, 30.0_f32.to_radians());
                    assert!(
                        initial
                            .visibility
                            .incoming
                            .iter()
                            .all(|page| page.visible_area == 0.0)
                    );
                    assert!(!initial.finished);
                    assert!(initial.visibility.outgoing_front_area > 0.0);
                }
                let revealed = project_prepared(&old, &new, d, recipe, 0.75, false).unwrap();
                assert!(
                    revealed
                        .visibility
                        .incoming
                        .iter()
                        .all(|page| page.visible_area > 0.0)
                );
                // Even both visible faces are still only partial curl proof;
                // navigation's canonical flat completion is a later receipt.
                assert!(!revealed.finished);
                let mut previous_h = spine(&old, 1.0);
                for step in 0..200 {
                    let p = step as f32 / 200.0;
                    let pose = scene(&old, &new, d, p, recipe.lift);
                    let h = pose.leaf.as_ref().unwrap().hinge;
                    assert_eq!(h, 650.0 - 300.0 * smooth(p));
                    assert!(h <= previous_h);
                    assert!((h - previous_h).abs() < 3.0);
                    previous_h = h;
                    let under = &pose.surfaces[0];
                    assert!(std::ptr::eq(under.page, &new.pages[s]));
                    assert!((under.rect.size() - new.pages[s].rect.size()).length() < 0.001);
                    assert!(
                        (under.rect.left() - h - (new.pages[s].rect.left() - 350.0)).abs() < 0.001
                    );
                    for rest in &pose.surfaces[1..] {
                        assert!(std::ptr::eq(rest.page, &old.pages[1 - s]));
                        assert!((rest.rect.size() - old.pages[1 - s].rect.size()).length() < 0.001);
                        assert!(
                            (rest.rect.right() - h - (old.pages[1 - s].rect.right() - 650.0)).abs()
                                < 0.001
                        );
                        // Floor ownership remains on opposite sides of the
                        // common spine, even in the former 50% loss example.
                        // These are rectangular floor placements. Compare
                        // their exact separating edges, rather than clipping
                        // touching polygons: f32 line intersections can move
                        // a boundary point one ulp and shoelace cancellation
                        // then reports a spurious microscopic triangle area.
                        assert!(if s == 0 {
                            under.rect.right() <= rest.rect.left()
                        } else {
                            rest.rect.right() <= under.rect.left()
                        });
                    }
                    project_prepared(&old, &new, d, recipe, p, false).unwrap();
                }
                let landed = scene(&old, &new, d, 0.9999, recipe.lift);
                let leaf = landed.leaf.unwrap();
                let back = new.pages[1 - s].rect;
                let left_u = if leaf.sign > 0.0 { 1.0 } else { 0.0 };
                assert!((leaf.position(left_u, 0.0) - back.min).length() < 0.001);
                assert!((leaf.position(1.0 - left_u, 1.0) - back.max).length() < 0.001);
                let end = project_prepared(&old, &new, d, recipe, 1.0, false).unwrap();
                assert_eq!(end.hinge_x, 350.0);
                assert!(
                    end.visibility
                        .incoming
                        .iter()
                        .all(|page| (page.fraction() - 1.0).abs() < 0.0001)
                );
            }
        }
    }

    #[test]
    fn moving_spine_reveal_and_print_coordinates_use_the_same_translated_placement() {
        let (mut old, mut new) = fixture(true);
        old.gutter_width = 0.0;
        new.gutter_width = 0.0;
        old.spine_x = Some(450.0);
        new.spine_x = Some(500.0);
        old.pages[0].rect = Rect::from_min_max(pos2(150.0, 200.0), pos2(450.0, 800.0));
        old.pages[1].rect = Rect::from_min_max(pos2(450.0, 200.0), pos2(750.0, 800.0));
        new.pages[0].rect = Rect::from_min_max(pos2(100.0, 50.0), pos2(500.0, 950.0));
        new.pages[1].rect = Rect::from_min_max(pos2(500.0, 50.0), pos2(850.0, 950.0));
        for reading_rtl in [false, true] {
            for forward in [false, true] {
                let direction = TurnDirection {
                    reading_rtl,
                    forward,
                };
                let s = usize::from(direction.turns_right_slot());
                let recipe = prepare_turn(&old, &new, direction).unwrap();
                let pose = |p| scene(&old, &new, direction, p, recipe.lift);
                let first = pose(0.0);
                let under = &first.surfaces[0];
                assert_eq!(under.rect.size(), new.pages[s].rect.size());
                assert_eq!(under.polygon, rectangle(old.pages[s].rect));
                let extra =
                    |p| polygon_area(&pose(p).surfaces[0].polygon) - area(old.pages[s].rect);
                assert!(extra(0.002) > extra(0.001));
                assert!(extra(0.001) < 10.0);
                let halfway = pose(0.5);
                let under = &halfway.surfaces[0];
                assert_eq!(under.polygon, rectangle(under.rect));
                assert_eq!(
                    under.rect,
                    new.pages[s].rect.translate(egui::vec2(-25.0, 0.0))
                );
                let mut meshes = Vec::new();
                flat_surface(&mut meshes, under);
                let image = meshes.last().unwrap();
                let expected_uv = [
                    pos2(0.0, 0.0),
                    pos2(1.0, 0.0),
                    pos2(1.0, 1.0),
                    pos2(0.0, 1.0),
                ];
                for ((vertex, position), uv) in image
                    .mesh
                    .vertices
                    .iter()
                    .zip(rectangle(under.rect))
                    .zip(expected_uv)
                {
                    assert_eq!(vertex.pos, position);
                    assert_eq!(vertex.uv, uv);
                }
                let near_end = pose(0.9999);
                let under = &near_end.surfaces[0];
                assert!((under.rect.min - new.pages[s].rect.min).length() < 0.001);
                assert!(
                    near_end
                        .surfaces
                        .iter()
                        .skip(1)
                        .filter(|rest| rest.alpha < 1.0)
                        .all(|rest| rest.alpha < 0.0001)
                );
            }
        }
    }
    #[test]
    fn destination_clip_crops_translated_print_without_refitting_uv_or_accepting_a_new_clip() {
        let (mut old, mut new) = fixture(true);
        new.spine_x = Some(508.0);
        for page in &mut new.pages {
            page.rect = page.rect.translate(egui::vec2(40.0, 0.0));
        }
        let clip = Rect::from_min_max(pos2(150.0, 100.0), pos2(800.0, 580.0));
        for page in old.pages.iter_mut().chain(&mut new.pages) {
            page.clip_rect = clip;
        }
        let direction = TurnDirection {
            reading_rtl: true,
            forward: true,
        };
        let recipe = prepare_turn(&old, &new, direction).unwrap();
        let pose = scene(&old, &new, direction, 0.5, recipe.lift);
        let under = &pose.surfaces[0];
        assert_eq!(
            under.rect,
            old.pages[0].rect.translate(egui::vec2(20.0, 0.0))
        );
        assert_eq!(under.polygon, rectangle(under.rect.intersect(clip)));
        let mut meshes = Vec::new();
        flat_surface(&mut meshes, under);
        let image = meshes.last().unwrap();
        assert_eq!(image.clip_rect, clip);
        for vertex in &image.mesh.vertices {
            assert!(clip.contains(vertex.pos));
            let expected = pos2(
                (vertex.pos.x - under.rect.left()) / under.rect.width(),
                (vertex.pos.y - under.rect.top()) / under.rect.height(),
            );
            assert!((vertex.uv - expected).length() < 0.00001);
            assert!(unit_uv().contains(vertex.uv));
        }
        for page in &mut new.pages {
            page.clip_rect.max.y += 10.0;
        }
        assert_eq!(
            prepare_turn(&old, &new, direction).unwrap_err().sub_check(),
            "destination_clip_changed"
        );
    }
    #[test]
    fn a_hidden_initial_pair_does_not_relax_single_visibility_admission() {
        let (old, mut new) = fixture(false);
        new.pages[0].rect.max.x = new.pages[0].rect.min.x + 1.0;
        let direction = TurnDirection {
            reading_rtl: false,
            forward: true,
        };
        assert_eq!(
            prepare_turn(&old, &new, direction).unwrap_err().sub_check(),
            "initial_target_visibility"
        );
    }
    #[test]
    fn thirteen_reported_scan_pairs_use_exact_trim_uv_and_effective_destination_clip() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("page_turn_animation/real_scan_geometry.json"))
                .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 13);
        let number = |value: &serde_json::Value| value.as_f64().unwrap() as f32;
        let rect = |value: &serde_json::Value| {
            Rect::from_min_max(
                pos2(number(&value[0]), number(&value[1])),
                pos2(number(&value[2]), number(&value[3])),
            )
        };
        let unit = |metadata: &serde_json::Value, first_id| {
            let pages = metadata.as_array().unwrap();
            let canonical = pages
                .iter()
                .enumerate()
                .map(|(i, metadata)| {
                    assert!(metadata["visible_source_roi"].is_null());
                    for size in ["source_size", "native_size"] {
                        assert!(
                            metadata[size]
                                .as_array()
                                .unwrap()
                                .iter()
                                .all(|edge| number(edge) > 0.0)
                        );
                    }
                    let mut page = page(first_id + i as u64, rect(&metadata["canonical_rect"]));
                    // Match the production owner fix: painter ancestry may
                    // alternate full-window/content scissors, while the
                    // transform's media destination has remained unchanged.
                    page.clip_rect =
                        rect(&metadata["clip_rect"]).intersect(rect(&metadata["viewport_rect"]));
                    if let PageMaterial::Texture { uv, .. } = &mut page.material {
                        *uv = std::array::from_fn(|corner| {
                            pos2(
                                number(&metadata["uv"][corner][0]),
                                number(&metadata["uv"][corner][1]),
                            )
                        });
                    }
                    page
                })
                .collect();
            CanonicalUnit {
                layout: UnitLayout::Spread,
                pixels_per_point: number(&pages[0]["pixels_per_point"]),
                spine_x: Some(number(&pages[0]["normal_layout_spine"]["hinge_x"])),
                gutter_width: number(&pages[0]["normal_layout_spine"]["gutter_width"]),
                pages: canonical,
            }
        };
        let mut raw_clip_changes = 0;
        let mut trimmed_pages = 0;
        for case in cases {
            let old = unit(&case["outgoing"], 1);
            let new = unit(&case["incoming"], 11);
            raw_clip_changes +=
                usize::from(case["outgoing"][0]["clip_rect"] != case["incoming"][0]["clip_rect"]);
            trimmed_pages += old
                .pages
                .iter()
                .chain(&new.pages)
                .filter(|page| match &page.material {
                    PageMaterial::Texture { uv, .. } => {
                        *uv != [
                            pos2(0.0, 0.0),
                            pos2(1.0, 0.0),
                            pos2(1.0, 1.0),
                            pos2(0.0, 1.0),
                        ]
                    }
                    PageMaterial::PartialTexture { .. } => true,
                    PageMaterial::Blank { .. } | PageMaterial::Vacant => false,
                })
                .count();
            assert_eq!(old.pages[0].clip_rect, new.pages[0].clip_rect);
            for reading_rtl in [false, true] {
                for forward in [false, true] {
                    let direction = TurnDirection {
                        reading_rtl,
                        forward,
                    };
                    let recipe = prepare_turn(&old, &new, direction).unwrap_or_else(|failure| {
                        panic!("sequence {}: {failure:?}", case["sequence"])
                    });
                    let s = usize::from(direction.turns_right_slot());
                    for step in 0..=200 {
                        let progress = step as f32 / 200.0;
                        let projected =
                            project_prepared(&old, &new, direction, recipe, progress, false)
                                .unwrap();
                        let expected_h = spine(&old, 1.0)
                            + (spine(&new, 1.0) - spine(&old, 1.0)) * smooth(progress);
                        assert!((projected.hinge_x - expected_h).abs() < 0.001);
                        assert_eq!(projected.finished, step == 200);
                        assert!(
                            projected
                                .visibility
                                .incoming
                                .iter()
                                .all(|page| page.visible_area.is_finite()
                                    && page.visible_area >= 0.0)
                        );
                        if step == 200 {
                            assert!(
                                projected
                                    .visibility
                                    .incoming
                                    .iter()
                                    .all(|page| (page.fraction() - 1.0).abs() < 0.0001)
                            );
                        } else {
                            let pose = scene(&old, &new, direction, progress, recipe.lift);
                            let under = &pose.surfaces[0];
                            assert!(
                                (under.rect.size() - new.pages[s].rect.size()).length() < 0.001
                            );
                            assert!(
                                (under.rect.left()
                                    - expected_h
                                    - (new.pages[s].rect.left() - spine(&new, 1.0)))
                                .abs()
                                    < 0.001
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(raw_clip_changes, 6);
        assert!(trimmed_pages > 0);
    }
    #[test]
    fn spread_window_landscape_splits_display_uv_and_underlay_in_all_quarter_rotations() {
        let base = [
            pos2(0.1, 0.2),
            pos2(0.8, 0.2),
            pos2(0.8, 0.9),
            pos2(0.1, 0.9),
        ];
        for rotation in 0..4 {
            let (mut unit, _) = fixture(false);
            let rect = Rect::from_min_max(pos2(120.0, 180.0), pos2(920.0, 580.0));
            unit.pages[0].rect = rect;
            let uv = std::array::from_fn(|i| base[(i + rotation) % 4]);
            if let PageMaterial::Texture { uv: basis, .. } = &mut unit.pages[0].material {
                *basis = uv;
            }
            unit.pages[0].underlay = Some(PageMaterial::Texture {
                texture_id: TextureId::User(77),
                uv,
                available_uv: unit_uv(),
            });
            let normalized = normalize_spread_window(unit, SpreadWindow::Landscape).unwrap();
            assert_eq!(normalized.layout, UnitLayout::Spread);
            assert_eq!(normalized.spine_x, Some(rect.center().x));
            assert_eq!(normalized.gutter_width, 0.0);
            assert_eq!(
                normalized.pages[0].rect.right(),
                normalized.pages[1].rect.left()
            );
            assert_eq!(
                normalized.pages[0].rect.union(normalized.pages[1].rect),
                rect
            );
            for (i, face) in normalized.pages.iter().enumerate() {
                assert_eq!(face.occurrence, Some(i as u64));
                for (layer, material) in std::iter::once(&face.material)
                    .chain(face.underlay.iter())
                    .enumerate()
                {
                    let PageMaterial::Texture {
                        texture_id,
                        uv: half,
                        available_uv,
                    } = material
                    else {
                        panic!()
                    };
                    assert_eq!(
                        *texture_id,
                        TextureId::User(if layer == 0 { 1 } else { 77 })
                    );
                    assert_eq!(*available_uv, unit_uv());
                    for display in [
                        pos2(0.0, 0.0),
                        pos2(1.0, 0.0),
                        pos2(1.0, 1.0),
                        pos2(0.0, 1.0),
                        pos2(0.3, 0.7),
                    ] {
                        let expected =
                            bilinear_uv(uv, pos2((i as f32 + display.x) * 0.5, display.y));
                        assert!((bilinear_uv(*half, display) - expected).length() < 0.00001);
                    }
                }
            }
        }
    }
    #[test]
    fn spread_window_side_preserves_real_rect_and_has_no_empty_surface_or_receipt() {
        for right in [false, true] {
            for gap in [0.0, 16.0] {
                let (unit, _) = fixture(false);
                let actual = unit.pages[0].clone();
                let normalized =
                    normalize_spread_window(unit, SpreadWindow::Side { right, gap }).unwrap();
                let real = &normalized.pages[usize::from(right)];
                let empty = &normalized.pages[usize::from(!right)];
                let h = normalized.spine_x.unwrap();
                assert_eq!(real.rect, actual.rect);
                assert_eq!(real.clip_rect, actual.clip_rect);
                assert_eq!(real.occurrence, Some(0));
                assert_eq!(
                    h,
                    if right {
                        actual.rect.left() - gap * 0.5
                    } else {
                        actual.rect.right() + gap * 0.5
                    }
                );
                assert!(vacant(empty));
                assert!(empty.occurrence.is_none() && empty.underlay.is_none());
                assert_eq!(empty.rect.left(), 2.0 * h - actual.rect.right());
                assert_eq!(empty.rect.right(), 2.0 * h - actual.rect.left());
                preflight(&normalized, &normalized).unwrap();
                let end = scene(
                    &normalized,
                    &normalized,
                    TurnDirection {
                        reading_rtl: false,
                        forward: true,
                    },
                    1.0,
                    0.0,
                );
                assert_eq!(end.surfaces.len(), 1);
                assert!(!vacant(end.surfaces[0].page));
                let receipt = visibility(&end, &normalized, &normalized);
                assert_eq!(receipt.incoming.len(), 1);
                assert!((receipt.incoming[0].fraction() - 1.0).abs() < 0.00001);
                // The mirrored slot may be outside the viewport; it has no ROI or visibility obligation.
                let mut clipped = normalized.clone();
                for p in &mut clipped.pages {
                    p.clip_rect = actual.rect;
                }
                preflight(&clipped, &clipped).unwrap();
            }
        }
    }
    #[test]
    fn page_turn_vacant_black_leaf_covers_unequal_static_pages_without_fading() {
        for rtl in [false, true] {
            let (mut pair, _) = fixture(true);
            let s = usize::from(!rtl);
            pair.spine_x = Some((pair.pages[0].rect.right() + pair.pages[1].rect.left()) * 0.5);
            let mut rest = pair.pages[1 - s].rect;
            if s == 1 {
                rest.min.x -= 80.0;
            } else {
                rest.max.x += 80.0;
            }
            rest.min.y -= 24.0;
            rest.max.y += 36.0;
            pair.pages[1 - s] = page(9, rest);
            pair.pages[s] = page(10, pair.pages[s].rect);
            let mut single = pair.clone();
            let mut real = pair.pages[s].rect;
            if s == 1 {
                real.max.x -= 70.0;
            } else {
                real.min.x += 70.0;
            }
            real.min.y += 50.0;
            real.max.y -= 40.0;
            single.pages[s] = page(11, real);
            let h = pair.spine_x.unwrap();
            single.pages[1 - s] = CanonicalPage {
                occurrence: None,
                rect: Rect::from_min_max(
                    pos2(2.0 * h - real.right(), real.top()),
                    pos2(2.0 * h - real.left(), real.bottom()),
                ),
                clip_rect: pair.pages[0].clip_rect,
                material: PageMaterial::Vacant,
                underlay: None,
                crop: None,
            };
            for forward in [false, true] {
                let (old, new) = if forward {
                    (&pair, &single)
                } else {
                    (&single, &pair)
                };
                let d = TurnDirection {
                    reading_rtl: rtl,
                    forward,
                };
                let recipe = prepare_turn(old, new, d).unwrap();
                for p in [0.0, 0.1, 0.25, 0.45, 0.6, 0.75, 0.95, 0.99999] {
                    let pose = scene(old, new, d, p, recipe.lift);
                    let floor = pose
                        .surfaces
                        .iter()
                        .find(|surface| surface.page.occurrence == Some(9))
                        .unwrap();
                    assert_eq!(floor.rect, rest);
                    assert_eq!(
                        floor.alpha, 1.0,
                        "static page must not fade: rtl={rtl} forward={forward} p={p}"
                    );
                    let leaf = pose.leaf.as_ref().unwrap();
                    let projected = project_prepared(old, new, d, recipe, p, false).unwrap();
                    assert!(projected.meshes.iter().any(|mesh| {
                        mesh.mesh.texture_id == TextureId::User(9)
                            && mesh
                                .mesh
                                .vertices
                                .iter()
                                .all(|vertex| vertex.color.a() == 255)
                    }));
                    assert_eq!(
                        projected.visibility.incoming.len(),
                        if forward { 1 } else { 2 }
                    );
                    if vacant(leaf.page) {
                        assert!(leaf.black_paper);
                        assert!(leaf.image_silhouette().is_empty());
                        assert!(matches!(leaf.owner, FaceOwner::Other));
                        assert!(projected.meshes.iter().any(|mesh| {
                            !mesh.mesh.indices.is_empty()
                                && mesh.mesh.texture_id == TextureId::default()
                                && mesh
                                    .mesh
                                    .vertices
                                    .iter()
                                    .all(|vertex| vertex.color == Color32::BLACK)
                        }));
                    }
                    if !forward && p == 0.0 {
                        assert_eq!(leaf.theta, 0.0);
                        let visible = projected
                            .visibility
                            .incoming
                            .iter()
                            .find(|v| v.occurrence == 9)
                            .unwrap();
                        assert!(
                            visible.fraction() < 0.00001,
                            "black front covers all of the larger incoming page"
                        );
                    }
                    if forward && p == 0.99999 {
                        let uncovered = subtract_polygon(&floor.polygon, &leaf.silhouette());
                        assert!(
                            uncovered.iter().map(|poly| polygon_area(poly)).sum::<f32>() < 1.0,
                            "black back covers all of the larger old page before flat adoption"
                        );
                    }
                }
                let end = scene(old, new, d, 1.0, recipe.lift);
                assert!(end.leaf.is_none() && end.black_floor.is_none());
                assert_eq!(end.shadow, 0.0);
                assert!(end.surfaces.iter().all(|surface| !vacant(surface.page)
                    && surface.rect == surface.page.rect
                    && surface.alpha == 1.0));
                assert_eq!(end.surfaces.len(), if forward { 1 } else { 2 });
            }
        }
    }
    #[test]
    fn normalized_spread_transitions_keep_hinge_printed_basis_and_exact_endpoints_in_four_routes() {
        let (pair, _) = fixture(true);
        let singleton = |id: u64, rect: Rect, kind| {
            let (mut unit, _) = fixture(false);
            unit.pages[0] = page(id, rect);
            normalize_spread_window(unit, kind).unwrap()
        };
        let landscape = singleton(
            31,
            Rect::from_min_max(pos2(80.0, 170.0), pos2(1000.0, 570.0)),
            SpreadWindow::Landscape,
        );
        let landscape2 = singleton(
            41,
            Rect::from_min_max(pos2(160.0, 210.0), pos2(960.0, 590.0)),
            SpreadWindow::Landscape,
        );
        let left = singleton(
            51,
            Rect::from_min_max(pos2(240.0, 110.0), pos2(600.0, 650.0)),
            SpreadWindow::Side {
                right: false,
                gap: 16.0,
            },
        );
        let right = singleton(
            61,
            Rect::from_min_max(pos2(330.0, 90.0), pos2(690.0, 630.0)),
            SpreadWindow::Side {
                right: true,
                gap: 16.0,
            },
        );
        for (old, new) in [
            (&pair, &landscape),
            (&landscape, &pair),
            (&landscape, &landscape2),
            (&pair, &left),
            (&left, &pair),
            (&pair, &right),
            (&right, &pair),
        ] {
            for reading_rtl in [false, true] {
                for forward in [false, true] {
                    let d = TurnDirection {
                        reading_rtl,
                        forward,
                    };
                    let recipe = prepare_turn(old, new, d).unwrap();
                    let s = usize::from(d.turns_right_slot());
                    let first = scene(old, new, d, 0.0, recipe.lift);
                    let near = scene(old, new, d, 0.00001, recipe.lift);
                    if let (Some(a), Some(b)) = (&first.leaf, &near.leaf) {
                        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                            assert!((a.position(u, v) - b.position(u, v)).length() < 0.001);
                        }
                    }
                    for step in 0..=200 {
                        let p = step as f32 / 200.0;
                        let projected = project_prepared(old, new, d, recipe, p, false).unwrap();
                        let h = spine(old, 1.0) + (spine(new, 1.0) - spine(old, 1.0)) * smooth(p);
                        assert!((projected.hinge_x - h).abs() < 0.001);
                        assert_eq!(projected.finished, step == 200);
                        let pose = scene(old, new, d, p, recipe.lift);
                        assert!(pose.surfaces.iter().all(|s| !vacant(s.page)));
                        for surface in &pose.surfaces {
                            assert!(
                                (surface.rect.size() - surface.page.rect.size()).length() < 0.001
                            );
                        }
                        if let Some(leaf) = &pose.leaf {
                            assert_eq!(leaf.black_paper, vacant(leaf.page));
                            if vacant(leaf.page) {
                                assert!(leaf.image_silhouette().is_empty());
                                assert!(matches!(leaf.owner, FaceOwner::Other));
                            }
                            let expected = if leaf.back {
                                &new.pages[1 - s]
                            } else {
                                &old.pages[s]
                            };
                            assert!(std::ptr::eq(leaf.page, expected));
                            for display in [pos2(0.0, 0.0), pos2(1.0, 1.0)] {
                                assert_eq!(
                                    material_uv(&leaf.page.material, display),
                                    material_uv(&expected.material, display)
                                );
                            }
                        }
                        if step == 200 {
                            assert!(pose.leaf.is_none());
                            assert_eq!(
                                pose.surfaces.len(),
                                new.pages.iter().filter(|page| !vacant(page)).count()
                            );
                            for surface in &pose.surfaces {
                                assert_eq!(surface.rect, surface.page.rect);
                            }
                            for visible in &projected.visibility.incoming {
                                assert!((visible.fraction() - 1.0).abs() < 0.0001);
                            }
                            assert_eq!(projected.angle_degrees, 180.0);
                        }
                    }
                    if vacant(&old.pages[s]) {
                        let leaf = first.leaf.as_ref().unwrap();
                        assert_eq!(leaf.theta, 0.0);
                        assert!(!leaf.back && std::ptr::eq(leaf.page, &old.pages[s]));
                        assert!(leaf.black_paper);
                    }
                    if vacant(&new.pages[1 - s]) {
                        let near_end = scene(old, new, d, 0.99999, recipe.lift);
                        let leaf = near_end.leaf.as_ref().unwrap();
                        assert!(leaf.back && std::ptr::eq(leaf.page, &new.pages[1 - s]));
                        assert!(leaf.black_paper);
                        assert!((leaf.theta - std::f32::consts::PI).abs() < 0.00001);
                        for surface in &near_end.surfaces {
                            if matches!(surface.owner, FaceOwner::Other) {
                                assert_eq!(surface.alpha, 1.0);
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn feasibility_silhouette_visibility_and_tessellation_are_measured_separately() {
        for (name, (old, new), forward) in [
            ("single_departure", fixture(false), true),
            ("single_arrival", fixture(false), false),
            ("equal_pair", fixture(true), true),
            ("portrait_pdf_scan_pair", portrait_pdf_fixture(true), true),
        ] {
            let d = TurnDirection {
                reading_rtl: true,
                forward,
            };
            let recipe = prepare_turn(&old, &new, d).unwrap();
            for i in 0..20 {
                project_prepared(&old, &new, d, recipe, i as f32 / 20.0, false).unwrap();
            }
            let mut visibility = Vec::with_capacity(300);
            let mut outer_mask = Vec::with_capacity(300);
            let mut tessellation = Vec::with_capacity(300);
            let mut total = Vec::with_capacity(300);
            for i in 0..300 {
                let p = project_prepared(&old, &new, d, recipe, (i % 100) as f32 / 100.0, true)
                    .unwrap();
                visibility.push(p.cost.silhouette_visibility_ms);
                outer_mask.push(p.cost.outer_mask_cpu_ms);
                tessellation.push(p.cost.tessellation_ms);
                total.push(p.cost.silhouette_visibility_ms + p.cost.tessellation_ms);
            }
            for values in [
                &mut visibility,
                &mut outer_mask,
                &mut tessellation,
                &mut total,
            ] {
                values.sort_by(f64::total_cmp);
            }
            println!(
                "R5-a {name} cached debug CPU n=300: silhouette_visibility p50={:.4}ms p95={:.4}ms; outer_mask p95={:.4}ms (subset); tessellation p50={:.4}ms p95={:.4}ms; sum p95={:.4}ms (not GPU/runtime acceptance)",
                visibility[150],
                visibility[285],
                outer_mask[285],
                tessellation[150],
                tessellation[285],
                total[285]
            );
        }
    }
    #[test]
    fn opposite_vacant_slots_skip_only_the_direction_with_no_actual_leaf() {
        let (old, new) = fixture(false);
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
        assert_eq!(
            prepare_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: true
                }
            )
            .unwrap_err()
            .sub_check(),
            "vacant_leaf_on_both_sides"
        );
        assert!(
            prepare_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: false,
                    forward: false
                }
            )
            .is_ok()
        );
        assert_eq!(
            prepare_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: true,
                    forward: false
                }
            )
            .unwrap_err()
            .sub_check(),
            "vacant_leaf_on_both_sides"
        );
        assert!(
            prepare_turn(
                &old,
                &new,
                TurnDirection {
                    reading_rtl: true,
                    forward: true
                }
            )
            .is_ok()
        );
    }
}
