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
use std::ptr;
use std::sync::Arc;

use ffmpeg_the_third::ffi::{
    AVBufferRef, AVHWDeviceContext, AVHWDeviceType, av_buffer_ref, av_buffer_unref,
    av_hwdevice_ctx_alloc, av_hwdevice_ctx_init,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11DeviceContext, ID3D11VideoContext, ID3D11VideoDevice,
};
use windows::core::Interface;

use super::d3d11_device::{GpuVideoDevice, GpuVideoError};

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
    let buf = unsafe {
        av_hwdevice_ctx_alloc(AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA)
    };
    if buf.is_null() {
        return Err(GpuVideoError::DeviceCreate(
            "av_hwdevice_ctx_alloc returned null".into(),
        ));
    }

    // 1. すべての COM ポインタを **AddRef 前に** 取得する (Codex P3 反映)。
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
        (*d3d11_ctx).lock = None;
        (*d3d11_ctx).unlock = None;
        (*d3d11_ctx).lock_ctx = ptr::null_mut();
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
