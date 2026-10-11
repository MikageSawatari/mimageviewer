//! Pure book-leaf projection. Materials and per-page placements are immutable;
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
    Texture {
        texture_id: TextureId,
        uv: [Pos2; 4],
        available_uv: Rect,
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
    fn silhouette(&self) -> Vec<Pos2> {
        let mut p = vec![
            self.position(0.0, 0.0),
            self.position(1.0, 0.0),
            self.position(1.0, 1.0),
            self.position(0.0, 1.0),
        ];
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
        accepted.ok_or(ProjectionFailure::IncomingNotVisible(
            "initial_target_visibility",
        ))?
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
    if progress == 0.0 && !meaningful(&receipt) {
        return Err(ProjectionFailure::IncomingNotVisible(
            "initial_target_visibility",
        ));
    }
    if let Some(start) = start {
        cost.silhouette_visibility_ms = start.elapsed().as_secs_f64() * 1000.0;
    }
    let mesh_start = measure.then(std::time::Instant::now);
    let mut meshes = Vec::with_capacity(24);
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
                180.0
            } else if direction.forward {
                90.0
            } else {
                0.0
            },
            if old.layout == UnitLayout::Single {
                spine(
                    if direction.forward { old } else { new },
                    direction.single_leaf_sign(),
                )
            } else {
                new.spine_x.unwrap_or(new.pages[0].rect.left())
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
    let width = old
        .pages
        .iter()
        .chain(&new.pages)
        .map(|p| p.rect.width())
        .fold(1.0_f32, f32::max);
    let height = old
        .pages
        .iter()
        .chain(&new.pages)
        .map(|p| p.rect.height())
        .fold(1.0_f32, f32::max);
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
fn max_leaf_distance(old: &CanonicalUnit, new: &CanonicalUnit) -> f32 {
    [old, new]
        .into_iter()
        .map(|unit| {
            if unit.layout == UnitLayout::Spread {
                let h = spine(unit, 1.0);
                unit.pages
                    .iter()
                    .flat_map(|p| [(p.rect.left() - h).abs(), (p.rect.right() - h).abs()])
                    .fold(1.0_f32, f32::max)
            } else {
                unit.pages[0].rect.width()
            }
        })
        .fold(1.0_f32, f32::max)
}
fn smooth(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
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
fn gutter(unit: &CanonicalUnit) -> f32 {
    if unit.layout == UnitLayout::Spread {
        unit.gutter_width
    } else {
        0.0
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
            surfaces.push(surface(page, page.rect, incoming_owner(page), 1.0));
        }
        return Scene {
            surfaces,
            leaf: None,
            shadow: 0.0,
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
    let theta = if !pair && !d.forward {
        lift * (1.0 - smooth(p))
    } else {
        lift + (if pair {
            std::f32::consts::PI
        } else {
            std::f32::consts::FRAC_PI_2
        } - lift)
            * smooth(p)
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
    let hinge = if pair {
        h0 + (h1 - h0) * morph
    } else if d.forward {
        h0
    } else {
        h1
    };
    let mut leaf_clip = front.clip_rect;
    if !pair {
        leaf_clip = slot(leaf_clip, hinge, 0.0, sign);
    }
    if pair {
        let under = &new.pages[s];
        let rect = slot(
            under.rect.intersect(outer_gate(front.rect, under.rect, p)),
            hinge,
            gutter(old),
            sign,
        );
        surfaces.push(surface(under, rect, incoming_owner(under), 1.0));
        let rest = &old.pages[1 - s];
        let rest_rect = slot(rest.rect, hinge, gutter(old), -sign);
        // Only canonical-footprint remnants retire in the final tenth.
        let hit = rest_rect.intersect(back.rect);
        if valid_rect(hit) {
            surfaces.push(surface(rest, hit, FaceOwner::Other, 1.0));
        }
        let alpha = 1.0 - smooth((p - 0.9) / 0.1);
        for r in subtract_rect(rest_rect, back.rect) {
            surfaces.push(surface(rest, r, FaceOwner::Other, alpha));
        }
    } else if d.forward {
        surfaces.push(surface(
            back,
            back.rect.intersect(outer_gate(front.rect, back.rect, p)),
            incoming_owner(back),
            1.0,
        ));
    } else {
        let hit = front.rect.intersect(back.rect);
        if valid_rect(hit) {
            surfaces.push(surface(front, hit, FaceOwner::Other, 1.0));
        }
        for r in subtract_rect(front.rect, back.rect) {
            surfaces.push(surface(
                front,
                r,
                FaceOwner::Other,
                1.0 - smooth((p - 0.9) / 0.1),
            ));
        }
    }
    let front_inner = if sign > 0.0 {
        front.rect.left() - h0
    } else {
        h0 - front.rect.right()
    };
    let front_outer = front_inner + front.rect.width();
    let back_inner = if -sign > 0.0 {
        back.rect.left() - h1
    } else {
        h1 - back.rect.right()
    };
    let back_outer = back_inner + back.rect.width();
    let is_back = pair && theta > std::f32::consts::FRAC_PI_2;
    let (page, owner) = if is_back {
        (back, incoming_owner(back))
    } else if !pair && !d.forward {
        (back, incoming_owner(back))
    } else {
        (front, FaceOwner::OutgoingFront)
    };
    let interpolate = |a: f32, b: f32| a + (b - a) * morph;
    let (inner, outer, top, bottom) = if pair {
        (
            interpolate(front_inner, back_inner),
            interpolate(front_outer, back_outer),
            interpolate(front.rect.top(), back.rect.top()),
            interpolate(front.rect.bottom(), back.rect.bottom()),
        )
    } else {
        let rect = page.rect;
        let inner = if sign > 0.0 {
            rect.left() - hinge
        } else {
            hinge - rect.right()
        };
        (inner, inner + rect.width(), rect.top(), rect.bottom())
    };
    let envelope = old
        .pages
        .iter()
        .chain(&new.pages)
        .map(|p| p.rect.width().max(p.rect.height()))
        .fold(1.0_f32, f32::max);
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
        focal: envelope.max(max_leaf_distance(old, new)) * 8.0,
        center_y: (top + bottom) * 0.5,
        clip: leaf_clip,
        back: is_back,
    };
    Scene {
        surfaces,
        leaf: Some(leaf),
        shadow: theta.sin().abs(),
    }
}
fn surface(page: &CanonicalPage, rect: Rect, owner: FaceOwner, alpha: f32) -> Surface<'_> {
    Surface {
        page,
        polygon: rectangle(rect.intersect(page.clip_rect)),
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
/// The spine moves monotonically. Half-plane clip and extra support-gap losses
/// are monotone in H; their maximum is therefore at one of the two endpoints,
/// independent of how many intermediate mesh frames will be painted.
fn admission(
    old: &CanonicalUnit,
    new: &CanonicalUnit,
    d: TurnDirection,
) -> Result<ProjectionCost, ProjectionFailure> {
    if old.layout == UnitLayout::Spread
        && (gutter(old) - gutter(new)).abs() > 1.0 / old.pixels_per_point
    {
        return Err(ProjectionFailure::UnsupportedGeometry(
            "spread_gutter_changed",
        ));
    }
    let cost = static_surface_loss(old, new, d);
    if cost.static_clip_loss_fraction > 0.02 + 0.00001 {
        return Err(ProjectionFailure::UnsupportedGeometry(
            "spine_static_surface_loss_clip",
        ));
    }
    if cost.static_support_loss_fraction > 0.02 + 0.00001 {
        return Err(ProjectionFailure::UnsupportedGeometry(
            "spine_static_surface_loss_support",
        ));
    }
    Ok(cost)
}
/// Perf-only caller may ask for rejected trajectory maxima without building a
/// mesh or weakening the admission. Monotone endpoint proof is shared.
pub(crate) fn static_surface_loss(
    old: &CanonicalUnit,
    new: &CanonicalUnit,
    d: TurnDirection,
) -> ProjectionCost {
    let mut cost = ProjectionCost::default();
    if old.layout == UnitLayout::Single || old.pages.len() != 2 || new.pages.len() != 2 {
        return cost;
    }
    let sign = if d.turns_right_slot() { 1.0 } else { -1.0 };
    let s = usize::from(sign > 0.0);
    let h0 = spine(old, sign);
    let h1 = spine(new, sign);
    let gap = gutter(old);
    for (page, side, normal_h, name) in [
        (&new.pages[s], sign, h1, "incoming_under"),
        (&old.pages[1 - s], -sign, h0, "outgoing_rest"),
    ] {
        let normal = page.rect.intersect(page.clip_rect);
        let denominator = area(normal);
        if denominator <= 0.0 {
            continue;
        }
        let normal_edge = if side > 0.0 {
            normal_h + gap * 0.5
        } else {
            normal_h - gap * 0.5
        };
        let inner = if side > 0.0 {
            page.rect.left()
        } else {
            page.rect.right()
        };
        let normal_gap = ((inner - normal_edge) * side).max(0.0);
        for h in [h0, h1] {
            let masked = slot(normal, h, gap, side);
            let fraction = (1.0 - area(masked) / denominator).max(0.0);
            if fraction > cost.static_clip_loss_fraction {
                cost.static_clip_loss_fraction = fraction;
                cost.static_clip_loss_surface = name;
                cost.static_clip_loss_hinge = h;
            }
            let edge = if side > 0.0 {
                h + gap * 0.5
            } else {
                h - gap * 0.5
            };
            let extra = ((inner - edge) * side - normal_gap).max(0.0);
            let fraction = extra * normal.height() / denominator;
            if fraction > cost.static_support_loss_fraction {
                cost.static_support_loss_fraction = fraction;
                cost.static_support_loss_surface = name;
                cost.static_support_loss_hinge = h;
            }
        }
    }
    cost
}
fn visibility(scene: &Scene<'_>, new: &CanonicalUnit, old: &CanonicalUnit) -> VisibilityReceipt {
    let mut faces: Vec<(Vec<Pos2>, FaceOwner)> = scene
        .surfaces
        .iter()
        .filter(|s| s.alpha > 0.0 && !s.polygon.is_empty())
        .map(|s| (s.polygon.clone(), s.owner))
        .collect();
    if let Some(leaf) = &scene.leaf {
        let polygon = leaf.silhouette();
        if polygon_area(&polygon) > 0.001 {
            faces.push((polygon, leaf.owner));
        }
    }
    let mut receipt = VisibilityReceipt {
        incoming: new
            .pages
            .iter()
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
            .map(|p| area(p.rect.intersect(p.clip_rect)))
            .sum(),
    };
    for (i, (poly, owner)) in faces.iter().enumerate() {
        let mut pieces = vec![poly.clone()];
        for (cover, _) in &faces[i + 1..] {
            pieces = pieces
                .into_iter()
                .flat_map(|p| subtract_polygon(&p, cover))
                .collect();
        }
        for polygon in pieces {
            match owner {
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
    if surface.polygon.len() < 3 || surface.alpha <= 0.0 {
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
        for p in &surface.polygon {
            let display = pos2(
                (p.x - surface.page.rect.left()) / surface.page.rect.width(),
                (p.y - surface.page.rect.top()) / surface.page.rect.height(),
            );
            mesh.vertices.push(egui::epaint::Vertex {
                pos: *p,
                uv: material_uv(material, display),
                color: color.gamma_multiply(surface.alpha),
            });
        }
        for i in 1..surface.polygon.len() - 1 {
            mesh.indices
                .extend_from_slice(&[0, i as u32, (i + 1) as u32]);
        }
        meshes.push(ProjectedMesh {
            mesh,
            clip_rect: surface.page.clip_rect,
        });
    }
}
fn leaf_meshes(meshes: &mut Vec<ProjectedMesh>, leaf: &Leaf<'_>, grid_u: usize, grid_v: usize) {
    // Neutral paper belongs only to the unknown reverse/edge. It never borrows
    // black/checker background and does not change the printed face's alpha.
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
    let materials: Vec<&PageMaterial> = if leaf.theta.cos().abs() < 0.00001 {
        vec![&paper]
    } else {
        leaf.page
            .underlay
            .iter()
            .chain(std::iter::once(&leaf.page.material))
            .collect()
    };
    for material in materials {
        let (texture, color) = material_style(material);
        let mut mesh = Mesh::with_texture(texture);
        // Interior mesh is clipped by the same destination/aperture as silhouette.
        // egui's scissor handles clipping; UVs preserve each material's own basis.
        for y in 0..=grid_v {
            for x in 0..=grid_u {
                let u = x as f32 / grid_u as f32;
                let v = y as f32 / grid_v as f32;
                let light = (0.84 + 0.16 * leaf.theta.cos().abs()
                    - 0.10 * leaf.theta.sin().abs() * u)
                    .max(0.72);
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: leaf.position(u, v),
                    uv: material_uv(material, leaf.display(u, v)),
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
    // Six low-alpha rings approximate a soft contact shadow. Each is clipped
    // to actual visible receiving surfaces, including the outer-expansion gate.
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
        for receiver in &scene.surfaces {
            if receiver.alpha <= 0.0 {
                continue;
            }
            let poly = clip_polygon(&shifted, &receiver.polygon);
            append_solid_polygon(
                &mut mesh,
                &poly,
                Color32::from_black_alpha((6.0 * scene.shadow * receiver.alpha) as u8),
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
        PageMaterial::Texture { texture_id, .. } => (*texture_id, Color32::WHITE),
        PageMaterial::Blank { color } => (TextureId::default(), *color),
    }
}
fn material_uv(material: &PageMaterial, display: Pos2) -> Pos2 {
    match material {
        PageMaterial::Texture { uv, .. } => bilinear_uv(*uv, display),
        PageMaterial::Blank { .. } => egui::epaint::WHITE_UV,
    }
}
fn incoming_owner(page: &CanonicalPage) -> FaceOwner {
    page.occurrence
        .map_or(FaceOwner::Other, FaceOwner::Incoming)
}
#[cfg(test)]
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
    if cover.len() < 3 {
        return vec![poly.to_vec()];
    }
    let mut remaining = poly.to_vec();
    let mut result = Vec::with_capacity(cover.len());
    for (a, b) in edges(cover) {
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
                    let hinge = if reading_rtl {
                        moving.rect.right()
                    } else {
                        moving.rect.left()
                    };
                    let mut previous_angle = if forward { 0.0 } else { 90.0 };
                    let mut previous_free_x: Option<f32> = None;
                    for step in 0..200 {
                        let progress = step as f32 / 200.0;
                        let pose = scene(&old, &new, direction, progress, recipe.lift);
                        let leaf = pose.leaf.as_ref().unwrap();
                        assert_eq!(leaf.hinge, hinge);
                        assert_eq!(leaf.position(0.0, 0.0), pos2(hinge, moving.rect.top()));
                        assert_eq!(leaf.position(0.0, 1.0), pos2(hinge, moving.rect.bottom()));
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
                            assert!((free_x - previous).abs() < moving.rect.width() * 0.025);
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
    fn unequal_scan_and_height_are_admitted_but_large_spine_shift_is_not() {
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
        assert!(
            project_turn(&old, &new, d, 0.0)
                .unwrap_err()
                .sub_check()
                .starts_with("spine_static_surface_loss")
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
    fn width_swap_and_two_percent_boundary_have_structural_reasons() {
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
                assert!(
                    cost.static_clip_loss_fraction
                        .max(cost.static_support_loss_fraction)
                        >= 0.5
                );
                assert!(
                    prepare_turn(&old, &new, d)
                        .unwrap_err()
                        .sub_check()
                        .starts_with("spine_static_surface_loss")
                );
            }
        }
        let (old, mut new) = fixture(true);
        let d = TurnDirection {
            reading_rtl: false,
            forward: true,
        };
        new.spine_x = old.spine_x.map(|h| h + old.pages[0].rect.width() * 0.019);
        assert!(admission(&old, &new, d).is_ok());
        new.spine_x = old.spine_x.map(|h| h + old.pages[0].rect.width() * 0.021);
        assert!(admission(&old, &new, d).is_err());
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
}
