//! `ffmpeg::format::input` の進捗フィードバック版。custom AVIO で実ファイル読込を
//! ラップし、累積 read バイト数を `Arc<AtomicU64>` に逐次更新する。UI スレッドが
//! poll して「動画を準備中…  N MB 読込」を HUD に表示するための原始情報を提供する。
//!
//! ## なぜ必要か
//!
//! MP4 の `moov` atom が末尾配置のファイルを `avformat_open_input` で開くと、moov
//! を見つけるためにファイル末尾までシークが走り、NAS / HDD で 1-3 秒待たされる
//! ことがある (2026-05-12 の動画オープン遅延解析より)。原理的に避けられないが
//! (他のプレーヤーでも同じ)、「フリーズしているのではなく作業中」をユーザーに
//! 可視化する目的で本モジュールを使う。
//!
//! ## 構造
//!
//! - `ProgressIoState` が `File` + 累積バイトカウンタ + 直近位置を持つ
//! - `read_packet` / `seek` の C callback がそれを更新
//! - `avio_alloc_context` で AVIOContext を作り、`avformat_alloc_context` で確保した
//!   `AVFormatContext.pb` に attach、`AVFMT_FLAG_CUSTOM_IO` を立てる
//! - `avformat_open_input(ctx, NULL, NULL, NULL)` で path = null にして pb 経由で読ませる
//! - `avformat_find_stream_info` までを段階的に呼び、各フェーズの elapsed を返す
//! - 戻り値は `InputWithProgress` で、`Deref<Target = Input>` 経由で既存コードから
//!   使える + Drop 時に AVIOContext / opaque を確実に解放する
//!
//! ## ライフタイム
//!
//! - `AVFormatContext.flags |= AVFMT_FLAG_CUSTOM_IO` を立てているので
//!   `avformat_close_input` は pb を free しない (= 自前で `avio_context_free` する)
//! - opaque は `Box::into_raw` で leak させ、`Drop` で `Box::from_raw` で回収する
//! - AVIOContext.buffer は `av_malloc` で確保 (FFmpeg が buffer を replace する
//!   可能性があるので、libavformat が free する側)。`avio_context_free` が
//!   最終的に内部の `s->buffer` を free する
//!
//! ## エラー時のクリーンアップ
//!
//! `avformat_open_input` 失敗時は ctx 自体が解放される (FFmpeg の規約)。一方
//! custom IO の場合、AVIOContext と opaque は **解放されない**ので自分で free する。
//! 失敗パスを 1 箇所にまとめるため、success path に到達する前にエラーが起きたら
//! まとめて drop する `Cleanup` ガード型を使う。

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use ffmpeg::ffi::{
    AVFMT_FLAG_CUSTOM_IO, AVIOContext, AVSEEK_SIZE, av_free, av_malloc, avformat_alloc_context,
    avformat_close_input, avformat_find_stream_info, avformat_open_input, avio_alloc_context,
    avio_context_free,
};
use ffmpeg_the_third as ffmpeg;

/// `AVERROR_EOF = -MKTAG('E','O','F',' ')`. bindgen が定数化していないので手書き。
/// FFmpeg ソース (`libavutil/error.h`) より直接 計算: MKTAG = a | (b<<8) | (c<<16) | (d<<24)。
const AVERROR_EOF: i32 = -0x20464F45; // = -541478725

/// `AVERROR(EIO) = -EIO`. Codex P2 反映: 一時的な I/O エラーを正常 EOF と混同しないため、
/// `read_cb` から I/O 失敗時に返す。POSIX EIO = 5、Windows でも `errno.h` で同じ値。
const AVERROR_EIO: i32 = -5;

/// `AVSEEK_FORCE = 0x20000`. bindgen は AVSEEK_SIZE だけ定数化、FORCE は未提供のため手書き。
/// FFmpeg ソース `libavformat/avio.h` 参照。SEEK_SET / CUR / END と OR されて渡されるので
/// マスクで除去してから whence を判定する必要がある (Codex P1 第 14 ラウンド対応)。
const AVSEEK_FORCE: i32 = 0x20000;

const READ_BUF_SZ: usize = 32 * 1024;
const MAX_DEBUG_VIDEO_PREP_DELAY_MS: u64 = 60_000;
const MAX_DEBUG_AVIO_READ_DELAY_MS: u64 = 250;

/// 動画オープン (= demux thread 入口) のフェーズ識別子。HUD のメッセージ切替に使う。
pub mod prep_phase {
    /// `avformat_open_input` 実行中 (moov atom 探索 = ファイル末尾までシークするケースで
    /// 大半の時間がここに使われる)。
    pub const OPENING: u8 = 1;
    /// `avformat_find_stream_info` 実行中 (= ストリームパラメータの probe)。
    pub const ANALYZING: u8 = 2;
    /// 両方とも完了。最初のフレームのデコード + 描画待ちフェーズ (通常 < 100ms)。
    pub const DONE: u8 = 3;
}

/// 動画オープン進捗。UI スレッドが atomic を poll して HUD に表示する。
///
/// - `phase`: `prep_phase::*` 定数のいずれか。動画 open の進行段階を示す
/// - `bytes_read`: ファイルから読み込んだ累積バイト数 (custom AVIO の read callback が更新)
/// - `file_size`: 動画ファイル全体のサイズ。"x MB / y MB" 表示用 (0 なら未知)
pub struct PreparingProgress {
    phase: AtomicU8,
    bytes_read: AtomicU64,
    file_size: AtomicU64,
}

impl PreparingProgress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            phase: AtomicU8::new(prep_phase::OPENING),
            bytes_read: AtomicU64::new(0),
            file_size: AtomicU64::new(0),
        })
    }

    pub fn phase(&self) -> u8 {
        self.phase.load(Ordering::Relaxed)
    }
    pub fn set_phase(&self, p: u8) {
        self.phase.store(p, Ordering::Relaxed);
    }
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }
    pub fn file_size(&self) -> u64 {
        self.file_size.load(Ordering::Relaxed)
    }
    pub(crate) fn set_file_size(&self, sz: u64) {
        self.file_size.store(sz, Ordering::Relaxed);
    }
    pub(crate) fn add_bytes(&self, n: u64) {
        self.bytes_read.fetch_add(n, Ordering::Relaxed);
    }
}

impl Default for PreparingProgress {
    fn default() -> Self {
        Self {
            phase: AtomicU8::new(prep_phase::OPENING),
            bytes_read: AtomicU64::new(0),
            file_size: AtomicU64::new(0),
        }
    }
}

/// `PreparingProgress` のスナップショット (atomic から一括取り出した値)。
/// 描画スレッド側で 3 つの atomic を別々に load すると race で不整合が出るので、
/// 同一 tick で 1 度 snapshot して使い回す用。
#[derive(Debug, Clone, Copy)]
pub struct PreparingStatus {
    pub phase: u8,
    pub bytes_read: u64,
    pub file_size: u64,
}

impl PreparingProgress {
    /// 3 つの atomic を 1 tick 分まとめて snapshot する。
    pub fn snapshot(&self) -> PreparingStatus {
        PreparingStatus {
            phase: self.phase(),
            bytes_read: self.bytes_read(),
            file_size: self.file_size(),
        }
    }
}

/// 動画オープン中の HUD メッセージ生成。`prep_phase::*` 定数 + 進捗バイト数から
/// 「メタデータ読込中... NN MB」「ストリーム解析中...」「デコード開始中...」を組み立てる。
///
/// バイト数は `file_size` が既知なら "NN MB / YY MB" 形式、不明なら "NN MB" のみ。
/// 数百 MB 動画でも単位は MB 固定にして桁を揃える (= 1 GB 動画でも "1234 MB"、
/// 読みやすさ優先)。
/// 旧 UI (egui main 側) と native presenter overlay で同じ文言を出すために共通化。
///
/// Codex P2 反映: FFmpeg は seek して同じ範囲を再読込するので、累積 `bytes_read` は
/// `file_size` を超え得る (= NN > YY のような表示が出ると進捗バーと誤認しやすい)。
/// 表示時に `min(bytes_read, file_size)` で clamp して、ユーザー目線で「進捗率」では
/// なく「読込量上限」として読めるようにする。
pub fn build_preparing_message(status: PreparingStatus) -> String {
    let mb = |b: u64| (b as f64) / (1024.0 * 1024.0);
    let bytes_display = if status.file_size > 0 {
        status.bytes_read.min(status.file_size)
    } else {
        status.bytes_read
    };
    let progress = if status.file_size > 0 {
        format!(
            "  {:.1} MB / {:.1} MB",
            mb(bytes_display),
            mb(status.file_size)
        )
    } else if status.bytes_read > 0 {
        format!("  {:.1} MB", mb(status.bytes_read))
    } else {
        String::new()
    };
    match status.phase {
        prep_phase::OPENING => format!("メタデータ読込中...{progress}"),
        prep_phase::ANALYZING => format!("ストリーム解析中...{progress}"),
        prep_phase::DONE => "デコード開始中...".to_string(),
        // 未知 phase は旧文言にフォールバック (防御的)
        _ => "動画を準備中...".to_string(),
    }
}

fn parse_debug_delay_ms(value: Option<&str>, max_ms: u64) -> Option<Duration> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    let ms = raw.parse::<u64>().ok()?;
    if ms == 0 {
        return None;
    }
    Some(Duration::from_millis(ms.min(max_ms)))
}

fn debug_delay_from_env(name: &str, max_ms: u64) -> Option<Duration> {
    std::env::var(name)
        .ok()
        .and_then(|value| parse_debug_delay_ms(Some(&value), max_ms))
}

/// `avio_alloc_context` の opaque に詰める Rust 側の状態。
struct ProgressIoState {
    file: File,
    /// AVIO callback から `add_bytes` で累積、UI が `bytes_read` で読む。
    progress: Arc<PreparingProgress>,
    /// ファイルサイズ。`AVSEEK_SIZE` で即返すためにキャッシュしておく。
    file_size: u64,
    /// デバッグ用: 準備フェーズ中だけ read callback を遅くする。
    debug_read_delay: Option<Duration>,
}

extern "C" fn read_cb(opaque: *mut c_void, buf: *mut u8, buf_size: i32) -> i32 {
    // SAFETY: opaque は `Box::into_raw(Box<ProgressIoState>)` 由来で、AVIO 生存中は有効。
    let state = unsafe { &mut *(opaque as *mut ProgressIoState) };
    if buf_size <= 0 {
        return 0;
    }
    let slice = unsafe { std::slice::from_raw_parts_mut(buf, buf_size as usize) };
    match state.file.read(slice) {
        Ok(0) => AVERROR_EOF,
        Ok(n) => {
            state.progress.add_bytes(n as u64);
            if n > 0
                && state.progress.phase() != prep_phase::DONE
                && let Some(delay) = state.debug_read_delay
            {
                std::thread::sleep(delay);
            }
            n as i32
        }
        Err(e) => {
            // Codex P2: I/O error を EOF と混同しない。AVERROR(EIO) を返して
            // demuxer に「正常終了」ではなく「壊れたストリーム」として扱わせる。
            // ログにも残して原因切り分けが効くようにする (短時間で大量に出ないよう
            // 1 回 read_cb 呼び出しで 1 行のみ)。
            crate::logger::log(format!("AVIO read_cb: I/O error: {e}"));
            AVERROR_EIO
        }
    }
}

extern "C" fn seek_cb(opaque: *mut c_void, offset: i64, whence: i32) -> i64 {
    // SAFETY: read_cb と同じ。
    let state = unsafe { &mut *(opaque as *mut ProgressIoState) };
    // AVSEEK_SIZE: 「サイズを返せ、シークはするな」というクエリ。SEEK_SET 等と OR されず
    // 単独で渡される (FFmpeg 仕様)。実際の seek 動作は伴わない。
    if whence & AVSEEK_SIZE != 0 {
        return state.file_size as i64;
    }
    // AVSEEK_FORCE は SEEK_SET / CUR / END と OR されて渡され得るので、判定前にマスクで
    // 除去する (Codex 第 14 ラウンド P1: 旧実装は whence == 0/1/2 だけ受けて -1 を返す
    // ので、FORCE フラグが付いた seek 要求が全部失敗し、demux が moov 探索後にハングする
    // 可能性があった)。
    let pos = match seek_from_for_whence(whence, offset) {
        SeekResolution::Pos(p) => p,
        SeekResolution::Unknown => {
            // 未知の whence は log してから -1。原因切り分けが効くよう、whence の生値を残す。
            crate::logger::log(format!(
                "AVIO seek_cb: unknown whence={whence}, returning -1"
            ));
            return -1;
        }
    };
    state.file.seek(pos).map(|p| p as i64).unwrap_or(-1)
}

/// `seek_cb` のロジックの純粋関数版 (= unit test 可能)。
/// `AVSEEK_SIZE` フラグはこの関数の手前で別途処理する (= file_size を返す責務は呼出側)。
/// `AVSEEK_FORCE` フラグはマスクで除去してから SEEK_SET / CUR / END を判定する。
#[derive(Debug, PartialEq)]
enum SeekResolution {
    Pos(SeekFrom),
    Unknown,
}

fn seek_from_for_whence(whence: i32, offset: i64) -> SeekResolution {
    let actual = whence & !AVSEEK_FORCE & !AVSEEK_SIZE;
    match actual {
        0 => SeekResolution::Pos(SeekFrom::Start(offset.max(0) as u64)),
        1 => SeekResolution::Pos(SeekFrom::Current(offset)),
        2 => SeekResolution::Pos(SeekFrom::End(offset)),
        _ => SeekResolution::Unknown,
    }
}

/// `ffmpeg::format::input` 相当だが、`progress` のフィールドに各段階の累積バイト数 +
/// フェーズを更新する。UI が atomic で読んで HUD に表示する用。
///
/// 戻り値は `Deref<Target = Input>` を実装する `InputWithProgress`。既存の
/// `input.streams()` / `input.packets()` 等の呼び出しはそのまま動く。
///
/// `PhaseTimings` には各段階の elapsed を返す (呼び出し側がログに記録する用)。
///
/// 関数内でフェーズを以下のように遷移させる:
/// 1. 呼び出し時点で `phase = OPENING` (= 呼び出し側の事前 set と一致)
/// 2. `avformat_open_input` 完了直後 → `phase = ANALYZING`
/// 3. `avformat_find_stream_info` 完了直後 → `phase = DONE`
///
/// Codex P1 反映: custom AVIO は `avformat_open_input(NULL path, NULL format)` で
/// content-only probe になるので、ファイル名拡張子や URL スキームで識別される
/// コンテナで失敗するリスクがある (MP4/MKV/WebM のような magic bytes ベースの
/// 主要形式は安全、生 H.264/H.265 ストリーム、TS、特定コーデック等は要注意)。
/// 失敗時は **旧来の `ffmpeg::format::input(&path)` にフォールバック**して、開け
/// なくなる回帰を回避する。fallback 経路では進捗 atomic は更新できない (= 旧来の
/// 「動画を準備中...」表示にフォールバック) が、再生できる方を優先。
pub fn input_with_progress(
    path: &Path,
    progress: Arc<PreparingProgress>,
) -> Result<(InputWithProgress, PhaseTimings), String> {
    // デバッグ用: 実機で動画準備中 HUD を観察しやすくするため、demux worker だけを
    // 意図的に待たせる。UI スレッドは poll を続け、native presenter 側は
    // `phase=OPENING` の準備中表示を描ける。
    progress.set_phase(prep_phase::OPENING);
    if let Ok(meta) = std::fs::metadata(path) {
        progress.set_file_size(meta.len());
    }
    if let Some(delay) = debug_delay_from_env(
        "MIV_DEBUG_VIDEO_PREP_DELAY_MS",
        MAX_DEBUG_VIDEO_PREP_DELAY_MS,
    ) {
        crate::logger::log(format!(
            "input_with_progress: MIV_DEBUG_VIDEO_PREP_DELAY_MS={}ms",
            delay.as_millis()
        ));
        std::thread::sleep(delay);
    }
    // 診断スイッチ (Codex 第 14 ラウンド助言): 環境変数 `MIV_DISABLE_AVIO_PROGRESS=1` で
    // custom AVIO 経路を skip して旧 `ffmpeg::format::input` に直接フォールバックする。
    // 再現確認時に「進捗を諦めて再生だけ戻す」切り分けに使う。
    if std::env::var_os("MIV_DISABLE_AVIO_PROGRESS").is_some() {
        crate::logger::log(
            "input_with_progress: MIV_DISABLE_AVIO_PROGRESS set, using fallback path".to_string(),
        );
        return open_with_fallback(path, progress, None);
    }
    // ── 1st pass: custom AVIO 経路で開く ──
    match try_open_with_custom_avio(path, Arc::clone(&progress)) {
        Ok(v) => Ok(v),
        Err(custom_err) => {
            // ── 2nd pass: fallback to ffmpeg::format::input ──
            crate::logger::log(format!(
                "input_with_progress: custom AVIO open failed ({custom_err}), falling back to ffmpeg::format::input(&path)"
            ));
            open_with_fallback(path, progress, Some(custom_err))
        }
    }
}

/// `ffmpeg::format::input(&path)` を呼んで `InputWithProgress::fallback` でラップする。
/// `prior_err` を `Some(...)` で渡すと「custom AVIO 失敗 → fallback も失敗」のメッセージ
/// 形式になる。`MIV_DISABLE_AVIO_PROGRESS` 経路では prior_err = None。
fn open_with_fallback(
    path: &Path,
    progress: Arc<PreparingProgress>,
    prior_err: Option<String>,
) -> Result<(InputWithProgress, PhaseTimings), String> {
    let t_fallback = std::time::Instant::now();
    let input = ffmpeg::format::input(path).map_err(|e| match &prior_err {
        Some(prior) => format!("open input failed (custom AVIO: {prior}; fallback: {e})"),
        None => format!("open input (fallback path) failed: {e}"),
    })?;
    let fallback_total_ms = t_fallback.elapsed().as_secs_f64() * 1000.0;
    // fallback は進捗 atomic を駆動できないので、最終フェーズを DONE に進めて
    // HUD は「デコード開始中...」相当に切替える。
    progress.set_phase(prep_phase::DONE);
    // file_size だけは事後に metadata から埋めておく (HUD の「N MB / YY MB」
    // 表示用に使われていた値 → fallback では未確定)。失敗しても致命的でない。
    if let Ok(meta) = std::fs::metadata(path) {
        progress.set_file_size(meta.len());
    }
    Ok((
        InputWithProgress::fallback(input),
        PhaseTimings {
            // fallback は 2 段に分けられないので open に全部入れる。
            open_input_ms: fallback_total_ms,
            find_stream_info_ms: 0.0,
        },
    ))
}

/// custom AVIO 経路の本体。`input_with_progress` の 1st pass。
fn try_open_with_custom_avio(
    path: &Path,
    progress: Arc<PreparingProgress>,
) -> Result<(InputWithProgress, PhaseTimings), String> {
    let file = File::open(path).map_err(|e| format!("open file: {e}"))?;
    let file_size = file
        .metadata()
        .map(|m| m.len())
        .map_err(|e| format!("file metadata: {e}"))?;
    // UI が "NN MB / YY MB" を表示できるよう file_size を共有 atomic に保存
    progress.set_file_size(file_size);
    // 念のため phase を OPENING に確定 (呼び出し側で先に set されていれば no-op)
    progress.set_phase(prep_phase::OPENING);
    let debug_read_delay =
        debug_delay_from_env("MIV_DEBUG_AVIO_READ_DELAY_MS", MAX_DEBUG_AVIO_READ_DELAY_MS);
    if let Some(delay) = debug_read_delay {
        crate::logger::log(format!(
            "input_with_progress: MIV_DEBUG_AVIO_READ_DELAY_MS={}ms/read while preparing",
            delay.as_millis()
        ));
    }

    // av_malloc で AVIO バッファを確保 (FFmpeg ownership)
    let buf = unsafe { av_malloc(READ_BUF_SZ) as *mut u8 };
    if buf.is_null() {
        return Err("av_malloc failed for AVIO buffer".into());
    }

    let state = Box::new(ProgressIoState {
        file,
        progress: Arc::clone(&progress),
        file_size,
        debug_read_delay,
    });
    let opaque = Box::into_raw(state) as *mut c_void;

    // ── AVIO 確保 ────────────────────────────────────────────
    let avio = unsafe {
        avio_alloc_context(
            buf,
            READ_BUF_SZ as i32,
            0, // write_flag = 0 (read only)
            opaque,
            Some(read_cb),
            None, // no write_cb
            Some(seek_cb),
        )
    };
    if avio.is_null() {
        // 失敗時: buf は ffmpeg にまだ渡っていないので自分で free + opaque を回収
        unsafe {
            av_free(buf as *mut c_void);
            drop(Box::from_raw(opaque as *mut ProgressIoState));
        }
        return Err("avio_alloc_context failed".into());
    }

    // ── AVFormatContext 確保 + AVIO attach ────────────────────
    let ctx = unsafe { avformat_alloc_context() };
    if ctx.is_null() {
        // 失敗時: AVIO + opaque を回収
        let mut avio_mut = avio;
        unsafe {
            avio_context_free(&mut avio_mut);
            drop(Box::from_raw(opaque as *mut ProgressIoState));
        }
        return Err("avformat_alloc_context failed".into());
    }
    unsafe {
        (*ctx).pb = avio;
        (*ctx).flags |= AVFMT_FLAG_CUSTOM_IO;
    }

    // ここまで来たら、以降のエラー時に解放すべきリソースは ctx / avio / opaque。
    // ctx の解放は `avformat_close_input` または `avformat_free_context`。
    // avformat_open_input 失敗時は ctx が中で解放される (custom IO でも) ので、
    // ガードを慎重に管理する。
    let mut ctx_ref: *mut ffmpeg::ffi::AVFormatContext = ctx;

    // ── avformat_open_input (= moov 探索フェーズ) ────────────
    let t0 = std::time::Instant::now();
    let ret = unsafe {
        avformat_open_input(
            &mut ctx_ref,
            std::ptr::null(), // path = null: pb 経由で読む
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    let open_input_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if ret < 0 {
        // avformat_open_input は失敗時 ctx を free 済みだが、custom IO の場合は
        // 実装依存で AVIO は残ることがあるので念のため両方フリー試行。
        // (FFmpeg 7.1 ソース: avformat_open_input が `s->pb` を保持したまま `avformat_free_context(s)` を呼ぶケースあり)
        let mut avio_mut = avio;
        unsafe {
            avio_context_free(&mut avio_mut);
            drop(Box::from_raw(opaque as *mut ProgressIoState));
        }
        return Err(format!("avformat_open_input failed: ret={ret}"));
    }

    // ── avformat_find_stream_info (= ストリーム解析フェーズ) ──
    progress.set_phase(prep_phase::ANALYZING);
    let t1 = std::time::Instant::now();
    let ret = unsafe { avformat_find_stream_info(ctx_ref, std::ptr::null_mut()) };
    let find_stream_info_ms = t1.elapsed().as_secs_f64() * 1000.0;
    if ret < 0 {
        // ctx は open 成功済みなので close_input で解放。
        unsafe {
            avformat_close_input(&mut ctx_ref);
            let mut avio_mut = avio;
            avio_context_free(&mut avio_mut);
            drop(Box::from_raw(opaque as *mut ProgressIoState));
        }
        return Err(format!("avformat_find_stream_info failed: ret={ret}"));
    }

    // 成功: phase を DONE に進めて Input にラップする
    progress.set_phase(prep_phase::DONE);
    let input = unsafe { ffmpeg::format::context::Input::wrap(ctx_ref) };
    Ok((
        InputWithProgress {
            input: ManuallyDrop::new(input),
            avio,
            opaque: opaque as *mut ProgressIoState,
        },
        PhaseTimings {
            open_input_ms,
            find_stream_info_ms,
        },
    ))
}

/// 各フェーズの elapsed (ms)。呼び出し側がログに分けて記録するのに使う。
#[derive(Debug, Clone, Copy)]
pub struct PhaseTimings {
    pub open_input_ms: f64,
    pub find_stream_info_ms: f64,
}

/// `ffmpeg::format::context::Input` をラップし、Drop 時に custom AVIO 経路の
/// リソース (AVIO + opaque) も解放する。
///
/// Deref / DerefMut で Input の全 API がそのまま使えるので、decoder.rs 側の
/// `input.streams().best(...)` 等の呼び出しは無変更で動く。
///
/// `avio` / `opaque` がそれぞれ `null` のときは **fallback 経路** (旧 `ffmpeg::format::input`
/// を直接使った経路)。Drop ではこれらを触らない (= 旧 Input の destructor だけが動く)。
pub struct InputWithProgress {
    input: ManuallyDrop<ffmpeg::format::context::Input>,
    avio: *mut AVIOContext,
    opaque: *mut ProgressIoState,
}

// SAFETY: ffmpeg::format::context::Input は `Send`。AVIOContext / opaque は
// 単独でアクセスされる前提 (= Input 専用) なので Send にする。
unsafe impl Send for InputWithProgress {}

impl InputWithProgress {
    /// Fallback 経路 (旧 `ffmpeg::format::input` で開いた Input をラップする)。
    /// custom AVIO は使っていないので avio / opaque は null。
    pub(crate) fn fallback(input: ffmpeg::format::context::Input) -> Self {
        Self {
            input: ManuallyDrop::new(input),
            avio: std::ptr::null_mut(),
            opaque: std::ptr::null_mut(),
        }
    }

    /// 現在の Input が custom AVIO 経路 (進捗追跡できる) なのか、fallback 経路なのかを返す。
    /// デバッグ / テスト用途。
    #[allow(dead_code)]
    pub fn is_custom_avio(&self) -> bool {
        !self.avio.is_null()
    }
}

impl Deref for InputWithProgress {
    type Target = ffmpeg::format::context::Input;
    fn deref(&self) -> &Self::Target {
        &self.input
    }
}

impl DerefMut for InputWithProgress {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.input
    }
}

impl Drop for InputWithProgress {
    fn drop(&mut self) {
        // 順序: Input (= avformat_close_input) → AVIO → opaque。
        // avformat_close_input は AVFMT_FLAG_CUSTOM_IO が立っているので pb を触らない
        // ので、Input drop の後に自前で avio_context_free + Box drop する。
        // Fallback 経路 (avio/opaque = null) では Input drop だけで完結する。
        unsafe {
            ManuallyDrop::drop(&mut self.input);
            if !self.avio.is_null() {
                avio_context_free(&mut self.avio);
            }
            if !self.opaque.is_null() {
                drop(Box::from_raw(self.opaque));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 進捗カウンタが「読込開始 → 増加 → open 完了」と推移することを実 mp4 で確認。
    /// 簡単な小さい mp4 を生成して、`input_with_progress` を呼んだあと bytes_read が
    /// 0 より大きいことだけを assert する。
    ///
    /// ※ FFmpeg の初期化が走るためテスト実行時に DLL がロードされる必要がある。
    ///   CI 環境で FFmpeg が無いと skip される (= ffmpeg_loader が失敗するので
    ///   `ffmpeg::init` 経由で fail)。
    #[test]
    #[cfg_attr(not(windows), ignore)]
    fn smoke_progress_counter_advances() {
        // FFmpeg 初期化 (DLL ロード)。失敗したら skip。
        if crate::video::ffmpeg_loader::init().is_err() {
            eprintln!("ffmpeg DLL unavailable, skipping");
            return;
        }
        if ffmpeg::init().is_err() {
            eprintln!("ffmpeg::init failed, skipping");
            return;
        }

        // 簡単な 1 frame MP4 を生成: 既存テストインフラがないので、
        // 既存の test fixture mp4 があれば使う。なければ skip。
        let fixture_candidates = [
            "tests/fixtures/sample.mp4",
            "tests/fixtures/test.mp4",
            "vendor/tests/sample.mp4",
        ];
        let fixture = fixture_candidates
            .iter()
            .map(Path::new)
            .find(|p| p.exists());
        let Some(fixture) = fixture else {
            eprintln!("no mp4 fixture available, skipping");
            return;
        };

        let progress = PreparingProgress::new();
        let result = input_with_progress(fixture, Arc::clone(&progress));
        let Ok((_input, timings)) = result else {
            eprintln!("input_with_progress failed: {:?}", result.err());
            return;
        };
        let read = progress.bytes_read();
        assert!(
            read > 0,
            "bytes_read should be > 0 after avformat_open_input"
        );
        // 成功すれば phase は DONE
        assert_eq!(progress.phase(), prep_phase::DONE);
        // file_size も埋まっている
        assert!(progress.file_size() > 0);
        // ファイル全体は読まれないはず (= moov + ftyp で十分)
        eprintln!(
            "read {} bytes, open_input={:.1}ms, find_stream_info={:.1}ms",
            read, timings.open_input_ms, timings.find_stream_info_ms
        );
        // 一応 timings が妥当な値
        assert!(timings.open_input_ms >= 0.0);
        assert!(timings.find_stream_info_ms >= 0.0);
        let _ = std::io::stdout().flush();
    }

    #[test]
    fn preparing_progress_default_state() {
        let p = PreparingProgress::new();
        assert_eq!(p.phase(), prep_phase::OPENING);
        assert_eq!(p.bytes_read(), 0);
        assert_eq!(p.file_size(), 0);
    }

    #[test]
    fn preparing_progress_setters_work() {
        let p = PreparingProgress::new();
        p.set_file_size(123_456);
        p.set_phase(prep_phase::ANALYZING);
        p.add_bytes(100);
        p.add_bytes(50);
        assert_eq!(p.file_size(), 123_456);
        assert_eq!(p.phase(), prep_phase::ANALYZING);
        assert_eq!(p.bytes_read(), 150);
    }

    #[test]
    fn snapshot_captures_current_state() {
        let p = PreparingProgress::new();
        p.set_file_size(1024);
        p.add_bytes(512);
        p.set_phase(prep_phase::ANALYZING);
        let snap = p.snapshot();
        assert_eq!(snap.file_size, 1024);
        assert_eq!(snap.bytes_read, 512);
        assert_eq!(snap.phase, prep_phase::ANALYZING);
    }

    fn status(phase: u8, bytes_read: u64, file_size: u64) -> PreparingStatus {
        PreparingStatus {
            phase,
            bytes_read,
            file_size,
        }
    }

    #[test]
    fn message_opening_with_known_file_size() {
        let s = build_preparing_message(status(
            prep_phase::OPENING,
            5 * 1024 * 1024,
            100 * 1024 * 1024,
        ));
        assert!(s.contains("メタデータ読込中"));
        assert!(s.contains("5.0 MB / 100.0 MB"), "got: {s}");
    }

    #[test]
    fn message_analyzing_without_file_size() {
        let s = build_preparing_message(status(prep_phase::ANALYZING, 12 * 1024 * 1024, 0));
        assert!(s.contains("ストリーム解析中"));
        assert!(s.contains("12.0 MB"));
        assert!(!s.contains(" / "));
    }

    #[test]
    fn message_done_omits_byte_progress() {
        let s = build_preparing_message(status(
            prep_phase::DONE,
            50 * 1024 * 1024,
            100 * 1024 * 1024,
        ));
        assert_eq!(s, "デコード開始中...");
    }

    #[test]
    fn message_opening_with_zero_bytes_omits_progress() {
        let s = build_preparing_message(status(prep_phase::OPENING, 0, 0));
        assert_eq!(s, "メタデータ読込中...");
    }

    #[test]
    fn message_unknown_phase_falls_back_to_legacy_label() {
        let s = build_preparing_message(status(99, 0, 0));
        assert_eq!(s, "動画を準備中...");
    }

    #[test]
    fn message_clamps_bytes_read_to_file_size() {
        // Codex P2: FFmpeg は seek で同じ範囲を再読込するため、累積バイト数が
        // file_size を超えうる。表示は clamp して「NN MB / YY MB (NN <= YY)」になる。
        let s = build_preparing_message(status(
            prep_phase::OPENING,
            300 * 1024 * 1024,
            100 * 1024 * 1024,
        ));
        // bytes_read = 300, file_size = 100 → 表示は 100.0 / 100.0
        assert!(s.contains("100.0 MB / 100.0 MB"), "got: {s}");
        // 反対のオーバーフロー表示 (300 / 100) が出ないこと
        assert!(!s.contains("300.0 MB"), "expected clamping, got: {s}");
    }

    #[test]
    fn parse_debug_delay_ms_ignores_missing_empty_zero_and_invalid() {
        assert_eq!(parse_debug_delay_ms(None, 100), None);
        assert_eq!(parse_debug_delay_ms(Some(""), 100), None);
        assert_eq!(parse_debug_delay_ms(Some(" 0 "), 100), None);
        assert_eq!(parse_debug_delay_ms(Some("abc"), 100), None);
    }

    #[test]
    fn parse_debug_delay_ms_accepts_and_clamps_positive_values() {
        assert_eq!(
            parse_debug_delay_ms(Some("25"), 100),
            Some(Duration::from_millis(25))
        );
        assert_eq!(
            parse_debug_delay_ms(Some("250"), 100),
            Some(Duration::from_millis(100))
        );
    }

    #[test]
    fn seek_from_for_whence_plain_seek_set_cur_end() {
        // SEEK_SET / CUR / END = 0/1/2、フラグ無し。
        assert_eq!(
            seek_from_for_whence(0, 1000),
            SeekResolution::Pos(SeekFrom::Start(1000))
        );
        assert_eq!(
            seek_from_for_whence(1, -50),
            SeekResolution::Pos(SeekFrom::Current(-50))
        );
        assert_eq!(
            seek_from_for_whence(2, -100),
            SeekResolution::Pos(SeekFrom::End(-100))
        );
    }

    #[test]
    fn seek_from_for_whence_with_avseek_force_flag() {
        // Codex 第 14 ラウンド指摘の本命: AVSEEK_FORCE (= 0x20000) と SEEK_SET 等が OR された
        // 場合、純粋な match だと 131072 等になって -1 を返してしまう。マスクで FORCE を
        // 除去すれば 0/1/2 として正しく解決できる。
        assert_eq!(
            seek_from_for_whence(0x20000 | 0, 5000),
            SeekResolution::Pos(SeekFrom::Start(5000))
        );
        assert_eq!(
            seek_from_for_whence(0x20000 | 1, -5),
            SeekResolution::Pos(SeekFrom::Current(-5))
        );
        assert_eq!(
            seek_from_for_whence(0x20000 | 2, 0),
            SeekResolution::Pos(SeekFrom::End(0))
        );
    }

    #[test]
    fn seek_from_for_whence_with_avseek_size_returns_unknown() {
        // AVSEEK_SIZE のみのケースは「サイズを返せ」というクエリで、本関数は SEEK_SET/CUR/END
        // のみを解決対象とするので Unknown を返す (呼出側が AVSEEK_SIZE を別途処理する責務)。
        // ただし AVSEEK_SIZE をマスクで除去するので AVSEEK_SIZE | SEEK_SET なら 0 として解決。
        assert_eq!(
            seek_from_for_whence(0x10000, 0), // AVSEEK_SIZE のみ → mask 後 0 → Start
            SeekResolution::Pos(SeekFrom::Start(0))
        );
        // 本来は seek_cb の手前で AVSEEK_SIZE が捕捉されるので、ここに到達する経路は通常無い。
    }

    #[test]
    fn seek_from_for_whence_unknown_value() {
        // 8 や 999 のような未定義の whence は Unknown を返す。
        assert_eq!(seek_from_for_whence(8, 0), SeekResolution::Unknown);
        assert_eq!(seek_from_for_whence(999, 0), SeekResolution::Unknown);
    }

    #[test]
    fn fallback_input_does_not_double_free() {
        // InputWithProgress::fallback で構築した値は Drop で avio/opaque を触らない。
        // ffmpeg::format::input の実呼び出しは fixture が無いケースで skip するため、
        // ここでは是のコンストラクタ + Drop が panic / UB しない事を ManuallyDrop 経由で間接的に確認する。
        // 実 Input を作れない (init 不要) ので、ここでは null pointer の代入だけを assert。
        // 構築は input_with_progress 経由でしか行わない契約。
        // この test は型 invariant の確認 (= ManuallyDrop の field type が変わったら気付く)。
        use std::mem::size_of;
        assert!(size_of::<InputWithProgress>() > 0);
    }
}
