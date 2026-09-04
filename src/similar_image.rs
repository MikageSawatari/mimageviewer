//! 「別バージョンの発見」用の正準画像デコードと Proxy 生成。
//!
//! 通常ファイル、ZIP 内の encoded bytes、PDF/サムネイル経路で既に得た raster は
//! すべて [`proxy_from_source`] を通る。永続化や UI はここでは扱わない。

use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use image::{DynamicImage, GenericImageView};

use crate::dupe::Proxy;
use crate::thumb_loader::{
    DctDecodeError, apply_exif_orientation_from_bytes, decode_jpeg_turbo_scaled_from_bytes,
};

/// §17.1 の 400 JPEG と synth 再計測で確定した DCT 縮小デコード目標。
pub const JPEG_DCT_TARGET_EDGE: u32 = 2048;
/// §3.2 / §17.1 の PDF self-check で確定した PDFium render の長辺。
pub const PDF_RENDER_LONG_EDGE: u32 = 1024;

#[derive(Clone, Copy)]
pub enum ProxySource<'a> {
    /// 通常ファイル。`verified_bytes` がある場合は再読込せず、その bytes を正とする。
    File {
        path: &'a Path,
        verified_bytes: Option<&'a [u8]>,
    },
    /// ZIP 等から既に取り出した encoded image。
    Encoded {
        filename_hint: &'a str,
        bytes: &'a [u8],
    },
    /// PDF render 又は正準条件でデコード済みの source raster。
    Raster {
        image: &'a DynamicImage,
        source_dims: (u32, u32),
        format: SimilarImageFormat,
    },
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimilarImageFormat {
    Other = 0,
    Jpeg = 1,
    Png = 2,
    Gif = 3,
    WebP = 4,
    Bmp = 5,
    Tiff = 6,
    Pdf = 7,
}

impl SimilarImageFormat {
    pub fn from_filename(name: &str) -> Self {
        let extension = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match extension.as_str() {
            "jpg" | "jpeg" | "jpe" | "jfif" => Self::Jpeg,
            "png" => Self::Png,
            "gif" => Self::Gif,
            "webp" => Self::WebP,
            "bmp" | "dib" => Self::Bmp,
            "tif" | "tiff" => Self::Tiff,
            "pdf" => Self::Pdf,
            _ => Self::Other,
        }
    }

    pub const fn from_i64(value: i64) -> Self {
        match value {
            1 => Self::Jpeg,
            2 => Self::Png,
            3 => Self::Gif,
            4 => Self::WebP,
            5 => Self::Bmp,
            6 => Self::Tiff,
            7 => Self::Pdf,
            _ => Self::Other,
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Other => "その他",
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Gif => "GIF",
            Self::WebP => "WebP",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
            Self::Pdf => "PDF",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeMethod {
    TurboJpegDct,
    Image,
    Wic,
    Susie,
    Raster,
}

impl DecodeMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TurboJpegDct => "turbojpeg_dct",
            Self::Image => "image_full",
            Self::Wic => "wic_full",
            Self::Susie => "susie_full",
            Self::Raster => "raster",
        }
    }
}

pub struct CanonicalProxy {
    pub proxy: Proxy,
    pub source_dims: (u32, u32),
    pub decoded_dims: (u32, u32),
    pub format: SimilarImageFormat,
    pub method: DecodeMethod,
    pub scale_num: u32,
    pub scale_den: u32,
    pub note: Option<String>,
    pub decode_ms: f64,
    pub proxy_ms: f64,
}

#[derive(Debug)]
pub enum ProxyError {
    Io(std::io::Error),
    Decode(String),
    Cancelled,
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "source read failed: {error}"),
            Self::Decode(error) => write!(formatter, "image decode failed: {error}"),
            Self::Cancelled => write!(formatter, "image decode cancelled"),
        }
    }
}

impl std::error::Error for ProxyError {}

/// ファイル又はその等価な source buffer から `Proxy` を作る唯一の製品経路。
pub fn proxy_from_source(
    source: ProxySource<'_>,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<CanonicalProxy, ProxyError> {
    let started = std::time::Instant::now();
    if is_cancelled(cancel) {
        return Err(ProxyError::Cancelled);
    }

    let mut result = match source {
        ProxySource::Raster {
            image,
            source_dims,
            format,
        } => Ok(finish(
            image,
            source_dims,
            format,
            DecodeMethod::Raster,
            1,
            1,
            None,
        )),
        ProxySource::File {
            path,
            verified_bytes,
        } => {
            let owned;
            let bytes = match verified_bytes {
                Some(bytes) => bytes,
                None => {
                    owned = std::fs::read(path).map_err(ProxyError::Io)?;
                    &owned
                }
            };
            decode_encoded(path.to_string_lossy().as_ref(), Some(path), bytes, cancel)
        }
        ProxySource::Encoded {
            filename_hint,
            bytes,
        } => decode_encoded(filename_hint, None, bytes, cancel),
    }?;
    result.decode_ms = started.elapsed().as_secs_f64() * 1000.0 - result.proxy_ms;
    Ok(result)
}

fn decode_encoded(
    filename_hint: &str,
    file_path: Option<&Path>,
    bytes: &[u8],
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<CanonicalProxy, ProxyError> {
    if is_cancelled(cancel) {
        return Err(ProxyError::Cancelled);
    }
    let format = SimilarImageFormat::from_filename(filename_hint);
    if format == SimilarImageFormat::Jpeg {
        match decode_jpeg_turbo_scaled_from_bytes(bytes, JPEG_DCT_TARGET_EDGE) {
            Ok((image, stats)) => {
                let orientation = crate::thumb_loader::read_exif_orientation_from_bytes(bytes);
                let image = apply_exif_orientation_from_bytes(image, bytes);
                let source_dims = stats.source_dims_after_exif(orientation);
                return Ok(finish(
                    &image,
                    source_dims,
                    format,
                    DecodeMethod::TurboJpegDct,
                    stats.scale_num,
                    8,
                    None,
                ));
            }
            Err(DctDecodeError::TerminalRejection(error)) => {
                return Err(ProxyError::Decode(format!(
                    "terminal JPEG rejection for {filename_hint}: {error}"
                )));
            }
            Err(DctDecodeError::Fallback(error)) => {
                return decode_full(
                    filename_hint,
                    file_path,
                    bytes,
                    format,
                    Some(format!("TurboJPEG fallback: {error}")),
                    cancel,
                );
            }
        }
    }
    decode_full(filename_hint, file_path, bytes, format, None, cancel)
}

fn decode_full(
    filename_hint: &str,
    file_path: Option<&Path>,
    bytes: &[u8],
    format: SimilarImageFormat,
    prefix_note: Option<String>,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<CanonicalProxy, ProxyError> {
    let (image, method, fallback_note) = match image::load_from_memory(bytes) {
        Ok(image) => (image, DecodeMethod::Image, None),
        Err(primary) => {
            if is_cancelled(cancel) {
                return Err(ProxyError::Cancelled);
            }
            let wic = file_path
                .and_then(crate::wic_decoder::decode_to_dynamic_image)
                .or_else(|| crate::wic_decoder::decode_to_dynamic_image_from_bytes(bytes));
            if let Some(image) = wic {
                (
                    image,
                    DecodeMethod::Wic,
                    Some(format!("image crate failed: {primary}")),
                )
            } else {
                if is_cancelled(cancel) {
                    return Err(ProxyError::Cancelled);
                }
                let susie = match file_path {
                    Some(path) => crate::susie_loader::decode_file(path, false, cancel.cloned()),
                    None => crate::susie_loader::decode_bytes(
                        filename_hint,
                        bytes,
                        false,
                        cancel.cloned(),
                    ),
                };
                match susie {
                    Ok(image) => (
                        image,
                        DecodeMethod::Susie,
                        Some(format!("image crate failed: {primary}; WIC failed")),
                    ),
                    Err(susie) => {
                        return Err(ProxyError::Decode(format!(
                            "{filename_hint}: image crate failed ({primary}); WIC failed; Susie failed ({susie})"
                        )));
                    }
                }
            }
        }
    };
    // File / ZIP の区別で向きが変わらないよう、どちらも同じ source bytes を読む。
    let image = apply_exif_orientation_from_bytes(image, bytes);
    let source_dims = image.dimensions();
    let note = match (prefix_note, fallback_note) {
        (Some(prefix), Some(fallback)) => Some(format!("{prefix}; {fallback}")),
        (Some(prefix), None) => Some(prefix),
        (None, fallback) => fallback,
    };
    Ok(finish(&image, source_dims, format, method, 1, 1, note))
}

fn finish(
    image: &DynamicImage,
    source_dims: (u32, u32),
    format: SimilarImageFormat,
    method: DecodeMethod,
    scale_num: u32,
    scale_den: u32,
    note: Option<String>,
) -> CanonicalProxy {
    let rgba = image.to_rgba8();
    let decoded_dims = rgba.dimensions();
    let proxy_started = std::time::Instant::now();
    let mut proxy = crate::dupe::proxy::build(rgba.as_raw(), rgba.width(), rgba.height());
    proxy.src_width = source_dims.0;
    proxy.src_height = source_dims.1;
    CanonicalProxy {
        proxy,
        source_dims,
        decoded_dims,
        format,
        method,
        scale_num,
        scale_den,
        note,
        decode_ms: 0.0,
        proxy_ms: proxy_started.elapsed().as_secs_f64() * 1000.0,
    }
}

fn is_cancelled(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbaImage};
    use std::io::Cursor;

    #[test]
    fn normal_zip_and_pdf_sources_share_the_proxy_builder() {
        let raster = RgbaImage::from_fn(96, 80, |x, y| {
            image::Rgba([(x * 3) as u8, (y * 5) as u8, (x ^ y) as u8, 255])
        });
        let dynamic = DynamicImage::ImageRgba8(raster);
        let mut bytes = Cursor::new(Vec::new());
        dynamic.write_to(&mut bytes, ImageFormat::Png).unwrap();
        let bytes = bytes.into_inner();

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("page.png");
        std::fs::write(&path, &bytes).unwrap();
        let normal = proxy_from_source(
            ProxySource::File {
                path: &path,
                verified_bytes: None,
            },
            None,
        )
        .unwrap();

        let encoded = proxy_from_source(
            ProxySource::Encoded {
                filename_hint: "page.png",
                bytes: &bytes,
            },
            None,
        )
        .unwrap();
        let rendered = proxy_from_source(
            ProxySource::Raster {
                image: &dynamic,
                source_dims: dynamic.dimensions(),
                format: SimilarImageFormat::Pdf,
            },
            None,
        )
        .unwrap();

        assert_eq!(normal.proxy, encoded.proxy);
        assert_eq!(normal.proxy, rendered.proxy);
    }
}
