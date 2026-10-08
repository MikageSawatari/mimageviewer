//! Review-only: public production compositor with a deterministic 2x AI runner.
//! No model loading, no app main, no real profile. All raster files are disposable.
use std::sync::Arc;
use mimageviewer::books::*;

fn main() {
    let root = std::path::PathBuf::from("target/review-v350-round3/annotation");
    std::fs::create_dir_all(&root).unwrap();
    mimageviewer::data_dir::set_test_override(Some(root.join("data")));
    let source_path = root.join("source.png");
    image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 0, 0, 255])).save(&source_path).unwrap();
    let source = CompositeSource::File { path: source_path };
    for dims in [None, Some([16, 16])] {
        let mut bubble = comic_core::BubbleObject::default();
        bubble.shape = comic_core::BubbleShape::RoundRect { half_w: 2.0, half_h: 2.0, corner_px: 0.0 };
        bubble.fill = Some(comic_core::Rgba::new(255, 0, 0, 255));
        bubble.fill_opacity = 1.0;
        bubble.outline.width_px = 0.0;
        bubble.text = comic_core::TextBlock::default();
        bubble.auto_size = false;
        let edits = BakedEditSnapshot {
            params: mimageviewer::adjustment::AdjustParams::default(),
            rotation: mimageviewer::rotation_db::Rotation::None,
            conceal: None, erase: None, local_adjust: None,
            comic: Some(BookComicSnapshot {
                objects: vec![comic_core::AnnotationObject::new_bubble(1, (8.0, 8.0), bubble)],
                fonts: Arc::new(comic_core::FontSet::new()), stamp_cache: Default::default(),
            }),
            comic_source_dims: dims, export_crop: None, crop_legacy_writeback: None,
            format: mimageviewer::capture::CaptureFormat::Png,
            jpeg_matte: mimageviewer::capture::JpegMatte::Black,
            stage: mimageviewer::bake_stage::BakeStage::Ai,
            creative_lut: None,
            ai: Some(BookAiSnapshot { run: Box::new(|image, _| {
                assert_eq!(image.size, [16, 16]);
                Ok(BookAiResult { image: egui::ColorImage::new([32, 32], vec![egui::Color32::BLACK; 1024]), used_upscale: true })
            }) }),
        };
        let dest = root.join(if dims.is_some() { "with-dims.png" } else { "without-dims.png" });
        write_composited_page(&source, &edits, &dest, mimageviewer::export_dialog::ExportScale::Full).unwrap();
        let output = image::open(dest).unwrap().to_rgba8();
        let red: Vec<_> = output.enumerate_pixels().filter(|(_, _, p)| p[0] > 200 && p[1] < 10).map(|(x, y, _)| (x, y)).collect();
        let bounds = (red.iter().map(|p| p.0).min().unwrap(), red.iter().map(|p| p.1).min().unwrap(),
                      red.iter().map(|p| p.0).max().unwrap(), red.iter().map(|p| p.1).max().unwrap());
        println!("comic_source_dims={dims:?}: output={}x{} red_bounds={bounds:?} at_expected_center={:?} at_original_center={:?}",
                 output.width(), output.height(), output.get_pixel(16, 16).0, output.get_pixel(8, 8).0);
        assert_eq!(output.get_pixel(16, 16)[0] > 200, true);
    }
}

