//! UI テーマを egui に適用するヘルパー (v0.7.0)。
//!
//! ユーザーが選択可能なテーマは 3 択:
//! - `System`: Windows の「アプリ用の色」レジストリを読んで Light / Dark を自動選択
//! - `Light`: メインウィンドウ白基調、フルスクリーン黒
//! - `Dark`: メインウィンドウ暗色、フルスクリーン黒
//!
//! フルスクリーンは `ui_fullscreen.rs` で CentralPanel の fill を `Color32::BLACK`
//! にハードコードしているためテーマ選択に関係なく黒背景になる。

use crate::settings::{TextContrast, UiTheme};
use egui::style::StyleModifier;

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
///
/// egui 0.33 の `set_visuals` は現在解決されているテーマに対してだけ Style.visuals を
/// 書き換えるため、`theme_preference = System` (egui のデフォルト) の状態だと
/// 起動時に `system_theme` が未取得で `fallback_theme = Dark` 側にしか反映されず、
/// 直後に egui-winit が Windows の Light を拾うと未変更の Light Style で描画されてしまう。
/// `set_theme` で `theme_preference` 自体を上書きすれば system_theme に関係なく
/// 常に目的のテーマに解決されるため、そちらを使う。
pub fn apply_resolved(ctx: &egui::Context, resolved: ResolvedTheme) {
    apply_resolved_with_contrast(ctx, resolved, TextContrast::Standard);
}

/// 解決済みテーマと文字コントラストを、メイン Context が持つ Light / Dark の
/// 両 Style へ適用する。
///
/// 選択中テーマだけを書き換えると、System の切替や暗色固定ダイアログの一時表示後に
/// もう一方の古い Style が露出する。常に両方を同じ所有境界で更新する。
pub fn apply_resolved_with_contrast(
    ctx: &egui::Context,
    resolved: ResolvedTheme,
    contrast: TextContrast,
) {
    let contrast = contrast.normalized();
    ctx.set_visuals_of(
        egui::Theme::Light,
        app_visuals(ResolvedTheme::Light, contrast),
    );
    ctx.set_visuals_of(
        egui::Theme::Dark,
        app_visuals(ResolvedTheme::Dark, contrast),
    );
    let preference = match resolved {
        ResolvedTheme::Light => egui::ThemePreference::Light,
        ResolvedTheme::Dark => egui::ThemePreference::Dark,
    };
    ctx.set_theme(preference);
    ctx.data_mut(|data| data.insert_temp(text_contrast_id(), contrast));
    apply_scrollbar_visibility_style(ctx);
}

/// 選択されたテーマを `ctx` に適用する (`resolve` + `apply_resolved`)。
pub fn apply(ctx: &egui::Context, theme: UiTheme) {
    apply_resolved(ctx, resolve(theme));
}

fn text_contrast_id() -> egui::Id {
    egui::Id::new("miv_text_contrast")
}

/// Context に最後に適用した文字コントラストを返す。独立したテスト Context や
/// native overlay Context では未登録になり得るため、その場合は標準へ戻す。
pub(crate) fn current_text_contrast(ctx: &egui::Context) -> TextContrast {
    ctx.data(|data| data.get_temp(text_contrast_id()))
        .unwrap_or_default()
}

/// mIV の意味別文字色を反映した egui Visuals。
///
/// ラベルだけでなく widget の全 interaction state を同時に更新するため、ツールバー、
/// メニュー、ComboBox、Window の通常描画は個別色指定なしで同じ配色になる。
pub(crate) fn app_visuals(resolved: ResolvedTheme, contrast: TextContrast) -> egui::Visuals {
    let contrast = contrast.normalized();
    let strong = contrast == TextContrast::Strong;
    let mut visuals = match resolved {
        ResolvedTheme::Light => egui::Visuals::light(),
        ResolvedTheme::Dark => egui::Visuals::dark(),
    };

    let (normal, secondary, interactive, hover, active, open, disabled_alpha) =
        match (resolved, strong) {
            (ResolvedTheme::Light, false) => (
                egui::Color32::from_gray(55),
                egui::Color32::from_gray(100),
                egui::Color32::from_gray(45),
                egui::Color32::from_gray(15),
                egui::Color32::BLACK,
                egui::Color32::from_gray(20),
                0.60,
            ),
            (ResolvedTheme::Light, true) => (
                egui::Color32::from_gray(20),
                egui::Color32::from_gray(70),
                egui::Color32::from_gray(16),
                egui::Color32::BLACK,
                egui::Color32::BLACK,
                egui::Color32::BLACK,
                0.72,
            ),
            (ResolvedTheme::Dark, false) => (
                egui::Color32::from_gray(205),
                egui::Color32::from_gray(160),
                egui::Color32::from_gray(215),
                egui::Color32::from_gray(245),
                egui::Color32::WHITE,
                egui::Color32::from_gray(232),
                0.62,
            ),
            (ResolvedTheme::Dark, true) => (
                egui::Color32::from_gray(242),
                egui::Color32::from_gray(205),
                egui::Color32::from_gray(245),
                egui::Color32::WHITE,
                egui::Color32::WHITE,
                egui::Color32::WHITE,
                0.74,
            ),
        };

    visuals.override_text_color = None;
    visuals.weak_text_color = Some(secondary);
    visuals.widgets.noninteractive.fg_stroke.color = normal;
    visuals.widgets.inactive.fg_stroke.color = interactive;
    visuals.widgets.hovered.fg_stroke.color = hover;
    visuals.widgets.active.fg_stroke.color = active;
    visuals.widgets.open.fg_stroke.color = open;
    visuals.disabled_alpha = disabled_alpha;

    match resolved {
        ResolvedTheme::Light => {
            visuals.warn_fg_color = match strong {
                false => egui::Color32::from_rgb(120, 78, 0),
                true => egui::Color32::from_rgb(92, 58, 0),
            };
            visuals.error_fg_color = match strong {
                false => egui::Color32::from_rgb(176, 42, 36),
                true => egui::Color32::from_rgb(135, 24, 20),
            };
            visuals.hyperlink_color = match strong {
                false => egui::Color32::from_rgb(25, 88, 140),
                true => egui::Color32::from_rgb(0, 65, 118),
            };
        }
        ResolvedTheme::Dark => {
            visuals.warn_fg_color = match strong {
                false => egui::Color32::from_rgb(238, 193, 94),
                true => egui::Color32::from_rgb(255, 213, 120),
            };
            visuals.error_fg_color = match strong {
                false => egui::Color32::from_rgb(245, 145, 138),
                true => egui::Color32::from_rgb(255, 178, 172),
            };
            visuals.hyperlink_color = match strong {
                false => egui::Color32::from_rgb(145, 196, 245),
                true => egui::Color32::from_rgb(178, 218, 255),
            };
        }
    }
    visuals
}

/// フルスクリーン、編集パネル、native HUD が共有する暗色 Visuals。
pub(crate) fn dark_visuals(contrast: TextContrast) -> egui::Visuals {
    app_visuals(ResolvedTheme::Dark, contrast)
}

/// 暗色固定の子 UI に、現在の文字コントラストを保った Visuals を適用する。
pub(crate) fn apply_dark_ui(ui: &mut egui::Ui) {
    let contrast = current_text_contrast(ui.ctx());
    *ui.visuals_mut() = dark_visuals(contrast);
}

/// ComboBox / Popup の子にだけ暗色 Style を適用する。
/// Context のテーマを変更しないため、ポップアップを開閉してもメインテーマへ漏れない。
pub(crate) fn dark_popup_style(ctx: &egui::Context) -> StyleModifier {
    let visuals = dark_visuals(current_text_contrast(ctx));
    StyleModifier::new(move |style| style.visuals = visuals.clone())
}

/// 暗色固定 popup / tooltip の背景枠。
pub(crate) fn dark_popup_frame(ctx: &egui::Context) -> egui::Frame {
    let mut style = (*ctx.style()).clone();
    style.visuals = dark_visuals(current_text_contrast(ctx));
    egui::Frame::popup(&style)
}

/// フルスクリーンの標準 menu popup。背景枠と子 widget の両方を暗色へ揃える。
fn dark_menu_popup_style(ctx: &egui::Context) -> StyleModifier {
    let visuals = dark_visuals(current_text_contrast(ctx));
    StyleModifier::new(move |style| {
        // Popup::menu installs this modifier by default. Popup::style replaces rather than
        // composes it, so apply the dark visuals and then restore menu-specific spacing/fills.
        style.visuals = visuals.clone();
        egui::containers::menu::menu_style(style);
    })
}

/// フルスクリーンの標準 menu popup。背景枠と子 widget の両方を暗色へ揃える。
#[allow(dead_code)] // lib target には bin 専用 fullscreen UI が含まれない。
pub(crate) fn dark_menu_popup<'a>(response: &'a egui::Response) -> egui::Popup<'a> {
    egui::Popup::menu(response)
        .style(dark_menu_popup_style(&response.ctx))
        .frame(dark_popup_frame(&response.ctx))
}

/// タイトルバーを持つ暗色固定 `egui::Window` のための限定スコープ。
///
/// Window は生成時に Context の Style を読むため局所 `Ui::visuals_mut` だけではタイトルバーを
/// 暗くできない。Light / Dark 両 Style と選択テーマを保存・復元し、以前の実装のように
/// 非選択側の Dark Style を書き残さないことを不変条件にする。
#[allow(dead_code)] // lib target には bin 専用 dialog / editor UI が含まれない。
pub(crate) fn with_dark_context_style<R>(
    ctx: &egui::Context,
    add_contents: impl FnOnce() -> R,
) -> R {
    struct Guard {
        ctx: egui::Context,
        light: std::sync::Arc<egui::Style>,
        dark: std::sync::Arc<egui::Style>,
        preference: egui::ThemePreference,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            self.ctx
                .set_style_of(egui::Theme::Light, self.light.clone());
            self.ctx.set_style_of(egui::Theme::Dark, self.dark.clone());
            self.ctx.set_theme(self.preference);
        }
    }

    let guard = Guard {
        ctx: ctx.clone(),
        light: ctx.style_of(egui::Theme::Light),
        dark: ctx.style_of(egui::Theme::Dark),
        preference: ctx.options(|options| options.theme_preference),
    };
    let mut dark_style = (*guard.dark).clone();
    dark_style.visuals = dark_visuals(current_text_contrast(ctx));
    ctx.set_style_of(egui::Theme::Dark, dark_style);
    ctx.set_theme(egui::ThemePreference::Dark);
    let result = add_contents();
    drop(guard);
    result
}

/// mImageViewer 向けにスクロールバーだけを少し見つけやすくする。
///
/// 色味は egui のテーマ既定に任せ、幅と opacity だけを調整する。
fn apply_scrollbar_visibility_style(ctx: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut style = (*ctx.style_of(theme)).clone();
        let scroll = &mut style.spacing.scroll;
        scroll.bar_width = 10.0;
        scroll.floating_width = 8.0;
        scroll.floating_allocated_width = 0.0;
        scroll.dormant_background_opacity = 0.10;
        scroll.active_background_opacity = 0.20;
        scroll.interact_background_opacity = 0.30;
        scroll.dormant_handle_opacity = 0.45;
        scroll.active_handle_opacity = 0.65;
        scroll.interact_handle_opacity = 0.80;
        ctx.set_style_of(theme, style);
    }
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

    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
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
        Some(if data == 0 {
            ResolvedTheme::Dark
        } else {
            ResolvedTheme::Light
        })
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
    0.2126 * srgb_to_linear(c.r()) + 0.7152 * srgb_to_linear(c.g()) + 0.0722 * srgb_to_linear(c.b())
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

    #[test]
    fn all_semantic_text_and_widget_states_meet_wcag_aa() {
        for resolved in [ResolvedTheme::Light, ResolvedTheme::Dark] {
            for contrast in [TextContrast::Standard, TextContrast::Strong] {
                let v = app_visuals(resolved, contrast);
                let widget_background = |widget: &egui::style::WidgetVisuals| {
                    if widget.weak_bg_fill == Color32::TRANSPARENT {
                        v.panel_fill
                    } else {
                        widget.weak_bg_fill
                    }
                };
                for (name, foreground, background) in [
                    ("normal", v.text_color(), v.panel_fill),
                    ("secondary", v.weak_text_color(), v.panel_fill),
                    ("warning", v.warn_fg_color, v.panel_fill),
                    ("error", v.error_fg_color, v.panel_fill),
                    (
                        "inactive widget",
                        v.widgets.inactive.fg_stroke.color,
                        widget_background(&v.widgets.inactive),
                    ),
                    (
                        "hovered widget",
                        v.widgets.hovered.fg_stroke.color,
                        widget_background(&v.widgets.hovered),
                    ),
                    (
                        "active widget",
                        v.widgets.active.fg_stroke.color,
                        widget_background(&v.widgets.active),
                    ),
                    (
                        "open popup widget",
                        v.widgets.open.fg_stroke.color,
                        widget_background(&v.widgets.open),
                    ),
                ] {
                    let ratio = contrast_ratio(foreground, background);
                    assert!(
                        ratio >= 4.5,
                        "{resolved:?}/{contrast:?} {name}: {foreground:?} on {background:?} = {ratio:.2}",
                    );
                }
            }
        }
    }

    #[test]
    fn strong_mode_increases_normal_and_secondary_contrast() {
        for resolved in [ResolvedTheme::Light, ResolvedTheme::Dark] {
            let standard = app_visuals(resolved, TextContrast::Standard);
            let strong = app_visuals(resolved, TextContrast::Strong);
            assert!(
                contrast_ratio(strong.text_color(), strong.panel_fill)
                    > contrast_ratio(standard.text_color(), standard.panel_fill)
            );
            assert!(
                contrast_ratio(strong.weak_text_color(), strong.panel_fill)
                    > contrast_ratio(standard.weak_text_color(), standard.panel_fill)
            );
        }
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

    #[test]
    fn apply_resolved_keeps_scrollbars_visible_without_background_color_changes() {
        let ctx = egui::Context::default();
        apply_resolved(&ctx, ResolvedTheme::Light);

        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let style = ctx.style_of(theme);
            let scroll = &style.spacing.scroll;
            assert_eq!(scroll.bar_width, 10.0, "{theme:?}");
            assert_eq!(scroll.floating_width, 8.0, "{theme:?}");
            assert!(scroll.dormant_handle_opacity >= 0.40, "{theme:?}");
            assert!(
                scroll.active_handle_opacity >= scroll.dormant_handle_opacity,
                "{theme:?}"
            );
            assert!(
                scroll.interact_handle_opacity >= scroll.active_handle_opacity,
                "{theme:?}"
            );
            assert!(scroll.interact_handle_opacity <= 0.80, "{theme:?}");
        }

        let default_light = egui::Visuals::light();
        assert_eq!(
            ctx.style_of(egui::Theme::Light)
                .visuals
                .widgets
                .inactive
                .bg_fill,
            default_light.widgets.inactive.bg_fill,
        );
    }

    #[test]
    fn dark_menu_popup_keeps_egui_menu_spacing_and_strokes() {
        let ctx = egui::Context::default();
        apply_resolved_with_contrast(&ctx, ResolvedTheme::Light, TextContrast::Strong);
        let mut style = (*ctx.style()).clone();

        dark_menu_popup_style(&ctx).apply(&mut style);

        assert_eq!(style.spacing.button_padding, egui::vec2(2.0, 0.0));
        assert_eq!(
            style.visuals.widgets.inactive.weak_bg_fill,
            Color32::TRANSPARENT
        );
        assert_eq!(style.visuals.widgets.inactive.bg_stroke, egui::Stroke::NONE);
        assert_eq!(style.visuals.widgets.hovered.bg_stroke, egui::Stroke::NONE);
        assert!(style.visuals.dark_mode);
        assert_eq!(
            style.visuals.text_color(),
            dark_visuals(TextContrast::Strong).text_color()
        );
    }

    #[test]
    fn dark_window_scope_restores_both_theme_styles_and_preference() {
        let ctx = egui::Context::default();
        apply_resolved_with_contrast(&ctx, ResolvedTheme::Light, TextContrast::Strong);
        let light_before = ctx.style_of(egui::Theme::Light);
        let dark_before = ctx.style_of(egui::Theme::Dark);
        let preference_before = ctx.options(|options| options.theme_preference);

        with_dark_context_style(&ctx, || {
            assert_eq!(
                ctx.options(|options| options.theme_preference),
                egui::ThemePreference::Dark
            );
            assert!(ctx.style().visuals.dark_mode);
        });

        assert_eq!(
            ctx.options(|options| options.theme_preference),
            preference_before
        );
        assert_eq!(ctx.style_of(egui::Theme::Light), light_before);
        assert_eq!(ctx.style_of(egui::Theme::Dark), dark_before);
        assert!(!ctx.style().visuals.dark_mode);
    }

    // ハイパーリンク色は mimageviewer では使用していないため検証対象外。
    // (egui::Visuals::light のデフォルト #009BFF は WCAG AA を満たさない)
}
