//! FFmpeg の `AVCodecContext.hw_device_ctx` を mIV 側で作成した D3D11 デバイスに
//! 紐付ける。これにより HW デコーダが mIV の D3D11 デバイス上に NV12 テクスチャを
//! 生成し、同じデバイス内の `ID3D11VideoProcessor` で blit 可能になる
//! (= GPU→CPU 転送なし)。
//!
//! ## なぜ手動で組むか
//! FFmpeg の `av_hwdevice_ctx_create(D3D11VA, NULL, NULL, 0)` は内部で **新しい**
//! D3D11 デバイスを作ってしまう。それでは mIV 側の `GpuVideoDevice` と別デバイスに
//! なり、テクスチャを共有できない。代わりに `av_hwdevice_ctx_alloc` で空の
//! `AVHWDeviceContext` を作り、`hwctx` を `AVD3D11VADeviceContext` にキャストして
//! mIV のデバイス/コンテキストを書き込み、`av_hwdevice_ctx_init` で確定させる。
//!
//! ## AVD3D11VADeviceContext の binding
//! `libavutil/hwcontext_d3d11va.h` のこの構造体は ffmpeg-sys-the-third の bindgen が
//! 拾わないので、本ファイルで **FFmpeg 7.x ABI に合わせて手書き** する。レイアウト
//! ずれは crash を生むので、フィールド順序とサイズは FFmpeg upstream を参照。

use std::os::raw::c_void;
use std::sync::Arc;

use ffmpeg_the_third::ffi::{
    AVBufferRef, AVHWDeviceContext, AVHWDeviceType, av_buffer_ref, av_buffer_unref,
    av_hwdevice_ctx_alloc, av_hwdevice_ctx_init,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11DeviceContext, ID3D11VideoContext, ID3D11VideoDevice,
};
use windows::Win32::System::Threading::{
    CRITICAL_SECTION, EnterCriticalSection, LeaveCriticalSection, TryEnterCriticalSection,
};
use windows::core::Interface;

use super::d3d11_device::{GpuVideoDevice, GpuVideoError};

/// D3D11VA lock/unlock callback の呼出回数 (= API レベル直列化が機能している証拠の
/// 取得用、2026-05-15 追加)。perf log に詳細イベントを出すと頻度が高すぎるので、
/// **contention が観測されたときだけ** perf event を出す (= `TryAcquire` 失敗時)。
/// 集計値は perf log でなく `state` イベントで定期的に dump する用途を想定。
///
/// `lock_calls`: FFmpeg からの lock callback 呼出総数 (uncontended 含む)
/// `lock_contended`: 上記のうち `TryAcquire` が即時取得に失敗した回数
/// `lock_total_wait_ns`: 競合時の cumulative wait time (nanoseconds)
pub static D3D11VA_LOCK_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static D3D11VA_LOCK_CONTENDED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static D3D11VA_LOCK_TOTAL_WAIT_NS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// FFmpeg D3D11VA の `lock` callback。`lock_ctx` は `GpuVideoDevice::d3d_lock_ptr()`
/// (= `*mut CRITICAL_SECTION`)。FFmpeg は内部で `ID3D11DeviceContext` /
/// `ID3D11VideoContext` を触る前にこれを、触り終えたら `unlock` を呼ぶ。
/// `blit_nv12_to_rgba` 側も同じ CS を握るので、FFmpeg decode と blit の context 操作が
/// 直列化される。
///
/// **CRITICAL_SECTION を使う理由** (Codex P1 2026-05-16): FFmpeg の
/// `hwcontext_d3d11va.h:91` が「lock must be recursive」と明示している。SRWLOCK は
/// 再入不可なので、FFmpeg が lock 内で再帰的に lock を取った瞬間 self-deadlock する。
/// CS は同一スレッドからの再 Enter を owner-thread counter で許容する。
///
/// **検証用計装** (2026-05-15): まず `TryEnterCriticalSection` で即時取得を試み、
/// 失敗したら本当に競合した時間を計測する。健全な状態 (= 直列化が機能している)
/// では `lock_contended` は限りなく 0 に近いはず。stuck 調査時は perf log の
/// `d3d11va_lock_contended` イベントの頻度と wait time を見る。
///
/// SAFETY: `lock_ctx` は `GpuVideoDevice::d3d_lock` (= `CRITICAL_SECTION`) の生ポインタ。
/// `GpuVideoDevice` は本 hw_device_ctx より長生きする (`Arc`、アプリ全体で 1 個)。
unsafe extern "C" fn d3d11va_lock(lock_ctx: *mut c_void) {
    if lock_ctx.is_null() {
        return;
    }
    D3D11VA_LOCK_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // 高頻度 callback なので perf overhead を避けるため、まず `TryEnter` で即時
    // 取得を試みる。`TryEnterCriticalSection` は競合時に FALSE で返る (= block しない)。
    // FALSE 時のみ実 wait を計測して perf log に出す。
    let acquired = unsafe { TryEnterCriticalSection(lock_ctx as *mut CRITICAL_SECTION) };
    if acquired.as_bool() {
        return;
    }
    D3D11VA_LOCK_CONTENDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let t0 = std::time::Instant::now();
    unsafe { EnterCriticalSection(lock_ctx as *mut CRITICAL_SECTION) };
    let wait_ns = t0.elapsed().as_nanos() as u64;
    D3D11VA_LOCK_TOTAL_WAIT_NS.fetch_add(wait_ns, std::sync::atomic::Ordering::Relaxed);
    // 5ms 超の競合のみ perf event に上げる (= 短い競合は集計値で十分)。
    let wait_ms = (wait_ns as f64) / 1_000_000.0;
    if wait_ms > 5.0 && crate::perf::is_enabled() {
        crate::perf::event(
            "video_decode",
            "d3d11va_lock_contended",
            None,
            0,
            &[
                ("wait_ms", serde_json::Value::from(wait_ms)),
                (
                    "lock_calls_total",
                    serde_json::Value::from(
                        D3D11VA_LOCK_CALLS.load(std::sync::atomic::Ordering::Relaxed) as i64,
                    ),
                ),
                (
                    "lock_contended_total",
                    serde_json::Value::from(
                        D3D11VA_LOCK_CONTENDED.load(std::sync::atomic::Ordering::Relaxed) as i64,
                    ),
                ),
                (
                    "live_decoders",
                    serde_json::Value::from(
                        crate::video::decoder::LIVE_VIDEO_DECODE_THREADS
                            .load(std::sync::atomic::Ordering::Acquire)
                            as i64,
                    ),
                ),
            ],
        );
    }
}

/// FFmpeg D3D11VA の `unlock` callback。`d3d11va_lock` の対。CS の owner-thread counter
/// を 1 decrement する。counter が 0 になった時点で他スレッドが Enter できる。
///
/// SAFETY: `d3d11va_lock` と同じ。
unsafe extern "C" fn d3d11va_unlock(lock_ctx: *mut c_void) {
    if lock_ctx.is_null() {
        return;
    }
    unsafe { LeaveCriticalSection(lock_ctx as *mut CRITICAL_SECTION) };
}

/// `libavutil/hwcontext_d3d11va.h` の `AVD3D11VADeviceContext` を手動で再現。
/// FFmpeg 7.x で観測されているレイアウト。
///
/// **注意**: FFmpeg メジャーバージョンが上がってメンバが増えると ABI が壊れる。
/// `vendor/ffmpeg/VERSION` を上げる際に必ずソースを確認すること。
#[repr(C)]
#[allow(dead_code, non_camel_case_types)]
struct AVD3D11VADeviceContext {
    /// `ID3D11Device*`。
    device: *mut c_void,
    /// `ID3D11DeviceContext*`。
    device_context: *mut c_void,
    /// `ID3D11VideoDevice*`。
    video_device: *mut c_void,
    /// `ID3D11VideoContext*`。
    video_context: *mut c_void,
    /// ロック/アンロックコールバック。複数スレッドが同一 device_context を呼ぶ
    /// 競合をシリアライズするために FFmpeg が呼ぶ。
    lock: Option<unsafe extern "C" fn(lock_ctx: *mut c_void)>,
    unlock: Option<unsafe extern "C" fn(lock_ctx: *mut c_void)>,
    lock_ctx: *mut c_void,
}

/// `GpuVideoDevice` を共有する `AVBufferRef*` (= AVHWDeviceContext) を作る。
///
/// 戻り値の `AVBufferRef*` は **caller 所有** (FFmpeg 慣習)。
/// `AVCodecContext.hw_device_ctx` には別途 `av_buffer_ref` した値を渡し、本値は
/// `av_buffer_unref` で解放する責務がある。
///
/// SAFETY: `gpu_dev` の D3D11 デバイス/コンテキストが `Arc<GpuVideoDevice>` 経由で
/// 寿命管理されており、本関数が返す `AVBufferRef*` を unref するまで生存している
/// ことを保証する必要がある。FFmpeg 側は AVHWDeviceContext の `free` で
/// Release を呼ぶため、`Arc` で保持してから AVHWDeviceContext を作るとよい。
pub unsafe fn create_ffmpeg_hw_device_ctx(
    gpu_dev: &Arc<GpuVideoDevice>,
) -> Result<*mut AVBufferRef, GpuVideoError> {
    let buf = unsafe { av_hwdevice_ctx_alloc(AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA) };
    if buf.is_null() {
        return Err(GpuVideoError::DeviceCreate(
            "av_hwdevice_ctx_alloc returned null".into(),
        ));
    }

    // 1. すべての COM ポインタを **AddRef 前に** 取得する。
    //    キャストや GetImmediateContext の失敗時に AddRef したまま手放してしまうリーク
    //    を避ける。全ポインタが揃ってから一括 AddRef + d3d11_ctx に書き込む。
    let imm_ctx: ID3D11DeviceContext = unsafe { gpu_dev.raw_device().GetImmediateContext() }
        .map_err(|e| {
            let mut buf_to_free = buf;
            unsafe { av_buffer_unref(&mut buf_to_free) };
            GpuVideoError::DeviceCreate(format!("GetImmediateContext failed: {e:?}"))
        })?;
    let video_dev: ID3D11VideoDevice = gpu_dev.raw_device().cast().map_err(|_| {
        let mut buf_to_free = buf;
        unsafe { av_buffer_unref(&mut buf_to_free) };
        GpuVideoError::NoVideoDevice
    })?;
    let video_ctx: ID3D11VideoContext = imm_ctx.cast().map_err(|_| {
        let mut buf_to_free = buf;
        unsafe { av_buffer_unref(&mut buf_to_free) };
        GpuVideoError::NoVideoDevice
    })?;
    let device_ptr = gpu_dev.raw_device().as_raw();
    let context_ptr = imm_ctx.as_raw();
    let video_dev_ptr = video_dev.as_raw();
    let video_ctx_ptr = video_ctx.as_raw();

    // 2. AVHWDeviceContext.hwctx を埋め、各ポインタを AddRef して FFmpeg 側に所有
    //    させる (FFmpeg は av_buffer_unref → AVHWDeviceContext::free で Release する)。
    //    すべてのポインタを書き込んでから AddRef するので、ここから先に失敗があっても
    //    av_buffer_unref で自動的に Release される (= リークなし)。
    unsafe {
        let hw_ctx = (*buf).data as *mut AVHWDeviceContext;
        let d3d11_ctx = (*hw_ctx).hwctx as *mut AVD3D11VADeviceContext;
        (*d3d11_ctx).device = device_ptr;
        (*d3d11_ctx).device_context = context_ptr;
        (*d3d11_ctx).video_device = video_dev_ptr;
        (*d3d11_ctx).video_context = video_ctx_ptr;
        // lock/unlock callback で `GpuVideoDevice::d3d_lock` (CRITICAL_SECTION、recursive)
        // を握らせる。これにより FFmpeg の D3D11VA decode (`avcodec_send_packet` 等が内部で
        // device context を触る) と mIV 側 `blit_nv12_to_rgba` の context 操作が同じ CS で
        // 直列化され、fast-swap 連射時の driver hard-stuck を防ぐ。
        (*d3d11_ctx).lock = Some(d3d11va_lock);
        (*d3d11_ctx).unlock = Some(d3d11va_unlock);
        (*d3d11_ctx).lock_ctx = gpu_dev.d3d_lock_ptr() as *mut c_void;
        addref_com(device_ptr);
        addref_com(context_ptr);
        addref_com(video_dev_ptr);
        addref_com(video_ctx_ptr);
    }

    // 2. 確定。失敗時は buf を release。
    let ret = unsafe { av_hwdevice_ctx_init(buf) };
    if ret < 0 {
        let mut buf_to_free = buf;
        unsafe { av_buffer_unref(&mut buf_to_free) };
        return Err(GpuVideoError::DeviceCreate(format!(
            "av_hwdevice_ctx_init failed: {ret}"
        )));
    }

    Ok(buf)
}

/// `AVCodecContext.hw_device_ctx` に **別の参照を作って** 渡すヘルパ。
/// caller が持っている `AVBufferRef*` は別途自前で unref する責務がある。
///
/// SAFETY: `src` が valid な AVBufferRef ポインタであること。
#[allow(dead_code)]
pub unsafe fn ref_for_codec(src: *mut AVBufferRef) -> *mut AVBufferRef {
    unsafe { av_buffer_ref(src) }
}

/// COM の AddRef を呼ぶ。`windows::core::Interface::as_raw()` は ref を増やさないため、
/// 所有権を別の C 側に渡すときは明示的に AddRef する必要がある。
///
/// SAFETY: `ptr` が `IUnknown` (= 全 COM インタフェースの基底) として有効なポインタで
/// あること。
unsafe fn addref_com(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    // IUnknown の vtable オフセット 0 = QueryInterface, 1 = AddRef, 2 = Release。
    // 関数ポインタの型: extern "system" fn(this: *mut c_void) -> u32
    type AddRefFn = unsafe extern "system" fn(this: *mut c_void) -> u32;
    unsafe {
        let vtable = *(ptr as *mut *mut *const c_void);
        let add_ref: AddRefFn = std::mem::transmute(*vtable.add(1));
        let _ = add_ref(ptr);
    }
}
