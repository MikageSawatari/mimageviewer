//! 画像 1 ページ分の非破壊編集を snapshot / 全置換するための共通モデル。
//!
//! 対象は `SidecarEntry` と同じ 6 系統 (個別補正、消しゴム、隠蔽、補正レイヤー、
//! crop、テキスト注釈)。貼り付けは複数 SQLite ファイルを ATTACH した 1 transaction
//! で行い、途中失敗で新旧 bundle が混在しないことを保証する。

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use comic_core::{AnnotationKind, AnnotationObject};
use local_adjust_core::LocalAdjustmentLayer;
use rusqlite::{Connection, params};

use crate::adjustment::AdjustParams;
use crate::export_crop::CropSettings;
use crate::mask_db::Shape;

#[derive(Clone, Debug, PartialEq)]
pub struct EditMaskSnapshot {
    pub pixels: Vec<bool>,
    pub shapes: Vec<Shape>,
    pub size: [usize; 2],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageEditBundle {
    pub source_size: [usize; 2],
    pub adjust: Option<AdjustParams>,
    pub mask: Option<EditMaskSnapshot>,
    pub conceal: Option<EditMaskSnapshot>,
    pub local_adjust_layers: Option<Vec<LocalAdjustmentLayer>>,
    pub export_crop: Option<CropSettings>,
    pub comic: Option<Vec<AnnotationObject>>,
}

#[derive(Clone, Debug)]
pub struct EditBundleClipboard {
    pub source_label: String,
    pub bundle: PageEditBundle,
}

#[derive(Clone, Debug)]
pub struct EditBundlePasteRequest {
    pub target_idx: usize,
    pub target_key: String,
    pub target_label: String,
    pub target_size: [usize; 2],
    pub items_generation: u64,
    /// 一覧が更新されても sidecar mirror を正しいフォルダへ反映するための固定座標。
    pub sidecar_coords: Option<(PathBuf, String)>,
}

pub struct EditBundleCopyPending {
    pub source_label: String,
    pub rx: Receiver<Result<PageEditBundle, String>>,
}

pub struct EditBundleApplyPending {
    pub request: EditBundlePasteRequest,
    pub source_label: String,
    pub rx: Receiver<Result<PreparedPageEditBundle, String>>,
}

#[derive(Clone, Debug)]
pub struct PreparedEditMask {
    pub compressed: Vec<u8>,
    pub shapes: Vec<Shape>,
    pub shapes_json: Option<String>,
    pub size: [usize; 2],
}

#[derive(Clone, Debug)]
pub struct PreparedPageEditBundle {
    pub source_size: [usize; 2],
    pub adjust: Option<AdjustParams>,
    pub adjust_json: Option<String>,
    pub mask: Option<PreparedEditMask>,
    pub conceal: Option<PreparedEditMask>,
    pub local_adjust_layers: Option<Vec<LocalAdjustmentLayer>>,
    pub local_adjust_json: Option<String>,
    pub export_crop: Option<CropSettings>,
    pub comic: Option<Vec<AnnotationObject>>,
    pub comic_json: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EditBundleDbPaths {
    pub adjustment: PathBuf,
    pub mask: PathBuf,
    pub conceal: PathBuf,
    pub local_adjust: PathBuf,
    pub export_crop: PathBuf,
    pub comic: PathBuf,
}

impl EditBundleDbPaths {
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            adjustment: dir.join("adjustment.db"),
            mask: dir.join("mask.db"),
            conceal: dir.join("conceal.db"),
            local_adjust: dir.join("local_adjust.db"),
            export_crop: dir.join("export_crop.db"),
            comic: dir.join("comic.db"),
        }
    }

    pub fn default_data_dir() -> Self {
        Self::in_dir(&crate::data_dir::get())
    }
}

impl PageEditBundle {
    pub fn has_any(&self) -> bool {
        self.adjust.is_some()
            || self.mask.is_some()
            || self.conceal.is_some()
            || self
                .local_adjust_layers
                .as_ref()
                .is_some_and(|layers| !layers.is_empty())
            || self.export_crop.is_some()
            || self
                .comic
                .as_ref()
                .is_some_and(|objects| !objects.is_empty())
    }

    /// snapshot を別 canvas の canonical source-pixel 空間へ変換する。
    ///
    /// - ラスターマスクは再サンプルする。
    /// - ベクター座標 / crop は X/Y を個別比率で変換する。
    /// - 太さや px 単位の効果値は X/Y 比率の平均で変換する。
    /// - 注釈の anchor は正規化位置を維持し、オブジェクト形状は縦横比を壊さない
    ///   uniform scale とする。canvas 外は最終 rasterize 時に clip される。
    pub fn transformed_to(&self, target_size: [usize; 2]) -> Result<Self, String> {
        let [source_w, source_h] = self.source_size;
        let [target_w, target_h] = target_size;
        if source_w == 0 || source_h == 0 || target_w == 0 || target_h == 0 {
            return Err("画像サイズを取得できませんでした".to_string());
        }
        if self.source_size == target_size {
            return Ok(self.clone());
        }

        let sx = target_w as f32 / source_w as f32;
        let sy = target_h as f32 / source_h as f32;
        let length_scale = (sx + sy) * 0.5;

        let transform_mask = |snapshot: &EditMaskSnapshot| {
            let mut shapes = snapshot.shapes.clone();
            for shape in &mut shapes {
                shape.scale_xy(sx, sy);
            }
            EditMaskSnapshot {
                pixels: crate::mask_db::rescale_mask(
                    &snapshot.pixels,
                    snapshot.size[0],
                    snapshot.size[1],
                    target_w,
                    target_h,
                ),
                shapes,
                size: target_size,
            }
        };

        let local_adjust_layers = self
            .local_adjust_layers
            .as_ref()
            .map(|layers| transform_local_adjust_layers(layers, target_size, length_scale))
            .transpose()?;

        let export_crop = self
            .export_crop
            .map(|mut crop| {
                crop.rect.min_x *= sx;
                crop.rect.max_x *= sx;
                crop.rect.min_y *= sy;
                crop.rect.max_y *= sy;
                crop.sanitized(target_w, target_h)
            })
            .filter(|crop| !crop.is_full(target_w, target_h));

        Ok(Self {
            source_size: target_size,
            adjust: self.adjust.clone(),
            mask: self.mask.as_ref().map(transform_mask),
            conceal: self.conceal.as_ref().map(transform_mask),
            local_adjust_layers,
            export_crop,
            comic: self
                .comic
                .as_ref()
                .map(|objects| transform_annotations(objects, sx, sy, target_size)),
        })
    }

    pub fn prepare(&self) -> Result<PreparedPageEditBundle, String> {
        let prepare_mask = |snapshot: &EditMaskSnapshot| PreparedEditMask {
            compressed: crate::mask_db::compress_mask(&snapshot.pixels),
            shapes: snapshot.shapes.clone(),
            shapes_json: crate::mask_db::shapes_to_json(&snapshot.shapes),
            size: snapshot.size,
        };
        let adjust_json = self
            .adjust
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| format!("個別補正の保存準備に失敗しました: {e}"))?;
        let local_adjust_json = self
            .local_adjust_layers
            .as_ref()
            .filter(|layers| !layers.is_empty())
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| format!("補正レイヤーの保存準備に失敗しました: {e}"))?;
        let comic_json = self
            .comic
            .as_ref()
            .filter(|objects| !objects.is_empty())
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| format!("テキスト注釈の保存準備に失敗しました: {e}"))?;
        Ok(PreparedPageEditBundle {
            source_size: self.source_size,
            adjust: self.adjust.clone(),
            adjust_json,
            mask: self.mask.as_ref().map(prepare_mask),
            conceal: self.conceal.as_ref().map(prepare_mask),
            local_adjust_layers: self
                .local_adjust_layers
                .as_ref()
                .filter(|layers| !layers.is_empty())
                .cloned(),
            local_adjust_json,
            export_crop: self.export_crop,
            comic: self
                .comic
                .as_ref()
                .filter(|objects| !objects.is_empty())
                .cloned(),
            comic_json,
        })
    }
}

fn transform_local_adjust_layers(
    layers: &[LocalAdjustmentLayer],
    target_size: [usize; 2],
    length_scale: f32,
) -> Result<Vec<LocalAdjustmentLayer>, String> {
    let mut transformed = Vec::with_capacity(layers.len());
    for layer in layers {
        // LocalEffect には多数の px 単位パラメータがある。永続 JSON の `_px` 命名を
        // schema contract として一括変換し、新しい effect 追加時の取りこぼしを防ぐ。
        let mut value = serde_json::to_value(layer)
            .map_err(|e| format!("補正レイヤーのサイズ変換に失敗しました: {e}"))?;
        scale_pixel_fields(&mut value, length_scale, None);
        let mut layer: LocalAdjustmentLayer = serde_json::from_value(value)
            .map_err(|e| format!("補正レイヤーのサイズ変換に失敗しました: {e}"))?;
        layer.resize_masks_to(target_size[0], target_size[1]);
        transformed.push(layer);
    }
    Ok(transformed)
}

fn scale_pixel_fields(value: &mut serde_json::Value, scale: f32, field_name: Option<&str>) {
    match value {
        serde_json::Value::Object(map) => {
            for (name, child) in map {
                scale_pixel_fields(child, scale, Some(name));
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                scale_pixel_fields(child, scale, field_name);
            }
        }
        serde_json::Value::Number(number)
            if field_name.is_some_and(|name| name.ends_with("_px")) =>
        {
            let Some(raw) = number.as_f64() else {
                return;
            };
            let scaled = raw * scale as f64;
            *number = if number.is_i64() {
                serde_json::Number::from(scaled.round() as i64)
            } else if number.is_u64() {
                serde_json::Number::from(scaled.round().max(0.0) as u64)
            } else if let Some(number) = serde_json::Number::from_f64(scaled) {
                number
            } else {
                return;
            };
        }
        _ => {}
    }
}

fn transform_annotations(
    objects: &[AnnotationObject],
    sx: f32,
    sy: f32,
    target_size: [usize; 2],
) -> Vec<AnnotationObject> {
    let length_scale = (sx + sy) * 0.5;
    let mut transformed = comic_core::scale_scene(objects, length_scale);
    let max_x = target_size[0] as f32;
    let max_y = target_size[1] as f32;
    for (source, target) in objects.iter().zip(&mut transformed) {
        target.pivot = (
            (source.pivot.0 * sx).clamp(0.0, max_x),
            (source.pivot.1 * sy).clamp(0.0, max_y),
        );
        if let (AnnotationKind::Bubble(source), AnnotationKind::Bubble(target)) =
            (&source.kind, &mut target.kind)
            && let (Some(source_tail), Some(target_tail)) = (&source.tail, &mut target.tail)
        {
            target_tail.tip = (
                (source_tail.tip.0 * sx).clamp(0.0, max_x),
                (source_tail.tip.1 * sy).clamp(0.0, max_y),
            );
        }
    }
    transformed
}

impl PreparedPageEditBundle {
    /// 6 DB を 1 transaction で全置換する。`adjustment.db` を main database として
    /// 開くため、SQLite の multi-database super-journal が利用できる。
    pub fn apply_atomic(&self, paths: &EditBundleDbPaths, key: &str) -> Result<(), String> {
        let conn = Connection::open(&paths.adjustment)
            .map_err(|e| format!("編集DBを開けませんでした: {e}"))?;
        conn.busy_timeout(Duration::from_secs(2))
            .map_err(|e| format!("編集DBの待機設定に失敗しました: {e}"))?;
        attach(&conn, "mask_edit", &paths.mask)?;
        attach(&conn, "conceal_edit", &paths.conceal)?;
        attach(&conn, "local_edit", &paths.local_adjust)?;
        attach(&conn, "crop_edit", &paths.export_crop)?;
        attach(&conn, "comic_edit", &paths.comic)?;

        // WAL の attached database を跨ぐ transaction は crash-atomic にならない。
        // 現行 DB は DELETE journal だが、外部変更を検出した場合は安全側で中止する。
        for schema in [
            "main",
            "mask_edit",
            "conceal_edit",
            "local_edit",
            "crop_edit",
            "comic_edit",
        ] {
            let mode: String = conn
                .query_row(&format!("PRAGMA {schema}.journal_mode"), [], |row| {
                    row.get(0)
                })
                .map_err(|e| format!("編集DBのjournal確認に失敗しました: {e}"))?;
            if ["wal", "memory", "off"]
                .iter()
                .any(|unsafe_mode| mode.eq_ignore_ascii_case(unsafe_mode))
            {
                return Err(format!(
                    "編集DBが{mode}モードのため、安全な一括貼り付けを実行できません"
                ));
            }
        }

        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| format!("編集DBの一括更新を開始できませんでした: {e}"))?;
        let result = self.apply_inside_transaction(&conn, key);
        match result {
            Ok(()) => conn
                .execute_batch("COMMIT")
                .map_err(|e| format!("編集内容を確定できませんでした: {e}")),
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn apply_inside_transaction(&self, conn: &Connection, key: &str) -> Result<(), String> {
        if let Some(json) = &self.adjust_json {
            conn.execute(
                "INSERT INTO main.page_params (page_path, params_json) VALUES (?1, ?2)
                 ON CONFLICT(page_path) DO UPDATE SET params_json = excluded.params_json",
                params![key, json],
            )
            .map_err(db_write_error)?;
        } else {
            conn.execute("DELETE FROM main.page_params WHERE page_path = ?1", [key])
                .map_err(db_write_error)?;
        }

        replace_mask_row(conn, "mask_edit", "masks", "path", key, &self.mask)?;
        replace_mask_row(
            conn,
            "conceal_edit",
            "conceal_entries",
            "page_path",
            key,
            &self.conceal,
        )?;

        if let Some(json) = &self.local_adjust_json {
            conn.execute(
                "INSERT INTO local_edit.local_adjust_pages (page_path, layers_json, updated_at)
                 VALUES (?1, ?2, unixepoch())
                 ON CONFLICT(page_path) DO UPDATE SET
                    layers_json = excluded.layers_json, updated_at = unixepoch()",
                params![key, json],
            )
            .map_err(db_write_error)?;
        } else {
            conn.execute(
                "DELETE FROM local_edit.local_adjust_pages WHERE page_path = ?1",
                [key],
            )
            .map_err(db_write_error)?;
        }

        if let Some(crop) = self.export_crop {
            conn.execute(
                "INSERT INTO crop_edit.export_crop_pages
                    (page_path, min_x, min_y, max_x, max_y, aspect_mode, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
                 ON CONFLICT(page_path) DO UPDATE SET
                    min_x = excluded.min_x, min_y = excluded.min_y,
                    max_x = excluded.max_x, max_y = excluded.max_y,
                    aspect_mode = excluded.aspect_mode, updated_at = unixepoch()",
                params![
                    key,
                    crop.rect.min_x,
                    crop.rect.min_y,
                    crop.rect.max_x,
                    crop.rect.max_y,
                    crop.aspect_mode.stable_key(),
                ],
            )
            .map_err(db_write_error)?;
        } else {
            conn.execute(
                "DELETE FROM crop_edit.export_crop_pages WHERE page_path = ?1",
                [key],
            )
            .map_err(db_write_error)?;
        }

        if let Some(json) = &self.comic_json {
            conn.execute(
                "INSERT INTO comic_edit.comic_entries (page_path, doc_version, doc_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(page_path) DO UPDATE SET
                    doc_version = excluded.doc_version, doc_json = excluded.doc_json",
                params![key, crate::comic_db::DOC_VERSION as i64, json],
            )
            .map_err(db_write_error)?;
        } else {
            conn.execute(
                "DELETE FROM comic_edit.comic_entries WHERE page_path = ?1",
                [key],
            )
            .map_err(db_write_error)?;
        }
        Ok(())
    }

    pub fn to_sidecar_entry(&self) -> crate::sidecar::SidecarEntry {
        let sidecar_mask = |mask: &PreparedEditMask| {
            crate::sidecar::SidecarMask::from_raw(
                &mask.compressed,
                &mask.shapes,
                mask.size[0] as u32,
                mask.size[1] as u32,
            )
        };
        crate::sidecar::SidecarEntry {
            adjust: self.adjust.clone(),
            mask: self.mask.as_ref().map(sidecar_mask),
            conceal: self.conceal.as_ref().map(sidecar_mask),
            local_adjust_layers: self.local_adjust_layers.clone(),
            export_crop: self.export_crop,
            comic: self.comic.clone(),
            tags: None,
        }
    }
}

fn attach(conn: &Connection, schema: &str, path: &Path) -> Result<(), String> {
    conn.execute(
        &format!("ATTACH DATABASE ?1 AS {schema}"),
        [path.to_string_lossy().as_ref()],
    )
    .map(|_| ())
    .map_err(|e| format!("編集DBを一括更新用に開けませんでした: {e}"))
}

fn replace_mask_row(
    conn: &Connection,
    schema: &str,
    table: &str,
    key_column: &str,
    key: &str,
    mask: &Option<PreparedEditMask>,
) -> Result<(), String> {
    if let Some(mask) = mask {
        let sql = if table == "masks" {
            format!(
                "INSERT INTO {schema}.{table} ({key_column}, mask_data, width, height, vectors)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT({key_column}) DO UPDATE SET
                    mask_data = excluded.mask_data, width = excluded.width,
                    height = excluded.height, vectors = excluded.vectors"
            )
        } else {
            format!(
                "INSERT INTO {schema}.{table} ({key_column}, bitmap_w, bitmap_h, bitmap_data, shapes)
                 VALUES (?1, ?3, ?4, ?2, ?5)
                 ON CONFLICT({key_column}) DO UPDATE SET
                    bitmap_w = excluded.bitmap_w, bitmap_h = excluded.bitmap_h,
                    bitmap_data = excluded.bitmap_data, shapes = excluded.shapes"
            )
        };
        conn.execute(
            &sql,
            params![
                key,
                &mask.compressed,
                mask.size[0] as i64,
                mask.size[1] as i64,
                mask.shapes_json.as_deref(),
            ],
        )
        .map_err(db_write_error)?;
    } else {
        conn.execute(
            &format!("DELETE FROM {schema}.{table} WHERE {key_column} = ?1"),
            [key],
        )
        .map_err(db_write_error)?;
    }
    Ok(())
}

fn db_write_error(error: rusqlite::Error) -> String {
    format!("編集内容の一括更新に失敗しました: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export_crop::{CropAspectMode, CropRect};
    use crate::mask_db::{LineKind, ShapeOp};

    fn sample_mask() -> EditMaskSnapshot {
        EditMaskSnapshot {
            pixels: vec![true, false, false, true],
            shapes: vec![Shape::Line {
                op: ShapeOp::Add,
                kind: LineKind::Diagonal,
                p0: (1.0, 1.0),
                p1: (2.0, 2.0),
                thickness: 2.0,
            }],
            size: [2, 2],
        }
    }

    #[test]
    fn transformed_bundle_resamples_masks_and_scales_crop() {
        let bundle = PageEditBundle {
            source_size: [2, 2],
            mask: Some(sample_mask()),
            export_crop: Some(CropSettings {
                rect: CropRect {
                    min_x: 0.5,
                    min_y: 0.25,
                    max_x: 1.5,
                    max_y: 1.75,
                },
                aspect_mode: CropAspectMode::Free,
            }),
            ..Default::default()
        };
        let transformed = bundle.transformed_to([4, 6]).unwrap();
        let mask = transformed.mask.unwrap();
        assert_eq!(mask.size, [4, 6]);
        assert_eq!(mask.pixels.len(), 24);
        match &mask.shapes[0] {
            Shape::Line {
                p0, p1, thickness, ..
            } => {
                assert_eq!(*p0, (2.0, 3.0));
                assert_eq!(*p1, (4.0, 6.0));
                assert_eq!(*thickness, 5.0);
            }
            _ => panic!("expected line"),
        }
        let crop = transformed.export_crop.unwrap();
        assert_eq!(crop.rect.min_x, 1.0);
        assert_eq!(crop.rect.min_y, 0.75);
        assert_eq!(crop.rect.max_x, 3.0);
        assert_eq!(crop.rect.max_y, 5.25);
    }

    #[test]
    fn pixel_field_scaling_preserves_signed_values() {
        let mut value = serde_json::json!({
            "offset_px": -4,
            "radius_px": 6,
            "opacity": 0.5
        });
        scale_pixel_fields(&mut value, 1.5, None);
        assert_eq!(value["offset_px"], -6);
        assert_eq!(value["radius_px"], 9);
        assert_eq!(value["opacity"], 0.5);
    }

    #[test]
    fn transformed_full_crop_is_removed() {
        let bundle = PageEditBundle {
            source_size: [100, 100],
            export_crop: Some(CropSettings {
                rect: CropRect {
                    min_x: 0.0,
                    min_y: 0.0,
                    max_x: 100.0,
                    max_y: 100.0,
                },
                aspect_mode: CropAspectMode::Free,
            }),
            ..Default::default()
        };
        assert!(
            bundle
                .transformed_to([200, 300])
                .unwrap()
                .export_crop
                .is_none()
        );
    }

    fn init_bundle_databases(paths: &EditBundleDbPaths) {
        let _ = crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment).unwrap();
        let _ = crate::mask_db::MaskDb::open_at(&paths.mask).unwrap();
        let _ = crate::conceal_db::ConcealDb::open_at(&paths.conceal).unwrap();
        let _ = crate::local_adjust_db::LocalAdjustDb::open_at(&paths.local_adjust).unwrap();
        let _ = crate::export_crop::CropDb::open_at(&paths.export_crop).unwrap();
        let _ = crate::comic_db::ComicDb::open_at(&paths.comic).unwrap();
    }

    #[test]
    fn atomic_apply_replaces_missing_systems_instead_of_merging() {
        let dir = tempfile::tempdir().unwrap();
        let paths = EditBundleDbPaths::in_dir(dir.path());
        init_bundle_databases(&paths);
        let key = "target";
        let old_adjust = AdjustParams::default();
        crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment)
            .unwrap()
            .set_page_params(key, &old_adjust)
            .unwrap();
        crate::mask_db::MaskDb::open_at(&paths.mask)
            .unwrap()
            .set(key, &sample_mask().pixels, &sample_mask().shapes, 2, 2)
            .unwrap();

        let bundle = PageEditBundle {
            source_size: [2, 2],
            conceal: Some(sample_mask()),
            ..Default::default()
        };
        bundle.prepare().unwrap().apply_atomic(&paths, key).unwrap();

        assert!(
            crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment)
                .unwrap()
                .get_page_params(key)
                .is_none()
        );
        assert!(
            crate::mask_db::MaskDb::open_at(&paths.mask)
                .unwrap()
                .dimensions(key)
                .is_none()
        );
        assert!(
            crate::conceal_db::ConcealDb::open_at(&paths.conceal)
                .unwrap()
                .dimensions(key)
                .is_some()
        );
    }

    #[test]
    fn atomic_apply_rolls_back_all_databases_when_one_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let paths = EditBundleDbPaths::in_dir(dir.path());
        init_bundle_databases(&paths);
        let key = "target";
        let old_adjust = AdjustParams::default();
        crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment)
            .unwrap()
            .set_page_params(key, &old_adjust)
            .unwrap();
        Connection::open(&paths.local_adjust)
            .unwrap()
            .execute("DROP TABLE local_adjust_pages", [])
            .unwrap();

        let mut replacement = AdjustParams::default();
        replacement.brightness = 0.25;
        let bundle = PageEditBundle {
            source_size: [2, 2],
            adjust: Some(replacement),
            local_adjust_layers: Some(Vec::new()),
            ..Default::default()
        };
        assert!(bundle.prepare().unwrap().apply_atomic(&paths, key).is_err());
        assert_eq!(
            crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment)
                .unwrap()
                .get_page_params(key),
            Some(old_adjust)
        );
    }
}
