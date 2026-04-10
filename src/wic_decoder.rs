//! Windows Imaging Component (WIC) を使った汎用画像デコーダ。
//!
//! `image` クレートが対応していない HEIC/HEIF, AVIF, JPEG XL, RAW (CR2/NEF/ARW/DNG 等)
//! などのフォーマットを Windows のネイティブコーデック経由でデコードする。
//!
//! 必要なコーデックは Microsoft Store から無料インストールできる:
//! - HEIC/HEIF: HEIF Image Extensions
//! - AVIF:      AV1 Video Extensions
//! - JPEG XL:   JPEG XL Image Extensions
//! - RAW:       Raw Image Extension
//!
//! インストールされていないフォーマットは `decode_to_dynamic_image` が `None` を返す。
//!
//! スレッドセーフ: 各呼び出しで独立に COM を初期化するので、複数ワーカースレッドから
//! 並列に呼び出せる。

use std::path::Path;

/// WIC で扱える可能性の高い拡張子 (小文字)。
///
/// このリストに含まれる拡張子は、`image` クレートでデコードに失敗した場合に
/// 自動的に WIC へフォールバックする対象になる。実際にデコードできるかは
/// 対応コーデックがインストールされているかどうかに依存する。
pub const WIC_SUPPORTED_EXTENSIONS: &[&str] = &[
    // モダン形式
    "heic", "heif", "avif", "jxl",
    // TIFF (image クレートも対応するが WIC の方が高機能)
    "tiff", "tif",
    // カメラ RAW (Raw Image Extension が必要)
    "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2",
    "raf", "orf", "rw2", "pef", "ptx", "rwl", "iiq",
];

/// 拡張子が WIC で扱える可能性があるか判定する。
pub fn is_wic_supported_extension(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    WIC_SUPPORTED_EXTENSIONS.contains(&lower.as_str())
}

/// 指定パスの画像ファイルを WIC でデコードして `image::DynamicImage` を返す。
///
/// 戻り値:
/// - `Some(img)`: デコード成功 (フル解像度 RGBA8 で返る)
/// - `None`: コーデック未インストール、ファイル破損、対応外フォーマット等で失敗
///
/// COM は呼び出しごとに初期化・解放するため、ワーカースレッドから自由に呼べる。
/// `S_FALSE` (= 既に初期化済み) も成功として扱う。
pub fn decode_to_dynamic_image(path: &Path) -> Option<image::DynamicImage> {
    #[cfg(not(windows))]
    {
        let _ = path;
        return None;
    }

    #[cfg(windows)]
    unsafe {
        use windows::core::{Interface, GUID, PCWSTR};
        use windows::Win32::Foundation::{GENERIC_READ, S_OK};
        use windows::Win32::Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapFrameDecode,
            IWICBitmapSource, IWICFormatConverter, IWICImagingFactory,
            WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };

        // COM 初期化 (S_OK と S_FALSE の両方を成功として扱う)
        let co_hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let co_initialized = co_hr.is_ok();

        // 内部処理を inner 関数に分離して、結果に関わらず CoUninitialize できるようにする
        let result = decode_inner(path);

        if co_initialized && co_hr == S_OK {
            CoUninitialize();
        }

        return result;

        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe fn decode_inner(path: &Path) -> Option<image::DynamicImage> {
            // ファクトリ生成
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;

            // パスをワイド文字列に変換
            let path_str = path.to_string_lossy();
            let path_wide: Vec<u16> = path_str
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            // デコーダ生成 (ファイル名から)
            let null_guid: GUID = GUID::zeroed();
            let decoder = factory
                .CreateDecoderFromFilename(
                    PCWSTR(path_wide.as_ptr()),
                    Some(&null_guid),
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .ok()?;

            // 最初のフレームを取得 (静止画は 1 フレームのみ。アニメ HEIF/AVIF は最初のフレームのみ)
            let frame: IWICBitmapFrameDecode = decoder.GetFrame(0).ok()?;

            // ピクセルフォーマット変換: 任意の入力 → 32bpp BGRA (straight alpha)
            let converter: IWICFormatConverter = factory.CreateFormatConverter().ok()?;
            let frame_source: IWICBitmapSource = frame.cast().ok()?;
            converter
                .Initialize(
                    &frame_source,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .ok()?;

            // サイズ取得
            let mut width = 0u32;
            let mut height = 0u32;
            converter.GetSize(&mut width, &mut height).ok()?;
            if width == 0 || height == 0 || width > 32768 || height > 32768 {
                return None;
            }

            // ピクセル列を読む
            let stride = width
                .checked_mul(4)?
                .min(u32::MAX);
            let buffer_size = (stride as usize)
                .checked_mul(height as usize)?;
            let mut pixels = vec![0u8; buffer_size];
            converter
                .CopyPixels(std::ptr::null(), stride, &mut pixels)
                .ok()?;

            // BGRA → RGBA に並び替え (R と B を入れ替え)
            for chunk in pixels.chunks_exact_mut(4) {
                chunk.swap(0, 2);
            }

            // image::DynamicImage に組み立て
            let rgba = image::RgbaImage::from_raw(width, height, pixels)?;
            Some(image::DynamicImage::ImageRgba8(rgba))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_wic_supported_extension_lowercase() {
        assert!(is_wic_supported_extension("heic"));
        assert!(is_wic_supported_extension("heif"));
        assert!(is_wic_supported_extension("avif"));
        assert!(is_wic_supported_extension("jxl"));
        assert!(is_wic_supported_extension("dng"));
        assert!(is_wic_supported_extension("cr2"));
        assert!(is_wic_supported_extension("nef"));
        assert!(is_wic_supported_extension("arw"));
    }

    #[test]
    fn is_wic_supported_extension_uppercase() {
        assert!(is_wic_supported_extension("HEIC"));
        assert!(is_wic_supported_extension("Avif"));
        assert!(is_wic_supported_extension("CR2"));
    }

    #[test]
    fn is_wic_supported_extension_negative() {
        assert!(!is_wic_supported_extension("jpg"));
        assert!(!is_wic_supported_extension("png"));
        assert!(!is_wic_supported_extension("webp"));
        assert!(!is_wic_supported_extension("bmp"));
        assert!(!is_wic_supported_extension("gif"));
        assert!(!is_wic_supported_extension("txt"));
        assert!(!is_wic_supported_extension(""));
    }
}
