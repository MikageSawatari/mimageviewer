use std::path::PathBuf;

use crate::grid_item::GridItem;

/// `all_media` の各要素がどの `GridItem` になるかのスキャン時分類。
/// Image → `GridItem::Image`、Video → `GridItem::Video`、Audio → `GridItem::Audio`。
/// 動画のアップスケール派生ペア除去 (`filter_upscaled_video_pairs_fast`) は Video のみ対象。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScanMediaKind {
    Image,
    Video,
    Audio,
}

/// ディレクトリ走査結果 (read_dir + 各エントリ metadata 取得の成果物)。
///
/// Ctrl+↑↓ 移動時は DFS スレッドで事前に走査しておき、UI スレッドの
/// `load_folder` で `read_dir` を走らせずに items を組み立てるために使う。
/// 通常パス (ユーザーが明示的に開いたフォルダ等) では `scan_directory` を
/// UI スレッドで呼んで即座に生成する。
pub(crate) struct ScannedDir {
    /// (GridItem, (mtime, file_size)) の対。GridItem は Folder / ZipFile /
    /// PdfFile / ConvertibleArchive のいずれか。load_folder 内でソートされる。
    pub folders: Vec<(GridItem, Option<(i64, i64)>)>,
    /// (path, kind, mtime, file_size) のタプル。load_folder 内で sort_order
    /// 設定に基づいてソートされる。
    pub all_media: Vec<(PathBuf, ScanMediaKind, i64, i64)>,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageFolderPageCountOptions {
    pub(crate) include_convertible_archives: bool,
    pub(crate) show_hidden_files: bool,
    pub(crate) skip_duplicate_images: bool,
    pub(crate) image_ext_priority: Vec<String>,
    pub(crate) fingerprint: i64,
}

pub(crate) fn image_folder_page_count_options(
    settings: &crate::settings::Settings,
) -> ImageFolderPageCountOptions {
    // Page-count metadata is independent of auto-opening image-only folders.
    // Even when auto-open is disabled, image-only child folders should expose
    // the same background-loaded page count as ZIP and PDF containers.
    ImageFolderPageCountOptions {
        include_convertible_archives: !settings.archive_file_handling_ignores_convertible(),
        show_hidden_files: settings.show_hidden_files,
        skip_duplicate_images: settings.skip_duplicate_images,
        image_ext_priority: settings.image_ext_priority.clone(),
        fingerprint: image_page_recognition_fingerprint(settings),
    }
}

/// ZIP と画像フォルダのページ数 cache が共有する画像認識規則 fingerprint。
/// Susie の有効/無効と対応拡張子集合も identity に含め、プラグイン構成変更後に
/// 古いページ数を恒久再利用しない。
pub(crate) fn image_page_recognition_fingerprint(settings: &crate::settings::Settings) -> i64 {
    // 永続 cache 用の決定的 FNV-1a。判定規則を変えたときは version bytes を上げる。
    let mut hash = 0xcbf29ce484222325u64;
    let mut mix = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    mix(b"image-folder-page-count-v1\0");
    mix(&[u8::from(settings.show_hidden_files)]);
    mix(&[u8::from(settings.skip_duplicate_images)]);
    mix(&[u8::from(
        !settings.archive_file_handling_ignores_convertible(),
    )]);
    mix(&[u8::from(settings.susie_enabled)]);
    for extension in &settings.image_ext_priority {
        mix(extension.as_bytes());
        mix(&[0]);
    }
    if let Some(pool) = crate::susie_loader::try_get_pool() {
        for extension in pool.extensions() {
            mix(extension.as_bytes());
            mix(&[0]);
        }
    }
    i64::from_ne_bytes(hash.to_ne_bytes())
}

/// 通常フォルダを実際の一覧走査と同じ規則で分類し、本扱いなら表示ページ数を返す。
/// `Ok(None)` は走査成功だが画像だけの本ではない、`Err` は走査自体の失敗。
pub(crate) fn image_folder_page_count(
    path: &std::path::Path,
    options: &ImageFolderPageCountOptions,
) -> std::io::Result<Option<u32>> {
    // ページ数取得では走査失敗を空フォルダとして cache できないため Result を維持する。
    // 取得済み ReadDir をそのまま共通分類へ渡し、同じディレクトリを二度開かない。
    let entries = std::fs::read_dir(path)?;
    let mut scan = scan_directory_entries(
        entries,
        options.include_convertible_archives,
        options.show_hidden_files,
    );
    if !is_image_only_book_contents(!scan.folders.is_empty(), &scan.all_media) {
        return Ok(None);
    }
    if options.skip_duplicate_images {
        filter_image_ext_duplicates(&mut scan.all_media, &options.image_ext_priority);
    }
    u32::try_from(scan.all_media.len())
        .map(Some)
        .map_err(|_| std::io::Error::other("画像のみフォルダのページ数が上限を超えています"))
}

/// 1 ディレクトリ分の分類結果が「画像だけの本」かを判定する共通述語。
///
/// 通常一覧のページ数判定とサブフォルダ展開の集約判定が同じ境界を使うために分離する。
/// `has_container` は実サブフォルダ、ZIP/PDF、設定上有効な変換アーカイブのいずれかが
/// 1 件でもあれば true。テキスト等の非対応ファイルは従来どおり判定に影響しない。
pub(super) fn is_image_only_book_contents(
    has_container: bool,
    all_media: &[(PathBuf, ScanMediaKind, i64, i64)],
) -> bool {
    !has_container
        && !all_media.is_empty()
        && all_media
            .iter()
            .all(|(_, kind, _, _)| *kind == ScanMediaKind::Image)
}

/// ディレクトリ走査: `read_dir` + 各エントリの `file_type()` / `metadata()` 呼び出し。
///
/// **Windows パフォーマンス上の注意**: `entry.file_type()` と `entry.metadata()` は
/// `FindFirstFile`/`FindNextFile` が返した WIN32_FIND_DATA をそのまま再利用するので
/// syscall は不要。対して `Path::is_dir()` は都度 `GetFileAttributes` を呼び出すため
/// 数百枚のフォルダで per-entry 1-5ms、合計 500-1000ms のブロック源になる
/// (AI 画像フォルダで計測実績あり)。必ず `entry.file_type()` 側を使うこと。
/// 方針は [docs/ui-responsiveness.md §1.1](../../docs/ui-responsiveness.md) にまとめてある。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn scan_directory(path: &std::path::Path) -> ScannedDir {
    scan_directory_with_convertible_archives(path, true, false)
}

pub(crate) fn scan_directory_with_settings(
    path: &std::path::Path,
    settings: &crate::settings::Settings,
) -> ScannedDir {
    scan_directory_with_convertible_archives(
        path,
        !settings.archive_file_handling_ignores_convertible(),
        settings.show_hidden_files,
    )
}

pub(crate) fn scan_directory_with_convertible_archives(
    path: &std::path::Path,
    include_convertible_archives: bool,
    show_hidden_files: bool,
) -> ScannedDir {
    let Ok(entries) = std::fs::read_dir(path) else {
        return ScannedDir {
            folders: Vec::new(),
            all_media: Vec::new(),
        };
    };
    scan_directory_entries(entries, include_convertible_archives, show_hidden_files)
}

fn scan_directory_entries(
    entries: std::fs::ReadDir,
    include_convertible_archives: bool,
    show_hidden_files: bool,
) -> ScannedDir {
    let mut folders: Vec<(GridItem, Option<(i64, i64)>)> = Vec::new();
    let mut all_media: Vec<(PathBuf, ScanMediaKind, i64, i64)> = Vec::new();
    let mut entry_file_names_ci: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for entry in entries.flatten() {
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
        // file_type() は FindFirstFile のキャッシュ読み (syscall なし)。
        // metadata() も同様にキャッシュから返るが、失敗しても fallback 0 で続行する。
        let kind = entry
            .file_type()
            .map(|ft| crate::fs_entry::classify_dir_entry(&entry, &ft))
            .unwrap_or(crate::fs_entry::DirEntryKind::Other);
        entry_file_names_ci.insert(entry.file_name().to_string_lossy().to_lowercase());
        if crate::fs_entry::should_hide_fs_entry(&entry, show_hidden_files) {
            continue;
        }
        let p = entry.path();
        if kind.is_directory() {
            if crate::video::upscale::paths::has_work_dir_suffix(&p) {
                continue;
            }
            let meta = entry.metadata().ok();
            let mtime = meta
                .as_ref()
                .map_or(0, |m| crate::ui_helpers::mtime_secs(m));
            folders.push((GridItem::Folder(p), Some((mtime, 0))));
        } else if crate::folder_tree::is_apple_double(&p) {
            // macOS/iPhone AppleDouble メタデータ - スキップ
        } else if kind.is_file()
            && let Some(ext) = p.extension().and_then(|e| e.to_str())
        {
            let ext_lower = ext.to_ascii_lowercase();
            let meta = entry.metadata().ok();
            let mtime = meta
                .as_ref()
                .map_or(0, |m| crate::ui_helpers::mtime_secs(m));
            let file_size = meta.as_ref().map_or(0, |m| m.len() as i64);
            if crate::folder_tree::is_recognized_image_ext(&ext_lower) {
                all_media.push((p, ScanMediaKind::Image, mtime, file_size));
            } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
                all_media.push((p, ScanMediaKind::Video, mtime, file_size));
            } else if crate::folder_tree::is_audio_ext(&ext_lower) {
                all_media.push((p, ScanMediaKind::Audio, mtime, file_size));
            } else if crate::folder_tree::is_zip_extension(&ext_lower) {
                folders.push((GridItem::ZipFile(p), Some((mtime, file_size))));
            } else if ext_lower == "pdf" {
                folders.push((GridItem::PdfFile(p), Some((mtime, file_size))));
            } else if include_convertible_archives
                && let Some(fmt) =
                    crate::archive_converter::ArchiveFormat::from_extension(&ext_lower)
            {
                folders.push((
                    GridItem::ConvertibleArchive {
                        path: p,
                        format: fmt,
                    },
                    Some((mtime, file_size)),
                ));
            }
        }
    }
    filter_upscaled_video_pairs_fast(&mut all_media, &entry_file_names_ci);
    ScannedDir { folders, all_media }
}

pub(super) fn filter_upscaled_video_pairs_fast(
    all_media: &mut Vec<(PathBuf, ScanMediaKind, i64, i64)>,
    entry_file_names_ci: &std::collections::HashSet<String>,
) {
    let mut source_stem_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (path, kind, _, _) in all_media.iter() {
        if *kind != ScanMediaKind::Video || is_miv_upscaled_derivative(path) {
            continue;
        }
        if let Some(stem) = file_stem_ci(path) {
            *source_stem_counts.entry(stem).or_insert(0) += 1;
        }
    }

    let derivative_source_stems: std::collections::HashSet<String> = all_media
        .iter()
        .filter(|(_, kind, _, _)| *kind == ScanMediaKind::Video)
        .filter_map(|(path, _, _, _)| {
            source_stem_for_miv_upscaled_derivative(path, entry_file_names_ci)
        })
        .filter(|source_stem| source_stem_counts.get(source_stem).copied() == Some(1))
        .collect();

    if derivative_source_stems.is_empty() {
        return;
    }

    all_media.retain(|(path, kind, _, _)| {
        if is_miv_upscaled_derivative(path) {
            return true;
        }
        // 音声はアップスケール動画の companion ではない (例: song.mp3 と song.miv.mkv は
        // 別メディア) ので、同一 stem でも常に残す (Codex P2)。元動画と companion 画像
        // (sidecar サムネ等) は従来どおり同一 stem のとき隠す。
        if *kind == ScanMediaKind::Audio {
            return true;
        }
        file_stem_ci(path).is_none_or(|stem| !derivative_source_stems.contains(&stem))
    });
}

/// 同じ物理フォルダ内の動画と同名 stem の画像を一覧から除外する。
///
/// `media` は必ず 1 フォルダ分だけを渡す。フラットビュー全体を渡すと、別フォルダの
/// 同名ファイルまで衝突する。`use_sidecar` が有効なら、除外画像を動画サムネイルへ
/// 引き継ぐため `(video_path, image_path)` を返す。画像の除外自体は設定に関係なく行う。
pub(super) fn filter_video_image_duplicates(
    media: &mut Vec<(PathBuf, ScanMediaKind, i64, i64)>,
    use_sidecar: bool,
) -> Vec<(PathBuf, PathBuf)> {
    let mut videos_by_stem: std::collections::HashMap<String, Vec<PathBuf>> =
        std::collections::HashMap::new();
    for (path, kind, _, _) in media.iter() {
        if *kind == ScanMediaKind::Video {
            videos_by_stem
                .entry(super::stem_lower(path))
                .or_default()
                .push(path.clone());
        }
    }
    if videos_by_stem.is_empty() {
        return Vec::new();
    }

    let mut sidecars = Vec::new();
    if use_sidecar {
        for (path, kind, _, _) in media.iter() {
            if *kind != ScanMediaKind::Image {
                continue;
            }
            if let Some(videos) = videos_by_stem.get(&super::stem_lower(path)) {
                sidecars.extend(videos.iter().cloned().map(|video| (video, path.clone())));
            }
        }
    }

    media.retain(|(path, kind, _, _)| {
        *kind != ScanMediaKind::Image || !videos_by_stem.contains_key(&super::stem_lower(path))
    });
    sidecars
}

/// ZIP/PDF/対応アーカイブと同名の実フォルダがあれば、実フォルダを一覧の正本にする。
pub(super) fn filter_virtual_folder_duplicates(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
) {
    let real_folder_names: std::collections::HashSet<String> = folders
        .iter()
        .filter_map(|item| match item {
            GridItem::Folder(path) => path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_lowercase),
            _ => None,
        })
        .collect();
    let keep: Vec<bool> = folders
        .iter()
        .map(|item| match item {
            GridItem::ZipFile(path)
            | GridItem::PdfFile(path)
            | GridItem::ConvertibleArchive { path, .. } => {
                !real_folder_names.contains(&super::stem_lower(path))
            }
            _ => true,
        })
        .collect();
    let mut iter = keep.iter();
    folders.retain(|_| *iter.next().unwrap());
    let mut iter = keep.iter();
    folder_metas.retain(|_| *iter.next().unwrap());
}

/// 同名の ZIP/CBZ があれば、変換元になる RAR/7z/LZH 等を一覧から除外する。
pub(super) fn filter_convertible_archive_duplicates(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
) {
    let zip_stems: std::collections::HashSet<String> = folders
        .iter()
        .filter_map(|item| match item {
            GridItem::ZipFile(path) => Some(super::stem_lower(path)),
            _ => None,
        })
        .collect();
    if zip_stems.is_empty() {
        return;
    }
    let keep: Vec<bool> = folders
        .iter()
        .map(|item| match item {
            GridItem::ConvertibleArchive { path, .. } => {
                !zip_stems.contains(&super::stem_lower(path))
            }
            _ => true,
        })
        .collect();
    let mut iter = keep.iter();
    folders.retain(|_| *iter.next().unwrap());
    let mut iter = keep.iter();
    folder_metas.retain(|_| *iter.next().unwrap());
}

/// 同名ステムの画像を拡張子優先順で 1 件へ絞る。一覧と画像フォルダのページ数で共有する。
pub(super) fn filter_image_ext_duplicates(
    all_media: &mut Vec<(PathBuf, ScanMediaKind, i64, i64)>,
    priority: &[String],
) {
    let mut best: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for (index, (path, kind, _, _)) in all_media.iter().enumerate() {
        if *kind != ScanMediaKind::Image {
            continue;
        }
        let stem = super::stem_lower(path);
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_lowercase();
        let rank = priority
            .iter()
            .position(|candidate| candidate == &extension)
            .unwrap_or(usize::MAX);
        match best.get(&stem) {
            Some(&(existing_rank, _)) if rank >= existing_rank => {}
            _ => {
                best.insert(stem, (rank, index));
            }
        }
    }

    let mut stem_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (path, kind, _, _) in all_media.iter() {
        if *kind == ScanMediaKind::Image {
            *stem_counts.entry(super::stem_lower(path)).or_insert(0) += 1;
        }
    }
    let keep_indices: std::collections::HashSet<usize> = best
        .iter()
        .filter(|(stem, _)| stem_counts.get(stem.as_str()).copied().unwrap_or(0) > 1)
        .map(|(_, &(_, index))| index)
        .collect();
    if keep_indices.is_empty() {
        return;
    }
    let mut index = 0usize;
    all_media.retain(|(path, kind, _, _)| {
        let current = index;
        index += 1;
        if *kind != ScanMediaKind::Image {
            return true;
        }
        let stem = super::stem_lower(path);
        stem_counts.get(&stem).copied().unwrap_or(0) <= 1 || keep_indices.contains(&current)
    });
}

pub(super) fn is_miv_upscaled_derivative(path: &std::path::Path) -> bool {
    // Fast UI-path check: a `.miv.mkv` name marks an upscaled derivative visually.
    // Pairing/hiding stays stricter and also requires the sibling `.miv.json`.
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_lowercase().ends_with(".miv.mkv"))
}

fn source_stem_for_miv_upscaled_derivative(
    path: &std::path::Path,
    entry_file_names_ci: &std::collections::HashSet<String>,
) -> Option<String> {
    let file_name = path.file_name()?.to_string_lossy().to_lowercase();
    let source_stem = file_name.strip_suffix(".miv.mkv")?;
    if source_stem.is_empty() {
        return None;
    }
    let sidecar_name = format!("{source_stem}.miv.json");
    entry_file_names_ci
        .contains(&sidecar_name)
        .then(|| source_stem.to_owned())
}

fn file_stem_ci(path: &std::path::Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_lowercase)
}

/// `ScannedDir` の内容シグネチャ (path + mtime + size + 種別) を u64 ハッシュ化する。
/// フォーカス復帰時の差分判定用。`read_dir` の返却順は NTFS で保証されないので
/// 並び順非依存にするため path で明示的にソートしてからハッシュする。
/// プロセス内比較専用 (DefaultHasher は Rust バージョン間で安定でないため永続化しない)。
///
/// 既知の制限: mtime は `mtime_secs` (秒精度) を使うので、同一秒内に同サイズで
/// 上書きされた場合は差分検知できず再ロードがスキップされる。画像ファイルが
/// 偶然同サイズで <1 秒以内に書き換わる現実的なシナリオは稀なため許容している。
/// 必要なら `metadata.modified()` の SystemTime を秒+nanos で取り直す拡張が可能。
pub(crate) fn signature_from_scan(scan: &ScannedDir) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut entries: Vec<(&std::ffi::OsStr, i64, i64, &'static str)> =
        Vec::with_capacity(scan.folders.len() + scan.all_media.len());
    for (item, meta) in &scan.folders {
        let (path, kind) = match item {
            GridItem::Folder(p) => (p.as_os_str(), "folder"),
            GridItem::ZipFile(p) => (p.as_os_str(), "zip"),
            GridItem::PdfFile(p) => (p.as_os_str(), "pdf"),
            GridItem::ConvertibleArchive { path, .. } => (path.as_os_str(), "archive"),
            _ => continue,
        };
        let (mtime, size) = meta.unwrap_or((0, 0));
        entries.push((path, mtime, size, kind));
    }
    for (p, media_kind, mtime, size) in &scan.all_media {
        let kind = match media_kind {
            ScanMediaKind::Image => "image",
            ScanMediaKind::Video => "video",
            ScanMediaKind::Audio => "audio",
        };
        entries.push((p.as_os_str(), *mtime, *size, kind));
    }
    entries.sort();
    let mut hasher = DefaultHasher::new();
    entries.len().hash(&mut hasher);
    for e in &entries {
        e.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod page_count_tests {
    use super::*;

    fn options(skip_duplicate_images: bool) -> ImageFolderPageCountOptions {
        ImageFolderPageCountOptions {
            include_convertible_archives: true,
            show_hidden_files: true,
            skip_duplicate_images,
            image_ext_priority: vec!["png".to_owned(), "jpg".to_owned()],
            fingerprint: 1,
        }
    }

    #[test]
    fn page_count_options_do_not_depend_on_image_folder_auto_open() {
        let mut settings = crate::settings::Settings::default();
        settings.auto_fullscreen_zip_pdf = false;
        settings.detached_viewer_open_images_in_window = false;
        settings.auto_fullscreen_image_folders = false;

        assert!(!settings.auto_fullscreen_image_folders_enabled());
        let options = image_folder_page_count_options(&settings);
        assert_eq!(options.show_hidden_files, settings.show_hidden_files);
        assert_eq!(
            options.skip_duplicate_images,
            settings.skip_duplicate_images
        );
    }

    #[test]
    fn image_folder_page_count_accepts_images_and_ignores_unrelated_files() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("001.jpg"), b"not decoded during scan").unwrap();
        std::fs::write(temp.path().join("002.png"), b"not decoded during scan").unwrap();
        std::fs::write(temp.path().join("notes.txt"), b"metadata").unwrap();

        assert_eq!(
            image_folder_page_count(temp.path(), &options(false)).unwrap(),
            Some(2)
        );
    }

    #[test]
    fn image_folder_page_count_rejects_empty_mixed_and_nested_folders() {
        let empty = tempfile::TempDir::new().unwrap();
        assert_eq!(
            image_folder_page_count(empty.path(), &options(false)).unwrap(),
            None
        );

        let mixed = tempfile::TempDir::new().unwrap();
        std::fs::write(mixed.path().join("001.jpg"), b"image").unwrap();
        std::fs::write(mixed.path().join("clip.mp4"), b"video").unwrap();
        assert_eq!(
            image_folder_page_count(mixed.path(), &options(false)).unwrap(),
            None
        );

        let nested = tempfile::TempDir::new().unwrap();
        std::fs::write(nested.path().join("001.jpg"), b"image").unwrap();
        std::fs::create_dir(nested.path().join("chapter-2")).unwrap();
        assert_eq!(
            image_folder_page_count(nested.path(), &options(false)).unwrap(),
            None
        );
    }

    #[test]
    fn image_folder_page_count_uses_the_same_duplicate_rule_as_the_grid() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("001.jpg"), b"jpg").unwrap();
        std::fs::write(temp.path().join("001.png"), b"png").unwrap();
        std::fs::write(temp.path().join("002.jpg"), b"jpg").unwrap();

        assert_eq!(
            image_folder_page_count(temp.path(), &options(false)).unwrap(),
            Some(3)
        );
        assert_eq!(
            image_folder_page_count(temp.path(), &options(true)).unwrap(),
            Some(2)
        );
    }

    #[test]
    fn image_folder_page_count_propagates_scan_errors() {
        let temp = tempfile::TempDir::new().unwrap();
        let missing = temp.path().join("missing");
        assert!(image_folder_page_count(&missing, &options(false)).is_err());
    }

    #[test]
    fn page_count_fingerprint_changes_with_susie_enablement() {
        let mut disabled = crate::settings::Settings::default();
        disabled.susie_enabled = false;
        let mut enabled = disabled.clone();
        enabled.susie_enabled = true;

        assert_ne!(
            image_page_recognition_fingerprint(&disabled),
            image_page_recognition_fingerprint(&enabled)
        );
    }
}
