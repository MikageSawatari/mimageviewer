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
    scan_directory_with_convertible_archives(path, true)
}

pub(crate) fn scan_directory_with_settings(
    path: &std::path::Path,
    settings: &crate::settings::Settings,
) -> ScannedDir {
    scan_directory_with_convertible_archives(
        path,
        !settings.archive_file_handling_ignores_convertible(),
    )
}

pub(crate) fn scan_directory_with_convertible_archives(
    path: &std::path::Path,
    include_convertible_archives: bool,
) -> ScannedDir {
    let mut folders: Vec<(GridItem, Option<(i64, i64)>)> = Vec::new();
    let mut all_media: Vec<(PathBuf, ScanMediaKind, i64, i64)> = Vec::new();
    let mut entry_file_names_ci: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    let Ok(entries) = std::fs::read_dir(path) else {
        return ScannedDir { folders, all_media };
    };
    for entry in entries.flatten() {
        // file_type() は FindFirstFile のキャッシュ読み (syscall なし)。
        // metadata() も同様にキャッシュから返るが、失敗しても fallback 0 で続行する。
        let kind = entry
            .file_type()
            .map(|ft| crate::fs_entry::classify_dir_entry(&entry, &ft))
            .unwrap_or(crate::fs_entry::DirEntryKind::Other);
        entry_file_names_ci.insert(entry.file_name().to_string_lossy().to_lowercase());
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
            let file_size = meta.map_or(0, |m| m.len() as i64);
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
                if fmt == crate::archive_converter::ArchiveFormat::Rar
                    && crate::archive_converter::is_non_first_rar_part(&p)
                {
                    continue;
                }
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
