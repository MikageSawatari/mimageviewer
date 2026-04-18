//! mimageviewer-susie32 のライブラリインタフェース。
//!
//! バイナリ (`main.rs`) はこのライブラリを使って stdin/stdout ワーカーループを実装する。
//! 統合テスト (`tests/plugin_decode.rs`) は `PluginHost` を直接使ってプラグインの
//! ロード・デコードを検証する。

#![cfg(windows)]

pub mod plugin;
pub mod protocol;
