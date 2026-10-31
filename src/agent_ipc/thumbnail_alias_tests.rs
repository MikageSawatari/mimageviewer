//! Handler regressions for producer-returned 8.3 spellings and edit aliases.
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

fn value<T>(row: &PageRow<T>) -> &T {
    match row {
        PageRow::Value(value) => value,
        PageRow::TooLong(_) => panic!("fixture identity must fit"),
    }
}

fn thumbnail(fixture: &Fixture, input: ItemRef) -> ThumbnailResponse {
    let Payload::Thumbnails(response) = fixture
        .handlers
        .get_thumbnail(vec![input], None, None, &context(fixture))
        .unwrap()
    else {
        panic!("expected thumbnail response");
    };
    response
}

fn png() -> Vec<u8> {
    // Asymmetric pixels distinguish all four rotations, including 90/270.
    let pixels = image::RgbImage::from_fn(7, 5, |x, y| {
        image::Rgb([(x * 31) as u8, (y * 47) as u8, ((x + y) * 19) as u8])
    });
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(pixels)
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

struct Spellings {
    long: PathBuf,
    short: PathBuf,
    native: bool,
}

impl Spellings {
    fn new(root: &Path, extension: &str, bytes: &[u8]) -> Self {
        let long = root.join(format!("Long filename for alias regression.{extension}"));
        std::fs::write(&long, bytes).unwrap();
        if let Some(short) = input_path::native_name(&long, NameForm::Short).unwrap() {
            if input_path::key(&short) != input_path::key(&long) {
                assert_eq!(std::fs::read(&short).unwrap(), bytes);
                return Self {
                    long,
                    short,
                    native: true,
                };
            }
        }
        // Some test volumes disable 8.3 creation. These are separate real
        // fixture files, with their name relationship supplied by the lookup
        // override; this branch does not claim to exercise native 8.3 aliases.
        let short = root.join(format!("SHORT~1.{extension}"));
        std::fs::write(&short, bytes).unwrap();
        Self {
            long,
            short,
            native: false,
        }
    }

    fn run<T>(&self, action: impl FnOnce() -> T) -> T {
        if self.native {
            action()
        } else {
            let long = self.long.clone();
            let short = self.short.clone();
            input_path::with_name_lookup(
                move |path, form| {
                    assert!(
                        input_path::key(path) == input_path::key(&long)
                            || input_path::key(path) == input_path::key(&short)
                    );
                    Ok(Some(match form {
                        NameForm::Long => long.clone(),
                        NameForm::Short => short.clone(),
                    }))
                },
                action,
            )
        }
    }
}

#[test]
fn collection_short_path_roundtrips_into_item_info_and_thumbnail() {
    use crate::collection_store::{CollectionRegistration, CollectionResolvedKind};
    let fixture = Fixture::new();
    let pixels = png();
    let paths = Spellings::new(fixture._dir.path(), "png", &pixels);
    let actor = fixture.runtime.as_ref().unwrap().client();
    let collection = actor
        .create_collection("short spelling".into())
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    let added = actor
        .add_batch(
            collection.definition.id,
            collection.definition.revision,
            vec![
                CollectionRegistration::from_trusted_path(
                    &paths.short,
                    CollectionResolvedKind::Image,
                )
                .unwrap(),
            ],
        )
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(
        input_path::key(&added.snapshot.entries[0].source_path),
        input_path::key(&paths.short)
    );
    paths.run(|| {
        let page = fixture
            .handlers
            .get_collection(
                Some(collection.definition.id.to_string()),
                None,
                None,
                None,
                &context(&fixture),
            )
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let returned = value(&page.items[0]);
        assert_eq!(
            input_path::key(Path::new(&returned.path)),
            input_path::key(&paths.short)
        );
        let input = ItemRef {
            path: returned.path.clone(),
            subresource: returned.subresource.clone(),
        };
        let info = fixture
            .handlers
            .item_info(vec![input.clone()], None, None, &context(&fixture))
            .unwrap();
        assert_eq!(info.items.len(), 1);
        assert_eq!(value(&info.items[0]).item, input);
        assert_eq!(value(&info.items[0]).kind, ItemKind::Image);
        assert_eq!(
            value(&info.items[0]).dimensions,
            InfoField::Present(Dimensions {
                width: 7,
                height: 5,
                unit: DimensionUnit::Pixels,
            })
        );
        let response = thumbnail(&fixture, input.clone());
        assert_eq!(value(&response.page.items[0]).item, input);
        assert_eq!(value(&response.page.items[0]).status, ThumbnailStatus::Ok);
        assert_eq!(response.images.len(), 1);
    });
}

#[test]
fn conceal_follows_both_alias_directions_for_file_zip_and_pdf() {
    let fixture = Fixture::new();
    let pixels = png();
    let (archive, legacy) = cp932_zip(&pixels);
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    for (extension, bytes, member, key_member) in [
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
        // A fake PDF is intentional: conceal must reject before PDF decoding.
        (
            "pdf",
            b"not a PDF".as_slice(),
            Some("page_0"),
            Some("page_0"),
        ),
    ] {
        let paths = Spellings::new(fixture._dir.path(), extension, bytes);
        for (source, requested) in [(&paths.long, &paths.short), (&paths.short, &paths.long)] {
            let key = match (extension, key_member) {
                ("pdf", _) => crate::edit_source::page_key_for_grid_item(
                    &crate::grid_item::GridItem::PdfPage {
                        pdf_path: source.clone(),
                        page_num: 0,
                        content_type: None,
                    },
                )
                .unwrap(),
                ("zip", Some(member)) => crate::adjustment_db::zip_entry_key(source, member),
                _ => crate::adjustment_db::normalize_path(source),
            };
            conceal.set(&key, &[true], &[], 1, 1).unwrap();
            let before = std::fs::read(crate::conceal_db::ConcealDb::db_path()).unwrap();
            paths.run(|| {
                let input = item(requested, member);
                let response = thumbnail(&fixture, input.clone());
                assert!(
                    response.images.is_empty(),
                    "{extension}: {source:?} -> {requested:?}"
                );
                assert_eq!(value(&response.page.items[0]).item, input);
                assert_eq!(
                    value(&response.page.items[0]).status,
                    ThumbnailStatus::HiddenByEdit,
                    "{extension}: {source:?} -> {requested:?}"
                );
            });
            assert_eq!(
                std::fs::read(crate::conceal_db::ConcealDb::db_path()).unwrap(),
                before,
                "alias lookup must not migrate conceal keys"
            );
            conceal.delete(&key).unwrap();
        }
    }
}

#[test]
fn zip_rotation_prefers_given_long_short_and_current_before_legacy_per_spelling() {
    use crate::rotation_db::{Rotation, RotationDb};
    let fixture = Fixture::new();
    let pixels = png();
    let (archive, legacy) = cp932_zip(&pixels);
    let given = fixture._dir.path().join("Given collection spelling.zip");
    let long = fixture._dir.path().join("Long resolved filename.zip");
    let short = fixture._dir.path().join("SHORT~1.ZIP");
    for path in [&given, &long, &short] {
        std::fs::write(path, &archive).unwrap();
    }
    // Three independently addressable spellings let this test distinguish
    // given/long/short priority even on volumes with no native short names.
    let rows: Vec<_> = [&given, &long, &short]
        .into_iter()
        .flat_map(|path| {
            ["日本.png", legacy.as_str()]
                .map(|name| crate::adjustment_db::zip_entry_key(path, name))
        })
        .zip([
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ])
        .collect();
    let db = RotationDb::open().unwrap();
    for (key, rotation) in &rows {
        db.set_key(key, *rotation).unwrap();
    }
    let lookup_given = given.clone();
    let lookup_long = long.clone();
    let lookup_short = short.clone();
    input_path::with_name_lookup(
        move |path, form| {
            assert!(
                [&lookup_given, &lookup_long, &lookup_short]
                    .iter()
                    .any(|candidate| { input_path::key(path) == input_path::key(candidate) })
            );
            Ok(Some(match form {
                NameForm::Long => lookup_long.clone(),
                NameForm::Short => lookup_short.clone(),
            }))
        },
        || {
            let settings =
                crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
            for (key, rotation) in &rows {
                let before = std::fs::read(RotationDb::db_path()).unwrap();
                let response = thumbnail(&fixture, item(&given, Some("日本.png")));
                assert_eq!(value(&response.page.items[0]).status, ThumbnailStatus::Ok);
                assert_eq!(response.images.len(), 1);
                let expected = crate::capture::rotate_dynamic_image(
                    image::load_from_memory(&pixels).unwrap(),
                    *rotation,
                );
                let (webp, width, height) = crate::catalog::encode_thumb_webp(
                    &expected,
                    DEFAULT_THUMBNAIL_SIDE,
                    f32::from(settings.thumb_quality),
                )
                .unwrap();
                assert_eq!(response.images[0].webp, webp, "priority at {key}");
                assert_eq!(
                    value(&response.page.items[0]).dimensions,
                    Some(Dimensions {
                        width,
                        height,
                        unit: DimensionUnit::Pixels
                    })
                );
                assert_eq!(std::fs::read(RotationDb::db_path()).unwrap(), before);
                db.set_key(key, Rotation::None).unwrap();
            }
        },
    );
}
