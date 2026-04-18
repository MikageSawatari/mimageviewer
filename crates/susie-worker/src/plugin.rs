//! Susie 画像プラグイン (`.spi`) のロード・呼び出し。
//!
//! ## API 契約 (Susie Plugin Spec 0.07 / 0.09 image input)
//!
//! すべて `__stdcall` (Rust では `extern "system"`)。
//! 関数名 (LoadLibrary + GetProcAddress で解決):
//!
//! - `GetPluginInfo(infono, buf, buflen) -> int`
//!     - infono=0: API 種別文字列 (`00IN` = image input)
//!     - infono=1: プラグイン名
//!     - infono=2N (N>=1): 第 N フォーマットの拡張子リスト (`*.PI;*.MAG`)
//!     - infono=2N+1: 第 N フォーマットの表示名
//!     - 戻り値: 書き込んだバイト数。0 で列挙終了。
//! - `IsSupported(filename, dw) -> int`
//!     - dw: 先頭 2KB のバイト列を指すポインタ (DWORD に cast された pointer)。
//!     - 戻り値: 0 = 非対応 / 非 0 = 対応。
//! - `GetPicture(buf, len, flag, pHBInfo, pHBm, progress, lData) -> int`
//!     - flag の下位 3 ビット: 0 = buf はファイル名 / 1 = buf はメモリ上のバイナリ。
//!     - len: flag=0 なら 0、flag=1 ならバイト長。
//!     - pHBInfo / pHBm: 出力される LocalAlloc 由来の HANDLE (BITMAPINFO / DIB bits)。
//!     - 戻り値: 0 = 成功。それ以外はエラー。
//!
//! ## クラッシュ耐性
//! このワーカー自体が隔離プロセスなので、プラグインが落ちてもメインプロセスには
//! 影響しない。ここでは panic やエラーを拾って次のプラグインへ進むだけでよい。

use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{FreeLibrary, HANDLE, HLOCAL, HMODULE, LocalFree, FARPROC};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Memory::{LocalLock, LocalUnlock};

// ─────────────────────────────────────────────────────────────────
// C ABI 型
// ─────────────────────────────────────────────────────────────────

type GetPluginInfoFn =
    unsafe extern "system" fn(infono: i32, buf: *mut u8, buflen: i32) -> i32;
type IsSupportedFn =
    unsafe extern "system" fn(filename: *const u8, dw: *const c_void) -> i32;
type GetPictureFn = unsafe extern "system" fn(
    buf: *const c_void,
    len: i32,
    flag: u32,
    hb_info: *mut isize,
    hb_bm: *mut isize,
    progress: FARPROC,
    ldata: isize,
) -> i32;

// ─────────────────────────────────────────────────────────────────
// ロード済みプラグインの情報
// ─────────────────────────────────────────────────────────────────

pub struct PluginInfo {
    pub name: String,
    /// 正規化済み拡張子 (小文字、先頭 `.` なし)。重複排除済み。
    pub extensions: Vec<String>,
}

pub struct LoadedPlugin {
    #[allow(dead_code)]
    pub path: PathBuf,
    pub info: PluginInfo,
    hmod: HMODULE,
    p_is_supported: IsSupportedFn,
    p_get_picture: GetPictureFn,
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.hmod);
        }
    }
}

/// ロード済みプラグインの集合。
#[derive(Default)]
pub struct PluginSet {
    pub plugins: Vec<PluginInfo>,
    /// 実ハンドル (API 呼び出し用)。`plugins` と同じ順。
    loaded: Vec<LoadedPlugin>,
}

impl PluginSet {
    /// 拡張子に一致するプラグインを順に試してデコードする。
    /// 最初に成功したプラグインの BGRA 画像 (top-down) を返す。
    pub fn decode_file(&self, path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        self.decode_bytes(&path.display().to_string(), &bytes)
    }

    pub fn decode_bytes(&self, hint: &str, bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
        let ext = Path::new(hint)
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        // 拡張子マッチ優先、その後は全プラグイン
        let mut order: Vec<usize> = (0..self.loaded.len())
            .filter(|i| self.loaded[*i].info.extensions.iter().any(|e| e == &ext))
            .collect();
        for i in 0..self.loaded.len() {
            if !order.contains(&i) {
                order.push(i);
            }
        }

        let mut last_err: Option<String> = None;
        for i in order {
            match self.loaded[i].decode_bytes(hint, bytes) {
                Ok((w, h, bgra)) => return Ok((w, h, bgra)),
                Err(e) => {
                    last_err = Some(format!("{}: {e}", self.loaded[i].info.name));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| "no plugins available".to_string()))
    }
}

// ─────────────────────────────────────────────────────────────────
// PluginHost — ディレクトリ走査 + 一括ロード
// ─────────────────────────────────────────────────────────────────

pub struct PluginHost {
    pub plugins: PluginSet,
}

impl PluginHost {
    pub fn load_dir(dir: &Path) -> Self {
        let mut loaded: Vec<LoadedPlugin> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase())
                    != Some("spi".to_string())
                {
                    continue;
                }
                match LoadedPlugin::load(&p) {
                    Ok(pl) => loaded.push(pl),
                    Err(_e) => {
                        // プラグインロード失敗はスキップするだけ。詳細はログ非対応で割愛。
                    }
                }
            }
        }
        let infos = loaded
            .iter()
            .map(|pl| PluginInfo {
                name: pl.info.name.clone(),
                extensions: pl.info.extensions.clone(),
            })
            .collect();
        PluginHost {
            plugins: PluginSet {
                plugins: infos,
                loaded,
            },
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// LoadedPlugin の実装
// ─────────────────────────────────────────────────────────────────

impl LoadedPlugin {
    fn load(path: &Path) -> Result<Self, String> {
        // Windows で `.spi` は 32bit DLL。本ワーカー自身が 32bit でビルドされている
        // 前提で LoadLibraryW が成功する。
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let hmod = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }
            .map_err(|e| format!("LoadLibraryW failed: {e}"))?;
        if hmod.is_invalid() {
            return Err("LoadLibraryW returned invalid handle".into());
        }

        let p_get_info = unsafe { GetProcAddress(hmod, pcstr(b"GetPluginInfo\0")) }
            .ok_or_else(|| {
                unsafe { let _ = FreeLibrary(hmod); }
                "GetPluginInfo not found".to_string()
            })?;
        let p_is_supported = unsafe { GetProcAddress(hmod, pcstr(b"IsSupported\0")) }
            .ok_or_else(|| {
                unsafe { let _ = FreeLibrary(hmod); }
                "IsSupported not found".to_string()
            })?;
        let p_get_picture = unsafe { GetProcAddress(hmod, pcstr(b"GetPicture\0")) }
            .ok_or_else(|| {
                unsafe { let _ = FreeLibrary(hmod); }
                "GetPicture not found".to_string()
            })?;

        let get_info: GetPluginInfoFn = unsafe { std::mem::transmute(p_get_info) };
        let is_supported: IsSupportedFn = unsafe { std::mem::transmute(p_is_supported) };
        let get_picture: GetPictureFn = unsafe { std::mem::transmute(p_get_picture) };

        // API 種別確認 (infono=0 が "00IN" でないと画像入力プラグインではない)
        let api = get_plugin_info_string(get_info, 0);
        if !api.starts_with("00IN") {
            unsafe {
                let _ = FreeLibrary(hmod);
            }
            return Err(format!("not an image input plugin (api={api})"));
        }

        let name = get_plugin_info_string(get_info, 1);

        // 拡張子列挙: infono = 2, 4, 6, ... を読み、0 長が返るまで続ける
        let mut extensions = Vec::new();
        let mut n = 1;
        loop {
            let ext_line = get_plugin_info_string(get_info, 2 * n);
            if ext_line.is_empty() {
                break;
            }
            for part in ext_line.split([';', ',', ' ']) {
                let e = part.trim().trim_start_matches('*').trim_start_matches('.');
                if e.is_empty() {
                    continue;
                }
                let e_lower = e.to_ascii_lowercase();
                if !extensions.contains(&e_lower) {
                    extensions.push(e_lower);
                }
            }
            n += 1;
            if n > 64 {
                break; // 異常ケース: 無限ループ回避
            }
        }

        Ok(LoadedPlugin {
            path: path.to_path_buf(),
            info: PluginInfo { name, extensions },
            hmod,
            p_is_supported: is_supported,
            p_get_picture: get_picture,
        })
    }

    fn decode_bytes(&self, hint: &str, bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
        // IsSupported: 先頭 2KB を渡して非 0 が返れば対応。
        let hint_cstr = CString::new(hint).unwrap_or_else(|_| CString::new("?").unwrap());
        let mut header = [0u8; 2048];
        let n = bytes.len().min(2048);
        header[..n].copy_from_slice(&bytes[..n]);

        let supported =
            unsafe { (self.p_is_supported)(hint_cstr.as_ptr() as *const u8, header.as_ptr() as *const c_void) };
        if supported == 0 {
            return Err("not supported by this plugin".into());
        }

        // GetPicture: flag=1 でメモリ渡し
        let mut h_info: isize = 0;
        let mut h_bm: isize = 0;
        let rc = unsafe {
            (self.p_get_picture)(
                bytes.as_ptr() as *const c_void,
                bytes.len() as i32,
                1, // memory pointer
                &mut h_info,
                &mut h_bm,
                FARPROC::default(),
                0,
            )
        };
        if rc != 0 {
            return Err(format!("GetPicture failed rc={rc}"));
        }
        if h_info == 0 || h_bm == 0 {
            return Err("GetPicture returned null handle".into());
        }

        // HANDLE (via LocalAlloc) → ロックしてポインタ取得 → BITMAPINFO / DIB 読み出し
        let result = unsafe { convert_dib(h_info, h_bm) };

        unsafe {
            let _ = LocalFree(Some(HLOCAL(h_info as *mut c_void)));
            let _ = LocalFree(Some(HLOCAL(h_bm as *mut c_void)));
        }

        result
    }
}

// ─────────────────────────────────────────────────────────────────
// ヘルパ
// ─────────────────────────────────────────────────────────────────

/// バイトリテラル (null 終端込み) を PCSTR に変換。
fn pcstr(s: &[u8]) -> windows::core::PCSTR {
    windows::core::PCSTR(s.as_ptr())
}

/// `GetPluginInfo(infono, buf, 256)` を呼び、UTF-8 文字列として返す (非 UTF-8 バイトはロスあり)。
fn get_plugin_info_string(fp: GetPluginInfoFn, infono: i32) -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { fp(infono, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    let slice = &buf[..(n as usize).min(buf.len())];
    // Null 終端で切り詰め
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).into_owned()
}

// ─────────────────────────────────────────────────────────────────
// DIB → BGRA (top-down) 変換
// ─────────────────────────────────────────────────────────────────

/// Susie が返した BITMAPINFO (via LocalAlloc ハンドル) と DIB ビット列を読み、
/// BGRA (8 bit x 4 channel, top-down) に変換する。
///
/// # Safety
/// - `h_info` / `h_bm` は `LocalAlloc` で確保されたハンドルでなければならない。
/// - 呼び出し側で `LocalFree` を行うこと。
unsafe fn convert_dib(h_info: isize, h_bm: isize) -> Result<(u32, u32, Vec<u8>), String> {
    let info_ptr = unsafe { LocalLock(HLOCAL(h_info as *mut c_void)) } as *const u8;
    if info_ptr.is_null() {
        return Err("LocalLock(hInfo) failed".into());
    }
    let bm_ptr = unsafe { LocalLock(HLOCAL(h_bm as *mut c_void)) } as *const u8;
    if bm_ptr.is_null() {
        unsafe {
            let _ = LocalUnlock(HLOCAL(h_info as *mut c_void));
        }
        return Err("LocalLock(hBm) failed".into());
    }

    let result = unsafe { convert_dib_inner(info_ptr, bm_ptr) };

    unsafe {
        let _ = LocalUnlock(HLOCAL(h_info as *mut c_void));
        let _ = LocalUnlock(HLOCAL(h_bm as *mut c_void));
    }
    result
}

unsafe fn convert_dib_inner(
    info_ptr: *const u8,
    bm_ptr: *const u8,
) -> Result<(u32, u32, Vec<u8>), String> {
    // BITMAPINFOHEADER (40 bytes) フィールドを生 pointer から安全に読み出す
    let read_u32 = |off: usize| -> u32 {
        let mut b = [0u8; 4];
        unsafe { std::ptr::copy_nonoverlapping(info_ptr.add(off), b.as_mut_ptr(), 4) };
        u32::from_le_bytes(b)
    };
    let read_i32 = |off: usize| -> i32 {
        let mut b = [0u8; 4];
        unsafe { std::ptr::copy_nonoverlapping(info_ptr.add(off), b.as_mut_ptr(), 4) };
        i32::from_le_bytes(b)
    };
    let read_u16 = |off: usize| -> u16 {
        let mut b = [0u8; 2];
        unsafe { std::ptr::copy_nonoverlapping(info_ptr.add(off), b.as_mut_ptr(), 2) };
        u16::from_le_bytes(b)
    };

    let bi_size = read_u32(0);
    if bi_size < 40 {
        return Err(format!("unexpected bi_size: {bi_size}"));
    }
    let width = read_i32(4);
    let height_raw = read_i32(8);
    let planes = read_u16(12);
    let bit_count = read_u16(14);
    let compression = read_u32(16);
    let clr_used = read_u32(32);

    if planes != 1 {
        return Err(format!("unsupported planes: {planes}"));
    }
    if compression != 0 {
        // BI_RGB 以外 (RLE8/RLE4/BITFIELDS/JPEG/PNG) は未対応。Susie で返してくる
        // プラグインは稀だが、出たら簡易エラーで返して他プラグインに試行させる。
        return Err(format!("unsupported compression: {compression}"));
    }
    if width <= 0 {
        return Err(format!("invalid width: {width}"));
    }
    let w = width as u32;
    let (h, bottom_up) = if height_raw < 0 {
        (height_raw.unsigned_abs(), false)
    } else {
        (height_raw as u32, true)
    };
    if h == 0 {
        return Err("height zero".into());
    }

    // パレット (<=8bpp の場合): BITMAPINFOHEADER の直後に RGBQUAD 配列 (BGR0 x N) が並ぶ。
    let palette_offset = bi_size as usize;
    let palette_count = match bit_count {
        1 => 2,
        4 => 16,
        8 => {
            if clr_used > 0 && clr_used <= 256 {
                clr_used as usize
            } else {
                256
            }
        }
        _ => 0,
    };

    let mut palette: Vec<[u8; 4]> = Vec::with_capacity(palette_count);
    for i in 0..palette_count {
        let base = palette_offset + i * 4;
        let mut quad = [0u8; 4];
        unsafe { std::ptr::copy_nonoverlapping(info_ptr.add(base), quad.as_mut_ptr(), 4) };
        palette.push(quad);
    }

    // 行ストライド (4 バイト境界)
    let stride = ((w as usize * bit_count as usize + 31) / 32) * 4;

    let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];

    for y in 0..h as usize {
        let src_y = if bottom_up { (h as usize - 1) - y } else { y };
        let row_src = unsafe { bm_ptr.add(src_y * stride) };
        let row_dst = &mut bgra[y * (w as usize) * 4..(y + 1) * (w as usize) * 4];

        match bit_count {
            24 => {
                for x in 0..w as usize {
                    let b = unsafe { *row_src.add(x * 3) };
                    let g = unsafe { *row_src.add(x * 3 + 1) };
                    let r = unsafe { *row_src.add(x * 3 + 2) };
                    row_dst[x * 4] = b;
                    row_dst[x * 4 + 1] = g;
                    row_dst[x * 4 + 2] = r;
                    row_dst[x * 4 + 3] = 0xFF;
                }
            }
            32 => {
                // BI_RGB 32bpp: 慣例的に BGR0。アルファが来ていれば使うが 0 なら不透明扱い。
                let mut has_alpha = false;
                for x in 0..w as usize {
                    let b = unsafe { *row_src.add(x * 4) };
                    let g = unsafe { *row_src.add(x * 4 + 1) };
                    let r = unsafe { *row_src.add(x * 4 + 2) };
                    let a = unsafe { *row_src.add(x * 4 + 3) };
                    if a != 0 {
                        has_alpha = true;
                    }
                    row_dst[x * 4] = b;
                    row_dst[x * 4 + 1] = g;
                    row_dst[x * 4 + 2] = r;
                    row_dst[x * 4 + 3] = a;
                }
                if !has_alpha {
                    for x in 0..w as usize {
                        row_dst[x * 4 + 3] = 0xFF;
                    }
                }
            }
            8 => {
                for x in 0..w as usize {
                    let idx = unsafe { *row_src.add(x) } as usize;
                    let quad = palette
                        .get(idx)
                        .copied()
                        .unwrap_or([0, 0, 0, 0]);
                    row_dst[x * 4] = quad[0];
                    row_dst[x * 4 + 1] = quad[1];
                    row_dst[x * 4 + 2] = quad[2];
                    row_dst[x * 4 + 3] = 0xFF;
                }
            }
            4 => {
                for x in 0..w as usize {
                    let byte = unsafe { *row_src.add(x / 2) };
                    let nib = if x % 2 == 0 {
                        (byte >> 4) & 0x0F
                    } else {
                        byte & 0x0F
                    };
                    let quad = palette
                        .get(nib as usize)
                        .copied()
                        .unwrap_or([0, 0, 0, 0]);
                    row_dst[x * 4] = quad[0];
                    row_dst[x * 4 + 1] = quad[1];
                    row_dst[x * 4 + 2] = quad[2];
                    row_dst[x * 4 + 3] = 0xFF;
                }
            }
            1 => {
                for x in 0..w as usize {
                    let byte = unsafe { *row_src.add(x / 8) };
                    let bit = 7 - (x % 8);
                    let idx = ((byte >> bit) & 0x01) as usize;
                    let quad = palette.get(idx).copied().unwrap_or([0, 0, 0, 0]);
                    row_dst[x * 4] = quad[0];
                    row_dst[x * 4 + 1] = quad[1];
                    row_dst[x * 4 + 2] = quad[2];
                    row_dst[x * 4 + 3] = 0xFF;
                }
            }
            other => {
                return Err(format!("unsupported bit_count: {other}"));
            }
        }
    }

    Ok((w, h, bgra))
}

// windows::Win32::Graphics::Gdi::BITMAPINFOHEADER は参考のためインポート不要
// (生ポインタから手動で読んでいるのでレイアウト依存なし)。
#[allow(dead_code)]
fn _unused_keep_handle_type(_: HANDLE) {}

// OsStr → wide 用のインポートに依存するので、`main.rs` でも持っておく。
use std::os::windows::ffi::OsStrExt;
