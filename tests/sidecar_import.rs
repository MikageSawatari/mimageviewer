//! フォルダ側サイドカー (`mimageviewer.dat`) の統合テスト。
//!
//! 主な検証対象は「フォルダを丸ごと別ドライブへ移動 → 中央 DB は空 → サイドカーからの復元」
//! というシナリオ。GUI 起動を避けるため、[`mimageviewer::sidecar::import_to_dbs`] を
//! 直接叩き、中央 DB と同じ `open_at()` で一時ファイルを開く。
//!
//! # テストシナリオ
//!
//! | # | ケース | 期待動作 |
//! | - | ------ | ------ |
//! | 1 | フォルダ移動: 空 DB + サイドカーあり | adjust/mask が DB に復元される |
//! | 2 | DB authoritative: DB に既存エントリ | サイドカーの値で上書きされない |
//! | 3 | ZIP 内エントリ | `<folder>/book.zip::001.jpg` 形式で復元 |
//! | 4 | PDF ページ | `<folder>/doc.pdf::page_5` 形式で復元 |
//! | 5 | サイドカー無し | 何もしない (panic しない) |
//! | 6 | バージョン新 (v2) | disabled=true、インポートしない |
//! | 7 | SidecarFile flush → load ラウンドトリップ | 値が保存される |
//! | 8 | 空エントリ → ファイル削除 | flush で .dat が消える |
//! | 9 | 読み取り専用相当 (存在しないフォルダ) | flush が panic しない |

use std::path::Path;
use tempfile::TempDir;

use mimageviewer::adjustment::AdjustParams;
use mimageviewer::adjustment_db::{normalize_path, AdjustmentDb};
use mimageviewer::mask_db::{compress_mask, MaskDb};
use mimageviewer::sidecar::{
    self, reconstruct_image_key, reconstruct_virtual_key, SidecarFile, SidecarMask,
    SIDECAR_FILENAME,
};

// ── ヘルパー ────────────────────────────────────────────────────────

/// テンプフォルダ + 空の中央 DB を用意する。
struct TestEnv {
    /// サイドカー置き場 (ユーザーが画像を持つフォルダに相当)
    folder: TempDir,
    /// 中央 DB 置き場 (%APPDATA% に相当)
    _data_dir: TempDir,
    adjust_db: AdjustmentDb,
    mask_db: MaskDb,
}

impl TestEnv {
    fn new() -> Self {
        let folder = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();
        let adjust_db = AdjustmentDb::open_at(&data_dir.path().join("adjustment.db")).unwrap();
        let mask_db = MaskDb::open_at(&data_dir.path().join("mask.db")).unwrap();
        Self {
            folder,
            _data_dir: data_dir,
            adjust_db,
            mask_db,
        }
    }

    fn folder_path(&self) -> &Path {
        self.folder.path()
    }

    fn sidecar_path(&self) -> std::path::PathBuf {
        self.folder_path().join(SIDECAR_FILENAME)
    }
}

fn sample_params(brightness: f32) -> AdjustParams {
    let mut p = AdjustParams::default();
    p.brightness = brightness;
    p.contrast = 5.0;
    p
}

/// 8x8 の `[(x + y) % 2 == 0]` パターン (目視しやすいチェック柄) のマスク。
fn sample_mask_8x8() -> Vec<bool> {
    let mut m = Vec::with_capacity(64);
    for y in 0..8 {
        for x in 0..8 {
            m.push((x + y) % 2 == 0);
        }
    }
    m
}

/// サイドカーに adjust + mask を書いて flush する。
fn write_sidecar(
    folder: &Path,
    image_adjust: Option<(&str, AdjustParams)>,
    mask_entry: Option<(&str, Vec<bool>, u32, u32)>,
) {
    let mut sc = SidecarFile::new(folder.to_path_buf());
    if let Some((rel, params)) = image_adjust {
        sc.set_adjust(rel, params);
    }
    if let Some((rel, mask, w, h)) = mask_entry {
        let raw = compress_mask(&mask);
        sc.set_mask(rel, SidecarMask::from_raw(&raw, &[], w, h));
    }
    sc.flush();
}

// ── テスト 1: フォルダ移動シナリオ (中心ケース) ───────────────────────

#[test]
fn folder_move_restores_adjust_and_mask_from_sidecar() {
    let env = TestEnv::new();
    let folder = env.folder_path();

    // 旧フォルダで作られたサイドカー (相対キーなので移動後の folder でも有効)
    write_sidecar(
        folder,
        Some(("photo.jpg", sample_params(25.0))),
        Some(("photo.jpg", sample_mask_8x8(), 8, 8)),
    );
    assert!(env.sidecar_path().exists());

    // 中央 DB は空の状態 (= 移動後の新しい %APPDATA% に相当)
    let loaded_sidecar = SidecarFile::load(folder);
    let stats = sidecar::import_to_dbs(
        folder,
        &loaded_sidecar,
        Some(&env.adjust_db),
        Some(&env.mask_db),
    );

    assert_eq!(stats.imported_adjust, 1);
    assert_eq!(stats.imported_mask, 1);

    // 復元後: 中央 DB を絶対パスキーで叩くと取れる
    let abs_key = reconstruct_image_key(folder, "photo.jpg");
    let restored = env.adjust_db.get_page_params(&abs_key).unwrap();
    assert_eq!(restored.brightness, 25.0);
    assert_eq!(restored.contrast, 5.0);

    let mask = env.mask_db.get(&abs_key, 8, 8).unwrap();
    assert_eq!(mask, sample_mask_8x8());
}

// ── テスト 2: 中央 DB が authoritative (サイドカーで上書きされない) ────

#[test]
fn central_db_is_authoritative_over_sidecar() {
    let env = TestEnv::new();
    let folder = env.folder_path();

    let abs_key = reconstruct_image_key(folder, "photo.jpg");
    // DB に先に値 X を入れておく
    env.adjust_db
        .set_page_params(&abs_key, &sample_params(99.0))
        .unwrap();

    // サイドカーに別の値 Y を用意
    write_sidecar(folder, Some(("photo.jpg", sample_params(10.0))), None);

    let loaded_sidecar = SidecarFile::load(folder);
    let stats = sidecar::import_to_dbs(
        folder,
        &loaded_sidecar,
        Some(&env.adjust_db),
        Some(&env.mask_db),
    );

    assert_eq!(stats.imported_adjust, 0, "must not overwrite existing DB entry");
    assert_eq!(stats.skipped_adjust, 1);

    // DB は元の値のまま
    let kept = env.adjust_db.get_page_params(&abs_key).unwrap();
    assert_eq!(kept.brightness, 99.0);
}

// ── テスト 3: ZIP 内エントリ ─────────────────────────────────────────

#[test]
fn zip_image_entry_roundtrip() {
    let env = TestEnv::new();
    let folder = env.folder_path();

    write_sidecar(
        folder,
        Some(("book.zip::001.jpg", sample_params(7.0))),
        Some(("book.zip::001.jpg", sample_mask_8x8(), 8, 8)),
    );

    let loaded_sidecar = SidecarFile::load(folder);
    let stats = sidecar::import_to_dbs(
        folder,
        &loaded_sidecar,
        Some(&env.adjust_db),
        Some(&env.mask_db),
    );
    assert_eq!(stats.imported_adjust, 1);
    assert_eq!(stats.imported_mask, 1);

    let abs_key = reconstruct_virtual_key(folder, "book.zip::001.jpg").unwrap();
    // `App::page_path_key` が ZipImage に対して作るキーと同形式であることをヒューマンリーダブルに確認
    let expected_prefix = normalize_path(&folder.join("book.zip"));
    assert!(
        abs_key.starts_with(&expected_prefix),
        "abs_key={abs_key} expected prefix={expected_prefix}"
    );
    assert!(abs_key.ends_with("::001.jpg"));

    let params = env.adjust_db.get_page_params(&abs_key).unwrap();
    assert_eq!(params.brightness, 7.0);
    let mask = env.mask_db.get(&abs_key, 8, 8).unwrap();
    assert_eq!(mask, sample_mask_8x8());
}

// ── テスト 4: PDF ページ ─────────────────────────────────────────────

#[test]
fn pdf_page_entry_roundtrip() {
    let env = TestEnv::new();
    let folder = env.folder_path();

    write_sidecar(folder, Some(("doc.pdf::page_5", sample_params(-3.0))), None);

    let loaded_sidecar = SidecarFile::load(folder);
    let stats = sidecar::import_to_dbs(
        folder,
        &loaded_sidecar,
        Some(&env.adjust_db),
        Some(&env.mask_db),
    );
    assert_eq!(stats.imported_adjust, 1);

    let abs_key = reconstruct_virtual_key(folder, "doc.pdf::page_5").unwrap();
    assert!(abs_key.ends_with("::page_5"));
    let params = env.adjust_db.get_page_params(&abs_key).unwrap();
    assert_eq!(params.brightness, -3.0);
}

// ── テスト 5: サイドカー無し ────────────────────────────────────────

#[test]
fn missing_sidecar_is_noop() {
    let env = TestEnv::new();
    let folder = env.folder_path();

    // サイドカーを一切書かない
    assert!(!env.sidecar_path().exists());

    let loaded_sidecar = SidecarFile::load(folder);
    let stats = sidecar::import_to_dbs(
        folder,
        &loaded_sidecar,
        Some(&env.adjust_db),
        Some(&env.mask_db),
    );
    assert_eq!(stats.imported_adjust, 0);
    assert_eq!(stats.imported_mask, 0);
}

// ── テスト 6: 新バージョンのサイドカーはインポートしない ─────────────

#[test]
fn newer_version_sidecar_is_skipped() {
    let env = TestEnv::new();
    let folder = env.folder_path();

    // 将来バージョン (v999) のサイドカーを JSON で直接書く
    let fake = serde_json::json!({
        "version": 999,
        "items": {
            "photo.jpg": { "adjust": {"brightness": 42.0} }
        }
    });
    std::fs::write(env.sidecar_path(), fake.to_string()).unwrap();

    let loaded_sidecar = SidecarFile::load(folder);
    // items は空扱いになる (disabled フラグが立ち、読み込まれない)
    assert_eq!(loaded_sidecar.items().len(), 0, "newer version must not leak items");

    let stats = sidecar::import_to_dbs(
        folder,
        &loaded_sidecar,
        Some(&env.adjust_db),
        Some(&env.mask_db),
    );
    assert_eq!(stats.imported_adjust, 0);
}

// ── テスト 7: SidecarFile ラウンドトリップ (flush → load) ───────────

#[test]
fn sidecar_flush_then_load_preserves_data() {
    let env = TestEnv::new();
    let folder = env.folder_path();

    // 書き込み → flush
    {
        let mut sc = SidecarFile::new(folder.to_path_buf());
        sc.set_adjust("a.jpg", sample_params(1.5));
        sc.set_adjust("b.jpg", sample_params(-2.0));
        let raw = compress_mask(&sample_mask_8x8());
        sc.set_mask("a.jpg", SidecarMask::from_raw(&raw, &[], 8, 8));
        sc.flush();
    }

    // 再読み込み
    let sc2 = SidecarFile::load(folder);
    assert_eq!(sc2.items().len(), 2);
    let a = sc2.items().get("a.jpg").unwrap();
    assert!(a.adjust.is_some());
    assert!(a.mask.is_some());
    assert_eq!(a.adjust.as_ref().unwrap().brightness, 1.5);

    let b = sc2.items().get("b.jpg").unwrap();
    assert!(b.adjust.is_some());
    assert!(b.mask.is_none());
}

// ── テスト 8: 全削除で .dat が消える ─────────────────────────────────

#[test]
fn flush_removes_dat_when_all_entries_cleared() {
    let env = TestEnv::new();
    let folder = env.folder_path();
    let path = env.sidecar_path();

    // 1 エントリ書いて flush → ファイルができる
    let mut sc = SidecarFile::new(folder.to_path_buf());
    sc.set_adjust("a.jpg", sample_params(1.0));
    sc.flush();
    assert!(path.exists());

    // 削除 → flush → ファイルが消える
    sc.remove_adjust("a.jpg");
    sc.flush();
    assert!(!path.exists(), "sidecar file must be removed when items empty");
}

// ── テスト 9: 書き込み不能フォルダで落ちない ────────────────────────

#[test]
fn flush_on_nonexistent_folder_does_not_panic() {
    // 存在しないパスを指定 (読み取り専用メディアの代用)。flush がクラッシュしないことを確認。
    let mut sc = SidecarFile::new("Z:/this/path/definitely/does/not/exist/__mimageviewer_test__".into());
    sc.set_adjust("a.jpg", sample_params(1.0));
    sc.flush(); // 失敗するはずだが panic してはいけない

    // 以降の flush はすべて no-op (disabled フラグ) になる
    sc.set_adjust("b.jpg", sample_params(2.0));
    sc.flush();
}

// ── テスト 10: サイドカー有 + DB 一部既存 (混合) ────────────────────

#[test]
fn partial_overlap_imports_only_missing() {
    let env = TestEnv::new();
    let folder = env.folder_path();

    // DB には a.jpg だけ既にある
    let abs_a = reconstruct_image_key(folder, "a.jpg");
    env.adjust_db
        .set_page_params(&abs_a, &sample_params(100.0))
        .unwrap();

    // サイドカーには a.jpg と b.jpg 両方ある
    {
        let mut sc = SidecarFile::new(folder.to_path_buf());
        sc.set_adjust("a.jpg", sample_params(1.0));
        sc.set_adjust("b.jpg", sample_params(2.0));
        sc.flush();
    }

    let loaded_sidecar = SidecarFile::load(folder);
    let stats = sidecar::import_to_dbs(
        folder,
        &loaded_sidecar,
        Some(&env.adjust_db),
        Some(&env.mask_db),
    );
    assert_eq!(stats.imported_adjust, 1, "only b.jpg was missing");
    assert_eq!(stats.skipped_adjust, 1);

    // a.jpg は元の値のまま
    let a = env.adjust_db.get_page_params(&abs_a).unwrap();
    assert_eq!(a.brightness, 100.0);
    // b.jpg は復元
    let abs_b = reconstruct_image_key(folder, "b.jpg");
    let b = env.adjust_db.get_page_params(&abs_b).unwrap();
    assert_eq!(b.brightness, 2.0);
}

// ── テスト 11: ZIP のフォルダ相対キー構築が App::page_path_key と一致 ──

#[test]
fn zip_and_pdf_keys_match_normalize_convention() {
    // テスト 3/4 の再確認: reconstruct_virtual_key が adjustment_db::normalize_path と
    // 整合した出力を返すこと。これがずれると import で復元されない。
    let folder = Path::new("C:/Books/Comics");

    let zip_key = reconstruct_virtual_key(folder, "vol1.zip::001.jpg").unwrap();
    let expected_zip = format!("{}::001.jpg", normalize_path(&folder.join("vol1.zip")));
    assert_eq!(zip_key, expected_zip);

    let pdf_key = reconstruct_virtual_key(folder, "manual.pdf::page_10").unwrap();
    let expected_pdf = format!("{}::page_10", normalize_path(&folder.join("manual.pdf")));
    assert_eq!(pdf_key, expected_pdf);
}

// ── テスト 12: 属性無視 (非 Windows) / 設定時 (Windows) で flush 成功 ──

#[test]
fn flush_succeeds_and_file_readable_even_with_hidden_attrs() {
    // Windows では HIDDEN+SYSTEM 属性が付くが、プロセス内からは普通に読めるはず。
    let env = TestEnv::new();
    let folder = env.folder_path();

    {
        let mut sc = SidecarFile::new(folder.to_path_buf());
        sc.set_adjust("x.jpg", sample_params(3.14));
        sc.flush();
    }

    // 再読込
    let sc2 = SidecarFile::load(folder);
    assert_eq!(sc2.items().len(), 1);
    let params = sc2.items().get("x.jpg").unwrap().adjust.as_ref().unwrap();
    assert_eq!(params.brightness, 3.14);

    // 上書きも動くこと (既存 HIDDEN+SYSTEM ファイルの rename)
    {
        let mut sc3 = SidecarFile::new(folder.to_path_buf());
        sc3.set_adjust("x.jpg", sample_params(2.71));
        sc3.flush();
    }
    let sc4 = SidecarFile::load(folder);
    assert_eq!(
        sc4.items().get("x.jpg").unwrap().adjust.as_ref().unwrap().brightness,
        2.71
    );
}
