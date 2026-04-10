//! グリッド要素のデータモデル。
//!
//! `GridItem` は一覧に表示される各セルの種別 (フォルダ・画像・動画・ZIP 内画像・
//! ZIP 内サブディレクトリ境界) を表す。
//! `ThumbnailState` は各セルのサムネイル読み込み状態。
//!
//! どちらも純粋なデータ型で、UI 状態や I/O は持たない。

use std::path::PathBuf;

#[derive(Clone)]
pub enum GridItem {
    Folder(PathBuf),
    Image(PathBuf),
    Video(PathBuf),
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
}

impl GridItem {
    /// 表示用の名前を返す。
    /// - 通常: ファイル名
    /// - ZipImage: ZIP 内エントリのベース名
    /// - ZipSeparator: ディレクトリ表示名
    pub fn name(&self) -> &str {
        match self {
            GridItem::Folder(p) | GridItem::Image(p) | GridItem::Video(p) => {
                p.file_name().and_then(|n| n.to_str()).unwrap_or("")
            }
            GridItem::ZipImage { entry_name, .. } => {
                crate::zip_loader::entry_basename(entry_name)
            }
            GridItem::ZipSeparator { dir_display } => dir_display,
        }
    }
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
    Loaded {
        tex: egui::TextureHandle,
        from_cache: bool,
        rendered_at_px: u32,
    },
    /// 読み込みに失敗した（再試行しない）
    #[allow(dead_code)]
    Failed,
    /// 段階 B: 先読み範囲外に出て GPU テクスチャを破棄済み
    /// 再び範囲内に入ったら再ロードされる
    Evicted,
}
