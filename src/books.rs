use std::collections::HashSet;
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
    pub edit_copies: Vec<BookPathMapping>,
}

#[derive(Clone, Debug)]
pub struct BookPathMapping {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookTransferKind {
    Copy,
    Move,
}

#[derive(Debug)]
pub struct BookTransferSummary {
    pub source_folder: PathBuf,
    pub target_book_name: String,
    pub target_folder: PathBuf,
    pub pages: usize,
    pub kind: BookTransferKind,
    pub source_entries: Vec<BookPageEntry>,
    pub edit_moves: Vec<BookPathMapping>,
    pub edit_copies: Vec<BookPathMapping>,
}

#[derive(Debug)]
pub enum BookOpResult {
    Append(BookAppendSummary),
    Transfer(BookTransferSummary),
    List(Vec<BookInfo>),
    Created {
        name: String,
        path: PathBuf,
    },
    Renamed {
        old_name: String,
        new_name: String,
        edit_moves: Vec<BookPathMapping>,
    },
    Deleted {
        name: String,
    },
    Reordered {
        folder: PathBuf,
        count: usize,
        edit_moves: Vec<BookPathMapping>,
    },
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
    ClipboardImage {
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
        return Ok(BookOpResult::Renamed {
            old_name,
            new_name,
            edit_moves: Vec::new(),
        });
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
    let edit_moves = book_page_paths(&to)?
        .into_iter()
        .filter_map(|new_path| {
            let name = new_path.file_name()?.to_owned();
            Some(BookPathMapping {
                from: from.join(name),
                to: new_path,
            })
        })
        .collect();
    Ok(BookOpResult::Renamed {
        old_name,
        new_name,
        edit_moves,
    })
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
    let mut edit_copies = Vec::new();
    for (offset, source) in sources.into_iter().enumerate() {
        let page_no = start + offset;
        let dest = destination_for_source(&folder, page_no, &source)?;
        if let Some(src) = source_edit_copy_path(&root, &folder, &source) {
            edit_copies.push(BookPathMapping {
                from: src,
                to: dest.clone(),
            });
        }
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
        edit_copies,
    }))
}

pub fn flush_reorder(folder: PathBuf, ordered_paths: Vec<PathBuf>) -> Result<BookOpResult, String> {
    let edit_moves = flush_reorder_paths(folder.clone(), ordered_paths)?;
    let count = edit_moves.len();
    Ok(BookOpResult::Reordered {
        folder,
        count,
        edit_moves,
    })
}

pub fn transfer_pages_between_books(
    root: PathBuf,
    source_folder: PathBuf,
    current_order_paths: Vec<PathBuf>,
    selected_paths: Vec<PathBuf>,
    target_book_name: String,
    kind: BookTransferKind,
) -> Result<BookOpResult, String> {
    if selected_paths.is_empty() {
        return Err("移動/コピーするページがありません".to_string());
    }
    if current_order_paths.is_empty() {
        return Err("本にページがありません".to_string());
    }
    ensure_direct_book_target(&root, &source_folder)?;
    let target_book_name = normalize_book_name(&target_book_name);
    let target_folder = book_folder(&root, &target_book_name);
    if crate::folder_tree::path_eq(&source_folder, &target_folder) {
        return Err("同じ本への移動/コピーはまだ対応していません".to_string());
    }
    fs::create_dir_all(&target_folder).map_err(|e| {
        format!(
            "移動先の本フォルダを作成できません: {}: {e}",
            target_folder.display()
        )
    })?;
    ensure_direct_book_target(&root, &target_folder)?;

    let selected_keys = selected_paths
        .iter()
        .map(|path| crate::search_index_db::normalize_path(path))
        .collect::<HashSet<_>>();
    let current_keys = current_order_paths
        .iter()
        .map(|path| crate::search_index_db::normalize_path(path))
        .collect::<HashSet<_>>();
    if !selected_keys.iter().all(|key| current_keys.contains(key)) {
        return Err("選択ページが現在の本に見つかりません".to_string());
    }
    let start = next_page_number(&target_folder)?;
    if start + selected_paths.len() - 1 > MAX_BOOK_PAGES {
        return Err(format!(
            "移動先の本のページ数が上限 {} を超えます (現在 {}, 追加 {})",
            MAX_BOOK_PAGES,
            start.saturating_sub(1),
            selected_paths.len()
        ));
    }

    let commit_mappings = flush_reorder_paths(source_folder.clone(), current_order_paths.clone())?;
    let committed_paths = commit_mappings
        .iter()
        .map(|mapping| mapping.to.clone())
        .collect::<Vec<_>>();
    let mut edit_moves = commit_mappings.clone();
    let mut selected_committed = Vec::new();
    let mut remaining_committed = Vec::new();
    for (old_path, committed_path) in current_order_paths.iter().zip(committed_paths.iter()) {
        if selected_keys.contains(&crate::search_index_db::normalize_path(old_path)) {
            selected_committed.push(committed_path.clone());
        } else {
            remaining_committed.push(committed_path.clone());
        }
    }
    if selected_committed.is_empty() {
        return Err("選択ページが現在の本に見つかりません".to_string());
    }

    let mut completed = Vec::with_capacity(selected_committed.len());
    let mut transfer_mappings = Vec::with_capacity(selected_committed.len());
    for (offset, src) in selected_committed.iter().enumerate() {
        let dest = destination_for_existing_page(&target_folder, start + offset, src)?;
        let result = match kind {
            BookTransferKind::Copy => copy_page_to_destination(src, &dest)
                .map(|_| CompletedTransfer::Copied { dest: dest.clone() }),
            BookTransferKind::Move => {
                move_page_to_destination(src, &dest).map(|mode| CompletedTransfer::Moved {
                    src: src.clone(),
                    dest: dest.clone(),
                    mode,
                })
            }
        };
        match result {
            Ok(done) => {
                transfer_mappings.push(BookPathMapping {
                    from: src.clone(),
                    to: dest.clone(),
                });
                completed.push(done);
            }
            Err(err) => {
                rollback_completed_transfers(&completed);
                return Err(err);
            }
        }
    }

    let edit_copies = if kind == BookTransferKind::Copy {
        transfer_mappings.clone()
    } else {
        Vec::new()
    };
    if kind == BookTransferKind::Move {
        edit_moves.extend(transfer_mappings);
    }

    let source_after_paths = if kind == BookTransferKind::Move {
        match flush_reorder_paths(source_folder.clone(), remaining_committed) {
            Ok(compact_mappings) => {
                let paths = compact_mappings
                    .iter()
                    .map(|mapping| mapping.to.clone())
                    .collect::<Vec<_>>();
                edit_moves.extend(compact_mappings);
                paths
            }
            Err(err) => {
                return Err(format!(
                    "ページ移動は完了しましたが、元本の番号整理に失敗しました: {err}"
                ));
            }
        }
    } else {
        committed_paths
    };
    let source_entries = source_after_paths
        .into_iter()
        .map(|path| {
            let display_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("page")
                .to_string();
            BookPageEntry { path, display_name }
        })
        .collect::<Vec<_>>();

    Ok(BookOpResult::Transfer(BookTransferSummary {
        source_folder,
        target_book_name,
        target_folder,
        pages: selected_committed.len(),
        kind,
        source_entries,
        edit_moves,
        edit_copies,
    }))
}

fn flush_reorder_paths(
    folder: PathBuf,
    ordered_paths: Vec<PathBuf>,
) -> Result<Vec<BookPathMapping>, String> {
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

    Ok(finalized
        .into_iter()
        .map(|(from, to)| BookPathMapping { from, to })
        .collect())
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
        BookPageSource::ClipboardImage {
            format, jpeg_matte, ..
        } => {
            let (width, height, rgba) = read_clipboard_rgba_image()?;
            crate::capture::save_rgba_exact_with_matte(
                dest, format, jpeg_matte, width, height, &rgba,
            )
        }
    }
}

fn source_edit_copy_path(
    root: &Path,
    dest_folder: &Path,
    source: &BookPageSource,
) -> Option<PathBuf> {
    let src = match source {
        BookPageSource::File { src, .. } | BookPageSource::AdjustedFile { src, .. } => src,
        _ => return None,
    };
    let source_book = containing_book_folder(root, src)?;
    (!crate::folder_tree::path_eq(&source_book, dest_folder)).then(|| src.clone())
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

#[cfg(windows)]
fn read_clipboard_rgba_image() -> Result<(u32, u32, Vec<u8>), String> {
    use windows::Win32::Foundation::{HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5};

    unsafe {
        if OpenClipboard(Some(HWND::default())).is_err() {
            return Err("クリップボードを開けません".to_string());
        }

        let result = (|| {
            let hmem = GetClipboardData(CF_DIB.0 as u32)
                .or_else(|_| GetClipboardData(CF_DIBV5.0 as u32))
                .map_err(|_| "クリップボードに画像がありません".to_string())?;
            if hmem.is_invalid() {
                return Err("クリップボードに画像がありません".to_string());
            }
            let global = HGLOBAL(hmem.0);
            let size = GlobalSize(global);
            if size == 0 {
                return Err("クリップボード画像が空です".to_string());
            }
            let ptr = GlobalLock(global) as *const u8;
            if ptr.is_null() {
                return Err("クリップボード画像を読み取れません".to_string());
            }
            let bytes = std::slice::from_raw_parts(ptr, size);
            let decoded = decode_cf_dib_rgba(bytes);
            let _ = GlobalUnlock(global);
            decoded
        })();

        let _ = CloseClipboard();
        result
    }
}

#[cfg(not(windows))]
fn read_clipboard_rgba_image() -> Result<(u32, u32, Vec<u8>), String> {
    Err("この環境ではクリップボード画像を読み取れません".to_string())
}

fn decode_cf_dib_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;

    let header_size = read_u32_le(bytes, 0)? as usize;
    if header_size < 40 || bytes.len() < header_size {
        return Err("クリップボード画像のヘッダーが不正です".to_string());
    }
    let width_i = read_i32_le(bytes, 4)?;
    let height_i = read_i32_le(bytes, 8)?;
    if width_i <= 0 || height_i == 0 || height_i == i32::MIN {
        return Err("クリップボード画像のサイズが不正です".to_string());
    }
    let width = width_i as u32;
    let top_down = height_i < 0;
    let height = if top_down {
        (-height_i) as u32
    } else {
        height_i as u32
    };
    let planes = read_u16_le(bytes, 12)?;
    let bit_count = read_u16_le(bytes, 14)?;
    let compression = read_u32_le(bytes, 16)?;
    if planes != 1 {
        return Err("クリップボード画像の形式が不正です".to_string());
    }
    if !matches!(bit_count, 16 | 24 | 32) {
        return Err("対応していないクリップボード画像形式です".to_string());
    }
    if compression != BI_RGB && compression != BI_BITFIELDS {
        return Err("対応していないクリップボード画像形式です".to_string());
    }
    if compression == BI_RGB && bit_count == 16 {
        return Err("対応していないクリップボード画像形式です".to_string());
    }

    let color_table_bytes = color_table_bytes(bytes, bit_count)?;
    let (pixel_offset, masks) = if compression == BI_BITFIELDS {
        let masks = dib_bitfield_masks(bytes, header_size)?;
        let offset = if header_size == 40 {
            40usize
                .checked_add(12)
                .and_then(|v| v.checked_add(color_table_bytes))
                .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?
        } else {
            header_size
                .checked_add(color_table_bytes)
                .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?
        };
        (offset, Some(masks))
    } else {
        (
            header_size
                .checked_add(color_table_bytes)
                .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?,
            None,
        )
    };

    let stride = dib_stride(width, bit_count)?;
    let needed = (height as usize)
        .checked_sub(1)
        .and_then(|last| last.checked_mul(stride))
        .and_then(|v| v.checked_add((width as usize * bit_count as usize).div_ceil(8)))
        .and_then(|v| v.checked_add(pixel_offset))
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?;
    if bytes.len() < needed {
        return Err("クリップボード画像のデータが不足しています".to_string());
    }

    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?;
    let mut rgba = vec![
        0u8;
        (pixel_count as usize)
            .checked_mul(4)
            .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?
    ];
    let mut any_alpha = bit_count != 32;
    for y in 0..height {
        let src_y = if top_down { y } else { height - 1 - y };
        let row = pixel_offset + src_y as usize * stride;
        for x in 0..width {
            let dst = (y as usize * width as usize + x as usize) * 4;
            match (bit_count, masks) {
                (24, _) => {
                    let src = row + x as usize * 3;
                    rgba[dst] = bytes[src + 2];
                    rgba[dst + 1] = bytes[src + 1];
                    rgba[dst + 2] = bytes[src];
                    rgba[dst + 3] = 255;
                }
                (32, Some((red, green, blue, alpha))) => {
                    let src = row + x as usize * 4;
                    let value = u32::from_le_bytes([
                        bytes[src],
                        bytes[src + 1],
                        bytes[src + 2],
                        bytes[src + 3],
                    ]);
                    rgba[dst] = mask_channel_to_u8(value, red);
                    rgba[dst + 1] = mask_channel_to_u8(value, green);
                    rgba[dst + 2] = mask_channel_to_u8(value, blue);
                    rgba[dst + 3] = if alpha == 0 {
                        255
                    } else {
                        let a = mask_channel_to_u8(value, alpha);
                        any_alpha |= a != 0;
                        a
                    };
                }
                (32, None) => {
                    let src = row + x as usize * 4;
                    rgba[dst] = bytes[src + 2];
                    rgba[dst + 1] = bytes[src + 1];
                    rgba[dst + 2] = bytes[src];
                    rgba[dst + 3] = bytes[src + 3];
                    any_alpha |= bytes[src + 3] != 0;
                }
                (16, Some((red, green, blue, alpha))) => {
                    let src = row + x as usize * 2;
                    let value = u16::from_le_bytes([bytes[src], bytes[src + 1]]) as u32;
                    rgba[dst] = mask_channel_to_u8(value, red);
                    rgba[dst + 1] = mask_channel_to_u8(value, green);
                    rgba[dst + 2] = mask_channel_to_u8(value, blue);
                    rgba[dst + 3] = if alpha == 0 {
                        255
                    } else {
                        mask_channel_to_u8(value, alpha)
                    };
                }
                _ => return Err("対応していないクリップボード画像形式です".to_string()),
            }
        }
    }

    if !any_alpha {
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
    Ok((width, height, rgba))
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "クリップボード画像のヘッダーが不正です".to_string())?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "クリップボード画像のヘッダーが不正です".to_string())?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_i32_le(bytes: &[u8], offset: usize) -> Result<i32, String> {
    Ok(read_u32_le(bytes, offset)? as i32)
}

fn color_table_bytes(bytes: &[u8], bit_count: u16) -> Result<usize, String> {
    let clr_used = read_u32_le(bytes, 32)? as usize;
    if clr_used == 0 || bit_count > 8 {
        return Ok(0);
    }
    clr_used
        .checked_mul(4)
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())
}

fn dib_bitfield_masks(bytes: &[u8], header_size: usize) -> Result<(u32, u32, u32, u32), String> {
    if header_size >= 56 {
        Ok((
            read_u32_le(bytes, 40)?,
            read_u32_le(bytes, 44)?,
            read_u32_le(bytes, 48)?,
            read_u32_le(bytes, 52)?,
        ))
    } else if header_size == 40 {
        Ok((
            read_u32_le(bytes, 40)?,
            read_u32_le(bytes, 44)?,
            read_u32_le(bytes, 48)?,
            0,
        ))
    } else {
        Err("クリップボード画像のマスク情報が不正です".to_string())
    }
}

fn dib_stride(width: u32, bit_count: u16) -> Result<usize, String> {
    let bits = (width as usize)
        .checked_mul(bit_count as usize)
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())?;
    bits.checked_add(31)
        .map(|v| (v / 32) * 4)
        .ok_or_else(|| "クリップボード画像のサイズが大きすぎます".to_string())
}

fn mask_channel_to_u8(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let raw = ((value & mask) >> shift) as u64;
    let max = if bits >= 32 {
        u32::MAX as u64
    } else {
        (1u64 << bits) - 1
    };
    ((raw * 255 + max / 2) / max) as u8
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

#[derive(Clone, Copy)]
enum MoveMode {
    Rename,
    CopyDelete,
}

enum CompletedTransfer {
    Copied {
        dest: PathBuf,
    },
    Moved {
        src: PathBuf,
        dest: PathBuf,
        mode: MoveMode,
    },
}

fn copy_page_to_destination(src: &Path, dest: &Path) -> Result<(), String> {
    match copy_file_snapshot(src, dest) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(dest);
            Err(err)
        }
    }
}

fn move_page_to_destination(src: &Path, dest: &Path) -> Result<MoveMode, String> {
    if dest.exists() {
        return Err(format!("移動先ページが既に存在します: {}", dest.display()));
    }
    match fs::rename(src, dest) {
        Ok(()) => Ok(MoveMode::Rename),
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_page_to_destination(src, dest)?;
            if let Err(delete_err) = fs::remove_file(src) {
                let _ = fs::remove_file(dest);
                return Err(format!(
                    "移動元ページを削除できません: {}: {delete_err}",
                    src.display()
                ));
            }
            Ok(MoveMode::CopyDelete)
        }
        Err(err) => Err(format!(
            "ページを移動できません: {} → {}: {err}",
            src.display(),
            dest.display()
        )),
    }
}

fn rollback_completed_transfers(completed: &[CompletedTransfer]) {
    for item in completed.iter().rev() {
        match item {
            CompletedTransfer::Copied { dest } => {
                let _ = fs::remove_file(dest);
            }
            CompletedTransfer::Moved { src, dest, mode } => {
                rollback_moved_page(src, dest, *mode);
            }
        }
    }
}

fn rollback_moved_page(src: &Path, dest: &Path, mode: MoveMode) {
    if src.exists() {
        let _ = fs::remove_file(dest);
        return;
    }
    match mode {
        MoveMode::Rename => {
            let _ = fs::rename(dest, src);
        }
        MoveMode::CopyDelete => {
            if copy_file_snapshot(dest, src).is_ok() {
                let _ = fs::remove_file(dest);
            }
        }
    }
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
        BookPageSource::ClipboardImage {
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

fn destination_for_existing_page(
    folder: &Path,
    page_no: usize,
    source_path: &Path,
) -> Result<PathBuf, String> {
    let path = folder.join(final_reorder_name(page_no, source_path));
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
    Ok(book_page_paths(folder)?.len())
}

fn book_page_paths(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(folder)
        .map_err(|e| format!("本フォルダを読み取れません: {}: {e}", folder.display()))?
    {
        let entry = entry.map_err(|e| format!("本フォルダの項目を読み取れません: {e}"))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if page_number_from_name(&name).is_some() && is_supported_book_image_path(&path) {
            paths.push(path);
        }
    }
    paths.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(page_number_from_name)
            .unwrap_or(MAX_BOOK_PAGES + 1)
    });
    Ok(paths)
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
    fn decode_cf_dib_rgba_reads_bottom_up_24bpp() {
        let mut dib = vec![0u8; 40 + 8];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        dib[40..43].copy_from_slice(&[0, 0, 255]);
        dib[43..46].copy_from_slice(&[0, 255, 0]);

        let (width, height, rgba) = decode_cf_dib_rgba(&dib).unwrap();

        assert_eq!((width, height), (2, 1));
        assert_eq!(
            rgba,
            vec![
                255, 0, 0, 255, //
                0, 255, 0, 255,
            ]
        );
    }

    #[test]
    fn decode_cf_dib_rgba_reads_top_down_32bpp_bitfields() {
        let mut dib = vec![0u8; 56 + 4];
        dib[0..4].copy_from_slice(&56u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[16..20].copy_from_slice(&3u32.to_le_bytes());
        dib[40..44].copy_from_slice(&0x00ff_0000u32.to_le_bytes());
        dib[44..48].copy_from_slice(&0x0000_ff00u32.to_le_bytes());
        dib[48..52].copy_from_slice(&0x0000_00ffu32.to_le_bytes());
        dib[52..56].copy_from_slice(&0xff00_0000u32.to_le_bytes());
        dib[56..60].copy_from_slice(&[255, 0, 0, 128]);

        let (width, height, rgba) = decode_cf_dib_rgba(&dib).unwrap();

        assert_eq!((width, height), (1, 1));
        assert_eq!(rgba, vec![0, 0, 255, 128]);
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

    #[test]
    fn transfer_copy_commits_current_order_before_copying_selected_pages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("src");
        let target = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        let a = source.join("0001_a.jpg");
        let b = source.join("0002_b.jpg");
        let c = source.join("0003_c.jpg");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::write(&c, b"c").unwrap();

        let result = transfer_pages_between_books(
            root.clone(),
            source.clone(),
            vec![c.clone(), a.clone(), b.clone()],
            vec![c.clone(), b.clone()],
            "dst".to_string(),
            BookTransferKind::Copy,
        )
        .unwrap();

        let BookOpResult::Transfer(summary) = result else {
            panic!("expected transfer result");
        };
        assert_eq!(fs::read(source.join("0001_c.jpg")).unwrap(), b"c");
        assert_eq!(fs::read(source.join("0002_a.jpg")).unwrap(), b"a");
        assert_eq!(fs::read(source.join("0003_b.jpg")).unwrap(), b"b");
        assert_eq!(fs::read(target.join("0001_c.jpg")).unwrap(), b"c");
        assert_eq!(fs::read(target.join("0002_b.jpg")).unwrap(), b"b");
        assert_eq!(
            summary
                .edit_copies
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (source.join("0001_c.jpg"), target.join("0001_c.jpg")),
                (source.join("0003_b.jpg"), target.join("0002_b.jpg")),
            ]
        );
        assert_eq!(
            summary
                .edit_moves
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (c, source.join("0001_c.jpg")),
                (a, source.join("0002_a.jpg")),
                (b, source.join("0003_b.jpg")),
            ]
        );
    }

    #[test]
    fn transfer_move_renames_selected_pages_and_compacts_source() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("src");
        let target = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        let a = source.join("0001_a.jpg");
        let b = source.join("0002_b.jpg");
        let c = source.join("0003_c.jpg");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::write(&c, b"c").unwrap();

        let result = transfer_pages_between_books(
            root,
            source.clone(),
            vec![a.clone(), b.clone(), c.clone()],
            vec![b.clone(), c.clone()],
            "dst".to_string(),
            BookTransferKind::Move,
        )
        .unwrap();
        let BookOpResult::Transfer(summary) = result else {
            panic!("expected transfer result");
        };

        assert_eq!(fs::read(source.join("0001_a.jpg")).unwrap(), b"a");
        assert!(!source.join("0002_b.jpg").exists());
        assert!(!source.join("0003_c.jpg").exists());
        assert_eq!(fs::read(target.join("0001_b.jpg")).unwrap(), b"b");
        assert_eq!(fs::read(target.join("0002_c.jpg")).unwrap(), b"c");
        assert!(summary.edit_copies.is_empty());
        assert_eq!(
            summary
                .edit_moves
                .iter()
                .filter(|m| m.from != m.to)
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (source.join("0002_b.jpg"), target.join("0001_b.jpg")),
                (source.join("0003_c.jpg"), target.join("0002_c.jpg")),
            ]
        );
    }

    #[test]
    fn append_book_page_reports_edit_copy_mapping() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();
        let page = source.join("0001_a.jpg");
        fs::write(&page, b"a").unwrap();

        let result = append_pages(
            root.clone(),
            "dst".to_string(),
            vec![BookPageSource::File {
                src: page.clone(),
                original_name: "0001_a.jpg".to_string(),
            }],
        )
        .unwrap();

        let BookOpResult::Append(summary) = result else {
            panic!("expected append result");
        };
        assert_eq!(
            fs::read(root.join("dst").join("0001_0001_a.jpg")).unwrap(),
            b"a"
        );
        assert_eq!(
            summary
                .edit_copies
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![(page, root.join("dst").join("0001_0001_a.jpg"))]
        );
    }

    #[test]
    fn rename_book_reports_edit_move_mappings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("books");
        let source = root.join("old");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("0001_a.jpg"), b"a").unwrap();
        fs::write(source.join("0002_b.jpg"), b"b").unwrap();

        let result = rename_book(&root, "old", "new").unwrap();

        let BookOpResult::Renamed { edit_moves, .. } = result else {
            panic!("expected rename result");
        };
        assert_eq!(
            edit_moves
                .iter()
                .map(|m| (m.from.clone(), m.to.clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    root.join("old").join("0001_a.jpg"),
                    root.join("new").join("0001_a.jpg")
                ),
                (
                    root.join("old").join("0002_b.jpg"),
                    root.join("new").join("0002_b.jpg")
                ),
            ]
        );
    }
}
