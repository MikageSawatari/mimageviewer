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

/// テスト用に本体と同じフォント fallback を `ctx` に登録する。
/// これをしないと `豆腐` 文字だらけのスナップショットになり、ラベル・見出しや
/// 絵文字混じりテキストの実際のレイアウトを検証できない。
fn install_app_fonts(ctx: &egui::Context) {
    mimageviewer::ui_fonts::configure_fonts(ctx);
}

/// テストハーネスのユーティリティ: 指定テーマで UI を描画し、`name` でスナップショットを取る。
fn snapshot_with_theme(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    build_ui: impl FnMut(&mut egui::Ui),
) {
    snapshot_with_theme_and_contrast(
        name,
        resolved,
        mimageviewer::settings::TextContrast::Standard,
        build_ui,
    );
}

fn snapshot_with_theme_and_contrast(
    name: &str,
    resolved: mimageviewer::os_theme::ResolvedTheme,
    contrast: mimageviewer::settings::TextContrast,
    mut build_ui: impl FnMut(&mut egui::Ui),
) {
    let mut fonts_ready = false;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(480.0, 360.0))
        .build(move |ctx| {
            mimageviewer::os_theme::apply_resolved_with_contrast(ctx, resolved, contrast);
            if !fonts_ready {
                install_app_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    egui::Frame::central_panel(ui.style())
                        .outer_margin(8.0)
                        .inner_margin(0.0)
                        .show(ui, |ui| build_ui(ui));
                });
        });

    harness.run();
    harness.snapshot(name);
}

fn contrast_fixture(ui: &mut egui::Ui) {
    ui.set_min_width(440.0);
    ui.heading("文字コントラスト");
    ui.label("通常文字：ツールバーやメニューと共通の色です。");
    ui.label(egui::RichText::new("薄い文字：補足情報や件数表示です。").weak());
    ui.horizontal(|ui| {
        let _ = ui.button("通常ボタン");
        ui.add_enabled(false, egui::Button::new("無効ボタン"));
    });
    ui.label(egui::RichText::new("注意表示").color(ui.visuals().warn_fg_color));
    ui.label(egui::RichText::new("エラー表示").color(ui.visuals().error_fg_color));
}

#[test]
fn text_contrast_strong_light() {
    snapshot_with_theme_and_contrast(
        "text_contrast_strong_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        mimageviewer::settings::TextContrast::Strong,
        contrast_fixture,
    );
}

#[test]
fn text_contrast_strong_dark() {
    snapshot_with_theme_and_contrast(
        "text_contrast_strong_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        mimageviewer::settings::TextContrast::Strong,
        contrast_fixture,
    );
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

#[test]
fn metadata_text_fallback_emoji_symbols_dark() {
    snapshot_with_theme(
        "metadata_text_fallback_emoji_symbols_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.set_width(440.0);
            ui.label(egui::RichText::new("説明").strong());
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("愛💗𝓈𝒸𝓇𝑒𝒶𝓂…𝓈𝒸𝓇𝑒𝒶𝓂…💗")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("🍧original  🧠今までのおうたの再生リスト")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("🐾今までのおうたの再生リスト")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("CJK mix: 简体字测试 / 繁體字測試 / 日本語 / 한글")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("✉Contact form")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("★お気に入り  ♪BGM  ※注釈  ☎Info")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.label(
                egui::RichText::new("🧠今までのおうたの再生リスト")
                    .font(mimageviewer::ui_fonts::user_text_font(12.0)),
            );
            ui.label(
                egui::RichText::new("⋈ -------------------------------- ⋈")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let _ = ui.button("🔖");
                let _ = ui.button("✏");
                let _ = ui.button("↻ プラグインを再読み込み");
            });
        },
    );
}

/// `draw_cell_filename` のフォント family が `miv-user-text` であることの回帰防止。
/// 絵文字 (💎) / 数学英字 (𝓈𝒸𝓇𝑒𝒶𝓂) / 日本語 / ASCII を 1 つのラベルに混ぜて、
/// ベースラインのずれが PNG 差分として検知できる状態にする。
#[test]
fn cell_filename_mixed_glyphs_dark() {
    snapshot_with_theme(
        "cell_filename_mixed_glyphs_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            let (_resp, painter) =
                ui.allocate_painter(egui::vec2(220.0, 160.0), egui::Sense::hover());
            let inner = painter.clip_rect();
            painter.rect_filled(inner, 2.0, egui::Color32::from_gray(60));
            mimageviewer::ui_helpers::draw_cell_filename(
                &painter,
                inner,
                "001 - お返事まだカナ💎𝓈𝒸𝓇𝑒𝒶𝓂おじ",
                egui::Color32::WHITE,
                true,
                0.0,
            );
        },
    );
}

#[test]
fn stats_histogram_compact_columns_light() {
    snapshot_with_theme(
        "stats_histogram_compact_columns_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.set_width(440.0);
            ui.heading("読み込み時間 (decode + display)");
            ui.add_space(4.0);

            let mut hist = [0_u64; mimageviewer::stats::LOAD_TIME_BUCKETS];
            for (bucket, count) in [
                47, 6, 6, 0, 0, 1, 2, 0, 4, 3, 4, 2, 2, 0, 0, 0, 0, 1, 0, 0, 9,
            ]
            .into_iter()
            .enumerate()
            {
                hist[bucket] = count;
            }

            mimageviewer::ui_helpers::draw_histogram(
                ui,
                &hist,
                mimageviewer::stats::ThumbStats::load_time_label,
                None,
            );
        },
    );
}

/// 更新後「重要な変更点」ダイアログ本体 (version_highlights::render) の回帰防止。
/// 複数バージョンまたぎの合成 payload を食わせて、必読 (⚠) / 新機能 (・) の 2 段構成と
/// バージョン見出しのレイアウト崩れを実機なしで検知する (docs/version-highlights-plan.md §5)。
#[test]
fn whats_new_dialog_multi_version_dark() {
    use mimageviewer::version_highlights::{HighlightItem, VersionHighlights};
    const MUST_15: &[HighlightItem] = &[HighlightItem {
        title: "操作の既定が変わりました",
        body: "従来の動作は設定から選べます。",
    }];
    const MUST_20: &[HighlightItem] = &[HighlightItem {
        title: "ツールバーの設定は右クリックに変わりました",
        body: "ツールバーを右クリックして表示項目・並び順・表示形式を変更します。",
    }];
    const HIGH_20: &[HighlightItem] = &[HighlightItem {
        title: "よく使う本をツールバーにピン留め",
        body: "本棚の管理画面で本を固定すると、ツールバーにボタンが並びます。",
    }];
    const V15: VersionHighlights = VersionHighlights {
        version: "1.5.0",
        must_read: MUST_15,
        highlights: &[],
    };
    const V20: VersionHighlights = VersionHighlights {
        version: "2.0.0",
        must_read: MUST_20,
        highlights: HIGH_20,
    };
    // ダイアログは新しいバージョンを上に出す (= 更新履歴と同じ並び)。テストもその順で渡す。
    let entries = [&V20, &V15];
    snapshot_with_theme(
        "whats_new_multi_version_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        move |ui| {
            ui.set_width(440.0);
            mimageviewer::version_highlights::render(ui, &entries);
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
        PoolStatus::ReadyWithPlugins {
            count: plugins.len(),
        },
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
        PoolStatus::ReadyWithPlugins {
            count: plugins.len(),
        },
        plugins,
    );
}

// ---------------------------------------------------------------------------
// 更新履歴 (GitHub release body) の Markdown 描画スナップショット
// ---------------------------------------------------------------------------

/// バージョン更新ダイアログに表示する release body の代表サンプル。
/// 見出し / 箇条書き / ネスト / `**強調**` / `` `コード` `` / `<kbd>キー</kbd>` を網羅し、
/// 整形描画 ([mimageviewer::changelog_markdown]) の見た目を回帰検出できるようにする。
fn changelog_body_fixture() -> &'static str {
    "### v0.9.1\n\
     - **キャプチャ保存**: 画像フルスクリーン中に <kbd>Ctrl</kbd>+<kbd>S</kbd> を押すと、\
     表示中の画像を保存できます。保存形式は環境設定の `キャプチャ保存` ページで設定します\n\
     - **比較ビュー**: <kbd>X</kbd> でピン留めし、<kbd>C</kbd> でトグル表示します\n\
     \u{0020}\u{0020}- <kbd>Shift</kbd>+<kbd>C</kbd> で左右に並べたワイプ比較\n\
     - 設定ファイル `settings.db` は初回起動時に自動移行されます"
}

#[test]
fn changelog_markdown_light() {
    snapshot_with_theme(
        "changelog_markdown_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::changelog_markdown::render(ui, changelog_body_fixture());
        },
    );
}

#[test]
fn changelog_markdown_dark() {
    snapshot_with_theme(
        "changelog_markdown_dark",
        mimageviewer::os_theme::ResolvedTheme::Dark,
        |ui| {
            ui.set_width(440.0);
            mimageviewer::changelog_markdown::render(ui, changelog_body_fixture());
        },
    );
}
