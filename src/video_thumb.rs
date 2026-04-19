//! Windows Shell API を使った動画サムネイル取得

use std::mem::size_of;
use std::path::Path;
use std::time::Instant;

use eframe::egui;

/// Shell API 呼び出しの診断情報。呼び出し側のログに流し込むための構造体。
pub struct VideoThumbDiag {
    /// GetImage の HRESULT (Ok=0、エラー時は負値)
    pub hresult: i32,
    /// GetImage 呼び出しの所要時間 (ms)
    pub get_image_ms: u32,
    /// 取得できた場合のピクセル寸法 (w, h)
    pub dims: Option<(i32, i32)>,
    /// 取得できた場合のピクセル平均 (R, G, B) — 汎用アイコン/黒プレースホルダ検知用
    pub avg_rgb: Option<(u8, u8, u8)>,
    /// 取得できた場合のピクセル R/G/B の最大 - 最小。< 10 は「ほぼ単色」
    pub span_rgb: Option<(u8, u8, u8)>,
    /// 失敗した場合だけ Some。Some のときは結果の ColorImage は None。
    pub fail_stage: Option<VideoThumbFailStage>,
}

impl VideoThumbDiag {
    pub fn stage_label(&self) -> &'static str {
        match self.fail_stage {
            None => "ok",
            Some(VideoThumbFailStage::CreateItem) => "SHCreateItem-fail",
            Some(VideoThumbFailStage::Cast) => "IShellItemImageFactory-cast-fail",
            Some(VideoThumbFailStage::GetImage) => "GetImage-fail",
            Some(VideoThumbFailStage::GetObject) => "GetObject-fail",
            Some(VideoThumbFailStage::GetDibits) => "GetDIBits-fail",
            Some(VideoThumbFailStage::InvalidBitmap) => "invalid-bitmap-size",
        }
    }

    pub fn hresult_hex(&self) -> String {
        format!("0x{:08x}", self.hresult as u32)
    }
}

pub enum VideoThumbFailStage {
    CreateItem,
    Cast,
    GetImage,
    GetObject,
    GetDibits,
    InvalidBitmap,
}

/// 動画ファイルから Windows Shell API でサムネイルを取得する。
/// `shell_size` は要求する正方形サイズ（px）。
/// COM は関数内でスレッドローカルに初期化し、呼び出し側は何もしなくてよい。
///
/// 呼び出し側でログに出したい診断情報を `diag` に書き込んで返す。
pub fn get_video_thumbnail(
    path: &Path,
    shell_size: i32,
) -> (Option<egui::ColorImage>, VideoThumbDiag) {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GetDIBits, GetObjectA, SelectObject,
    };
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT,
        SIIGBF_THUMBNAILONLY,
    };
    use windows::core::Interface;
    use windows::core::PCWSTR;

    let mut diag = VideoThumbDiag {
        hresult: 0,
        get_image_ms: 0,
        dims: None,
        avg_rgb: None,
        span_rgb: None,
        fail_stage: None,
    };

    unsafe {
        let _com = crate::wic_decoder::ComScope::init();
        let path_str = path.to_string_lossy();
        let path_wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();

        let item: windows::Win32::UI::Shell::IShellItem =
            match SHCreateItemFromParsingName(PCWSTR(path_wide.as_ptr()), None) {
                Ok(it) => it,
                Err(e) => {
                    diag.hresult = e.code().0;
                    diag.fail_stage = Some(VideoThumbFailStage::CreateItem);
                    return (None, diag);
                }
            };

        let factory: IShellItemImageFactory = match item.cast() {
            Ok(f) => f,
            Err(e) => {
                diag.hresult = e.code().0;
                diag.fail_stage = Some(VideoThumbFailStage::Cast);
                return (None, diag);
            }
        };

        // SIIGBF_THUMBNAILONLY: 汎用アイコンにフォールバックせず、本物のサムネだけ返す。
        // 未抽出なら失敗扱い (呼び出し側でリトライ)。
        let sz = SIZE { cx: shell_size, cy: shell_size };
        let t0 = Instant::now();
        let hbmp = match factory.GetImage(sz, SIIGBF_RESIZETOFIT | SIIGBF_THUMBNAILONLY) {
            Ok(h) => {
                diag.get_image_ms = t0.elapsed().as_millis() as u32;
                h
            }
            Err(e) => {
                diag.get_image_ms = t0.elapsed().as_millis() as u32;
                diag.hresult = e.code().0;
                diag.fail_stage = Some(VideoThumbFailStage::GetImage);
                return (None, diag);
            }
        };

        let mut bm = BITMAP::default();
        let bm_size = GetObjectA(
            hbmp.into(),
            size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut _ as *mut std::ffi::c_void),
        );
        if bm_size == 0 {
            let _ = DeleteObject(hbmp.into());
            diag.fail_stage = Some(VideoThumbFailStage::GetObject);
            return (None, diag);
        }
        if bm.bmWidth <= 0 || bm.bmHeight == 0 {
            let _ = DeleteObject(hbmp.into());
            diag.dims = Some((bm.bmWidth, bm.bmHeight));
            diag.fail_stage = Some(VideoThumbFailStage::InvalidBitmap);
            return (None, diag);
        }
        let width = bm.bmWidth;
        let height = bm.bmHeight.unsigned_abs() as i32;
        diag.dims = Some((width, height));

        let mem_dc = CreateCompatibleDC(None);
        let old_obj = SelectObject(mem_dc, hbmp.into());

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let rows = GetDIBits(
            mem_dc,
            hbmp,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut std::ffi::c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(mem_dc, old_obj);
        let _ = DeleteDC(mem_dc);
        let _ = DeleteObject(hbmp.into());

        if rows == 0 {
            diag.fail_stage = Some(VideoThumbFailStage::GetDibits);
            return (None, diag);
        }

        // GDI は BGRA 順で返す。R と B を入れ替え、A を 255 に強制する。
        // 同時にピクセル統計 (avg / span) を計算して診断に載せる。
        let (mut sum_r, mut sum_g, mut sum_b) = (0u64, 0u64, 0u64);
        let (mut min_r, mut min_g, mut min_b) = (255u8, 255u8, 255u8);
        let (mut max_r, mut max_g, mut max_b) = (0u8, 0u8, 0u8);
        let n = (width * height) as u64;
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2);
            chunk[3] = 255;
            let r = chunk[0];
            let g = chunk[1];
            let b = chunk[2];
            sum_r += r as u64;
            sum_g += g as u64;
            sum_b += b as u64;
            min_r = min_r.min(r);
            min_g = min_g.min(g);
            min_b = min_b.min(b);
            max_r = max_r.max(r);
            max_g = max_g.max(g);
            max_b = max_b.max(b);
        }
        diag.avg_rgb = Some(((sum_r / n) as u8, (sum_g / n) as u8, (sum_b / n) as u8));
        diag.span_rgb = Some((max_r - min_r, max_g - min_g, max_b - min_b));

        let size = [width as usize, height as usize];
        (
            Some(egui::ColorImage::from_rgba_unmultiplied(size, &pixels)),
            diag,
        )
    }
}
