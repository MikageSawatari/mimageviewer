use super::*;

/// レーティングフィルタを 1 アイテムに適用し、可視かを返す。
///
/// レーティング対象 (コンテナ + 画像系) は全 6 バケット (★なし + ★1〜5) で判定し、
/// 非レーティング対象 (Separator 等) は常に可視。
/// 「★5 のみ表示」操作で未評価フォルダが残らないよう、コンテナもページ系と
/// 同じ厳密フィルタに揃えた (★なしフォルダに入りたいときは「なし」を ON に戻す)。
///
/// 前提: 呼び出し側で `rating_filter` が全 ON でないことを確認済み
/// (全 ON なら全アイテム可視なのでそもそも呼ばない)。
pub(super) fn passes_rating_filter(item: &GridItem, stars: u8, rating_filter: &[bool; 6]) -> bool {
    if item.accepts_rating() {
        let s = stars as usize;
        s <= 5 && rating_filter[s]
    } else {
        true
    }
}

pub(super) fn cmp_option_last<T: Ord>(
    a: Option<T>,
    b: Option<T>,
    ascending: bool,
) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) if ascending => a.cmp(&b),
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

pub(super) fn path_extension_lower(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub(super) fn format_details_duration(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return String::new();
    }
    let total = secs.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(windows)]
pub(super) fn format_details_timestamp(secs: i64, show_seconds: bool) -> String {
    if secs <= 0 {
        return String::new();
    }
    const WINDOWS_TICKS_PER_SEC: i128 = 10_000_000;
    const UNIX_TO_WINDOWS_SECS: i128 = 11_644_473_600;
    let ticks = (secs as i128 + UNIX_TO_WINDOWS_SECS) * WINDOWS_TICKS_PER_SEC;
    if ticks <= 0 || ticks > u64::MAX as i128 {
        return String::new();
    }

    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    use windows::Win32::System::Time::FileTimeToSystemTime;

    let ticks = ticks as u64;
    let filetime = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local_filetime = FILETIME::default();
    let mut st = SYSTEMTIME::default();
    if unsafe { FileTimeToLocalFileTime(&filetime, &mut local_filetime) }.is_err()
        || unsafe { FileTimeToSystemTime(&local_filetime, &mut st) }.is_err()
    {
        return String::new();
    }
    if show_seconds {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    } else {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute
        )
    }
}

#[cfg(not(windows))]
pub(super) fn format_details_timestamp(secs: i64, _show_seconds: bool) -> String {
    if secs <= 0 {
        String::new()
    } else {
        secs.to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FacetField {
    Kind,
    Ext,
    Place,
    AiModel,
    AiTool,
    Tags,
    Date,
    Size,
    Edits,
}

pub(super) enum DetailsSortPrimary {
    None,
    Text(String),
    U8(u8),
    U32(u32),
    U64(Option<u64>),
    I64(Option<i64>),
}

pub(super) struct DetailsSortRow {
    pub(super) idx: usize,
    pub(super) primary: DetailsSortPrimary,
    pub(super) name_key: crate::filename_sort::SortNameKey,
}

pub(super) fn facet_kind_for_item(item: &GridItem) -> crate::settings::FacetItemKind {
    use crate::settings::FacetItemKind;
    match item {
        GridItem::Folder(_) => FacetItemKind::Folder,
        GridItem::Image(_) => FacetItemKind::Image,
        GridItem::Video(_) => FacetItemKind::Video,
        GridItem::ZipFile(_) => FacetItemKind::Zip,
        GridItem::PdfFile(_) => FacetItemKind::Pdf,
        GridItem::ConvertibleArchive { .. } => FacetItemKind::Archive,
        GridItem::ZipImage { .. } => FacetItemKind::ZipImage,
        GridItem::PdfPage { .. } => FacetItemKind::PdfPage,
        GridItem::ZipSeparator { .. } => FacetItemKind::Separator,
        GridItem::SearchContainer { .. } => FacetItemKind::SearchContainer,
        // ZipDir はネスト ZIP ツリーの仮想サブコンテナ。ZIP 内には実フォルダが無いので、
        // facet 上は Folder バケツに入れる (= 「フォルダ」絞り込みで子コンテナだけが出る)。
        GridItem::ZipDir { .. } => FacetItemKind::Folder,
        // ファイル名スタックの集約セルは画像の集まりなので Image バケツ (= 画像絞り込みで残る)。
        GridItem::Stack { .. } => FacetItemKind::Image,
    }
}

pub(super) fn facet_ext_for_item(item: &GridItem) -> String {
    match item {
        GridItem::Folder(_) | GridItem::ZipSeparator { .. } | GridItem::ZipDir { .. } => {
            String::new()
        }
        GridItem::Image(p)
        | GridItem::Video(p)
        | GridItem::ZipFile(p)
        | GridItem::PdfFile(p)
        | GridItem::ConvertibleArchive { path: p, .. }
        | GridItem::SearchContainer { path: p, .. } => path_extension_lower(p),
        // ファイル名スタックの集約セルは代表画像の拡張子で facet 絞り込みに乗せる。
        GridItem::Stack { representative, .. } => path_extension_lower(representative),
        GridItem::ZipImage { entry_name, .. } => Path::new(entry_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase(),
        GridItem::PdfPage { .. } => "pdf".to_string(),
    }
}

pub(super) fn tag_item_path(item: &GridItem) -> Option<&Path> {
    match item {
        GridItem::Folder(p)
        | GridItem::Image(p)
        | GridItem::Video(p)
        | GridItem::ZipFile(p)
        | GridItem::PdfFile(p)
        | GridItem::ConvertibleArchive { path: p, .. } => Some(p.as_path()),
        _ => None,
    }
}

pub(super) fn item_supports_tags(item: &GridItem) -> bool {
    tag_item_path(item).is_some()
}

pub(super) fn facet_tag_filter_applies(item: &GridItem) -> bool {
    matches!(
        item,
        GridItem::Image(_)
            | GridItem::Video(_)
            | GridItem::ZipFile(_)
            | GridItem::PdfFile(_)
            | GridItem::ConvertibleArchive { .. }
    )
}

/// AI モデル/ツールのファセット絞り込みを **この種別に適用するか**。
///
/// `facet_tag_filter_applies` と同じ思想で、**スタック集約セルは対象外 = 素通し**にする。
/// スタックは隠れメンバーの AI メタデータを集約セル単体では評価できず、`facet_ai_model_values`
/// が常に空を返すため、ゲートしないと AI モデル/ツール絞り込み中に**スタックが全落ち**する
/// (= タグ絞り込みでは素通しなのに AI 絞り込みでは全消えする、という不整合になる)。
/// メンバー単位メタデータ (タグ / AI モデル / AI ツール) の絞り込みでは、スタックは
/// 一貫して素通し扱いにする (種別 / 拡張子の絞り込みは従来どおり代表画像で評価される)。
///
/// Image / ZipImage 以外の通常コンテナ (Folder / ZIP / PDF / 動画) は従来どおり「非該当で落とす」
/// 挙動を維持する (= AI モデル絞り込み中は AI 画像だけを見せる既存仕様、v1.7.0〜)。
pub(super) fn facet_ai_filter_applies(item: &GridItem) -> bool {
    !matches!(item, GridItem::Stack { .. })
}

/// `current` が `anchor` 配下 (= 同一 or 子孫) かを case-insensitive に判定する。
/// Windows の case-insensitive FS 対応 (`C:\Photos` と `c:\photos` を同一扱い)。
/// ドライブ文字は保持するので、cross-drive の偶然一致を起こさない
/// (例: `C:\books\vol1.zip` と `D:\books\vol1.zip` は別扱い)。
///
/// component-boundary も守る (`/books/book-a` と `/books/book-a-extra` を別扱い)。
pub(super) fn path_in_subtree_ci(current: &std::path::Path, anchor: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| p.to_string_lossy().to_lowercase().replace('\\', "/");
    let cur = norm(current);
    let anc = norm(anchor);
    if cur == anc {
        return true;
    }
    cur.strip_prefix(&anc)
        .is_some_and(|tail| tail.starts_with('/'))
}

/// パスからファイル名のステム部分を小文字で取得するヘルパー。
pub(super) fn stem_lower(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// ファイルパスが AI 生成メタデータ抽出対象の画像拡張子か (大文字小文字無視)。
pub(super) fn is_ai_metadata_path(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "jfif"
        )
    })
}

/// フルスクリーン画像のメタデータ (AI プロンプト / EXIF / XMP) を読み込むワーカー本体。
/// ZipImage は ZIP エントリを 1 回だけ開いて 3 パーサー間で bytes を共有する。
/// それ以外 (Image / Video) はファイルを直接パーサーに渡す。パーサー側で
/// 必要に応じて full-file read が行われる (XMP の JPEG/PNG は全体読み)。
pub(super) fn run_metadata_load(
    key: String,
    item: GridItem,
    hidden: &[String],
    cancel: &AtomicBool,
) -> Option<MetadataLoadResult> {
    // 各段で cancel チェック。ZIP の bytes 読み → AI → EXIF → XMP の順で重い。
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let (metadata, exif, xmp, panorama) = match &item {
        GridItem::Image(p) => {
            let metadata = crate::png_metadata::extract_metadata(p);
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let exif = crate::exif_reader::read_exif(p, hidden);
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            // xtw (X/Twitter) と GPano は同じ XMP packet を参照するので 1 回読み +
            // 1 回 extract_xmp_packet にまとめる (Codex P2 第 19 ラウンド: 旧コードは
            // 2 回 fs::read + 2 回 extract を実行していた)。
            let bundle = crate::xmp_reader::read_xmp_bundle(p);
            (metadata, exif, bundle.tweet, bundle.panorama)
        }
        GridItem::Video(p) => {
            // 動画は AI/EXIF/GPano なし、XMP xtw のみ (mXD が MP4/MOV に X/Twitter 情報を埋める)
            let xmp = crate::xmp_reader::read_tweet_info(p);
            (None, None, xmp, None)
        }
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => {
            // ZIP エントリは 1 回展開して bytes を 3 パーサーで共有する。
            // XMP 系 (xtw + GPano) も bundle で 1 回 extract に統合 (Codex P2 第 19)。
            let bytes = crate::zip_loader::read_entry_bytes(zip_path, entry_name).ok();
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let metadata = bytes
                .as_ref()
                .and_then(|b| crate::png_metadata::extract_metadata_from_bytes(b));
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let exif = bytes
                .as_ref()
                .and_then(|b| crate::exif_reader::read_exif_from_bytes(b, hidden));
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let bundle = bytes
                .as_ref()
                .map(|b| crate::xmp_reader::read_xmp_bundle_from_bytes(b))
                .unwrap_or_default();
            (metadata, exif, bundle.tweet, bundle.panorama)
        }
        _ => (None, None, None, None),
    };

    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    // 外部メタデータサイドカー (FS 画像のみ。docs §11)。動画 / ZIP 内画像 / PDF ページは
    // サイドカー対象外なので None (Codex P2-6: metadata_cache_key は Video も返すため
    // GridItem::Image で明示ゲートする)。
    let sidecar = match &item {
        GridItem::Image(p) => crate::external_metadata::read_for_display(p),
        _ => None,
    };

    Some(MetadataLoadResult {
        key,
        metadata,
        exif,
        xmp,
        panorama,
        sidecar,
    })
}

pub(super) fn run_details_meta_load(
    generation: u64,
    targets: Vec<DetailsMetaTarget>,
    initial_done: usize,
    initial_failed: usize,
    total: usize,
    cache_dir: PathBuf,
    io_sem: Arc<crate::io_semaphore::GlobalIoSemaphore>,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<DetailsMetaEvent>,
) {
    let mut done = initial_done;
    let mut failed = initial_failed;
    let mut catalog_maps: std::collections::HashMap<
        PathBuf,
        Option<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    > = std::collections::HashMap::new();

    for target in targets {
        if cancel.load(Ordering::Relaxed) {
            return;
        }

        let mut created_at = None;
        let mut ai_metadata_checked = false;
        let mut ai_models = Vec::new();
        let mut ai_tool = None;
        let mut zip_entry_bytes: Option<Vec<u8>> = None;
        let mut dims = if target.load_image_dims {
            target.warm_image_dims
        } else {
            None
        };
        let mut video_probe: Option<DetailsVideoProbe> = None;
        if target.load_created_at
            && let Some(path) = details_created_time_path(&target.item)
        {
            created_at = {
                let _permit = io_sem.acquire(target.priority);
                std::fs::metadata(path)
                    .ok()
                    .and_then(|m| m.created().ok().and_then(|t| system_time_to_unix_secs(t)))
            };
        }

        if target.load_ai_metadata {
            ai_metadata_checked = true;
            let metadata = match &target.item {
                GridItem::Image(path) => {
                    let _permit = io_sem.acquire(target.priority);
                    crate::png_metadata::extract_metadata(path)
                }
                GridItem::ZipImage {
                    zip_path,
                    entry_name,
                } => {
                    if zip_entry_bytes.is_none() {
                        let _permit = io_sem.acquire(target.priority);
                        zip_entry_bytes =
                            crate::zip_loader::read_entry_bytes(zip_path, entry_name).ok();
                    }
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    zip_entry_bytes
                        .as_ref()
                        .and_then(|bytes| crate::png_metadata::extract_metadata_from_bytes(bytes))
                }
                _ => None,
            };
            if let Some(meta) = metadata.as_ref() {
                ai_models = crate::png_metadata::model_names(meta);
                ai_tool = crate::png_metadata::ai_tool_name(meta).map(str::to_string);
            }
        }

        if target.load_image_dims
            && dims.is_none()
            && let (Some(folder), Some(key)) =
                (target.catalog_folder.as_ref(), target.catalog_key.as_ref())
        {
            if !catalog_maps.contains_key(folder) {
                let map = {
                    let _permit = io_sem.acquire(target.priority);
                    crate::catalog::CatalogDb::open(&cache_dir, folder)
                        .and_then(|db| db.load_all())
                        .ok()
                };
                catalog_maps.insert(folder.clone(), map);
            }
            dims = catalog_maps
                .get(folder)
                .and_then(|m| m.as_ref())
                .and_then(|m| m.get(key))
                .filter(|entry| {
                    entry.mtime == target.source_mtime && entry.file_size == target.source_size
                })
                .and_then(|entry| entry.source_dims);
        }

        if target.load_image_dims
            && dims.is_none()
            && let GridItem::Image(path) = &target.item
        {
            let probed = {
                let _permit = io_sem.acquire(target.priority);
                crate::fast_resize::probe_dims(path)
            };
            dims = probed.map(|[w, h]| (w as u32, h as u32));
        }

        if target.load_image_dims
            && dims.is_none()
            && let GridItem::ZipImage {
                zip_path,
                entry_name,
            } = &target.item
        {
            if zip_entry_bytes.is_none() {
                let _permit = io_sem.acquire(target.priority);
                zip_entry_bytes = crate::zip_loader::read_entry_bytes(zip_path, entry_name).ok();
            }
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            dims = zip_entry_bytes
                .as_deref()
                .and_then(probe_image_dims_from_bytes);
        }

        if target.load_image_dims
            && dims.is_none()
            && let GridItem::PdfPage {
                content_type: Some(crate::pdf_loader::PdfPageContentType::Raster { w, h }),
                ..
            } = &target.item
            && *w > 0
            && *h > 0
        {
            dims = Some((*w, *h));
        }

        if target.load_video_meta
            && let GridItem::Video(path) = &target.item
        {
            video_probe = {
                let _permit = io_sem.acquire(target.priority);
                probe_video_details(path, &cancel)
            };
        }

        let image_dims_failed = matches!(
            &target.item,
            GridItem::Image(_) | GridItem::ZipImage { .. } | GridItem::PdfPage { .. }
        ) && target.load_image_dims
            && dims.is_none();
        let video_meta_failed = target.load_video_meta
            && matches!(target.item, GridItem::Video(_))
            && video_probe.is_none();
        let created_at_failed = target.load_created_at && created_at.is_none();
        failed += usize::from(image_dims_failed || video_meta_failed || created_at_failed);
        let (video_duration_secs, video_dims, video_codec) = video_probe
            .map(|probe| (probe.duration_secs, probe.dims, probe.codec))
            .unwrap_or((None, None, None));
        let meta = DetailsLazyMeta {
            source_mtime: target.source_mtime,
            source_size: target.source_size,
            created_at,
            created_at_failed,
            ai_metadata_checked,
            ai_models,
            ai_tool,
            image_dims: dims,
            image_dims_failed,
            video_duration_secs,
            video_dims,
            video_codec,
            video_meta_failed,
        };
        if tx
            .send(DetailsMetaEvent::Item {
                generation,
                key: target.key,
                meta,
            })
            .is_err()
        {
            return;
        }

        done = (done + 1).min(total);
        if tx
            .send(DetailsMetaEvent::Progress {
                generation,
                done,
                total,
            })
            .is_err()
        {
            return;
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let _ = tx.send(DetailsMetaEvent::Finished { generation, failed });
}

fn probe_image_dims_from_bytes(bytes: &[u8]) -> Option<(u32, u32)> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let (mut w, mut h) = reader.into_dimensions().ok()?;
    let orientation = crate::thumb_loader::read_exif_orientation_from_bytes(bytes);
    if matches!(orientation, 5..=8) {
        std::mem::swap(&mut w, &mut h);
    }
    Some((w, h))
}

pub(super) fn details_duration_to_secs(duration: i64) -> Option<f64> {
    if duration == i64::MIN || duration <= 0 {
        return None;
    }
    let secs = duration as f64 / 1_000_000.0;
    (secs.is_finite() && secs > 0.0).then_some(secs)
}

pub(super) fn system_time_to_unix_secs(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
}

pub(super) fn details_created_time_path(item: &GridItem) -> Option<&Path> {
    match item {
        GridItem::Folder(path)
        | GridItem::Image(path)
        | GridItem::Video(path)
        | GridItem::ZipFile(path)
        | GridItem::PdfFile(path) => Some(path.as_path()),
        GridItem::ConvertibleArchive { path, .. } | GridItem::SearchContainer { path, .. } => {
            Some(path.as_path())
        }
        // ファイル名スタック: 代表画像の作成日時を使う (実ファイル)。
        GridItem::Stack { representative, .. } => Some(representative.as_path()),
        GridItem::ZipImage { .. }
        | GridItem::ZipSeparator { .. }
        | GridItem::ZipDir { .. }
        | GridItem::PdfPage { .. } => None,
    }
}

pub(super) fn probe_video_details(path: &Path, cancel: &AtomicBool) -> Option<DetailsVideoProbe> {
    use ffmpeg::media::Type as MediaType;
    use ffmpeg_the_third as ffmpeg;

    ffmpeg::init().ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let input = ffmpeg::format::input_with_interrupt(path, move || {
        cancel.load(Ordering::Relaxed) || std::time::Instant::now() >= deadline
    })
    .ok()?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let duration_secs = details_duration_to_secs(input.duration());
    let video_stream = input.streams().best(MediaType::Video)?;
    let params = video_stream.parameters();
    let codec = params.id().name().to_string();
    let ctx = ffmpeg::codec::context::Context::from_parameters(params).ok()?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let decoder = ctx.decoder().video().ok()?;
    let dims = match (decoder.width(), decoder.height()) {
        (w, h) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    };
    Some(DetailsVideoProbe {
        duration_secs,
        dims,
        codec: (!codec.is_empty()).then_some(codec),
    })
}

pub(super) fn ctrl_f_progress_countable(item: &GridItem) -> bool {
    !matches!(item, GridItem::ZipSeparator { .. })
}

pub(super) fn ctrl_f_progress_total(items: &[GridItem]) -> usize {
    items
        .iter()
        .filter(|it| ctrl_f_progress_countable(it))
        .count()
}

pub(super) fn mark_ctrl_f_progress(progress: Option<&SearchProgressShared>, matched: bool) {
    if let Some(progress) = progress {
        progress.mark_item_done(matched);
    }
}

/// Ctrl+F メタデータ検索のワーカー本体。UI スレッドから spawn され、結果は
/// `SearchPending.rx` で受信される。`cancel` が立ったら中断して Cancelled を返す
/// (呼び出し側はキャンセル時に Pending をクリアするので Done のみ送る実装でも OK)。
pub(super) fn run_metadata_search(
    tokens: &[crate::search_query::Token],
    items: &[GridItem],
    xmp_snapshot: &std::collections::HashMap<String, Option<crate::xmp_reader::XmpTweetInfo>>,
    _fts_meta: Option<&std::sync::Arc<crate::fts_meta::FtsMetaDb>>,
    pdf_passwords: &crate::pdf_passwords::PdfPasswordStore,
    target: &crate::fts_index::SearchTarget,
    mode: crate::search_query::MatchMode,
    cancel: &AtomicBool,
    progress: Option<&SearchProgressShared>,
) -> SearchThreadResult {
    // Ctrl+F (現在地フィルタ) はインデックスを使わず、表示中アイテムを on-demand に
    // 判定する (docs/search-container-item-redesign.md §4.1)。構造アイテム
    // (Folder / ZipFile / PdfFile / ZipImage) も「持っている次元」で一貫して
    // 絞り込む: ファイル名は全種別が持ち、PDF は document info も持つ。
    let mut matches: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut xmp_additions: Vec<(String, Option<crate::xmp_reader::XmpTweetInfo>)> = Vec::new();
    let mut additions_lookup: std::collections::HashMap<
        String,
        Option<crate::xmp_reader::XmpTweetInfo>,
    > = std::collections::HashMap::new();

    // §19 target フィルタ: target が含むソースだけを判定対象にする。
    let use_name = target.includes(crate::fts_index::SourceKind::Filename);
    let use_png = target.includes(crate::fts_index::SourceKind::PngPrompt);
    let use_exif = target.includes(crate::fts_index::SourceKind::Exif);
    let use_xmp = target.includes(crate::fts_index::SourceKind::XmpTweet);
    let use_video_meta = target.includes(crate::fts_index::SourceKind::VideoMeta);
    let use_pdf_meta = target.includes(crate::fts_index::SourceKind::PdfMeta);
    // 外部メタデータサイドカー (FS 画像のみ。docs §14-5)。TARGET_CHOICES は Ctrl+F/Ctrl+G
    // で共有されるので、ここで対応しないと「サイドカー」絞り込みが無反応・「すべて」が
    // サイドカーを取りこぼす。
    let use_sidecar = target.includes(crate::fts_index::SourceKind::Sidecar);
    // Image / Video の fallback 経路は name/png/exif/xmp/video_meta/sidecar のいずれかが
    // 対象でないと結果が常に空になる。PdfMeta-only 等で無駄な per-file 走査を避ける。
    let fallback_contributes =
        use_name || use_png || use_exif || use_xmp || use_video_meta || use_sidecar;

    // Pass 1: 構造アイテム + ZIP 内画像 (名前照合中心、PDF のみ document info I/O)。
    // 構造アイテムも一貫して絞り込む (§4.1): 名前がマッチしないものは非表示にする。
    // 検索対象がファイル名次元を含まない (EXIF / タグ単独指定) なら構造アイテムは
    // 全件非表示になる ("タグで絞ったらタグを持つアイテムだけ残る" = 正しい挙動)。
    let mut zip_separators: Vec<usize> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return SearchThreadResult::Done {
                matches,
                xmp_additions,
            };
        }
        let processed_in_pass1 = match item {
            GridItem::Folder(_)
            | GridItem::ZipFile(_)
            | GridItem::ConvertibleArchive { .. }
            | GridItem::ZipDir { .. }
            | GridItem::Stack { .. } => {
                // フォルダ / ZIP / 変換対象アーカイブ / ネスト ZIP 子コンテナ / スタック集約セル:
                // ファイル名 (basename = ZipDir は最後のセグメント、Stack は prefix キー) で照合。
                // これらの子/メンバーは現 items に含まれないので名前照合のみ (plan §3.6 MVP)。
                if use_name && crate::search_query::matches_with_mode(tokens, &item.name(), mode) {
                    matches.insert(idx);
                }
                true
            }
            GridItem::PdfFile(path) => {
                // PDF: ファイル名 + PDF document info を 1 つの hay にまとめて判定する
                // (§4.1.1)。filename と title をまたぐクエリや exclude トークンを
                // 正しく扱うため、Image/Video と同じ combined-hay 方式にする
                // (2 つの hay を別々に matches すると "scan invoice" や
                // "scan -draft" を取りこぼす — Codex P2)。まずファイル名だけで
                // 部分判定し、結論が出れば document info の IPC を省く。
                let name = item.name();
                let name_hay: &str = if use_name { &name } else { "" };
                if use_pdf_meta {
                    match crate::search_query::decide_partial_with_mode(tokens, name_hay, mode) {
                        crate::search_query::PartialResult::Decided(true) => {
                            matches.insert(idx);
                        }
                        crate::search_query::PartialResult::Decided(false) => {}
                        crate::search_query::PartialResult::NeedsMore => {
                            // 保護 PDF でパスワード未保存なら get_document_info は
                            // 失敗 → doc_text 空 = ファイル名のみで判定 (= 非マッチ)。
                            let password = pdf_passwords.get(path);
                            let doc_text =
                                crate::pdf_loader::get_document_info(path, password.as_deref())
                                    .map(|info| info.as_search_text())
                                    .unwrap_or_default();
                            let hay = hay_of(&doc_text, name_hay, None);
                            if crate::search_query::matches_with_mode(tokens, &hay, mode) {
                                matches.insert(idx);
                            }
                        }
                    }
                } else if use_name && crate::search_query::matches_with_mode(tokens, name_hay, mode)
                {
                    // PDF メタが検索対象外 → ファイル名のみで照合。
                    matches.insert(idx);
                }
                true
            }
            GridItem::Image(_) | GridItem::Video(_) => {
                // Pass 2 で処理 (on-demand メタ読み取り)。
                false
            }
            GridItem::ZipImage { entry_name, .. } => {
                // §4.1.2: ZIP 内画像は常にファイル名 (エントリ basename) のみで照合。
                // ZIP を開いてメタを読む経路 (旧 Pass 3) は廃止した。
                if use_name {
                    let name = crate::zip_loader::entry_basename(entry_name);
                    if crate::search_query::matches_with_mode(tokens, name, mode) {
                        matches.insert(idx);
                    }
                }
                true
            }
            GridItem::ZipSeparator { .. } => {
                // 付随グループに可視アイテムが残るかを Pass 1 完了後に判定する。
                zip_separators.push(idx);
                false
            }
            GridItem::PdfPage { .. } => {
                // PDF ページ表示中は Ctrl+F 自体を無効化する (§4.1.1) ため通常は
                // ここに来ない。防御的に "Page N" のファイル名照合だけ残す。
                if use_name && crate::search_query::matches_with_mode(tokens, &item.name(), mode) {
                    matches.insert(idx);
                }
                true
            }
            GridItem::SearchContainer { .. } => {
                // Ctrl+F と Ctrl+G は排他なので通常出現しない。防御的に常に残す。
                matches.insert(idx);
                true
            }
        };
        if processed_in_pass1 {
            mark_ctrl_f_progress(progress, matches.contains(&idx));
        }
    }

    // ZipSeparator: 付随する ZIP グループ (separator の次〜次の separator 手前) に
    // 可視 ZipImage が残るときだけ表示する (§4.1)。ZIP 表示中のグリッドは
    // separator と ZipImage だけで構成されるため、Pass 1 完了時点でグループの
    // 可視判定は確定している。
    for &sep_idx in &zip_separators {
        let group_has_visible = ((sep_idx + 1)..items.len())
            .take_while(|&probe| !matches!(items[probe], GridItem::ZipSeparator { .. }))
            .any(|probe| matches.contains(&probe));
        if group_has_visible {
            matches.insert(sep_idx);
        }
    }

    // Pass 2: Image / Video — cheap hay で決まらなければ XMP / 動画メタを lazy 読み取り
    // (ファイル I/O)。target フィルタの use_* / fallback_contributes は Pass 1 の
    // 手前で算出済み。単一ソース選択時はそのソース由来の文字列だけで hay を作る。
    for (idx, item) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return SearchThreadResult::Done {
                matches,
                xmp_additions,
            };
        }
        let (path, is_image) = match item {
            GridItem::Image(p) => (p, true),
            GridItem::Video(p) => (p, false),
            _ => continue,
        };
        // INDEX_VERSION=5 で fts_meta fast path は廃止 (原文は Tantivy 側 STORED)。
        // Ctrl+F は表示中の数十〜数千件しか触らないので、毎回 on-demand 経路に回す。
        // target が画像系ソース
        // (Filename/PngPrompt/Exif/XmpTweet) を一つも含まないなら、fallback hay は常に
        // 空になり matches は必ず false → file I/O もバイト走査も全て無駄。
        if !fallback_contributes {
            mark_ctrl_f_progress(progress, false);
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        // 最初は PNG tEXt だけ読む (cheap hay)。EXIF / XMP は NeedsMore の時だけ lazy 読み。
        let (meta_text, meta_origin) = if is_image && use_png && is_ai_metadata_path(path) {
            crate::png_metadata::build_searchable_from_path_with_origin(path)
        } else {
            (String::new(), None)
        };
        let name_for_hay = if use_name { name.as_str() } else { "" };
        let hay_no_xmp = hay_of(&meta_text, name_for_hay, None);
        match crate::search_query::decide_partial_with_mode(tokens, &hay_no_xmp, mode) {
            crate::search_query::PartialResult::Decided(true) => {
                matches.insert(idx);
            }
            crate::search_query::PartialResult::Decided(false) => {}
            crate::search_query::PartialResult::NeedsMore => {
                let key = crate::adjustment_db::normalize_path(path);
                // XMP は target に含まれる場合のみ読む (I/O 節約)
                let xmp_opt = if use_xmp {
                    if let Some(cached) = xmp_snapshot.get(&key) {
                        cached.clone()
                    } else if let Some(added) = additions_lookup.get(&key) {
                        added.clone()
                    } else {
                        let xmp = crate::xmp_reader::read_tweet_info(path);
                        additions_lookup.insert(key.clone(), xmp.clone());
                        xmp_additions.push((key.clone(), xmp.clone()));
                        xmp
                    }
                } else {
                    None
                };
                // EXIF も同じく target に含まれる時だけ
                let mut extended_meta = meta_text.clone();
                if is_image && use_exif {
                    if let Some(exif) = crate::exif_reader::read_exif(path, &[]) {
                        let exif_part = exif_hay(
                            &exif,
                            meta_origin.is_some_and(|o| o.suppresses_exif_user_comment()),
                        );
                        if !exif_part.is_empty() {
                            if !extended_meta.is_empty() {
                                extended_meta.push('\n');
                            }
                            extended_meta.push_str(&exif_part);
                        }
                    }
                }
                if !is_image && use_video_meta {
                    let video_meta = crate::ingest_text::build_video_metadata_text(path);
                    if !video_meta.is_empty() {
                        if !extended_meta.is_empty() {
                            extended_meta.push('\n');
                        }
                        extended_meta.push_str(&video_meta);
                    }
                }
                // 外部メタデータサイドカー (FS 画像のみ): 同名 JSON/TXT の値テキストを
                // on-demand 読みして hay に載せる (docs §14-5)。matches_with_mode が hay を
                // lowercase するので未正規化テキストのままで照合できる。
                if is_image && use_sidecar {
                    if let Some(sc_text) = crate::external_metadata::read_search_text(path) {
                        if !sc_text.is_empty() {
                            if !extended_meta.is_empty() {
                                extended_meta.push('\n');
                            }
                            extended_meta.push_str(&sc_text);
                        }
                    }
                }
                let hay = hay_of(&extended_meta, name_for_hay, xmp_opt.as_ref());
                if crate::search_query::matches_with_mode(tokens, &hay, mode) {
                    matches.insert(idx);
                }
            }
        }
        mark_ctrl_f_progress(progress, matches.contains(&idx));
    }

    SearchThreadResult::Done {
        matches,
        xmp_additions,
    }
}

/// メタデータ文字列とファイル名を改行で繋いだ検索対象文字列を構築する。
/// mXD が埋めた XMP tweet 情報 (本文・投稿者・引用元) があれば末尾に追記する。
///
/// **Codex round-8 Should-fix #1 + round-9 #1 対応**:
/// fts_meta.db fast path (ingest_text::build_all_text_for_file → append_xmp) と互換な
/// 検索対象を作る。EXIF は呼び出し側で meta_text に含めて渡し、XMP 全フィールドは
/// この関数内で `ingest_text::append_xmp` と同じフィールド集合を連結する。
pub(super) fn hay_of(
    meta_text: &str,
    name: &str,
    xmp: Option<&crate::xmp_reader::XmpTweetInfo>,
) -> String {
    let mut out = if meta_text.is_empty() {
        name.to_string()
    } else {
        format!("{meta_text}\n{name}")
    };
    if let Some(x) = xmp {
        // ingest_text::append_xmp と同じ 9 フィールドを連結 (Codex round-9 Should-fix #1)
        for field in [
            x.tweet_id.as_deref(),
            x.author_screen_name.as_deref(),
            x.author_display_name.as_deref(),
            x.posted_at.as_deref(),
            x.description.as_deref(),
            x.creator.as_deref(),
            x.quoted_by_screen_name.as_deref(),
            x.quoted_by_tweet_id.as_deref(),
            x.source.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            out.push('\n');
            out.push_str(field);
        }
    }
    out
}

/// EXIF 全タグ値を 1 つの文字列に連結 (空白区切り)。
///
/// Tantivy 側 `exif_text` STORED (ingest_text::append_exif 経由で生成) と同じ形に揃え、
/// Ctrl+F の on-demand fallback 経路でも EXIF を検索対象に含める。
pub(super) fn exif_hay(info: &crate::exif_reader::ExifInfo, skip_user_comment: bool) -> String {
    let mut out = String::new();
    for (_group, tags) in &info.sections {
        for (_name, value) in tags {
            if skip_user_comment && _name == "UserComment" {
                continue;
            }
            if !value.is_empty() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(value);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_image_dims_from_bytes_reads_png_dimensions() {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(13, 7));
        let mut cursor = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();

        assert_eq!(
            probe_image_dims_from_bytes(&cursor.into_inner()),
            Some((13, 7))
        );
    }

    #[test]
    fn probe_image_dims_from_bytes_rejects_invalid_data() {
        assert_eq!(probe_image_dims_from_bytes(b"not an image"), None);
    }

    #[test]
    fn facet_member_metadata_filters_exempt_stack_consistently() {
        use std::path::PathBuf;
        let img = GridItem::Image(PathBuf::from("a.png"));
        let stack = GridItem::Stack {
            key: "a".into(),
            representative: PathBuf::from("a_0.png"),
            count: 3,
        };
        let folder = GridItem::Folder(PathBuf::from("dir"));

        // 通常画像はタグ / AI ファセットの対象。
        assert!(facet_tag_filter_applies(&img));
        assert!(facet_ai_filter_applies(&img));

        // スタック集約セルは **両方とも対象外 = 素通し** (= 全落ちしない。タグと AI で挙動を揃える)。
        assert!(!facet_tag_filter_applies(&stack));
        assert!(!facet_ai_filter_applies(&stack));

        // フォルダはタグ対象外だが AI ファセットは従来どおり「非該当で落とす」挙動を維持
        // (= facet_ai_filter_applies はスタック以外を対象にする)。
        assert!(!facet_tag_filter_applies(&folder));
        assert!(facet_ai_filter_applies(&folder));
    }
}
