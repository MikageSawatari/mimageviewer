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

/// 内側に ZIP を含む外側 ZIP を作る。
fn write_inner_zip() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("inner_img1.jpg", options).unwrap();
        w.write_all(b"inner jpg 1").unwrap();
        w.start_file("sub/inner_img2.png", options).unwrap();
        w.write_all(b"inner png 2").unwrap();
        w.finish().unwrap();
    }
    cursor.into_inner()
}

#[test]
fn enumerate_includes_nested_zip_entries() {
    let tmp = TempDir::new().unwrap();
    let zip_path = tmp.path().join("outer.zip");
    let inner_bytes = write_inner_zip();

    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("cover.jpg", options).unwrap();
        w.write_all(b"cover jpg").unwrap();
        w.start_file("chapters/ch01.zip", options).unwrap();
        w.write_all(&inner_bytes).unwrap();
        w.finish().unwrap();
    }

    let entries = zip_loader::enumerate_image_entries(&zip_path).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.entry_name.as_str()).collect();
    assert!(names.contains(&"cover.jpg"));
    assert!(names.contains(&"chapters/ch01.zip/inner_img1.jpg"));
    assert!(names.contains(&"chapters/ch01.zip/sub/inner_img2.png"));
    assert_eq!(entries.len(), 3);
}

#[test]
fn read_entry_bytes_from_nested_zip() {
    let tmp = TempDir::new().unwrap();
    let zip_path = tmp.path().join("outer.zip");
    let inner_bytes = write_inner_zip();

    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("chapters/ch01.zip", options).unwrap();
        w.write_all(&inner_bytes).unwrap();
        w.finish().unwrap();
    }

    let data =
        zip_loader::read_entry_bytes(&zip_path, "chapters/ch01.zip/inner_img1.jpg").unwrap();
    assert_eq!(data, b"inner jpg 1");
    let data =
        zip_loader::read_entry_bytes(&zip_path, "chapters/ch01.zip/sub/inner_img2.png").unwrap();
    assert_eq!(data, b"inner png 2");
}

#[test]
fn enumerate_two_level_nested_zip() {
    let tmp = TempDir::new().unwrap();
    let zip_path = tmp.path().join("outer.zip");

    // Level 2 inner: contains just img.png
    let mut l2 = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut l2);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("img.png", options).unwrap();
        w.write_all(b"deepest").unwrap();
        w.finish().unwrap();
    }
    let l2_bytes = l2.into_inner();

    // Level 1: contains l2.zip
    let mut l1 = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut l1);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("l2.zip", options).unwrap();
        w.write_all(&l2_bytes).unwrap();
        w.finish().unwrap();
    }
    let l1_bytes = l1.into_inner();

    // Outer: contains l1.zip
    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("l1.zip", options).unwrap();
        w.write_all(&l1_bytes).unwrap();
        w.finish().unwrap();
    }

    let entries = zip_loader::enumerate_image_entries(&zip_path).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entry_name, "l1.zip/l2.zip/img.png");

    let data = zip_loader::read_entry_bytes(&zip_path, "l1.zip/l2.zip/img.png").unwrap();
    assert_eq!(data, b"deepest");
}
