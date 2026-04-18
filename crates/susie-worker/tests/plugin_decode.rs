//! Susie プラグインの実ロード・実デコードを確認する統合テスト。
//!
//! ## 必要なデータ
//! 以下のいずれかの場所に testdata が配置されている必要がある:
//!   - 環境変数 `MIV_TESTDATA` が指すディレクトリ
//!   - `<workspace_root>/testdata/`
//!   - `C:\home\mimageviewer\testdata\` (固定パスフォールバック)
//!
//! testdata が無い環境では全テストが即 `return` で skip する (panic しない)。
//!
//! ## 実行方法
//! ```
//! cargo test --release --target i686-pc-windows-msvc -p mimageviewer-susie32
//! ```
//! ワーカーは 32bit でしか `.spi` をロードできないので 32bit ターゲット必須。

#![cfg(windows)]

use mimageviewer_susie32::plugin::PluginHost;
use std::path::PathBuf;

/// testdata ルートディレクトリを解決する。見つからなければ `None`。
fn find_testdata_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MIV_TESTDATA") {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    // <workspace_root>/testdata
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ws = manifest.join("..").join("..").join("testdata");
    if ws.is_dir() {
        return Some(ws);
    }
    // 固定パスフォールバック (開発者マシン想定)
    let fixed = PathBuf::from(r"C:\home\mimageviewer\testdata");
    if fixed.is_dir() {
        return Some(fixed);
    }
    None
}

/// 簡易 BMP ヘッダリーダー (BITMAPFILEHEADER + BITMAPINFOHEADER から幅・高さを取得)。
/// BI_RGB の 8bpp / 24bpp DIB にしか対応しないが、テストデータ (C165/C206.BMP) は範囲内。
fn read_bmp_dims(path: &std::path::Path) -> Option<(u32, u32)> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 26 || &data[0..2] != b"BM" {
        return None;
    }
    // BITMAPINFOHEADER は offset 14 から。width=18..22, height=22..26。
    let w = u32::from_le_bytes(data[18..22].try_into().ok()?);
    let h_raw = i32::from_le_bytes(data[22..26].try_into().ok()?);
    let h = h_raw.unsigned_abs();
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

/// プラグインフォルダ・画像フォルダ・BMP 参照フォルダへのパスをまとめて返す。
/// 取れなかったら `None` で skip。
fn setup() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let root = find_testdata_root()?;
    let plugins = root.join("susie-plugins").join("extracted");
    let retro = root.join("retro-images").join("formats");
    let bmp_dir = retro.join("bmp");
    if !plugins.is_dir() || !retro.is_dir() || !bmp_dir.is_dir() {
        eprintln!(
            "skip: testdata incomplete (plugins={}, retro={}, bmp={})",
            plugins.display(),
            retro.display(),
            bmp_dir.display()
        );
        return None;
    }
    Some((plugins, retro, bmp_dir))
}

/// PI プラグインと実画像で 1 枚デコード。BMP 参照と幅・高さが一致することを確認。
#[test]
fn decode_pi_c165() {
    let Some((plugin_dir, retro, bmp_dir)) = setup() else {
        return;
    };
    let img_path = retro.join("pi").join("C165.PI");
    let bmp_path = bmp_dir.join("C165.BMP");
    if !img_path.exists() || !bmp_path.exists() {
        eprintln!("skip: missing sample image");
        return;
    }

    let (exp_w, exp_h) = read_bmp_dims(&bmp_path)
        .expect("BMP header parse failed");
    eprintln!("reference BMP: {exp_w}x{exp_h}");

    let host = PluginHost::load_dir(&plugin_dir);
    eprintln!(
        "loaded {} plugins: {:?}",
        host.plugins.plugins.len(),
        host.plugins
            .plugins
            .iter()
            .map(|p| &p.name)
            .collect::<Vec<_>>()
    );
    assert!(
        !host.plugins.plugins.is_empty(),
        "no plugins loaded from {}",
        plugin_dir.display()
    );

    let (w, h, bgra) = host
        .plugins
        .decode_file(&img_path)
        .expect("PI decode failed");
    eprintln!("decoded PI: {w}x{h}, {} bytes", bgra.len());

    assert_eq!((w, h), (exp_w, exp_h), "PI dims mismatch vs BMP reference");
    assert_eq!(bgra.len() as u32, w * h * 4, "pixel buffer size mismatch");
    // 完全な黒/白べた塗りではないことを確認
    let all_same = bgra.chunks_exact(4).all(|p| p == &bgra[..4]);
    assert!(!all_same, "decoded image is all the same color (suspect)");
}

/// MAG プラグインでも同様にデコードできることを確認。
#[test]
fn decode_mag_c165() {
    let Some((plugin_dir, retro, bmp_dir)) = setup() else {
        return;
    };
    let img_path = retro.join("mag").join("C165.MAG");
    let bmp_path = bmp_dir.join("C165.BMP");
    if !img_path.exists() || !bmp_path.exists() {
        eprintln!("skip: missing sample image");
        return;
    }

    let (exp_w, exp_h) = read_bmp_dims(&bmp_path).expect("BMP header parse failed");
    let host = PluginHost::load_dir(&plugin_dir);
    assert!(!host.plugins.plugins.is_empty());

    let (w, h, bgra) = host
        .plugins
        .decode_file(&img_path)
        .expect("MAG decode failed");
    eprintln!("decoded MAG: {w}x{h}, {} bytes", bgra.len());

    assert_eq!((w, h), (exp_w, exp_h), "MAG dims mismatch vs BMP reference");
    assert_eq!(bgra.len() as u32, w * h * 4);
    let all_same = bgra.chunks_exact(4).all(|p| p == &bgra[..4]);
    assert!(!all_same);
}

/// 2 枚目 (C206) でも PI / MAG の両方でデコードが通ること。
#[test]
fn decode_c206_pi_and_mag() {
    let Some((plugin_dir, retro, bmp_dir)) = setup() else {
        return;
    };
    let bmp_path = bmp_dir.join("C206.BMP");
    let pi_path = retro.join("pi").join("C206.PI");
    let mag_path = retro.join("mag").join("C206.MAG");
    if !bmp_path.exists() || !pi_path.exists() || !mag_path.exists() {
        eprintln!("skip: missing C206 sample");
        return;
    }
    let (exp_w, exp_h) = read_bmp_dims(&bmp_path).expect("BMP header parse failed");

    let host = PluginHost::load_dir(&plugin_dir);
    assert!(!host.plugins.plugins.is_empty());

    let (pw, ph, _) = host
        .plugins
        .decode_file(&pi_path)
        .expect("C206.PI decode failed");
    assert_eq!((pw, ph), (exp_w, exp_h));

    let (mw, mh, _) = host
        .plugins
        .decode_file(&mag_path)
        .expect("C206.MAG decode failed");
    assert_eq!((mw, mh), (exp_w, exp_h));
}

/// プラグインが対応拡張子を正しく報告することを確認 (スモーク)。
#[test]
fn plugins_report_extensions() {
    let Some((plugin_dir, _, _)) = setup() else {
        return;
    };
    let host = PluginHost::load_dir(&plugin_dir);
    let mut found_pi = false;
    let mut found_mag = false;
    for pi in &host.plugins.plugins {
        for e in &pi.extensions {
            if e == "pi" {
                found_pi = true;
            }
            if e == "mag" {
                found_mag = true;
            }
        }
    }
    assert!(found_pi, "no plugin reported 'pi' extension");
    assert!(found_mag, "no plugin reported 'mag' extension");
}
