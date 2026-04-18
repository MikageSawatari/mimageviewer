//! Susie プラグイン診断パネルの描画 (v0.7.0〜)。
//!
//! 環境設定ダイアログの「Susie プラグイン」ページで、現在のワーカープール状態を
//! ユーザーに分かりやすく表示するためのレンダリングロジック。UI から切り出すことで
//! `egui_kittest` によるスナップショットテストが可能になっている。
//!
//! 単体実装なので呼び出し側 (preferences.rs) は `render_diagnostic()` を 1 本呼ぶだけで、
//! `PoolStatus` の各バリアントに応じた文言・配色・レイアウトを描く。

use crate::susie_loader::{PluginInfo, PoolStatus, WORKER_EXE_NAME};

/// Susie プラグイン診断メッセージを描画する。
///
/// - `status`: 現在のプール状態 (UI から `pool_status(enabled)` で取得)。
/// - `plugins`: `ReadyWithPlugins` のときに展開対象とするプラグイン情報。
///   それ以外のバリアントでは使用しない (空でよい)。
pub fn render_diagnostic(ui: &mut egui::Ui, status: &PoolStatus, plugins: &[PluginInfo]) {
    match status {
        PoolStatus::ReadyWithPlugins { .. } => {
            let mut any_shadowed = false;
            for pi in plugins {
                // プラグインが名乗る拡張子のうち、本体がネイティブ対応している
                // ものは実際には本体が優先されるので "(本体優先)" マークを付ける。
                // デコードパスは image → WIC → Susie の順なので、
                // SUPPORTED_EXTENSIONS に含まれる拡張子は Susie に回ってこない。
                let mut parts: Vec<String> = Vec::with_capacity(pi.extensions.len());
                let mut plugin_has_shadow = false;
                for e in &pi.extensions {
                    if crate::folder_tree::SUPPORTED_EXTENSIONS.contains(&e.as_str()) {
                        parts.push(format!("{e} (本体優先)"));
                        plugin_has_shadow = true;
                        any_shadowed = true;
                    } else {
                        parts.push(e.clone());
                    }
                }
                let header = if plugin_has_shadow {
                    format!("{}  ⚠", pi.name)
                } else {
                    pi.name.clone()
                };
                ui.collapsing(header, |ui| {
                    ui.label(
                        egui::RichText::new(format!("対応拡張子: {}", parts.join(", ")))
                            .weak(),
                    );
                });
            }
            if any_shadowed {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "(本体優先) の拡張子は mImageViewer 本体の内蔵デコーダが\n\
                         使われるため、このプラグインは呼ばれません。",
                    )
                    .size(11.0)
                    .weak(),
                );
            }
        }
        PoolStatus::ReadyButEmpty => {
            ui.label(
                egui::RichText::new(
                    "プラグインが 1 つも読み込まれていません。\n\
                     「📁 フォルダを開く」から .spi ファイル (32bit) を配置し、\n\
                     「⟳ プラグインを再読み込み」を押してください。",
                )
                .weak(),
            );
        }
        PoolStatus::WorkerExeMissing { expected_path } => {
            ui.label(
                egui::RichText::new(format!(
                    "⚠ Susie ワーカー ({}) を APPDATA に展開できませんでした。",
                    WORKER_EXE_NAME,
                ))
                .color(egui::Color32::from_rgb(200, 100, 50)),
            );
            ui.label(
                egui::RichText::new(format!(
                    "展開先パス: {}",
                    expected_path.display(),
                ))
                .monospace()
                .size(11.0)
                .weak(),
            );
            ui.label(
                egui::RichText::new(
                    "通常はアプリ起動時に自動展開されます。\n\
                     展開先のフォルダに書き込み権限があるか確認してください。",
                )
                .size(11.0)
                .weak(),
            );
        }
        PoolStatus::WorkerSpawnFailed => {
            ui.label(
                egui::RichText::new(
                    "⚠ ワーカープロセスの起動またはハンドシェイクに失敗しました。",
                )
                .color(egui::Color32::from_rgb(200, 100, 50)),
            );
            ui.label(
                egui::RichText::new("ヘルプ → ログフォルダを開く から詳細を確認できます。")
                    .size(11.0)
                    .weak(),
            );
        }
        PoolStatus::NotInitialized => {
            ui.label(
                egui::RichText::new(
                    "プラグインはまだロードされていません。\n\
                     「⟳ プラグインを再読み込み」を押すと起動されます。",
                )
                .weak(),
            );
        }
        PoolStatus::DisabledBySettings => {
            ui.label(
                egui::RichText::new(
                    "Susie プラグインは無効化されています\n\
                     (上の「Susie 画像プラグインを有効にする」を ON にしてください)。",
                )
                .weak(),
            );
        }
    }
}
