//! mimageviewer ライブラリクレート。
//! 統合テストやベンチマーク bin から公開モジュールにアクセスするためのエントリポイント。
//!
//! `app` は bin (main.rs) 専属の private module なので lib からはアクセスできない。
//! lib 経由で参照される module (例: `video`) が `crate::app::FOO` を使う場合、
//! main.rs と lib.rs の両方からアクセスできるよう、ここに `app` 互換 stub を置く
//! (= main.rs の app module の同名定数の真値とは別実体だが、lib 側で実用上問題がないもの)。

#[cfg(windows)]
#[doc(hidden)]
pub mod app {
    /// GPU テクスチャ上限。bin (lib) 側の video モジュールで `crate::app::MAX_TEXTURE_DIM`
    /// として参照される。本体 (main.rs) の `app` module 内の同名定数と値が一致するよう、
    /// 変更時はここも合わせて更新すること。
    pub const MAX_TEXTURE_DIM: usize = 8192;
    pub const VIDEO_RESUME_MIN_POSITION_SECS: f64 = 3.0;
    pub const VIDEO_RESUME_END_GUARD_SECS: f64 = 5.0;
}

pub mod activity_gate;
pub mod adjustment;
pub mod adjustment_db;
pub mod ai;
pub mod archive_cache;
pub mod archive_converter;
pub mod audio_normalize_db;
pub mod cache_maintenance;
pub mod catalog;
pub mod data_dir;
pub mod delete_worker;
#[cfg(windows)]
pub mod dwm_iconic_thumbnail;
#[cfg(windows)]
pub mod dwm_transitions;
pub mod exif_reader;
pub mod external_links;
pub mod fast_resize;
pub mod folder_rating_counter;
pub mod folder_thumb_pins;
pub mod folder_tree;
pub mod fts_index;
pub mod fts_meta;
pub mod fts_writer_dispatcher;
pub mod global_search;
// global_search_ui は App (main.rs 側 private module) に impl するため bin crate のみで公開する
pub mod gpu_info;
pub mod grid_item;
pub mod indexer_manager;
pub mod indexer_progress;
pub mod indexer_supervisor;
pub mod ingest_text;
pub mod ingest_worker;
pub mod io_semaphore;
pub mod logger;
pub mod mask_db;
pub mod name_bulk_indexer;
pub mod name_index_supervisor;
pub mod os_theme;
pub mod path_key;
pub mod pdf_loader;
pub mod pdf_passwords;
pub mod perf;
pub mod png_metadata;
pub mod post_filter;
pub mod rating_db;
pub mod rating_write_worker;
pub mod search_index_db;
pub mod search_norm;
pub mod search_query;
pub mod search_walker;
pub mod search_watcher;
pub mod settings;
pub mod settings_db;
pub mod sidecar;
pub mod stats;
pub mod susie_loader;
pub mod sys_memory;
pub mod tag_write_worker;
pub mod thumb_loader;
pub mod ui_fonts;
pub mod ui_helpers;
pub mod ui_susie_diagnostic;
pub mod ui_text_links;
pub mod undo_stack;
pub mod update_check;
#[cfg(windows)]
pub mod video;
pub mod video_bookmarks;
pub mod video_chapter_thumbs;
pub mod video_pins;
pub mod wic_decoder;
pub mod xmp_reader;
pub mod xmp_writer;
pub mod zip_loader;
