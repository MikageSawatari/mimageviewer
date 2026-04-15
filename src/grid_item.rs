//! グリッド要素のデータモデル。
//!
//! `GridItem` は一覧に表示される各セルの種別 (フォルダ・画像・動画・ZIP/PDF ファイル・
//! ZIP 内画像・ZIP 内サブディレクトリ境界・PDF ページ) を表す。
//! `ThumbnailState` は各セルのサムネイル読み込み状態。
//!
//! どちらも純粋なデータ型で、UI 状態や I/O は持たない。

use std::borrow::Cow;
use std::path::PathBuf;

#[derive(Clone)]
pub enum GridItem {
    Folder(PathBuf),
    Image(PathBuf),
    Video(PathBuf),
    /// フォルダ一覧に表示される ZIP ファイル (1枚目のサムネイル + バッジ)
    ZipFile(PathBuf),
    /// フォルダ一覧に表示される PDF ファイル (1ページ目のサムネイル + バッジ)
    PdfFile(PathBuf),
    /// タスク 3: ZIP ファイル内の画像エントリ
    ZipImage {
        zip_path: PathBuf,
        entry_name: String,
    },
    /// タスク 3: ZIP 内のサブディレクトリ境界を示す擬似アイテム
    /// (1 セル分を占め、作品名など大きな文字で表示される)
    ZipSeparator {
        /// 表示されるディレクトリ名 (ルート直下の場合は "(root)")
        dir_display: String,
    },
    /// PDF ファイル内の 1 ページ
    PdfPage {
        pdf_path: PathBuf,
        /// ページ番号 (0-indexed)
        page_num: u32,
    },
}

impl GridItem {
    /// 表示用の名前を返す。
    /// - 通常: ファイル名
    /// - ZipImage: ZIP 内エントリのベース名
    /// - ZipSeparator: ディレクトリ表示名
    /// - PdfPage: "Page N" (1-indexed)
    pub fn name(&self) -> Cow<'_, str> {
        match self {
            GridItem::Folder(p) | GridItem::Image(p) | GridItem::Video(p)
            | GridItem::ZipFile(p) | GridItem::PdfFile(p) => {
                Cow::Borrowed(p.file_name().and_then(|n| n.to_str()).unwrap_or(""))
            }
            GridItem::ZipImage { entry_name, .. } => {
                Cow::Borrowed(crate::zip_loader::entry_basename(entry_name))
            }
            GridItem::ZipSeparator { dir_display } => Cow::Borrowed(dir_display),
            GridItem::PdfPage { page_num, .. } => {
                Cow::Owned(format!("Page {}", page_num + 1))
            }
        }
    }

    /// チェックボックスで選択できるアイテムか (画像・動画・ZIP 内画像・PDF ページ)。
    /// フォルダ・ZIP/PDF ファイル・ZIP セパレータはナビゲーション用なので対象外。
    pub fn is_checkable(&self) -> bool {
        matches!(
            self,
            GridItem::Image(_)
                | GridItem::Video(_)
                | GridItem::ZipImage { .. }
                | GridItem::PdfPage { .. }
        )
    }
}

/// PDF ページのカタログキーを生成する。
/// サムネイルキャッシュの保存・参照で一致させるため、全箇所でこの関数を使うこと。
pub fn pdf_page_cache_key(page_num: u32) -> String {
    format!("page_{:04}", page_num)
}

/// サムネイルセルの読み込み状態。
pub enum ThumbnailState {
    /// まだロードされていない
    Pending,
    /// 読み込み済みで GPU テクスチャとして保持中
    ///
    /// `from_cache = true` の場合は WebP キャッシュ (q=75) から復元した状態で、
    /// 段階 E のアイドル時アップグレードで元画像から再デコードされる対象になる。
    /// `rendered_at_px` は生成時の長辺ピクセル数で、現在のセルサイズと比較して
    /// 著しく小さい場合 (列数変更後など) もアップグレード対象になる。
    /// `source_dims` は元画像のピクセル寸法 (旧カタログ由来は None)。
    Loaded {
        tex: egui::TextureHandle,
        from_cache: bool,
        rendered_at_px: u32,
        source_dims: Option<(u32, u32)>,
    },
    /// 読み込みに失敗した（再試行しない）
    Failed,
    /// 段階 B: 先読み範囲外に出て GPU テクスチャを破棄済み
    /// 再び範囲内に入ったら再ロードされる
    Evicted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name() {
        let item = GridItem::Folder(PathBuf::from(r"C:\foo\bar"));
        assert_eq!(item.name(), "bar");
    }

    #[test]
    fn image_name() {
        let item = GridItem::Image(PathBuf::from(r"D:\photos\sunset.jpg"));
        assert_eq!(item.name(), "sunset.jpg");
    }

    #[test]
    fn video_name() {
        let item = GridItem::Video(PathBuf::from(r"E:\videos\clip.mp4"));
        assert_eq!(item.name(), "clip.mp4");
    }

    #[test]
    fn zip_image_name() {
        let item = GridItem::ZipImage {
            zip_path: PathBuf::from(r"C:\archive.zip"),
            entry_name: "chapter1/page01.jpg".to_string(),
        };
        assert_eq!(item.name(), "page01.jpg");
    }

    #[test]
    fn zip_separator_name() {
        let item = GridItem::ZipSeparator {
            dir_display: "Chapter 1".to_string(),
        };
        assert_eq!(item.name(), "Chapter 1");
    }

    #[test]
    fn name_root_path() {
        // ルートパスの場合、file_name() は None → ""
        let item = GridItem::Folder(PathBuf::from(r"C:\"));
        assert_eq!(item.name(), "");
    }
}
