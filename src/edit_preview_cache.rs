//! 非破壊編集結果をグリッドへ戻すための永続プレビューキャッシュ。
//!
//! 元画像や編集 DB は変更せず、source 解像度で erase / local-adjust / conceal まで処理した
//! 下地と comic 注釈レイヤーを分離し、最後段の crop を両方へ適用して最大辺 2048px で保存する。
//! 色調補正はサムネイル表示時に**下地だけ**へ適用し、その後で注釈レイヤーを合成する。
//! これにより fullscreen の `edit -> color -> comic` と同じ順序を保つ。
//!
//! encode / ファイル I/O / LRU prune は専用 worker だけで実行する。UI スレッドは
//! [`EditPreviewCacheService`] へ command を送るだけでブロックしない。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::page_dims::CanonicalPageDimensions;
use crate::source_coverage::{EditPreviewGeometry, SourceCoverage};

pub const PREVIEW_LONG_SIDE: u32 = 2048;
pub const PREVIEW_WEBP_QUALITY: f32 = 90.0;
pub const DEFAULT_MAX_BYTES: u64 = 1_000_000_000;
// v6: 消しゴム補完の色調合わせ前に生成した下地を再利用しない。
// v5: ZIP/PDF 内ページを親コンテナの手動代表サムネイルとして使うときも、
// container の mtime + size で stale 判定できるよう source_container_size を保持する。
const CACHE_FORMAT_VERSION: i64 = 6;

/// edit-result の上にだけ注釈を焼くための worker payload。
///
/// final composite を流用すると色調補正 / final AI / post-filter までキャッシュへ
/// 混入するため、注釈の scene と描画リソースを snapshot し、このモジュールの worker
/// で edit-result に直接合成する。
pub struct EditPreviewAnnotations {
    pub objects: Vec<comic_core::AnnotationObject>,
    pub fonts: Arc<comic_core::FontSet>,
    pub stamp_cache: std::collections::HashMap<String, Option<Arc<comic_core::RgbaOverlay>>>,
    pub source_dims: [usize; 2],
}

/// 色調補正より後に合成する注釈ラスターレイヤー。
///
/// `Multiply` は白を無効果とする RGB 係数、`Normal` は straight-alpha の通常合成。
/// comic-core の `AnnotationLayer` と同じ順序・意味をキャッシュ復元後も維持する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachedAnnotationBlend {
    Normal,
    Multiply,
}

#[derive(Debug, Clone)]
pub struct CachedAnnotationLayer {
    pub blend: CachedAnnotationBlend,
    pub image: egui::ColorImage,
}

/// サムネイル色調補正後の下地へ、キャッシュ済み注釈を z 順に合成する。
pub fn composite_cached_annotation_layers(
    base: &egui::ColorImage,
    layers: &[CachedAnnotationLayer],
) -> egui::ColorImage {
    let [width, height] = base.size;
    let mut pixels = base.pixels.clone();
    if width == 0 || height == 0 {
        return egui::ColorImage::new([width, height], pixels);
    }
    for layer in layers {
        let [layer_width, layer_height] = layer.image.size;
        let copy_width = width.min(layer_width);
        let copy_height = height.min(layer_height);
        if copy_width == 0 || copy_height == 0 {
            continue;
        }
        pixels
            .par_chunks_mut(width)
            .take(copy_height)
            .enumerate()
            .for_each(|(y, row)| {
                let layer_row = &layer.image.pixels[y * layer_width..][..copy_width];
                for (dst, src) in row[..copy_width].iter_mut().zip(layer_row) {
                    match layer.blend {
                        CachedAnnotationBlend::Normal => {
                            let [fr, fg, fb, fa] = src.to_srgba_unmultiplied();
                            if fa == 0 {
                                continue;
                            }
                            let [br, bg, bb, ba] = dst.to_srgba_unmultiplied();
                            let foreground_alpha = fa as f32 / 255.0;
                            let background_alpha = ba as f32 / 255.0;
                            let output_alpha =
                                foreground_alpha + background_alpha * (1.0 - foreground_alpha);
                            if output_alpha <= 0.0 {
                                *dst = egui::Color32::TRANSPARENT;
                                continue;
                            }
                            let blend = |foreground: u8, background: u8| -> u8 {
                                ((foreground as f32 * foreground_alpha
                                    + background as f32
                                        * background_alpha
                                        * (1.0 - foreground_alpha))
                                    / output_alpha)
                                    .round()
                                    .clamp(0.0, 255.0) as u8
                            };
                            *dst = egui::Color32::from_rgba_unmultiplied(
                                blend(fr, br),
                                blend(fg, bg),
                                blend(fb, bb),
                                (output_alpha * 255.0).round().clamp(0.0, 255.0) as u8,
                            );
                        }
                        CachedAnnotationBlend::Multiply => {
                            let [factor_r, factor_g, factor_b, _] = src.to_srgba_unmultiplied();
                            let [br, bg, bb, ba] = dst.to_srgba_unmultiplied();
                            let multiply = |value: u8, factor: u8| -> u8 {
                                ((value as u32 * factor as u32 + 127) / 255) as u8
                            };
                            *dst = egui::Color32::from_rgba_unmultiplied(
                                multiply(br, factor_r),
                                multiply(bg, factor_g),
                                multiply(bb, factor_b),
                                ba,
                            );
                        }
                    }
                }
            });
    }
    egui::ColorImage::new([width, height], pixels)
}

fn db_path() -> PathBuf {
    crate::data_dir::get().join("edit_preview_cache.db")
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn init_schema(conn: &Connection) -> rusqlite::Result<bool> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let reset = version != CACHE_FORMAT_VERSION;
    if reset {
        // v1 は comic 合成済み 1 枚だけを保存していた。v3 / v6 では MI-GAN の生成規約も
        // 更新した。いずれも派生キャッシュとして安全に全件作り直す。
        conn.execute_batch("DROP TABLE IF EXISTS edit_previews;")?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS edit_previews (
             item_key       TEXT PRIMARY KEY,
             source_mtime   INTEGER NOT NULL,
             source_size    INTEGER NOT NULL,
             source_container_size INTEGER,
             source_width   INTEGER NOT NULL,
             source_height  INTEGER NOT NULL,
             source_coverage_json TEXT,
             layout_dims_json TEXT,
             metadata_content_hash TEXT,
             cached_path    TEXT NOT NULL,
             annotation_layers_json TEXT NOT NULL,
             cached_bytes   INTEGER NOT NULL,
             updated_at     INTEGER NOT NULL,
             last_access_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_edit_previews_lru
             ON edit_previews(last_access_at, updated_at);",
    )?;
    // Coverage is an additive extension of the released v6 rows. NULL means the generation
    // region is unknown; retaining both the row and its WebPs preserves the listing preview.
    let columns = conn
        .prepare("PRAGMA table_info(edit_previews)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns
        .iter()
        .any(|column| column == "source_coverage_json")
    {
        conn.execute_batch("ALTER TABLE edit_previews ADD COLUMN source_coverage_json TEXT;")?;
    }
    if !columns.iter().any(|column| column == "layout_dims_json") {
        conn.execute_batch("ALTER TABLE edit_previews ADD COLUMN layout_dims_json TEXT;")?;
    }
    if !columns
        .iter()
        .any(|column| column == "metadata_content_hash")
    {
        conn.execute_batch("ALTER TABLE edit_previews ADD COLUMN metadata_content_hash TEXT;")?;
    }
    conn.pragma_update(None, "user_version", CACHE_FORMAT_VERSION)?;
    Ok(reset)
}

#[derive(Debug)]
pub struct EditPreviewData {
    /// 色調未補正の下地。サムネイル補正はこの画像だけへ適用する。
    pub adjustment_base: egui::ColorImage,
    /// 色調補正後に z 順で合成する注釈ラスターレイヤー。
    pub annotation_layers: Vec<CachedAnnotationLayer>,
    /// identity 色調時にそのままアップロードできる完成画像。
    pub image: egui::ColorImage,
    pub source_dims: (u32, u32),
    /// PDF page-box layout units, independent of canonical pixels and the preview raster.
    pub layout_dims: Option<(u32, u32)>,
    /// Generation-time provenance. source_dims remains the cropped listing aspect ratio.
    pub coverage: SourceCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedAnnotationLayerRecord {
    blend: CachedAnnotationBlend,
    path: String,
    bytes: u64,
}

/// Result of fixing up one row inserted by content-identity restore.
///
/// A retained row owns every returned path. `Skipped` means the copied row was removed because
/// its source files were already incomplete or its persisted layout was invalid.
pub(crate) enum RestoredPreviewCopyFixup {
    Retained(Vec<PathBuf>),
    Skipped,
}

struct EncodedAnnotationLayer {
    blend: CachedAnnotationBlend,
    webp: Vec<u8>,
}

#[derive(Clone, Copy)]
enum PreviewSourceValidation {
    Page { mtime: i64, size: i64 },
    Container { mtime: i64, size: i64 },
}

/// SQLite は対応表と LRU だけを保持し、WebP 本体は個別ファイルに置く。
///
/// BLOB を SQLite に入れないのは、上限 prune 後に DB ファイルの予約領域が縮まず、
/// ユーザーが指定したディスク容量と実使用量が乖離するのを避けるため。
pub struct EditPreviewCacheDb {
    conn: Mutex<Connection>,
    root: PathBuf,
    /// Process-local generation of rows visible through this DB owner. Thumbnail workers echo the
    /// sampled value so a result read before a successful whole-cache clear cannot materialize
    /// after the clear acknowledgement.
    epoch: AtomicU64,
}

impl EditPreviewCacheDb {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(&db_path())
    }

    pub(crate) fn open_at(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let reset = init_schema(&conn)?;
        let root = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("edit_preview_cache");
        if reset {
            let _ = std::fs::remove_dir_all(&root);
        }
        Ok(Self {
            conn: Mutex::new(conn),
            root,
            epoch: AtomicU64::new(0),
        })
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// page source の mtime + size が一致する WebP を `display_px` へ縮小して返す。
    /// 不一致・欠損・破損行はその場で掃除する。サムネイル worker からだけ
    /// 呼ばれるため、ファイル読み込みと Lanczos3 縮小で UI はブロックしない。
    pub fn load(
        &self,
        item_key: &str,
        source_mtime: i64,
        source_size: i64,
        display_px: u32,
    ) -> Option<EditPreviewData> {
        self.load_validated(
            item_key,
            PreviewSourceValidation::Page {
                mtime: source_mtime,
                size: source_size,
            },
            display_px,
        )
    }

    /// ZIP/PDF 内ページを親コンテナの代表として読む場合の検証経路。
    ///
    /// 親 `LoadRequest` は ZIP entry の非圧縮サイズを持たないため、通常の page identity
    /// では照合できない。保存 worker が記録した container size と、thumbnail worker が
    /// 読んだ現在の container mtime + size を照合する。どちらの I/O も UI thread 外。
    pub fn load_for_container(
        &self,
        item_key: &str,
        container_mtime: i64,
        container_size: i64,
        display_px: u32,
    ) -> Option<EditPreviewData> {
        self.load_validated(
            item_key,
            PreviewSourceValidation::Container {
                mtime: container_mtime,
                size: container_size,
            },
            display_px,
        )
    }

    fn load_validated(
        &self,
        item_key: &str,
        validation: PreviewSourceValidation,
        display_px: u32,
    ) -> Option<EditPreviewData> {
        let row: Option<(
            i64,
            i64,
            Option<i64>,
            i64,
            i64,
            String,
            String,
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
        )> = {
            let conn = self.conn.lock().ok()?;
            conn.query_row(
                "SELECT source_mtime, source_size, source_container_size,
                        source_width, source_height, cached_path,
                        annotation_layers_json, updated_at, source_coverage_json, layout_dims_json,
                        metadata_content_hash
                 FROM edit_previews WHERE item_key = ?1",
                params![item_key],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                    ))
                },
            )
            .optional()
            .ok()?
        };
        let (
            stored_mtime,
            stored_size,
            stored_container_size,
            width,
            height,
            cached_path,
            annotation_layers_json,
            updated_at,
            source_coverage_json,
            layout_dims_json,
            metadata_content_hash,
        ) = row?;
        let source_matches = match validation {
            PreviewSourceValidation::Page { mtime, size } => {
                stored_mtime == mtime && stored_size == size
            }
            PreviewSourceValidation::Container { mtime, size } => {
                stored_mtime == mtime && stored_container_size == Some(size)
            }
        };
        if !source_matches || width <= 0 || height <= 0 {
            self.delete_if_row_matches(
                item_key,
                stored_mtime,
                stored_size,
                &cached_path,
                &annotation_layers_json,
                updated_at,
            );
            return None;
        }

        // A released writer can replace the WebP through its old UPSERT without updating any new
        // columns. Trust metadata only when it names this exact artifact. Bind to the content-hash
        // basename rather than the directory so content-identity copies retain their provenance.
        let metadata_matches_material = metadata_content_hash.as_deref().is_some_and(|hash| {
            Path::new(&cached_path)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(format!("{hash}.base.webp").as_str())
        });
        let source_coverage_json = metadata_matches_material
            .then_some(source_coverage_json)
            .flatten();
        let layout_dims_json = metadata_matches_material
            .then_some(layout_dims_json)
            .flatten();
        // Malformed metadata bound to this artifact remains an error, not legacy provenance.
        let coverage = match source_coverage_json.as_deref() {
            None => SourceCoverage::Unknown,
            Some(json) => match serde_json::from_str::<EditPreviewGeometry>(json) {
                Ok(geometry)
                    if geometry.is_valid()
                        && (geometry.pixel_bounds.2, geometry.pixel_bounds.3)
                            == (width as u32, height as u32) =>
                {
                    SourceCoverage::Known(geometry)
                }
                _ => {
                    crate::logger::log(
                        "edit_preview_cache: invalid source coverage metadata; preserving cached row",
                    );
                    return None;
                }
            },
        };
        let layout_dims = match layout_dims_json.as_deref() {
            None => None,
            Some(json) => match serde_json::from_str::<(u32, u32)>(json) {
                Ok((width, height)) if width > 0 && height > 0 => Some((width, height)),
                _ => {
                    crate::logger::log(
                        "edit_preview_cache: invalid layout dimensions metadata; preserving cached row",
                    );
                    return None;
                }
            },
        };

        let records: Vec<CachedAnnotationLayerRecord> =
            match serde_json::from_str(&annotation_layers_json) {
                Ok(records) => records,
                Err(_) => {
                    self.delete_if_row_matches(
                        item_key,
                        stored_mtime,
                        stored_size,
                        &cached_path,
                        &annotation_layers_json,
                        updated_at,
                    );
                    return None;
                }
            };
        let cached_path_ref = Path::new(&cached_path);
        if !cache_path_is_owned(&self.root, cached_path_ref)
            || records
                .iter()
                .any(|record| !cache_path_is_owned(&self.root, Path::new(&record.path)))
        {
            self.delete_if_row_matches(
                item_key,
                stored_mtime,
                stored_size,
                &cached_path,
                &annotation_layers_json,
                updated_at,
            );
            return None;
        }
        let webp = match std::fs::read(cached_path_ref) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            _ => {
                self.delete_if_row_matches(
                    item_key,
                    stored_mtime,
                    stored_size,
                    &cached_path,
                    &annotation_layers_json,
                    updated_at,
                );
                return None;
            }
        };
        let adjustment_base = crate::catalog::decode_thumb_to_color_image(&webp).or_else(|| {
            self.delete_if_row_matches(
                item_key,
                stored_mtime,
                stored_size,
                &cached_path,
                &annotation_layers_json,
                updated_at,
            );
            None
        })?;
        let cached_preview_size = adjustment_base.size;
        let display_dims = fit_output_dims(cached_preview_size, display_px)?;
        let adjustment_base = resize_color_image_to_dims(adjustment_base, display_dims)?;
        let mut annotation_layers = Vec::with_capacity(records.len());
        for record in &records {
            let layer = std::fs::read(&record.path)
                .ok()
                .filter(|bytes| !bytes.is_empty())
                .and_then(|bytes| crate::catalog::decode_thumb_to_color_image(&bytes));
            let Some(image) = layer.filter(|image| image.size == cached_preview_size) else {
                self.delete_if_row_matches(
                    item_key,
                    stored_mtime,
                    stored_size,
                    &cached_path,
                    &annotation_layers_json,
                    updated_at,
                );
                return None;
            };
            let image = resize_color_image_to_dims(image, display_dims)?;
            annotation_layers.push(CachedAnnotationLayer {
                blend: record.blend,
                image,
            });
        }
        let image = composite_cached_annotation_layers(&adjustment_base, &annotation_layers);

        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "UPDATE edit_previews SET last_access_at = ?1
                 WHERE item_key = ?2 AND cached_path = ?3 AND updated_at = ?4",
                params![now_secs(), item_key, &cached_path, updated_at],
            );
        }
        Some(EditPreviewData {
            adjustment_base,
            annotation_layers,
            image,
            source_dims: (width as u32, height as u32),
            layout_dims,
            coverage,
        })
    }

    fn save_encoded(
        &self,
        item_key: &str,
        source_mtime: i64,
        source_size: i64,
        source_container_size: Option<i64>,
        geometry: EditPreviewGeometry,
        layout_dims: Option<(u32, u32)>,
        base_webp: &[u8],
        annotation_layers: &[EncodedAnnotationLayer],
    ) -> Result<(), String> {
        if base_webp.is_empty() || annotation_layers.iter().any(|layer| layer.webp.is_empty()) {
            return Err("encoded preview was empty".to_string());
        }
        if !geometry.is_valid() {
            return Err("invalid edit preview source geometry".to_string());
        }
        let source_dims = (geometry.pixel_bounds.2, geometry.pixel_bounds.3);
        let source_coverage_json = serde_json::to_string(&geometry).map_err(|e| e.to_string())?;
        let layout_dims_json = layout_dims
            .filter(|&(width, height)| width > 0 && height > 0)
            .map(|dims| serde_json::to_string(&dims))
            .transpose()
            .map_err(|e| e.to_string())?;
        let mut content_hasher = Sha256::new();
        // Identical cropped pixels at different source positions are distinct materials. Including
        // provenance also keeps stale-row cleanup correlated when saves occur in the same second.
        content_hasher.update(source_coverage_json.as_bytes());
        if let Some(json) = &layout_dims_json {
            content_hasher.update(json.as_bytes());
        }
        content_hasher.update(base_webp);
        for layer in annotation_layers {
            content_hasher.update(match layer.blend {
                CachedAnnotationBlend::Normal => b"normal".as_slice(),
                CachedAnnotationBlend::Multiply => b"multiply".as_slice(),
            });
            content_hasher.update(&layer.webp);
        }
        let content_hash = format!("{:x}", content_hasher.finalize());
        let dir = cache_dir_for_item(&self.root, item_key);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let final_path = dir.join(format!("{content_hash}.base.webp"));
        write_cache_file(&final_path, base_webp)?;
        let mut layer_records = Vec::with_capacity(annotation_layers.len());
        for (index, layer) in annotation_layers.iter().enumerate() {
            let kind = match layer.blend {
                CachedAnnotationBlend::Normal => "normal",
                CachedAnnotationBlend::Multiply => "multiply",
            };
            let path = dir.join(format!("{content_hash}.{index}.{kind}.webp"));
            write_cache_file(&path, &layer.webp)?;
            layer_records.push(CachedAnnotationLayerRecord {
                blend: layer.blend,
                path: path.to_string_lossy().to_string(),
                bytes: layer.webp.len() as u64,
            });
        }

        let final_path_string = final_path.to_string_lossy().to_string();
        let annotation_layers_json =
            serde_json::to_string(&layer_records).map_err(|e| e.to_string())?;
        let old_paths: Vec<String> = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            let old: Option<(String, String)> = conn
                .query_row(
                    "SELECT cached_path, annotation_layers_json
                 FROM edit_previews WHERE item_key = ?1",
                    params![item_key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            old.map(|(path, layers)| cache_paths(&path, &layers))
                .unwrap_or_default()
        };

        let now = now_secs();
        let cached_bytes =
            base_webp.len() as u64 + layer_records.iter().map(|record| record.bytes).sum::<u64>();
        {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO edit_previews
                 (item_key, source_mtime, source_size, source_container_size,
                  source_width, source_height,
                  cached_path, annotation_layers_json, cached_bytes, updated_at, last_access_at,
                  source_coverage_json, layout_dims_json, metadata_content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11, ?12, ?13)
                 ON CONFLICT(item_key) DO UPDATE SET
                    source_mtime=excluded.source_mtime,
                    source_size=excluded.source_size,
                    source_container_size=excluded.source_container_size,
                    source_width=excluded.source_width,
                    source_height=excluded.source_height,
                    source_coverage_json=excluded.source_coverage_json,
                    layout_dims_json=excluded.layout_dims_json,
                    metadata_content_hash=excluded.metadata_content_hash,
                    cached_path=excluded.cached_path,
                    annotation_layers_json=excluded.annotation_layers_json,
                    cached_bytes=excluded.cached_bytes,
                    updated_at=excluded.updated_at,
                    last_access_at=excluded.last_access_at",
                params![
                    item_key,
                    source_mtime,
                    source_size,
                    source_container_size,
                    source_dims.0 as i64,
                    source_dims.1 as i64,
                    &final_path_string,
                    &annotation_layers_json,
                    cached_bytes as i64,
                    now,
                    &source_coverage_json,
                    &layout_dims_json,
                    &content_hash,
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        let current_paths: std::collections::HashSet<&str> =
            std::iter::once(final_path_string.as_str())
                .chain(layer_records.iter().map(|record| record.path.as_str()))
                .collect();
        for old_path in old_paths {
            if !current_paths.contains(old_path.as_str()) {
                remove_file_and_empty_parents(&self.root, Path::new(&old_path));
            }
        }
        Ok(())
    }

    /// Returns `true` only when an existing preview row was removed.
    ///
    /// Callers use this distinction to avoid publishing a cache invalidation for a read-only
    /// viewer close when there was no cached preview in the first place. Explicit edit mutations
    /// may still choose to publish an invalidation even when this returns `false`.
    pub fn delete(&self, item_key: &str) -> bool {
        let paths: Vec<String> = {
            let Ok(conn) = self.conn.lock() else {
                return false;
            };
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT cached_path, annotation_layers_json
                 FROM edit_previews WHERE item_key = ?1",
                    params![item_key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .ok()
                .flatten();
            let Some((path, layers)) = row else {
                return false;
            };
            let Ok(deleted) = conn.execute(
                "DELETE FROM edit_previews WHERE item_key = ?1",
                params![item_key],
            ) else {
                return false;
            };
            if deleted == 0 {
                return false;
            }
            cache_paths(&path, &layers)
        };
        for path in paths {
            remove_file_and_empty_parents(&self.root, Path::new(&path));
        }
        true
    }

    fn delete_if_row_matches(
        &self,
        item_key: &str,
        source_mtime: i64,
        source_size: i64,
        cached_path: &str,
        annotation_layers_json: &str,
        updated_at: i64,
    ) {
        let deleted = self
            .conn
            .lock()
            .ok()
            .and_then(|conn| {
                conn.execute(
                    "DELETE FROM edit_previews
                     WHERE item_key = ?1 AND source_mtime = ?2 AND source_size = ?3
                       AND cached_path = ?4 AND updated_at = ?5",
                    params![item_key, source_mtime, source_size, cached_path, updated_at],
                )
                .ok()
            })
            .unwrap_or(0);
        if deleted > 0 {
            for path in cache_paths(cached_path, annotation_layers_json) {
                remove_file_and_empty_parents(&self.root, Path::new(&path));
            }
        }
    }

    pub fn clear(&self) -> Result<(), String> {
        let paths = self.all_paths();
        let conn = self
            .conn
            .lock()
            .map_err(|_| "edit preview cache DB mutex is poisoned".to_string())?;
        conn.execute("DELETE FROM edit_previews", [])
            .map_err(|error| error.to_string())?;
        // The DB DELETE is the cache-clear linearization point. Keep the bump under the same lock:
        // a loader that read a row first returns the previous epoch, while one that samples the new
        // epoch can only observe the post-clear DB contents.
        self.epoch.fetch_add(1, Ordering::AcqRel);
        drop(conn);
        for path in paths {
            remove_file_and_empty_parents(&self.root, Path::new(&path));
        }
        if let Err(error) = std::fs::remove_dir_all(&self.root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::logger::log(format!(
                "edit_preview_cache: orphan file cleanup failed after DB clear: {error}"
            ));
        }
        Ok(())
    }

    pub fn prune(&self, max_bytes: u64) {
        if max_bytes == 0 {
            if let Err(error) = self.clear() {
                crate::logger::log(format!(
                    "edit_preview_cache: zero-budget clear failed: {error}"
                ));
            }
            return;
        }
        let mut rows: Vec<(String, Vec<String>, u64)> = {
            let Ok(conn) = self.conn.lock() else {
                return;
            };
            let Ok(mut stmt) = conn.prepare(
                "SELECT item_key, cached_path, annotation_layers_json, cached_bytes
                 FROM edit_previews
                 ORDER BY last_access_at ASC, updated_at ASC",
            ) else {
                return;
            };
            let Ok(mapped) = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    cache_paths(&r.get::<_, String>(1)?, &r.get::<_, String>(2)?),
                    r.get::<_, i64>(3)?.max(0) as u64,
                ))
            }) else {
                return;
            };
            mapped.flatten().collect()
        };
        let mut total: u64 = rows.iter().map(|(_, _, bytes)| *bytes).sum();
        if total <= max_bytes {
            return;
        }
        let mut removed = Vec::new();
        for (key, paths, bytes) in rows.drain(..) {
            if total <= max_bytes {
                break;
            }
            total = total.saturating_sub(bytes);
            removed.push((key, paths));
        }
        if let Ok(mut conn) = self.conn.lock()
            && let Ok(tx) = conn.transaction()
        {
            for (key, _) in &removed {
                let _ = tx.execute(
                    "DELETE FROM edit_previews WHERE item_key = ?1",
                    params![key],
                );
            }
            let _ = tx.commit();
        }
        for (_, paths) in removed {
            for path in paths {
                remove_file_and_empty_parents(&self.root, Path::new(&path));
            }
        }
    }

    fn all_paths(&self) -> Vec<String> {
        let Ok(conn) = self.conn.lock() else {
            return Vec::new();
        };
        let Ok(mut stmt) =
            conn.prepare("SELECT cached_path, annotation_layers_json FROM edit_previews")
        else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |r| {
            Ok(cache_paths(
                &r.get::<_, String>(0)?,
                &r.get::<_, String>(1)?,
            ))
        }) else {
            return Vec::new();
        };
        rows.flatten().flatten().collect()
    }

    #[cfg(test)]
    fn total_bytes(&self) -> u64 {
        self.conn
            .lock()
            .ok()
            .and_then(|conn| {
                conn.query_row(
                    "SELECT COALESCE(SUM(cached_bytes), 0) FROM edit_previews",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
            })
            .unwrap_or(0)
            .max(0) as u64
    }
}

/// Give a content-identity restore copy its own WebP files and rewrite the copied row to them.
///
/// This is deliberately a post-copy fixup: the generic SQLite copier remains schema-agnostic,
/// while this cache module owns the persisted file-layout contract. The caller runs on the
/// restore worker and keeps the SQLite transaction open across this operation.
pub(crate) fn fixup_restored_preview_copy(
    tx: &rusqlite::Transaction<'_>,
    root: &Path,
    source_key: &str,
    destination_key: &str,
) -> Result<RestoredPreviewCopyFixup, String> {
    let source_row: Option<(String, String)> = tx
        .query_row(
            "SELECT cached_path, annotation_layers_json
               FROM edit_previews WHERE item_key = ?1",
            [source_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((cached_path, annotation_layers_json)) = source_row else {
        delete_restored_preview_row(tx, destination_key)?;
        return Ok(RestoredPreviewCopyFixup::Skipped);
    };
    let Ok(source_layers) =
        serde_json::from_str::<Vec<CachedAnnotationLayerRecord>>(&annotation_layers_json)
    else {
        delete_restored_preview_row(tx, destination_key)?;
        return Ok(RestoredPreviewCopyFixup::Skipped);
    };
    let Some((copies, destination_cached_path, destination_layers)) = restored_preview_copy_layout(
        root,
        source_key,
        destination_key,
        Path::new(&cached_path),
        &source_layers,
    ) else {
        delete_restored_preview_row(tx, destination_key)?;
        return Ok(RestoredPreviewCopyFixup::Skipped);
    };

    // Validate the complete source set before creating anything. An incomplete cache row is
    // intentionally not restored: producing no destination row is safer than another dangling
    // row that can only heal after the full image is viewed again.
    if copies.iter().any(|(source, _)| {
        std::fs::metadata(source)
            .map(|metadata| !metadata.is_file() || metadata.len() == 0)
            .unwrap_or(true)
    }) {
        delete_restored_preview_row(tx, destination_key)?;
        return Ok(RestoredPreviewCopyFixup::Skipped);
    }

    let destination_dir = cache_dir_for_item(root, destination_key);
    std::fs::create_dir_all(&destination_dir).map_err(|error| error.to_string())?;
    let mut copied_paths = Vec::with_capacity(copies.len());
    for (source, destination) in &copies {
        if let Err(error) = std::fs::copy(source, destination) {
            cleanup_restored_preview_copy_files(root, &copied_paths);
            return Err(format!(
                "copy {} -> {}: {error}",
                source.display(),
                destination.display()
            ));
        }
        copied_paths.push(destination.clone());
    }

    let destination_layers_json = match serde_json::to_string(&destination_layers) {
        Ok(json) => json,
        Err(error) => {
            cleanup_restored_preview_copy_files(root, &copied_paths);
            return Err(error.to_string());
        }
    };
    let updated = match tx.execute(
        "UPDATE edit_previews
            SET cached_path = ?1, annotation_layers_json = ?2
          WHERE item_key = ?3 AND cached_path = ?4",
        params![
            destination_cached_path.to_string_lossy(),
            destination_layers_json,
            destination_key,
            cached_path,
        ],
    ) {
        Ok(updated) => updated,
        Err(error) => {
            cleanup_restored_preview_copy_files(root, &copied_paths);
            return Err(error.to_string());
        }
    };
    if updated != 1 {
        cleanup_restored_preview_copy_files(root, &copied_paths);
        return Err(format!(
            "copied edit preview row changed before path fixup: {destination_key}"
        ));
    }
    Ok(RestoredPreviewCopyFixup::Retained(copied_paths))
}

fn restored_preview_copy_layout(
    root: &Path,
    source_key: &str,
    destination_key: &str,
    cached_path: &Path,
    source_layers: &[CachedAnnotationLayerRecord],
) -> Option<(
    Vec<(PathBuf, PathBuf)>,
    PathBuf,
    Vec<CachedAnnotationLayerRecord>,
)> {
    let source_dir = cache_dir_for_item(root, source_key);
    let destination_dir = cache_dir_for_item(root, destination_key);
    if cached_path.parent()? != source_dir {
        return None;
    }
    let base_filename = cached_path.file_name()?.to_str()?;
    let content_hash = base_filename.strip_suffix(".base.webp")?;
    if content_hash.len() != 64
        || !content_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    if cached_path != source_dir.join(format!("{content_hash}.base.webp")) {
        return None;
    }

    let destination_cached_path = destination_dir.join(base_filename);
    let mut copies = vec![(cached_path.to_path_buf(), destination_cached_path.clone())];
    let mut destination_layers = Vec::with_capacity(source_layers.len());
    for (index, layer) in source_layers.iter().enumerate() {
        let kind = match layer.blend {
            CachedAnnotationBlend::Normal => "normal",
            CachedAnnotationBlend::Multiply => "multiply",
        };
        let filename = format!("{content_hash}.{index}.{kind}.webp");
        let source_path = Path::new(&layer.path);
        if source_path != source_dir.join(&filename) {
            return None;
        }
        let destination_path = destination_dir.join(filename);
        copies.push((source_path.to_path_buf(), destination_path.clone()));
        destination_layers.push(CachedAnnotationLayerRecord {
            blend: layer.blend,
            path: destination_path.to_string_lossy().into_owned(),
            bytes: layer.bytes,
        });
    }
    Some((copies, destination_cached_path, destination_layers))
}

fn delete_restored_preview_row(
    tx: &rusqlite::Transaction<'_>,
    destination_key: &str,
) -> Result<(), String> {
    tx.execute(
        "DELETE FROM edit_previews WHERE item_key = ?1",
        [destination_key],
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn cache_dir_for_item(root: &Path, item_key: &str) -> PathBuf {
    let key_hash = format!("{:x}", Sha256::digest(item_key.as_bytes()));
    root.join(&key_hash[..2]).join(key_hash)
}

pub(crate) fn cleanup_restored_preview_copy_files(root: &Path, paths: &[PathBuf]) {
    for path in paths {
        remove_file_and_empty_parents(root, path);
    }
}

fn cache_paths(cached_path: &str, annotation_layers_json: &str) -> Vec<String> {
    let mut paths = vec![cached_path.to_string()];
    if let Ok(records) =
        serde_json::from_str::<Vec<CachedAnnotationLayerRecord>>(annotation_layers_json)
    {
        paths.extend(records.into_iter().map(|record| record.path));
    }
    paths
}

fn write_cache_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, bytes).map_err(|e| e.to_string())?;
    if let Err(error) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        if !path.exists() {
            return Err(error.to_string());
        }
    }
    Ok(())
}

fn remove_file_and_empty_parents(root: &Path, path: &Path) {
    // cached_path は自前 DB の値だが、破損・手動改変された DB から data dir 外を
    // 削除しないよう、実ファイル操作の ownership boundary でも検証する。
    if !cache_path_is_owned(root, path) {
        return;
    }
    let _ = std::fs::remove_file(path);
    let mut parent = path.parent();
    while let Some(dir) = parent {
        if dir == root || !dir.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
        parent = dir.parent();
    }
}

fn cache_path_is_owned(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    !relative.as_os_str().is_empty()
        && relative.components().all(|component| {
            matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}

fn bake_cached_annotation_layers(
    base: &egui::ColorImage,
    annotations: EditPreviewAnnotations,
) -> Vec<CachedAnnotationLayer> {
    let [width, height] = base.size;
    let source_long = annotations.source_dims[0]
        .max(annotations.source_dims[1])
        .max(1);
    let scale = width.max(height) as f32 / source_long as f32;
    let objects = if (scale - 1.0).abs() > 1e-4 {
        comic_core::scale_scene(&annotations.objects, scale)
    } else {
        annotations.objects
    };
    let mut stamp_cache = annotations.stamp_cache;
    let cancel = AtomicBool::new(false);
    let (stamps, _updates, _decode_ms) = crate::comic_stamp::build_stamp_images_from_cache_snapshot(
        &objects,
        &mut stamp_cache,
        &cancel,
    );
    let layers =
        comic_core::bake_annotation_layers(&objects, width, height, &annotations.fonts, &stamps);
    layers
        .into_iter()
        .map(|layer| {
            let (blend, overlay) = match layer {
                comic_core::AnnotationLayer::Normal(overlay) => {
                    (CachedAnnotationBlend::Normal, overlay)
                }
                comic_core::AnnotationLayer::Multiply(overlay) => {
                    (CachedAnnotationBlend::Multiply, overlay)
                }
            };
            let pixels = overlay
                .pixels
                .chunks_exact(4)
                .map(|rgba| {
                    egui::Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3])
                })
                .collect();
            CachedAnnotationLayer {
                blend,
                image: egui::ColorImage::new([overlay.w, overlay.h], pixels),
            }
        })
        .collect()
}

fn prepare_preview_components(
    pixels: Arc<egui::ColorImage>,
    annotations: Option<EditPreviewAnnotations>,
    crop: Option<crate::export_crop::CropRect>,
    coordinate_dims: CanonicalPageDimensions,
) -> Option<(
    Arc<egui::ColorImage>,
    Vec<CachedAnnotationLayer>,
    EditPreviewGeometry,
)> {
    let layers = annotations
        .map(|annotations| bake_cached_annotation_layers(&pixels, annotations))
        .unwrap_or_default();
    crop_preview_components(pixels, layers, crop, coordinate_dims)
}

fn crop_preview_components(
    mut prepared: Arc<egui::ColorImage>,
    mut layers: Vec<CachedAnnotationLayer>,
    crop: Option<crate::export_crop::CropRect>,
    coordinate_dims: CanonicalPageDimensions,
) -> Option<(
    Arc<egui::ColorImage>,
    Vec<CachedAnnotationLayer>,
    EditPreviewGeometry,
)> {
    let [width, height] = prepared.size;
    if layers.iter().any(|layer| layer.image.size != prepared.size) {
        return None;
    }
    let (x, y, w, h) = crop
        .map(|crop| crop.pixel_bounds(width, height))
        .unwrap_or((0, 0, width, height));
    let geometry = EditPreviewGeometry::new(
        coordinate_dims,
        (u32::try_from(width).ok()?, u32::try_from(height).ok()?),
        (
            u32::try_from(x).ok()?,
            u32::try_from(y).ok()?,
            u32::try_from(w).ok()?,
            u32::try_from(h).ok()?,
        ),
    )?;
    if let Some(crop) = crop {
        prepared = Arc::new(crate::export_crop::crop_color_image(&prepared, crop).ok()?);
        for layer in &mut layers {
            layer.image = crate::export_crop::crop_color_image(&layer.image, crop).ok()?;
        }
    }
    Some((prepared, layers, geometry))
}

fn fit_output_dims([width, height]: [usize; 2], max_side: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let width = width as u32;
    let height = height as u32;
    let max_side = max_side.max(1);
    if width <= max_side && height <= max_side {
        return Some((width, height));
    }
    let scale = (max_side as f64 / width as f64).min(max_side as f64 / height as f64);
    Some((
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    ))
}

fn preview_output_dims(size: [usize; 2]) -> Option<(u32, u32)> {
    fit_output_dims(size, PREVIEW_LONG_SIDE)
}

fn color_image_to_premultiplied_rgba(pixels: &egui::ColorImage) -> Option<image::RgbaImage> {
    let [width, height] = pixels.size;
    if width == 0 || height == 0 || pixels.pixels.len() != width.saturating_mul(height) {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels.pixels.len().saturating_mul(4));
    for pixel in &pixels.pixels {
        // Color32 は gamma-space premultiplied alpha。透明画素の黒 RGB を
        // 隣接色へ混ぜないよう、縮小中もその表現を保つ。
        rgba.extend_from_slice(&pixel.to_array());
    }
    image::RgbaImage::from_raw(width as u32, height as u32, rgba)
}

fn premultiplied_rgba_to_unmultiplied_bytes(pixels: &image::RgbaImage) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixels.len());
    for pixel in pixels.pixels() {
        let [r, g, b, a] = pixel.0;
        rgba.extend_from_slice(
            &egui::Color32::from_rgba_premultiplied(r, g, b, a).to_srgba_unmultiplied(),
        );
    }
    rgba
}

fn resize_color_image_to_dims(
    pixels: egui::ColorImage,
    output_dims: (u32, u32),
) -> Option<egui::ColorImage> {
    if pixels.size == [output_dims.0 as usize, output_dims.1 as usize] {
        return Some(pixels);
    }
    let rgba = color_image_to_premultiplied_rgba(&pixels)?;
    let resized = crate::fast_resize::resize_rgba8_exact(
        &rgba,
        output_dims.0,
        output_dims.1,
        crate::fast_resize::Quality::Lanczos3,
    );
    Some(egui::ColorImage::from_rgba_premultiplied(
        [resized.width() as usize, resized.height() as usize],
        resized.as_raw(),
    ))
}

fn encode_preview_exact(
    pixels: &egui::ColorImage,
    output_dims: (u32, u32),
    lossless: bool,
) -> Option<Vec<u8>> {
    let rgba = color_image_to_premultiplied_rgba(pixels)?;
    let resized = crate::fast_resize::resize_rgba8_exact(
        &rgba,
        output_dims.0,
        output_dims.1,
        crate::fast_resize::Quality::Lanczos3,
    );
    // WebP encoder の入力は straight alpha なので、フィルタ後にだけ
    // premultiplied から戻す。先に戻すと透過画素の黒が縮小で混入する。
    let unmultiplied = premultiplied_rgba_to_unmultiplied_bytes(&resized);
    let encoder = webp::Encoder::from_rgba(&unmultiplied, resized.width(), resized.height());
    Some(if lossless {
        encoder.encode_lossless().to_vec()
    } else {
        encoder.encode(PREVIEW_WEBP_QUALITY).to_vec()
    })
}

#[cfg(test)]
fn encode_preview(pixels: &egui::ColorImage) -> Option<Vec<u8>> {
    encode_preview_exact(pixels, preview_output_dims(pixels.size)?, false)
}

fn encode_preview_components(
    base: &egui::ColorImage,
    layers: &[CachedAnnotationLayer],
) -> Option<(Vec<u8>, Vec<EncodedAnnotationLayer>)> {
    let output_dims = preview_output_dims(base.size)?;
    let base_webp = encode_preview_exact(base, output_dims, false)?;
    let mut encoded_layers = Vec::with_capacity(layers.len());
    for layer in layers {
        if layer.image.size != base.size {
            return None;
        }
        encoded_layers.push(EncodedAnnotationLayer {
            blend: layer.blend,
            // 透明縁と Multiply の「白 = 無効果」を壊さないよう、注釈だけは lossless。
            webp: encode_preview_exact(&layer.image, output_dims, true)?,
        });
    }
    Some((base_webp, encoded_layers))
}

pub enum EditPreviewEvent {
    Saved { item_key: String },
    Invalidated { item_key: String },
    Cleared,
}

enum EditPreviewCommand {
    Save {
        item_key: String,
        source_mtime: i64,
        source_size: i64,
        source_container_path: Option<PathBuf>,
        coordinate_dims: CanonicalPageDimensions,
        layout_dims: Option<(u32, u32)>,
        pixels: Arc<egui::ColorImage>,
        annotations: Option<EditPreviewAnnotations>,
        crop: Option<crate::export_crop::CropRect>,
        max_bytes: u64,
        repaint_ctx: Option<egui::Context>,
    },
    Delete {
        item_key: String,
        /// Explicit edit mutations must refresh a materialized thumbnail even when no persistent
        /// preview existed. A read-only viewer close only refreshes when it actually removed one.
        notify_if_missing: bool,
    },
    Prune {
        max_bytes: u64,
    },
    Clear {
        completed: Option<mpsc::SyncSender<Result<(), String>>>,
    },
}

fn publish_delete_invalidation(removed: bool, notify_if_missing: bool) -> bool {
    removed || notify_if_missing
}

fn preview_save_stamp(
    source_mtime: i64,
    source_size: i64,
    container: Option<&Path>,
    resolve_epub: impl FnOnce(&Path) -> Option<(i64, i64)>,
) -> Option<(i64, i64, Option<i64>)> {
    match container {
        Some(path)
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("epub")) =>
        {
            let (generation_id, generation_size) = resolve_epub(path)?;
            Some((generation_id, generation_size, Some(generation_size)))
        }
        other => Some((
            source_mtime,
            source_size,
            other
                .and_then(|path| std::fs::metadata(path).ok())
                .map(|meta| meta.len() as i64),
        )),
    }
}

pub struct EditPreviewCacheService {
    db: Arc<EditPreviewCacheDb>,
    tx: mpsc::Sender<EditPreviewCommand>,
    event_rx: mpsc::Receiver<EditPreviewEvent>,
    /// rename migration が旧 key 宛の save/delete を追い越さないための FIFO 未完了数。
    pending_commands: Arc<AtomicUsize>,
}

impl EditPreviewCacheService {
    pub fn open() -> Result<Self, String> {
        let db = Arc::new(EditPreviewCacheDb::open().map_err(|e| e.to_string())?);
        let (tx, rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker_db = Arc::clone(&db);
        let pending_commands = Arc::new(AtomicUsize::new(0));
        let worker_pending_commands = Arc::clone(&pending_commands);
        std::thread::Builder::new()
            .name("edit-preview-cache".to_string())
            .spawn(move || {
                while let Ok(command) = rx.recv() {
                    match command {
                        EditPreviewCommand::Save {
                            item_key,
                            source_mtime,
                            source_size,
                            source_container_path,
                            coordinate_dims,
                            layout_dims,
                            pixels,
                            annotations,
                            crop,
                            max_bytes,
                            repaint_ctx,
                        } => {
                            let result = prepare_preview_components(
                                pixels,
                                annotations,
                                crop,
                                coordinate_dims,
                            )
                            .ok_or_else(|| "edit preview composition failed".to_string())
                            .and_then(
                                |(prepared, layers, geometry)| {
                                    encode_preview_components(&prepared, &layers)
                                        .map(|(base_webp, encoded_layers)| {
                                            (geometry, base_webp, encoded_layers)
                                        })
                                        .ok_or_else(|| {
                                            "edit preview WebP encode failed".to_string()
                                        })
                                },
                            );
                            let epub_read = source_container_path.as_deref().and_then(|path| {
                                path.extension()
                                    .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
                                    .then(|| crate::pdf_loader::resolve_read_target(path).ok())
                                    .flatten()
                            });
                            let Some((source_mtime, source_size, source_container_size)) =
                                preview_save_stamp(
                                    source_mtime,
                                    source_size,
                                    source_container_path.as_deref(),
                                    |_| epub_read.as_ref()?.stamp.generation_catalog_pair(),
                                )
                            else {
                                crate::logger::log(
                                    "edit_preview_cache: EPUB generation unavailable",
                                );
                                worker_pending_commands.fetch_sub(1, Ordering::AcqRel);
                                continue;
                            };
                            match result.and_then(|(geometry, base_webp, encoded_layers)| {
                                worker_db.save_encoded(
                                    &item_key,
                                    source_mtime,
                                    source_size,
                                    source_container_size,
                                    geometry,
                                    layout_dims,
                                    &base_webp,
                                    &encoded_layers,
                                )
                            }) {
                                Ok(()) => {
                                    worker_db.prune(max_bytes);
                                    let _ = event_tx.send(EditPreviewEvent::Saved { item_key });
                                    if let Some(ctx) = repaint_ctx {
                                        ctx.request_repaint();
                                    }
                                }
                                Err(err) => crate::logger::log(format!(
                                    "edit_preview_cache: save failed: {err}"
                                )),
                            }
                        }
                        EditPreviewCommand::Delete {
                            item_key,
                            notify_if_missing,
                        } => {
                            let removed = worker_db.delete(&item_key);
                            if publish_delete_invalidation(removed, notify_if_missing) {
                                let _ = event_tx.send(EditPreviewEvent::Invalidated { item_key });
                            }
                        }
                        EditPreviewCommand::Prune { max_bytes } => worker_db.prune(max_bytes),
                        EditPreviewCommand::Clear { completed } => {
                            let result = worker_db.clear();
                            if result.is_ok() {
                                let _ = event_tx.send(EditPreviewEvent::Cleared);
                            } else if let Err(error) = &result {
                                crate::logger::log(format!(
                                    "edit_preview_cache: clear failed: {error}"
                                ));
                            }
                            if let Some(completed) = completed {
                                let _ = completed.send(result);
                            }
                        }
                    }
                    worker_pending_commands.fetch_sub(1, Ordering::Release);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            db,
            tx,
            event_rx,
            pending_commands,
        })
    }

    pub fn db(&self) -> Arc<EditPreviewCacheDb> {
        Arc::clone(&self.db)
    }

    pub fn epoch(&self) -> u64 {
        self.db.epoch()
    }

    fn send_command(
        &self,
        command: EditPreviewCommand,
    ) -> Result<(), mpsc::SendError<EditPreviewCommand>> {
        self.pending_commands.fetch_add(1, Ordering::AcqRel);
        self.tx.send(command).map_err(|error| {
            self.pending_commands.fetch_sub(1, Ordering::AcqRel);
            error
        })
    }

    /// 既に enqueue 済みの preview save/delete/prune/clear が DB へ着地していないか。
    pub fn has_pending_commands(&self) -> bool {
        self.pending_commands.load(Ordering::Acquire) != 0
    }

    pub fn save(
        &self,
        item_key: String,
        source_mtime: i64,
        source_size: i64,
        source_container_path: Option<PathBuf>,
        coordinate_dims: CanonicalPageDimensions,
        layout_dims: Option<(u32, u32)>,
        pixels: Arc<egui::ColorImage>,
        annotations: Option<EditPreviewAnnotations>,
        crop: Option<crate::export_crop::CropRect>,
        max_bytes: u64,
        repaint_ctx: Option<egui::Context>,
    ) {
        let _ = self.send_command(EditPreviewCommand::Save {
            item_key,
            source_mtime,
            source_size,
            source_container_path,
            coordinate_dims,
            layout_dims,
            pixels,
            annotations,
            crop,
            max_bytes,
            repaint_ctx,
        });
    }

    pub fn delete(&self, item_key: String) {
        let _ = self.send_command(EditPreviewCommand::Delete {
            item_key,
            notify_if_missing: true,
        });
    }

    /// Removes a stale preview at a viewer-close boundary without invalidating an unchanged raw
    /// thumbnail when no preview row existed.
    pub fn delete_if_present(&self, item_key: String) {
        let _ = self.send_command(EditPreviewCommand::Delete {
            item_key,
            notify_if_missing: false,
        });
    }

    pub fn prune(&self, max_bytes: u64) {
        let _ = self.send_command(EditPreviewCommand::Prune { max_bytes });
    }

    pub fn clear(&self) {
        let _ = self.send_command(EditPreviewCommand::Clear { completed: None });
    }

    /// importのterminal refreshが旧previewを再利用しないよう、worker queue上で
    /// それ以前のsaveを着地させた後にclear完了を通知する。
    pub fn clear_with_completion(&self) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let (completed_tx, completed_rx) = mpsc::sync_channel(1);
        self.send_command(EditPreviewCommand::Clear {
            completed: Some(completed_tx),
        })
        .map_err(|_| "編集プレビューキャッシュの消去workerが終了しています".to_string())?;
        Ok(completed_rx)
    }

    pub fn try_recv(&self) -> Result<EditPreviewEvent, mpsc::TryRecvError> {
        self.event_rx.try_recv()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_geometry((width, height): (u32, u32)) -> EditPreviewGeometry {
        EditPreviewGeometry::new(
            CanonicalPageDimensions::new(width, height),
            (width, height),
            (0, 0, width, height),
        )
        .unwrap()
    }

    fn test_db() -> (tempfile::TempDir, EditPreviewCacheDb) {
        let temp = tempfile::tempdir().unwrap();
        let db = EditPreviewCacheDb::open_at(&temp.path().join("preview.db")).unwrap();
        (temp, db)
    }

    fn preview_row_paths(db: &EditPreviewCacheDb, item_key: &str) -> Option<Vec<PathBuf>> {
        let row: Option<(String, String)> = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT cached_path, annotation_layers_json
                   FROM edit_previews WHERE item_key = ?1",
                [item_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap();
        row.map(|(cached_path, layers_json)| {
            cache_paths(&cached_path, &layers_json)
                .into_iter()
                .map(PathBuf::from)
                .collect()
        })
    }

    fn save_preview_with_one_layer(db: &EditPreviewCacheDb, item_key: &str, seed: u8) {
        let base = egui::ColorImage::filled(
            [8, 4],
            egui::Color32::from_rgb(seed, seed.wrapping_add(40), seed.wrapping_add(80)),
        );
        let layer = CachedAnnotationLayer {
            blend: CachedAnnotationBlend::Multiply,
            image: egui::ColorImage::filled([8, 4], egui::Color32::from_rgb(230, 180, 120)),
        };
        let (base_webp, layers) = encode_preview_components(&base, &[layer]).unwrap();
        db.save_encoded(
            item_key,
            10,
            20,
            None,
            full_geometry((8, 4)),
            None,
            &base_webp,
            &layers,
        )
        .unwrap();
    }

    /// Exercise the released writer's actual base-only artifact naming and UPSERT, rather than
    /// simulating an overwrite by clearing new columns. SQL copied from 629a7d85b^.
    fn released_writer_save_base(
        db: &EditPreviewCacheDb,
        item_key: &str,
        source_dims: (u32, u32),
        base_webp: &[u8],
    ) -> PathBuf {
        let content_hash = format!("{:x}", Sha256::digest(base_webp));
        let dir = cache_dir_for_item(&db.root, item_key);
        std::fs::create_dir_all(&dir).unwrap();
        let final_path = dir.join(format!("{content_hash}.base.webp"));
        write_cache_file(&final_path, base_webp).unwrap();
        let old_paths = preview_row_paths(db, item_key).unwrap_or_default();
        let final_path_string = final_path.to_string_lossy().to_string();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO edit_previews
                 (item_key, source_mtime, source_size, source_container_size,
                  source_width, source_height,
                  cached_path, annotation_layers_json, cached_bytes, updated_at, last_access_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
                 ON CONFLICT(item_key) DO UPDATE SET
                    source_mtime=excluded.source_mtime,
                    source_size=excluded.source_size,
                    source_container_size=excluded.source_container_size,
                    source_width=excluded.source_width,
                    source_height=excluded.source_height,
                    cached_path=excluded.cached_path,
                    annotation_layers_json=excluded.annotation_layers_json,
                    cached_bytes=excluded.cached_bytes,
                    updated_at=excluded.updated_at,
                    last_access_at=excluded.last_access_at",
                params![
                    item_key,
                    10,
                    20,
                    Some(30),
                    source_dims.0 as i64,
                    source_dims.1 as i64,
                    &final_path_string,
                    "[]",
                    base_webp.len() as i64,
                    now_secs()
                ],
            )
            .unwrap();
        for path in old_paths {
            if path != final_path {
                remove_file_and_empty_parents(&db.root, &path);
            }
        }
        final_path
    }

    fn assert_released_writer_replacement_discards_unbound_metadata(legacy_dims: (u32, u32)) {
        let (temp, db) = test_db();
        let geometry = EditPreviewGeometry::new(
            CanonicalPageDimensions::new(1200, 800),
            (12, 8),
            (2, 2, 8, 4),
        )
        .unwrap();
        let original_webp =
            encode_preview(&egui::ColorImage::filled([8, 4], egui::Color32::RED)).unwrap();
        db.save_encoded(
            "page",
            10,
            20,
            Some(30),
            geometry,
            Some((612_000, 792_000)),
            &original_webp,
            &[],
        )
        .unwrap();
        assert_eq!(
            db.load("page", 10, 20, 2048).unwrap().coverage,
            SourceCoverage::Known(geometry)
        );

        let old_writer_webp = encode_preview(&egui::ColorImage::filled(
            [legacy_dims.0 as usize, legacy_dims.1 as usize],
            egui::Color32::BLUE,
        ))
        .unwrap();
        let legacy_path = released_writer_save_base(&db, "page", legacy_dims, &old_writer_webp);
        let conn = db.conn.lock().unwrap();
        let (remaining_geometry, remaining_layout, remaining_binding): (String, String, String) =
            conn.query_row(
                "SELECT source_coverage_json, layout_dims_json, metadata_content_hash
             FROM edit_previews WHERE item_key = 'page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<EditPreviewGeometry>(&remaining_geometry).unwrap(),
            geometry
        );
        assert_eq!(
            serde_json::from_str::<(u32, u32)>(&remaining_layout).unwrap(),
            (612_000, 792_000)
        );
        assert_ne!(
            legacy_path.file_name().unwrap().to_str().unwrap(),
            format!("{remaining_binding}.base.webp")
        );
        drop(conn);
        drop(db);

        let db = EditPreviewCacheDb::open_at(&temp.path().join("preview.db")).unwrap();
        let loaded = db.load("page", 10, 20, 2048).unwrap();
        assert_eq!(loaded.source_dims, legacy_dims);
        assert_eq!(loaded.coverage, SourceCoverage::Unknown);
        assert_eq!(loaded.layout_dims, None);
        assert_eq!(
            loaded.image,
            crate::catalog::decode_thumb_to_color_image(&old_writer_webp).unwrap()
        );
        assert_eq!(
            db.load_for_container("page", 10, 30, 2048)
                .unwrap()
                .coverage,
            SourceCoverage::Unknown
        );
        assert_eq!(
            preview_row_paths(&db, "page").unwrap(),
            vec![legacy_path.clone()]
        );
        assert_eq!(std::fs::read(legacy_path).unwrap(), old_writer_webp);

        // A subsequent new writer atomically binds fresh provenance again.
        db.save_encoded(
            "page",
            10,
            20,
            Some(30),
            geometry,
            Some((612_000, 792_000)),
            &original_webp,
            &[],
        )
        .unwrap();
        let refreshed = db.load("page", 10, 20, 2048).unwrap();
        assert_eq!(refreshed.coverage, SourceCoverage::Known(geometry));
        assert_eq!(refreshed.layout_dims, Some((612_000, 792_000)));
    }

    #[test]
    fn released_writer_same_size_overwrite_keeps_preview_but_not_newer_metadata() {
        assert_released_writer_replacement_discards_unbound_metadata((8, 4));
    }

    #[test]
    fn released_writer_different_size_overwrite_keeps_preview_but_not_newer_metadata() {
        assert_released_writer_replacement_discards_unbound_metadata((4, 6));
    }

    #[test]
    fn additive_artifact_binding_migration_retains_unbound_rows_and_webps() {
        let (temp, db) = test_db();
        save_preview_with_one_layer(&db, "unbound", 42);
        let paths = preview_row_paths(&db, "unbound").unwrap();
        let original_bytes = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        db.conn.lock().unwrap().execute_batch(
            "UPDATE edit_previews SET layout_dims_json = '[612000,792000]' WHERE item_key = 'unbound';
             ALTER TABLE edit_previews DROP COLUMN metadata_content_hash;",
        ).unwrap();
        drop(db);
        let db = EditPreviewCacheDb::open_at(&temp.path().join("preview.db")).unwrap();
        let conn = db.conn.lock().unwrap();
        let (version, binding): (i64, Option<String>) = (
            conn.query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap(),
            conn.query_row(
                "SELECT metadata_content_hash FROM edit_previews WHERE item_key = 'unbound'",
                [],
                |row| row.get(0),
            )
            .unwrap(),
        );
        assert_eq!(version, 6);
        assert_eq!(binding, None);
        drop(conn);
        let loaded = db.load("unbound", 10, 20, 2048).unwrap();
        assert_eq!(loaded.source_dims, (8, 4));
        assert_eq!(loaded.annotation_layers.len(), 1);
        assert_eq!(loaded.coverage, SourceCoverage::Unknown);
        assert_eq!(loaded.layout_dims, None);
        assert_eq!(preview_row_paths(&db, "unbound").unwrap(), paths);
        for (path, bytes) in paths.iter().zip(original_bytes) {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }

    fn red_transparent_edge() -> egui::ColorImage {
        egui::ColorImage::new([2, 1], vec![egui::Color32::RED, egui::Color32::TRANSPARENT])
    }

    fn assert_half_transparent_red(pixel: egui::Color32) {
        let [r, g, b, a] = pixel.to_srgba_unmultiplied();
        assert!(
            (120..=136).contains(&a),
            "opaque + transparent average should retain half alpha: {a}"
        );
        assert!(
            r >= 250,
            "transparent black must not darken the straight red channel: {r}"
        );
        assert_eq!([g, b], [0, 0]);
    }

    #[test]
    fn display_resize_filters_transparent_edges_as_premultiplied_alpha() {
        let resized = resize_color_image_to_dims(red_transparent_edge(), (1, 1)).unwrap();
        assert_eq!(resized.size, [1, 1]);
        assert_half_transparent_red(resized.pixels[0]);
    }

    #[test]
    fn content_restore_copies_owned_base_and_layer_and_invalidation_is_isolated() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("edit_preview_cache.db");
        let db = EditPreviewCacheDb::open_at(&db_path).unwrap();
        let pairs = [
            (
                temp.path().join("source-a.png"),
                temp.path().join("destination-a.png"),
            ),
            (
                temp.path().join("source-b.png"),
                temp.path().join("destination-b.png"),
            ),
        ];
        for (index, (source, _)) in pairs.iter().enumerate() {
            save_preview_with_one_layer(
                &db,
                &crate::path_key::normalize_keep_drive(source),
                40 + index as u8,
            );
        }
        drop(db);

        let mappings = pairs
            .iter()
            .map(|(source, destination)| {
                crate::rename_key_migration::StoreCopyPathMapping::exact(source, destination)
            })
            .collect::<Vec<_>>();
        let report = crate::rename_key_migration::copy_stores_at(temp.path(), &mappings);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.rows, 2);

        let db = EditPreviewCacheDb::open_at(&db_path).unwrap();
        let mut all_paths = Vec::new();
        for (source, destination) in &pairs {
            let source_key = crate::path_key::normalize_keep_drive(source);
            let destination_key = crate::path_key::normalize_keep_drive(destination);
            let source_paths = preview_row_paths(&db, &source_key).unwrap();
            let destination_paths = preview_row_paths(&db, &destination_key).unwrap();
            assert_eq!(source_paths.len(), 2, "base + one annotation layer");
            assert_eq!(destination_paths.len(), 2, "base + one annotation layer");
            let expected_dir = cache_dir_for_item(&db.root, &destination_key);
            assert!(
                destination_paths
                    .iter()
                    .all(|path| path.parent() == Some(expected_dir.as_path()))
            );
            for (source_path, destination_path) in source_paths.iter().zip(&destination_paths) {
                assert_ne!(source_path, destination_path);
                assert_eq!(
                    source_path.file_name(),
                    destination_path.file_name(),
                    "content-hash filenames stay unchanged"
                );
                assert_eq!(
                    std::fs::read(source_path).unwrap(),
                    std::fs::read(destination_path).unwrap()
                );
            }
            let loaded = db.load(&destination_key, 10, 20, 2048).unwrap();
            assert_eq!(loaded.annotation_layers.len(), 1);
            assert_eq!(
                loaded.coverage,
                SourceCoverage::Known(full_geometry((8, 4)))
            );
            all_paths.push((source_key, destination_key, source_paths, destination_paths));
        }

        let (source_key, _, _, destination_paths) = &all_paths[0];
        assert!(db.delete(source_key));
        assert!(destination_paths.iter().all(|path| path.is_file()));

        let (_, destination_key, source_paths, _) = &all_paths[1];
        assert!(db.delete(destination_key));
        assert!(source_paths.iter().all(|path| path.is_file()));
    }

    #[test]
    fn content_restore_skips_source_row_with_missing_cache_file() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("edit_preview_cache.db");
        let source = temp.path().join("source-missing.png");
        let destination = temp.path().join("destination-missing.png");
        let source_key = crate::path_key::normalize_keep_drive(&source);
        let destination_key = crate::path_key::normalize_keep_drive(&destination);
        let db = EditPreviewCacheDb::open_at(&db_path).unwrap();
        save_preview_with_one_layer(&db, &source_key, 90);
        let source_paths = preview_row_paths(&db, &source_key).unwrap();
        std::fs::remove_file(&source_paths[1]).unwrap();
        drop(db);

        let report = crate::rename_key_migration::copy_stores_at(
            temp.path(),
            &[crate::rename_key_migration::StoreCopyPathMapping::exact(
                &source,
                &destination,
            )],
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(
            report.rows, 0,
            "skipped cache rows do not count as restored"
        );

        let db = EditPreviewCacheDb::open_at(&db_path).unwrap();
        assert!(preview_row_paths(&db, &source_key).is_some());
        assert!(preview_row_paths(&db, &destination_key).is_none());
        assert!(!cache_dir_for_item(&db.root, &destination_key).exists());
    }

    #[test]
    fn lossless_preview_encode_unmultiplies_only_after_resize() {
        let webp = encode_preview_exact(&red_transparent_edge(), (1, 1), true).unwrap();
        let decoded = crate::catalog::decode_thumb_to_color_image(&webp).unwrap();
        assert_eq!(decoded.size, [1, 1]);
        assert_half_transparent_red(decoded.pixels[0]);
    }

    #[test]
    fn preview_roundtrip_and_source_validation() {
        let (_temp, db) = test_db();
        let webp = encode_preview(&egui::ColorImage::filled(
            [8, 4],
            egui::Color32::from_rgb(20, 80, 140),
        ))
        .unwrap();
        db.save_encoded(
            "page-a",
            10,
            20,
            None,
            full_geometry((8, 4)),
            None,
            &webp,
            &[],
        )
        .unwrap();

        let hit = db.load("page-a", 10, 20, 2048).unwrap();
        assert_eq!(hit.source_dims, (8, 4));
        assert_eq!(hit.image.size, [8, 4]);
        assert!(db.load("page-a", 11, 20, 2048).is_none());
        assert_eq!(db.total_bytes(), 0);
    }

    #[test]
    fn delete_reports_whether_a_persistent_preview_was_removed() {
        let (_temp, db) = test_db();
        assert!(!db.delete("missing"));

        let webp = encode_preview(&egui::ColorImage::filled(
            [8, 4],
            egui::Color32::from_rgb(20, 80, 140),
        ))
        .unwrap();
        db.save_encoded(
            "page-delete",
            10,
            20,
            None,
            full_geometry((8, 4)),
            None,
            &webp,
            &[],
        )
        .unwrap();

        assert!(db.delete("page-delete"));
        assert!(!db.delete("page-delete"));
        assert_eq!(db.total_bytes(), 0);
    }

    #[test]
    fn clear_reports_a_poisoned_db_lock_instead_of_publishing_success() {
        let (_temp, db) = test_db();
        let before = db.epoch();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = db.conn.lock().unwrap();
            panic!("poison preview DB mutex");
        }));

        let error = db.clear().unwrap_err();
        assert!(error.contains("poisoned"), "{error}");
        assert_eq!(db.epoch(), before, "a failed DELETE must not advance epoch");
    }

    #[test]
    fn successful_clear_advances_epoch_after_removing_rows() {
        let (_temp, db) = test_db();
        save_preview_with_one_layer(&db, "page-before-clear", 42);
        let read_epoch = db.epoch();
        assert!(db.load("page-before-clear", 10, 20, 2048).is_some());

        db.clear().unwrap();

        assert_eq!(db.epoch(), read_epoch.wrapping_add(1));
        assert!(db.load("page-before-clear", 10, 20, 2048).is_none());
    }

    #[test]
    fn read_only_close_does_not_publish_missing_preview_invalidation() {
        assert!(!publish_delete_invalidation(false, false));
        assert!(publish_delete_invalidation(true, false));
        assert!(publish_delete_invalidation(false, true));
    }

    #[test]
    fn virtual_page_preview_can_validate_against_container_identity() {
        let (_temp, db) = test_db();
        let webp = encode_preview(&egui::ColorImage::filled(
            [8, 4],
            egui::Color32::from_rgb(20, 80, 140),
        ))
        .unwrap();
        db.save_encoded(
            "archive::page.jpg",
            10,
            20,
            Some(200),
            full_geometry((8, 4)),
            None,
            &webp,
            &[],
        )
        .unwrap();

        assert!(
            db.load_for_container("archive::page.jpg", 10, 200, 2048)
                .is_some(),
            "parent thumbnails validate the archive, not the entry's uncompressed size"
        );
        assert!(
            db.load_for_container("archive::page.jpg", 10, 201, 2048)
                .is_none(),
            "a replaced archive must invalidate its edited representative preview"
        );
        assert_eq!(db.total_bytes(), 0);
    }

    #[test]
    fn epub_preview_save_and_load_use_generation_in_existing_integer_columns() {
        let (_temp, db) = test_db();
        let webp = encode_preview(&egui::ColorImage::filled(
            [8, 4],
            egui::Color32::from_rgb(20, 80, 140),
        ))
        .unwrap();
        let path = Path::new("book.epub");
        let stamp = preview_save_stamp(123, 456, Some(path), |_| Some((17, 4096))).unwrap();
        assert_eq!(stamp, (17, 4096, Some(4096)));
        db.save_encoded(
            "book.epub::page_0",
            stamp.0,
            stamp.1,
            stamp.2,
            full_geometry((8, 4)),
            None,
            &webp,
            &[],
        )
        .unwrap();
        assert!(
            db.load_for_container("book.epub::page_0", 17, 4096, 2048)
                .is_some()
        );
        assert!(
            db.load_for_container("book.epub::page_0", 18, 4096, 2048)
                .is_none()
        );
        let pdf = preview_save_stamp(123, 456, None, |_| panic!("PDF must not resolve EPUB"));
        assert_eq!(pdf, Some((123, 456, None)));
    }

    #[test]
    fn preview_roundtrip_preserves_separate_annotation_layer() {
        let (_temp, db) = test_db();
        let base = egui::ColorImage::filled([8, 4], egui::Color32::from_rgb(20, 80, 140));
        let annotation = CachedAnnotationLayer {
            blend: CachedAnnotationBlend::Normal,
            image: egui::ColorImage::filled(
                [8, 4],
                egui::Color32::from_rgba_unmultiplied(240, 120, 160, 128),
            ),
        };
        let (base_webp, layers) = encode_preview_components(&base, &[annotation]).unwrap();
        db.save_encoded(
            "page-layers",
            10,
            20,
            None,
            full_geometry((8, 4)),
            None,
            &base_webp,
            &layers,
        )
        .unwrap();

        let hit = db.load("page-layers", 10, 20, 2048).unwrap();
        assert_eq!(hit.adjustment_base.size, [8, 4]);
        assert_eq!(hit.annotation_layers.len(), 1);
        assert_eq!(
            hit.annotation_layers[0].blend,
            CachedAnnotationBlend::Normal
        );
        assert_ne!(hit.image.pixels[0], hit.adjustment_base.pixels[0]);
    }

    #[test]
    fn preview_load_downscales_base_and_layers_to_display_size() {
        let (_temp, db) = test_db();
        let base = egui::ColorImage::filled([80, 40], egui::Color32::from_rgb(20, 80, 140));
        let annotation = CachedAnnotationLayer {
            blend: CachedAnnotationBlend::Normal,
            image: egui::ColorImage::filled(
                [80, 40],
                egui::Color32::from_rgba_unmultiplied(240, 120, 160, 128),
            ),
        };
        let (base_webp, layers) = encode_preview_components(&base, &[annotation]).unwrap();
        db.save_encoded(
            "page-display",
            10,
            20,
            None,
            full_geometry((80, 40)),
            None,
            &base_webp,
            &layers,
        )
        .unwrap();

        let hit = db.load("page-display", 10, 20, 24).unwrap();
        assert_eq!(hit.source_dims, (80, 40));
        assert_eq!(hit.adjustment_base.size, [24, 12]);
        assert_eq!(hit.annotation_layers[0].image.size, [24, 12]);
        assert_eq!(hit.image.size, [24, 12]);
        assert_ne!(hit.image.pixels[0], hit.adjustment_base.pixels[0]);
    }

    #[test]
    fn prune_removes_oldest_entries_to_limit() {
        let (_temp, db) = test_db();
        let webp_a =
            encode_preview(&egui::ColorImage::filled([16, 16], egui::Color32::RED)).unwrap();
        let webp_b =
            encode_preview(&egui::ColorImage::filled([16, 16], egui::Color32::BLUE)).unwrap();
        db.save_encoded("a", 1, 1, None, full_geometry((16, 16)), None, &webp_a, &[])
            .unwrap();
        db.save_encoded("b", 1, 1, None, full_geometry((16, 16)), None, &webp_b, &[])
            .unwrap();
        db.prune(webp_b.len() as u64);
        assert!(db.total_bytes() <= webp_b.len() as u64);
    }

    #[test]
    fn crop_is_applied_before_resize_and_becomes_preview_source_dims() {
        let pixels = Arc::new(egui::ColorImage::filled(
            [12, 8],
            egui::Color32::from_rgb(20, 80, 140),
        ));
        let crop = crate::export_crop::CropRect {
            min_x: 2.0,
            min_y: 1.0,
            max_x: 10.0,
            max_y: 7.0,
        };
        let (prepared, layers, geometry) = prepare_preview_components(
            pixels,
            None,
            Some(crop),
            CanonicalPageDimensions::new(12, 8),
        )
        .unwrap();
        assert_eq!(prepared.size, [8, 6]);
        assert_eq!(prepared.pixels.len(), 48);
        assert!(layers.is_empty());
        assert_eq!(geometry.raster_dims, (12, 8));
        assert_eq!(geometry.pixel_bounds, (2, 1, 8, 6));
        assert!(encode_preview(&prepared).is_some());
    }

    #[test]
    fn fractional_crop_uses_identical_real_pixel_bounds_for_base_normal_and_multiply() {
        let pixels = Arc::new(egui::ColorImage::new(
            [12, 8],
            (0..96)
                .map(|index| egui::Color32::from_rgb(index, 120, 200))
                .collect(),
        ));
        let layers = [
            CachedAnnotationBlend::Normal,
            CachedAnnotationBlend::Multiply,
        ]
        .into_iter()
        .map(|blend| CachedAnnotationLayer {
            blend,
            image: (*pixels).clone(),
        })
        .collect();
        let crop = crate::export_crop::CropRect {
            min_x: 2.9,
            min_y: 1.2,
            max_x: 9.1,
            max_y: 6.01,
        };
        let (base, layers, geometry) = crop_preview_components(
            Arc::clone(&pixels),
            layers,
            Some(crop),
            CanonicalPageDimensions::new(1200, 800),
        )
        .unwrap();
        assert_eq!(geometry.pixel_bounds, (2, 1, 8, 6));
        assert_eq!(geometry.raster_dims, (12, 8));
        assert_eq!(geometry.coordinate_dims.as_tuple(), (1200, 800));
        assert_eq!(base.size, [8, 6]);
        assert_eq!(base.pixels[0], pixels.pixels[14]);
        assert_eq!(*base.pixels.last().unwrap(), pixels.pixels[81]);
        for layer in &layers {
            assert_eq!(layer.image.size, base.size);
            assert_eq!(layer.image.pixels, base.pixels);
        }

        let (temp, db) = test_db();
        let path = temp.path().join("preview.db");
        let (webp, encoded_layers) = encode_preview_components(&base, &layers).unwrap();
        db.save_encoded(
            "fractional",
            10,
            20,
            None,
            geometry,
            None,
            &webp,
            &encoded_layers,
        )
        .unwrap();
        drop(db);
        let db = EditPreviewCacheDb::open_at(&path).unwrap();
        let loaded = db.load("fractional", 10, 20, 4).unwrap();
        assert_eq!(loaded.source_dims, (8, 6), "listing aspect remains cropped");
        assert_eq!(loaded.adjustment_base.size, [4, 3]);
        assert_eq!(loaded.coverage, SourceCoverage::Known(geometry));
        assert_eq!(
            loaded.annotation_layers[0].blend,
            CachedAnnotationBlend::Normal
        );
        assert_eq!(
            loaded.annotation_layers[1].blend,
            CachedAnnotationBlend::Multiply
        );
        assert!(
            loaded
                .annotation_layers
                .iter()
                .all(|layer| layer.image.size == [4, 3])
        );
        assert_eq!(
            loaded.coverage.source_uv_rect(),
            Some(egui::Rect::from_min_max(
                egui::pos2(2.0 / 12.0, 1.0 / 8.0),
                egui::pos2(10.0 / 12.0, 7.0 / 8.0),
            ))
        );
    }

    #[test]
    fn uncropped_new_preview_still_persists_known_coordinate_and_raster_dimensions() {
        let (_temp, db) = test_db();
        let pixels = Arc::new(egui::ColorImage::filled([12, 8], egui::Color32::WHITE));
        let (base, layers, geometry) = prepare_preview_components(
            pixels,
            None,
            None,
            CanonicalPageDimensions::new(6000, 4000),
        )
        .unwrap();
        let (webp, encoded_layers) = encode_preview_components(&base, &layers).unwrap();
        db.save_encoded(
            "uncropped",
            10,
            20,
            None,
            geometry,
            None,
            &webp,
            &encoded_layers,
        )
        .unwrap();
        let loaded = db.load("uncropped", 10, 20, 4).unwrap();
        assert_eq!(geometry.pixel_bounds, (0, 0, 12, 8));
        assert_eq!(
            loaded.coverage.coordinate_dims().unwrap().as_tuple(),
            (6000, 4000)
        );
        assert!(matches!(loaded.coverage, SourceCoverage::Known(_)));
        assert_eq!(
            loaded.coverage.source_uv_rect(),
            SourceCoverage::FullSource.source_uv_rect()
        );
        assert_eq!(loaded.image.size, [4, 3]);
        assert_eq!(loaded.layout_dims, None);
    }

    #[test]
    fn preview_roundtrip_preserves_pagebox_layout_independently_of_coordinate_and_raster_sizes() {
        let (temp, db) = test_db();
        let path = temp.path().join("preview.db");
        let geometry = EditPreviewGeometry::new(
            CanonicalPageDimensions::new(6000, 4000),
            (12, 8),
            (2, 1, 4, 4),
        )
        .unwrap();
        let webp = encode_preview(&egui::ColorImage::filled([4, 4], egui::Color32::WHITE)).unwrap();
        let pagebox = (612_000, 792_000);
        db.save_encoded(
            "pdf::page_0",
            10,
            20,
            Some(30),
            geometry,
            Some(pagebox),
            &webp,
            &[],
        )
        .unwrap();
        drop(db);

        let db = EditPreviewCacheDb::open_at(&path).unwrap();
        let loaded = db.load("pdf::page_0", 10, 20, 2).unwrap();
        assert_eq!(loaded.layout_dims, Some(pagebox));
        assert_eq!(loaded.source_dims, (4, 4));
        assert_eq!(loaded.image.size, [2, 2]);
        assert_eq!(loaded.coverage, SourceCoverage::Known(geometry));
        assert_eq!(
            loaded.coverage.coordinate_dims().unwrap().as_tuple(),
            (6000, 4000)
        );
        assert_eq!(
            db.load_for_container("pdf::page_0", 10, 30, 4)
                .unwrap()
                .layout_dims,
            Some(pagebox)
        );

        db.save_encoded("pdf::page_0", 10, 20, Some(30), geometry, None, &webp, &[])
            .unwrap();
        assert_eq!(
            db.load("pdf::page_0", 10, 20, 4).unwrap().layout_dims,
            None,
            "a subsequent unknown layout must not inherit an older row's page-box units"
        );
    }

    #[test]
    fn released_v6_migration_retains_rows_webps_and_unknown_listing_preview() {
        let (temp, db) = test_db();
        save_preview_with_one_layer(&db, "legacy", 42);
        let paths = preview_row_paths(&db, "legacy").unwrap();
        let bytes = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        // This recreates the released v6 schema without changing any existing row or WebP.
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "ALTER TABLE edit_previews DROP COLUMN source_coverage_json;
                            ALTER TABLE edit_previews DROP COLUMN layout_dims_json;
                            ALTER TABLE edit_previews DROP COLUMN metadata_content_hash;",
            )
            .unwrap();
        drop(db);

        let path = temp.path().join("preview.db");
        let db = EditPreviewCacheDb::open_at(&path).unwrap();
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            6
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM edit_previews", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT source_coverage_json FROM edit_previews WHERE item_key = 'legacy'",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .unwrap(),
            None
        );
        drop(conn);
        let loaded = db.load("legacy", 10, 20, 2048).unwrap();
        assert_eq!(loaded.source_dims, (8, 4));
        assert_eq!(loaded.annotation_layers.len(), 1);
        assert_eq!(loaded.coverage, SourceCoverage::Unknown);
        assert_eq!(loaded.layout_dims, None);
        assert_eq!(
            loaded.coverage.resolve_for_display(false),
            Ok(SourceCoverage::FullSource)
        );
        assert_eq!(
            loaded.coverage.resolve_for_display(true),
            Err(crate::source_coverage::SourceCoverageUnavailable::UnknownWithCurrentCrop)
        );
        for (path, expected) in paths.iter().zip(bytes) {
            assert_eq!(std::fs::read(path).unwrap(), expected);
        }
        drop(db);
        let reopened = EditPreviewCacheDb::open_at(&path).unwrap();
        assert_eq!(
            reopened.load("legacy", 10, 20, 2048).unwrap().coverage,
            SourceCoverage::Unknown
        );
    }

    #[test]
    fn current_crop_update_before_preview_delete_cannot_change_generated_coverage() {
        let (temp, db) = test_db();
        let crop_db = crate::export_crop::CropDb::open_at(&temp.path().join("crop.db")).unwrap();
        let crop_a = crate::export_crop::CropRect {
            min_x: 1.2,
            min_y: 1.2,
            max_x: 7.2,
            max_y: 6.2,
        };
        let crop_b = crate::export_crop::CropRect {
            min_x: 4.2,
            min_y: 0.2,
            max_x: 10.2,
            max_y: 5.2,
        };
        let settings = |rect| {
            crate::export_crop::CropSettings::authored(
                rect,
                crate::export_crop::CropAspectMode::Free,
                [12, 8],
            )
        };
        crop_db.set("page", settings(crop_a)).unwrap();
        let pixels = Arc::new(egui::ColorImage::filled([12, 8], egui::Color32::WHITE));
        let (base, layers, geometry) = prepare_preview_components(
            pixels,
            None,
            Some(crop_a),
            CanonicalPageDimensions::new(120, 80),
        )
        .unwrap();
        let (webp, encoded_layers) = encode_preview_components(&base, &layers).unwrap();
        db.save_encoded("page", 10, 20, None, geometry, None, &webp, &encoded_layers)
            .unwrap();

        // Deterministic ordinary ordering: crop DB updated; queued preview Delete has not run yet.
        crop_db.set("page", settings(crop_b)).unwrap();
        let loaded = db.load("page", 10, 20, 2048).unwrap();
        assert_eq!(crop_db.get("page").unwrap().rect, crop_b);
        assert_eq!(loaded.coverage, SourceCoverage::Known(geometry));
        assert_eq!(geometry.pixel_bounds, (1, 1, 7, 6));
        assert_ne!(geometry.pixel_bounds, (4, 0, 7, 6));
        assert_eq!(
            loaded.coverage.resolve_for_display(true),
            Ok(loaded.coverage)
        );
        crop_db.remove("page").unwrap();
        assert_eq!(
            db.load("page", 10, 20, 2048).unwrap().coverage,
            loaded.coverage
        );
    }

    #[test]
    fn generation_uses_supplied_memory_crop_even_when_persisted_crop_differs() {
        let (temp, db) = test_db();
        let crop_db = crate::export_crop::CropDb::open_at(&temp.path().join("crop.db")).unwrap();
        let saved_crop = crate::export_crop::CropRect {
            min_x: 1.2,
            min_y: 1.2,
            max_x: 7.2,
            max_y: 6.2,
        };
        let memory_crop = crate::export_crop::CropRect {
            min_x: 4.2,
            min_y: 0.2,
            max_x: 10.2,
            max_y: 5.2,
        };
        crop_db
            .set(
                "page",
                crate::export_crop::CropSettings::authored(
                    saved_crop,
                    crate::export_crop::CropAspectMode::Free,
                    [12, 8],
                ),
            )
            .unwrap();
        // Producer input models an in-memory edit that has not reached the crop DB. It must carry
        // its own generation provenance without inferring it from the independent settings row.
        let pixels = Arc::new(egui::ColorImage::filled([12, 8], egui::Color32::WHITE));
        let (base, layers, geometry) = prepare_preview_components(
            pixels,
            None,
            Some(memory_crop),
            CanonicalPageDimensions::new(120, 80),
        )
        .unwrap();
        let (webp, encoded_layers) = encode_preview_components(&base, &layers).unwrap();
        db.save_encoded("page", 10, 20, None, geometry, None, &webp, &encoded_layers)
            .unwrap();
        assert_eq!(crop_db.get("page").unwrap().rect, saved_crop);
        assert_eq!(geometry.pixel_bounds, (4, 0, 7, 6));
        assert_eq!(
            db.load("page", 10, 20, 2048).unwrap().coverage,
            SourceCoverage::Known(geometry)
        );
    }

    #[test]
    fn identical_pixels_at_different_positions_publish_distinct_owned_materials() {
        let (_temp, db) = test_db();
        let webp = encode_preview(&egui::ColorImage::filled([4, 4], egui::Color32::WHITE)).unwrap();
        let a =
            EditPreviewGeometry::new(CanonicalPageDimensions::new(12, 8), (12, 8), (1, 1, 4, 4))
                .unwrap();
        let b =
            EditPreviewGeometry::new(CanonicalPageDimensions::new(12, 8), (12, 8), (6, 2, 4, 4))
                .unwrap();
        db.save_encoded("page", 10, 20, None, a, None, &webp, &[])
            .unwrap();
        let path_a = preview_row_paths(&db, "page").unwrap();
        db.save_encoded("page", 10, 20, None, b, None, &webp, &[])
            .unwrap();
        let path_b = preview_row_paths(&db, "page").unwrap();
        assert_ne!(path_a, path_b);
        assert!(!path_a[0].exists());
        assert!(path_b[0].exists());
        assert_eq!(
            db.load("page", 10, 20, 2048).unwrap().coverage,
            SourceCoverage::Known(b)
        );
    }

    #[test]
    fn display_color_is_applied_before_cached_annotations() {
        let base = egui::ColorImage::filled([1, 1], egui::Color32::from_rgb(128, 128, 128));
        let layer = CachedAnnotationLayer {
            blend: CachedAnnotationBlend::Normal,
            image: egui::ColorImage::filled([1, 1], egui::Color32::from_rgb(240, 120, 160)),
        };
        let params = crate::adjustment::AdjustParams {
            gamma: 0.2,
            ..Default::default()
        };
        let adjusted_base = crate::adjustment::apply_adjustments_fast(&base, &params);
        let output = composite_cached_annotation_layers(&adjusted_base, &[layer]);

        assert_ne!(adjusted_base.pixels[0], base.pixels[0]);
        assert_eq!(
            output.pixels[0],
            egui::Color32::from_rgb(240, 120, 160),
            "注釈は gamma の後段なので色を変えない"
        );
    }

    #[test]
    fn tampered_cached_path_is_rejected_without_touching_external_file() {
        let (temp, db) = test_db();
        let external = temp.path().join("outside.webp");
        std::fs::write(&external, b"do not delete").unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO edit_previews
                 (item_key, source_mtime, source_size, source_width, source_height,
                  cached_path, annotation_layers_json, cached_bytes, updated_at, last_access_at)
                 VALUES (?1, 1, 1, 8, 8, ?2, '[]', 13, 1, 1)",
                params!["tampered", external.to_string_lossy().as_ref()],
            )
            .unwrap();
        }

        assert!(db.load("tampered", 1, 1, 2048).is_none());
        assert!(external.exists());
        assert_eq!(db.total_bytes(), 0);
    }

    #[test]
    fn malformed_new_metadata_is_rejected_without_becoming_legacy_or_removing_owned_data() {
        let (_temp, db) = test_db();
        save_preview_with_one_layer(&db, "damaged-metadata", 42);
        let paths = preview_row_paths(&db, "damaged-metadata").unwrap();
        let original_bytes = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        let geometry = full_geometry((8, 4));
        let valid_geometry_json = serde_json::to_string(&geometry).unwrap();
        let invalid_geometry_json = serde_json::to_string(&EditPreviewGeometry {
            raster_dims: (0, 4),
            ..geometry
        })
        .unwrap();
        let mismatched_geometry_json = serde_json::to_string(&full_geometry((4, 4))).unwrap();
        let cases: [(&str, Option<&str>); 7] = [
            ("{", None),
            ("null", None),
            (&invalid_geometry_json, None),
            (&mismatched_geometry_json, None),
            (&valid_geometry_json, Some("{")),
            (&valid_geometry_json, Some("null")),
            (&valid_geometry_json, Some("[0,4]")),
        ];
        for (coverage_json, layout_json) in cases {
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE edit_previews SET source_coverage_json = ?1, layout_dims_json = ?2
                 WHERE item_key = 'damaged-metadata'",
                    params![coverage_json, layout_json],
                )
                .unwrap();
            assert!(db.load("damaged-metadata", 10, 20, 2048).is_none());
            assert_eq!(preview_row_paths(&db, "damaged-metadata").unwrap(), paths);
            for (path, expected) in paths.iter().zip(&original_bytes) {
                assert_eq!(&std::fs::read(path).unwrap(), expected);
            }
        }
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE edit_previews SET source_coverage_json = NULL, layout_dims_json = NULL
             WHERE item_key = 'damaged-metadata'",
                [],
            )
            .unwrap();
        let legacy = db.load("damaged-metadata", 10, 20, 2048).unwrap();
        assert_eq!(legacy.coverage, SourceCoverage::Unknown);
        assert_eq!(legacy.layout_dims, None);
    }

    #[test]
    fn tampered_parent_component_is_not_owned_by_cache_root() {
        let (_temp, db) = test_db();
        let escaped = db.root.join("child").join("..").join("outside.webp");
        assert!(!cache_path_is_owned(&db.root, &escaped));
    }
}
