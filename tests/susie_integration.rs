//! Susie ワーカーサブプロセスを介した end-to-end 統合テスト。
//!
//! 64bit の本体側から `susie_loader::decode_file` を呼び、以下を検証する:
//!   - 32bit ワーカープロセスが spawn できる
//!   - Handshake でプラグイン一覧が取れる
//!   - `decode_file` が IPC 越しに RGBA 画像を返す
//!   - 返ってきた画像が BMP リファレンスと同じ寸法・ほぼ同じ内容になっている
//!     (BGRA → RGBA 変換のチャネル入れ替え崩れ等を検出)
//!
//! ## 必要な前提
//! 1. 32bit ワーカーをリリースビルド済み:
//!    `cargo build --release --target i686-pc-windows-msvc -p mimageviewer-susie32`
//! 2. testdata が配置済み (`MIV_TESTDATA` / `<repo>/testdata` / `C:\home\mimageviewer\testdata`)。
//!
//! テストは testdata / ワーカー exe が無いと自動的に skip する。
//!
//! ## 実行
//! ```
//! cargo test --target x86_64-pc-windows-msvc --test susie_integration -- --test-threads=1
//! ```
//! pool は `OnceLock` のグローバルを共有するため、環境変数を `set_var` するタイミングで
//! race しないよう `--test-threads=1` を推奨。

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::Once;

use mimageviewer::susie_loader;

static SETUP: Once = Once::new();

fn setup_env() {
    SETUP.call_once(|| {
        let Some(worker) = find_worker_exe() else {
            eprintln!("skip-hint: 32bit worker exe not found");
            return;
        };
        let Some(root) = find_testdata_root() else {
            eprintln!("skip-hint: testdata root not found");
            return;
        };
        let plugin_dir = root.join("susie-plugins").join("extracted");
        eprintln!("setup worker={}", worker.display());
        eprintln!("setup plugins={}", plugin_dir.display());
        // Rust 2024 edition: set_var は unsafe
        unsafe {
            std::env::set_var("MIV_SUSIE_WORKER", &worker);
            std::env::set_var("MIV_SUSIE_PLUGIN_DIR", &plugin_dir);
        }
    });
}

fn find_worker_exe() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest.join("target/i686-pc-windows-msvc/release/mimageviewer-susie32.exe"),
        manifest.join("target/i686-pc-windows-msvc/debug/mimageviewer-susie32.exe"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn find_testdata_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MIV_TESTDATA") {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ws = manifest.join("testdata");
    if ws.is_dir() {
        return Some(ws);
    }
    let fixed = PathBuf::from(r"C:\home\mimageviewer\testdata");
    if fixed.is_dir() {
        return Some(fixed);
    }
    None
}

fn testdata_ok() -> bool {
    find_worker_exe().is_some() && find_testdata_root().is_some()
}

fn bmp_path(name: &str) -> Option<PathBuf> {
    find_testdata_root()
        .map(|r| r.join("retro-images").join("formats").join("bmp").join(name))
        .filter(|p| p.exists())
}

fn retro_path(format: &str, name: &str) -> Option<PathBuf> {
    find_testdata_root()
        .map(|r| r.join("retro-images").join("formats").join(format).join(name))
        .filter(|p| p.exists())
}

/// 2 枚の RGBA 画像 (同寸法) のチャネル平均絶対誤差を返す。
fn mean_abs_diff_rgba(a: &image::RgbaImage, b: &image::RgbaImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    let mut sum: u64 = 0;
    let mut count: u64 = 0;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        for i in 0..3 {
            let diff = (pa.0[i] as i32 - pb.0[i] as i32).unsigned_abs() as u64;
            sum += diff;
            count += 1;
        }
    }
    sum as f64 / count as f64
}

/// ワーカープール起動 + プラグイン列挙。
#[test]
fn pool_initializes_and_reports_plugins() {
    setup_env();
    if !testdata_ok() {
        eprintln!("skip: missing worker exe or testdata");
        return;
    }
    // pool 未起動なら起動する
    susie_loader::reload(true, true);
    let pool = susie_loader::try_get_pool().expect("pool should be initialized after reload");
    assert!(pool.is_ready(), "pool not ready after reload");

    let plugins = pool.plugins();
    eprintln!("loaded {} plugins", plugins.len());
    for p in plugins {
        eprintln!("  {} ({})", p.name, p.extensions.join(","));
    }
    assert!(!plugins.is_empty(), "no plugins loaded");
    assert!(
        pool.supports_extension("pi"),
        "pool does not report 'pi' extension"
    );
    assert!(
        pool.supports_extension("mag"),
        "pool does not report 'mag' extension"
    );
}

/// `decode_file` 経由で PI を IPC デコードし、BMP 参照と比較。
#[test]
fn decode_pi_matches_bmp_reference() {
    setup_env();
    if !testdata_ok() {
        eprintln!("skip: missing worker exe or testdata");
        return;
    }
    let Some(pi) = retro_path("pi", "C165.PI") else {
        eprintln!("skip: missing C165.PI");
        return;
    };
    let Some(bmp) = bmp_path("C165.BMP") else {
        eprintln!("skip: missing C165.BMP");
        return;
    };
    susie_loader::reload(true, true);

    let decoded = susie_loader::decode_file(&pi, true, None).expect("IPC decode failed");
    let decoded_rgba = decoded.to_rgba8();
    let reference = image::open(&bmp).expect("BMP open failed").to_rgba8();
    eprintln!(
        "decoded PI: {}x{}, reference BMP: {}x{}",
        decoded_rgba.width(),
        decoded_rgba.height(),
        reference.width(),
        reference.height()
    );
    assert_eq!(decoded_rgba.dimensions(), reference.dimensions());

    let diff = mean_abs_diff_rgba(&decoded_rgba, &reference);
    eprintln!("mean abs channel diff vs BMP: {diff:.2}");
    // 完全一致は期待しない (PI と BMP のパレット丸め差の可能性)。20/255 以下を目安とする。
    assert!(diff < 20.0, "PI vs BMP pixel diff too large: {diff}");
}

/// MAG でも同様に BMP と近い内容になっているか。
#[test]
fn decode_mag_matches_bmp_reference() {
    setup_env();
    if !testdata_ok() {
        eprintln!("skip: missing worker exe or testdata");
        return;
    }
    let Some(mag) = retro_path("mag", "C165.MAG") else {
        return;
    };
    let Some(bmp) = bmp_path("C165.BMP") else {
        return;
    };
    susie_loader::reload(true, true);

    let decoded = susie_loader::decode_file(&mag, true, None).expect("IPC decode failed");
    let decoded_rgba = decoded.to_rgba8();
    let reference = image::open(&bmp).expect("BMP open failed").to_rgba8();
    assert_eq!(decoded_rgba.dimensions(), reference.dimensions());

    let diff = mean_abs_diff_rgba(&decoded_rgba, &reference);
    eprintln!("mean abs channel diff vs BMP: {diff:.2}");
    assert!(diff < 20.0, "MAG vs BMP pixel diff too large: {diff}");
}

/// バイト列からのデコード (ZIP 内画像の経路をシミュレート)。
#[test]
fn decode_bytes_via_ipc() {
    setup_env();
    if !testdata_ok() {
        eprintln!("skip: missing worker exe or testdata");
        return;
    }
    let Some(pi) = retro_path("pi", "C165.PI") else {
        return;
    };
    let Some(bmp) = bmp_path("C165.BMP") else {
        return;
    };
    susie_loader::reload(true, true);

    let bytes = std::fs::read(&pi).unwrap();
    let decoded =
        susie_loader::decode_bytes("C165.PI", &bytes, true, None).expect("bytes decode failed");
    let decoded_rgba = decoded.to_rgba8();
    let reference = image::open(&bmp).expect("BMP open failed").to_rgba8();
    assert_eq!(decoded_rgba.dimensions(), reference.dimensions());
    let diff = mean_abs_diff_rgba(&decoded_rgba, &reference);
    assert!(diff < 20.0, "bytes-decode PI vs BMP diff too large: {diff}");
}

/// 並列実行オフ設定 (worker_count = 1) でもデコードが通ること。
#[test]
fn decode_works_with_parallel_off() {
    setup_env();
    if !testdata_ok() {
        eprintln!("skip: missing worker exe or testdata");
        return;
    }
    let Some(pi) = retro_path("pi", "C165.PI") else {
        return;
    };
    // parallel = false: プールサイズ 1 で再起動
    susie_loader::reload(true, false);
    let decoded = susie_loader::decode_file(&pi, true, None).expect("single-worker decode failed");
    assert!(decoded.width() > 0 && decoded.height() > 0);

    // パラレル ON に戻しておく (後続テストが影響を受けないように)
    susie_loader::reload(true, true);
}

// -----------------------------------------------------------------------
// ZIP / 7z から Susie 対応拡張子 (PI / MAG) を検出できるかの end-to-end テスト
// -----------------------------------------------------------------------
//
// v0.7.0 Phase A では zip_loader が独自ハードコードの IMAGE_EXTS
// (jpg/jpeg/png/webp/bmp/gif) しか見ておらず、ZIP 内の PI / MAG がサムネイル
// 一覧から落ちる不整合があった。`is_recognized_image_ext` 経由への統一 +
// Susie プール初期化待ちを行うようにした上で、以下のテストで回帰を検知する。

use std::io::Write as _;

fn make_zip_with_entries(
    path: &std::path::Path,
    entries: &[(&str, &[u8])],
) {
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

fn make_7z_with_entries(
    path: &std::path::Path,
    entries: &[(&str, &[u8])],
) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = sevenz_rust2::ArchiveWriter::new(file).unwrap();
    for (name, data) in entries {
        let entry = sevenz_rust2::ArchiveEntry::new_file(name);
        writer.push_archive_entry::<&[u8]>(entry, Some(*data)).unwrap();
    }
    writer.finish().unwrap();
}

/// ZIP 内の `.pi` / `.mag` エントリが `enumerate_image_entries` で列挙される。
/// ZIP の中身は空バイトで十分 (デコードは呼ばれない)。
#[test]
fn zip_enumerates_susie_extensions() {
    setup_env();
    if !testdata_ok() {
        eprintln!("skip: missing worker exe or testdata");
        return;
    }
    // プール未起動なら起動し、Susie が pi/mag を報告する状態にする。
    susie_loader::reload(true, true);
    assert!(
        susie_loader::try_get_pool()
            .map(|p| p.supports_extension("pi") && p.supports_extension("mag"))
            .unwrap_or(false),
        "test precondition: Susie plugins for pi/mag must be loaded",
    );

    let tmp = tempfile::TempDir::new().unwrap();
    let zip_path = tmp.path().join("retro.zip");
    make_zip_with_entries(
        &zip_path,
        &[
            ("page01.pi", b"stub_pi_bytes"),
            ("page02.mag", b"stub_mag_bytes"),
            ("page03.jpg", b"stub_jpg_bytes"),
            ("notes.txt", b"skip"),
        ],
    );

    let entries = mimageviewer::zip_loader::enumerate_image_entries(&zip_path)
        .expect("enumerate_image_entries should succeed");
    let names: Vec<String> = entries.iter().map(|e| e.entry_name.clone()).collect();
    eprintln!("enumerated: {names:?}");
    assert_eq!(names.len(), 3, "expected 3 image entries, got {names:?}");
    assert!(
        names.contains(&"page01.pi".to_string()),
        "PI entry missing from ZIP enumeration",
    );
    assert!(
        names.contains(&"page02.mag".to_string()),
        "MAG entry missing from ZIP enumeration",
    );
    assert!(names.contains(&"page03.jpg".to_string()));
    assert!(!names.iter().any(|n| n.ends_with("notes.txt")));
}

/// 7z → ZIP 変換が `.pi` / `.mag` を画像として抽出する。
/// `archive_converter::is_image_entry` が Susie 対応拡張子も含めて画像扱いすることを確認。
#[test]
fn sevenz_convert_includes_susie_extensions() {
    setup_env();
    if !testdata_ok() {
        eprintln!("skip: missing worker exe or testdata");
        return;
    }
    susie_loader::reload(true, true);
    assert!(
        susie_loader::try_get_pool()
            .map(|p| p.supports_extension("pi") && p.supports_extension("mag"))
            .unwrap_or(false),
        "test precondition: Susie plugins for pi/mag must be loaded",
    );

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("retro.7z");
    let dst = tmp.path().join("retro_out.zip");
    make_7z_with_entries(
        &src,
        &[
            ("page01.pi", b"stub_pi_bytes"),
            ("page02.mag", b"stub_mag_bytes"),
            ("page03.jpg", b"stub_jpg_bytes"),
            ("notes.txt", b"skip"),
        ],
    );

    let summary = mimageviewer::archive_converter::scan_summary(
        &src,
        mimageviewer::archive_converter::ArchiveFormat::SevenZ,
    )
    .expect("scan_summary should succeed");
    assert_eq!(
        summary.image_count, 3,
        "scan should find PI + MAG + JPG, got {}",
        summary.image_count,
    );

    let cancel = std::sync::atomic::AtomicBool::new(false);
    let stats = mimageviewer::archive_converter::convert_to_zip(
        &src,
        &dst,
        mimageviewer::archive_converter::ArchiveFormat::SevenZ,
        &cancel,
        None,
    )
    .expect("convert_to_zip should succeed");
    assert_eq!(stats.image_count, 3);

    let file = std::fs::File::open(&dst).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(
        names.contains(&"page01.pi".to_string()),
        "PI not carried into converted ZIP: {names:?}",
    );
    assert!(
        names.contains(&"page02.mag".to_string()),
        "MAG not carried into converted ZIP: {names:?}",
    );
    assert!(!names.iter().any(|n| n.ends_with("notes.txt")));
}
