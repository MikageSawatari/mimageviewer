//! settings モジュールの統合テスト。
//! JSON ファイル経由での設定読み書きをテストする。

use tempfile::TempDir;
use mimageviewer::settings::{Settings, FavoriteEntry, ThumbAspect, SortOrder, CachePolicy, Parallelism};

/// Settings を JSON ファイルに保存し、そこから読み込むラウンドトリップ。
/// Settings::load/save は固定パスを使うので、直接 serde を使ってテストする。
#[test]
fn settings_json_file_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("settings.json");

    let mut original = Settings::default();
    original.grid_cols = 6;
    original.thumb_px = 256;
    original.thumb_quality = 90;

    // ファイルに保存
    let json = serde_json::to_string_pretty(&original).unwrap();
    std::fs::write(&path, &json).unwrap();

    // ファイルから読み込み
    let data = std::fs::read_to_string(&path).unwrap();
    let loaded: Settings = serde_json::from_str(&data).unwrap();

    assert_eq!(loaded.grid_cols, 6);
    assert_eq!(loaded.thumb_px, 256);
    assert_eq!(loaded.thumb_quality, 90);
}

#[test]
fn broken_json_falls_back_to_defaults() {
    let broken = "{ invalid json !!!! }";
    let result: Result<Settings, _> = serde_json::from_str(broken);
    assert!(result.is_err());
    // アプリでは unwrap_or_default() でフォールバックする
    let settings = result.unwrap_or_default();
    assert_eq!(settings.grid_cols, 4);
}

#[test]
fn legacy_favorites_migration() {
    // 旧フォーマット（文字列配列）と新フォーマット（オブジェクト配列）の混在
    let json = r#"{
        "favorites": [
            "C:\\old_style\\folder",
            {"name": "New Style", "path": "D:\\new"}
        ]
    }"#;
    let loaded: Settings = serde_json::from_str(json).unwrap();
    assert_eq!(loaded.favorites.len(), 2);
    assert_eq!(loaded.favorites[0].name, "folder");
    assert_eq!(loaded.favorites[0].path, std::path::PathBuf::from(r"C:\old_style\folder"));
    assert_eq!(loaded.favorites[1].name, "New Style");
}

#[test]
fn partial_json_uses_defaults_for_missing_fields() {
    let json = r#"{"grid_cols": 8, "thumb_px": 1024}"#;
    let loaded: Settings = serde_json::from_str(json).unwrap();
    assert_eq!(loaded.grid_cols, 8);
    assert_eq!(loaded.thumb_px, 1024);
    // 指定していないフィールドはデフォルト値
    assert_eq!(loaded.thumb_quality, 75);
    assert_eq!(loaded.prefetch_back, 4);
    assert_eq!(loaded.cache_policy, CachePolicy::Auto);
}

#[test]
fn all_enums_survive_roundtrip() {
    // 全 enum バリアントが serialize → deserialize で保持される
    for aspect in ThumbAspect::all() {
        let json = serde_json::to_string(aspect).unwrap();
        let loaded: ThumbAspect = serde_json::from_str(&json).unwrap();
        assert_eq!(*aspect, loaded);
    }

    for order in SortOrder::all() {
        let json = serde_json::to_string(order).unwrap();
        let loaded: SortOrder = serde_json::from_str(&json).unwrap();
        assert_eq!(*order, loaded);
    }

    for policy in &[CachePolicy::Off, CachePolicy::Auto, CachePolicy::Always] {
        let json = serde_json::to_string(policy).unwrap();
        let loaded: CachePolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(*policy, loaded);
    }

    for par in &[Parallelism::Auto, Parallelism::Manual(1), Parallelism::Manual(8)] {
        let json = serde_json::to_string(par).unwrap();
        let loaded: Parallelism = serde_json::from_str(&json).unwrap();
        assert_eq!(*par, loaded);
    }
}
