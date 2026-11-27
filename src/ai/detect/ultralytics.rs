//! Worker-only Ultralytics letterbox, inference, class-aware NMS and mask patches.

use super::{DetectError, DetectTensor, Detection, check_cancel, detect_iou};
use crate::ai::AiError;
use crate::ai::detector_catalog::{DetectorFormat, DetectorInfo};
use crate::ai::runtime::{AiRuntime, ExternalModelKey};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaskPatch {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) bits: Vec<bool>,
}

#[derive(Debug, Clone)]
pub(crate) struct UltralyticsParams {
    pub(crate) selected_classes: Vec<bool>,
    pub(crate) thresholds: Vec<f32>,
    pub(crate) contour_margin_percent: f32,
}

#[derive(Debug)]
pub(crate) struct UltralyticsOutput {
    pub(crate) detections: Vec<Detection>,
    /// One patch for each detection when contour was requested, otherwise empty.
    pub(crate) patches: Vec<MaskPatch>,
}

#[derive(Debug, Clone, Copy)]
struct Letterbox {
    original: [usize; 2],
    size: usize,
    resized: [usize; 2],
    left: usize,
    top: usize,
    ratio: f64,
}

impl Letterbox {
    fn new(original: [usize; 2], size: usize) -> Result<Self, DetectError> {
        if original.contains(&0)
            || size == 0
            || size
                .checked_mul(size)
                .and_then(|n| n.checked_mul(3 * size_of::<f32>()))
                .is_none_or(|n| n > isize::MAX as usize)
        {
            return Err(DetectError::InvalidInput(
                "invalid letterbox dimensions".into(),
            ));
        }
        let ratio = (size as f64 / original[0] as f64).min(size as f64 / original[1] as f64);
        // Python's round is ties-to-even, unlike Rust's round.
        let resized = original.map(|n| (n as f64 * ratio).round_ties_even() as usize);
        if resized.contains(&0) || resized.iter().any(|&n| n > size) {
            return Err(DetectError::InvalidInput(
                "letterbox rounded an axis to zero".into(),
            ));
        }
        let padding = resized.map(|n| (size - n) as f64 / 2.0);
        let left = (padding[0] - 0.1).round_ties_even() as usize;
        let top = (padding[1] - 0.1).round_ties_even() as usize;
        debug_assert_eq!(
            left + resized[0] + (padding[0] + 0.1).round_ties_even() as usize,
            size
        );
        debug_assert_eq!(
            top + resized[1] + (padding[1] + 0.1).round_ties_even() as usize,
            size
        );
        Ok(Self {
            original,
            size,
            resized,
            left,
            top,
            ratio,
        })
    }

    fn restore(&self, rect: [f32; 4]) -> [f32; 4] {
        std::array::from_fn(|i| {
            let axis = i % 2;
            let pad = [self.left, self.top][axis];
            ((rect[i] as f64 - pad as f64) / self.ratio).clamp(0.0, self.original[axis] as f64)
                as f32
        })
    }
}

/// Explicit two-neighbour interpolation with half-pixel centers and replicated edges.
/// This deliberately does not use image crate's scale-dependent triangular filter.
fn sample_axis(coordinate: f64, length: usize) -> (usize, usize, f64) {
    let coordinate = coordinate.clamp(0.0, (length - 1) as f64);
    let low = coordinate.floor() as usize;
    (low, (low + 1).min(length - 1), coordinate - low as f64)
}

fn bilinear(values: [f64; 4], fx: f64, fy: f64) -> f64 {
    let top = values[0] * (1.0 - fx) + values[1] * fx;
    let bottom = values[2] * (1.0 - fx) + values[3] * fx;
    top * (1.0 - fy) + bottom * fy
}

fn preprocess(
    image: &egui::ColorImage,
    lb: Letterbox,
) -> Result<ndarray::Array4<f32>, DetectError> {
    let [w, h] = image.size;
    if w.checked_mul(h) != Some(image.pixels.len()) {
        return Err(DetectError::InvalidInput(
            "invalid source dimensions/pixels".into(),
        ));
    }
    let mut input = ndarray::Array4::from_elem((1, 3, lb.size, lb.size), 114.0 / 255.0);
    let x_samples: Vec<_> = (0..lb.resized[0])
        .map(|x| sample_axis((x as f64 + 0.5) * w as f64 / lb.resized[0] as f64 - 0.5, w))
        .collect();
    for y in 0..lb.resized[1] {
        let (y0, y1, fy) = sample_axis((y as f64 + 0.5) * h as f64 / lb.resized[1] as f64 - 0.5, h);
        for (x, &(x0, x1, fx)) in x_samples.iter().enumerate() {
            let pixels = [
                image.pixels[y0 * w + x0],
                image.pixels[y0 * w + x1],
                image.pixels[y1 * w + x0],
                image.pixels[y1 * w + x1],
            ];
            for c in 0..3 {
                let values = pixels.map(|pixel| [pixel.r(), pixel.g(), pixel.b()][c] as f64);
                // LetterBox resizes the u8 image before its float normalization.
                input[[0, c, y + lb.top, x + lb.left]] =
                    bilinear(values, fx, fy).round_ties_even() as f32 / 255.0;
            }
        }
    }
    Ok(input)
}

fn validate_params(
    detector: &DetectorInfo,
    params: &UltralyticsParams,
    contour: bool,
) -> Result<(), DetectError> {
    let classes = detector.names.len();
    if classes == 0
        || params.selected_classes.len() != classes
        || params.thresholds.len() != classes
        || params
            .thresholds
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        || !params.contour_margin_percent.is_finite()
        || params.contour_margin_percent < 0.0
        || (contour && detector.format != DetectorFormat::UltralyticsSegment)
    {
        return Err(DetectError::InvalidInput(
            "invalid classes, thresholds or contour options".into(),
        ));
    }
    Ok(())
}

fn dilation_radius(size: [usize; 2], percent: f32) -> usize {
    ((size[0].max(size[1]) as f32 * percent / 100.0).ceil() as usize).max(2)
}

/// The catalog's metadata and the session must describe the same file identity.
fn check_detector_file(detector: &DetectorInfo) -> Result<(), DetectError> {
    match ExternalModelKey::read(&detector.key.path) {
        Ok(key) if key == detector.key => Ok(()),
        Ok(_) => Err(DetectError::DetectorChanged),
        Err(AiError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(DetectError::DetectorChanged)
        }
        Err(error) => Err(DetectError::LoadFailed(error)),
    }
}

pub(crate) fn detect_ultralytics_with_trace(
    runtime: &AiRuntime,
    detector: &DetectorInfo,
    image: &egui::ColorImage,
    params: &UltralyticsParams,
    contour: bool,
    cancel: &AtomicBool,
    trace_label: &'static str,
) -> Result<UltralyticsOutput, DetectError> {
    detect_with_radius(
        runtime,
        detector,
        image,
        params,
        contour,
        dilation_radius(image.size, params.contour_margin_percent),
        cancel,
        trace_label,
    )
}

// The raw-mask reference test uses radius zero to compare the sampled logits to
// Ultralytics retina_masks. The production entry above always applies the minimum 2px.
#[allow(clippy::too_many_arguments)]
fn detect_with_radius(
    runtime: &AiRuntime,
    detector: &DetectorInfo,
    image: &egui::ColorImage,
    params: &UltralyticsParams,
    contour: bool,
    radius: usize,
    cancel: &AtomicBool,
    trace_label: &'static str,
) -> Result<UltralyticsOutput, DetectError> {
    check_cancel(cancel)?;
    validate_params(detector, params, contour)?;
    let lb = Letterbox::new(image.size, detector.input_size)?;
    let pre = UltraPerf::new(
        "detect_ultralytics_preprocess",
        detector,
        image.size,
        trace_label,
    );
    let input = preprocess(image, lb)?;
    drop(pre);
    check_cancel(cancel)?;
    check_detector_file(detector)?;
    let result = runtime
        .with_external_session_traced(&detector.key.path, trace_label, |session| {
            Ok((|| {
                check_cancel(cancel)?;
                // The runtime may have waited for another detector while the file was replaced.
                check_detector_file(detector)?;
                if session.inputs().len() != 1 {
                    return Err(DetectError::OutputShape("expected one model input".into()));
                }
                let inference = UltraPerf::new(
                    "detect_ultralytics_inference",
                    detector,
                    image.size,
                    trace_label,
                );
                let tensor = ort::value::Tensor::from_array(input).map_err(|e| {
                    DetectError::InferenceFailed(AiError::ort_status(
                        &e,
                        format!("Detector input: {e}"),
                    ))
                })?;
                let outputs = session.run(ort::inputs![tensor]).map_err(|e| {
                    DetectError::InferenceFailed(AiError::ort_status(
                        &e,
                        format!("Detector run: {e}"),
                    ))
                })?;
                drop(inference);
                check_cancel(cancel)?;
                let mut post = UltraPerf::new(
                    "detect_ultralytics_postprocess",
                    detector,
                    image.size,
                    trace_label,
                );
                let expected = if detector.format == DetectorFormat::UltralyticsSegment {
                    2
                } else {
                    1
                };
                if outputs.len() != expected {
                    return Err(DetectError::OutputShape(format!(
                        "expected {expected} outputs, got {}",
                        outputs.len()
                    )));
                }
                let mut tensors = Vec::with_capacity(expected);
                for i in 0..expected {
                    let (shape, data) = outputs[i]
                        .try_extract_tensor::<f32>()
                        .map_err(|e| DetectError::OutputShape(format!("output {i}: {e}")))?;
                    tensors.push(DetectTensor {
                        shape: shape.as_ref(),
                        data,
                    });
                }
                let output = decode(
                    &tensors,
                    detector.format,
                    detector.names.len(),
                    lb,
                    params,
                    contour,
                    radius,
                    cancel,
                )?;
                post.count = output.detections.len();
                check_cancel(cancel)?;
                Ok(output)
            })())
        })
        .map_err(DetectError::LoadFailed)??;
    check_cancel(cancel)?;
    Ok(result)
}

#[derive(Debug)]
struct Candidate {
    detection: Detection,
    anchor: usize,
}

fn tensor_len(tensor: &DetectTensor<'_>) -> bool {
    tensor.shape.iter().try_fold(1usize, |n, &dim| {
        usize::try_from(dim).ok().and_then(|dim| n.checked_mul(dim))
    }) == Some(tensor.data.len())
}

#[allow(clippy::too_many_arguments)]
fn decode(
    outputs: &[DetectTensor<'_>],
    format: DetectorFormat,
    classes: usize,
    lb: Letterbox,
    params: &UltralyticsParams,
    contour: bool,
    radius: usize,
    cancel: &AtomicBool,
) -> Result<UltralyticsOutput, DetectError> {
    if classes == 0
        || params.selected_classes.len() != classes
        || params.thresholds.len() != classes
    {
        return Err(DetectError::InvalidInput(
            "class options do not match the detector".into(),
        ));
    }
    let segment = format == DetectorFormat::UltralyticsSegment;
    if contour && !segment {
        return Err(DetectError::InvalidInput(
            "contour requires segment output".into(),
        ));
    }
    let channels = classes
        .checked_add(if segment { 36 } else { 4 })
        .ok_or_else(|| DetectError::OutputShape("class count overflow".into()))?;
    if outputs.len() != if segment { 2 } else { 1 } {
        return Err(DetectError::OutputShape("unexpected output count".into()));
    }
    let prediction = &outputs[0];
    if prediction.shape.len() != 3
        || prediction.shape[0] != 1
        || usize::try_from(prediction.shape[1]).ok() != Some(channels)
        || prediction.shape[2] <= 0
        || !tensor_len(prediction)
    {
        return Err(DetectError::OutputShape(format!(
            "invalid prediction shape {:?}",
            prediction.shape
        )));
    }
    let anchors = prediction.shape[2] as usize;
    if segment {
        let proto = &outputs[1];
        if proto.shape.len() != 4
            || proto.shape[0] != 1
            || proto.shape[1] != 32
            || proto.shape[2] <= 0
            || proto.shape[3] <= 0
            || !tensor_len(proto)
        {
            return Err(DetectError::OutputShape(format!(
                "invalid prototype shape {:?}",
                proto.shape
            )));
        }
    }
    let at = |channel: usize, anchor: usize| prediction.data[channel * anchors + anchor];
    let mut candidates = Vec::new();
    for anchor in 0..anchors {
        if anchor % 1024 == 0 {
            check_cancel(cancel)?;
        }
        // Max-class selection precedes selection/threshold: an unselected winning
        // class must not be reinterpreted as a weaker selected class.
        let mut class_idx = 0;
        let mut score = f32::NEG_INFINITY;
        for class in 0..classes {
            let value = at(4 + class, anchor);
            if !value.is_finite() {
                return Err(DetectError::OutputShape("non-finite class score".into()));
            }
            if value > score {
                class_idx = class;
                score = value;
            }
        }
        if !params.selected_classes[class_idx] || score < params.thresholds[class_idx] {
            continue;
        }
        let xywh = [at(0, anchor), at(1, anchor), at(2, anchor), at(3, anchor)];
        if xywh.iter().any(|v| !v.is_finite()) {
            return Err(DetectError::OutputShape("non-finite box".into()));
        }
        if xywh[2] <= 0.0 || xywh[3] <= 0.0 {
            continue;
        }
        let rect = [
            xywh[0] - xywh[2] / 2.0,
            xywh[1] - xywh[3] / 2.0,
            xywh[0] + xywh[2] / 2.0,
            xywh[1] + xywh[3] / 2.0,
        ];
        if rect.iter().any(|v| !v.is_finite()) {
            return Err(DetectError::OutputShape("box coordinate overflow".into()));
        }
        candidates.push(Candidate {
            detection: Detection {
                class_idx,
                score,
                rect,
            },
            anchor,
        });
    }
    candidates.sort_by(|a, b| b.detection.score.total_cmp(&a.detection.score));
    let mut kept: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if kept.iter().all(|other| {
            other.detection.class_idx != candidate.detection.class_idx
                || detect_iou(&other.detection.rect, &candidate.detection.rect) <= 0.7
        }) {
            kept.push(candidate);
            if kept.len() == 300 {
                break;
            }
        }
    }
    let mut output = UltralyticsOutput {
        detections: Vec::with_capacity(kept.len()),
        patches: Vec::new(),
    };
    for mut candidate in kept {
        check_cancel(cancel)?;
        candidate.detection.rect = lb.restore(candidate.detection.rect);
        let r = candidate.detection.rect;
        if r[2] <= r[0] || r[3] <= r[1] {
            continue;
        }
        if contour {
            let coefficients: [f32; 32] =
                std::array::from_fn(|i| at(4 + classes + i, candidate.anchor));
            let patch = mask_patch(r, coefficients, &outputs[1], lb, radius, cancel)?;
            if !patch.bits.iter().any(|&inside| inside) {
                continue;
            }
            output.patches.push(patch);
        }
        output.detections.push(candidate.detection);
    }
    Ok(output)
}

fn mask_patch(
    rect: [f32; 4],
    coefficients: [f32; 32],
    proto: &DetectTensor<'_>,
    lb: Letterbox,
    radius: usize,
    cancel: &AtomicBool,
) -> Result<MaskPatch, DetectError> {
    let pw = proto.shape[3] as usize;
    let ph = proto.shape[2] as usize;
    // Crop at the original resolution before dilation. Lower edges use floor,
    // upper edges ceil, matching the stage-0 reference sampling contract.
    let x0 = rect[0].floor() as usize;
    let y0 = rect[1].floor() as usize;
    let x1 = (rect[2].ceil() as usize).min(lb.original[0]);
    let y1 = (rect[3].ceil() as usize).min(lb.original[1]);
    let logits = bounded_logits(coefficients, proto, lb, [x0, y0, x1, y1], cancel)?;
    let x = x0.saturating_sub(radius);
    let y = y0.saturating_sub(radius);
    let end_x = x1.saturating_add(radius).min(lb.original[0]);
    let end_y = y1.saturating_add(radius).min(lb.original[1]);
    let w = end_x - x;
    let h = end_y - y;
    let count = w
        .checked_mul(h)
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or_else(|| DetectError::OutputShape("mask patch dimensions overflow".into()))?;
    let mut bits = vec![false; count];
    for oy in y0..y1 {
        check_cancel(cancel)?;
        let py = proto_coordinate(oy, lb.ratio, lb.top, ph, lb.size);
        let (py0, py1, fy) = sample_axis(py, ph);
        for ox in x0..x1 {
            let px = proto_coordinate(ox, lb.ratio, lb.left, pw, lb.size);
            let (px0, px1, fx) = sample_axis(px, pw);
            bits[(oy - y) * w + ox - x] = bilinear(
                [
                    logits.get(px0, py0) as f64,
                    logits.get(px1, py0) as f64,
                    logits.get(px0, py1) as f64,
                    logits.get(px1, py1) as f64,
                ],
                fx,
                fy,
            ) > 0.0;
        }
    }
    let mut patch = MaskPatch { x, y, w, h, bits };
    dilate_square(&mut patch, radius, cancel)?;
    Ok(patch)
}

fn proto_coordinate(
    pixel: usize,
    ratio: f64,
    pad: usize,
    proto_edge: usize,
    input_edge: usize,
) -> f64 {
    ((pixel as f64 + 0.5) * ratio + pad as f64) * proto_edge as f64 / input_edge as f64 - 0.5
}

struct LogitPatch {
    x: usize,
    y: usize,
    w: usize,
    values: Vec<f32>,
}

impl LogitPatch {
    fn get(&self, x: usize, y: usize) -> f32 {
        self.values[(y - self.y) * self.w + x - self.x]
    }
}

/// Multiply only prototype pixels required by interpolation inside the original
/// box. A small detected object never builds a full prototype-plane logit image.
fn bounded_logits(
    coefficients: [f32; 32],
    proto: &DetectTensor<'_>,
    lb: Letterbox,
    bounds: [usize; 4],
    cancel: &AtomicBool,
) -> Result<LogitPatch, DetectError> {
    if coefficients.iter().any(|v| !v.is_finite()) {
        return Err(DetectError::OutputShape(
            "non-finite mask coefficient".into(),
        ));
    }
    let pw = proto.shape[3] as usize;
    let ph = proto.shape[2] as usize;
    let plane = pw * ph; // All shape products were checked before decoding.
    let x = sample_axis(
        proto_coordinate(bounds[0], lb.ratio, lb.left, pw, lb.size),
        pw,
    )
    .0;
    let y = sample_axis(
        proto_coordinate(bounds[1], lb.ratio, lb.top, ph, lb.size),
        ph,
    )
    .0;
    let end_x = sample_axis(
        proto_coordinate(bounds[2] - 1, lb.ratio, lb.left, pw, lb.size),
        pw,
    )
    .1 + 1;
    let end_y = sample_axis(
        proto_coordinate(bounds[3] - 1, lb.ratio, lb.top, ph, lb.size),
        ph,
    )
    .1 + 1;
    let w = end_x - x;
    let h = end_y - y;
    let mut values = vec![0.0; w * h];
    for (channel, &coefficient) in coefficients.iter().enumerate() {
        check_cancel(cancel)?;
        for row in 0..h {
            let start = channel * plane + (y + row) * pw + x;
            for (logit, &value) in values[row * w..(row + 1) * w]
                .iter_mut()
                .zip(&proto.data[start..start + w])
            {
                *logit += coefficient * value;
            }
        }
    }
    if values.iter().any(|v: &f32| !v.is_finite()) {
        return Err(DetectError::OutputShape("non-finite mask logit".into()));
    }
    Ok(LogitPatch { x, y, w, values })
}

/// Separable sliding windows keep dilation linear in the bounded patch area,
/// even for a large original image and a large margin.
fn dilate_square(
    patch: &mut MaskPatch,
    radius: usize,
    cancel: &AtomicBool,
) -> Result<(), DetectError> {
    if radius == 0 || patch.w == 0 || patch.h == 0 {
        return Ok(());
    }
    let (w, h) = (patch.w, patch.h);
    let mut horizontal = vec![false; patch.bits.len()];
    for y in 0..h {
        check_cancel(cancel)?;
        let row = &patch.bits[y * w..(y + 1) * w];
        let mut count = row[..radius.saturating_add(1).min(w)]
            .iter()
            .filter(|&&b| b)
            .count();
        for x in 0..w {
            horizontal[y * w + x] = count != 0;
            if x >= radius {
                count -= usize::from(row[x - radius]);
            }
            if let Some(enter) = x
                .checked_add(radius)
                .and_then(|v| v.checked_add(1))
                .filter(|&v| v < w)
            {
                count += usize::from(row[enter]);
            }
        }
    }
    for x in 0..w {
        if x % 256 == 0 {
            check_cancel(cancel)?;
        }
        let mut count = (0..radius.saturating_add(1).min(h))
            .filter(|&y| horizontal[y * w + x])
            .count();
        for y in 0..h {
            patch.bits[y * w + x] = count != 0;
            if y >= radius {
                count -= usize::from(horizontal[(y - radius) * w + x]);
            }
            if let Some(enter) = y
                .checked_add(radius)
                .and_then(|v| v.checked_add(1))
                .filter(|&v| v < h)
            {
                count += usize::from(horizontal[enter * w + x]);
            }
        }
    }
    Ok(())
}

struct UltraPerf {
    event: &'static str,
    format: &'static str,
    input_size: usize,
    original: [usize; 2],
    trace_label: &'static str,
    count: usize,
    start: Option<Instant>,
}

impl UltraPerf {
    fn new(
        event: &'static str,
        detector: &DetectorInfo,
        original: [usize; 2],
        trace_label: &'static str,
    ) -> Self {
        Self {
            event,
            format: if detector.format == DetectorFormat::UltralyticsSegment {
                "segment"
            } else {
                "detect"
            },
            input_size: detector.input_size,
            original,
            trace_label,
            count: 0,
            start: crate::perf::is_enabled().then(Instant::now),
        }
    }
}

impl Drop for UltraPerf {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            crate::perf::event(
                "ai",
                self.event,
                None,
                0,
                &[
                    ("trace_label", self.trace_label.into()),
                    ("format", self.format.into()),
                    ("input_size", self.input_size.into()),
                    ("w", self.original[0].into()),
                    ("h", self.original[1].into()),
                    ("count", self.count.into()),
                    ("ms", (start.elapsed().as_secs_f64() * 1000.0).into()),
                ],
            );
        }
    }
}

#[cfg(test)]
mod tests;
