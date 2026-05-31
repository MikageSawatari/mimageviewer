use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Color32, ColorImage, Key, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions,
};
use image::{ImageBuffer, Rgba, RgbaImage};

fn main() -> eframe::Result<()> {
    let initial_path = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mIV Mask Lab")
            .with_inner_size([1280.0, 840.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "mIV Mask Lab",
        options,
        Box::new(move |cc| Ok(Box::new(MaskLabApp::new(cc, initial_path)))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Polygon,
    Brush,
    SmartBrush,
    BoundaryAdd,
    BoundaryRemove,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Tool::Polygon => "Polygon",
            Tool::Brush => "Brush",
            Tool::SmartBrush => "Smart Brush",
            Tool::BoundaryAdd => "Add Edge",
            Tool::BoundaryRemove => "Remove Edge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaintOp {
    Add,
    Remove,
}

struct LoadedImage {
    path: PathBuf,
    rgba: RgbaImage,
    color_image: ColorImage,
    edge: Vec<f32>,
    w: usize,
    h: usize,
}

struct MaskLabApp {
    image: Option<LoadedImage>,
    texture: Option<TextureHandle>,
    mask_texture: Option<TextureHandle>,
    mask: Vec<bool>,
    mask_dirty: bool,
    tool: Tool,
    paint_op: PaintOp,
    polygon: Vec<Pos2>,
    use_snap: bool,
    snap_radius: f32,
    brush_radius: f32,
    color_tolerance: f32,
    edge_threshold: f32,
    boundary_width: i32,
    morph_radius: i32,
    smooth_iters: usize,
    alpha: u8,
    status: String,
}

impl MaskLabApp {
    fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        let mut app = Self {
            image: None,
            texture: None,
            mask_texture: None,
            mask: Vec::new(),
            mask_dirty: false,
            tool: Tool::Polygon,
            paint_op: PaintOp::Add,
            polygon: Vec::new(),
            use_snap: true,
            snap_radius: 8.0,
            brush_radius: 28.0,
            color_tolerance: 38.0,
            edge_threshold: 38.0,
            boundary_width: 2,
            morph_radius: 2,
            smooth_iters: 1,
            alpha: 112,
            status: "Drop a JPEG or PNG file here.".to_string(),
        };
        if let Some(path) = initial_path {
            app.load_path(&cc.egui_ctx, &path);
        }
        app
    }

    fn load_path(&mut self, ctx: &egui::Context, path: &Path) {
        match load_image(path) {
            Ok(img) => {
                self.mask = vec![false; img.w * img.h];
                self.texture = Some(ctx.load_texture(
                    "source_image",
                    img.color_image.clone(),
                    TextureOptions::LINEAR,
                ));
                self.mask_texture = None;
                self.mask_dirty = true;
                self.polygon.clear();
                self.status = format!("Loaded {}", path.display());
                self.image = Some(img);
            }
            Err(e) => {
                self.status = format!("Load failed: {e}");
            }
        }
    }

    fn image_size(&self) -> Option<(usize, usize)> {
        self.image.as_ref().map(|img| (img.w, img.h))
    }

    fn ensure_mask_texture(&mut self, ctx: &egui::Context) {
        if !self.mask_dirty {
            return;
        }
        let Some((w, h)) = self.image_size() else {
            return;
        };
        let pixels: Vec<Color32> = self
            .mask
            .iter()
            .map(|&on| {
                if on {
                    Color32::from_rgba_unmultiplied(255, 40, 80, self.alpha)
                } else {
                    Color32::TRANSPARENT
                }
            })
            .collect();
        let overlay = ColorImage::new([w, h], pixels);
        if let Some(tex) = &mut self.mask_texture {
            tex.set(overlay, TextureOptions::NEAREST);
        } else {
            self.mask_texture =
                Some(ctx.load_texture("mask_overlay", overlay, TextureOptions::NEAREST));
        }
        self.mask_dirty = false;
    }

    fn clear_mask(&mut self) {
        self.mask.fill(false);
        self.mask_dirty = true;
    }

    fn fill_polygon(&mut self) {
        if self.polygon.len() < 3 {
            self.status = "Polygon needs at least 3 points.".to_string();
            return;
        }
        let points = if self.use_snap {
            if let Some(img) = &self.image {
                snap_polygon(&self.polygon, &img.edge, img.w, img.h, self.snap_radius)
            } else {
                self.polygon.clone()
            }
        } else {
            self.polygon.clone()
        };
        if let Some((w, h)) = self.image_size() {
            fill_polygon_mask(&mut self.mask, w, h, &points, self.paint_op == PaintOp::Add);
            self.mask_dirty = true;
            self.status = format!("Filled polygon ({} points).", points.len());
        }
        self.polygon.clear();
    }

    fn apply_brush(&mut self, p: Pos2) {
        let Some(img) = &self.image else {
            return;
        };
        match self.tool {
            Tool::Brush => apply_plain_brush(
                &mut self.mask,
                img.w,
                img.h,
                p,
                self.brush_radius,
                self.paint_op == PaintOp::Add,
            ),
            Tool::SmartBrush => apply_smart_brush(
                &mut self.mask,
                img,
                p,
                self.brush_radius,
                self.color_tolerance,
                self.edge_threshold,
                self.paint_op == PaintOp::Add,
            ),
            Tool::BoundaryAdd => apply_boundary_brush(
                &mut self.mask,
                img,
                p,
                self.brush_radius,
                self.edge_threshold,
                self.boundary_width,
                true,
            ),
            Tool::BoundaryRemove => apply_boundary_brush(
                &mut self.mask,
                img,
                p,
                self.brush_radius,
                self.edge_threshold,
                self.boundary_width,
                false,
            ),
            Tool::Polygon => {}
        }
        self.mask_dirty = true;
    }

    fn image_to_screen(rect: Rect, img_w: usize, img_h: usize, p: Pos2) -> Pos2 {
        let sx = rect.width() / img_w as f32;
        let sy = rect.height() / img_h as f32;
        Pos2::new(rect.left() + p.x * sx, rect.top() + p.y * sy)
    }

    fn screen_to_image(rect: Rect, img_w: usize, img_h: usize, p: Pos2) -> Option<Pos2> {
        if !rect.contains(p) {
            return None;
        }
        let x = ((p.x - rect.left()) / rect.width()) * img_w as f32;
        let y = ((p.y - rect.top()) / rect.height()) * img_h as f32;
        Some(Pos2::new(
            x.clamp(0.0, img_w as f32 - 1.0),
            y.clamp(0.0, img_h as f32 - 1.0),
        ))
    }

    fn save_mask(&mut self) {
        let Some(img) = &self.image else {
            return;
        };
        match save_mask_png(img, &self.mask) {
            Ok(path) => self.status = format!("Saved {}", path.display()),
            Err(e) => self.status = format!("Save mask failed: {e}"),
        }
    }

    fn save_overlay(&mut self) {
        let Some(img) = &self.image else {
            return;
        };
        match save_overlay_png(img, &self.mask) {
            Ok(path) => self.status = format!("Saved {}", path.display()),
            Err(e) => self.status = format!("Save overlay failed: {e}"),
        }
    }
}

impl eframe::App for MaskLabApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            if let Some(path) = file.path {
                self.load_path(ctx, &path);
                break;
            }
        }

        if ctx.input(|i| i.key_pressed(Key::Enter)) && self.tool == Tool::Polygon {
            self.fill_polygon();
        }
        if ctx.input(|i| i.key_pressed(Key::Backspace)) && self.tool == Tool::Polygon {
            self.polygon.pop();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.polygon.clear();
        }

        egui::SidePanel::left("tools")
            .resizable(false)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.heading("Mask Lab");
                ui.label(&self.status);
                ui.separator();

                ui.label("Tool");
                for tool in [
                    Tool::Polygon,
                    Tool::Brush,
                    Tool::SmartBrush,
                    Tool::BoundaryAdd,
                    Tool::BoundaryRemove,
                ] {
                    ui.radio_value(&mut self.tool, tool, tool.label());
                }

                ui.separator();
                ui.label("Paint");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.paint_op, PaintOp::Add, "Add");
                    ui.radio_value(&mut self.paint_op, PaintOp::Remove, "Remove");
                });

                ui.separator();
                ui.collapsing("Polygon", |ui| {
                    ui.checkbox(&mut self.use_snap, "Snap edges");
                    ui.add(
                        egui::Slider::new(&mut self.snap_radius, 0.0..=32.0).text("snap radius"),
                    );
                    if ui.button("Finish polygon").clicked() {
                        self.fill_polygon();
                    }
                    if ui.button("Undo point").clicked() {
                        self.polygon.pop();
                    }
                    if ui.button("Clear points").clicked() {
                        self.polygon.clear();
                    }
                    ui.label("Click to add points. Enter finishes. Backspace removes a point.");
                });

                ui.collapsing("Brush", |ui| {
                    ui.add(egui::Slider::new(&mut self.brush_radius, 2.0..=160.0).text("radius"));
                    ui.add(
                        egui::Slider::new(&mut self.color_tolerance, 1.0..=120.0)
                            .text("color tolerance"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.edge_threshold, 1.0..=160.0)
                            .text("edge threshold"),
                    );
                    ui.add(egui::Slider::new(&mut self.boundary_width, 1..=8).text("edge width"));
                    ui.label(
                        "Smart Brush grows inside the brush circle and stops at strong edges.",
                    );
                });

                ui.separator();
                ui.label("Mask Adjust");
                ui.add(egui::Slider::new(&mut self.morph_radius, 1..=12).text("radius"));
                ui.horizontal(|ui| {
                    if ui.button("Expand").clicked() {
                        if let Some((w, h)) = self.image_size() {
                            self.mask = dilate_mask(&self.mask, w, h, self.morph_radius);
                            self.mask_dirty = true;
                        }
                    }
                    if ui.button("Shrink").clicked() {
                        if let Some((w, h)) = self.image_size() {
                            self.mask = erode_mask(&self.mask, w, h, self.morph_radius);
                            self.mask_dirty = true;
                        }
                    }
                });
                ui.add(egui::Slider::new(&mut self.smooth_iters, 1..=8).text("smooth iters"));
                if ui.button("Smooth").clicked() {
                    if let Some((w, h)) = self.image_size() {
                        self.mask = smooth_mask(&self.mask, w, h, self.smooth_iters);
                        self.mask_dirty = true;
                    }
                }

                ui.separator();
                ui.add(egui::Slider::new(&mut self.alpha, 20..=220).text("overlay alpha"));
                if ui.button("Clear mask").clicked() {
                    self.clear_mask();
                }
                ui.horizontal(|ui| {
                    if ui.button("Save mask").clicked() {
                        self.save_mask();
                    }
                    if ui.button("Save overlay").clicked() {
                        self.save_overlay();
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.ensure_mask_texture(ctx);
            let (img_w, img_h, tex_id) = match (&self.image, &self.texture) {
                (Some(img), Some(tex)) => (img.w, img.h, tex.id()),
                _ => {
                    ui.centered_and_justified(|ui| {
                        ui.label("Drop a JPEG or PNG file here.");
                    });
                    return;
                }
            };

            let available = ui.available_size();
            let image_aspect = img_w as f32 / img_h as f32;
            let mut draw_size = available;
            if draw_size.x / draw_size.y > image_aspect {
                draw_size.x = draw_size.y * image_aspect;
            } else {
                draw_size.y = draw_size.x / image_aspect;
            }
            let (rect, response) = ui.allocate_exact_size(draw_size, Sense::click_and_drag());
            let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            ui.painter().image(tex_id, rect, uv, Color32::WHITE);
            if let Some(mask_tex) = &self.mask_texture {
                ui.painter().image(mask_tex.id(), rect, uv, Color32::WHITE);
            }

            let pointer_img = response
                .interact_pointer_pos()
                .and_then(|p| Self::screen_to_image(rect, img_w, img_h, p));

            if self.tool == Tool::Polygon {
                if response.double_clicked() {
                    self.fill_polygon();
                } else if response.clicked() {
                    if let Some(p) = pointer_img {
                        self.polygon.push(p);
                    }
                }
            } else if (response.dragged() || response.clicked()) && pointer_img.is_some() {
                self.apply_brush(pointer_img.unwrap());
            }

            if !self.polygon.is_empty() {
                let screen_points: Vec<Pos2> = self
                    .polygon
                    .iter()
                    .map(|&p| Self::image_to_screen(rect, img_w, img_h, p))
                    .collect();
                for pair in screen_points.windows(2) {
                    ui.painter()
                        .line_segment([pair[0], pair[1]], Stroke::new(2.0, Color32::YELLOW));
                }
                if let Some(cursor) = pointer_img {
                    let last = *screen_points.last().unwrap();
                    let cur = Self::image_to_screen(rect, img_w, img_h, cursor);
                    ui.painter()
                        .line_segment([last, cur], Stroke::new(1.0, Color32::LIGHT_YELLOW));
                }
                for p in screen_points {
                    ui.painter().circle_filled(p, 4.0, Color32::YELLOW);
                }
            }

            if let Some(p) = pointer_img {
                if self.tool != Tool::Polygon {
                    let center = Self::image_to_screen(rect, img_w, img_h, p);
                    let scale = rect.width() / img_w as f32;
                    ui.painter().circle_stroke(
                        center,
                        self.brush_radius * scale,
                        Stroke::new(1.0, Color32::LIGHT_GREEN),
                    );
                }
            }
        });
    }
}

fn load_image(path: &Path) -> Result<LoadedImage, String> {
    let dyn_img = image::open(path).map_err(|e| e.to_string())?;
    let rgba = dyn_img.to_rgba8();
    let w = rgba.width() as usize;
    let h = rgba.height() as usize;
    let color_image = ColorImage::from_rgba_unmultiplied([w, h], rgba.as_raw());
    let gray = compute_gray(&rgba);
    let edge = compute_sobel_edge(&gray, w, h);
    Ok(LoadedImage {
        path: path.to_path_buf(),
        rgba,
        color_image,
        edge,
        w,
        h,
    })
}

fn compute_gray(img: &RgbaImage) -> Vec<f32> {
    img.pixels()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect()
}

fn compute_sobel_edge(gray: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    if w < 3 || h < 3 {
        return out;
    }
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = |xx: usize, yy: usize| gray[yy * w + xx];
            let gx = -i(x - 1, y - 1) + i(x + 1, y - 1) - 2.0 * i(x - 1, y) + 2.0 * i(x + 1, y)
                - i(x - 1, y + 1)
                + i(x + 1, y + 1);
            let gy = -i(x - 1, y - 1) - 2.0 * i(x, y - 1) - i(x + 1, y - 1)
                + i(x - 1, y + 1)
                + 2.0 * i(x, y + 1)
                + i(x + 1, y + 1);
            out[y * w + x] = (gx * gx + gy * gy).sqrt().min(255.0);
        }
    }
    out
}

fn fill_polygon_mask(mask: &mut [bool], w: usize, h: usize, pts: &[Pos2], add: bool) {
    if pts.len() < 3 {
        return;
    }
    let min_x = pts
        .iter()
        .map(|p| p.x)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_x = pts
        .iter()
        .map(|p| p.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(w as f32 - 1.0) as usize;
    let min_y = pts
        .iter()
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_y = pts
        .iter()
        .map(|p| p.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(h as f32 - 1.0) as usize;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if point_in_polygon(x as f32 + 0.5, y as f32 + 0.5, pts) {
                mask[y * w + x] = add;
            }
        }
    }
}

fn point_in_polygon(x: f32, y: f32, pts: &[Pos2]) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let pi = pts[i];
        let pj = pts[j];
        let crosses = (pi.y > y) != (pj.y > y);
        if crosses {
            let x_at_y = (pj.x - pi.x) * (y - pi.y) / (pj.y - pi.y).max(1e-6) + pi.x;
            if x < x_at_y {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn snap_polygon(pts: &[Pos2], edge: &[f32], w: usize, h: usize, radius: f32) -> Vec<Pos2> {
    if pts.len() < 2 || radius <= 0.0 {
        return pts.to_vec();
    }
    let mut out = Vec::new();
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let nx = -dy / len;
        let ny = dx / len;
        let steps = (len / 3.0).ceil().max(1.0) as usize;
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let base = Pos2::new(a.x + dx * t, a.y + dy * t);
            out.push(best_edge_near(base, nx, ny, edge, w, h, radius));
        }
    }
    smooth_points(&out, 2)
}

fn best_edge_near(
    base: Pos2,
    nx: f32,
    ny: f32,
    edge: &[f32],
    w: usize,
    h: usize,
    radius: f32,
) -> Pos2 {
    let mut best = base;
    let mut best_score = -1.0;
    let r = radius.round() as i32;
    for d in -r..=r {
        let px = base.x + nx * d as f32;
        let py = base.y + ny * d as f32;
        if px < 0.0 || py < 0.0 || px >= w as f32 || py >= h as f32 {
            continue;
        }
        let idx = py as usize * w + px as usize;
        let score = edge[idx] - (d.unsigned_abs() as f32 * 1.5);
        if score > best_score {
            best_score = score;
            best = Pos2::new(px, py);
        }
    }
    best
}

fn smooth_points(points: &[Pos2], iterations: usize) -> Vec<Pos2> {
    let mut cur = points.to_vec();
    if cur.len() < 5 {
        return cur;
    }
    for _ in 0..iterations {
        let mut next = cur.clone();
        for i in 0..cur.len() {
            let prev = cur[(i + cur.len() - 1) % cur.len()];
            let p = cur[i];
            let n = cur[(i + 1) % cur.len()];
            next[i] = Pos2::new(
                (prev.x + p.x * 2.0 + n.x) * 0.25,
                (prev.y + p.y * 2.0 + n.y) * 0.25,
            );
        }
        cur = next;
    }
    cur
}

fn apply_plain_brush(mask: &mut [bool], w: usize, h: usize, p: Pos2, radius: f32, add: bool) {
    let r2 = radius * radius;
    let min_x = (p.x - radius).floor().max(0.0) as usize;
    let max_x = (p.x + radius).ceil().min(w as f32 - 1.0) as usize;
    let min_y = (p.y - radius).floor().max(0.0) as usize;
    let max_y = (p.y + radius).ceil().min(h as f32 - 1.0) as usize;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - p.x;
            let dy = y as f32 + 0.5 - p.y;
            if dx * dx + dy * dy <= r2 {
                mask[y * w + x] = add;
            }
        }
    }
}

fn apply_smart_brush(
    mask: &mut [bool],
    img: &LoadedImage,
    p: Pos2,
    radius: f32,
    tolerance: f32,
    edge_threshold: f32,
    add: bool,
) {
    let sx = p.x.round() as i32;
    let sy = p.y.round() as i32;
    if sx < 0 || sy < 0 || sx >= img.w as i32 || sy >= img.h as i32 {
        return;
    }
    let min_x = (p.x - radius).floor().max(0.0) as i32;
    let max_x = (p.x + radius).ceil().min(img.w as f32 - 1.0) as i32;
    let min_y = (p.y - radius).floor().max(0.0) as i32;
    let max_y = (p.y + radius).ceil().min(img.h as f32 - 1.0) as i32;
    let bw = (max_x - min_x + 1) as usize;
    let bh = (max_y - min_y + 1) as usize;
    let mut visited = vec![false; bw * bh];
    let mut q = VecDeque::new();
    let seed = pixel_rgb(&img.rgba, sx as usize, sy as usize);
    let tol2 = tolerance * tolerance * 3.0;
    let r2 = radius * radius;

    let local_idx = |x: i32, y: i32| -> usize { (y - min_y) as usize * bw + (x - min_x) as usize };
    visited[local_idx(sx, sy)] = true;
    q.push_back((sx, sy));
    while let Some((x, y)) = q.pop_front() {
        mask[y as usize * img.w + x as usize] = add;
        for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
            if nx < min_x || nx > max_x || ny < min_y || ny > max_y {
                continue;
            }
            let li = local_idx(nx, ny);
            if visited[li] {
                continue;
            }
            visited[li] = true;
            let dx = nx as f32 + 0.5 - p.x;
            let dy = ny as f32 + 0.5 - p.y;
            if dx * dx + dy * dy > r2 {
                continue;
            }
            let gi = ny as usize * img.w + nx as usize;
            if img.edge[gi] > edge_threshold {
                continue;
            }
            if color_dist2(seed, pixel_rgb(&img.rgba, nx as usize, ny as usize)) > tol2 {
                continue;
            }
            q.push_back((nx, ny));
        }
    }
}

fn apply_boundary_brush(
    mask: &mut [bool],
    img: &LoadedImage,
    p: Pos2,
    radius: f32,
    edge_threshold: f32,
    width: i32,
    add: bool,
) {
    let r2 = radius * radius;
    let min_x = (p.x - radius).floor().max(0.0) as i32;
    let max_x = (p.x + radius).ceil().min(img.w as f32 - 1.0) as i32;
    let min_y = (p.y - radius).floor().max(0.0) as i32;
    let max_y = (p.y + radius).ceil().min(img.h as f32 - 1.0) as i32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - p.x;
            let dy = y as f32 + 0.5 - p.y;
            if dx * dx + dy * dy > r2 {
                continue;
            }
            let idx = y as usize * img.w + x as usize;
            if img.edge[idx] < edge_threshold {
                continue;
            }
            for oy in -width..=width {
                for ox in -width..=width {
                    if ox * ox + oy * oy > width * width {
                        continue;
                    }
                    let nx = x + ox;
                    let ny = y + oy;
                    if nx >= 0 && ny >= 0 && nx < img.w as i32 && ny < img.h as i32 {
                        mask[ny as usize * img.w + nx as usize] = add;
                    }
                }
            }
        }
    }
}

fn pixel_rgb(img: &RgbaImage, x: usize, y: usize) -> [f32; 3] {
    let p = img.get_pixel(x as u32, y as u32);
    [p[0] as f32, p[1] as f32, p[2] as f32]
}

fn color_dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    dr * dr + dg * dg + db * db
}

fn dilate_mask(mask: &[bool], w: usize, h: usize, radius: i32) -> Vec<bool> {
    morph_mask(mask, w, h, radius, true)
}

fn erode_mask(mask: &[bool], w: usize, h: usize, radius: i32) -> Vec<bool> {
    morph_mask(mask, w, h, radius, false)
}

fn morph_mask(mask: &[bool], w: usize, h: usize, radius: i32, dilate: bool) -> Vec<bool> {
    let mut out = vec![!dilate; w * h];
    let r2 = radius * radius;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut value = !dilate;
            'search: for oy in -radius..=radius {
                for ox in -radius..=radius {
                    if ox * ox + oy * oy > r2 {
                        continue;
                    }
                    let nx = x + ox;
                    let ny = y + oy;
                    let sample = if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                        mask[ny as usize * w + nx as usize]
                    } else {
                        false
                    };
                    if dilate && sample {
                        value = true;
                        break 'search;
                    }
                    if !dilate && !sample {
                        value = false;
                        break 'search;
                    }
                    if !dilate {
                        value = true;
                    }
                }
            }
            out[y as usize * w + x as usize] = value;
        }
    }
    out
}

fn smooth_mask(mask: &[bool], w: usize, h: usize, iterations: usize) -> Vec<bool> {
    let mut cur = mask.to_vec();
    for _ in 0..iterations {
        let mut next = cur.clone();
        for y in 0..h {
            for x in 0..w {
                let mut count = 0;
                let mut total = 0;
                for oy in -1..=1 {
                    for ox in -1..=1 {
                        let nx = x as i32 + ox;
                        let ny = y as i32 + oy;
                        if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                            total += 1;
                            if cur[ny as usize * w + nx as usize] {
                                count += 1;
                            }
                        }
                    }
                }
                next[y * w + x] = count * 2 >= total;
            }
        }
        cur = next;
    }
    cur
}

fn save_mask_png(img: &LoadedImage, mask: &[bool]) -> Result<PathBuf, String> {
    let mut out = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(img.w as u32, img.h as u32);
    for y in 0..img.h {
        for x in 0..img.w {
            let v = if mask[y * img.w + x] { 255 } else { 0 };
            out.put_pixel(x as u32, y as u32, Rgba([v, v, v, 255]));
        }
    }
    let path = derived_path(&img.path, "_mask.png");
    out.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn save_overlay_png(img: &LoadedImage, mask: &[bool]) -> Result<PathBuf, String> {
    let mut out = img.rgba.clone();
    for y in 0..img.h {
        for x in 0..img.w {
            if mask[y * img.w + x] {
                let p = out.get_pixel_mut(x as u32, y as u32);
                p[0] = ((p[0] as u16 + 255) / 2) as u8;
                p[1] = (p[1] as u16 / 2) as u8;
                p[2] = ((p[2] as u16 + 80) / 2) as u8;
                p[3] = 255;
            }
        }
    }
    let path = derived_path(&img.path, "_mask_overlay.png");
    out.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn derived_path(src: &Path, suffix: &str) -> PathBuf {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    parent.join(format!("{stem}{suffix}"))
}
