//! egui_kittest による UI スナップショットテスト (v0.7.0〜)。
//!
//! ## 目的
//!
//! 3 テーマ (Light / Dark / System) × 主要 UI (メイン・環境設定・メタデータパネル等) の
//! 見た目を PNG スナップショットとして保存し、意図しない見た目変化を回帰として検出する。
//! カラースキーム・パネル崩れ・余白計算の回帰を自動検知するのが狙い。
//!
//! ## 実行
//!
//! ```
//! cargo test --test ui_snapshot
//! ```
//!
//! ## スナップショット更新 (意図的に見た目を変えたとき)
//!
//! ```
//! UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot
//! ```
//!
//! 更新後は `tests/snapshots/ui_snapshot/*.png` の差分を目視確認してからコミットする。
//!
//! ## 参考
//!
//! - [egui_kittest docs](https://docs.rs/egui_kittest/)
//! - mimageviewer 側のポリシー: [docs/ui-snapshot-policy.md](../docs/ui-snapshot-policy.md)

use egui_kittest::Harness;
use std::sync::Arc;

/// テスト用に本体と同じ日本語フォントを `ctx` に登録する。
/// これをしないと `豆腐` 文字だらけのスナップショットになり、ラベル・見出しの
/// 実際のレイアウトを検証できない。本体の `main::setup_fonts` と同等の挙動。
fn install_japanese_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let font_paths = [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];
    for path in &font_paths {
        if let Ok(data) = std::fs::read(path) {
            fonts.font_data.insert(
                "japanese".to_owned(),
                Arc::new(egui::FontData::from_owned(data)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "japanese".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "japanese".to_owned());
            break;
        }
    }
    ctx.set_fonts(fonts);
}

/// テストハーネスのユーティリティ: 指定テーマで UI を描画し、`name` でスナップショットを取る。
fn snapshot_with_theme(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    build_ui: impl FnMut(&mut egui::Ui),
) {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(480.0, 360.0))
        .build_ui(build_ui);

    // 日本語フォント + テーマ適用 (egui::Visuals::light / dark)
    install_japanese_font(&harness.ctx);
    mimageviewer::os_theme::apply_resolved(&harness.ctx, resolved);

    harness.run();
    harness.snapshot(name);
}

/// シンプルなラベル+ボタンを Light テーマで描画して、基盤が動くことを確認する
/// スモークテスト。
#[test]
fn smoke_label_and_button_light() {
    snapshot_with_theme(
        "smoke_label_and_button_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.heading("mImageViewer");
            ui.label("UI スナップショット基盤のスモークテストです。");
            ui.separator();
            let _ = ui.button("OK");
        },
    );
}

/// 同じ UI を Dark テーマで描画。Light/Dark で差が出ることを目視確認用に保存しておく。
#[test]
fn smoke_label_and_button_dark() {
    snapshot_with_theme(
        "smoke_label_and_button_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.heading("mImageViewer");
            ui.label("UI スナップショット基盤のスモークテストです。");
            ui.separator();
            let _ = ui.button("OK");
        },
    );
}

// ---------------------------------------------------------------------------
// Susie 診断 UI (PoolStatus 各バリアントのレンダリング) のスナップショット
// ---------------------------------------------------------------------------

use mimageviewer::susie_loader::{PluginInfo, PoolStatus};
use mimageviewer::ui_susie_diagnostic::render_diagnostic;
use std::path::PathBuf;

fn snapshot_diagnostic_themed(
    name: &str,
    theme: mimageviewer::os_theme::ResolvedTheme,
    status: PoolStatus,
    plugins: Vec<PluginInfo>,
) {
    snapshot_with_theme(name, theme, move |ui| {
        ui.label(egui::RichText::new("ロード済みプラグイン").strong());
        ui.add_space(4.0);
        render_diagnostic(ui, &status, &plugins);
    });
}

fn snapshot_diagnostic(name: &str, status: PoolStatus, plugins: Vec<PluginInfo>) {
    snapshot_diagnostic_themed(
        name,
        mimageviewer::os_theme::ResolvedTheme::Light,
        status,
        plugins,
    );
}

#[test]
fn susie_diagnostic_disabled_by_settings() {
    snapshot_diagnostic(
        "susie_diagnostic_disabled",
        PoolStatus::DisabledBySettings,
        Vec::new(),
    );
}

#[test]
fn susie_diagnostic_not_initialized() {
    snapshot_diagnostic(
        "susie_diagnostic_not_initialized",
        PoolStatus::NotInitialized,
        Vec::new(),
    );
}

#[test]
fn susie_diagnostic_worker_missing() {
    snapshot_diagnostic(
        "susie_diagnostic_worker_missing",
        PoolStatus::WorkerExeMissing {
            expected_path: PathBuf::from(
                "C:\\Users\\example\\AppData\\Roaming\\mimageviewer\\mimageviewer-susie32.exe",
            ),
        },
        Vec::new(),
    );
}

#[test]
fn susie_diagnostic_worker_spawn_failed() {
    snapshot_diagnostic(
        "susie_diagnostic_worker_spawn_failed",
        PoolStatus::WorkerSpawnFailed,
        Vec::new(),
    );
}

#[test]
fn susie_diagnostic_ready_but_empty() {
    snapshot_diagnostic(
        "susie_diagnostic_ready_but_empty",
        PoolStatus::ReadyButEmpty,
        Vec::new(),
    );
}

fn ready_with_plugins_fixture() -> Vec<PluginInfo> {
    // レトロ専用 (本体優先がない) プラグイン + シャドウありのプラグインを混在させ、
    // 「⚠」マーカーと「本体優先」バッジ・注記が両方表示されるケースをカバーする。
    vec![
        PluginInfo {
            name: "ifpi.spi (PC-98 PI)".to_string(),
            extensions: vec!["pi".to_string()],
        },
        PluginInfo {
            name: "ifmag.spi (PC-98 MAG)".to_string(),
            extensions: vec!["mag".to_string()],
        },
        PluginInfo {
            name: "ifjpegt.spi (JPEG 再実装)".to_string(),
            extensions: vec!["jpg".to_string(), "jpeg".to_string()],
        },
    ]
}

#[test]
fn susie_diagnostic_ready_with_plugins() {
    let plugins = ready_with_plugins_fixture();
    snapshot_diagnostic(
        "susie_diagnostic_ready_with_plugins",
        PoolStatus::ReadyWithPlugins { count: plugins.len() },
        plugins,
    );
}

/// Light / Dark でも診断 UI が破綻せず読めることを確認する。
#[test]
fn susie_diagnostic_ready_with_plugins_dark() {
    let plugins = ready_with_plugins_fixture();
    snapshot_diagnostic_themed(
        "susie_diagnostic_ready_with_plugins_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        PoolStatus::ReadyWithPlugins { count: plugins.len() },
        plugins,
    );
}
