//! Edited clipboard output owns a source/edit snapshot, never viewer state or a DB writer.
//! Decode, composition, matte conversion, and clipboard I/O all run on the worker.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use eframe::egui;

use crate::app::{App, LocalAiActivityLease};
use crate::books::{BakedEditSnapshot, CompositeSource};
use crate::export_dialog::{DecodedExportComposite, ExportRenderError};
use crate::ui_dialogs::context_menu::{
    ClipboardImageWriteResult, EditedClipboardWriteRequest, request_edited_clipboard_write,
    write_edited_rgba_to_clipboard,
};

pub(crate) struct EditedClipboardPage {
    pub source: CompositeSource,
    pub edits: BakedEditSnapshot,
}

pub(crate) enum EditedClipboardPages {
    Single(Box<EditedClipboardPage>),
    Spread {
        left: Box<EditedClipboardPage>,
        right: Box<EditedClipboardPage>,
    },
}

impl EditedClipboardPages {
    fn first(&self) -> &EditedClipboardPage {
        match self {
            Self::Single(page) | Self::Spread { left: page, .. } => page,
        }
    }

    fn expects_ai(&self) -> bool {
        match self {
            Self::Single(page) => page.edits.ai.is_some(),
            Self::Spread { left, right } => left.edits.ai.is_some() || right.edits.ai.is_some(),
        }
    }
}

pub(crate) struct EditedClipboardCopyPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<ClipboardImageWriteResult>,
}

impl Drop for EditedClipboardCopyPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// Kept separate from the real renderer/writer so worker lifetime and wake contracts
/// can be tested without touching the OS clipboard or starting an application.
fn spawn_worker(
    ctx: egui::Context,
    lease: LocalAiActivityLease,
    work: impl FnOnce(&Arc<AtomicBool>) -> ClipboardImageWriteResult + Send + 'static,
) -> Result<EditedClipboardCopyPending, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("clipboard-edited-copy".into())
        .spawn(move || {
            let _lease = lease;
            let result = work(&worker_cancel);
            let _ = tx.send(result);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        })
        .map_err(|error| format!("画像コピーの処理を開始できませんでした: {error}"))?;
    Ok(EditedClipboardCopyPending { cancel, rx })
}

fn render_pages(
    pages: &EditedClipboardPages,
    cancel: &Arc<AtomicBool>,
    raw: Option<&crate::raw::RawDecodeContext>,
) -> Result<egui::ColorImage, ExportRenderError> {
    fn decode<'a>(
        page: &'a EditedClipboardPage,
        cancel: &Arc<AtomicBool>,
        raw: Option<&crate::raw::RawDecodeContext>,
    ) -> Result<crate::export_dialog::DecodedExportPage<'a>, ExportRenderError> {
        crate::export_dialog::decode_output_page(&page.source, &page.edits, 4096, cancel, raw)
    }
    if cancel.load(Ordering::Acquire) {
        return Err(ExportRenderError::Cancelled);
    }
    let decoded = match pages {
        EditedClipboardPages::Single(page) => {
            DecodedExportComposite::Single(decode(page, cancel, raw)?)
        }
        EditedClipboardPages::Spread { left, right } => DecodedExportComposite::Spread {
            left: decode(left, cancel, raw)?,
            right: decode(right, cancel, raw)?,
        },
    };
    // Exactly Ctrl+E's `_0` renderer, including composition order and spread joining.
    crate::export_dialog::render_export_composite_owned(decoded, None, None, cancel)
}

fn flattened_rgba(image: &egui::ColorImage, matte: crate::capture::JpegMatte) -> Vec<u8> {
    let rgba = crate::capture::color_image_to_rgba(image);
    let rgb = crate::capture::flatten_rgba_to_rgb(&rgba, image.size[0] as u32, matte);
    rgb.as_chunks::<3>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
        .collect()
}

fn run_copy(
    pages: &EditedClipboardPages,
    raw: &crate::raw::RawDecodeContext,
    request: EditedClipboardWriteRequest,
    cancel: &Arc<AtomicBool>,
) -> ClipboardImageWriteResult {
    let started = std::time::Instant::now();
    let mut size = None;
    let result = match render_pages(pages, cancel, Some(raw)) {
        Ok(image) => {
            size = Some(image.size);
            if cancel.load(Ordering::Acquire) {
                ClipboardImageWriteResult::Superseded
            } else {
                let rgba = flattened_rgba(&image, pages.first().edits.jpeg_matte);
                if cancel.load(Ordering::Acquire) {
                    ClipboardImageWriteResult::Superseded
                } else {
                    write_edited_rgba_to_clipboard(
                        image.size[0] as u32,
                        image.size[1] as u32,
                        &rgba,
                        request,
                    )
                }
            }
        }
        Err(ExportRenderError::Cancelled) => ClipboardImageWriteResult::Superseded,
        Err(ExportRenderError::Failed(message)) => ClipboardImageWriteResult::Failed(message),
    };
    if let ClipboardImageWriteResult::Failed(message) = &result {
        crate::logger::log(format!("Edited clipboard copy failed: {message}"));
    }
    if crate::perf::is_enabled() {
        crate::perf::event(
            "clipboard",
            "edited_copy_complete",
            None,
            request.sequence(),
            &[
                (
                    "ms",
                    serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                ),
                ("size", serde_json::json!(size)),
                (
                    "stage",
                    serde_json::json!(format!("{:?}", pages.first().edits.stage)),
                ),
                ("outcome", serde_json::json!(format!("{result:?}"))),
            ],
        );
    }
    result
}

impl App {
    pub(crate) fn request_edited_clipboard_copy(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        fullscreen: bool,
    ) {
        let started = std::time::Instant::now();
        let pages = match self.prepare_edited_clipboard_pages(idx, fullscreen) {
            Ok(pages) => pages,
            Err(message) => {
                crate::logger::log(format!("Edited clipboard snapshot failed: {message}"));
                self.show_feedback_error(message);
                return;
            }
        };
        let request = request_edited_clipboard_write();
        // Dropping the old receiver prevents its terminal result from becoming a toast.
        self.edited_clipboard_copy = None;
        if pages.expects_ai() {
            self.show_feedback_toast("編集を反映した画像を準備しています…".to_string());
        }
        if crate::perf::is_enabled() {
            crate::perf::event(
                "clipboard",
                "edited_copy_request",
                None,
                request.sequence(),
                &[
                    (
                        "snapshot_ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    (
                        "stage",
                        serde_json::json!(format!("{:?}", pages.first().edits.stage)),
                    ),
                    (
                        "spread",
                        serde_json::json!(matches!(pages, EditedClipboardPages::Spread { .. })),
                    ),
                ],
            );
        }
        let raw = crate::raw::RawDecodeContext::new(
            Arc::clone(&self.raw_develop_executor),
            self.settings.raw_brightness,
        );
        match spawn_worker(ctx.clone(), self.local_ai_activity_lease(), move |cancel| {
            run_copy(&pages, &raw, request, cancel)
        }) {
            Ok(pending) => self.edited_clipboard_copy = Some(pending),
            Err(message) => {
                crate::logger::log(&message);
                self.show_feedback_error(message);
            }
        }
    }

    pub(crate) fn poll_edited_clipboard_copy(&mut self) {
        let Some(pending) = &self.edited_clipboard_copy else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                let message = "画像コピーの処理が中断されました".to_string();
                crate::logger::log(&message);
                ClipboardImageWriteResult::Failed(message)
            }
        };
        self.edited_clipboard_copy = None;
        match result {
            ClipboardImageWriteResult::Written => {
                self.show_feedback_toast("編集を反映した画像をコピーしました".to_string());
            }
            ClipboardImageWriteResult::Superseded => {}
            ClipboardImageWriteResult::ClipboardChanged => self.show_feedback_warning(
                "コピーの準備中にクリップボードが変わったため、クリップボードを書き換えませんでした"
                    .to_string(),
            ),
            ClipboardImageWriteResult::Failed(message) => self
                .show_feedback_error(format!("画像をコピーできませんでした: {message}")),
        }
    }
}

#[cfg(test)]
#[path = "edited_clipboard_copy_tests.rs"]
mod tests;

#[cfg(test)]
mod worker_tests {
    use super::*;
    use crate::bake_stage::BakeStage;
    use crate::export_dialog::{ExportComposite, ExportPageComposite};
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn page(path: &Path, stage: BakeStage, canonical: bool) -> EditedClipboardPage {
        let params = crate::adjustment::AdjustParams {
            brightness: 8.0,
            post_filter: crate::adjustment::PostFilter::Sepia,
            ..Default::default()
        };
        let mut bubble = comic_core::BubbleObject {
            shape: comic_core::BubbleShape::RoundRect {
                half_w: 1.0,
                half_h: 1.0,
                corner_px: 0.0,
            },
            fill: Some(comic_core::Rgba::new(255, 235, 59, 255)),
            text: comic_core::TextBlock::default(),
            auto_size: false,
            ..Default::default()
        };
        bubble.outline.width_px = 0.0;
        EditedClipboardPage {
            source: CompositeSource::File { path: path.into() },
            edits: BakedEditSnapshot {
                params,
                rotation: crate::rotation_db::Rotation::Cw90,
                conceal: None,
                erase: None,
                local_adjust: None,
                comic: Some(crate::books::BookComicSnapshot {
                    objects: vec![comic_core::AnnotationObject::new_bubble(
                        1,
                        (4.0, 2.0),
                        bubble,
                    )],
                    fonts: Arc::new(comic_core::FontSet::new()),
                    stamp_cache: Default::default(),
                }),
                comic_source_dims: canonical.then_some([8, 4]),
                export_crop: Some(crate::export_crop::CropSettings {
                    rect: crate::export_crop::CropRect {
                        min_x: 1.0,
                        min_y: 0.0,
                        max_x: 7.0,
                        max_y: 4.0,
                    },
                    aspect_mode: crate::export_crop::CropAspectMode::Free,
                    source_size: canonical.then_some([8, 4]),
                }),
                crop_legacy_writeback: None,
                format: crate::capture::CaptureFormat::Png,
                jpeg_matte: crate::capture::JpegMatte::White,
                stage,
                creative_lut: None,
                ai: Some(crate::books::BookAiSnapshot {
                    run: Box::new(|image, _| {
                        let [w, h] = image.size;
                        Ok(crate::books::BookAiResult {
                            image: egui::ColorImage::new(
                                [w * 2, h * 2],
                                (0..h * 2)
                                    .flat_map(|y| {
                                        (0..w * 2).map(move |x| image.pixels[(y / 2) * w + x / 2])
                                    })
                                    .collect(),
                            ),
                            used_upscale: true,
                        })
                    }),
                }),
            },
        }
    }

    fn export_page(page: EditedClipboardPage) -> ExportPageComposite {
        ExportPageComposite {
            source: page.source,
            edits: page.edits,
            pdf_render_long_edge: 4096,
            predicted_size: [8, 4],
            has_conceal_mask: false,
        }
    }

    #[test]
    fn edited_clipboard_pixels_match_ctrl_e_and_batch_for_each_stage_single_and_spread() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.png");
        image::RgbaImage::from_fn(8, 4, |x, y| {
            image::Rgba([50 + x as u8 * 10, 70 + y as u8 * 15, 140, 255])
        })
        .save(&path)
        .unwrap();
        let original = std::fs::read(&path).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut stage_pixels = Vec::new();
        for stage in BakeStage::ALL {
            for canonical in [false, true] {
                let single = EditedClipboardPages::Single(Box::new(page(&path, stage, canonical)));
                let actual = render_pages(&single, &cancel, None).unwrap();
                let ctrl_e = ExportComposite::Single(export_page(page(&path, stage, canonical)));
                let expected = crate::export_dialog::render_export_composite_owned(
                    crate::export_dialog::decode_export_composite(&ctrl_e, &cancel, None).unwrap(),
                    None,
                    None,
                    &cancel,
                )
                .unwrap();
                assert_eq!(actual, expected, "Ctrl+E {stage:?}, canonical={canonical}");
                // This is the materialized batch output before file encoding/scaling.
                let batch_page = page(&path, stage, canonical);
                let base = crate::books::decode_composite_source_for_materialization(
                    &batch_page.source,
                    4096,
                    Arc::clone(&cancel),
                    None,
                )
                .unwrap();
                let batch = crate::books::compose_export_entry(
                    base,
                    &batch_page.edits,
                    None,
                    None,
                    Arc::clone(&cancel),
                )
                .unwrap();
                assert_eq!(actual, batch, "batch {stage:?}, canonical={canonical}");
                let spread = EditedClipboardPages::Spread {
                    left: Box::new(page(&path, stage, canonical)),
                    right: Box::new(page(&path, stage, canonical)),
                };
                let actual_spread = render_pages(&spread, &cancel, None).unwrap();
                let ctrl_e_spread = ExportComposite::Spread {
                    left: export_page(page(&path, stage, canonical)),
                    right: export_page(page(&path, stage, canonical)),
                };
                let expected_spread = crate::export_dialog::render_export_composite_owned(
                    crate::export_dialog::decode_export_composite(&ctrl_e_spread, &cancel, None)
                        .unwrap(),
                    None,
                    None,
                    &cancel,
                )
                .unwrap();
                assert_eq!(actual_spread, expected_spread);
                assert_eq!(
                    actual_spread,
                    crate::capture::combine_spread_color_images(&batch, &batch).unwrap()
                );
                if canonical {
                    stage_pixels.push(actual);
                }
            }
        }
        assert_ne!(
            stage_pixels[0], stage_pixels[1],
            "AI stage changes resolution"
        );
        assert_ne!(
            stage_pixels[1], stage_pixels[2],
            "display stage applies sepia"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            1,
            "no sidecar or output files"
        );
    }

    #[test]
    fn edited_clipboard_matte_matches_jpeg_for_transparent_and_partial_alpha() {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [16, 1],
            &[[90, 130, 180, 0], [90, 130, 180, 128]].repeat(8).concat(),
        );
        for matte in [
            crate::capture::JpegMatte::Black,
            crate::capture::JpegMatte::White,
            crate::capture::JpegMatte::Checker,
        ] {
            let actual = flattened_rgba(&image, matte);
            let jpeg = crate::capture::flatten_rgba_to_rgb(
                &crate::capture::color_image_to_rgba(&image),
                16,
                matte,
            );
            for (index, pixel) in actual.as_chunks::<4>().0.iter().enumerate() {
                assert_eq!(&pixel[..3], &jpeg[index * 3..index * 3 + 3]);
                assert_eq!(pixel[3], 255);
                if index % 2 == 0 {
                    assert_eq!(&pixel[..3], &matte.color_at(index as u32, 0));
                }
            }
        }
    }

    fn idle_context() -> egui::Context {
        let ctx = egui::Context::default();
        for _ in 0..3 {
            ctx.run_pass(Default::default(), |_| {})
                .drop_without_applying_deltas();
        }
        ctx
    }

    #[test]
    fn edited_clipboard_edits_only_worker_holds_lease_until_cancelled_worker_really_exits() {
        let count = Arc::new(AtomicUsize::new(0));
        let lease = LocalAiActivityLease::new(Arc::clone(&count));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (exited_tx, exited_rx) = mpsc::channel();
        let ctx = idle_context();
        let (wake_tx, wake_rx) = mpsc::channel();
        ctx.set_request_repaint_callback(move |event| {
            let _ = wake_tx.send(event.viewport_id);
        });
        let pending = spawn_worker(ctx, lease, move |cancel| {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(cancel.load(Ordering::Acquire));
            exited_tx.send(()).unwrap();
            ClipboardImageWriteResult::Superseded
        })
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(count.load(Ordering::Acquire), 1);
        drop(pending);
        assert_eq!(
            count.load(Ordering::Acquire),
            1,
            "receiver cancellation cannot drop the worker's lease"
        );
        release_tx.send(()).unwrap();
        exited_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            wake_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            egui::ViewportId::ROOT
        );
        // The repaint callback runs inside the terminal lease interval.
    }

    #[test]
    fn edited_clipboard_decode_failure_wakes_root_and_returns_an_error() {
        let ctx = idle_context();
        let (wake_tx, wake_rx) = mpsc::channel();
        ctx.set_request_repaint_callback(move |event| {
            let _ = wake_tx.send(event.viewport_id);
        });
        let count = Arc::new(AtomicUsize::new(0));
        let raw = crate::raw::RawDecodeContext::new(
            Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()),
            crate::raw::RawBrightness::None,
        );
        let tmp = tempfile::tempdir().unwrap();
        let pages = EditedClipboardPages::Single(Box::new(page(
            &tmp.path().join("missing.png"),
            BakeStage::Edits,
            false,
        )));
        let request = request_edited_clipboard_write();
        let pending = spawn_worker(ctx, LocalAiActivityLease::new(count), move |cancel| {
            run_copy(&pages, &raw, request, cancel)
        })
        .unwrap();
        assert!(matches!(
            pending.rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            ClipboardImageWriteResult::Failed(_)
        ));
        assert_eq!(
            wake_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            egui::ViewportId::ROOT
        );
    }

    #[test]
    fn edited_clipboard_new_request_cancels_old_job_and_discards_its_result() {
        let mut app = crate::app::setup_app_for_test();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let pending = spawn_worker(
            idle_context(),
            app.local_ai_activity_lease(),
            move |cancel| {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                assert!(cancel.load(Ordering::Acquire));
                finished_tx.send(()).unwrap();
                ClipboardImageWriteResult::Written
            },
        )
        .unwrap();
        let old_cancel = Arc::clone(&pending.cancel);
        app.edited_clipboard_copy = Some(pending);
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        app.items = vec![crate::grid_item::GridItem::Image(
            app.tmp.path().join("missing.png"),
        )];
        app.request_edited_clipboard_copy(&idle_context(), 0, false);
        assert!(old_cancel.load(Ordering::Acquire));
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let result = app
            .edited_clipboard_copy
            .as_ref()
            .unwrap()
            .rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(
            matches!(result, ClipboardImageWriteResult::Failed(_)),
            "only the newer decode failure reaches the current receiver"
        );
    }

    #[test]
    fn edited_clipboard_poll_reports_os_change_and_failures_but_silences_superseded_results() {
        use crate::feedback::FeedbackKind;
        for (result, expected) in [
            (
                ClipboardImageWriteResult::Written,
                Some(("編集を反映した画像をコピーしました", FeedbackKind::Info)),
            ),
            (
                ClipboardImageWriteResult::ClipboardChanged,
                Some((
                    "コピーの準備中にクリップボードが変わったため、クリップボードを書き換えませんでした",
                    FeedbackKind::Warning,
                )),
            ),
            (
                ClipboardImageWriteResult::Failed("decode failure".into()),
                Some((
                    "画像をコピーできませんでした: decode failure",
                    FeedbackKind::Error,
                )),
            ),
            (ClipboardImageWriteResult::Superseded, None),
        ] {
            let mut app = crate::app::setup_app_for_test();
            let (tx, rx) = mpsc::channel();
            tx.send(result).unwrap();
            app.edited_clipboard_copy = Some(EditedClipboardCopyPending {
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
            });
            app.feedback_toasts_for_test.clear();
            app.poll_edited_clipboard_copy();
            assert!(app.edited_clipboard_copy.is_none());
            let expected: Vec<_> = expected
                .into_iter()
                .map(|(text, kind)| (text.to_string(), kind))
                .collect();
            assert_eq!(app.feedback_toasts_for_test, expected);
        }
    }
}
