//! mIV Remote 向けの動画・音声ストリーミング基盤。
//!
//! encoder、メモリ fMP4 セグメンタ、音声・映像 tap と、それらを remote session owner に
//! 従属させる generation worker を所有する。IPC / HTTP への公開は増分 6 で追加する。

pub(crate) mod audio_encoder;
pub(crate) mod encoder;
pub(crate) mod playlist;
pub(crate) mod quality;
pub(crate) mod segmenter;
pub(crate) mod session;
pub(crate) mod timeline;
pub(crate) mod video_tap;
