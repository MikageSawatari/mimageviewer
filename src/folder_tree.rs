//! ファイルシステム上のフォルダツリー走査ヘルパー。
//!
//! - サポート画像/動画拡張子の定数
//! - フォルダ内に画像があるかの判定
//! - Ctrl+↑/↓ 用の深さ優先前順 DFS (next/prev)
//! - キャッシュ作成用の再帰サブフォルダ列挙
//!
//! ZIP ファイルもフォルダの一種としてナビゲーション対象に含める (タスク 3)。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

// -----------------------------------------------------------------------
// サポート拡張子
// -----------------------------------------------------------------------

/// 標準サポートする画像拡張子。
///
/// 前半は `image` クレートで直接デコードできる形式。
/// 後半は WIC (Windows Imaging Component) でデコードする形式で、
/// 対応コーデックが Microsoft Store からインストールされている必要がある:
/// - heic/heif → HEIF Image Extensions
/// - avif      → AV1 Video Extensions
/// - jxl       → JPEG XL Image Extensions
/// - cr2/nef/arw 等 → Raw Image Extension
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    // image クレートで直接デコード
    "jpg", "jpeg", "png", "webp", "bmp", "gif",
    // WIC 経由 (モダン形式)
    "heic", "heif", "avif", "jxl",
    // WIC 経由 (TIFF: image クレートも対応するが WIC の方が高機能)
    "tiff", "tif",
    // WIC 経由 (カメラ RAW)
    "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2",
    "raf", "orf", "rw2", "pef", "ptx", "rwl", "iiq",
];
pub const SUPPORTED_VIDEO_EXTENSIONS: &[&str] =
    &["mpg", "mpeg", "mp4", "avi", "mov", "mkv", "wmv"];

// -----------------------------------------------------------------------
// 画像有無の判定
// -----------------------------------------------------------------------

/// フォルダ内に対応画像ファイルが1枚以上あるか確認する。
/// path が .zip ファイルの場合は ZIP 内の画像エントリを確認する (タスク 3)。
pub fn folder_has_images(path: &Path) -> bool {
    // .zip ファイルなら ZIP 内画像エントリを確認
    if path.is_file() {
        let is_zip = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("zip"))
            .unwrap_or(false);
        if is_zip {
            return crate::zip_loader::enumerate_image_entries(path)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        }
        return false;
    }

    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
                .unwrap_or(false)
        })
}

// -----------------------------------------------------------------------
// フォルダツリー走査（深さ優先・前順）
// -----------------------------------------------------------------------

/// Ctrl+↑↓ フォルダ移動：画像なしフォルダを最大 skip_limit 回スキップする。
/// skip_limit 回以内に画像ありフォルダが見つかればそこへ移動。
/// 見つからなければ直近の隣フォルダ（1ステップ先）へ移動。
pub fn navigate_folder_with_skip<F>(
    start: &Path,
    nav_fn: F,
    skip_limit: usize,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> Option<PathBuf>,
{
    let first = nav_fn(start)?;
    let mut candidate = first.clone();
    for _ in 0..skip_limit {
        if folder_has_images(&candidate) {
            return Some(candidate);
        }
        match nav_fn(&candidate) {
            Some(next) => candidate = next,
            None => return Some(first),
        }
    }
    // skip_limit 回分全て画像なし → 直近の隣フォルダにフォールバック
    Some(first)
}

/// 深さ優先前順で次のフォルダを返す。
/// 子があれば最初の子、なければ次の兄弟、なければ祖先の次の兄弟。
pub fn next_folder_dfs(current: &Path) -> Option<PathBuf> {
    // 1. 子フォルダがあれば最初の子へ
    if let Some(first_child) = sorted_subdirs(current).into_iter().next() {
        return Some(first_child);
    }
    // 2. 子がなければ、次の兄弟または祖先の次の兄弟を探す
    next_sibling_or_ancestor_sibling(current)
}

/// 深さ優先前順で前のフォルダを返す。
/// 前の兄弟がいればその最後の子孫、最初の子であれば親。
pub fn prev_folder_dfs(current: &Path) -> Option<PathBuf> {
    let parent = current.parent()?;
    let siblings = sorted_subdirs(parent);
    let pos = siblings.iter().position(|s| path_eq(s, current))?;

    if pos == 0 {
        // 最初の子 → 親へ
        Some(parent.to_path_buf())
    } else {
        // 前の兄弟の最後の子孫へ
        Some(last_descendant_dir(&siblings[pos - 1]))
    }
}

/// path の次の兄弟を返す。兄弟がなければ親で再帰する。
fn next_sibling_or_ancestor_sibling(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let siblings = sorted_subdirs(parent);
    let pos = siblings.iter().position(|s| path_eq(s, path))?;

    if pos + 1 < siblings.len() {
        Some(siblings[pos + 1].clone())
    } else {
        next_sibling_or_ancestor_sibling(parent)
    }
}

/// path の最も深い最後の子孫フォルダを返す（子がなければ path 自身）。
fn last_descendant_dir(path: &Path) -> PathBuf {
    let children = sorted_subdirs(path);
    match children.last() {
        Some(last) => last_descendant_dir(last),
        None => path.to_path_buf(),
    }
}

// -----------------------------------------------------------------------
// 再帰的サブフォルダ列挙 (キャッシュ作成用)
// -----------------------------------------------------------------------

/// path 以下のすべてのサブフォルダ（path 自身を含む）を再帰的に収集する。
pub fn walk_dirs_recursive(path: &Path, out: &mut Vec<PathBuf>, cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    if !path.is_dir() {
        return;
    }
    out.push(path.to_path_buf());
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_dirs_recursive(&p, out, cancel);
            }
        }
    }
}

// -----------------------------------------------------------------------
// 共通ユーティリティ
// -----------------------------------------------------------------------

/// path 配下の "子フォルダ + .zip ファイル" をソート済みで返す。
/// .zip もナビゲーション対象として扱う (タスク 3)。
pub fn sorted_subdirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.is_dir() {
                Some(p)
            } else if p.is_file() {
                let is_zip = p
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("zip"))
                    .unwrap_or(false);
                if is_zip { Some(p) } else { None }
            } else {
                None
            }
        })
        .collect();
    dirs.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    dirs
}

/// Windows のファイルシステムは大文字小文字を区別しないため小文字化して比較。
pub fn path_eq(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// 与えられたパスを「開けるパス」に解決する。
///
/// - 通常のディレクトリならそのまま返す
/// - `.zip` ファイル (ファイルとして存在) ならそのまま返す
/// - 存在しない/開けない場合は親ディレクトリを再帰的に遡り、最初に見つかった
///   有効なディレクトリを返す
/// - どこにも辿り着けない場合 (ドライブ自体が存在しない等) は `None`
///
/// 起動時の last_folder 復元やアドレスバー入力で、削除済み・移動済み・取り外された
/// ドライブのパスでもクラッシュせず最も近い場所を表示するために使う。
pub fn resolve_openable_path(path: &Path) -> Option<std::path::PathBuf> {
    fn is_zip(p: &Path) -> bool {
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("zip"))
            .unwrap_or(false)
    }

    // そのまま開けるか
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    if path.is_file() && is_zip(path) {
        return Some(path.to_path_buf());
    }

    // 親を再帰的に遡る
    let mut current = path.parent();
    while let Some(p) = current {
        if p.as_os_str().is_empty() {
            return None;
        }
        if p.is_dir() {
            return Some(p.to_path_buf());
        }
        current = p.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_eq_same() {
        assert!(path_eq(Path::new("C:/foo/bar"), Path::new("C:/foo/bar")));
    }

    #[test]
    fn path_eq_case_insensitive() {
        // Windows 想定: 大文字小文字を無視する
        assert!(path_eq(Path::new("C:/Foo/Bar"), Path::new("c:/foo/bar")));
        assert!(path_eq(Path::new("D:/IMG.JPG"), Path::new("d:/img.jpg")));
    }

    #[test]
    fn path_eq_different() {
        assert!(!path_eq(Path::new("C:/foo"), Path::new("C:/bar")));
        assert!(!path_eq(Path::new("C:/foo/a"), Path::new("C:/foo/b")));
    }

    #[test]
    fn supported_extensions_contains_common_formats() {
        for ext in ["jpg", "jpeg", "png", "webp", "bmp", "gif"] {
            assert!(SUPPORTED_EXTENSIONS.contains(&ext), "missing: {}", ext);
        }
    }

    #[test]
    fn supported_video_extensions_contains_common_formats() {
        for ext in ["mp4", "mov", "mkv", "avi"] {
            assert!(
                SUPPORTED_VIDEO_EXTENSIONS.contains(&ext),
                "missing: {}",
                ext
            );
        }
    }
}
