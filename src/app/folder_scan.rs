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

#[derive(Clone)]
pub(crate) struct ScannedFolderEntry {
    pub(crate) item: GridItem,
    pub(crate) display_meta: Option<(i64, i64)>,
    pub(crate) sort_meta: crate::settings::ListingSortMetadata,
}

#[derive(Clone, Debug)]
pub(crate) struct ScannedMediaEntry {
    pub(crate) path: PathBuf,
    pub(crate) kind: ScanMediaKind,
    pub(crate) mtime: i64,
    pub(crate) file_size: i64,
    pub(crate) sort_meta: crate::settings::ListingSortMetadata,
}

/// 通常フォルダ一覧を作るときに、物理フォルダ内には存在するが一覧へ出さなかった項目数。
///
/// `hidden` / `ignored_archive` / `ignored_epub` / `unsupported` / `system` は既存の `read_dir` ループ内で
/// 分類し、`same_name` は同名設定の filter が実際に除いた差分を加算する。件数表示のための
/// 追加走査や metadata I/O は行わない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OmittedFolderEntryCounts {
    pub(crate) same_name: usize,
    /// Recognized entries excluded by the global file-type policy.
    pub(crate) file_type: usize,
    pub(crate) hidden: usize,
    /// 環境設定の書庫処理が「無視する」のため一覧へ出さなかった変換対象書庫。
    pub(crate) ignored_archive: usize,
    /// EPUB の処理が「無視する」のため一覧へ出さなかった本。
    pub(crate) ignored_epub: usize,
    pub(crate) unsupported: usize,
    /// OS / エクスプローラーが勝手に作る付随ファイル。利用者の持ち物ではないので主数字から外す。
    pub(crate) system: usize,
}

impl OmittedFolderEntryCounts {
    /// チップに出す主数字。**利用者のファイルは種類を問わず全部数える。**
    ///
    /// 当初は「対象外拡張子」を除いていたが、除外の動機だった Thumbs.db / desktop.ini は
    /// hidden 属性を持つので `hidden` 側へ入り、除外しても静かにならない。一方で `.txt` や
    /// 未対応形式という「消えたら困るかもしれない実ファイル」の signal を落としていた。
    /// ノイズ源を名前で `system` に分離し、それ以外は全部数えるほうが目的に合う。
    pub(crate) fn primary_count(self) -> usize {
        self.same_name
            .saturating_add(self.file_type)
            .saturating_add(self.hidden)
            .saturating_add(self.ignored_archive)
            .saturating_add(self.ignored_epub)
            .saturating_add(self.unsupported)
    }
}

/// OS / エクスプローラーが自動生成する付随ファイルか。
///
/// これらはフォルダを開いただけで現れ、利用者が意識して置いたものではない。主数字に混ぜると
/// チップが常時点灯して意味を失うので、内訳だけに出す。
pub(crate) fn is_system_metadata_name(name: &str) -> bool {
    const SYSTEM_FILES: &[&str] = &["thumbs.db", "ehthumbs.db", "desktop.ini", ".ds_store"];
    // AppleDouble (`._*`) は macOS / iPhone からのコピーで大量に付いてくる同種のもの。
    name.starts_with("._") || SYSTEM_FILES.contains(&name.to_lowercase().as_str())
}

/// ディレクトリ走査結果 (read_dir + 各エントリ metadata 取得の成果物)。
///
/// Ctrl+↑↓ 移動時は DFS スレッドで事前に走査しておき、UI スレッドの
/// `load_folder` で `read_dir` を走らせずに items を組み立てるために使う。
/// フル機能ウィンドウの直接ロードでは `scan_directory` を UI スレッドで呼ぶ経路も残る。
/// 複数ウィンドウの grid Folder 候補は worker で生成し、画像本と確定した結果だけを
/// detached context へ移譲する。
pub(crate) struct ScannedDir {
    pub(crate) precise_stamps: std::collections::HashMap<u64, Option<(std::time::SystemTime, u64)>>,
    pub(crate) reconcile: Option<FolderScanReconcile>,
    pub(crate) complete_audio_inventory: Option<std::collections::HashSet<String>>,
    /// Folder / ZipFile / PdfFile / ConvertibleArchive と、表示用metadata・一覧sort用metadata。
    /// load_folder 内で3本を同じ順序へ並べ替える。
    pub folders: Vec<ScannedFolderEntry>,
    /// 物理mediaのpath / kind / 表示用mtime・size / 一覧sort用metadata。
    /// load_folder 内で sort_order 設定に基づいてソートされる。
    pub all_media: Vec<ScannedMediaEntry>,
    /// 走査時点で一覧へ出さなかった項目。`same_name` は後段の同名 filter 適用時に入る。
    pub omitted: OmittedFolderEntryCounts,
    /// Source-eligible auxiliary candidates retain their original order independently
    /// of the display projection. Image visibility never changes candidate selection.
    pub(crate) auxiliary_media: AuxiliaryMediaCandidates,
}

impl std::fmt::Debug for ScannedDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScannedDir")
            .field("folders", &self.folders.len())
            .field("media", &self.all_media.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default)]
pub(crate) struct AuxiliaryMediaCandidates {
    entries: Vec<ScannedMediaEntry>,
}

impl AuxiliaryMediaCandidates {
    pub(crate) fn capture(
        media: &[ScannedMediaEntry],
        policy: &crate::file_type_visibility::FileTypeVisibility,
        entry_file_names_ci: &std::collections::HashSet<String>,
    ) -> Self {
        let mut entries = media
            .iter()
            .filter(|entry| {
                entry.kind == ScanMediaKind::Image || policy.allows_physical_file(&entry.path)
            })
            .cloned()
            .collect::<Vec<_>>();
        // Source exclusions can restore an original whose derivative is hidden.
        // Images remain metadata inputs to the same shipped eligibility rule.
        filter_upscaled_video_pairs_fast(&mut entries, entry_file_names_ci);
        let source_stems = entries
            .iter()
            .filter(|entry| matches!(entry.kind, ScanMediaKind::Video | ScanMediaKind::Audio))
            .map(|entry| super::stem_lower(&entry.path))
            .collect::<std::collections::HashSet<_>>();
        let image_stems = entries
            .iter()
            .filter(|entry| entry.kind == ScanMediaKind::Image)
            .map(|entry| super::stem_lower(&entry.path))
            .collect::<std::collections::HashSet<_>>();
        entries.retain(|entry| match entry.kind {
            ScanMediaKind::Image => source_stems.contains(&super::stem_lower(&entry.path)),
            ScanMediaKind::Video | ScanMediaKind::Audio => {
                image_stems.contains(&super::stem_lower(&entry.path))
            }
        });
        Self { entries }
    }

    pub(crate) fn sidecars(&self) -> Vec<(PathBuf, PathBuf)> {
        discover_scanned_video_sidecars(&self.entries)
    }
}

/// App::load_folder が走査結果から実際の一覧を組み立てた結果。
///
/// App::load_folder と remote IPC の通常フォルダ一覧 / 読書位置再計算が
/// この同じ pure helper を使う。表示対象・sort・重複除外を変える場合はここだけを変更し、
/// remote 側に別のオーケストレーションを増やさない。
pub(crate) struct MaterializedFolderListing {
    pub(crate) row_record: std::sync::Arc<FolderRowRecord>,
    pub(crate) items: Vec<GridItem>,
    pub(crate) metas: Vec<Option<(i64, i64)>>,
    /// `items` とalignedな一覧sort metadata。表示/cache metadataとは別owner。
    pub(crate) sort_metas: Vec<crate::settings::ListingSortMetadata>,
    /// 走査時の除外に、ここで適用した同名 filter の件数を足した最終内訳。
    /// 走査側と filter 側の両方を見られるのはこの関数だけなので、集計者もここに置く。
    /// 呼び出し側で数え直すと本体一覧と remote 一覧で違う数が出る。
    pub(crate) omitted: OmittedFolderEntryCounts,
    pub(crate) video_thumb_overrides: Vec<(PathBuf, PathBuf)>,
    pub(crate) folder_count: usize,
    pub(crate) media_count: usize,
    pub(crate) folder_sort: crate::settings::SortOrder,
    pub(crate) sort_ms: f64,
    pub(crate) duplicate_filter_ms: f64,
    pub(crate) rating_order: FolderRatingOrder,
}

pub(crate) enum FolderRatingOrder {
    Standard,
    Prepared {
        cache: std::collections::HashMap<usize, u8>,
        write_generation: u64,
        requested_count: usize,
        read_ms: f64,
    },
    Failed,
}

/// Cross-folder sidecar discovery result for aggregate views.
///
/// Keys are normalized full video paths. A basename/stem key is intentionally never emitted:
/// aggregate views may contain the same filename from unrelated physical parents.
#[derive(Default)]
pub(crate) struct AggregateVideoSidecars {
    pub(crate) by_video_path: std::collections::HashMap<String, PathBuf>,
    pub(crate) scanned_parents: usize,
    pub(crate) skipped_parents: usize,
    pub(crate) scan_errors: Vec<(PathBuf, String)>,
}

pub(crate) fn discover_aggregate_video_sidecars_while(
    settings: &crate::settings::Settings,
    videos: &[PathBuf],
    max_parent_scans: usize,
    mut keep_running: impl FnMut() -> bool,
) -> Option<AggregateVideoSidecars> {
    if !settings.skip_image_if_video_exists || !settings.video_thumb_use_sidecar_image {
        return Some(AggregateVideoSidecars::default());
    }
    let requested = videos
        .iter()
        .map(|path| crate::path_key::normalize_keep_drive(path))
        .collect::<std::collections::HashSet<_>>();
    if requested.is_empty() {
        return Some(AggregateVideoSidecars::default());
    }

    let mut parent_keys = std::collections::HashSet::new();
    let mut parents = Vec::new();
    let mut ordered = videos.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|path| {
        crate::folder_tree::SUPPORTED_AUDIO_EXTENSIONS
            .iter()
            .any(|ext| {
                path.extension()
                    .is_some_and(|value| value.eq_ignore_ascii_case(ext))
            })
    });
    for video in ordered {
        if !keep_running() {
            return None;
        }
        if let Some(parent) = video.parent() {
            let key = crate::path_key::normalize_keep_drive(parent);
            if parent_keys.insert(key) {
                parents.push(parent.to_path_buf());
            }
        }
    }

    let mut result = AggregateVideoSidecars {
        scanned_parents: parents.len().min(max_parent_scans),
        skipped_parents: parents.len().saturating_sub(max_parent_scans),
        ..AggregateVideoSidecars::default()
    };
    for parent in parents.into_iter().take(max_parent_scans) {
        if !keep_running() {
            return None;
        }
        let scan = match scan_directory_with_settings(&parent, settings) {
            Ok(scan) => scan,
            Err(error) => {
                result.scan_errors.push((parent, error.to_string()));
                continue;
            }
        };
        for (video, image) in scan.auxiliary_media.sidecars() {
            let key = crate::path_key::normalize_keep_drive(&video);
            if requested.contains(&key) {
                result.by_video_path.insert(key, image);
            }
        }
    }
    keep_running().then_some(result)
}

pub(crate) fn materialize_local_folder_listing(
    path: &std::path::Path,
    scan: ScannedDir,
    settings: &crate::settings::Settings,
) -> MaterializedFolderListing {
    materialize_local_folder_listing_with_order(
        path,
        scan,
        settings,
        crate::rating_sort::ListingOrderRequest::from_settings(settings),
        |keys| {
            crate::rating_db::RatingDb::open_readonly(crate::rating_db::RatingDb::db_path())
                .map_err(|error| error.to_string())?
                .get_many_complete(keys, 0)
                .map_err(|error| error.to_string())
        },
    )
}

pub(crate) fn materialize_local_folder_listing_with_order(
    path: &std::path::Path,
    scan: ScannedDir,
    settings: &crate::settings::Settings,
    request: crate::rating_sort::ListingOrderRequest,
    read_rating: impl FnOnce(&[String]) -> Result<crate::rating_db::CompleteRatingFacts, String>,
) -> MaterializedFolderListing {
    let mut scan = scan;
    let precise_stamps = std::mem::take(&mut scan.precise_stamps);
    project_scanned_directory(&mut scan, &settings.file_type_visibility);
    let mut auxiliary_media = scan.auxiliary_media;
    let mut omitted = scan.omitted;
    let mut folders = Vec::with_capacity(scan.folders.len());
    let mut metas = Vec::with_capacity(scan.folders.len());
    let mut sort_metas = Vec::with_capacity(scan.folders.len() + scan.all_media.len());
    for entry in scan.folders {
        folders.push(entry.item);
        metas.push(entry.display_meta);
        sort_metas.push(entry.sort_meta);
    }
    let mut all_media = scan.all_media;
    let compiled = crate::books::is_direct_book_folder(&settings.books_root_path(), path);
    if compiled {
        folders.clear();
        metas.clear();
        sort_metas.clear();
        all_media.retain(|entry| entry.kind == ScanMediaKind::Image);
    }
    let folder_count = folders.len();
    let media_count = all_media.len();

    let sort_started = std::time::Instant::now();
    let folder_sort = if compiled {
        crate::settings::SortOrder::Numeric
    } else {
        match request {
            crate::rating_sort::ListingOrderRequest::Standard(sort) => sort,
            crate::rating_sort::ListingOrderRequest::Rating(_) => {
                crate::settings::SortOrder::FileName
            }
        }
    };
    let media_sort = super::folder_media_sort_order(
        folder_sort,
        compiled,
        folders.is_empty(),
        all_media
            .iter()
            .all(|entry| entry.kind == ScanMediaKind::Image),
        settings.auto_fullscreen_image_folders_enabled(),
    );
    crate::grid_item::sort_folder_block_with_metadata(
        &mut folders,
        &mut metas,
        &mut sort_metas,
        folder_sort,
    );
    let mut keyed_media = all_media
        .into_iter()
        .map(|entry| {
            let name = entry
                .path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let key = media_sort.name_key(name);
            (entry, key)
        })
        .collect::<Vec<_>>();
    keyed_media.sort_by(|(left, left_key), (right, right_key)| {
        media_sort.compare_listing_keys(left_key, left.sort_meta, right_key, right.sort_meta)
    });
    let mut all_media = keyed_media
        .into_iter()
        .map(|(entry, _)| entry)
        .collect::<Vec<_>>();
    let mut keyed_auxiliary = auxiliary_media
        .entries
        .into_iter()
        .map(|entry| {
            let name = entry
                .path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let key = media_sort.name_key(name);
            (entry, key)
        })
        .collect::<Vec<_>>();
    keyed_auxiliary.sort_by(|(left, left_key), (right, right_key)| {
        media_sort.compare_listing_keys(left_key, left.sort_meta, right_key, right.sort_meta)
    });
    auxiliary_media.entries = keyed_auxiliary
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();
    let sort_ms = sort_started.elapsed().as_secs_f64() * 1000.0;

    let duplicate_started = std::time::Instant::now();
    let mut same_name = 0usize;
    if settings.skip_zip_if_folder_exists {
        same_name = same_name.saturating_add(filter_virtual_folder_duplicates_with_sort_metadata(
            &mut folders,
            &mut metas,
            &mut sort_metas,
        ));
    }
    if settings.skip_archive_if_zip_exists {
        same_name =
            same_name.saturating_add(filter_convertible_archive_duplicates_with_sort_metadata(
                &mut folders,
                &mut metas,
                &mut sort_metas,
            ));
    }
    if settings.skip_epub_if_pdf_exists {
        same_name = same_name.saturating_add(filter_epub_pdf_duplicates_with_sort_metadata(
            &mut folders,
            &mut metas,
            &mut sort_metas,
        ));
    }
    let video_thumb_overrides =
        if settings.skip_image_if_video_exists && settings.video_thumb_use_sidecar_image {
            let surviving_sources = all_media
                .iter()
                .filter(|entry| matches!(entry.kind, ScanMediaKind::Video | ScanMediaKind::Audio))
                .map(|entry| &entry.path)
                .collect::<std::collections::HashSet<_>>();
            auxiliary_media
                .sidecars()
                .into_iter()
                .filter(|(source, _)| surviving_sources.contains(source))
                .collect()
        } else {
            Vec::new()
        };
    if settings.skip_image_if_video_exists {
        let filtered = filter_video_image_duplicates(&mut all_media, false);
        same_name = same_name.saturating_add(filtered.omitted);
    }
    if settings.skip_duplicate_images {
        same_name = same_name.saturating_add(filter_image_ext_duplicates(
            &mut all_media,
            &settings.image_ext_priority,
        ));
    }
    omitted.same_name = same_name;
    let duplicate_filter_ms = duplicate_started.elapsed().as_secs_f64() * 1000.0;

    let mut items = folders;
    for entry in all_media {
        items.push(match entry.kind {
            ScanMediaKind::Image => GridItem::Image(entry.path),
            ScanMediaKind::Video => GridItem::Video(entry.path),
            ScanMediaKind::Audio => GridItem::Audio(entry.path),
        });
        metas.push(Some((entry.mtime, entry.file_size)));
        sort_metas.push(entry.sort_meta);
    }
    // Candidate exclusion is complete. Rating facts are read once here, before
    // the final arrangement and before any index (including video indices) is derived.
    let rating_order = if let crate::rating_sort::ListingOrderRequest::Rating(spec) = request {
        if compiled || media_sort != folder_sort {
            crate::grid_item::arrange_grid_items_with_sort_metadata(
                &mut items,
                &mut metas,
                &mut sort_metas,
                &settings.grid_display_order,
                Some(media_sort),
            );
            FolderRatingOrder::Standard
        } else {
            let item_keys = items
                .iter()
                .map(crate::rating_sort::physical_item_key)
                .collect::<Vec<_>>();
            let requested = item_keys
                .iter()
                .filter_map(Clone::clone)
                .collect::<Vec<_>>();
            let read_started = std::time::Instant::now();
            let read_result = read_rating(&requested);
            let read_ms = read_started.elapsed().as_secs_f64() * 1000.0;
            match read_result {
                Ok(facts) => {
                    let mut rating_keys = item_keys
                        .iter()
                        .map(|key| match key {
                            Some(key) => facts.key_for_requested(key),
                            None => crate::rating_sort::RatingSortKey::Unsupported,
                        })
                        .collect::<Vec<_>>();
                    crate::grid_item::arrange_grid_items_with_rating_keys(
                        &mut items,
                        &mut metas,
                        &mut sort_metas,
                        &mut rating_keys,
                        &settings.grid_display_order,
                        spec,
                    );
                    let values = facts
                        .iter_requested_ratings()
                        .map(|(key, stars)| (key.to_owned(), stars))
                        .collect::<std::collections::HashMap<_, _>>();
                    let cache = items
                        .iter()
                        .enumerate()
                        .filter_map(|(idx, item)| {
                            crate::rating_sort::physical_item_key(item)
                                .map(|key| (idx, values[&key]))
                        })
                        .collect();
                    FolderRatingOrder::Prepared {
                        cache,
                        write_generation: facts.write_generation(),
                        requested_count: requested.len(),
                        read_ms,
                    }
                }
                Err(_) => {
                    crate::grid_item::arrange_grid_items_with_sort_metadata(
                        &mut items,
                        &mut metas,
                        &mut sort_metas,
                        &settings.grid_display_order,
                        Some(crate::settings::SortOrder::FileName),
                    );
                    FolderRatingOrder::Failed
                }
            }
        }
    } else {
        crate::grid_item::arrange_grid_items_with_sort_metadata(
            &mut items,
            &mut metas,
            &mut sort_metas,
            &settings.grid_display_order,
            Some(media_sort),
        );
        FolderRatingOrder::Standard
    };

    let row_record = std::sync::Arc::new(FolderRowRecord::from_projection(
        path,
        &items,
        &metas,
        &precise_stamps,
        &video_thumb_overrides,
    ));
    MaterializedFolderListing {
        row_record,
        items,
        metas,
        sort_metas,
        omitted,
        video_thumb_overrides,
        folder_count,
        media_count,
        folder_sort,
        sort_ms,
        duplicate_filter_ms,
        rating_order,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ImageFolderPageCountOptions {
    pub(crate) file_type_visibility: crate::file_type_visibility::FileTypeVisibility,
    pub(crate) include_convertible_archives: bool,
    pub(crate) include_epub: bool,
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
        file_type_visibility: settings.file_type_visibility.clone(),
        include_convertible_archives: !settings.archive_file_handling_ignores_convertible(),
        include_epub: !settings.epub_file_handling_ignores_epub(),
        show_hidden_files: settings.show_hidden_files,
        skip_duplicate_images: settings.skip_duplicate_images,
        image_ext_priority: settings.image_ext_priority.clone(),
        fingerprint: image_page_recognition_fingerprint(settings),
    }
}

/// ZIP と画像フォルダのページ数 cache が共有する画像認識規則 fingerprint。
/// ネイティブ対応拡張子、Susie の有効/無効と対応拡張子集合も identity に含め、
/// 形式追加やプラグイン構成変更後に古いページ数を恒久再利用しない。
pub(crate) fn image_page_recognition_fingerprint(settings: &crate::settings::Settings) -> i64 {
    image_page_recognition_fingerprint_with_native_extensions(
        settings,
        crate::folder_tree::SUPPORTED_EXTENSIONS,
    )
}

fn image_page_recognition_fingerprint_with_native_extensions(
    settings: &crate::settings::Settings,
    native_extensions: &[&str],
) -> i64 {
    // 永続 cache 用の決定的 FNV-1a。拡張子以外の判定規則を変えたときは version bytes を上げる。
    let mut hash = 0xcbf29ce484222325u64;
    let mut mix = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    mix(b"image-folder-page-count-v2\0");
    mix(&settings.file_type_visibility.fingerprint().to_le_bytes());
    mix(&[u8::from(settings.skip_image_if_video_exists)]);
    mix(&[u8::from(settings.skip_zip_if_folder_exists)]);
    mix(&[u8::from(settings.skip_archive_if_zip_exists)]);
    mix(&[u8::from(settings.skip_epub_if_pdf_exists)]);
    mix(&[u8::from(settings.show_hidden_files)]);
    mix(&[u8::from(settings.skip_duplicate_images)]);
    mix(&[u8::from(
        !settings.archive_file_handling_ignores_convertible(),
    )]);
    mix(&[u8::from(!settings.epub_file_handling_ignores_epub())]);
    mix(&[u8::from(settings.susie_enabled)]);
    // Use the scanner's native list, independently of the user's saved priority list.
    // New native formats must invalidate cached counts even if priorities already contain them.
    mix(b"native-image-extensions\0");
    for extension in native_extensions {
        mix(extension.as_bytes());
        mix(&[0]);
    }
    mix(b"image-extension-priority\0");
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
        options.include_epub,
        options.show_hidden_files,
        &options.file_type_visibility,
        None,
    )?;
    project_scanned_directory(&mut scan, &options.file_type_visibility);
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
pub(crate) fn is_image_only_book_contents(
    has_container: bool,
    all_media: &[ScannedMediaEntry],
) -> bool {
    !has_container
        && !all_media.is_empty()
        && all_media
            .iter()
            .all(|entry| entry.kind == ScanMediaKind::Image)
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
    scan_directory_with_convertible_archives(path, true, true, false).unwrap_or_else(|_| {
        ScannedDir {
            precise_stamps: Default::default(),
            reconcile: None,
            complete_audio_inventory: None,
            folders: Vec::new(),
            all_media: Vec::new(),
            omitted: OmittedFolderEntryCounts::default(),
            auxiliary_media: Default::default(),
        }
    })
}

#[cfg(test)]
thread_local! {
    static CALLER_SCAN_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn caller_scan_count_for_test() -> usize {
    CALLER_SCAN_COUNT.with(std::cell::Cell::get)
}

pub(crate) fn scan_directory_with_settings(
    path: &std::path::Path,
    settings: &crate::settings::Settings,
) -> std::io::Result<ScannedDir> {
    #[cfg(test)]
    CALLER_SCAN_COUNT.with(|count| count.set(count.get() + 1));
    scan_directory_with_visibility_cancel(
        path,
        !settings.archive_file_handling_ignores_convertible(),
        !settings.epub_file_handling_ignores_epub(),
        settings.show_hidden_files,
        &settings.file_type_visibility,
        None,
    )
}

/// Project recognized candidates before duplicate winners and book recognition are computed.
/// Directories are structural passages and always survive the global policy.
pub(crate) fn project_scanned_directory(
    scan: &mut ScannedDir,
    policy: &crate::file_type_visibility::FileTypeVisibility,
) {
    let before = scan.folders.len() + scan.all_media.len();
    scan.folders
        .retain(|entry| policy.allows_grid_item(&entry.item));
    scan.all_media
        .retain(|entry| policy.allows_physical_file(&entry.path));
    scan.omitted.file_type = scan
        .omitted
        .file_type
        .saturating_add(before.saturating_sub(scan.folders.len() + scan.all_media.len()));
}

pub(crate) fn discover_scanned_video_sidecars(
    media: &[ScannedMediaEntry],
) -> Vec<(PathBuf, PathBuf)> {
    let mut candidates = media.to_vec();
    filter_video_image_duplicates(&mut candidates, true).sidecars
}

pub(crate) fn scan_directory_with_visibility_cancel(
    path: &std::path::Path,
    include_convertible_archives: bool,
    include_epub: bool,
    show_hidden_files: bool,
    policy: &crate::file_type_visibility::FileTypeVisibility,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> std::io::Result<ScannedDir> {
    scan_directory_entries(
        std::fs::read_dir(path)?,
        include_convertible_archives,
        include_epub,
        show_hidden_files,
        policy,
        cancel,
    )
}

pub(crate) fn scan_directory_with_convertible_archives(
    path: &std::path::Path,
    include_convertible_archives: bool,
    include_epub: bool,
    show_hidden_files: bool,
) -> std::io::Result<ScannedDir> {
    scan_directory_with_convertible_archives_cancel(
        path,
        include_convertible_archives,
        include_epub,
        show_hidden_files,
        None,
    )
}

pub(crate) fn scan_directory_with_convertible_archives_cancel(
    path: &std::path::Path,
    include_convertible_archives: bool,
    include_epub: bool,
    show_hidden_files: bool,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> std::io::Result<ScannedDir> {
    let entries = std::fs::read_dir(path)?;
    scan_directory_entries(
        entries,
        include_convertible_archives,
        include_epub,
        show_hidden_files,
        &crate::file_type_visibility::FileTypeVisibility::default(),
        cancel,
    )
}

fn scan_directory_entries<I>(
    entries: I,
    include_convertible_archives: bool,
    include_epub: bool,
    show_hidden_files: bool,
    policy: &crate::file_type_visibility::FileTypeVisibility,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> std::io::Result<ScannedDir>
where
    I: IntoIterator<Item = std::io::Result<std::fs::DirEntry>>,
{
    let mut precise_stamps = std::collections::HashMap::new();
    let mut folders: Vec<ScannedFolderEntry> = Vec::new();
    let mut all_media: Vec<ScannedMediaEntry> = Vec::new();
    let mut omitted = OmittedFolderEntryCounts::default();
    let mut complete_audio_inventory = Some(std::collections::HashSet::new());
    let mut entry_file_names_ci: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for entry in entries {
        if cancel.is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "folder scan cancelled",
            ));
        }
        // A ReadDir can fail after yielding some entries (for example when a network share
        // disconnects during FindNextFile). Treating that as a successful partial snapshot lets
        // catalog delete_missing remove rows for every unseen tail entry, so discard the whole
        // scan instead.
        let entry = entry?;
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
        // file_type() は FindFirstFile のキャッシュ読み (syscall なし)。
        // metadata() も同様にキャッシュから返るが、失敗しても fallback 0 で続行する。
        let kind = match entry.file_type() {
            Ok(ft) => crate::fs_entry::classify_dir_entry(&entry, &ft),
            Err(_) => {
                complete_audio_inventory = None;
                crate::fs_entry::DirEntryKind::Other
            }
        };
        let entry_name = entry.file_name().to_string_lossy().to_lowercase();
        // OS 由来の付随ファイルはどの経路で落ちても `system` に寄せる。分類だけで、
        // 一覧へ出す / 出さないの判定は一切変えない。
        if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mp3"))
        {
            if let Some(inventory) = complete_audio_inventory.as_mut() {
                inventory.insert(entry_name.clone());
            }
        }
        let is_system_file = is_system_metadata_name(&entry_name);
        if kind.is_directory() || policy.allows_physical_file(&entry.path()) {
            entry_file_names_ci.insert(entry_name);
        }
        if crate::fs_entry::should_hide_fs_entry(&entry, show_hidden_files) {
            if is_system_file {
                omitted.system = omitted.system.saturating_add(1);
            } else {
                omitted.hidden = omitted.hidden.saturating_add(1);
            }
            continue;
        }
        let p = entry.path();
        if kind.is_directory() {
            if crate::video::upscale::paths::has_work_dir_suffix(&p) {
                continue;
            }
            let meta = entry.metadata().ok();
            precise_stamps.insert(
                folder_scan_path_key(&p),
                meta.as_ref()
                    .and_then(|m| m.modified().ok().map(|modified| (modified, m.len()))),
            );
            let mtime = meta
                .as_ref()
                .map_or(0, |m| crate::ui_helpers::mtime_secs(m));
            folders.push(ScannedFolderEntry {
                item: GridItem::Folder(p),
                display_meta: Some((mtime, 0)),
                sort_meta: crate::settings::ListingSortMetadata::new(mtime, None),
            });
        } else if crate::folder_tree::is_apple_double(&p) {
            // macOS/iPhone AppleDouble メタデータ - スキップ
            omitted.system = omitted.system.saturating_add(1);
        } else if kind.is_file()
            && let Some(ext) = p.extension().and_then(|e| e.to_str())
        {
            let ext_lower = ext.to_ascii_lowercase();
            let meta = entry.metadata().ok();
            precise_stamps.insert(
                folder_scan_path_key(&p),
                meta.as_ref()
                    .and_then(|m| m.modified().ok().map(|modified| (modified, m.len()))),
            );
            let mtime = meta
                .as_ref()
                .map_or(0, |m| crate::ui_helpers::mtime_secs(m));
            let file_size = meta.as_ref().map_or(0, |m| m.len() as i64);
            let sort_meta = crate::settings::ListingSortMetadata::new(
                mtime,
                meta.as_ref().map(|m| m.len() as i64),
            );
            if crate::folder_tree::is_recognized_image_ext(&ext_lower) {
                all_media.push(ScannedMediaEntry {
                    path: p,
                    kind: ScanMediaKind::Image,
                    mtime,
                    file_size,
                    sort_meta,
                });
            } else if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
                all_media.push(ScannedMediaEntry {
                    path: p,
                    kind: ScanMediaKind::Video,
                    mtime,
                    file_size,
                    sort_meta,
                });
            } else if crate::folder_tree::is_audio_ext(&ext_lower) {
                all_media.push(ScannedMediaEntry {
                    path: p,
                    kind: ScanMediaKind::Audio,
                    mtime,
                    file_size,
                    sort_meta,
                });
            } else if crate::folder_tree::is_zip_extension(&ext_lower) {
                folders.push(ScannedFolderEntry {
                    item: GridItem::ZipFile(p),
                    display_meta: Some((mtime, file_size)),
                    sort_meta,
                });
            } else if crate::folder_tree::is_paged_document_path(&p) {
                if include_epub || ext_lower != "epub" {
                    folders.push(ScannedFolderEntry {
                        item: GridItem::PdfFile(p),
                        display_meta: Some((mtime, file_size)),
                        sort_meta,
                    });
                } else {
                    omitted.ignored_epub = omitted.ignored_epub.saturating_add(1);
                }
            } else if let Some(fmt) =
                crate::archive_converter::ArchiveFormat::from_extension(&ext_lower)
            {
                if include_convertible_archives {
                    folders.push(ScannedFolderEntry {
                        item: GridItem::ConvertibleArchive {
                            path: p,
                            format: fmt,
                        },
                        display_meta: Some((mtime, file_size)),
                        sort_meta,
                    });
                } else {
                    omitted.ignored_archive = omitted.ignored_archive.saturating_add(1);
                }
            } else if is_system_file {
                omitted.system = omitted.system.saturating_add(1);
            } else {
                omitted.unsupported = omitted.unsupported.saturating_add(1);
            }
        } else if kind.is_file() {
            // 拡張子のない通常ファイルも、一覧対象外のファイルとして内訳に含める。
            if is_system_file {
                omitted.system = omitted.system.saturating_add(1);
            } else {
                omitted.unsupported = omitted.unsupported.saturating_add(1);
            }
        }
    }
    let auxiliary_media =
        AuxiliaryMediaCandidates::capture(&all_media, policy, &entry_file_names_ci);
    let mut scan = ScannedDir {
        precise_stamps,
        reconcile: None,
        complete_audio_inventory,
        folders,
        all_media,
        omitted,
        auxiliary_media,
    };
    project_scanned_directory(&mut scan, policy);
    filter_upscaled_video_pairs_fast(&mut scan.all_media, &entry_file_names_ci);
    Ok(scan)
}

#[derive(Debug, Default)]
pub(crate) struct VideoImageDuplicateFilterResult {
    pub(crate) sidecars: Vec<(PathBuf, PathBuf)>,
    pub(crate) omitted: usize,
}

pub(super) fn filter_upscaled_video_pairs_fast(
    all_media: &mut Vec<ScannedMediaEntry>,
    entry_file_names_ci: &std::collections::HashSet<String>,
) {
    let mut source_stem_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for entry in all_media.iter() {
        if entry.kind != ScanMediaKind::Video || is_miv_upscaled_derivative(&entry.path) {
            continue;
        }
        if let Some(stem) = file_stem_ci(&entry.path) {
            *source_stem_counts.entry(stem).or_insert(0) += 1;
        }
    }

    let derivative_source_stems: std::collections::HashSet<String> = all_media
        .iter()
        .filter(|entry| entry.kind == ScanMediaKind::Video)
        .filter_map(|entry| {
            source_stem_for_miv_upscaled_derivative(&entry.path, entry_file_names_ci)
        })
        .filter(|source_stem| source_stem_counts.get(source_stem).copied() == Some(1))
        .collect();

    if derivative_source_stems.is_empty() {
        return;
    }

    all_media.retain(|entry| {
        if is_miv_upscaled_derivative(&entry.path) {
            return true;
        }
        // 音声はアップスケール動画の companion ではない (例: song.mp3 と song.miv.mkv は
        // 別メディア) ので、同一 stem でも常に残す (Codex P2)。元動画と companion 画像
        // (sidecar サムネ等) は従来どおり同一 stem のとき隠す。
        if entry.kind == ScanMediaKind::Audio {
            return true;
        }
        file_stem_ci(&entry.path).is_none_or(|stem| !derivative_source_stems.contains(&stem))
    });
}

/// 同じ物理フォルダ内の動画と同名 stem の画像を一覧から除外する。
///
/// `media` は必ず 1 フォルダ分だけを渡す。フラットビュー全体を渡すと、別フォルダの
/// 同名ファイルまで衝突する。`use_sidecar` が有効なら、除外画像を動画サムネイルへ
/// 引き継ぐため `(video_path, image_path)` を返す。画像の除外自体は設定に関係なく行う。
pub(crate) fn filter_video_image_duplicates(
    media: &mut Vec<ScannedMediaEntry>,
    use_sidecar: bool,
) -> VideoImageDuplicateFilterResult {
    let mut videos_by_stem: std::collections::HashMap<String, Vec<PathBuf>> =
        std::collections::HashMap::new();
    for entry in media.iter() {
        if matches!(entry.kind, ScanMediaKind::Video | ScanMediaKind::Audio) {
            videos_by_stem
                .entry(super::stem_lower(&entry.path))
                .or_default()
                .push(entry.path.clone());
        }
    }
    if videos_by_stem.is_empty() {
        return VideoImageDuplicateFilterResult::default();
    }

    let mut sidecars = Vec::new();
    if use_sidecar {
        for entry in media.iter() {
            if entry.kind != ScanMediaKind::Image {
                continue;
            }
            if let Some(videos) = videos_by_stem.get(&super::stem_lower(&entry.path)) {
                sidecars.extend(
                    videos
                        .iter()
                        .cloned()
                        .map(|video| (video, entry.path.clone())),
                );
            }
        }
    }

    let before = media.len();
    media.retain(|entry| {
        entry.kind != ScanMediaKind::Image
            || !videos_by_stem.contains_key(&super::stem_lower(&entry.path))
    });
    VideoImageDuplicateFilterResult {
        sidecars,
        omitted: before.saturating_sub(media.len()),
    }
}

/// ZIP/PDF/対応アーカイブと同名の実フォルダがあれば、実フォルダを一覧の正本にする。
pub(crate) fn filter_virtual_folder_duplicates(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
) -> usize {
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
    keep.iter().filter(|keep| !**keep).count()
}

fn filter_virtual_folder_duplicates_with_sort_metadata(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
    sort_metas: &mut Vec<crate::settings::ListingSortMetadata>,
) -> usize {
    assert_eq!(folders.len(), folder_metas.len());
    assert_eq!(folders.len(), sort_metas.len());
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
    let mut iter = keep.iter();
    sort_metas.retain(|_| *iter.next().unwrap());
    keep.iter().filter(|keep| !**keep).count()
}

/// 同名の ZIP/CBZ があれば、変換元になる RAR/7z/LZH 等を一覧から除外する。
pub(crate) fn filter_convertible_archive_duplicates(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
) -> usize {
    let zip_stems: std::collections::HashSet<String> = folders
        .iter()
        .filter_map(|item| match item {
            GridItem::ZipFile(path) => Some(super::stem_lower(path)),
            _ => None,
        })
        .collect();
    if zip_stems.is_empty() {
        return 0;
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
    keep.iter().filter(|keep| !**keep).count()
}

fn filter_convertible_archive_duplicates_with_sort_metadata(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
    sort_metas: &mut Vec<crate::settings::ListingSortMetadata>,
) -> usize {
    assert_eq!(folders.len(), folder_metas.len());
    assert_eq!(folders.len(), sort_metas.len());
    let zip_stems: std::collections::HashSet<String> = folders
        .iter()
        .filter_map(|item| match item {
            GridItem::ZipFile(path) => Some(super::stem_lower(path)),
            _ => None,
        })
        .collect();
    if zip_stems.is_empty() {
        return 0;
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
    let mut iter = keep.iter();
    sort_metas.retain(|_| *iter.next().unwrap());
    keep.iter().filter(|keep| !**keep).count()
}

/// 同じ一覧に同名 PDF がある場合だけ EPUB を隠す。双方とも論理パスを保持する。
pub(crate) fn filter_epub_pdf_duplicates(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
) -> usize {
    let mut sort_metas = vec![crate::settings::ListingSortMetadata::new(0, None); folders.len()];
    filter_epub_pdf_duplicates_with_sort_metadata(folders, folder_metas, &mut sort_metas)
}

/// 通常一覧では整列用 metadata も同じ位置で間引く。
fn filter_epub_pdf_duplicates_with_sort_metadata(
    folders: &mut Vec<GridItem>,
    folder_metas: &mut Vec<Option<(i64, i64)>>,
    sort_metas: &mut Vec<crate::settings::ListingSortMetadata>,
) -> usize {
    assert_eq!(folders.len(), folder_metas.len());
    assert_eq!(folders.len(), sort_metas.len());
    let pdf_stems: std::collections::HashSet<String> = folders
        .iter()
        .filter_map(|item| match item {
            GridItem::PdfFile(path)
                if path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")) =>
            {
                Some(super::stem_lower(path))
            }
            _ => None,
        })
        .collect();
    if pdf_stems.is_empty() {
        return 0;
    }
    let keep: Vec<bool> = folders
        .iter()
        .map(|item| match item {
            GridItem::PdfFile(path)
                if path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("epub")) =>
            {
                !pdf_stems.contains(&super::stem_lower(path))
            }
            _ => true,
        })
        .collect();
    let mut iter = keep.iter();
    folders.retain(|_| *iter.next().unwrap());
    let mut iter = keep.iter();
    folder_metas.retain(|_| *iter.next().unwrap());
    let mut iter = keep.iter();
    sort_metas.retain(|_| *iter.next().unwrap());
    keep.iter().filter(|keep| !**keep).count()
}

/// 同名ステムの画像を拡張子優先順で 1 件へ絞る。一覧と画像フォルダのページ数で共有する。
pub(crate) fn filter_image_ext_duplicates(
    all_media: &mut Vec<ScannedMediaEntry>,
    priority: &[String],
) -> usize {
    let mut best: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for (index, entry) in all_media.iter().enumerate() {
        if entry.kind != ScanMediaKind::Image {
            continue;
        }
        let stem = super::stem_lower(&entry.path);
        let extension = entry
            .path
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
    for entry in all_media.iter() {
        if entry.kind == ScanMediaKind::Image {
            *stem_counts
                .entry(super::stem_lower(&entry.path))
                .or_insert(0) += 1;
        }
    }
    let keep_indices: std::collections::HashSet<usize> = best
        .iter()
        .filter(|(stem, _)| stem_counts.get(stem.as_str()).copied().unwrap_or(0) > 1)
        .map(|(_, &(_, index))| index)
        .collect();
    if keep_indices.is_empty() {
        return 0;
    }
    let before = all_media.len();
    let mut index = 0usize;
    all_media.retain(|entry| {
        let current = index;
        index += 1;
        if entry.kind != ScanMediaKind::Image {
            return true;
        }
        let stem = super::stem_lower(&entry.path);
        stem_counts.get(&stem).copied().unwrap_or(0) <= 1 || keep_indices.contains(&current)
    });
    before.saturating_sub(all_media.len())
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

/// Lossless, process-local physical row identity. Path hashing treats Windows
/// separators consistently without allocating or folding the displayed filename's case.
pub(crate) fn folder_scan_path_key(path: &std::path::Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    hash.finish()
}

#[derive(Clone, Debug)]
pub(crate) struct FolderScanRow {
    pub(crate) key: u64,
    pub(crate) parent_key: u64,
    kind: &'static str,
    size: i64,
    folder_mtime: i64,
    modified: Option<std::time::SystemTime>,
    auxiliary: Option<(u64, Option<(std::time::SystemTime, u64)>)>,
}

impl PartialEq for FolderScanRow {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.parent_key == other.parent_key
            && self.kind == other.kind
            && self.size == other.size
            && self.modified == other.modified
            && self.auxiliary == other.auxiliary
    }
}
impl Eq for FolderScanRow {}

#[derive(Debug)]
pub(crate) struct FolderRowRecord {
    pub(crate) rows: Vec<FolderScanRow>,
    pub(crate) known: bool,
}

#[cfg(test)]
thread_local! {
    static ROW_RECORD_DROPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn row_record_drops_for_test() -> usize {
    ROW_RECORD_DROPS.with(std::cell::Cell::get)
}

#[cfg(test)]
impl Drop for FolderRowRecord {
    fn drop(&mut self) {
        ROW_RECORD_DROPS.with(|count| count.set(count.get() + 1));
    }
}

impl FolderRowRecord {
    fn from_projection(
        physical_source: &std::path::Path,
        items: &[GridItem],
        metas: &[Option<(i64, i64)>],
        stamps: &std::collections::HashMap<u64, Option<(std::time::SystemTime, u64)>>,
        sidecars: &[(PathBuf, PathBuf)],
    ) -> Self {
        let auxiliaries = sidecars
            .iter()
            .map(|(source, image)| (source, image))
            .collect::<std::collections::HashMap<_, _>>();
        let mut known = true;
        // Production ScannedDir entries come from one read_dir(physical_source).
        // Projection only excludes/reorders those direct children; it never joins sources.
        // Reuse that boundary instead of parsing the same parent for every row on UI load.
        let parent_key = folder_scan_path_key(physical_source);
        let mut rows = Vec::with_capacity(items.len());
        rows.extend(items.iter().zip(metas).filter_map(|(item, meta)| {
            let (path, kind) = match item {
                GridItem::Folder(p) => (p, "folder"),
                GridItem::ZipFile(p) => (p, "zip"),
                GridItem::PdfFile(p) => (p, "pdf"),
                GridItem::ConvertibleArchive { path, .. } => (path, "archive"),
                GridItem::Image(p) => (p, "image"),
                GridItem::Video(p) => (p, "video"),
                GridItem::Audio(p) => (p, "audio"),
                _ => {
                    known = false;
                    return None;
                }
            };
            let key = folder_scan_path_key(path);
            let modified = if kind == "folder" {
                None
            } else {
                stamps
                    .get(&key)
                    .copied()
                    .map(|stamp| stamp.map(|(modified, _)| modified))
                    .unwrap_or_else(|| {
                        #[cfg(test)]
                        {
                            meta.map(|(seconds, _)| {
                                std::time::SystemTime::UNIX_EPOCH
                                    + std::time::Duration::from_secs(seconds.max(0) as u64)
                            })
                        }
                        #[cfg(not(test))]
                        {
                            None
                        }
                    })
            };
            if kind != "folder" && modified.is_none() {
                known = false;
            }
            let auxiliary = auxiliaries.get(path).map(|aux| {
                let aux_key = folder_scan_path_key(aux);
                let modified = stamps.get(&aux_key).copied().flatten();
                if modified.is_none() {
                    known = false;
                }
                (aux_key, modified)
            });
            Some(FolderScanRow {
                key,
                parent_key,
                kind,
                size: meta.map_or(0, |(_, size)| size),
                folder_mtime: meta.map_or(0, |(mtime, _)| mtime),
                modified,
                auxiliary,
            })
        }));
        Self { rows, known }
    }

    pub(crate) fn preserve_directory_sort_stamps(&self, scan: &mut ScannedDir) {
        let stamps = self
            .rows
            .iter()
            .filter(|row| row.kind == "folder")
            .map(|row| (row.key, row.folder_mtime))
            .collect::<std::collections::HashMap<_, _>>();
        for entry in &mut scan.folders {
            if let GridItem::Folder(path) = &entry.item
                && let Some(mtime) = stamps.get(&folder_scan_path_key(path))
            {
                entry.sort_meta.mtime = *mtime;
                let size = entry.display_meta.map_or(0, |(_, size)| size);
                entry.display_meta = Some((*mtime, size));
            }
        }
    }

    pub(crate) fn same_remaining_rows<'a>(
        &self,
        previous: &Self,
        removed: impl Iterator<Item = &'a PathBuf>,
        scope: &std::path::Path,
    ) -> bool {
        if !self.known || !previous.known {
            return false;
        }
        let removed = removed
            .map(|path| folder_scan_path_key(path))
            .collect::<std::collections::HashSet<_>>();
        let parent = folder_scan_path_key(scope);
        self.rows.iter().eq(previous
            .rows
            .iter()
            .filter(|row| row.parent_key == parent && !removed.contains(&row.key)))
    }
}

pub(crate) struct FolderScanReconcile {
    pub(crate) observed_signature: FolderScanSignature,
    pub(crate) parent_mtime: std::time::SystemTime,
    pub(crate) materialized: Box<MaterializedFolderListing>,
}

impl ScannedDir {
    /// The rescan worker duplicates raw scan input; UI never clones the listing.
    pub(crate) fn clone_for_projection(&self) -> Self {
        Self {
            precise_stamps: self.precise_stamps.clone(),
            reconcile: None,
            complete_audio_inventory: None,
            folders: self.folders.clone(),
            all_media: self.all_media.clone(),
            omitted: self.omitted,
            auxiliary_media: self.auxiliary_media.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FolderScanSignature {
    full: u64,
    listing: u64,
}

impl PartialEq for FolderScanSignature {
    fn eq(&self, other: &Self) -> bool {
        self.full == other.full
    }
}

impl Eq for FolderScanSignature {}

impl FolderScanSignature {
    pub(crate) fn same_listing(self, other: Self) -> bool {
        self.listing == other.listing
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: u64) -> Self {
        Self {
            full: value,
            listing: value,
        }
    }
}

/// `ScannedDir` の内容シグネチャ (path + mtime + size + 種別) をハッシュ化する。
/// フォーカス復帰時の差分判定用。`read_dir` の返却順は NTFS で保証されないので
/// 並び順非依存にするため path で明示的にソートしてからハッシュする。
/// プロセス内比較専用 (DefaultHasher は Rust バージョン間で安定でないため永続化しない)。
///
/// 従来の秒精度の tuple stream を維持する。同秒・同サイズの書換えはこの signature では
/// 区別しない。§1.375 の監視更新は別の FolderRowRecord が取得済み modified() を使って
/// 精密比較するため、この既存 signature の精度へ依存しない。
pub(crate) fn signature_from_scan(scan: &ScannedDir) -> FolderScanSignature {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut entries: Vec<(&std::ffi::OsStr, i64, i64, &'static str)> =
        Vec::with_capacity(scan.folders.len() + scan.all_media.len());
    for entry in &scan.folders {
        let (path, kind) = match &entry.item {
            GridItem::Folder(p) => (p.as_os_str(), "folder"),
            GridItem::ZipFile(p) => (p.as_os_str(), "zip"),
            GridItem::PdfFile(p) => (p.as_os_str(), "pdf"),
            GridItem::ConvertibleArchive { path, .. } => (path.as_os_str(), "archive"),
            _ => continue,
        };
        let (mtime, size) = entry.display_meta.unwrap_or((0, 0));
        entries.push((path, mtime, size, kind));
    }
    for entry in &scan.all_media {
        let kind = match entry.kind {
            ScanMediaKind::Image => "image",
            ScanMediaKind::Video => "video",
            ScanMediaKind::Audio => "audio",
        };
        entries.push((entry.path.as_os_str(), entry.mtime, entry.file_size, kind));
    }
    let visible_source_stems = scan
        .all_media
        .iter()
        .filter(|entry| matches!(entry.kind, ScanMediaKind::Video | ScanMediaKind::Audio))
        .map(|entry| super::stem_lower(&entry.path))
        .collect::<std::collections::HashSet<_>>();
    let display_paths = scan
        .all_media
        .iter()
        .map(|entry| &entry.path)
        .collect::<std::collections::HashSet<_>>();
    for entry in &scan.auxiliary_media.entries {
        if entry.kind == ScanMediaKind::Image
            && visible_source_stems.contains(&super::stem_lower(&entry.path))
            && !display_paths.contains(&entry.path)
        {
            entries.push((
                entry.path.as_os_str(),
                entry.mtime,
                entry.file_size,
                "image",
            ));
        }
    }
    entries.sort();
    let mut hasher = DefaultHasher::new();
    let mut listing_hasher = DefaultHasher::new();
    entries.len().hash(&mut hasher);
    entries.len().hash(&mut listing_hasher);
    // `omitted` は意図的に含めない。この signature は「一覧を差し替える必要があるか」を
    // 判定するもので、`hidden` / `unsupported` には Explorer が書く Thumbs.db / desktop.ini
    // (どちらも hidden 属性) が入る。これを含めるとサムネイルキャッシュが書かれるたびに
    // items 差し替えと AI / PDF キャッシュ破棄が走り、この signature が防いでいるはずの
    // ちらつきを自ら起こす。チップは一覧を作った同じ走査から導出するので、表示中の一覧とは
    // 常に整合しており、実際に一覧が変わったときの再ロードで更新される。
    for e in &entries {
        e.hash(&mut hasher);
        let &(path, mtime, size, kind) = e;
        (path, if kind == "folder" { 0 } else { mtime }, size, kind).hash(&mut listing_hasher);
    }
    FolderScanSignature {
        full: hasher.finish(),
        listing: listing_hasher.finish(),
    }
}

#[cfg(test)]
pub(crate) mod signature_tests {
    use super::*;

    pub(crate) fn fixture(root: &std::path::Path) -> ScannedDir {
        let folders = [
            GridItem::Folder(root.join("child")),
            GridItem::ZipFile(root.join("book.zip")),
            GridItem::PdfFile(root.join("book.pdf")),
            GridItem::PdfFile(root.join("book.epub")),
            GridItem::ConvertibleArchive {
                path: root.join("book.rar"),
                format: crate::archive_converter::ArchiveFormat::Rar,
            },
        ]
        .into_iter()
        .map(|item| ScannedFolderEntry {
            display_meta: Some((
                10,
                if matches!(item, GridItem::Folder(_)) {
                    0
                } else {
                    20
                },
            )),
            sort_meta: crate::settings::ListingSortMetadata::new(10, None),
            item,
        })
        .collect();
        let all_media = [
            ScanMediaKind::Image,
            ScanMediaKind::Video,
            ScanMediaKind::Audio,
        ]
        .into_iter()
        .zip(["image.png", "video.mp4", "audio.mp3"])
        .map(|(kind, name)| ScannedMediaEntry {
            path: root.join(name),
            kind,
            mtime: 10,
            file_size: 20,
            sort_meta: crate::settings::ListingSortMetadata::new(10, Some(20)),
        })
        .collect();
        ScannedDir {
            precise_stamps: Default::default(),
            reconcile: None,
            complete_audio_inventory: None,
            folders,
            all_media,
            omitted: Default::default(),
            auxiliary_media: Default::default(),
        }
    }

    pub(crate) fn real_changes(root: &std::path::Path) -> Vec<(&'static str, ScannedDir)> {
        let mut cases = Vec::new();
        for (label, modify) in [
            ("file added", 0),
            ("file removed", 1),
            ("file renamed", 2),
            ("folder added", 3),
            ("folder removed", 4),
            ("folder renamed", 5),
            ("folder kind", 6),
            ("folder size", 7),
            ("media kind", 8),
        ] {
            let mut scan = fixture(root);
            match modify {
                0 => {
                    let mut entry = scan.all_media[0].clone();
                    entry.path = root.join("new.png");
                    scan.all_media.push(entry);
                }
                1 => {
                    scan.all_media.remove(0);
                }
                2 => scan.all_media[0].path = root.join("renamed.png"),
                3 => {
                    let mut entry = scan.folders[0].clone();
                    entry.item = GridItem::Folder(root.join("new_child"));
                    scan.folders.push(entry);
                }
                4 => {
                    scan.folders.remove(0);
                }
                5 => scan.folders[0].item = GridItem::Folder(root.join("renamed_child")),
                6 => scan.folders[0].item = GridItem::ZipFile(root.join("child")),
                7 => scan.folders[0].display_meta = Some((10, 1)),
                8 => scan.all_media[0].kind = ScanMediaKind::Video,
                _ => unreachable!(),
            }
            cases.push((label, scan));
        }
        for index in 1..5 {
            for size_change in [false, true] {
                let mut scan = fixture(root);
                scan.folders[index].display_meta =
                    Some(if size_change { (10, 21) } else { (11, 20) });
                cases.push(("archive/PDF/EPUB stamp", scan));
            }
        }
        for index in 0..3 {
            for size_change in [false, true] {
                let mut scan = fixture(root);
                if size_change {
                    scan.all_media[index].file_size += 1;
                } else {
                    scan.all_media[index].mtime += 1;
                }
                cases.push(("media stamp", scan));
            }
        }
        cases
    }

    #[test]
    fn hidden_video_sidecar_remains_in_focus_return_signature() {
        let mut scan = fixture(std::path::Path::new("sidecar_fixture"));
        let image = scan
            .all_media
            .iter()
            .find(|entry| entry.kind == ScanMediaKind::Image)
            .unwrap()
            .path
            .clone();
        let video = scan
            .all_media
            .iter()
            .find(|entry| entry.kind == ScanMediaKind::Video)
            .unwrap()
            .path
            .clone();
        let video_entry = scan
            .all_media
            .iter_mut()
            .find(|entry| entry.path == video)
            .unwrap();
        video_entry.path = image.with_extension("mp4");
        scan.auxiliary_media = AuxiliaryMediaCandidates::capture(
            &scan.all_media,
            &Default::default(),
            &Default::default(),
        );
        let mut policy = crate::file_type_visibility::FileTypeVisibility::default();
        policy
            .excluded_categories
            .insert(crate::file_type_visibility::FileTypeCategory::Image);
        project_scanned_directory(&mut scan, &policy);
        assert!(!scan.all_media.iter().any(|entry| entry.path == image));
        assert_eq!(
            scan.auxiliary_media
                .entries
                .iter()
                .filter(|entry| entry.kind == ScanMediaKind::Image)
                .count(),
            1
        );
        let before = signature_from_scan(&scan);
        project_scanned_directory(&mut scan, &policy);
        assert_eq!(
            before,
            signature_from_scan(&scan),
            "reprojection cannot duplicate auxiliary stamps"
        );
        scan.auxiliary_media
            .entries
            .iter_mut()
            .find(|entry| entry.kind == ScanMediaKind::Image)
            .unwrap()
            .mtime += 1;
        assert_ne!(before, signature_from_scan(&scan));
        assert!(!before.same_listing(signature_from_scan(&scan)));
    }

    #[test]
    fn signature_from_scan_splits_only_real_folder_mtimes() {
        let root = std::path::Path::new("signature_fixture");
        let scan = fixture(root);
        let initial = signature_from_scan(&scan);
        let mut changed = fixture(root);
        changed.folders[0].display_meta = Some((11, 0));
        let updated = signature_from_scan(&changed);
        assert_ne!(
            initial, updated,
            "normal signature equality must include folder mtime"
        );
        assert!(initial.same_listing(updated));
        for (label, scan) in real_changes(root) {
            let changed = signature_from_scan(&scan);
            assert_ne!(initial, changed, "{label}");
            assert!(!initial.same_listing(changed), "{label}");
        }
    }

    #[test]
    fn signature_from_scan_preserves_original_hash_and_order_independence() {
        use std::hash::{Hash, Hasher};
        let root = std::path::Path::new("signature_fixture");
        let mut scan = fixture(root);
        let signature = signature_from_scan(&scan);
        // Original signature's exact tuple stream, independent of the new implementation.
        let mut entries = vec![
            (root.join("child"), 10i64, 0i64, "folder"),
            (root.join("book.zip"), 10, 20, "zip"),
            (root.join("book.pdf"), 10, 20, "pdf"),
            (root.join("book.epub"), 10, 20, "pdf"),
            (root.join("book.rar"), 10, 20, "archive"),
            (root.join("image.png"), 10, 20, "image"),
            (root.join("video.mp4"), 10, 20, "video"),
            (root.join("audio.mp3"), 10, 20, "audio"),
        ];
        entries.sort();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        entries.len().hash(&mut hasher);
        for (path, mtime, size, kind) in entries {
            (path.as_os_str(), mtime, size, kind).hash(&mut hasher);
        }
        assert_eq!(signature.full, hasher.finish());
        scan.folders.reverse();
        scan.all_media.reverse();
        scan.omitted.unsupported = 99;
        let reordered = signature_from_scan(&scan);
        assert_eq!(signature, reordered);
        assert!(signature.same_listing(reordered));
    }
}

#[cfg(test)]
mod page_count_tests {
    use super::*;

    #[test]
    fn epub_is_paged_grid_item_and_same_name_pdf_wins_only_when_enabled() {
        let temp = tempfile::TempDir::new().unwrap();
        for name in [
            "Book.EPUB",
            "book.PDF",
            "other.epub",
            "comic.zip",
            "archive.rar",
        ] {
            std::fs::write(temp.path().join(name), b"source").unwrap();
        }
        let mut settings = crate::settings::Settings::default();
        let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
        assert!(scan.folders.iter().any(|entry| matches!(&entry.item, GridItem::PdfFile(path) if path.file_name().unwrap() == "Book.EPUB")));
        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
        assert_eq!(listing.omitted.same_name, 1);
        assert!(!listing.items.iter().any(|item| matches!(item, GridItem::PdfFile(path) if path.file_name().unwrap() == "Book.EPUB")));
        assert!(listing.items.iter().any(|item| matches!(item, GridItem::PdfFile(path) if path.file_name().unwrap() == "book.PDF")));
        assert!(listing.items.iter().any(|item| matches!(item, GridItem::PdfFile(path) if path.file_name().unwrap() == "other.epub")));
        assert!(
            listing
                .items
                .iter()
                .any(|item| matches!(item, GridItem::ZipFile(_)))
        );
        assert!(
            listing
                .items
                .iter()
                .any(|item| matches!(item, GridItem::ConvertibleArchive { .. }))
        );

        settings.skip_epub_if_pdf_exists = false;
        let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
        assert!(listing.items.iter().any(|item| matches!(item, GridItem::PdfFile(path) if path.file_name().unwrap() == "Book.EPUB")));
        assert!(listing.items.iter().any(|item| matches!(item, GridItem::PdfFile(path) if path.file_name().unwrap() == "book.PDF")));
    }

    fn options(skip_duplicate_images: bool) -> ImageFolderPageCountOptions {
        ImageFolderPageCountOptions {
            file_type_visibility: Default::default(),
            include_convertible_archives: true,
            include_epub: true,
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
    fn sibling_output_temp_is_absent_from_real_folder_scan() {
        let temp = tempfile::TempDir::new().unwrap();
        let shelf = temp.path().join("shelf");
        std::fs::create_dir(&shelf).unwrap();
        std::fs::write(shelf.join("book.pdf"), b"PDF").unwrap();
        let mut cache = crate::epub_cache::EpubCache::open_at(&temp.path().join("data")).unwrap();
        let reserved = cache
            .reserve_sibling_output(&shelf.join("new-book.pdf"))
            .unwrap();
        let _held_temp = reserved.create_file().unwrap();
        let token = "a".repeat(64);
        let old_temp = shelf.join(format!(".miv-part-{token}.pdf"));
        let new_temp = shelf.join(format!(".miv-part-{token}.tmp"));
        std::fs::write(&old_temp, b"temporary").unwrap();
        std::fs::write(&new_temp, b"temporary").unwrap();
        let scan = scan_directory_with_convertible_archives(&shelf, true, true, true).unwrap();
        assert_eq!(scan.folders.len(), 1);
        assert!(
            matches!(&scan.folders[0].item, GridItem::PdfFile(path) if path == &shelf.join("book.pdf"))
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
    fn omitted_counts_reuse_scan_results_and_separate_system_files() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("001.jpg"), b"jpg").unwrap();
        std::fs::write(temp.path().join("001.png"), b"png").unwrap();
        std::fs::write(temp.path().join("notes.txt"), b"text").unwrap();
        std::fs::write(temp.path().join("state"), b"extensionless").unwrap();
        std::fs::write(temp.path().join("._finder.jpg"), b"apple double").unwrap();
        std::fs::write(temp.path().join("Thumbs.db"), b"explorer cache").unwrap();
        std::fs::write(temp.path().join("desktop.ini"), b"explorer view").unwrap();

        // 内訳はこの既存 read_dir の結果から導出する。件数取得専用の再走査 API は持たない。
        let mut scan =
            scan_directory_with_convertible_archives(temp.path(), true, true, false).unwrap();
        scan.omitted.same_name =
            filter_image_ext_duplicates(&mut scan.all_media, &["png".to_owned(), "jpg".to_owned()]);

        assert_eq!(scan.omitted.same_name, 1);
        assert_eq!(scan.omitted.hidden, 0);
        assert_eq!(scan.omitted.unsupported, 2, "notes.txt と拡張子なし");
        assert_eq!(
            scan.omitted.system, 3,
            "AppleDouble / Thumbs.db / desktop.ini"
        );
        // 利用者のファイルは種類を問わず数え、OS 由来の 3 件だけを主数字から外す。
        assert_eq!(scan.omitted.primary_count(), 3);
        assert_eq!(scan.all_media.len(), 1);
    }

    #[test]
    fn materialized_listing_reports_scan_and_same_name_omissions_together() {
        // チップの数字は走査時の除外と同名 filter の両方から出る。両方を見られるのは
        // `materialize_local_folder_listing` だけなので、集計もここが持つ。呼び出し側で
        // 数え直すと本体一覧と remote 一覧で違う数字が出る (v2.12.0 取り込み時の実害)。
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("001.jpg"), b"jpg").unwrap();
        std::fs::write(temp.path().join("001.png"), b"png").unwrap();
        std::fs::write(temp.path().join("notes.txt"), b"text").unwrap();
        std::fs::write(temp.path().join("Thumbs.db"), b"explorer cache").unwrap();

        let scan =
            scan_directory_with_convertible_archives(temp.path(), true, true, false).unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.skip_duplicate_images = true;
        settings.image_ext_priority = vec!["png".to_owned(), "jpg".to_owned()];

        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);

        assert_eq!(listing.omitted.same_name, 1, "001.jpg が png に負ける");
        assert_eq!(listing.omitted.ignored_archive, 0);
        assert_eq!(listing.omitted.unsupported, 1, "notes.txt");
        assert_eq!(listing.omitted.system, 1, "Thumbs.db");
        assert_eq!(listing.items.len(), 1, "残るのは 001.png だけ");
    }

    #[test]
    fn rating_order_reads_survivors_once_and_failure_uses_name_order() {
        use crate::rating_sort::{
            ListingOrderRequest, RatingSortDirection, RatingSortSpec, RatingSortUnratedPosition,
        };
        let temp = tempfile::TempDir::new().unwrap();
        let db = crate::rating_db::RatingDb::open_at(temp.path().join("ratings.db")).unwrap();
        let path = |name: &str| temp.path().join(name);
        let key = |name: &str| crate::adjustment_db::normalize_path(&path(name));
        db.set_user_rating(&key("z.jpg"), 5, None).unwrap();
        db.set_user_rating(&key("a.jpg"), 4, None).unwrap();
        let scan = || ScannedDir {
            precise_stamps: Default::default(),
            reconcile: None,
            complete_audio_inventory: None,
            folders: Vec::new(),
            all_media: ["a.jpg", "z.jpg", "x.jpg", "x.png"]
                .into_iter()
                .map(|name| ScannedMediaEntry {
                    path: path(name),
                    kind: ScanMediaKind::Image,
                    mtime: 0,
                    file_size: 1,
                    sort_meta: crate::settings::ListingSortMetadata::new(0, Some(1)),
                })
                .collect(),
            omitted: OmittedFolderEntryCounts::default(),
            auxiliary_media: Default::default(),
        };
        let mut settings = crate::settings::Settings::default();
        settings.skip_duplicate_images = true;
        settings.image_ext_priority = vec!["png".into(), "jpg".into()];
        let request = ListingOrderRequest::Rating(RatingSortSpec {
            direction: RatingSortDirection::Desc,
            unrated_position: RatingSortUnratedPosition::BelowAll,
        });
        let mut reads = 0;
        let listing = materialize_local_folder_listing_with_order(
            temp.path(),
            scan(),
            &settings,
            request,
            |keys| {
                reads += 1;
                assert_eq!(keys.len(), 3, "excluded x.jpg must not be requested");
                assert!(!keys.contains(&key("x.jpg")));
                db.get_many_complete(keys, 7)
                    .map_err(|error| error.to_string())
            },
        );
        assert_eq!(reads, 1);
        assert_eq!(
            listing
                .items
                .iter()
                .map(|item| item.name().into_owned())
                .collect::<Vec<_>>(),
            ["z.jpg", "a.jpg", "x.png"]
        );
        let FolderRatingOrder::Prepared {
            cache,
            write_generation,
            ..
        } = listing.rating_order
        else {
            panic!("complete facts expected")
        };
        assert_eq!(write_generation, 7);
        assert_eq!(cache, [(0, 5), (1, 4), (2, 0)].into());

        let failed = materialize_local_folder_listing_with_order(
            temp.path(),
            scan(),
            &settings,
            request,
            |_| Err("injected".into()),
        );
        assert!(matches!(failed.rating_order, FolderRatingOrder::Failed));
        assert_eq!(
            failed
                .items
                .iter()
                .map(|item| item.name().into_owned())
                .collect::<Vec<_>>(),
            ["a.jpg", "x.png", "z.jpg"]
        );
    }

    #[test]
    fn unsupported_files_are_counted_but_system_files_are_not() {
        let counts = OmittedFolderEntryCounts {
            file_type: 0,
            same_name: 0,
            hidden: 0,
            ignored_archive: 0,
            ignored_epub: 0,
            unsupported: 5,
            system: 0,
        };
        assert_eq!(
            counts.primary_count(),
            5,
            "見えていない実ファイルは対象外拡張子でも数える"
        );

        let system_only = OmittedFolderEntryCounts {
            file_type: 0,
            same_name: 0,
            hidden: 0,
            ignored_archive: 0,
            ignored_epub: 0,
            unsupported: 0,
            system: 7,
        };
        assert_eq!(
            system_only.primary_count(),
            0,
            "Thumbs.db だけのフォルダでチップを点けない"
        );
    }

    #[test]
    fn materialized_size_sort_keeps_real_zero_known_and_display_unknown_legacy_meta() {
        let media = |name: &str, file_size: i64, known: bool| ScannedMediaEntry {
            path: PathBuf::from(format!(r"C:\sizes\{name}")),
            kind: ScanMediaKind::Image,
            mtime: 7,
            file_size,
            sort_meta: crate::settings::ListingSortMetadata::new(7, known.then_some(file_size)),
        };
        let scan = || ScannedDir {
            precise_stamps: Default::default(),
            reconcile: None,
            complete_audio_inventory: None,
            folders: vec![ScannedFolderEntry {
                item: GridItem::Folder(PathBuf::from(r"C:\sizes\child")),
                display_meta: Some((7, 0)),
                sort_meta: crate::settings::ListingSortMetadata::new(7, None),
            }],
            all_media: vec![
                media("unknown.jpg", 0, false),
                media("ten.jpg", 10, true),
                media("zero.jpg", 0, true),
            ],
            omitted: OmittedFolderEntryCounts::default(),
            auxiliary_media: Default::default(),
        };
        let mut settings = crate::settings::Settings::default();
        settings.sort_order = crate::settings::SortOrder::SizeAsc;
        let listing =
            materialize_local_folder_listing(std::path::Path::new(r"C:\sizes"), scan(), &settings);
        let image_names = listing
            .items
            .iter()
            .filter_map(|item| match item {
                GridItem::Image(path) => path.file_name()?.to_str().map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(image_names, ["zero.jpg", "ten.jpg", "unknown.jpg"]);
        let unknown_index = listing
            .items
            .iter()
            .position(|item| item.name().as_ref() == "unknown.jpg")
            .unwrap();
        assert_eq!(listing.metas[unknown_index], Some((7, 0)));

        settings.sort_order = crate::settings::SortOrder::SizeDesc;
        let listing =
            materialize_local_folder_listing(std::path::Path::new(r"C:\sizes"), scan(), &settings);
        let image_names = listing
            .items
            .iter()
            .filter_map(|item| match item {
                GridItem::Image(path) => path.file_name()?.to_str().map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(image_names, ["ten.jpg", "zero.jpg", "unknown.jpg"]);
    }

    #[test]
    fn ignored_archive_reclassification_preserves_the_primary_total() {
        let counts = OmittedFolderEntryCounts {
            file_type: 0,
            same_name: 3,
            hidden: 2,
            ignored_archive: 4,
            ignored_epub: 0,
            unsupported: 1,
            system: 7,
        };
        let before_reclassification = counts
            .same_name
            .saturating_add(counts.hidden)
            .saturating_add(counts.unsupported.saturating_add(counts.ignored_archive));
        let current_breakdown_sum = counts
            .same_name
            .saturating_add(counts.hidden)
            .saturating_add(counts.ignored_archive)
            .saturating_add(counts.unsupported);

        assert_eq!(counts.primary_count(), 10);
        assert_eq!(
            counts.primary_count(),
            before_reclassification,
            "旧 unsupported に含まれていた書庫を分離しても主数字は変えない"
        );
        assert_eq!(
            counts.primary_count(),
            current_breakdown_sum,
            "主数字は現在の主内訳の和と一致する"
        );
    }

    #[test]
    fn system_metadata_names_are_matched_case_insensitively() {
        for name in [
            "thumbs.db",
            "Thumbs.db",
            "DESKTOP.INI",
            ".DS_Store",
            "._img.jpg",
        ] {
            assert!(is_system_metadata_name(name), "{name} should be system");
        }
        for name in ["notes.txt", "001.jpg", "thumbs.jpg", "my_desktop.ini.txt"] {
            assert!(
                !is_system_metadata_name(name),
                "{name} should not be system"
            );
        }
    }

    #[test]
    fn video_image_filter_returns_the_actual_folded_difference() {
        let mut media = vec![
            ScannedMediaEntry {
                path: PathBuf::from(r"C:\media\clip.mp4"),
                kind: ScanMediaKind::Video,
                mtime: 0,
                file_size: 0,
                sort_meta: crate::settings::ListingSortMetadata::new(0, Some(0)),
            },
            ScannedMediaEntry {
                path: PathBuf::from(r"C:\media\clip.jpg"),
                kind: ScanMediaKind::Image,
                mtime: 0,
                file_size: 0,
                sort_meta: crate::settings::ListingSortMetadata::new(0, Some(0)),
            },
            ScannedMediaEntry {
                path: PathBuf::from(r"C:\media\other.jpg"),
                kind: ScanMediaKind::Image,
                mtime: 0,
                file_size: 0,
                sort_meta: crate::settings::ListingSortMetadata::new(0, Some(0)),
            },
        ];

        let filtered = filter_video_image_duplicates(&mut media, false);

        assert_eq!(filtered.omitted, 1);
        assert!(filtered.sidecars.is_empty());
        assert_eq!(media.len(), 2);
    }

    #[test]
    fn aggregate_video_sidecars_are_full_path_scoped_and_cancel_atomically() {
        let temp = tempfile::TempDir::new().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        std::fs::create_dir(&left).unwrap();
        std::fs::create_dir(&right).unwrap();
        let left_video = left.join("same.mp4");
        let right_video = right.join("same.mp4");
        let left_image = left.join("same.jpg");
        let right_image = right.join("same.jpg");
        for path in [&left_video, &right_video, &left_image, &right_image] {
            std::fs::write(path, b"fixture").unwrap();
        }
        let mut settings = crate::settings::Settings::default();
        settings.skip_image_if_video_exists = true;
        settings.video_thumb_use_sidecar_image = true;

        let found = discover_aggregate_video_sidecars_while(
            &settings,
            &[left_video.clone(), right_video.clone()],
            64,
            || true,
        )
        .unwrap();
        assert_eq!(found.scanned_parents, 2);
        assert_eq!(found.skipped_parents, 0);
        assert_eq!(
            found
                .by_video_path
                .get(&crate::path_key::normalize_keep_drive(&left_video)),
            Some(&left_image)
        );
        assert_eq!(
            found
                .by_video_path
                .get(&crate::path_key::normalize_keep_drive(&right_video)),
            Some(&right_image)
        );
        assert!(!found.by_video_path.contains_key("same"));

        let mut polls = 0;
        assert!(
            discover_aggregate_video_sidecars_while(
                &settings,
                &[left_video, right_video],
                64,
                || {
                    polls += 1;
                    polls < 2
                },
            )
            .is_none(),
            "cancelled discovery must not publish a partial source map"
        );
    }

    #[test]
    fn image_folder_page_count_propagates_scan_errors() {
        let temp = tempfile::TempDir::new().unwrap();
        let missing = temp.path().join("missing");
        assert!(image_folder_page_count(&missing, &options(false)).is_err());
    }

    #[test]
    fn directory_scan_discards_partial_result_after_entry_error() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("001.jpg"), b"image").unwrap();
        let first_entry = std::fs::read_dir(temp.path())
            .unwrap()
            .next()
            .expect("fixture entry")
            .unwrap();
        let entries = vec![
            Ok(first_entry),
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "simulated FindNextFile failure",
            )),
        ];

        let error =
            match scan_directory_entries(entries, true, true, false, &Default::default(), None) {
                Ok(_) => panic!("a partial directory snapshot must not be published"),
                Err(error) => error,
            };

        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
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

    #[test]
    fn image_ext_priority_completion_changes_page_count_fingerprint_only_once() {
        let mut settings = crate::settings::Settings::default();
        settings.susie_enabled = false;
        settings.image_ext_priority.truncate(27);
        let old = image_page_recognition_fingerprint(&settings);
        crate::settings::apply_load_time_migrations(&mut settings);
        let completed = image_page_recognition_fingerprint(&settings);
        assert_ne!(old, completed);
        crate::settings::apply_load_time_migrations(&mut settings);
        assert_eq!(completed, image_page_recognition_fingerprint(&settings));
    }

    #[test]
    fn native_raw_addition_invalidates_page_counts_with_already_complete_priorities() {
        use crate::catalog::{CatalogDb, ContainerPageKind, ContainerPageMeta};

        let mut settings = crate::settings::Settings::default();
        settings.susie_enabled = false;
        let saved_priority = settings.image_ext_priority.clone();
        assert!(!crate::settings::normalize_image_ext_priority(
            &mut settings.image_ext_priority
        ));
        crate::settings::apply_load_time_migrations(&mut settings);
        assert_eq!(settings.image_ext_priority, saved_priority);

        // Model format support before the eight RAW additions, keeping every setting unchanged.
        let added_raw = ["crw", "srw", "3fr", "erf", "kdc", "dcr", "mrw", "mos"];
        let released_native: Vec<_> = crate::folder_tree::SUPPORTED_EXTENSIONS
            .iter()
            .copied()
            .filter(|extension| !added_raw.contains(extension))
            .collect();
        let old_fingerprint =
            image_page_recognition_fingerprint_with_native_extensions(&settings, &released_native);
        let options = image_folder_page_count_options(&settings);
        assert_ne!(old_fingerprint, options.fingerprint);

        let temp = tempfile::TempDir::new().unwrap();
        let catalog = CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap();
        let kind = ContainerPageKind::Folder;
        // Keep the source identity unchanged: only native recognition changed across the upgrade.
        for (name, stale_count, expected_count) in [("raw-only", None, 1), ("pictures", Some(1), 2)]
        {
            let folder = temp.path().join(name);
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("001.crw"), b"not decoded during scan").unwrap();
            if stale_count.is_some() {
                std::fs::write(folder.join("002.jpg"), b"not decoded during scan").unwrap();
            }
            catalog
                .set_container_page_meta(name, kind, 200, 0, old_fingerprint, stale_count)
                .unwrap();
            assert_eq!(
                catalog
                    .get_container_page_meta(name, kind, 200, 0, old_fingerprint)
                    .unwrap(),
                Some(ContainerPageMeta {
                    page_count: stale_count
                })
            );
            assert_eq!(
                catalog
                    .get_container_page_meta(name, kind, 200, 0, options.fingerprint)
                    .unwrap(),
                None
            );
            assert_eq!(
                image_folder_page_count(&folder, &options).unwrap(),
                Some(expected_count)
            );
        }
        assert_eq!(settings.image_ext_priority, saved_priority);
    }

    #[test]
    fn audio_and_video_sidecars_share_all_four_released_flag_combinations() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["Song.MP3", "song.flac", "song.mp4", "song.jpg", "SONG.png"] {
            std::fs::write(temp.path().join(name), b"fixture").unwrap();
        }
        for skip in [false, true] {
            for use_sidecar in [false, true] {
                let mut settings = crate::settings::Settings::default();
                settings.skip_image_if_video_exists = skip;
                settings.video_thumb_use_sidecar_image = use_sidecar;
                settings.skip_duplicate_images = false;
                settings.sort_order = crate::settings::SortOrder::FileName;
                let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
                let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
                assert_eq!(
                    listing
                        .items
                        .iter()
                        .filter(|item| matches!(item, GridItem::Audio(_)))
                        .count(),
                    2,
                );
                assert_eq!(
                    listing
                        .items
                        .iter()
                        .filter(|item| matches!(item, GridItem::Video(_)))
                        .count(),
                    1,
                );
                assert_eq!(
                    listing
                        .items
                        .iter()
                        .filter(|item| matches!(item, GridItem::Image(_)))
                        .count(),
                    if skip { 0 } else { 2 },
                );
                assert_eq!(listing.omitted.same_name, if skip { 2 } else { 0 });
                if skip && use_sidecar {
                    assert_eq!(listing.video_thumb_overrides.len(), 6);
                    let selected = listing
                        .video_thumb_overrides
                        .iter()
                        .map(|(source, image)| {
                            (crate::path_key::normalize_keep_drive(source), image)
                        })
                        .collect::<std::collections::HashMap<_, _>>();
                    let video = selected
                        [&crate::path_key::normalize_keep_drive(&temp.path().join("song.mp4"))];
                    assert_eq!(
                        selected
                            [&crate::path_key::normalize_keep_drive(&temp.path().join("Song.MP3"))],
                        video,
                    );
                    assert_eq!(
                        selected[&crate::path_key::normalize_keep_drive(
                            &temp.path().join("song.flac")
                        )],
                        video,
                    );
                } else {
                    assert!(listing.video_thumb_overrides.is_empty());
                }
            }
        }
    }

    #[test]
    fn audio_sidecar_candidates_keep_video_order_and_last_candidate_adoption() {
        let root = std::path::Path::new(r"C:\same-parent");
        let entry = |name: &str, kind| ScannedMediaEntry {
            path: root.join(name),
            kind,
            mtime: 0,
            file_size: 1,
            sort_meta: crate::settings::ListingSortMetadata::new(0, Some(1)),
        };
        for candidates in [["song.jpg", "song.png"], ["song.png", "song.jpg"]] {
            let mut media = vec![
                entry("song.mp4", ScanMediaKind::Video),
                entry("song.mp3", ScanMediaKind::Audio),
                entry("song.flac", ScanMediaKind::Audio),
                entry(candidates[0], ScanMediaKind::Image),
                entry(candidates[1], ScanMediaKind::Image),
            ];
            let filtered = filter_video_image_duplicates(&mut media, true);
            let selected = filtered
                .sidecars
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>();
            assert_eq!(filtered.omitted, 2);
            assert_eq!(media.len(), 3);
            for source in ["song.mp4", "song.mp3", "song.flac"] {
                assert_eq!(
                    selected.get(&root.join(source)),
                    Some(&root.join(candidates[1]))
                );
            }
        }
    }

    #[test]
    fn aggregate_audio_sidecars_are_full_path_scoped_and_video_parents_keep_priority() {
        let temp = tempfile::tempdir().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        std::fs::create_dir(&left).unwrap();
        std::fs::create_dir(&right).unwrap();
        let left_audio = left.join("same.mp3");
        let right_audio = right.join("same.FLAC");
        let right_video = right.join("same.mp4");
        let left_image = left.join("same.jpg");
        let right_image = right.join("same.png");
        for path in [
            &left_audio,
            &right_audio,
            &right_video,
            &left_image,
            &right_image,
        ] {
            std::fs::write(path, b"fixture").unwrap();
        }
        let mut settings = crate::settings::Settings::default();
        settings.skip_image_if_video_exists = true;
        settings.video_thumb_use_sidecar_image = true;
        // The audio parent appears first in the request, yet the released video budget wins.
        let sources = [left_audio.clone(), right_audio.clone(), right_video.clone()];
        let complete =
            discover_aggregate_video_sidecars_while(&settings, &sources, 64, || true).unwrap();
        assert_eq!(complete.by_video_path.len(), 3);
        assert_eq!(
            complete
                .by_video_path
                .get(&crate::path_key::normalize_keep_drive(&left_audio)),
            Some(&left_image),
        );
        for source in [&right_audio, &right_video] {
            assert_eq!(
                complete
                    .by_video_path
                    .get(&crate::path_key::normalize_keep_drive(source)),
                Some(&right_image),
            );
        }
        let capped =
            discover_aggregate_video_sidecars_while(&settings, &sources, 1, || true).unwrap();
        assert_eq!(capped.scanned_parents, 1);
        assert_eq!(capped.skipped_parents, 1);
        assert_eq!(capped.by_video_path.len(), 2);
        assert!(
            !capped
                .by_video_path
                .contains_key(&crate::path_key::normalize_keep_drive(&left_audio))
        );

        let mut polls = 0;
        assert!(
            discover_aggregate_video_sidecars_while(&settings, &sources, 64, || {
                polls += 1;
                polls < 5
            })
            .is_none()
        );
        assert_eq!(
            polls, 5,
            "cancellation after the first parent discards partial provenance"
        );
    }

    #[test]
    fn auxiliary_candidates_keep_selected_sidecar_in_each_explicit_candidate_order() {
        for source_kind in [ScanMediaKind::Audio, ScanMediaKind::Video] {
            let source = PathBuf::from(if source_kind == ScanMediaKind::Audio {
                "album/song.mp3"
            } else {
                "album/song.mp4"
            });
            for extensions in [["jpg", "png"], ["png", "jpg"]] {
                let mut media = vec![ScannedMediaEntry {
                    path: source.clone(),
                    kind: source_kind,
                    mtime: 1,
                    file_size: 5,
                    sort_meta: crate::settings::ListingSortMetadata::new(1, Some(5)),
                }];
                media.extend(extensions.iter().map(|ext| ScannedMediaEntry {
                    path: source.with_extension(ext),
                    kind: ScanMediaKind::Image,
                    mtime: 1,
                    file_size: 5,
                    sort_meta: crate::settings::ListingSortMetadata::new(1, Some(5)),
                }));
                for hide_adopted in [false, true] {
                    let mut policy = crate::file_type_visibility::FileTypeVisibility::default();
                    if hide_adopted {
                        policy.excluded_extensions.insert(extensions[1].into());
                    }
                    let selected =
                        AuxiliaryMediaCandidates::capture(&media, &policy, &Default::default())
                            .sidecars()
                            .into_iter()
                            .collect::<std::collections::HashMap<_, _>>();
                    assert_eq!(
                        selected.get(&source),
                        Some(&source.with_extension(extensions[1]))
                    );
                }
            }
        }
    }

    #[test]
    fn folder_keeps_selected_sidecar_when_only_its_image_kind_is_hidden() {
        use crate::settings::SortOrder;
        let temp = tempfile::tempdir().unwrap();
        for source_name in ["song.mp3", "movie.mp4"] {
            let source = temp.path().join(source_name);
            let jpg = source.with_extension("jpg");
            let png = source.with_extension("png");
            std::fs::write(&source, b"media").unwrap();
            std::fs::write(&jpg, b"jpg").unwrap();
            std::fs::write(&png, b"png-longer").unwrap();
            for (sort, adopted) in [
                (SortOrder::FileName, &png),
                (SortOrder::FileNameDesc, &jpg),
                (SortOrder::SizeAsc, &png),
                (SortOrder::SizeDesc, &jpg),
            ] {
                let mut settings = crate::settings::Settings::default();
                settings.sort_order = sort;
                settings.skip_image_if_video_exists = true;
                settings.video_thumb_use_sidecar_image = true;
                for hide_adopted in [false, true] {
                    if hide_adopted {
                        settings
                            .file_type_visibility
                            .excluded_extensions
                            .insert(adopted.extension().unwrap().to_str().unwrap().to_owned());
                    }
                    let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
                    let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
                    let selected = listing
                        .video_thumb_overrides
                        .into_iter()
                        .collect::<std::collections::HashMap<_, _>>();
                    assert_eq!(
                        selected.get(&source),
                        Some(adopted),
                        "{sort:?}, hidden={hide_adopted}"
                    );
                    assert!(listing.items.iter().any(|item| matches!(item,
                        GridItem::Audio(path) | GridItem::Video(path) if path == &source)));
                    assert!(
                        !listing
                            .items
                            .iter()
                            .any(|item| matches!(item, GridItem::Image(_)))
                    );
                }
            }
        }
    }

    #[test]
    fn visibility_hidden_images_remain_audio_sidecars_and_hidden_audio_keeps_prune_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let audio = temp.path().join("song.mp3");
        let image = temp.path().join("song.png");
        std::fs::write(&audio, b"audio").unwrap();
        std::fs::write(&image, b"sidecar").unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.skip_image_if_video_exists = true;
        settings.video_thumb_use_sidecar_image = true;
        settings
            .file_type_visibility
            .excluded_extensions
            .insert("png".into());
        let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
        assert_eq!(
            scan.auxiliary_media
                .entries
                .iter()
                .filter(|entry| entry.kind == ScanMediaKind::Image)
                .count(),
            1
        );
        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
        assert_eq!(listing.items, vec![GridItem::Audio(audio.clone())]);
        assert_eq!(listing.video_thumb_overrides, vec![(audio.clone(), image)]);
        assert_eq!(listing.omitted.file_type, 1);
        settings
            .file_type_visibility
            .excluded_extensions
            .insert("mp3".into());
        let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
        assert_eq!(
            scan.complete_audio_inventory.as_ref().unwrap(),
            &std::collections::HashSet::from(["song.mp3".into()])
        );
        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
        assert!(listing.items.is_empty());
        assert!(listing.video_thumb_overrides.is_empty());
        assert_eq!(listing.omitted.file_type, 2);
    }

    #[test]
    fn complete_audio_inventory_keeps_hidden_mp3_before_display_and_duplicate_filters() {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "Shown.MP3",
            ".hidden.mp3",
            "shown.mp4",
            "shown.jpg",
            "other.flac",
        ] {
            std::fs::write(temp.path().join(name), b"fixture").unwrap();
        }
        std::fs::create_dir(temp.path().join("directory.mp3")).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
            let wide = temp
                .path()
                .join(".hidden.mp3")
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            unsafe {
                SetFileAttributesW(windows::core::PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN)
            }
            .unwrap();
        }
        let mut settings = crate::settings::Settings::default();
        settings.show_hidden_files = false;
        settings.skip_image_if_video_exists = true;
        let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
        assert_eq!(
            scan.complete_audio_inventory.as_ref().unwrap(),
            &std::collections::HashSet::from(["shown.mp3".to_owned(), ".hidden.mp3".to_owned()]),
        );
        assert!(
            !scan
                .all_media
                .iter()
                .any(|entry| entry.path.file_name().unwrap() == ".hidden.mp3")
        );
        let inventory = scan.complete_audio_inventory.clone();
        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
        assert!(listing.items.iter().any(
            |item| matches!(item, GridItem::Audio(path) if path.file_name().unwrap() == "Shown.MP3")
        ));
        assert!(
            !listing
                .items
                .iter()
                .any(|item| matches!(item, GridItem::Image(_)))
        );
        assert_eq!(
            inventory.unwrap().len(),
            2,
            "prune ownership is independent of visible rows"
        );
    }

    #[test]
    fn failed_or_canceled_directory_scans_never_supply_a_complete_audio_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let fallback = scan_directory(&missing);
        assert!(fallback.complete_audio_inventory.is_none());
        assert!(fallback.all_media.is_empty());
        assert!(
            scan_directory_with_settings(&missing, &crate::settings::Settings::default()).is_err()
        );

        std::fs::write(temp.path().join("song.mp3"), b"audio").unwrap();
        let entry = std::fs::read_dir(temp.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let interrupted = std::io::Error::new(
            std::io::ErrorKind::ConnectionAborted,
            "incomplete audio inventory",
        );
        assert!(
            scan_directory_entries(
                vec![Ok(entry), Err(interrupted)],
                true,
                true,
                false,
                &Default::default(),
                None
            )
            .is_err()
        );

        let cancel = std::sync::atomic::AtomicBool::new(true);
        let canceled = scan_directory_with_convertible_archives_cancel(
            temp.path(),
            true,
            true,
            false,
            Some(&cancel),
        );
        assert!(matches!(canceled, Err(error) if error.kind() == std::io::ErrorKind::Interrupted));
    }
}

#[cfg(test)]
mod file_type_visibility_tests {
    use super::*;
    use crate::file_type_visibility::FileTypeCategory;

    fn fixture(names: &[&str]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        for name in names {
            std::fs::write(temp.path().join(name), b"fixture").unwrap();
        }
        temp
    }

    #[test]
    fn policy_precedes_same_name_winners_and_keeps_aligned_metadata() {
        let temp = fixture(&[
            "movie.mp4",
            "movie.jpg",
            "book.zip",
            "book.rar",
            "raw.cr2",
            "._page.jpg",
        ]);
        std::fs::create_dir(temp.path().join("passage.zip")).unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.file_type_visibility.excluded_categories.extend([
            FileTypeCategory::Video,
            FileTypeCategory::Zip,
            FileTypeCategory::Raw,
        ]);
        let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
        assert_eq!(scan.omitted.file_type, 3);
        assert_eq!(scan.omitted.system, 1);
        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
        let names = listing
            .items
            .iter()
            .map(|item| item.name().into_owned())
            .collect::<Vec<_>>();
        assert!(names.contains(&"movie.jpg".to_owned()));
        assert!(names.contains(&"book.rar".to_owned()));
        assert!(names.contains(&"passage.zip".to_owned()));
        assert_eq!(listing.items.len(), listing.metas.len());
        assert_eq!(listing.items.len(), listing.sort_metas.len());
        assert_eq!(listing.omitted.file_type, 3);
        assert_eq!(listing.omitted.same_name, 0);
    }

    #[test]
    fn hidden_images_remain_video_sidecar_resources() {
        let temp = fixture(&["movie.mp4", "movie.jpg"]);
        let mut settings = crate::settings::Settings::default();
        settings.skip_image_if_video_exists = true;
        settings.video_thumb_use_sidecar_image = true;
        settings
            .file_type_visibility
            .excluded_categories
            .insert(FileTypeCategory::Image);
        let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
        assert_eq!(listing.items.len(), 1);
        assert!(matches!(listing.items[0], GridItem::Video(_)));
        assert_eq!(listing.omitted.file_type, 1);
        assert_eq!(listing.omitted.same_name, 0);
        assert_eq!(
            listing.video_thumb_overrides,
            vec![(temp.path().join("movie.mp4"), temp.path().join("movie.jpg"))]
        );
    }

    #[test]
    fn visible_projection_controls_image_book_count_and_fingerprint() {
        let temp = fixture(&["page.jpg", "page.png", "clip.mp4", "book.zip"]);
        let mut settings = crate::settings::Settings::default();
        settings.skip_duplicate_images = true;
        let old = image_page_recognition_fingerprint(&settings);
        settings
            .file_type_visibility
            .excluded_categories
            .extend([FileTypeCategory::Video, FileTypeCategory::Zip]);
        settings
            .file_type_visibility
            .excluded_extensions
            .insert("png".into());
        assert_ne!(old, image_page_recognition_fingerprint(&settings));
        assert_eq!(
            image_folder_page_count(temp.path(), &image_folder_page_count_options(&settings))
                .unwrap(),
            Some(1)
        );
        std::fs::create_dir(temp.path().join("child")).unwrap();
        assert_eq!(
            image_folder_page_count(temp.path(), &image_folder_page_count_options(&settings))
                .unwrap(),
            None
        );
        std::fs::remove_dir(temp.path().join("child")).unwrap();
        settings
            .file_type_visibility
            .excluded_categories
            .insert(FileTypeCategory::Image);
        assert_eq!(
            image_folder_page_count(temp.path(), &image_folder_page_count_options(&settings))
                .unwrap(),
            None
        );
    }

    #[test]
    fn hidden_upscaled_derivative_does_not_remove_visible_original() {
        let temp = fixture(&[
            "movie.mp4",
            "movie.miv.mkv",
            "movie.miv.json",
            "movie.jpg",
            "movie.png",
        ]);
        let mut settings = crate::settings::Settings::default();
        settings.skip_image_if_video_exists = true;
        settings.video_thumb_use_sidecar_image = true;
        let initial = scan_directory_with_settings(temp.path(), &settings).unwrap();
        assert!(
            !initial
                .all_media
                .iter()
                .any(|entry| entry.path == temp.path().join("movie.mp4")),
            "the JSON inventory must activate the shipped derivative-pair rule"
        );
        settings
            .file_type_visibility
            .excluded_extensions
            .insert("mkv".into());
        settings
            .file_type_visibility
            .excluded_extensions
            .insert("png".into());
        let scan = scan_directory_with_settings(temp.path(), &settings).unwrap();
        assert!(
            scan.all_media
                .iter()
                .any(|entry| entry.path == temp.path().join("movie.mp4"))
        );
        let listing = materialize_local_folder_listing(temp.path(), scan, &settings);
        assert_eq!(
            listing.items,
            vec![GridItem::Video(temp.path().join("movie.mp4"))]
        );
        assert_eq!(
            listing
                .video_thumb_overrides
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>()
                .get(&temp.path().join("movie.mp4")),
            Some(&temp.path().join("movie.png"))
        );
    }
}

#[cfg(test)]
mod section1375_row_tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime};

    #[cfg(windows)]
    #[test]
    fn section1375_scan_record_and_delete_share_windows_separator_identity() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("spelling");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("page.png"), b"same-source").unwrap();
        let slash_folder = PathBuf::from(folder.to_string_lossy().replace('\\', "/"));
        let settings = crate::settings::Settings::default();
        let previous = materialize_local_folder_listing(
            &folder,
            scan_directory_with_settings(&slash_folder, &settings).unwrap(),
            &settings,
        );
        let current = materialize_local_folder_listing(
            &folder,
            scan_directory_with_settings(&folder, &settings).unwrap(),
            &settings,
        );
        assert!(current.row_record.same_remaining_rows(
            &previous.row_record,
            std::iter::empty(),
            &folder,
        ));
        let removed = slash_folder.join("page.png");
        std::fs::remove_file(&removed).unwrap();
        let empty = materialize_local_folder_listing(
            &folder,
            scan_directory_with_settings(&folder, &settings).unwrap(),
            &settings,
        );
        assert!(empty.row_record.same_remaining_rows(
            &previous.row_record,
            std::iter::once(&removed),
            &folder,
        ));
        assert_ne!(
            folder_scan_path_key(std::path::Path::new(r"C:\scope\page.png")),
            folder_scan_path_key(std::path::Path::new(r"D:\scope\page.png")),
        );
        assert_ne!(
            folder_scan_path_key(std::path::Path::new(r"C:\scope\page.png")),
            folder_scan_path_key(std::path::Path::new(r"C:\scope\Page.png")),
        );
    }

    #[test]
    fn section1375_scan_projection_keeps_one_physical_scope_after_filter_and_order() {
        let temp = tempfile::tempdir().unwrap();
        let settings = crate::settings::Settings {
            skip_duplicate_images: true,
            image_ext_priority: vec!["jpg".into(), "png".into()],
            ..Default::default()
        };
        let mut records = Vec::new();
        for name in ["a", "b"] {
            let folder = temp.path().join(name);
            std::fs::create_dir(&folder).unwrap();
            std::fs::create_dir(folder.join("child")).unwrap();
            for file in ["z.jpg", "cover.jpg", "cover.png", "book.zip", "book.pdf"] {
                std::fs::write(folder.join(file), b"same-content").unwrap();
            }
            let scan = scan_directory_with_settings(&folder, &settings).unwrap();
            let listing = materialize_local_folder_listing(&folder, scan, &settings);
            assert_eq!(listing.omitted.same_name, 1);
            assert_eq!(listing.row_record.rows.len(), 5);
            assert!(listing.row_record.known);
            assert!(
                listing
                    .row_record
                    .rows
                    .iter()
                    .all(|row| { row.parent_key == folder_scan_path_key(&folder) })
            );
            assert!(listing.row_record.same_remaining_rows(
                &listing.row_record,
                std::iter::empty(),
                &folder,
            ));
            records.push((folder, listing.row_record));
        }
        assert!(!records[0].1.same_remaining_rows(
            &records[1].1,
            std::iter::empty(),
            &records[0].0,
        ));
    }

    #[test]
    fn section1375_signature_precision_order_tail_and_unknown() {
        let root = std::path::Path::new(r"C:\section1375");
        let mut old = signature_tests::fixture(root);
        let path = old.all_media[0].path.clone();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        old.precise_stamps
            .insert(folder_scan_path_key(&path), Some((t, 20)));
        let settings = crate::settings::Settings::default();
        let previous =
            materialize_local_folder_listing(root, old.clone_for_projection(), &settings)
                .row_record;
        let mut new = old.clone_for_projection();
        new.precise_stamps.insert(
            folder_scan_path_key(&path),
            Some((t + Duration::from_micros(1), 20)),
        );
        assert_eq!(
            signature_from_scan(&old),
            signature_from_scan(&new),
            "the legacy seconds signature stays unchanged; row records own precise comparison"
        );
        let current = materialize_local_folder_listing(root, new, &settings).row_record;
        assert!(!current.same_remaining_rows(&previous, std::iter::empty(), root));
        let mut rows = previous.rows.clone();
        rows.reverse();
        let changed_order = FolderRowRecord { rows, known: true };
        assert!(!changed_order.same_remaining_rows(&previous, std::iter::empty(), root));
        let mut unknown = old;
        unknown
            .precise_stamps
            .insert(folder_scan_path_key(&path), None);
        let current = materialize_local_folder_listing(root, unknown, &settings).row_record;
        assert!(!current.known);
        assert!(!current.same_remaining_rows(&current, std::iter::empty(), root));
    }

    #[test]
    fn section1375_large_10k_100k_projection_compare_delete_and_first_fork() {
        for count in [10_000, 100_000] {
            let root = std::path::Path::new(r"C:\section1375-large");
            let mut scan = signature_tests::fixture(root);
            scan.folders.clear();
            scan.all_media.clear();
            scan.all_media
                .extend((0..count).map(|index| ScannedMediaEntry {
                    path: root.join(format!("image-{index:06}.jpg")),
                    kind: ScanMediaKind::Image,
                    mtime: 10,
                    file_size: 20,
                    sort_meta: crate::settings::ListingSortMetadata::new(10, Some(20)),
                }));
            scan.precise_stamps
                .extend(scan.all_media.iter().map(|entry| {
                    (
                        folder_scan_path_key(&entry.path),
                        Some((SystemTime::UNIX_EPOCH + Duration::from_secs(10), 20)),
                    )
                }));
            let worker = std::thread::spawn(move || {
                let settings = crate::settings::Settings::default();
                let start = Instant::now();
                let projected =
                    materialize_local_folder_listing(root, scan.clone_for_projection(), &settings);
                let projection_ms = start.elapsed().as_secs_f64() * 1000.0;
                let mut initial_record_samples = Vec::with_capacity(5);
                for _ in 0..5 {
                    let initial_start = Instant::now();
                    let initial = FolderRowRecord::from_projection(
                        root,
                        &projected.items,
                        &projected.metas,
                        &scan.precise_stamps,
                        &projected.video_thumb_overrides,
                    );
                    initial_record_samples.push(initial_start.elapsed().as_secs_f64() * 1000.0);
                    assert!(initial.known);
                    assert_eq!(initial.rows, projected.row_record.rows);
                }
                initial_record_samples.sort_by(f64::total_cmp);
                let initial_record_ms = initial_record_samples[2];
                let initial_record_max_ms = initial_record_samples[4];
                let previous = projected.row_record.clone();
                let start = Instant::now();
                assert!(previous.same_remaining_rows(&previous, std::iter::empty(), root));
                let compare_ms = start.elapsed().as_secs_f64() * 1000.0;
                let fork = Arc::clone(&previous);
                assert!(previous.same_remaining_rows(&fork, std::iter::empty(), root));
                for (label, variant) in [("tail", 0), ("all", 1), ("order", 2), ("unknown", 3)] {
                    let mut changed = FolderRowRecord {
                        rows: previous.rows.clone(),
                        known: true,
                    };
                    match variant {
                        0 => changed.rows.last_mut().unwrap().size += 1,
                        1 => changed.rows.iter_mut().for_each(|row| row.size += 1),
                        2 => changed.rows.reverse(),
                        _ => changed.known = false,
                    }
                    let start = Instant::now();
                    assert!(!changed.same_remaining_rows(&previous, std::iter::empty(), root));
                    eprintln!(
                        "section1375 rows={count} compare_{label}_ms={:.3}",
                        start.elapsed().as_secs_f64() * 1000.0
                    );
                }

                let removed = scan.all_media.last().unwrap().path.clone();
                scan.all_media.pop();
                let current = materialize_local_folder_listing(root, scan, &settings).row_record;
                assert!(!current.same_remaining_rows(&previous, std::iter::empty(), root));
                assert!(current.same_remaining_rows(&previous, std::iter::once(&removed), root));
                eprintln!(
                    "section1375 rows={count} projection_ms={projection_ms:.3} initial_record_ms={initial_record_ms:.3} initial_record_max_ms={initial_record_max_ms:.3} samples=5 precise_stamp_entries={count} compare_ms={compare_ms:.3} scalar_record_bytes={} shared_fork=true",
                    previous.rows.capacity() * std::mem::size_of::<FolderScanRow>()
                );
                previous
            });
            let previous = worker.join().unwrap();
            let start = Instant::now();
            let fork = previous.clone();
            assert!(Arc::ptr_eq(&previous, &fork));
            eprintln!(
                "section1375 rows={count} ui_fork_enqueue_us={:.3}",
                start.elapsed().as_secs_f64() * 1_000_000.0
            );
        }
    }
}

#[cfg(test)]
mod section1375_scan_perf_tests {
    use super::*;
    #[test]
    fn section1375_large_physical_scan_10k_100k() {
        for count in [10_000, 100_000] {
            let temp = tempfile::tempdir().unwrap();
            for index in 0..count {
                std::fs::write(temp.path().join(format!("row-{index:06}.jpg")), b"x").unwrap();
            }
            let worker = std::thread::spawn(move || {
                let start = std::time::Instant::now();
                let scan = scan_directory_with_visibility_cancel(
                    temp.path(),
                    true,
                    true,
                    false,
                    &Default::default(),
                    None,
                )
                .unwrap();
                let scan_ms = start.elapsed().as_secs_f64() * 1000.0;
                let start = std::time::Instant::now();
                let listing =
                    materialize_local_folder_listing(temp.path(), scan, &Default::default());
                let projection_ms = start.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(listing.items.len(), count);
                assert!(listing.row_record.known);
                eprintln!(
                    "section1375 physical_rows={count} scan_ms={scan_ms:.3} projection_record_ms={projection_ms:.3}"
                );
            });
            worker.join().unwrap();
        }
    }
}
