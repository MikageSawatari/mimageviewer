//! swscale (CPU 経路) の入力フレーム前処理ヘルパ。
//!
//! FFmpeg の `sws_scale` / `sws_init_context` は **HW pix_fmt (D3D11) や None** を
//! 入力に渡すと内部 `av_assert0` で `abort()` を呼び、`ucrtbase!abort` 経由で
//! プロセスごと落ちる (`0xc0000409 / FAST_FAIL_FATAL_APP_EXIT`)。2026-05-12 に
//! `src/video/thumbnail.rs` (= grid 表示用ホバーサムネ worker) でこのクラッシュが
//! 観測された。
//!
//! 単純に「HW frame を skip」すると、HW accel が auto-attach されたコーデック
//! (H.264 / HEVC / AV1) では **サムネが完全に取れなくなる** ので性能的に受け入れ難い。
//! 正しい解は **HW decode を維持しつつ `av_hwframe_transfer_data` で CPU メモリへ
//! download してから swscale に渡す**。download コストは frame size に比例する
//! 程度で、HW decode 自体の速度はそのまま活かせる。
//!
//! 本 module はこの download パスを 1 箇所にまとめる: `thumbnail.rs` /
//! `tile_thumbnails.rs` / `screenshot.rs` と mIV Remote の decoder-side
//! `stream/video_tap.rs` で共有する。`decoder.rs` の PC 表示用 CPU fallback は固有の
//! deinterlace / `scaler_key` lifecycle があるため独自実装のまま。

use ffmpeg::format::Pixel;
use ffmpeg::util::frame::video::Video;
use ffmpeg_the_third as ffmpeg;

/// `frame` が `Pixel::D3D11` (= HW surface) のとき、CPU メモリへ download した
/// SW frame を `sw_holder` に格納し、それへの参照を返す。
/// `frame` が既に SW なら、そのまま `frame` への参照を返す。
///
/// 使い方:
/// ```ignore
/// let mut sw_holder: Option<Video> = None;
/// let frame_for_scaler = match prepare_frame_for_swscale(&frame, &mut sw_holder) {
///     Ok(f) => f,
///     Err(e) => { logger::log(e); continue; }
/// };
/// scaler.run(frame_for_scaler, &mut rgba)?;
/// ```
///
/// **戻り値の lifetime**: `sw_holder` を borrow するので、`sw_holder` が
/// scope 内で生きている間だけ有効。`scaler.run` を呼んだ後に `drop(sw_holder)`
/// する必要は無い (= scope 終わりで自動 drop)。
///
/// `Pixel::None` を入力に渡すのも av_assert0 を踏むので、Err として弾く
/// (= 呼出側で skip させる)。
pub fn prepare_frame_for_swscale<'a>(
    frame: &'a Video,
    sw_holder: &'a mut Option<Video>,
) -> Result<&'a Video, String> {
    let fmt = frame.format();
    if matches!(fmt, Pixel::None) {
        return Err(format!(
            "prepare_frame_for_swscale: unsupported input pix_fmt {fmt:?} (would crash swscale av_assert0)"
        ));
    }
    if !matches!(fmt, Pixel::D3D11) {
        // SW frame: そのまま返す
        return Ok(frame);
    }
    // HW (D3D11) frame: av_hwframe_transfer_data で CPU メモリへ download
    let mut sw = Video::empty();
    unsafe {
        let ret = ffmpeg::ffi::av_hwframe_transfer_data(sw.as_mut_ptr(), frame.as_ptr(), 0);
        if ret < 0 {
            return Err(format!("av_hwframe_transfer_data failed: ret={ret}"));
        }
    }
    let sw_fmt = sw.format();
    if matches!(sw_fmt, Pixel::D3D11 | Pixel::None) {
        return Err(format!(
            "av_hwframe_transfer_data produced unsupported output pix_fmt: {sw_fmt:?}"
        ));
    }
    *sw_holder = Some(sw);
    Ok(sw_holder.as_ref().expect("just stored Some"))
}
