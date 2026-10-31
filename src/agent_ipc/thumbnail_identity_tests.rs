//! Conceal identity must survive independently shortened path components.
use super::super::input_path::{self, NameForm};
use super::*;
use std::io::Write;
use std::path::PathBuf;

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
    let encoded = encoding_rs::SHIFT_JIS.encode("\u{65e5}\u{672c}.png").0;
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
    assert_ne!(legacy, "\u{65e5}\u{672c}.png");
    (bytes, legacy)
}

#[derive(Clone)]
struct Spellings {
    long: PathBuf,
    short: PathBuf,
    mixed: Vec<PathBuf>,
}

impl Spellings {
    fn new(root: &Path, extension: &str, bytes: &[u8]) -> Self {
        let names = [
            ["Long parent directory".to_owned(), "PARENT~1".to_owned()],
            ["Long child directory".to_owned(), "CHILD~1".to_owned()],
            [
                format!("Secret photograph.{extension}"),
                format!("SECRET~1.{extension}"),
            ],
        ];
        let spelling = |bits: usize| {
            names
                .iter()
                .enumerate()
                .fold(root.to_owned(), |path, (index, forms)| {
                    path.join(&forms[(bits >> index) & 1])
                })
        };
        let long = spelling(0);
        let short = spelling(7);
        let mixed = (1..7).map(spelling).collect();
        // Only request spellings need files. Stored mixed keys are normalized
        // identities, and must never be sent to a filesystem name lookup.
        for path in [&long, &short] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        Self { long, short, mixed }
    }

    fn name(&self, form: NameForm) -> PathBuf {
        match form {
            NameForm::Long => self.long.clone(),
            NameForm::Short => self.short.clone(),
        }
    }
}

fn edit_key(path: &Path, member: Option<&str>) -> String {
    match member {
        Some("page_0") => crate::edit_source::page_key_for_pdf(path, 0),
        Some(member) => crate::adjustment_db::zip_entry_key(path, member),
        None => key(path),
    }
}

#[test]
fn conceal_all_mixed_component_keys_hide_long_and_short_file_zip_legacy_and_pdf_requests() {
    let fixture = Fixture::new();
    let pixels = png();
    let (archive, legacy) = cp932_zip(&pixels);
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    for (extension, bytes, member, stored_member) in [
        ("png", pixels.as_slice(), None, None),
        (
            "zip",
            archive.as_slice(),
            Some("\u{65e5}\u{672c}.png"),
            Some("\u{65e5}\u{672c}.png"),
        ),
        (
            "zip",
            archive.as_slice(),
            Some("\u{65e5}\u{672c}.png"),
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
        assert_eq!(paths.mixed.len(), 6);
        for stored in &paths.mixed {
            let stored_key = edit_key(stored, stored_member);
            conceal.set(&stored_key, &[true], &[], 1, 1).unwrap();
            let before = std::fs::read(crate::conceal_db::ConcealDb::db_path()).unwrap();
            for requested in [&paths.long, &paths.short] {
                let original = requested.clone();
                let lookup_paths = paths.clone();
                input_path::with_name_lookup(
                    move |path, form| {
                        assert_eq!(path, original, "only the original request may be queried");
                        Ok(Some(lookup_paths.name(form)))
                    },
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
        |path, _| panic!("DB read must fail before filesystem lookup: {path:?}"),
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
fn conceal_request_name_errors_fail_closed_with_readable_pixels() {
    let fixture = Fixture::new();
    let paths = Spellings::new(fixture._dir.path(), "png", &png());
    let requested = paths.long.clone();
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    conceal
        .set(&key(&paths.mixed[0]), &[true], &[], 1, 1)
        .unwrap();
    for failing_form in [NameForm::Long, NameForm::Short] {
        let original = requested.clone();
        let lookup_paths = paths.clone();
        input_path::with_name_lookup(
            move |path, form| {
                assert_eq!(path, original);
                if form == failing_form {
                    Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                } else {
                    Ok(Some(lookup_paths.name(form)))
                }
            },
            || {
                assert_eq!(
                    call(&fixture, vec![item(&requested, None)])
                        .unwrap_err()
                        .code,
                    ErrorCode::Internal
                );
            },
        );
    }
}

#[test]
fn conceal_unicode_collision_missing_and_existing_requests_are_independent_in_both_orders() {
    let fixture = Fixture::new();
    let missing = fixture._dir.path().join("\u{0130}mage photograph.PNG");
    let existing = fixture._dir.path().join("i\u{0307}mage photograph.PNG");
    assert_ne!(missing, existing);
    assert_eq!(key(&missing), key(&existing));
    assert!(!missing.exists());
    std::fs::write(&existing, png()).unwrap();
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    conceal.set(&key(&existing), &[true], &[], 1, 1).unwrap();
    for missing_first in [true, false] {
        let lookup_missing = missing.clone();
        let lookup_existing = existing.clone();
        input_path::with_name_lookup(
            move |path, _| {
                // Comparison must be on the exact original Path: normalization
                // intentionally maps these distinct filenames to the same key.
                if path == lookup_missing {
                    Ok(None)
                } else {
                    assert_eq!(
                        path, lookup_existing,
                        "stored keys are never filesystem paths"
                    );
                    Ok(Some(lookup_existing.clone()))
                }
            },
            || {
                let inputs = if missing_first {
                    vec![item(&missing, None), item(&existing, None)]
                } else {
                    vec![item(&existing, None), item(&missing, None)]
                };
                let response = call(&fixture, inputs.clone()).unwrap();
                assert_eq!(response.page.items.len(), 2);
                assert!(response.images.is_empty());
                for (index, input) in inputs.iter().enumerate() {
                    assert_eq!(&row(&response, index).item, input);
                    assert_eq!(
                        row(&response, index).status,
                        if input.path == missing.to_string_lossy() {
                            ThumbnailStatus::Unavailable
                        } else {
                            ThumbnailStatus::HiddenByEdit
                        }
                    );
                }
            },
        );
    }
}

#[test]
fn conceal_unicode_collision_hides_both_existing_files() {
    let fixture = Fixture::new();
    let paths = [
        fixture._dir.path().join("\u{0130}mage photograph.PNG"),
        fixture._dir.path().join("i\u{0307}mage photograph.PNG"),
    ];
    assert_ne!(paths[0], paths[1]);
    assert_eq!(key(&paths[0]), key(&paths[1]));
    for path in &paths {
        std::fs::write(path, png()).unwrap();
    }
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    conceal.set(&key(&paths[0]), &[true], &[], 1, 1).unwrap();
    let originals = paths.clone();
    input_path::with_name_lookup(
        move |path, _| {
            assert!(originals.iter().any(|original| original == path));
            Ok(Some(path.to_owned()))
        },
        || {
            let response = call(
                &fixture,
                paths.iter().map(|path| item(path, None)).collect(),
            )
            .unwrap();
            assert_eq!(response.page.items.len(), 2);
            for index in 0..2 {
                assert_eq!(row(&response, index).status, ThumbnailStatus::HiddenByEdit);
            }
            assert!(response.images.is_empty());
        },
    );
}

#[test]
fn conceal_normalized_mixed_unicode_parent_key_is_never_queried_as_a_path() {
    let fixture = Fixture::new();
    let root = fixture._dir.path();
    let requested = root
        .join("\u{0130}mage parent directory")
        .join("\u{0130}mage photograph.PNG");
    let short = root.join("PARENT~1").join("SECRET~1.PNG");
    let stored_paths = [
        root.join("\u{0130}mage parent directory")
            .join("SECRET~1.PNG"),
        root.join("PARENT~1").join("\u{0130}mage photograph.PNG"),
    ];
    std::fs::create_dir_all(requested.parent().unwrap()).unwrap();
    std::fs::write(&requested, png()).unwrap();
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    for stored in &stored_paths {
        let stored_key = key(stored);
        assert!(stored_key.contains("i\u{0307}mage"));
        assert!(stored_key.contains('~'));
        assert_ne!(Path::new(&stored_key), stored);
        conceal.set(&stored_key, &[true], &[], 1, 1).unwrap();
        let original = requested.clone();
        let lookup_short = short.clone();
        input_path::with_name_lookup(
            move |path, form| {
                assert_eq!(
                    path, original,
                    "a lossy stored key must never reach name lookup"
                );
                Ok(Some(match form {
                    NameForm::Long => original.clone(),
                    NameForm::Short => lookup_short.clone(),
                }))
            },
            || {
                let response = call(&fixture, vec![item(&requested, None)]).unwrap();
                assert_eq!(row(&response, 0).status, ThumbnailStatus::HiddenByEdit);
                assert!(response.images.is_empty());
            },
        );
        conceal.delete(&stored_key).unwrap();
    }
}

#[test]
fn conceal_nonmatching_component_counts_do_not_hide_readable_file() {
    let fixture = Fixture::new();
    let paths = Spellings::new(fixture._dir.path(), "png", &png());
    let requested = paths.long.clone();
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    for stored in [
        fixture._dir.path().join("PARENT~1").join("SECRET~1.png"),
        paths
            .short
            .parent()
            .unwrap()
            .join("extra")
            .join("SECRET~1.png"),
    ] {
        conceal.set(&key(&stored), &[true], &[], 1, 1).unwrap();
    }
    let original = requested.clone();
    input_path::with_name_lookup(
        move |path, form| {
            assert_eq!(path, original);
            Ok(Some(paths.name(form)))
        },
        || {
            let response = call(&fixture, vec![item(&requested, None)]).unwrap();
            assert_eq!(row(&response, 0).status, ThumbnailStatus::Ok);
            assert_eq!(response.images.len(), 1);
        },
    );
}

#[test]
fn conceal_unrelated_drive_server_and_zip_member_keys_do_not_hide_requested_entry() {
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
        edit_key(Path::new(other_drive), Some("\u{65e5}\u{672c}.png")),
        edit_key(
            Path::new("//unrelated-host/share/UNRELA~1/book.zip"),
            Some("\u{65e5}\u{672c}.png"),
        ),
        edit_key(&paths.mixed[0], Some("unrequested.png")),
    ] {
        conceal.set(&stored_key, &[true], &[], 1, 1).unwrap();
    }
    let original = requested.clone();
    input_path::with_name_lookup(
        move |path, form| {
            assert_eq!(path, original);
            Ok(Some(paths.name(form)))
        },
        || {
            let response = call(
                &fixture,
                vec![item(&requested, Some("\u{65e5}\u{672c}.png"))],
            )
            .unwrap();
            assert_eq!(row(&response, 0).status, ThumbnailStatus::Ok);
            assert_eq!(response.images.len(), 1);
        },
    );
}

#[test]
fn conceal_unc_component_names_preserve_root_and_match_case_insensitively() {
    let fixture = Fixture::new();
    let requested = PathBuf::from(r"\\PrimaryHost\Share\Long directory\Secret photograph.png");
    let short = PathBuf::from(r"\\PrimaryHost\Share\LONGDI~1\SECRET~1.png");
    let stored = PathBuf::from(r"\\PRIMARYHOST\Share\LONGDI~1\Secret photograph.png");
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    for path in [
        stored.as_path(),
        Path::new(r"\\OtherHost\Share\LONGDI~1\Secret photograph.png"),
        Path::new("Z:/LONGDI~1/Secret photograph.png"),
    ] {
        conceal.set(&key(path), &[true], &[], 1, 1).unwrap();
    }
    let original = requested.clone();
    input_path::with_name_lookup(
        move |path, form| {
            assert_eq!(path, original);
            Ok(Some(match form {
                // Name results may use different case and separators. Both
                // full results must be normalized before their components split.
                NameForm::Long => {
                    PathBuf::from(original.to_string_lossy().to_uppercase().replace('\\', "/"))
                }
                NameForm::Short => short.clone(),
            }))
        },
        || {
            let components = input_path::component_names(&requested).unwrap().unwrap();
            assert_eq!(components[0], ["", ""]);
            assert_eq!(components[1], ["", ""]);
            assert_eq!(components[2], ["primaryhost", "primaryhost"]);
            let response = call(&fixture, vec![item(&requested, None)]).unwrap();
            assert_eq!(row(&response, 0).status, ThumbnailStatus::HiddenByEdit);
            assert!(response.images.is_empty());
        },
    );
}
