//! `App` のダイアログ・オーバーレイ表示メソッドを集めたサブモジュール。
//!
//! 各ファイルは `impl crate::app::App { fn show_xxx_dialog(...) {...} }` の
//! 形でメソッドを 1 つだけ提供する。これらのメソッドは `App::update()` から
//! 呼び出される。
//!
//! ダイアログを増やしたい場合は、ここに新しい .rs を追加し、`mod` 宣言を
//! 加えるだけで `update()` から `self.show_new_dialog(ctx)` として呼べる。

mod cache_creator;
mod cache_manager;
mod cache_policy;
pub(crate) mod context_menu;
pub(crate) mod duplicate_settings;
mod exif_settings;
mod fav_add;
mod favorites_editor;
mod open_folder;
mod preferences;
mod rotation_reset;
mod slideshow_settings;
mod stats_dialog;
mod thumb_quality;
mod thumb_quality_fullscreen;
mod pdf_password;
mod toolbar_settings;
mod ai_model_setup;
mod about;
