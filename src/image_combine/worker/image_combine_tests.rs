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
