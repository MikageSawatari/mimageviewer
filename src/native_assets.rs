//! loose-deps ポータブルビルド (feature = "portable") 専用の native ファイル所在解決。
//!
//! ## 役割
//!
//! 通常ビルドは pdfium / onnxruntime / susie ワーカー / vst3-host / AI モデルを
//! `include_bytes!` で exe に埋め込み、初回利用時に `data_dir` (= `%APPDATA%/mimageviewer`)
//! へ展開して使う。ポータブルビルドでは **何も埋め込まず・展開せず**、これらの native
//! ファイルを exe と同じディレクトリ (bundled root) に loose 配置したものから直接解決する。
//!
//! このモジュールはポータブル側の所在解決プリミティブ (`bundled_root` / `bundled`) だけを
//! 提供する。各 `include_bytes!` const は呼び出し元モジュール (pdf_loader / ai::runtime /
//! susie_loader / video::dsp::extract / ai::model_manager) に残し `#[cfg(not(feature =
//! "portable"))]` でゲートする。これで:
//!
//! - 通常ビルドの展開ロジックは一切変えない → 出荷済み挙動への退行リスクがゼロ。
//! - ポータブル exe には ~300MB の native 依存が埋め込まれない → exe が激減・展開ゼロ。
//!
//! モジュール宣言自体が `#[cfg(feature = "portable")]` (main.rs) なので、通常ビルドでは
//! このファイルはコンパイルされない。`cargo check --features portable` (CI) が
//! ポータブル分岐の腐りを機械的に検出する。詳細: `docs/portable-build-plan.md`。

use std::path::PathBuf;

/// bundled native ファイルを探す基準ディレクトリ (= 実行中 exe と同じディレクトリ)。
///
/// `current_exe()` 失敗時は `.` にフォールバックする (実用上ほぼ起きない。万一起きても
/// 後続の存在チェックで明確なエラーになる)。設定・キャッシュ等の **書き込み先** は
/// `data_dir` (= `<exe_dir>/data`) を使うこと。本関数が指すのは **読み取り専用の同梱物**
/// (DLL / worker exe / モデル) のルート。
pub fn bundled_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// bundled native ファイル (`<exe_dir>/<name>`) のパスを返す。存在しなければ Err。
///
/// pdfium.dll / onnxruntime*.dll / vst3-host exe のように「無ければ機能が成立しない」
/// ファイル向け。エラー文面は「zip を完全展開したか」を案内する。
pub fn bundled(name: &str) -> Result<PathBuf, String> {
    let path = bundled_root().join(name);
    if !path.exists() {
        return Err(format!(
            "ポータブル版の同梱ファイルが見つかりません: {}\n\
             (zip を完全に展開し、exe と同じフォルダに DLL / モデルが揃っているか確認してください)",
            path.display()
        ));
    }
    Ok(path)
}
