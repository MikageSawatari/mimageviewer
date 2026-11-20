//! App fixture policy and teardown, without native AI initialization.
use super::*;
use crate::ai::runtime::{AiRuntimeInitOwner, AiRuntimeInitSnapshot, AiRuntimeInitWait};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn not_started_keeps_the_configured_owner_and_trt_lifecycle() {
    let env = phase_c_support::setup_app_with_ai_runtime(AiRuntimeFixture::Start);
    let owner = Arc::clone(&env.ai_runtime_init);
    let lifecycle = owner.trt_worker_lifecycle();
    let before = lifecycle.snapshot();
    AiRuntimeFixture::NotStarted.configure(&env);
    assert!(Arc::ptr_eq(&owner, &env.ai_runtime_init));
    assert!(Arc::ptr_eq(
        &lifecycle,
        &env.ai_runtime_init.trt_worker_lifecycle()
    ));
    assert_eq!(before, lifecycle.snapshot());
    assert!(matches!(owner.snapshot(), AiRuntimeInitSnapshot::Failed(_)));
}

#[test]
fn not_started_first_full_frame_preserves_failed_owner_without_extracting_ort() {
    let mut env = phase_c_support::setup_app_with_ai_runtime(AiRuntimeFixture::NotStarted);
    let owner = Arc::clone(&env.ai_runtime_init);
    let lifecycle = owner.trt_worker_lifecycle();
    let lifecycle_before = lifecycle.snapshot();
    let failure = match owner.snapshot() {
        AiRuntimeInitSnapshot::Failed(error) => error,
        _ => panic!("NotStarted must immediately be Failed"),
    };
    assert!(
        failure
            .to_string()
            .contains(AiRuntimeFixture::NOT_STARTED_MESSAGE)
    );
    assert!(!env.initialized);
    let folder = env.tmp.path().join("empty-startup");
    std::fs::create_dir(&folder).unwrap();
    env.settings.startup_folder_mode = crate::settings::StartupFolderMode::Specific;
    env.settings.startup_folder_path = Some(folder);
    env.settings.update_check_enabled = false;
    env.settings.creative_luts.clear();
    env.creative_lut_library = crate::creative_lut::CreativeLutLibrary::default();
    // Match the real-Full fixtures' terminal search startup and stable focus.
    // Keep initialized=false so render_initial_open still calls ensure_ai_runtime.
    env.indexer_init = IndexerInit::Unavailable;
    env.last_main_focused = true;
    assert!(!env.indexer_init.overlay_active());
    let ctx = egui::Context::default();
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    backend
        .update(
            &mut env.app,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 640.0),
                )),
                ..Default::default()
            },
        )
        .drop_without_applying_deltas();
    assert!(
        env.initialized,
        "the real first Full must reach initial_open"
    );
    assert!(Arc::ptr_eq(&owner, &env.ai_runtime_init));
    assert!(Arc::ptr_eq(
        &lifecycle,
        &env.ai_runtime_init.trt_worker_lifecycle()
    ));
    assert_eq!(lifecycle_before, lifecycle.snapshot());
    match owner.snapshot() {
        AiRuntimeInitSnapshot::Failed(error) => assert!(Arc::ptr_eq(&failure, &error)),
        _ => panic!("the first Full must not start ai-runtime-init"),
    }
    for filename in ["onnxruntime.dll", "onnxruntime_providers_shared.dll"] {
        assert!(
            !env.tmp.path().join(filename).exists(),
            "{filename} was extracted"
        );
    }
}

/// The initializer touches only disposable files and returns before publishing Failed,
/// just as the production initializer finishes DLL access before terminal publication.
fn controlled_initializer(
    app: &App,
) -> (
    Arc<AiRuntimeInitOwner>,
    std::sync::mpsc::Sender<()>,
    std::sync::mpsc::Receiver<bool>,
) {
    let owner = Arc::clone(&app.ai_runtime_init);
    assert!(matches!(owner.snapshot(), AiRuntimeInitSnapshot::Dormant));
    let dir = crate::data_dir::get();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (touched_tx, touched_rx) = std::sync::mpsc::channel();
    assert!(owner.start_for_test(move || {
        release_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("release fixture initializer");
        let touched = std::fs::write(dir.join("fixture-init-finished"), b"done").is_ok();
        touched_tx.send(touched).unwrap();
        Err(crate::ai::AiError::Ort(
            "fixture terminal failure".to_owned(),
        ))
    }));
    assert!(matches!(
        owner.snapshot(),
        AiRuntimeInitSnapshot::Initializing
    ));
    (owner, release_tx, touched_rx)
}

fn release_on_cleanup_wait(
    cleanup: &mut AiRuntimeFixtureCleanup,
    release: &std::sync::mpsc::Sender<()>,
) -> Arc<AtomicBool> {
    let called = Arc::new(AtomicBool::new(false));
    let hook_called = Arc::clone(&called);
    let release = release.clone();
    cleanup.wait_hook = Some(Box::new(move || {
        hook_called.store(true, Ordering::SeqCst);
        release.send(()).unwrap();
    }));
    called
}

fn drop_fixture_and_assert_cleanup(
    owner: &AiRuntimeInitOwner,
    release: std::sync::mpsc::Sender<()>,
    touched: std::sync::mpsc::Receiver<bool>,
    dir: &Path,
    hook_called: &AtomicBool,
    drop_fixture: impl FnOnce(),
) {
    let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(drop_fixture));
    // Recover even if Drop panicked or its wait was removed. Use the owner directly:
    // the fixture wait itself is the subject of the destructive regression check.
    let hook_ran = hook_called.load(Ordering::SeqCst);
    if !hook_ran {
        let _ = release.send(());
    }
    let terminal = owner.wait_terminal_while(|| true);
    let written = touched.recv_timeout(std::time::Duration::from_secs(1));
    assert!(
        matches!(terminal, AiRuntimeInitWait::Failed(_)),
        "recover the controlled initializer before checking teardown assertions"
    );
    if let Err(panic) = dropped {
        std::panic::resume_unwind(panic);
    }
    assert!(
        hook_ran,
        "env drop must release the initializer from inside its cleanup wait"
    );
    assert!(
        matches!(written, Ok(true)),
        "initializer must write successfully before TempDir removal: {written:?}"
    );
    assert!(
        !dir.exists(),
        "TempDir must be removed after initialization"
    );
}

#[test]
fn start_app_fixture_waits_before_removing_data_dir() {
    let mut env = phase_c_support::setup_app_with_ai_runtime(AiRuntimeFixture::Start);
    let dir = env.tmp.path().to_path_buf();
    let (owner, release, touched) = controlled_initializer(&env);
    let hook_called = release_on_cleanup_wait(&mut env.ai_runtime_cleanup, &release);
    drop_fixture_and_assert_cleanup(&owner, release, touched, &dir, &hook_called, || drop(env));
}

#[test]
fn start_replacement_tracks_new_app_owner_through_cleanup() {
    for drop_before_reload in [false, true] {
        let mut env = phase_c_support::setup_app();
        let old_owner = Arc::clone(&env.ai_runtime_init);
        let dir = env.tmp.path().to_path_buf();
        let settings = env.settings.clone();
        if drop_before_reload {
            drop(env.app);
            env.app = env
                .ai_runtime_cleanup
                .app_from_settings_after_drop(settings, AiRuntimeFixture::Start);
        } else {
            env.replace_app_from_settings(settings, AiRuntimeFixture::Start);
        }
        assert!(!Arc::ptr_eq(&old_owner, &env.ai_runtime_init));
        assert!(Arc::ptr_eq(
            env.ai_runtime_cleanup
                .owner
                .as_ref()
                .expect("Start cleanup owner"),
            &env.ai_runtime_init
        ));
        let (owner, release, touched) = controlled_initializer(&env);
        let hook_called = release_on_cleanup_wait(&mut env.ai_runtime_cleanup, &release);
        drop_fixture_and_assert_cleanup(&owner, release, touched, &dir, &hook_called, || drop(env));
    }
}

#[test]
fn start_history_case_fixture_waits_before_removing_data_dir() {
    let mut env = history_parent_1473::setup_with_ai_runtime(false, AiRuntimeFixture::Start);
    let dir = crate::data_dir::get();
    let (owner, release, touched) = controlled_initializer(&env);
    let hook_called = release_on_cleanup_wait(&mut env.ai_runtime_cleanup, &release);
    drop_fixture_and_assert_cleanup(&owner, release, touched, &dir, &hook_called, || drop(env));
}

#[test]
fn cleanup_enters_terminal_wait_before_releasing_initializer() {
    let env = phase_c_support::setup_app_with_ai_runtime(AiRuntimeFixture::Start);
    let (owner, release, touched) = controlled_initializer(&env);
    let mut cleanup = AiRuntimeFixtureCleanup::new(&env, AiRuntimeFixture::Start);
    let mut release = Some(release);
    // Release from inside the actual wait predicate: this cannot pass merely
    // because the worker finished before cleanup inspected Initializing.
    cleanup.wait_while(|| {
        if let Some(release) = release.take() {
            release.send(()).unwrap();
        }
    });
    assert!(release.is_none(), "cleanup must enter the terminal wait");
    assert!(matches!(owner.snapshot(), AiRuntimeInitSnapshot::Failed(_)));
    assert!(
        touched
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
    );
    assert!(env.tmp.path().join("fixture-init-finished").exists());
}

// Preserve byte offsets/newlines while masking comments and literals. This is
// only a lexical audit, not Rust name resolution or macro expansion.
fn rust_code_only(source: &str) -> String {
    fn literal_end(source: &str, start: usize) -> Option<usize> {
        let bytes = source.as_bytes();
        match bytes[start] {
            b'r' => {
                let mut quote = start + 1;
                while bytes.get(quote) == Some(&b'#') {
                    quote += 1;
                }
                if bytes.get(quote) != Some(&b'"') {
                    return None;
                }
                let hashes = &bytes[start + 1..quote];
                let mut end = quote + 1;
                while end < bytes.len() {
                    if bytes[end] == b'"'
                        && bytes.get(end + 1..end + 1 + hashes.len()) == Some(hashes)
                    {
                        return Some(end + 1 + hashes.len());
                    }
                    end += 1;
                }
                Some(bytes.len())
            }
            b'"' => {
                let mut end = start + 1;
                while end < bytes.len() {
                    match bytes[end] {
                        b'\\' => end = (end + 2).min(bytes.len()),
                        b'"' => return Some(end + 1),
                        _ => end += 1,
                    }
                }
                Some(bytes.len())
            }
            b'\'' => {
                // A lifetime/label has no closing quote after one character.
                let mut end = start + 1;
                if bytes.get(end) == Some(&b'\\') {
                    end = (end + 2).min(bytes.len());
                    while end < bytes.len() && !matches!(bytes[end], b'\'' | b'\n' | b'\r') {
                        end += 1;
                    }
                } else {
                    end += source[end..].chars().next()?.len_utf8();
                }
                (bytes.get(end) == Some(&b'\'')).then_some(end + 1)
            }
            _ => None,
        }
    }

    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let end = if bytes[at..].starts_with(b"//") {
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            at
        } else if bytes[at..].starts_with(b"/*") {
            at += 2;
            let mut depth = 1;
            while at < bytes.len() && depth > 0 {
                if bytes[at..].starts_with(b"/*") {
                    depth += 1;
                    at += 2;
                } else if bytes[at..].starts_with(b"*/") {
                    depth -= 1;
                    at += 2;
                } else {
                    at += 1;
                }
            }
            at
        } else if let Some(end) = literal_end(source, at) {
            end
        } else {
            at += 1;
            continue;
        };
        for byte in &mut code[start..end] {
            if !matches!(*byte, b'\n' | b'\r') {
                *byte = b' ';
            }
        }
        at = end;
    }
    String::from_utf8(code).expect("mask complete UTF-8 comments and literals")
}

// Auxiliary audit limits: no macro expansion, inferred Default construction
// (let app: App = Default::default()), or calls from other crates.
fn scan_app_constructor_references(source: &str) -> (String, Vec<std::ops::Range<usize>>) {
    static PATTERNS: std::sync::OnceLock<[regex::Regex; 5]> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"\bApp\s*::\s*(?:new_from_settings\w*|new|default)\b",
            r"\bSelf\s*::\s*new_from_settings\w*\b",
            r"<\s*(?:crate\s*::\s*app\s*::\s*)?App\s+as\s+",
            r"\btype\s+\w+\s*=\s*(?:crate\s*::\s*app\s*::\s*)?App\s*;",
            r"\buse\s+[^;]*\bApp\s+as\s+\w+",
        ]
        .map(|pattern| regex::Regex::new(pattern).expect("App constructor audit pattern"))
    });
    let code = rust_code_only(source);
    let mut references: Vec<_> = patterns
        .iter()
        .flat_map(|pattern| pattern.find_iter(&code).map(|found| found.range()))
        .collect();
    references.sort_by_key(|reference| reference.start);
    references.dedup();
    (code, references)
}

#[test]
fn app_constructor_scanner_detects_indirect_references_without_comments_or_literals() {
    let cases = [
        ("App::new_from_settings(settings);", 1),
        ("let make = App::new_from_settings;", 1),
        ("let make = App::new_from_settings_with_load_meta;", 1),
        ("let make = App::new;", 1),
        ("let make = App::default;", 1),
        ("let make = crate::app::App\n::\ndefault;", 1),
        ("let make = Self::new_from_settings;", 1),
        ("let make = Self::new_from_settings_with_load_meta;", 1),
        ("let make = <App as Default>::default;", 1),
        (
            "let make = < crate::app::App as std::default::Default >::default;",
            1,
        ),
        ("type FixtureApp = App;", 1),
        ("type FixtureApp = crate::app::App;", 1),
        ("use crate::app::App as FixtureApp;", 1),
        ("use super::App as FixtureApp;", 1),
        ("use crate::app::{App as FixtureApp, GridItem};", 1),
        ("App::something_else();", 0),
        ("App::default_settings();", 0),
        (
            "// let make = App::new_from_settings;\nApp::something_else();",
            0,
        ),
        (
            "/* App::new /* Self::new_from_settings */ App::default */",
            0,
        ),
        (
            r#"let text = "App::new_from_settings"; App::something_else();"#,
            0,
        ),
        (
            r##"let text = r#"a quote: " App::new_from_settings "#;"##,
            0,
        ),
        (r#"let quote = '"'; let make = App::new;"#, 1),
        (r"let slash = '\\'; let make = App::default;", 1),
        ("let letter = 'あ'; let make = App::new;", 1),
        ("fn fixture<'a>(_: &'a str) { App::default; }", 1),
        (
            "// App::new\nlet make = App::default; // Self::new_from_settings\n",
            1,
        ),
    ];
    for (source, expected) in cases {
        let (code, references) = scan_app_constructor_references(source);
        assert_eq!(references.len(), expected, "{source}");
        assert_eq!(code.len(), source.len(), "preserve diagnostic byte offsets");
        assert_eq!(
            code.bytes().filter(|byte| *byte == b'\n').count(),
            source.bytes().filter(|byte| *byte == b'\n').count(),
            "preserve diagnostic lines"
        );
    }
}

// Like ime_focus's raw TextEdit inventory: exact path/anchor/count exemptions,
// with stale exemptions rejected. Only these two test helpers may construct App.
#[test]
fn app_test_constructors_require_explicit_ai_runtime_fixture() {
    fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read Rust source directory") {
            let path = entry.expect("read Rust source entry").path();
            if path.is_dir() {
                collect(&path, files);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    let exemptions = [
        (
            "src/app/tests.rs",
            "let app = App::new_from_settings(settings);",
            "settings-reload helper applies the mandatory fixture policy immediately",
        ),
        (
            "src/app/tests.rs",
            "App::new_from_settings_with_load_meta_book_query_repaint_and_similar_capability(",
            "AppTestConfig helper applies the mandatory fixture policy immediately",
        ),
        (
            "src/lib.rs",
            "app::App::new_from_settings_with_load_meta_and_raw_executor(",
            "production GUI constructor is outside the test fixture policy",
        ),
        (
            "src/app.rs",
            "Self::new_from_settings(crate::settings::Settings::load())",
            "production App Default delegates to its settings constructor",
        ),
        (
            "src/app.rs",
            "Self::new_from_settings_with_load_meta(",
            "production App constructor delegates with default load metadata",
        ),
        (
            "src/app.rs",
            "Self::new_from_settings_with_load_meta_and_book_query_repaint(",
            "production App constructor delegates with a repaint callback",
        ),
        (
            "src/app.rs",
            "Self::new_from_settings_with_load_meta_and_raw_executor(",
            "production App constructor delegates with its RAW executor",
        ),
        (
            "src/app.rs",
            "Self::new_from_settings_with_load_meta_book_query_repaint_and_similar_capability(",
            "production App constructor delegates with product Similar capability",
        ),
        (
            "src/app/tests.rs",
            "type Target = App;",
            "AppTestEnv Deref associated type, not an alternate constructor alias",
        ),
        (
            "src/app/tests/history_parent_1473.rs",
            "type Target = App;",
            "CaseEnv Deref associated type, not an alternate constructor alias",
        ),
        (
            "src/app/multiwindow_scenario_tests.rs",
            "<App as eframe::App>::logic(",
            "headless driver invokes existing App logic without constructing another App",
        ),
        (
            "src/app/multiwindow_scenario_tests.rs",
            "<App as eframe::App>::ui(",
            "headless driver invokes existing App UI without constructing another App",
        ),
        (
            "src/app/similar_navigation_tests.rs",
            "<App as eframe::App>::logic(",
            "headless driver invokes existing App logic without constructing another App",
        ),
        (
            "src/app/similar_navigation_tests.rs",
            "<App as eframe::App>::ui(",
            "headless driver invokes existing App UI without constructing another App",
        ),
    ];
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect(&root.join("src"), &mut files);
    collect(&root.join("tests"), &mut files);
    files.sort();
    let mut counts = vec![0usize; exemptions.len()];
    let mut violations = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let source = std::fs::read_to_string(&path).expect("read UTF-8 Rust source");
        let (code, references) = scan_app_constructor_references(&source);
        for reference in references {
            let line_start = code[..reference.start].rfind('\n').map_or(0, |idx| idx + 1);
            let line_end = code[reference.start..]
                .find('\n')
                .map_or(code.len(), |idx| reference.start + idx);
            let line = &code[line_start..line_end];
            if let Some(idx) = exemptions.iter().position(|(file, anchor, reason)| {
                *file == relative && line.contains(*anchor) && !reason.is_empty()
            }) {
                counts[idx] += 1;
            } else {
                let number = source[..reference.start]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
                    + 1;
                violations.push(format!(
                    "{relative}:{number}: {}",
                    source[line_start..line_end].trim()
                ));
            }
        }
    }
    for ((file, anchor, reason), count) in exemptions.iter().zip(counts) {
        assert_eq!(
            count, 1,
            "stale App constructor exemption: {file} / {anchor} ({reason})"
        );
    }
    assert!(
        violations.is_empty(),
        "App tests must use an explicit AiRuntimeFixture helper:\n{}",
        violations.join("\n")
    );
}
