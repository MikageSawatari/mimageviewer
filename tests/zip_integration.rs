//! zip_loader モジュールの統合テスト。
//! 実 ZIP ファイルを使用してエントリ列挙をテストする。

use std::io::Write;
use tempfile::TempDir;

use mimageviewer::zip_loader;

/// テスト用の最小 ZIP ファイルを作成し、画像エントリを列挙する。
#[test]
fn enumerate_image_entries_from_real_zip() {
    let tmp = TempDir::new().unwrap();
    let zip_path = tmp.path().join("test.zip");

    // ZIP ファイルを作成
    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip_writer = zip::ZipWriter::new(file);

        // 画像ファイルを追加（中身はダミー）
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip_writer.start_file("img1.jpg", options).unwrap();
        zip_writer.write_all(b"fake jpg data").unwrap();

        zip_writer.start_file("subdir/img2.png", options).unwrap();
        zip_writer.write_all(b"fake png data").unwrap();

        zip_writer.start_file("deep/nested/img3.webp", options).unwrap();
        zip_writer.write_all(b"fake webp data").unwrap();

        // 非画像ファイル（除外されるはず）
        zip_writer.start_file("readme.txt", options).unwrap();
        zip_writer.write_all(b"not an image").unwrap();

        zip_writer.start_file("data.json", options).unwrap();
        zip_writer.write_all(b"{}").unwrap();

        zip_writer.finish().unwrap();
    }

    // 列挙
    let entries = zip_loader::enumerate_image_entries(&zip_path).unwrap();

    // 画像エントリのみ 3 件
    assert_eq!(entries.len(), 3);

    let names: Vec<&str> = entries.iter().map(|e| e.entry_name.as_str()).collect();
    assert!(names.contains(&"img1.jpg"));
    assert!(names.contains(&"subdir/img2.png"));
    assert!(names.contains(&"deep/nested/img3.webp"));
}

#[test]
fn enumerate_empty_zip_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let zip_path = tmp.path().join("empty.zip");

    // 画像なし ZIP を作成
    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip_writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip_writer.start_file("readme.txt", options).unwrap();
        zip_writer.write_all(b"no images here").unwrap();
        zip_writer.finish().unwrap();
    }

    let entries = zip_loader::enumerate_image_entries(&zip_path).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn entry_name_helpers() {
    assert_eq!(zip_loader::entry_basename("work1/ch01/page01.jpg"), "page01.jpg");
    assert_eq!(zip_loader::entry_basename("root_image.png"), "root_image.png");
    assert_eq!(zip_loader::entry_dir("work1/ch01/page01.jpg"), "work1/ch01");
    assert_eq!(zip_loader::entry_dir("root_image.png"), "");
}
