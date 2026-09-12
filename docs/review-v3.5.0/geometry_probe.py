"""Run arithmetic probes against functions extracted from the reviewed source.

This does not start mImageViewer or access an application profile. The continuous
case isolates the single-page, untrimmed branch of continuous_reading_page_rects;
the actual quantization, extent, rectangle snap and offset functions are compiled
verbatim from the current source, using the already built egui library.
"""
from pathlib import Path
import re
import subprocess

root = Path(__file__).resolve().parents[2]
out = root / "target" / "review-v350"
out.mkdir(exist_ok=True)
transform = (root / "src/displayed_image_transform.rs").read_text(encoding="utf-8")
fullscreen = (root / "src/ui_fullscreen.rs").read_text(encoding="utf-8")


def function(source, name):
    match = re.search(r"^(?:pub\(crate\) )?fn " + name + r"\(", source, re.M)
    assert match, name
    end = source.index("\n}", match.start()) + 2
    return source[match.start():end]


functions = [function(transform, name) for name in [
    "normalized_pixels_per_point", "quantize_points_to_physical_pixels",
    "physical_pixel_extent", "snap_rect_to_physical_pixels",
]]
functions.append(function(fullscreen, "vertical_reading_offsets"))
epsilon = re.search(r"^const PHYSICAL_PIXEL_EXTENT_EPSILON:.*;$", transform, re.M)[0]
program = epsilon + "\n" + "\n".join(functions) + r'''
fn main() {
    // A source page is 501 x 1001 physical pixels. Original fit, no trim,
    // no spread; scroll positions put a unit above the viewport origin.
    for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
        let scale = 1.0 / ppp;
        let height = 1001.0 / ppp;
        for gap in [0.0, 1.0, 20.0] {
            let offsets = vertical_reading_offsets(&[height; 3], gap, 0, ppp);
            let rects: Vec<_> = offsets.iter().map(|offset| {
                let frame = egui::Rect::from_min_size(
                    egui::pos2(0.0, quantize_points_to_physical_pixels(
                        offset - height * 0.5, ppp)),
                    egui::vec2(501.0 / ppp, height));
                snap_rect_to_physical_pixels(frame, egui::vec2(501.0, 1001.0), scale, ppp)
            }).collect();
            let measured = (rects[1].top() - rects[0].bottom()) * ppp;
            let expected = (gap * ppp).round();
            println!("continuous ppp={ppp} gap_setting={gap} expected_px={expected} actual_px={measured} first_bottom={} next_top={}", rects[0].bottom()*ppp, rects[1].top()*ppp);
        }
    }
    for full_width in [640.0_f32, 800.0, 860.0, 1200.0] {
        let panel_width = 430.0_f32;
        let reserved = panel_width.min(full_width * 0.5);
        let content_width = full_width - reserved;
        // Current ui_fullscreen.rs::draw_music_fullscreen_view reconstruction.
        let inferred = panel_width.min((content_width + panel_width) * 0.5);
        println!("music full_width={full_width} reserved={reserved} reconstructed={inferred} drawn_right={} overflow={}", content_width+inferred, content_width+inferred-full_width);
    }
}
'''
probe = out / "geometry_probe.rs"
probe.write_text(program, encoding="utf-8")
egui = max((root / "target/debug/deps").glob("libegui-*.rlib"), key=lambda p: p.stat().st_mtime)
exe = out / "geometry_probe.exe"
subprocess.run(["rustc", "--edition=2024", str(probe), "-L", f"dependency={root / 'target/debug/deps'}", "--extern", f"egui={egui}", "-o", str(exe)], check=True)
result = subprocess.run([str(exe)], check=True, capture_output=True, text=True)
print(result.stdout, end="")
(Path(__file__).parent / "geometry-probe.log").write_text(result.stdout, encoding="utf-8")
