//! catalog モジュールの統合テスト。
//! 実ファイルシステム上で CatalogDb のライフサイクルをテストする。

use std::collections::HashSet;
use std::path::Path;
use tempfile::TempDir;

use mimageviewer::catalog;

#[test]
fn catalog_db_full_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");
    let folder = Path::new(r"C:\test\photos");

    // 1) DB を開いてスキーマが正常に作成される
    let db = catalog::CatalogDb::open(&cache_dir, folder).unwrap();

    // 2) save → load_all のラウンドトリップ
    db.save("img1.jpg", 1000, 5000, 256, 192, Some((4000, 3000)), b"webp_data_1")
        .unwrap();
    db.save("img2.png", 2000, 8000, 256, 256, None, b"webp_data_2")
        .unwrap();

    let map = db.load_all().unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map["img1.jpg"].mtime, 1000);
    assert_eq!(map["img1.jpg"].source_dims, Some((4000, 3000)));
    assert_eq!(map["img2.png"].source_dims, None);

    // 3) delete_missing で不要エントリを削除
    let existing: HashSet<String> = ["img1.jpg".to_string()].into_iter().collect();
    db.delete_missing(&existing).unwrap();
    let map = db.load_all().unwrap();
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("img1.jpg"));

    // 4) 上書き保存
    db.save("img1.jpg", 3000, 6000, 256, 192, Some((4000, 3000)), b"updated")
        .unwrap();
    let map = db.load_all().unwrap();
    assert_eq!(map["img1.jpg"].mtime, 3000);
    assert_eq!(map["img1.jpg"].jpeg_data, b"updated");
}

#[test]
fn db_path_for_creates_subdirectory() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");
    let folder = Path::new(r"D:\my_photos");

    // open すると DB ファイルとサブディレクトリが作成される
    let _db = catalog::CatalogDb::open(&cache_dir, folder).unwrap();

    let db_path = catalog::db_path_for(&cache_dir, folder);
    assert!(db_path.exists(), "DB file should exist at {}", db_path.display());
}

#[test]
fn encode_decode_webp_pipeline() {
    // 小さなテスト画像を生成
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(100, 80, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    }));

    // WebP エンコード
    let (webp_data, w, h) = catalog::encode_thumb_webp(&img, 64, 75.0).unwrap();
    assert!(!webp_data.is_empty());
    assert!(w <= 64);
    assert!(h <= 64);

    // WebP デコード
    let color_image = catalog::decode_thumb_to_color_image(&webp_data);
    assert!(color_image.is_some());
    let ci = color_image.unwrap();
    assert_eq!(ci.size[0], w as usize);
    assert_eq!(ci.size[1], h as usize);
}

#[test]
fn cache_stats_and_delete() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");

    // DB を2つ作成して閉じる
    {
        let _db1 = catalog::CatalogDb::open(&cache_dir, Path::new(r"C:\folder1")).unwrap();
        let _db2 = catalog::CatalogDb::open(&cache_dir, Path::new(r"C:\folder2")).unwrap();
    }

    let (count, bytes) = catalog::cache_stats(&cache_dir);
    assert_eq!(count, 2);
    assert!(bytes > 0);

    // 全削除
    let deleted = catalog::delete_all_cache(&cache_dir);
    assert_eq!(deleted, 2);

    let (count, _) = catalog::cache_stats(&cache_dir);
    assert_eq!(count, 0);
}

#[test]
fn reopen_db_preserves_data() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");
    let folder = Path::new(r"C:\persistent");

    // 1) 開いてデータ保存
    {
        let db = catalog::CatalogDb::open(&cache_dir, folder).unwrap();
        db.save("photo.jpg", 999, 1234, 128, 96, Some((1920, 1080)), b"persistent_data")
            .unwrap();
    }

    // 2) 再度開いてデータが残っていることを確認
    {
        let db = catalog::CatalogDb::open(&cache_dir, folder).unwrap();
        let map = db.load_all().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map["photo.jpg"].jpeg_data, b"persistent_data");
        assert_eq!(map["photo.jpg"].source_dims, Some((1920, 1080)));
    }
}
