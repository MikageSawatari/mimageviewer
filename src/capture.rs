use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CaptureFormat {
    #[default]
    Png,
    Jpeg95,
    Jpeg85,
    Jpeg75,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegMatte {
    Black,
    White,
    Checker,
}

impl JpegMatte {
    pub fn from_fs_transparent_bg_mode(mode: u8) -> Self {
        match mode {
            1 => Self::White,
            2 => Self::Checker,
            _ => Self::Black,
        }
    }

    pub(crate) fn color_at(self, x: u32, y: u32) -> [u8; 3] {
        match self {
            Self::Black => [0, 0, 0],
            Self::White => [255, 255, 255],
            Self::Checker => {
                let cell = ((x / 8) + (y / 8)) % 2;
                let v = if cell == 0 { 224 } else { 176 };
                [v, v, v]
            }
        }
    }
}

impl CaptureFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg95 => "JPEG 95",
            Self::Jpeg85 => "JPEG 85",
            Self::Jpeg75 => "JPEG 75",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg95 | Self::Jpeg85 | Self::Jpeg75 => "jpg",
        }
    }

    pub(crate) fn jpeg_quality(self) -> Option<u8> {
        match self {
            Self::Png => None,
            Self::Jpeg95 => Some(95),
            Self::Jpeg85 => Some(85),
            Self::Jpeg75 => Some(75),
        }
    }
}

#[derive(Clone)]
pub struct CaptureConceal {
    pub mask: Arc<Vec<bool>>,
    pub preset: crate::conceal::ConcealPreset,
}

/// Pixel output job for save/copy/compare paths.
///
/// `source` must already be the final composite pixels for the page. The capture
/// worker only applies output-only operations such as conceal, crop, rotation,
/// and spread composition; it must not re-run AdjustParams / AI / final filters.
pub struct CapturePixelJob {
    pub basename: String,
    pub source: Arc<egui::ColorImage>,
    pub conceal: Option<CaptureConceal>,
    pub crop: Option<crate::export_crop::CropRect>,
    pub rotation: crate::rotation_db::Rotation,
}

pub enum CapturePixelWork {
    Single(CapturePixelJob),
    Spread {
        basename: String,
        left: CapturePixelJob,
        right: CapturePixelJob,
    },
}

impl CapturePixelJob {
    pub fn already_adjusted(basename: String, source: Arc<egui::ColorImage>) -> Self {
        Self {
            basename,
            source,
            conceal: None,
            crop: None,
            rotation: crate::rotation_db::Rotation::None,
        }
    }

    pub fn with_conceal(
        mut self,
        mask: Arc<Vec<bool>>,
        preset: crate::conceal::ConcealPreset,
    ) -> Self {
        self.conceal = Some(CaptureConceal { mask, preset });
        self
    }

    pub fn with_crop(mut self, crop: crate::export_crop::CropRect) -> Self {
        self.crop = Some(crop);
        self
    }

    pub fn with_rotation(mut self, rotation: crate::rotation_db::Rotation) -> Self {
        self.rotation = rotation;
        self
    }

    /// Worker が公開する出力寸法を、画素処理を始める前に同じ規則で求める。
    ///
    /// 比較準備ではこの寸法を request 側に保持し、worker の完了結果が元の
    /// source / crop / rotation / spread と整合する場合だけ Ready として公開する。
    pub(crate) fn output_size(&self) -> Result<[usize; 2], String> {
        let [width, height] = self.source.size;
        if width == 0 || height == 0 || self.source.pixels.len() != width.saturating_mul(height) {
            return Err("キャプチャ元画像のサイズが不正です".to_string());
        }
        let cropped = self.crop.map_or([width, height], |crop| {
            let (_, _, crop_width, crop_height) = crop.pixel_bounds(width, height);
            [crop_width, crop_height]
        });
        Ok(rotated_size(cropped, self.rotation))
    }
}

impl CapturePixelWork {
    /// `run_pixel_work` の出力キャンバス寸法。Spread は左右を横に連結した union。
    pub(crate) fn output_size(&self) -> Result<[usize; 2], String> {
        match self {
            Self::Single(job) => job.output_size(),
            Self::Spread { left, right, .. } => {
                let [left_width, left_height] = left.output_size()?;
                let [right_width, right_height] = right.output_size()?;
                let width = left_width
                    .checked_add(right_width)
                    .ok_or_else(|| "見開きキャプチャの幅が大きすぎます".to_string())?;
                Ok([width, left_height.max(right_height)])
            }
        }
    }
}

pub fn default_output_dir() -> PathBuf {
    if let Some(pictures) = pictures_dir()
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Pictures")))
    {
        pictures.join("mimageviewer")
    } else {
        crate::data_dir::get().join("captures")
    }
}

pub fn open_output_dir_async(output_dir: PathBuf) {
    std::thread::Builder::new()
        .name("capture-open-dir".into())
        .spawn(move || {
            if let Err(err) = std::fs::create_dir_all(&output_dir) {
                crate::logger::log(format!(
                    "capture output dir create failed: {}: {err}",
                    output_dir.display()
                ));
                return;
            }
            #[cfg(windows)]
            {
                if let Err(err) = std::process::Command::new("explorer.exe")
                    .arg(&output_dir)
                    .spawn()
                {
                    crate::logger::log(format!(
                        "capture output dir open failed: {}: {err}",
                        output_dir.display()
                    ));
                }
            }
            #[cfg(not(windows))]
            {
                let _ = output_dir;
            }
        })
        .ok();
}

pub fn reveal_path_async(path: PathBuf) {
    std::thread::Builder::new()
        .name("capture-reveal-path".into())
        .spawn(move || {
            #[cfg(windows)]
            {
                let arg = format!("/select,{}", path.display());
                if let Err(err) = std::process::Command::new("explorer.exe").arg(arg).spawn() {
                    crate::logger::log(format!("capture reveal failed: {}: {err}", path.display()));
                    if let Some(parent) = path.parent() {
                        let _ = std::process::Command::new("explorer.exe")
                            .arg(parent)
                            .spawn();
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let _ = path;
            }
        })
        .ok();
}

#[cfg(windows)]
fn pictures_dir() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Pictures, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

    unsafe {
        let pwstr = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok()?;
        let path = pwstr.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(pwstr.0 as *const core::ffi::c_void));
        path
    }
}

#[cfg(not(windows))]
fn pictures_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Pictures"))
}

pub fn basename_for_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(sanitize_basename)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "capture".to_string())
}

pub fn basename_from_text(input: &str) -> String {
    sanitize_basename(input)
}

fn sanitize_basename(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\u{1F}' => {
                out.push('_');
            }
            _ => out.push(ch),
        }
    }
    let trimmed = out.trim_matches([' ', '.']).to_string();
    if trimmed.is_empty() {
        "capture".to_string()
    } else {
        trimmed
    }
}

pub fn save_rgba_unique(
    output_dir: &Path,
    basename: &str,
    format: CaptureFormat,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<PathBuf, String> {
    save_rgba_unique_with_matte(
        output_dir,
        basename,
        format,
        JpegMatte::Black,
        width,
        height,
        rgba,
    )
}

pub fn save_rgba_unique_with_matte(
    output_dir: &Path,
    basename: &str,
    format: CaptureFormat,
    jpeg_matte: JpegMatte,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<PathBuf, String> {
    if width == 0 || height == 0 {
        return Err("capture size is zero".to_string());
    }
    let expected_len = width as usize * height as usize * 4;
    if rgba.len() != expected_len {
        return Err(format!(
            "invalid RGBA buffer length: got {}, expected {}",
            rgba.len(),
            expected_len
        ));
    }

    std::fs::create_dir_all(output_dir).map_err(|e| {
        format!(
            "保存先フォルダを作成できません: {}: {e}",
            output_dir.display()
        )
    })?;

    let basename = sanitize_basename(basename);
    let ext = format.extension();
    for seq in 1..=9999 {
        let path = output_dir.join(format!("{basename}_{seq:04}.{ext}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                let mut writer = BufWriter::new(file);
                encode_rgba(&mut writer, format, jpeg_matte, width, height, rgba)?;
                writer.flush().map_err(|e| {
                    format!("保存ファイルを flush できません: {}: {e}", path.display())
                })?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "保存ファイルを作成できません: {}: {e}",
                    path.display()
                ));
            }
        }
    }

    Err(format!(
        "保存ファイル名の連番が上限に達しました: {}",
        output_dir.display()
    ))
}

pub fn save_rgba_exact_with_matte(
    path: &Path,
    format: CaptureFormat,
    jpeg_matte: JpegMatte,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("capture size is zero".to_string());
    }
    let expected_len = width as usize * height as usize * 4;
    if rgba.len() != expected_len {
        return Err(format!(
            "invalid RGBA buffer length: got {}, expected {}",
            rgba.len(),
            expected_len
        ));
    }
    let Some(parent) = path.parent() else {
        return Err(format!("保存先フォルダが不正です: {}", path.display()));
    };
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("保存先フォルダを作成できません: {}: {e}", parent.display()))?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("保存ファイルを作成できません: {}: {e}", path.display()))?;
    let mut writer = BufWriter::new(file);
    encode_rgba(&mut writer, format, jpeg_matte, width, height, rgba)?;
    writer
        .flush()
        .map_err(|e| format!("保存ファイルを flush できません: {}: {e}", path.display()))
}

pub fn run_pixel_job(job: CapturePixelJob) -> Result<(String, u32, u32, Vec<u8>), String> {
    let mut image = job.source.as_ref().clone();
    if let Some(conceal) = job.conceal {
        let expected = image.size[0]
            .checked_mul(image.size[1])
            .ok_or_else(|| "隠蔽加工マスクのサイズが大きすぎます".to_string())?;
        if conceal.mask.len() != expected {
            return Err(format!(
                "隠蔽加工マスクのサイズが一致しません: mask={}, expected={}",
                conceal.mask.len(),
                expected
            ));
        }
        image = crate::conceal_compose::compose_with_preset(
            &image,
            conceal.mask.as_ref(),
            &conceal.preset,
        );
    }
    if let Some(crop) = job.crop {
        image = crate::export_crop::crop_color_image(&image, crop)?;
    }
    if !job.rotation.is_none() {
        image = rotate_color_image(&image, job.rotation);
    }
    let width = image.size[0] as u32;
    let height = image.size[1] as u32;
    Ok((job.basename, width, height, color_image_to_rgba(&image)))
}

pub fn rotated_size(size: [usize; 2], rotation: crate::rotation_db::Rotation) -> [usize; 2] {
    match rotation {
        crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
            [size[1], size[0]]
        }
        crate::rotation_db::Rotation::None | crate::rotation_db::Rotation::Cw180 => size,
    }
}

pub fn rotate_dynamic_image(
    image: image::DynamicImage,
    rotation: crate::rotation_db::Rotation,
) -> image::DynamicImage {
    match rotation {
        crate::rotation_db::Rotation::None => image,
        crate::rotation_db::Rotation::Cw90 => image.rotate90(),
        crate::rotation_db::Rotation::Cw180 => image.rotate180(),
        crate::rotation_db::Rotation::Cw270 => image.rotate270(),
    }
}

pub fn rotate_color_image(
    image: &egui::ColorImage,
    rotation: crate::rotation_db::Rotation,
) -> egui::ColorImage {
    let [w, h] = image.size;
    if rotation.is_none() || w == 0 || h == 0 {
        return image.clone();
    }
    if image.pixels.len() != w.saturating_mul(h) {
        return image.clone();
    }

    let out_size = rotated_size(image.size, rotation);
    let mut out = vec![egui::Color32::TRANSPARENT; image.pixels.len()];
    let out_w = out_size[0];
    for y in 0..h {
        for x in 0..w {
            let src = y * w + x;
            let (dx, dy) = match rotation {
                crate::rotation_db::Rotation::None => (x, y),
                crate::rotation_db::Rotation::Cw90 => (h - 1 - y, x),
                crate::rotation_db::Rotation::Cw180 => (w - 1 - x, h - 1 - y),
                crate::rotation_db::Rotation::Cw270 => (y, w - 1 - x),
            };
            out[dy * out_w + dx] = image.pixels[src];
        }
    }
    egui::ColorImage::new(out_size, out)
}

pub fn run_pixel_work(work: CapturePixelWork) -> Result<(String, u32, u32, Vec<u8>), String> {
    match work {
        CapturePixelWork::Single(job) => run_pixel_job(job),
        CapturePixelWork::Spread {
            basename,
            left,
            right,
        } => {
            let (_, left_w, left_h, left_rgba) = run_pixel_job(left)?;
            let (_, right_w, right_h, right_rgba) = run_pixel_job(right)?;
            combine_spread_rgba(
                basename, left_w, left_h, left_rgba, right_w, right_h, right_rgba,
            )
        }
    }
}

fn combine_spread_rgba(
    basename: String,
    left_w: u32,
    left_h: u32,
    left_rgba: Vec<u8>,
    right_w: u32,
    right_h: u32,
    right_rgba: Vec<u8>,
) -> Result<(String, u32, u32, Vec<u8>), String> {
    if left_w == 0 || left_h == 0 || right_w == 0 || right_h == 0 {
        return Err("見開きキャプチャのサイズが 0 です".to_string());
    }
    if left_rgba.len() != left_w as usize * left_h as usize * 4
        || right_rgba.len() != right_w as usize * right_h as usize * 4
    {
        return Err("見開きキャプチャの RGBA サイズが不正です".to_string());
    }

    let width = left_w
        .checked_add(right_w)
        .ok_or_else(|| "見開きキャプチャの幅が大きすぎます".to_string())?;
    let height = left_h.max(right_h);
    let len = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "見開きキャプチャの画像が大きすぎます".to_string())? as usize;
    let mut out = vec![0_u8; len];
    blit_centered(&mut out, width, height, 0, left_w, left_h, &left_rgba);
    blit_centered(
        &mut out,
        width,
        height,
        left_w,
        right_w,
        right_h,
        &right_rgba,
    );
    Ok((basename, width, height, out))
}

pub fn combine_spread_color_images(
    left: &egui::ColorImage,
    right: &egui::ColorImage,
) -> Result<egui::ColorImage, String> {
    let [left_w, left_h] = left.size;
    let [right_w, right_h] = right.size;
    if left_w == 0 || left_h == 0 || right_w == 0 || right_h == 0 {
        return Err("見開きキャプチャのサイズが 0 です".to_string());
    }
    if left.pixels.len() != left_w * left_h || right.pixels.len() != right_w * right_h {
        return Err("見開きキャプチャの ColorImage サイズが不正です".to_string());
    }

    let width = left_w
        .checked_add(right_w)
        .ok_or_else(|| "見開きキャプチャの幅が大きすぎます".to_string())?;
    let height = left_h.max(right_h);
    let len = width
        .checked_mul(height)
        .ok_or_else(|| "見開きキャプチャの画像が大きすぎます".to_string())?;
    let mut out = vec![egui::Color32::TRANSPARENT; len];
    blit_centered_color(&mut out, width, height, 0, left_w, left_h, &left.pixels);
    blit_centered_color(
        &mut out,
        width,
        height,
        left_w,
        right_w,
        right_h,
        &right.pixels,
    );
    Ok(egui::ColorImage::new([width, height], out))
}

fn blit_centered_color(
    dst: &mut [egui::Color32],
    dst_w: usize,
    dst_h: usize,
    dst_x: usize,
    src_w: usize,
    src_h: usize,
    src: &[egui::Color32],
) {
    let dst_y = (dst_h - src_h) / 2;
    for y in 0..src_h {
        let src_start = y * src_w;
        let dst_start = (dst_y + y) * dst_w + dst_x;
        dst[dst_start..dst_start + src_w].copy_from_slice(&src[src_start..src_start + src_w]);
    }
}

fn blit_centered(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    dst_x: u32,
    src_w: u32,
    src_h: u32,
    src: &[u8],
) {
    let dst_y = (dst_h - src_h) / 2;
    let dst_stride = dst_w as usize * 4;
    let src_stride = src_w as usize * 4;
    for y in 0..src_h as usize {
        let src_start = y * src_stride;
        let dst_start = (dst_y as usize + y) * dst_stride + dst_x as usize * 4;
        dst[dst_start..dst_start + src_stride]
            .copy_from_slice(&src[src_start..src_start + src_stride]);
    }
}

pub fn color_image_to_rgba(image: &egui::ColorImage) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    rgba
}

pub fn align_rgba_to_canvas_lanczos(
    src_w: u32,
    src_h: u32,
    src_rgba: &[u8],
    canvas_w: u32,
    canvas_h: u32,
) -> Result<Vec<u8>, String> {
    if src_w == 0 || src_h == 0 || canvas_w == 0 || canvas_h == 0 {
        return Err("比較画像のサイズが 0 です".to_string());
    }
    if src_rgba.len() != src_w as usize * src_h as usize * 4 {
        return Err("比較画像の RGBA サイズが不正です".to_string());
    }
    if src_w == canvas_w && src_h == canvas_h {
        return Ok(src_rgba.to_vec());
    }

    let src = image::RgbaImage::from_raw(src_w, src_h, src_rgba.to_vec())
        .ok_or_else(|| "比較画像の RGBA バッファを作成できません".to_string())?;
    let scale = (canvas_w as f64 / src_w as f64).min(canvas_h as f64 / src_h as f64);
    let resized_w = ((src_w as f64 * scale).round() as u32).clamp(1, canvas_w);
    let resized_h = ((src_h as f64 * scale).round() as u32).clamp(1, canvas_h);
    let resized = crate::fast_resize::resize_rgba8_exact(
        &src,
        resized_w,
        resized_h,
        crate::fast_resize::Quality::Lanczos3,
    );

    let len = canvas_w
        .checked_mul(canvas_h)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "比較画像のキャンバスが大きすぎます".to_string())? as usize;
    let mut out = vec![0_u8; len];
    let offset_x = (canvas_w - resized_w) / 2;
    blit_centered_exact_y(
        &mut out,
        canvas_w,
        offset_x,
        (canvas_h - resized_h) / 2,
        resized_w,
        resized_h,
        resized.as_raw(),
    );
    Ok(out)
}

fn blit_centered_exact_y(
    dst: &mut [u8],
    dst_w: u32,
    dst_x: u32,
    dst_y: u32,
    src_w: u32,
    src_h: u32,
    src: &[u8],
) {
    let dst_stride = dst_w as usize * 4;
    let src_stride = src_w as usize * 4;
    for y in 0..src_h as usize {
        let src_start = y * src_stride;
        let dst_start = (dst_y as usize + y) * dst_stride + dst_x as usize * 4;
        dst[dst_start..dst_start + src_stride]
            .copy_from_slice(&src[src_start..src_start + src_stride]);
    }
}

pub fn diff_rgba_color(
    width: u32,
    height: u32,
    pinned_rgba: &[u8],
    current_rgba: &[u8],
) -> Result<Vec<u8>, String> {
    let expected = width as usize * height as usize * 4;
    if pinned_rgba.len() != expected || current_rgba.len() != expected {
        return Err("差分比較の RGBA サイズが不正です".to_string());
    }
    let mut out = Vec::with_capacity(expected);
    for (a, b) in pinned_rgba
        .chunks_exact(4)
        .zip(current_rgba.chunks_exact(4))
    {
        let dr = ((a[0] as f32 - b[0] as f32).abs() / 255.0).sqrt();
        let dg = ((a[1] as f32 - b[1] as f32).abs() / 255.0).sqrt();
        let db = ((a[2] as f32 - b[2] as f32).abs() / 255.0).sqrt();
        out.extend_from_slice(&[
            (dr * 255.0).round() as u8,
            (dg * 255.0).round() as u8,
            (db * 255.0).round() as u8,
            255,
        ]);
    }
    Ok(out)
}

fn encode_rgba<W: Write>(
    writer: &mut W,
    format: CaptureFormat,
    jpeg_matte: JpegMatte,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), String> {
    use image::ImageEncoder;

    if let Some(quality) = format.jpeg_quality() {
        let rgb = flatten_rgba_to_rgb(rgba, width, jpeg_matte);
        let image = image::RgbImage::from_raw(width, height, rgb)
            .ok_or_else(|| "RGB バッファの作成に失敗しました".to_string())?;
        let jpeg = turbojpeg::compress_image(&image, quality as i32, turbojpeg::Subsamp::Sub2x2)
            .map_err(|e| format!("JPEG エンコードに失敗しました: {e}"))?;
        writer
            .write_all(jpeg.as_ref())
            .map_err(|e| format!("JPEG 書き込みに失敗しました: {e}"))
    } else {
        image::codecs::png::PngEncoder::new(writer)
            .write_image(rgba, width, height, image::ColorType::Rgba8.into())
            .map_err(|e| format!("PNG エンコードに失敗しました: {e}"))
    }
}

fn flatten_rgba_to_rgb(rgba: &[u8], width: u32, matte: JpegMatte) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        let alpha = px[3] as u16;
        if alpha == 255 {
            rgb.extend_from_slice(&px[..3]);
            continue;
        }

        let x = (i as u32) % width;
        let y = (i as u32) / width;
        let bg = matte.color_at(x, y);
        for channel in 0..3 {
            let fg = px[channel] as u16;
            let bg = bg[channel] as u16;
            let blended = (fg * alpha + bg * (255 - alpha) + 127) / 255;
            rgb.push(blended as u8);
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rgba() -> Vec<u8> {
        vec![
            255, 0, 0, 255, //
            0, 255, 0, 255, //
            0, 0, 255, 255, //
            255, 255, 255, 255,
        ]
    }

    #[test]
    fn save_rgba_unique_uses_next_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_rgba_unique(
            dir.path(),
            "movie",
            CaptureFormat::Png,
            2,
            2,
            &sample_rgba(),
        )
        .unwrap();
        let second = save_rgba_unique(
            dir.path(),
            "movie",
            CaptureFormat::Png,
            2,
            2,
            &sample_rgba(),
        )
        .unwrap();

        assert_eq!(first.file_name().unwrap(), "movie_0001.png");
        assert_eq!(second.file_name().unwrap(), "movie_0002.png");
    }

    #[test]
    fn save_rgba_unique_can_encode_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_rgba_unique(
            dir.path(),
            "movie",
            CaptureFormat::Jpeg85,
            2,
            2,
            &sample_rgba(),
        )
        .unwrap();

        assert_eq!(path.file_name().unwrap(), "movie_0001.jpg");
        assert!(std::fs::metadata(path).unwrap().len() > 0);
    }

    #[test]
    fn run_pixel_job_uses_final_composite_pixels_as_input() {
        let src = egui::ColorImage::new(
            [2, 1],
            vec![
                egui::Color32::from_rgb(10, 20, 30),
                egui::Color32::from_rgb(200, 210, 220),
            ],
        );
        let expected = crate::capture::color_image_to_rgba(&src);
        let job = CapturePixelJob::already_adjusted("sample".to_string(), Arc::new(src));

        let (basename, width, height, rgba) = run_pixel_job(job).unwrap();

        assert_eq!(basename, "sample");
        assert_eq!((width, height), (2, 1));
        assert_eq!(rgba, expected);
    }

    #[test]
    fn run_pixel_job_applies_conceal_on_worker_path() {
        let src = egui::ColorImage::new(
            [2, 1],
            vec![egui::Color32::BLACK, egui::Color32::from_rgb(10, 20, 30)],
        );
        let mut preset = crate::conceal::ConcealPreset::default();
        preset.conceal_type = crate::conceal::ConcealType::WhiteFill;
        preset.fill_opacity_percent = 100;
        preset.fill_edge = crate::conceal::FillEdge::Sharp;
        let job = CapturePixelJob::already_adjusted("sample".to_string(), Arc::new(src))
            .with_conceal(Arc::new(vec![true, false]), preset);

        let (_basename, width, height, rgba) = run_pixel_job(job).unwrap();

        assert_eq!((width, height), (2, 1));
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[4..8], &[10, 20, 30, 255]);
    }

    #[test]
    fn run_pixel_job_applies_crop_after_conceal() {
        let src = egui::ColorImage::new(
            [3, 1],
            vec![
                egui::Color32::RED,
                egui::Color32::GREEN,
                egui::Color32::BLUE,
            ],
        );
        let job = CapturePixelJob::already_adjusted("sample".to_string(), Arc::new(src)).with_crop(
            crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 0.0,
                max_x: 3.0,
                max_y: 1.0,
            },
        );

        let (_basename, width, height, rgba) = run_pixel_job(job).unwrap();

        assert_eq!((width, height), (2, 1));
        assert_eq!(&rgba[0..4], &[0, 255, 0, 255]);
        assert_eq!(&rgba[4..8], &[0, 0, 255, 255]);
    }

    #[test]
    fn rotate_color_image_matches_display_rotation() {
        use crate::rotation_db::Rotation;

        let px = |v| egui::Color32::from_rgb(v, 0, 0);
        let src = egui::ColorImage::new([2, 3], vec![px(1), px(2), px(3), px(4), px(5), px(6)]);

        let cw90 = rotate_color_image(&src, Rotation::Cw90);
        assert_eq!(cw90.size, [3, 2]);
        assert_eq!(cw90.pixels, vec![px(5), px(3), px(1), px(6), px(4), px(2)]);

        let cw180 = rotate_color_image(&src, Rotation::Cw180);
        assert_eq!(cw180.size, [2, 3]);
        assert_eq!(cw180.pixels, vec![px(6), px(5), px(4), px(3), px(2), px(1)]);

        let cw270 = rotate_color_image(&src, Rotation::Cw270);
        assert_eq!(cw270.size, [3, 2]);
        assert_eq!(cw270.pixels, vec![px(2), px(4), px(6), px(1), px(3), px(5)]);
    }

    #[test]
    fn run_pixel_job_applies_rotation_after_crop() {
        use crate::rotation_db::Rotation;

        let src = egui::ColorImage::new(
            [3, 2],
            vec![
                egui::Color32::from_rgb(1, 0, 0),
                egui::Color32::from_rgb(2, 0, 0),
                egui::Color32::from_rgb(3, 0, 0),
                egui::Color32::from_rgb(4, 0, 0),
                egui::Color32::from_rgb(5, 0, 0),
                egui::Color32::from_rgb(6, 0, 0),
            ],
        );
        let job = CapturePixelJob::already_adjusted("sample".to_string(), Arc::new(src))
            .with_crop(crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 0.0,
                max_x: 3.0,
                max_y: 2.0,
            })
            .with_rotation(Rotation::Cw90);

        let (_basename, width, height, rgba) = run_pixel_job(job).unwrap();

        assert_eq!((width, height), (2, 2));
        assert_eq!(&rgba[0..4], &[5, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[2, 0, 0, 255]);
        assert_eq!(&rgba[8..12], &[6, 0, 0, 255]);
        assert_eq!(&rgba[12..16], &[3, 0, 0, 255]);
    }

    #[test]
    fn run_pixel_work_applies_conceal_per_spread_page() {
        let left = egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]);
        let right = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(10, 20, 30)]);
        let mut preset = crate::conceal::ConcealPreset::default();
        preset.conceal_type = crate::conceal::ConcealType::WhiteFill;
        preset.fill_opacity_percent = 100;
        preset.fill_edge = crate::conceal::FillEdge::Sharp;
        let work = CapturePixelWork::Spread {
            basename: "spread".to_string(),
            left: CapturePixelJob::already_adjusted("left".to_string(), Arc::new(left))
                .with_conceal(Arc::new(vec![true]), preset),
            right: CapturePixelJob::already_adjusted("right".to_string(), Arc::new(right)),
        };

        let (_basename, width, height, rgba) = run_pixel_work(work).unwrap();

        assert_eq!((width, height), (2, 1));
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[4..8], &[10, 20, 30, 255]);
    }

    #[test]
    fn run_pixel_work_combines_spread_centered() {
        let left = egui::ColorImage::new([1, 2], vec![egui::Color32::RED; 2]);
        let right = egui::ColorImage::new([2, 1], vec![egui::Color32::GREEN; 2]);
        let work = CapturePixelWork::Spread {
            basename: "spread".to_string(),
            left: CapturePixelJob::already_adjusted("left".to_string(), Arc::new(left)),
            right: CapturePixelJob::already_adjusted("right".to_string(), Arc::new(right)),
        };

        let (basename, width, height, rgba) = run_pixel_work(work).unwrap();

        assert_eq!(basename, "spread");
        assert_eq!((width, height), (3, 2));
        assert_eq!(rgba.len(), 3 * 2 * 4);
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[0, 255, 0, 255]);
        assert_eq!(&rgba[8..12], &[0, 255, 0, 255]);
        assert_eq!(&rgba[12..16], &[255, 0, 0, 255]);
        assert_eq!(&rgba[16..20], &[0, 0, 0, 0]);
        assert_eq!(&rgba[20..24], &[0, 0, 0, 0]);
    }

    #[test]
    fn pixel_work_output_size_matches_cropped_rotated_spread_union() {
        use crate::rotation_db::Rotation;

        let left = egui::ColorImage::filled([6, 4], egui::Color32::RED);
        let right = egui::ColorImage::filled([5, 7], egui::Color32::GREEN);
        let work = CapturePixelWork::Spread {
            basename: "spread".to_string(),
            left: CapturePixelJob::already_adjusted("left".to_string(), Arc::new(left))
                .with_crop(crate::export_crop::CropRect {
                    min_x: 1.0,
                    min_y: 0.0,
                    max_x: 4.0,
                    max_y: 4.0,
                })
                .with_rotation(Rotation::Cw90),
            right: CapturePixelJob::already_adjusted("right".to_string(), Arc::new(right))
                .with_crop(crate::export_crop::CropRect {
                    min_x: 0.0,
                    min_y: 2.0,
                    max_x: 5.0,
                    max_y: 4.0,
                }),
        };

        assert_eq!(work.output_size().unwrap(), [9, 3]);
        let (_basename, width, height, _rgba) = run_pixel_work(work).unwrap();
        assert_eq!([width as usize, height as usize], [9, 3]);
    }

    #[test]
    fn align_rgba_to_canvas_lanczos_centers_with_transparent_padding() {
        let src = vec![255, 0, 0, 255, 255, 0, 0, 255];
        let out = align_rgba_to_canvas_lanczos(1, 2, &src, 4, 4).unwrap();

        assert_eq!(out.len(), 4 * 4 * 4);
        assert_eq!(&out[0..4], &[0, 0, 0, 0]);
        assert_eq!(&out[4..8], &[255, 0, 0, 255]);
        assert_eq!(&out[8..12], &[255, 0, 0, 255]);
        assert_eq!(&out[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn diff_rgba_color_highlights_changed_channels() {
        let a = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let b = vec![0, 0, 0, 255, 0, 255, 96, 255];
        let out = diff_rgba_color(2, 1, &a, &b).unwrap();

        assert_eq!(&out[0..4], &[0, 0, 0, 255]);
        assert!(out[4] > 0);
        assert_eq!(out[5], 0);
        assert!(out[6] > 0);
        assert_ne!(out[4], out[6]);
        assert_eq!(out[7], 255);
    }

    #[test]
    fn flatten_rgba_to_rgb_uses_selected_jpeg_matte() {
        let rgba = vec![255, 0, 0, 128, 0, 255, 0, 0];

        let white = flatten_rgba_to_rgb(&rgba, 2, JpegMatte::White);
        let black = flatten_rgba_to_rgb(&rgba, 2, JpegMatte::Black);

        assert_eq!(white, vec![255, 127, 127, 255, 255, 255]);
        assert_eq!(black, vec![128, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn basename_for_path_sanitizes_windows_reserved_chars() {
        let base = basename_for_path(Path::new(r#"C:\tmp\a:b?c.jpg"#));
        assert_eq!(base, "a_b_c");
    }
}
