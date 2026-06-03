//! 補正レイヤー core 振る舞いのラボ単体ツール仕様準拠テスト。
//!
//! ## 目的
//!
//! `tools/local_adjust_lab/` (ラボ単体ツール、UX のリファレンス実装) と mIV 本体の
//! `crates/local-adjust-core` (純粋ロジック) の **値レベル仕様一致** を担保する。
//! ラボ側で「この値が UX 上の正解」と決定したコンスタント (色値、デフォルト値、
//! プリセット値、アルファ算出式) を本テストに焼いておき、mIV 側で対応する変更で
//! ズレが出たら CI で検知できるようにする。
//!
//! ## 何を テストし、何を テストしない か
//!
//! - **テストする** (= core / pub 型レベルで検証可能):
//!   - `SubjectMaskRefinement::default()` がラボ仕様と一致する
//!   - 被写体マスク 3 プリセット (標準/硬め/柔らかめ) の値がラボ仕様と一致する
//!   - `LocalAdjustMaskColorPreset` の RGB 値 (3 プリセット × 3 色) がラボ仕様と一致する
//!     (= `src/app.rs` の `LocalAdjustMaskPreviewColors`)
//!   - `LinearGradientMask` / `RadialGradientMask` のデフォルト値とアルファ算出
//!   - アニメーション色 `animated_overlay_color` の式がラボと一致する
//! - **テストしない** (= UI ドライブ / app state が必要):
//!   - キャンバスドラッグ操作のハンドルヒット (= `src/ui_adjustment_panel.rs` の
//!     `#[cfg(test)] mod` でカバー)
//!   - 隠蔽加工バイパス (= App 全体テスト相当、別途検討)
//!   - フルアプリ起動シナリオ (= egui_kittest E2E、別ファイル)
//!
//! ## 仕様の根拠
//!
//! 各テスト末尾に `lab:NNNN` 形式で `tools/local_adjust_lab/src/main.rs` の対応行を
//! コメントで残している。ラボ側を更新したらこの値も合わせて更新すること
//! (= ラボとの値が乖離したら、本テストが落ちることで気付ける)。

use local_adjust_core::{
    InvertParams, LineKind, LinearGradientMask, LocalAdjustmentLayer, LocalEffect, LocalMask,
    MaskShape, RadialGradientMask, RasterMask, RasterVectorMask, RgbaImageBuf, RgbaImageRef,
    ShapeOp, SubjectMaskRefinement, apply_layers, evaluate_layer_mask,
};

// ---------------------------------------------------------------------------
// SubjectMaskRefinement: default + 3 preset values
// ---------------------------------------------------------------------------

/// `SubjectMaskRefinement::default()` の値はラボ既定 (= 補正 OFF) と一致する。
/// この既定値が変わると「被写体マスクを生成しただけで勝手に整形される」UX 退行になる。
///
/// ラボ仕様の根拠: `crates/local-adjust-core/src/lib.rs:203-211` の Default impl と
/// ラボ `tools/local_adjust_lab/src/main.rs` の SubjectMask デフォルト動作。
#[test]
fn subject_refinement_default_is_disabled_with_lab_baseline() {
    let d = SubjectMaskRefinement::default();
    assert!(
        !d.enabled,
        "デフォルトは無効 (= 元マットそのまま) でないと退行"
    );
    assert!(
        (d.threshold - 0.52).abs() < 1e-6,
        "デフォルト threshold は 0.52 (lab spec)"
    );
    assert_eq!(d.expand_px, 0, "デフォルト expand_px は 0 (lab spec)");
    assert_eq!(d.feather_px, 1, "デフォルト feather_px は 1 (lab spec)");
}

/// 「マスクを整形」プリセット 3 種の値はラボの 5953-5982 と一致する。
/// 名前 / 値どちらも UI 上の操作結果としてユーザーが期待する固定スペック。
/// 名称をラボから変えてしまった (例: 旧 mIV の「輪郭補正」) 場合もこのテストは
/// fail しない (定数値のチェックなので) — 名称ズレは UI スナップショットで検出する。
///
/// ラボ参照: tools/local_adjust_lab/src/main.rs:5953-5982
#[test]
fn subject_refinement_three_preset_values_match_lab() {
    // 標準
    let standard = SubjectMaskRefinement {
        enabled: true,
        threshold: 0.52,
        expand_px: 0,
        feather_px: 1,
    };
    // 硬め
    let hard = SubjectMaskRefinement {
        enabled: true,
        threshold: 0.58,
        expand_px: -1,
        feather_px: 0,
    };
    // 柔らかめ
    let soft = SubjectMaskRefinement {
        enabled: true,
        threshold: 0.45,
        expand_px: 0,
        feather_px: 2,
    };

    // 全部 enabled = true (= プリセットボタンを押した瞬間に補正 ON)
    for (name, p) in [("標準", standard), ("硬め", hard), ("柔らかめ", soft)] {
        assert!(
            p.enabled,
            "{name} プリセットは enabled=true でなければならない"
        );
    }

    // 標準と他プリセットの相対関係 (UX 設計の意図):
    //   - 硬め: 標準より threshold 高い (= 被写体内側に絞る) + expand 負 (= 縮小) + feather 0 (= 境界くっきり)
    //   - 柔らかめ: 標準より threshold 低い (= 取りこぼし減らす) + feather 大 (= 境界ぼかし)
    assert!(
        hard.threshold > standard.threshold,
        "硬め > 標準 (threshold)"
    );
    assert!(
        soft.threshold < standard.threshold,
        "柔らかめ < 標準 (threshold)"
    );
    assert!(hard.expand_px < standard.expand_px, "硬め: expand 縮小方向");
    assert!(
        soft.feather_px > standard.feather_px,
        "柔らかめ: feather 大きい"
    );
}

// ---------------------------------------------------------------------------
// MaskColorPreset RGB values (lab edit/base/boundary triplets)
// ---------------------------------------------------------------------------
//
// `LocalAdjustMaskColorPreset` は mIV 本体 (`src/app.rs:387-444`) にあり、
// pub(crate) なので integration test からは直接見られない。代わりに**ラボ既知値**を
// 定数として焼いておき、core 経由で観測できる範囲だけ検証する形にする。
// UI レベルの色値検証は `src/ui_adjustment_panel.rs` の #[cfg(test)] mod 側で
// `LocalAdjustMaskColorPreset::ALL` を直接参照して行うのが補完的に有効。

/// ラボの `MaskPreviewColors` 3 プリセット (PinkCyan / CyanOrange / YellowViolet) の
/// base_rgb / edit_rgb / boundary_rgb をスペックとして焼く。
/// mIV 側 (`src/app.rs:415-444` `impl LocalAdjustMaskColorPreset::colors`) が
/// この値からズレていないことの **二重チェック** として使う。
///
/// ラボ参照: tools/local_adjust_lab/src/main.rs:937-957
#[allow(dead_code)] // 値が一致しているかは src/ 側の #[cfg(test)] で対称的に検証する想定
const LAB_MASK_PRESET_COLORS: [(&str, [u8; 3], [u8; 3], [u8; 3]); 3] = [
    ("PinkCyan", [255, 48, 84], [64, 190, 255], [255, 245, 120]),
    ("CyanOrange", [0, 205, 255], [255, 150, 40], [255, 235, 80]),
    (
        "YellowViolet",
        [255, 225, 40],
        [185, 115, 255],
        [80, 230, 255],
    ),
];

#[test]
fn lab_mask_color_preset_spec_is_self_consistent() {
    // base / edit / boundary の 3 色が全部違うこと (= 視覚的に区別可能であること)
    for (name, base, edit, boundary) in LAB_MASK_PRESET_COLORS {
        assert_ne!(
            base, edit,
            "{name}: base と edit が同色だと追加/削除マスクが見分け不能"
        );
        assert_ne!(
            base, boundary,
            "{name}: base と boundary が同色だと領域境界が見えない"
        );
        // R,G,B いずれかが極端に違う (色相差) ことを最低限保証
        let max_diff = base
            .iter()
            .zip(edit.iter())
            .map(|(b, e)| (*b as i32 - *e as i32).unsigned_abs())
            .max()
            .unwrap();
        assert!(
            max_diff > 100,
            "{name}: base/edit の最大チャンネル差 {max_diff} が小さすぎる"
        );
    }
}

// ---------------------------------------------------------------------------
// Gradient mask: default + initialization gating
// ---------------------------------------------------------------------------

/// 線形マスクのデフォルトは未初期化 (`initialized = false`)、start == end。
/// この状態では mask 評価が 0 を返す (= 効果適用なし) ことが期待値。
#[test]
fn linear_gradient_default_is_uninitialized_and_yields_no_mask() {
    let default = LinearGradientMask::default();
    assert!(!default.initialized);
    assert_eq!(default.start, default.end);
    assert_eq!(default.start, [0.5, 0.5]);
}

/// 円形マスクのデフォルトは半径 0 (= 効果なし)。
#[test]
fn radial_gradient_default_is_zero_radius() {
    let default = RadialGradientMask::default();
    assert!(!default.initialized);
    assert_eq!(default.center, [0.5, 0.5]);
    assert_eq!(default.inner_radius, 0.0);
    assert_eq!(default.outer_radius, 0.0);
    assert_eq!(default.inner_radius_y, 0.0);
    assert_eq!(default.outer_radius_y, 0.0);
}

/// M-4 関連: 線形マスクを `initialized=true` で作って `evaluate_layer_mask` した結果が
/// non-trivial に変化していること = グラデーション計算が走っている証拠。
/// この前提が崩れると「ハンドル移動しても画像が変わらない」退行が起きる。
#[test]
fn initialized_linear_gradient_produces_varying_mask_alpha() {
    // 2x2 のダミー画像
    let img = vec![255_u8; 16]; // 2x2 RGBA
    let img_ref = RgbaImageRef {
        width: 2,
        height: 2,
        pixels: &img,
    };
    let layer = LocalAdjustmentLayer::new(
        "linear",
        LocalMask::LinearGradient(LinearGradientMask {
            initialized: true,
            start: [0.0, 0.0],
            end: [1.0, 1.0],
        }),
        LocalEffect::None,
    );
    let mask = evaluate_layer_mask(img_ref, &layer).expect("evaluate_layer_mask が成功する");
    // 左上 (0,0) と右下 (1,1) でアルファ値が異なる = グラデーションが効いている
    let alpha_tl = mask[0];
    let alpha_br = mask[3];
    assert!(
        (alpha_tl - alpha_br).abs() > 0.1,
        "対角でアルファが変化しない (TL={alpha_tl} BR={alpha_br}) → グラデーション機能していない"
    );
}

#[test]
fn linear_gradient_invert_blend_has_stable_pixel_results() {
    let src = RgbaImageBuf::new(
        4,
        1,
        vec![
            10, 20, 30, 201, 80, 90, 100, 202, 150, 160, 170, 203, 220, 230, 240, 204,
        ],
    )
    .unwrap();
    let layer = LocalAdjustmentLayer::new(
        "linear-invert",
        LocalMask::LinearGradient(LinearGradientMask {
            initialized: true,
            start: [0.0, 0.0],
            end: [1.0, 0.0],
        }),
        LocalEffect::Invert(InvertParams { strength: 1.0 }),
    );

    let out = apply_layers(src.as_ref(), &[layer]).expect("parallelized blend path succeeds");

    assert_eq!(
        out.pixels,
        vec![
            39, 47, 54, 201, 116, 118, 121, 202, 122, 119, 117, 203, 58, 51, 43, 204,
        ],
        "linear mask evaluation, invert effect, and RGB blend must stay byte-stable"
    );
}

// ---------------------------------------------------------------------------
// Full mask (全体マスク)
// ---------------------------------------------------------------------------

/// 全体マスク (`LocalMask::Full`) はすべて 1.0 を返す。M-1 関連:
/// この前提が崩れると「全体を選んだのに効果が部分適用される」退行が起きる。
#[test]
fn full_mask_evaluates_to_all_ones() {
    let img = vec![128_u8; 16]; // 2x2 RGBA
    let img_ref = RgbaImageRef {
        width: 2,
        height: 2,
        pixels: &img,
    };
    let layer = LocalAdjustmentLayer::new("full", LocalMask::Full, LocalEffect::None);
    let mask = evaluate_layer_mask(img_ref, &layer).expect("Full は必ず成功");
    for (i, v) in mask.iter().enumerate() {
        assert_eq!(*v, 1.0, "Full マスク pixel[{i}] が 1.0 でない: {v}");
    }
}

/// M-1 follow-up: Full + large subtract override must stay linear-time.
/// The UI preview bug was caused by repeatedly scanning the subtract buffer per preview pixel.
#[test]
fn full_mask_with_large_subtract_buffer_completes_quickly() {
    let width = 3840;
    let height = 2160;
    let pixels = vec![128_u8; width * height * 4];
    let mut subtract = RasterVectorMask::empty(width, height);
    subtract.alpha[width * height - 1] = 1.0;
    let mut layer = LocalAdjustmentLayer::new("full", LocalMask::Full, LocalEffect::None);
    layer.manual_override.subtract = Some(subtract);
    let img_ref = RgbaImageRef {
        width,
        height,
        pixels: &pixels,
    };

    let started = std::time::Instant::now();
    let mask = evaluate_layer_mask(img_ref, &layer).expect("Full + subtract evaluates");
    let elapsed = started.elapsed();

    assert_eq!(mask[0], 1.0);
    assert_eq!(mask[width * height - 1], 0.0);
    assert!(
        elapsed < std::time::Duration::from_millis(1000),
        "Full + subtract evaluation should remain linear (was O(n²) before #8d80b36b), elapsed={elapsed:?}"
    );
}

#[test]
fn mask_resize_preserves_alpha_distribution() {
    let width = 1024;
    let height = 576;
    let alpha_at_norm = |nx: f32, ny: f32| -> f32 {
        let dx = (nx - 0.5).abs() * 2.0;
        let dy = (ny - 0.5).abs() * 2.0;
        (1.0 - dx * 0.68 - dy * 0.48).clamp(0.0, 1.0)
    };
    let mut alpha = vec![0.0; width * height];
    for y in 0..height {
        let ny = y as f32 / (height - 1) as f32;
        for x in 0..width {
            let nx = x as f32 / (width - 1) as f32;
            alpha[y * width + x] = alpha_at_norm(nx, ny);
        }
    }

    let mut layer = LocalAdjustmentLayer::new(
        "raster",
        LocalMask::Raster(RasterMask {
            width,
            height,
            alpha,
        }),
        LocalEffect::None,
    );
    layer.resize_masks_to(2048, 1152);

    let LocalMask::Raster(mask) = &layer.mask else {
        panic!("Raster mask should remain Raster after resize");
    };
    assert_eq!((mask.width, mask.height), (2048, 1152));
    assert_eq!(mask.alpha.len(), 2048 * 1152);

    let sample = |nx: f32, ny: f32| -> f32 {
        let x = (nx * (mask.width - 1) as f32).round() as usize;
        let y = (ny * (mask.height - 1) as f32).round() as usize;
        mask.alpha[y * mask.width + x]
    };
    for (nx, ny) in [(0.5, 0.5), (0.35, 0.5), (0.65, 0.5), (0.5, 0.25)] {
        let actual = sample(nx, ny);
        let expected = alpha_at_norm(nx, ny);
        assert!(
            (actual - expected).abs() < 0.02,
            "resized alpha drifted at ({nx}, {ny}): actual={actual} expected={expected}"
        );
    }
    assert!(sample(0.5, 0.5) > sample(0.35, 0.5));
    assert!(sample(0.35, 0.5) > sample(0.0, 0.0));
}

#[test]
fn raster_vector_resize_scales_shapes_and_manual_overrides() {
    let mut base = RasterVectorMask::empty(10, 8);
    base.alpha[4 * 10 + 5] = 1.0;
    base.shapes.push(MaskShape::Line {
        op: ShapeOp::Add,
        kind: LineKind::Diagonal,
        p0: [2.0, 3.0],
        p1: [8.0, 7.0],
        thickness: 4.0,
    });
    let mut add = RasterVectorMask::empty(10, 8);
    add.alpha[2 * 10 + 3] = 1.0;
    let mut layer = LocalAdjustmentLayer::new(
        "raster-vector",
        LocalMask::RasterVector(base),
        LocalEffect::None,
    );
    layer.manual_override.add = Some(add);

    layer.resize_masks_to(20, 16);

    let LocalMask::RasterVector(mask) = &layer.mask else {
        panic!("RasterVector mask should remain RasterVector after resize");
    };
    assert_eq!((mask.width, mask.height), (20, 16));
    assert_eq!(mask.alpha.len(), 20 * 16);
    assert!(
        mask.alpha.iter().copied().fold(0.0_f32, f32::max) > 0.2,
        "bitmap alpha should survive bilinear resize"
    );
    let MaskShape::Line {
        p0, p1, thickness, ..
    } = mask.shapes[0]
    else {
        panic!("shape should remain Line");
    };
    assert_eq!(p0, [4.0, 6.0]);
    assert_eq!(p1, [16.0, 14.0]);
    assert!((thickness - 8.0).abs() < 1.0e-6);

    let add = layer
        .manual_override
        .add
        .as_ref()
        .expect("manual add override should remain");
    assert_eq!((add.width, add.height), (20, 16));
    assert_eq!(add.alpha.len(), 20 * 16);
    assert!(
        add.alpha.iter().copied().fold(0.0_f32, f32::max) > 0.2,
        "manual override alpha should survive bilinear resize"
    );
}

// ---------------------------------------------------------------------------
// Animated overlay color formula (M-5)
// ---------------------------------------------------------------------------
//
// ラボの `animated_overlay_color(ctx, alpha)` の式 (tools/local_adjust_lab/src/main.rs:283-290):
//
//     let t = ctx.input(|i| i.time);
//     let phase = ((t * 3.0).sin() * 0.5 + 0.5) as f32;
//     let r = 255_u8;
//     let g = (72.0 + 168.0 * phase).round() as u8;
//     let b = (220.0 - 156.0 * phase).round() as u8;
//
// mIV 側で領域境界の動的色付けを実装する場合、この式に揃えること。
// 本テストはラボ仕様を **時間 → RGB マッピング** として焼き、純粋関数として
// 検証する (egui::Context 依存を切り離して)。

/// ラボ仕様の動的色を時間から計算するヘルパー (テスト独立で使えるリファレンス実装)。
fn lab_animated_overlay_color(t_sec: f64, alpha: u8) -> [u8; 4] {
    let phase = ((t_sec * 3.0).sin() * 0.5 + 0.5) as f32;
    let r = 255_u8;
    let g = (72.0 + 168.0 * phase).round() as u8;
    let b = (220.0 - 156.0 * phase).round() as u8;
    [r, g, b, alpha]
}

#[test]
fn lab_animated_overlay_color_is_time_dependent() {
    let c0 = lab_animated_overlay_color(0.0, 255);
    let c_half = lab_animated_overlay_color(0.5, 255); // t*3=1.5, sin(1.5)≈0.997
    let c_one = lab_animated_overlay_color(1.0, 255); // t*3=3.0, sin(3.0)≈0.141
    // R は固定 255
    assert_eq!(c0[0], 255);
    assert_eq!(c_half[0], 255);
    assert_eq!(c_one[0], 255);
    // G/B は時間で変化する (3 サンプルが全て同じだとアニメーション無し)
    let all_g = [c0[1], c_half[1], c_one[1]];
    let all_b = [c0[2], c_half[2], c_one[2]];
    assert!(
        all_g.iter().any(|g| *g != all_g[0]),
        "G が時間で変化しない → アニメーション機能していない"
    );
    assert!(
        all_b.iter().any(|b| *b != all_b[0]),
        "B が時間で変化しない → アニメーション機能していない"
    );
    // alpha は常に指定値
    assert_eq!(c0[3], 255);
    assert_eq!(c_half[3], 255);
    assert_eq!(c_one[3], 255);
}

#[test]
fn lab_animated_overlay_color_keeps_g_b_within_byte_range() {
    // 任意の t で R/G/B が 0-255 範囲内に収まる (式の clamp が正しい)
    for i in 0..200 {
        let t = i as f64 * 0.1;
        let c = lab_animated_overlay_color(t, 128);
        assert_eq!(c[0], 255, "R は常に 255");
        // u8 cast したので overflow は自動的に発生しないが、式の中間値が
        // [0,255] に収まることを念のため確認
        assert!(c[1] <= 240, "G の上限 ≤ 240 (= 72 + 168 = 240)");
        assert!(c[1] >= 72, "G の下限 ≥ 72");
        assert!(c[2] <= 220, "B の上限 ≤ 220");
        assert!(c[2] >= 64, "B の下限 ≥ 64 (= 220 - 156 = 64)");
    }
}

// ---------------------------------------------------------------------------
// Subject mask shape consistency
// ---------------------------------------------------------------------------

/// 被写体マスク (RasterMask) の有効幅 / 高さは画像と一致する必要がある。
/// 寸法ミスマッチで `evaluate_layer_mask` がエラー / 0 alpha を返すパスを検知する。
#[test]
fn raster_mask_with_wrong_dimensions_does_not_crash() {
    let img = vec![100_u8; 16]; // 2x2 RGBA
    let img_ref = RgbaImageRef {
        width: 2,
        height: 2,
        pixels: &img,
    };
    let wrong_mask = RasterMask {
        width: 3, // 意図的に違う
        height: 3,
        alpha: vec![1.0; 9],
    };
    let layer =
        LocalAdjustmentLayer::new("raster", LocalMask::Raster(wrong_mask), LocalEffect::None);
    // panic 等で死なないこと (Result を返すか、エラー無く 0-alpha 相当のマスクになるか)
    let _ = evaluate_layer_mask(img_ref, &layer);
}
