//! Opt-in evaluation. Generated artifacts must stay under target/.
//! FOLDER_LIST accepts a JSON array of absolute image-folder paths and writes
//! numeric/count-only JSON unless THUMBS=1 explicitly opts into framed JPEGs.
//!
//! The default worker decode shares production's fixed analysis decode. Explicit
//! MIV_COVER_AUTO_EVAL_DECODE=full remains a diagnostic comparison. DISPLAY_PX
//! is accepted to verify analysis independence from display geometry.
//! IMAGE_ROTATION physically rotates a disposable input (0/90/180/270), while
//! SAVED_ROTATION overrides the corrective saved angle. Without that override,
//! saved rotation is truth.saved_rotation minus IMAGE_ROTATION (default 0).
//! REENCODE_INPUT=1 also reencodes the zero-degree ZIP cover, providing a
//! numerical control for JPEG recompression in the physical-rotation matrix.
//! FALLBACK=edge/edge18/density/density18/layout/layout-snap enables evaluation-only
//! crops after structural declines. FALLBACK_RICHER=1 also permits richer-elsewhere.
//! All angles are evaluated; directory counts use the selected saved angle.
use super::*;
use image::Rgb;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

mod book_list;
mod fallback;
mod thumbs;
use fallback::*;

#[derive(Deserialize)]
struct Truth {
    front: Option<[f64; 2]>,
    front_y: Option<[f64; 2]>,
    #[serde(default)]
    ltr: bool,
    page_aspect: Option<f64>,
    #[serde(default)]
    saved_rotation: i32,
}

struct DecodeMode {
    worker: bool,
    display_px: u32,
    thumb_px: u32,
    image_rotation: Rotation,
    saved_rotation: Option<Rotation>,
    reencode_input: bool,
}

impl DecodeMode {
    fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let worker = match std::env::var("MIV_COVER_AUTO_EVAL_DECODE").as_deref() {
            Ok("full") => false,
            Err(std::env::VarError::NotPresent) | Ok("worker") => true,
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
            image_rotation: env_rotation("MIV_COVER_AUTO_EVAL_IMAGE_ROTATION")?
                .unwrap_or(Rotation::None),
            saved_rotation: env_rotation("MIV_COVER_AUTO_EVAL_SAVED_ROTATION")?,
            reencode_input: std::env::var("MIV_COVER_AUTO_EVAL_REENCODE_INPUT").as_deref()
                == Ok("1"),
        })
    }

    fn name(&self) -> &'static str {
        if self.worker { "worker" } else { "full" }
    }
}

fn env_rotation(name: &str) -> Result<Option<Rotation>, Box<dyn std::error::Error>> {
    std::env::var(name)
        .ok()
        .map(|value| {
            let degrees: i32 = value.parse()?;
            if ![0, 90, 180, 270].contains(&degrees) {
                return Err(format!("{name} must be 0, 90, 180 or 270").into());
            }
            Ok(Rotation::from_degrees(degrees))
        })
        .transpose()
}

fn rotated_image(image: &DynamicImage, rotation: Rotation) -> DynamicImage {
    match rotation {
        Rotation::None => image.clone(),
        Rotation::Cw90 => image.rotate90(),
        Rotation::Cw180 => image.rotate180(),
        Rotation::Cw270 => image.rotate270(),
    }
}

fn rotated_rect(rect: [f64; 4], dims: (u32, u32), rotation: Rotation) -> [f64; 4] {
    let [x0, y0, x1, y1] = rect;
    let (w, h) = (f64::from(dims.0), f64::from(dims.1));
    match rotation {
        Rotation::None => rect,
        Rotation::Cw90 => [h - y1, x0, h - y0, x1],
        Rotation::Cw180 => [w - x1, h - y1, w - x0, h - y0],
        Rotation::Cw270 => [y0, w - x1, y1, w - x0],
    }
}

struct LoadedCase {
    image: DynamicImage,
    sample: Result<f64, crate::cover_auto_sample::PageAspectDecline>,
    shot_exif: bool,
    zip_leaf: Option<String>,
    worker: Option<crate::thumb_loader::cover_auto_eval_decode::DecodedCover>,
    original_dims: (u32, u32),
}

#[derive(Default)]
struct RatioDistribution(BTreeMap<i64, u32>);

impl RatioDistribution {
    fn record(&mut self, native: (u32, u32), aspect: Option<f64>, saved: Rotation) -> Option<f64> {
        let ratio = layout_ratio(native, aspect, saved)?;
        // Stabilize exact decimal bin boundaries against floating point division.
        let scaled = (ratio * 5.0 + 1e-9).floor();
        if scaled < i64::MAX as f64 {
            *self.0.entry(scaled as i64).or_default() += 1;
        }
        Some(ratio)
    }

    fn json(&self) -> serde_json::Value {
        json!({"bin_width": 0.2, "count": self.0.values().copied().sum::<u32>(),
            "bins": self.0.iter().map(|(&bin, &count)| json!({
                "lower": bin as f64 / 5.0, "upper": (bin + 1) as f64 / 5.0, "count": count,
            })).collect::<Vec<_>>()})
    }
}

fn load_case(
    directory: &Path,
    name: &str,
    truth: &Truth,
    mode: &DecodeMode,
    output: &Path,
) -> Result<LoadedCase, Box<dyn std::error::Error>> {
    if Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err("truth case name must be one normal filename component".into());
    }
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
        let reference = crate::thumb_loader::apply_exif_orientation_from_bytes(
            image::load_from_memory(&bytes)?,
            &bytes,
        );
        let original_dims = (reference.width(), reference.height());
        let image = rotated_image(&reference, mode.image_rotation);
        let mut decode_path = zip_path.clone();
        if !mode.image_rotation.is_none() || mode.reencode_input {
            // Replace only the cover; real following pages remain the source
            // for the bounded production page-aspect sampler.
            decode_path = target_output(output.join("inputs"))?.join(format!("{name}.zip"));
            let mut generated = zip::ZipWriter::new(std::fs::File::create(&decode_path)?);
            for index in 0..archive.len() {
                let entry = archive.by_index(index)?;
                if entry.name() == leaf {
                    let format = image::ImageFormat::from_path(&leaf)?;
                    let mut encoded = std::io::Cursor::new(Vec::new());
                    if format == image::ImageFormat::Jpeg {
                        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 95)
                            .encode_image(&image)?;
                    } else {
                        image.write_to(&mut encoded, format)?;
                    }
                    generated.start_file(&leaf, zip::write::SimpleFileOptions::default())?;
                    generated.write_all(encoded.get_ref())?;
                } else {
                    generated.raw_copy_file(entry)?;
                }
            }
            generated.finish()?;
        }
        let prefix = leaf
            .rsplit_once('/')
            .map(|(parent, _)| format!("{parent}/"));
        // Deliberately use the production prefix-only dimension sampler. Truth
        // aspect is allowed only for PNG dummies, never to rescue a ZIP sample.
        let sample =
            crate::cover_auto_sample::sample_zip_pages(&decode_path, &leaf, prefix.as_deref());
        let worker = mode
            .worker
            .then(|| {
                crate::thumb_loader::cover_auto_eval_decode::decode(
                    &decode_path,
                    Some(&leaf),
                    mode.display_px,
                    mode.thumb_px,
                )
            })
            .transpose()?;
        Ok(LoadedCase {
            image,
            sample,
            shot_exif: crate::thumb_loader::cover_auto_eval_decode::shot_exif(&bytes),
            zip_leaf: Some(leaf),
            worker,
            original_dims,
        })
    } else {
        let mut path = directory.join(format!("{name}.png"));
        let bytes = std::fs::read(&path)?;
        let reference = crate::thumb_loader::apply_exif_orientation_from_bytes(
            image::load_from_memory(&bytes)?,
            &bytes,
        );
        let original_dims = (reference.width(), reference.height());
        let image = rotated_image(&reference, mode.image_rotation);
        if !mode.image_rotation.is_none() {
            path = target_output(output.join("inputs"))?.join(format!("{name}.png"));
            image.save(&path)?;
        }
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
            image,
            sample: truth
                .page_aspect
                .ok_or(crate::cover_auto_sample::PageAspectDecline::TooFew),
            shot_exif: crate::thumb_loader::cover_auto_eval_decode::shot_exif(&bytes),
            zip_leaf: None,
            worker,
            original_dims,
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

fn target_output(output: PathBuf) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let target = std::env::current_dir()?.join("target");
    // canonical paths returned from this helper have a verbatim prefix on
    // Windows; accept them too when creating the nested disposable inputs.
    let resolved_target = target.canonicalize().ok();
    let requested = if output.is_absolute() {
        output
    } else {
        std::env::current_dir()?.join(output)
    };
    let relative = requested
        .strip_prefix(&target)
        .or_else(|error| {
            resolved_target
                .as_ref()
                .map_or(Err(error), |resolved| requested.strip_prefix(resolved))
        })
        .map_err(|_| "evaluation output must be under this workspace's target directory")?;
    if !relative
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err(
            "evaluation output must contain only normal path components under target".into(),
        );
    }
    std::fs::create_dir_all(&target)?;
    let target = target.canonicalize()?;
    let mut current = target.clone();
    // Verify existing ancestors before creating the next level, including
    // Windows junctions. Never materialize rotated inputs outside target.
    for part in relative.components() {
        current.push(part.as_os_str());
        if current.exists() {
            if !current.canonicalize()?.starts_with(&target) {
                return Err("evaluation output traverses a link outside target".into());
            }
        } else {
            std::fs::create_dir(&current)?;
        }
    }
    Ok(current)
}

/// Folder inputs have no ground truth and may contain private images. Use the
/// representative worker's source selection, bounded sample and analysis, but
/// emit neither source identities nor a contact sheet or copied input images.
fn evaluate_folder_list(
    list: &Path,
    output: &Path,
    fallback_config: FallbackConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    book_list::evaluate(
        list,
        output,
        true,
        fallback_config,
        &book_list::Options::default(),
    )
}

/// Run one directory per invocation, e.g. dummies and realistic separately.
/// The JSON row index maps to the contact sheet (left-to-right, top-to-bottom).
/// Green = truth, red = prediction; bottom status strip: green good, red wrong,
/// gray declined. Missing or unreadable inputs fail instead of inflating counts.
#[test]
#[ignore = "requires MIV_COVER_AUTO_EVAL_DIR, MIV_COVER_AUTO_EVAL_FOLDER_LIST or MIV_COVER_AUTO_EVAL_BOOK_LIST; external evaluation images are never repository fixtures"]
fn cover_auto_crop_directory_evaluation() -> Result<(), Box<dyn std::error::Error>> {
    let fallback_config = FallbackConfig::from_env()?;
    let output = target_output(
        std::env::var_os("MIV_COVER_AUTO_EVAL_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/cover-auto-eval")),
    )?;
    if let Some(list) = std::env::var_os("MIV_COVER_AUTO_EVAL_BOOK_LIST") {
        return book_list::evaluate(
            Path::new(&list),
            &output,
            false,
            fallback_config,
            &book_list::Options::from_env()?,
        );
    }
    if let Some(list) = std::env::var_os("MIV_COVER_AUTO_EVAL_FOLDER_LIST") {
        return book_list::evaluate(
            Path::new(&list),
            &output,
            true,
            fallback_config,
            &book_list::Options::from_env()?,
        );
    }
    let directory = PathBuf::from(
        std::env::var_os("MIV_COVER_AUTO_EVAL_DIR").ok_or("set MIV_COVER_AUTO_EVAL_DIR")?,
    );
    let mode = DecodeMode::from_env()?;
    evaluate_directory(&directory, &output, &mode, fallback_config)
}

// The ignored entry and synthetic tests share parsing, decode, sampling,
// angle selection, scoring, and artifact output without process env mutation.
fn evaluate_directory(
    directory: &Path,
    output: &Path,
    mode: &DecodeMode,
    fallback_config: FallbackConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = target_output(output.to_path_buf())?;
    let _worker_data = mode.worker.then(crate::data_dir::TestDataDirGuard::new);
    std::fs::create_dir_all(&output)?;
    let truth: BTreeMap<String, Truth> =
        serde_json::from_slice(&std::fs::read(directory.join("truth.json"))?)
            .map_err(|error| format!("truth.json parse failed: {error}"))?;
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
    let mut fallback_stats = FallbackStats::default();
    let mut ratio_distribution = RatioDistribution::default();
    for (index, (name, truth)) in truth.iter().enumerate() {
        let LoadedCase {
            image,
            sample,
            shot_exif,
            zip_leaf,
            worker,
            original_dims,
        } = load_case(&directory, name, truth, &mode, &output)?;
        let dims = (image.width(), image.height());
        let decoded_image = worker.as_ref().map_or(&image, |decoded| &decoded.image);
        let native_dims = worker.as_ref().map_or(dims, |decoded| decoded.native_dims);
        if native_dims != dims {
            return Err(format!(
                "{name}: native/reference dimensions differ: {native_dims:?} vs {dims:?}"
            )
            .into());
        }
        let binding = if truth.ltr {
            ReadingDirection::Ltr
        } else {
            ReadingDirection::Rtl
        };
        if ![0, 90, 180, 270].contains(&truth.saved_rotation) {
            return Err(format!("{name}: truth.saved_rotation must be 0, 90, 180 or 270").into());
        }
        let saved = mode.saved_rotation.unwrap_or_else(|| {
            Rotation::from_degrees(truth.saved_rotation - mode.image_rotation.degrees())
        });
        let aspect = sample.ok();
        let shot_exif = worker
            .as_ref()
            .map_or(shot_exif, |decoded| decoded.shot_exif);
        let outcomes = evaluate_angles(
            decoded_image,
            native_dims,
            aspect,
            shot_exif,
            binding,
            fallback_config,
        );
        let selected = outcomes[EVAL_ROTATIONS
            .iter()
            .position(|&rotation| rotation == saved)
            .unwrap()];
        let ratio = ratio_distribution.record(native_dims, aspect, saved);
        let detected = selected.final_result;
        let expected = truth.front.map(|x| {
            let y = truth.front_y.unwrap_or([0.0, original_dims.1 as f64]);
            rotated_rect([x[0], y[0], x[1], y[1]], original_dims, mode.image_rotation)
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
        fallback_stats.record(selected, iou, true);
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
                *reasons.entry(reason.as_str().into_owned()).or_default() += 1;
                ("declined", [115, 115, 115])
            }
        };
        let reason = detected
            .err()
            .map(AutoCropDecline::as_str)
            .unwrap_or_else(|| "cropped".into());
        rows.push(json!({"index":index,"name":name,"status":status,"reason":reason,"iou":iou,"truth":expected,"prediction":predicted,"page_aspect":aspect,"layout_ratio":ratio,"zip_leaf":zip_leaf,"ltr":truth.ltr,
            "page_aspect_decline":sample.err().map(crate::cover_auto_sample::PageAspectDecline::as_str),
            "decoded_dims":[decoded_image.width(),decoded_image.height()],"native_dims":native_dims,
            "saved_rotation":saved.degrees(),"image_rotation":mode.image_rotation.degrees(),
            "all_crops":outcomes.map(|outcome|outcome.final_result.ok().map(CoverCrop::values)),
            "original_decline":selected.original.err().map(|reason|reason.as_str().into_owned()),
            "fallback":selected.fallback.map(|diagnostic|diagnostic.json(fallback_config)),
            "all_fallbacks":outcomes.map(|outcome|outcome.fallback.map(|diagnostic|diagnostic.json(fallback_config))),
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
    let summary = json!({"algorithm":"autocrop:v2","decode_mode":mode.name(),"display_px":mode.display_px,"thumb_px":mode.thumb_px,
        "analysis_decode_height":1024,"image_rotation":mode.image_rotation.degrees(),
        "physical_jpeg_reencode_quality":(!mode.image_rotation.is_none() || mode.reencode_input).then_some(95),
        "reencode_input_control":mode.reencode_input,
        "saved_rotation_override":mode.saved_rotation.map(Rotation::degrees),
        "iou_threshold":0.85,"good":good,"wrong":wrong,"declined":declined,
        "fallback_mode":fallback_config.name(),"fallback_cropped":fallback_stats.cropped,
        "fallback_richer":fallback_config.includes_richer(),
        "fallback_good":fallback_stats.good,"fallback_wrong":fallback_stats.wrong,
        "fallback_original_declines":fallback_stats.reasons,
        "fallback_inward_fraction_distribution":fallback_stats.distribution(),
        "fallback_by_layout":fallback_stats.layout_summary(true),
        "fallback_layout_ratio_distribution":fallback_stats.layout_ratio_distribution(),
        "ratio_distribution":ratio_distribution.json(),
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

fn synthetic_shot_jpeg(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&DynamicImage::new_rgb8(width, height))?;
    // IFD0: Orientation=1 and ExifIFD pointer at byte 38. ExifIFD:
    // ExposureTime=1/125, stored as a rational at byte 56. This exercises G1
    // through actual JPEG metadata, independently of cover/body dimensions.
    let mut tiff = b"II*\0\x08\0\0\0".to_vec();
    let entry = |bytes: &mut Vec<u8>, tag: u16, format: u16, value: u32| {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&format.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    };
    tiff.extend_from_slice(&2_u16.to_le_bytes());
    entry(&mut tiff, 274, 3, 1);
    entry(&mut tiff, 34665, 4, 38);
    tiff.extend_from_slice(&0_u32.to_le_bytes());
    tiff.extend_from_slice(&1_u16.to_le_bytes());
    entry(&mut tiff, 33434, 5, 56);
    tiff.extend_from_slice(&0_u32.to_le_bytes());
    tiff.extend_from_slice(&1_u32.to_le_bytes());
    tiff.extend_from_slice(&125_u32.to_le_bytes());
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    bytes.extend_from_slice(b"Exif\0\0");
    bytes.extend_from_slice(&tiff);
    bytes.extend_from_slice(&jpeg[2..]);
    assert!(crate::thumb_loader::cover_auto_eval_decode::shot_exif(
        &bytes
    ));
    Ok(bytes)
}

#[test]
fn folder_evaluation_emits_counts_and_numeric_rows_without_images_or_names()
-> Result<(), Box<dyn std::error::Error>> {
    let target = std::env::current_dir()?.join("target");
    std::fs::create_dir_all(&target)?;
    let temp = tempfile::Builder::new()
        .prefix("cover-auto-folder-eval-")
        .tempdir_in(&target)?;
    let folder = temp.path().join("private-folder-name");
    std::fs::create_dir(&folder)?;
    std::fs::write(
        folder.join("000-private-cover-name.jpg"),
        synthetic_shot_jpeg(150, 100)?,
    )?;
    for index in 1..=8 {
        image::DynamicImage::new_rgb8(100, 150)
            .save(folder.join(format!("{index:03}-private-body-name.png")))?;
    }
    let list = temp.path().join("folders.json");
    std::fs::write(&list, serde_json::to_vec(&vec![folder])?)?;
    let output = temp.path().join("output");
    evaluate_folder_list(&list, &output, FallbackConfig::default())?;
    let artifacts: Vec<_> = std::fs::read_dir(&output)?
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(artifacts, vec![std::ffi::OsString::from("summary.json")]);
    let text = std::fs::read_to_string(output.join("summary.json"))?;
    assert!(!text.contains("private-"));
    assert!(!text.contains(&temp.path().to_string_lossy().into_owned()));
    let summary: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(summary["total"], 1);
    assert_eq!(summary["cropped"], 0);
    assert_eq!(summary["declined"], 1);
    assert_eq!(summary["decline_reasons"]["photo-like/shot-exif"], 1);
    assert_eq!(summary["rows"][0]["index"], 0);
    assert_eq!(summary["rows"][0]["native_dims"], json!([150, 100]));
    assert_eq!(summary["rows"][0]["reason"], "photo-like/shot-exif");
    assert!(summary["rows"][0]["crop"].is_null());
    Ok(())
}

#[test]
fn evaluation_truth_rotation_uses_source_edges_without_pixel_center_drift() {
    let bounds = [11.0, 23.0, 81.0, 169.0];
    for rotation in [
        Rotation::None,
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
    ] {
        let rotated = rotated_rect(bounds, (200, 300), rotation);
        let dimensions = match rotation {
            Rotation::Cw90 | Rotation::Cw270 => (300, 200),
            Rotation::None | Rotation::Cw180 => (200, 300),
        };
        assert_eq!(
            rotated_rect(
                rotated,
                dimensions,
                Rotation::from_degrees(-rotation.degrees())
            ),
            bounds
        );
    }
    assert_eq!(
        rotated_rect(bounds, (200, 300), Rotation::Cw90),
        [131.0, 11.0, 277.0, 81.0]
    );
}

#[test]
fn folder_evaluation_fallback_synthetic_counts_keep_guard_and_emit_no_images_or_identities()
-> Result<(), Box<dyn std::error::Error>> {
    let target = std::env::current_dir()?.join("target");
    std::fs::create_dir_all(&target)?;
    let temp = tempfile::Builder::new()
        .prefix("cover-auto-fallback-eval-")
        .tempdir_in(&target)?;
    let mut folders = Vec::new();
    // Both covers transpose the body dimensions. Only real shooting EXIF may
    // guard them now; the otherwise identical plain cover reaches fallback.
    for index in 0..2 {
        let width = 150;
        let folder = temp.path().join(format!("private-folder-{index}"));
        std::fs::create_dir(&folder)?;
        if index == 0 {
            std::fs::write(
                folder.join("000-private-cover.jpg"),
                synthetic_shot_jpeg(width, 100)?,
            )?;
        } else {
            DynamicImage::new_rgb8(width, 100).save(folder.join("000-private-cover.png"))?;
        }
        for page in 1..=8 {
            DynamicImage::new_rgb8(100, 150)
                .save(folder.join(format!("{page:03}-private-body.png")))?;
        }
        folders.push(folder);
    }
    let list = temp.path().join("folders.json");
    std::fs::write(&list, serde_json::to_vec(&folders)?)?;
    for mode in [
        None,
        Some("edge"),
        Some("edge18"),
        Some("density"),
        Some("density18"),
        Some("layout"),
        Some("layout-snap"),
    ] {
        // No process environment mutation: production defaults and each
        // experiment can run concurrently with unrelated test processes.
        let config = FallbackConfig::parse(mode, false)?;
        let output = temp.path().join(mode.unwrap_or("baseline"));
        evaluate_folder_list(&list, &output, config)?;
        let artifacts: Vec<_> = std::fs::read_dir(&output)?
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(artifacts, vec![std::ffi::OsString::from("summary.json")]);
        let text = std::fs::read_to_string(output.join("summary.json"))?;
        assert!(!text.contains("private-"));
        assert!(!text.contains(&temp.path().to_string_lossy().into_owned()));
        let summary: serde_json::Value = serde_json::from_str(&text)?;
        let count = u32::from(mode.is_some());
        assert_eq!(summary["total"], 2);
        assert_eq!(summary["cropped"], count);
        assert_eq!(summary["declined"], 2 - count);
        assert_eq!(summary["fallback_cropped"], count);
        assert!(summary["fallback_good"].is_null());
        assert!(summary["fallback_wrong"].is_null());
        assert_eq!(
            summary["rows"][0]["original_decline"],
            "photo-like/shot-exif"
        );
        assert!(summary["rows"][0]["fallback"].is_null());
        assert!(summary["rows"][0]["crop"].is_null());
        assert_eq!(summary["ratio_distribution"]["count"], 2);
        assert_eq!(
            summary["ratio_distribution"]["bins"],
            json!([{"lower":2.2,"upper":2.4,"count":2}])
        );
        assert_eq!(summary["rows"][1]["fallback"].is_object(), mode.is_some());
        assert_eq!(summary["rows"][1]["crop"].is_array(), mode.is_some());
        assert_eq!(
            summary["fallback_inward_fraction_distribution"]["count"],
            count
        );
        if mode.is_some() {
            let reason = summary["rows"][1]["original_decline"].as_str().unwrap();
            assert_eq!(summary["fallback_original_declines"][reason], 1);
            assert_eq!(summary["rows"][1]["all_crops"].as_array().unwrap().len(), 4);
            assert_eq!(
                summary["rows"][1]["all_fallbacks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|value| value.is_object())
                    .count(),
                2
            );
            if mode.is_some_and(|mode| mode.starts_with("layout")) {
                assert_eq!(summary["fallback_by_layout"]["T2"]["cropped"], 1);
                assert!(summary["fallback_by_layout"]["T2"]["good"].is_null());
                assert!(summary["fallback_by_layout"]["T2"]["wrong"].is_null());
                assert_eq!(summary["fallback_layout_ratio_distribution"]["count"], 1);
                assert_eq!(summary["rows"][1]["fallback"]["layout"]["kind"], "T2");
            }
        }
    }
    Ok(())
}

fn synthetic_decode_mode(worker: bool) -> DecodeMode {
    DecodeMode {
        worker,
        display_px: 256,
        thumb_px: crate::settings::Settings::default().thumb_px,
        image_rotation: Rotation::None,
        saved_rotation: None,
        reencode_input: false,
    }
}

fn synthetic_zip_pages(
    path: &Path,
    cover: (u32, u32),
    page: (u32, u32),
) -> Result<(), Box<dyn std::error::Error>> {
    let mut archive = zip::ZipWriter::new(std::fs::File::create(path)?);
    for index in 0..=8 {
        let dims = if index == 0 { cover } else { page };
        let mut png = std::io::Cursor::new(Vec::new());
        DynamicImage::new_rgb8(dims.0, dims.1).write_to(&mut png, image::ImageFormat::Png)?;
        archive.start_file(
            if index == 0 {
                "000_cover.png".to_owned()
            } else {
                format!("{index:03}.png")
            },
            zip::write::SimpleFileOptions::default(),
        )?;
        archive.write_all(png.get_ref())?;
    }
    archive.finish()?;
    Ok(())
}

#[test]
fn evaluation_null_page_aspect_samples_zip_and_declines_missing_png_without_early_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let target = std::env::current_dir()?.join("target");
    std::fs::create_dir_all(&target)?;
    let temp = tempfile::Builder::new()
        .prefix("cover-auto-null-eval-")
        .tempdir_in(target)?;
    synthetic_zip_pages(
        &temp.path().join("null-sampled-zip.zip"),
        (280, 200),
        (70, 100),
    )?;
    synthetic_zip_pages(
        &temp.path().join("null-square-zip.zip"),
        (200, 200),
        (100, 100),
    )?;
    DynamicImage::new_rgb8(200, 200).save(temp.path().join("null-square-png.png"))?;
    let truth = json!({
        "null-sampled-zip": {"front":[0.0,142.8],"page_aspect":null,"ltr":false},
        "null-square-zip": {"front":null,"page_aspect":null,"ltr":false},
        "null-square-png": {"front":null,"page_aspect":null,"ltr":false}
    });
    // Option<f64> has always accepted JSON null. Do not turn missing PNG
    // page evidence into a fabricated ratio, or rescue a bad ZIP sample via truth.
    let parsed: BTreeMap<String, Truth> = serde_json::from_value(truth.clone())?;
    assert!(parsed.values().all(|case| case.page_aspect.is_none()));
    std::fs::write(temp.path().join("truth.json"), serde_json::to_vec(&truth)?)?;
    {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let mode = synthetic_decode_mode(true);
        let loaded = load_case(
            temp.path(),
            "null-sampled-zip",
            &parsed["null-sampled-zip"],
            &mode,
            temp.path(),
        )?;
        assert_eq!(loaded.sample, Ok(0.7));
        let loaded = load_case(
            temp.path(),
            "null-square-zip",
            &parsed["null-square-zip"],
            &mode,
            temp.path(),
        )?;
        assert_eq!(
            loaded.sample,
            Err(crate::cover_auto_sample::PageAspectDecline::Landscape)
        );
        let loaded = load_case(
            temp.path(),
            "null-square-png",
            &parsed["null-square-png"],
            &mode,
            temp.path(),
        )?;
        assert_eq!(
            loaded.sample,
            Err(crate::cover_auto_sample::PageAspectDecline::TooFew)
        );
    }
    for worker in [false, true] {
        let output = temp.path().join(if worker { "worker" } else { "full" });
        evaluate_directory(
            temp.path(),
            &output,
            &synthetic_decode_mode(worker),
            FallbackConfig::parse(Some("layout"), false)?,
        )?;
        let summary: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("summary.json"))?)?;
        assert_eq!(summary["good"], 1);
        assert_eq!(summary["wrong"], 0);
        assert_eq!(summary["negative_cropped"], 0);
        assert_eq!(summary["negative_declined"], 2);
        assert_eq!(summary["fallback_by_layout"]["T2"]["good"], 1);
        for row in summary["rows"].as_array().unwrap() {
            match row["name"].as_str().unwrap() {
                "null-sampled-zip" => {
                    assert_eq!(row["page_aspect"], 0.7);
                    assert_eq!(row["status"], "good");
                }
                _ => {
                    assert!(row["page_aspect"].is_null());
                    assert_eq!(row["reason"], "no-page-aspect");
                    assert!(row["fallback"].is_null());
                }
            }
        }
    }
    Ok(())
}

#[test]
fn evaluation_layout_directory_summary_counts_each_type_and_keeps_positions_and_iou()
-> Result<(), Box<dyn std::error::Error>> {
    let target = std::env::current_dir()?.join("target");
    std::fs::create_dir_all(&target)?;
    let temp = tempfile::Builder::new()
        .prefix("cover-auto-layout-summary-")
        .tempdir_in(target)?;
    let mut truth = serde_json::Map::new();
    for (kind, width, left) in [
        ("T1", 224, 81.2),
        ("T2", 280, 0.0),
        ("T3", 392, 88.2),
        ("T4", 476, 88.2),
    ] {
        for ltr in [false, true] {
            let name = format!("{kind}-{}", if ltr { "ltr" } else { "rtl" });
            DynamicImage::new_rgb8(width, 200).save(temp.path().join(format!("{name}.png")))?;
            let front = if ltr {
                [f64::from(width) - left - 142.8, f64::from(width) - left]
            } else {
                [left, left + 142.8]
            };
            truth.insert(name, json!({"front":front,"ltr":ltr,"page_aspect":0.7}));
        }
    }
    std::fs::write(temp.path().join("truth.json"), serde_json::to_vec(&truth)?)?;
    for mode in ["layout", "layout-snap"] {
        let output = temp.path().join(mode);
        evaluate_directory(
            temp.path(),
            &output,
            &synthetic_decode_mode(true),
            FallbackConfig::parse(Some(mode), false)?,
        )?;
        let summary: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("summary.json"))?)?;
        assert_eq!(summary["good"], 8);
        assert_eq!(summary["wrong"], 0);
        assert_eq!(summary["fallback_cropped"], 8);
        assert_eq!(summary["ratio_distribution"]["count"], 8);
        for kind in ["T1", "T2", "T3", "T4"] {
            assert_eq!(
                summary["fallback_by_layout"][kind],
                json!({"cropped":2,"good":2,"wrong":0})
            );
        }
        for row in summary["rows"].as_array().unwrap() {
            let layout = &row["fallback"]["layout"];
            assert_eq!(
                layout["kind"].as_str().unwrap(),
                &row["name"].as_str().unwrap()[..2]
            );
            assert_eq!(layout["predicted"], layout["final_position"]);
            assert_eq!(layout["snap_applied"], false);
            assert!(row["iou"].as_f64().unwrap() > 0.999);
        }
    }
    Ok(())
}

#[test]
fn evaluation_layout_ratio_histogram_keeps_decimal_boundaries_and_saved_angle() {
    let mut histogram = RatioDistribution::default();
    for width in [160, 180, 200, 240] {
        histogram.record((width, 200), Some(0.5), Rotation::None);
    }
    histogram.record((200, 240), Some(0.5), Rotation::Cw90);
    assert_eq!(
        histogram.json(),
        json!({"bin_width":0.2,"count":5,"bins":[
            {"lower":1.6,"upper":1.8,"count":1},
            {"lower":1.8,"upper":2.0,"count":1},
            {"lower":2.0,"upper":2.2,"count":1},
            {"lower":2.4,"upper":2.6,"count":2}
        ]})
    );
    assert!(histogram.record((200, 200), None, Rotation::None).is_none());
    assert_eq!(histogram.json()["count"], 5);
}
