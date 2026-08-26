//! Susie プラグイン診断パネルの描画 (v0.7.0〜)。
//!
//! 環境設定ダイアログの「Susie プラグイン」ページで、現在のワーカープール状態を
//! ユーザーに分かりやすく表示するためのレンダリングロジック。UI から切り出すことで
//! `egui_kittest` によるスナップショットテストが可能になっている。
//!
//! 単体実装なので呼び出し側 (preferences.rs) は `render_diagnostic()` を 1 本呼ぶだけで、
//! `PoolStatus` の各バリアントに応じた文言・配色・レイアウトを描く。

use crate::susie_loader::{PluginInfo, PoolStatus, SusieWorkerHealth, WORKER_EXE_NAME};

/// Susie プラグイン診断メッセージを描画する。
///
/// - `status`: 現在のプール状態 (UI から `pool_status(enabled)` で取得)。
/// - `plugins`: `ReadyWithPlugins` のときに展開対象とするプラグイン情報。
///   それ以外のバリアントでは使用しない (空でよい)。
pub fn render_diagnostic(ui: &mut egui::Ui, status: &PoolStatus, plugins: &[PluginInfo]) {
    match status {
        PoolStatus::ReadyWithPlugins { health, .. } => {
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
                        egui::RichText::new(format!("対応拡張子: {}", parts.join(", "))).weak(),
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
            render_recovery_history(ui, health);
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
                egui::RichText::new(format!("展開先パス: {}", expected_path.display(),))
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
                egui::RichText::new("⚠ ワーカープロセスの起動またはハンドシェイクに失敗しました。")
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
        PoolStatus::WorkersExhausted { health } => {
            ui.label(
                egui::RichText::new(
                    "⚠ プラグインが繰り返し異常終了したため、Susie 形式の読み込みを打ち切りました。",
                )
                .color(ui.visuals().warn_fg_color),
            );
            ui.label(
                egui::RichText::new(
                    "上の「⟳ プラグインを再読み込み」を押すと、もう一度使えるようになります。\n\
                     同じ画像で再発する場合は、そのプラグインを外すか、\n\
                     「プラグインを並列実行する」を OFF にして切り分けてください。",
                )
                .size(11.0)
                .weak(),
            );
            render_health_details(ui, health);
        }
    }
}

/// 復帰の履歴。**何も起きていなければ何も出さない。** 常時出す情報ではなく、
/// 「読めない画像があった」ときに理由を辿るためのもの。
fn render_recovery_history(ui: &mut egui::Ui, health: &SusieWorkerHealth) {
    if health.restarts == 0 && health.crashing_subjects == 0 && !health.degraded() {
        return;
    }
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("⚠ プラグインの異常終了から復帰しました")
            .color(ui.visuals().warn_fg_color),
    );
    render_health_details(ui, health);
}

fn render_health_details(ui: &mut egui::Ui, health: &SusieWorkerHealth) {
    ui.label(
        egui::RichText::new(health_detail_text(health))
            .size(11.0)
            .weak(),
    );
}

/// 診断の数値内訳。不具合報告へそのまま写せる粒度で出す。
///
/// 0 の項目は省く。「作り直し 0 回」「打ち切り 0 件」を並べても読む側の手掛かりに
/// ならず、実際に起きた項目が埋もれる。
pub fn health_detail_text(health: &SusieWorkerHealth) -> String {
    let mut lines = vec![format!(
        "ワーカー: {} / {}",
        health.live_workers, health.started_workers
    )];
    if health.restarts > 0 {
        lines.push(format!("作り直した回数: {}", health.restarts));
    }
    if health.gave_up_workers > 0 {
        lines.push(format!(
            "作り直しを打ち切った数: {}",
            health.gave_up_workers
        ));
    }
    if health.crashing_subjects > 0 {
        lines.push(format!(
            "異常終了を起こした画像: {} 件 (このセッションでは再試行しません)",
            health.crashing_subjects
        ));
    }
    if let Some(reason) = &health.last_failure {
        lines.push(format!("最後のエラー: {reason}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::health_detail_text;
    use crate::susie_loader::SusieWorkerHealth;

    #[test]
    fn a_healthy_pool_reports_only_the_worker_count() {
        let text = health_detail_text(&SusieWorkerHealth {
            started_workers: 3,
            live_workers: 3,
            ..SusieWorkerHealth::default()
        });
        assert_eq!(text, "ワーカー: 3 / 3");
    }

    #[test]
    fn everything_that_happened_is_listed_once() {
        let text = health_detail_text(&SusieWorkerHealth {
            started_workers: 3,
            live_workers: 1,
            restarts: 4,
            gave_up_workers: 2,
            crashing_subjects: 3,
            last_failure: Some("unexpected end of file".to_string()),
        });
        assert!(text.contains("ワーカー: 1 / 3"), "{text}");
        assert!(text.contains("作り直した回数: 4"), "{text}");
        assert!(text.contains("作り直しを打ち切った数: 2"), "{text}");
        assert!(text.contains("異常終了を起こした画像: 3 件"), "{text}");
        assert!(text.contains("unexpected end of file"), "{text}");
    }

    /// 折り返し由来の空白を行頭に残さない (Rust の行継続を書き損ねると混入する)。
    #[test]
    fn detail_lines_have_no_leading_whitespace() {
        let text = health_detail_text(&SusieWorkerHealth {
            started_workers: 3,
            live_workers: 0,
            restarts: 5,
            gave_up_workers: 3,
            crashing_subjects: 1,
            last_failure: Some("broken pipe".to_string()),
        });
        for line in text.lines() {
            assert_eq!(line, line.trim_start(), "行頭に空白が残っている: {line:?}");
        }
    }
}
