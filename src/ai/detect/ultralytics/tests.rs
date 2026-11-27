use super::*;

fn params(classes: usize) -> UltralyticsParams {
    UltralyticsParams {
        selected_classes: vec![true; classes],
        thresholds: vec![0.25; classes],
        contour_margin_percent: 0.6,
    }
}

struct Outputs {
    prediction_shape: Vec<i64>,
    prediction: Vec<f32>,
    proto_shape: Vec<i64>,
    proto: Vec<f32>,
}

impl Outputs {
    fn new(classes: usize, anchors: usize, segment: bool, size: usize) -> Self {
        let channels = 4 + classes + if segment { 32 } else { 0 };
        Self {
            prediction_shape: vec![1, channels as i64, anchors as i64],
            prediction: vec![0.0; channels * anchors],
            proto_shape: vec![1, 32, (size / 4) as i64, (size / 4) as i64],
            proto: vec![0.0; if segment { 32 * (size / 4).pow(2) } else { 0 }],
        }
    }

    fn set(&mut self, anchor: usize, channel: usize, value: f32) {
        self.prediction[channel * self.prediction_shape[2] as usize + anchor] = value;
    }

    fn bbox(&mut self, anchor: usize, xywh: [f32; 4]) {
        for (channel, value) in xywh.into_iter().enumerate() {
            self.set(anchor, channel, value);
        }
    }

    fn decode(
        &self,
        segment: bool,
        lb: Letterbox,
        params: &UltralyticsParams,
        contour: bool,
        radius: usize,
    ) -> Result<UltralyticsOutput, DetectError> {
        let mut outputs = vec![DetectTensor {
            shape: &self.prediction_shape,
            data: &self.prediction,
        }];
        if segment {
            outputs.push(DetectTensor {
                shape: &self.proto_shape,
                data: &self.proto,
            });
        }
        decode(
            &outputs,
            if segment {
                DetectorFormat::UltralyticsSegment
            } else {
                DetectorFormat::UltralyticsDetect
            },
            params.thresholds.len(),
            lb,
            params,
            contour,
            radius,
            &AtomicBool::new(false),
        )
    }
}

#[test]
fn detect_ultralytics_letterbox_python_round_padding_and_roundtrip() {
    for (original, size, resized, pad) in [
        ([4, 16], 10, [2, 10], [4, 0]),  // 2.5 -> even 2
        ([12, 16], 10, [8, 10], [1, 0]), // 7.5 -> even 8
        ([4, 6], 8, [5, 8], [1, 0]),     // odd padding splits 1/2
        ([6, 4], 8, [8, 5], [0, 1]),
        ([1913, 641], 640, [640, 214], [0, 213]),
    ] {
        let lb = Letterbox::new(original, size).unwrap();
        assert_eq!(lb.resized, resized);
        assert_eq!([lb.left, lb.top], pad);
    }
    for original in [[1913, 641], [641, 1913], [7, 11], [100, 100]] {
        for size in [32, 640] {
            let lb = Letterbox::new(original, size).unwrap();
            let rect = [
                original[0] as f32 * 0.15,
                original[1] as f32 * 0.2,
                original[0] as f32 * 0.7,
                original[1] as f32 * 0.9,
            ];
            let letterbox = std::array::from_fn(|i| {
                (rect[i] as f64 * lb.ratio + [lb.left, lb.top][i % 2] as f64) as f32
            });
            for (a, b) in lb.restore(letterbox).into_iter().zip(rect) {
                assert!((a - b).abs() < 0.001);
            }
            assert_eq!(
                lb.restore([-100.0, -100.0, size as f32 * 2.0, size as f32 * 2.0]),
                [0.0, 0.0, original[0] as f32, original[1] as f32]
            );
        }
    }
}

#[test]
fn detect_ultralytics_resize_is_two_by_two_halfpixel_in_both_directions() {
    let pixels: Vec<_> = (0..25)
        .map(|n| egui::Color32::from_rgb(n * 10, 255 - n * 10, 77))
        .collect();
    let image = egui::ColorImage::new([5, 5], pixels);
    let input = preprocess(&image, Letterbox::new([5, 5], 2).unwrap()).unwrap();
    // Sampling .75/3.25 uses precisely four neighbours, even when downscaling.
    for (x, y, expected) in [(0, 0, 45.0), (1, 0, 70.0), (0, 1, 170.0), (1, 1, 195.0)] {
        assert_eq!(input[[0, 0, y, x]], expected / 255.0);
        assert_eq!(input[[0, 1, y, x]], (255.0 - expected) / 255.0);
        assert_eq!(input[[0, 2, y, x]], 77.0 / 255.0);
    }
    let image = egui::ColorImage::new(
        [2, 2],
        [0, 100, 200, 240]
            .map(|v| egui::Color32::from_rgb(v, 3, 9))
            .to_vec(),
    );
    let input = preprocess(&image, Letterbox::new([2, 2], 4).unwrap()).unwrap();
    for (x, expected) in [0.0, 25.0, 75.0, 100.0].into_iter().enumerate() {
        assert_eq!(input[[0, 0, 0, x]], expected / 255.0);
    }
    assert_eq!(input[[0, 0, 1, 0]], 50.0 / 255.0);
    assert_eq!(input[[0, 0, 3, 3]], 240.0 / 255.0);
    let image = egui::ColorImage::filled([4, 6], egui::Color32::from_rgb(21, 82, 153));
    let lb = Letterbox::new([4, 6], 8).unwrap();
    let input = preprocess(&image, lb).unwrap();
    assert_eq!(input.shape(), &[1, 3, 8, 8]);
    for c in 0..3 {
        assert_eq!(input[[0, c, 0, 0]], 114.0 / 255.0);
        assert_eq!(input[[0, c, 7, 7]], 114.0 / 255.0);
        assert_eq!(input[[0, c, 0, 1]], [21.0, 82.0, 153.0][c] / 255.0);
    }
}

#[test]
fn detect_ultralytics_selects_max_class_before_selection_and_class_threshold() {
    let lb = Letterbox::new([32, 32], 32).unwrap();
    let mut output = Outputs::new(2, 4, false, 32);
    for i in 0..4 {
        output.bbox(i, [4.0 + i as f32 * 7.0, 8.0, 2.0, 2.0]);
    }
    for (i, scores) in [[0.8, 0.9], [0.5, 0.49], [0.49, 0.48], [0.6, 0.6]]
        .into_iter()
        .enumerate()
    {
        output.set(i, 4, scores[0]);
        output.set(i, 5, scores[1]);
    }
    let params = UltralyticsParams {
        selected_classes: vec![true, false],
        thresholds: vec![0.5, 0.25],
        contour_margin_percent: 0.6,
    };
    let result = output.decode(false, lb, &params, false, 0).unwrap();
    assert_eq!(result.detections.len(), 2);
    assert_eq!(
        result
            .detections
            .iter()
            .map(|d| d.score)
            .collect::<Vec<_>>(),
        [0.6, 0.5]
    );
    assert!(result.detections.iter().all(|d| d.class_idx == 0));
    assert!(result.patches.is_empty());
    let mut params = params;
    params.selected_classes[1] = true;
    params.thresholds[1] = 0.9;
    let result = output.decode(false, lb, &params, false, 0).unwrap();
    assert_eq!(result.detections.len(), 3);
    assert_eq!(result.detections[0].class_idx, 1);
}

#[test]
fn detect_ultralytics_nms_uses_point_seven_per_class_and_caps_globally() {
    let lb = Letterbox::new([64, 64], 64).unwrap();
    let mut outputs = Outputs::new(2, 4, false, 64);
    for (i, center) in [20.0, 21.0, 22.0, 20.0].into_iter().enumerate() {
        outputs.bbox(i, [center, 20.0, 10.0, 10.0]);
        outputs.set(i, if i == 3 { 5 } else { 4 }, 0.9 - i as f32 * 0.05);
    }
    let result = outputs.decode(false, lb, &params(2), false, 0).unwrap();
    // 1px offset: IoU=.818 suppressed; 2px: IoU=.667 retained;
    // identical box of a different class is retained.
    assert_eq!(result.detections.len(), 3);
    assert_eq!(
        result
            .detections
            .iter()
            .map(|d| d.class_idx)
            .collect::<Vec<_>>(),
        [0, 0, 1]
    );
    let mut many = Outputs::new(2, 310, false, 4096);
    for i in 0..310 {
        many.bbox(i, [4.0 + i as f32 * 4.0, 20.0, 2.0, 2.0]);
        many.set(i, 4 + i % 2, 0.9 - i as f32 * 0.001);
    }
    let result = many
        .decode(
            false,
            Letterbox::new([4096, 4096], 4096).unwrap(),
            &params(2),
            false,
            0,
        )
        .unwrap();
    assert_eq!(result.detections.len(), 300);
    assert!(
        result
            .detections
            .windows(2)
            .all(|p| p[0].score >= p[1].score)
    );
    assert!((result.detections[299].score - 0.601).abs() < 0.00001);
}

#[test]
fn detect_ultralytics_nms_precedes_original_coordinate_clipping() {
    let lb = Letterbox::new([32, 16], 32).unwrap();
    let mut outputs = Outputs::new(1, 2, false, 32);
    // The clipped boxes coincide, but their model-space IoU is only 1/3.
    outputs.bbox(0, [10.0, 7.0, 10.0, 6.0]); // y=4..10 -> clipped y=0..2
    outputs.bbox(1, [10.0, 9.0, 10.0, 2.0]); // y=8..10 -> clipped y=0..2
    outputs.set(0, 4, 0.9);
    outputs.set(1, 4, 0.8);
    let result = outputs.decode(false, lb, &params(1), false, 0).unwrap();
    assert_eq!(result.detections.len(), 2);
    assert_eq!(result.detections[0].rect, result.detections[1].rect);
}

#[test]
fn detect_ultralytics_mask_multiplies_all_coefficients_and_interpolates_logits_before_threshold() {
    let mut outputs = Outputs::new(1, 1, true, 8);
    outputs.bbox(0, [4.0, 4.0, 8.0, 8.0]);
    outputs.set(0, 4, 0.9);
    outputs.set(0, 5 + 31, 2.0);
    outputs.proto[31 * 4..32 * 4].copy_from_slice(&[-3.0, 1.0, -3.0, 1.0]);
    let result = outputs
        .decode(
            true,
            Letterbox::new([8, 8], 8).unwrap(),
            &params(1),
            true,
            0,
        )
        .unwrap();
    let patch = &result.patches[0];
    assert_eq!((patch.x, patch.y, patch.w, patch.h), (0, 0, 8, 8));
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(patch.bits[y * 8 + x], x >= 5);
        }
    }
    // Thresholding the prototypes first would put the boundary at x=4.
    assert!(!patch.bits[4]);
    let boxed = outputs
        .decode(
            true,
            Letterbox::new([8, 8], 8).unwrap(),
            &params(1),
            false,
            0,
        )
        .unwrap();
    assert_eq!(boxed.detections, result.detections);
    assert!(boxed.patches.is_empty());
    outputs.proto.fill(0.0);
    let result = outputs
        .decode(
            true,
            Letterbox::new([8, 8], 8).unwrap(),
            &params(1),
            true,
            0,
        )
        .unwrap();
    assert!(result.patches[0].bits.iter().all(|&v| !v)); // strictly >0, never >=0
}

#[test]
fn detect_ultralytics_mask_sampling_uses_original_centers_and_replicated_edges() {
    let mut outputs = Outputs::new(1, 1, true, 8);
    outputs.bbox(0, [4.0, 4.0, 8.0, 8.0]);
    outputs.set(0, 4, 0.9);
    outputs.set(0, 5, 1.0);
    outputs.proto[..4].copy_from_slice(&[-3.0, 1.0, -3.0, 1.0]);
    for original in [[64, 32], [32, 64]] {
        let lb = Letterbox::new(original, 8).unwrap();
        let result = outputs.decode(true, lb, &params(1), true, 0).unwrap();
        let patch = &result.patches[0];
        assert_eq!((patch.w, patch.h), (original[0], original[1]));
        for y in 0..patch.h {
            for x in 0..patch.w {
                let px = ((x as f64 + 0.5) * lb.ratio + lb.left as f64) / 4.0 - 0.5;
                assert_eq!(patch.bits[y * patch.w + x], px > 0.75);
            }
        }
    }
    let (low, high, fraction) = sample_axis(-0.5, 2);
    assert_eq!((low, high, fraction), (0, 1, 0.0));
    assert_eq!(sample_axis(3.0, 2), (1, 1, 0.0));
}

#[test]
fn detect_ultralytics_mask_accepts_nonquarter_prototypes_and_computes_only_box_roi() {
    let lb = Letterbox::new([640, 640], 640).unwrap();
    let shape = [1, 32, 80, 120];
    let mut data = vec![1.0; 32 * 80 * 120];
    // Outside the ROI must not be multiplied or inspected for a small box.
    data[0] = f32::NAN;
    let proto = DetectTensor {
        shape: &shape,
        data: &data,
    };
    let logits = bounded_logits(
        [1.0; 32],
        &proto,
        lb,
        [320, 320, 328, 328],
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(logits.values.len() < 16);
    assert!(logits.values.iter().all(|&v| v == 32.0));
    let patch = mask_patch(
        [320.0, 320.0, 328.0, 328.0],
        [1.0; 32],
        &proto,
        lb,
        2,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!((patch.x, patch.y, patch.w, patch.h), (318, 318, 12, 12));
    assert!(patch.bits.iter().all(|&b| b));
    let mut outputs = Outputs::new(1, 1, true, 640);
    outputs.proto_shape = shape.to_vec();
    outputs.proto = data;
    outputs.bbox(0, [324.0, 324.0, 8.0, 8.0]);
    outputs.set(0, 4, 0.9);
    outputs.set(0, 5, 1.0);
    let result = outputs.decode(true, lb, &params(1), true, 2).unwrap();
    assert_eq!(result.patches[0], patch);
}

#[test]
fn detect_ultralytics_crop_original_box_before_square_dilation_and_clip_patch() {
    let mut outputs = Outputs::new(1, 1, true, 8);
    outputs.set(0, 4, 0.9);
    outputs.set(0, 5, 1.0);
    outputs.proto[..4].fill(1.0);
    let lb = Letterbox::new([64, 32], 8).unwrap();
    // Original box [16,8,24,16], and dilation is in original px.
    outputs.bbox(0, [2.5, 3.5, 1.0, 1.0]);
    let result = outputs.decode(true, lb, &params(1), true, 2).unwrap();
    let patch = &result.patches[0];
    assert_eq!((patch.x, patch.y, patch.w, patch.h), (14, 6, 12, 12));
    assert!(patch.bits.iter().all(|&b| b));
    outputs.bbox(0, [0.0625, 2.0625, 0.125, 0.125]);
    let result = outputs.decode(true, lb, &params(1), true, 2).unwrap();
    let patch = &result.patches[0];
    assert_eq!((patch.x, patch.y, patch.w, patch.h), (0, 0, 3, 3));
    assert!(patch.bits.iter().all(|&b| b));
    assert_eq!(dilation_radius([1000, 500], 0.6), 6);
    assert_eq!(dilation_radius([1000, 500], 0.0), 2);
    assert_eq!(dilation_radius([8000, 6000], 0.5), 40);
    assert_eq!(dilation_radius([1, 1], 0.0), 2);
}

#[test]
fn detect_ultralytics_square_dilation_matches_naive_neighborhood() {
    for (w, h) in [(1, 1), (3, 5), (9, 7)] {
        let bits: Vec<_> = (0..w * h).map(|i| i % 7 == 3 || i % 13 == 0).collect();
        for radius in [0, 1, 2, 4, 20] {
            let mut patch = MaskPatch {
                x: 0,
                y: 0,
                w,
                h,
                bits: bits.clone(),
            };
            dilate_square(&mut patch, radius, &AtomicBool::new(false)).unwrap();
            for y in 0..h {
                for x in 0..w {
                    let expected = (y.saturating_sub(radius)..=y.saturating_add(radius).min(h - 1))
                        .any(|sy| {
                            (x.saturating_sub(radius)..=x.saturating_add(radius).min(w - 1))
                                .any(|sx| bits[sy * w + sx])
                        });
                    assert_eq!(
                        patch.bits[y * w + x],
                        expected,
                        "{w}x{h}, radius {radius}, {x},{y}"
                    );
                }
            }
        }
    }
}

#[test]
fn detect_ultralytics_runtime_shapes_and_nonfinite_data_return_errors() {
    let lb = Letterbox::new([8, 8], 8).unwrap();
    assert!(matches!(
        decode(
            &[],
            DetectorFormat::UltralyticsDetect,
            1,
            lb,
            &params(1),
            false,
            0,
            &AtomicBool::new(false)
        ),
        Err(DetectError::OutputShape(_))
    ));
    for shape in [
        vec![],
        vec![1, 5],
        vec![2, 5, 1],
        vec![1, 1, 5],
        vec![1, 5, -1],
        vec![1, 5, 0],
        vec![1, 5, i64::MAX],
    ] {
        let mut outputs = Outputs::new(1, 1, false, 8);
        outputs.prediction_shape = shape;
        assert!(matches!(
            outputs.decode(false, lb, &params(1), false, 0),
            Err(DetectError::OutputShape(_))
        ));
    }
    let mut outputs = Outputs::new(1, 1, true, 8);
    outputs.bbox(0, [4.0, 4.0, 8.0, 8.0]);
    outputs.set(0, 4, 0.9);
    outputs.set(0, 5, 1.0);
    for shape in [
        vec![],
        vec![1, 32, 2],
        vec![2, 32, 2, 2],
        vec![1, 31, 2, 2],
        vec![1, 32, -2, 2],
        vec![1, 32, 1, 5],
    ] {
        outputs.proto_shape = shape;
        assert!(matches!(
            outputs.decode(true, lb, &params(1), true, 0),
            Err(DetectError::OutputShape(_))
        ));
    }
    outputs.proto_shape = vec![1, 32, 2, 2];
    outputs.proto.pop();
    assert!(matches!(
        outputs.decode(true, lb, &params(1), true, 0),
        Err(DetectError::OutputShape(_))
    ));
    outputs.proto.push(0.0);
    for channel in [0, 4, 5] {
        let previous = outputs.prediction[channel];
        outputs.prediction[channel] = f32::NAN;
        assert!(matches!(
            outputs.decode(true, lb, &params(1), true, 0),
            Err(DetectError::OutputShape(_))
        ));
        outputs.prediction[channel] = previous;
    }
    outputs.proto[0] = f32::INFINITY;
    assert!(matches!(
        outputs.decode(true, lb, &params(1), true, 0),
        Err(DetectError::OutputShape(_))
    ));
    // Box-only operation never reconstructs or reads prototype logits.
    assert!(outputs.decode(true, lb, &params(1), false, 0).is_ok());
}

fn detector_at(path: &std::path::Path) -> DetectorInfo {
    DetectorInfo {
        key: ExternalModelKey::read(path).unwrap(),
        name: "fixture".into(),
        format: DetectorFormat::UltralyticsDetect,
        names: vec!["class 0".into()],
        thresholds: vec![0.25],
        input_size: 32,
        shape: None,
        license: None,
    }
}

#[test]
fn detect_ultralytics_changed_file_rejects_catalog_identity_without_onnx_load() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fixture.onnx");
    std::fs::write(&path, b"fixture").unwrap();
    let detector = detector_at(&path);
    check_detector_file(&detector).unwrap();
    std::fs::write(&path, b"changed-size").unwrap();
    assert!(matches!(
        check_detector_file(&detector),
        Err(DetectError::DetectorChanged)
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        check_detector_file(&detector),
        Err(DetectError::DetectorChanged)
    ));
}

#[test]
fn detect_ultralytics_rejects_invalid_options_and_honors_cancel() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fixture.onnx");
    std::fs::write(&path, b"fixture").unwrap();
    let detector = detector_at(&path);
    validate_params(&detector, &params(1), false).unwrap();
    assert!(validate_params(&detector, &params(1), true).is_err());
    for bad in [f32::NAN, f32::INFINITY, -0.01, 1.01] {
        let mut p = params(1);
        p.thresholds[0] = bad;
        assert!(validate_params(&detector, &p, false).is_err());
    }
    assert!(validate_params(&detector, &params(2), false).is_err());
    for original in [[0, 8], [8, 0]] {
        assert!(Letterbox::new(original, 8).is_err());
    }
    assert!(Letterbox::new([8, 8], usize::MAX).is_err());
    let lb = Letterbox::new([8, 8], 8).unwrap();
    let mut image = egui::ColorImage::filled([8, 8], egui::Color32::BLACK);
    image.pixels.clear();
    assert!(matches!(
        preprocess(&image, lb),
        Err(DetectError::InvalidInput(_))
    ));
    let outputs = Outputs::new(1, 1, false, 8);
    let tensors = [DetectTensor {
        shape: &outputs.prediction_shape,
        data: &outputs.prediction,
    }];
    let result = decode(
        &tensors,
        DetectorFormat::UltralyticsDetect,
        1,
        lb,
        &params(1),
        false,
        0,
        &AtomicBool::new(true),
    );
    assert!(matches!(result, Err(DetectError::Cancelled)));
}

#[derive(serde::Deserialize)]
struct Reference {
    model: std::path::PathBuf,
    image: std::path::PathBuf,
    threshold: f32,
    boxes: Vec<ReferenceBox>,
    #[serde(default)]
    masks: Vec<ReferenceMask>,
}

#[derive(serde::Deserialize)]
struct ReferenceBox {
    class: usize,
    score: f32,
    xyxy: [f32; 4],
}

#[derive(serde::Deserialize)]
struct ReferenceMask {
    index: usize,
    png: std::path::PathBuf,
}

#[test]
#[ignore = "requires MIV_DETECT_REFERENCE external JSON and real ONNX runtime/EP; no fixtures are bundled"]
fn detect_ultralytics_reference_matches_external_models() {
    let Some(reference_path) = std::env::var_os("MIV_DETECT_REFERENCE") else {
        return;
    };
    crate::test_gpu_group::assert_gpu_test_group();
    let _data = crate::data_dir::TestDataDirGuard::new();
    let cases: Vec<Reference> =
        serde_json::from_slice(&std::fs::read(reference_path).unwrap()).unwrap();
    assert!(!cases.is_empty(), "reference contains no cases");
    let owner = crate::ai::runtime::AiRuntimeInitOwner::new();
    owner.start(|| {});
    let runtime = match owner.wait_terminal_while(|| true) {
        crate::ai::runtime::AiRuntimeInitWait::Ready(runtime) => runtime,
        _ => panic!("runtime initialization failed"),
    };
    let mut failures = Vec::new();
    let mut totals = ReferenceTotals::default();
    for (index, case) in cases.iter().enumerate() {
        let label = format!(
            "case {}: {} / {}",
            index + 1,
            case.model.display(),
            case.image.display()
        );
        match compare_reference_case(&runtime, case) {
            Ok(stats) => {
                eprintln!(
                    "{label}: 参照 {}・mIV {}・対応 {}・境目で許した数 {}・IoU 0.95 未満の組 {}・箱 IoU 最小 {}・輪郭 IoU 最小 {}",
                    case.boxes.len(),
                    stats.actual_count,
                    stats.matched,
                    stats.boundary_allowed,
                    stats.below_point95,
                    stats
                        .min_box_iou
                        .map_or_else(|| "n/a".into(), |v| format!("{v:.6}")),
                    stats
                        .min_mask_iou
                        .map_or_else(|| "n/a".into(), |v| format!("{v:.6}")),
                );
                for diagnostic in &stats.diagnostics {
                    eprintln!("{label}: {diagnostic}");
                }
                totals.include(&stats);
                failures.extend(
                    stats
                        .failures
                        .into_iter()
                        .map(|failure| format!("{label}: {failure}")),
                );
            }
            Err(error) => {
                eprintln!(
                    "{label}: 参照 {}・mIV n/a・対応 n/a・境目で許した数 n/a・IoU 0.95 未満の組 n/a・箱 IoU 最小 n/a・輪郭 IoU 最小 n/a",
                    case.boxes.len()
                );
                failures.push(format!("{label}: {error}"));
            }
        }
    }
    eprintln!(
        "全体: 組の数 {}・0.95 以上の割合 {}・箱 IoU 最小 {}・輪郭 IoU 最小 {}",
        totals.matched,
        totals
            .high_iou_fraction()
            .map_or_else(|| "n/a".into(), |v| format!("{:.3}%", v * 100.0)),
        totals
            .min_box_iou
            .map_or_else(|| "n/a".into(), |v| format!("{v:.6}")),
        totals
            .min_mask_iou
            .map_or_else(|| "n/a".into(), |v| format!("{v:.6}")),
    );
    if let Some(failure) = totals.quality_failure() {
        failures.push(failure);
    }
    assert!(
        failures.is_empty(),
        "reference comparison failures:\n{}",
        failures.join("\n")
    );
}

#[derive(Default)]
struct ReferenceComparison {
    actual_count: usize,
    matched: usize,
    boundary_allowed: usize,
    below_point95: usize,
    min_box_iou: Option<f32>,
    min_mask_iou: Option<f64>,
    diagnostics: Vec<String>,
    failures: Vec<String>,
}

#[derive(Default)]
struct ReferenceTotals {
    matched: usize,
    below_point95: usize,
    min_box_iou: Option<f32>,
    min_mask_iou: Option<f64>,
}

impl ReferenceTotals {
    fn include(&mut self, stats: &ReferenceComparison) {
        self.matched += stats.matched;
        self.below_point95 += stats.below_point95;
        if let Some(iou) = stats.min_box_iou {
            self.min_box_iou = Some(self.min_box_iou.map_or(iou, |min| min.min(iou)));
        }
        if let Some(iou) = stats.min_mask_iou {
            self.min_mask_iou = Some(self.min_mask_iou.map_or(iou, |min| min.min(iou)));
        }
    }

    fn high_iou_fraction(&self) -> Option<f64> {
        (self.matched != 0)
            .then(|| (self.matched - self.below_point95) as f64 / self.matched as f64)
    }

    fn quality_failure(&self) -> Option<String> {
        // Integer comparison keeps exactly 95% inclusive and weights every pair,
        // rather than averaging per-case percentages. With no pairs it is vacuous.
        ((self.matched - self.below_point95) as u128 * 100 < self.matched as u128 * 95).then(|| {
            format!(
                "only {}/{} box pairs have IoU >= 0.95; at least 95% required",
                self.matched - self.below_point95,
                self.matched
            )
        })
    }
}

fn load_reference_image(path: &std::path::Path) -> Result<image::DynamicImage, String> {
    if crate::thumb_loader::is_jpeg_ext(path) {
        // Existing product TurboJPEG helper; u32::MAX selects 1/1 DCT scale.
        // It does not apply EXIF orientation. Do not silently fall back to a
        // different JPEG decoder if it fails: that would change the oracle input.
        let (image, stats) =
            crate::thumb_loader::decode_jpeg_turbo_scaled_from_path(path, u32::MAX)
                .map_err(|e| format!("reference JPEG TurboJPEG decode: {e:?}"))?;
        if stats.scale_num != 8 || (image.width(), image.height()) != (stats.src_w, stats.src_h) {
            return Err("reference JPEG must be decoded at full size without orientation".into());
        }
        Ok(image)
    } else {
        image::open(path).map_err(|e| e.to_string())
    }
}

fn compare_reference_case(
    runtime: &AiRuntime,
    case: &Reference,
) -> Result<ReferenceComparison, String> {
    if !case.model.is_absolute() || !case.image.is_absolute() {
        return Err("model and image paths must be absolute".into());
    }
    let detector =
        crate::ai::detector_catalog::read_detector(&case.model).map_err(|e| e.to_string())?;
    // Match the reference's un-oriented TurboJPEG pixels for JPEG inputs.
    let rgba = load_reference_image(&case.image)?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let contour = detector.format == DetectorFormat::UltralyticsSegment;
    let params = UltralyticsParams {
        selected_classes: vec![true; detector.names.len()],
        thresholds: vec![case.threshold; detector.names.len()],
        contour_margin_percent: 0.0,
    };
    // Compare undilated logits to retina_masks. Production margins are covered
    // above; applying even the minimum 2px here obscures tiny-mask parity.
    let output = detect_with_radius(
        runtime,
        &detector,
        &image,
        &params,
        contour,
        0,
        &AtomicBool::new(false),
        "reference",
    )
    .map_err(|e| e.to_string())?;
    let matches = match_reference_boxes(&output.detections, &case.boxes);
    let mut stats =
        compare_reference_boxes(&output.detections, &case.boxes, &matches, case.threshold);
    for reference in &case.masks {
        let Some(actual_index) = matches.get(reference.index) else {
            stats.failures.push(format!(
                "mask index {} is outside reference boxes",
                reference.index
            ));
            continue;
        };
        // An unmatched boundary box has no corresponding mask to compare.
        let Some(actual_index) = *actual_index else {
            continue;
        };
        let result = (|| -> Result<f64, String> {
            if !contour || !reference.png.is_absolute() {
                return Err("reference masks require segmentation and absolute PNG paths".into());
            }
            let mask = image::open(&reference.png)
                .map_err(|e| e.to_string())?
                .to_luma8();
            if [mask.width() as usize, mask.height() as usize] != size {
                return Err("reference mask size differs from original image".into());
            }
            let patch = output
                .patches
                .get(actual_index)
                .ok_or("missing corresponding mask patch")?;
            let mut intersection = 0usize;
            let mut union = 0usize;
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let expected = mask.get_pixel(x as u32, y as u32)[0] != 0;
                    let actual = x >= patch.x
                        && y >= patch.y
                        && x - patch.x < patch.w
                        && y - patch.y < patch.h
                        && patch.bits[(y - patch.y) * patch.w + x - patch.x];
                    intersection += usize::from(expected && actual);
                    union += usize::from(expected || actual);
                }
            }
            Ok(if union == 0 {
                1.0
            } else {
                intersection as f64 / union as f64
            })
        })();
        match result {
            Ok(iou) => record_reference_mask_quality(
                &mut stats,
                reference.index,
                detect_iou(
                    &output.detections[actual_index].rect,
                    &case.boxes[reference.index].xyxy,
                ),
                iou,
            ),
            Err(error) => stats
                .failures
                .push(format!("mask {}: {error}", reference.index)),
        }
    }
    Ok(stats)
}

fn record_reference_mask_quality(
    stats: &mut ReferenceComparison,
    index: usize,
    box_iou: f32,
    mask_iou: f64,
) {
    stats.min_mask_iou = Some(stats.min_mask_iou.map_or(mask_iou, |min| min.min(mask_iou)));
    let required = if box_iou >= 0.95 { 0.9 } else { 0.8 };
    if mask_iou < required {
        stats.failures.push(format!(
            "mask {index} IoU {mask_iou} < {required} (box IoU {box_iou})"
        ));
    }
}

fn compare_reference_boxes(
    actual: &[Detection],
    reference: &[ReferenceBox],
    matches: &[Option<usize>],
    threshold: f32,
) -> ReferenceComparison {
    let mut stats = ReferenceComparison {
        actual_count: actual.len(),
        ..Default::default()
    };
    let mut actual_matched = vec![false; actual.len()];
    for (index, (reference, matched)) in reference.iter().zip(matches).enumerate() {
        if let Some(actual_index) = *matched {
            let actual = &actual[actual_index];
            actual_matched[actual_index] = true;
            stats.matched += 1;
            let iou = detect_iou(&actual.rect, &reference.xyxy);
            stats.min_box_iou = Some(stats.min_box_iou.map_or(iou, |min| min.min(iou)));
            stats.below_point95 += usize::from(iou < 0.95);
            if iou < 0.85 {
                stats.failures.push(format!(
                    "reference box {index} / mIV box {actual_index}: IoU {iou} < 0.85"
                ));
            }
            let delta = (actual.score - reference.score).abs();
            if !delta.is_finite() || delta > 0.05 {
                stats.failures.push(format!("reference box {index} / mIV box {actual_index}: score difference {delta} > 0.05"));
            }
        } else {
            stats.diagnostics.push(unmatched_box_diagnostic(
                "reference",
                index,
                reference.class,
                reference.score,
                reference.xyxy,
                actual
                    .iter()
                    .enumerate()
                    .map(|(index, a)| (index, a.class_idx, a.score, a.rect)),
            ));
            if reference.score.is_finite() && reference.score < threshold + 0.03 {
                stats.boundary_allowed += 1;
            } else {
                stats.failures.push(format!(
                    "unmatched reference box {index}: score {} is not below {}",
                    reference.score,
                    threshold + 0.03
                ));
            }
        }
    }
    for (index, actual) in actual
        .iter()
        .enumerate()
        .filter(|(index, _)| !actual_matched[*index])
    {
        stats.diagnostics.push(unmatched_box_diagnostic(
            "mIV",
            index,
            actual.class_idx,
            actual.score,
            actual.rect,
            reference
                .iter()
                .enumerate()
                .map(|(index, r)| (index, r.class, r.score, r.xyxy)),
        ));
        if actual.score.is_finite() && actual.score < threshold + 0.03 {
            stats.boundary_allowed += 1;
        } else {
            stats.failures.push(format!(
                "unmatched mIV box {index}: score {} is not below {}",
                actual.score,
                threshold + 0.03
            ));
        }
    }
    stats
}

fn unmatched_box_diagnostic(
    side: &str,
    index: usize,
    class: usize,
    score: f32,
    rect: [f32; 4],
    opposite: impl IntoIterator<Item = (usize, usize, f32, [f32; 4])>,
) -> String {
    // Search every opposite box, including other classes and already matched
    // boxes: this diagnoses class changes and geometric mismatches independently.
    let best = opposite
        .into_iter()
        .map(|(index, class, score, other)| (index, class, score, detect_iou(&rect, &other)))
        .max_by(|a, b| a.3.total_cmp(&b.3));
    let own = format!("unmatched {side} box {index}: class={class} score={score:.8} xyxy={rect:?}");
    match best {
        Some((index, class, score, iou)) => format!(
            "{own}; max-IoU opposite box {index}: class={class} score={score:.8} IoU={iou:.6}"
        ),
        None => format!("{own}; opposite boxes: none"),
    }
}

/// Greedily take same-class pairs in descending IoU order, without reassigning
/// an earlier pair to improve cardinality. Original indices break exact ties.
fn match_reference_boxes(actual: &[Detection], reference: &[ReferenceBox]) -> Vec<Option<usize>> {
    let mut candidates = Vec::new();
    for (r, reference) in reference.iter().enumerate() {
        for (a, actual) in actual.iter().enumerate() {
            let iou = detect_iou(&actual.rect, &reference.xyxy);
            if actual.class_idx == reference.class && iou >= 0.5 {
                candidates.push((r, a, iou));
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });
    let mut matches = vec![None; reference.len()];
    let mut actual_matched = vec![false; actual.len()];
    for (r, a, _) in candidates {
        if matches[r].is_none() && !actual_matched[a] {
            matches[r] = Some(a);
            actual_matched[a] = true;
        }
    }
    matches
}

#[test]
fn detect_ultralytics_reference_matching_is_global_iou_greedy_with_half_iou_cutoff() {
    let reference = vec![
        ReferenceBox {
            class: 0,
            score: 0.9,
            xyxy: [3.0, 0.0, 13.0, 10.0],
        },
        ReferenceBox {
            class: 0,
            score: 0.9,
            xyxy: [0.0, 0.0, 10.0, 10.0],
        },
    ];
    let actual = vec![
        Detection {
            class_idx: 0,
            score: 0.9,
            rect: [1.0, 0.0, 11.0, 10.0],
        },
        Detection {
            class_idx: 0,
            score: 0.9,
            rect: [-2.0, 0.0, 8.0, 10.0],
        },
    ];
    // The global maximum is reference 1 / actual 0 (9/11). Greedy keeps it,
    // even though rematching it could create two lower-IoU pairs (both 2/3).
    assert_eq!(
        match_reference_boxes(&actual, &reference),
        vec![None, Some(0)]
    );
    let reference = vec![ReferenceBox {
        class: 0,
        score: 0.9,
        xyxy: [0.0, 0.0, 10.0, 10.0],
    }];
    let mut actual = vec![Detection {
        class_idx: 0,
        score: 0.9,
        rect: [0.0, 0.0, 5.0, 10.0],
    }];
    assert_eq!(match_reference_boxes(&actual, &reference), vec![Some(0)]);
    actual[0].rect[2] = 4.99;
    assert_eq!(match_reference_boxes(&actual, &reference), vec![None]);
    actual[0].rect = reference[0].xyxy;
    actual[0].class_idx = 1;
    assert_eq!(match_reference_boxes(&actual, &reference), vec![None]);
}

#[test]
fn detect_ultralytics_reference_box_quality_enforces_floor_and_pair_weighted_global_ratio() {
    let reference = vec![ReferenceBox {
        class: 0,
        score: 0.9,
        xyxy: [0.0, 0.0, 100.0, 100.0],
    }];
    let compare = |width| {
        let actual = vec![Detection {
            class_idx: 0,
            score: 0.9,
            rect: [0.0, 0.0, width, 100.0],
        }];
        let matched = match_reference_boxes(&actual, &reference);
        compare_reference_boxes(&actual, &reference, &matched, 0.25)
    };
    let at_floor = compare(85.0);
    assert_eq!(at_floor.matched, 1);
    assert_eq!(at_floor.below_point95, 1);
    assert!(at_floor.failures.is_empty());
    assert_eq!(at_floor.min_box_iou, Some(0.85));
    let below_floor = compare(84.9);
    assert_eq!(below_floor.matched, 1);
    assert!(
        below_floor
            .failures
            .iter()
            .any(|failure| failure.contains("< 0.85"))
    );
    let high = compare(95.0);
    assert_eq!(high.below_point95, 0);
    assert!(high.failures.is_empty());
    assert_eq!(compare(94.9).below_point95, 1);

    let mut good_case = ReferenceTotals::default();
    for _ in 0..19 {
        good_case.include(&high);
    }
    // Include a nineteen-pair case and a one-pair case. Averaging their
    // percentages would yield 50%; aggregating pairs yields exactly 95%.
    let good_case = ReferenceComparison {
        matched: good_case.matched,
        below_point95: good_case.below_point95,
        min_box_iou: good_case.min_box_iou,
        ..Default::default()
    };
    let mut total = ReferenceTotals::default();
    assert_eq!(total.high_iou_fraction(), None);
    assert!(total.quality_failure().is_none());
    total.include(&good_case);
    total.include(&at_floor);
    assert_eq!(total.matched, 20);
    assert_eq!(total.high_iou_fraction(), Some(0.95));
    assert_eq!(total.min_box_iou, Some(0.85));
    assert!(total.quality_failure().is_none());
    total.include(&at_floor);
    assert!(total.quality_failure().is_some());
}

#[test]
fn detect_ultralytics_reference_mask_quality_uses_matched_box_iou_threshold() {
    let mut total = ReferenceTotals::default();
    for (box_iou, mask_iou, passes) in [
        (0.95, 0.9, true),
        (0.95, 0.899, false),
        (0.949, 0.8, true),
        (0.949, 0.799, false),
    ] {
        let mut stats = ReferenceComparison::default();
        record_reference_mask_quality(&mut stats, 0, box_iou, mask_iou);
        assert_eq!(
            stats.failures.is_empty(),
            passes,
            "box={box_iou}, mask={mask_iou}"
        );
        assert_eq!(stats.min_mask_iou, Some(mask_iou));
        total.include(&stats);
    }
    assert_eq!(total.min_mask_iou, Some(0.799));
}

#[test]
fn detect_ultralytics_reference_jpeg_uses_full_size_without_exif_orientation() {
    let temp = tempfile::tempdir().unwrap();
    let rgb = image::RgbImage::from_fn(768, 480, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 251) as u8, ((x + y) % 251) as u8])
    });
    let jpeg = turbojpeg::compress_image(&rgb, 90, turbojpeg::Subsamp::Sub2x2).unwrap();
    let mut payload = b"Exif\0\0II".to_vec();
    payload.extend_from_slice(&0x002A_u16.to_le_bytes());
    payload.extend_from_slice(&8_u32.to_le_bytes());
    payload.extend_from_slice(&1_u16.to_le_bytes());
    payload.extend_from_slice(&0x0112_u16.to_le_bytes());
    payload.extend_from_slice(&3_u16.to_le_bytes());
    payload.extend_from_slice(&1_u32.to_le_bytes());
    payload.extend_from_slice(&6_u16.to_le_bytes());
    payload.extend_from_slice(&0_u16.to_le_bytes());
    payload.extend_from_slice(&0_u32.to_le_bytes());
    let jpeg_bytes = jpeg.as_ref();
    let mut bytes = jpeg_bytes[..2].to_vec();
    bytes.extend_from_slice(&[0xFF, 0xE1]);
    bytes.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(&jpeg_bytes[2..]);
    let path = temp.path().join("reference.JPG");
    std::fs::write(&path, &bytes).unwrap();
    let decoded = load_reference_image(&path).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (768, 480));
    // Prove the fixture's tag would rotate the raster if orientation were applied.
    let oriented = crate::thumb_loader::apply_exif_orientation_from_bytes(decoded, &bytes);
    assert_eq!((oriented.width(), oriented.height()), (480, 768));
}

#[test]
fn detect_ultralytics_reference_unmatched_diagnostic_searches_all_opposite_classes() {
    let rect = [0.0, 0.0, 100.0, 50.0];
    let diagnostic = unmatched_box_diagnostic(
        "reference",
        4,
        5,
        0.4,
        rect,
        [
            (0, 5, 0.27, [200.0, 200.0, 300.0, 250.0]),
            (1, 7, 0.41, rect),
        ],
    );
    assert!(diagnostic.contains("unmatched reference box 4: class=5 score="));
    assert!(diagnostic.contains("xyxy=[0.0, 0.0, 100.0, 50.0]"));
    assert!(diagnostic.contains("max-IoU opposite box 1: class=7 score="));
    assert!(diagnostic.contains("IoU=1.000000"));
    let diagnostic = unmatched_box_diagnostic("mIV", 2, 5, 0.4, rect, std::iter::empty());
    assert!(diagnostic.contains("unmatched mIV box 2:"));
    assert!(diagnostic.ends_with("opposite boxes: none"));
}

#[test]
fn detect_ultralytics_reference_allows_only_strict_boundary_unmatched_on_either_side() {
    let threshold = 0.25;
    let mut reference = vec![
        ReferenceBox {
            class: 0,
            score: 0.9,
            xyxy: [0.0, 0.0, 10.0, 10.0],
        },
        ReferenceBox {
            class: 1,
            score: 0.27,
            xyxy: [20.0, 20.0, 30.0, 30.0],
        },
    ];
    let mut actual = vec![
        Detection {
            class_idx: 0,
            score: 0.88,
            rect: [0.0, 0.0, 10.0, 10.0],
        },
        Detection {
            class_idx: 2,
            score: 0.26,
            rect: [40.0, 40.0, 50.0, 50.0],
        },
    ];
    let matched = match_reference_boxes(&actual, &reference);
    assert_eq!(matched, vec![Some(0), None]);
    let stats = compare_reference_boxes(&actual, &reference, &matched, threshold);
    assert!(stats.failures.is_empty(), "{:?}", stats.failures);
    assert_eq!(
        (stats.actual_count, stats.matched, stats.boundary_allowed),
        (2, 1, 2)
    );
    assert_eq!(stats.min_box_iou, Some(1.0));
    assert_eq!(
        stats.diagnostics.len(),
        2,
        "also diagnose allowed boundary boxes"
    );
    assert!(stats.diagnostics[0].contains("unmatched reference box 1:"));
    assert!(stats.diagnostics[1].contains("unmatched mIV box 1:"));
    // Equality is rejected, not included in the tolerance on either side.
    reference[1].score = threshold + 0.03;
    let stats = compare_reference_boxes(&actual, &reference, &matched, threshold);
    assert_eq!(stats.boundary_allowed, 1);
    assert_eq!(stats.failures.len(), 1);
    assert!(stats.failures[0].contains("unmatched reference box 1"));
    reference[1].score = 0.27;
    actual[1].score = threshold + 0.03;
    let stats = compare_reference_boxes(&actual, &reference, &matched, threshold);
    assert_eq!(stats.boundary_allowed, 1);
    assert_eq!(stats.failures.len(), 1);
    assert!(stats.failures[0].contains("unmatched mIV box 1"));
}

#[test]
fn detect_ultralytics_reference_enforces_one_to_one_class_iou_and_score_difference() {
    let make_reference = || ReferenceBox {
        class: 0,
        score: 0.9,
        xyxy: [0.0, 0.0, 10.0, 10.0],
    };
    let mut actual = vec![Detection {
        class_idx: 0,
        score: 0.9,
        rect: [0.0, 0.0, 10.0, 10.0],
    }];
    let reference = vec![make_reference(), make_reference()];
    let matched = match_reference_boxes(&actual, &reference);
    let stats = compare_reference_boxes(&actual, &reference, &matched, 0.25);
    assert_eq!(stats.matched, 1);
    assert_eq!(
        stats.failures.len(),
        1,
        "one prediction must not satisfy both references"
    );
    let reference = vec![make_reference()];
    for wrong_class in [false, true] {
        actual[0].class_idx = usize::from(wrong_class);
        actual[0].rect = if wrong_class {
            [0.0, 0.0, 10.0, 10.0]
        } else {
            [0.0, 0.0, 4.9, 10.0]
        };
        let matched = match_reference_boxes(&actual, &reference);
        let stats = compare_reference_boxes(&actual, &reference, &matched, 0.25);
        assert_eq!(stats.matched, 0);
        assert_eq!(stats.failures.len(), 2);
        assert_eq!(stats.min_box_iou, None);
    }
    actual[0].class_idx = 0;
    actual[0].rect = reference[0].xyxy;
    actual[0].score = 0.84;
    let matched = match_reference_boxes(&actual, &reference);
    let stats = compare_reference_boxes(&actual, &reference, &matched, 0.25);
    assert_eq!(stats.matched, 1);
    assert_eq!(stats.failures.len(), 1);
    assert!(stats.failures[0].contains("score difference"));
    actual[0].score = 0.86;
    assert!(
        compare_reference_boxes(&actual, &reference, &matched, 0.25)
            .failures
            .is_empty()
    );
}
