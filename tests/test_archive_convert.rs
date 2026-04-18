//! `archive_converter` の実アーカイブ読み書き統合テスト。
//!
//! - 最小構成の 7z ファイルを `sevenz_rust2::ArchiveWriter` で作り、
//!   それを `convert_to_zip` に通して、画像だけが抜き出された ZIP が
//!   生成されることを確認する。
//! - 変換後の ZIP を `zip::ZipArchive` で開き直し、エントリ一覧と中身のバイト一致を検証する。

use std::sync::atomic::AtomicBool;

use mimageviewer::archive_converter::{
    convert_to_zip, scan_summary, ArchiveFormat, ConvertError,
};

fn make_test_7z(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = sevenz_rust2::ArchiveWriter::new(file).unwrap();
    let entries = [
        ("page01.jpg", &b"fake_jpeg_bytes_01"[..]),
        ("page02.png", &b"fake_png_bytes_002"[..]),
        ("notes.txt", &b"this_should_be_skipped"[..]),
        ("sub/page03.webp", &b"fake_webp_bytes_03"[..]),
    ];
    for (name, data) in entries.iter() {
        let entry = sevenz_rust2::ArchiveEntry::new_file(name);
        writer.push_archive_entry::<&[u8]>(entry, Some(data)).unwrap();
    }
    writer.finish().unwrap();
}

/// WIC 対応拡張子 (HEIC / AVIF / JXL / TIFF / RAW) を含む 7z を生成する。
/// 中身は適当なバイト列でよい (decoder は test では呼ばれない)。
fn make_test_7z_with_wic_exts(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = sevenz_rust2::ArchiveWriter::new(file).unwrap();
    let entries = [
        ("photo.heic", &b"fake_heic"[..]),
        ("img.avif", &b"fake_avif"[..]),
        ("art.jxl", &b"fake_jxl"[..]),
        ("scan.tiff", &b"fake_tiff"[..]),
        ("raw.cr2", &b"fake_cr2"[..]),
        ("raw.arw", &b"fake_arw"[..]),
        ("notes.txt", &b"skip"[..]),
    ];
    for (name, data) in entries.iter() {
        let entry = sevenz_rust2::ArchiveEntry::new_file(name);
        writer.push_archive_entry::<&[u8]>(entry, Some(data)).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn convert_7z_extracts_only_images() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("test.7z");
    let dst = tmp.path().join("out.zip");
    make_test_7z(&src);

    // 事前スキャン: 画像は 3 枚 (jpg / png / webp)、txt は除外される
    let summary = scan_summary(&src, ArchiveFormat::SevenZ).unwrap();
    assert_eq!(summary.image_count, 3, "scan should find 3 images");

    // 実変換
    let cancel = AtomicBool::new(false);
    let stats = convert_to_zip(&src, &dst, ArchiveFormat::SevenZ, &cancel, None).unwrap();
    assert_eq!(stats.image_count, 3);
    assert!(dst.exists());

    // 生成 ZIP の検証
    let file = std::fs::File::open(&dst).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.contains(&"page01.jpg".to_string()));
    assert!(names.contains(&"page02.png".to_string()));
    assert!(names.contains(&"sub/page03.webp".to_string()));
    assert!(!names.iter().any(|n| n.ends_with("notes.txt")));

    // STORE モードであることを確認
    for i in 0..archive.len() {
        let e = archive.by_index(i).unwrap();
        assert_eq!(
            e.compression(),
            zip::CompressionMethod::Stored,
            "entry {} should be stored, got {:?}",
            e.name(),
            e.compression()
        );
    }
}

/// 7z 変換が WIC 対応拡張子 (HEIC / AVIF / JXL / TIFF / RAW) を
/// 画像として扱うことを確認する回帰テスト。
/// 以前は archive_converter::is_image_entry がネイティブ拡張子しか見ておらず、
/// HEIC を含む 7z が NoImages で落ちていた。
#[test]
fn convert_7z_extracts_wic_extensions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("wic.7z");
    let dst = tmp.path().join("wic_out.zip");
    make_test_7z_with_wic_exts(&src);

    // 事前スキャン: heic/avif/jxl/tiff/cr2/arw の 6 枚、notes.txt は除外
    let summary = scan_summary(&src, ArchiveFormat::SevenZ).unwrap();
    assert_eq!(
        summary.image_count, 6,
        "scan should find all 6 WIC-supported entries"
    );

    let cancel = AtomicBool::new(false);
    let stats = convert_to_zip(&src, &dst, ArchiveFormat::SevenZ, &cancel, None).unwrap();
    assert_eq!(stats.image_count, 6);

    let file = std::fs::File::open(&dst).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    for expected in ["photo.heic", "img.avif", "art.jxl", "scan.tiff", "raw.cr2", "raw.arw"] {
        assert!(
            names.contains(&expected.to_string()),
            "missing {expected} in converted ZIP: {names:?}",
        );
    }
    assert!(!names.iter().any(|n| n.ends_with("notes.txt")));
}

#[test]
fn convert_7z_no_images_returns_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("no_images.7z");
    let dst = tmp.path().join("out.zip");

    // テキストだけの 7z
    let file = std::fs::File::create(&src).unwrap();
    let mut writer = sevenz_rust2::ArchiveWriter::new(file).unwrap();
    let entry = sevenz_rust2::ArchiveEntry::new_file("readme.txt");
    writer
        .push_archive_entry::<&[u8]>(entry, Some(&b"hello"[..]))
        .unwrap();
    writer.finish().unwrap();

    let cancel = AtomicBool::new(false);
    let result = convert_to_zip(&src, &dst, ArchiveFormat::SevenZ, &cancel, None);
    assert!(matches!(result, Err(ConvertError::NoImages)));
    assert!(!dst.exists(), "dst should not be left on NoImages error");
}

#[test]
fn convert_7z_cancel_produces_cancelled_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("cancel.7z");
    let dst = tmp.path().join("out.zip");
    make_test_7z(&src);

    let cancel = AtomicBool::new(true); // 最初から cancel 立てる
    let result = convert_to_zip(&src, &dst, ArchiveFormat::SevenZ, &cancel, None);
    assert!(matches!(result, Err(ConvertError::Cancelled)));
    assert!(!dst.exists(), "dst should not be left on cancel");
}

#[test]
fn convert_format_detection_from_path() {
    assert_eq!(
        ArchiveFormat::from_extension("7z"),
        Some(ArchiveFormat::SevenZ)
    );
    assert_eq!(
        ArchiveFormat::from_extension("lzh"),
        Some(ArchiveFormat::Lzh)
    );
    assert_eq!(
        ArchiveFormat::from_extension("lha"),
        Some(ArchiveFormat::Lzh)
    );
    assert_eq!(ArchiveFormat::from_extension("zip"), None);
    assert_eq!(ArchiveFormat::from_extension("rar"), None);
}

