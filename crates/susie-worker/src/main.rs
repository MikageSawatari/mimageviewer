//! 32bit Susie 画像プラグイン (`.spi`) を扱うワーカープロセス。
//!
//! メインプロセス (`mimageviewer.exe`, 64bit) が `LoadLibrary` で `.spi` を
//! 直接ロードできないため、このワーカーを 32bit でビルドして子プロセスとして
//! 起動する。通信は stdin/stdout のバイナリプロトコル。
//!
//! ## ライフサイクル
//! 1. 起動直後、`Handshake` を受け取るまでブロック。
//! 2. `Handshake { plugin_dir }` を受けたらプラグインフォルダを走査して `.spi` を
//!    全部 `LoadLibrary` → `GetPluginInfo` で対応拡張子を取得し、応答する。
//! 3. 以降 `DecodeFile` / `DecodeBytes` 要求ごとに、対応しそうな順に
//!    `IsSupported` → `GetPicture` を試す。最初に成功したプラグインの結果を返す。
//! 4. `Shutdown` または stdin クローズで終了。
//!
//! ## プロトコル
//! フレーム: `[4B msg_len LE][1B msg_type][payload]`
//! 詳細は各 `encode_*` / `decode_*` を参照。

#![cfg(windows)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::{c_void, CStr, CString, OsString};
use std::io::{self, Write};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use mimageviewer_susie32::plugin::{PluginHost, PluginSet};
use mimageviewer_susie32::protocol::{
    read_msg, write_msg, MSG_DECODE_BYTES, MSG_DECODE_FILE, MSG_HANDSHAKE, MSG_SHUTDOWN,
    STATUS_ERR, STATUS_OK,
};

fn main() {
    // windows_subsystem="windows" にすると stderr が無効化されるが、worker は
    // 親プロセスから Stdio::null() で拾われるので問題ない。デバッグ時は
    // cargo build (debug profile) でコンソールを有効にしている。

    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();

    // ── フェーズ 1: Handshake 待ち ──
    let plugins: PluginSet = loop {
        let msg = match read_msg(&mut stdin) {
            Ok(m) => m,
            Err(_) => return,
        };
        if msg.is_empty() {
            continue;
        }
        match msg[0] {
            MSG_HANDSHAKE => {
                let plugin_dir = match decode_handshake(&msg[1..]) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = write_err(&mut stdout, &format!("handshake decode: {e}"));
                        continue;
                    }
                };
                let host = PluginHost::load_dir(&plugin_dir);
                let resp = encode_handshake_response(&host);
                if write_msg(&mut stdout, &resp).is_err() {
                    return;
                }
                break host.plugins;
            }
            MSG_SHUTDOWN => return,
            t => {
                let _ = write_err(&mut stdout, &format!("expected handshake, got type {t}"));
            }
        }
    };

    // ── フェーズ 2: デコード要求ループ ──
    loop {
        let msg = match read_msg(&mut stdin) {
            Ok(m) => m,
            Err(_) => return,
        };
        if msg.is_empty() {
            continue;
        }
        match msg[0] {
            MSG_DECODE_FILE => {
                match decode_file_request(&msg[1..]) {
                    Ok(path) => match plugins.decode_file(&path) {
                        Ok((w, h, bgra)) => {
                            let _ = write_msg(&mut stdout, &encode_decode_response(w, h, &bgra));
                        }
                        Err(e) => {
                            let _ = write_err(&mut stdout, &e);
                        }
                    },
                    Err(e) => {
                        let _ = write_err(&mut stdout, &format!("decode_file request: {e}"));
                    }
                }
            }
            MSG_DECODE_BYTES => {
                match decode_bytes_request(&msg[1..]) {
                    Ok((hint, bytes)) => match plugins.decode_bytes(&hint, &bytes) {
                        Ok((w, h, bgra)) => {
                            let _ = write_msg(&mut stdout, &encode_decode_response(w, h, &bgra));
                        }
                        Err(e) => {
                            let _ = write_err(&mut stdout, &e);
                        }
                    },
                    Err(e) => {
                        let _ = write_err(&mut stdout, &format!("decode_bytes request: {e}"));
                    }
                }
            }
            MSG_SHUTDOWN => return,
            t => {
                let _ = write_err(&mut stdout, &format!("unknown msg type {t}"));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// プロトコルのエンコード/デコード (worker 側)
// ─────────────────────────────────────────────────────────────────

/// Handshake ペイロード: `[2B dir_len][utf8 bytes]`
fn decode_handshake(payload: &[u8]) -> Result<PathBuf, String> {
    if payload.len() < 2 {
        return Err("payload too short".into());
    }
    let n = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if payload.len() < 2 + n {
        return Err("payload truncated".into());
    }
    let s = std::str::from_utf8(&payload[2..2 + n]).map_err(|e| format!("utf8: {e}"))?;
    Ok(PathBuf::from(s))
}

/// Handshake 応答: `[1B STATUS_OK][2B plugin_count LE]
///                  [各 plugin: [1B name_len][name][2B ext_count][各 ext: [1B len][ext]]]`
fn encode_handshake_response(host: &PluginHost) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    buf.push(STATUS_OK);
    let count = host.plugins.plugins.len() as u16;
    buf.extend_from_slice(&count.to_le_bytes());
    for pi in &host.plugins.plugins {
        let name = pi.name.as_bytes();
        buf.push(name.len().min(255) as u8);
        buf.extend_from_slice(&name[..name.len().min(255)]);
        let ec = pi.extensions.len() as u16;
        buf.extend_from_slice(&ec.to_le_bytes());
        for ext in &pi.extensions {
            let eb = ext.as_bytes();
            buf.push(eb.len().min(255) as u8);
            buf.extend_from_slice(&eb[..eb.len().min(255)]);
        }
    }
    buf
}

/// DecodeFile ペイロード: `[2B path_len][utf8 bytes]`
fn decode_file_request(payload: &[u8]) -> Result<PathBuf, String> {
    if payload.len() < 2 {
        return Err("payload too short".into());
    }
    let n = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if payload.len() < 2 + n {
        return Err("payload truncated".into());
    }
    let s = std::str::from_utf8(&payload[2..2 + n]).map_err(|e| format!("utf8: {e}"))?;
    Ok(PathBuf::from(s))
}

/// DecodeBytes ペイロード: `[2B hint_len][hint utf8][4B bytes_len][bytes]`
fn decode_bytes_request(payload: &[u8]) -> Result<(String, Vec<u8>), String> {
    if payload.len() < 2 {
        return Err("payload too short".into());
    }
    let n = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if payload.len() < 2 + n + 4 {
        return Err("payload truncated".into());
    }
    let hint = std::str::from_utf8(&payload[2..2 + n])
        .map_err(|e| format!("hint utf8: {e}"))?
        .to_string();
    let rest = &payload[2 + n..];
    let bl = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
    if rest.len() < 4 + bl {
        return Err("bytes truncated".into());
    }
    Ok((hint, rest[4..4 + bl].to_vec()))
}

/// 成功応答: `[1B STATUS_OK][4B w LE][4B h LE][bgra pixels (top-down, w*h*4 bytes)]`
fn encode_decode_response(w: u32, h: u32, bgra: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 4 + 4 + bgra.len());
    buf.push(STATUS_OK);
    buf.extend_from_slice(&w.to_le_bytes());
    buf.extend_from_slice(&h.to_le_bytes());
    buf.extend_from_slice(bgra);
    buf
}

/// エラー応答を書き出すヘルパ。
fn write_err<W: Write>(w: &mut W, msg: &str) -> io::Result<()> {
    let mut buf = Vec::with_capacity(1 + msg.len());
    buf.push(STATUS_ERR);
    buf.extend_from_slice(msg.as_bytes());
    write_msg(w, &buf)
}

// c_void / CStr / CString / OsString / OsStringExt のインポートは plugin サブモジュール内でも
// 使うが、main.rs に並べておくことでリンカエラーで途絶するのを防ぐ (将来の拡張用)。
#[allow(dead_code)]
fn _unused_keep_imports(_: *mut c_void, _: &CStr, _: &CString, _: OsString, _: &Path) {
    let _ = OsString::from_wide(&[0u16]);
}
