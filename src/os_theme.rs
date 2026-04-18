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
