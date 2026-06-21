fn main() {
    // 先に vendor ファイルの存在チェックをして、cryptic な include_bytes! エラーに
    // 代わって復旧手順付きの明確なメッセージを出す。
    check_vendor_files();

    // 絵文字スタンプ (Twemoji SVG) を exe に同梱するためのコード生成 (Inc 4c)。
    generate_emoji_assets();

    // FFmpeg DLL は `target/release/` には自動でコピーしない。
    //
    // 本体 (`mimageviewer-core.exe`) は配布物としては直接使われず、ランチャー
    // (`crates/launcher/`、`mimageviewer.exe` を生成) が `include_bytes!` で
    // core と FFmpeg DLL 6 つ (avcodec / avformat / avutil / avfilter / swscale /
    // swresample) を内包し、起動時に `%APPDATA%\mimageviewer\runtime\<v>\`
    // に展開して core を spawn する。
    //
    // 開発時に直接 `target/release/mimageviewer-core.exe` を実行したいときは、
    // `vendor/ffmpeg/bin/*.dll` を手動で同じディレクトリにコピーすること。
    // (もしくは PowerShell で:
    //   `Copy-Item vendor/ffmpeg/bin/*.dll target/release/`)
    //
    // 経緯: 当初 `include_bytes!` → APPDATA 展開で本体に直接埋め込もうとしたが、
    // `ffmpeg-the-third` は MSVC import library 経由なので Windows ローダが
    // exe ロード時点 (Rust コードが走るより前) に DLL を解決する必要があり間に
    // 合わない。`/DELAYLOAD` も rustc 経由の link.exe で Delay Import Directory が
    // 空のまま生成される問題があり機能せず、最終的にランチャー方式に切り替えた。

    #[cfg(target_os = "windows")]
    {
        // VS_FIXEDFILEINFO は winresource が CARGO_PKG_VERSION_{MAJOR,MINOR,PATCH}
        // から自動で 0.9.0.0 を埋める。文字列版 (ファイルプロパティに表示される) も
        // 揃えるため CARGO_PKG_VERSION から導出する (以前は "0.1.0.0" 決め打ちで
        // クラッシュダンプ / サポート画面で「v0.1.0.0」と表示されていた)。
        let version_str = format!("{}.0", env!("CARGO_PKG_VERSION"));
        let mut res = winresource::WindowsResource::new();
        res.set("FileDescription", "mImageViewer");
        res.set("ProductName", "mImageViewer");
        res.set("FileVersion", &version_str);
        res.set("ProductVersion", &version_str);
        res.set("LegalCopyright", "Copyright (C) 2026 Mikage Sawatari");
        // 本バイナリは内部実体 `mimageviewer-core.exe` であり、配布する
        // `mimageviewer.exe` は別途 launcher が生成する。OriginalFilename は
        // PE リソースのファイル名識別なので core 側はそれに合わせる。
        res.set("OriginalFilename", "mimageviewer-core.exe");

        // アイコンファイルが存在する場合のみ埋め込む
        if std::path::Path::new("assets/icon.ico").exists() {
            res.set_icon("assets/icon.ico");
        }

        if let Err(e) = res.compile() {
            eprintln!("winresource compile error: {e}");
        }
    }
}

/// `include_bytes!` で埋め込む vendor ファイル (PDFium / ONNX Runtime / Susie 32bit ワーカー /
/// FFmpeg LGPL DLL / AI モデル) が揃っているかをビルド前にチェックし、欠落時は復旧用
/// セットアップ手順を含む明確なエラーメッセージで終了する。
fn check_vendor_files() {
    // (path, setup script / 取得方法)
    let mut required: Vec<(&str, &str)> = vec![
        (
            "vendor/pdfium/bin/pdfium.dll",
            "bash scripts/setup-pdfium.sh",
        ),
        ("vendor/ort/onnxruntime.dll", "bash scripts/setup-ort.sh"),
        (
            "vendor/ort/onnxruntime_providers_shared.dll",
            "bash scripts/setup-ort.sh",
        ),
        (
            "vendor/susie-worker/mimageviewer-susie32.exe",
            "bash scripts/setup-susie-worker.sh",
        ),
        // FFmpeg 7.x LGPL shared build (BtbN ffmpeg-n7.1*-win64-lgpl-shared)
        // バージョンを上げる (8.x 等) と DLL のメジャー番号が変わるため、ここと
        // src/video/ffmpeg_loader.rs の include_bytes! パスを揃えて更新すること。
        (
            "vendor/ffmpeg/bin/avcodec-61.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/avformat-61.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/avutil-59.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/avfilter-10.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/swscale-8.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/swresample-5.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/avcodec.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/avformat.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/avutil.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/avfilter.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/swscale.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/swresample.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        // AI モデルは配布スクリプトが無いので、既存インストール済み環境からコピーする旨を案内。
        // ここに並べるのは `src/ai/model_manager.rs` の EMBEDDED_MODELS と
        // `scripts/build-portable.ps1` の $models に一致させること
        // (= 旧 anime_classifier_mobilenetv3.onnx は現在どちらからも外れているので要求しない)。
        (
            "vendor/models/realesrgan_x4plus.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/realesrgan_x4plus_anime_6b.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/realesr_general_x4v3.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/realcugan_4x_conservative.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/4x_NMKD-Siax_200k.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/dejpg_realplksr_otf.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/migan.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
    ];

    // VST3 host bridge プロセスは **通常ビルド (インストーラ / 単体 exe) のみ**
    // `include_bytes!` で本体 (`mimageviewer-core.exe`) に内包する。CMake で
    // `crates/vst3-host/` をビルドするとここに配置され、初回 VST3 enable 時に
    // `%APPDATA%\mimageviewer\vst3\` へ展開する (PDFium / Susie ワーカーと同パターン)。
    //
    // ポータブルビルド (feature = "portable") では、未署名 exe が一部のセキュリティ
    // ソフトに誤検知され zip ダウンロードがブロックされる事象を避けるため **同梱しない**
    // (`scripts/build-portable.ps1` も copy 対象外、`vst3_supported()` が false を返して
    // VST3 機能を自動無効化する)。よって portable では必須ファイルにしない。
    // 将来 exe をコード署名して再同梱する場合は build-portable.ps1 と合わせて復活させる。
    if std::env::var_os("CARGO_FEATURE_PORTABLE").is_none() {
        required.push((
            "vendor/vst3-host/mimageviewer-vst3-host.exe",
            "cmake -S crates/vst3-host -B crates/vst3-host/build -G \"Visual Studio 18 2026\" -A x64 && cmake --build crates/vst3-host/build --config Release",
        ));
    }

    let missing: Vec<&(&str, &str)> = required
        .iter()
        .filter(|(p, _)| !std::path::Path::new(p).exists())
        .collect();

    if missing.is_empty() {
        // 変更されたら再チェックするよう cargo に通知
        for (p, _) in &required {
            println!("cargo:rerun-if-changed={p}");
        }
        return;
    }

    eprintln!();
    eprintln!("================================================================");
    eprintln!(" vendor ファイルが不足しています (include_bytes! で必要)");
    eprintln!("================================================================");
    for (p, how) in &missing {
        eprintln!("  - {p}");
        eprintln!("      → {how}");
    }
    eprintln!();
    eprintln!("不足分をまとめて取得する場合:");
    eprintln!("  bash scripts/bootstrap-vendor.sh");
    eprintln!();
    eprintln!("(vst3-host exe は SDK が ~490 MB なので bootstrap には含まれない。");
    eprintln!(" 別 worktree やバックアップにビルド済み exe があればコピー、");
    eprintln!(" 無ければ CLAUDE.md「VST3 host bridge 管理」節の cmake 手順を実行)");
    eprintln!();
    eprintln!("詳細は CLAUDE.md の「vendor/ 一括セットアップ」節を参照してください。");
    eprintln!("================================================================");
    eprintln!();
    std::process::exit(1);
}

/// `vendor/twemoji/svg/*.svg` をスキャンし、ファイル名 (= emoji キー) → SVG バイト列の
/// 配列を `$OUT_DIR/emoji_svgs.rs` へ生成する。`src/comic_stamp.rs` が `include!` で
/// 取り込み、`include_bytes!` で各 SVG を exe に埋め込む (絵文字スタンプの同梱、Inc 4c)。
///
/// アセット未配置 (fresh clone で `scripts/setup-twemoji.sh` 未実行) の場合は空配列を
/// 生成し、`cargo:warning` で取得方法を案内する。dev ビルドはブロックしない
/// (= スタンプはアセット導入後に有効化される)。
fn generate_emoji_assets() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = std::path::Path::new(&out_dir).join("emoji_svgs.rs");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let svg_dir = std::path::Path::new(&manifest)
        .join("vendor")
        .join("twemoji")
        .join("svg");
    println!("cargo:rerun-if-changed=vendor/twemoji/svg");

    // (key, abs_path) を収集。キーは Twemoji 命名 (小文字 16 進 + '-') のみ許可。
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&svg_dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|s| s.to_str()) != Some("svg") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                continue;
            }
            // svg_dir は manifest 由来の絶対パスなので verbatim (\\?\) にはならない。
            // include_bytes! は Windows でも forward slash を受けるので '/' に正規化する。
            let abs = path.to_string_lossy().replace('\\', "/");
            entries.push((stem.to_string(), abs));
        }
    }
    entries.sort();

    let mut src = String::new();
    src.push_str("// @generated by build.rs from vendor/twemoji/svg/*.svg — do not edit.\n");
    src.push_str("pub static EMOJI_SVGS: &[(&str, &[u8])] = &[\n");
    for (key, abs) in &entries {
        src.push_str(&format!("    ({key:?}, include_bytes!({abs:?})),\n"));
    }
    src.push_str("];\n");
    std::fs::write(&dest, &src).expect("write emoji_svgs.rs");

    if entries.is_empty() {
        println!(
            "cargo:warning=絵文字スタンプ用アセットが未配置です (vendor/twemoji/svg)。\
             `bash scripts/setup-twemoji.sh` で取得するとスタンプが exe に同梱されます。"
        );
    }
}
