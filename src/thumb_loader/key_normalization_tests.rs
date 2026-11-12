use super::*;
use crate::cover_auto_crop::AutoCropIdentity;
use crate::file_type_visibility::FileTypeVisibility;
use crate::settings::{ReadingDirection, SortOrder};

// Copied verbatim from c4e92e5df:src/thumb_loader.rs, apart from its name.
fn master_representative_cache_key_for_policy(
    key: &str,
    policy: &crate::file_type_visibility::FileTypeVisibility,
) -> String {
    if !is_representative_cache_key(key) {
        return key.to_owned();
    }
    let split = key.find(CACHE_KEY_PIN_SUFFIX).unwrap_or(key.len());
    let (base, pin) = key.split_at(split);
    let base = base
        .rsplit_once("|types:")
        .filter(|(_, fingerprint)| {
            fingerprint.len() == 16 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map_or(base, |(base, _)| base);
    if policy.is_all_visible() {
        format!("{base}{pin}")
    } else {
        let suffix = format!("|types:{:016x}", policy.fingerprint());
        format!("{base}{suffix}{pin}")
    }
}

fn policies() -> [FileTypeVisibility; 2] {
    [
        FileTypeVisibility::default(),
        FileTypeVisibility {
            excluded_extensions: ["gif".into()].into_iter().collect(),
            ..Default::default()
        },
    ]
}

#[test]
fn representative_cache_key_matches_master_on_literal_names_and_generation_layouts() {
    let mut corpus = Vec::new();
    for name in [
        "book/",
        "book|ordinary/",
        "book|types:0000000000000001/",
        "book|types:0000000000000001|types:0000000000000002/",
        "book|types:0000000000000001",
        "book|types:0000000000000001|types:0000000000000002",
        "book|types:GGGGGGGGGGGGGGGG",
        "book|autocrop:v1:rtl/",
        "book|autocrop:v1:ltr/",
        "book|autocrop:v1:rtl|generation:one/",
        "book|autocrop:v1:ltr|generation:two/",
        "book|generation:one/",
        "book|generation:10:20/",
        "book|generation:10:20|types:0000000000000001",
        "book|autocrop:v1:rtl|types:0000000000000001/",
        "book|autocrop:v2:rtl",
        "book|autocrop:v1:rtl|types:0000000000000001",
    ] {
        for prefix in ["zipdir:", "zipthumb:", "pdfthumb:", "page:"] {
            let base = format!("{prefix}{name}");
            corpus.push(base.clone());
            corpus.push(format!("{base}#pin:leaf|autocrop:v1:rtl|generation:one/"));
        }
    }
    corpus.extend([
        "pdfthumb:a.epub|generation:17:4096".into(),
        "pdfthumb:a.epub|generation:17:4096|types:0000000000000001".into(),
        "pdfthumb:a.epub#pin:pdf||page|1|17|4096|generation:17:4096".into(),
        "pdfthumb:a.epub|types:0000000000000001#pin:pdf||page|1|17|4096|generation:17:4096".into(),
        "zipdir:book/#pin:leaf|autocrop:v1:rtl".into(),
        "zipdir:book/#pin:leaf|types:0000000000000001".into(),
    ]);
    for policy in policies() {
        for key in &corpus {
            let expected = master_representative_cache_key_for_policy(key, &policy);
            assert_eq!(
                representative_cache_key_for_policy(key, &policy),
                expected,
                "{key}"
            );
        }
        assert_ne!(
            representative_cache_key_for_policy(
                "zipdir:book|autocrop:v1:rtl|generation:one/",
                &policy
            ),
            representative_cache_key_for_policy("zipdir:book|generation:one/", &policy),
        );
    }
}

#[test]
fn representative_cache_key_real_builders_are_idempotent_and_auto_round_trips() {
    let directory = tempfile::tempdir().unwrap();
    let zip = directory.path().join("book.zip");
    std::fs::write(&zip, b"metadata only").unwrap();
    let source = crate::folder_thumb_pins::FolderPinSource::ZipEntry {
        zip_rel: String::new(),
        entry: "book/cover|generation:one|autocrop:v1:rtl.png".into(),
    };
    let target = crate::folder_thumb_pins::resolve_pin_target(&zip, &source).unwrap();
    let pinned = target.pinned_cache_key(
        "zipthumb:a.zip",
        crate::folder_thumb_pins::PinKeyMode::Exact,
        None,
    );
    let zipdir = target.pinned_cache_key(
        "zipdir:book/",
        crate::folder_thumb_pins::PinKeyMode::Exact,
        Some("book/"),
    );
    let epub_path = directory.path().join("a.epub");
    std::fs::write(&epub_path, b"metadata only").unwrap();
    let pdf_source = crate::folder_thumb_pins::FolderPinSource::PdfPage {
        pdf_rel: String::new(),
        page: 1,
    };
    let pdf = crate::folder_thumb_pins::resolve_pin_target(&epub_path, &pdf_source).unwrap();
    let mut request = LoadRequest {
        path: epub_path.clone(),
        pdf_page: Some(1),
        ..Default::default()
    };
    pdf.assign_to_load_request(
        &mut request,
        "pdfthumb:a.epub",
        crate::folder_thumb_pins::PinKeyMode::Exact,
        None,
    );
    let read = crate::pdf_loader::generation_target_for_test(&epub_path, 17, 4096);
    let stamped = stamp_resolved_pdf_request(&request, &read).unwrap();
    let epub = cache_key_for_request(&stamped).unwrap().into_owned();
    let folder = folder_thumb_auto_cache_key("book", SortOrder::Numeric, 3);
    for key in [
        pinned,
        zipdir,
        epub,
        folder,
        "pdfthumb:a.epub|generation:17:4096".into(),
    ] {
        for policy in policies() {
            let released = master_representative_cache_key_for_policy(&key, &policy);
            for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
                let auto = AutoCropIdentity::new(binding);
                let on = representative_cache_key_for_policy_with_auto(&key, &policy, Some(auto));
                assert_eq!(
                    representative_cache_key_for_policy_with_auto(&on, &policy, Some(auto)),
                    on
                );
                assert_eq!(representative_cache_key_for_policy(&on, &policy), released);
                for other in policies() {
                    assert_eq!(
                        representative_cache_key_for_policy_with_auto(&on, &other, Some(auto)),
                        representative_cache_key_for_policy_with_auto(&key, &other, Some(auto)),
                    );
                }
            }
        }
    }
}
