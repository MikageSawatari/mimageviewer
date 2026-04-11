//! Windows Shell API を使った動画サムネイル取得

use std::mem::size_of;
use std::path::Path;

use eframe::egui;

/// 動画ファイルから Windows Shell API でサムネイルを取得する。
/// `shell_size` は要求する正方形サイズ（px）。
/// COM は関数内でスレッドローカルに初期化し、呼び出し側は何もしなくてよい。
pub fn get_video_thumbnail(path: &Path, shell_size: i32) -> Option<egui::ColorImage> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectA,
        SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT,
    };
    use windows::core::Interface;
    use windows::core::PCWSTR;

    unsafe {
        let _com = crate::wic_decoder::ComScope::init();

        return get_thumbnail_inner(path, shell_size);

        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe fn get_thumbnail_inner(path: &Path, shell_size: i32) -> Option<egui::ColorImage> {
            // パスをワイド文字列に変換（NUL終端付き）
            let path_str = path.to_string_lossy();
            let path_wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();

            // IShellItem を取得
            let item: windows::Win32::UI::Shell::IShellItem =
                SHCreateItemFromParsingName(PCWSTR(path_wide.as_ptr()), None).ok()?;

            // IShellItemImageFactory にキャスト（QI）
            let factory: IShellItemImageFactory = item.cast().ok()?;

            // サムネイル HBITMAP を取得
            let sz = SIZE { cx: shell_size, cy: shell_size };
            let hbmp = factory.GetImage(sz, SIIGBF_RESIZETOFIT).ok()?;

            // HBITMAP のサイズを取得
            let mut bm = BITMAP::default();
            let bm_size = GetObjectA(
                hbmp.into(),
                size_of::<BITMAP>() as i32,
                Some(&mut bm as *mut _ as *mut std::ffi::c_void),
            );
            if bm_size == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
                let _ = DeleteObject(hbmp.into());
                return None;
            }
            let width = bm.bmWidth;
            let height = bm.bmHeight.unsigned_abs() as i32;

            // メモリ DC を作ってビットマップを選択
            let mem_dc = CreateCompatibleDC(None);
            let old_obj = SelectObject(mem_dc, hbmp.into());

            // BITMAPINFO: 32bpp トップダウン RGB
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,   // 負 = トップダウン
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

            // DC を戻してリソースを解放
            SelectObject(mem_dc, old_obj);
            let _ = DeleteDC(mem_dc);
            let _ = DeleteObject(hbmp.into());

            if rows == 0 {
                return None;
            }

            // GDI は BGRA 順で返すので R と B を入れ替え、A を 255 に強制する
            for chunk in pixels.chunks_exact_mut(4) {
                chunk.swap(0, 2); // B ↔ R
                chunk[3] = 255;
            }

            let size = [width as usize, height as usize];
            Some(egui::ColorImage::from_rgba_unmultiplied(size, &pixels))
        }
    }
}
