//! `musicvideo-core` — 音楽ビデオ書き出しの純ロジック（egui / FFmpeg / wgpu 非依存）。
//!
//! 正本: `docs/music-video-export-plan.md`。P1a のブリーフ: `docs/brief-musicvideo-p1a.md`。
//!
//! テキストは `comic-core`、スペクトラム解析は `music-core` を呼ぶ。ここで別のフォント
//! スタックや別の解析器を立てない。
