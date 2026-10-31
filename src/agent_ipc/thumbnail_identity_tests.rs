//! Conceal identity must survive independently shortened path components.
use super::super::input_path::{self, NameForm};
use super::*;
use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

type Fixture = super::super::tests::Fixture;

fn context(fixture: &Fixture) -> ReadContext {
    ReadContext::new(
        &fixture.handlers,
        PermissionSnapshot {
            level: AgentLevel::ReadOnly,
            thumbnails: true,
            gps: false,
        },
        Arc::new(AtomicBool::new(false)),
    )
}

fn item(path: &Path, member: Option<&str>) -> ItemRef {
    ItemRef {
        path: path.to_string_lossy().into_owned(),
        subresource: member.map(str::to_owned),
    }
}

fn call(fixture: &Fixture, items: Vec<ItemRef>) -> Result<ThumbnailResponse, AgentError> {
    match fixture
        .handlers
        .get_thumbnail(items, None, None, &context(fixture))?
    {
        Payload::Thumbnails(response) => Ok(response),
        _ => panic!("expected thumbnails"),
    }
}

fn row(response: &ThumbnailResponse, index: usize) -> &ThumbnailItem {
    match &response.page.items[index] {
        PageRow::Value(value) => value,
        PageRow::TooLong(_) => panic!("fixture identity must fit"),
    }
}

fn key(path: &Path) -> String {
    crate::adjustment_db::normalize_path(path)
}

fn png() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(7, 5, image::Rgb([43, 97, 151])))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn cp932_zip(pixels: &[u8]) -> (Vec<u8>, String) {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "xxxx.png",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(pixels).unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    let encoded = encoding_rs::SHIFT_JIS.encode("日本.png").0;
    let offsets: Vec<_> = bytes
        .windows(8)
        .enumerate()
        .filter_map(|(index, name)| (name == b"xxxx.png").then_some(index))
        .collect();
    assert_eq!(offsets.len(), 2);
    for offset in offsets {
        bytes[offset..offset + 8].copy_from_slice(&encoded);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let legacy = archive.by_index_raw(0).unwrap().name().to_owned();
    assert_ne!(legacy, "日本.png");
    (bytes, legacy)
}

#[derive(Clone)]
struct Spellings {
    long: PathBuf,
    short: PathBuf,
    mixed: [PathBuf; 2],
}

impl Spellings {
    fn new(root: &Path, extension: &str, bytes: &[u8]) -> Self {
        let long = root
            .join("Long parent directory")
            .join("Long child directory")
            .join(format!("Secret photograph.{extension}"));
        let short = root
            .join("PARENT~1")
            .join("CHILD~1")
            .join(format!("SECRET~1.{extension}"));
        let mixed = [
            root.join("PARENT~1")
                .join("Long child directory")
                .join(format!("Secret photograph.{extension}")),
            root.join("Long parent directory")
                .join("CHILD~1")
                .join(format!("SECRET~1.{extension}")),
        ];
        // Real, distinct files let the handler read either request spelling.
        // Only their name relationship is mocked; this is not native 8.3 QA.
        for path in [&long, &short, &mixed[0], &mixed[1]] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        Self { long, short, mixed }
    }

    fn lookup(&self, path: &Path, form: NameForm) -> std::io::Result<Option<PathBuf>> {
        assert!(
            [&self.long, &self.short, &self.mixed[0], &self.mixed[1]]
                .into_iter()
                .any(|candidate| key(candidate) == key(path)),
            "unexpected name lookup: {path:?}"
        );
        Ok(Some(match form {
            NameForm::Long => self.long.clone(),
            NameForm::Short => self.short.clone(),
        }))
    }
}

fn edit_key(path: &Path, member: Option<&str>) -> String {
    match member {
        Some(member) => crate::adjustment_db::zip_entry_key(path, member),
        None => key(path),
    }
}

#[test]
fn conceal_mixed_component_aliases_hide_file_zip_current_legacy_and_pdf_pages() {
    let fixture = Fixture::new();
    let pixels = png();
    let (archive, legacy) = cp932_zip(&pixels);
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    for (extension, bytes, member, stored_member) in [
        ("png", pixels.as_slice(), None, None),
        (
            "zip",
            archive.as_slice(),
            Some("日本.png"),
            Some("日本.png"),
        ),
        (
            "zip",
            archive.as_slice(),
            Some("日本.png"),
            Some(legacy.as_str()),
        ),
        // Hidden pages must be rejected before the PDF decoder is invoked.
        (
            "pdf",
            b"not a PDF".as_slice(),
            Some("page_0"),
            Some("page_0"),
        ),
    ] {
        let paths = Spellings::new(fixture._dir.path(), extension, bytes);
        for stored in &paths.mixed {
            let stored_key = edit_key(stored, stored_member);
            conceal.set(&stored_key, &[true], &[], 1, 1).unwrap();
            let before = std::fs::read(crate::conceal_db::ConcealDb::db_path()).unwrap();
            for requested in [&paths.long, &paths.short] {
                let lookup_paths = paths.clone();
                input_path::with_name_lookup(
                    move |path, form| lookup_paths.lookup(path, form),
                    || {
                        let input = item(requested, member);
                        let response = call(&fixture, vec![input.clone()]).unwrap();
                        assert_eq!(row(&response, 0).item, input);
                        assert_eq!(
                            row(&response, 0).status,
                            ThumbnailStatus::HiddenByEdit,
                            "{stored_key} -> {requested:?}"
                        );
                        assert!(response.images.is_empty());
                    },
                );
                assert_eq!(
                    std::fs::read(crate::conceal_db::ConcealDb::db_path()).unwrap(),
                    before,
                    "identity lookup must leave saved conceal keys untouched"
                );
            }
            conceal.delete(&stored_key).unwrap();
        }
    }
}

#[test]
fn conceal_valid_sqlite_with_missing_table_rejects_thumbnail_request() {
    let fixture = Fixture::new();
    let path = fixture._dir.path().join("readable.png");
    std::fs::write(&path, png()).unwrap();
    let db_path = crate::conceal_db::ConcealDb::db_path();
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    connection
        .execute_batch("CREATE TABLE unrelated_fixture(value INTEGER); INSERT INTO unrelated_fixture VALUES (1);")
        .unwrap();
    drop(connection);
    // Opening succeeds: this exercises checked SQL reads, not corrupt-file
    // handling at open_readonly.
    drop(crate::conceal_db::ConcealDb::open_readonly(&db_path).unwrap());
    let before = std::fs::read(&db_path).unwrap();
    input_path::with_name_lookup(
        |path, _| Ok(Some(path.to_owned())),
        || {
            assert_eq!(
                call(&fixture, vec![item(&path, None)]).unwrap_err().code,
                ErrorCode::Internal
            );
        },
    );
    assert_eq!(std::fs::read(&db_path).unwrap(), before);
}

#[test]
fn conceal_deleted_mixed_key_is_skipped_and_missing_identity_is_cached_per_call() {
    let fixture = Fixture::new();
    let paths = Spellings::new(fixture._dir.path(), "png", &png());
    let deleted = fixture._dir.path().join("DELETED~1").join("old.png");
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    conceal.set(&key(&deleted), &[true], &[], 1, 1).unwrap();
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    let requested = paths.long.clone();
    input_path::with_name_lookup(
        move |path, form| {
            if key(path) == key(&deleted) {
                assert_eq!(form, NameForm::Long);
                observed.set(observed.get() + 1);
                return Ok(None);
            }
            paths.lookup(path, form)
        },
        || {
            for expected_count in [1, 2] {
                let response = call(
                    &fixture,
                    vec![item(&requested, None), item(&requested, None)],
                )
                .unwrap();
                assert_eq!(response.page.items.len(), 2);
                assert_eq!(response.images.len(), 2);
                for index in 0..2 {
                    assert_eq!(row(&response, index).status, ThumbnailStatus::Ok);
                }
                assert_eq!(count.get(), expected_count);
            }
        },
    );
}

#[test]
fn conceal_stored_mixed_identity_error_fails_closed_with_readable_pixels() {
    let fixture = Fixture::new();
    let pixels = png();
    let paths = Spellings::new(fixture._dir.path(), "png", &pixels);
    let stored = paths.mixed[0].clone();
    let requested = paths.long.clone();
    assert_eq!(std::fs::read(&requested).unwrap(), pixels);
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    conceal.set(&key(&stored), &[true], &[], 1, 1).unwrap();
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    input_path::with_name_lookup(
        move |path, form| {
            if key(path) == key(&stored) {
                assert_eq!(form, NameForm::Long);
                observed.set(observed.get() + 1);
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            paths.lookup(path, form)
        },
        || {
            assert_eq!(
                call(&fixture, vec![item(&requested, None)])
                    .unwrap_err()
                    .code,
                ErrorCode::Internal
            );
            assert_eq!(count.get(), 1);
        },
    );
}

#[test]
fn conceal_stored_mixed_identity_is_resolved_once_per_call_for_repeated_items() {
    let fixture = Fixture::new();
    let paths = Spellings::new(fixture._dir.path(), "png", &png());
    let stored = paths.mixed[0].clone();
    let requested = paths.short.clone();
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    conceal.set(&key(&stored), &[true], &[], 1, 1).unwrap();
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    input_path::with_name_lookup(
        move |path, form| {
            if key(path) == key(&stored) {
                assert_eq!(form, NameForm::Long);
                observed.set(observed.get() + 1);
            }
            paths.lookup(path, form)
        },
        || {
            for expected_count in [1, 2] {
                let response = call(
                    &fixture,
                    vec![item(&requested, None), item(&requested, None)],
                )
                .unwrap();
                assert_eq!(response.page.items.len(), 2);
                assert!(response.images.is_empty());
                for index in 0..2 {
                    assert_eq!(row(&response, index).status, ThumbnailStatus::HiddenByEdit);
                }
                assert_eq!(count.get(), expected_count);
            }
        },
    );
}

#[test]
fn conceal_unrelated_drive_server_and_zip_member_keys_are_not_resolved() {
    let fixture = Fixture::new();
    let (archive, _) = cp932_zip(&png());
    let paths = Spellings::new(fixture._dir.path(), "zip", &archive);
    let requested = paths.long.clone();
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    let other_drive = if key(&requested).starts_with("z:") {
        "Y:/UNRELA~1/book.zip"
    } else {
        "Z:/UNRELA~1/book.zip"
    };
    for stored_key in [
        edit_key(Path::new(other_drive), Some("日本.png")),
        edit_key(
            Path::new("//unrelated-host/share/UNRELA~1/book.zip"),
            Some("日本.png"),
        ),
        edit_key(&paths.mixed[0], Some("unrequested.png")),
    ] {
        conceal.set(&stored_key, &[true], &[], 1, 1).unwrap();
    }
    input_path::with_name_lookup(
        move |path, form| {
            assert!(
                key(path) == key(&paths.long) || key(path) == key(&paths.short),
                "irrelevant conceal path must be filtered before name lookup: {path:?}"
            );
            paths.lookup(path, form)
        },
        || {
            let response = call(&fixture, vec![item(&requested, Some("日本.png"))]).unwrap();
            assert_eq!(row(&response, 0).status, ThumbnailStatus::Ok);
            assert_eq!(response.images.len(), 1);
        },
    );
}

#[test]
fn conceal_unc_host_domain_is_case_insensitive_and_other_servers_are_not_resolved() {
    let fixture = Fixture::new();
    let requested = PathBuf::from(r"\\PrimaryHost\Share\Long directory\Secret photograph.png");
    let stored = PathBuf::from(r"\\PRIMARYHOST\Share\LONGDI~1\Secret photograph.png");
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    for path in [
        stored.as_path(),
        Path::new(r"\\OtherHost\Share\LONGDI~1\Secret photograph.png"),
        Path::new("Z:/LONGDI~1/Secret photograph.png"),
    ] {
        conceal.set(&key(path), &[true], &[], 1, 1).unwrap();
    }
    let long = requested.clone();
    input_path::with_name_lookup(
        move |path, _| {
            assert!(
                key(path) == key(&long) || key(path) == key(&stored),
                "other server/drive must be filtered before name lookup: {path:?}"
            );
            // Different case and separators still denote the same edit key.
            Ok(Some(PathBuf::from(
                long.to_string_lossy().to_uppercase().replace('\\', "/"),
            )))
        },
        || {
            let response = call(&fixture, vec![item(&requested, None)]).unwrap();
            assert_eq!(row(&response, 0).status, ThumbnailStatus::HiddenByEdit);
            assert!(response.images.is_empty());
        },
    );
}

#[test]
fn conceal_resolves_original_request_before_normalizing_its_identity() {
    let fixture = Fixture::new();
    let requested = fixture._dir.path().join("İmage photograph.PNG");
    std::fs::write(&requested, png()).unwrap();
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    conceal.set(&key(&requested), &[true], &[], 1, 1).unwrap();
    let lookup_path = requested.clone();
    input_path::with_name_lookup(
        move |path, _| {
            // The lowercase edit key expands capital dotted I into two code
            // points. It is an identity, not a path to pass to a filesystem API.
            assert_eq!(path, lookup_path);
            Ok(Some(lookup_path.clone()))
        },
        || {
            let response = call(&fixture, vec![item(&requested, None)]).unwrap();
            assert_eq!(row(&response, 0).status, ThumbnailStatus::HiddenByEdit);
            assert!(response.images.is_empty());
        },
    );
}

#[test]
fn conceal_resolves_original_stored_mixed_spelling_before_normalization() {
    let fixture = Fixture::new();
    let requested = fixture._dir.path().join("Secret photograph.PNG");
    std::fs::write(&requested, png()).unwrap();
    let stored = fixture
        ._dir
        .path()
        .join("MIXED~1")
        .join("İmage photograph.PNG");
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    // Keep the stored spelling verbatim to exercise resolution before folding.
    conceal
        .set(&stored.to_string_lossy(), &[true], &[], 1, 1)
        .unwrap();
    let lookup_requested = requested.clone();
    input_path::with_name_lookup(
        move |path, form| {
            if path == stored {
                assert_eq!(form, NameForm::Long);
            } else {
                assert_eq!(path, lookup_requested);
            }
            Ok(Some(lookup_requested.clone()))
        },
        || {
            let response = call(&fixture, vec![item(&requested, None)]).unwrap();
            assert_eq!(row(&response, 0).status, ThumbnailStatus::HiddenByEdit);
            assert!(response.images.is_empty());
        },
    );
}
