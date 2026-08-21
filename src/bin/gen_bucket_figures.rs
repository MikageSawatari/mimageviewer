//! バケツの各設定による選択範囲の違いを、マニュアル掲載用 WebP へ生成する。
//!
//! 合成した決定的な元画像へ本体と同じ `mask_db::flood_fill_bitmap_mask` を適用するため、
//! バケツの挙動を変更したときは次のコマンドで 7 点を再生成し、表示差を確認する。
//!
//! ```text
//! cargo run --release --features dev-tools --bin gen_bucket_figures
//! ```
//!
//! 出力先は `htdocs/mimageviewer/manual/images/fig-bucket-*.webp`。

use std::{error::Error, fs, io, path::PathBuf};

use egui::{Color32, ColorImage};
use mimageviewer::mask_db::{BucketFill, BucketFillOutcome, BucketRegion, flood_fill_bitmap_mask};

const PANEL_W: usize = 300;
const PANEL_H: usize = 200;
const GUTTER: usize = 4;
const WEBP_QUALITY: f32 = 85.0;

const PAPER: Color32 = Color32::from_rgb(241, 237, 226);
const INK: Color32 = Color32::from_rgb(48, 54, 61);
const MASK_RGB: [u8; 3] = [180, 80, 220];
const MASK_ALPHA: u16 = 140;

type AnyResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone)]
struct Source {
    image: ColorImage,
}

impl Source {
    fn new(width: usize, height: usize, color: Color32) -> Self {
        Self {
            image: ColorImage::new([width, height], vec![color; width * height]),
        }
    }

    fn width(&self) -> usize {
        self.image.size[0]
    }

    fn height(&self) -> usize {
        self.image.size[1]
    }

    fn set(&mut self, x: usize, y: usize, color: Color32) {
        if x < self.width() && y < self.height() {
            let width = self.width();
            self.image.pixels[y * width + x] = color;
        }
    }

    fn fill_rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, color: Color32) {
        let width = self.width();
        let x1 = x1.min(width);
        let y1 = y1.min(self.height());
        for y in y0.min(y1)..y1 {
            self.image.pixels[y * width + x0.min(x1)..y * width + x1].fill(color);
        }
    }

    fn fill_circle(&mut self, center: (f32, f32), radius: f32, color: Color32) {
        let radius_sq = radius * radius;
        for y in 0..self.height() {
            for x in 0..self.width() {
                let dx = x as f32 + 0.5 - center.0;
                let dy = y as f32 + 0.5 - center.1;
                if dx * dx + dy * dy <= radius_sq {
                    self.set(x, y, color);
                }
            }
        }
    }

    fn fill_rotated_rect(
        &mut self,
        center: (f32, f32),
        half_length: f32,
        half_width: f32,
        angle_deg: f32,
        color: Color32,
    ) {
        let angle = angle_deg.to_radians();
        let (sin, cos) = angle.sin_cos();
        for y in 0..self.height() {
            for x in 0..self.width() {
                let dx = x as f32 + 0.5 - center.0;
                let dy = y as f32 + 0.5 - center.1;
                let along = dx * cos + dy * sin;
                let across = -dx * sin + dy * cos;
                if along.abs() <= half_length && across.abs() <= half_width {
                    self.set(x, y, color);
                }
            }
        }
    }

    fn fill_horizontal_gradient(
        &mut self,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        from: u8,
        to: u8,
    ) {
        let span = x1.saturating_sub(x0).max(1);
        for x in x0..x1.min(self.width()) {
            let t = (x - x0) as f32 / (span - 1).max(1) as f32;
            let value = (f32::from(from) + f32::from(to - from) * t).round() as u8;
            self.fill_rect(x, y0, x + 1, y1, Color32::from_rgb(value, value, value));
        }
    }
}

struct RenderedPanel {
    rgb: Vec<u8>,
    width: usize,
    height: usize,
    mask: Vec<bool>,
    mask_width: usize,
    mask_height: usize,
}

impl RenderedPanel {
    fn mask_count(&self) -> usize {
        self.mask.iter().filter(|inside| **inside).count()
    }

    fn mask_at(&self, x: usize, y: usize) -> bool {
        x < self.mask_width && y < self.mask_height && self.mask[y * self.mask_width + x]
    }
}

fn main() -> AnyResult<()> {
    let output_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("htdocs")
        .join("mimageviewer")
        .join("manual")
        .join("images");
    fs::create_dir_all(&output_dir)?;

    figure_region(&output_dir)?;
    figure_shapes(&output_dir)?;
    figure_tolerance(&output_dir)?;
    figure_leak_stop(&output_dir)?;
    figure_outset(&output_dir)?;
    figure_gap(&output_dir)?;
    figure_corridor(&output_dir)?;

    println!("generated 7 bucket figures in {}", output_dir.display());
    Ok(())
}

fn figure_region(output_dir: &std::path::Path) -> AnyResult<()> {
    let mut source = Source::new(PANEL_W, PANEL_H, PAPER);
    source.fill_rect(25, 55, 170, 145, INK);
    source.fill_rect(68, 78, 76, 122, PAPER);
    source.fill_rect(96, 72, 104, 128, PAPER);
    source.fill_rect(125, 96, 148, 104, PAPER);
    source.fill_circle((238.0, 68.0), 30.0, INK);
    source.fill_rect(215, 120, 278, 166, INK);
    let seed = (45, 100);

    let whole = render_bucket(&source, seed, BucketRegion::Whole, 0, 0.0, 0.0, 10.0, 1)?;
    let connected = render_bucket(&source, seed, BucketRegion::Connected, 0, 0.0, 0.0, 10.0, 1)?;
    let rect = render_bucket(&source, seed, BucketRegion::Rect, 0, 0.0, 0.0, 10.0, 1)?;

    require(
        whole.mask_count() > connected.mask_count() + 3_000,
        "fig-bucket-region: Whole must include the disconnected shapes",
    )?;
    require(
        rect.mask_count() > connected.mask_count() + 500 && rect.mask_count() < whole.mask_count(),
        "fig-bucket-region: Rect must close the holes without selecting every shape",
    )?;
    save_figure(
        output_dir,
        "fig-bucket-region.webp",
        &[whole, connected, rect],
    )
}

fn figure_shapes(output_dir: &std::path::Path) -> AnyResult<()> {
    let mut source = Source::new(PANEL_W, PANEL_H, PAPER);
    source.fill_rect(70, 40, 230, 160, INK);
    source.fill_circle((70.0, 100.0), 60.0, INK);
    source.fill_circle((230.0, 100.0), 60.0, INK);
    let seed = (150, 100);

    let rect = render_bucket(&source, seed, BucketRegion::Rect, 0, 0.0, 0.0, 10.0, 1)?;
    let ellipse = render_bucket(&source, seed, BucketRegion::Ellipse, 0, 0.0, 0.0, 10.0, 1)?;
    let circle = render_bucket(&source, seed, BucketRegion::Circle, 0, 0.0, 0.0, 10.0, 1)?;

    require_pairwise_differences("fig-bucket-shapes", &[&rect, &ellipse, &circle], 800)?;
    save_figure(
        output_dir,
        "fig-bucket-shapes.webp",
        &[rect, ellipse, circle],
    )
}

fn figure_tolerance(output_dir: &std::path::Path) -> AnyResult<()> {
    let mut source = Source::new(PANEL_W, PANEL_H, PAPER);
    source.fill_horizontal_gradient(25, 55, 275, 145, 50, 130);
    let seed = (35, 100);
    let low = render_bucket(
        &source,
        seed,
        BucketRegion::Connected,
        12,
        0.0,
        0.0,
        10.0,
        1,
    )?;
    let high = render_bucket(
        &source,
        seed,
        BucketRegion::Connected,
        90,
        0.0,
        0.0,
        10.0,
        1,
    )?;
    require(
        high.mask_count() > low.mask_count() * 3,
        "fig-bucket-tolerance: the high tolerance must select much more of the gradient",
    )?;
    save_figure(output_dir, "fig-bucket-tolerance.webp", &[low, high])
}

fn figure_leak_stop(output_dir: &std::path::Path) -> AnyResult<()> {
    let mut source = Source::new(PANEL_W, PANEL_H, PAPER);
    source.fill_rect(20, 45, 135, 155, INK);
    source.fill_rect(135, 97, 178, 103, INK);
    source.fill_rect(178, 60, 280, 140, INK);
    let seed = (65, 100);
    let open = render_bucket(&source, seed, BucketRegion::Connected, 0, 0.0, 0.0, 10.0, 1)?;
    let stopped = render_bucket(&source, seed, BucketRegion::Connected, 0, 3.0, 0.0, 10.0, 1)?;
    require(
        open.mask_at(225, 100) && !stopped.mask_at(225, 100),
        "fig-bucket-leak-stop: leak stop 3 must keep the neighboring block unselected",
    )?;
    require(
        stopped.mask_count() + 5_000 < open.mask_count(),
        "fig-bucket-leak-stop: the two panels must have a clearly different area",
    )?;
    save_figure(output_dir, "fig-bucket-leak-stop.webp", &[open, stopped])
}

fn figure_outset(output_dir: &std::path::Path) -> AnyResult<()> {
    const SCALE: usize = 4;
    let mut source = Source::new(PANEL_W / SCALE, PANEL_H / SCALE, PAPER);
    source.fill_rect(14, 14, 61, 36, Color32::from_rgb(176, 173, 165));
    source.fill_rect(16, 16, 59, 34, Color32::from_rgb(103, 104, 104));
    source.fill_rect(18, 18, 57, 32, INK);
    let seed = (37, 25);
    let zero = render_bucket(
        &source,
        seed,
        BucketRegion::Connected,
        0,
        0.0,
        0.0,
        10.0,
        SCALE,
    )?;
    let two = render_bucket(
        &source,
        seed,
        BucketRegion::Connected,
        0,
        0.0,
        2.0,
        10.0,
        SCALE,
    )?;
    require(
        !zero.mask_at(16, 25) && two.mask_at(16, 25),
        "fig-bucket-outset: outset 2 must cover the two-pixel fringe",
    )?;
    require(
        two.mask_count() > zero.mask_count() + 150,
        "fig-bucket-outset: outset 2 must visibly enlarge the selected area",
    )?;
    save_figure(output_dir, "fig-bucket-outset.webp", &[zero, two])
}

fn figure_gap(output_dir: &std::path::Path) -> AnyResult<()> {
    let mut source = Source::new(PANEL_W, PANEL_H, PAPER);
    source.fill_rect(20, 55, 280, 145, INK);
    // 黒帯内の文字相当の抜け。各文字は別成分で、帯全体の 10% より十分小さい。
    source.fill_rect(88, 74, 96, 126, PAPER);
    source.fill_rect(112, 74, 120, 126, PAPER);
    source.fill_rect(96, 96, 112, 104, PAPER);
    source.fill_rect(150, 74, 174, 82, PAPER);
    source.fill_rect(158, 82, 166, 118, PAPER);
    source.fill_rect(150, 118, 174, 126, PAPER);
    let seed = (50, 100);
    let zero = render_bucket(&source, seed, BucketRegion::Rect, 0, 0.0, 0.0, 0.0, 1)?;
    let ten = render_bucket(&source, seed, BucketRegion::Rect, 0, 0.0, 0.0, 10.0, 1)?;
    require(
        !zero.mask_at(92, 90) && ten.mask_at(92, 90),
        "fig-bucket-gap: 10% must fill the letter-shaped hole left by 0%",
    )?;
    require(
        ten.mask_count() > zero.mask_count() + 10_000,
        "fig-bucket-gap: the filled band must be clearly larger",
    )?;
    save_figure(output_dir, "fig-bucket-gap.webp", &[zero, ten])
}

fn figure_corridor(output_dir: &std::path::Path) -> AnyResult<()> {
    let mut source = Source::new(PANEL_W, PANEL_H, PAPER);
    source.fill_rotated_rect((150.0, 100.0), 138.0, 14.0, 25.0, INK);
    source.fill_rotated_rect((150.0, 100.0), 138.0, 14.0, -25.0, INK);
    let left_seed = (55, 56);
    let right_seed = (245, 56);
    let left = render_bucket(&source, left_seed, BucketRegion::Rect, 0, 0.0, 0.0, 10.0, 1)?;
    let right = render_bucket(
        &source,
        right_seed,
        BucketRegion::Rect,
        0,
        0.0,
        0.0,
        10.0,
        1,
    )?;
    require(
        left.mask_at(245, 144) && !right.mask_at(245, 144),
        "fig-bucket-corridor: the left click must choose its crossing arm",
    )?;
    require(
        right.mask_at(55, 144) && !left.mask_at(55, 144),
        "fig-bucket-corridor: the right click must choose the other crossing arm",
    )?;
    save_figure(output_dir, "fig-bucket-corridor.webp", &[left, right])
}

#[allow(clippy::too_many_arguments)]
fn render_bucket(
    source: &Source,
    seed: (usize, usize),
    region: BucketRegion,
    tolerance: u8,
    leak_stop: f32,
    outset: f32,
    gap_tolerance: f32,
    display_scale: usize,
) -> AnyResult<RenderedPanel> {
    let mut mask = vec![false; source.width() * source.height()];
    let outcome = flood_fill_bitmap_mask(
        &mut mask,
        &source.image,
        seed.0,
        seed.1,
        BucketFill {
            tolerance,
            region,
            value: true,
            leak_stop,
            outset,
            gap_tolerance,
        },
    );
    if outcome != BucketFillOutcome::Filled {
        return Err(io::Error::other(format!(
            "bucket figure failed at seed {seed:?} for {region:?}: {outcome:?}"
        ))
        .into());
    }

    let mut logical_rgb = Vec::with_capacity(mask.len() * 3);
    for (source_pixel, inside) in source.image.pixels.iter().zip(&mask) {
        let base = [source_pixel.r(), source_pixel.g(), source_pixel.b()];
        let out = if *inside {
            blend_rgb(base, MASK_RGB, MASK_ALPHA)
        } else {
            base
        };
        logical_rgb.extend_from_slice(&out);
    }

    let (mut rgb, width, height) =
        scale_rgb_nearest(&logical_rgb, source.width(), source.height(), display_scale);
    let seed_center = (
        seed.0 * display_scale + display_scale / 2,
        seed.1 * display_scale + display_scale / 2,
    );
    draw_cross_marker(&mut rgb, width, height, seed_center);

    Ok(RenderedPanel {
        rgb,
        width,
        height,
        mask,
        mask_width: source.width(),
        mask_height: source.height(),
    })
}

fn blend_rgb(base: [u8; 3], overlay: [u8; 3], alpha: u16) -> [u8; 3] {
    let inverse = 255 - alpha;
    std::array::from_fn(|channel| {
        ((u16::from(base[channel]) * inverse + u16::from(overlay[channel]) * alpha + 127) / 255)
            as u8
    })
}

fn scale_rgb_nearest(
    source: &[u8],
    width: usize,
    height: usize,
    scale: usize,
) -> (Vec<u8>, usize, usize) {
    let scale = scale.max(1);
    if scale == 1 {
        return (source.to_vec(), width, height);
    }
    let output_width = width * scale;
    let output_height = height * scale;
    let mut output = vec![0; output_width * output_height * 3];
    for y in 0..output_height {
        let source_y = y / scale;
        for x in 0..output_width {
            let source_x = x / scale;
            let source_offset = (source_y * width + source_x) * 3;
            let output_offset = (y * output_width + x) * 3;
            output[output_offset..output_offset + 3]
                .copy_from_slice(&source[source_offset..source_offset + 3]);
        }
    }
    (output, output_width, output_height)
}

fn draw_cross_marker(rgb: &mut [u8], width: usize, height: usize, center: (usize, usize)) {
    for offset in -7..=7 {
        for thickness in -1..=1 {
            set_rgb(
                rgb,
                width,
                height,
                center.0 as isize + offset,
                center.1 as isize + thickness,
                [22, 22, 22],
            );
            set_rgb(
                rgb,
                width,
                height,
                center.0 as isize + thickness,
                center.1 as isize + offset,
                [22, 22, 22],
            );
        }
    }
    for offset in -5..=5 {
        set_rgb(
            rgb,
            width,
            height,
            center.0 as isize + offset,
            center.1 as isize,
            [255, 244, 96],
        );
        set_rgb(
            rgb,
            width,
            height,
            center.0 as isize,
            center.1 as isize + offset,
            [255, 244, 96],
        );
    }
}

fn set_rgb(rgb: &mut [u8], width: usize, height: usize, x: isize, y: isize, color: [u8; 3]) {
    if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
        return;
    }
    let offset = (y as usize * width + x as usize) * 3;
    rgb[offset..offset + 3].copy_from_slice(&color);
}

fn save_figure(
    output_dir: &std::path::Path,
    filename: &str,
    panels: &[RenderedPanel],
) -> AnyResult<()> {
    require(!panels.is_empty(), "a figure needs at least one panel")?;
    let panel_width = panels[0].width;
    let panel_height = panels[0].height;
    require(
        panels
            .iter()
            .all(|panel| panel.width == panel_width && panel.height == panel_height),
        "all panels in one figure must have the same display size",
    )?;
    require_pairwise_differences(filename, &panels.iter().collect::<Vec<_>>(), 1)?;

    let width = panel_width * panels.len() + GUTTER * (panels.len() + 1);
    let height = panel_height + GUTTER * 2;
    let mut rgb = vec![255; width * height * 3];
    for (panel_index, panel) in panels.iter().enumerate() {
        let x0 = GUTTER + panel_index * (panel_width + GUTTER);
        for y in 0..panel_height {
            let source_start = y * panel_width * 3;
            let target_start = ((y + GUTTER) * width + x0) * 3;
            rgb[target_start..target_start + panel_width * 3]
                .copy_from_slice(&panel.rgb[source_start..source_start + panel_width * 3]);
        }
    }

    let encoded = webp::Encoder::from_rgb(&rgb, width as u32, height as u32).encode(WEBP_QUALITY);
    let output_path = output_dir.join(filename);
    fs::write(&output_path, encoded.as_ref())?;
    let counts: Vec<_> = panels.iter().map(RenderedPanel::mask_count).collect();
    println!(
        "{}: {}x{}, mask pixels {:?}",
        filename, width, height, counts
    );
    Ok(())
}

fn require_pairwise_differences(
    figure: &str,
    panels: &[&RenderedPanel],
    minimum: usize,
) -> AnyResult<()> {
    for left in 0..panels.len() {
        for right in left + 1..panels.len() {
            require(
                panels[left].mask_width == panels[right].mask_width
                    && panels[left].mask_height == panels[right].mask_height,
                "compared masks must have the same dimensions",
            )?;
            let difference = panels[left]
                .mask
                .iter()
                .zip(&panels[right].mask)
                .filter(|(a, b)| a != b)
                .count();
            require(
                difference >= minimum,
                &format!("{figure}: panels {left} and {right} differ by only {difference} pixels"),
            )?;
        }
    }
    Ok(())
}

fn require(condition: bool, message: &str) -> AnyResult<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}
