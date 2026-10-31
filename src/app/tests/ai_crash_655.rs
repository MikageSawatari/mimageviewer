//! Opt-in, windowless investigation of 5ch >>655. Never starts eframe or a product exe.
//! Run each case in a fresh test process: ModelManager/ORT use process-global OnceLocks.
//! MIV_655_CASE=rapid|wait|off|cached; MIV_655_SIZE=256|768|1440 (portrait 3:4).
use super::*;
use std::io::Write;
use std::time::{Duration, Instant};

fn make_zip(path: &Path, width: u32) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for page in 0..30_u32 {
        let image = image::RgbImage::from_fn(width, width * 4 / 3, |x, y| {
            image::Rgb([
                ((x + page * 7) % 256) as u8,
                ((y + page * 13) % 256) as u8,
                ((x / 8 + y / 8 + page * 17) % 256) as u8,
            ])
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        zip.start_file(
            format!("{page:03}.png"),
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(png.get_ref()).unwrap();
    }
    zip.finish().unwrap();
}

fn pump(app: &mut App, ctx: &egui::Context) {
    app.frame_counter += 1;
    app.service_cancelled_ai_terminals();
    app.poll_prefetch(ctx, PollPrefetchOrigin::TopLevel);
    app.poll_final_ai(ctx);
    app.poll_final_effects(ctx);
    // There is no renderer: consume texture deltas to avoid retaining upload copies.
    ctx.tex_manager().write().take_delta().clear();
}

fn trace(app: &App, label: &str) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        let mut memory: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        memory.cb = std::mem::size_of_val(&memory) as u32;
        if GetProcessMemoryInfo(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            &mut memory as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            memory.cb,
        ) != 0
        {
            eprintln!(
                "655 memory label={label} private_bytes={} working_set={} peak_working_set={}",
                memory.PrivateUsage, memory.WorkingSetSize, memory.PeakWorkingSetSize
            );
        }
    }
    let mut pending: Vec<_> = app
        .final_ai_pending
        .iter()
        .map(|(key, pending)| {
            (
                pending.job_id,
                key.edit_key.idx,
                key.edit_key.items_generation,
                pending.cancel.load(Ordering::Relaxed),
            )
        })
        .collect();
    pending.sort_unstable();
    let queued = app
        .ai_job_queue
        .as_ref()
        .map(|queue| {
            let state = queue.inner.0.lock().unwrap();
            state
                .display
                .iter()
                .chain(state.prefetch.iter())
                .map(|job| {
                    (
                        job.job_id,
                        job.key.edit_key.idx,
                        job.cancel.load(Ordering::Relaxed),
                        job.source.pixels.len() * std::mem::size_of::<egui::Color32>(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let queued_ids: std::collections::HashSet<_> = queued.iter().map(|row| row.0).collect();
    let in_flight: Vec<_> = pending
        .iter()
        .filter(|row| !queued_ids.contains(&row.0))
        .collect();
    eprintln!(
        "655 {label} current={:?} generation={} pending={pending:?} queued={queued:?} popped_or_result_waiting={in_flight:?} activity={} live_results={} retained={} retained_bytes={}",
        app.fullscreen_idx,
        app.items_generation,
        app.local_ai_activity.load(Ordering::Acquire),
        app.final_ai_cache.len(),
        app.retained_final_ai_cache.len(),
        app.retained_final_ai_cache_bytes()
    );
}

fn wait_until(app: &mut App, ctx: &egui::Context, what: &str, mut done: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(180);
    while !done(app) {
        pump(app, ctx);
        assert!(Instant::now() < deadline, "655 timeout: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn enter(app: &mut App, ctx: &egui::Context, idx: usize) -> Option<FinalAiKey> {
    if let Some(previous) = app.fullscreen_idx {
        // This is the landing boundary used by wheel/page/seek navigation, without an OS window.
        app.land_still_page_navigation_target(
            ctx,
            previous,
            idx,
            crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
            &mut None,
        );
    } else {
        app.open_fullscreen(idx, HistoryTrigger::UserChosen);
    }
    assert_eq!(app.fullscreen_idx, Some(idx));
    wait_until(app, ctx, "source pixels", |app| {
        app.current_raw_source_pixels(idx).is_some()
    });
    let params = app.effective_params(idx).clone();
    let source = app.current_raw_source_pixels(idx).unwrap();
    let key = app.final_ai_key_for_pixels(app.current_edit_result_key(idx), source.size, &params);
    // The normal selector starts Display work and consults retained results.
    let _ = app.ensure_final_composite_texture(ctx, idx);
    app.evict_final_pipeline_cache(idx);
    app.prefetch_final_ai(ctx, idx);
    trace(app, "enter");
    key
}

#[test]
#[ignore = "opt-in GPU investigation; fresh process and isolated data_dir required"]
fn ai_crash_655_headless_zip_navigation() {
    let case = std::env::var("MIV_655_CASE").unwrap_or_else(|_| "rapid".into());
    assert!(["rapid", "wait", "off", "cached"].contains(&case.as_str()));
    let width: u32 = std::env::var("MIV_655_SIZE")
        .unwrap_or_else(|_| "256".into())
        .parse()
        .unwrap();
    assert!((32..=1536).contains(&width));
    let mut env = phase_c_support::setup_app();
    assert_eq!(crate::data_dir::get(), env.tmp.path());
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
        };
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
    crate::logger::init();
    crate::perf::init(true, None);
    #[cfg(windows)]
    crate::native_exception_log::install().expect("isolated native exception recorder");
    let evidence = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("target/ai-crash-655/artifacts/{case}-{width}"));
    std::fs::create_dir_all(&evidence).unwrap();
    // Declared after AppTestEnv: preserve logs on ordinary assertion/unwind before its tempdir drops.
    struct Evidence {
        source: PathBuf,
        destination: PathBuf,
    }
    impl Drop for Evidence {
        fn drop(&mut self) {
            crate::log_writer::shutdown_until(Instant::now() + Duration::from_secs(2));
            if let Ok(files) = std::fs::read_dir(self.source.join("logs")) {
                for file in files.flatten() {
                    if file.path().is_file() {
                        let _ = std::fs::copy(file.path(), self.destination.join(file.file_name()));
                    }
                }
            }
        }
    }
    let _evidence = Evidence {
        source: env.tmp.path().to_path_buf(),
        destination: evidence.clone(),
    };
    let models = env.tmp.path().join("models");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/models/realesr_general_x4v3.onnx"),
        models.join("realesr_general_x4v3.onnx"),
    )
    .unwrap();
    let fixture = env.tmp.path().join("655-30-pages.zip");
    make_zip(&fixture, width);
    eprintln!(
        "655 start case={case} dimensions={width}x{} data_dir={} fixture={}",
        width * 4 / 3,
        env.tmp.path().display(),
        fixture.display()
    );
    std::fs::copy(&fixture, evidence.join("30-pages.zip")).unwrap();
    let ctx = egui::Context::default();
    env.load_zip_as_folder(fixture, StartupListIntent::ExplicitList);
    let deadline = Instant::now() + Duration::from_secs(30);
    while env.items.len() != 30 {
        env.poll_zip_enumerate();
        env.finish_page_identity_preparation_for_test();
        assert!(Instant::now() < deadline, "30 ZIP pages not adopted");
        std::thread::sleep(Duration::from_millis(2));
    }
    env.settings.ai_feature_mode = crate::settings::AiFeatureMode::Light;
    env.settings.global_preset.upscale_model = (case != "off").then(|| "realesr_general_v3".into());
    // Eleven pages must fit for the cached control; the released default is ten entries.
    if case == "cached" {
        env.settings.retained_final_ai_cache_max_entries = 20;
    }
    if case != "off" {
        env.ensure_ai_runtime();
        wait_until(&mut env, &ctx, "ORT initialization", |app| {
            matches!(
                app.ai_runtime_init.snapshot(),
                crate::ai::runtime::AiRuntimeInitSnapshot::Ready(_)
                    | crate::ai::runtime::AiRuntimeInitSnapshot::Failed(_)
            )
        });
        assert!(
            env.ai_runtime_init.ready_runtime().is_some(),
            "ORT initialization failed"
        );
    }
    let started = Instant::now();
    if case == "cached" {
        for idx in 0..=10 {
            let key = enter(&mut env, &ctx, idx).expect("AI key");
            wait_until(&mut env, &ctx, "warm AI result", |app| {
                app.final_ai_cache.contains_key(&key) || app.final_ai_failed.contains(&key)
            });
            assert!(!env.final_ai_failed.contains(&key));
        }
        // Close/reopen preserves retained results, matching a warm viewing session.
        env.clear_all_final_pipeline_caches_for_session_close();
        trace(&env, "warmed");
    }
    let mut advanced_before_completion = 0;
    let mut previous_key = None;
    for idx in 0..=10 {
        let unfinished = previous_key.is_some_and(|key| !env.final_ai_cache.contains_key(&key));
        if idx > 0 && unfinished {
            advanced_before_completion += 1;
        }
        previous_key = enter(&mut env, &ctx, idx);
        if let Some(key) = previous_key {
            if case == "wait" {
                wait_until(&mut env, &ctx, "AI before next page", |app| {
                    app.final_ai_cache.contains_key(&key) || app.final_ai_failed.contains(&key)
                });
                assert!(!env.final_ai_failed.contains(&key));
            }
        } else {
            assert_eq!(case, "off", "AI was unexpectedly skipped");
        }
    }
    trace(&env, "after_ten_pages");
    if case != "off" {
        let params = env.effective_params(10).clone();
        let source = env.current_raw_source_pixels(10).unwrap();
        let key = env
            .final_ai_key_for_pixels(env.current_edit_result_key(10), source.size, &params)
            .unwrap();
        wait_until(&mut env, &ctx, "last page AI", |app| {
            app.final_ai_cache.contains_key(&key) || app.final_ai_failed.contains(&key)
        });
        assert!(!env.final_ai_failed.contains(&key));
        trace(&env, "last_page_ready");
    }
    // Explicitly cancel and drain before the fixture restores the process-global data_dir.
    env.clear_all_final_pipeline_caches_for_session_close();
    if let Some(queue) = env.ai_job_queue.as_ref() {
        queue.cancel_all();
    }
    wait_until(&mut env, &ctx, "AI activity drain", |app| {
        app.local_ai_activity.load(Ordering::Acquire) == 0
    });
    pump(&mut env, &ctx);
    eprintln!(
        "655 complete case={case} elapsed_ms={} advanced_before_completion={advanced_before_completion}",
        started.elapsed().as_millis()
    );
}
