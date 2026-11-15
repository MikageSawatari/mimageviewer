use super::*;
use std::io::Cursor;

fn temporary() -> tempfile::TempDir {
    let target = std::env::current_dir().unwrap().join("target");
    std::fs::create_dir_all(&target).unwrap();
    tempfile::Builder::new()
        .prefix("cover-book-list-")
        .tempdir_in(target)
        .unwrap()
}

fn jacket() -> DynamicImage {
    // Same synthetic faces as the detector's jacket fixtures, no real images.
    DynamicImage::ImageRgb8(RgbImage::from_fn(892, 512, |x, y| {
        Rgb(if x < 140 {
            if y % 73 < 8 { [65; 3] } else { [246, 242, 220] }
        } else if x < 498 {
            let wave = (y % 80) as u8;
            [35 + wave, 55 + (y % 155) as u8, 200 - wave]
        } else if x < 534 {
            [18, 28, 38]
        } else if y % 73 < 8 {
            [75; 3]
        } else {
            [246, 242, 220]
        })
    }))
}

fn zip_book(path: &Path, cover: &DynamicImage) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for page in 0..=4 {
        let mut bytes = Cursor::new(Vec::new());
        let image = if page == 0 {
            cover.clone()
        } else {
            DynamicImage::new_rgb8(70, 100)
        };
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        zip.start_file(
            format!("{page:03}.png"),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(bytes.get_ref()).unwrap();
    }
    zip.finish().unwrap();
}

fn folder_book(path: &Path, cover: &DynamicImage) {
    std::fs::create_dir(path).unwrap();
    cover.save(path.join("000.png")).unwrap();
    for page in 1..=4 {
        DynamicImage::new_rgb8(70, 100)
            .save(path.join(format!("{page:03}.png")))
            .unwrap();
    }
}

fn pdf_book(path: &Path) {
    // Four real pages, vector content on the cover. PageSizes + rendering use
    // the ordinary shared PDF operations through the in-process test backend.
    let content = "0.2 0.4 0.7 rg 0 0 360 200 re f\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 360 200] /Resources << >> /Contents 7 0 R >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 140 200] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 140 200] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 140 200] >>".to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 8\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Root 1 0 R /Size 8 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    std::fs::write(path, bytes).unwrap();
}

fn list(path: &Path, books: &[PathBuf]) {
    // Exercise the UTF-8 BOM written by the documented PowerShell input route.
    let mut bytes = b"\xef\xbb\xbf".to_vec();
    bytes.extend(serde_json::to_vec(books).unwrap());
    std::fs::write(path, bytes).unwrap();
}

fn summary(output: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(output.join("summary.json")).unwrap()).unwrap()
}

fn assert_frame(output: &Path, row: &serde_json::Value, field: &str, blue: bool) {
    let raster = image::open(output.join(row["image_filename"].as_str().unwrap()))
        .unwrap()
        .to_rgb8();
    let bounds: [f64; 4] = serde_json::from_value(row[field].clone()).unwrap();
    let x = ((bounds[0] * f64::from(raster.width())).floor() as u32 + 1).min(raster.width() - 1);
    let y = ((bounds[1] + bounds[3]) * 0.5 * f64::from(raster.height())) as u32;
    let [r, g, b] = raster.get_pixel(x, y).0.map(i32::from);
    if blue {
        assert!(b > r + 40 && b > g + 40, "{row}: {r},{g},{b}");
    } else {
        assert!(r > b + 40 && r > g + 40, "{row}: {r},{g},{b}");
    }
}

#[test]
fn evaluation_book_list_mixed_real_sources_saved_rotations_and_failures_continue() {
    let temp = temporary();
    let zip = temp.path().join("book.zip");
    let folder = temp.path().join("folder");
    let pdf = temp.path().join("book.pdf");
    let source = rotated_image(&jacket(), Rotation::Cw270);
    zip_book(&zip, &source);
    folder_book(&folder, &source);
    pdf_book(&pdf);
    let bad_zip = temp.path().join("broken.zip");
    std::fs::write(&bad_zip, b"not a zip").unwrap();
    let bad_decode = temp.path().join("bad-decode.zip");
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&bad_decode).unwrap());
    archive
        .start_file("000.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"not an image").unwrap();
    archive.finish().unwrap();
    let missing = temp.path().join("missing.pdf");
    let books = vec![
        zip.clone(),
        missing,
        pdf.clone(),
        bad_zip,
        folder.clone(),
        bad_decode,
        pdf.clone(),
    ];
    let rotations = temp.path().join("rotations.db");
    let db = crate::rotation_db::RotationDb::open_at(&rotations).unwrap();
    for (book, location) in [
        (
            &zip,
            ReadLocation::ArchiveEntry {
                archive: zip.clone(),
                entry: "000.png".into(),
            },
        ),
        (
            &pdf,
            ReadLocation::PdfPage {
                pdf: pdf.clone(),
                page: 0,
            },
        ),
        (&folder, ReadLocation::File(folder.join("000.png"))),
    ] {
        let key = crate::edit_source::caption_rotation_key(&location, Some(book)).unwrap();
        db.set_key(&key, Rotation::Cw90).unwrap();
    }
    drop(db);
    let rotation_bytes = std::fs::read(&rotations).unwrap();
    let input = temp.path().join("books.json");
    list(&input, &books);
    let output = temp.path().join("output");
    evaluate(
        &input,
        &output,
        false,
        FallbackConfig::default(),
        &Options {
            thumbs: true,
            rotation_db: Some(rotations.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let data = summary(&output);
    assert_eq!(data["total"], 7);
    assert_eq!(data["failed"], 3);
    assert_eq!(data["rows"].as_array().unwrap().len(), 7);
    // Repeated PDF input also stays on the in-process backend for the batch.
    for (index, label) in [(0, "zip"), (2, "pdf"), (4, "folder"), (6, "pdf")] {
        let row = &data["rows"][index];
        assert_eq!(row["sequence"], index + 1);
        assert_eq!(row["kind"], label);
        assert_eq!(row["path"], json!(books[index]));
        assert_eq!(row["rotation"], 90);
        assert!((row["page_aspect"].as_f64().unwrap() - 0.7).abs() < 1e-6);
        let dims: [u32; 2] = serde_json::from_value(row["decoded_dims"].clone()).unwrap();
        let expected = DynamicImage::new_rgb8(dims[1], dims[0]).thumbnail(640, 640);
        let raster = image::open(output.join(row["image_filename"].as_str().unwrap())).unwrap();
        assert_eq!(
            (raster.width(), raster.height()),
            (expected.width(), expected.height())
        );
        assert!(row["fallback_crop"].is_null());
        if label != "pdf" {
            assert!(!row["current_crop"].is_null(), "{row}");
            assert_frame(&output, row, "current_crop", false);
        }
    }
    for index in [1, 3, 5] {
        assert_eq!(data["rows"][index]["reason"], "error");
        assert!(data["rows"][index]["image_filename"].is_null());
    }
    assert!(
        data["rows"][5]["error"]
            .as_str()
            .unwrap()
            .starts_with("cover decode:")
    );
    assert_eq!(std::fs::read_dir(output.join("thumbs")).unwrap().count(), 4);
    assert_eq!(std::fs::read_dir(&output).unwrap().count(), 2);
    assert_eq!(std::fs::read(&rotations).unwrap(), rotation_bytes);
    // The reader never updates the existing rotation store.
    assert_eq!(
        crate::rotation_db::RotationDb::open_readonly_at(&rotations)
            .unwrap()
            .get_key(&crate::edit_source::page_key_for_pdf(&pdf, 0)),
        Some(Rotation::Cw90)
    );
}

#[test]
fn evaluation_book_list_production_layout_red_comparison_blue_and_thumbnail_opt_in() {
    let temp = temporary();
    let folder = temp.path().join("folder");
    folder_book(&folder, &DynamicImage::new_rgb8(150, 100));
    let input = temp.path().join("folders.json");
    list(&input, std::slice::from_ref(&folder));
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        let output = temp.path().join(format!("{binding:?}"));
        evaluate(
            &input,
            &output,
            true,
            FallbackConfig::parse(Some("layout"), false).unwrap(),
            &Options {
                binding,
                thumbs: true,
                ..Default::default()
            },
        )
        .unwrap();
        let data = summary(&output);
        let row = &data["rows"][0];
        assert_eq!(row["production_layout"], "T2");
        assert_eq!(row["production_method"], "layout-fallback");
        assert_eq!(data["fallback_cropped"], 0);
        assert!(row["fallback_crop"].is_null());
        assert_eq!(row["path"], json!(folder));
        assert_frame(&output, row, "current_crop", false);
        let x0 = row["current_crop"][0].as_f64().unwrap();
        if binding == ReadingDirection::Rtl {
            assert_eq!(x0, 0.0);
        } else {
            assert!(x0 > 0.5);
        }
    }
    // Outside the production layout range, an explicitly selected experiment
    // can still draw its additional crop in blue.
    let comparison = temp.path().join("comparison");
    folder_book(&comparison, &DynamicImage::new_rgb8(300, 100));
    let comparison_input = temp.path().join("comparison.json");
    list(&comparison_input, &[comparison]);
    let comparison_output = temp.path().join("comparison-output");
    evaluate(
        &comparison_input,
        &comparison_output,
        true,
        FallbackConfig::parse(Some("edge"), false).unwrap(),
        &Options {
            thumbs: true,
            ..Default::default()
        },
    )
    .unwrap();
    let comparison_data = summary(&comparison_output);
    let comparison_row = &comparison_data["rows"][0];
    assert!(comparison_row["current_crop"].is_null());
    assert_eq!(comparison_data["fallback_cropped"], 1);
    assert_eq!(comparison_row["fallback_type"], "edge");
    assert_frame(&comparison_output, comparison_row, "fallback_crop", true);

    for folders_only in [false, true] {
        let output = temp.path().join(format!("no-thumbs-{folders_only}"));
        evaluate(
            &input,
            &output,
            folders_only,
            FallbackConfig::default(),
            &Options::default(),
        )
        .unwrap();
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), 1);
        let data = summary(&output);
        assert!(data["rows"][0]["image_filename"].is_null());
        assert_eq!(data["rows"][0].get("path").is_none(), folders_only);
    }
}

#[test]
fn evaluation_book_list_binding_parser_keeps_default_and_rejects_invalid_values() {
    assert_eq!(parse_binding(None), Ok(ReadingDirection::Rtl));
    assert_eq!(parse_binding(Some("rtl")), Ok(ReadingDirection::Rtl));
    assert_eq!(parse_binding(Some("ltr")), Ok(ReadingDirection::Ltr));
    assert!(parse_binding(Some("other")).is_err());
}
