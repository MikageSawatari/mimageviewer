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

#[cfg(windows)]
fn snapshot_with_ui_font(
    name: &str,
    settings: mimageviewer::settings::UiFontSettings,
    mut build_ui: impl FnMut(&mut egui::Ui),
) {
    let mut fonts_ready = false;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(480.0, 260.0))
        .build(move |ctx| {
            mimageviewer::os_theme::apply_resolved(
                ctx,
                mimageviewer::os_theme::ResolvedTheme::Dark,
            );
            if !fonts_ready {
                mimageviewer::ui_fonts::configure_fonts_with_settings(ctx, &settings);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| build_ui(ui));
        });
    harness.run();
    harness.snapshot(name);
}

#[cfg(windows)]
fn windows_font_settings(
    display_name: &str,
    path: &str,
    family: &str,
) -> mimageviewer::settings::UiFontSettings {
    let mut db = fontdb::Database::new();
    db.load_font_file(path)
        .unwrap_or_else(|err| panic!("{display_name} should load from {path}: {err}"));
    let face = db
        .faces()
        .find(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
        })
        .unwrap_or_else(|| panic!("{path} should contain the {family} face"));
    mimageviewer::settings::UiFontSettings {
        selection: mimageviewer::settings::UiFontSelection::Face {
            display_name: display_name.to_owned(),
            path: std::path::PathBuf::from(path),
            face_index: face.index,
            post_script_name: face.post_script_name.clone(),
        },
        vertical_adjust: 0.0,
    }
}

#[cfg(windows)]
fn recommended_ui_font_fixture(ui: &mut egui::Ui, label: &str, typographic_points: f32) {
    // Windows の 9/10pt を 96 DPI 時の egui logical point へ換算する。
    let size = typographic_points * (96.0 / 72.0);
    let body_font = egui::FontId::new(size, egui::FontFamily::Proportional);
    let toolbar_font = egui::FontId::new(
        size,
        egui::FontFamily::Name(std::sync::Arc::<str>::from(
            mimageviewer::ui_fonts::TOOLBAR_TEXT_FAMILY_NAME,
        )),
    );

    ui.set_width(440.0);
    ui.heading(format!("{label}  {typographic_points}pt"));
    ui.label(egui::RichText::new("mImageViewer  表示サンプル  Aa 0123").font(body_font.clone()));
    ui.label(
        egui::RichText::new("日本語・簡体字测试・한글・💗・𝓈𝒸𝓇𝑒𝒶𝓂")
            .font(mimageviewer::ui_fonts::user_text_font(size)),
    );
    ui.add_space(8.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(430.0, 34.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new("フォルダー:").font(toolbar_font.clone()));
                let _ = ui.add_sized(
                    [68.0, 28.0],
                    egui::Button::new(egui::RichText::new("前へ").font(toolbar_font.clone())),
                );
                let _ = ui.add_sized(
                    [68.0, 28.0],
                    egui::Button::new(egui::RichText::new("次へ").font(toolbar_font)),
                );
            },
        );
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("動画・音声 HUD:").weak());
        let (speed_rect, _) = ui.allocate_exact_size(egui::vec2(43.0, 28.0), egui::Sense::hover());
        ui.painter().text(
            speed_rect.center() + egui::vec2(0.0, 4.0),
            egui::Align2::CENTER_CENTER,
            "x1",
            mimageviewer::ui_fonts::hud_text_font(12.0),
            egui::Color32::from_rgb(238, 238, 238),
        );
        let (norm_rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(norm_rect, 5.0, egui::Color32::from_rgb(55, 105, 170));
        ui.painter().text(
            norm_rect.center() + egui::vec2(0.0, 4.0),
            egui::Align2::CENTER_CENTER,
            "Norm",
            mimageviewer::ui_fonts::hud_text_font(11.0),
            egui::Color32::from_rgb(255, 198, 62),
        );
        ui.painter().text(
            egui::pos2(norm_rect.max.x + 68.0, norm_rect.center().y + 4.0),
            egui::Align2::RIGHT_CENTER,
            "0.0dB",
            mimageviewer::ui_fonts::hud_text_font(13.0),
            egui::Color32::from_rgb(238, 238, 238),
        );
        ui.add_space(72.0);
        ui.painter().text(
            egui::pos2(norm_rect.max.x + 82.0, norm_rect.center().y + 4.0),
            egui::Align2::LEFT_CENTER,
            "01:23 / 04:56",
            mimageviewer::ui_fonts::hud_text_font(14.0),
            egui::Color32::from_rgb(238, 238, 238),
        );
        ui.add_space(110.0);
    });
    ui.separator();
    ui.label(egui::RichText::new("実メトリクスから自動補正・手動調整 0pt").weak());
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

/// v2.7.0: 任意 UI フォントを設定した場合の通常文字・ツールバー文字と、
/// 日本語 / 記号 fallback の縦位置を同時に固定する。
#[cfg(windows)]
#[test]
fn custom_ui_font_meiryo_bold_alignment_dark() {
    let settings = mimageviewer::settings::UiFontSettings {
        selection: mimageviewer::settings::UiFontSelection::Face {
            display_name: "Meiryo Bold".to_string(),
            path: std::path::PathBuf::from(r"C:\Windows\Fonts\meiryob.ttc"),
            face_index: 0,
            post_script_name: String::new(),
        },
        vertical_adjust: 0.75,
    };
    snapshot_with_ui_font(
        "custom_ui_font_meiryo_bold_alignment_dark",
        settings,
        |ui| {
            ui.set_width(440.0);
            ui.heading("UI フォント");
            ui.label("mImageViewer  表示サンプル  Aa 0123");
            ui.label(
                egui::RichText::new("日本語・簡体字测试・한글・💗・𝓈𝒸𝓇𝑒𝒶𝓂")
                    .font(mimageviewer::ui_fonts::user_text_font(18.0)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let toolbar_font = egui::FontId::new(
                    14.0,
                    egui::FontFamily::Name(std::sync::Arc::<str>::from(
                        mimageviewer::ui_fonts::TOOLBAR_TEXT_FAMILY_NAME,
                    )),
                );
                ui.label(egui::RichText::new("フォルダー:").font(toolbar_font.clone()));
                let _ = ui.button(egui::RichText::new("前へ").font(toolbar_font.clone()));
                let _ = ui.button(egui::RichText::new("次へ").font(toolbar_font));
            });
            ui.separator();
            ui.label(egui::RichText::new("自動補正 + 0.75 pt").weak());
        },
    );
}

#[cfg(windows)]
#[test]
fn recommended_ui_font_biz_udp_gothic_9pt_alignment_dark() {
    snapshot_with_ui_font(
        "recommended_ui_font_biz_udp_gothic_9pt_alignment_dark",
        windows_font_settings(
            "BIZ UDPGothic",
            r"C:\Windows\Fonts\BIZ-UDGothicR.ttc",
            "BIZ UDPGothic",
        ),
        |ui| recommended_ui_font_fixture(ui, "BIZ UDPGothic", 9.0),
    );
}

#[cfg(windows)]
#[test]
fn recommended_ui_font_meiryo_10pt_alignment_dark() {
    snapshot_with_ui_font(
        "recommended_ui_font_meiryo_10pt_alignment_dark",
        windows_font_settings("Meiryo", r"C:\Windows\Fonts\meiryo.ttc", "Meiryo"),
        |ui| recommended_ui_font_fixture(ui, "Meiryo", 10.0),
    );
}

#[cfg(windows)]
#[test]
fn recommended_ui_font_meiryo_ui_10pt_alignment_dark() {
    snapshot_with_ui_font(
        "recommended_ui_font_meiryo_ui_10pt_alignment_dark",
        windows_font_settings("Meiryo UI", r"C:\Windows\Fonts\meiryo.ttc", "Meiryo UI"),
        |ui| recommended_ui_font_fixture(ui, "Meiryo UI", 10.0),
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

/// ZIP / PDF / RAR 等の形式バッジが、中央の代替アイコンを変えずに左下へコンパクトに
/// 収まり、ファイル名プレートとも重ならないことを確認する。
#[test]
fn compact_file_format_badges_light() {
    snapshot_with_theme(
        "compact_file_format_badges_light",
        mimageviewer::os_theme::ResolvedTheme::Light,
        |ui| {
            ui.set_width(440.0);
            ui.horizontal(|ui| {
                for (icon, name, kind) in [
                    ("📦", "comic.zip", "ZIP"),
                    ("📄", "document.pdf", "PDF"),
                    ("🗜", "archive.rar", "RAR"),
                ] {
                    let (response, painter) =
                        ui.allocate_painter(egui::vec2(138.0, 180.0), egui::Sense::hover());
                    let inner = response.rect.shrink(4.0);
                    painter.rect_filled(inner, 3.0, egui::Color32::from_gray(228));
                    painter.text(
                        inner.center() - egui::vec2(0.0, 10.0),
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(32.0),
                        egui::Color32::from_gray(70),
                    );
                    mimageviewer::ui_helpers::draw_cell_filename(
                        &painter,
                        inner,
                        name,
                        egui::Color32::from_gray(35),
                        false,
                        mimageviewer::ui_helpers::estimated_file_badge_width(inner),
                    );
                    match kind {
                        "ZIP" => mimageviewer::ui_helpers::draw_zip_badge(&painter, inner),
                        "PDF" => mimageviewer::ui_helpers::draw_pdf_badge(&painter, inner),
                        _ => {
                            mimageviewer::ui_helpers::draw_archive_badge(&painter, inner, kind);
                        }
                    }
                }
            });
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
