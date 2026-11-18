use super::*;
use crate::cover_auto_crop::AutoCropIdentity;
use crate::settings::ReadingDirection::{Ltr, Rtl};
use std::path::PathBuf;

#[test]
fn rar_auto_key_golden_uses_logical_tiles_and_physical_zipdirs() {
    let auto = Some(AutoCropIdentity::new(Rtl));
    for (base, physical, expected) in [
        ("archivethumb:rar:book.rar", "book.rar", "v4r"),
        ("archivethumb:rar:book.cbr", "cache.zip", "v4r"),
        ("archivethumb:rar:book.rar", "pending", "v4r"),
        ("archivethumb:7z:book.7z", "cache.zip", "v4"),
        ("archivethumb:lzh:book.lzh", "cache.zip", "v4"),
        ("zipthumb:book.zip", "book.zip", "v4"),
        ("pdfthumb:book.pdf", "book.pdf", "v4"),
        ("zipdir:chapter/", "BOOK.RAR", "v4r"),
        ("zipdir:chapter/", "book.CBR", "v4r"),
        ("zipdir:chapter/", "cache.zip", "v4"),
    ] {
        for tail in ["", "#pin:cover|generation:1:2"] {
            let source = format!("{base}{tail}");
            assert_eq!(
                representative_cache_key_for_policy_with_auto_in_archive(
                    &source,
                    &Default::default(),
                    auto,
                    Some(Path::new(physical)),
                ),
                format!("{base}|autocrop:{expected}:rtl{tail}"),
            );
        }
    }
}

#[test]
fn rar_auto_key_normalization_binding_and_off_round_trip() {
    for physical in [Path::new("book.rar"), Path::new("cache.zip")] {
        for base in ["archivethumb:rar:book.rar", "zipdir:chapter/"] {
            for tail in ["", "#pin:cover|generation:1:2"] {
                for policy in [
                    crate::file_type_visibility::FileTypeVisibility::default(),
                    crate::file_type_visibility::FileTypeVisibility {
                        excluded_extensions: ["gif".into()].into_iter().collect(),
                        ..Default::default()
                    },
                ] {
                    let plain =
                        representative_cache_key_for_policy(&format!("{base}{tail}"), &policy);
                    let key = |source: &str, auto| {
                        representative_cache_key_for_policy_with_auto_in_archive(
                            source,
                            &policy,
                            auto,
                            Some(physical),
                        )
                    };
                    let rtl = key(&plain, Some(AutoCropIdentity::new(Rtl)));
                    let ltr = key(&rtl, Some(AutoCropIdentity::new(Ltr)));
                    assert_eq!(key(&rtl, Some(AutoCropIdentity::new(Rtl))), rtl);
                    assert_eq!(key(&ltr, Some(AutoCropIdentity::new(Rtl))), rtl);
                    assert_eq!(key(&ltr, None), plain);
                    assert_eq!(key(&plain, Some(AutoCropIdentity::new(Ltr))), ltr);
                    assert_eq!(rtl.matches("|autocrop:").count(), 1);
                    assert_eq!(legacy_auto_child_keys(&rtl).is_none(), rtl.contains("v4r:"));
                }
            }
        }
    }
    for literal in [
        "zipthumb:book|autocrop:v4r:rtl",
        "pdfthumb:book|autocrop:v4r:rtl",
        "archivethumb:7z:book|autocrop:v4r:rtl",
        "folderthumb:book|autocrop:v4r:rtl",
        "zipdir:chapter|autocrop:v4r:rtl/",
    ] {
        assert!(
            representative_auto_crop_key_parts(literal).is_none(),
            "{literal}"
        );
        assert_eq!(
            representative_cache_key_for_policy(literal, &Default::default()),
            literal
        );
    }
}

#[test]
fn rar_auto_request_renormalizes_all_row_keys_with_physical_path() {
    for physical in ["book.rar", "cache.zip"] {
        let parent = ParentRow {
            source: physical.into(),
            folder: "books".into(),
            row_key: "zipdir:chapter/|autocrop:v4:rtl#pin:cover".into(),
        };
        let mut req = LoadRequest {
            path: PathBuf::from(physical),
            cache_key_override: Some("zipdir:chapter/|autocrop:v4:rtl#pin:cover".into()),
            parent_row: Some(parent.clone()),
            catalog_target: RequestCatalog::Parent {
                row: parent,
                work: crate::catalog::CatalogWork::capture(Path::new("unused-rar-key-cache")),
            },
            ..Default::default()
        };
        req.set_auto_cover_crop(Some(AutoCropIdentity::new(Rtl)));
        let key = cache_key_for_request(&req).unwrap().into_owned();
        assert_eq!(req.parent_row.as_ref().unwrap().row_key, key);
        let RequestCatalog::Parent { row, .. } = &req.catalog_target else {
            panic!("parent target")
        };
        assert_eq!(row.row_key, key);
        assert_eq!(
            key,
            format!(
                "zipdir:chapter/|autocrop:{}:rtl#pin:cover",
                if physical.ends_with("rar") {
                    "v4r"
                } else {
                    "v4"
                }
            )
        );
        req.set_auto_cover_crop(Some(AutoCropIdentity::new(Rtl)));
        assert_eq!(cache_key_for_request(&req).unwrap(), key);
        req.set_auto_cover_crop(None);
        assert_eq!(
            cache_key_for_request(&req).unwrap(),
            "zipdir:chapter/#pin:cover"
        );
    }
}
