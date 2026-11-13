//! Opt-in evaluation: input books and all generated artifacts stay outside the
//! source tree when MIV_COVER_AUTO_EVAL_OUTPUT names an external directory.
//!
//! MIV_COVER_AUTO_EVAL_DECODE=worker selects the real thumbnail worker's source
//! decode (default: full). MIV_COVER_AUTO_EVAL_DISPLAY_PX overrides its display
//! target, 256..=2048; the default is the App startup geometry's 256 pixels.
//! thumb_px comes from Settings::default() (512). No saved user settings are read.
use super::*;
use image::Rgb;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Truth {
    front: Option<[f64; 2]>,
    front_y: Option<[f64; 2]>,
    #[serde(default)]
    ltr: bool,
    page_aspect: Option<f64>,
}

struct DecodeMode {
    worker: bool,
    display_px: u32,
    thumb_px: u32,
}

impl DecodeMode {
    fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let worker = match std::env::var("MIV_COVER_AUTO_EVAL_DECODE").as_deref() {
            Err(std::env::VarError::NotPresent) | Ok("full") => false,
            Ok("worker") => true,
            _ => return Err("MIV_COVER_AUTO_EVAL_DECODE must be full or worker".into()),
        };
        // App initialization: last_cell_size/h=200, pixels_per_point=1.
        // Runtime cell geometry/DPI can change display_px; it is not a setting.
        let display_px = std::env::var("MIV_COVER_AUTO_EVAL_DISPLAY_PX")
            .ok()
            .map(|value| value.parse::<u32>())
            .transpose()?
            .unwrap_or_else(|| crate::thumb_loader::compute_display_px(200.0, 200.0, 1.0));
        if !(256..=2048).contains(&display_px) {
            return Err("MIV_COVER_AUTO_EVAL_DISPLAY_PX must be in 256..=2048".into());
        }
        Ok(Self {
            worker,
            display_px,
            thumb_px: crate::settings::Settings::default().thumb_px,
        })
    }

    fn name(&self) -> &'static str {
        if self.worker { "worker" } else { "full" }
    }
}

struct LoadedCase {
    image: DynamicImage,
    aspect: Option<f64>,
    zip_leaf: Option<String>,
    worker: Option<crate::thumb_loader::cover_auto_eval_decode::DecodedCover>,
}

fn load_case(
    directory: &Path,
    name: &str,
    truth: &Truth,
    mode: &DecodeMode,
) -> Result<LoadedCase, Box<dyn std::error::Error>> {
    let zip_path = directory.join(format!("{name}.zip"));
    if zip_path.is_file() {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path)?)?;
        let mut names: Vec<_> = archive
            .file_names()
            .filter(|name| {
                Path::new(name)
                    .file_stem()
                    .is_some_and(|stem| stem == "000_cover")
            })
            .map(str::to_owned)
            .collect();
        names.sort();
        let leaf = names
            .into_iter()
            .next()
            .ok_or("ZIP has no 000_cover entry")?;
        let mut bytes = Vec::new();
        archive.by_name(&leaf)?.read_to_end(&mut bytes)?;
        let prefix = leaf
            .rsplit_once('/')
            .map(|(parent, _)| format!("{parent}/"));
        // Deliberately use the production prefix-only dimension sampler. Truth
        // aspect is allowed only for PNG dummies, never to rescue a ZIP sample.
        let aspect =
            crate::cover_auto_sample::sample_zip_page_aspect(&zip_path, &leaf, prefix.as_deref());
        let worker = mode
            .worker
            .then(|| {
                crate::thumb_loader::cover_auto_eval_decode::decode(
                    &zip_path,
                    Some(&leaf),
                    mode.display_px,
                    mode.thumb_px,
                )
            })
            .transpose()?;
        Ok(LoadedCase {
            image: image::load_from_memory(&bytes)?,
            aspect,
            zip_leaf: Some(leaf),
            worker,
        })
    } else {
        let path = directory.join(format!("{name}.png"));
        let worker = mode
            .worker
            .then(|| {
                crate::thumb_loader::cover_auto_eval_decode::decode(
                    &path,
                    None,
                    mode.display_px,
                    mode.thumb_px,
                )
            })
            .transpose()?;
        Ok(LoadedCase {
            image: image::open(path)?,
            aspect: truth.page_aspect,
            zip_leaf: None,
            worker,
        })
    }
}

fn box_iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let intersection =
        (a[2].min(b[2]) - a[0].max(b[0])).max(0.0) * (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = |r: [f64; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - intersection;
    if union > 0.0 {
        intersection / union
    } else {
        0.0
    }
}

fn outline(image: &mut RgbImage, bounds: [f64; 4], color: Rgb<u8>) {
    let [left, top, right, bottom] = bounds.map(|v| v.max(0.0).round() as u32);
    if image.width() == 0 || image.height() == 0 {
        return;
    }
    let (left, right) = (left.min(image.width() - 1), right.min(image.width() - 1));
    let (top, bottom) = (top.min(image.height() - 1), bottom.min(image.height() - 1));
    for thickness in 0..3 {
        for x in left..=right {
            image.put_pixel(x, (top + thickness).min(bottom), color);
            image.put_pixel(x, bottom.saturating_sub(thickness).max(top), color);
        }
        for y in top..=bottom {
            image.put_pixel((left + thickness).min(right), y, color);
            image.put_pixel(right.saturating_sub(thickness).max(left), y, color);
        }
    }
}

/// Run one directory per invocation, e.g. dummies and realistic separately.
/// The JSON row index maps to the contact sheet (left-to-right, top-to-bottom).
/// Green = truth, red = prediction; bottom status strip: green good, red wrong,
/// gray declined. Missing or unreadable inputs fail instead of inflating counts.
#[test]
#[ignore = "requires MIV_COVER_AUTO_EVAL_DIR; external evaluation images are never repository fixtures"]
fn cover_auto_crop_directory_evaluation() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(
        std::env::var_os("MIV_COVER_AUTO_EVAL_DIR").ok_or("set MIV_COVER_AUTO_EVAL_DIR")?,
    );
    let output = std::env::var_os("MIV_COVER_AUTO_EVAL_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/cover-auto-eval"));
    let mode = DecodeMode::from_env()?;
    let _worker_data = mode.worker.then(crate::data_dir::TestDataDirGuard::new);
    std::fs::create_dir_all(&output)?;
    let truth: BTreeMap<String, Truth> =
        serde_json::from_slice(&std::fs::read(directory.join("truth.json"))?)?;
    if truth.is_empty() {
        return Err("truth.json contains no cases".into());
    }
    let cell_width = 400;
    let cell_height = 260;
    let columns = 3;
    let mut sheet = RgbImage::from_pixel(
        columns * cell_width,
        (truth.len() as u32).div_ceil(columns) * cell_height,
        Rgb([235, 235, 235]),
    );
    let (mut good, mut wrong, mut declined) = (0, 0, 0);
    let (mut negative_cropped, mut negative_declined) = (0, 0);
    let mut reasons = BTreeMap::<String, u32>::new();
    let mut rows = Vec::new();
    for (index, (name, truth)) in truth.iter().enumerate() {
        let LoadedCase {
            mut image,
            aspect,
            zip_leaf,
            worker,
        } = load_case(&directory, name, truth, &mode)?;
        if let Some(decoded) = &worker {
            // Worker-mode truth coordinates are EXIF-upright, like CoverCrop.
            // This reference raster is used only for the comparison sheet.
            image = crate::thumb_loader::apply_orientation(image, decoded.orientation);
        }
        let dims = (image.width(), image.height());
        let decoded_image = worker.as_ref().map_or(&image, |decoded| &decoded.image);
        let native_dims = worker.as_ref().map_or(dims, |decoded| decoded.native_dims);
        if native_dims != dims {
            return Err(format!(
                "{name}: native/reference dimensions differ: {native_dims:?} vs {dims:?}"
            )
            .into());
        }
        let raster = analysis_raster(decoded_image);
        let binding = if truth.ltr {
            ReadingDirection::Ltr
        } else {
            ReadingDirection::Rtl
        };
        let detected = cover_auto_crop(&raster, native_dims, aspect, binding);
        let expected = truth.front.map(|x| {
            let y = truth.front_y.unwrap_or([0.0, dims.1 as f64]);
            [x[0], y[0], x[1], y[1]]
        });
        let predicted = detected.as_ref().ok().map(|crop| {
            let [x0, y0, x1, y1] = crop.values();
            [
                x0 as f64 * dims.0 as f64 / 1e6,
                y0 as f64 * dims.1 as f64 / 1e6,
                x1 as f64 * dims.0 as f64 / 1e6,
                y1 as f64 * dims.1 as f64 / 1e6,
            ]
        });
        let iou = predicted.zip(expected).map(|(a, b)| box_iou(a, b));
        let (status, color) = match detected {
            Ok(_) if iou.is_some_and(|iou| iou >= 0.85) => {
                good += 1;
                ("good", [20, 160, 60])
            }
            Ok(_) => {
                wrong += 1;
                if expected.is_none() {
                    negative_cropped += 1;
                }
                ("wrong", [210, 35, 35])
            }
            Err(reason) => {
                declined += 1;
                if expected.is_none() {
                    negative_declined += 1;
                }
                *reasons.entry(reason.as_str().to_owned()).or_default() += 1;
                ("declined", [115, 115, 115])
            }
        };
        let reason = detected
            .err()
            .map(AutoCropDecline::as_str)
            .unwrap_or("cropped");
        rows.push(json!({"index":index,"name":name,"status":status,"reason":reason,"iou":iou,"truth":expected,"prediction":predicted,"page_aspect":aspect,"zip_leaf":zip_leaf,"ltr":truth.ltr,
            "decoded_dims":[decoded_image.width(),decoded_image.height()],"native_dims":native_dims,
            "dct_scale_num":worker.as_ref().and_then(|decoded|decoded.dct_scale_num),"orientation":worker.as_ref().map(|decoded|decoded.orientation)}));
        let mut tile = image.thumbnail(cell_width - 12, cell_height - 14).to_rgb8();
        let project = |r: [f64; 4]| {
            [
                r[0] * tile.width() as f64 / dims.0 as f64,
                r[1] * tile.height() as f64 / dims.1 as f64,
                r[2] * tile.width() as f64 / dims.0 as f64,
                r[3] * tile.height() as f64 / dims.1 as f64,
            ]
        };
        let projected_truth = expected.map(project);
        let projected_prediction = predicted.map(project);
        if let Some(rect) = projected_truth {
            outline(&mut tile, rect, Rgb([0, 210, 0]));
        }
        if let Some(rect) = projected_prediction {
            outline(&mut tile, rect, Rgb([240, 0, 0]));
        }
        let x = index as u32 % columns * cell_width;
        let y = index as u32 / columns * cell_height;
        image::imageops::replace(
            &mut sheet,
            &tile,
            (x + (cell_width - tile.width()) / 2) as i64,
            (y + 3) as i64,
        );
        for yy in y + cell_height - 7..y + cell_height {
            for xx in x..x + cell_width {
                sheet.put_pixel(xx, yy, Rgb(color));
            }
        }
    }
    sheet.save(output.join("sheet.png"))?;
    let summary = json!({"algorithm":"autocrop:v1","decode_mode":mode.name(),"display_px":mode.display_px,"thumb_px":mode.thumb_px,
        "jpeg_decode_target":mode.display_px.max(mode.thumb_px),"iou_threshold":0.85,"good":good,"wrong":wrong,"declined":declined,
        "negative_cropped":negative_cropped,"negative_declined":negative_declined,"decline_reasons":reasons,"sheet_columns":columns,"rows":rows});
    std::fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    println!(
        "cover auto evaluation: good={good} wrong={wrong} declined={declined}; {}",
        output.display()
    );
    Ok(())
}
