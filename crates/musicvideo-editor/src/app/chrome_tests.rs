//! Headless tests use the production frame boundary, never eframe or native audio.
use super::*;
use musicvideo_core::{Animatable, Interp, Keyframe, NormRect, Track};

struct Harness {
    app: MusicVideoLabApp,
    ctx: egui::Context,
    output: egui::FullOutput,
    width: f32,
}

impl Harness {
    fn new(width: f32) -> Self {
        let mut h = Self::unstarted(width);
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn unstarted(width: f32) -> Self {
        Self {
            app: MusicVideoLabApp::new(Arc::new(Assets::default())),
            ctx: crate::project::test_context(),
            output: Default::default(),
            width,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.output = self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(self.width, 1000.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                self.app.process_editor_frame(ctx);
            },
        );
    }

    fn panel(&self, name: &str) -> egui::Rect {
        egui::containers::panel::PanelState::load(&self.ctx, egui::Id::new(name))
            .unwrap()
            .rect
    }

    fn heights(&self) -> (f32, f32) {
        (
            self.panel("musicvideo-controls").height(),
            self.panel("musicvideo-status").height(),
        )
    }

    fn text_pos(&mut self, text: &str) -> egui::Pos2 {
        self.frame(vec![]);
        self.frame(vec![]);
        self.output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().starts_with(text) => {
                    Some(t.pos + t.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing text: {text}"))
    }

    fn click(&mut self, text: &str) {
        let pos = self.text_pos(text);
        self.frame(pointer(pos, true));
        self.frame(pointer(pos, false));
    }
}

fn pending_status_operations(h: &mut Harness, mask: usize) -> (Arc<AtomicBool>, Arc<AtomicBool>) {
    let export_cancel = Arc::new(AtomicBool::new(false));
    let resource_cancel = Arc::new(AtomicBool::new(false));
    if mask & 1 != 0 {
        let (_tx, rx) = mpsc::channel();
        h.app.export_state = ExportState::Running(ExportPending {
            kind: ExportKind::Png,
            cancel: export_cancel.clone(),
            progress: Arc::new(AtomicUsize::new(20)),
            total_frames: 100,
            rx,
        });
    }
    if mask & 2 != 0 {
        let (_tx, rx) = mpsc::channel();
        h.app.audio_pending = Some(AudioPending {
            request: AudioLoadRequest {
                session: h.app.document.id(),
                expected: None,
                path: "audio.wav".into(),
            },
            cancel: resource_cancel.clone(),
            rx,
            queued: None,
        });
    }
    (export_cancel, resource_cancel)
}

fn status_button(h: &Harness, labels: &[&str]) -> egui::Pos2 {
    let status = h.panel("musicvideo-status");
    let (text_rect, text_clip) = h
        .output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if labels.contains(&t.galley.text()) && status.contains(t.pos) => {
                Some((t.galley.rect.translate(t.pos.to_vec2()), s.clip_rect))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing status button {labels:?} at width {}", h.width));
    assert!(
        status.contains_rect(text_rect),
        "button label outside status: {text_rect:?}"
    );
    assert!(
        text_clip.contains_rect(text_rect),
        "button label clipped: {text_rect:?}"
    );
    let (button_rect, button_clip) = h
        .output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Rect(r) if r.rect.contains_rect(text_rect) => Some((r.rect, s.clip_rect)),
            _ => None,
        })
        .min_by(|a, b| a.0.area().total_cmp(&b.0.area()))
        .expect("painted button background");
    assert!(
        status.contains_rect(button_rect),
        "button outside status: {button_rect:?}"
    );
    assert!(
        button_clip.contains_rect(button_rect),
        "button background clipped: rect={button_rect:?}, clip={button_clip:?}, width={}, labels={labels:?}",
        h.width
    );
    text_rect.center()
}

#[test]
fn l3_review_cancel_controls_are_visible_and_clickable_in_narrow_combined_states() {
    let font = crate::project::load_default_font();
    for width in [500.0, 480.0, 520.0, 100.0, 160.0, 250.0, 900.0] {
        for mask in [3, 1, 2] {
            for missing in [true, false] {
                for restoring in [true, false] {
                    for cancel_export in [true, false] {
                        if (cancel_export && mask & 1 == 0) || (!cancel_export && mask & 2 == 0) {
                            continue;
                        }
                        let mut h = Harness::new(width);
                        crate::project::setup_egui(&h.ctx, font.as_ref());
                        let baseline = h.heights();
                        let (export_cancel, resource_cancel) =
                            pending_status_operations(&mut h, mask);
                        if missing {
                            h.app.preview_report.unresolved_assets = vec![
                                UnresolvedAsset::Image(ImageId::new(BACKGROUND_ID)),
                                UnresolvedAsset::Font("missing".into()),
                            ];
                            h.app.audio_error = Some("missing audio".into());
                        }
                        if restoring {
                            h.app.runtime_resources = RuntimeResourceState::Restoring {
                                manifest: ResourceManifest::default(),
                                generation: 1,
                            };
                        }
                        h.app.status = "長い状態通知 ".repeat(100);
                        for _ in 0..3 {
                            h.frame(vec![]);
                        }
                        assert_eq!(h.panel("musicvideo-status").height(), baseline.1);
                        let labels = if cancel_export {
                            ["書き出しをキャンセル", "書出取消", "E"]
                        } else {
                            ["素材読込をキャンセル", "素材取消", "R"]
                        };
                        let pos = status_button(&h, &labels);
                        h.frame(pointer(pos, true));
                        h.frame(pointer(pos, false));
                        assert_eq!(
                            export_cancel.load(Ordering::Relaxed),
                            cancel_export,
                            "export cancellation, width={width}, mask={mask}, missing={missing}, restoring={restoring}"
                        );
                        assert_eq!(
                            resource_cancel.load(Ordering::Relaxed),
                            !cancel_export,
                            "resource cancellation, width={width}, mask={mask}, missing={missing}, restoring={restoring}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn l3_review_long_project_name_never_hides_the_unsaved_marker() {
    let font = crate::project::load_default_font();
    for width in [500.0, 100.0, 160.0, 250.0, 900.0] {
        for mask in 0..=3 {
            let mut h = Harness::new(width);
            crate::project::setup_egui(&h.ctx, font.as_ref());
            h.app.document.replace_with_loaded(
                PathBuf::from(format!("{}.mivmv.json", "長いプロジェクト名".repeat(60))),
                &h.app.project,
            );
            h.app.project.display_gain_db = 4.0;
            h.app.document.note_edit();
            pending_status_operations(&mut h, mask);
            for _ in 0..3 {
                h.frame(vec![]);
            }
            let status = h.panel("musicvideo-status");
            let (rect, clip) = h
                .output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t)
                        if ["（未保存）", "*"].contains(&t.galley.text())
                            && status.contains(t.pos) =>
                    {
                        Some((t.galley.rect.translate(t.pos.to_vec2()), s.clip_rect))
                    }
                    _ => None,
                })
                .expect("independent unsaved marker");
            assert!(status.contains_rect(rect));
            assert!(
                clip.contains_rect(rect),
                "unsaved marker is clipped at {width}pt: rect={rect:?}, clip={clip:?}, mask={mask}"
            );
        }
    }
}

fn titles(output: &egui::FullOutput) -> Vec<&str> {
    output
        .viewport_output
        .values()
        .flat_map(|v| &v.commands)
        .filter_map(|c| {
            if let egui::ViewportCommand::Title(title) = c {
                Some(title.as_str())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn l3_review_title_is_sent_initially_and_only_after_its_text_changes() {
    let mut h = Harness::unstarted(1400.0);
    h.frame(vec![]);
    assert_eq!(titles(&h.output), ["新規（未保存） - mIV 音楽ビデオ"]);
    for position in [0.0, 1.0, 4.0] {
        h.app.position_secs = position;
        h.app.status = format!("Position {position}");
        h.frame(vec![]);
        assert!(titles(&h.output).is_empty());
    }
    h.app
        .document
        .replace_with_loaded("saved.mivmv.json".into(), &h.app.project);
    h.frame(vec![]);
    assert_eq!(titles(&h.output), ["saved.mivmv.json - mIV 音楽ビデオ"]);
    h.frame(vec![]);
    assert!(titles(&h.output).is_empty());
    h.app.project.display_gain_db = 4.0;
    h.app.document.note_edit();
    h.frame(vec![]);
    assert_eq!(
        titles(&h.output),
        ["saved.mivmv.json（未保存） - mIV 音楽ビデオ"]
    );
    h.frame(vec![]);
    assert!(titles(&h.output).is_empty());
    h.app.project.display_gain_db = 0.0;
    h.app.document.note_edit();
    h.frame(vec![]);
    assert_eq!(titles(&h.output), ["saved.mivmv.json - mIV 音楽ビデオ"]);
}

#[test]
fn l3_review_status_details_preserves_hidden_material_actions_and_progress() {
    let mut h = Harness::new(500.0);
    pending_status_operations(&mut h, 3);
    h.app.preview_report.unresolved_assets =
        vec![UnresolvedAsset::Image(ImageId::new(BACKGROUND_ID))];
    h.click("詳細");
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(h.output.shapes.iter().any(
        |s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == "20 / 100 フレーム")
    ));
    h.click("未解決の素材 1 件");
    assert!(h.app.show_missing_resources);
    assert!(
        !h.app
            .export_state
            .pending()
            .unwrap()
            .cancel
            .load(Ordering::Relaxed)
    );
    assert!(
        !h.app
            .audio_pending
            .as_ref()
            .unwrap()
            .cancel
            .load(Ordering::Relaxed)
    );
}

#[test]
fn l3_review_status_details_survives_async_completions_and_keeps_menu_actions() {
    let font = crate::project::load_default_font();
    for width in [500.0, 1400.0] {
        for completed in 1..=7 {
            let mut h = Harness::new(width);
            crate::project::setup_egui(&h.ctx, font.as_ref());
            // A font notice remains unresolved even when an image finishes loading.
            h.app.preview_report.unresolved_assets = vec![UnresolvedAsset::Font("missing".into())];
            let (export_tx, export_rx) = mpsc::channel();
            h.app.export_state = ExportState::Running(ExportPending {
                kind: ExportKind::Png,
                cancel: Arc::new(AtomicBool::new(false)),
                progress: Arc::new(AtomicUsize::new(20)),
                total_frames: 100,
                rx: export_rx,
            });
            let path = PathBuf::from("loaded.png");
            let image = Arc::new(ImageAsset::new(1, 1, vec![2, 3, 4, 255]).unwrap());
            let lease = h
                .app
                .resource_store
                .as_ref()
                .unwrap()
                .store_image(path.clone(), &image)
                .unwrap();
            let (image_tx, image_rx) = mpsc::channel();
            h.app.image_pending = Some(ImagePending {
                request: ImageLoadRequest {
                    session: h.app.document.id(),
                    target: ImageLoadTarget::AddLayer,
                    target_lifetime: None,
                    path: path.clone(),
                },
                cancel: Arc::new(AtomicBool::new(false)),
                rx: image_rx,
                queued: None,
            });
            h.click("詳細");
            h.frame(vec![]);
            assert!(egui::Popup::is_any_open(&h.ctx));
            assert!(h.app.document.is_dirty(&h.app.project));

            // Deliver actual worker messages through the production completion handlers.
            // Loads/export keep update's pre-editor order; saving is polled after input.
            if completed & 1 != 0 {
                export_tx
                    .send(Ok(ExportCompletion::Png(ExportOutcome::Completed {
                        output_dir: "frames".into(),
                        stats: crate::export::RenderStats {
                            frames: 100,
                            width: h.app.project.output.width,
                            height: h.app.project.output.height,
                            fps: h.app.project.output.fps,
                            analysis: Default::default(),
                            composition: Default::default(),
                            total: Default::default(),
                            realtime_ratio: 1.0,
                        },
                    })))
                    .unwrap();
            }
            if completed & 2 != 0 {
                image_tx.send(Ok((path.clone(), image, lease))).unwrap();
            }
            h.app.poll_loads(&h.ctx);
            h.app.poll_export();
            if completed & 4 != 0 {
                let checkpoint = h
                    .app
                    .document
                    .begin_save("saved.mivmv.json".into(), &h.app.project);
                let (tx, rx) = mpsc::channel();
                h.app.project_save_pending = Some(ProjectSavePending::Writing(ProjectSaveWrite {
                    checkpoint,
                    rx,
                }));
                tx.send(Ok(())).unwrap();
            }
            for _ in 0..3 {
                h.frame(vec![]);
            }
            assert_eq!(h.app.export_state.pending().is_none(), completed & 1 != 0);
            assert_eq!(h.app.image_pending.is_none(), completed & 2 != 0);
            assert_eq!(h.app.document.is_dirty(&h.app.project), completed & 4 == 0);
            assert!(
                egui::Popup::is_any_open(&h.ctx),
                "details closed: width={width}, completed={completed}"
            );
            let item = h
                .output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == "未解決の素材 1 件" => {
                        let pos = t.pos + t.galley.rect.center().to_vec2();
                        // At wide widths the status row also has this button. Click the popup's item.
                        h.ctx
                            .layer_id_at(pos)
                            .filter(|layer| layer.order == egui::Order::Foreground)
                            .map(|_| pos)
                    }
                    _ => None,
                })
                .expect("visible unresolved-material action inside the open details popup");
            h.frame(pointer(item, true));
            h.frame(pointer(item, false));
            assert!(
                h.app.show_missing_resources,
                "menu action ignored: width={width}, completed={completed}"
            );
            assert!(!egui::Popup::is_any_open(&h.ctx));
        }
    }
}

fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        modifiers,
        physical_key: None,
        pressed: true,
        repeat: false,
    }
}

fn loaded_project() -> LoadedProject {
    LoadedProject {
        path: PathBuf::from("new.mivmv.json"),
        project: default_project(25.0),
        audio: None,
        waveform: None,
        images: vec![],
        resources: ResourceManifest::default(),
        image_errors: HashMap::new(),
        audio_error: None,
    }
}

fn add_lyrics(h: &mut Harness) -> LayerId {
    let mut layer = musicvideo_core::new_lyrics_layer(&h.app.project);
    let id = layer.id;
    let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
        unreachable!()
    };
    lyrics.lines = vec![LyricLine {
        text: "original".into(),
        time_secs: Some(1.0),
    }];
    h.app.project.layers.push(layer);
    h.app.editor_state.reset_for_project(&h.app.project);
    h.app
        .editor_state
        .select_layer(&h.app.project, h.app.project.layers.len() - 1);
    id
}

#[test]
fn p4b3_admission_blocks_lrc_loading_ready_and_real_lyrics_row_confirmation() {
    let mut h = Harness::unstarted(1400.0);
    h.app = super::tests::h3a_app();
    let manifest = h.app.editor_state.resource_manifest().clone();
    let id = add_lyrics(&mut h);
    let LayerKind::Lyrics(lyrics) = &mut h.app.project.layers.last_mut().unwrap().kind else {
        unreachable!()
    };
    lyrics.lines = vec![
        LyricLine {
            text: "first".into(),
            time_secs: Some(0.1),
        },
        LyricLine {
            text: "second".into(),
            time_secs: Some(0.2),
        },
    ];
    h.app
        .editor_state
        .reset_for_project_with_resources(&h.app.project, manifest.clone());
    h.app
        .editor_state
        .select_layer(&h.app.project, h.app.project.layers.len() - 1);
    h.app.runtime_resources = RuntimeResourceState::Ready(manifest);
    h.app.lrc_load = ready_lrc(&h.app, id);
    assert!(h.app.mp4_export_disabled_reason().unwrap().contains("読込"));
    let (_tx, rx) = mpsc::channel();
    h.app.lrc_load = LrcLoadState::Loading {
        owner: lrc_owner(&h.app, id),
        rx,
    };
    assert!(h.app.mp4_export_disabled_reason().unwrap().contains("読込"));
    h.app.lrc_load = LrcLoadState::Idle;
    h.click("first\nsecond");
    let focused = h
        .ctx
        .memory(|memory| memory.focused())
        .expect("lyrics text focused");
    let mut native = egui::text_edit::TextEditState::load(&h.ctx, focused).unwrap();
    native
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(5),
            egui::text::CCursor::new(6),
        )));
    native.store(&h.ctx, focused);
    h.frame(vec![]);
    h.frame(vec![egui::Event::Cut]);
    assert!(h.app.editor_state.has_pending_lyrics_edit());
    assert!(
        h.app
            .mp4_export_disabled_reason()
            .unwrap()
            .contains("行対応")
    );
    assert_eq!(h.app.current_lyrics(id).unwrap().len(), 2);
}

fn lrc_owner(app: &MusicVideoLabApp, id: LayerId) -> LrcLoadOwner {
    LrcLoadOwner {
        session: app.document.id(),
        layer: id,
        target_lifetime: app.editor_state.layer_target_lifetime(id).unwrap(),
        baseline: app.current_lyrics(id).unwrap(),
    }
}

fn ready_lrc(app: &MusicVideoLabApp, id: LayerId) -> LrcLoadState {
    LrcLoadState::Ready {
        owner: lrc_owner(app, id),
        path: PathBuf::from("new.lrc"),
        report: parse_lrc(b"[00:02.00]loaded\n").unwrap(),
    }
}

#[test]
fn l3_chrome_heights_are_invariant_across_all_host_states_and_widths() {
    for width in [900.0, 1093.0, 1400.0, 2000.0] {
        for state in 0..11 {
            let mut h = Harness::new(width);
            let baseline = h.heights();
            match state {
                0 => {}
                1 => {
                    h.app.project.display_gain_db = 3.0;
                    h.app.document.note_edit();
                }
                2 => {
                    let (_tx, rx) = mpsc::channel();
                    h.app.audio_pending = Some(AudioPending {
                        request: AudioLoadRequest {
                            session: h.app.document.id(),
                            expected: None,
                            path: PathBuf::from("audio.wav"),
                        },
                        cancel: Arc::new(AtomicBool::new(false)),
                        rx,
                        queued: None,
                    });
                }
                3 => {
                    h.app.preview_report.unresolved_assets = vec![
                        UnresolvedAsset::Image(ImageId::new(BACKGROUND_ID)),
                        UnresolvedAsset::Font("missing".into()),
                    ];
                    h.app.audio_error = Some("missing audio".into());
                    h.app.project.images.extend((10..50).map(|id| ImageRef {
                        id: ImageId::new(id.to_string()),
                        path: PathBuf::from(format!("image-{id}.png")),
                    }));
                }
                4 => {
                    let (_tx, rx) = mpsc::channel();
                    h.app.export_state = ExportState::Running(ExportPending {
                        kind: ExportKind::Png,
                        cancel: Arc::new(AtomicBool::new(false)),
                        progress: Arc::new(AtomicUsize::new(20)),
                        total_frames: 100,
                        rx,
                    });
                }
                5 => {
                    h.app.last_export = Some(ExportOutcome::Completed {
                        output_dir: "output".into(),
                        stats: crate::export::RenderStats {
                            frames: 100,
                            width: 320,
                            height: 180,
                            fps: 30.0,
                            analysis: Default::default(),
                            composition: Default::default(),
                            total: Default::default(),
                            realtime_ratio: 1.0,
                        },
                    })
                }
                6 => h.app.preview_perf.push(PreviewPerfSample {
                    display_interval_ms: 16.0,
                    ..Default::default()
                }),
                7 => h.app.project_load = ProjectLoadState::Ready(loaded_project()),
                8 => {
                    let id = add_lyrics(&mut h);
                    h.app.lrc_load = ready_lrc(&h.app, id);
                }
                9 => {
                    h.app.runtime_resources = RuntimeResourceState::Unresolved {
                        manifest: ResourceManifest::default(),
                        error: "restore failed".repeat(50),
                    }
                }
                10 => {
                    h.app.runtime_resources = RuntimeResourceState::Restoring {
                        manifest: ResourceManifest::default(),
                        generation: 1,
                    }
                }
                _ => unreachable!(),
            }
            h.app.status = "長い状態通知 ".repeat(100);
            for _ in 0..3 {
                h.frame(vec![]);
                assert_eq!(h.heights(), baseline, "width {width}, state {state}");
            }
            assert_eq!(baseline.1, 30.0);
            assert_eq!(h.panel("musicvideo-status").bottom(), 1000.0);
            assert!(h.panel("musicvideo-timeline").bottom() <= h.panel("musicvideo-status").top());
        }
    }
}

#[test]
fn l3_play_pause_button_and_seek_digits_do_not_change_toolbar_geometry() {
    let ctx = crate::project::test_context();
    let font = crate::project::load_default_font();
    crate::project::setup_egui(&ctx, font.as_ref());
    let app = MusicVideoLabApp::new(Arc::new(Assets::default()));
    let mut toolbar = ToolbarState::from_app(&app);
    for width in [900.0, 1093.0, 1400.0, 2000.0] {
        let mut baseline = None;
        for (playing, position) in [(false, 0.0), (true, 123.45), (false, 36000.0)] {
            toolbar.playing = playing;
            toolbar.position_secs = position;
            toolbar.duration_secs = 36000.0;
            for _ in 0..2 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1000.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        let height = egui::TopBottomPanel::top("toolbar")
                            .show(ctx, |ui| {
                                ui.horizontal_wrapped(|ui| toolbar.draw(ui, &mut vec![]));
                            })
                            .response
                            .rect
                            .height();
                        if let Some(expected) = baseline {
                            assert_eq!(height, expected);
                        } else {
                            baseline = Some(height);
                        }
                    },
                );
            }
        }
    }
}

#[test]
fn l3_project_modal_switch_and_continue_use_the_original_transitions() {
    for switch in [false, true] {
        let mut h = Harness::new(1400.0);
        h.app.project.display_gain_db = 7.0;
        h.app.project_load = ProjectLoadState::Ready(loaded_project());
        h.click(if switch {
            "読込結果へ切り替える"
        } else {
            "現在の編集を続ける"
        });
        assert!(matches!(h.app.project_load, ProjectLoadState::Idle));
        if switch {
            assert_eq!(h.app.project.duration_secs, 25.0);
            assert_eq!(
                h.app.document.path(),
                Some(std::path::Path::new("new.mivmv.json"))
            );
        } else {
            assert_eq!(h.app.project.display_gain_db, 7.0);
            assert!(h.app.document.path().is_none());
        }
    }
}

#[test]
fn l3_lrc_modal_replace_and_keep_use_the_original_transitions() {
    for replace in [false, true] {
        let mut h = Harness::new(1400.0);
        let id = add_lyrics(&mut h);
        h.app.lrc_load = ready_lrc(&h.app, id);
        h.click(if replace {
            "LRC の内容へ置換"
        } else {
            "現在の歌詞を保持"
        });
        assert!(matches!(h.app.lrc_load, LrcLoadState::Idle));
        assert_eq!(
            h.app.current_lyrics(id).unwrap()[0].text,
            if replace { "loaded" } else { "original" }
        );
    }
}

#[test]
fn l3_modal_blocks_active_properties_drag_and_history_on_its_first_frame() {
    for project_modal in [false, true] {
        let mut h = Harness::new(1400.0);
        let id = add_lyrics(&mut h);
        h.app.editor_state.set_inspector_tab(InspectorTab::Project);
        let pos = h.text_pos("表示ゲイン");
        h.frame(pointer(pos, true));
        if project_modal {
            h.app.project_load = ProjectLoadState::Ready(loaded_project());
        } else {
            h.app.lrc_load = ready_lrc(&h.app, id);
        }
        let project = h.app.project.clone();
        let generation = h.app.document.edit_generation();
        h.frame(vec![
            egui::Event::PointerMoved(pos + egui::vec2(60.0, 0.0)),
            key(egui::Key::Z, egui::Modifiers::CTRL),
        ]);
        assert_eq!(h.app.project, project);
        assert_eq!(h.app.document.edit_generation(), generation);
        h.frame(pointer(pos + egui::vec2(60.0, 0.0), false));
        assert_eq!(h.app.project, project);
    }
}

#[test]
fn l3_modal_blocks_focused_lyrics_text_input() {
    for project_modal in [false, true] {
        let mut h = Harness::new(1400.0);
        let id = add_lyrics(&mut h);
        h.click("original");
        if project_modal {
            h.app.project_load = ProjectLoadState::Ready(loaded_project());
        } else {
            h.app.lrc_load = ready_lrc(&h.app, id);
        }
        let project = h.app.project.clone();
        h.frame(vec![
            key(
                egui::Key::A,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            ),
            egui::Event::Text("blocked".into()),
        ]);
        assert_eq!(h.app.project, project);
    }
}

#[test]
fn l3_modal_blocks_active_timeline_key_drag() {
    for project_modal in [false, true] {
        let mut h = Harness::new(1400.0);
        let id = add_lyrics(&mut h);
        h.app.project.layers[0].rect = Animatable::Track(
            Track::new(vec![Keyframe {
                t: 2.0,
                value: NormRect::new(0.0, 0.0, 0.5, 0.5),
                interp: Interp::Linear,
            }])
            .unwrap(),
        );
        h.frame(vec![]);
        h.frame(vec![]);
        // The diamond is painted as a closed four-point path.
        let pos = h
            .output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Path(p)
                    if p.closed
                        && p.points.len() == 4
                        && p.points[0].y > h.panel("musicvideo-timeline").top() =>
                {
                    Some((p.points[0] + p.points[2].to_vec2()) * 0.5)
                }
                _ => None,
            })
            .expect("timeline key marker");
        h.frame(pointer(pos, true));
        if project_modal {
            h.app.project_load = ProjectLoadState::Ready(loaded_project());
        } else {
            h.app.lrc_load = ready_lrc(&h.app, id);
        }
        let project = h.app.project.clone();
        h.frame(vec![egui::Event::PointerMoved(
            pos + egui::vec2(100.0, 0.0),
        )]);
        h.frame(pointer(pos + egui::vec2(100.0, 0.0), false));
        assert_eq!(h.app.project, project);
    }
}

#[test]
fn l3_diagnostics_default_closed_and_opening_preserves_chrome_height_and_data() {
    let mut h = Harness::new(1400.0);
    assert!(!h.app.show_diagnostics);
    let heights = h.heights();
    h.app.preview_perf.push(PreviewPerfSample {
        display_interval_ms: 16.0,
        ..Default::default()
    });
    h.app.last_export = Some(ExportOutcome::Cancelled {
        output_dir: "output".into(),
        completed_frames: 42,
    });
    h.click("診断");
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(h.app.show_diagnostics);
    assert_eq!(h.heights(), heights);
    for text in [
        "プレビュー実測",
        "worker:",
        "UI:",
        "表示間隔−worker",
        "完了済み: 42",
        "シーク時は",
        "合成 ",
        "計測をリセット",
    ] {
        assert!(
            h.output.shapes.iter().any(
                |s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text().starts_with(text))
            ),
            "missing diagnostic: {text}"
        );
    }
}

#[test]
fn l3_missing_window_reports_the_original_image_replacement_request() {
    let mut h = Harness::new(1400.0);
    let id = ImageId::new(BACKGROUND_ID);
    h.app.preview_report.unresolved_assets = vec![UnresolvedAsset::Image(id.clone())];
    h.click("未解決の素材 1 件");
    let pos = h.text_pos("画像を差し替える");
    // Render the production window, inspect the request before its native file dialog.
    let mut actions = Vec::new();
    for pressed in [true, false] {
        let _ = h.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(h.width, 1000.0),
                )),
                events: pointer(pos, pressed),
                ..Default::default()
            },
            |ctx| h.app.draw_tool_windows(ctx, true, &mut actions),
        );
    }
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, ChromeAction::ReplaceImage(request) if request == &id))
    );
}

#[test]
fn l3_completion_during_project_modal_preserves_lrc_payload_and_admission_order() {
    let mut h = Harness::new(1400.0);
    let id = add_lyrics(&mut h);
    h.app.project_load = ProjectLoadState::Ready(loaded_project());
    let owner = lrc_owner(&h.app, id);
    let (tx, rx) = mpsc::channel();
    h.app.lrc_load = LrcLoadState::Loading { owner, rx };
    // A conflicting LRC arrives while the Project decision is being shown.
    let LayerKind::Lyrics(lyrics) = &mut h.app.project.layers.last_mut().unwrap().kind else {
        unreachable!()
    };
    lyrics.lines[0].text = "edited".into();
    tx.send(Ok((
        "new.lrc".into(),
        parse_lrc(b"[00:02.00]loaded\n").unwrap(),
    )))
    .unwrap();
    h.frame(vec![]);
    assert!(matches!(h.app.lrc_load, LrcLoadState::Ready { .. }));
    h.click("現在の編集を続ける");
    assert!(matches!(h.app.lrc_load, LrcLoadState::Ready { .. }));
    assert_eq!(h.app.current_lyrics(id).unwrap()[0].text, "edited");
    h.click("LRC の内容へ置換");
    assert_eq!(h.app.current_lyrics(id).unwrap()[0].text, "loaded");
}

#[test]
fn l3_project_completion_during_lrc_modal_and_switch_invalidates_the_old_lrc_owner() {
    let mut h = Harness::new(1400.0);
    let id = add_lyrics(&mut h);
    h.app.lrc_load = ready_lrc(&h.app, id);
    let (tx, rx) = mpsc::channel();
    h.app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
        started_at: h.app.document.edit_generation(),
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        queued: None,
    });
    tx.send(Ok(loaded_project())).unwrap();
    h.frame(vec![]);
    // H1 already regards an active LRC as a pending edit, even without a generation change.
    assert!(matches!(h.app.project_load, ProjectLoadState::Ready(_)));
    assert!(matches!(h.app.lrc_load, LrcLoadState::Ready { .. }));
    h.click("読込結果へ切り替える");
    assert_eq!(h.app.project.duration_secs, 25.0);
    assert!(matches!(h.app.lrc_load, LrcLoadState::Idle));
}

#[test]
fn l3_modal_decision_is_applied_before_same_frame_project_completion_admission() {
    let mut h = Harness::new(1400.0);
    let id = add_lyrics(&mut h);
    h.app.lrc_load = ready_lrc(&h.app, id);
    let (tx, rx) = mpsc::channel();
    h.app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
        started_at: h.app.document.edit_generation(),
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        queued: None,
    });
    let pos = h.text_pos("LRC の内容へ置換");
    h.frame(pointer(pos, true));
    tx.send(Ok(loaded_project())).unwrap();
    h.frame(pointer(pos, false));
    // LRC confirmation edits the generation before H1 considers the loaded Project.
    assert!(matches!(h.app.project_load, ProjectLoadState::Ready(_)));
    assert!(matches!(h.app.lrc_load, LrcLoadState::Idle));
    assert_eq!(h.app.current_lyrics(id).unwrap()[0].text, "loaded");
    assert_ne!(h.app.project.duration_secs, 25.0);
}

#[test]
fn l3_discard_lrc_decision_precedes_same_frame_unchanged_project_admission() {
    let mut h = Harness::new(1400.0);
    let id = add_lyrics(&mut h);
    h.app.lrc_load = ready_lrc(&h.app, id);
    let LayerKind::Lyrics(lyrics) = &mut h.app.project.layers.last_mut().unwrap().kind else {
        unreachable!()
    };
    lyrics.lines[0].text = "edited".into();
    h.app.document.note_edit();
    let (tx, rx) = mpsc::channel();
    h.app.project_load = ProjectLoadState::Loading(ProjectLoadPending {
        started_at: h.app.document.edit_generation(),
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        queued: None,
    });
    let pos = h.text_pos("現在の歌詞を保持");
    h.frame(pointer(pos, true));
    tx.send(Ok(loaded_project())).unwrap();
    h.frame(pointer(pos, false));
    assert!(matches!(h.app.lrc_load, LrcLoadState::Idle));
    // Discard does not mutate the document; after it there is no pending LRC edit.
    assert!(matches!(h.app.project_load, ProjectLoadState::Idle));
    assert_eq!(h.app.project.duration_secs, 25.0);
}

#[test]
fn l3_window_title_tracks_the_project_name_and_dirty_suffix() {
    let mut h = Harness::new(1400.0);
    h.app
        .document
        .replace_with_loaded(PathBuf::from("saved.mivmv.json"), &h.app.project);
    h.frame(vec![]);
    assert!(h.output.viewport_output.values().flat_map(|v| &v.commands).any(|c| matches!(c, egui::ViewportCommand::Title(title) if title == "saved.mivmv.json - mIV 音楽ビデオ")));
    h.app.project.display_gain_db = 4.0;
    h.app.document.note_edit();
    h.frame(vec![]);
    assert!(h.output.viewport_output.values().flat_map(|v| &v.commands).any(|c| matches!(c, egui::ViewportCommand::Title(title) if title == "saved.mivmv.json（未保存） - mIV 音楽ビデオ")));
}

#[test]
fn l3_image_completion_during_modal_keeps_the_existing_session_and_lifetime_checks() {
    for project_modal in [false, true] {
        let mut h = Harness::new(1400.0);
        h.app.project.images.push(ImageRef {
            id: ImageId::new(BACKGROUND_ID),
            path: "old.png".into(),
        });
        let lyric_id = add_lyrics(&mut h);
        if project_modal {
            h.app.project_load = ProjectLoadState::Ready(loaded_project());
        } else {
            h.app.lrc_load = ready_lrc(&h.app, lyric_id);
        }
        let id = ImageId::new(BACKGROUND_ID);
        let path = PathBuf::from("loaded.png");
        let image = Arc::new(ImageAsset::new(1, 1, vec![2, 3, 4, 255]).unwrap());
        let lease = h
            .app
            .resource_store
            .as_ref()
            .unwrap()
            .store_image(path.clone(), &image)
            .unwrap();
        let revision = lease.revision();
        let (tx, rx) = mpsc::channel();
        h.app.image_pending = Some(ImagePending {
            request: ImageLoadRequest {
                session: h.app.document.id(),
                target: ImageLoadTarget::Replace(id.clone()),
                target_lifetime: h.app.editor_state.image_target_lifetime(&id),
                path: path.clone(),
            },
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            queued: None,
        });
        tx.send(Ok((path.clone(), image, lease))).unwrap();
        // Keep update's pre-editor resource completion order, with the actual admission code.
        h.app.poll_loads(&h.ctx);
        h.frame(vec![]);
        assert!(h.app.load_choice_open());
        assert!(h.app.image_pending.is_none());
        assert_eq!(
            h.app
                .editor_state
                .resource_manifest()
                .image(&id)
                .unwrap()
                .revision(),
            revision
        );
        assert_eq!(
            h.app
                .project
                .images
                .iter()
                .find(|image| image.id == id)
                .unwrap()
                .path,
            path
        );
        h.click(if project_modal {
            "現在の編集を続ける"
        } else {
            "現在の歌詞を保持"
        });
        assert_eq!(
            h.app
                .editor_state
                .resource_manifest()
                .image(&id)
                .unwrap()
                .revision(),
            revision
        );
    }
}

#[test]
fn l3_restore_completion_during_either_modal_keeps_the_decision_payload() {
    for project_modal in [false, true] {
        let mut h = Harness::new(1400.0);
        let id = add_lyrics(&mut h);
        if project_modal {
            h.app.project_load = ProjectLoadState::Ready(loaded_project());
        } else {
            h.app.lrc_load = ready_lrc(&h.app, id);
        }
        let project = h.app.project.clone();
        let manifest = h.app.editor_state.resource_manifest().clone();
        h.app.runtime_resources = RuntimeResourceState::Restoring {
            manifest: manifest.clone(),
            generation: 1,
        };
        h.app.apply_resource_restore_completion(RestoreCompletion {
            session: h.app.document.id(),
            generation: 1,
            manifest: manifest.clone(),
            result: Ok((
                RestoredResources {
                    manifest,
                    audio: None,
                    images: vec![],
                },
                None,
            )),
        });
        h.frame(vec![]);
        assert!(h.app.runtime_resources_ready());
        assert!(h.app.load_choice_open());
        assert_eq!(h.app.project, project);
        h.click(if project_modal {
            "現在の編集を続ける"
        } else {
            "現在の歌詞を保持"
        });
        assert_eq!(h.app.project, project);
    }
}
