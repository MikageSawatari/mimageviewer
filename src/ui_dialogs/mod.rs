//! `App` のダイアログ・オーバーレイ表示メソッドを集めたサブモジュール。
//!
//! 各ファイルは `impl crate::app::App { fn show_xxx_dialog(...) {...} }` の
//! 形でメソッドを 1 つだけ提供する。これらのメソッドは `App::update()` から
//! 呼び出される。
//!
//! ダイアログを増やしたい場合は、ここに新しい .rs を追加し、`mod` 宣言を
//! 加えるだけで `update()` から `self.show_new_dialog(ctx)` として呼べる。

mod about;
pub(crate) mod archive_cache_manager;
pub(crate) mod archive_convert;
pub(crate) mod batch_convert;
mod cache_creator;
mod cache_manager;
pub(crate) mod context_menu;
mod context_shortcuts;
pub(crate) mod editing_addon;
mod fav_add;
pub(crate) mod favorites_editor;
mod first_setup;
mod metadata_cleanup;
pub(crate) mod metadata_transfer;
pub(crate) mod new_folder;
mod open_folder;
mod pdf_password;
pub(crate) mod preferences;
pub(crate) mod rename_item;
mod rotation_reset;
mod settings_incompatible;
pub(crate) mod settings_restore;
pub(crate) mod smart_folder_editor;
mod stats_dialog;
mod subfolder_expansion;
mod tag_apply;
mod tag_editor;
mod thumb_quality;
mod thumb_quality_fullscreen;
pub(crate) mod trt_install;
pub(crate) mod trt_worker_notice;
mod update_notice;
pub(crate) mod video_upscale;
#[cfg(windows)]
mod vst3_actions;
#[cfg(windows)]
mod vst3_manager;
mod whats_new;

/// 長文ダイアログの縦スクロールバーが本文へ重ならないよう、floating bar の最大幅を
/// viewport の右側へ予約する。アプリ共通 style は一覧の表示面積を優先して floating の
/// 予約幅を 0 にしているため、折り返し本文を持つダイアログだけで局所適用する。
fn non_overlapping_dialog_scroll_style(
    mut scroll: eframe::egui::style::ScrollStyle,
) -> eframe::egui::style::ScrollStyle {
    if scroll.floating {
        scroll.floating_allocated_width = scroll
            .floating_allocated_width
            .max(scroll.bar_inner_margin + scroll.bar_width);
    }
    scroll
}

#[cfg(test)]
mod tests {
    use super::non_overlapping_dialog_scroll_style;

    #[test]
    fn dialog_scroll_style_reserves_the_full_floating_bar_width() {
        let mut scroll = eframe::egui::style::ScrollStyle::floating();
        scroll.bar_width = 10.0;
        scroll.bar_inner_margin = 4.0;
        scroll.floating_allocated_width = 0.0;

        let scroll = non_overlapping_dialog_scroll_style(scroll);

        assert_eq!(scroll.floating_allocated_width, 14.0);
        assert!(scroll.allocated_width() >= scroll.bar_width);
    }

    #[test]
    fn dialog_scroll_style_leaves_solid_scrollbars_unchanged() {
        let scroll = eframe::egui::style::ScrollStyle::solid();
        assert_eq!(non_overlapping_dialog_scroll_style(scroll), scroll);
    }
}
