use super::*;
use std::sync::{Mutex, atomic::AtomicUsize};

fn item(id: usize) -> Box<BatchExportItem> {
    Box::new(BatchExportItem::new(
        format!("source{id}"),
        "pictures".into(),
        crate::books::CompositeSource::File {
            path: format!("source{id}.png").into(),
        },
        crate::books::BakedEditSnapshot {
            params: Default::default(),
            rotation: crate::rotation_db::Rotation::None,
            conceal: None,
            erase: None,
            local_adjust: None,
            comic: None,
            comic_source_dims: None,
            export_crop: None,
            crop_legacy_writeback: None,
            format: crate::capture::CaptureFormat::Png,
            jpeg_matte: crate::capture::JpegMatte::Black,
            stage: Default::default(),
            creative_lut: None,
            ai: None,
        },
    ))
}

fn spec(count: usize) -> RenderSpec {
    RenderSpec {
        options: CombineOptions {
            layout: CombineLayout::Row,
            ..Default::default()
        },
        items: (0..count)
            .map(|id| {
                (
                    id,
                    CombineItemSpec {
                        annotation_space: None,
                        source: CombineSourceRef::File {
                            path: format!("source{id}.png").into(),
                        },
                        label_override: None,
                    },
                )
            })
            .collect(),
    }
}

fn worker(load: Materializer) -> Worker {
    Worker {
        items: HashMap::new(),
        load,
        analyze_pdf: Box::new(|_, _, _, _| panic!("unexpected PDF analysis")),
        cancel: Arc::new(AtomicBool::new(false)),
        small_edge: 256,
        fonts: Some(None),
        before_encode: None,
        before_write: None,
    }
}

fn pixels() -> LoadedPixels {
    LoadedPixels {
        image: RgbaImage::from_pixel(8, 6, image::Rgba([30, 60, 90, 255])),
        decode_ms: 0.0,
        compose_ms: 0.0,
        guard: None,
    }
}

fn load_into(worker: &mut Worker, count: usize) {
    for id in 0..count {
        let item = item(id);
        let pixels = (worker.load)(&item, &worker.cancel).unwrap();
        worker.items.insert(
            id,
            LoadedItem {
                item,
                dims: pixels.image.dimensions(),
                small: pixels.image.clone(),
            },
        );
    }
}

#[test]
fn image_combine_worker_coalesces_latest_generation_and_remove_drops_snapshot() {
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    let (tx, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    tx.send(Command::Prepare {
        id: 0,
        item: item(0),
    })
    .unwrap();
    for generation in 1..=7 {
        tx.send(Command::Render {
            generation,
            spec: spec(1),
        })
        .unwrap();
    }
    drop(tx);
    worker.run(rx, events, egui::Context::default());
    let results = results.try_iter().collect::<Vec<_>>();
    assert!(matches!(results[0], Event::Loaded { id: 0, .. }));
    assert_eq!(
        results
            .iter()
            .filter(|event| matches!(event, Event::Rendered { .. }))
            .count(),
        1
    );
    assert!(matches!(
        results.last().unwrap(),
        Event::Rendered { generation: 7, .. }
    ));
    let (tx, rx) = mpsc::channel();
    let (events, _) = mpsc::channel();
    tx.send(Command::Remove { ids: vec![0] }).unwrap();
    drop(tx);
    worker.run(rx, events, egui::Context::default());
    assert!(worker.items.is_empty());
}

#[test]
fn image_combine_worker_reuses_snapshots_and_holds_one_full_image() {
    let live = Arc::new(AtomicUsize::new(0));
    let identities = Arc::new(Mutex::new(Vec::new()));
    let captured = identities.clone();
    let count = live.clone();
    let mut worker = worker(Box::new(move |item, _| {
        assert_eq!(
            count.fetch_add(1, Ordering::SeqCst),
            0,
            "full sources overlap"
        );
        captured
            .lock()
            .unwrap()
            .push((&item.edits as *const _ as usize, item.filename.clone()));
        let mut pixels = pixels();
        pixels.guard = Some(ImageCountGuard(count.clone()));
        Ok(pixels)
    }));
    load_into(&mut worker, 3);
    let directory = tempfile::tempdir().unwrap();
    let (progress, rx) = mpsc::channel();
    let write = WriteSpec {
        render: spec(3),
        directory: directory.path().into(),
        name: "joined".into(),
        replace: None,
        data_dir: directory.path().join("data"),
        progress,
    };
    let path = worker.write(&write).unwrap().unwrap();
    assert_eq!(image::image_dimensions(path).unwrap(), (24, 6));
    assert_eq!(live.load(Ordering::SeqCst), 0);
    let identities = identities.lock().unwrap();
    assert_eq!(&identities[..3], &identities[3..]);
    assert_eq!(
        rx.try_iter()
            .filter(|event| matches!(event, ExportEvent::Step { .. }))
            .count(),
        3
    );
    assert!(!directory.path().join("combine.db").exists());
}

#[test]
fn image_combine_worker_cancel_checkpoints_create_no_file() {
    for checkpoint in 0..3 {
        let mut worker = worker(Box::new(move |_, cancel| {
            if checkpoint == 0 {
                cancel.store(true, Ordering::Relaxed);
                return Err("cancelled during compose".into());
            }
            Ok(pixels())
        }));
        // Preview was already loaded before cancellation.
        worker.items.insert(
            0,
            LoadedItem {
                item: item(0),
                dims: (8, 6),
                small: pixels().image,
            },
        );
        if checkpoint == 1 {
            worker.before_encode = Some(Box::new(|cancel| cancel.store(true, Ordering::Relaxed)));
        }
        if checkpoint == 2 {
            worker.before_write = Some(Box::new(|cancel| cancel.store(true, Ordering::Relaxed)));
        }
        let directory = tempfile::tempdir().unwrap();
        let (progress, _) = mpsc::channel();
        let write = WriteSpec {
            render: spec(1),
            directory: directory.path().into(),
            name: "joined".into(),
            replace: None,
            data_dir: directory.path().join("data"),
            progress,
        };
        assert!(worker.write(&write).unwrap().is_none());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn image_combine_worker_dimension_change_fails_whole_write() {
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    load_into(&mut worker, 1);
    worker.items.get_mut(&0).unwrap().dims = (9, 6);
    let directory = tempfile::tempdir().unwrap();
    let (progress, _) = mpsc::channel();
    let error = worker
        .write(&WriteSpec {
            render: spec(1),
            directory: directory.path().into(),
            name: "joined".into(),
            replace: None,
            data_dir: directory.path().join("data"),
            progress,
        })
        .unwrap_err();
    assert!(error.contains("元の画像が変わりました"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn image_combine_worker_cancel_after_create_finishes_and_never_overwrites() {
    let directory = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    std::fs::write(directory.path().join("joined.png"), b"original").unwrap();
    let result = write_new_with(directory.path(), "joined", "png", &cancel, |file| {
        cancel.store(true, Ordering::Relaxed);
        file.write_all(b"complete")
    })
    .unwrap()
    .unwrap();
    assert_eq!(result.file_name().unwrap(), "joined_1.png");
    assert_eq!(std::fs::read(result).unwrap(), b"complete");
    assert_eq!(
        std::fs::read(directory.path().join("joined.png")).unwrap(),
        b"original"
    );
}

#[test]
fn image_combine_worker_write_protocol_emits_one_output_and_terminal() {
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    load_into(&mut worker, 2);
    let directory = tempfile::tempdir().unwrap();
    let (progress, results) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let (events, _) = mpsc::channel();
    tx.send(Command::Write(WriteSpec {
        render: spec(2),
        directory: directory.path().into(),
        name: "joined".into(),
        replace: None,
        data_dir: directory.path().join("data"),
        progress,
    }))
    .unwrap();
    drop(tx);
    worker.run(rx, events, egui::Context::default());
    let results = results.try_iter().collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|e| matches!(e, ExportEvent::Completed(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|e| matches!(e, ExportEvent::AllDone))
            .count(),
        1
    );
    assert!(matches!(results.last(), Some(ExportEvent::AllDone)));
    let recipe = super::super::store::load_recipe(
        &directory.path().join("data"),
        &directory.path().join("joined.png"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(recipe.v, 1);
    assert_eq!(recipe.items.len(), 2);
    assert!(super::super::store::source_path(&recipe.items[0].source).is_absolute());
}

#[test]
fn image_combine_worker_queued_cancel_sends_terminal_and_font_failure_blocks_labels_only() {
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    load_into(&mut worker, 1);
    let mut labelled = spec(1);
    labelled.options.labels.placement = LabelPlacement::Overlay;
    assert!(
        worker
            .preview(&labelled)
            .unwrap_err()
            .contains("ラベルのフォント")
    );
    assert!(worker.preview(&spec(1)).is_ok());
    let directory = tempfile::tempdir().unwrap();
    let (progress, results) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let (events, _) = mpsc::channel();
    tx.send(Command::Render {
        generation: 2,
        spec: spec(1),
    })
    .unwrap();
    tx.send(Command::Write(WriteSpec {
        render: spec(1),
        directory: directory.path().into(),
        name: "joined".into(),
        replace: None,
        data_dir: directory.path().join("data"),
        progress,
    }))
    .unwrap();
    drop(tx);
    worker.cancel.store(true, Ordering::Relaxed);
    worker.run(rx, events, egui::Context::default());
    let results = results.try_iter().collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], ExportEvent::Cancelled));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn image_combine_worker_recipe_only_failure_keeps_output_and_completion_order() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("not-a-directory");
    std::fs::write(&data, b"keep this user file").unwrap();
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    load_into(&mut worker, 1);
    let (progress, results) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let (events, _) = mpsc::channel();
    tx.send(Command::Write(WriteSpec {
        render: spec(1),
        directory: directory.path().into(),
        name: "joined".into(),
        replace: None,
        data_dir: data.clone(),
        progress,
    }))
    .unwrap();
    drop(tx);
    worker.run(rx, events, egui::Context::default());
    let results = results.try_iter().collect::<Vec<_>>();
    assert!(
        matches!(&results[..], [ExportEvent::Step { .. }, ExportEvent::Completed(_), ExportEvent::Failed(failure), ExportEvent::AllDone]
        if failure.label == "構成の保存" && failure.message.contains("やり直しは使えません"))
    );
    assert_eq!(
        image::image_dimensions(directory.path().join("joined.png")).unwrap(),
        (8, 6)
    );
    assert_eq!(std::fs::read(data).unwrap(), b"keep this user file");
}

#[test]
fn image_combine_worker_replace_protects_sources_and_advances_previous_second() {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.png");
    let output = directory.path().join("joined.png");
    std::fs::write(&source, b"source identity").unwrap();
    std::fs::write(&output, b"old output").unwrap();
    let previous = UNIX_EPOCH + Duration::from_secs(4_000_000_000) + Duration::from_millis(700);
    std::fs::File::options()
        .write(true)
        .open(&output)
        .unwrap()
        .set_modified(previous)
        .unwrap();
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    load_into(&mut worker, 1);
    let mut render = spec(1);
    render.items[0].1.source = CombineSourceRef::File {
        path: source.clone(),
    };
    let (progress, _) = mpsc::channel();
    let mut write = WriteSpec {
        render,
        directory: directory.path().into(),
        name: "ignored".into(),
        replace: Some(output.clone()),
        data_dir: directory.path().join("data"),
        progress,
    };
    let start = SystemTime::now();
    assert_eq!(worker.write(&write).unwrap(), Some(output.clone()));
    assert_eq!(image::image_dimensions(&output).unwrap(), (8, 6));
    let modified = std::fs::metadata(&output).unwrap().modified().unwrap();
    assert!(modified >= start);
    assert_eq!(modified, UNIX_EPOCH + Duration::from_secs(4_000_000_001));
    assert_eq!(std::fs::read(&source).unwrap(), b"source identity");
    assert!(!directory.path().join("ignored.png").exists());
    write.replace = Some(source.clone());
    assert!(
        worker
            .write(&write)
            .unwrap_err()
            .contains("元ファイルは上書きしません")
    );
    assert_eq!(std::fs::read(source).unwrap(), b"source identity");
}

#[test]
fn image_combine_worker_replace_cancel_before_write_preserves_original() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.png");
    let output = directory.path().join("joined.png");
    std::fs::write(&source, b"source identity").unwrap();
    std::fs::write(&output, b"original output").unwrap();
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    load_into(&mut worker, 1);
    worker.before_write = Some(Box::new(|cancel| cancel.store(true, Ordering::Relaxed)));
    let mut render = spec(1);
    render.items[0].1.source = CombineSourceRef::File { path: source };
    let (progress, _) = mpsc::channel();
    let write = WriteSpec {
        render,
        directory: directory.path().into(),
        name: "ignored".into(),
        replace: Some(output.clone()),
        data_dir: directory.path().join("data"),
        progress,
    };
    assert!(worker.write(&write).unwrap().is_none());
    assert_eq!(std::fs::read(output).unwrap(), b"original output");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

fn pdf_comic_item(source_dims: Option<[usize; 2]>) -> Box<BatchExportItem> {
    let mut item = item(0);
    item.source = crate::books::CompositeSource::PdfPage {
        pdf_path: "source.pdf".into(),
        page_num: 3,
        password: Some("source-password".into()),
    };
    item.edits.comic = Some(crate::books::BookComicSnapshot {
        objects: Vec::new(),
        fonts: Arc::new(comic_core::FontSet::new()),
        stamp_cache: HashMap::new(),
    });
    item.edits.comic_source_dims = source_dims;
    item
}

#[test]
fn image_combine_worker_completes_pdf_native_annotation_space_once_and_reuses_snapshot() {
    for known_dims in [None, Some([6000, 8000])] {
        let expected = known_dims.unwrap_or([9000, 10000]);
        let identities = Arc::new(Mutex::new(Vec::new()));
        let captured = identities.clone();
        let mut worker = worker(Box::new(move |item, _| {
            assert_eq!(item.edits.comic_source_dims, Some(expected));
            captured
                .lock()
                .unwrap()
                .push(&item.edits as *const _ as usize);
            Ok(pixels())
        }));
        let analyses = Arc::new(AtomicUsize::new(0));
        let captured = analyses.clone();
        worker.analyze_pdf = Box::new(move |path, page, password, cancel| {
            assert_eq!(path, Path::new("source.pdf"));
            assert_eq!(page, 3);
            assert_eq!(password, Some("source-password"));
            assert!(!cancel.load(Ordering::Relaxed));
            captured.fetch_add(1, Ordering::SeqCst);
            Ok(crate::pdf_loader::PdfPageContentType::Raster { w: 9000, h: 10000 })
        });
        let (commands, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        commands
            .send(Command::Prepare {
                id: 0,
                item: pdf_comic_item(known_dims),
            })
            .unwrap();
        drop(commands);
        worker.run(rx, events, egui::Context::default());
        assert!(matches!(
            results.try_recv().unwrap(),
            Event::Loaded { id: 0, .. }
        ));
        assert_eq!(
            worker.items[&0].item.edits.comic_source_dims,
            Some(expected)
        );
        assert_eq!(
            analyses.load(Ordering::SeqCst),
            usize::from(known_dims.is_none())
        );
        let directory = tempfile::tempdir().unwrap();
        let (progress, _) = mpsc::channel();
        let mut render = spec(1);
        render.items[0].1.source = CombineSourceRef::PdfPage {
            pdf_path: "source.pdf".into(),
            page_num: 3,
        };
        let write = WriteSpec {
            render,
            directory: directory.path().into(),
            name: "joined".into(),
            replace: None,
            data_dir: directory.path().join("data"),
            progress,
        };
        assert!(worker.write(&write).unwrap().is_some());
        assert_eq!(
            analyses.load(Ordering::SeqCst),
            usize::from(known_dims.is_none())
        );
        let identities = identities.lock().unwrap();
        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0], identities[1]);
    }
}

#[test]
fn image_combine_worker_pdf_annotation_analysis_failure_stays_an_item_failure() {
    let mut worker = worker(Box::new(|_, _| {
        panic!("failed preparation must not decode")
    }));
    let mut item = pdf_comic_item(None);
    let crate::books::CompositeSource::PdfPage { password, .. } = &mut item.source else {
        unreachable!()
    };
    *password = None;
    worker.analyze_pdf = Box::new(|_, _, password, _| {
        assert!(password.is_none());
        Err("password required".into())
    });
    let (commands, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    commands.send(Command::Prepare { id: 0, item }).unwrap();
    drop(commands);
    worker.run(rx, events, egui::Context::default());
    assert!(
        matches!(results.try_recv().unwrap(), Event::Failed { id: 0, message } if message.contains("パスワードが必要です"))
    );
    assert!(worker.items.is_empty());
}

#[test]
fn image_combine_worker_vector_without_recorded_annotation_space_fails_target() {
    let mut worker = worker(Box::new(|_, _| {
        panic!("analysis-only test must not decode")
    }));
    worker.analyze_pdf = Box::new(|_, _, _, _| Ok(crate::pdf_loader::PdfPageContentType::Vector));
    let mut absent = pdf_comic_item(None);
    assert_eq!(
        worker.prepare_pdf_comic_snapshot(&mut absent).unwrap_err(),
        "注釈の位置を再現できません。一覧から結合し直してください"
    );
    // Recorded spaces are already exact; vector analysis must not replace them.
    for dimensions in [[1600, 2400], [3200, 4800]] {
        let mut mounted = pdf_comic_item(Some(dimensions));
        worker.prepare_pdf_comic_snapshot(&mut mounted).unwrap();
        assert_eq!(mounted.edits.comic_source_dims, Some(dimensions));
    }
    let (commands, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    commands
        .send(Command::Prepare {
            id: 0,
            item: absent,
        })
        .unwrap();
    drop(commands);
    worker.run(rx, events, egui::Context::default());
    assert!(
        matches!(results.try_recv().unwrap(), Event::Failed { id: 0, message }
        if message == "注釈の位置を再現できません。一覧から結合し直してください")
    );
    assert!(worker.items.is_empty());
}

#[test]
fn image_combine_worker_records_each_snapshot_annotation_space_on_write_and_replace() {
    let directory = tempfile::tempdir().unwrap();
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    load_into(&mut worker, 2);
    let mut render = spec(2);
    for id in 0..2 {
        let source = directory.path().join(format!("source{id}.png"));
        std::fs::write(&source, b"source identity").unwrap();
        let loaded = worker.items.get_mut(&id).unwrap();
        loaded.item.source = crate::books::CompositeSource::File {
            path: source.clone(),
        };
        loaded.item.edits.comic_source_dims = Some([1000 + id * 100, 2000 + id * 100]);
        render.items[id].1.source = CombineSourceRef::File { path: source };
        render.items[id].1.label_override = Some(format!("label{id}"));
        render.items[id].1.annotation_space = Some([1, 1]);
    }
    render.items.reverse();
    let data_dir = directory.path().join("data");
    let output = directory.path().join("joined.png");
    for replace in [false, true] {
        let (progress, results) = mpsc::channel();
        let (commands, rx) = mpsc::channel();
        let (events, _) = mpsc::channel();
        if replace {
            worker
                .items
                .get_mut(&1)
                .unwrap()
                .item
                .edits
                .comic_source_dims = Some([3000, 4000]);
            worker
                .items
                .get_mut(&0)
                .unwrap()
                .item
                .edits
                .comic_source_dims = None;
        }
        commands
            .send(Command::Write(WriteSpec {
                render: render.clone(),
                directory: directory.path().to_path_buf(),
                name: "joined".into(),
                replace: replace.then(|| output.clone()),
                data_dir: data_dir.clone(),
                progress,
            }))
            .unwrap();
        drop(commands);
        worker.run(rx, events, egui::Context::default());
        assert_eq!(
            results
                .try_iter()
                .filter(|event| matches!(event, ExportEvent::Completed(_)))
                .count(),
            1
        );
        let recipe = super::super::store::load_recipe(&data_dir, &output)
            .unwrap()
            .unwrap();
        assert_eq!(recipe.items[0].label_override.as_deref(), Some("label1"));
        assert_eq!(recipe.items[1].label_override.as_deref(), Some("label0"));
        assert_eq!(
            recipe.items[0].annotation_space,
            Some(if replace { [3000, 4000] } else { [1100, 2100] })
        );
        assert_eq!(
            recipe.items[1].annotation_space,
            if replace { None } else { Some([1000, 2000]) }
        );
    }
}

#[test]
fn image_combine_worker_vector_with_recorded_space_or_without_annotations_needs_no_analysis() {
    let mut worker = worker(Box::new(|_, _| Ok(pixels())));
    let (commands, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    commands
        .send(Command::Prepare {
            id: 0,
            item: pdf_comic_item(Some([1600, 2400])),
        })
        .unwrap();
    let mut unannotated = pdf_comic_item(None);
    unannotated.edits.comic = None;
    commands
        .send(Command::Prepare {
            id: 1,
            item: unannotated,
        })
        .unwrap();
    drop(commands);
    worker.run(rx, events, egui::Context::default());
    assert_eq!(
        results
            .try_iter()
            .filter(|event| matches!(event, Event::Loaded { .. }))
            .count(),
        2
    );
    assert_eq!(
        worker.items[&0].item.edits.comic_source_dims,
        Some([1600, 2400])
    );
    assert_eq!(worker.items[&1].item.edits.comic_source_dims, None);
}
