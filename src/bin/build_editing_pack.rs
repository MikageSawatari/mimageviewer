//! 編集用追加パック (オノマトペ向け OFL フォント + 被写体分離 ONNX モデル) の
//! GitHub Releases 用アセット作成ツール。
//!
//! `build_trt_pack.rs` (TensorRT pack ビルダ) の編集パック版。アプリ側 consumer
//! ([`mimageviewer::editing_addon`] / [`mimageviewer::editing_addon_download`]) と
//! **同じ serde 型** ([`PackManifest`] / [`PackIndex`]) を使って manifest / index を
//! 生成するので、フォーマットの食い違いが起きない。
//!
//! ## 入力 (デフォルト)
//!
//! - `tools/comic_lab/assets/fonts/` : `*.ttf` (= フォント本体) + `*-OFL.txt` (= ライセンス文面)
//! - `vendor/editing-pack/models/`   : `*.onnx` (= 被写体分離モデル) + `*LICENSE*` (= ライセンス文面)
//!   (任意。無ければフォントのみの pack を作る。BiRefNet 検証完了後にここへ置く)
//!
//! `--fonts <dir>` / `--models <dir>` で上書き可。
//!
//! ## 出力 (`dist/editing-pack-<version>/`)
//!
//! - `editing-pack-<version>.zip` : 配布する pack 本体。中身は
//!     pack-manifest.json (zip root) + fonts/* + models/* + 各ライセンス txt
//! - `editing-pack-index.json`    : この pack 1 個を載せた配布一覧。GitHub Releases
//!     のタグ `editing-pack-v1` に zip と一緒にアップロードする。
//!
//! ## 使い方
//!
//! ```sh
//! cargo run --release --bin build_editing_pack
//! # オプション:
//! #   --fonts <dir>            フォント素材ディレクトリ
//! #   --models <dir>           モデル素材ディレクトリ (任意)
//! #   --out <dir>              出力ディレクトリ
//! #   --pack-version <ver>     pack バージョン (= packs/<ver>/ のディレクトリ名)
//! #   --app-min-version <ver>  この pack を導入できる mIV 最小バージョン
//! #   --subject-model-name <s> index に載せる被写体分離モデルの表示名
//! ```
//!
//! 生成後は `editing-pack-<version>.zip` と `editing-pack-index.json` を GitHub
//! Releases (タグ `editing-pack-v1`) にアップロードすれば、本体の DL フローが動く。
//! ライセンス検証は `editing_addon_download` 側の per-file sha256 が担う。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use mimageviewer::editing_addon::{FileKind, IndexEntry, PackFile, PackIndex, PackManifest};

/// pack バージョン (= ディスク上の `packs/<version>/` ディレクトリ名 & active pointer 値)。
/// pack の中身 (フォント追加 / モデル差し替え) を変えたら bump する。
const DEFAULT_PACK_VERSION: &str = "2026.06.0";

/// この pack を導入できる mIV 最小バージョン。編集 (comic) 機能が出荷される版に揃える。
/// `pick_pack` は `app_min_version <= 現在の mIV` の pack だけを候補にする。
const DEFAULT_APP_MIN_VERSION: &str = "1.1.0";

/// pack 識別子 (manifest の `pack_id`)。
const PACK_ID: &str = "editing-base";

/// 被写体分離モデルのライセンス SPDX id (BiRefNet は MIT)。
const MODEL_LICENSE: &str = "MIT";

/// フォントのライセンス SPDX id (Google Fonts 系 OFL)。
const FONT_LICENSE: &str = "OFL-1.1";

struct Args {
    fonts_dir: PathBuf,
    models_dir: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    pack_version: String,
    app_min_version: String,
    subject_model_name: Option<String>,
    zip_name: Option<String>,
}

fn parse_args() -> Args {
    let mut fonts_dir = PathBuf::from("tools/comic_lab/assets/fonts");
    let mut models_dir: Option<PathBuf> = None;
    let mut models_dir_explicit = false;
    let mut out_dir: Option<PathBuf> = None;
    let mut pack_version = DEFAULT_PACK_VERSION.to_string();
    let mut app_min_version = DEFAULT_APP_MIN_VERSION.to_string();
    let mut subject_model_name: Option<String> = None;
    let mut zip_name: Option<String> = None;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--fonts" => fonts_dir = PathBuf::from(expect_val(&mut it, "--fonts")),
            "--models" => {
                models_dir = Some(PathBuf::from(expect_val(&mut it, "--models")));
                models_dir_explicit = true;
            }
            "--out" => out_dir = Some(PathBuf::from(expect_val(&mut it, "--out"))),
            "--pack-version" => pack_version = expect_val(&mut it, "--pack-version"),
            "--app-min-version" => app_min_version = expect_val(&mut it, "--app-min-version"),
            "--subject-model-name" => {
                subject_model_name = Some(expect_val(&mut it, "--subject-model-name"))
            }
            "--zip-name" => zip_name = Some(expect_val(&mut it, "--zip-name")),
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                eprintln!("ERROR: 未知の引数: {other}\n");
                print_help();
                std::process::exit(2);
            }
        }
    }

    // models が明示されず default も存在しないなら None (= フォントのみ pack)。
    if !models_dir_explicit {
        let default_models = PathBuf::from("vendor/editing-pack/models");
        if default_models.is_dir() {
            models_dir = Some(default_models);
        }
    }

    Args {
        fonts_dir,
        models_dir,
        out_dir,
        pack_version,
        app_min_version,
        subject_model_name,
        zip_name,
    }
}

fn expect_val(it: &mut impl Iterator<Item = String>, flag: &str) -> String {
    it.next().unwrap_or_else(|| {
        eprintln!("ERROR: {flag} に値がありません");
        std::process::exit(2);
    })
}

fn print_help() {
    eprintln!(
        "build_editing_pack - 編集用追加パックの GitHub Releases アセットを生成\n\n\
         オプション:\n\
         \x20 --fonts <dir>            フォント素材 (default: tools/comic_lab/assets/fonts)\n\
         \x20 --models <dir>           モデル素材 (任意, default: vendor/editing-pack/models if exists)\n\
         \x20 --out <dir>              出力先 (default: dist/editing-pack-<version>)\n\
         \x20 --pack-version <ver>     pack バージョン (default: {DEFAULT_PACK_VERSION})\n\
         \x20 --app-min-version <ver>  必要 mIV 最小版 (default: {DEFAULT_APP_MIN_VERSION})\n\
         \x20 --subject-model-name <s> index 表示用モデル名 (default: model_id から導出)\n\
         \x20 --zip-name <name>        zip ファイル名 (default: editing-pack-<version>.zip)"
    );
}

fn main() {
    let args = parse_args();

    if !args.fonts_dir.is_dir() {
        eprintln!(
            "ERROR: フォント素材ディレクトリがありません: {}",
            args.fonts_dir.display()
        );
        std::process::exit(1);
    }

    let zip_name = args
        .zip_name
        .clone()
        .unwrap_or_else(|| format!("editing-pack-{}.zip", args.pack_version));
    let out_dir = args
        .out_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("dist/editing-pack-{}", args.pack_version)));

    println!("[build_editing_pack] pack version: {}", args.pack_version);
    println!(
        "[build_editing_pack] app_min_version: {}",
        args.app_min_version
    );
    println!("[build_editing_pack] fonts: {}", args.fonts_dir.display());
    match &args.models_dir {
        Some(d) => println!("[build_editing_pack] models: {}", d.display()),
        None => println!("[build_editing_pack] models: (なし — フォントのみの pack を作成します)"),
    }

    // ── pack に入れるファイルを集める ──
    // (相対パス, 絶対パス, FileKind, license, model_id) のタプルで保持する。
    let mut staged: Vec<StagedFile> = Vec::new();

    // フォント素材: *.ttf / *.otf → Font, *-OFL.txt (= *OFL*) → License。
    gather_fonts(&args.fonts_dir, &mut staged);

    // モデル素材 (任意): *.onnx → SubjectMatteModel, *LICENSE* → License。
    if let Some(models) = &args.models_dir {
        gather_models(models, &mut staged);
    }

    let font_count = staged.iter().filter(|f| f.kind == FileKind::Font).count() as u32;
    let model_count = staged
        .iter()
        .filter(|f| f.kind == FileKind::SubjectMatteModel)
        .count();
    if font_count == 0 {
        eprintln!("ERROR: フォント (.ttf/.otf) が 1 つも見つかりません");
        std::process::exit(1);
    }
    println!(
        "[build_editing_pack] 収集: フォント {font_count} 書体 / モデル {model_count} 個 / \
         合計 {} ファイル",
        staged.len()
    );

    // ── per-file sha256 + サイズ → PackFile ──
    let mut files: Vec<PackFile> = Vec::with_capacity(staged.len());
    let mut uncompressed_bytes: u64 = 0;
    for sf in &staged {
        let (sha, bytes) = sha256_and_size(&sf.abs).unwrap_or_else(|e| {
            eprintln!("ERROR: hash {}: {e}", sf.abs.display());
            std::process::exit(1);
        });
        uncompressed_bytes += bytes;
        files.push(PackFile {
            path: sf.rel.clone(),
            kind: sf.kind,
            license: sf.license.clone(),
            sha256: sha,
            bytes,
            model_id: sf.model_id.clone(),
        });
    }
    // 安定した並び (= 再現性のある zip / manifest)。
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let manifest = PackManifest {
        schema: 1,
        pack_id: PACK_ID.to_string(),
        version: args.pack_version.clone(),
        app_min_version: args.app_min_version.clone(),
        files,
    };

    // ── 出力ディレクトリ準備 ──
    if out_dir.exists() {
        println!(
            "[build_editing_pack] 既存 {} を削除して作り直す",
            out_dir.display()
        );
        if let Err(e) = fs::remove_dir_all(&out_dir) {
            eprintln!("ERROR: remove_dir_all {}: {e}", out_dir.display());
            std::process::exit(1);
        }
    }
    fs::create_dir_all(&out_dir).unwrap();

    // ── zip を作る (pack-manifest.json + 全 staged ファイル) ──
    let zip_path = out_dir.join(&zip_name);
    let manifest_json = serde_json::to_string_pretty(&manifest).unwrap();
    build_pack_zip(&zip_path, &manifest_json, &staged).unwrap_or_else(|e| {
        eprintln!("ERROR: build zip: {e}");
        std::process::exit(1);
    });

    // ── zip 全体の sha256 + サイズ ──
    let (zip_sha256, zip_bytes) = sha256_and_size(&zip_path).unwrap_or_else(|e| {
        eprintln!("ERROR: hash zip: {e}");
        std::process::exit(1);
    });

    // ── 被写体分離モデルの表示名 (index 用) ──
    let subject_model = args.subject_model_name.clone().unwrap_or_else(|| {
        manifest
            .subject_matte_model()
            .and_then(|f| f.model_id.clone())
            .unwrap_or_default()
    });

    // ── index を作る (この pack 1 個) ──
    let index = PackIndex {
        schema: 1,
        packs: vec![IndexEntry {
            version: args.pack_version.clone(),
            app_min_version: args.app_min_version.clone(),
            zip_name: zip_name.clone(),
            zip_sha256: zip_sha256.clone(),
            zip_bytes,
            uncompressed_bytes,
            font_count,
            subject_model: subject_model.clone(),
        }],
    };
    let index_path = out_dir.join("editing-pack-index.json");
    let index_json = serde_json::to_string_pretty(&index).unwrap();
    fs::write(&index_path, &index_json).unwrap();

    // ── サマリ ──
    println!();
    println!("==================== 完了 ====================");
    println!("出力ディレクトリ: {}", out_dir.display());
    println!(
        "zip:   {} ({:.1} MiB, sha256={}…)",
        zip_path.display(),
        zip_bytes as f64 / 1024.0 / 1024.0,
        &zip_sha256[..12]
    );
    println!("index: {}", index_path.display());
    println!(
        "展開後サイズ: {:.1} MiB / フォント {font_count} 書体 / モデル {model_count} 個",
        uncompressed_bytes as f64 / 1024.0 / 1024.0
    );
    if !subject_model.is_empty() {
        println!("被写体分離モデル表示名: {subject_model}");
    }
    println!();
    println!("次のステップ:");
    println!(
        "  1. {} と {} を GitHub Releases (タグ: editing-pack-v1) にアップロード",
        zip_name, "editing-pack-index.json"
    );
    println!(
        "     (アップロード URL は editing_addon_download.rs の DEFAULT_PACK_BASE_URL と一致させる)"
    );
    println!("  2. 本体を起動してテキスト編集に入り、DL フローが通ることを確認");
}

/// staged されるファイル 1 個 (zip に入れる相対パス + 種別)。
struct StagedFile {
    /// pack ディレクトリからの相対パス (例: "fonts/OtomanopeeOne-Regular.ttf")。
    rel: String,
    /// ソースの絶対パス。
    abs: PathBuf,
    kind: FileKind,
    license: String,
    model_id: Option<String>,
}

/// フォント素材を収集する。`*.ttf`/`*.otf` は Font、`*OFL*` (= ライセンス文面) は License。
/// それ以外 (README 等) は無視する。
fn gather_fonts(dir: &Path, out: &mut Vec<StagedFile>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: read_dir {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !ft.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        let kind = if lower.ends_with(".ttf") || lower.ends_with(".otf") {
            FileKind::Font
        } else if lower.contains("ofl") && lower.ends_with(".txt") {
            FileKind::License
        } else {
            continue; // README.md 等は pack に入れない
        };
        out.push(StagedFile {
            rel: format!("fonts/{name}"),
            abs: entry.path(),
            kind,
            license: FONT_LICENSE.to_string(),
            model_id: None,
        });
    }
}

/// モデル素材を収集する。`*.onnx` は SubjectMatteModel、`*LICENSE*`/`*license*` は License。
fn gather_models(dir: &Path, out: &mut Vec<StagedFile>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ERROR: read_dir {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !ft.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        let (kind, model_id) = if lower.ends_with(".onnx") {
            // model_id = 拡張子を除いたファイル名 (例: birefnet_fp16)。
            let stem = name.trim_end_matches(|c| c != '.').trim_end_matches('.');
            (FileKind::SubjectMatteModel, Some(stem.to_string()))
        } else if lower.contains("license") && lower.ends_with(".txt") {
            (FileKind::License, None)
        } else {
            continue;
        };
        out.push(StagedFile {
            rel: format!("models/{name}"),
            abs: entry.path(),
            kind,
            license: MODEL_LICENSE.to_string(),
            model_id,
        });
    }
}

/// pack zip を作る。`pack-manifest.json` を root に書き、続けて全 staged ファイルを
/// `<rel>` のパスで格納する。
fn build_pack_zip(
    zip_path: &Path,
    manifest_json: &str,
    staged: &[StagedFile],
) -> Result<(), String> {
    let file = fs::File::create(zip_path).map_err(|e| format!("create zip: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(6))
        .large_file(true);

    // pack-manifest.json を最初に書く。
    zip.start_file("pack-manifest.json", opts)
        .map_err(|e| format!("start pack-manifest.json: {e}"))?;
    zip.write_all(manifest_json.as_bytes())
        .map_err(|e| format!("write pack-manifest.json: {e}"))?;

    // 各データファイル。zip 内パスは manifest の path と一致させる。
    for sf in staged {
        zip.start_file(&sf.rel, opts)
            .map_err(|e| format!("start {}: {e}", sf.rel))?;
        let mut src =
            fs::File::open(&sf.abs).map_err(|e| format!("open {}: {e}", sf.abs.display()))?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = src
                .read(&mut buf)
                .map_err(|e| format!("read {}: {e}", sf.abs.display()))?;
            if n == 0 {
                break;
            }
            zip.write_all(&buf[..n])
                .map_err(|e| format!("zip write {}: {e}", sf.rel))?;
        }
    }
    zip.finish().map_err(|e| format!("zip finish: {e}"))?;
    Ok(())
}

/// SHA-256 (hex 小文字) + バイト数を計算する。
fn sha256_and_size(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut bytes: u64 = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes += n as u64;
    }
    Ok((hex_encode(&hasher.finalize()), bytes))
}

/// バイト列を hex 小文字に変換 (依存追加せず手書き)。
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encode_basic() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0xab]), "00ffab");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn gather_fonts_classifies() {
        let base = std::env::temp_dir().join("miv_build_editing_pack_fonts_t1");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("Foo-Regular.ttf"), b"ttf").unwrap();
        fs::write(base.join("Foo-OFL.txt"), b"ofl").unwrap();
        fs::write(base.join("README.md"), b"readme").unwrap();
        let mut out = Vec::new();
        gather_fonts(&base, &mut out);
        // README.md は除外、ttf + OFL の 2 件。
        assert_eq!(out.len(), 2);
        assert!(
            out.iter()
                .any(|f| f.kind == FileKind::Font && f.rel == "fonts/Foo-Regular.ttf")
        );
        assert!(
            out.iter()
                .any(|f| f.kind == FileKind::License && f.rel == "fonts/Foo-OFL.txt")
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn gather_models_classifies() {
        let base = std::env::temp_dir().join("miv_build_editing_pack_models_t1");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("birefnet_fp16.onnx"), b"onnx").unwrap();
        fs::write(base.join("BiRefNet-LICENSE.txt"), b"mit").unwrap();
        let mut out = Vec::new();
        gather_models(&base, &mut out);
        assert_eq!(out.len(), 2);
        let model = out
            .iter()
            .find(|f| f.kind == FileKind::SubjectMatteModel)
            .expect("model");
        assert_eq!(model.rel, "models/birefnet_fp16.onnx");
        assert_eq!(model.model_id.as_deref(), Some("birefnet_fp16"));
        assert!(out.iter().any(|f| f.kind == FileKind::License));
        let _ = fs::remove_dir_all(&base);
    }
}
