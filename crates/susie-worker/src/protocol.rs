//! Susie ワーカー IPC のバイナリプロトコル共通コード (worker 側)。
//!
//! フレーム: `[4B msg_len LE][payload]`
//!   payload 先頭 1B が msg_type。

use std::io::{self, Read, Write};

pub const MSG_HANDSHAKE: u8 = 1;
pub const MSG_DECODE_FILE: u8 = 2;
pub const MSG_DECODE_BYTES: u8 = 3;
pub const MSG_SHUTDOWN: u8 = 4;

pub const STATUS_OK: u8 = 0;
pub const STATUS_ERR: u8 = 1;

/// 1 フレームを読み込む。stdin 切断時は Err を返す。
pub fn read_msg<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    // 上限: 256MB。巨大な RAW 画像をメモリ経由で受け取るケースもありうるが、
    // 現実的なレトロ画像は数 MB なので十分大きい。
    if len > 256 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("susie-worker: message too large: {len} bytes"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// 1 フレームを書き出す。
pub fn write_msg<W: Write>(w: &mut W, data: &[u8]) -> io::Result<()> {
    let len = data.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(data)?;
    w.flush()
}
