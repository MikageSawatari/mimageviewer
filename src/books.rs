use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use eframe::egui;

pub const DEFAULT_BOOK_NAME: &str = "名前なし";
pub const MAX_BOOK_PAGES: usize = 9999;

#[derive(Clone, Debug)]
pub struct BookInfo {
    pub name: String,
    pub path: PathBuf,
    pub page_count: usize,
}

#[derive(Clone, Debug)]
pub struct BookPageEntry {
    pub path: PathBuf,
    pub display_name: String,
}

#[derive(Debug)]
pub struct BookAppendSummary {
    pub book_name: String,
    pub folder: PathBuf,
    pub added: usize,
    pub first_path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum BookOpResult {
    Append(BookAppendSummary),
    List(Vec<BookInfo>),
    Created { name: String, path: PathBuf },
    Renamed { old_name: String, new_name: String },
    Deleted { name: String },
    Reordered { folder: PathBuf, count: usize },
}

pub struct BookOpPending {
    pub rx: std::sync::mpsc::Receiver<Result<BookOpResult, String>>,
}

pub enum BookPageSource {
    File {
        src: PathBuf,
        original_name: String,
    },
    AdjustedFile {
        src: PathBuf,
        original_name: String,
        params: crate::adjustment::AdjustParams,
        rotation: crate::rotation_db::Rotation,
        format: crate::capture::CaptureFormat,
        jpeg_matte: crate::capture::JpegMatte,
    },
    ZipEntry {
        zip_path: PathBuf,
        entry_name: String,
        original_name: String,
    },
    AdjustedZipEntry {
        zip_path: PathBuf,
        entry_name: String,
        original_name: String,
        params: crate::adjustment::AdjustParams,
        rotation: crate::rotation_db::Rotation,
        format: crate::capture::CaptureFormat,
        jpeg_matte: crate::capture::JpegMatte,
    },
    AdjustedPdfPage {
        pdf_path: PathBuf,
        page_num: u32,
        password: Option<String>,
        basename: String,
        params: crate::adjustment::AdjustParams,
        rotation: crate::rotation_db::Rotation,
        format: crate::capture::CaptureFormat,
        jpeg_matte: crate::capture::JpegMatte,
    },
    Rendered {
        work: crate::capture::CapturePixelWork,
        format: crate::capture::CaptureFormat,
        jpeg_matte: crate::capture::JpegMatte,
    },
    VideoFrame {
        path: PathBuf,
        target_secs: f64,
        basename: String,
        format: crate::capture::CaptureFormat,
        jpeg_matte: crate::capture::JpegMatte,
    },
}

pub fn default_books_root() -> PathBuf {
    crate::capture::default_output_dir().join("books")
}

pub fn settings_books_root(settings: &crate::settings::Settings) -> PathBuf {
    settings
        .book_root
        .clone()
        .unwrap_or_else(default_books_root)
}

pub fn normalize_book_name(input: &str) -> String {
    sanitize_filename(input, DEFAULT_BOOK_NAME)
}

pub fn book_folder(root: &Path, name: &str) -> PathBuf {
    root.join(normalize_book_name(name))
}

pub fn active_book_folder(settings: &crate::settings::Settings) -> PathBuf {
    book_folder(&settings_books_root(settings), &settings.active_book_name)
}

pub fn path_is_under_or_equal(path: &Path, root: &Path) -> bool {
    let path_norm = crate::search_index_db::normalize_path(path);
    let root_norm = crate::search_index_db::normalize_path(root);
    path_norm == root_norm
        || path_norm.starts_with(&(root_norm.trim_end_matches('/').to_owned() + "/"))
}

pub fn path_is_under_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path_is_under_or_equal(path, root))
}

pub fn is_direct_book_folder(root: &Path, path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| crate::folder_tree::path_eq(parent, root))
}

pub fn containing_book_folder(root: &Path, path: &Path) -> Option<PathBuf> {
    let parent = if path.is_dir() { path } else { path.parent()? };
    if is_direct_book_folder(root, parent) {
        return Some(parent.to_path_buf());
    }
    None
}

pub fn list_books(root: &Path) -> Result<Vec<BookInfo>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in
        fs::read_dir(root).map_err(|e| format!("本棚を読み取れません: {}: {e}", root.display()))?
    {
        let entry = entry.map_err(|e| format!("本棚の項目を読み取れません: {e}"))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|e| format!("本棚の項目種別を読み取れません: {}: {e}", path.display()))?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        rows.push(BookInfo {
            page_count: book_page_count(&path)?,
            path,
            name,
        });
    }
    rows.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(rows)
}

pub fn create_book(root: &Path, name: &str) -> Result<BookOpResult, String> {
    let name = normalize_book_name(name);
    let path = book_folder(root, &name);
    if path.exists() {
        return Err(format!("同名の本が既にあります: {name}"));
    }
    fs::create_dir_all(root)
        .map_err(|e| format!("本棚フォルダを作成できません: {}: {e}", root.display()))?;
    fs::create_dir(&path).map_err(|e| format!("本を作成できません: {}: {e}", path.display()))?;
    Ok(BookOpResult::Created { name, path })
}

pub fn rename_book(root: &Path, old_name: &str, new_name: &str) -> Result<BookOpResult, String> {
    let old_name = normalize_book_name(old_name);
    let new_name = normalize_book_name(new_name);
    if old_name == new_name {
        return Ok(BookOpResult::Renamed { old_name, new_name });
    }
    let from = book_folder(root, &old_name);
    let to = book_folder(root, &new_name);
    ensure_direct_book_target(root, &from)?;
    if to.exists() {
        return Err(format!("同名の本が既にあります: {new_name}"));
    }
    fs::rename(&from, &to).map_err(|e| {
        format!(
            "本の名前を変更できません: {} → {}: {e}",
            from.display(),
            to.display()
        )
    })?;
    Ok(BookOpResult::Renamed { old_name, new_name })
}

pub fn delete_book(root: &Path, name: &str) -> Result<BookOpResult, String> {
    let name = normalize_book_name(name);
    let path = book_folder(root, &name);
    ensure_direct_book_target(root, &path)?;
    fs::remove_dir_all(&path)
        .map_err(|e| format!("本を削除できません: {}: {e}", path.display()))?;
    Ok(BookOpResult::Deleted { name })
}

pub fn append_pages(
    root: PathBuf,
    book_name: String,
    sources: Vec<BookPageSource>,
) -> Result<BookOpResult, String> {
    if sources.is_empty() {
        return Err("追加するページがありません".to_string());
    }
    let added = sources.len();
    let book_name = normalize_book_name(&book_name);
    let folder = book_folder(&root, &book_name);
    fs::create_dir_all(&folder)
        .map_err(|e| format!("本フォルダを作成できません: {}: {e}", folder.display()))?;
    ensure_direct_book_target(&root, &folder)?;

    let start = next_page_number(&folder)?;
    if start + sources.len() - 1 > MAX_BOOK_PAGES {
        return Err(format!(
            "本のページ数が上限 {} を超えます (現在 {}, 追加 {})",
            MAX_BOOK_PAGES,
            start.saturating_sub(1),
            sources.len()
        ));
    }

    let mut first_path = None;
    for (offset, source) in sources.into_iter().enumerate() {
        let page_no = start + offset;
        let dest = destination_for_source(&folder, page_no, &source)?;
        write_source(source, &dest)?;
        if first_path.is_none() {
            first_path = Some(dest);
        }
    }

    Ok(BookOpResult::Append(BookAppendSummary {
        book_name,
        folder,
        added,
        first_path,
    }))
}

pub fn flush_reorder(folder: PathBuf, ordered_paths: Vec<PathBuf>) -> Result<BookOpResult, String> {
    if ordered_paths.len() > MAX_BOOK_PAGES {
        return Err(format!("本のページ数が上限 {} を超えます", MAX_BOOK_PAGES));
    }
    if !folder.is_dir() {
        return Err(format!("本フォルダではありません: {}", folder.display()));
    }
    for path in &ordered_paths {
        if path
            .parent()
            .is_none_or(|parent| !crate::folder_tree::path_eq(parent, &folder))
        {
            return Err(format!(
                "本フォルダ外のページは並べ替えできません: {}",
                path.display()
            ));
        }
    }

    let pid = std::process::id();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut moved = Vec::with_capacity(ordered_paths.len());
    for (idx, src) in ordered_paths.iter().enumerate() {
        let temp = unique_temp_path(&folder, pid, stamp, idx)?;
        fs::rename(src, &temp).map_err(|e| {
            rollback_temp_moves(&moved);
            format!("一時ファイルへ移動できません: {}: {e}", src.display())
        })?;
        moved.push((src.clone(), temp));
    }

    let mut finalized = Vec::with_capacity(moved.len());
    for (idx, (original, temp)) in moved.iter().enumerate() {
        let final_name = final_reorder_name(idx + 1, original);
        let dest = folder.join(final_name);
        if dest.exists() {
            rollback_reorder_pass2(&finalized, &moved);
            return Err(format!("並べ替え先が既に存在します: {}", dest.display()));
        }
        if let Err(e) = fs::rename(temp, &dest) {
            rollback_reorder_pass2(&finalized, &moved);
            return Err(format!(
                "ページ番号を確定できません: {}: {e}",
                dest.display()
            ));
        }
        finalized.push((original.clone(), dest));
    }

    Ok(BookOpResult::Reordered {
        folder,
        count: ordered_paths.len(),
    })
}

fn write_source(source: BookPageSource, dest: &Path) -> Result<(), String> {
    match source {
        BookPageSource::File { src, .. } => copy_file_snapshot(&src, dest),
        BookPageSource::AdjustedFile {
            src,
            params,
            rotation,
            format,
            jpeg_matte,
            ..
        } => {
            let image = decode_file_color_image(&src)?;
            write_adjusted_color_image(dest, image, &params, rotation, format, jpeg_matte)
        }
        BookPageSource::ZipEntry {
            zip_path,
            entry_name,
            ..
        } => {
            let bytes = crate::zip_loader::read_entry_bytes(&zip_path, &entry_name)
                .map_err(|e| format!("ZIP 内画像を読み取れません: {}: {e}", entry_name))?;
            write_bytes_create_new(dest, &bytes)
        }
        BookPageSource::AdjustedZipEntry {
            zip_path,
            entry_name,
            params,
            rotation,
            format,
            jpeg_matte,
            ..
        } => {
            let bytes = crate::zip_loader::read_entry_bytes(&zip_path, &entry_name)
                .map_err(|e| format!("ZIP 内画像を読み取れません: {}: {e}", entry_name))?;
            let image = decode_bytes_color_image(&entry_name, &bytes)?;
            write_adjusted_color_image(dest, image, &params, rotation, format, jpeg_matte)
        }
        BookPageSource::AdjustedPdfPage {
            pdf_path,
            page_num,
            password,
            params,
            rotation,
            format,
            jpeg_matte,
            ..
        } => {
            let result = crate::pdf_loader::render_page(
                &pdf_path,
                page_num,
                4096,
                password.as_deref(),
                None,
                crate::pdf_loader::JobPriority::Critical,
                0,
                crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
            )
            .map_err(|e| format!("PDF ページを描画できません: {}: {e}", pdf_path.display()))?;
            let image = dynamic_image_to_color_image(&result.image);
            write_adjusted_color_image(dest, image, &params, rotation, format, jpeg_matte)
        }
        BookPageSource::Rendered {
            work,
            format,
            jpeg_matte,
        } => {
            let (_basename, width, height, rgba) = crate::capture::run_pixel_work(work)?;
            crate::capture::save_rgba_exact_with_matte(
                dest, format, jpeg_matte, width, height, &rgba,
            )
        }
        BookPageSource::VideoFrame {
            path,
            target_secs,
            format,
            jpeg_matte,
            ..
        } => {
            let frame = crate::video::screenshot::capture_frame(&path, target_secs)
                .map_err(|e| format!("動画フレーム取得に失敗しました: {e}"))?;
            crate::capture::save_rgba_exact_with_matte(
                dest,
                format,
                jpeg_matte,
                frame.width,
                frame.height,
                &frame.rgba,
            )
        }
    }
}

fn decode_file_color_image(path: &Path) -> Result<egui::ColorImage, String> {
    let image = image::open(path)
        .or_else(|_| {
            crate::wic_decoder::decode_to_dynamic_image(path).ok_or_else(|| {
                image::ImageError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "wic decode failed",
                ))
            })
        })
        .or_else(|_| {
            crate::susie_loader::decode_file(path, true, None).map_err(image::ImageError::IoError)
        })
        .map_err(|e| format!("画像をデコードできません: {}: {e}", path.display()))?;
    let image = crate::thumb_loader::apply_exif_orientation(image, path);
    Ok(dynamic_image_to_color_image(&image))
}

fn decode_bytes_color_image(hint: &str, bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let image = image::load_from_memory(bytes)
        .or_else(|_| {
            crate::wic_decoder::decode_to_dynamic_image_from_bytes(bytes).ok_or_else(|| {
                image::ImageError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "wic decode failed",
                ))
            })
        })
        .or_else(|_| {
            crate::susie_loader::decode_bytes(hint, bytes, true, None)
                .map_err(image::ImageError::IoError)
        })
        .map_err(|e| format!("ZIP 内画像をデコードできません: {hint}: {e}"))?;
    let image = crate::thumb_loader::apply_exif_orientation_from_bytes(image, bytes);
    Ok(dynamic_image_to_color_image(&image))
}

fn write_adjusted_color_image(
    dest: &Path,
    image: egui::ColorImage,
    params: &crate::adjustment::AdjustParams,
    rotation: crate::rotation_db::Rotation,
    format: crate::capture::CaptureFormat,
    jpeg_matte: crate::capture::JpegMatte,
) -> Result<(), String> {
    let mut adjusted = crate::adjustment::apply_adjustments_fast(&image, params);
    if !rotation.is_none() {
        adjusted = crate::capture::rotate_color_image(&adjusted, rotation);
    }
    let rgba = crate::capture::color_image_to_rgba(&adjusted);
    crate::capture::save_rgba_exact_with_matte(
        dest,
        format,
        jpeg_matte,
        adjusted.size[0] as u32,
        adjusted.size[1] as u32,
        &rgba,
    )
}

fn dynamic_image_to_color_image(img: &image::DynamicImage) -> egui::ColorImage {
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

fn copy_file_snapshot(src: &Path, dest: &Path) -> Result<(), String> {
    let Some(parent) = dest.parent() else {
        return Err(format!("保存先が不正です: {}", dest.display()));
    };
    fs::create_dir_all(parent)
        .map_err(|e| format!("本フォルダを作成できません: {}: {e}", parent.display()))?;
    let mut input =
        fs::File::open(src).map_err(|e| format!("画像を開けません: {}: {e}", src.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| format!("ページを作成できません: {}: {e}", dest.display()))?;
    std::io::copy(&mut input, &mut output)
        .map_err(|e| format!("ページを書き込めません: {}: {e}", dest.display()))?;
    output
        .flush()
        .map_err(|e| format!("ページを flush できません: {}: {e}", dest.display()))
}

fn write_bytes_create_new(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let Some(parent) = dest.parent() else {
        return Err(format!("保存先が不正です: {}", dest.display()));
    };
    fs::create_dir_all(parent)
        .map_err(|e| format!("本フォルダを作成できません: {}: {e}", parent.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| format!("ページを作成できません: {}: {e}", dest.display()))?;
    output
        .write_all(bytes)
        .map_err(|e| format!("ページを書き込めません: {}: {e}", dest.display()))?;
    output
        .flush()
        .map_err(|e| format!("ページを flush できません: {}: {e}", dest.display()))
}

fn destination_for_source(
    folder: &Path,
    page_no: usize,
    source: &BookPageSource,
) -> Result<PathBuf, String> {
    let raw_name = match source {
        BookPageSource::File { original_name, .. } => sanitize_filename(original_name, "page"),
        BookPageSource::AdjustedFile {
            original_name,
            format,
            ..
        } => adjusted_destination_name(original_name, *format),
        BookPageSource::ZipEntry { original_name, .. } => sanitize_filename(original_name, "page"),
        BookPageSource::AdjustedZipEntry {
            original_name,
            format,
            ..
        } => adjusted_destination_name(original_name, *format),
        BookPageSource::AdjustedPdfPage {
            basename, format, ..
        } => format!(
            "{}.{}",
            crate::capture::basename_from_text(basename),
            format.extension()
        ),
        BookPageSource::Rendered { work, format, .. } => {
            let basename = match work {
                crate::capture::CapturePixelWork::Single(job) => job.basename.as_str(),
                crate::capture::CapturePixelWork::Spread { basename, .. } => basename.as_str(),
            };
            format!(
                "{}.{}",
                crate::capture::basename_from_text(basename),
                format.extension()
            )
        }
        BookPageSource::VideoFrame {
            basename, format, ..
        } => format!(
            "{}.{}",
            crate::capture::basename_from_text(basename),
            format.extension()
        ),
    };
    let name = sanitize_filename(&raw_name, "page");
    let path = folder.join(format!("{page_no:04}_{name}"));
    if path.exists() {
        return Err(format!("ページ番号が既に存在します: {}", path.display()));
    }
    Ok(path)
}

fn adjusted_destination_name(original_name: &str, format: crate::capture::CaptureFormat) -> String {
    let stem = Path::new(original_name)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("page");
    format!(
        "{}.{}",
        crate::capture::basename_from_text(stem),
        format.extension()
    )
}

fn next_page_number(folder: &Path) -> Result<usize, String> {
    let mut max_page = 0usize;
    if !folder.exists() {
        return Ok(1);
    }
    for entry in fs::read_dir(folder)
        .map_err(|e| format!("本フォルダを読み取れません: {}: {e}", folder.display()))?
    {
        let entry = entry.map_err(|e| format!("本フォルダの項目を読み取れません: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(num) = page_number_from_name(&name) {
            max_page = max_page.max(num);
        }
    }
    if max_page >= MAX_BOOK_PAGES {
        return Err(format!(
            "本のページ数が上限 {} に達しています",
            MAX_BOOK_PAGES
        ));
    }
    Ok(max_page + 1)
}

fn book_page_count(folder: &Path) -> Result<usize, String> {
    let mut count = 0usize;
    for entry in fs::read_dir(folder)
        .map_err(|e| format!("本フォルダを読み取れません: {}: {e}", folder.display()))?
    {
        let entry = entry.map_err(|e| format!("本フォルダの項目を読み取れません: {e}"))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if page_number_from_name(&name).is_some() && is_supported_book_image_path(&path) {
            count += 1;
        }
    }
    Ok(count)
}

fn page_number_from_name(name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.len() < 6 || bytes[4] != b'_' {
        return None;
    }
    let digits = &bytes[0..4];
    if !digits.iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value = std::str::from_utf8(digits).ok()?.parse::<usize>().ok()?;
    (1..=MAX_BOOK_PAGES).contains(&value).then_some(value)
}

fn is_supported_book_image_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "webp"
            | "bmp"
            | "gif"
            | "heic"
            | "heif"
            | "avif"
            | "jxl"
            | "tif"
            | "tiff"
    )
}

fn ensure_direct_book_target(root: &Path, path: &Path) -> Result<(), String> {
    if !is_direct_book_folder(root, path) {
        return Err(format!("本棚直下の本ではありません: {}", path.display()));
    }
    Ok(())
}

fn unique_temp_path(folder: &Path, pid: u32, stamp: u128, idx: usize) -> Result<PathBuf, String> {
    for retry in 0..100 {
        let path = folder.join(format!(".miv-book-tmp-{pid}-{stamp}-{idx:04}-{retry:02}"));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("並べ替え用の一時ファイル名を作れません".to_string())
}

fn rollback_temp_moves(moved: &[(PathBuf, PathBuf)]) {
    for (original, temp) in moved.iter().rev() {
        let _ = fs::rename(temp, original);
    }
}

fn rollback_reorder_pass2(finalized: &[(PathBuf, PathBuf)], moved: &[(PathBuf, PathBuf)]) {
    for (original, dest) in finalized.iter().rev() {
        let _ = fs::rename(dest, original);
    }
    for (original, temp) in moved.iter().rev() {
        if temp.exists() {
            let _ = fs::rename(temp, original);
        }
    }
}

fn final_reorder_name(page_no: usize, original: &Path) -> String {
    let original_name = original
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("page");
    let suffix = if page_number_from_name(original_name).is_some() && original_name.len() > 5 {
        &original_name[5..]
    } else {
        original_name
    };
    format!("{page_no:04}_{}", sanitize_filename(suffix, "page"))
}

fn sanitize_filename(input: &str, fallback: &str) -> String {
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
        fallback.to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_book_name_preserves_japanese_and_replaces_windows_invalids() {
        assert_eq!(normalize_book_name("  名前:なし?  "), "名前_なし_");
        assert_eq!(normalize_book_name("..."), DEFAULT_BOOK_NAME);
    }

    #[test]
    fn page_number_requires_four_digits_and_underscore() {
        assert_eq!(page_number_from_name("0001_a.jpg"), Some(1));
        assert_eq!(page_number_from_name("9999_a.jpg"), Some(9999));
        assert_eq!(page_number_from_name("10000_a.jpg"), None);
        assert_eq!(page_number_from_name("0000_a.jpg"), None);
        assert_eq!(page_number_from_name("001_a.jpg"), None);
    }

    #[test]
    fn reorder_name_renumbers_but_preserves_original_suffix() {
        assert_eq!(
            final_reorder_name(12, Path::new("0007_表紙?.png")),
            "0012_表紙_.png"
        );
        assert_eq!(
            final_reorder_name(1, Path::new("loose.jpg")),
            "0001_loose.jpg"
        );
    }

    #[test]
    fn create_book_rejects_existing_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        create_book(&root, "既存").unwrap();
        assert!(create_book(&root, "既存").is_err());
    }

    #[test]
    fn flush_reorder_rolls_back_when_final_name_conflicts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let folder = tmp.path().join("books").join("book");
        fs::create_dir_all(&folder).unwrap();
        let conflict = folder.join("0001_a.jpg");
        let page_a = folder.join("0002_a.jpg");
        let page_b = folder.join("0003_b.jpg");
        fs::write(&conflict, b"conflict").unwrap();
        fs::write(&page_a, b"a").unwrap();
        fs::write(&page_b, b"b").unwrap();

        let result = flush_reorder(folder.clone(), vec![page_a.clone(), page_b.clone()]);

        assert!(result.is_err());
        assert_eq!(fs::read(&conflict).unwrap(), b"conflict");
        assert_eq!(fs::read(&page_a).unwrap(), b"a");
        assert_eq!(fs::read(&page_b).unwrap(), b"b");
        let temp_left = fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".miv-book-tmp-")
            });
        assert!(!temp_left);
    }
}
