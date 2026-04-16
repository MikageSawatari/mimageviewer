//! パスをキーとして DB に保存するときの正規化ルール。
//!
//! ドライブ文字 (例: `C:`) を除外、小文字化、バックスラッシュ→スラッシュ統一。
//! USB / 外付け HDD のドライブレター変化で保存情報が失われないようにするため。
//!
//! ドライブ文字を保持したい場合 (お気に入り検索のスコープ判定など) は
//! この関数を使わず、呼び出し側で個別に正規化する。

use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_drive_letter() {
        assert_eq!(normalize(Path::new(r"C:\Foo\Bar")), "/foo/bar");
        assert_eq!(normalize(Path::new(r"D:\Photos\IMG.jpg")), "/photos/img.jpg");
    }

    #[test]
    fn no_drive_letter_passthrough() {
        assert_eq!(normalize(Path::new("/foo/bar")), "/foo/bar");
        assert_eq!(normalize(Path::new(r"\\server\share\file")), "//server/share/file");
    }

    #[test]
    fn lowercases_and_unifies_slashes() {
        assert_eq!(normalize(Path::new(r"C:\Mixed/Slash\Path")), "/mixed/slash/path");
    }
}
