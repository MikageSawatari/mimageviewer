//! `zip_loader::enumerate_image_entries` の end-to-end テスト。
//!
//! 以前、ZIP 内画像判定は `zip_loader::IMAGE_EXTS` のハードコードリスト
//! (jpg/jpeg/png/webp/bmp/gif) を見ていたため、本体が開ける HEIC / AVIF /
//! JXL / TIFF / RAW が ZIP 内ではサムネイル一覧から落ちるバグがあった
//! (v0.7.0 で `folder_tree::is_recognized_image_ext` 経由に統一)。
//!
//! ここではサンプル ZIP を temp フォルダに作って列挙結果を検証する。
//! Susie 拡張子 (PI / MAG 等) はプール初期化に実ワーカー exe + プラグインが
//! 必要なため `tests/susie_integration.rs` 側で扱う。

use std::io::Write;
use std::path::Path;

use mimageviewer::zip_loader::enumerate_image_entries;

/// 指定エントリ名・バイト列で ZIP を構築する (STORE モード、最小構成)。
fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(data).unwrap();
    }
    zw.finish().unwrap();
}

/// ネイティブ拡張子だけの ZIP は従来通り正しく列挙される (ベースライン)。
#[test]
fn enumerate_native_extensions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let zip = tmp.path().join("native.zip");
    make_zip(
        &zip,
        &[
            ("page01.jpg", b"fake"),
            ("page02.png", b"fake"),
            ("page03.webp", b"fake"),
            ("page04.gif", b"fake"),
            ("page05.bmp", b"fake"),
            ("notes.txt", b"skip"),
        ],
    );

    let entries = enumerate_image_entries(&zip).unwrap();
    let names: Vec<String> = entries.iter().map(|e| e.entry_name.clone()).collect();
    assert_eq!(names.len(), 5, "expected 5 image entries, got {names:?}");
    assert!(names.contains(&"page01.jpg".to_string()));
    assert!(names.contains(&"page05.bmp".to_string()));
    assert!(!names.iter().any(|n| n.ends_with("notes.txt")));
}

/// WIC 対応拡張子 (HEIC / AVIF / JXL / TIFF / RAW) を含む ZIP でも
/// 画像として列挙される。v0.7.0 以前は hardcoded `IMAGE_EXTS` に含まれない
/// ため列挙から落ちていた。
#[test]
fn enumerate_wic_extensions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let zip = tmp.path().join("wic.zip");
    make_zip(
        &zip,
        &[
            ("photo.heic", b"fake"),
            ("pic.heif", b"fake"),
            ("anime.avif", b"fake"),
            ("art.jxl", b"fake"),
            ("scan.tiff", b"fake"),
            ("scan.tif", b"fake"),
            ("raw.dng", b"fake"),
            ("raw.cr2", b"fake"),
            ("raw.cr3", b"fake"),
            ("raw.nef", b"fake"),
            ("raw.arw", b"fake"),
            ("raw.raf", b"fake"),
            ("raw.orf", b"fake"),
            ("notes.txt", b"skip"),
        ],
    );

    let entries = enumerate_image_entries(&zip).unwrap();
    let names: Vec<String> = entries.iter().map(|e| e.entry_name.clone()).collect();
    assert_eq!(
        names.len(),
        13,
        "expected all 13 WIC-supported entries, got {names:?}",
    );
    for expected in [
        "photo.heic",
        "pic.heif",
        "anime.avif",
        "art.jxl",
        "scan.tiff",
        "scan.tif",
        "raw.dng",
        "raw.cr2",
        "raw.cr3",
        "raw.nef",
        "raw.arw",
        "raw.raf",
        "raw.orf",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "missing {expected} in enumerate_image_entries result: {names:?}",
        );
    }
    assert!(!names.iter().any(|n| n.ends_with("notes.txt")));
}

/// ネイティブ / WIC / 非画像を混在させても、画像だけが正しい順序で列挙される。
#[test]
fn enumerate_mixed_extensions_preserves_zip_order() {
    let tmp = tempfile::TempDir::new().unwrap();
    let zip = tmp.path().join("mixed.zip");
    make_zip(
        &zip,
        &[
            ("intro.txt", b"skip"),
            ("page01.jpg", b"fake"),
            ("page02.heic", b"fake"),
            ("readme.md", b"skip"),
            ("page03.png", b"fake"),
            ("raw/page04.cr2", b"fake"),
            ("audio.mp3", b"skip"),
            ("page05.webp", b"fake"),
        ],
    );

    let entries = enumerate_image_entries(&zip).unwrap();
    let names: Vec<String> = entries.iter().map(|e| e.entry_name.clone()).collect();
    assert_eq!(
        names,
        vec![
            "page01.jpg".to_string(),
            "page02.heic".to_string(),
            "page03.png".to_string(),
            "raw/page04.cr2".to_string(),
            "page05.webp".to_string(),
        ],
        "entries should appear in ZIP central-directory order, images only",
    );
}
