//! Worker 専用の検出層。ツール・マスクの形・余白・UI 状態は扱わない。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::AiError;
use super::runtime::AiRuntime;

/// 画像座標 (元の画像の px)。左上と右下。
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub class_idx: usize,
    pub score: f32,
    pub rect: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct FaceDetectParams {
    pub score_threshold: f32,
    pub nms_iou: f32,
}

impl Default for FaceDetectParams {
    fn default() -> Self {
        Self {
            score_threshold: 0.6,
            nms_iou: 0.3,
        }
    }
}

#[derive(Debug)]
pub enum DetectError {
    ModelMissing(PathBuf),
    LoadFailed(AiError),
    InferenceFailed(AiError),
    OutputShape(String),
    InvalidInput(String),
    Cancelled,
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelMissing(path) => write!(f, "Detection model not found: {}", path.display()),
            Self::LoadFailed(error) => write!(f, "Detection model load failed: {error}"),
            Self::InferenceFailed(error) => write!(f, "Detection inference failed: {error}"),
            Self::OutputShape(detail) => write!(f, "Unexpected detection output: {detail}"),
            Self::InvalidInput(detail) => write!(f, "Invalid detection input: {detail}"),
            Self::Cancelled => write!(f, "Detection cancelled"),
        }
    }
}

impl std::error::Error for DetectError {}

/// 長辺 640/1280/2560 のうち元より小さい段と、元の寸法。
/// 各段は元画像から直接縮小する。整数丸め後の縦横それぞれの倍率を逆変換に使う。
fn face_detect_stages(size: [usize; 2]) -> Vec<[usize; 2]> {
    let long = size[0].max(size[1]);
    if size.contains(&0) {
        return Vec::new();
    }
    let mut stages = Vec::new();
    for edge in [640, 1280, 2560] {
        if edge < long {
            stages.push(
                size.map(|n| ((n as f64 * edge as f64 / long as f64).round() as usize).max(1)),
            );
        }
    }
    stages.push(size);
    stages
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), DetectError> {
    if cancel.load(Ordering::Acquire) {
        Err(DetectError::Cancelled)
    } else {
        Ok(())
    }
}

/// RGB 抽出は既存の被写体マスク入力と同じ color.r()/g()/b()。
fn source_rgb(image: &egui::ColorImage) -> Result<image::RgbImage, DetectError> {
    let [w, h] = image.size;
    if w == 0
        || h == 0
        || w > u32::MAX as usize
        || h > u32::MAX as usize
        || w.checked_mul(h) != Some(image.pixels.len())
    {
        return Err(DetectError::InvalidInput(
            "invalid source dimensions/pixels".into(),
        ));
    }
    let pixels = image
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b()])
        .collect();
    image::RgbImage::from_raw(w as u32, h as u32, pixels)
        .ok_or_else(|| DetectError::InvalidInput("invalid RGB buffer".into()))
}

/// BGR float 0..255、NCHW。32 の倍数まで右と下を 0 埋めする。
fn face_detect_input(rgb: &image::RgbImage) -> ndarray::Array4<f32> {
    let w = rgb.width() as usize;
    let h = rgb.height() as usize;
    let padded_w = w.div_ceil(32) * 32;
    let padded_h = h.div_ceil(32) * 32;
    let mut input = ndarray::Array4::zeros((1, 3, padded_h, padded_w));
    for (x, y, pixel) in rgb.enumerate_pixels() {
        for c in 0..3 {
            input[[0, c, y as usize, x as usize]] = pixel[2 - c] as f32;
        }
    }
    input
}

/// Worker から呼ぶ。同期のモデル照合・読み込み・推論を含むので UI から呼ばない。
/// キャンセルは段の前と、ロード後・推論後・返却前で確認する。
pub fn detect_faces(
    runtime: &AiRuntime,
    model_path: &Path,
    image: &egui::ColorImage,
    params: &FaceDetectParams,
    cancel: &AtomicBool,
) -> Result<Vec<Detection>, DetectError> {
    let total = DetectPerf::new("detect_faces_total", image.size);
    let result = (|| -> Result<Vec<Detection>, DetectError> {
        check_cancel(cancel)?;
        if !params.score_threshold.is_finite()
            || !(0.0..=1.0).contains(&params.score_threshold)
            || !params.nms_iou.is_finite()
            || !(0.0..=1.0).contains(&params.nms_iou)
        {
            return Err(DetectError::InvalidInput(
                "thresholds must be in 0..=1".into(),
            ));
        }
        let mut rgb = None;
        let mut combined = Vec::new();
        // Validate the source even when no valid stage could be constructed.
        if image.size.contains(&0) {
            return Err(DetectError::InvalidInput("empty image".into()));
        }
        for size in face_detect_stages(image.size) {
            check_cancel(cancel)?;
            let preprocess = DetectPerf::new("detect_faces_preprocess", size);
            if rgb.is_none() {
                rgb = Some(source_rgb(image)?);
            }
            let source = rgb.as_ref().unwrap();
            // image::thumbnail averages source areas when reducing, like INTER_AREA.
            let reduced;
            let stage_rgb = if size == image.size {
                source
            } else {
                reduced = image::imageops::thumbnail(source, size[0] as u32, size[1] as u32);
                &reduced
            };
            let input = face_detect_input(stage_rgb);
            let padded = [input.shape()[3], input.shape()[2]];
            drop(preprocess);
            let stage = runtime
                .with_external_session(model_path, |session| {
                    // Keep typed detection errors separate from session creation errors.
                    Ok((|| -> Result<Vec<Detection>, DetectError> {
                        check_cancel(cancel)?;
                        let inference = DetectPerf::new("detect_faces_inference", size);
                        let tensor = ort::value::Tensor::from_array(input).map_err(|e| {
                            DetectError::InferenceFailed(AiError::ort_status(
                                &e,
                                format!("YuNet input: {e}"),
                            ))
                        })?;
                        let outputs =
                            session.run(ort::inputs!["input" => tensor]).map_err(|e| {
                                DetectError::InferenceFailed(AiError::ort_status(
                                    &e,
                                    format!("YuNet run: {e}"),
                                ))
                            })?;
                        drop(inference);
                        check_cancel(cancel)?;
                        let _postprocess = DetectPerf::new("detect_faces_postprocess", size);
                        let boxes = decode_yunet(padded, params, |name| {
                            let value = outputs.get(name).ok_or_else(|| {
                                DetectError::OutputShape(format!("missing {name}"))
                            })?;
                            let (shape, data) = value
                                .try_extract_tensor::<f32>()
                                .map_err(|e| DetectError::OutputShape(format!("{name}: {e}")))?;
                            Ok(DetectTensor {
                                shape: shape.as_ref(),
                                data,
                            })
                        })?;
                        Ok(restore_face_coordinates(boxes, size, image.size))
                    })())
                })
                .map_err(|error| match error {
                    AiError::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound => {
                        DetectError::ModelMissing(model_path.to_owned())
                    }
                    other => DetectError::LoadFailed(other),
                })??;
            combined.extend(stage);
        }
        check_cancel(cancel)?;
        let result = detect_nms(combined, params.nms_iou);
        check_cancel(cancel)?;
        Ok(result)
    })();
    drop(total);
    result
}

struct DetectTensor<'a> {
    shape: &'a [i64],
    data: &'a [f32],
}

fn checked_output<'a>(
    name: &str,
    value: DetectTensor<'a>,
    anchors: usize,
    channels: usize,
) -> Result<&'a [f32], DetectError> {
    if value.shape != [1, anchors as i64, channels as i64] || value.data.len() != anchors * channels
    {
        return Err(DetectError::OutputShape(format!(
            "{name}: expected [1,{anchors},{channels}], got {:?} ({} values)",
            value.shape,
            value.data.len()
        )));
    }
    Ok(value.data)
}

/// 名前・次元を検証して読む。未使用の kps も形式の契約を確認する。
fn decode_yunet<'a>(
    padded: [usize; 2],
    params: &FaceDetectParams,
    mut output: impl FnMut(&str) -> Result<DetectTensor<'a>, DetectError>,
) -> Result<Vec<Detection>, DetectError> {
    let [w, h] = padded;
    let mut boxes = Vec::new();
    for stride in [8, 16, 32] {
        let cols = w / stride;
        let anchors = cols * (h / stride);
        let mut read = |prefix, channels| {
            let name = format!("{prefix}_{stride}");
            checked_output(&name, output(&name)?, anchors, channels)
        };
        let cls = read("cls", 1)?;
        let obj = read("obj", 1)?;
        let bbox = read("bbox", 4)?;
        read("kps", 10)?;
        for i in 0..anchors {
            let score = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
            if !score.is_finite() || score < params.score_threshold {
                continue;
            }
            let b = &bbox[i * 4..i * 4 + 4];
            let s = stride as f32;
            let cx = (i % cols) as f32 * s + b[0] * s;
            let cy = (i / cols) as f32 * s + b[1] * s;
            let half_w = b[2].exp() * s * 0.5;
            let half_h = b[3].exp() * s * 0.5;
            let rect = [cx - half_w, cy - half_h, cx + half_w, cy + half_h];
            if rect.iter().any(|v| !v.is_finite()) {
                return Err(DetectError::OutputShape(format!(
                    "bbox_{stride}: non-finite box at {i}"
                )));
            }
            boxes.push(Detection {
                class_idx: 0,
                score,
                rect,
            });
        }
    }
    Ok(detect_nms(boxes, params.nms_iou))
}

fn restore_face_coordinates(
    boxes: Vec<Detection>,
    stage: [usize; 2],
    original: [usize; 2],
) -> Vec<Detection> {
    let scale_x = original[0] as f32 / stage[0] as f32;
    let scale_y = original[1] as f32 / stage[1] as f32;
    boxes
        .into_iter()
        .filter_map(|mut detection| {
            for (i, value) in detection.rect.iter_mut().enumerate() {
                let axis = i % 2;
                *value = (*value * if axis == 0 { scale_x } else { scale_y })
                    .clamp(0.0, original[axis] as f32);
            }
            (detection.rect[2] > detection.rect[0] && detection.rect[3] > detection.rect[1])
                .then_some(detection)
        })
        .collect()
}

fn detect_iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let intersection =
        (a[2].min(b[2]) - a[0].max(b[0])).max(0.0) * (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = |r: &[f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - intersection;
    if union > 0.0 {
        intersection / union
    } else {
        0.0
    }
}

fn detect_nms(mut boxes: Vec<Detection>, iou: f32) -> Vec<Detection> {
    boxes.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for candidate in boxes {
        if kept.iter().all(|other| {
            other.class_idx != candidate.class_idx
                || detect_iou(&candidate.rect, &other.rect) <= iou
        }) {
            kept.push(candidate);
        }
    }
    kept
}

/// RAII records successful and error/cancelled stages alike, matching AI event fields.
struct DetectPerf {
    name: &'static str,
    size: [usize; 2],
    start: Option<Instant>,
}

impl DetectPerf {
    fn new(name: &'static str, size: [usize; 2]) -> Self {
        Self {
            name,
            size,
            start: crate::perf::is_enabled().then(Instant::now),
        }
    }
}

impl Drop for DetectPerf {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            crate::perf::event(
                "ai",
                self.name,
                None,
                0,
                &[
                    ("w", self.size[0].into()),
                    ("h", self.size[1].into()),
                    ("ms", (start.elapsed().as_secs_f64() * 1000.0).into()),
                ],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct DetectOutputs(HashMap<String, (Vec<i64>, Vec<f32>)>);

    impl DetectOutputs {
        fn new(w: usize, h: usize) -> Self {
            let mut values = HashMap::new();
            for stride in [8, 16, 32] {
                let n = (w / stride) * (h / stride);
                for (prefix, channels) in [("cls", 1), ("obj", 1), ("bbox", 4), ("kps", 10)] {
                    values.insert(
                        format!("{prefix}_{stride}"),
                        (vec![1, n as i64, channels as i64], vec![0.0; n * channels]),
                    );
                }
            }
            Self(values)
        }

        fn read(&self, name: &str) -> Result<DetectTensor<'_>, DetectError> {
            let (shape, data) = self
                .0
                .get(name)
                .ok_or_else(|| DetectError::OutputShape(format!("missing {name}")))?;
            Ok(DetectTensor { shape, data })
        }

        fn values(&mut self, name: &str) -> &mut Vec<f32> {
            &mut self.0.get_mut(name).unwrap().1
        }

        fn decode(&self, params: &FaceDetectParams) -> Result<Vec<Detection>, DetectError> {
            decode_yunet([64, 64], params, |name| self.read(name))
        }
    }

    fn face(score: f32, rect: [f32; 4]) -> Detection {
        Detection {
            class_idx: 0,
            score,
            rect,
        }
    }

    #[test]
    fn detect_stage_long_edges_and_aspect_ratio() {
        for (long, expected) in [
            (4608, vec![640, 1280, 2560, 4608]),
            (1500, vec![640, 1280, 1500]),
            (640, vec![640]),
            (500, vec![500]),
        ] {
            let stages = face_detect_stages([long, long / 2]);
            assert_eq!(
                stages.iter().map(|s| s[0].max(s[1])).collect::<Vec<_>>(),
                expected
            );
            assert!(stages.iter().all(|s| s[0] == s[1] * 2));
            assert_eq!(
                face_detect_stages([long / 2, long]),
                stages.iter().map(|s| [s[1], s[0]]).collect::<Vec<_>>()
            );
        }
        assert_eq!(face_detect_stages([641, 481]), vec![[640, 480], [641, 481]]);
    }

    #[test]
    fn detect_padding_is_zero_and_input_is_bgr_unnormalized() {
        let image = egui::ColorImage::filled([641, 481], egui::Color32::from_rgb(11, 22, 33));
        let rgb = source_rgb(&image).unwrap();
        let input = face_detect_input(&rgb);
        assert_eq!(input.shape(), &[1, 3, 512, 672]);
        for c in 0..3 {
            assert_eq!(input[[0, c, 0, 0]], [33.0, 22.0, 11.0][c]);
            assert_eq!(input[[0, c, 480, 640]], [33.0, 22.0, 11.0][c]);
            for y in 0..512 {
                for x in 0..672 {
                    if y >= 481 || x >= 641 {
                        assert_eq!(input[[0, c, y, x]], 0.0);
                    }
                }
            }
        }
    }

    #[test]
    fn detect_yunet_decodes_named_anchor_score_and_box() {
        let mut outputs = DetectOutputs::new(64, 64);
        let anchor = 2 * 8 + 3;
        outputs.values("cls_8")[anchor] = 0.64;
        outputs.values("obj_8")[anchor] = 0.81;
        outputs.values("bbox_8")[anchor * 4..anchor * 4 + 4].copy_from_slice(&[
            0.5,
            0.25,
            2.0_f32.ln(),
            3.0_f32.ln(),
        ]);
        let result = outputs.decode(&FaceDetectParams::default()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].class_idx, 0);
        assert!((result[0].score - 0.72).abs() < 1e-6);
        for (actual, expected) in result[0].rect.iter().zip([20.0, 6.0, 36.0, 30.0]) {
            assert!((actual - expected).abs() < 1e-5);
        }
    }

    #[test]
    fn detect_yunet_clamps_cls_and_obj_before_sqrt_and_includes_threshold() {
        for (cls, obj, count) in [
            (4.0, 0.25, 1),
            (0.25, 4.0, 1),
            (-1.0, 1.0, 0),
            (1.0, -1.0, 0),
        ] {
            let mut outputs = DetectOutputs::new(64, 64);
            outputs.values("cls_8")[9] = cls;
            outputs.values("obj_8")[9] = obj;
            let result = outputs
                .decode(&FaceDetectParams {
                    score_threshold: 0.5,
                    nms_iou: 0.3,
                })
                .unwrap();
            assert_eq!(result.len(), count);
            if count == 1 {
                assert_eq!(result[0].score, 0.5);
            }
        }
    }

    #[test]
    fn detect_yunet_uses_padded_columns_and_all_strides() {
        let mut outputs = DetectOutputs::new(96, 64);
        for stride in [8, 16, 32] {
            let anchor = 96 / stride + 1;
            outputs.values(&format!("cls_{stride}"))[anchor] = 1.0;
            outputs.values(&format!("obj_{stride}"))[anchor] = 1.0;
            // Different strides have non-overlapping boxes at their own grid coordinates.
        }
        let result = decode_yunet([96, 64], &FaceDetectParams::default(), |name| {
            outputs.read(name)
        })
        .unwrap();
        assert_eq!(result.len(), 3);
        for stride in [8, 16, 32] {
            let s = stride as f32;
            assert!(
                result
                    .iter()
                    .any(|d| d.rect == [s * 0.5, s * 0.5, s * 1.5, s * 1.5])
            );
        }
    }

    #[test]
    fn detect_yunet_rejects_missing_transposed_truncated_and_kps_shapes() {
        for name in ["cls_8", "obj_16", "bbox_16", "kps_8"] {
            for bad in 0..4 {
                let mut outputs = DetectOutputs::new(64, 64);
                match bad {
                    0 => {
                        outputs.0.remove(name);
                    }
                    1 => {
                        outputs.0.get_mut(name).unwrap().0.swap(1, 2);
                    }
                    2 => {
                        outputs.values(name).pop();
                    }
                    _ => {
                        outputs.0.get_mut(name).unwrap().0[0] = 2;
                    }
                }
                assert!(
                    matches!(
                        outputs.decode(&FaceDetectParams::default()),
                        Err(DetectError::OutputShape(_))
                    ),
                    "{name} case {bad}"
                );
            }
        }
    }

    #[test]
    fn detect_nms_merges_stages_preserves_separate_faces_and_highest_score() {
        let first = restore_face_coordinates(
            vec![face(0.8, [10.0, 20.0, 30.0, 40.0])],
            [640, 320],
            [1280, 640],
        );
        let mut combined = first;
        combined.push(face(0.9, [20.0, 40.0, 60.0, 80.0]));
        combined.push(face(0.7, [200.0, 200.0, 220.0, 220.0]));
        assert_eq!(
            detect_nms(combined, 0.3),
            vec![
                face(0.9, [20.0, 40.0, 60.0, 80.0]),
                face(0.7, [200.0, 200.0, 220.0, 220.0])
            ]
        );
    }

    #[test]
    fn detect_nms_uses_the_given_iou_threshold() {
        // IoU = 50 / 150 = 1/3: merged at 0.3, both kept at 0.5 (catches a wrong threshold).
        let a = face(0.9, [0.0, 0.0, 10.0, 10.0]);
        let b = face(0.8, [5.0, 0.0, 15.0, 10.0]);
        assert!((detect_iou(&a.rect, &b.rect) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(detect_nms(vec![a.clone(), b.clone()], 0.3), vec![a.clone()]);
        assert_eq!(detect_nms(vec![a.clone(), b.clone()], 0.5), vec![a, b]);
    }

    #[test]
    fn detect_restores_both_axes_with_their_own_scale() {
        // Stage 640x480 of a 1280x1440 original: x scale 2, y scale 3, no clipping.
        let result = restore_face_coordinates(
            vec![face(0.9, [100.0, 50.0, 200.0, 150.0])],
            [640, 480],
            [1280, 1440],
        );
        assert_eq!(result, vec![face(0.9, [200.0, 150.0, 400.0, 450.0])]);
    }

    #[test]
    fn detect_restores_each_axis_clips_image_and_drops_padding_only_boxes() {
        let boxes = vec![
            face(0.9, [-10.0, 10.0, 650.0, 490.0]),
            face(0.8, [650.0, 100.0, 670.0, 200.0]),
        ];
        let result = restore_face_coordinates(boxes, [640, 480], [641, 481]);
        assert_eq!(result.len(), 1);
        for (actual, expected) in
            result[0]
                .rect
                .iter()
                .zip([0.0, 10.0 * 481.0 / 480.0, 641.0, 481.0])
        {
            assert!((actual - expected).abs() < 1e-5);
        }
    }

    #[test]
    fn detect_cancelled_and_invalid_input_do_not_load_runtime() {
        let runtime = AiRuntime::empty_for_test();
        let image = egui::ColorImage::filled([1, 1], egui::Color32::BLACK);
        let result = detect_faces(
            &runtime,
            Path::new("not-a-model.onnx"),
            &image,
            &FaceDetectParams::default(),
            &AtomicBool::new(true),
        );
        assert!(matches!(result, Err(DetectError::Cancelled)));
        let params = FaceDetectParams {
            score_threshold: f32::NAN,
            ..Default::default()
        };
        assert!(matches!(
            detect_faces(
                &runtime,
                Path::new("not-a-model.onnx"),
                &image,
                &params,
                &AtomicBool::new(false)
            ),
            Err(DetectError::InvalidInput(_))
        ));
        assert!(matches!(
            detect_faces(
                &runtime,
                Path::new("not-a-model.onnx"),
                &image,
                &FaceDetectParams::default(),
                &AtomicBool::new(false)
            ),
            Err(DetectError::ModelMissing(_))
        ));
    }
}
