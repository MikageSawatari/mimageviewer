//! Synthetic direct-RAR worker regressions. No user archives or settings.
use super::*;
use crate::cover_auto_material::AutoMaterial;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;

thread_local! {
    static BEFORE_SAMPLE: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    static BEFORE_ANALYSIS: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    static AUTOMATIC_MS: Cell<Option<f64>> = const { Cell::new(None) };
}

pub(super) fn before_sample() {
    BEFORE_SAMPLE.with(|hook| {
        let hook = hook.borrow_mut().take();
        if let Some(hook) = hook {
            hook();
        }
    });
}

pub(super) fn automatic_ms(actual: f64) -> f64 {
    AUTOMATIC_MS.with(|injected| injected.take().unwrap_or(actual))
}

pub(super) fn before_analysis() {
    BEFORE_ANALYSIS.with(|hook| {
        let hook = hook.borrow_mut().take();
        if let Some(hook) = hook {
            hook();
        }
    });
}

struct Hooks;
impl Drop for Hooks {
    fn drop(&mut self) {
        BEFORE_SAMPLE.with(|hook| {
            hook.borrow_mut().take();
        });
        BEFORE_ANALYSIS.with(|hook| {
            hook.borrow_mut().take();
        });
        AUTOMATIC_MS.with(|injected| injected.set(None));
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/archives/cover-auto-rar")
        .join(name)
}

fn request(path: &Path, key: &str, entry: &str) -> LoadRequest {
    let mut req = LoadRequest {
        path: path.to_owned(),
        zip_entry: Some(entry.into()),
        cache_key_override: Some(key.into()),
        source_policy: LoadSourcePolicy::SourceOnly,
        mtime: 7,
        file_size: 9,
        force_cache: true,
        ..Default::default()
    };
    req.set_auto_cover_crop(Some(crate::cover_auto_crop::AutoCropIdentity::new(
        crate::settings::ReadingDirection::Rtl,
    )));
    req
}

fn run(
    req: &mut LoadRequest,
    catalog: &Arc<crate::catalog::CatalogDb>,
    decision: CacheDecision,
    cancel: Option<&Arc<AtomicBool>>,
) -> Vec<ThumbMsg> {
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicUsize::new(0));
    process_load_request(
        req,
        &std::sync::RwLock::new(HashMap::new()),
        &tx,
        Some(catalog),
        64,
        75,
        64,
        decision,
        &done,
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        cancel,
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(AtomicUsize::new(1)),
        None,
        None,
        None,
        None,
        None,
    );
    assert_eq!(done.load(Ordering::Relaxed), 1);
    rx.try_iter().collect()
}

fn always() -> CacheDecision {
    CacheDecision::from_settings(&crate::settings::Settings {
        cache_policy: crate::settings::CachePolicy::Always,
        ..Default::default()
    })
}

#[test]
fn cover_auto_rar_worker_saves_v4r_and_reuses_judged_and_declined_without_sampling() {
    let _data = crate::data_dir::TestDataDirGuard::new();
    for (name, cropped) in [
        ("jacket.rar", true),
        ("cover-only.rar", false),
        ("two-pages.rar", false),
        ("zipdir.rar", true),
        ("crc-error.rar", true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let catalog = Arc::new(
            crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap(),
        );
        let dir = name == "zipdir.rar";
        let mut req = request(
            &fixture(name),
            if dir {
                "zipdir:book/"
            } else {
                "archivethumb:rar:book.rar"
            },
            if dir { "book/000.png" } else { "000.png" },
        );
        req.zip_dir_prefix = dir.then(|| "book/".to_owned());
        let key = cache_key_for_request(&req).unwrap().into_owned();
        assert!(key.contains("|autocrop:v4r:rtl"));
        // The shipped v4 NotAutomatic row must not suppress a new judgement.
        let old_key = key.replace("|autocrop:v4r:", "|autocrop:v4:");
        let bytes =
            crate::rar_loader::read_entry_bytes(&req.path, req.zip_entry.as_deref().unwrap())
                .unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        let (encoded, w, h) = crate::catalog::encode_thumb_webp(&image, 64, 75.0).unwrap();
        catalog
            .save(
                &old_key,
                7,
                9,
                w,
                h,
                Some((892, 512)),
                &encoded,
                &CoverIdentity::Unrotatable,
                AutoMaterial::NotAutomatic,
            )
            .unwrap();
        let before = crate::cover_auto_sample::container_sample_calls_for_test();
        let messages = run(&mut req, &catalog, always(), None);
        let material = messages
            .iter()
            .find_map(ThumbMsg::pixels)
            .unwrap()
            .auto_material;
        let AutoMaterial::Automatic(results) = material else {
            panic!("RAR was not judged")
        };
        assert_eq!(
            results.crops().iter().any(Option::is_some),
            cropped,
            "{name}"
        );
        assert_eq!(
            crate::cover_auto_sample::container_sample_calls_for_test(),
            before + 1
        );
        assert!(messages.last().unwrap().is_finalized());
        assert_eq!(
            catalog.load_one(&key).unwrap().unwrap().auto_material,
            material
        );
        req.source_policy = LoadSourcePolicy::CacheOnly;
        let messages = run(&mut req, &catalog, always(), None);
        assert_eq!(
            messages
                .iter()
                .find_map(ThumbMsg::pixels)
                .unwrap()
                .auto_material,
            material
        );
        assert_eq!(
            crate::cover_auto_sample::container_sample_calls_for_test(),
            before + 1
        );
        // A logical RAR tile shares its key with CachedZip. A ZipDir belongs
        // to its physical tree, so changing that path selects the ZIP v4 key.
        if !dir {
            req.path = temp.path().join("converted.zip");
            let messages = run(&mut req, &catalog, always(), None);
            assert_eq!(
                messages
                    .iter()
                    .find_map(ThumbMsg::pixels)
                    .unwrap()
                    .auto_material,
                material
            );
            assert_eq!(
                crate::cover_auto_sample::container_sample_calls_for_test(),
                before + 1
            );
        }
    }
}

#[test]
fn cover_auto_worker_sample_io_failure_displays_uncropped_and_does_not_force_save() {
    use std::io::Write;
    let _data = crate::data_dir::TestDataDirGuard::new();
    for extension in ["rar", "zip"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(format!("book.{extension}"));
        if extension == "rar" {
            std::fs::copy(fixture("jacket.rar"), &path).unwrap();
        } else {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            for index in 0..9 {
                let name = format!("{index:03}.png");
                let bytes =
                    crate::rar_loader::read_entry_bytes(&fixture("jacket.rar"), &name).unwrap();
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(&bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        let catalog = Arc::new(
            crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap(),
        );
        let mut req = request(
            &path,
            if extension == "rar" {
                "archivethumb:rar:book.rar"
            } else {
                "zipthumb:book.zip"
            },
            "000.png",
        );
        let key = cache_key_for_request(&req).unwrap().into_owned();
        let _hooks = Hooks;
        BEFORE_SAMPLE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || std::fs::remove_file(path).unwrap()))
        });
        let messages = run(&mut req, &catalog, always(), None);
        assert_eq!(
            messages
                .iter()
                .find_map(ThumbMsg::pixels)
                .unwrap()
                .auto_material,
            AutoMaterial::NotAutomatic
        );
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        assert!(messages.last().unwrap().is_finalized());
        assert!(catalog.load_one(&key).unwrap().is_none());
    }
}

#[test]
fn cover_auto_worker_sample_cancel_uses_existing_cancel_completion_without_saving() {
    let _data = crate::data_dir::TestDataDirGuard::new();
    let temp = tempfile::tempdir().unwrap();
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let mut req = request(
        &fixture("jacket.rar"),
        "archivethumb:rar:book.rar",
        "000.png",
    );
    let key = cache_key_for_request(&req).unwrap().into_owned();
    let cancel = Arc::new(AtomicBool::new(false));
    let token = cancel.clone();
    let _hooks = Hooks;
    BEFORE_SAMPLE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || token.store(true, Ordering::Relaxed)))
    });
    let messages = run(&mut req, &catalog, always(), Some(&cancel));
    assert!(messages.iter().all(|message| message.pixels().is_none()));
    assert!(messages.iter().any(ThumbMsg::is_canceled));
    assert!(catalog.load_one(&key).unwrap().is_none());
}

#[test]
fn cover_auto_rar_sampling_cost_is_cached_without_changing_threshold_or_policy() {
    let _data = crate::data_dir::TestDataDirGuard::new();
    let temp = tempfile::tempdir().unwrap();
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let mut req = request(
        &fixture("jacket.rar"),
        "archivethumb:rar:book.rar",
        "000.png",
    );
    req.force_cache = false;
    let decision = CacheDecision::from_settings(&crate::settings::Settings {
        cache_policy: crate::settings::CachePolicy::Auto,
        cache_threshold_ms: 1_000_000,
        cache_size_threshold_bytes: u64::MAX,
        cache_webp_always: false,
        cache_pdf_always: false,
        cache_zip_always: false,
        ..Default::default()
    });
    let _hooks = Hooks;
    AUTOMATIC_MS.with(|injected| injected.set(Some(1_000_001.0)));
    let before = crate::cover_auto_sample::container_sample_calls_for_test();
    run(&mut req, &catalog, decision, None);
    let key = cache_key_for_request(&req).unwrap().into_owned();
    assert!(catalog.load_one(&key).unwrap().is_some());
    req.source_policy = LoadSourcePolicy::CacheOnly;
    assert!(
        run(&mut req, &catalog, decision, None)
            .iter()
            .any(|message| message.pixels().is_some())
    );
    assert_eq!(
        crate::cover_auto_sample::container_sample_calls_for_test(),
        before + 1
    );
    assert!(!auto_cover_should_cache(
        decision,
        &req.path,
        9,
        1.0,
        1.0,
        1_000_001.0,
        AutoMaterial::NotAutomatic
    ));
}

#[test]
fn cover_auto_rar_updated_stamp_and_thumbnail_cache_clear_rejudge() {
    let _data = crate::data_dir::TestDataDirGuard::new();
    let temp = tempfile::tempdir().unwrap();
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let mut req = request(
        &fixture("jacket.rar"),
        "archivethumb:rar:book.rar",
        "000.png",
    );
    let before = crate::cover_auto_sample::container_sample_calls_for_test();
    run(&mut req, &catalog, always(), None);
    req.source_policy = LoadSourcePolicy::CacheOrSource;
    // A new source observation invalidates the same logical key by stamp.
    req.mtime += 1;
    run(&mut req, &catalog, always(), None);
    assert_eq!(
        crate::cover_auto_sample::container_sample_calls_for_test(),
        before + 2
    );
    let key = cache_key_for_request(&req).unwrap().into_owned();
    assert_eq!(catalog.load_one(&key).unwrap().unwrap().mtime, req.mtime);
    catalog.delete_one(&key).unwrap();
    run(&mut req, &catalog, always(), None);
    assert_eq!(
        crate::cover_auto_sample::container_sample_calls_for_test(),
        before + 3
    );
    assert!(matches!(
        catalog.load_one(&key).unwrap().unwrap().auto_material,
        AutoMaterial::Automatic(_)
    ));
}

#[test]
fn cover_auto_worker_analysis_decode_cancel_is_not_a_saved_not_automatic_decision() {
    use std::io::Write;
    let _data = crate::data_dir::TestDataDirGuard::new();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("book.zip");
    let jacket = crate::cover_auto_sample::rar_tests::fixture_jacket().to_rgb8();
    let jpeg = turbojpeg::compress_image(&jacket, 90, turbojpeg::Subsamp::Sub2x2)
        .unwrap()
        .to_vec();
    let body = crate::rar_loader::read_entry_bytes(&fixture("jacket.rar"), "001.png").unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for index in 0..9 {
        zip.start_file(
            format!("{index:03}.{}", if index == 0 { "jpg" } else { "png" }),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(if index == 0 { &jpeg } else { &body })
            .unwrap();
    }
    zip.finish().unwrap();
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let mut req = request(&path, "zipthumb:book.zip", "000.jpg");
    let key = cache_key_for_request(&req).unwrap().into_owned();
    let cancel = Arc::new(AtomicBool::new(false));
    let token = cancel.clone();
    let _hooks = Hooks;
    BEFORE_ANALYSIS.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || token.store(true, Ordering::Relaxed)))
    });
    let messages = run(&mut req, &catalog, always(), Some(&cancel));
    assert!(messages.iter().all(|message| message.pixels().is_none()));
    assert!(messages.iter().any(ThumbMsg::is_canceled));
    assert!(catalog.load_one(&key).unwrap().is_none());
}

#[test]
fn cover_auto_zip_deflate_corrupt_sample_is_judged_saved_and_cache_reused() {
    let _data = crate::data_dir::TestDataDirGuard::new();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("broken-deflate.zip");
    crate::zip_loader::cover_auto_deflate_corruption_fixture_for_test(&path);
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let mut req = request(&path, "zipthumb:broken-deflate.zip", "000.png");
    let before = crate::cover_auto_sample::container_sample_calls_for_test();
    let messages = run(&mut req, &catalog, always(), None);
    let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
    let AutoMaterial::Automatic(results) = pixels.auto_material else {
        panic!("valid body pages must be judged")
    };
    assert!(results.crops().iter().any(Option::is_some));
    assert_eq!(
        crate::cover_auto_sample::container_sample_calls_for_test(),
        before + 1
    );
    let key = cache_key_for_request(&req).unwrap().into_owned();
    assert_eq!(
        catalog.load_one(&key).unwrap().unwrap().auto_material,
        pixels.auto_material
    );
    req.source_policy = LoadSourcePolicy::CacheOnly;
    let cached = run(&mut req, &catalog, always(), None);
    assert_eq!(
        cached
            .iter()
            .find_map(ThumbMsg::pixels)
            .unwrap()
            .auto_material,
        pixels.auto_material
    );
    assert_eq!(
        crate::cover_auto_sample::container_sample_calls_for_test(),
        before + 1
    );
}
