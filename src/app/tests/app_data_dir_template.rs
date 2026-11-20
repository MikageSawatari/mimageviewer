//! Empty DB templates are built by this executable in a separate process.
use super::*;
use crate::similar_index::SimilarFeatureCapability;
use rusqlite::{Connection, types::Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::time::{Duration, Instant, UNIX_EPOCH};

const FORMAT_VERSION: u32 = 1;
const BUILDER_TEST: &str = "app::tests::app_data_dir_template::build_template_in_child";
const BUILDER_DATA_DIR: &str = "MIV_TEST_TEMPLATE_DATA_DIR";
const BUILDER_CAPABILITY: &str = "MIV_TEST_TEMPLATE_SIMILAR_CAPABILITY";
const BUILDER_DONE: &str = "builder-done";
const MANIFEST: &str = "manifest.txt";
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

fn capability_name(capability: SimilarFeatureCapability) -> &'static str {
    match capability {
        SimilarFeatureCapability::Enabled => "enabled",
        SimilarFeatureCapability::Paused => "paused",
    }
}

fn template_key(
    exe_stem: &OsStr,
    exe_len: u64,
    exe_mtime_ns: u128,
    capability: SimilarFeatureCapability,
    format_version: u32,
) -> PathBuf {
    PathBuf::from(exe_stem)
        .join(format!("{exe_len}-{exe_mtime_ns}"))
        .join(format!("v{format_version}-{}", capability_name(capability)))
}

fn template_location(capability: SimilarFeatureCapability) -> PathBuf {
    // Recompute on every use: distinct linked generations must never share a cache.
    let exe = std::env::current_exe().expect("test executable path");
    let metadata = std::fs::metadata(&exe).expect("test executable metadata");
    let modified = metadata
        .modified()
        .expect("test executable mtime")
        .duration_since(UNIX_EPOCH)
        .expect("test executable mtime after epoch")
        .as_nanos();
    exe.parent()
        .expect("test executable parent")
        .join("app-test-template")
        .join(template_key(
            exe.file_stem().expect("test executable stem"),
            metadata.len(),
            modified,
            capability,
            FORMAT_VERSION,
        ))
}

pub(super) fn assert_empty_destination(data_dir: &Path) {
    assert!(
        std::fs::read_dir(data_dir)
            .expect("read template destination")
            .next()
            .is_none(),
        "Template requires an empty data_dir before new_for_test writes settings.json: {}",
        data_dir.display()
    );
}

fn is_template_db(name: &str) -> bool {
    name != "settings.db" && name.ends_with(".db")
}

fn database_names(data_dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(data_dir).expect("read database directory") {
        let entry = entry.expect("read database entry");
        let name = entry.file_name().into_string().expect("UTF-8 DB file name");
        if entry.file_type().expect("DB file type").is_file() && is_template_db(&name) {
            names.push(name);
        }
    }
    names.sort();
    names
}

fn read_manifest(template: &Path) -> Vec<String> {
    let manifest =
        std::fs::read_to_string(template.join(MANIFEST)).expect("read template manifest");
    let names: Vec<_> = manifest.lines().map(str::to_owned).collect();
    assert!(!names.is_empty(), "template manifest must contain DBs");
    let mut unique = BTreeSet::new();
    for name in &names {
        assert!(
            is_template_db(name)
                && !name.chars().any(|c| matches!(c, '/' | '\\' | ':'))
                && unique.insert(name),
            "invalid or duplicate template DB name: {name}"
        );
    }
    names
}

fn validate_builder_output(exit_code: Option<i32>, work: &Path) -> Vec<String> {
    assert_eq!(exit_code, Some(0), "template builder must exit with code 0");
    assert!(
        work.join(BUILDER_DONE).is_file(),
        "template builder marker missing"
    );
    let names = database_names(work);
    assert!(!names.is_empty(), "template builder produced zero DBs");
    names
}

fn checkpoint_database(path: &Path) {
    let conn = Connection::open(path).expect("open template DB for checkpoint");
    conn.busy_timeout(RESPONSE_TIMEOUT).unwrap();
    let (busy, _, _): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("checkpoint template DB");
    assert_eq!(busy, 0, "checkpoint busy: {}", path.display());
    conn.close().expect("close checkpoint connection");
    assert_empty_wal(path);
}

fn assert_empty_wal(path: &Path) {
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    match std::fs::metadata(PathBuf::from(wal)) {
        Ok(metadata) => assert_eq!(metadata.len(), 0, "non-empty WAL: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("inspect WAL for {}: {error}", path.display()),
    }
}

fn ensure_template(capability: SimilarFeatureCapability) -> PathBuf {
    // Caller holds test_override_lock throughout construction, publication and copying.
    let template = template_location(capability);
    if template.join(MANIFEST).is_file() {
        return template;
    }
    let mut temporary_name = template.file_name().unwrap().to_os_string();
    temporary_name.push(format!(".tmp-{}", std::process::id()));
    let temporary = template.with_file_name(temporary_name);
    std::fs::create_dir_all(template.parent().unwrap()).expect("create template parent");
    std::fs::create_dir(&temporary).expect("create private template directory");
    let work = temporary.join("data");
    std::fs::create_dir(&work).expect("create builder data_dir");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([BUILDER_TEST, "--exact", "--ignored", "--nocapture"])
        .env(BUILDER_DATA_DIR, &work)
        .env(BUILDER_CAPABILITY, capability_name(capability))
        .status()
        .expect("start template builder child");
    let names = validate_builder_output(status.code(), &work);
    for name in &names {
        let source = work.join(name);
        checkpoint_database(&source);
        std::fs::copy(&source, temporary.join(name)).expect("copy checkpointed template DB");
    }
    std::fs::write(temporary.join(MANIFEST), format!("{}\n", names.join("\n")))
        .expect("write template manifest");
    std::fs::remove_dir_all(&work).expect("remove private builder data_dir");
    match std::fs::rename(&temporary, &template) {
        Ok(()) => {}
        Err(_) if template.join(MANIFEST).is_file() => {
            // Another process published this immutable generation first.
            std::fs::remove_dir_all(&temporary).expect("remove losing private template");
        }
        Err(error) => panic!("publish template {}: {error}", template.display()),
    }
    template
}

pub(super) fn copy_template(data_dir: &Path, capability: SimilarFeatureCapability) {
    let template = ensure_template(capability);
    for name in read_manifest(&template) {
        std::fs::copy(template.join(&name), data_dir.join(&name))
            .unwrap_or_else(|error| panic!("copy template DB {name}: {error}"));
    }
}

#[test]
#[ignore = "only the parent fixture runs this with its private data_dir and capability"]
fn build_template_in_child() {
    let data_dir =
        PathBuf::from(std::env::var_os(BUILDER_DATA_DIR).expect("builder data_dir required"));
    let capability = match std::env::var(BUILDER_CAPABILITY).as_deref() {
        Ok("enabled") => SimilarFeatureCapability::Enabled,
        Ok("paused") => SimilarFeatureCapability::Paused,
        other => panic!("builder capability required: {other:?}"),
    };
    let _lock = crate::data_dir::test_override_lock();
    crate::data_dir::set_test_override(Some(data_dir.clone()));
    let _guard = phase_c_support::OverrideGuard;
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let app = App::new_for_test(AppTestConfig {
        ai_runtime: AiRuntimeFixture::NotStarted,
        data_dir_fixture: AppDataDirFixture::Fresh,
        data_dir: data_dir.clone(),
        settings: None,
        similar_feature_capability: capability,
    });
    drop(app);
    std::fs::write(data_dir.join(BUILDER_DONE), b"done").expect("write builder completion marker");
}

fn receive<T>(mut poll: impl FnMut() -> Result<T, std::sync::mpsc::TryRecvError>) -> T {
    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    loop {
        match poll() {
            Ok(value) => return value,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < deadline, "worker response timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("worker response failed: {error}"),
        }
    }
}

fn assert_initial_responses(app: &mut App) {
    // Enumerated from the constructor: each opens its own connection inside a worker.
    let loader = app
        .content_identity_index_load_pending
        .as_ref()
        .expect("identity loader");
    assert_eq!(
        receive(|| loader.try_recv())
            .expect("identity initial load")
            .len(),
        0
    );
    let bookmarks = app
        .book_bookmark_service
        .as_ref()
        .expect("bookmark service");
    bookmarks.list_all(817);
    match receive(|| bookmarks.try_recv()) {
        crate::book_bookmarks::BookBookmarkEvent::AllListed { request_id, result } => {
            assert_eq!(request_id, 817);
            assert!(result.expect("bookmark initial list").is_empty());
        }
        _ => panic!("unexpected bookmark reply"),
    }
    let stats = app
        .collection_auto_aspect_cache
        .as_mut()
        .expect("collection aspect worker")
        .begin_maintenance(crate::auto_aspect_cache::CollectionAutoAspectMaintenance::Count)
        .expect("request aspect count")
        .recv_timeout(RESPONSE_TIMEOUT)
        .expect("aspect count response")
        .expect("aspect initialization");
    assert_eq!(stats.remaining, 0);
    assert_eq!(stats.deleted, 0);
    let meters = app
        .book_resume_writer
        .as_ref()
        .expect("resume writer")
        .read_all()
        .recv_timeout(RESPONSE_TIMEOUT)
        .expect("resume read response")
        .expect("resume initialization and meter migration");
    assert!(meters.values.is_empty());
    let history = app
        .reading_history_writer
        .as_ref()
        .expect("history writer")
        .read_recent_for_test()
        .recv_timeout(RESPONSE_TIMEOUT)
        .expect("history read response")
        .expect("history writer initialization");
    assert!(history.is_empty());
}

#[test]
fn fresh_start_opens_every_constructor_db_and_worker() {
    let mut env = phase_c_support::setup_app_with_fresh_data_dir();
    // Keep this inventory aligned with the synchronous DB opens in App's constructor.
    macro_rules! present {
        ($($field:ident),+ $(,)?) => {
            $(assert!(env.$field.is_some(), stringify!($field));)+
        };
    }
    present!(
        archive_cache_db,
        edit_preview_cache,
        search_index_db,
        rotation_db,
        audio_normalize_db,
        video_pin_db,
        folder_thumb_pin_db,
        video_bookmark_db,
        book_bookmark_service,
        video_chapter_thumb_db,
        video_tile_cache,
        rating_db,
        tags_db,
        view_trim_db,
        content_identity_recorder,
        book_resume_db,
        book_resume_writer,
        reading_history_db,
        reading_history_writer,
        auto_aspect_cache_db,
        collection_auto_aspect_cache,
        adjustment_db,
        local_adjust_db,
        export_crop_db,
        mask_db,
        conceal_db,
        comic_db,
        comic_user_stamp_db,
    );
    let spread = env.spread_db.as_ref().expect("spread DB open");
    spread
        .set_mode_only(
            Path::new("C:/fixture/spread.zip"),
            crate::settings::SpreadMode::Single,
        )
        .expect("fresh spread DB must not use read-only recovery");
    assert_initial_responses(&mut env);

    // The index loader's success is separate from the recorder's connection. Exercise
    // the real recorder too, using its existing successful-ledger-update accessor.
    let source = env.tmp.path().join("identity-probe.png");
    image::RgbaImage::new(2, 2).save(&source).unwrap();
    let recorder = env.content_identity_recorder.as_ref().unwrap();
    recorder.record(
        crate::content_identity::ContentIdentitySource::from_path(&source).unwrap(),
        crate::content_identity::ContentIdentityTrigger::Edit,
    );
    let entry = recorder
        .recv_update_for_test(RESPONSE_TIMEOUT)
        .expect("recorder initialized and wrote");
    assert_eq!(
        entry.file_key,
        crate::path_key::normalize_keep_drive(&source)
    );
}

#[derive(Debug, PartialEq)]
struct DatabaseSnapshot {
    definitions: Vec<Vec<Value>>,
    user_version: i64,
    tables: BTreeMap<String, Vec<Vec<Value>>>,
}

fn query_values(conn: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut statement = conn.prepare(sql).unwrap();
    let columns = statement.column_count();
    let mut rows: Vec<Vec<Value>> = statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get::<_, Value>(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    // SQLite values have no total ordering; Debug retains both type and value.
    rows.sort_by_cached_key(|row| format!("{row:?}"));
    rows
}

fn snapshot(path: &Path) -> DatabaseSnapshot {
    let conn = Connection::open(path).unwrap();
    conn.busy_timeout(RESPONSE_TIMEOUT).unwrap();
    let definitions = query_values(&conn, "SELECT type, name, tbl_name, sql FROM sqlite_master");
    let user_version = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let mut tables = BTreeMap::new();
    for definition in &definitions {
        if definition[0] != Value::Text("table".into()) {
            continue;
        }
        let Value::Text(name) = &definition[1] else {
            panic!("table name must be text")
        };
        let quoted = name.replace('"', "\"\"");
        let mut rows = query_values(&conn, &format!("SELECT * FROM \"{quoted}\""));
        if name == "folder_thumb_pin_revision" {
            assert_eq!(rows.len(), 1, "exactly one pin revision row");
            let columns = conn
                .prepare(&format!("SELECT * FROM \"{quoted}\""))
                .unwrap();
            let instance = columns.column_index("instance_id").unwrap();
            let revision = columns.column_index("revision").unwrap();
            assert_eq!(rows[0][revision], Value::Integer(0));
            let Value::Text(id) = &rows[0][instance] else {
                panic!("instance_id must be text")
            };
            let uuid = uuid::Uuid::parse_str(id).expect("pin instance UUID");
            assert_eq!(&uuid.to_string(), id, "canonical UUID format");
            rows[0][instance] = Value::Text("<instance UUID>".into());
        }
        tables.insert(name.clone(), rows);
    }
    DatabaseSnapshot {
        definitions,
        user_version,
        tables,
    }
}

fn non_db_names(data_dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(data_dir)
        .unwrap()
        .map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if let Some(timestamp) = name.strip_prefix("settings.json.migrated-") {
                assert!(!timestamp.is_empty() && timestamp.bytes().all(|b| b.is_ascii_digit()));
                "settings.json.migrated-<time>".to_owned()
            } else {
                name
            }
        })
        .filter(|name| !name.ends_with(".db"))
        .collect()
}

fn drop_app_before_file_comparison(app: App) {
    // This service does not join its worker in Drop. Retain its DB until the
    // worker releases the last other Arc, then close it before listing sidecars.
    let preview_db = app.edit_preview_cache.as_ref().expect("preview cache").db();
    drop(app);
    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    while Arc::strong_count(&preview_db) != 1 {
        assert!(Instant::now() < deadline, "preview worker did not close");
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(preview_db);
}

fn assert_template_matches_fresh(with_settings: bool, capability: SimilarFeatureCapability) {
    // Retain each TempDir after dropping App and before releasing its override/lock.
    // Sequential scopes avoid re-entering the process-global fixture lock.
    let settings_root = tempfile::tempdir().unwrap();
    let settings = |root: &Path| {
        with_settings.then(|| {
            let mut settings = crate::settings::Settings::default();
            settings.book_root = Some(root.join("books"));
            settings
        })
    };
    let (fresh_names, fresh_non_db, fresh_snapshots) = {
        let mut env = phase_c_support::setup_app_with_options(
            capability,
            AiRuntimeFixture::NotStarted,
            AppDataDirFixture::Fresh,
            settings(settings_root.path()),
            |_| {},
        );
        assert_initial_responses(&mut env);
        drop_app_before_file_comparison(env.app);
        let names = database_names(env.tmp.path());
        let snapshots: BTreeMap<_, _> = names
            .iter()
            .map(|name| (name.clone(), snapshot(&env.tmp.path().join(name))))
            .collect();
        let non_db = non_db_names(env.tmp.path());
        (names, non_db, snapshots)
    };
    let mut env = phase_c_support::setup_app_with_options(
        capability,
        AiRuntimeFixture::NotStarted,
        AppDataDirFixture::Template,
        settings(settings_root.path()),
        |_| {},
    );
    assert_initial_responses(&mut env);
    let template = template_location(capability);
    let manifest = read_manifest(&template);
    assert_eq!(
        manifest.iter().collect::<BTreeSet<_>>(),
        fresh_names.iter().collect::<BTreeSet<_>>()
    );
    assert_eq!(
        manifest,
        database_names(&template),
        "manifest covers all template DBs"
    );
    drop_app_before_file_comparison(env.app);
    assert_eq!(fresh_names, database_names(env.tmp.path()));
    for name in manifest {
        assert_eq!(
            fresh_snapshots[&name],
            snapshot(&env.tmp.path().join(&name)),
            "template/fresh DB mismatch: {name}, settings={with_settings}, capability={capability:?}"
        );
    }
    assert_eq!(fresh_non_db, non_db_names(env.tmp.path()));
}

#[test]
fn template_matches_fresh_without_settings() {
    assert_template_matches_fresh(false, SimilarFeatureCapability::Enabled);
}

#[test]
fn template_matches_fresh_with_book_root_settings() {
    assert_template_matches_fresh(true, SimilarFeatureCapability::Enabled);
}

#[test]
fn paused_template_matches_fresh() {
    assert_template_matches_fresh(false, SimilarFeatureCapability::Paused);
    assert_template_matches_fresh(true, SimilarFeatureCapability::Paused);
}

#[test]
fn template_key_separates_executable_generations_and_fixture_conditions() {
    let key = |stem: &str, len, time, capability, version| {
        template_key(OsStr::new(stem), len, time, capability, version)
    };
    let base = key("app", 10, 20, SimilarFeatureCapability::Enabled, 1);
    for different in [
        key("other", 10, 20, SimilarFeatureCapability::Enabled, 1),
        key("app", 11, 20, SimilarFeatureCapability::Enabled, 1),
        key("app", 10, 21, SimilarFeatureCapability::Enabled, 1),
        key("app", 10, 20, SimilarFeatureCapability::Paused, 1),
        key("app", 10, 20, SimilarFeatureCapability::Enabled, 2),
    ] {
        assert_ne!(base, different);
    }
    assert_eq!(base, PathBuf::from("app").join("10-20").join("v1-enabled"));
}

#[test]
fn template_rejects_prepared_data_dir() {
    // Catch inside the lock so expected panics cannot poison the fixture lock.
    let _lock = crate::data_dir::test_override_lock();
    let tmp = tempfile::tempdir().unwrap();
    crate::data_dir::set_test_override(Some(tmp.path().to_path_buf()));
    let _guard = phase_c_support::OverrideGuard;
    for filename in ["old.db", "settings.json", "directory"] {
        let path = tmp.path().join(filename);
        if filename == "directory" {
            std::fs::create_dir(&path).unwrap();
        } else {
            std::fs::write(&path, b"prepared").unwrap();
        }
        for settings in [None, Some(crate::settings::Settings::default())] {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    App::new_for_test(AppTestConfig {
                        ai_runtime: AiRuntimeFixture::NotStarted,
                        data_dir_fixture: AppDataDirFixture::Template,
                        data_dir: tmp.path().to_path_buf(),
                        settings,
                        similar_feature_capability: SimilarFeatureCapability::Enabled,
                    })
                }))
                .is_err(),
                "prepared {filename} must panic before constructor"
            );
        }
        if path.is_dir() {
            std::fs::remove_dir(&path).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
    }
}

#[test]
fn builder_validation_rejects_nonzero_exit() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(BUILDER_DONE), b"done").unwrap();
    std::fs::write(tmp.path().join("test.db"), b"db").unwrap();
    for code in [Some(1), None] {
        assert!(std::panic::catch_unwind(|| validate_builder_output(code, tmp.path())).is_err());
    }
}

#[test]
fn builder_validation_rejects_missing_marker() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("test.db"), b"db").unwrap();
    assert!(std::panic::catch_unwind(|| validate_builder_output(Some(0), tmp.path())).is_err());
}

#[test]
fn builder_validation_rejects_zero_databases() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(BUILDER_DONE), b"done").unwrap();
    std::fs::write(tmp.path().join("settings.db"), b"excluded").unwrap();
    std::fs::create_dir(tmp.path().join("nested")).unwrap();
    std::fs::write(tmp.path().join("nested/other.db"), b"not top-level").unwrap();
    assert!(std::panic::catch_unwind(|| validate_builder_output(Some(0), tmp.path())).is_err());
}

#[test]
fn builder_validation_rejects_nonempty_wal() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("test.db");
    std::fs::write(tmp.path().join("test.db-wal"), b"uncheckpointed").unwrap();
    assert!(std::panic::catch_unwind(|| assert_empty_wal(&db)).is_err());
    std::fs::write(tmp.path().join("test.db-wal"), b"").unwrap();
    assert_empty_wal(&db);
    std::fs::remove_file(tmp.path().join("test.db-wal")).unwrap();
    assert_empty_wal(&db);
}
