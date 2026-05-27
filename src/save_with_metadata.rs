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
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            jpeg_quality: 95,
            jpeg_subsampling: turbojpeg::Subsamp::Sub2x2,
            webp_lossless: false,
            webp_quality: 90.0,
            include_metadata: true,
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
    let (w, h) = (pixels.size[0], pixels.size[1]);
    if w == 0 || h == 0 {
        return Err(SaveError::InvalidPixels(format!("size {w}x{h}")));
    }

    // メタデータ抽出元のバイト列を取得 (include_metadata=false なら不要)。
    let src_bytes_owned: Option<Vec<u8>> = if options.include_metadata {
        match (src_bytes, src_path) {
            (Some(b), _) => Some(b.to_vec()),
            (None, Some(p)) => Some(std::fs::read(p).map_err(SaveError::IoError)?),
            (None, None) => None,
        }
    } else {
        None
    };
    let src_bytes_ref: Option<&[u8]> = src_bytes_owned.as_deref();

    let encoded = match &src_format {
        SrcFormat::Jpeg => encode_jpeg_with_metadata(pixels, src_bytes_ref, options)?,
        SrcFormat::Png => encode_png_with_metadata(pixels, src_bytes_ref, options)?,
        SrcFormat::Webp => encode_webp_with_metadata(pixels, src_bytes_ref, options)?,
        SrcFormat::Other(ext) => return Err(SaveError::UnsupportedFormat(ext.clone())),
    };

    // 親ディレクトリ作成 + create_new(true) で書き出し。
    if let Some(parent) = dst_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(SaveError::IoError)?;
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst_path)
        .map_err(SaveError::IoError)?;
    use std::io::Write;
    file.write_all(&encoded).map_err(SaveError::IoError)?;
    file.flush().map_err(SaveError::IoError)?;
    Ok(())
}

/// `dst_path` の親フォルダで `basename` + `_NNNN.<ext>` の最初の空き番号を探して保存する。
/// `seq_start` から `seq_max` まで試し、ファイル名衝突は `create_new(true)` で検出する。
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
    let ext = src_format.extension();
    for seq in seq_start..=seq_max {
        let dst = output_dir.join(format!("{basename}_{seq:04}.{ext}"));
        match save_image_with_metadata(
            pixels,
            src_path,
            src_bytes,
            &dst,
            src_format.clone(),
            options,
        ) {
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
        let app1_segments = extract_jpeg_app1_segments(src)?;
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

/// PNG の tEXt / iTXt / zTXt チャンクを**生バイト**でそのまま取り出す。
/// 各要素は `[length(4) + chunk_type(4) + data + crc(4)]` の生バイト列。
/// 出現順を保ち、IHDR / IDAT / IEND など本体チャンクは除外する。
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
                out.push(png[pos..chunk_end].to_vec());
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
/// 出力 webp crate は通常 VP8 / VP8L 単体の小さな RIFF を返すので、それを解析して
/// 必要に応じて VP8X + メタデータ chunk を追加した新しい RIFF にする。
fn inject_webp_metadata(
    out: &[u8],
    iccp: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
) -> Result<Vec<u8>, SaveError> {
    if out.len() < 12 || &out[..4] != b"RIFF" || &out[8..12] != b"WEBP" {
        return Err(SaveError::EncodingFailed("出力 WebP RIFF 不正".into()));
    }
    // 出力 RIFF のチャンクを解析 (VP8 / VP8L のいずれか 1 つの想定)
    let mut chunks: Vec<(String, Vec<u8>)> = Vec::new();
    let mut canvas: Option<(u32, u32)> = None;
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
        // 出力 RIFF 側の VP8 / VP8L から canvas サイズを取り出す
        if canvas.is_none() {
            if fourcc == "VP8 " {
                canvas = parse_vp8_canvas(&data);
            } else if fourcc == "VP8L" {
                canvas = parse_vp8l_canvas(&data);
            }
        }
        chunks.push((fourcc, data));
        pos = data_end + (size & 1);
    }
    let (cw, ch) = canvas
        .ok_or_else(|| SaveError::EncodingFailed("出力 WebP の canvas サイズ取得失敗".into()))?;

    // VP8X ヘッダを構築 (= chunk 列の先頭)
    let mut flags: u8 = 0;
    if iccp.is_some() {
        flags |= 1 << 5; // ICC profile
    }
    if exif.is_some() {
        flags |= 1 << 3; // EXIF
    }
    if xmp.is_some() {
        flags |= 1 << 2; // XMP
    }
    let mut vp8x = vec![0u8; 10];
    vp8x[0] = flags;
    let w1 = cw - 1;
    let h1 = ch - 1;
    vp8x[4] = (w1 & 0xFF) as u8;
    vp8x[5] = ((w1 >> 8) & 0xFF) as u8;
    vp8x[6] = ((w1 >> 16) & 0xFF) as u8;
    vp8x[7] = (h1 & 0xFF) as u8;
    vp8x[8] = ((h1 >> 8) & 0xFF) as u8;
    vp8x[9] = ((h1 >> 16) & 0xFF) as u8;

    // 順序: VP8X, [ICCP], VP8/VP8L, [EXIF], [XMP]
    let mut new_chunks: Vec<(String, Vec<u8>)> = Vec::new();
    new_chunks.push(("VP8X".to_string(), vp8x));
    if let Some(iccp_data) = iccp {
        new_chunks.push(("ICCP".to_string(), iccp_data));
    }
    for (fc, data) in chunks {
        new_chunks.push((fc, data));
    }
    if let Some(exif_data) = exif {
        new_chunks.push(("EXIF".to_string(), exif_data));
    }
    if let Some(xmp_data) = xmp {
        new_chunks.push(("XMP ".to_string(), xmp_data));
    }

    Ok(rebuild_webp_riff(&new_chunks))
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

// ── 共通 ヘルパー ───────────────────────────────────────────────────

fn color_image_to_rgba(image: &ColorImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.pixels.len() * 4);
    for px in &image.pixels {
        out.extend_from_slice(&[px.r(), px.g(), px.b(), px.a()]);
    }
    out
}

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

    /// PNG CRC32 (= IEEE 802.3 / Ethernet 多項式) を計算する。
    /// テスト fixture 用なので非効率な bit-by-bit 実装で十分。
    fn png_crc32(bytes: &[u8]) -> u32 {
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
        let crc = png_crc32(&crc_input);
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
}
