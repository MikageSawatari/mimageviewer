"""Compile current production geometry verbatim, without starting the application.

The fixture provides page/unit sizes and follows the callers' snapped common
origin + unit offset placement. Both page placement and painted-band arithmetic
are extracted, including trim and rotation handling. This is arithmetic coverage,
not native UI/GPU verification. Generated files stay in target/review-v350-round2.
"""
from pathlib import Path
import re
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
out = root / "target/review-v350-round2"
out.mkdir(exist_ok=True)
revision = sys.argv[1] if len(sys.argv) > 1 else None
def source_text(path):
    if revision:
        return subprocess.check_output(["git", "show", f"{revision}:{path}"], cwd=root, text=True, encoding="utf-8")
    return (root / path).read_text(encoding="utf-8")
transform = source_text("src/displayed_image_transform.rs")
fullscreen = source_text("src/ui_fullscreen.rs")
rotation = source_text("src/rotation_db.rs")


def block(source, declaration):
    match = re.search(r"^" + declaration, source, re.M)
    assert match, declaration
    return source[match.start():source.index("\n}", match.start()) + 2]


def function(source, name):
    return block(source, r"(?:pub\(crate\) )?fn " + name + r"\(")


parts = ["#![allow(dead_code)]", "mod rotation_db { #[derive(Clone, Copy, Debug)]",
         block(rotation, "pub enum Rotation"), "}",
         "mod displayed_image_transform { use crate::rotation_db::Rotation;",
         re.search(r"^const PHYSICAL_PIXEL_EXTENT_EPSILON:.*;$", transform, re.M)[0]]
parts.extend(function(transform, name) for name in [
    "normalized_pixels_per_point", "quantize_points_to_physical_pixels",
    "physical_pixel_extent", "quantized_band_px", "visible_paint_band_px",
    "rotate_bbox_to_display", "forward_uv",
])
parts += ["}", "use displayed_image_transform::quantize_points_to_physical_pixels;"]
for declaration in ["struct ContinuousReadingPageSize", "impl ContinuousReadingPageSize",
                    "enum ContinuousAxis", "struct ContinuousReadingUnitSize",
                    "struct ContinuousUnitDrawnSpan"]:
    if declaration.startswith("enum"):
        parts.append("#[derive(Clone, Copy, Debug)]")
    parts.append(block(fullscreen, declaration))
parts.extend(function(fullscreen, name) for name in [
    "continuous_reading_page_rects", "continuous_unit_drawn_span", "vertical_reading_offsets",
])
parts.append(r'''
fn main() {
    let mut total = 0;
    let mut mismatches = 0;
    for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
        for physical_scale in [1.0_f32, 0.73, 1.125] {
            let scale = physical_scale / ppp;
            for mixed in [false, true] {
                for trimmed in [false, true] {
                    let sizes: Vec<_> = (0..5).map(|i| {
                        let source_height = if mixed && i % 2 == 0 { 1000.0 } else { 1001.0 };
                        let mut page = ContinuousReadingPageSize::full(i, 501.0 * scale, source_height * scale);
                        page.logical_scale = scale;
                        if trimmed {
                            page.content_bbox = Some(egui::Rect::from_min_max(egui::pos2(0.1, 0.13), egui::pos2(0.9, 0.86)));
                        }
                        ContinuousReadingUnitSize { width: page.width, height: page.height,
                            pages: vec![page], page_gap: 0.0, logical_scale: scale }
                    }).collect();
                    let heights: Vec<_> = sizes.iter().map(|s| {
                        let span = continuous_unit_drawn_span(s, ppp);
                        span.y_max - span.y_min
                    }).collect();
                    for gap in [0.0_f32, 1.0, 20.0] {
                        let offsets = vertical_reading_offsets(&heights, gap, 2, ppp);
                        for base_px in [-1000.5_f32, -0.5, 0.0, 0.5, 987.25] {
                            let origin = quantize_points_to_physical_pixels(base_px / ppp, ppp);
                            let bands: Vec<_> = sizes.iter().zip(&offsets).map(|(size, offset)| {
                                let unit = egui::Rect::from_center_size(egui::pos2(0.0, origin + offset), egui::vec2(size.width, size.height));
                                let rect = continuous_reading_page_rects(unit, size, ppp)[0].1;
                                let (lo, hi) = size.pages[0].drawn_band(ContinuousAxis::Y, ppp);
                                ((rect.top() + lo) * ppp, (rect.top() + hi) * ppp)
                            }).collect();
                            let expected = quantize_points_to_physical_pixels(gap, ppp) * ppp;
                            let actual: Vec<_> = bands.windows(2).map(|p| p[1].0-p[0].1).collect();
                            total += actual.len();
                            let failures = actual.iter().filter(|a| (**a - expected).abs() > 0.01).count();
                            mismatches += failures;
                            if base_px == 0.0 && (failures > 0 || (gap == 0.0 && physical_scale == 1.0)) {
                                println!("ppp={ppp} physical_scale={physical_scale} mixed={mixed} trim={trimmed} gap_px={expected} gaps={actual:?} painted={bands:?}");
                            }
                        }
                    }
                }
            }
        }
    }
    println!("Checked {total} adjacent boundaries; {mismatches} differ from the configured physical gap by >0.01px.");
}
''')
suffix = "-baseline" if revision else ""
probe = out / f"geometry_probe{suffix}.rs"
probe.write_text("\n".join(parts), encoding="utf-8")
egui = max((root / "target/debug/deps").glob("libegui-*.rlib"), key=lambda p: p.stat().st_mtime)
exe = out / f"geometry_probe{suffix}.exe"
subprocess.run(["rustc", "--edition=2024", str(probe), "-L", f"dependency={root / 'target/debug/deps'}",
                "--extern", f"egui={egui}", "-o", str(exe)], check=True)
result = subprocess.run([str(exe)], check=True, capture_output=True, text=True)
print(result.stdout, end="")
Path(__file__).with_name(f"geometry-probe{suffix}.log").write_text(result.stdout, encoding="utf-8")
