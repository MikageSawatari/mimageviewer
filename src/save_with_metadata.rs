//! 元画像のメタデータを保持したまま編集後の画像を保存するモジュール。
//!
//! 対応フォーマット (Phase 5):
//! - **JPEG**: turbojpeg で encode → 元 JPEG の APP1 (EXIF / XMP) を SOI 直後に挿入
//! - **PNG**: image crate の PngEncoder で encode → 元 PNG の tEXt / iTXt / zTXt
//!   チャンクを IHDR と IDAT の間に**生バイト**で挿入 (= AI prompt は zlib 圧縮も含めて
//!   そのまま転記)
//! - **WebP**: webp crate で encode → 元 WebP の RIFF コンテナを解析して
//!   ICCP / EXIF / XMP チャンクを抽出 → VP8X 拡張コンテナを構築して挿入
//!
//! 非対応形式 (HEIC / AVIF / JXL / TIFF / RAW) は [`SrcFormat::Other`] として扱い、
//! 呼び出し側で JPEG / PNG にフォールバックする (この場合メタデータは失われる)。
//!
//! 詳細仕様: [docs/conceal-feature-plan.md §10.5-10.7](../../docs/conceal-feature-plan.md)

use std::io;
use std::path::{Path, PathBuf};

use eframe::egui::ColorImage;

// ── 公開型 ──────────────────────────────────────────────────────────────

/// 元画像のフォーマット種別。
///
/// `Other` は WIC でデコードできるが書き出し API を持たない形式 (HEIC / AVIF /
/// JXL / TIFF / RAW) を指す。呼び出し側 (Ctrl+E export ダイアログ) で JPEG / PNG
/// へフォールバック確認を行う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SrcFormat {
    Jpeg,
    Png,
    Webp,
    /// 非対応形式。`String` は拡張子 (lower case、`.` 抜き、例: "heic")。
    Other(String),
}

impl SrcFormat {
    /// 拡張子文字列 ("jpg" / "png" / "webp" / "heic" など、`.` 抜き lower case)
    /// から判定。`None` は拡張子無し / 認識不可。
    pub fn from_ext(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Self::Jpeg,
            "png" => Self::Png,
            "webp" => Self::Webp,
            other => Self::Other(other.to_string()),
        }
    }

    /// `Path` の拡張子から判定 (`from_ext` のラッパー)。
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        Self::from_ext(&ext)
    }

    /// このフォーマットでメタデータ保持 (本モジュール経由の書き込み) が可能か。
    pub fn supports_metadata_writeback(&self) -> bool {
        matches!(self, Self::Jpeg | Self::Png | Self::Webp)
    }

    /// 出力時の拡張子 (`Other` は元の拡張子をそのまま返す)。
    pub fn extension(&self) -> &str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Other(s) => s,
        }
    }
}

/// 保存オプション。
#[derive(Clone, Debug)]
pub struct SaveOptions {
    /// JPEG 品質 (0-100、既定 95)。
    pub jpeg_quality: u8,
    /// JPEG のクロマサブサンプリング。既定 4:2:0 (`Sub2x2`、サイズ最小)。
    pub jpeg_subsampling: turbojpeg::Subsamp,
    /// WebP を lossless (VP8L) で書くか。既定 false (lossy VP8)。
    pub webp_lossless: bool,
    /// WebP lossy 品質 (0.0-100.0、既定 90.0)。`webp_lossless=true` の時は無視。
    pub webp_quality: f32,
    /// 元のメタデータを保持するか。`false` ならメタデータ無しの素の画像を出力。
    pub include_metadata: bool,
    /// 呼び出し側で **既に** EXIF Orientation 通りに pixels を回転済みか。
    /// 既定 `true` (= mIV の通常 fullscreen 表示パスは `wic_decoder` や
    /// EXIF orientation 経路でローカル JPEG を canonical orientation に揃える)。
    ///
    /// ZIP 内 JPEG 等で **pixels が未回転のまま** export する場合は `false` を
    /// 渡す。`false` のときは EXIF Orientation タグを書き換えず元の値を保つ
    /// (= ビューアが元 Orientation で再回転して正しく表示する)。
    ///
    /// 詳細: 通常パス (= `true`) では pixels が canonical 状態なので
    /// Orientation を 1 に書き換えて二重回転を防ぐ。それ以外のパスでは
    /// 二重回転を起こさないために Orientation を温存する。
    pub caller_applied_orientation: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            jpeg_quality: 95,
            jpeg_subsampling: turbojpeg::Subsamp::Sub2x2,
            webp_lossless: false,
            webp_quality: 90.0,
            include_metadata: true,
            caller_applied_orientation: true,
        }
    }
}

/// 保存エラー。
#[derive(Debug)]
pub enum SaveError {
    /// `SrcFormat::Other` を渡された (フォールバック処理は呼び出し側)。
    UnsupportedFormat(String),
    /// アニメーション WebP は対応外 (静止画前提のため)。
    AnimatedWebpNotSupported,
    /// 元画像のメタデータ抽出に失敗。
    MetadataReadFailed(String),
    /// エンコードに失敗。
    EncodingFailed(String),
    /// I/O エラー。
    IoError(io::Error),
    /// `pixels` のサイズが不正 (width または height = 0)。
    InvalidPixels(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedFormat(ext) => {
                write!(f, "メタデータ書き込み非対応形式: .{ext}")
            }
            Self::AnimatedWebpNotSupported => write!(f, "アニメーション WebP は対応外"),
            Self::MetadataReadFailed(msg) => write!(f, "メタデータ抽出失敗: {msg}"),
            Self::EncodingFailed(msg) => write!(f, "エンコード失敗: {msg}"),
            Self::IoError(e) => write!(f, "I/O エラー: {e}"),
            Self::InvalidPixels(msg) => write!(f, "ピクセルバッファ不正: {msg}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        Self::IoError(e)
    }
}

// ── 公開 API ────────────────────────────────────────────────────────────

/// 編集後のピクセルを保存する (元画像のメタデータを保持)。
///
/// - `pixels`: 編集後の ColorImage (RGBA8)
/// - `src_path`: メタデータ抽出元のファイルパス (`None` ならパスからは読まない)
/// - `src_bytes`: メタデータ抽出元のバイト列 (`None` ならバイト列からは読まない)。
///   `src_path` と `src_bytes` の両方を渡した場合は `src_bytes` を優先する
///   (ZIP 内エントリ等で path が存在しないケース用)。
/// - `dst_path`: 出力ファイルパス。親ディレクトリは事前に作成しておくこと。
/// - `src_format`: 元画像のフォーマット (出力もこれに合わせる)
/// - `options`: エンコード品質 + メタデータ保持設定
///
/// ファイルは `OpenOptions::create_new(true)` 相当で書く (上書き禁止)。既に
/// `dst_path` に同名ファイルがあると失敗するので、呼び出し側で連番を付ける等の
/// 衝突回避を行うこと。
pub fn save_image_with_metadata(
    pixels: &ColorImage,
    src_path: Option<&Path>,
    src_bytes: Option<&[u8]>,
    dst_path: &Path,
    src_format: SrcFormat,
    options: &SaveOptions,
) -> Result<(), SaveError> {
    let encoded = encode_image_with_metadata(pixels, src_path, src_bytes, src_format, options)?;
    write_bytes_create_new(&encoded, dst_path)
}

/// 内部実装: pixels を encode して `Vec<u8>` を返す。`save_image_with_metadata`
/// と `save_image_with_metadata_unique` で encode を共有することで、unique 探索時
/// にファイル名衝突のたびに re-encode しないようにする (Codex P2)。
fn encode_image_with_metadata(
    pixels: &ColorImage,
    src_path: Option<&Path>,
    src_bytes: Option<&[u8]>,
    src_format: SrcFormat,
    options: &SaveOptions,
) -> Result<Vec<u8>, SaveError> {
    let (w, h) = (pixels.size[0], pixels.size[1]);
    if w == 0 || h == 0 {
        return Err(SaveError::InvalidPixels(format!("size {w}x{h}")));
    }
    if let SrcFormat::Other(ext) = &src_format {
        return Err(SaveError::UnsupportedFormat(ext.clone()));
    }

    // src bytes は次のいずれかで必要:
    // - include_metadata=true (= メタデータ抽出元)
    // - src_format=Webp で src_bytes/src_path のいずれかが与えられている
    //   (= アニメーション判定。include_metadata=false でもこの検査だけは
    //   常に走らせたいので src を読む — Codex P2)
    let needs_src_for_anim_check =
        matches!(src_format, SrcFormat::Webp) && (src_bytes.is_some() || src_path.is_some());
    let src_bytes_owned: Option<Vec<u8>> = if options.include_metadata || needs_src_for_anim_check {
        match (src_bytes, src_path) {
            (Some(b), _) => Some(b.to_vec()),
            (None, Some(p)) => Some(std::fs::read(p).map_err(SaveError::IoError)?),
            (None, None) => None,
        }
    } else {
        None
    };
    let src_bytes_ref: Option<&[u8]> = src_bytes_owned.as_deref();

    // アニメーション WebP は静止画前提なので include_metadata と無関係に拒否。
    if matches!(src_format, SrcFormat::Webp)
        && let Some(src) = src_bytes_ref
        && webp_is_animated(src)
    {
        return Err(SaveError::AnimatedWebpNotSupported);
    }

    match &src_format {
        SrcFormat::Jpeg => encode_jpeg_with_metadata(pixels, src_bytes_ref, options),
        SrcFormat::Png => encode_png_with_metadata(pixels, src_bytes_ref, options),
        SrcFormat::Webp => encode_webp_with_metadata(pixels, src_bytes_ref, options),
        SrcFormat::Other(_) => unreachable!("filtered above"),
    }
}

fn write_bytes_create_new(bytes: &[u8], dst_path: &Path) -> Result<(), SaveError> {
    if let Some(parent) = dst_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(SaveError::IoError)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst_path)
        .map_err(SaveError::IoError)?;
    use std::io::Write;
    file.write_all(bytes).map_err(SaveError::IoError)?;
    file.flush().map_err(SaveError::IoError)?;
    Ok(())
}

/// `dst_path` の親フォルダで `basename` + `_NNNN.<ext>` の最初の空き番号を探して保存する。
/// `seq_start` から `seq_max` まで試し、ファイル名衝突は `create_new(true)` で検出する。
///
/// **encode は 1 度だけ走らせる** (Codex P2)。連番探索のたびに re-encode すると
/// バッチエクスポートで大量の同一画像を出すときに無駄な CPU を食う。
///
/// 戻り値は実際に書いたファイルのパス。
pub fn save_image_with_metadata_unique(
    pixels: &ColorImage,
    src_path: Option<&Path>,
    src_bytes: Option<&[u8]>,
    output_dir: &Path,
    basename: &str,
    src_format: SrcFormat,
    options: &SaveOptions,
    seq_start: u32,
    seq_max: u32,
) -> Result<PathBuf, SaveError> {
    if seq_start > seq_max {
        return Err(SaveError::EncodingFailed(format!(
            "seq_start={seq_start} > seq_max={seq_max}"
        )));
    }
    let encoded =
        encode_image_with_metadata(pixels, src_path, src_bytes, src_format.clone(), options)?;
    let ext = src_format.extension();
    for seq in seq_start..=seq_max {
        let dst = output_dir.join(format!("{basename}_{seq:04}.{ext}"));
        match write_bytes_create_new(&encoded, &dst) {
            Ok(()) => return Ok(dst),
            Err(SaveError::IoError(e)) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(SaveError::EncodingFailed(format!(
        "連番が {seq_max} まで埋まっています: {}",
        output_dir.display()
    )))
}

// ── JPEG ──────────────────────────────────────────────────────────────

fn encode_jpeg_with_metadata(
    pixels: &ColorImage,
    src_bytes: Option<&[u8]>,
    options: &SaveOptions,
) -> Result<Vec<u8>, SaveError> {
    // RGBA → RGB (アルファは black-matte で flatten)。
    let (w, h) = (pixels.size[0] as u32, pixels.size[1] as u32);
    let rgba = color_image_to_rgba(pixels);
    let rgb = flatten_rgba_to_rgb_black(&rgba, w);
    let image = image::RgbImage::from_raw(w, h, rgb)
        .ok_or_else(|| SaveError::EncodingFailed("RGB バッファ作成失敗".into()))?;
    let jpeg_bytes = turbojpeg::compress_image(
        &image,
        options.jpeg_quality as i32,
        options.jpeg_subsampling,
    )
    .map_err(|e| SaveError::EncodingFailed(format!("turbojpeg: {e}")))?;
    let mut out: Vec<u8> = jpeg_bytes.as_ref().to_vec();

    // メタデータ抽出: src JPEG の APP1 セグメントを SOI 直後に挿入。
    if let Some(src) = src_bytes
        && options.include_metadata
    {
        let mut app1_segments = extract_jpeg_app1_segments(src)?;
        // EXIF APP1 の Orientation 処理 (Codex P1 / r2 P2)。
        // - `caller_applied_orientation = true`: pixels は canonical 向きに回転
        //   済みなので Orientation を 1 に書き換える (= viewer の二重回転防止)。
        // - `caller_applied_orientation = false` (ZIP 内 JPEG 等): pixels は
        //   未回転なので Orientation を温存 (= viewer が元通りに再回転して
        //   正しく表示)。
        if options.caller_applied_orientation {
            for seg in app1_segments.iter_mut() {
                reset_exif_orientation_in_app1(seg);
            }
        }
        if !app1_segments.is_empty() {
            out = splice_jpeg_app1_segments(&out, &app1_segments)?;
        }
    }
    Ok(out)
}

/// JPEG バイト列から APP1 セグメントを**生バイト**でそのまま取り出す。
/// `Vec<Vec<u8>>` の各要素は `[0xFF, 0xE1, len_hi, len_lo, payload...]` の生表現。
/// 元の出現順を保つ (Exif APP1 / XMP APP1 が複数並ぶケースに対応)。
fn extract_jpeg_app1_segments(jpeg: &[u8]) -> Result<Vec<Vec<u8>>, SaveError> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err(SaveError::MetadataReadFailed("JPEG SOI 不正".into()));
    }
    let mut segs: Vec<Vec<u8>> = Vec::new();
    let mut pos = 2;
    while pos + 4 <= jpeg.len() {
        if jpeg[pos] != 0xFF {
            // パディング 0xFF だけ続いているケースも見かけるので 1 byte ずつ skip
            pos += 1;
            continue;
        }
        // 連続する 0xFF は marker 単体 (= padding) と marker prefix の区別が必要。
        // 0xFF, 0xFF, ... の連続は最初の非 0xFF が marker。
        let mut marker_pos = pos;
        while marker_pos + 1 < jpeg.len() && jpeg[marker_pos + 1] == 0xFF {
            marker_pos += 1;
        }
        if marker_pos + 1 >= jpeg.len() {
            break;
        }
        let marker = jpeg[marker_pos + 1];
        // SOS (0xFFDA) 以降は entropy-coded data なので scan を打ち切る。
        if marker == 0xDA {
            break;
        }
        // marker 単体 (length なし): 0x00, 0x01, 0xD0-0xD9
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            pos = marker_pos + 2;
            continue;
        }
        // length 付き marker
        if marker_pos + 4 > jpeg.len() {
            break;
        }
        let len_hi = jpeg[marker_pos + 2];
        let len_lo = jpeg[marker_pos + 3];
        let seg_len = ((len_hi as usize) << 8) | (len_lo as usize); // length 自体を含む
        // length 自身が 2 を切るのは不正 (= length field の最小値は 2)。
        // 不正セグメントを raw に転写すると出力 JPEG が壊れるので skip する
        // (Codex P2)。
        if seg_len < 2 {
            // 不正長 → 残りの scan を諦めるのが安全
            break;
        }
        let seg_end = marker_pos + 2 + seg_len;
        if seg_end > jpeg.len() {
            break;
        }
        if marker == 0xE1 {
            // APP1: [0xFF, 0xE1, len_hi, len_lo, payload...]
            let mut seg = Vec::with_capacity(2 + seg_len);
            seg.extend_from_slice(&jpeg[marker_pos..seg_end]);
            segs.push(seg);
        }
        pos = seg_end;
    }
    Ok(segs)
}

/// JPEG APP1 EXIF セグメントの `Orientation` タグを 1 に書き換える。
///
/// mIV は表示前に `wic_decoder` / EXIF orientation 経路でピクセルを既に回転させているため、
/// 元 EXIF の Orientation をそのままコピーすると EXIF-aware ビューア
/// (例: Windows フォト) が**もう一度回転**してしまう。出力ピクセルは「上が天」なので
/// Orientation は 1 (= No rotation) に固定する。`Exif\0\0` で始まらない
/// (= XMP only など) セグメントは何もしない。`Orientation` タグが無ければ no-op。
///
/// EXIF TIFF パーサは IFD0 のみを舐めて Orientation (tag 0x0112、type=SHORT) を探す
/// 軽量実装。他の IFD (Exif IFD / GPS IFD / Interop) は触らない。
fn reset_exif_orientation_in_app1(app1_segment: &mut [u8]) {
    // APP1 marker (2) + length (2) + "Exif\0\0" (6) = 10 bytes 必要
    if app1_segment.len() < 10 {
        return;
    }
    if &app1_segment[4..10] != b"Exif\0\0" {
        return; // not EXIF (XMP APP1 等)
    }
    let tiff_start = 10;
    if tiff_start + 8 > app1_segment.len() {
        return;
    }
    let little_endian = match &app1_segment[tiff_start..tiff_start + 2] {
        b"II" => true,
        b"MM" => false,
        _ => return,
    };
    let magic_ok = if little_endian {
        app1_segment[tiff_start + 2] == 0x2A && app1_segment[tiff_start + 3] == 0x00
    } else {
        app1_segment[tiff_start + 2] == 0x00 && app1_segment[tiff_start + 3] == 0x2A
    };
    if !magic_ok {
        return;
    }
    let read_u16 = |buf: &[u8]| -> u16 {
        if little_endian {
            u16::from_le_bytes([buf[0], buf[1]])
        } else {
            u16::from_be_bytes([buf[0], buf[1]])
        }
    };
    let read_u32 = |buf: &[u8]| -> u32 {
        if little_endian {
            u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])
        } else {
            u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]])
        }
    };
    let ifd0_offset = read_u32(&app1_segment[tiff_start + 4..tiff_start + 8]) as usize;
    let ifd0_start = tiff_start + ifd0_offset;
    if ifd0_start + 2 > app1_segment.len() {
        return;
    }
    let entry_count = read_u16(&app1_segment[ifd0_start..ifd0_start + 2]) as usize;
    let entries_start = ifd0_start + 2;
    if entries_start + entry_count * 12 > app1_segment.len() {
        return;
    }
    for i in 0..entry_count {
        let entry_pos = entries_start + i * 12;
        let tag = read_u16(&app1_segment[entry_pos..entry_pos + 2]);
        if tag != 0x0112 {
            continue;
        }
        // Orientation の正規 schema: type=SHORT(3), count=1 で value 4 バイトの
        // 先頭 2 バイトに inline 格納。不正な EXIF はこの schema に従わず
        // value 4 バイトが offset として使われていることがある。その offset を
        // 0x0001 で上書きすると TIFF の別領域を破壊する → 必ず schema 検査して
        // 一致したときだけ書き換える (Codex r2 P3)。
        let dtype = read_u16(&app1_segment[entry_pos + 2..entry_pos + 4]);
        let count = read_u32(&app1_segment[entry_pos + 4..entry_pos + 8]);
        if dtype != 3 || count != 1 {
            // 不正 schema は no-op (= 元の Orientation 値を温存)
            return;
        }
        let value_pos = entry_pos + 8;
        if little_endian {
            app1_segment[value_pos] = 0x01;
            app1_segment[value_pos + 1] = 0x00;
            app1_segment[value_pos + 2] = 0x00;
            app1_segment[value_pos + 3] = 0x00;
        } else {
            app1_segment[value_pos] = 0x00;
            app1_segment[value_pos + 1] = 0x01;
            app1_segment[value_pos + 2] = 0x00;
            app1_segment[value_pos + 3] = 0x00;
        }
        return;
    }
}

/// JPEG の SOI (FFD8) 直後に APP1 セグメントを挿入する。
/// 既存の APP1 はすべて取り除いてから新規挿入することで重複を避ける。
fn splice_jpeg_app1_segments(jpeg: &[u8], app1: &[Vec<u8>]) -> Result<Vec<u8>, SaveError> {
    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err(SaveError::EncodingFailed("出力 JPEG の SOI が無い".into()));
    }
    // 出力 JPEG の既存 APP1 を取り除く (turbojpeg は通常 APP1 を出さないが念のため)
    let mut stripped: Vec<u8> = Vec::with_capacity(jpeg.len());
    stripped.extend_from_slice(&jpeg[..2]); // SOI
    let mut pos = 2;
    while pos + 4 <= jpeg.len() {
        if jpeg[pos] != 0xFF {
            stripped.push(jpeg[pos]);
            pos += 1;
            continue;
        }
        let marker = jpeg[pos + 1];
        if marker == 0xDA {
            stripped.extend_from_slice(&jpeg[pos..]);
            break;
        }
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            stripped.extend_from_slice(&jpeg[pos..pos + 2]);
            pos += 2;
            continue;
        }
        if pos + 4 > jpeg.len() {
            stripped.extend_from_slice(&jpeg[pos..]);
            break;
        }
        let seg_len = ((jpeg[pos + 2] as usize) << 8) | (jpeg[pos + 3] as usize);
        let seg_end = pos + 2 + seg_len;
        if seg_end > jpeg.len() {
            stripped.extend_from_slice(&jpeg[pos..]);
            break;
        }
        if marker != 0xE1 {
            stripped.extend_from_slice(&jpeg[pos..seg_end]);
        }
        pos = seg_end;
    }

    // SOI (2 bytes) の直後に APP1 群を挿入。
    let mut out: Vec<u8> =
        Vec::with_capacity(stripped.len() + app1.iter().map(|s| s.len()).sum::<usize>());
    out.extend_from_slice(&stripped[..2]);
    for seg in app1 {
        out.extend_from_slice(seg);
    }
    out.extend_from_slice(&stripped[2..]);
    Ok(out)
}

// ── PNG ───────────────────────────────────────────────────────────────

fn encode_png_with_metadata(
    pixels: &ColorImage,
    src_bytes: Option<&[u8]>,
    options: &SaveOptions,
) -> Result<Vec<u8>, SaveError> {
    use image::ImageEncoder;
    let (w, h) = (pixels.size[0] as u32, pixels.size[1] as u32);
    let rgba = color_image_to_rgba(pixels);
    let mut out: Vec<u8> = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgba, w, h, image::ColorType::Rgba8.into())
        .map_err(|e| SaveError::EncodingFailed(format!("PNG encoder: {e}")))?;
    if let Some(src) = src_bytes
        && options.include_metadata
    {
        let chunks = extract_png_text_chunks_raw(src)?;
        if !chunks.is_empty() {
            out = splice_png_text_chunks(&out, &chunks)?;
        }
    }
    Ok(out)
}

/// IEEE 802.3 多項式 (0xEDB88320 reflected) の CRC32 — PNG / zlib と同じ系。
/// テスト fixture と raw chunk 検証で共有する。
fn png_crc32_compute(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// PNG の tEXt / iTXt / zTXt チャンクを**生バイト**でそのまま取り出す。
/// 各要素は `[length(4) + chunk_type(4) + data + crc(4)]` の生バイト列。
/// 出現順を保ち、IHDR / IDAT / IEND など本体チャンクは除外する。
///
/// チャンクの **CRC32 を検証**し、不正なものは skip する (Codex P2)。元 PNG が
/// 壊れた tEXt を含んでいる場合、それを raw で転写すると出力 PNG が strict
/// デコーダ (libpng 等) で reject される。
fn extract_png_text_chunks_raw(png: &[u8]) -> Result<Vec<Vec<u8>>, SaveError> {
    if png.len() < 8 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(SaveError::MetadataReadFailed("PNG signature 不正".into()));
    }
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut pos = 8;
    while pos + 12 <= png.len() {
        let length =
            u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
        let chunk_type = &png[pos + 4..pos + 8];
        let chunk_end = pos + 8 + length + 4; // length + type + data + crc
        if chunk_end > png.len() {
            break;
        }
        match chunk_type {
            b"tEXt" | b"iTXt" | b"zTXt" => {
                // CRC32 を検証 (= chunk_type + data に対して計算)。
                let crc_pos = pos + 8 + length;
                let stored_crc = u32::from_be_bytes([
                    png[crc_pos],
                    png[crc_pos + 1],
                    png[crc_pos + 2],
                    png[crc_pos + 3],
                ]);
                let computed = png_crc32_compute(&png[pos + 4..crc_pos]);
                if computed == stored_crc {
                    out.push(png[pos..chunk_end].to_vec());
                }
                // CRC 不一致は skip (= 出力 PNG を守る方を優先)。
            }
            b"IEND" => break,
            _ => {}
        }
        pos = chunk_end;
    }
    Ok(out)
}

/// PNG の IHDR (= 最初のチャンク) と IDAT の間に raw chunk 群を挿入する。
fn splice_png_text_chunks(png: &[u8], chunks: &[Vec<u8>]) -> Result<Vec<u8>, SaveError> {
    if png.len() < 8 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(SaveError::EncodingFailed("出力 PNG signature 不正".into()));
    }
    // IHDR chunk 終端を探す (= 最初のチャンク末尾)。
    let pos = 8;
    if pos + 12 > png.len() {
        return Err(SaveError::EncodingFailed(
            "出力 PNG が短すぎる (IHDR が無い)".into(),
        ));
    }
    let ihdr_length =
        u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
    if &png[pos + 4..pos + 8] != b"IHDR" {
        return Err(SaveError::EncodingFailed(
            "出力 PNG の最初のチャンクが IHDR ではない".into(),
        ));
    }
    let ihdr_end = pos + 8 + ihdr_length + 4;
    if ihdr_end > png.len() {
        return Err(SaveError::EncodingFailed(
            "出力 PNG の IHDR が truncated".into(),
        ));
    }
    let mut total_extra = 0;
    for c in chunks {
        total_extra += c.len();
    }
    let mut out: Vec<u8> = Vec::with_capacity(png.len() + total_extra);
    out.extend_from_slice(&png[..ihdr_end]);
    for c in chunks {
        out.extend_from_slice(c);
    }
    out.extend_from_slice(&png[ihdr_end..]);
    Ok(out)
}

// ── WebP ──────────────────────────────────────────────────────────────

fn encode_webp_with_metadata(
    pixels: &ColorImage,
    src_bytes: Option<&[u8]>,
    options: &SaveOptions,
) -> Result<Vec<u8>, SaveError> {
    let (w, h) = (pixels.size[0] as u32, pixels.size[1] as u32);
    let rgba = color_image_to_rgba(pixels);
    let encoder = webp::Encoder::from_rgba(&rgba, w, h);
    let encoded = if options.webp_lossless {
        encoder.encode_lossless()
    } else {
        encoder.encode(options.webp_quality.clamp(1.0, 100.0))
    };
    let mut out: Vec<u8> = encoded.to_vec();

    if let Some(src) = src_bytes
        && options.include_metadata
    {
        // 元 WebP がアニメーションだったら拒否 (= モザイクは静止画前提)
        // src がアニメーションなら ANIM / ANMF チャンクが居る。
        if webp_is_animated(src) {
            return Err(SaveError::AnimatedWebpNotSupported);
        }
        let (iccp, exif, xmp) = extract_webp_metadata_chunks(src)?;
        if iccp.is_some() || exif.is_some() || xmp.is_some() {
            out = inject_webp_metadata(&out, iccp, exif, xmp)?;
        }
    }
    Ok(out)
}

/// WebP RIFF から ICCP / EXIF / XMP の payload を取り出す。各 `Vec<u8>` はチャンク
/// payload (チャンクヘッダなし、生 EXIF / XMP / ICC バイト列)。
fn extract_webp_metadata_chunks(
    webp: &[u8],
) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>), SaveError> {
    if webp.len() < 12 || &webp[..4] != b"RIFF" || &webp[8..12] != b"WEBP" {
        return Err(SaveError::MetadataReadFailed(
            "WebP RIFF/WEBP ヘッダ不正".into(),
        ));
    }
    let mut iccp: Option<Vec<u8>> = None;
    let mut exif: Option<Vec<u8>> = None;
    let mut xmp: Option<Vec<u8>> = None;
    let mut pos = 12;
    while pos + 8 <= webp.len() {
        let fourcc = &webp[pos..pos + 4];
        let size = u32::from_le_bytes([webp[pos + 4], webp[pos + 5], webp[pos + 6], webp[pos + 7]])
            as usize;
        let data_start = pos + 8;
        let data_end = data_start + size;
        if data_end > webp.len() {
            break;
        }
        match fourcc {
            b"ICCP" => iccp = Some(webp[data_start..data_end].to_vec()),
            b"EXIF" => exif = Some(webp[data_start..data_end].to_vec()),
            b"XMP " => xmp = Some(webp[data_start..data_end].to_vec()),
            _ => {}
        }
        pos = data_end + (size & 1); // RIFF パディング
    }
    Ok((iccp, exif, xmp))
}

/// WebP がアニメーションか (= ANIM / ANMF チャンクを持つか) を判定する。
fn webp_is_animated(webp: &[u8]) -> bool {
    if webp.len() < 12 || &webp[..4] != b"RIFF" || &webp[8..12] != b"WEBP" {
        return false;
    }
    let mut pos = 12;
    while pos + 8 <= webp.len() {
        let fourcc = &webp[pos..pos + 4];
        let size = u32::from_le_bytes([webp[pos + 4], webp[pos + 5], webp[pos + 6], webp[pos + 7]])
            as usize;
        if fourcc == b"ANIM" || fourcc == b"ANMF" {
            return true;
        }
        let data_end = pos + 8 + size;
        if data_end > webp.len() {
            break;
        }
        pos = data_end + (size & 1);
    }
    false
}

/// 出力 WebP に VP8X 拡張コンテナを構築してメタデータチャンクを挿入する。
///
/// 出力が **既に VP8X 付き** (= libwebp が透過画像のために extended container
/// 形式で出した場合) のときは、その VP8X の flags にメタデータ系ビットを OR
/// するだけにする。重複 VP8X を作ると壊れた WebP になる (Codex P1)。
///
/// 透過なし入力で出力が VP8 / VP8L 単体のときは、canvas サイズを VP8/VP8L
/// から取り出して新規 VP8X を先頭に挿入する。
///
/// 既存 VP8X を保ったまま flags をマージするので、`ALPH` チャンクなど
/// libwebp 側の追加チャンクが落ちることもない。
fn inject_webp_metadata(
    out: &[u8],
    iccp: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
) -> Result<Vec<u8>, SaveError> {
    if out.len() < 12 || &out[..4] != b"RIFF" || &out[8..12] != b"WEBP" {
        return Err(SaveError::EncodingFailed("出力 WebP RIFF 不正".into()));
    }
    // 出力 RIFF のチャンクを解析。VP8X / VP8 / VP8L / ALPH / 既存 EXIF/XMP/ICCP を
    // すべて列挙する (existing VP8X / metadata はマージ・置換対象)。
    let mut chunks: Vec<(String, Vec<u8>)> = Vec::new();
    let mut canvas: Option<(u32, u32)> = None;
    let mut existing_vp8x_idx: Option<usize> = None;
    let mut pos = 12;
    while pos + 8 <= out.len() {
        let fourcc = std::str::from_utf8(&out[pos..pos + 4])
            .map_err(|_| SaveError::EncodingFailed("WebP fourcc not utf8".into()))?
            .to_string();
        let size =
            u32::from_le_bytes([out[pos + 4], out[pos + 5], out[pos + 6], out[pos + 7]]) as usize;
        let data_start = pos + 8;
        let data_end = data_start + size;
        if data_end > out.len() {
            break;
        }
        let data = out[data_start..data_end].to_vec();
        // canvas サイズ取得 (VP8X が先頭にあればそこから、なければ VP8/VP8L から)
        if canvas.is_none() {
            match fourcc.as_str() {
                "VP8X" if data.len() >= 10 => {
                    let w =
                        ((data[4] as u32) | ((data[5] as u32) << 8) | ((data[6] as u32) << 16)) + 1;
                    let h =
                        ((data[7] as u32) | ((data[8] as u32) << 8) | ((data[9] as u32) << 16)) + 1;
                    canvas = Some((w, h));
                }
                "VP8 " => canvas = parse_vp8_canvas(&data),
                "VP8L" => canvas = parse_vp8l_canvas(&data),
                _ => {}
            }
        }
        // 既存 metadata chunks は次の append フェーズで重複しないよう除外しつつ
        // VP8X の位置だけ記憶しておく。
        let drop = matches!(fourcc.as_str(), "EXIF" | "XMP " | "ICCP");
        if !drop {
            if fourcc == "VP8X" {
                existing_vp8x_idx = Some(chunks.len());
            }
            chunks.push((fourcc, data));
        }
        pos = data_end + (size & 1);
    }
    let (cw, ch) = canvas
        .ok_or_else(|| SaveError::EncodingFailed("出力 WebP の canvas サイズ取得失敗".into()))?;

    // 追加するメタデータ flags を計算 (VP8X bit2=XMP, bit3=EXIF, bit5=ICC)。
    let mut add_flags: u8 = 0;
    if iccp.is_some() {
        add_flags |= 1 << 5;
    }
    if exif.is_some() {
        add_flags |= 1 << 3;
    }
    if xmp.is_some() {
        add_flags |= 1 << 2;
    }

    if let Some(idx) = existing_vp8x_idx {
        // 既存 VP8X の flags にメタデータビットを OR (= 透過 ALPH bit などは保つ)。
        if let Some((_, data)) = chunks.get_mut(idx)
            && !data.is_empty()
        {
            data[0] |= add_flags;
        }
    } else {
        // 新規 VP8X を構築 (canvas size + alpha + metadata flags)。
        // VP8L (lossless) は VP8X 無しで alpha を表現できるので、
        // メタデータを追加する際は新規 VP8X に alpha bit を立てる必要がある
        // (Codex r2 P2)。ALPH チャンクが居る (VP8 + ALPH 構成) ケースも
        // 同様に alpha bit を立てる。
        let has_alpha = chunks
            .iter()
            .any(|(fc, data)| fc == "ALPH" || (fc == "VP8L" && vp8l_has_alpha(data)));
        let mut vp8x = vec![0u8; 10];
        let mut flags = add_flags;
        if has_alpha {
            flags |= 1 << 4; // alpha
        }
        vp8x[0] = flags;
        let w1 = cw - 1;
        let h1 = ch - 1;
        vp8x[4] = (w1 & 0xFF) as u8;
        vp8x[5] = ((w1 >> 8) & 0xFF) as u8;
        vp8x[6] = ((w1 >> 16) & 0xFF) as u8;
        vp8x[7] = (h1 & 0xFF) as u8;
        vp8x[8] = ((h1 >> 8) & 0xFF) as u8;
        vp8x[9] = ((h1 >> 16) & 0xFF) as u8;
        chunks.insert(0, ("VP8X".to_string(), vp8x));
    }

    // ICCP は VP8X 直後に置く慣習。VP8X の index を再取得して insert する。
    if let Some(iccp_data) = iccp {
        let after_vp8x = chunks
            .iter()
            .position(|(fc, _)| fc == "VP8X")
            .map(|i| i + 1)
            .unwrap_or(0);
        chunks.insert(after_vp8x, ("ICCP".to_string(), iccp_data));
    }
    // EXIF / XMP は末尾に append (画像チャンクの後ろが標準)。
    if let Some(exif_data) = exif {
        chunks.push(("EXIF".to_string(), exif_data));
    }
    if let Some(xmp_data) = xmp {
        chunks.push(("XMP ".to_string(), xmp_data));
    }

    Ok(rebuild_webp_riff(&chunks))
}

fn rebuild_webp_riff(chunks: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(b"WEBP");
    for (fourcc, data) in chunks {
        body.extend_from_slice(fourcc.as_bytes());
        let sz = data.len() as u32;
        body.extend_from_slice(&sz.to_le_bytes());
        body.extend_from_slice(data);
        if data.len() & 1 == 1 {
            body.push(0);
        }
    }
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// VP8 (lossy) チャンクから canvas サイズを取り出す。
/// VP8 bitstream の最初の 10 バイト: 3 frame tag + 3 sync code "9d 01 2a" + 4 size。
fn parse_vp8_canvas(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 10 {
        return None;
    }
    if data[3..6] != [0x9d, 0x01, 0x2a] {
        return None;
    }
    let w = u16::from_le_bytes([data[6], data[7]]) as u32 & 0x3FFF;
    let h = u16::from_le_bytes([data[8], data[9]]) as u32 & 0x3FFF;
    Some((w, h))
}

/// VP8L (lossless) チャンクから canvas サイズを取り出す。
/// VP8L: signature byte (0x2f) + 28 bits = 14 bits w-1 + 14 bits h-1。
fn parse_vp8l_canvas(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 5 || data[0] != 0x2f {
        return None;
    }
    let b1 = data[1] as u32;
    let b2 = data[2] as u32;
    let b3 = data[3] as u32;
    let b4 = data[4] as u32;
    let w1 = b1 | ((b2 & 0x3F) << 8);
    let h1 = (b2 >> 6) | (b3 << 2) | ((b4 & 0x0F) << 10);
    Some((w1 + 1, h1 + 1))
}

/// VP8L チャンクが alpha (透過) を含むかを判定する。
///
/// VP8L bitstream は signature byte (`0x2f`) の後ろの 4 bytes に
/// `14 bits w-1 | 14 bits h-1 | 1 bit alpha_is_used | 3 bits version` を
/// LSB-first で持つ。alpha bit = byte index 4 の bit 4 (= `0x10`)。
fn vp8l_has_alpha(data: &[u8]) -> bool {
    data.len() >= 5 && data[0] == 0x2f && (data[4] & 0x10) != 0
}

// ── 共通 ヘルパー ───────────────────────────────────────────────────

/// `ColorImage` から RGBA8 を取り出す。egui の `Color32` は**premultiplied**
/// 表現で保持されているため、`to_srgba_unmultiplied()` で unmultiplied (= 通常の
/// sRGB) に戻してからエンコーダに渡す。これをやらないと半透明ピクセルが
/// premultiplied 値で書かれてしまい、PNG / WebP デコーダで暗く再現される
/// (Codex P1)。
fn color_image_to_rgba(image: &ColorImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.pixels.len() * 4);
    for px in &image.pixels {
        let [r, g, b, a] = px.to_srgba_unmultiplied();
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}

/// 透過 RGBA を black-matte で RGB に flatten する (= JPEG 用)。
/// 入力 RGBA は**unmultiplied** であることを前提とする (= `color_image_to_rgba` の
/// 出力を直接渡せる)。
fn flatten_rgba_to_rgb_black(rgba: &[u8], _width: u32) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for px in rgba.chunks_exact(4) {
        let alpha = px[3] as u16;
        if alpha == 255 {
            rgb.extend_from_slice(&px[..3]);
            continue;
        }
        // black matte (= 透明部は 0 black に flatten)
        for channel in 0..3 {
            let fg = px[channel] as u16;
            let blended = (fg * alpha + 127) / 255;
            rgb.push(blended as u8);
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    fn sample_color_image(w: usize, h: usize) -> ColorImage {
        let pixels: Vec<egui::Color32> = (0..w * h)
            .map(|i| {
                let r = (i % 256) as u8;
                let g = ((i / 4) % 256) as u8;
                let b = ((i / 16) % 256) as u8;
                egui::Color32::from_rgba_unmultiplied(r, g, b, 255)
            })
            .collect();
        ColorImage {
            size: [w, h],
            pixels,
            source_size: egui::vec2(w as f32, h as f32),
        }
    }

    fn build_jpeg_with_app1(payload: &[u8]) -> Vec<u8> {
        // 最小 JPEG (SOI + APP1 + DQT + SOF0 + SOS + EOI) を手で組むのは大変なので、
        // turbojpeg で実 JPEG を作って APP1 を SOI 直後に挿入する。
        let img = sample_color_image(8, 8);
        let rgba = color_image_to_rgba(&img);
        let rgb = flatten_rgba_to_rgb_black(&rgba, 8);
        let rimg = image::RgbImage::from_raw(8, 8, rgb).unwrap();
        let jpeg = turbojpeg::compress_image(&rimg, 90, turbojpeg::Subsamp::Sub2x2).unwrap();
        let jpeg_bytes = jpeg.as_ref();
        let len = payload.len() + 2; // length は length-bytes 自身を含む
        let mut out = Vec::with_capacity(jpeg_bytes.len() + 4 + payload.len());
        out.extend_from_slice(&jpeg_bytes[..2]); // SOI
        out.push(0xFF);
        out.push(0xE1); // APP1
        out.push((len >> 8) as u8);
        out.push((len & 0xFF) as u8);
        out.extend_from_slice(payload);
        out.extend_from_slice(&jpeg_bytes[2..]);
        out
    }

    #[test]
    fn src_format_from_ext_handles_common_cases() {
        assert_eq!(SrcFormat::from_ext("jpg"), SrcFormat::Jpeg);
        assert_eq!(SrcFormat::from_ext("JPEG"), SrcFormat::Jpeg);
        assert_eq!(SrcFormat::from_ext("png"), SrcFormat::Png);
        assert_eq!(SrcFormat::from_ext("WebP"), SrcFormat::Webp);
        match SrcFormat::from_ext("heic") {
            SrcFormat::Other(s) => assert_eq!(s, "heic"),
            _ => panic!("expected Other"),
        }
    }

    #[test]
    fn extract_jpeg_app1_segments_finds_inserted_segment() {
        let payload = b"Exif\x00\x00MM\x00*\x00\x00\x00\x08test-exif-data";
        let jpeg = build_jpeg_with_app1(payload);
        let segs = extract_jpeg_app1_segments(&jpeg).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0][0], 0xFF);
        assert_eq!(segs[0][1], 0xE1);
        // payload は length(2) の後ろに来る
        assert_eq!(&segs[0][4..4 + payload.len()], payload);
    }

    #[test]
    fn splice_jpeg_app1_segments_roundtrip_preserves_payload() {
        let payload = b"Exif\x00\x00xxxxxxxxxx-roundtrip-test-marker";
        let src_jpeg = build_jpeg_with_app1(payload);

        // 別の JPEG を encode (APP1 無し)
        let img = sample_color_image(16, 16);
        let opts = SaveOptions {
            include_metadata: true,
            ..Default::default()
        };
        let out = encode_jpeg_with_metadata(&img, Some(&src_jpeg), &opts).unwrap();
        // 出力 JPEG から APP1 を再抽出して同じ payload か確認
        let out_segs = extract_jpeg_app1_segments(&out).unwrap();
        assert_eq!(out_segs.len(), 1, "exactly one APP1 should be re-inserted");
        assert_eq!(&out_segs[0][4..4 + payload.len()], payload);
    }

    #[test]
    fn save_jpeg_unique_increments_seq() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"Exif\x00\x00unique-seq-test";
        let src = build_jpeg_with_app1(payload);
        let img = sample_color_image(8, 8);
        let opts = SaveOptions::default();
        let p1 = save_image_with_metadata_unique(
            &img,
            None,
            Some(&src),
            dir.path(),
            "mosaic",
            SrcFormat::Jpeg,
            &opts,
            1,
            10,
        )
        .unwrap();
        let p2 = save_image_with_metadata_unique(
            &img,
            None,
            Some(&src),
            dir.path(),
            "mosaic",
            SrcFormat::Jpeg,
            &opts,
            1,
            10,
        )
        .unwrap();
        assert_eq!(p1.file_name().unwrap(), "mosaic_0001.jpg");
        assert_eq!(p2.file_name().unwrap(), "mosaic_0002.jpg");
        // 両方とも APP1 を保持していること
        let b1 = std::fs::read(&p1).unwrap();
        let b2 = std::fs::read(&p2).unwrap();
        let s1 = extract_jpeg_app1_segments(&b1).unwrap();
        let s2 = extract_jpeg_app1_segments(&b2).unwrap();
        assert_eq!(s1.len(), 1);
        assert_eq!(s2.len(), 1);
    }

    #[test]
    fn unsupported_format_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let img = sample_color_image(4, 4);
        let opts = SaveOptions::default();
        let dst = dir.path().join("foo.heic");
        let result = save_image_with_metadata(
            &img,
            None,
            None,
            &dst,
            SrcFormat::Other("heic".to_string()),
            &opts,
        );
        match result {
            Err(SaveError::UnsupportedFormat(ext)) => assert_eq!(ext, "heic"),
            other => panic!("expected UnsupportedFormat, got {other:?}"),
        }
    }

    #[test]
    fn jpeg_skip_metadata_when_flag_false() {
        let payload = b"Exif\x00\x00should-not-appear";
        let src = build_jpeg_with_app1(payload);
        let img = sample_color_image(8, 8);
        let opts = SaveOptions {
            include_metadata: false,
            ..Default::default()
        };
        let out = encode_jpeg_with_metadata(&img, Some(&src), &opts).unwrap();
        let segs = extract_jpeg_app1_segments(&out).unwrap();
        assert!(segs.is_empty(), "include_metadata=false should drop APP1");
    }

    fn make_minimal_png_with_text_chunk(keyword: &str, text: &str) -> Vec<u8> {
        // 8x8 RGBA PNG を encode してから tEXt チャンクを IHDR 直後に挿入
        let img = sample_color_image(8, 8);
        let rgba = color_image_to_rgba(&img);
        let mut bytes: Vec<u8> = Vec::new();
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&rgba, 8, 8, image::ColorType::Rgba8.into())
            .unwrap();
        // tEXt chunk を組み立て
        let mut chunk_data: Vec<u8> = Vec::new();
        chunk_data.extend_from_slice(keyword.as_bytes());
        chunk_data.push(0);
        chunk_data.extend_from_slice(text.as_bytes());
        // CRC は type (tEXt) + data に対して取る
        let mut crc_input: Vec<u8> = Vec::with_capacity(4 + chunk_data.len());
        crc_input.extend_from_slice(b"tEXt");
        crc_input.extend_from_slice(&chunk_data);
        let crc = png_crc32_compute(&crc_input);
        let mut chunk: Vec<u8> = Vec::new();
        chunk.extend_from_slice(&(chunk_data.len() as u32).to_be_bytes());
        chunk.extend_from_slice(b"tEXt");
        chunk.extend_from_slice(&chunk_data);
        chunk.extend_from_slice(&crc.to_be_bytes());
        splice_png_text_chunks(&bytes, &[chunk]).unwrap()
    }

    #[test]
    fn png_roundtrip_preserves_text_chunk() {
        let src = make_minimal_png_with_text_chunk("parameters", "test-prompt-1234");
        let img = sample_color_image(16, 16);
        let opts = SaveOptions::default();
        let out = encode_png_with_metadata(&img, Some(&src), &opts).unwrap();
        let chunks = extract_png_text_chunks_raw(&out).unwrap();
        assert_eq!(chunks.len(), 1);
        // chunk_data から keyword と text を取り出す
        let c = &chunks[0];
        let length = u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as usize;
        assert_eq!(&c[4..8], b"tEXt");
        let data = &c[8..8 + length];
        let null = data.iter().position(|&b| b == 0).unwrap();
        let keyword = std::str::from_utf8(&data[..null]).unwrap();
        let text = std::str::from_utf8(&data[null + 1..]).unwrap();
        assert_eq!(keyword, "parameters");
        assert_eq!(text, "test-prompt-1234");
    }

    #[test]
    fn webp_roundtrip_preserves_metadata() {
        // 元 WebP を webp::Encoder で encode してから手で XMP / EXIF チャンクを差し込む
        let img = sample_color_image(16, 16);
        let rgba = color_image_to_rgba(&img);
        let raw = webp::Encoder::from_rgba(&rgba, 16, 16).encode(85.0);
        // 元 WebP に XMP / EXIF を inject (= テスト用 fixture)
        let src_with_meta = inject_webp_metadata(
            raw.as_ref(),
            None,
            Some(b"FAKE-EXIF-DATA".to_vec()),
            Some(b"<x:xmpmeta>FAKE</x:xmpmeta>".to_vec()),
        )
        .unwrap();
        // 同じ画像で save (= encode + 再 inject)
        let opts = SaveOptions::default();
        let out = encode_webp_with_metadata(&img, Some(&src_with_meta), &opts).unwrap();
        let (iccp, exif, xmp) = extract_webp_metadata_chunks(&out).unwrap();
        assert!(iccp.is_none());
        assert_eq!(exif.unwrap(), b"FAKE-EXIF-DATA");
        assert_eq!(xmp.unwrap(), b"<x:xmpmeta>FAKE</x:xmpmeta>");
    }

    #[test]
    fn webp_rejects_animated_source() {
        // ANIM チャンクが入っているだけの最小 WebP fixture
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes()); // size placeholder
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"ANIM");
        bytes.extend_from_slice(&6u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0]); // 6 bytes payload
        let total = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&total.to_le_bytes());
        assert!(webp_is_animated(&bytes));

        let img = sample_color_image(4, 4);
        let opts = SaveOptions::default();
        let result = encode_webp_with_metadata(&img, Some(&bytes), &opts);
        assert!(
            matches!(result, Err(SaveError::AnimatedWebpNotSupported)),
            "should reject animated WebP"
        );
    }

    // ── Codex P1/P2 修正対応の回帰テスト ──

    /// 半透明ピクセル (premultiplied alpha) が unmultiplied で書かれていることを確認。
    /// `color_image_to_rgba` が `to_srgba_unmultiplied` を経由していれば、
    /// `Color32::from_rgba_unmultiplied(200, 100, 50, 128)` を取り出したとき
    /// (200, 100, 50, 128) に戻る (= premultiplied 中間値 (100, 50, 25, 128) にならない)。
    #[test]
    fn color_image_to_rgba_returns_unmultiplied() {
        let img = ColorImage {
            size: [1, 1],
            pixels: vec![egui::Color32::from_rgba_unmultiplied(200, 100, 50, 128)],
            source_size: egui::vec2(1.0, 1.0),
        };
        let rgba = color_image_to_rgba(&img);
        assert_eq!(rgba.len(), 4);
        // unmultiplied なら 200, 100, 50 のまま (= premultiplied だと丸誤差 ±1 で
        // ~100, ~50, ~25 になる)。許容誤差 ±2 で確認。
        assert!((rgba[0] as i32 - 200).abs() <= 2, "R: got {}", rgba[0]);
        assert!((rgba[1] as i32 - 100).abs() <= 2, "G: got {}", rgba[1]);
        assert!((rgba[2] as i32 - 50).abs() <= 2, "B: got {}", rgba[2]);
        assert_eq!(rgba[3], 128);
    }

    /// アニメーション WebP は `include_metadata=false` でも拒否されること (Codex P2)。
    /// メタデータは要らなくても、静止画 RGBA を ANIM 入力に対して書くのは妥当でない。
    #[test]
    fn webp_animated_rejected_even_without_metadata() {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"ANIM");
        bytes.extend_from_slice(&6u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0]);
        let total = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&total.to_le_bytes());

        let img = sample_color_image(4, 4);
        let opts = SaveOptions {
            include_metadata: false,
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("anim.webp");
        let result =
            save_image_with_metadata(&img, None, Some(&bytes), &dst, SrcFormat::Webp, &opts);
        assert!(
            matches!(result, Err(SaveError::AnimatedWebpNotSupported)),
            "animated WebP must be rejected regardless of include_metadata"
        );
    }

    /// 不正な APP1 length (< 2) を含む src JPEG をエンコードに通しても、
    /// 出力 JPEG に raw 転写されないことを確認 (Codex P2)。
    #[test]
    fn jpeg_malformed_app1_length_does_not_break_output() {
        // turbojpeg で作った正常 JPEG の SOI 直後に「長さ=0 の APP1」だけ挟む。
        let img = sample_color_image(8, 8);
        let rgba = color_image_to_rgba(&img);
        let rgb = flatten_rgba_to_rgb_black(&rgba, 8);
        let rimg = image::RgbImage::from_raw(8, 8, rgb).unwrap();
        let jpeg = turbojpeg::compress_image(&rimg, 90, turbojpeg::Subsamp::Sub2x2).unwrap();
        let jpeg_bytes = jpeg.as_ref();
        let mut malformed = Vec::with_capacity(jpeg_bytes.len() + 4);
        malformed.extend_from_slice(&jpeg_bytes[..2]); // SOI
        malformed.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x01]); // APP1 length=1 (不正)
        malformed.extend_from_slice(&jpeg_bytes[2..]);
        // この malformed の APP1 を抽出すると、不正長で打ち切られて空 Vec を返すはず。
        let segs = extract_jpeg_app1_segments(&malformed).unwrap();
        assert!(segs.is_empty(), "malformed APP1 should be skipped");
    }

    /// CRC が壊れた tEXt チャンクが出力に転写されないことを確認 (Codex P2)。
    #[test]
    fn png_bad_crc_chunk_is_skipped() {
        // 正常な tEXt + 壊れた CRC の tEXt をそれぞれ作って、抽出側で
        // 正常な方だけ拾われることを見る。
        let img = sample_color_image(8, 8);
        let rgba = color_image_to_rgba(&img);
        let mut bytes: Vec<u8> = Vec::new();
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&rgba, 8, 8, image::ColorType::Rgba8.into())
            .unwrap();

        // 正常 tEXt
        let mut ok_data: Vec<u8> = Vec::new();
        ok_data.extend_from_slice(b"good");
        ok_data.push(0);
        ok_data.extend_from_slice(b"value");
        let mut crc_in: Vec<u8> = b"tEXt".to_vec();
        crc_in.extend_from_slice(&ok_data);
        let ok_crc = png_crc32_compute(&crc_in);
        let mut ok_chunk: Vec<u8> = Vec::new();
        ok_chunk.extend_from_slice(&(ok_data.len() as u32).to_be_bytes());
        ok_chunk.extend_from_slice(b"tEXt");
        ok_chunk.extend_from_slice(&ok_data);
        ok_chunk.extend_from_slice(&ok_crc.to_be_bytes());

        // 壊れた CRC tEXt
        let mut bad_data: Vec<u8> = Vec::new();
        bad_data.extend_from_slice(b"bad");
        bad_data.push(0);
        bad_data.extend_from_slice(b"value");
        let mut bad_chunk: Vec<u8> = Vec::new();
        bad_chunk.extend_from_slice(&(bad_data.len() as u32).to_be_bytes());
        bad_chunk.extend_from_slice(b"tEXt");
        bad_chunk.extend_from_slice(&bad_data);
        bad_chunk.extend_from_slice(&0xDEAD_BEEFu32.to_be_bytes()); // 不正 CRC

        let with_chunks = splice_png_text_chunks(&bytes, &[ok_chunk, bad_chunk]).unwrap();
        let extracted = extract_png_text_chunks_raw(&with_chunks).unwrap();
        assert_eq!(extracted.len(), 1, "only the good CRC chunk should remain");
        // ok_chunk であることを確認 (= "good\0value" を含む)
        let c = &extracted[0];
        let length = u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as usize;
        let data = &c[8..8 + length];
        let null = data.iter().position(|&b| b == 0).unwrap();
        assert_eq!(&data[..null], b"good");
    }

    /// JPEG export 時に EXIF APP1 の Orientation タグが 1 に書き換わることを確認 (Codex P1)。
    #[test]
    fn jpeg_exif_orientation_is_reset_to_1() {
        // EXIF APP1 を手で作る (little-endian TIFF, IFD0 に Orientation=6)。
        let mut exif_payload: Vec<u8> = Vec::new();
        exif_payload.extend_from_slice(b"Exif\x00\x00"); // 6 bytes header
        let tiff_start = exif_payload.len();
        exif_payload.extend_from_slice(b"II"); // little-endian
        exif_payload.extend_from_slice(&0x002Au16.to_le_bytes()); // magic
        exif_payload.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset = 8 (from TIFF start)
        // IFD0 at tiff_start + 8
        exif_payload.extend_from_slice(&1u16.to_le_bytes()); // entry count = 1
        // Entry: tag=0x0112 (Orientation), type=3 (SHORT), count=1, value=6 (rotated 90 CW)
        exif_payload.extend_from_slice(&0x0112u16.to_le_bytes());
        exif_payload.extend_from_slice(&3u16.to_le_bytes());
        exif_payload.extend_from_slice(&1u32.to_le_bytes());
        exif_payload.extend_from_slice(&6u16.to_le_bytes());
        exif_payload.extend_from_slice(&0u16.to_le_bytes()); // padding
        // Next IFD offset = 0
        exif_payload.extend_from_slice(&0u32.to_le_bytes());
        let _ = tiff_start;

        // 上記 payload を APP1 に詰めて src JPEG を作る
        let img = sample_color_image(8, 8);
        let rgba = color_image_to_rgba(&img);
        let rgb = flatten_rgba_to_rgb_black(&rgba, 8);
        let rimg = image::RgbImage::from_raw(8, 8, rgb).unwrap();
        let jpeg = turbojpeg::compress_image(&rimg, 90, turbojpeg::Subsamp::Sub2x2).unwrap();
        let jpeg_bytes = jpeg.as_ref();
        let app1_len = exif_payload.len() + 2;
        let mut src = Vec::with_capacity(jpeg_bytes.len() + 4 + exif_payload.len());
        src.extend_from_slice(&jpeg_bytes[..2]);
        src.push(0xFF);
        src.push(0xE1);
        src.push((app1_len >> 8) as u8);
        src.push((app1_len & 0xFF) as u8);
        src.extend_from_slice(&exif_payload);
        src.extend_from_slice(&jpeg_bytes[2..]);

        // encode 経由で Orientation が 1 に書き換わっているか確認
        let opts = SaveOptions::default();
        let out = encode_jpeg_with_metadata(&img, Some(&src), &opts).unwrap();
        let segs = extract_jpeg_app1_segments(&out).unwrap();
        assert_eq!(segs.len(), 1);
        let seg = &segs[0];
        // APP1 payload[4..10] = "Exif\0\0"
        assert_eq!(&seg[4..10], b"Exif\x00\x00");
        let tiff = 10;
        // IFD0 = tiff + 8
        let ifd0 = tiff + 8;
        let count = u16::from_le_bytes([seg[ifd0], seg[ifd0 + 1]]);
        assert_eq!(count, 1);
        let entry = ifd0 + 2;
        let tag = u16::from_le_bytes([seg[entry], seg[entry + 1]]);
        assert_eq!(tag, 0x0112);
        // value field at entry + 8, little-endian short = 1
        assert_eq!(seg[entry + 8], 0x01);
        assert_eq!(seg[entry + 9], 0x00);
    }

    /// 透過 RGBA を WebP に書き出したとき、`VP8X` が 1 つだけになることを確認 (Codex P1)。
    /// libwebp は alpha 入力に対して extended container (= VP8X + ALPH + VP8/VP8L) を
    /// 出すので、そこにメタデータを追加するとき VP8X を新規挿入してはいけない。
    #[test]
    fn webp_transparent_input_does_not_duplicate_vp8x() {
        // 半透明 RGBA pixel image を encode
        let pixels: Vec<egui::Color32> = (0..16 * 16)
            .map(|i| {
                let a = if i % 2 == 0 { 128 } else { 255 };
                egui::Color32::from_rgba_unmultiplied(200, 100, 50, a)
            })
            .collect();
        let img = ColorImage {
            size: [16, 16],
            pixels,
            source_size: egui::vec2(16.0, 16.0),
        };

        // 元 WebP も透過付きで作って metadata を inject
        let rgba = color_image_to_rgba(&img);
        let raw = webp::Encoder::from_rgba(&rgba, 16, 16).encode(85.0);
        let src_with_meta = inject_webp_metadata(
            raw.as_ref(),
            None,
            None,
            Some(b"<x:xmpmeta>FAKE</x:xmpmeta>".to_vec()),
        )
        .unwrap();
        // 半透明入力 + XMP メタデータの結合で encode
        let opts = SaveOptions::default();
        let out = encode_webp_with_metadata(&img, Some(&src_with_meta), &opts).unwrap();
        // VP8X の出現回数を数える
        let mut vp8x_count = 0;
        let mut pos = 12;
        while pos + 8 <= out.len() {
            if &out[pos..pos + 4] == b"VP8X" {
                vp8x_count += 1;
            }
            let size = u32::from_le_bytes([out[pos + 4], out[pos + 5], out[pos + 6], out[pos + 7]])
                as usize;
            pos += 8 + size + (size & 1);
        }
        assert_eq!(vp8x_count, 1, "VP8X should appear exactly once");
    }

    // ── Codex r2 修正対応の回帰テスト ──

    /// `caller_applied_orientation = false` のとき Orientation が温存される
    /// ことを確認 (ZIP 内 JPEG export 用、Codex r2 P2)。
    #[test]
    fn jpeg_orientation_preserved_when_caller_did_not_apply() {
        // EXIF APP1 with Orientation=6 を作る
        let mut exif_payload: Vec<u8> = Vec::new();
        exif_payload.extend_from_slice(b"Exif\x00\x00");
        exif_payload.extend_from_slice(b"II");
        exif_payload.extend_from_slice(&0x002Au16.to_le_bytes());
        exif_payload.extend_from_slice(&8u32.to_le_bytes());
        exif_payload.extend_from_slice(&1u16.to_le_bytes());
        exif_payload.extend_from_slice(&0x0112u16.to_le_bytes());
        exif_payload.extend_from_slice(&3u16.to_le_bytes());
        exif_payload.extend_from_slice(&1u32.to_le_bytes());
        exif_payload.extend_from_slice(&6u16.to_le_bytes());
        exif_payload.extend_from_slice(&0u16.to_le_bytes());
        exif_payload.extend_from_slice(&0u32.to_le_bytes());

        let img = sample_color_image(8, 8);
        let rgba = color_image_to_rgba(&img);
        let rgb = flatten_rgba_to_rgb_black(&rgba, 8);
        let rimg = image::RgbImage::from_raw(8, 8, rgb).unwrap();
        let jpeg = turbojpeg::compress_image(&rimg, 90, turbojpeg::Subsamp::Sub2x2).unwrap();
        let jpeg_bytes = jpeg.as_ref();
        let app1_len = exif_payload.len() + 2;
        let mut src = Vec::with_capacity(jpeg_bytes.len() + 4 + exif_payload.len());
        src.extend_from_slice(&jpeg_bytes[..2]);
        src.extend_from_slice(&[0xFF, 0xE1]);
        src.push((app1_len >> 8) as u8);
        src.push((app1_len & 0xFF) as u8);
        src.extend_from_slice(&exif_payload);
        src.extend_from_slice(&jpeg_bytes[2..]);

        let opts = SaveOptions {
            caller_applied_orientation: false,
            ..Default::default()
        };
        let out = encode_jpeg_with_metadata(&img, Some(&src), &opts).unwrap();
        let segs = extract_jpeg_app1_segments(&out).unwrap();
        let seg = &segs[0];
        let entry = 10 + 8 + 2;
        // Orientation 値 = 6 (= 温存) のまま
        assert_eq!(seg[entry + 8], 0x06);
        assert_eq!(seg[entry + 9], 0x00);
    }

    /// 不正な Orientation entry (type != SHORT) を書き換えないことを確認 (Codex r2 P3)。
    #[test]
    fn malformed_orientation_entry_is_not_touched() {
        // Orientation tag に type=LONG(4) count=2 を持つ不正 EXIF を作る。
        // value field は offset (= TIFF 内の他箇所を指している) を装う 4 bytes。
        // この offset の中身 (= 0xDEADBEEF をマーカーとして) が **書き換わらない**
        // ことを確認する。
        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(b"Exif\x00\x00");
        payload.extend_from_slice(b"II");
        payload.extend_from_slice(&0x002Au16.to_le_bytes());
        payload.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        payload.extend_from_slice(&1u16.to_le_bytes()); // count
        payload.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        payload.extend_from_slice(&4u16.to_le_bytes()); // type=LONG (not SHORT)
        payload.extend_from_slice(&2u32.to_le_bytes()); // count=2 (不正、Orientation は 1 のはず)
        let bogus_value = 0xDEADBEEFu32.to_le_bytes();
        payload.extend_from_slice(&bogus_value);
        payload.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0
        let snapshot = payload.clone();
        let mut wrapped = Vec::with_capacity(4 + payload.len());
        wrapped.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x00]);
        let len = payload.len() + 2;
        wrapped[2] = (len >> 8) as u8;
        wrapped[3] = (len & 0xFF) as u8;
        wrapped.extend_from_slice(&payload);

        reset_exif_orientation_in_app1(&mut wrapped);
        // payload (= APP1 marker / length を抜いた中身) が変わっていないこと
        assert_eq!(&wrapped[4..], &snapshot[..]);
    }

    /// 透過 VP8L (= 透過 lossless WebP) からメタデータ付き export したとき、
    /// 新規 VP8X に alpha bit (0x10) が立つことを確認 (Codex r2 P2)。
    #[test]
    fn webp_lossless_transparent_sets_alpha_flag_in_new_vp8x() {
        // libwebp lossless で透過 RGBA を encode (= VP8L 単体出力、VP8X 無し)。
        let pixels: Vec<egui::Color32> = (0..16 * 16)
            .map(|i| {
                let a = if i % 2 == 0 { 128 } else { 255 };
                egui::Color32::from_rgba_unmultiplied(200, 100, 50, a)
            })
            .collect();
        let img = ColorImage {
            size: [16, 16],
            pixels,
            source_size: egui::vec2(16.0, 16.0),
        };
        let rgba = color_image_to_rgba(&img);
        let raw = webp::Encoder::from_rgba(&rgba, 16, 16).encode_lossless();
        let raw_bytes = raw.as_ref();
        // 元 src には メタデータが入った状態の WebP を渡す (= XMP 付き)。
        // 元 src 自体は VP8X 無しの VP8L 単体でもよい (この test では src として
        // 同じ raw_bytes を使い、出力に XMP が追加される系を見る)。
        let src_with_xmp = inject_webp_metadata(
            raw_bytes,
            None,
            None,
            Some(b"<x:xmpmeta>FAKE</x:xmpmeta>".to_vec()),
        )
        .unwrap();
        // この出力に VP8X が含まれていること、かつ alpha bit が立っていること
        // を確認する。
        let mut pos = 12;
        let mut found_vp8x = false;
        while pos + 8 <= src_with_xmp.len() {
            let fourcc = &src_with_xmp[pos..pos + 4];
            let size = u32::from_le_bytes([
                src_with_xmp[pos + 4],
                src_with_xmp[pos + 5],
                src_with_xmp[pos + 6],
                src_with_xmp[pos + 7],
            ]) as usize;
            if fourcc == b"VP8X" {
                found_vp8x = true;
                let flags = src_with_xmp[pos + 8];
                assert!(
                    flags & 0x10 != 0,
                    "alpha bit (0x10) should be set on new VP8X, got flags=0x{:02X}",
                    flags
                );
                assert!(flags & 0x04 != 0, "XMP bit (0x04) should be set");
            }
            pos += 8 + size + (size & 1);
        }
        assert!(found_vp8x, "VP8X should be inserted");
    }

    /// `vp8l_has_alpha` の境界条件確認。
    #[test]
    fn vp8l_has_alpha_basic() {
        // signature 0x2f + 4 bytes (alpha bit on byte 4 = 0x10)
        let with_alpha = [0x2f, 0, 0, 0, 0x10];
        let without_alpha = [0x2f, 0, 0, 0, 0x00];
        let too_short = [0x2f, 0];
        let wrong_sig = [0x00, 0, 0, 0, 0x10];
        assert!(vp8l_has_alpha(&with_alpha));
        assert!(!vp8l_has_alpha(&without_alpha));
        assert!(!vp8l_has_alpha(&too_short));
        assert!(!vp8l_has_alpha(&wrong_sig));
    }
}
