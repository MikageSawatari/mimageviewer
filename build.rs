fn main() {
    // 先に vendor ファイルの存在チェックをして、cryptic な include_bytes! エラーに
    // 代わって復旧手順付きの明確なメッセージを出す。
    check_vendor_files();

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
/// AI モデル) が揃っているかをビルド前にチェックし、欠落時は復旧用セットアップ手順を含む
/// 明確なエラーメッセージで終了する。
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
