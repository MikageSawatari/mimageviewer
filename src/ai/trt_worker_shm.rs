//! TensorRT 推論ワーカー用の共有メモリラッパー (Windows 専用)。
//!
//! `CreateFileMappingW` / `OpenFileMappingW` + `MapViewOfFile` で名前付き
//! 共有メモリ領域を確保し、親プロセス・子プロセス両方からバイト列を直接
//! 読み書きできるようにする。
//!
//! 用途: タイル単位推論の入出力テンソル (~MB-tens-of-MB) を IPC で
//! 効率的にやり取りするため。stdout パイプの 64 KB 帯域では帯域不足。
//!
//! ## 安全性
//!
//! 共有メモリは複数プロセスからアクセスされる本質的に unsafe な機構なので、
//! このラッパーは「同時アクセスがない」前提で動く:
//! - 親が書く間、子は触らない (= コマンド送信前)
//! - 子が書く間、親は触らない (= コマンド処理中)
//! - レスポンス受信後に親が読む
//!
//! プロトコル (trt_worker_proto.rs) で順序が決まっているので race にならない。

#![cfg(windows)]

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
    UnmapViewOfFile,
};
use windows::core::PCWSTR;

/// 共有メモリ領域。RAII で Map/Unmap + Close を自動。
///
/// `Send` だが `Sync` ではない (slice の借用が並行でなされない前提)。
pub struct SharedMem {
    handle: HANDLE,
    /// `MapViewOfFile` の戻り値ポインタ。`unmap` で解除。
    ptr: *mut u8,
    size: usize,
    name: String,
}

unsafe impl Send for SharedMem {}

impl SharedMem {
    /// 新しい共有メモリを作成 (親プロセス側で呼ぶ)。
    ///
    /// 同名の領域が既にあったらエラー。`size` バイトの匿名 (file backing なし、
    /// pagefile.sys backing) 領域を確保する。
    pub fn create(name: &str, size: usize) -> io::Result<Self> {
        let wname: Vec<u16> = OsString::from(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let high: u32 = ((size as u64) >> 32) as u32;
        let low: u32 = (size as u64 & 0xFFFF_FFFF) as u32;
        // Safety: 標準的な Win32 API。引数は valid な NULL 終端 wide string と
        // バイト数。返り値は HANDLE もしくはエラー。
        let handle = unsafe {
            CreateFileMappingW(
                windows::Win32::Foundation::INVALID_HANDLE_VALUE,
                None, // SECURITY_ATTRIBUTES = None (default DACL)
                PAGE_READWRITE,
                high,
                low,
                PCWSTR(wname.as_ptr()),
            )
        }
        .map_err(io_err)?;

        if handle.is_invalid() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("CreateFileMappingW returned invalid HANDLE for {name}"),
            ));
        }

        // GetLastError == ERROR_ALREADY_EXISTS (183) は今回は許容しない
        // (同名 shm が既存 = バグ or 衝突)。
        let last_err = unsafe { windows::Win32::Foundation::GetLastError() };
        if last_err.0 == 183 {
            // 名前が衝突 → 既存ハンドルを閉じてエラー
            let _ = unsafe { CloseHandle(handle) };
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("shared memory '{name}' already exists"),
            ));
        }

        let ptr = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size) };
        if ptr.Value.is_null() {
            let err = io::Error::last_os_error();
            let _ = unsafe { CloseHandle(handle) };
            return Err(err);
        }

        Ok(SharedMem {
            handle,
            ptr: ptr.Value as *mut u8,
            size,
            name: name.to_string(),
        })
    }

    /// 既存の共有メモリを開く (子プロセス側で呼ぶ)。
    ///
    /// 親が `create` した後でないと存在しない。タイミング次第で `NotFound` が
    /// 返ることがあるので、呼び出し側はリトライを検討する。
    pub fn open(name: &str, size: usize) -> io::Result<Self> {
        let wname: Vec<u16> = OsString::from(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // Safety: 標準的な Win32 API、name は NULL 終端 wide。
        let handle = unsafe {
            OpenFileMappingW(
                FILE_MAP_ALL_ACCESS.0,
                false,
                PCWSTR(wname.as_ptr()),
            )
        }
        .map_err(io_err)?;

        if handle.is_invalid() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("OpenFileMappingW failed for {name}"),
            ));
        }

        let ptr = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size) };
        if ptr.Value.is_null() {
            let err = io::Error::last_os_error();
            let _ = unsafe { CloseHandle(handle) };
            return Err(err);
        }

        Ok(SharedMem {
            handle,
            ptr: ptr.Value as *mut u8,
            size,
            name: name.to_string(),
        })
    }

    /// バイト列をそのまま書き込む。`bytes.len() <= self.size` でなければ panic。
    pub fn write(&mut self, bytes: &[u8]) {
        assert!(
            bytes.len() <= self.size,
            "shm write overflow: tried {} bytes into {} byte region",
            bytes.len(),
            self.size
        );
        // Safety: ptr は valid mapped view、size 内に収まることを上で確認。
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr, bytes.len());
        }
    }

    /// 共有メモリの先頭 `len` バイトを読む。`len <= self.size` でなければ panic。
    pub fn read_to_vec(&self, len: usize) -> Vec<u8> {
        assert!(
            len <= self.size,
            "shm read overflow: tried {} bytes from {} byte region",
            len,
            self.size
        );
        let mut buf = vec![0u8; len];
        unsafe {
            std::ptr::copy_nonoverlapping(self.ptr, buf.as_mut_ptr(), len);
        }
        buf
    }

    /// 共有メモリの slice として参照する (読み取り)。
    ///
    /// # Safety
    ///
    /// - 同時に他プロセスが書き込んでいないこと
    /// - 返した slice の lifetime 内で `self` が drop されないこと (Rust の
    ///   ライフタイムで保証されるが unsafe ブロックの中の生 slice 化に注意)
    #[allow(dead_code)]
    pub unsafe fn as_slice(&self, len: usize) -> &[u8] {
        assert!(len <= self.size);
        unsafe { std::slice::from_raw_parts(self.ptr, len) }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for SharedMem {
    fn drop(&mut self) {
        // Safety: ptr / handle はコンストラクタで取得済みの valid 値。
        // UnmapViewOfFile / CloseHandle は冪等ではないので必ず 1 回。
        unsafe {
            let _ = UnmapViewOfFile(windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.ptr as *mut std::ffi::c_void,
            });
            let _ = CloseHandle(self.handle);
        }
    }
}

fn io_err(e: windows::core::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("windows: {e}"))
}

// 未使用警告対策 (GENERIC_READ/WRITE はコメント参照のため import している)
#[allow(dead_code)]
const _UNUSED: (windows::Win32::Foundation::GENERIC_ACCESS_RIGHTS, windows::Win32::Foundation::GENERIC_ACCESS_RIGHTS) =
    (GENERIC_READ, GENERIC_WRITE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shm_create_write_read() {
        let name = format!("miv_trt_test_{}_{}", std::process::id(), 0);
        let mut shm = SharedMem::create(&name, 1024).expect("create");
        let payload = b"hello world";
        shm.write(payload);
        let read_back = shm.read_to_vec(payload.len());
        assert_eq!(&read_back, payload);
    }

    #[test]
    fn shm_create_open_same_process() {
        // 同プロセスでも create + open で同じ領域が見えることを確認 (test 用)。
        let name = format!("miv_trt_test_{}_{}", std::process::id(), 1);
        let mut writer = SharedMem::create(&name, 64).expect("create");
        let opener = SharedMem::open(&name, 64).expect("open");

        writer.write(b"ABCDEF");
        let read_back = opener.read_to_vec(6);
        assert_eq!(&read_back, b"ABCDEF");
    }

    #[test]
    fn shm_drop_releases_handle() {
        // create → drop → 同名で create が再度通ることを確認。
        // Drop で UnmapViewOfFile + CloseHandle がちゃんと走っているかの
        // sanity check (リーク検知)。
        let name = format!("miv_trt_test_{}_{}", std::process::id(), 2);
        {
            let _shm = SharedMem::create(&name, 64).expect("first create");
        }
        // 1 回目の drop が完了すれば、同名で再作成できる。
        let _shm2 = SharedMem::create(&name, 64).expect("second create after drop");
    }
}
