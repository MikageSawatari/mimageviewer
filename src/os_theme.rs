//! UI テーマを egui に適用するヘルパー (v0.7.0)。
//!
//! ユーザーが選択可能なテーマは 3 択:
//! - `System`: Windows の「アプリ用の色」レジストリを読んで Light / Dark を自動選択
//! - `Light`: メインウィンドウ白基調、フルスクリーン黒
//! - `Dark`: メインウィンドウ暗色、フルスクリーン黒
//!
//! フルスクリーンは `ui_fullscreen.rs` で CentralPanel の fill を `Color32::BLACK`
//! にハードコードしているためテーマ選択に関係なく黒背景になる。

use crate::settings::UiTheme;

/// 実際に egui に適用する解決後のテーマ (Light / Dark のみ)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedTheme {
    Light,
    Dark,
}

/// `UiTheme` から実際に描画に使う Light / Dark を解決する。
/// `System` はレジストリから取得、失敗時は Light。
///
/// 呼び出し側は毎フレーム呼んで前回値と比較することで、`System` 選択中に
/// Windows 側の Light/Dark トグルにも追従できる。レジストリ参照は
/// `RegGetValueW` 一発なのでホットパスで呼んでも十分軽い。
pub fn resolve(theme: UiTheme) -> ResolvedTheme {
    match theme {
        UiTheme::Dark => ResolvedTheme::Dark,
        UiTheme::Light | UiTheme::Standard => ResolvedTheme::Light,
        UiTheme::System => detect_os_preference().unwrap_or(ResolvedTheme::Light),
    }
}

/// 解決済みの Light / Dark を `ctx` に適用する。
pub fn apply_resolved(ctx: &egui::Context, resolved: ResolvedTheme) {
    match resolved {
        ResolvedTheme::Light => ctx.set_visuals(egui::Visuals::light()),
        ResolvedTheme::Dark => ctx.set_visuals(egui::Visuals::dark()),
    }
}

/// 選択されたテーマを `ctx` に適用する (`resolve` + `apply_resolved`)。
pub fn apply(ctx: &egui::Context, theme: UiTheme) {
    apply_resolved(ctx, resolve(theme));
}

/// `UiTheme` を解決した結果が Dark かを返す (System の場合は OS 設定に追従)。
/// B キーの透過背景「反対色」判定などに使う。
pub fn is_dark_effective(theme: UiTheme) -> bool {
    matches!(resolve(theme), ResolvedTheme::Dark)
}

/// Windows の「アプリ用の色」(`HKCU\...\Personalize\AppsUseLightTheme`) を読んで
/// Light / Dark を返す。取得失敗時は `None`。
#[cfg(windows)]
fn detect_os_preference() -> Option<ResolvedTheme> {
    use std::ffi::c_void;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_VALUE_TYPE, RRF_RT_REG_DWORD, RegGetValueW,
    };
    use windows::core::PCWSTR;

    let subkey: Vec<u16> =
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
            .encode_utf16()
            .collect();
    let value: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();

    let mut data: u32 = 0;
    let mut size: u32 = std::mem::size_of::<u32>() as u32;
    let mut type_: REG_VALUE_TYPE = REG_VALUE_TYPE(0);
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            Some(&mut type_),
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    if result.is_ok() {
        Some(if data == 0 { ResolvedTheme::Dark } else { ResolvedTheme::Light })
    } else {
        None
    }
}

#[cfg(not(windows))]
fn detect_os_preference() -> Option<ResolvedTheme> {
    None
}

/// WCAG 2.x 相対輝度計算。値は [0, 1]。
/// sRGB 成分を線形化してから `0.2126R + 0.7152G + 0.0722B` で合成する。
#[cfg(test)]
pub(crate) fn relative_luminance(c: egui::Color32) -> f64 {
    fn srgb_to_linear(v: u8) -> f64 {
        let x = v as f64 / 255.0;
        if x <= 0.03928 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * srgb_to_linear(c.r())
        + 0.7152 * srgb_to_linear(c.g())
        + 0.0722 * srgb_to_linear(c.b())
}

/// WCAG コントラスト比 (>= 1.0)。4.5 以上で通常テキストの AA 合格、
/// 7.0 以上で AAA 合格。
#[cfg(test)]
pub(crate) fn contrast_ratio(a: egui::Color32, b: egui::Color32) -> f64 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (lighter, darker) = if la >= lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    /// 代表的な既知の値で `contrast_ratio` を検証 (WCAG 計算の自己チェック)。
    #[test]
    fn contrast_ratio_known_values() {
        // 完全な白黒は 21:1 が理論最大
        let ratio = contrast_ratio(Color32::WHITE, Color32::BLACK);
        assert!(
            (ratio - 21.0).abs() < 0.01,
            "white/black should be ~21:1, got {ratio:.3}",
        );
        // 同色は 1:1
        let ratio = contrast_ratio(Color32::GRAY, Color32::GRAY);
        assert!(
            (ratio - 1.0).abs() < 1e-9,
            "same color should be 1:1, got {ratio:.3}",
        );
    }

    /// ライトテーマ (`Visuals::light`) の本文テキストと panel 背景が
    /// WCAG AA (>= 4.5:1) を満たす。egui のデフォルト値が崩れたら検知する。
    #[test]
    fn light_theme_text_on_panel_meets_wcag_aa() {
        let v = egui::Visuals::light();
        let text = v.text_color();
        let bg = v.panel_fill;
        let ratio = contrast_ratio(text, bg);
        assert!(
            ratio >= 4.5,
            "Light theme text {:?} on panel {:?} contrast = {:.2} (< 4.5)",
            text, bg, ratio,
        );
    }

    /// ダークテーマの本文テキストと panel 背景が WCAG AA を満たす。
    #[test]
    fn dark_theme_text_on_panel_meets_wcag_aa() {
        let v = egui::Visuals::dark();
        let text = v.text_color();
        let bg = v.panel_fill;
        let ratio = contrast_ratio(text, bg);
        assert!(
            ratio >= 4.5,
            "Dark theme text {:?} on panel {:?} contrast = {:.2} (< 4.5)",
            text, bg, ratio,
        );
    }

    /// フルスクリーン表示は CentralPanel を Color32::BLACK にハードコードしているため、
    /// 白テキスト (ファイル名・カウンタ表示など) とのコントラストは AAA (>= 7.0) を
    /// 満たす。テーマに関係なく黒背景なので白が最適。
    #[test]
    fn fullscreen_overlay_white_on_black_meets_wcag_aaa() {
        let ratio = contrast_ratio(Color32::WHITE, Color32::BLACK);
        assert!(
            ratio >= 7.0,
            "Fullscreen white on black contrast = {ratio:.2} (< 7.0 AAA)",
        );
    }

    // ハイパーリンク色は mimageviewer では使用していないため検証対象外。
    // (egui::Visuals::light のデフォルト #009BFF は WCAG AA を満たさない)
}
