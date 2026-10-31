//! バージョン情報ダイアログ。

use crate::app::App;
use eframe::egui;

const EGUI_LICENSE_MIT: &str = include_str!("../../vendor/egui-wgpu/LICENSE-MIT");
const EGUI_LICENSE_APACHE: &str = include_str!("../../vendor/egui-wgpu/LICENSE-APACHE");
const LIBAVIF_LICENSE: &str = include_str!("../../LIBAVIF-LICENSE.txt");
const DAV1D_LICENSE: &str = include_str!("../../DAV1D-LICENSE.txt");
const LIBYUV_LICENSE: &str = include_str!("../../LIBYUV-LICENSE.txt");
const LIBRAW_LICENSE: &str = include_str!("../../LIBRAW-LICENSE.txt");
const ZLIB_LICENSE: &str = include_str!("../../ZLIB-LICENSE.txt");
const RUST_DEFLATE_NOTICES: &str = include_str!("../../RUST-DEFLATE-NOTICES.txt");
const EVERYTHING_IPC_NOTICE: &str = include_str!("../../EVERYTHING-IPC-NOTICE.txt");
const LIBJPEG_TURBO_LICENSE: &str = include_str!("../../LIBJPEG-TURBO-LICENSE.txt");
// HarmNet の推論と重みは core に埋め込むため、portable を含む全構成で表示する。
const HARMNET_NOTICE: (&str, &str, &str) = (
    "EffeTune Note Spectrogram (HarmNet L)",
    "MIT — © 2025-2026 Yoshiyuki Kobayashi",
    include_str!("../../third_party/effetune-harmnet/LICENSE"),
);
const IJG_ATTRIBUTION: &str =
    "This software is based in part on the work of the Independent JPEG Group.";

fn draw_license_text(ui: &mut egui::Ui, title: &str, id: &str, text: &str) {
    egui::CollapsingHeader::new(title)
        .id_salt(id)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: egui::scroll_area::DragScroll::Always,
                    ..Default::default()
                })
                .id_salt(format!("{id}_scroll"))
                .max_height(180.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).monospace().size(10.0)).wrap(),
                    );
                });
        });
}

/// The same bundled-version/source notice used by the dialog and headless snapshots.
#[doc(hidden)]
pub fn draw_raw_license_snapshot_fixture(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("LibRaw (CDDL-1.0)").strong());
    ui.label("本ソフトウェアは LibRaw を CDDL-1.0 で使用しています。");
    ui.label(format!("同梱バージョン: {}", env!("MIV_LIBRAW_BUILD_ID")));
    ui.hyperlink_to(
        "対応するソースコード",
        format!(
            "https://mikage.to/mimageviewer/libraw-{}-source.tar.gz",
            env!("MIV_LIBRAW_BUILD_ID")
        ),
    );
    ui.hyperlink_to("LibRaw プロジェクト", "https://www.libraw.org/");
    ui.add(egui::Label::new(IJG_ATTRIBUTION).wrap());
    ui.label("ライセンス全文と著作権表記は以下から確認できます。");
    draw_license_text(
        ui,
        "LibRaw ライセンス・著作権表記 全文",
        "about_libraw_license",
        LIBRAW_LICENSE,
    );
    draw_license_text(ui, "zlib License 全文", "about_zlib_license", ZLIB_LICENSE);
    draw_license_text(
        ui,
        "libjpeg-turbo ライセンス・著作権表記 全文",
        "about_libjpeg_turbo_license",
        LIBJPEG_TURBO_LICENSE,
    );
}

/// Bundled AVIF versions and full notices, shared by the dialog and headless UI tests.
#[doc(hidden)]
pub fn draw_avif_license_snapshot_fixture(ui: &mut egui::Ui) {
    ui.label(format!(
        "AVIF 同梱版: {}",
        crate::avif_decoder::BUILD_ID.trim()
    ));
    draw_license_text(
        ui,
        "libavif ライセンス・著作権表記 全文",
        "about_libavif_license",
        LIBAVIF_LICENSE,
    );
    draw_license_text(
        ui,
        "dav1d ライセンス・著作権表記 全文",
        "about_dav1d_license",
        DAV1D_LICENSE,
    );
    draw_license_text(
        ui,
        "libyuv ライセンス・著作権表記 全文",
        "about_libyuv_license",
        LIBYUV_LICENSE,
    );
}

// portable は Mixwright bundle / notices を同梱しない。HarmNet は上の共通 notice。
#[cfg(not(feature = "portable"))]
const EFFETUNE_NOTICES: &[(&str, &str, &str)] = &[
    (
        "EffeTune Mixwright THIRD-PARTY-NOTICES 全文",
        "Contents/Resources/THIRD-PARTY-NOTICES.txt",
        include_str!(
            "../../third_party/effetune-mixwright/v0.15.0/Contents/Resources/THIRD-PARTY-NOTICES.txt"
        ),
    ),
    (
        "EffeTune WebView THIRD-PARTY-NOTICES 全文",
        "Contents/Resources/webview/THIRD-PARTY-NOTICES.txt",
        include_str!(
            "../../third_party/effetune-mixwright/v0.15.0/Contents/Resources/webview/THIRD-PARTY-NOTICES.txt"
        ),
    ),
    (
        "EffeTune DSP NOTICE 全文",
        "Contents/Resources/webview/plugins/dsp/NOTICE.txt",
        include_str!(
            "../../third_party/effetune-mixwright/v0.15.0/Contents/Resources/webview/plugins/dsp/NOTICE.txt"
        ),
    ),
];

impl App {
    pub(crate) fn show_about_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_about_dialog {
            return;
        }
        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        // 追加パックのライセンスサマリは closure の借用衝突を避けるため事前にキャプチャする。
        let pack_about = self.ensure_editing_pack_about();
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        egui::Window::new("バージョン情報")
            .drag_area(egui::WindowDrag::Anywhere)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .vscroll(true)
            .drag_to_scroll(egui::scroll_area::DragScroll::Always)
            .default_width(620.0)
            .max_height((ctx.content_rect().height() - 80.0).max(320.0))
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                #[cfg(test)]
                ctx.data_mut(|data| {
                    data.insert_temp(
                        egui::Id::unique("about_outer_scroll_geometry"),
                        (ui.clip_rect(), ui.cursor().min.y),
                    )
                });
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.heading("mImageViewer");
                    ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(8.0);
                    ui.label("© 2025 Mikage Sawatari");
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(4.0);

                // サードパーティライセンス
                ui.label(egui::RichText::new("サードパーティ ライセンス").strong());
                ui.add_space(4.0);
                egui::Grid::new("third_party_licenses")
                    .num_columns(2)
                    .spacing([8.0, 2.0])
                    .show(ui, |ui| {
                        ui.label("ONNX Runtime");
                        ui.label("MIT — Microsoft");
                        ui.end_row();

                        ui.label("FFmpeg");
                        ui.label("LGPLv3-or-later — FFmpeg project");
                        ui.end_row();

                        ui.label("AVIF (libavif / dav1d / libyuv)");
                        ui.label("BSD ? AOMedia / VideoLAN / libyuv authors");
                        ui.end_row();
                        ui.label("LibRaw");
                        ui.label("CDDL-1.0 — LibRaw LLC");
                        ui.end_row();

                        ui.label("zlib 1.3.1");
                        ui.label("zlib — Jean-loup Gailly and Mark Adler");
                        ui.end_row();

                        ui.label("libjpeg-turbo");
                        ui.label("IJG / BSD-3-Clause / zlib");
                        ui.end_row();

                        ui.label("UnRAR");
                        ui.label("UnRAR license — Alexander Roshal / RARLAB");
                        ui.end_row();

                        ui.label(HARMNET_NOTICE.0);
                        ui.label(HARMNET_NOTICE.1);
                        ui.end_row();

                        #[cfg(not(feature = "portable"))]
                        {
                            ui.label("EffeTune Mixwright");
                            ui.label("MIT — © 2025-2026 Yoshiyuki Kobayashi");
                            ui.end_row();

                            ui.label("Steinberg VST3 SDK");
                            ui.label("MIT — Steinberg Media Technologies GmbH");
                            ui.end_row();
                        }

                        ui.label("eframe / egui");
                        ui.label("MIT OR Apache-2.0 — Emil Ernerfeldt and contributors");
                        ui.end_row();

                        ui.label("Real-ESRGAN");
                        ui.label("BSD-3-Clause — Xintao");
                        ui.end_row();

                        ui.label("Real-CUGAN");
                        ui.label("MIT — bilibili");
                        ui.end_row();

                        ui.label("NVIDIA Image Scaling");
                        ui.label("MIT — NVIDIA CORPORATION & AFFILIATES");
                        ui.end_row();

                        ui.label("Anime4K");
                        ui.label("MIT — bloc97");
                        ui.end_row();

                        ui.label("4x-NMKD-Siax-200k");
                        ui.label("WTFPL — Nmkd");
                        ui.end_row();

                        ui.label("MI-GAN");
                        ui.label("MIT");
                        ui.end_row();

                        ui.label("1xDeJPG_realplksr_otf");
                        ui.label("CC-BY-4.0 — Phhofm");
                        ui.end_row();

                        ui.label("絵文字 (Twemoji)");
                        ui.label("CC-BY 4.0 — Twitter, Inc. and other contributors");
                        ui.end_row();
                    });

                // FFmpeg は LGPL のライブラリを DLL として同梱・再配布しているので、
                // ライセンス名だけでなく「どのビルドか」と「対応ソースの入手先」を
                // アプリ内から辿れるようにする (docs/ffmpeg-lgpl-source-distribution.md
                // の Notice Template)。配布経路によっては (GitHub Releases など)
                // 製品ページを経由しないため、サイト側の記載だけでは導線が切れる。
                ui.add_space(8.0);
                ui.label(egui::RichText::new("FFmpeg (LGPLv3-or-later)").strong());
                ui.add_space(2.0);
                ui.label(
                    "本ソフトウェアは FFmpeg プロジェクトのライブラリを LGPLv3-or-later で\
                     使用しています。",
                );
                ui.label(format!("同梱バージョン: {}", env!("MIV_FFMPEG_BUILD_ID")));
                ui.hyperlink_to(
                    "対応するソースコード",
                    format!(
                        "https://mikage.to/mimageviewer/ffmpeg-{}-source.tar.gz",
                        env!("MIV_FFMPEG_BUILD_ID")
                    ),
                );
                ui.hyperlink_to("FFmpeg プロジェクト", "https://ffmpeg.org/");
                ui.hyperlink_to(
                    "LGPL-3.0 ライセンス全文",
                    "https://www.gnu.org/licenses/lgpl-3.0.html",
                );

                ui.add_space(8.0);
                draw_raw_license_snapshot_fixture(ui);
                ui.separator();
                draw_avif_license_snapshot_fixture(ui);

                draw_license_text(
                    ui,
                    "Everything IPC 定義 (MIT) 全文",
                    "about_everything_ipc_notice",
                    EVERYTHING_IPC_NOTICE,
                );
                draw_license_text(
                    ui,
                    &format!("{} — MIT License 全文", HARMNET_NOTICE.0),
                    "about_harmnet_license",
                    HARMNET_NOTICE.2,
                );

                ui.add_space(6.0);
                draw_license_text(
                    ui,
                    "egui MIT License 全文",
                    "about_egui_mit_license",
                    EGUI_LICENSE_MIT,
                );
                draw_license_text(
                    ui,
                    "egui Apache License 2.0 全文",
                    "about_egui_apache_license",
                    EGUI_LICENSE_APACHE,
                );

                draw_license_text(
                    ui,
                    "Rust DEFLATE (MIT) licenses / copyright notices",
                    "about_rust_deflate_notices",
                    RUST_DEFLATE_NOTICES,
                );

                #[cfg(not(feature = "portable"))]
                for &(title, relative_path, text) in EFFETUNE_NOTICES {
                    egui::CollapsingHeader::new(title)
                        .id_salt(("about_effetune_notice", relative_path))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .scroll_source(egui::scroll_area::ScrollSource {
                                    drag: egui::scroll_area::DragScroll::Always,
                                    ..Default::default()
                                })
                                .id_salt(("about_effetune_notice_scroll", relative_path))
                                .max_height(180.0)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(text).monospace().size(10.0),
                                        )
                                        .wrap(),
                                    );
                                });
                        });
                }

                // 編集用追加パック (導入済みのときだけ表示、spec §10)。
                if let Some(pack) = &pack_about {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("編集用追加パック (v{})", pack.version))
                            .strong(),
                    );
                    ui.add_space(4.0);
                    egui::Grid::new("editing_pack_licenses")
                        .num_columns(2)
                        .spacing([8.0, 2.0])
                        .show(ui, |ui| {
                            ui.label(format!("オノマトペ向けフォント ({}書体)", pack.font_count));
                            ui.label(format!("{} — Google Fonts 提供", pack.font_license));
                            ui.end_row();

                            ui.label(format!("被写体分離 ({})", pack.model_id));
                            ui.label(format!("{} — ZhengPeng7/BiRefNet", pack.model_license));
                            ui.end_row();
                        });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("各ライセンス全文は追加パック内に同梱されています。")
                            .size(10.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                }

                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    if ui.button("閉じる").clicked() {
                        self.show_about_dialog = false;
                    }
                });
            });
        if !open || escape_pressed {
            self.show_about_dialog = false;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn egui037_about_builtin_scrollarea_mouse_drag_changes_offset() {
        use eframe::egui;
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.show_about_dialog = true;
        let draw = |app: &mut crate::app::App, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 500.0),
                    )),
                    events,
                    ..Default::default()
                },
                |_| app.show_about_dialog_window(&ctx),
            )
            .drop_without_applying_deltas();
        };
        for _ in 0..4 {
            draw(&mut app, vec![]);
        }
        let geometry = || {
            ctx.data(|data| {
                data.get_temp::<(egui::Rect, f32)>(egui::Id::unique("about_outer_scroll_geometry"))
                    .unwrap()
            })
        };
        let (clip, before) = geometry();
        let start = egui::pos2(clip.right() - 28.0, clip.center().y);
        let end = start - egui::vec2(0.0, 75.0);
        draw(&mut app, vec![egui::Event::PointerMoved(start)]);
        draw(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        draw(&mut app, vec![egui::Event::PointerMoved(end)]);
        draw(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        draw(&mut app, vec![]);
        let (after_clip, after) = geometry();
        assert!(
            (after_clip.min - clip.min).length() < 1.0,
            "drag scroll must retain the Window position"
        );
        assert!(
            before - after > 20.0,
            "actual built-in ScrollArea offset must advance on mouse drag: before={before}, after={after}"
        );
    }

    use super::{
        DAV1D_LICENSE, EGUI_LICENSE_APACHE, EGUI_LICENSE_MIT, IJG_ATTRIBUTION, LIBAVIF_LICENSE,
        LIBJPEG_TURBO_LICENSE, LIBRAW_LICENSE, LIBYUV_LICENSE, ZLIB_LICENSE,
    };

    #[test]
    fn embedded_raw_notices_equal_tracked_files_with_bsd_and_ijg_attribution() {
        for (file, embedded) in [
            ("LIBRAW-LICENSE.txt", LIBRAW_LICENSE),
            ("LIBAVIF-LICENSE.txt", LIBAVIF_LICENSE),
            ("DAV1D-LICENSE.txt", DAV1D_LICENSE),
            ("LIBYUV-LICENSE.txt", LIBYUV_LICENSE),
            ("ZLIB-LICENSE.txt", ZLIB_LICENSE),
            ("LIBJPEG-TURBO-LICENSE.txt", LIBJPEG_TURBO_LICENSE),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
            assert_eq!(embedded, std::fs::read_to_string(path).unwrap(), "{file}");
        }
        assert!(LIBRAW_LICENSE.contains("COMMON DEVELOPMENT AND DISTRIBUTION LICENSE"));
        assert!(LIBRAW_LICENSE.contains("DCB and FBDD are Copyright (C) 2010,  Jacek Gozdz"));
        assert!(LIBRAW_LICENSE.contains("Redistributions in binary form must reproduce the above"));
        assert!(LIBRAW_LICENSE.contains("THIS SOFTWARE IS PROVIDED BY ROLAND KARLSSON"));
        assert!(ZLIB_LICENSE.contains("1995-2022 Jean-loup Gailly and Mark Adler"));
        assert!(ZLIB_LICENSE.contains("This notice may not be removed or altered"));
        assert!(LIBJPEG_TURBO_LICENSE.contains(IJG_ATTRIBUTION));
        assert!(LIBJPEG_TURBO_LICENSE.contains("Copyright (C)2009-2024 D. R. Commander."));
        assert!(LIBJPEG_TURBO_LICENSE.contains("Redistributions in binary form must reproduce"));
        assert!(LIBJPEG_TURBO_LICENSE.contains("LEGAL ISSUES"));
    }

    #[test]
    fn egui_license_texts_are_embedded_with_attribution() {
        assert!(EGUI_LICENSE_MIT.contains("Copyright (c) 2018-2021 Emil Ernerfeldt"));
        assert!(EGUI_LICENSE_APACHE.contains("Apache License"));
        assert!(EGUI_LICENSE_APACHE.contains("Version 2.0, January 2004"));
    }

    // cfg を付けず、通常構成と portable 構成の両方で同じ登録・本文を検査する。
    #[test]
    fn harmnet_notice_is_registered_with_embedded_mit_license_in_every_build_flavor() {
        use super::HARMNET_NOTICE;
        assert_eq!(HARMNET_NOTICE.0, "EffeTune Note Spectrogram (HarmNet L)");
        assert!(HARMNET_NOTICE.1.contains("MIT"));
        let license = HARMNET_NOTICE.2;
        assert!(license.contains("Copyright (c) 2025-2026, Yoshiyuki Kobayashi"));
        assert!(license.contains("Permission is hereby granted, free of charge"));
        assert!(license.contains("THE SOFTWARE IS PROVIDED \"AS IS\""));
        let tracked = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("third_party/effetune-harmnet/LICENSE"),
        )
        .expect("HarmNet MIT license must be present");
        assert_eq!(license, tracked);
    }

    #[test]
    fn harmnet_notice_is_drawn_and_full_license_can_be_opened_in_every_build_flavor() {
        use egui_kittest::kittest::Queryable;

        let mut app = crate::app::setup_app_for_test();
        app.show_about_dialog = true;
        // Isolate the optional pack from the fixture; the dialog uses its normal
        // rendering path, including the notice grid and collapsing license text.
        app.editing_pack_about_loaded = true;
        app.editing_pack_about = None;
        let mut fonts_ready = false;
        let mut harness = crate::test_ui::builder()
            .with_size(eframe::egui::vec2(900.0, 2000.0))
            .build_ui_state(
                move |root_ui, app: &mut crate::app::AppTestEnvForTest| {
                    let ctx = &root_ui.ctx().clone();
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ctx);
                        fonts_ready = true;
                        ctx.request_repaint();
                        return;
                    }
                    app.show_about_dialog_window(ctx);
                },
                app,
            );
        harness.run();
        harness.get_by_label("EffeTune Note Spectrogram (HarmNet L)");
        assert!(
            harness.query_by_label(super::HARMNET_NOTICE.2).is_none(),
            "the full text starts collapsed"
        );
        harness
            .get_by_label("EffeTune Note Spectrogram (HarmNet L) — MIT License 全文")
            .click_accesskit();
        harness.run();
        harness.get_by_label(super::HARMNET_NOTICE.2);
    }

    #[cfg(not(feature = "portable"))]
    #[test]
    fn effetune_notice_texts_include_attribution_and_match_vendor_when_present() {
        use super::EFFETUNE_NOTICES;
        let main_notice = EFFETUNE_NOTICES[0].2;
        assert!(main_notice.contains("Copyright (c) 2025-2026, Yoshiyuki Kobayashi"));
        assert!(main_notice.contains("Steinberg Media Technologies GmbH"));
        assert!(main_notice.contains("MIT License"));
        assert!(EFFETUNE_NOTICES[2].2.contains("PFFFT"));
        assert!(
            EFFETUNE_NOTICES[2]
                .2
                .contains("fdlibm 5.3 (atan and atan2)")
        );
        assert!(
            EFFETUNE_NOTICES[2]
                .2
                .contains("Copyright (C) 1993 by Sun Microsystems, Inc.")
        );
        assert_eq!(EFFETUNE_NOTICES.len(), 3);
        let upstream = EFFETUNE_NOTICES[0].2;
        for attribution in [
            "lie v3.3.0",
            "Calvin Metcalf, Jordan Harband",
            "immediate v3.0.6",
            "Brian Cavalier",
            "setImmediate v1.0.5",
            "Donavon West, and Domenic Denicola",
            "pako v1.0.11 - zlib-derived code",
            "Jean-loup Gailly and Mark Adler",
            "Vitaly Puzrin and Andrey Tupitsin",
        ] {
            assert!(upstream.contains(attribution), "missing: {attribution}");
        }
        for &(_, relative_path, embedded) in EFFETUNE_NOTICES {
            let tracked = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("third_party/effetune-mixwright/v0.15.0")
                    .join(relative_path),
            )
            .expect("Approved EffeTune notice must be present");
            assert_eq!(
                embedded.as_bytes(),
                tracked,
                "notice differs: {relative_path}"
            );
        }
        for attribution in ["Emscripten", "University of Illinois", "musl", "Node.js"] {
            assert!(
                EFFETUNE_NOTICES[2].2.contains(attribution),
                "missing: {attribution}"
            );
        }

        let vendor = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("vendor/effetune-mixwright/EffeTune Mixwright.vst3");
        if !vendor.exists() {
            return;
        }
        let vendor_version = std::fs::read_to_string(vendor.parent().unwrap().join("VERSION"))
            .expect("EffeTune vendor VERSION must be present");
        assert_eq!(vendor_version.trim(), "v0.15.0");
        for &(_, relative_path, embedded) in EFFETUNE_NOTICES {
            let source = std::fs::read(vendor.join(relative_path))
                .expect("EffeTune vendor notice must be present");
            assert_eq!(
                embedded.as_bytes(),
                source,
                "notice differs: {relative_path}"
            );
        }
    }
}
