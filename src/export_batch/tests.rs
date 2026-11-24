use super::*;
use crate::export_dialog::ExportScale;

fn file_item(path: &Path, filename: &str) -> BatchExportItem {
    let edits = item(filename, "source").edits;
    BatchExportItem::new(
        filename.into(),
        "source".into(),
        crate::books::CompositeSource::File {
            path: path.to_owned(),
        },
        edits,
    )
}

fn options(directory: &Path) -> BatchExportOptions {
    BatchExportOptions::from_legacy(
        directory.to_owned(),
        "<filename>".into(),
        crate::capture::CaptureFormat::Png,
        ExportScale::Full,
    )
}

fn run(options: BatchExportOptions, items: Vec<BatchExportItem>) -> Vec<ExportEvent> {
    let (tx, rx) = mpsc::channel();
    run_batch_export(
        BatchExportPlan {
            options,
            items,
            raw: None,
        },
        Arc::new(AtomicBool::new(false)),
        tx,
    );
    rx.into_iter().collect()
}

fn completed_paths(events: &[ExportEvent]) -> Vec<PathBuf> {
    events
        .iter()
        .filter_map(|event| {
            if let ExportEvent::Completed(success) = event {
                Some(success.path.clone())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn legacy_all_format_scale_combinations_preserve_bytes_and_names() {
    use crate::capture::CaptureFormat;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    let image = image::RgbaImage::from_fn(513, 261, |x, y| {
        image::Rgba([
            (x * 7) as u8,
            (y * 11) as u8,
            (x + y) as u8,
            if x % 5 == 0 { 96 } else { 255 },
        ])
    });
    image.save(&source).unwrap();
    for format in [
        CaptureFormat::Png,
        CaptureFormat::Jpeg95,
        CaptureFormat::Jpeg85,
        CaptureFormat::Jpeg75,
    ] {
        for scale in [
            ExportScale::Full,
            ExportScale::Half,
            ExportScale::Quarter,
            ExportScale::LongEdge(256),
            ExportScale::LongEdge(4096),
        ] {
            let out = tempfile::tempdir_in(temp.path()).unwrap();
            let legacy = tempfile::tempdir_in(temp.path()).unwrap();
            let mut entries = Vec::new();
            for _ in 0..2 {
                let mut entry = file_item(&source, "source");
                entry.edits.format = format;
                entry.edits.rotation = crate::rotation_db::Rotation::Cw90;
                entries.push(entry);
            }
            // Reference remains the pre-cutover books writer. Run and capture fixed
            // output hashes in the authorized verification checkout before acceptance.
            for (index, entry) in entries.iter().enumerate() {
                let name = if index == 0 {
                    format!("source_source.{}", format.extension())
                } else {
                    format!("source_source_1.{}", format.extension())
                };
                crate::books::write_composited_page(
                    &entry.source,
                    &entry.edits,
                    &legacy.path().join(name),
                    scale,
                    None,
                )
                .unwrap();
            }
            let events = run(
                BatchExportOptions::from_legacy(
                    out.path().to_owned(),
                    "<dirname>_<filename>".into(),
                    format,
                    scale,
                ),
                entries,
            );
            let paths = completed_paths(&events);
            assert_eq!(paths.len(), 2, "{format:?} {scale:?}");
            for path in paths {
                let expected = legacy.path().join(path.file_name().unwrap());
                assert!(expected.exists(), "filename changed: {}", path.display());
                let reference_bytes = std::fs::read(expected).unwrap();
                use sha2::Digest;
                eprintln!(
                    "legacy export reference {format:?}/{scale:?}/{} sha256={:x}",
                    path.file_name().unwrap().to_string_lossy(),
                    sha2::Sha256::digest(&reference_bytes)
                );
                assert_eq!(
                    std::fs::read(path).unwrap(),
                    reference_bytes,
                    "{format:?} {scale:?}"
                );
            }
        }
    }
}

#[test]
fn unique_target_path_avoids_existing_files_and_names_claimed_in_this_run() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("out.png"), b"x").unwrap();
    let mut claimed = OutputReservation::default();
    let Target::Write { path: first, .. } = claimed
        .reserve(temp.path(), "out", "png", OverwritePolicy::Rename)
        .unwrap()
    else {
        panic!("write");
    };
    let Target::Write { path: second, .. } = claimed
        .reserve(temp.path(), "out", "png", OverwritePolicy::Rename)
        .unwrap()
    else {
        panic!("write");
    };
    assert_eq!(first, temp.path().join("out_1.png"));
    assert_eq!(second, temp.path().join("out_2.png"));
}

#[test]
fn two_items_that_expand_to_the_same_name_both_get_written() {
    let temp = tempfile::tempdir().unwrap();
    let mut claimed = OutputReservation::default();
    let a = resolve_item_stem("<dirname>", &item("one", "same"), 1);
    let b = resolve_item_stem("<dirname>", &item("two", "same"), 2);
    let Target::Write { path: a, .. } = claimed
        .reserve(temp.path(), &a, "png", OverwritePolicy::Rename)
        .unwrap()
    else {
        panic!("write");
    };
    let Target::Write { path: b, .. } = claimed
        .reserve(temp.path(), &b, "png", OverwritePolicy::Rename)
        .unwrap()
    else {
        panic!("write");
    };
    assert_ne!(a, b);
}

#[test]
fn resize_modes_cover_orientations_and_one_upscale_rule() {
    for source in [[1200, 600], [600, 1200], [600, 600]] {
        for mode in [
            BatchResizeMode::None,
            BatchResizeMode::Percent(50),
            BatchResizeMode::Percent(200),
            BatchResizeMode::LongEdge(256),
            BatchResizeMode::ShortEdge(1024),
            BatchResizeMode::Width(256),
            BatchResizeMode::Height(1024),
            BatchResizeMode::Fit {
                width: 800,
                height: 700,
            },
        ] {
            let allow = BatchResize {
                mode,
                allow_upscale: true,
                ..Default::default()
            }
            .scaled_size(source);
            let disallow = BatchResize {
                mode,
                allow_upscale: false,
                ..Default::default()
            }
            .scaled_size(source);
            assert_eq!(
                disallow,
                if allow[0] > source[0] || allow[1] > source[1] {
                    source
                } else {
                    allow
                }
            );
            assert!(allow.iter().all(|v| *v > 0));
            if let BatchResizeMode::Fit { width, height } = mode {
                assert!(allow[0] <= width as usize && allow[1] <= height as usize);
            }
        }
    }
    assert_eq!(
        BatchResize {
            mode: BatchResizeMode::ShortEdge(256),
            ..Default::default()
        }
        .scaled_size([600, 1200]),
        [256, 512]
    );
    assert_eq!(
        BatchResize {
            mode: BatchResizeMode::Width(256),
            ..Default::default()
        }
        .scaled_size([1200, 600]),
        [256, 128]
    );
    assert_eq!(
        BatchResize {
            mode: BatchResizeMode::Height(256),
            ..Default::default()
        }
        .scaled_size([600, 1200]),
        [128, 256]
    );
}

#[test]
fn legacy_dimensions_match_for_odd_landscape_portrait_and_square() {
    for size in [[513, 261], [261, 513], [513, 513], [1, 1], [11, 3]] {
        for scale in [
            ExportScale::Full,
            ExportScale::Half,
            ExportScale::Quarter,
            ExportScale::LongEdge(256),
            ExportScale::LongEdge(16384),
        ] {
            let opts = BatchExportOptions::from_legacy(
                PathBuf::new(),
                String::new(),
                crate::capture::CaptureFormat::Png,
                scale,
            );
            assert_eq!(opts.resize.scaled_size(size), scale.scaled_size(size));
        }
    }
}

#[test]
fn same_format_uses_extension_table_and_independent_quality() {
    let mut format = BatchFormat {
        kind: BatchFormatKind::SameAsSource,
        jpeg_quality: 41,
        webp_quality: 73,
        ..Default::default()
    };
    for ext in ["jpg", "JPEG", "jfif"] {
        assert_eq!(format.resolve(ext), SrcFormat::Jpeg);
    }
    assert_eq!(format.resolve("PNG"), SrcFormat::Png);
    assert_eq!(format.resolve("WEBP"), SrcFormat::Webp);
    for ext in ["pdf", "gif", "bmp", "tiff", "avif", "jxl", "raw", "psd", ""] {
        assert_eq!(format.resolve(ext), SrcFormat::Jpeg);
    }
    format.same_fallback = SameFallback::Png;
    assert_eq!(format.resolve("pdf"), SrcFormat::Png);
    assert_eq!((format.jpeg_quality, format.webp_quality), (41, 73));
}

#[test]
fn template_ext_padding_datetime_and_unknown_tokens() {
    let date = ("20261125".into(), "123456".into());
    assert_eq!(
        expand_template(
            "<filename>_<ext>_<num>_<num:1>_<num:9>_<date>_<time>_<num:0>_<num:10>_<unknown>",
            "photo",
            "folder",
            "png",
            7,
            Some(&date)
        ),
        "photo_png_0007_7_000000007_20261125_123456_<num:0>_<num:10>_<unknown>"
    );
    assert_eq!(
        expand_template("<date>_<time>", "", "", "", 1, None),
        "<date>_<time>"
    );
    assert_eq!(
        expand_template("<filename>", "<num>", "", "", 1, None),
        "<num>"
    );
}

#[test]
fn filename_preview_keeps_deferred_dates_without_source_io_or_marker_collisions() {
    let mut source = item("shot", "trip");
    source.source_ext = "jpg".into();
    assert_eq!(
        preview_item_stem("<filename>/<num:2>_<ext>_<date>_<time>", &source, 7),
        "shot_07_jpg_<date>_<time>"
    );
    source.filename = "__miv_export_deferred__date".into();
    assert_eq!(
        preview_item_stem("<filename>_<date>", &source, 1),
        "__miv_export_deferred__date_<date>"
    );
}

#[test]
fn item_source_locations_are_pure_and_archive_extension_is_entry_extension() {
    let zip = BatchExportItem::new(
        "z".into(),
        "archive".into(),
        crate::books::CompositeSource::ZipEntry {
            zip_path: PathBuf::from(r"C:\images\archive.zip"),
            entry_name: "nested/page.JPEG".into(),
        },
        item("z", "").edits,
    );
    assert_eq!(zip.source_dir, PathBuf::from(r"C:\images"));
    assert_eq!(zip.source_ext, "jpeg");
    let pdf = BatchExportItem::new(
        "p".into(),
        "book".into(),
        crate::books::CompositeSource::PdfPage {
            pdf_path: PathBuf::from(r"C:\images\book.pdf"),
            page_num: 0,
            password: None,
        },
        item("p", "").edits,
    );
    assert_eq!(pdf.source_dir, zip.source_dir);
    assert_eq!(pdf.source_ext, "pdf");
}

#[test]
fn destination_structure_and_subfolder_are_normalized_without_io() {
    let first = file_item(Path::new(r"C:\Images\Trip\a.png"), "a");
    let second = file_item(Path::new(r"c:\images\Other\b.png"), "b");
    let items = vec![first, second];
    let base = paths::common_ancestor(&items).unwrap();
    assert_eq!(base, PathBuf::from(r"C:\Images"));
    let dest = BatchDestination::Folder {
        path: PathBuf::from(r"D:\out"),
        keep_structure: true,
    };
    assert_eq!(
        paths::output_directory(&dest, &items[0], Some(&base)),
        PathBuf::from(r"D:\out\Trip")
    );
    assert_eq!(
        paths::output_directory(
            &BatchDestination::SameAsSource {
                subfolder: "../out".into()
            },
            &items[0],
            None
        ),
        items[0].source_dir.join("_out")
    );
    assert_eq!(
        paths::output_directory(
            &BatchDestination::SameAsSource {
                subfolder: String::new()
            },
            &items[0],
            None
        ),
        items[0].source_dir
    );
    let a = file_item(Path::new(r"C:\one\a.png"), "a");
    let b = file_item(Path::new(r"D:\two\b.png"), "b");
    let items = vec![a, b];
    assert!(paths::common_ancestor(&items).is_none());
    assert_eq!(
        paths::output_directory(&dest, &items[0], None),
        PathBuf::from(r"D:\out\C\one")
    );
    let unc = file_item(Path::new(r"\\server\share\folder\a.png"), "a");
    assert_eq!(
        paths::output_directory(&dest, &unc, None),
        PathBuf::from(r"D:\out\server_share\folder")
    );
    let parent_path = file_item(Path::new(r"C:\..\..\folder\a.png"), "a");
    assert_eq!(
        paths::output_directory(&dest, &parent_path, Some(Path::new(r"C:\"))),
        PathBuf::from(r"D:\out\folder")
    );
}

#[test]
fn options_missing_fields_take_defaults_and_roundtrip() {
    let options: BatchExportOptions = serde_json::from_str("{}").unwrap();
    assert_eq!(options, BatchExportOptions::default());
    assert_eq!(
        serde_json::from_str::<BatchExportOptions>(&serde_json::to_string(&options).unwrap())
            .unwrap(),
        options
    );
}

#[test]
fn skip_existing_never_calls_decoder_and_duplicate_gets_number() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("same.png"), b"existing").unwrap();
    let mut opts = options(temp.path());
    opts.overwrite = OverwritePolicy::Skip;
    let (tx, rx) = mpsc::channel();
    let mut decoded = 0;
    run_batch_export_with_decoder(
        BatchExportPlan {
            options: opts,
            items: vec![item("same", ""), item("same", "")],
            raw: None,
        },
        Arc::new(AtomicBool::new(false)),
        tx,
        &mut |_, _, _, _| {
            decoded += 1;
            Ok((
                eframe::egui::ColorImage::new([1, 1], vec![eframe::egui::Color32::RED]),
                None,
            ))
        },
    );
    let events: Vec<_> = rx.into_iter().collect();
    assert_eq!(decoded, 1);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ExportEvent::Skipped { .. }))
    );
    assert_eq!(
        completed_paths(&events),
        vec![temp.path().join("same_1.png")]
    );
    assert_eq!(
        std::fs::read(temp.path().join("same.png")).unwrap(),
        b"existing"
    );
}

#[test]
fn replace_is_atomic_and_protects_input_with_case_or_dot_alias() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("source.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 30, 40, 255]))
        .save(&input)
        .unwrap();
    let original = std::fs::read(&input).unwrap();
    let mut opts = options(temp.path());
    opts.overwrite = OverwritePolicy::Replace;
    let events = run(opts.clone(), vec![file_item(&input, "SOURCE")]);
    assert!(
        events.iter().any(
            |e| matches!(e,ExportEvent::Failed(f) if f.message == "元ファイルは上書きしません")
        )
    );
    assert_eq!(std::fs::read(&input).unwrap(), original);
    opts.destination = BatchDestination::Folder {
        path: temp.path().join("."),
        keep_structure: false,
    };
    let events = run(opts.clone(), vec![file_item(&input, "source")]);
    assert!(completed_paths(&events).is_empty());
    std::fs::write(temp.path().join("output.png"), b"old").unwrap();
    opts.template = "output".into();
    let events = run(
        opts,
        vec![file_item(&input, "one"), file_item(&input, "two")],
    );
    assert_eq!(
        completed_paths(&events),
        vec![
            temp.path().join(".").join("output.png"),
            temp.path().join(".").join("output_1.png")
        ]
    );
    assert_eq!(
        image::image_dimensions(temp.path().join("output.png")).unwrap(),
        (2, 2)
    );
    assert!(
        std::fs::read_dir(temp.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("miv-tmp"))
    );
}

#[test]
fn reservations_unify_directory_spelling_and_case_for_all_policies() {
    let temp = tempfile::tempdir().unwrap();
    for policy in [
        OverwritePolicy::Rename,
        OverwritePolicy::Skip,
        OverwritePolicy::Replace,
    ] {
        let mut reservation = OutputReservation::default();
        let Target::Write { path: first, .. } = reservation
            .reserve(temp.path(), "A", "png", policy)
            .unwrap()
        else {
            panic!()
        };
        std::fs::write(&first, b"first").unwrap();
        reservation.record_written(&first).unwrap();
        let Target::Write { path: second, .. } = reservation
            .reserve(&temp.path().join("."), "a", "png", policy)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(second.file_name().unwrap(), "a_1.png");
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        std::fs::remove_file(first).unwrap();
    }
}

#[test]
fn webp_oversize_fails_one_item_and_continues_without_panicking() {
    let temp = tempfile::tempdir().unwrap();
    let mut opts = options(temp.path());
    opts.format.kind = BatchFormatKind::Webp;
    let (tx, rx) = mpsc::channel();
    run_batch_export_with_decoder(
        BatchExportPlan {
            options: opts,
            items: vec![item("huge", ""), item("small", "")],
            raw: None,
        },
        Arc::new(AtomicBool::new(false)),
        tx,
        &mut |item, _, _, _| {
            let width = if item.filename == "huge" { 16384 } else { 1 };
            Ok((
                eframe::egui::ColorImage::new([width, 1], vec![eframe::egui::Color32::RED; width]),
                None,
            ))
        },
    );
    let events: Vec<_> = rx.into_iter().collect();
    assert!(events.iter().any(
        |e| matches!(e,ExportEvent::Failed(f) if f.label == "huge" && f.message.contains("16384×1"))
    ));
    assert_eq!(
        completed_paths(&events),
        vec![temp.path().join("small.webp")]
    );
    assert!(matches!(events.last(), Some(ExportEvent::AllDone)));
    assert!(!temp.path().join("huge.webp").exists());
    assert!(validate_dimensions(&SrcFormat::Jpeg, [65501, 1]).is_err());
    assert!(validate_dimensions(&SrcFormat::Jpeg, [65500, 1]).is_ok());
}

#[test]
fn cancellation_during_erase_uses_same_token_and_never_writes() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("source.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 30, 40, 255]))
        .save(&input)
        .unwrap();
    let mut entry = file_item(&input, "result");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);
    let cancel = Arc::new(AtomicBool::new(false));
    let expected = Arc::clone(&cancel);
    entry.edits.erase = Some(crate::books::BookEraseSnapshot {
        mask: crate::books::BookMaskSnapshot {
            bitmap: vec![true; 4],
            shapes: Vec::new(),
            size: [2, 2],
        },
        run: Box::new(move |_, _, _, token| {
            assert!(Arc::ptr_eq(token, &expected));
            entered_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            assert!(token.load(Ordering::Relaxed));
            Err("cancelled injected erase".into())
        }),
    });
    let (tx, rx) = mpsc::channel();
    let plan = BatchExportPlan {
        options: options(temp.path()),
        items: vec![entry],
        raw: None,
    };
    let worker_cancel = Arc::clone(&cancel);
    let worker = std::thread::spawn(move || run_batch_export(plan, worker_cancel, tx));
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    cancel.store(true, Ordering::Relaxed);
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    let events: Vec<_> = rx.into_iter().collect();
    assert!(matches!(events.last(), Some(ExportEvent::Cancelled)));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ExportEvent::Completed(_) | ExportEvent::Failed(_)))
    );
    assert!(!temp.path().join("result.png").exists());
}

#[test]
fn file_time_uses_source_file_and_archive_time() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 30, 40, 255]))
        .save(&source)
        .unwrap();
    let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(timestamp)
        .unwrap();
    let mut opts = options(temp.path());
    opts.keep_file_time = true;
    opts.template = "<filename>_<date>_<time>".into();
    let events = run(opts.clone(), vec![file_item(&source, "output")]);
    let output = completed_paths(&events).pop().unwrap();
    assert_eq!(
        std::fs::metadata(output).unwrap().modified().unwrap(),
        std::fs::metadata(&source).unwrap().modified().unwrap()
    );
    let archive = temp.path().join("archive.zip");
    std::fs::write(&archive, b"archive fixture handled by injected decoder").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&archive)
        .unwrap()
        .set_modified(timestamp)
        .unwrap();
    let entries = vec![
        BatchExportItem::new(
            "zip".into(),
            "archive".into(),
            crate::books::CompositeSource::ZipEntry {
                zip_path: archive.clone(),
                entry_name: "page.png".into(),
            },
            item("", "").edits,
        ),
        BatchExportItem::new(
            "pdf".into(),
            "archive".into(),
            crate::books::CompositeSource::PdfPage {
                pdf_path: archive.clone(),
                page_num: 0,
                password: None,
            },
            item("", "").edits,
        ),
    ];
    let (tx, rx) = mpsc::channel();
    run_batch_export_with_decoder(
        BatchExportPlan {
            options: opts,
            items: entries,
            raw: None,
        },
        Arc::new(AtomicBool::new(false)),
        tx,
        &mut |_, _, _, _| {
            Ok((
                eframe::egui::ColorImage::new([1, 1], vec![eframe::egui::Color32::RED]),
                None,
            ))
        },
    );
    let events: Vec<_> = rx.into_iter().collect();
    assert_eq!(completed_paths(&events).len(), 2);
    for output in completed_paths(&events) {
        assert_eq!(
            std::fs::metadata(output).unwrap().modified().unwrap(),
            timestamp
        );
    }
}

#[cfg(windows)]
#[test]
fn junction_alias_destinations_never_overwrite_or_skip_earlier_output() {
    use std::os::windows::process::CommandExt;
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    let alias = temp.path().join("alias");
    std::fs::create_dir(&real).unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&alias)
        .arg(&real)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "junction fixture: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    // Remove the link itself before TempDir recursively removes its target tree.
    struct Junction(PathBuf);
    impl Drop for Junction {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    let _junction = Junction(alias.clone());
    for name in ["a.png", "b.png"] {
        image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 30, 40, 255]))
            .save(real.join(name))
            .unwrap();
    }
    for policy in [OverwritePolicy::Skip, OverwritePolicy::Replace] {
        let mut opts = options(&real);
        opts.overwrite = policy;
        opts.template = "fixed".into();
        let subfolder = format!("output-{policy:?}");
        opts.destination = BatchDestination::SameAsSource {
            subfolder: subfolder.clone(),
        };
        let events = run(
            opts,
            vec![
                file_item(&real.join("a.png"), "a"),
                file_item(&alias.join("b.png"), "b"),
            ],
        );
        assert_eq!(completed_paths(&events).len(), 2);
        assert!(real.join(&subfolder).join("fixed.png").exists());
        assert!(real.join(&subfolder).join("fixed_1.png").exists());
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ExportEvent::Skipped { .. } | ExportEvent::Failed(_)))
        );
    }
    let mut opts = options(&alias);
    opts.overwrite = OverwritePolicy::Replace;
    opts.template = "a".into();
    let before = std::fs::read(real.join("a.png")).unwrap();
    let events = run(opts, vec![file_item(&real.join("a.png"), "a")]);
    assert!(
        events.iter().any(
            |e| matches!(e,ExportEvent::Failed(f) if f.message == "元ファイルは上書きしません")
        )
    );
    assert_eq!(std::fs::read(real.join("a.png")).unwrap(), before);
}

#[cfg(windows)]
#[test]
fn short_filename_alias_never_replaces_or_skips_this_runs_output() {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::{Win32::Storage::FileSystem::GetShortPathNameW, core::PCWSTR};
    let temp = tempfile::tempdir().unwrap();
    for policy in [OverwritePolicy::Skip, OverwritePolicy::Replace] {
        let mut reservations = OutputReservation::default();
        let Target::Write { path: first, .. } = reservations
            .reserve(temp.path(), "Long Batch Export Filename", "png", policy)
            .unwrap()
        else {
            panic!()
        };
        std::fs::write(&first, b"first-output").unwrap();
        reservations.record_written(&first).unwrap();
        let wide: Vec<_> = first.as_os_str().encode_wide().chain(Some(0)).collect();
        let needed = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), None) };
        if needed == 0 {
            eprintln!("volume does not expose short names");
            return;
        }
        let mut short = vec![0; needed as usize];
        let len = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut short)) };
        assert!(len > 0 && len < needed);
        let short = PathBuf::from(std::ffi::OsString::from_wide(&short[..len as usize]));
        if short.file_name() == first.file_name() {
            eprintln!("8.3 creation disabled on fixture volume");
            return;
        }
        let stem = short.file_stem().unwrap().to_str().unwrap();
        let Target::Write { path: second, .. } = reservations
            .reserve(temp.path(), stem, "png", policy)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(second.file_stem().unwrap(), format!("{stem}_1").as_str());
        assert_eq!(std::fs::read(&first).unwrap(), b"first-output");
        std::fs::remove_file(first).unwrap();
    }
}

fn png_with_prompt(path: &Path) -> Vec<u8> {
    png_with_text(path, b"parameters\0batch-test-prompt")
}

fn png_with_text(path: &Path, payload: &[u8]) -> Vec<u8> {
    image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 30, 40, 255]))
        .save(path)
        .unwrap();
    let mut png = std::fs::read(path).unwrap();
    let mut chunk = (payload.len() as u32).to_be_bytes().to_vec();
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(payload);
    // PNG CRC32, kept local so the test adds no dependency.
    let mut crc = !0u32;
    for byte in &chunk[4..] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(crc & 1));
        }
    }
    chunk.extend_from_slice(&(!crc).to_be_bytes());
    png.splice(33..33, chunk);
    std::fs::write(path, &png).unwrap();
    png
}

#[test]
fn thumbnail_notice_follows_completed_only_for_copied_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    png_with_text(
        &source,
        b"XML:com.adobe.xmp\0<rdf xmlns:xmpGImg=\"http://ns.adobe.com/xap/1.0/g/img/\"/>",
    );
    for (kind, keep, notice) in [
        (BatchFormatKind::Png, true, true),
        (BatchFormatKind::Png, false, false),
        (BatchFormatKind::Jpeg, true, false),
    ] {
        let out = tempfile::tempdir_in(temp.path()).unwrap();
        let mut opts = options(out.path());
        opts.format.kind = kind;
        opts.keep_metadata = keep;
        let events = run(opts, vec![file_item(&source, "result")]);
        assert_eq!(completed_paths(&events).len(), 1);
        let warning = events
            .iter()
            .position(|e| matches!(e,ExportEvent::ThumbnailNotice { kinds, .. } if kinds.xmp));
        assert_eq!(warning.is_some(), notice);
        if let Some(index) = warning {
            assert!(matches!(events[index - 1], ExportEvent::Completed(_)));
        }
    }
}

#[test]
fn worker_metadata_is_only_kept_for_matching_output_format() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    png_with_prompt(&source);
    for (kind, keep, expected) in [
        (BatchFormatKind::Png, true, true),
        (BatchFormatKind::Png, false, false),
        (BatchFormatKind::Jpeg, true, false),
        (BatchFormatKind::Webp, true, false),
    ] {
        let out = tempfile::tempdir_in(temp.path()).unwrap();
        let mut opts = options(out.path());
        opts.format.kind = kind;
        opts.keep_metadata = keep;
        let events = run(opts, vec![file_item(&source, "result")]);
        let path = completed_paths(&events).pop().unwrap();
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(
            bytes
                .windows(b"batch-test-prompt".len())
                .any(|v| v == b"batch-test-prompt"),
            expected
        );
    }
}

#[test]
fn zip_decode_returns_entry_bytes_for_metadata_without_second_entry_read() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    let png = png_with_prompt(&source);
    let archive = temp.path().join("archive.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    zip.start_file("nested/page.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&png).unwrap();
    zip.finish().unwrap();
    let entry = BatchExportItem::new(
        "zip-page".into(),
        "archive".into(),
        crate::books::CompositeSource::ZipEntry {
            zip_path: archive,
            entry_name: "nested/page.png".into(),
        },
        item("", "").edits,
    );
    let (_, retained) = crate::books::decode_composite_source_for_materialization_with_bytes(
        &entry.source,
        4096,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .unwrap();
    assert_eq!(retained.as_deref(), Some(png.as_slice()));
    let mut opts = options(temp.path());
    opts.keep_metadata = true;
    let events = run(opts, vec![entry]);
    let path = completed_paths(&events).pop().unwrap();
    let output = std::fs::read(path).unwrap();
    assert!(
        output
            .windows(b"batch-test-prompt".len())
            .any(|v| v == b"batch-test-prompt")
    );
}

fn item(filename: &str, dirname: &str) -> BatchExportItem {
    BatchExportItem {
        composition_check: None,
        source_dir: PathBuf::new(),
        source_ext: "png".into(),
        filename: filename.to_string(),
        dirname: dirname.to_string(),
        source: crate::books::CompositeSource::File {
            path: PathBuf::from("unused"),
        },
        edits: crate::books::BakedEditSnapshot {
            params: crate::adjustment::AdjustParams::default(),
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
            stage: crate::bake_stage::BakeStage::default(),
            creative_lut: None,
            ai: None,
        },
    }
}

#[test]
fn template_expands_every_placeholder() {
    let got = expand_filename_template("<dirname>/<filename>_<num>", "shot", "trip", 7);

    assert_eq!(got, "trip/shot_0007");
}

#[test]
fn template_keeps_unknown_placeholders_instead_of_dropping_them() {
    let got = expand_filename_template("<filename>_<unknown>", "shot", "trip", 1);

    assert_eq!(got, "shot_<unknown>");
}

#[test]
fn stem_replaces_path_separators_so_the_output_stays_in_the_chosen_folder() {
    let got = resolve_item_stem("<dirname>/<filename>", &item("shot", "trip"), 1);

    assert_eq!(got, "trip_shot");
}

#[test]
fn worker_qualifies_captured_selection_against_latest_hidden_and_preserves_visible_order() {
    use crate::book_composition::{
        BookComposition, BookCompositionService, CandidatePage, CompositionOutputCheck,
        CompositionPageRef, PageDomain, ReadingDirection,
    };
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("real.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([50, 60, 70, 255]))
        .save(&input)
        .unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(temp.path().join("data"), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/selected.pdf").unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv()
        .unwrap()
        .status
        .unwrap();
    let mut items: Vec<_> = [2, 0, 1]
        .into_iter()
        .map(|page| {
            let mut item = item(&format!("page-{page}"), "book");
            item.source = crate::books::CompositeSource::File {
                path: input.clone(),
            };
            item.composition_check = Some(CompositionOutputCheck::new(
                service.clone(),
                CandidatePage {
                    identity: crate::book_resume_db::PageIdentity::Document {
                        container: "C:/selected.pdf".into(),
                        page_num: page,
                    },
                    physical_archive: None,
                    raw_tree: None,
                },
            ));
            item
        })
        .collect();
    // A hidden selection may be nonexistent; admission must precede decode.
    items[2].source = crate::books::CompositeSource::File {
        path: temp.path().join("hidden-must-not-decode.png"),
    };
    let mut record = BookComposition::new(
        ReadingDirection::Ltr,
        (0..3)
            .map(|page_num| CompositionPageRef::PdfPage { page_num })
            .collect(),
    );
    record
        .hidden_pages
        .push(CompositionPageRef::PdfPage { page_num: 1 });
    service
        .save(scope, PageDomain::Document, record)
        .recv()
        .unwrap()
        .status
        .unwrap();
    let output = temp.path().join("out");
    let (tx, rx) = mpsc::channel();
    run_batch_export(
        BatchExportPlan {
            options: BatchExportOptions::from_legacy(
                output.clone(),
                "<num>_<filename>".into(),
                crate::capture::CaptureFormat::Png,
                ExportScale::Full,
            ),
            items,
            raw: None,
        },
        Arc::new(AtomicBool::new(false)),
        tx,
    );
    let events: Vec<_> = rx.into_iter().collect();
    let completed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ExportEvent::Completed(result) => Some(result.label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(completed, vec!["page-2", "page-0"]);
    assert!(events.iter().any(|event| matches!(event, ExportEvent::Failed(failure) if failure.label == "page-1" && failure.message == crate::book_composition::COMPOSITION_OUTPUT_HIDDEN_NOTICE)));
    assert!(output.join("0001_page-2.png").exists());
    assert!(output.join("0002_page-0.png").exists());
    assert_eq!(std::fs::read_dir(output).unwrap().count(), 2);
    owner.shutdown_and_join();
}

#[test]
fn worker_reports_unreadable_composition_without_reading_source_or_writing_page() {
    use crate::book_composition::{
        BookCompositionService, CandidatePage, CompositionOutputCheck, PageDomain,
    };
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("book_composition.db"),
        b"preserve unreadable composition",
    )
    .unwrap();
    let (service, owner) = BookCompositionService::spawn_at(data.clone(), None).unwrap();
    service
        .prepare(
            crate::book_scope::BookScopeKey::document("C:/selected.pdf").unwrap(),
            PageDomain::Document,
        )
        .recv()
        .unwrap();
    let mut source = item("unreadable", "book");
    source.composition_check = Some(CompositionOutputCheck::new(
        service,
        CandidatePage {
            identity: crate::book_resume_db::PageIdentity::Document {
                container: "C:/selected.pdf".into(),
                page_num: 0,
            },
            physical_archive: None,
            raw_tree: None,
        },
    ));
    let output = temp.path().join("out");
    // Admission now precedes output-folder creation. Keep the original empty-folder
    // assertion by providing that folder as part of the fixture.
    std::fs::create_dir_all(&output).unwrap();
    let (tx, rx) = mpsc::channel();
    run_batch_export(
        BatchExportPlan {
            options: BatchExportOptions::from_legacy(
                output.clone(),
                "<filename>".into(),
                crate::capture::CaptureFormat::Png,
                ExportScale::Full,
            ),
            items: vec![source],
            raw: None,
        },
        Arc::new(AtomicBool::new(false)),
        tx,
    );
    let events: Vec<_> = rx.into_iter().collect();
    assert!(events.iter().any(
        |event| matches!(event, ExportEvent::Failed(failure) if failure.label == "unreadable")
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ExportEvent::Completed(_)))
    );
    assert_eq!(std::fs::read_dir(output).unwrap().count(), 0);
    owner.shutdown_and_join();
    assert_eq!(
        std::fs::read(data.join("book_composition.db")).unwrap(),
        b"preserve unreadable composition"
    );
}
