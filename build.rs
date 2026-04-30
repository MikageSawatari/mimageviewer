fn main() {
    // 先に vendor ファイルの存在チェックをして、cryptic な include_bytes! エラーに
    // 代わって復旧手順付きの明確なメッセージを出す。
    check_vendor_files();

    // FFmpeg DLL は `target/release/` には自動でコピーしない。
    //
    // 本体 (`mimageviewer-core.exe`) は配布物としては直接使われず、ランチャー
    // (`crates/launcher/`、`mimageviewer.exe` を生成) が `include_bytes!` で
    // core と FFmpeg DLL 5 つを内包し、起動時に `%APPDATA%\mimageviewer\runtime\<v>\`
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
        let mut res = winresource::WindowsResource::new();
        res.set("FileDescription", "mImageViewer");
        res.set("ProductName", "mImageViewer");
        res.set("FileVersion", "0.1.0.0");
        res.set("ProductVersion", "0.1.0.0");
        res.set("LegalCopyright", "Copyright (C) 2025 Mikage Sawatari");
        res.set("OriginalFilename", "mimageviewer.exe");

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
    let required: &[(&str, &str)] = &[
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
            "vendor/ffmpeg/lib/swscale.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/swresample.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        // AI モデルは配布スクリプトが無いので、既存インストール済み環境からコピーする旨を案内
        (
            "vendor/models/anime_classifier_mobilenetv3.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
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
        // VST3 host bridge プロセス。CMake で `crates/vst3-host/` をビルドすると
        // ここに配置される。本体 (`mimageviewer-core.exe`) に `include_bytes!` で
        // 内包し、初回 VST3 enable 時に `%APPDATA%\mimageviewer\vst3\` へ展開する
        // (PDFium / Susie ワーカーと同パターン)。
        (
            "vendor/vst3-host/mimageviewer-vst3-host.exe",
            "cmake -S crates/vst3-host -B crates/vst3-host/build -G \"Visual Studio 17 2022\" -A x64 && cmake --build crates/vst3-host/build --config Release",
        ),
    ];

    let missing: Vec<&(&str, &str)> = required
        .iter()
        .filter(|(p, _)| !std::path::Path::new(p).exists())
        .collect();

    if missing.is_empty() {
        // 変更されたら再チェックするよう cargo に通知
        for (p, _) in required {
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
    eprintln!("詳細は CLAUDE.md の vendor/ セクションを参照してください。");
    eprintln!("================================================================");
    eprintln!();
    std::process::exit(1);
}
