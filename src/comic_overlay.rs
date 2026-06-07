//! mIV 側のテキスト注釈 (comic) オーバーレイ用グルー。
//!
//! レイアウト / テッセレーション / ラスタライズの本体は egui 非依存の `comic-core`
//! に置き、本モジュールはその橋渡しだけを担う:
//!   - comic-core の `RgbaOverlay` (straight-alpha RGBA8) を egui の `ColorImage` の
//!     上に CPU 合成する純関数 (`composite_overlay_over`、単体テスト可能)、
//!   - 注釈描画用のフォントセット読み込み (`load_comic_fonts`)、
//!   - Inc 1 の読み取り専用表示を検証するための固定フィクスチャ (`demo_objects`)。
//!
//! 統合契約は docs/comic-integration-plan.md (Inc 1 / §5.2 合成経路 / §5.4 座標変換 /
//! D1 最前面 / D8 canonical 座標)。
//!
//! ## 合成の座標・解像度 (§5.4)
//!
//! 下地 (`ensure_final_composite_pixels`) は **canonical(非回転)ソース解像度** で
//! 返るので、comic はその解像度に **等倍(S=1)** で焼く → くっきり。回転は paint-time
//! (`draw_rotated_image_ex`) が下地と一緒に掛けるので、本モジュールでは回転を扱わない
//! (D8)。
//!
//! エクスポートの注釈ベイクは **base(canonical ソース)解像度で焼いてから crop+ダウンサンプル**
//! する (`comic_composited_pixels_for_export`)。設計メモ D10 の「ダウンサンプル後に最終解像度で
//! 直焼き」は **採用しない判断** (2026-06-07)。crop/scale/comic 座標が多段で掛かりズレ系の視覚
//! バグが出やすく、最終出力縮小が重視機能でないため複雑さに見合わない。詳細は
//! docs/comic-ui-bugfix-checklist.md の C5 エントリ。

use comic_core::{
    AnnotationObject, FontSet, LoadedFont, Orientation, Rgba, RgbaOverlay, StrokeStyle, TextBlock,
};
use egui::{Color32, ColorImage};

/// フィクスチャ / フォントセットで使うフォントキー。`FontSet::insert` は実キーと
/// 空キーの両方に登録するので `font_key: ""` でも解決するが、明示しておく。
pub const COMIC_FONT_KEY: &str = "ui";

/// 注釈テキスト描画に使う候補フォント (Windows 同梱の日本語 TTC を優先)。
/// 本格的なフォント列挙・選択は Inc 4b で実装する。ここでは 1 本確保できれば十分。
const FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\YuGothR.ttc",
    r"C:\Windows\Fonts\msgothic.ttc",
];

/// 候補から最初に読めた日本語フォントを 1 本だけ登録した `FontSet` を返す。
///
/// 注: フォントファイル (TTC, ~9MB) を同期読みするので、呼び出し側は **一度だけ**
/// (遅延 + キャッシュ) 読むこと。Inc 4b で worker 化 / フォント管理に置き換える。
pub fn load_comic_fonts() -> Option<FontSet> {
    for path in FONT_CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if let Ok(font) = LoadedFont::from_bytes(COMIC_FONT_KEY, bytes) {
            let mut set = FontSet::new();
            set.insert(font);
            return Some(set);
        }
    }
    None
}

/// comic-core の straight-alpha `RgbaOverlay` を `base` の上に src-over 合成し、
/// `base` と同寸の新しい `ColorImage` を返す純関数。
///
/// 合成は straight-alpha sRGBA(u8) 空間で行う(mIV の既存 CPU 合成 = 隠蔽 conceal と
/// 同じ流儀。GPU が別 quad を sRGB で重ねるのと整合)。`base` が不透明(写真)の通常
/// ケースでは前景テキストがそのまま乗り、半透明縁(アンチエイリアス)は素直に馴染む。
///
/// `overlay` は §5.4 の S=1 ベイクで `base` と同寸になる前提だが、万一寸法がずれても
/// 重なり矩形にクリップして安全に動く(防御的)。
pub fn composite_overlay_over(base: &ColorImage, overlay: &RgbaOverlay) -> ColorImage {
    let [w, h] = base.size;
    let mut pixels = base.pixels.clone();
    let cw = w.min(overlay.w);
    let ch = h.min(overlay.h);
    for y in 0..ch {
        for x in 0..cw {
            let oi = (y * overlay.w + x) * 4;
            let oa = overlay.pixels[oi + 3];
            if oa == 0 {
                continue; // 完全透明: 下地そのまま
            }
            let di = y * w + x;
            let [br, bg, bb, ba] = pixels[di].to_srgba_unmultiplied();
            let oaf = oa as f32 / 255.0;
            let baf = ba as f32 / 255.0;
            let out_a = oaf + baf * (1.0 - oaf);
            if out_a <= 0.0 {
                pixels[di] = Color32::TRANSPARENT;
                continue;
            }
            let blend = |fc: u8, bc: u8| -> u8 {
                let v = (fc as f32 * oaf + bc as f32 * baf * (1.0 - oaf)) / out_a;
                v.round().clamp(0.0, 255.0) as u8
            };
            pixels[di] = Color32::from_rgba_unmultiplied(
                blend(overlay.pixels[oi], br),
                blend(overlay.pixels[oi + 1], bg),
                blend(overlay.pixels[oi + 2], bb),
                (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
            );
        }
    }
    ColorImage::new([w, h], pixels)
}

/// Inc 1 検証用の固定注釈シーン。canonical ソース画素座標 `(w, h)` を基準に、
/// 横書き(袋文字)テキストを左上、縦書きテキストを右上に置く。crop / AI / 色補正 /
/// 回転を変えても最前面で正しい位置・解像度・回転になることを実機確認するための物。
/// Inc 2 で永続化(`comic.db`)読み込みに置き換える。
pub fn demo_objects(w: usize, h: usize) -> Vec<AnnotationObject> {
    let wf = w as f32;
    let hf = h as f32;
    let size = (hf * 0.04).clamp(28.0, 120.0);
    let outline_w = (size * 0.12).max(3.0);

    let horizontal = TextBlock {
        text: "テキスト注釈 — Inc 1 デモ\nText annotation overlay".to_string(),
        size_px: size,
        color: Rgba::BLACK,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: outline_w,
        }),
        font_key: COMIC_FONT_KEY.to_string(),
        ..TextBlock::default()
    };
    let vertical = TextBlock {
        text: "縦書き\n注釈".to_string(),
        size_px: size,
        orientation: Orientation::Vertical,
        color: Rgba::new(225, 35, 35, 255),
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: outline_w,
        }),
        font_key: COMIC_FONT_KEY.to_string(),
        ..TextBlock::default()
    };

    vec![
        AnnotationObject::new_text(1, (wf * 0.06, hf * 0.08), horizontal),
        AnnotationObject::new_text(2, (wf * 0.82, hf * 0.08), vertical),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_overlay_replaces_base() {
        // 下地: 不透明黒 2px。overlay: px0=不透明赤 / px1=完全透明。
        let base = ColorImage::new([2, 1], vec![Color32::BLACK, Color32::BLACK]);
        let mut ov = RgbaOverlay::new(2, 1);
        ov.pixels[0..4].copy_from_slice(&[255, 0, 0, 255]); // px0 opaque red
        // px1 は new() で 0,0,0,0 のまま (透明)
        let out = composite_overlay_over(&base, &ov);
        assert_eq!(out.pixels[0].to_srgba_unmultiplied(), [255, 0, 0, 255]);
        assert_eq!(
            out.pixels[1].to_srgba_unmultiplied(),
            [0, 0, 0, 255],
            "透明部は下地のまま"
        );
    }

    #[test]
    fn half_alpha_blends_midway() {
        // 50% 白を不透明黒に重ねる → ~50% グレー・不透明。
        let base = ColorImage::new([1, 1], vec![Color32::BLACK]);
        let mut ov = RgbaOverlay::new(1, 1);
        ov.pixels[0..4].copy_from_slice(&[255, 255, 255, 128]);
        let out = composite_overlay_over(&base, &ov);
        let [r, g, b, a] = out.pixels[0].to_srgba_unmultiplied();
        assert_eq!(a, 255, "不透明下地への合成は不透明のまま");
        for c in [r, g, b] {
            assert!((c as i32 - 128).abs() <= 2, "~50% グレー: {c}");
        }
    }

    #[test]
    fn over_transparent_base_keeps_overlay_alpha() {
        // 完全透明な下地に半透明前景 → 出力 alpha は前景の alpha (out_a = oaf)。
        let base = ColorImage::new([1, 1], vec![Color32::TRANSPARENT]);
        let mut ov = RgbaOverlay::new(1, 1);
        ov.pixels[0..4].copy_from_slice(&[10, 20, 30, 100]);
        let out = composite_overlay_over(&base, &ov);
        let [r, g, b, a] = out.pixels[0].to_srgba_unmultiplied();
        // 低 alpha では egui Color32 の乗算済み内部表現への往復で ±1 LSB の丸め誤差が
        // 出る (30→31 等)。合成自体は正しいので許容差で判定する。
        for (got, want) in [(r, 10), (g, 20), (b, 30)] {
            assert!((got as i32 - want).abs() <= 1, "rgb got={got} want={want}");
        }
        assert!((a as i32 - 100).abs() <= 1, "a={a}");
    }

    #[test]
    fn mismatched_dims_clip_safely() {
        // overlay が下地より小さくても panic せず、下地サイズを保つ。
        let base = ColorImage::new([2, 2], vec![Color32::BLACK; 4]);
        let ov = RgbaOverlay::new(1, 1); // all transparent
        let out = composite_overlay_over(&base, &ov);
        assert_eq!(out.size, [2, 2]);
        assert_eq!(out.pixels.len(), 4);
    }
}
