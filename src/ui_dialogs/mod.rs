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
mod cache_creator;
mod cache_manager;
pub(crate) mod context_menu;
mod context_shortcuts;
pub(crate) mod editing_addon;
mod fav_add;
pub(crate) mod favorites_editor;
mod first_setup;
mod metadata_cleanup;
pub(crate) mod new_folder;
mod open_folder;
mod pdf_password;
pub(crate) mod preferences;
pub(crate) mod rename_item;
mod rotation_reset;
pub(crate) mod settings_restore;
mod stats_dialog;
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
