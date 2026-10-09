//! Windows publish failures must retain the existing destination and produce actionable diagnostics.
#![cfg(windows)]

use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::AtomicBool;

use mimageviewer::archive_converter::{
    ArchiveFormat, ConvertError, ConvertOptions, convert_to_zip, convert_to_zip_with_password,
};

#[test]
fn locked_zip_publish_reports_plain_message_and_native_error_without_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    // This integration binary owns its logger and uses only disposable data.
    mimageviewer::data_dir::DATA_DIR
        .set(dir.path().join("data"))
        .unwrap();
    mimageviewer::logger::init_for_worker("archive-conversion-failure-test");

    let src = dir.path().join("source.zip");
    let dst = dir.path().join("book.zip");
    let tmp = dst.with_extension("zip.part");
    let mut source_zip = zip::ZipWriter::new(std::fs::File::create(&src).unwrap());
    source_zip
        .start_file(
            "page.jpg",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    source_zip.write_all(b"image bytes").unwrap();
    source_zip.finish().unwrap();
    std::fs::write(&dst, b"existing user ZIP").unwrap();
    // Allow readers and writers, but refuse deletion/replacement while the handle is open.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001 | 0x0000_0002)
        .open(&dst)
        .unwrap();

    let result = convert_to_zip(
        &src,
        &dst,
        ArchiveFormat::Zip,
        &AtomicBool::new(false),
        None,
    );

    let ConvertError::Archive(message) = result.unwrap_err() else {
        panic!("publish failure should have a plain user-facing message");
    };
    assert_eq!(
        message,
        "変換したZIPを保存できませんでした。保存先が使用中か、読み取り専用か、書き込みが許可されていません。"
    );
    assert_eq!(std::fs::read(&dst).unwrap(), b"existing user ZIP");
    assert!(
        !tmp.exists(),
        "failed conversion must clean its temporary ZIP"
    );

    let log_path = mimageviewer::data_dir::logs_dir().join("archive-conversion-failure-test.log");
    let log = std::fs::read_to_string(&log_path).unwrap();
    let publish_line = log
        .lines()
        .find(|line| line.contains("failed operation=publish_zip"))
        .expect("publish failure must use the existing file logger");
    for expected in [
        format!("src={src:?}"),
        format!("tmp={tmp:?}"),
        format!("dst={dst:?}"),
        "native_error=Some(5)".to_string(),
        "PermissionDenied".to_string(),
    ] {
        assert!(
            publish_line.contains(&expected),
            "missing {expected}: {publish_line}"
        );
    }
    drop(held);

    // Explicit sibling/batch conversions still refuse an existing destination.
    let result = convert_to_zip_with_password(
        &src,
        &dst,
        ArchiveFormat::Zip,
        None,
        &AtomicBool::new(false),
        None,
        ConvertOptions {
            no_clobber: true,
            verify: true,
        },
    );
    assert!(matches!(
        result,
        Err(ConvertError::Archive(message))
            if message == "同名の ZIP が既に存在するため上書きしませんでした"
    ));
    assert_eq!(std::fs::read(&dst).unwrap(), b"existing user ZIP");
    assert!(!tmp.exists());

    // Releasing the lock permits the same conversion without retry machinery.
    let summary = convert_to_zip(
        &src,
        &dst,
        ArchiveFormat::Zip,
        &AtomicBool::new(false),
        None,
    )
    .unwrap();
    assert_eq!(summary.image_count, 1);
    let mut converted = zip::ZipArchive::new(std::fs::File::open(&dst).unwrap()).unwrap();
    let mut image = Vec::new();
    std::io::Read::read_to_end(&mut converted.by_name("page.jpg").unwrap(), &mut image).unwrap();
    assert_eq!(image, b"image bytes");
    assert!(!tmp.exists());
}
