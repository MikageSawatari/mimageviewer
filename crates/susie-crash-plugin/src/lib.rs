//! 開発専用の Susie 画像プラグイン。**クラッシュさせるためにある。**
//!
//! Susie ワーカーはプラグインを隔離プロセスで動かすので、プラグインが落ちても本体は
//! 生き残る。しかしその後ワーカーが再生成されるか、後続の要求が別のワーカーで通るかは、
//! 実際に落としてみないと確かめられない。手元に「落ちるプラグイン」が無かったため、
//! この経路はずっと検証されていなかった。
//!
//! ## 挙動の選び方
//!
//! `GetPicture` にはファイル名が渡らない (ワーカーは `flag=1` = メモリ渡しで呼ぶ) ので、
//! **入力ファイルの中身の先頭行**で挙動を決める。ファイル名は自由に付けてよい。
//!
//! | 先頭行 | 挙動 |
//! | --- | --- |
//! | `MIVOK` | 8x8 の単色画像を正常に返す (比較対象。クラッシュ後にこれが通れば復帰している) |
//! | `MIVCRASH` | `GetPicture` の中でアクセス違反 |
//! | `MIVHALF` | 呼ばれるたび 50% でアクセス違反 |
//! | `MIVSUPPORTCRASH` | `IsSupported` の中でアクセス違反 (プラグイン選択の段階で死ぬ) |
//!
//! それ以外の中身は「対応していない画像」として通常のエラーを返す。
//!
//! ## API
//!
//! [`crates/susie-worker/src/plugin.rs`](../../susie-worker/src/plugin.rs) の契約に従う。
//! `__stdcall` かつ**装飾なしの名前**でエクスポートする必要があるので、`plugin.def` で
//! エクスポート名を明示している (これが無いと `_GetPicture@28` になり、ワーカーの
//! `GetProcAddress("GetPicture")` が見つけられない)。

#![cfg(windows)]

use std::ffi::c_void;

// ─────────────────────────────────────────────────────────────────
// 挙動の選択
// ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Behavior {
    Ok,
    Crash,
    CrashHalf,
    CrashInIsSupported,
    Unsupported,
}

/// 先頭行 (最大 64 バイト) を読んで挙動を決める。
fn behavior_from_bytes(bytes: &[u8]) -> Behavior {
    let head = &bytes[..bytes.len().min(64)];
    let line_end = head
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(head.len());
    match head[..line_end].trim_ascii() {
        b"MIVOK" => Behavior::Ok,
        b"MIVCRASH" => Behavior::Crash,
        b"MIVHALF" => Behavior::CrashHalf,
        b"MIVSUPPORTCRASH" => Behavior::CrashInIsSupported,
        _ => Behavior::Unsupported,
    }
}

/// アクセス違反を起こす。`panic!` や `abort()` ではなく本物の AV にするのは、
/// 実際のプラグイン不具合 (不正なポインタ演算) と同じ死に方を再現するため。
/// `abort` だと Rust のランタイムが介在し、SEH の経路が変わる。
fn crash_now() -> ! {
    unsafe {
        // volatile write なので最適化で消えない。
        std::ptr::null_mut::<u8>().write_volatile(1);
    }
    // ここへは到達しない。
    unreachable!("access violation did not fire");
}

/// 呼び出し回数だけを種にした決定的でない 50% 判定。乱数クレートを足さずに、
/// 「同じファイルでも落ちたり落ちなかったりする」状態を作れればよい。
fn crash_half_should_fire() -> bool {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed) % 2 == 1
}

// ─────────────────────────────────────────────────────────────────
// Susie plugin API
// ─────────────────────────────────────────────────────────────────

/// `infono`:
/// - 0: API 種別 (`00IN` = image input)
/// - 1: プラグイン名
/// - 2: 第 1 フォーマットの拡張子リスト
/// - 3: 第 1 フォーマットの表示名
/// - 4 以降: 0 を返して列挙終了
///
/// # Safety
/// `buf` は `buflen` バイト書き込める領域を指していること。
#[unsafe(no_mangle)]
pub unsafe extern "system" fn GetPluginInfo(infono: i32, buf: *mut u8, buflen: i32) -> i32 {
    let text: &[u8] = match infono {
        0 => b"00IN",
        1 => b"mIV crash test plugin (development only)",
        2 => b"*.miv-crashtest",
        3 => b"mIV crash test",
        _ => return 0,
    };
    if buf.is_null() || buflen <= 0 {
        return 0;
    }
    // 終端の NUL を入れるため 1 バイト残す。
    let n = text.len().min(buflen as usize - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
        buf.add(n).write(0);
    }
    n as i32
}

/// `dw` は先頭 2KB のバイト列を指す (ワーカーは常にメモリ渡しで呼ぶ)。
///
/// # Safety
/// `dw` は 2KB 読み出せる領域を指していること。`filename` は NUL 終端文字列。
#[unsafe(no_mangle)]
pub unsafe extern "system" fn IsSupported(_filename: *const u8, dw: *const c_void) -> i32 {
    if dw.is_null() {
        return 0;
    }
    // 先頭行の判定に必要な分だけ読む。2KB 未満のファイルでもワーカー側が
    // 2KB のバッファを渡すため、この範囲の読み出しは安全。
    let head = unsafe { std::slice::from_raw_parts(dw as *const u8, 64) };
    match behavior_from_bytes(head) {
        Behavior::Unsupported => 0,
        Behavior::CrashInIsSupported => crash_now(),
        _ => 1,
    }
}

/// 成功時は `hb_info` に BITMAPINFOHEADER、`hb_bm` に DIB ビット列の
/// `LocalAlloc` ハンドルを返す。
///
/// # Safety
/// `buf` は `len` バイト読み出せる領域 (flag=1 のとき)。
/// `hb_info` / `hb_bm` は書き込み可能なポインタであること。
#[unsafe(no_mangle)]
pub unsafe extern "system" fn GetPicture(
    buf: *const c_void,
    len: i32,
    flag: u32,
    hb_info: *mut isize,
    hb_bm: *mut isize,
    _progress: *const c_void,
    _ldata: isize,
) -> i32 {
    // 下位 3 ビットが 0 ならファイル名渡し。ワーカーは常に 1 (メモリ) で呼ぶので、
    // ファイル名経路は「対応外」として素直に失敗させる。
    if flag & 0x07 != 1 || buf.is_null() || len <= 0 {
        return 1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, len as usize) };
    match behavior_from_bytes(bytes) {
        Behavior::Crash | Behavior::CrashInIsSupported => crash_now(),
        Behavior::CrashHalf => {
            if crash_half_should_fire() {
                crash_now();
            }
            unsafe { write_solid_image(hb_info, hb_bm) }
        }
        Behavior::Ok => unsafe { write_solid_image(hb_info, hb_bm) },
        Behavior::Unsupported => 1,
    }
}

// ─────────────────────────────────────────────────────────────────
// 正常系の出力
// ─────────────────────────────────────────────────────────────────

const IMAGE_SIDE: usize = 8;
const BITMAPINFOHEADER_SIZE: usize = 40;

/// 8x8 24bit (BI_RGB, bottom-up) の単色画像を `LocalAlloc` で返す。
///
/// stride は 4 バイト境界に揃える必要がある。8 * 3 = 24 は既に 4 の倍数なので
/// パディングは要らないが、式は一般形のまま書いておく。
///
/// # Safety
/// `hb_info` / `hb_bm` は書き込み可能なポインタであること。
unsafe fn write_solid_image(hb_info: *mut isize, hb_bm: *mut isize) -> i32 {
    use windows::Win32::System::Memory::{LMEM_FIXED, LocalAlloc};

    if hb_info.is_null() || hb_bm.is_null() {
        return 1;
    }
    let stride = (IMAGE_SIDE * 24).div_ceil(32) * 4;
    let bits_len = stride * IMAGE_SIDE;

    // ワーカーは受け取ったハンドルを LocalFree する契約なので、ここでは解放しない。
    let Ok(info) = (unsafe { LocalAlloc(LMEM_FIXED, BITMAPINFOHEADER_SIZE) }) else {
        return 1;
    };
    let Ok(bits) = (unsafe { LocalAlloc(LMEM_FIXED, bits_len) }) else {
        return 1;
    };
    if info.is_invalid() || bits.is_invalid() {
        return 1;
    }

    unsafe {
        let p = info.0 as *mut u8;
        std::ptr::write_bytes(p, 0, BITMAPINFOHEADER_SIZE);
        let put_u32 = |off: usize, v: u32| p.add(off).cast::<u32>().write_unaligned(v);
        let put_i32 = |off: usize, v: i32| p.add(off).cast::<i32>().write_unaligned(v);
        let put_u16 = |off: usize, v: u16| p.add(off).cast::<u16>().write_unaligned(v);
        put_u32(0, BITMAPINFOHEADER_SIZE as u32); // biSize
        put_i32(4, IMAGE_SIDE as i32); // biWidth
        put_i32(8, IMAGE_SIDE as i32); // biHeight (正 = bottom-up)
        put_u16(12, 1); // biPlanes
        put_u16(14, 24); // biBitCount
        put_u32(16, 0); // biCompression = BI_RGB

        // BGR 順で一様に塗る。中身が何色かは問わないが、目視で分かる色にしておく。
        let p_bits = bits.0 as *mut u8;
        for row in 0..IMAGE_SIDE {
            for col in 0..IMAGE_SIDE {
                let at = p_bits.add(row * stride + col * 3);
                at.write(0x30); // B
                at.add(1).write(0xC0); // G
                at.add(2).write(0x30); // R
            }
        }

        hb_info.write(info.0 as isize);
        hb_bm.write(bits.0 as isize);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_line_selects_the_behavior() {
        assert_eq!(behavior_from_bytes(b"MIVOK\n...."), Behavior::Ok);
        assert_eq!(behavior_from_bytes(b"MIVCRASH\r\n"), Behavior::Crash);
        assert_eq!(behavior_from_bytes(b"MIVHALF"), Behavior::CrashHalf);
        assert_eq!(
            behavior_from_bytes(b"MIVSUPPORTCRASH\n"),
            Behavior::CrashInIsSupported
        );
    }

    #[test]
    fn anything_else_is_declined_rather_than_crashing() {
        assert_eq!(behavior_from_bytes(b""), Behavior::Unsupported);
        assert_eq!(behavior_from_bytes(b"\x89PNG\r\n"), Behavior::Unsupported);
        assert_eq!(behavior_from_bytes(b"MIVOKAY"), Behavior::Unsupported);
    }

    #[test]
    fn trailing_spaces_do_not_change_the_selection() {
        assert_eq!(behavior_from_bytes(b"MIVOK  \n"), Behavior::Ok);
        assert_eq!(behavior_from_bytes(b"  MIVCRASH\n"), Behavior::Crash);
    }

    /// 50% 判定は「同じ入力でも落ちたり落ちなかったりする」ことが要件なので、
    /// 連続呼び出しで両方の答えが出ることだけを固定する。
    #[test]
    fn the_half_decision_alternates_rather_than_always_agreeing() {
        let first = crash_half_should_fire();
        let second = crash_half_should_fire();
        assert_ne!(first, second);
    }
}
