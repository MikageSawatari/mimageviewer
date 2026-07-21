//! パスをキーとして DB に保存するときの正規化ルール。
//!
//! ドライブ文字 (例: `C:`) を除外、小文字化、バックスラッシュ→スラッシュ統一。
//! USB / 外付け HDD のドライブレター変化で保存情報が失われないようにするため。
//!
//! ドライブ文字を保持したい場合 (お気に入り検索のスコープ判定など) は
//! この関数を使わず、呼び出し側で個別に正規化する。

use std::path::Path;

/// ドライブルート (`C:\` など) または共有ルートとして扱うパスか。
///
/// ルート catalog とドライブ一覧 seed はこの判定を共有し、ドライブ直下の同名項目が
/// 別ドライブのキャッシュを拾わないようにする。
pub fn is_drive_or_share_root(path: &Path) -> bool {
    path.parent().is_none()
}

/// ドライブ文字を除いて小文字化・スラッシュ統一したパス文字列を返す。
pub fn normalize(path: &Path) -> String {
    let s = path.to_string_lossy();
    let no_drive = if s.len() >= 2 && s.chars().nth(1) == Some(':') {
        &s[2..]
    } else {
        &s
    };
    no_drive.to_lowercase().replace('\\', "/")
}

/// ドライブ文字を **保持** したまま小文字化・スラッシュ統一したパス文字列を返す。
/// rotation_db / rating_db / video_pins / video_bookmarks / video_tile_thumb_cache 等の
/// DB キー用に使う共通実装。`normalize` (= ドライブ文字除外、お気に入り検索用) とは
/// 別の規則であることに注意。
pub fn normalize_keep_drive(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

/// DB キーと実ファイル列挙結果のように、表記が異なり得る 2 パスを
/// `normalize_keep_drive` と同じ規則で比較する。
pub fn eq_keep_drive(a: &Path, b: &Path) -> bool {
    normalize_keep_drive(a) == normalize_keep_drive(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_drive_letter() {
        assert_eq!(normalize(Path::new(r"C:\Foo\Bar")), "/foo/bar");
        assert_eq!(
            normalize(Path::new(r"D:\Photos\IMG.jpg")),
            "/photos/img.jpg"
        );
    }

    #[test]
    fn no_drive_letter_passthrough() {
        assert_eq!(normalize(Path::new("/foo/bar")), "/foo/bar");
        assert_eq!(
            normalize(Path::new(r"\\server\share\file")),
            "//server/share/file"
        );
    }

    #[test]
    fn lowercases_and_unifies_slashes() {
        assert_eq!(
            normalize(Path::new(r"C:\Mixed/Slash\Path")),
            "/mixed/slash/path"
        );
    }

    #[test]
    fn keep_drive_equality_accepts_db_key_and_filesystem_spelling() {
        assert!(eq_keep_drive(
            Path::new("c:/media/bookmark.mp4"),
            Path::new(r"C:\Media\Bookmark.mp4")
        ));
        assert!(!eq_keep_drive(
            Path::new("c:/media/bookmark.mp4"),
            Path::new(r"D:\Media\Bookmark.mp4")
        ));
    }

    #[test]
    fn drive_root_detection_matches_parent_boundary() {
        assert!(is_drive_or_share_root(Path::new(r"C:\")));
        assert!(!is_drive_or_share_root(Path::new(r"C:\Photos")));
    }
}
