fn main() {
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
